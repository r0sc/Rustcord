# Rustcord architecture (for implementers)

Lightweight third-party desktop chat client in Rust. Talks to Discord's HTTP API and Gateway
directly as a **user** client. Not a port of the official client.

## Goals that constrain every module
- Window visible < 1 s; cached UI shown immediately, network connects in the background.
- < 150 MB RAM with a few servers open: downscale images at decode time, cap animation frames,
  evict textures, cap messages per channel (`state::MAX_MESSAGES_PER_CHANNEL`), 2 tokio workers.
  Where it goes (AMD iGPU, release, offline): ~80 MB private is the GL driver as soon as the
  context exists; each MB of texture costs ~2.7 MB private (hence the 12 MB texture budget and
  idle eviction in `ui/images.rs`); all of `AppState` is < 10 MB even for a large account.
- egui repaints only on input/events (reactive). Never call `request_repaint()` every frame;
  animations use `request_repaint_after(next_frame_delay)`. A plain `request_repaint()` costs
  egui two passes, so wakes from other threads use `request_repaint_after(1 ms)`: a backend
  event (`EventSink::send`, a burst shares the pass) and a video frame (`FrameWriter::write`)
  each wake one pass; `RustcordApp::settle_after_events` adds one more pass only if the event
  pass moved a widget (hover, the chat's stick-to-bottom and area sizes use the previous pass's
  rects). UI code that must show a state change on the next pass without moving a widget
  requests its own repaint. Every wake from another thread names `ViewportId::ROOT`
  (`request_repaint_of`, `request_repaint_after_for`): a bare `request_repaint*` targets the
  viewport being run, which is an overlay window while one repaints (docs/OVERLAY_DESIGN.md §2).
- Windows first; cross-platform where it costs nothing (`#[cfg(windows)]` for Win32-only code
  with a no-op fallback).

## Hard rules
- **Token**: `auth::Token` only. Never log it, never format it into errors/toasts, never write
  it to disk except via `auth::save_token` (OS keyring). Only send it to `https://discord.com/api/*`
  and the gateway IDENTIFY/RESUME. Image/CDN requests use a separate HTTP client without auth.
- **Login secrets** (password, 2FA code, MFA/QR tickets): `auth::Secret` only (zeroized,
  redacted Debug); never logged, persisted or put into error text. Login calls go through
  `api::auth::AuthClient` (no `Authorization`, no `RateLimiter`, one request per user action,
  429 is reported, never retried). A captcha response is shown to the user inside the login card
  (`src/captcha.rs`, Windows and macOS) and the request is sent again with the answer the *user*
  solved; elsewhere the attempt stops with a message. Never solve, bypass or automate one. Any
  token a login produces re-enters via `Command::Login`.
- **Normal user client behaviour**: no automation, no background sending, no mass actions.
  Every request is user-triggered, except the gateway heartbeat/identify/resume, read acks while
  a channel is viewed (setting `send_read_acks`), and an op 37 guild subscription (member list
  range 0-99 + typing) for the viewed guild channel, sent even when the member list is hidden.
  Quests: the page's fetch when it opens (at most every 10 min), and quest video progress
  (`POST /quests/{id}/video-progress`, at most one per 10 s) only while a quest video the user
  started plays in Rustcord's player (also in the background after its window closes, until it
  ends, the quest is done or the user stops it), capped by real play time. Never quest heartbeats, never an
  Electron user agent (docs/QUESTS_NOTES.md).
- **Rate limits**: respect REST buckets + global + 429 `retry_after`; gateway send limit
  (120 / 60 s, keep headroom); reconnect backoff with jitter; never hammer on errors (cap retries;
  4xx other than 429 are not retried).
- No new crates without asking the integrator (the user must approve anything > a few MB).
  Available: eframe/egui 0.37 (glow), tokio, reqwest 0.13 (native-tls, json, gzip),
  tokio-tungstenite 0.30 (native-tls), futures-util, serde, serde_json, toml 1.x, directories 6,
  flate2, base64 0.23, percent-encoding, image 0.25 (png, jpeg, gif, webp, ico), chrono 0.4
  (clock), rfd 0.18, keyring 4.2 (v1 API: `keyring::Entry`), windows-sys 0.61 (Win32 Foundation,
  UI_Shell, UI_WindowsAndMessaging, System_LibraryLoader – add more windows-sys *features* if
  needed, that's fine), rsa 0.10 + sha2 0.11 + getrandom 0.4 (`sys_rng`) for the QR login key,
  qrcodegen 1.8, zeroize 1.9. Voice (pinned, approved): davey 0.2.0, opus-rs 0.1.42, cpal 0.18.2,
  aes-gcm 0.10.3, chacha20poly1305 0.10.1, tokio `net`. Video (pinned, approved): openh264 0.9.10,
  windows-capture 2.0.1, `windows` 0.62.2 (Media Foundation features). Media playback (pinned,
  approved, pure Rust): re_mp4 =0.5.1 (MP4/MOV demux), symphonia-core + symphonia-codec-aac
  =0.6.1 (AAC-LC; MPL-2.0, unmodified). Emoji data (approved): emojis 0.9.1. Build-only: embed-resource 3 (Windows build-dependency, icon resource),
  resvg 0.49 (dev-dependency, `examples/export_icons.rs`). Linux / macOS (target-gated, approved):
  libc 0.2 (`cfg(unix)`: font mmap, `malloc_trim`, `/proc`, libproc), Linux ksni =0.3.7
  (`async-io` + `blocking`, never its `tokio` default), zbus 5.19 (`async-io` + `blocking-api`,
  the one keyring builds), winit 0.30.13 (`x11`, to force X11); macOS tray-icon =0.26.1,
  objc2-user-notifications 0.3.2, objc2 0.6, block2 0.6, objc2-foundation / objc2-app-kit 0.3.2.
  Updater (approved, already in the tree): ed25519-dalek 2.2, semver 1.0.
- These crate versions are newer than your training data. **Check real APIs in the downloaded
  sources** under `~/.cargo/registry/src/index.crates.io-*/`
  (e.g. `egui-0.37.0\src`, `reqwest-0.13.*`, `tokio-tungstenite-0.30.0`, `toml-1.*`) instead of
  guessing. egui notes: `docs/EGUI_0_37_NOTES.md`. Protocol notes: `docs/PROTOCOL_NOTES.md`,
  login protocol: `docs/AUTH_NOTES.md`.

## Building (path length)
In a deep folder the MSVC linker fails with `LNK1104` (260-character path limit); build with a
short target dir:

PowerShell:
```
$env:CARGO_TARGET_DIR = 'C:\rc-target'
cargo check
```
Bash:
```
export CARGO_TARGET_DIR='C:/rc-target'
cargo check
```
`cargo test <module>` for unit tests.

## Data flow
```
            Command (tokio mpsc)                     HTTP          discord.com/api/v9
 UI thread ─────────────────────► backend::run ──► api::ApiClient ───────────────►
 (eframe)                              │
 AppState  ◄────────────────────────── ├──► gateway::spawn ── WebSocket ── gateway.discord.gg
 ImageCache   BackendEvent (std mpsc   │
              + request_repaint_after) └──► cache::images::ImageLoader ── cdn/media.discordapp, static.klipy.com
```
- `state::AppState` is owned by the UI thread; it is pure data + logic (no I/O, no egui).
- `backend.rs` defines `Command`/`BackendEvent` (the only UI<->network contract).
- `gateway/` normalizes wire payloads into `model::GatewayEvent`; READY guild shape differences,
  DM recipients, member list ops are handled there.
- `cache/` persists `cache::Snapshot` (JSON) and image bytes; `AppState::{from,to}_snapshot`.
- `ui/` renders through `UiCx` (borrows of state, view state, image cache, theme, settings,
  command sender, local profile `overrides`).
- Login (`ui/login.rs`, tabs QR code / Email / Token; progress via `BackendEvent::LoginStep`):
  - `api/auth.rs`: `AuthClient`, unauthenticated calls to discord.com (`/experiments`
    fingerprint, `/auth/login`, `/auth/mfa/*`, `/users/@me/remote-auth/login`) and the error
    classifier (captcha, 2FA codes, 429 …).
  - `gateway/remote_auth.rs`: QR login over `wss://remote-auth-gateway.discord.gg/?v=2`
    (RSA-OAEP key per code, heartbeat, close codes); the ticket exchange is the backend's
    (`Command::QrExchange`), so a captcha there can be answered like the others.
  - `captcha.rs`: hCaptcha in a wry webview that is a child of the main window, laid over the
    login card's slot once per pass (`Inline::sync`, `docs/AUTH_NOTES.md` section 0). The backend parks the refused request
    (`LoginStep::Captcha`) and resends it with the token (`Command::Captcha`).
  - `backend::run` keeps the login state (shared auth client, 2FA ticket, QR task) and feeds every
    resulting token into `Command::Login` (validate, keyring, gateway). It also holds Discord's
    device cookies, which survive a logout; Logout closes the gateway, then ends the token at
    Discord (`POST /auth/logout`, bounded, errors ignored).
- Self-updater (`src/update/`, `docs/UPDATER_DESIGN.md`): one task on the same runtime with its
  own reqwest client (no token, no Discord headers, `Rustcord/<version>` user agent) that only
  talks to github.com. UI → updater: `update::sync_ui` (hidden / busy / interacted, every
  `logic` pass) and the button fns, as events over an mpsc channel; updater → UI:
  `update::status()` (a `Status` behind a mutex, read once per pass via `ui::update::status`)
  and `request_repaint_after(1 ms)` when it changes. `machine.rs` decides without I/O, `feed.rs`
  fetches and verifies (signature before parsing), `store.rs` keeps `<cache>/update/state.toml`,
  `platform.rs` installs (Windows setup `--update`, Linux `install.sh --update` or pkexec,
  macOS bundle swap). `update::start` runs before the first frame without network.

## Module ownership (one implementer per row; don't edit other rows' files)
| Owner | Files |
|---|---|
| api | `src/api/*`, body of `backend::run` (+ private helpers in backend.rs) |
| gateway | `src/gateway/*` (add `payloads.rs`, `inflate.rs`, ... as needed) |
| state | `src/state.rs` |
| cache | `src/cache/*`, `src/ui/images.rs` |
| theme+notify | `src/theme/*`, `src/notify.rs` |
| ui-shell | `src/ui/mod.rs`, `login.rs`, `members.rs`, `chrome.rs` (frameless window: title bar buttons, drag, edge resize), `motion.rs` (DESIGN §7 tokens and helpers) |
| cosmetics | `src/ui/cosmetics.rs` (shared renderers), `src/api/{cosmetics,collectibles}.rs`, `src/ui/user_settings/{cosmetics,connections}.rs`, `src/theme/gradient.rs`: see `docs/COSMETICS_NOTES.md` |
| ui-messages | `src/ui/messages.rs`, `composer.rs`, `polls.rs`, `forward.rs` |
| markdown | `src/ui/markdown.rs`, `src/ui/markdown/parse.rs` |
| phase2-profile | `src/overrides.rs` (local banner/avatar and local cosmetics: client-only, never uploaded) |
| social / server / settings UI | `src/ui/{sidebar,friends,inbox,profile,profile_modal,threads,forum,server_menu,server_settings,user_settings}.rs`, `src/ui/user_settings/*`: split in `docs/SOCIAL_UI_DESIGN.md` |
| profile | `src/ui/guild_card.rs` (server tag card: join / apply on click), badge popover and `tag_button` in `cosmetics.rs`, server marks (`icons::guild_mark`): see `docs/GUILD_CARD_NOTES.md` |
| integrator | `src/model.rs`, `src/backend.rs` enums, `src/main.rs`, `src/config.rs`, `src/auth.rs`, `Cargo.toml` |
| voice | `src/voice/*`, split per file in `docs/VOICE_DESIGN.md` (audio) and `docs/VIDEO_DESIGN.md` (video, Go Live) |
| chat essentials | `src/api/chat.rs`, `src/api/upload.rs`, `src/state/chat.rs`, `src/emoji.rs`, gateway typing/presence/op 3/op 8: split in `docs/CHAT_ESSENTIALS_DESIGN.md` |
| social | `src/api/social.rs`, `src/state/social.rs`, gateway thread/recipient/note dispatches: split in `docs/SOCIAL_DESIGN.md` |
| account & server | `src/api/account.rs`, `src/api/settings_proto.rs`, `src/api/guild_admin.rs`, `src/state/account.rs`, gateway settings-proto/ban dispatches: split in `docs/ACCOUNT_SERVER_DESIGN.md` |
| activities | `src/activity/*` (game detection thread, detectable-games table, my game in op 3, app icons), `src/api/apps.rs`, `src/ui/user_settings/games.rs`; activity art and cards in `ui/members.rs` (`art_slot`) and `ui/profile.rs` (`activity_section`) |
| quests | `src/quests.rs`, `src/api/quests.rs`, `src/gateway/quests.rs`, `src/ui/quests.rs`; the Quests row in `ui/sidebar.rs`: see `docs/QUESTS_NOTES.md` |
| media | `src/media/*` (player worker, Range reads, MP4/H.264/AAC, shared speaker output), `src/ui/viewer.rs` (inline video and gifv, the lightbox `Modal::Media`): see `docs/MEDIA_DESIGN.md` |
| plugins | `src/plugins/*` (registry + plugins.toml, one module per area), `src/ui/user_settings/plugins.rs`; one-line hooks elsewhere guarded by `plugins::enabled`: see `docs/PLUGINS_NOTES.md` |
| overlay | `src/ui/overlay/*`, `src/ui/user_settings/overlay.rs`: see `docs/OVERLAY_DESIGN.md` |
| updater | `src/update/*` (engine; `state.rs` is the UI contract), `src/ui/update.rs` (screen, title-bar button, demo) and the Updates rows in `ui/user_settings/app.rs`; packaging: `rustcord-setup --update`, `install.sh --update`, `scripts/make-update-manifest.py`, release.yml: see `docs/UPDATER_DESIGN.md` |

Assets & build: `assets/logo.svg` / `logo-small.svg` → `examples/export_icons.rs` (resvg) →
`assets/icons/*.png` + `assets/rustcord.ico`, and the flat one-colour `assets/logo-mono.svg` →
`assets/rustcord-tray.ico`. `build.rs` (embed-resource, `assets/rustcord.rc`) embeds them as
icon id 1 (exe, window and balloon icon) and id 2 (notification-area icon, loaded by `notify.rs`
at the small-icon size, falling back to id 1). The in-app mark (home button, title bars) is
painted by `ui/sidebar.rs` `logo_mark` from the same geometry. The packages use the same
PNGs: Linux hicolor icons 16-512, the macOS `.icns` from 16-1024 (`iconutil` in
`scripts/build-macos.sh`) and `icons/rustcord-tray-36.png` (logo-mono in black: the macOS menu
bar template image). Package metadata: `assets/linux/rustcord.desktop`, `assets/macos/Info.plist`
(`@VERSION@` is replaced with Cargo.toml's version).

Shared-contract items (pub signatures in stubs, `model.rs`, `backend.rs` enums, `UiCx`,
`ViewState`, `Snapshot`, `Theme` fields) must not be renamed or removed. You may add private
helpers, new private modules inside your folder, extra pub helper fns, and `impl` blocks. If a
contract change is unavoidable, make the smallest additive change and report it.

## Visual direction
Binding spec: `docs/DESIGN.md` (palette, derived tokens in `theme/tokens.rs`, type roles and
radius tiers on `ui::messages::Pal`, shared components next to `messages::icon_button`).
Original look, not a copy of Discord: blued-steel panes with ember only for the brand, the heat
bar and one primary action (`theme::Theme::ember_dark`). Layout and interaction follow Discord's web client as of its
2025 refresh (title bar, 40 px squircle server icons whose colours change on hover/selection,
channel-list card, floating user panel; `ui/sidebar.rs`, `docs/FEATURES_NOTES.md` Part 4), with
Rustcord's own logo (home button) and theme colors. Never use Discord's logo, wordmark, or blurple as a brand color, and never
the word "Discord" in the app name/title/icon (it may appear in explanatory text such as the
login hint and README).

## Test hooks
- `RUSTCORD_HOME=<dir>`: config+cache under that dir.
- `RUSTCORD_OFFLINE=1`: no keyring, no network; shows cached UI.
- `RUSTCORD_TRACE_STARTUP=<file>`: startup timings.
- `RUSTCORD_CAPTCHA_DEMO=1|auto|open` (with `RUSTCORD_LOGIN_TAB=email`): a password login gets a
  demo captcha (hCaptcha's test sitekey) instead of a request to Discord; `auto` signs in once by
  itself, `open` also shows the open-challenge layout. The webview is a native child window, so
  `RUSTCORD_SCREENSHOT` (egui's own frame) shows its slot empty: capture the window from the OS.
- `RUSTCORD_MEMSTATS=<file>`: appends process counters (working set, private, process heap) at
  startup steps, then 15 s after start (and every minute) capacity-based heap estimates of the big
  `AppState` structures, texture and font-atlas bytes (`ui::images::mem_hooks`).
  `RUSTCORD_MEMTEST=presences:<n>,members:<n>,textures:<MB>[@<side>]` adds what an offline
  snapshot can't carry: presences for `n` cached users, a member list of `n` for the open guild,
  never-drawn textures (`AppState::apply_memtest`).
- `RUSTCORD_SCREENSHOT=<file.png>`: save a screenshot after ~30 frames and exit (the window counts
  as focused, so text focus, completion popups and read acks do not depend on the desktop);
  `RUSTCORD_SCREENSHOT_FRAME=<n>` takes it at frame n instead (e.g. 300 to wait for a QR code).
  Screenshot runs skip the single-instance check and the tray icon (closing then quits).
- Desktop (`notify.rs`, `user_settings/app.rs` `logic`): one Rustcord per data dir (a named
  mutex + the tray window, both named after a hash of the config dir, so runs with their own
  `RUSTCORD_HOME` never meet the user's; Linux / macOS: a Unix socket of that name in
  `XDG_RUNTIME_DIR` or the temp dir); `--minimized` starts hidden in the tray, `--quit` asks
  the running one to quit and waits, `--clear-login` removes the saved login and exits.
  Autostart (`autostart.rs`: HKCU Run, `~/.config/autostart/rustcord.desktop`, a LaunchAgent)
  is switched on once per data dir for an installed copy only: never in debug builds, never with
  any `RUSTCORD_*` variable set, never over an existing entry. Linux runs on X11 (XWayland)
  whenever `DISPLAY` is set; without a tray host closing quits. `RUSTCORD_NAV_DEMO=no-tray`
  shows the Linux settings page as without a tray host.
- Installer (`src/bin/rustcord-setup`, flags in its main.rs): `RUSTCORD_SETUP_TEST=1` switches to
  the test identity "RustcordSetupTest" (own folder, Run value, shortcuts, Apps entry; use it
  with `--silent --dir <scratch>` and uninstall with `--uninstall --silent`);
  `RUSTCORD_SCREENSHOT=<png>` + `RUSTCORD_SETUP_SCREEN=install|progress|done|error|uninstall|removed`
  renders a screen without installing.
- Linux tarball installer (`scripts/linux/install.sh`): `RUSTCORD_SETUP_TEST=1` +
  `RUSTCORD_SETUP_PREFIX=<scratch>` installs everything under `<scratch>` instead of `$HOME`
  (`--quit` gets its own `RUSTCORD_HOME`, `--purge` skips the keyring); `--uninstall --purge`
  must leave `<scratch>` gone. Packaging: `scripts/build-linux.sh`, `scripts/build-macos.sh`,
  CI `.github/workflows/release.yml` (manual or `v*` tags only).
- Updater (`src/update`, `docs/UPDATER_DESIGN.md`): off in debug builds and whenever a
  `RUSTCORD_*` variable other than `RUSTCORD_HOME` is set (`RUSTCORD_HOME` alone: notice only).
  Test mode, only together: `RUSTCORD_UPDATE_FEED=<http(s)://127.0.0.1|[::1]|localhost[:port]/…/latest.json>`
  (signature at `<url>.sig`, packages next to it; no user info, query or fragment),
  `RUSTCORD_UPDATE_PUBKEY=<base64 of the 32-byte Ed25519 key>`, `RUSTCORD_UPDATE_FROM=<X.Y.Z>`
  (pretend to be that version; not passed on to the restarted copy), plus `RUSTCORD_HOME`,
  `RUSTCORD_SETUP_TEST`, `RUSTCORD_SETUP_PREFIX`, `RUSTCORD_OFFLINE` (an end-to-end run must
  not read the saved login or reach Discord), `RUSTCORD_TRACE_STARTUP` and the screenshot hooks
  (`RUSTCORD_SCREENSHOT`, `_FRAME`, `RUSTCORD_WINDOW_SIZE`) (Test mode also accepts the setup's
  "RustcordSetupTest" Apps entry on Windows, and with `RUSTCORD_SETUP_TEST=1` the tarball install
  under `$RUSTCORD_SETUP_PREFIX/.local/bin/rustcord` on Linux). State and downloads:
  `<cache>/update/` (`state.toml`, the staged package). `--after-update` (set by the updater):
  wait up to 30 s for the old copy's instance lock instead of handing off to it, then delete its
  leftovers (`rustcord.exe.old`, macOS `.Rustcord-old-*`). Windows and Linux start the new
  version after the event loop ended (`update::relaunch` in main.rs; Linux `exec`s, so a systemd
  unit keeps the PID), only when the updater asked for the quit, never during a call.
- `RUSTCORD_UPDATE_DEMO=checking|uptodate|available|ready|download|install|restart|error|deb|manual`
  (`ui/update.rs`): a fake updater state for screenshots, no network: the update screen
  (download, install, restart, error, deb, manual), the title-bar button (available, ready),
  the Updates rows with `RUSTCORD_OPEN=settings:windows` (checking, uptodate).
- `RUSTCORD_LOGIN_TAB=qr|email|token|mfa`: start on the login screen with that tab, ignoring
  any stored token (`mfa` = the 2FA form with all methods, no network). Use with an empty
  `RUSTCORD_HOME`; `qr` connects to the remote-auth gateway (never scan such a code in tests).
- `RUSTCORD_WINDOW_SIZE=<w>x<h>`: resize the window on the first frame.
- `RUSTCORD_OPEN=settings|profile`: open the settings page / my account popout (profile card
  with the status picker, opening above the user panel) at startup. `RUSTCORD_OPEN=channel:<id>`
  opens that channel (a DM id shows the DM list; a group DM its member list), `members` fills a
  demo member list for the open guild (all cached users, so server settings show role counts: two
  in a group of the first hoisted role,
  mixed presence, also stored as presences, with a custom status, a game with rich presence and
  art, music and a phone; offline has none; `members::demo_activity`; `friends` likewise shows
  one friend per activity type when the snapshot has enough users), `status:<online|idle|dnd|invisible>` sets my status with a sample custom status,
  `toast` shows a sample error toast, `muted` / `deafened` switch the user panel's microphone /
  headphones off, `voice:<channel id>` shows me connected (and speaking) in that voice channel
  without joining anything (voice panel, green channel bar, me listed under it), `call:<dm id>` a call
going on in that DM / group DM (its other members in it, the second muted, the third deafened:
call area above the chat, green handset in the DM list; with `voice:<dm id>` the call stage there),
`ring:<dm id>` the same call ringing me (incoming-call popup), `outgoing:<dm id>` the call I just
started there (I'm connected, the DM open, its other members rung: dimmed, pulsing tiles; ui/dm_call.rs `demo`). A comma list combines them in
  order, e.g. `channel:<group dm id>,toast`. Social / server / settings surfaces (no request is
  sent by the hook; an opened view sends its one fetch, answered "Not connected" offline):
  `settings:<page>`, `settings-search:<text>` (the settings sidebar search filled in), `server-settings[:<page>|role-<id>|channel-<id>]`, `server-menu`,
  `friends`, `inbox`, `threads`, `thread:<id>`, `card:<user id>`, `modal:<name>` (`modal:forward` = Forward on the newest message of the open channel), `confirm`; see
  `docs/SOCIAL_UI_DESIGN.md`. `modal:apply` / `modal:apply-rules`: the join form for the open
  server with sample questions / its rules only (no fetch).
- `RUSTCORD_PLUGINS=all|<Name>,<Name>`: switches those plugins on for this run (`plugins::apply_env`),
  without writing plugins.toml; `RUSTCORD_OPEN=settings:plugins` opens their page,
  `RUSTCORD_PLUGIN_SEARCH=<text>` fills in its search.
- `RUSTCORD_POINTER=<x>x<y>`: keep the pointer at that point (logical px) for hover states and
  tooltips in screenshots; with `RUSTCORD_CLICK=left|right` it also clicks there once (frames
  10-11), e.g. to open a context menu.
- `RUSTCORD_MSG_DEMO=<comma list>` (ui/messages.rs; `online` hides the composer's Offline chip): message-view states offline mode can't
  reach, on the newest message of the open channel: `hover` (hover toolbar), `reply` (composer
  reply bar), `edit` (inline edit), `picker` (reaction picker), `delete` (confirmation), `menu`
  (its "…" context menu), plus `failed` (a failed send with Retry/Dismiss), `start` (the
  history-start row), `new` (NEW divider above the 3 newest messages; offline runs usually show
  the snapshot's own read state anyway), `older` (scrolled up: "Jump to present" bar),
  `emoji` (the composer's emoji picker), `pins` (pinned-messages popout listing a few cached
  messages), `search` / `indexing` (search results panel with cached hits for "rust" / its
  "still indexing" state) and `detached` (the older half shown as a jump window with the
  target flashed), `forward` (ui/forward.rs: a forward of the message in the middle of the loaded
  history, with its origin link), `poll` (ui/polls.rs: three polls at the end of the open channel,
  a vote view, a multiple choice I voted in, a closed one, and the type 46 result message of that one), `poll-voters` (with `poll`: the voters popover
  open on "Polls" of the second poll, filled from the cached users). Composer (ui/composer.rs): `attach` (tray with sample files and an error),
  `upload` (an upload in flight), `typing` (two people typing), `gif` (GIF tab with placeholder
  category tiles; its Trending tile lists placeholder results), `ac` / `ac-channel` / `ac-emoji` (`@` / `#` / `:` autocomplete open), `cooldown`
  (send cooldown), `drop` (drag-and-drop overlay). Media (ui/viewer.rs `demo`, files seeded into
  the fixture's disk cache by `python tests/fixtures/make_fixture.py media`): `media` adds the
  fixture media to the open channel (demo.png and demo.gif at the top of the loaded history, a
  gifv embed with a WebM upload showing the can't-play card, demo.mp4 at the bottom), `viewer` /
  `viewer-zoom` open the lightbox on demo.png fitted / at 1:1, `viewer-video` on demo.mp4 paused
  at 1.0 s, `player` pauses the inline demo.mp4 at 1.0 s, `play` plays it muted (frames arrive from the
  worker: use `RUSTCORD_SCREENSHOT_FRAME=90`; `RUSTCORD_POINTER` over a gifv plays it), `loop`
  makes the `play` / `player` / `viewer-video` demo.mp4 loop (CPU measurements). None of
  them sends a request.
- `RUSTCORD_FRAME_STATS=1` (ui/frame_stats.rs): every 2 s one stderr line (release builds have no
  console: redirect stderr, e.g. `Start-Process -RedirectStandardError f.txt`): frames drawn,
  `frame` = eframe's time per frame (our passes + egui + tessellation + GL paint, not the vsync
  wait) mean and p95, `app` = our `logic` + `ui`, `tess` = one tessellation of the current shapes
  with their counts, `rest` = frame - app - tess (egui's end of pass, GL upload and draw), then
  the sections in ms per frame (`xN`: calls per frame): logic, title, rail, channels, page
  (Friends, call or chat column) with `+header`, `+members`, `+chat`, `+composer`, `+markdown`,
  `+player`, `+gifv` inside it (`+` = inside another section), user panel, popups (popouts,
  modals, the lightbox), end (toasts, image cache, hooks); `unscoped` = app - top-level sections.
  `RUSTCORD_FRAME_STATS=continuous` also repaints every frame (the cost of a full repaint).
  Off, each timer is one cached check.
- `RUSTCORD_MOTION_SCALE=<f>` (ui/motion.rs): every UI animation duration x f; with `RUSTCORD_SCREENSHOT_FRAME` it catches a menu, modal or toast mid-animation.
- `RUSTCORD_OVERLAY_DEMO=1` (Windows, ui/overlay): the game overlay with fake call members and a
  notification over Rustcord's own window (it plays the game); with `RUSTCORD_SCREENSHOT=<png>`
  each pane also saves `<stem>-overlay-voice.png` / `-overlay-toast.png` (over grey) and the run
  ends after them. `=gated` shows the panes only while a registered game is in front (real
  foreground detection; stderr says `overlay: voice shown|hidden`); try it with Registered
  games = `charmap.exe`. `RUSTCORD_OPEN=voice:<channel id>` fills the call.
- `RUSTCORD_NAV_DEMO=<comma list>` (ui/sidebar.rs `nav_demo`): navigation / social states
  offline mode can't reach, none sends a request: `folders` / `folder-open` (server bar
  folders), `mentions` (inbox mentions from cached messages), `profile` (loaded profiles; with
  `badge:<i>` the i-th shown badge's info popover opens once; with
  `tag-card:<member|join|apply|applied|invite|private|loading|error>` and
  `RUSTCORD_OPEN=card:<user id>` that user wears a server tag whose card opens once in that
  state, `ui/guild_card.rs` `demo`), `boost` (server marks: the first server boost level 2 with
  9 boosts, the second verified, the third partnered; the rail tooltip shows them with
  `RUSTCORD_POINTER` on a server tile, the header without),
  `tab:<online|all|pending|blocked|add>` (Friends tab), `settings` (sample devices, authorized apps
  and consents on those Settings pages, two sample security keys),
  `account:<password|totp|scan|sms|codes-key|codes|mfa|removal>` (that My Account security dialog
  with sample data; `mfa` = the 2FA re-check prompt), `games` (Registered Games with its "Add it!"
  picker open on sample windows), `icons` (every vector icon of `ui/icons.rs` at 16, 20, 24 and
  48 px, then the channel icon matrix plain / private / age-restricted / rules at 20 px, over the
  whole window), `focus:<n>` (ui/mod.rs `raw_input_hook`: drops the focus, then presses Tab n
  times, one per frame from frame 14, so the screenshot shows the focus ring where it lands; a
  negative n presses Shift+Tab, starting from the last control drawn. In the Ember fixture at
  1180x760: 1 the inbox button, 6 the first server tile, 13 the first channel row; with
  `RUSTCORD_OPEN=server-menu` 11 its first item; with `settings:windows` 4 the settings search,
  -3 the first switch; with `modal:create-channel` 6 its name field. More than 16 presses need a
  later `RUSTCORD_SCREENSHOT_FRAME`).
- `RUSTCORD_FAKE_GAME=<name>` (activity/scan.rs): game detection reports that game, started 47
  minutes ago (its icon from the saved games list `cache/detectable.tsv` when the name is in it):
  my user panel, my profile and Registered Games show it. Offline nothing is sent; online it would
  go out with op 3 like a real game, so use it offline. With `RUSTCORD_OPEN=status:<s>` my
  sample presence carries no game then. Activity art comes from the image cache only offline
  (`members::demo_activity` names the URLs to seed).
- `RUSTCORD_OPEN=quests[:blocked|empty|error|loading|watch|watch-play|bg|complete|claim]` (ui/quests.rs `demo`): the
  Quests page (DM home) with made-up quests (`DEMO_JSON`: Orbs, every section, a reward code, a
  captcha notice; dates relative to today), the blocked banner, the empty / failed / loading
  states, the quest video modal (a still frame, nothing sent; `watch-play` plays the media
  fixture's demo.mp4 muted in it, use `RUSTCORD_SCREENSHOT_FRAME=90`) or the claim platform
  modal; `bg` the same video playing without its modal (the user panel's "Quest video" section, the
  card's "Open video"); `complete` the Friends page with quest 101 completing (the "Quest complete"
  notification, `quests-notified.txt` written in the home). No request is sent.
- `RUSTCORD_CALL_DEMO=<comma list>` (ui/call.rs `demo`), with
  `RUSTCORD_OPEN=voice:<id>,channel:<id>` (the call view): `people` (cached users in the call:
  speaking, muted, deafened + live, camera), `video` (synthetic camera / stream frames, my camera
  on, the stream watched), `share` (I share my screen), `focus`, `chat`, `connecting`, `picker`
  (screen share picker, sample sources), `stream:<picture>` (with `people`: the watched stream shows that
  local PNG / JPEG; README screenshots). No request; with `settings:voice`, `video` shows the
  camera preview.

## egui 0.37 gotchas (see docs/EGUI_0_37_NOTES.md for the full list)
- `App::logic(ctx)` runs every frame *and* while the window is minimized; `App::ui(ui)` only when
  visible. Event draining/notifications live in `logic` (already wired in ui/mod.rs).
- `RichText::strong()` only changes color. Bold text: `.family(crate::theme::bold_family())`.
- Emoji: unicode emoji are just text, drawn with the OS color emoji font (epaint `color_fonts`;
  Windows: Segoe UI Emoji, memory-mapped; Linux: Noto Color Emoji), egui's monochrome NotoEmoji
  only as fallback (`theme/apply.rs` `build_fonts`). Emoji used as UI icons (💬 🔒 ⚙) must use
  `FontId::new(size, crate::theme::icon_family())` / `.family(icon_family())`, or they draw in
  color and ignore the icon color.
- `ctx.set_theme(egui::Theme::Dark|Light)` before customizing visuals; style via
  `global_style_mut` / `all_styles_mut` / `style_mut_of`.
- `Id::new` is deprecated → `Id::unique(..)` / `IdSalt`; `ui.id()` → `ui.scope_id()`.
- ScrollArea `state.offset` is private → `state.clamped_offset()`.
- `Margin` is i8, `CornerRadius` is u8.
- Enter-to-send with Shift+Enter newline: see §4 of the egui notes.
