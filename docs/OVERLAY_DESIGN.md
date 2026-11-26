# Game overlay (v1, Windows): implementer's spec

The in-game overlay works like Discord's current one: **out of process**. Rustcord opens its own
small windows on top of the game: borderless, transparent, always on top, click-through, never
focused, not in the taskbar or Alt+Tab. Rustcord never injects code, never hooks
DirectX/Vulkan/OpenGL, and never opens the game process beyond
`PROCESS_QUERY_LIMITED_INFORMATION`, which game detection already uses for the exe path. It
never reads game memory. It uses only user32 queries on the game's window: foreground, client
rect and monitor. This works over windowed and borderless-fullscreen games. Exclusive fullscreen
bypasses the compositor, so the overlay doesn't show there; the settings text says so.

v1 scope:
- the voice widget
- text notifications
- the "Game overlay" settings page

Out of v1: interactive overlay chat, a hotkey, an opacity setting, a preview button and
Linux/macOS. On Linux and macOS the page is hidden and the module compiles to a no-op stub.

## 1. Behaviour

- **Shown** only while all of these hold:
  - the overlay is enabled
  - the foreground window belongs to a game, by the same rule as `activity::scan::pick`: an
    added game (Registered games) by full path, else the detectable-games table. A process that
    won't tell its path counts by its file name alone.
  - that pane has something to show
- **Hidden** at once when anything else comes to the front, including Rustcord itself (its pid
  is never a game). A hidden pane's window is destroyed and created again the next time; that
  takes 10–20 ms on the UI thread.
- **Voice widget** (pane `voice`): shown while `state.voice.channel_id` is Some (joining or
  connected; guild voice and DM calls alike) and its corner is not Off.
  - Rows are everyone in my call, me included even before my voice state echoes, sorted by
    lowercase name, then id.
  - Each row has the avatar, a speaking ring, a mute or deafen mark, and a name pill.
  - Options:
    - show names: always, while speaking, never
    - show users: always, while speaking
    - avatar size: small or large
  - At most 10 rows. With "always" and more than 10 people, an 11th row says "N more".
- **Notifications** (pane `toast`): exactly the notifications that would go to the desktop now.
  The existing DND check, the OnePingPerDM plugin and `Settings::notification` come first, with
  Streamer Mode's masking. While a game is in front and the notification corner is not Off, the
  overlay shows the toast **instead of** the desktop notification; otherwise the desktop one
  shows as before.
  - At most 3 cards; each hides after 5 s.
  - The newest card is nearest the corner edge.
  - Taskbar flashing is unchanged.
- **Placement**: corners of the game window's client area, intersected with its monitor, with a
  16 pt margin. In borderless fullscreen that is the monitor's corner. If the voice widget and
  the notifications share a corner, the toast stack starts 8 pt past the voice widget.
- Streamer Mode's "Hide window from screen capture" also hides the overlay windows from capture
  (`WDA_EXCLUDEFROMCAPTURE`).

## 2. Mechanism (verified)

Each pane is an **eframe deferred viewport** (`ctx.show_viewport_deferred`) on the existing glow
context.
- It shares fonts, theme and textures (avatars from `ImageCache`).
- The overlay repaints on its own; the root UI does not run for it.
- Immediate viewports would re-run the whole root UI on every overlay repaint, so they are not
  used.

A throwaway probe confirmed every point below on this machine. It is in the scratch dir
`fix2\design-overlay\probe`: eframe 0.37.0 glow, winit 0.30.13, Windows 11, 125 % DPI, debug
build, pixels read back from the composited screen.

| Finding | Consequence |
|---|---|
| Builder flags that work: `with_transparent(true)`, `with_decorations(false)`, `with_window_level(AlwaysOnTop)`, `with_mouse_passthrough(true)`, `with_taskbar(false)`, `with_active(false)`, `with_visible(false)`, `with_drag_and_drop(false)` | Use exactly these. The root's GL config already has 8 alpha bits, and eframe 0.37 creates transparent child windows on Windows without a transparency check, so the root stays non-transparent. |
| `with_mouse_passthrough` makes winit add `WS_EX_LAYERED \| WS_EX_TRANSPARENT`. With no `SetLayeredWindowAttributes` the window **shows nothing**: every probe pixel was the window behind it. | Call `SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA)` once per HWND. After that, per-pixel alpha is exact: black @170 over magenta gave (85,0,85), opaque content stayed opaque, and empty pixels showed what is behind. |
| Click-through: `WindowFromPoint` over transparent and over opaque overlay pixels both returned the window below. | No hit-test work needed. |
| `decorations(false)` makes egui-winit request winit's undecorated shadow: a DWM drop shadow around the box, a grey 1 px line along the top and the Win11 border. | `SetWindowRgn` set to the **client rect** (in window coordinates) removes all three; every edge pixel then equals the background. Re-apply on every resize and on **every new HWND**. Stripping `WS_CAPTION` instead leaves a white 1 px row, because winit's `WM_NCCALCSIZE` keeps top += 1. |
| winit keeps `WS_EX_APPWINDOW` and rewrites `GWL_EXSTYLE` on every flag change (show, hit test) | Re-add `WS_EX_TOOLWINDOW \| WS_EX_NOACTIVATE` and clear `WS_EX_APPWINDOW` in every overlay pass where they are missing. With them in place, the first show (`SW_SHOWNOACTIVATE`) and a later hide/show (`SW_SHOW`) never took the foreground. |
| Creation: the first overlay pass ran 10–19 ms after the declaring pass, and private memory rose by 0.1 MB | Destroying a hidden pane is cheap |
| Idle: 0 overlay passes in 2 s while the root ticked at 10 Hz; root re-declaring an unchanged builder repaints nothing | 0 % idle holds |
| Root minimized (logic-only passes): a declared viewport is **never created**, because only a full root pass outputs viewports. Sending `ViewportCommand::RequestPaintWhileHidden` to ROOT once fixed it, and the overlay showed over other apps. | Send it while the pane's window doesn't exist yet |
| While any child viewport exists, eframe runs full root UI passes (unpainted) even when the root is minimized or in the tray: 7 in 1 s at 10 Hz wakes. A *hidden* child counts as visible (winit reports no occlusion on Windows). | Destroy hidden panes instead of hiding them. The remaining cost (root UI passes per event while minimized and an overlay shows) equals today's cost when the root merely sits behind the game. |

Two consequences of multiple viewports need small fixes outside the module (§7.1, §7.2):
1. `ctx.request_repaint*()` without an id targets the **current** viewport, which is
   `viewport_stack.last()` read through the shared context. A backend thread waking the UI while
   an overlay pass runs would wake the overlay instead of the root. The event then waits in the
   channel until something else wakes the root, which can delay a message or notification.
2. `App::clear_color` is global; its default (12,12,12,180) would fill the overlay.

Implementation notes (what differed from the first design):
- `main.rs` creates the egui context before eframe, and such a context has `embed_viewports`
  true: viewports then became embedded egui windows and no window existed. `main.rs` calls
  `set_embed_viewports(false)` on Windows.
- A null foreground window (the one in front closed and nothing took over) is a "no game"
  foreground, so the panes go. If nothing at all takes over, no event arrives and the panes stay
  until the next foreground change or root pass.
- `Notification.author_id` is `Option<Id>` (None for a ringing call).
- `ui::overlay::demo_active()`: a demo screenshot run lets the overlay close the window after the
  panes' own screenshots (`SHOTS_PENDING`), with a 20 s cap.
- `choice_row` takes an `id` to tell rows with the same label apart. A "N more" row is not drawn
  with names Never (no room).
- Measured (debug, this machine, demo with both panes shown): idle CPU 0 ms over 10 s, working set
  +1 MB against no demo. Screen capture is not available to the test session, so the composited
  look on a real desktop is only checked through the panes' own screenshots and window styles.

## 3. Files

| File | Content |
|---|---|
| `src/ui/overlay/mod.rs` | Module doc. `#[cfg(windows)] mod live; mod paint; mod win32;` and `pub use live::Overlay`. `#[cfg(not(windows))]`: a no-op `#[derive(Default)] pub struct Overlay;` with the same two methods (`logic` does nothing, `notify` returns false) and `_`-named args. |
| `src/ui/overlay/live.rs` | `Overlay`, `Pane`, the scenes, `logic`, `notify`, the pure helpers and their unit tests |
| `src/ui/overlay/win32.rs` | foreground watcher thread, window styling and placement, game area |
| `src/ui/overlay/paint.rs` | the deferred-viewport callback: placement handshake, drawing, demo screenshot |
| `src/ui/user_settings/overlay.rs` | the settings page (`#[cfg(windows)]`) |

Add to ARCHITECTURE.md's ownership table this row: "overlay: `src/ui/overlay/*`,
`src/ui/user_settings/overlay.rs`: see `docs/OVERLAY_DESIGN.md`".

## 4. Settings (`src/config.rs`, integrator; additive)

```rust
/// Settings > Game overlay (Windows, ui/overlay): Rustcord's own windows over the game, never inside it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlaySettings {
    pub enabled: bool,                    // true
    pub voice_corner: Corner,             // TopLeft
    pub notification_corner: Corner,      // BottomRight
    pub show_names: Show,                 // Always
    pub show_users: Show,                 // Always (Never is not offered; treated as Always)
    pub large_avatars: bool,              // true
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Corner { TopLeft, TopRight, BottomLeft, BottomRight, Off }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Show { #[default] Always, Speaking, Never }
```

`Settings` gets `pub overlay: OverlaySettings` (default as above). In TOML it is the `[overlay]`
table with `voice_corner = "top_left"` and so on. The settings file must stay loadable on
Linux/macOS: these types are data only. Keep helpers like labels in the Windows-only page.

## 5. The module

Constants (points unless noted):

| Constant | Value |
|---|---|
| `MARGIN` | 16 |
| `PAD` | 4 |
| `RING` | 3 (room for the speaking ring) |
| `NAME_MAX` | 160 (pill width, padding included) |
| `MAX_ROWS` | 10 |
| `MIN_GAP` | 100 ms (fastest overlay repaint) |
| `TOAST_FOR` | 5 s |
| `MAX_TOASTS` | 3 |
| `TOAST_W` × `TOAST_H` | 320 × 80 |
| `GAP` | 8 |
| small: avatar `A` / row / pill height / font | 24 / 32 / 20 / 13 |
| large: avatar `A` / row / pill height / font | 32 / 40 / 24 / 14 |

Viewport ids: `ViewportId::from_hash_of("rc_overlay_voice")` and `("rc_overlay_toast")`. Window
titles: "Rustcord voice overlay" and "Rustcord notification overlay". The titles are how
`win32::own_window` finds the window; never "Discord".

### 5.1 `live.rs`

```rust
#[derive(Default)] pub struct Overlay { voice: Pane, toast: Pane, toasts: Vec<Toast>, demo_toast: bool }
struct Toast { title: String, body: String, author: Option<Id>, until: Instant }
/// Some while declared: the builder (constant for the window's life) and a fresh state per window.
#[derive(Default)] struct Pane { live: Option<(egui::ViewportBuilder, Arc<Mutex<PaneState>>)> }
pub(super) struct PaneState { pub want: Want, pub tries: u8, pub shown: bool, pub styled: Option<(isize, bool)>,
                              pub last_paint: Option<Instant>, pub created: Instant, pub shot: Option<PathBuf>, pub title: &'static str }
#[derive(Clone, PartialEq)] pub(super) struct Want { pub scene: Scene, pub colors: Colors, pub area: [i32; 4] /*phys x,y,w,h*/,
    pub size: Vec2 /*pt*/, pub corner: Corner, pub offset: f32 /*pt, away from the corner edge*/, pub hide_from_capture: bool }
#[derive(Clone, PartialEq)] pub(super) enum Scene { Voice(VoiceScene), Toasts(Vec<ToastRow>) }
#[derive(Clone, PartialEq)] pub(super) struct VoiceScene { pub rows: Vec<Row>, pub names: Show, pub users: Show, pub large: bool }
#[derive(Clone, PartialEq)] pub(super) struct Row { pub id: Id, pub name: String, pub tex: Option<TextureId>,
    pub speaking: bool, pub muted: bool, pub deaf: bool, pub by_server: bool }
#[derive(Clone, PartialEq)] pub(super) struct ToastRow { pub title: String, pub body: String, pub tex: Option<TextureId>, pub seed: u64 }
#[derive(Clone, Copy, PartialEq)] pub(super) struct Colors { text, muted, card, rule, hc: bool, positive, danger, on_media, on_media_muted, pill, card_r: u8 }
```

`PaneState` stores HWNDs as `isize` (HWND is not `Send`). Lock with
`lock().unwrap_or_else(|e| e.into_inner())`.

`Colors::new(theme)` is built from `theme.tokens()` and `theme.colors`. In the drawing code
below, `pal.x` means `want.colors.x`.

| Field | Value |
|---|---|
| `text` | `colors.text` |
| `muted` | `text_muted` |
| `card` | `tk.raised` |
| `rule` | `tk.rule` |
| `hc` | `is_high_contrast` |
| `positive` | `tk.positive` |
| `danger` | `tk.danger_fill` |
| `on_media` | `tk.on_media` |
| `on_media_muted` | `tk.on_media_muted` |
| `pill` | `Color32::from_black_alpha(170)`, as on call tiles |
| `card_r` | `1.5 × layout.corner_radius` (container tier) |

**`pub fn logic(&mut self, ctx, state: &AppState, view: &ViewState, settings: &Settings, theme: &Theme, images: &mut ImageCache)`**,
every root pass, including logic-only passes:
1. `demo = demo()`, read once (`OnceLock`) from `RUSTCORD_OVERLAY_DEMO`: None, `Always` (any
   value except `gated`) or `Gated`.
   - If `!(settings.overlay.enabled || demo.is_some())`, or a `RUSTCORD_SCREENSHOT` run without
     a demo: close both panes and return.
   - Otherwise call `win32::watch(ctx)`. It starts once; screenshot runs and disabled overlays
     never start the thread.
2. `area: Option<[i32;4]>`:
   - `Always`: the root window as the fake game, so nothing private is under it:
     `ctx.input(|i| i.viewport().inner_rect)` × `ctx.pixels_per_point()`, rounded.
   - Otherwise: `win32::foreground().filter(|f| is_game(f, table, added)).and_then(|f| win32::game_area(f.hwnd))`.
3. Expire toasts, then `ctx.request_repaint_after(earliest until − now)` if any remain.
   - Demo: push one toast once (`demo_toast`) that never expires. Title is the first cached
     user's display name (else "Ferris"); body is "Anyone up for a round after this match?".
4. Voice `Want`:
   - Exists when `area` is set, `voice_corner != Off`, and there are rows:
     `voice_rows(state, view)` (demo: `demo_rows(state)`).
   - `tex` = `images.get(ctx, &user_avatar_url(guild_id, u, member, 128, false), 64, false).map(|t| t.id)`.
     Doing this every pass keeps the textures from idle eviction: eviction only runs in root
     `end_frame`, after this.
   - `size = voice_size(rows.len(), names, users, large)`, `offset = 0`.
5. Toast `Want`:
   - Exists when `area` is set, `notification_corner != Off` and toasts are not empty.
   - Rows are ordered top to bottom: newest first for top corners, newest last for bottom ones.
   - `tex` is the author's avatar as above (author None → None). `seed` = author id, else 0.
   - `size = (TOAST_W, n·TOAST_H + (n−1)·GAP)`.
   - `offset = voice.size.y + GAP` if the voice `Want` exists in the same corner, else 0.
6. `self.voice.sync(ctx, VOICE_ID, "Rustcord voice overlay", want)`, the same for the toast pane.
   With a demo, `eprintln!("overlay: voice shown|hidden")` on each change, for the `gated` test.

**`Pane::sync(ctx, id, title, want: Option<Want>)`**:
- `None` while declared: `live = None`, send `ViewportCommand::Visible(false)` to `id`, then
  `ctx.request_repaint()`. The root's next full pass GCs the undeclared viewport and drops its
  window.
- `Some(w)`:
  - Not declared: `live = Some((builder(title, &w, ctx.pixels_per_point()), Arc::new(Mutex::new(PaneState { want: w.clone(), tries: 0, shown: false, styled: None, last_paint: None, created: now, shot: demo_shot(title), title }))))`.
    The builder (§2 flags; position/size = the placed rect ÷ root ppp, a first guess) stays
    **identical for the window's life**, because a changed builder makes egui move/resize the
    window and fight `win32::set_client_rect`.
  - Declared and `state.want != w`: store it, `tries = 0`, then
    `ctx.request_repaint_after_for(MIN_GAP.saturating_sub(since last_paint), id)`. That caps
    speaking updates at 10 Hz.
  - `ctx.show_viewport_deferred(id, builder.clone(), move |ui, _| paint::show(ui, &shared))`,
    with `shared` a clone of the pane's Arc, every pass while declared.
  - If `!ctx.input(|i| i.raw.viewports.contains_key(&id))`:
    `ctx.send_viewport_cmd(ViewportCommand::RequestPaintWhileHidden)` (root; §2).

**`pub fn notify(&mut self, state, settings, title: &str, body: &str, author: Option<Id>) -> bool`**:
- Returns false unless the overlay is enabled (or a demo runs), `notification_corner != Off`,
  and a game is in front: demo `Always`, or `win32::foreground()` passes `is_game`.
- Else it pushes `Toast { until: now + TOAST_FOR, .. }`, dropping the oldest past
  `MAX_TOASTS`. The body has `\n` replaced by spaces. It returns true.

Pure helpers, all unit-tested:
- `fn is_game(fg: &win32::Foreground, table: Option<&Table>, added: &[AddedGame]) -> bool`
  - false for `std::process::id()`
  - `let p = fg.path.as_deref().unwrap_or(&fg.name)`
  - `added.iter().any(|g| g.exe == p) || table.is_some_and(|t| t.find(p).is_some())`
  - Doc comment: "same rule as `activity::scan::pick`".
- `fn voice_rows(state: &AppState, view: &ViewState) -> Vec<Row>`
  - Empty while `state.voice.channel_id` is None.
  - As `ui/call.rs` `tiles` builds people: ids of `state.voice_states[ch]` plus me if missing.
  - name = `member_display_name(guild_id, user, member)`, else "Unknown user"
  - `muted = self_mute || mute`, `deaf = self_deaf || deaf`; for me also `view.self_mute` and
    `view.self_deaf`
  - `by_server = mute || deaf`, `speaking = state.voice.speaking.contains`
  - Sorted by (lowercase name, id). `tex` is filled in by `logic`.
- `fn slots(n: usize, users: Show) -> usize`
  - Speaking → `n.min(MAX_ROWS)`
  - otherwise `n.min(MAX_ROWS) + (n > MAX_ROWS) as usize`
- `fn voice_size(n, names, users, large) -> Vec2`
  - `w = PAD + A + 2·RING + (names != Never) as f32 · (GAP + NAME_MAX) + PAD`
  - `h = 2·PAD + slots(n, users)·row`
- `fn place(area: [i32;4], size: [i32;2], corner: Corner, margin: i32, offset: i32) -> [i32;4]`
  - x: left `ax + margin`; right `ax + aw − margin − w`
  - y: top `ay + margin + offset`; bottom `ay + ah − margin − offset − h`
  - Then clamp x into `ax ..= max(ax, ax+aw−w)`, y likewise. Off is never placed.
- `fn visible(scene: &VoiceScene) -> (Vec<&Row>, usize /*more*/)`
  - Speaking → speaking rows, first `MAX_ROWS`, more = 0
  - else → first `MAX_ROWS`, more = `n − MAX_ROWS`
- `fn demo_rows(state) -> Vec<Row>`
  - me plus the first 4 cached users by id (`state.users`)
  - speaking: rows 0 and 2; row 1 muted; row 3 deafened by the server

### 5.2 `win32.rs`

windows-sys 0.61. Add the features `Win32_Graphics_Gdi` and `Win32_UI_Accessibility` to the
windows-sys entry in Cargo.toml; ARCHITECTURE allows new windows-sys features. Every unsafe block
gets a `// SAFETY:` line, as in `activity/scan.rs`.

```rust
#[derive(Clone, Debug, PartialEq)] pub struct Foreground { pub hwnd: isize, pub pid: u32, pub path: Option<String>, pub name: String }
pub fn watch(ctx: &egui::Context)          // once (OnceLock<egui::Context>): spawns "rustcord-foreground"
pub fn foreground() -> Option<Foreground>  // clone of the last one seen
pub fn game_area(hwnd: isize) -> Option<[i32; 4]>
pub fn own_window(title: &str) -> Option<HWND>
pub fn style(hwnd: HWND, hide_from_capture: bool, styled: &mut Option<(isize, bool)>) -> bool  // true: new HWND or setting
pub fn client_rect(hwnd: HWND) -> [i32; 4]
pub fn set_client_rect(hwnd: HWND, r: [i32; 4])
```

**Watcher thread** (event-driven, 0 CPU while idle; a polling timer would wake every second):
- `SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, null_mut(), Some(on_event), 0, 0, WINEVENT_OUTOFCONTEXT)`
  - Own-process events are kept, so Rustcord in front is seen.
  - A null hook → `eprintln!` and return.
- Call `seen(GetForegroundWindow())`, then loop `GetMessageW(.., null, 0, 0) > 0 → DispatchMessageW`.
  The thread lives for the whole process.
- `on_event`: if `idobject == OBJID_WINDOW`, call `seen(hwnd)`.
- `seen`:
  - Ignore a null hwnd.
  - pid comes from `GetWindowThreadProcessId`.
  - For another process, `path = crate::activity::process_image(pid).map(|(p, _)| p)`. If that is
    None, `name` = that pid's name from `crate::activity::processes()` (Toolhelp, protected
    processes only); else `name = file_name(path)`.
  - If the value changed: store it in the `Mutex`, then
    `ctx.request_repaint_after_for(1 ms, ViewportId::ROOT)`.
- Expose the two functions from `src/activity/scan.rs` (activities owner, additive):
  `#[cfg(windows)] pub(crate) use os::{image, processes};` in scan.rs, and
  `#[cfg(windows)] pub(crate) use scan::{image as process_image, processes};` in activity/mod.rs.

**`game_area`**:
- None if `IsIconic` or `!IsWindowVisible`.
- client = `GetClientRect` + `ClientToScreen(0,0)`.
- monitor = `GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST)).rcMonitor`.
- Return their intersection, or None if it is under 64 px either way.
- It is cheap enough to call every pass (a few user32 calls). Shortcut: a game moving or resizing
  without a foreground change is picked up at the next root pass. Upgrade to
  `EVENT_OBJECT_LOCATIONCHANGE` hooked on the game's pid if that shows.

**`own_window`**: `EnumThreadWindows(GetCurrentThreadId(), ..)` compares `GetWindowTextW` with
the title. Only this UI thread's windows are searched, so another Rustcord's overlay (a test
`RUSTCORD_HOME`) can never match.

**`style`** (every pass, cheap):
- `ex = GetWindowLongPtrW(GWL_EXSTYLE)`; `want = (ex | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW`.
- If `ex != want`: `SetWindowLongPtrW`.
- If the HWND or setting is new (`*styled != Some((hwnd as isize, hide))`) or `ex` was rewritten:
  - `SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA)`
  - `SetWindowDisplayAffinity(hwnd, if hide { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE })`
  - store `*styled`
- Return whether the HWND was new.

**`set_client_rect(hwnd, [x,y,w,h])`**:
- Measure the non-client offsets: `l = client.x − window.left`, `t = client.y − window.top`, and
  r, b likewise. winit's shadow hack makes t = 1.
- `SetWindowPos(hwnd, HWND_TOPMOST, x−l, y−t, w+l+r, h+t+b, SWP_NOACTIVATE | SWP_NOOWNERZORDER)`.
  HWND_TOPMOST also lifts the overlay back above a topmost game.
- `SetWindowRgn(hwnd, CreateRectRgn(l, t, l+w, t+h), 1)`. The system owns the region; don't
  delete it.

### 5.3 `paint.rs`: `pub(super) fn show(ui: &mut egui::Ui, shared: &Arc<Mutex<PaneState>>)`

1. `let Some(hwnd) = win32::own_window(s.title) else { return }`.
2. `let new = win32::style(hwnd, s.want.hide_from_capture, &mut s.styled)`.
3. `ppp = ctx.pixels_per_point()`. The overlay's own ppp includes the game monitor's DPI and
   zoom, so the content in points fits exactly.
   `target = place(want.area, (size·ppp).round(), corner, (MARGIN·ppp).round(), (offset·ppp).round())`.
4. Placement handshake:
   - If `new || (client_rect(hwnd) != target && s.tries < 4)`: `tries += 1`,
     `set_client_rect(hwnd, target)`, `ctx.request_repaint()`.
   - Else if `!s.shown`: `shown = true`, `ctx.send_viewport_cmd(Visible(true))`,
     `ctx.request_repaint()`. That extra pass re-adds the ex-styles that winit's show rewrites.
   - A move to a monitor with another DPI gets one `WM_DPICHANGED` resize from winit. The
     next pass corrects it, and the cap of 4 tries ends any loop.
5. `s.last_paint = Some(now)`. Draw the scene (§6) with `ui.painter()`.
6. Demo screenshot: if `s.shot` is Some, `shown` and `created.elapsed() ≥ 1 s`:
   - `take()` the path, then `ctx.request_screenshot(..)`.
   - Save a PNG composited over `#808080` (premultiplied: `c + 128·(255−a)/255`) so white text
     and dark pills both read.
   - Before that time, `ctx.request_repaint_after(remaining)`.
   - `demo_shot(title)` = `<RUSTCORD_SCREENSHOT stem>-overlay-voice.png` / `-overlay-toast.png`,
     only with a demo.

## 6. Visuals (docs/DESIGN.md tokens; fixed sizes, scaled by Zoom)

The overlay sits on arbitrary game pixels, so it uses the media tokens, like the call tiles.

**Voice widget**
- No background.
- Mirrored for right corners: avatar at the right edge, pill to its left, right-aligned.
- Packing:
  - Rows pack toward the corner's edge: top corners from the top; bottom corners so the last
    visible row touches the bottom.
  - Slot i: `y = PAD + (bottom ? slots − visible + i : i)·row`.
- Avatar:
  - `A` square at `PAD + RING` from the side edge, vertically centred in its row.
  - Drawn as `egui::Image::from_texture(SizedTexture::new(tex, Vec2::splat(A))).corner_radius(A/2).paint_at(ui, r)`.
  - Without a texture: `ui::images::paint_initial(&p, r, A/2, row.id.0, &row.name, true)`.
- Speaking: `circle_stroke(center, A/2 + 2, Stroke::new(2, pal.positive))`.
- Mark (muted or deaf):
  - A disc of diameter `A/2` centred at `avatar.max − Vec2::splat(A/4 − 2)`, filled `pal.pill`.
  - Icon `Icon::Deafened` if deaf, else `Icon::MicOff`, side `A/2 − 4`.
  - Icon colour is `pal.danger` when by the server, else `pal.on_media` (`icons::paint`).
- Name pill:
  - Shown if `names == Always`, or `Speaking` and the row speaks.
  - Placed `GAP` after the avatar slot, height pill h, radius 4 (chip tier), fill `pal.pill`.
  - Text is `ui::one_line(ui, name, FontId::new(font, theme::medium_family()), pal.on_media, NAME_MAX − 16)`,
    8 padding each side, centred vertically.
- "N more" row: the same pill without an avatar, text "{n} more" in `pal.on_media_muted`.

**Toast card** (each 320 × 80, `GAP` apart; E2 look, no shadow)
- Card:
  - Fill `pal.card`, radius `pal.card_r`.
  - Stroke 1 px `pal.rule`; High Contrast: 1.5 px `pal.muted` (DESIGN §6).
- Avatar 32 at (12, 12), round. Without a texture: `paint_initial` seeded by `seed` with the
  title's first letter. Streamer Mode passes no author, so the card shows "R" for "Rustcord".
- Title at x 56, y 12, width 252:
  - one line elided (`one_line`)
  - `crate::ui::semibold(14.0)`, `pal.text`
- Body at y 34:
  - `LayoutJob` in proportional 14, `pal.muted`
  - `wrap = TextWrapping { max_width: 252.0, max_rows: 2, break_anywhere: false, overflow_character: Some('…') }`

## 7. Changes outside the module (small, local)

1. **Cross-thread wakes target the root.** Replace each call as follows:
   - `request_repaint()` → `request_repaint_of(egui::ViewportId::ROOT)`
   - `request_repaint_after(d)` → `request_repaint_after_for(d, egui::ViewportId::ROOT)`

   Calls to change (line numbers as of 2026-11-15; other work may move them):
   - `backend.rs:1073` (`EventSink::send_after`)
   - `update/mod.rs:255, :398`
   - `voice/video/mod.rs:410`
   - `media/mod.rs:266, :275, :303`
   - `notify.rs:139`
   - `theme/fonts.rs:208`
   - `plugins/idle.rs:62`
   - `ui/viewer.rs:721, :738`

   Then grep for any other `request_repaint` on a context kept in a struct, a static, or a
   closure moved to another thread or task. Calls on the UI thread stay as they are.
2. `ui/mod.rs` (ui-shell):
   - add `pub mod overlay;`
   - add the `RustcordApp` field `overlay: overlay::Overlay` (`Default::default()`)
   - in `logic`, after `dm_call::logic(..)`:
     `self.overlay.logic(ctx, &self.state, &self.view, &self.settings, &self.theme, &mut self.images);`
   - in `drain_events`, wrap `crate::notify::show(title, body)`:
     ```rust
     // A game is in front: the overlay shows it instead of the desktop notification.
     let author = (!self.settings.streamer.hides_personal()).then_some(n.author_id);
     if !self.overlay.notify(&self.state, &self.settings, title, body, author) { crate::notify::show(title, body); }
     ```
   - In `impl eframe::App` add:
     `fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] { [0.0; 4] }`, with a comment that the
     clear is shared by all viewports and the overlay needs it transparent, while the root's
     panels cover the root. Verify per §10 step 2. If root screenshots change, also fill
     `ctx.content_rect()` with `guild_bar` on `LayerId::background()` at the top of `ui`.
   - First line of `raw_input_hook`:
     `if raw.viewport_id != egui::ViewportId::ROOT { return; }`. It runs for every viewport's
     pass, and the pointer, click and focus hooks are the root's.
3. `state.rs`: add `pub author_id: Id` to `Notification` and set
   `author_id: msg.author.id` in `build_notification`. Fix any test literals.
4. `config.rs`: §4. `Cargo.toml`: the two windows-sys features. `activity/`: the two re-exports
   in §5.2.
5. `ui/user_settings.rs`:
   - `GROUPS`: `("Activity settings", if cfg!(windows) { &[Page::ActivityPrivacy, Page::RegisteredGames, Page::GameOverlay] } else { &[Page::ActivityPrivacy, Page::RegisteredGames] })`.
     The nav, `from_name` and the search all go through `GROUPS`, so the page disappears
     elsewhere. If the branch coercion doesn't compile, use two `#[cfg]` `const` slices.
   - `#[cfg(windows)] mod overlay;`
   - In `page()`, replace the `empty_state` arm with
     `#[cfg(windows)] Page::GameOverlay => changed |= overlay::show(ui, cx),` and
     `#[cfg(not(windows))] Page::GameOverlay => {}`.
   - `info()` keywords: "overlay in-game voice widget notifications corner position avatar names".
   - `local_labels`: `Page::GameOverlay => &["Enable in-game overlay", "Position", "Avatar size", "Show names", "Show users"]`.
   - In the module doc, drop "overlay" from the "What Rustcord can't do" list.
6. Docs:
   - ARCHITECTURE.md:
     - the ownership row
     - in "Goals", one sentence: cross-thread wakes name `ViewportId::ROOT`
     - the `RUSTCORD_OVERLAY_DEMO` test hook
   - SETTINGS_NOTES.md §Game Overlay: "✔ out-of-process overlay, docs/OVERLAY_DESIGN.md".
   - EGUI_0_37_NOTES.md, a short "Multiple viewports" section:
     - the deferred lifecycle and GC
     - logic-only passes create nothing, so use `RequestPaintWhileHidden`
     - the global `clear_color`
     - repaint calls target the current viewport
     - layered passthrough needs `SetLayeredWindowAttributes`
     - the undecorated shadow is removed by a region
     - winit rewrites `GWL_EXSTYLE`

## 8. Settings page `ui/user_settings/overlay.rs`

`pub(super) fn show(ui, cx) -> bool` (true = settings changed; the caller saves). It uses
`super::{switch_row, setting_row, combo_icon}` and `super::super::{section, hint}`. It adds one
helper,
`fn choice_row<T: Copy + PartialEq>(ui, theme, label, hint: Option<&str>, value: &mut T, options: &[(T, &str)]) -> bool`:
`setting_row` with a 220-wide `egui::ComboBox` (`.icon(combo_icon)`, `selectable_label` items),
the pattern of `synced::pick_row`.

1. `switch_row` "Enable in-game overlay". Description: "Shows your voice call and new messages on
   top of games in windowed and borderless fullscreen. Exclusive fullscreen isn't supported."
2. The rest goes inside `ui.add_enabled_ui(o.enabled, ..)`:
   - `section` "Voice":
     - "Position", description "Where the people in your call show.", options Top left / Top
       right / Bottom left / Bottom right / Off
     - "Avatar size": Small / Large (`large_avatars`)
     - "Show names": Always / While speaking / Never
     - "Show users": Always / While speaking
   - `section` "Notifications":
     - "Position", same options, description "New direct messages and mentions, as your
       notification settings allow. Each one hides after 5 seconds."
3. `hint`: "The overlay shows while a game from Registered games is in front. It's a separate
   Rustcord window: nothing is added to the game."

## 9. Budget

| Item | Budget |
|---|---|
| Idle CPU | 0 (no timers; the hook thread sleeps in `GetMessageW`) |
| Overlay repaints | ≤ 10/s while speaking changes, < 1 ms each (≤ 11 avatars and pills); one root wake per toast expiry |
| Vsync | each window swaps with vsync, so an overlay paint can add one vblank of UI-thread wait. Fine at ≤ 10 Hz. |
| RAM | ≤ 2 small windows, ~0.1 MB each (measured), plus avatar textures already inside the 12 MB budget. No monitor-sized surfaces. |
| Startup | nothing before the first frame. The thread starts on the first `logic` pass with the overlay on; screenshot runs without a demo never start it. |

Known cost: when a topmost overlay sits over a flip-model borderless game, Windows composes
instead of using independent flip. Every out-of-process overlay has this cost, and turning the
overlay off removes it.

## 10. Tests and verification

**Unit tests** (`live.rs`, `#[cfg(test)]`, Windows):
- `place`: all four corners, margin, offset, clamping
- `slots` and `voice_size`: 10 + 1, "Speaking" without the "more" row, names Never is narrower
- `voice_rows`: me added before my echo, my view mute and deafen, server mute marks `by_server`,
  sorting, speaking
- `visible`
- `is_game`: own pid; added by path; table by path; a name-only process matches only bare-name
  table entries (use `Table::from_json`)
- `notify`: cap of 3 and order; false with a corner of Off, the overlay disabled, or no game
- `OverlaySettings`: toml without `[overlay]` gives the defaults; the enums round-trip snake_case
- `Page::from_name("overlay")` is Some only on Windows

**Run** (scratch `RUSTCORD_HOME` = a copy of `tests/fixtures/home`, `RUSTCORD_OFFLINE=1`,
`RUSTCORD_WINDOW_SIZE=1180x760`, debug):
1. After every step: `cargo clippy --all-targets -- -D warnings`, `cargo test`, and
   `cargo check --target x86_64-unknown-linux-gnu` (the stub must compile, with no unused
   warnings).
2. Step 1 (§7.1/§7.2) changes no pixel: take `RUSTCORD_SCREENSHOT` of the main view and of
   `RUSTCORD_OPEN=settings:overlay` before and after; the PNG sha256 must be equal for the main
   view.
3. `RUSTCORD_OVERLAY_DEMO=1` + `RUSTCORD_OPEN=voice:<voice channel id>` + `RUSTCORD_SCREENSHOT=<dir>/o.png`
   gives `o-overlay-voice.png` and `o-overlay-toast.png`. Look at them, then repeat with
   `<home>/config/settings.toml` `[overlay]` variants:
   - `show_names = "speaking"`, `show_users = "speaking"`, `large_avatars = false`
   - both corners `"top_right"` (stacking)
   - Lilac and High Contrast themes (toast card)
4. Idle and RAM:
   - Start `RUSTCORD_OVERLAY_DEMO=1` without a screenshot, via `Start-Process -PassThru`.
   - Read `TotalProcessorTime` at 5 s and 15 s: the delta must be < 50 ms.
   - Run with `RUSTCORD_MEMSTATS=<file>` with and without the demo: < 2 MB apart.
   - Stop only that PID.
5. Real foreground gating, with no game needed:
   - Write `<home>/config/games.json`:
     `[{"name":"Charmap","exe":"c:/windows/system32/charmap.exe"}]`.
   - Start Rustcord with `RUSTCORD_OVERLAY_DEMO=gated` (stderr to a file), then start
     `charmap.exe` with `-PassThru`.
   - After 2 s stderr shows "overlay: voice shown" and "toast shown".
   - Stop that charmap PID: "hidden" follows.
   - Kill only the processes you started.
6. Optional composite check: during the step 4 run, capture only Rustcord's own window rect with
   PowerShell `CopyFromScreen`. It must show the panes on Rustcord's UI with no frame, shadow or
   top line. Never capture other parts of the screen.
7. A real game, by the user: windowed and borderless, with Discord-like expectations. Alt+Tab,
   Rustcord in front, minimized, and in the tray (the panes must still appear there; §2
   `RequestPaintWhileHidden`).

## 11. Step order

1. §7.1 wake fix, `clear_color` and the `raw_input_hook` guard; verify §10 step 2.
2. Config §4, `Notification.author_id`, the activity re-exports, the Cargo features.
3. `live.rs` pure helpers with their tests, then `win32.rs`, `paint.rs`, `Overlay::logic` and
   `notify`, the stub `mod.rs`, then the wiring in `ui/mod.rs`.
4. The demo hook; §10 steps 3–6.
5. The settings page and `GROUPS`; a screenshot of `RUSTCORD_OPEN=settings:overlay`.
6. Docs §7.6; final clippy, tests, and the Linux check.
