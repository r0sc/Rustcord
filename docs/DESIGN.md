# Rustcord design spec

Binding for all UI work. It replaces the "Visual direction" paragraph of `docs/ARCHITECTURE.md`
(warm ember dark). Layout stays Discord web 1:1 (rail, channel card, chat, member list, floating
user panel, settings layers, menus, popouts). The look is Rustcord's own.

Decision record (2026-11-13): two proposals were judged against the brief
("Refined" and "Forged"; working files, not in the repo). Scores out of 5:

| Criterion | A "Refined" | B "Forged" |
|---|---|---|
| Discord layout 1:1, users feel at home | 4: ember for switches, focus and mentions breaks the colours users know | 5: Discord's state colours kept: green means on, blue means link and focus, gold means it mentions you |
| Modern, not generated | 3: neutral grey + one accent is still the shape of tell 2; adds a glow | 4: every hue has one job; signature is specific. Risk: teal slate + orange is a known pairing |
| Own branding | 4 | 5: ember only for brand, heat and the one main action |
| Calm for long sessions | 4 | 4 |
| egui effort | 5: no schema change, integer type scale | 4: three new TOML keys, half-pixel sizes; icon set already rendered at 16/20/24 |
| Contrast | 4 | 3: danger text on menus is 3.4:1 |

**Base: B.** Taken from A: no new `ThemeColors` fields (tokens are derived in code), the integer
type scale without letter-spacing, radius tiers derived from one setting, concentric menus,
right-aligned group counts, modal motion. Revised by the judge:
- neutral hue 220 → 245 (blued steel), which avoids the teal-and-orange pairing
- dark floating layers = `surface`
- danger lightened and moved toward pink-red, so it reaches 4.5:1 on menus and stays clear of ember
- light idle darkened to 3:1
- heat gradient only on dark themes
- the activity art slot always shows something
- app icons come from the existing `FetchGames` path
- the heat-bar glow is removed (the accessory we drop)

---

## 1. Concept

**The metal is cold; only what needs you is hot.** Panes are blued steel. Accent hues come
from the colours steel shows as it is heated, and each hue has exactly one job:

| Hue | Token | Its only job |
|---|---|---|
| ember | `accent` | brand mark, the heat bar, one primary button per view, text caret |
| temper blue | `link` | links, mention and channel chips, keyboard focus ring, text selection |
| straw | `attention` | messages that mention me (bar and tint), idle status |
| red-hot | `danger` | destructive actions, count badges, "New" divider, Do not disturb, errors |
| green | `positive` | online, voice connected, speaking, switches and checkboxes on, my activity icon |
| temper purple | `live` | streaming status |

Principles:
1. **Discord's layout and rhythm, our material.** Identity comes from colour temperature, the
   heat bar and the icon drawing. No control moves.
2. **Structure is information.** Fill steps separate panes. A hairline appears only where two
   equal fills meet or rows form a list. One container level. Radius grows with container size.
3. **Colour always comes with a word or a shape.** A dot has its word in the tooltip; a switch
   knob shows a check or a cross; a muted mic is slashed.
4. **Quiet type.** One family. Hierarchy comes from weight before size. Sentence case, no tracking.
5. **Motion only answers an action**, and `settings.reduced_motion` (config.rs:128, §7) turns it off.

**Signature (the one bold element): the heat bar** (§10.1). The rail's selection pill glows like
metal fresh from the forge. Nothing else in the chrome uses a gradient or a glow.

---

## 2. Colour

### 2.1 `ThemeColors` values (src/theme/mod.rs; no new fields)

| Field | Ember Dark | Paper Light | High Contrast (unchanged) | Used for |
|---|---|---|---|---|
| `guild_bar` | `#151a1e` | `#dae0e6` | `#000000` | frame: title bar, rail, behind the channel card |
| `sidebar` | `#1c2125` | `#eaeef2` | `#000000` | channel card, member list, settings nav |
| `background` | `#23282c` | `#f9fbfd` | `#0a0a0a` | chat, Friends, settings pages |
| `message_hover` | `#262b30` | `#f0f3f6` | `#222222` | message hover |
| `surface` | `#2c3236` | `#e5eaef` | `#161616` | composer, embeds, user panel; dark: floating layers |
| `code_background` | `#181d21` | `#ecf0f3` | `#1c1c1c` | "well": code, text inputs, search, segmented track |
| `text` | `#ece9e4` | `#1b2127` | `#ffffff` | text (dark: forge-lit warm white) |
| `text_muted` | `#abb4bc` | `#525e68` | `#d6d6d6` | secondary text, icons at rest |
| `accent` | `#f68842` | `#bc4602` | `#ffd400` | ember (§1) |
| `accent_text` | `#21110a` | `#ffffff` | `#000000` | label on accent |
| `link` | `#81bfeb` | `#0068a7` | `#66e0ff` | temper blue (§1) |
| `danger` | `#fc6d6e` | `#cc2827` | `#ff5c5c` | red-hot text, icons, lines |
| `mention_highlight` | `#373732` | `#eee7da` | `#3a3300` | row tint of messages that mention me |

Why there are no new fields: `ThemeColors` is `#[serde(default)]` from Ember Dark. A new
non-optional field would give a light custom theme dark values, and state colours stay fixed per
mode, as in Discord. Other theme changes:
- `ThemeFont::default().size` 14.5 → **15**; High Contrast → **16**.
- `ThemeLayout::default().corner_radius` 6 → **8**; High Contrast stays 4.

### 2.2 Derived tokens: new `src/theme/tokens.rs`

`pub struct Tokens` built by `Tokens::new(c: &ThemeColors)`. Mode is HC if `is_high_contrast(c)`,
else light if the relative luminance of `background` > 0.4, else dark. Build every constant as
`Rgba` and convert with `to_color32` so the Saturation setting applies. `Pal` (ui/messages.rs:71)
carries the tokens. `presence_color` (ui/mod.rs:1355) and `theme/apply.rs` read them.

| Token | Dark | Light | HC | Used for |
|---|---|---|---|---|
| `raised` | = `surface` | `#ffffff` | = `surface` | menus, popouts, modals, tooltips, pickers, autocomplete |
| `hover` | `text` @ 6 % | `text` @ 6 % | `text` @ 12 % | row and button hover (translucent, so it works on gradients) |
| `selected` | `text` @ 10 % | `text` @ 8 % | `text` @ 18 % | selected row, selected segment, toggled icon button |
| `rule` | `text` @ 10 % | `text` @ 12 % | `text_muted` @ 45 % | hairlines (list rows, menu separators, float edges) |
| `edge` | `#899198` | `#7c858d` | `#adadad` | component boundaries ≥ 3:1: input border, switch-off track. Formula: mix(background, text_muted, 0.75); HC 0.8 |
| `focus` | = `link` | = `link` | = `accent` | focus ring |
| `positive` | `#50c76a` | `#1d8b3d` | `#4fe07a` | §1: dots, icons, switch track |
| `positive_text` | = `positive` | `#00792b` | = `positive` | green text ("Voice connected") |
| `on_positive` | `#151a1e` | `#ffffff` | `#000000` | check and knob glyph drawn on `positive` (switch on, checkbox on) |
| `positive_fill` | `#25803f`, white label | `#25803f`, white label | `#4fe07a`, black label | Join / Accept call buttons |
| `attention` | `#ebc166` | `#a86b00` | `#ffd400` | §1 |
| `live` | `#a682e1` | `#7a4aba` | `#c9a2ff` | §1 |
| `danger_fill` | `#cc3d3f`, white label | `#cc2827`, white label | `#ff5c5c`, black label | destructive buttons, badges, "New" and "Live" tags, danger menu hover |
| `chip` | `link` @ 16 % | `link` @ 12 % | `link` @ 22 % | mention and channel chips, app tags |
| `selection` | `link` @ 35 % | `link` @ 22 % | `link` @ 35 % | selected text (replaces orange) |
| `heat` | `[#a42d15, accent, #feeec1]` | `[accent; 3]` | `[accent; 3]` | heat bar: cold end, body, core |
| `scrim` | black 60 % | black 40 % | black 80 % | behind modals |
| `media_scrim` | black 88 % | black 88 % | black 95 % | the media lightbox backdrop (§10.3 Media) |
| `on_media` | white 92 % | white 92 % | white 92 % | text and icons on media and on its black 56 % discs |
| `on_media_muted` | white 64 % | white 64 % | white 64 % | secondary text on media (time, "2 of 3", "Esc") |
| `media_hover` | white 14 % | white 14 % | white 14 % | hover of icon buttons and discs on media |

Presence: online = `positive`, idle = `attention`, dnd = `danger`, offline = `text_muted`,
streaming = `live`. The shapes stay: dot, crescent, bar, phone.

### 2.3 Accent budget (enforced in review)

Ember may appear only on:
- the logo mark
- the heat bar
- one primary button per view
- the text caret

Never on:
- list selection, server tiles, channel icons
- menu items, switches, checkboxes, radios
- hover borders, mention chips
- secondary buttons, decoration

### 2.4 Contrast (WCAG 2 formula, computed for every pair below)

| Pair | Dark | Light | HC |
|---|---|---|---|
| text on chat / frame / raised | 12.3 / 14.5 / 10.7 | 15.7 / 12.2 / 16.2 | 19.8 / 21 / 18.1 |
| muted on chat / sidebar / surface / selected | 7.1 / 7.7 / 6.2 / 5.9 | 6.4 / 5.7 / 5.5 / 4.9 | 13.6 / 14.5 / 12.5 / 9.3 |
| accent on chat; accent_text on accent | 6.0; 7.4 | 5.0; 5.2 | 13.8; 14.7 |
| link on chat / chip; focus on sidebar | 7.5 / 5.4; 8.2 | 5.7 / 4.8; 5.1 | 12.9 / 8.0; 14.7 |
| danger on chat / raised | 5.4 / 4.7 | 5.2 / 5.4 | 6.5 / 6.0 |
| label on danger_fill / positive_fill | 4.9 / 5.0 | 5.4 / 5.0 | 6.9 / 12.3 (black) |
| switch / checkbox on: `positive` vs chat / surface (≥ 3); `on_positive` on it | 6.9 / 6.0; 8.1 | 4.2 / 3.6; 4.4 | ≥ 10; 12.3 |
| edge on surface / chat (≥ 3) | 4.1 / 4.7 | 3.1 / 3.6 | 8.1 |
| positive / attention dot on sidebar (≥ 3) | 7.5 / 9.6 | 3.7 / 3.8 | ≥ 12 |
| pane steps (frame : sidebar : chat) | 1.08, 1.09 | 1.14, 1.12 | |

The existing test `builtin_names_are_unique_and_default_is_ember` still holds.
`is_high_contrast` stays false for Ember Dark and Paper Light (muted/background 7.1 and 6.4,
both below 9).

### 2.5 Gradient and custom themes

`from_gradient`/`retint` (theme/gradient.rs:59-70) start from `ember_dark()`/`paper_light()`.
They pick up the new text and surface tints automatically and keep ember and danger.
Afterwards, re-run `presets_are_readable` and `flat_themes_keep_their_fills`.

Custom TOML themes work unchanged. Tokens are derived from their colours; a file without
`corner_radius` or `size` gets 8 / 15.

---

## 3. Type

**Fonts:**
- gg sans (runtime download) with weights 400, 500, 600, 700.
- Fallbacks are system fonts only, nothing bundled: Segoe UI for 400; **Segoe UI Semibold
  (`seguisb.ttf`) for 600**, which today falls back to Bold (apply.rs:170 `build_fonts`); Segoe UI
  Bold for 700. 500 falls back to 400.
- The tofu fallback chain is already in theme/fonts.rs `FALLBACK` (it starts with seguisym.ttf).

B = `theme.font.size` (default 15). Every role scales with it. Add helpers on `Pal`:
`badge() meta() label() ui() body() heading() title() display()`. They replace the `size - n`
literals.

| Role | Size | Weight | Line height | Used for |
|---|---|---|---|---|
| display | B+9 = 24 | 700 | 30 | login title, full-profile name, DM start name |
| title | B+5 = 20 | 600 | 26 | settings page title, modal title, popout display name |
| heading | B+1 = 16 | 600 | 22 | chat-header channel name, server header, settings section heading, activity name in cards |
| body | B = 15 | 400; names 500 | messages 1.375 × B = 21 (`TextFormat::line_height`), rows 20 | messages, channel / DM / member names |
| ui | B−1 = 14 | 500 buttons, tabs, menu items; 400 inputs | 20 | buttons, menus, inputs, tooltips |
| label | B−2 = 13 | 600, `text_muted` | 18 | categories, member groups, settings nav groups, field labels, card labels |
| meta | B−3 = 12 | 400 | 16 | timestamps, status and activity lines, hints, descriptions |
| badge | B−4 = 11 | 700 | 14 | counts, "New", "Live" |

Rules:
- Sentence case everywhere.
- No ALL CAPS and no letter-spacing.
- Never more than three weights on one screen.
- Only author and member names carry role colours. Headings never do.
- egui `TextStyle` (apply.rs:30-36): Body = B, Button = B−1, Small = B−3, Monospace = B−1,
  Heading = B+5 semibold.

---

## 4. Spacing and sizes

**Spacing scale:** 2, 4, 8, 12, 16, 24, 32, 48. No 10, 14 or 20.

**Sizes (Discord's):**

| Element | Size |
|---|---|
| title bar | 32 |
| rail | 72; tiles 40, 8 apart |
| channel card | 240 |
| chat header | 48 |
| channel, menu and settings-nav rows | 32 |
| category | 24, with 16 above |
| DM and member rows | 42 |
| setting row | min 56 |
| composer | min 44 |
| user panel | 56, 8 margin |
| icon buttons | 32 hit area, 20 icon |
| inputs | 40 (forms, login), 32 (search, filters) |
| settings | nav 218, content up to 740 |

**Messages:** avatar 40 at x 16, text column at x 72, 16 between groups.

## 5. Radius

r = `layout.corner_radius` (default 8). Tiers are derived; a theme with 0 stays square.

| Tier | Value (r = 8) | Elements |
|---|---|---|
| chip | r/2 = 4 | chips, tags, badges' rectangles, inline code, 20 px activity art, role chips |
| control | r = 8 | buttons, inputs, row fills, menu items, tooltips, segmented items, embeds |
| container | 1.5 r = 12 | channel card corner, user panel, composer, menus, popouts, pickers, activity containers |
| modal | 2 r = 16 | modals, profile popout, login panel |
| full | — | avatars, switches, status dots, count badges |

- Nested radius = outer − inset, minimum 2. Example: a menu at 12 with 4 padding has items at 8.
- `Pal::card_radius()` becomes `r * 3 / 2`; add `Pal::modal_radius()` = `2r`.
- `PANEL_R` (sidebar.rs:1731) goes to the container tier.
- Rail tiles keep their own morph (`ICON_R`).

## 6. Elevation

| Level | What | Fill | Edge | Shadow (`egui::Shadow`) |
|---|---|---|---|---|
| E0 | panes | frame / sidebar / chat | `rule` only where equal fills meet | none |
| E1 | composer, embeds, user panel, Active now containers | `surface` | none (inputs: `edge`) | none |
| E2 | menus, tooltips, popouts, pickers, autocomplete | `raised` | 1 px `rule` | offset [0,8], blur 24, black 45 % dark / 14 % light |
| E3 | modals, confirms, login panel | `raised` + `scrim` | 1 px `rule` | offset [0,16], blur 48, black 50 % / 18 % |

- HC: no shadows; floats get a 1.5 px `text_muted` border.
- In-flow cards never get a shadow.
- Set `popup_shadow` = E2 and `window_shadow` = E3 in apply.rs:90-91.

## 7. Motion

Every animation goes through `src/ui/motion.rs` (tokens and helpers) and follows the ux-motion
skill (`~/.claude/skills/ux-motion/SKILL.md`, 20 principles). Motion answers an action, never
loops on its own (spinners while loading excepted) and asks for frames only while it moves.

| Token | Open | Close | Used for |
|---|---|---|---|
| `FAST` | 120 ms | instant | menus, context menus, submenus, tooltips, popovers, pickers: fade + grow 0.96 → 1 from their trigger (egui `animation_time` = `FAST`, `motion::end_pass`); switch knob (stretches 15 % mid-travel), segmented selection, category chevron, rail pill and tile morph; speaking rings fade out in `FAST` |
| `BASE` | 200 ms | 100 ms | modals and confirms (`motion::modal`: scrim fade; panel fade + grow 0.96 → 1 from the click that opened it), the lightbox (media 0.92 → 1 from the click), toasts (fade + 8 px rise), my reaction's pop (1 → 1.12 → 1), quest cards after the skeletons (fade + 8 px rise, 40 ms apart) |
| `LARGE` | 300 ms | instant | settings layers (fade + 0.98 → 1), the title-bar update pill (fade + 8 px slide), the login error shake (3 × ±6 px, decaying) |

- Curves: entering ease-out (`motion::ease`, cubic-out), leaving the same curve backwards
  (ease-in), moves between two spots cubic in-out. Linear only for spinners and progress bars.
- Instant on purpose (frequency of use): hover colours, message rows, channel rows under a
  collapsing category, autocomplete, tab / channel / server switches, scrolling to a message,
  closing menus and popovers, the update screen, the captcha slot, the game overlay. A press
  draws the control 1 px smaller per side while held, no tween.
- `reduced_motion` (config.rs:128) sets every duration to 0 and egui's scroll animation to none;
  static states (heat colours) stay; typing dots and call rings stop pulsing. On Windows it is
  also on while the system's "Animation effects" are off (`motion::os_reduced`, read once).
- `RUSTCORD_MOTION_SCALE=<f>` multiplies every duration (10: screenshots mid-animation).

---|---|
| heat bar and pill | 150 ms (existing `ANIM`) |
| switch knob, segmented selection, category chevron (0 ↔ −90°) | 120 ms |
| modal open | 140 ms fade + scale 0.97 → 1 |
| menu and popout open | 100 ms fade |

- No hover tweens: hover changes colour instantly.
- Repaints only through `request_repaint_after`.
- `reduced_motion` sets every duration to 0. Static states, including the heat colours, stay.

---

## 8. Icons: new `src/ui/icons.rs`

Our own vector set, drawn from simple geometry. **Never copy Discord's SVG paths or files.**

### 8.1 API

- `pub(crate) enum Icon { … }` (the lists in §8.3–8.5).
- `pub(crate) fn paint(p: &Painter, icon: Icon, rect: Rect, color: Color32, bg: Color32)`.
  `bg` paints cut-outs and badge rings, so an icon sits on any fill.
- `pub(crate) fn paint_badge(p, badge: Badge, rect, color, bg)`, with `Badge { Lock, Alert }`.
- `pub(crate) fn channel(kind: u8, locked: bool, nsfw: bool, rules: bool) -> (Icon, Option<Badge>)`.
  It is the single source for the channel list, chat header, autocomplete, inbox, Create channel,
  thread header and forum.
- `pub(crate) fn from_glyph(&str) -> Option<Icon>` is the migration shortcut: `icon_button`
  (messages.rs:130), `MenuIcon::Glyph` (messages.rs:150), `icon_line` (ui/mod.rs:1386) and
  `empty_state` (user_settings.rs:494) look the glyph up and paint the vector. That switches every
  call site in one edit. Comment it `shortcut: glyph keys stay in call sites, replace with Icon
  variants when a file is touched`. Unknown glyphs fall back to text.
- `paint_icon` and `sidebar::Icon` move here, along with `paint_activity_icon`,
  `paint_megaphone`, `paint_people`, `paint_phone`, both `paint_bell`s, `paint_camera`,
  `paint_clock`, `paint_star` and `MenuIcon::Copy`/`Link`. The two `channel_glyph` functions
  (sidebar.rs:1005, server_menu.rs:881), `channel_prefix` (messages.rs:348) and
  `threads::header_glyph` are deleted.

### 8.2 Drawing rules

- **Grid:** 24 units with a 2-unit margin (live area 20). Scale = width / 24. Snap the rect with
  `GuiRounding::round_to_pixels`.
- **Sizes:** 16 (menus, inline, chips), 20 (rows, header, composer, user panel, icon buttons),
  24 (call controls), 48 (empty states).
- **Stroke:** 2 units, at least 1.5 physical px. epaint has no round caps, so draw a dot of
  stroke/2 at every open end.
- **Arcs:** 12–16-segment polylines.
- **Fills:** filled shapes are unions of convex polygons (`convex_polygon` is convex-only).
- **Solid objects are filled:** speaker, mic, headphone cups, person, bubble, bell, gear, pin,
  lock, folder, controller, camera, trophy, megaphone.
- **Line symbols are stroked:** hash, waves, chevrons, ×, +, check, arrows, magnifier, pencil,
  frames.
- **State:** shown by colour and the "off" slash, never by swapping fill and outline. The slash is
  a 2-unit stroke over a 2.5-unit `bg` gap (as `paint_icon` does today).
- **Badges:** a 12-unit box at the bottom right with a 1.5-unit `bg` ring. At 16 px, drop
  details under 3 units (the hash inside Thread, the "!" in Alert).
- **Colour:** always the context's `text` or `text_muted`. State colours only for state
  (`positive` speaking or connected or my activity, `danger` muted, deafened, hang-up or delete).
  Multicolour only for status and the logo.
- **Exceptions:** GIF and Aa stay text badges, since they are words.

### 8.3 Key geometry (24-unit grid)

| Icon | Geometry |
|---|---|
| `Text` (#) | Strokes (10.5,3.5)→(7.5,20.5), (16.5,3.5)→(13.5,20.5), (4.5,8.5)→(20.5,8.5), (3.5,15.5)→(19.5,15.5). A 10° slant; the lower bar sits 1 unit left with the slant. |
| `Voice` | Body rect (3,9)–(7.5,15) r1. Cone quad (7,9)(12,4.5)(12,19.5)(7,15). Waves: arcs about (12.5,12), r 4.25 over ±45° and r 8 over ±48°. |
| `Announcement` | Quad (4.5,9)(17,4.25)(17,19.75)(4.5,15). Mouth cap rect (2.5,9)+(2×6) r1. Handle (7,15.5)→(8.5,20). Short stroke (20.5,10)→(20.5,14). |
| `Stage` | Head dot r3.25 at (12,8.75). Shoulders: half-disc (6.5,14)–(17.5,20). Arcs r7.5 about (12,10) at ±35° on both sides. |
| `Forum` | Back bubble outline (2,3)+(13×10) r3.5 with a tail at bottom left. Front bubble filled (9.5,10)+(12.5×9) r3 with a tail at bottom right. 1.5-unit `bg` gap. |
| `Media` | Outline (3,4)+(18×16) r3. Filled ridge (6,17.25)(10.5,11.5)(13.75,15)(15.75,12.75)(18.5,17.25). Sun dot r1.75 at (16,8.5). |
| `Thread` | Bubble outline (2.5,3.5)+(19×13.5) r4 with a tail at bottom left. Small hash inside (dropped at 16 px). |
| `Rules` | Page outline (5,3)–(19,21) r2. Check stroke (8.5,12.5)→(11,15)→(15.5,9.5). |
| `Directory` / `Folder` | Filled body (2.5,7)–(21.5,19.5) r2. Tab (2.5,4.5)–(10,7.5) r1.5. |
| `Badge::Lock` | Body (14,17)–(22.5,23.5) r1.5. Shackle arc r2.6 about (18.25,17), stroke 1.75. On `Voice` it replaces the outer wave. |
| `Badge::Alert` (NSFW / age-restricted) | Filled triangle (17.5,12.5)(23,22.5)(12,22.5). Knocked-out bar (17.5,15.5)→(17.5,19) and dot r0.9 at (17.5,21). |
| `Playing` | Body (2.5,7)+(19×10.5) r5.25. Grips r3.6 at (6.5,15.75) and (17.5,15.75). Cut-outs: d-pad strokes 1.75 wide; buttons r1.3 at (15.5,11.25) and (17.9,13.6). |
| `Listening` | Note heads 6×5 at (3.75,15) and (13.75,13). Stems x 9.25 and 19.25. Beam (8.25,5.25)(20.25,3.25)(20.25,7)(8.25,9). |
| `Streaming` | Dot r2.25 at the centre. Arcs r5.5 and r9.5 over 140°..220° and −40°..40°. |
| `Watching` | Almond from two curves (2.5,12)…(21.5,12), control y 5 and y 19. Pupil r3.25. |
| `Competing` | Cup half-disc (6.5,−2)+(11×16). Handles r3. Stem (12,14)→(12,18). Base (7.5,18)+(9×3) r1. |
| `Mic` | Capsule (8.5,2.5)+(7×12) r3.5. Lower arc r6.5 about (12,11). Stem (12,17.5)→(12,21). Foot 8.5..15.5. |
| `Headphones` | Upper arc r8.5 about (12,13). Cups (2.5,12.5)+(5×8.5) r2 and (16.5,12.5)+(5×8.5) r2. |
| `Gear` | 8 teeth 4×5 r1 at radius 10, rotated 45° apart. Body r7. Hole r2.75 in `bg`. Replaces the "⚙" glyph (sidebar.rs:1300). |
| `Members` | Front bust (head r3.5 at (9,8.5), shoulders) over a back bust offset (+7,−1), with a 1.5-unit `bg` gap. |
| `Inbox` | Tray outline (3,13)(5.5,4.5)(18.5,4.5)(21,13)(21,19.5)(3,19.5). Notched rim through (8,13)(9.5,15.75)(14.5,15.75)(16,13). |
| `Pin` | Rotated 45°: head (8,2.5)+(8×3) r1, tapered body, collar (5.5,12.5)+(13×2.5), needle (12,15)→(12,21.5). |

The rest are drawn on the same rules from these shapes:
- `At`: ring r3.5 plus an outer arc r8.5 open at the lower right.
- `Close`: two diagonals 6→18.
- `Plus`: a cross 5→19.
- `Check`: (5,12.5)→(10,17)→(19,7).
- `Search`: ring r6 at (10.5,10.5) plus a handle to (20,20).
- `Edit`: a pencil at 45° with a tip triangle.
- `Trash`: lid bar, handle and tapered can with two slits.
- `Reply`: a curved arrow left.
- `Copy`: two offset sheets (existing).
- `Link`: two rounded links (existing).
- `IdCard`: card outline, a disc and two bars, no letters.
- `Person`: head and shoulders.
- `PersonAdd` / `PersonRemove`: person plus a + / − badge.
- `Leave`: door frame plus an arrow out.
- `Block`: ring plus a diagonal.
- `Shield`: shield outline.
- `Ban`: a hammer.
- `Bell` / `BellOff`.
- `Archive`: box with lid and slot.
- `Envelope`.
- `SplitView`: frame with a vertical divider.
- `File`: page with a folded corner.
- `Image`.
- `Attach`: paperclip.
- `External`: square plus an arrow out of the top-right corner.
- `ArrowLeft` / `ArrowRight`.
- `Refresh` (clockwise) / `Reset` (counter-clockwise).
- `Hourglass`.
- `Key`.
- `Desktop`.
- `Card`: a rectangle with a stripe.
- `Video`: a frame with a play triangle.
- `Grip`: 2×3 dots.
- `Sort`: up and down arrows.
- `Minus`.
- `Chevron{Up,Down,Left,Right}`.
- `More`: three dots.
- `Smile`.
- `Eye` / `EyeOff`.
- `Lock` / `Unlock`.
- `Boost`: a faceted gem.
- Server marks (`icons::guild_mark`, one before a server's name: verified, else partnered, else
  the boost level; monochrome in the context's `text`, §8.2):
  - `Verified`: a seal (disc r 7.75 at (12,12) plus 12 discs r 2.25 on radius 7.75, every
    30°) with a check cut in `bg`, stroke 2, (8,12.25)→(10.75,15)→(16.25,9.25).
  - `Partnered`: the same seal; cut: two rings r 2.75, stroke 1.75, at (10.25,12) and
    (13.75,12); under 18 px one ring r 3.75.
  - `BoostTier1`: a solid gem (7.5,4)(16.5,4)(20.5,9)(12,20.5)(3.5,9), its girdle
    (3.5,9)→(20.5,9) cut, stroke 1.5.
  - `BoostTier2`: ring r 9 around a small solid gem (9.5,7.5)(14.5,7.5)(16.75,10.25)(12,16.75)
    (7.25,10.25).
  - `BoostTier3`: a disc r 10 with that small gem cut out.
- Quests (`ui/quests.rs`, the DM list's Quests row):
  - `Quest`: a medal, filled: disc r 5.5 at (12,15.5) under two crossing ribbon quads
    (6.5,2.5)(10.5,2.5)(13,10.5)(9.5,11.5) and its mirror about x 12, a 1.5-unit `bg` gap
    between ribbons and disc.
  - `Orb`: disc r 8.5 at (12,12) with a `bg` crescent highlight (disc r 3 at (9,9) minus disc
    r 3 at (10.25,10.25)).
  - `Gift`: box (3.5,11)–(20.5,21) r 1.5 and lid (2.5,7.5)–(21.5,11) r 1, filled, a `bg` gap
    x 11.25..12.75 through both; bow loops are rings r 2.75 at (9.25,5) and (14.75,5),
    stroke 1.75.
- `Camera` / `CameraOff`.
- `Screen`: monitor plus an up arrow.
- `Hangup`.
- `Phone`.
- `MicOff`.
- `Deafened`.
- Emoji-picker categories:
  - `Leaf`
  - `Food`: an apple
  - `Ball`: circle with seams
  - `Car`
  - `Bulb`
  - `Heart`
  - `Flag`
  - plus the existing `Clock` and `Star`
- Media (ui/viewer.rs):
  - `Play`: filled triangle (7,4.5)(19.5,12)(7,19.5)
  - `Pause`: filled rrects (6,4.5)–(10,19.5) and (14,4.5)–(18,19.5), r1
  - `Download`: stroke (12,3.5)→(12,15); head (7.5,10.5)→(12,15)→(16.5,10.5); tray (4,16)→(4,20)→(20,20)→(20,16)
  - `VoiceOff`: `Voice` with the off slash (`Voice.off()`)
  - `Expand`: four outward L corners, legs 5.5 units, at the 3.5 / 20.5 lines
  - `Collapse`: four inward L corners meeting at (9,9), (15,9), (15,15), (9,15)

### 8.4 Channel icon matrix

| Channel | Icon | Private (@everyone denied VIEW_CHANNEL) | Age-restricted (`nsfw`) |
|---|---|---|---|
| text (0) | `Text` | `Text` + Lock | `Text` + Alert |
| rules channel (id = `guild.rules_channel_id`) | `Rules` | `Rules` + Lock | — |
| announcement (5) | `Announcement` | + Lock | + Alert |
| voice (2) | `Voice` | Lock replaces the outer wave | + Alert |
| stage (13) | `Stage` | + Lock | — |
| forum (15) | `Forum` | + Lock | + Alert |
| media (16) | `Media` | + Lock | + Alert |
| thread (10, 11) | `Thread` | private thread (12): `Thread` + Lock | — |
| directory (14) | `Directory` | — | — |
| category (4) | `ChevronDown`, 12 px; rotates −90° when collapsed | — | — |
| DM (1) / group DM (3) | avatar in lists; header `At` / `Members` | — | — |
| hidden channel (ShowHiddenChannels plugin) | base icon + Lock, row muted | | |

Colour follows the row text: muted at rest, `text` on hover, selected or unread. Never accent.
A voice channel I am connected to keeps the existing 2 px `positive` bar.
`Guild.rules_channel_id: Option<Id>` must be added to model.rs `Guild` (serde default). This is
an additive contract change; report it.

### 8.5 Glyph replacement table (every emoji or glyph icon in `src/`)

| Glyph now | Icon | Sites |
|---|---|---|
| × ✖ | `Close` | `icon_button` × (14 calls), sidebar.rs:1583, friends.rs:613, plugins/windows.rs:147 |
| ✏ | `Edit` | composer.rs:972, friends.rs:644, messages.rs:2158,2301, server_menu.rs:343, server_settings.rs:688, user_settings.rs:521, security.rs:221, status.rs:333 |
| ✔ | `Check` | cosmetics.rs:969, friends.rs:608, inbox.rs:88,293, profile.rs:533, server_menu.rs:291,509,1133, server_settings.rs:486,922, status.rs:299, user_settings/cosmetics.rs:508, theme_editor.rs:264, security.rs:185, plugins/windows.rs:147 |
| + ➕ | `Plus`; `PersonAdd` where a person is added (friends.rs:617,641, profile.rs:541) | server_menu.rs:326,524, threads.rs:179, `icon_button` "+" |
| ➖ 👢 | `PersonRemove` | friends.rs:603 (remove friend), server_menu.rs:561 (kick) |
| 🗑 | `Trash` | composer.rs:985, messages.rs:2336, server_menu.rs:497, server_settings.rs:692, connections.rs:70, security.rs:217 |
| 💬 | `Message` (chat with someone); `Thread` (threads.rs:305,513, forum.rs:450, composer.rs:287); `Forum` (channel kind) | friends.rs:571, call.rs:352, messages.rs:2310 |
| ⟳ / ↺ | `Refresh` / `Reset` | screen_share.rs:55, user_settings/voice.rs:88 / keybinds.rs:137 |
| ⚙ | `Gear` | sidebar.rs:1300,1968, server_menu.rs:315,494,1003, plugins.rs:112 |
| ☺ 🙂 | `Smile` | composer.rs:264, messages.rs:230,2167,2292, status.rs:106 |
| ‹ › | `ChevronLeft` / `ChevronRight` | pickers.rs:779, search.rs:264,268, server_settings.rs:597,828, threads.rs:93, server_menu.rs:1095 |
| 🔒 🔓 | `Lock` / `Unlock` | pickers.rs:756, server_menu.rs:853, threads.rs:319,589, lists.rs:214 / friends.rs:621 |
| 📥 📌 👥 👪 👤 | `Inbox`, `Pin`, `Members`, `Members`, `Person` | inbox.rs:47; forum.rs:370, messages.rs:2318,2634, pins.rs:30; messages.rs:916; user_settings.rs:429; profile.rs:567, server_menu.rs:306,484, server_settings.rs:642,1158, sidebar.rs:440,1578 |
| 👁 ⌛ ↩ | `Eye`, `Hourglass`, `Reply` | composer.rs:969; profile.rs:538; messages.rs:2162,2304 |
| 🆔 | `IdCard` | 10 "Copy … ID" items (messages.rs:2346, profile_modal.rs:234, server_menu.rs:254,283,534, server_settings.rs:701,1167, sidebar.rs:463, …) |
| 🚪 🚫 🛡 🗄 🔨 🔔 | `Leave`, `Block`, `Shield`, `Archive`, `Ban`, `Bell` | friends.rs:648, server_menu.rs:430, threads.rs:182; friends.rs:624, messages.rs:2330; server_menu.rs:336; threads.rs:186,526; server_menu.rs:564; server_menu.rs:686 |
| 🔊 🎙 📢 🖼 📁 🧵 | channel matrix §8.4 | sidebar.rs:1007-1011,1145, server_menu.rs:883-885, autocomplete.rs:199-202, call.rs:340, threads.rs:77-80 |
| 📞 | `Phone`; Disconnect → `Hangup` in `danger` | sidebar.rs:1142, messages.rs:2633 |
| 📎 📄 🔍 📷 | `Attach`, `File`, `Search`, `Camera` | composer.rs:286,746, messages.rs:329; composer.rs:886,947, messages.rs:1696,2085; search.rs:78, forum.rs:126, server_settings.rs:432; server_menu.rs:1141 |
| ✉ ⊞ @ | `Envelope`, `SplitView`, `At` | messages.rs:2323; threads.rs:167; inbox tabs, autocomplete roles |
| ↗ | `External`, after the label | cosmetics.rs:972, user_settings.rs:483, security.rs:527 |
| → ← 🚀 • | `ArrowRight`, `ArrowLeft`, `Boost`, dot shape | `system_icon` messages.rs:2629-2637 |
| 🏠 | `Plus` at 24 | server_menu.rs:1066 `big_choice` |
| 🔑 🖥 💳 🎮 🎬 🔌 | `Key`, `Desktop`, `Card`, `Playing`, `Video`, `Link` (48 px empty states) | lists.rs:128,182, security.rs:214, user_settings.rs:430-433, connections.rs:34 |
| ⠿ ↕ − | `Grip`, `Sort`, `Minus` | server_settings.rs:635, forum.rs:136, app.rs:66, keybinds.rs:164, theme_editor.rs:81 |
| 🙂 🌿 🍔 ⚽ 🚗 💡 ❤ 🏁 | `Smile`, `Leaf`, `Food`, `Ball`, `Car`, `Bulb`, `Heart`, `Flag` | pickers.rs:50 `CATEGORY_ICONS` |

Glyphs inside strings lose the glyph and get a painted 14 px icon in front:
- "🔍 Search posts" becomes "Search posts"
- "👤 n" becomes Person + n
- "📎 file" becomes Attach + file
- "↕ Sort & View" becomes Sort + "Sort and view"
- "🗄 This thread is archived" becomes Archive + the text

**Not icons, leave them:**
- emoji in message content, reactions, samples and demo data (👍 ❤ 🔥 🎉 😀 🦀 👀)
- markdown list bullets • ◦
- ∞ as a value
- the GIF and Aa badges
- CJK language names
- window controls in chrome.rs

---

## 9. Components

**Buttons**
- Height 32 (40 in modal footers and on login; 24 inline). Padding 16, minimum width 64.
- Label: ui role 14/500, sentence case, verb first. Radius control.

| Variant | Rest | Hover / pressed | Disabled | Use |
|---|---|---|---|---|
| Primary | `accent`, `accent_text` | mix white 8 % / mix black 10 % | `surface` fill, `text_muted` (never dim orange) | one per view: Sign in, Save changes, Create channel, Message, Add friend |
| Secondary | `selected` | +4 % / +8 % text | text at 45 % | Change password, Watch, Retry, empty-state actions |
| Danger | `danger_fill`, its label | mix black 10 % / 18 % | as secondary | destructive confirms only |
| Positive | `positive_fill`, its label | same | as secondary | Join call, Accept call |
| Ghost | no fill, `text` | `hover` / `selected` | `text_muted` 60 % | Cancel |
| Icon | 32 hit area, 20 icon `text_muted` | icon `text` + `hover` fill, radius control | 40 % | toolbars; toggled on = icon `text` + `selected` fill, never accent |

**Focus ring**
- 2 px `focus` at a 2 px offset, radius = element radius + 2. Inset by 2 on full-bleed rows.
- Shown on keyboard focus.
- Every custom-painted control needs one: rail tiles, channel, DM and member rows, `icon_button`,
  menu items, switches.

**Inputs**
- Fill `code_background` (well), 1 px `edge` border, radius control, padding 12.
- Placeholder `text_muted`. Hover: border `text_muted`. Focus: the 2 px focus ring replaces the
  border.
- Error: `danger` border, and below it a 12 px `danger` line with `Alert` saying what is wrong and
  what to do.
- Labels: label role, 8 above the field.
- The search field has a `Search` icon inside, at the left.

**Switch**
- Track 40×24, pill. Knob 18.
- Off: track `edge`; knob `text` (dark) or white (light) with a 1.5 px × in the track colour.
- On: track `positive`; knob `on_positive` with a check in `positive`.
- Slides in `FAST`, cubic in-out; the knob stretches 15 % mid-travel (§7).

**Checkbox and radio**
- Checkbox 20, radius chip: off has a 1.5 px `edge` border; on is `positive` with an
  `on_positive` check.
- Radio 20: off is a 1.5 px `edge` ring; on is a 2 px `text` ring with an 8 px `text` dot.

**Segmented control** (login tabs, plugin filter, status picker, Friends tabs)
- Track `code_background`, radius control, padding 2.
- Segment 32 tall, radius r−2.
- Selected segment: `raised` (light) or `selected` (dark), label 500 `text`. Others: `text_muted`. The selection slides in `FAST`.

**Menus**
- E2, radius container, padding 4, 188–320 wide.
- Items 32 tall, radius r−4 = 8 (concentric). Label ui 14/500 on the left; icon 16 `text_muted`
  on the right.
- Hover or keyboard focus: `hover` fill and `text` label.
- Danger item: `danger` label and icon; on hover `danger_fill` with its label colour.
- Separator: 1 px `rule`, inset 8, 4 above and below. A submenu shows `ChevronRight`.
- Toggle items: checkbox on the right.

**Tooltips**
- Dark: `raised` with a `rule` edge. Light: inverted, `text` fill with white label.
- ui 14/500, radius control, padding 6×10, max width 240, E2. Delay 0.3 s
  (`style.interaction.tooltip_delay`).
- The text names the action; the shortcut follows in `text_muted` ("Mute", then "Ctrl+Shift+M").

**Modals** (ui/mod.rs:1100 `dialog`, :1146 `dialog_buttons`, :1162 `dialog_footer`)
- E3, widths 440 (confirm) / 520 (form) / 640 (wide), radius modal, padding 24.
- Title role on the left; `Close` icon button 32 at the top right.
- Footer right-aligned, 12 gap: Ghost "Cancel" plus one primary (or danger). No grey band.
- A confirm asks its question in the title and states the consequence in the body.
- Option lists, such as Create channel's type, are radio rows: icon 20, label 15/500, one-line
  meta description, radio. They sit in one `surface` group with `rule` separators, not separate
  cards.

**Chips, tags, badges**

| Kind | Look |
|---|---|
| mention / channel chip in text | `chip` fill, `link` text, radius chip, padding 0×2; hover `link` @ 28 % |
| role chip | 22 tall, radius chip, `surface` fill, 8 px colour dot (1 px `edge` ring if the dot is < 2:1 on the fill), meta 12/500. Wraps per chip; a long name ends in "…". |
| count badge | `danger_fill`, badge 11/700, pill, 2 px ring in the parent fill |
| "New" tag, "Live" tag | `danger_fill`, badge role, radius chip, sentence case |
| app / server tag | `chip` fill, `link` badge text, radius chip. Never accent. |
| composer state | dot + word ("Offline"), meta, `text_muted` on `selected`, pill |

**Empty states**
- Centred at 30 % height.
- A 48 px icon in `text_muted`, a heading line saying what is missing, one meta sentence saying
  what to do, and at most one secondary button that does it.

---

## 10. Main window

```
+-frame--------------------------------------------------------------------------[inbox][-][o][x]+
|    | Rust Workshop        v |  [#] general | Talk about anything Rust     [thr][pin][mem][Search]|
|[R] |------------------------|-----------------------------------------------+------------------|
|----| v Text                 |  (F) Ferris  10.11.2026 14:14                 | Maintainers    2 |
|#RW | [#] general <selected> |      Nice. I'm building a tiny chat client…   | (F)o Ferris      |
| PG | [#] help    (unread)   || (A) Alice   <attention bar + mention tint>   | (A)) Alice       |
| NO | [mg] showcase          ||     [@Ferris] that's awesome, [#showcase]    |      Reviewing PRs|
| +  | v Voice                | ------------------------------------- [New] | Online         2 |
|    | [spk] Lounge           |  (B) Bob                                      | (B)- Bob   [art] |
|    |                        |      Release notes                            |  [pad] Playing.. |
|    | +-user panel---------+ | +-composer (surface, r12)-----------------+   | (C)o Cara  [art] |
|    | |(F)o Ferris [m][h][g]| | | (+) Message #general     [GIF] [:)]    |   |  [note] Listen.. |
|    | |  [pad] Factorio | ..| | +-----------------------------------------+   |                  |
|    | +--------------------+ |                                               |                  |
+----+------------------------+-----------------------------------------------+------------------+
 #RW = heat bar on the selected server (4 px: cold ends, ember body, white-hot core)
```

### 10.1 Rail and heat bar (signature)

**Pill** (`decorate`, sidebar.rs:302):
- Height eases to 8 (unread), 20 (hover) and 40 (selected), 4 px wide, radius ne/se 4 (as now).
- Up to 20 px tall it is `text` (white), as on Discord.
- From 20 to 40, t = (h−20)/20, and each colour lerps from `text` to its heat stop:
  1. Paint the pill in `heat[0]`.
  2. Over [top+3, bottom−3], paint one vertex-coloured `Mesh` with stops at 0, 0.3, 0.5, 0.7
     and 1: `heat[0]`, `heat[1]`, `heat[2]`, `heat[1]`, `heat[0]`.
- It heats on select and cools on leave through `FAST` (§7).
- Light and HC themes use a solid accent (all stops = accent).
- No glow, no shadow.

**Tiles:**
- Initials tiles become neutral: `surface` fill with 600 `text` initials. Drop the hashed
  `initial_colors` and the accent lerp (sidebar.rs:489-491).
- Hover and selected: tile fill `selected`; image icons keep their colours.
- Home tile: the logo mark in `accent` on `surface`, always. Drop the accent fill (sidebar.rs:471).
- Add server: `Plus` in `text_muted` on `surface`, `positive` on hover.
- Mention badge: as now (danger_fill, ring in the rail colour).

### 10.2 Channel card

**Server header:** 48 tall, name heading 16/600, `ChevronDown`.

**Category row:** chevron 12 plus the name in the label role, `text_muted`; hover `text`.

**Channel row:**
- 32 tall, 2 apart. Icon 20 at x+8; name at x+36 in body 15/500.
- At rest: `text_muted`. Hover: `hover` fill. Selected: `selected` fill, `text` name and icon.
- Unread: `text` 600 plus a 4×8 `text` pill at the card edge.
- Muted channel: 45 % opacity.
- Hover actions (Invite, Gear) are 16 px in `text_muted`.
- The accent on the selected channel's icon goes (sidebar.rs:1090).

**Voice users:** avatar 24, name 14, speaking shown as a 2 px `positive` ring.

### 10.3 Chat

**Header:**
- Channel icon 20 (§8.4) and name heading 16/600; a 1×20 `rule` divider; topic in ui role
  `text_muted`, one line.
- Right side: Threads, Pin, Members, then, 8 px on, a 32 px search well: `code_background`, no
  border at rest (gradient themes: a `rule` edge), the focus ring while focused.
- The Members toggle uses the icon-button on state.

**Messages:**
- Avatar 40, author body 15/600 in the role colour, time meta.
- Body 15 with line height 21. 16 between groups. Compact mode unchanged.

**Mentions of me:** `mention_highlight` row fill plus a 2 px `attention` bar on the left
(not accent).

**Unread divider:** 1 px `danger` line with the "New" tag on the right.

**Hover toolbar:** E2, icon buttons 32.

**Reactions:**
- Pill 24 tall, `surface` fill, count meta.
- Mine: `chip` fill with a 1 px `link` border.

**Embeds:** `surface`, a 4 px left bar in the embed colour, radius control.

**Code:** `code_background`, mono B−1, radius control.

**Media** (ui/viewer.rs, docs/MEDIA_DESIGN.md):
- Images and GIFs keep their look; a click opens the lightbox (never the browser). GIF uploads
  move like gifv: always with "Play animated GIFs" on, else while hovered.
- Video uploads and Discord-proxied video embeds play in place, fitted into 420 × 320, radius
  control: the proxy's still (else black, or the first frame when the file is on disk), a 48 px
  black 56 % disc with `Play` 24 (`Refresh` "Replay" at the end, a spinner while loading),
  `Download` disc 32 top right on hover. Controls bar 40 on a black 0 → 64 % gradient, shown on
  hover and while paused after a start, hidden 2 s after the pointer stops while playing:
  `Play`/`Pause`, "0:12 / 1:03" meta `on_media`, seek bar (4 px track `on_media` @ 30 %,
  played part `on_media`, 12 px knob on hover; ←/→ 5 s), `Voice` / `VoiceOff` (hover reveals a
  64 px volume slider; session only), `Expand` "Full screen" (the lightbox).
- gifv: muted, looping, no controls; plays with "Play animated GIFs" on while the window is
  focused, or while hovered; at rest a "GIF" badge (badge 11/700, chip radius, black 56 %).
- Can't-play card in the player's place: `surface`, `rule` edge, meta `text_muted` text and
  secondary 32 buttons: "Rustcord can't play this video format." [Open in browser] [Download];
  "This video is too large to play here."; "Video didn't load. Check your connection, then
  select Retry." [Retry]; "This video is no longer available." [Open in browser].
- Lightbox (`Modal::Media`) under the title bar, which stays usable: `media_scrim`; top bar 56
  (avatar 32, name body 15/600 `on_media`, time `on_media_muted`; `Download`, `Copy` "Copy
  image", `Link` "Copy link", `Close` with "Esc"); arrows 40 discs 16 in from the sides when
  there is more than one item; the media fitted, never upscaled, 72 px from the sides and 56 from
  the bars; bottom bar "Open in browser" + `External` 16 (the only way to the browser) and
  "2 of 3". Images zoom (click fit ↔ 1:1, wheel up to 2×, drag pans); opens and closes per §7. No accent anywhere on media.

**Composer:**
- E1 `surface`, radius container, min 44, no border.
- `Plus` (attach menu) on the left. On the right: GIF badge, `Smile`, state chip.
- The caret is `accent`.

### 10.4 Member list and DM list

**Group header:**
- Label role "Maintainers" on the left. The count is meta `text_muted`, right-aligned, so counts
  form a column.
- No em dash (members.rs:127 `group_row`).

**Row:** 42 tall, avatar 32 with status, name body 15/500 in the role colour, line 2 meta
`text_muted`.

**Activities in rows (this fixes "no icons for some activities and music"):**

```
+---------------------------------------------------------+
| (B)o  Bob                                        [art]  |  art: 20x20, radius chip
|       [pad] Playing Factorio                            |  line 2: 14 px type icon + verb + name in 500
+---------------------------------------------------------+
```

- Line 2 shows the custom status (emoji + text) if one is set, else the activity sentence:
  - "Playing **Factorio**"
  - "Listening to **Spotify**"
  - "Live on **Twitch**"
  - "Watching **YouTube**"
  - "Competing in **…**"
- The **art slot always shows the activity**, so both stay visible. In order:
  1. `large_image`
  2. Spotify album art
  3. the application icon (§11.1)
  4. the type icon 16 in `text_muted` on a 20 px `hover` tile
- Go Live (screen share): a "Live" tag instead of art.
- Twitch or YouTube streaming: `Streaming` in `live`.
- Tooltip on the art: "Playing Factorio for 47 min" / "Listening to Ferris Song by Crabs".
- Offline rows: 45 % opacity, no line 2, no art.
- The same applies to DM rows, Friends rows and the Active now header lines.

### 10.5 User panel ("show my activity like Discord", reference 3.png)

```
+------------------------------------------------------------+
| (F)o  Ferris                        [mic]v  [hp]v  [gear]  |
|       [pad] Factorio | (crab) Rewriting it in Rust         |
+------------------------------------------------------------+
```

**Container:** E1 `surface`, radius container, 56 tall, 8 margin. Name 14/600.

**Line 2 (meta):**

| Situation | Line 2 |
|---|---|
| My activity is shown (detected game or rich presence, while my presence carries it) | `Playing`/`Listening`/… icon 14 in `positive`, then the activity name in `text_muted` |
| …and a custom status is set | then a 1×10 `rule` divider with 6 px gaps, then emoji + text. The status is cut first. |
| Custom status only | emoji + text |
| Neither | presence word: "Online", "Idle", "Do not disturb", "Invisible", or the connection word |
| Panel hovered | username (as now) |

- Tooltip on line 2: "Playing Factorio for 47 min".
- Repaint once a minute only while an activity is shown.
- No new traffic: it shows what my presence already sends.
- Hook: `my_line` (sidebar.rs:2010) gets my activity instead of `None` for `status_text`.

**Controls:**
- `Mic` / `Headphones` with a 12 px chevron (split buttons) and `Gear`.
- Muted or deafened: the slashed icon in `danger` on a `danger` @ 12 % tint. Tooltip "Unmute" /
  "Undeafen".

**Voice section** above the panel:
- "Voice connected" in label role, `positive_text`.
- Channel and server in meta.
- `Hangup` in `danger`.

**Quest video section** above the voice section (while a quest video session exists), 56 tall plus
a hairline:
- "Quest video" in label role, `positive_text`.
- The quest name in meta (a link, `text_muted`, `text` on hover; opens the video).
- Mute / Unmute (only if the video has sound) and Stop video (`Close`), 32 px, tinted on hover,
  `text_muted` icons.
- A 3 px progress bar under the row (`paint_bar`).

### 10.6 Title bar

- Keep: one bar, 32 tall, on frame.
- The location is centred: a 16 px icon (channel icon or server tile) and the name in ui 14/600.
- `Inbox` icon button on the right, then the window controls (chrome.rs, unchanged).
- Update button (ui/update.rs, docs/UPDATER_DESIGN.md §15.2), only while a new version is
  available or ready: a pill 24 tall, 8 left of `Inbox` (8 from the end where there is no
  inbox: login, settings), radius control, padding 8. `Download` 16 in `positive`, 6 gap, label
  13/600 in `positive_text`: "Update available" / "Update ready". Hover: `hover` fill; focus
  ring; the tooltip names the action ("Restart Rustcord to install 0.1.1"). While a call runs,
  "Update ready" is disabled (icon and label `text_muted`, tooltip "Installs after your call
  ends"). Green, not ember: it is a state, not the view's primary action.

---

## 11. Activities

### 11.1 Art sources, in order

1. Rich-presence `large_image` (`mp:` or a numeric app asset), as now (`cdn::activity_asset_url`,
   api/cdn.rs:140).
2. **`spotify:<id>` → `https://i.scdn.co/image/<id>`**: a new branch in `activity_asset_url`, and
   `i.scdn.co` allowed in the image loader. This is the host the official client loads album art
   from. Images go through the unauthenticated image client.
3. **Application icon:** `cdn::app_icon_url(app_id, icon)` from the saved detectable-games list,
   else from `GET /applications/public?application_ids=` (the official client's application
   store; ≤ 25 ids per request, asked once per id, through the rate limiter, answers saved).
   `/games` (`FetchGames`) stays for profile game widgets: it knows only games, while rich
   presence comes from any application. Confirmed by the integrator (2026-11-14).
   - Trigger: only when a row or card with an `application_id` and no `large_image` is drawn.
4. The type icon (§10.4 slot, or a 64 tile in cards).

`twitch:` and `youtube:` previews are P2. They need their image hosts, which also needs the
integrator's OK.

### 11.2 Activity cards (profile popout, full profile, Active now)

Inside popouts the card is flat: a label-role heading, then content. In Active now each person
has one `surface` container (radius container); there are no nested cards.

```
Playing                                     <- label role: "Playing" / "Listening to Spotify" / "Live on Twitch" / "Watching" / "Competing in"
+--------+  Factorio                        <- heading 16/600
|  art   |  Building a megabase             <- ui 14 text (details)
| 64 r8  |  Nauvis (3 of 4)                 <- ui 14 text_muted (state; party in parentheses)
+----(o)-+  for 47 min                      <- meta text_muted; small image 24 circle, 2 px ring in container fill
[ Ask to join ]  [ Watch ]                  <- rich presence buttons: secondary, 32, open the URL only on click
```

| Kind | Content |
|---|---|
| Game (0) | art = §11.1 at 64, radius control. Elapsed "for 47 min" from `timestamps.start`, repainted once a minute while visible. |
| Spotify (2) | album art 64; song (heading); "by Artist"; "on Album" (muted). A 4 px progress bar (track `edge` @ 50 %, fill `text`, radius 2) with elapsed and total times in meta at both ends. Repaints at 1 Hz only while the card is visible. |
| Streaming (1) | "Live" tag + stream title (heading) + game (muted). A secondary "Watch" button opens the URL only on click. Preview image: P2. |
| Watching (3) / Competing (5) | as Game, with label "Watching" / "Competing in" |
| Custom status (4) | stays the bubble next to the avatar: `raised` fill, radius container, emoji 20 + body |
| No art at all | a 64 `hover` tile with the type icon 32 in `text_muted` |

---

## 12. Popouts, settings, login, desktop surfaces

**Profile popout** (profile.rs; fixes reference 2.webp)
- Fixed width 340. Max height = window − 32, then it scrolls.
- Role chips wrap per chip (profile.rs:342-345 `role_pills`).
- Banner band (always there; the avatar overlaps its bottom edge): the image, else the user's
  accent colour, else `selected` over `raised`. A themed card's gradient is the band.
- Badges stay hidden until their icons load. No grey placeholder circles.
- Sections ("About me", "Member since", "Roles", "Note") use the label role, 16 apart. No hairline
  after each section.
- "alice", then 8 px, then "they/them" in `text_muted`, with no bullet.
- Mutuals become two ghost links: "3 mutual servers" and "3 mutual friends".
- Footer: primary "Message", `PersonAdd` and `More` icon buttons.

**Settings**
- Nav groups in the label role, sentence case: "User settings", "App settings".
- Selected nav row: `selected` fill only.
- Page title in the title role.
- Section heading in the heading role with 32 above. No rule under it (ui/mod.rs:1292 `section`).
- Setting row:
  - left: title ui 14/500 and a meta `text_muted` description
  - right: the control
  - a `rule` between rows of one group
- Close button: round 36 with a 1 px `edge` ring and `Close`. Tooltip "Close settings (Esc)".
  Remove the "ESC" caption (ui/mod.rs:1253).
- My Account: one container. "Edit profile" is primary; "Change password" and "Set up
  authenticator app" are secondary.
- Plugins: divided rows instead of the card grid.
  - Each row: name 15/600, description meta (two lines at most), `Gear` if the plugin has
    settings, and a switch.
  - Filter: segmented "All / On / Off" next to the search.
  - Count: "3 of 48 turned on".
- Appearance: the theme swatch selection ring is `text` 2 px (not accent).

**Login** (login.rs)
- Flat `guild_bar` background. Remove `backdrop` and `glow` (login.rs:172-200).
- Panel: E3, 440 wide, radius modal, padding 32.
- Logo 56; "Sign in to Rustcord" in display; "Use your Discord account." in ui `text_muted`.
- Segmented tabs, 40 px inputs, primary "Sign in".
- Footer (login.rs:298-302): two centred meta lines with no middle dots:
  - "Rustcord 0.1.0 is an unofficial client and is not affiliated with Discord."
  - "Third-party clients can break Discord's Terms of Service."

**Desktop surfaces** (owned by the desktop spec; these are the copy and look rules only)
- **Tray menu:** "Open Rustcord", "Quit Rustcord". Tooltip "Rustcord".
- **App settings > Windows:**
  - "Minimize to tray when I close the window". Description: "Rustcord keeps running in the
    notification area. Quit it from the tray icon."
  - "Open Rustcord when I sign in to Windows".
  - Both are switch rows.
- **App settings > Linux / macOS** (same page, id `windows`): "Linux settings" / "macOS
  settings"; "Open Rustcord when I log in"; the start-minimized meta says "system tray" /
  "menu bar"; macOS close row "Keep Rustcord in the menu bar when I close the window". Linux
  without a tray host: the close row is disabled with "No system tray found, so closing the
  window quits Rustcord." Camera, screen share and video calls are disabled controls there,
  each with a tooltip like "Camera isn't available on Linux yet."
- **Installer:** title "Rustcord setup"; heading "Install Rustcord"; option rows use the same
  sentence case; buttons "Install" (primary) and "Cancel". Done: "Rustcord is installed" with
  "Open Rustcord".
- **Uninstaller:** "Remove Rustcord".
- **Update screen** (ui/update.rs, copy and states: docs/UPDATER_DESIGN.md §15.1): replaces the
  login / main window under the 32 px title bar (Rustcord's name there, as on login). Flat
  `guild_bar` page, a centred column, no card: logo 56; 24; title in the title role, `text`
  ("Downloading update 0.1.1", "Installing update", "Restarting", "Update failed"); 16; a
  320 × 4 progress bar, radius 2, track `edge` @ 50 %, fill `text` (not ember); 8; one meta
  line `text_muted` ("7.3 MB of 12.0 MB"); 24; buttons 40 tall, ghost "Not now" left of the
  one primary ("Install", "Continue"). A failure shows `Alert` 16 and the message in `danger`
  (ui role) in the bar's place. Enter = the primary, Esc = "Not now" / "Continue". Static: it
  repaints only when the updater reports progress.
- **Settings > Windows / Linux / macOS > Updates:** switch row "Install updates automatically"
  (description by install kind; disabled with its reason on copies that can't update
  themselves and in development builds), then a row "Version 0.1.0" whose description says
  what the updater is doing, with a secondary "Check for updates".

---

## 13. Copy rules

- Write from the user's side, in plain words and the active voice. Sentence case.
- Proper names keep their case (Rustcord, Discord, Spotify, GIF, ID). So do permission names
  users search for ("Manage Channels").
- A button says what happens, and an action keeps one name through its flow: "Leave server" on
  the button, "Left Rust Workshop" in the toast.
- Errors say what happened and how to fix it, without apologising: "Message not sent. Check your
  connection, then select Retry."
- A confirmation asks its question in the title and states the consequence in the body. Never
  "Are you sure".
- Empty screens invite an action.
- No middle-dot chains, no "WORD — n" labels, no → / ↗ in labels, no emoji as icons, no filler.

| Now | New |
|---|---|
| Add Reaction / Copy Text / Mark Unread / Copy Message Link | Add reaction / Copy text / Mark unread / Copy message link |
| Set Custom Status / Edit Profile / Copy User ID / Edit User Profile | Set custom status / Edit profile / Copy user ID / Edit profile |
| Invite People / Notification Settings / Privacy Settings / Hide Muted Channels | Invite people / Notifications / Privacy / Hide muted channels |
| Edit Per-server Profile / Leave Server / Mark As Read / Create Channel / Upload a File / Create Thread / Open in Split View / Join Voice / Voice Settings / Close DM | Edit server profile / Leave server / Mark as read / Create channel / Upload a file / Create thread / Open in split view / Join voice / Voice settings / Close DM |
| USER SETTINGS / DISPLAY NAME / CHANNEL TYPE / NEW / ESC | User settings / Display name / Channel type / New / (removed) |
| Voice Connected / Active Now / Direct Messages / About Me / Member Since / Add Friend | Voice connected / Active now / Direct messages / About me / Member since / Add friend |
| Maintainers — 2 / Online — 2 / Friends • Online | Maintainers, Online (count right-aligned) / Online |
| You need the Manage Channels permission. | You can't create channels in this server. Ask an admin for the Manage Channels permission. |
| Leave 'Rust Workshop' / Are you sure you want to leave…? | Leave Rust Workshop? / You'll need a new invite to rejoin. [Leave server] |
| Enable Authenticator App | Set up authenticator app |
| 3 Mutual Servers • 3 Mutual Friends / Nauvis · 3 of 4 | 3 mutual servers, 3 mutual friends (two links) / Nauvis (3 of 4) |
| Esc to cancel · Enter to save | Escape to cancel, Enter to save |
| No messages cached | This is the start of your conversation with Alice. (offline adds: "Messages load when you're connected.") |
| No direct messages yet | No direct messages yet. Start one from your friends list. [Open Friends] |
| Not supported yet | Rustcord can't open this channel type yet. |
| 0 of 48 on | 0 of 48 turned on |
| Open in browser ↗ | Open in browser [External] |
| Rustcord v0.1.0 · Unofficial client · … | the two login footer lines in §12 |

---

## 14. Change list

The priorities:
- **P0** covers the user's complaints.
- **P1** is the identity work.
- **P2** is polish.

Owners follow the module table in ARCHITECTURE.md. Each area re-reads files before editing.

### (A) Theme and shared components

| # | P | Change | Files |
|---|---|---|---|
| A1 | P0 | Icon module §8: `Icon`, `Badge`, `paint`, `channel()`, `from_glyph`; move the existing painters in | new `src/ui/icons.rs`; sidebar.rs:1264-1330, :809; messages.rs:288; friends.rs:145; dm_call.rs:312; user_settings.rs:725; user_settings/voice.rs:316; pickers.rs:638,997 |
| A2 | P0 | `icon_button`, `MenuIcon::Glyph`, `icon_line`, `empty_state` paint vectors through `from_glyph`; menu icons 16, `text_muted` | messages.rs:130-180; ui/mod.rs:1386; user_settings.rs:494 |
| A3 | P0 | `Tokens` §2.2 and `Pal` fields (raised, hover, selected, rule, edge, focus, positive, positive_text, on_positive, positive_fill, attention, live, danger_fill, chip, selection, heat, scrim) | new `src/theme/tokens.rs` (re-export in theme/mod.rs); messages.rs:71-125 |
| A4 | P0 | `presence_color` reads tokens (online positive, idle attention, streaming live) | ui/mod.rs:1355 |
| A5 | P1 | Palette §2.1. Update `invalid_color_is_rejected` (it greps `background = "#221e1c"` → `#23282c`); re-run gradient tests | theme/mod.rs:182-247 and tests :547-555; theme/gradient.rs tests |
| A6 | P1 | egui visuals: hover stroke `edge` (no accent mix, :113); `active` = `selected` (not accent, :115-118); selection = `selection` token + `text` stroke (:97-98); shadows E2/E3 (:90-91); `window_corner_radius` 2r, `menu_corner_radius` 1.5r; tooltip delay 0.3 s | theme/apply.rs:57-131 |
| A7 | P1 | Type roles §3: default size 15 (HC 16), text styles, `Pal` role helpers; `seguisb.ttf` heads the semibold chain without gg sans | theme/mod.rs:182; theme/apply.rs:27-36, :162-215; messages.rs:102-122 |
| A8 | P1 | Radius tiers §5: default 8, `card_radius` 1.5r, `modal_radius` | theme/mod.rs:187; messages.rs:108; sidebar.rs:1731 |
| A9 | P1 | No ALL CAPS in shared helpers: `nav_heading`, `section` (no rule), "ESC" caption out, dialog footer without band, modal widths 440/520/640 | ui/mod.rs:1253,1263-1297,1100-1170 |
| A10 | P1 | Switch, checkbox, radio, segmented, focus ring per §9 (shared painters next to `icon_button`) | messages.rs (shared helpers); used by settings |
| A11 | P2 | Done: §7, `ui/motion.rs` | ui/mod.rs dialogs; menus; sidebar.rs category header |
| A12 | P2 | Update the "Visual direction" paragraph in ARCHITECTURE.md to point here | docs/ARCHITECTURE.md:125-132 |

### (B) Main window

| # | P | Change | Files |
|---|---|---|---|
| B1 | P0 | Channel icons §8.4 in the list, header, autocomplete, inbox, threads, forum; selected icon not accent; add `Guild.rules_channel_id` | sidebar.rs:1005-1012, :1088-1097; messages.rs:347-353, :874-881; autocomplete.rs:198-203; inbox.rs:279-281; threads.rs:76-80; forum.rs; model.rs `Guild` |
| B2 | P0 | Activity art: `spotify:` branch and `i.scdn.co` allowed; app-icon fallback via `FetchGames` (after the integrator's OK); type-icon fallback tile | api/cdn.rs:140-155; ui/images.rs (host check); members.rs:176-184; friends.rs:321,351; state/social.rs:301 |
| B3 | P0 | Member and DM row line 2 + art slot §10.4, with art tooltip | members.rs:137-230; sidebar.rs DM rows; friends.rs rows |
| B4 | P0 | User panel line 2 with my activity §10.5 | sidebar.rs:1740 `account_row`, :2010 `my_line`, :704 `status_text` |
| B5 | P0 | Gear, mic, headphones, hang-up as vectors; the muted state tint | sidebar.rs:1278-1330 → icons.rs; user panel |
| B6 | P1 | Heat bar §10.1; neutral initials tiles; home tile without accent fill; Add server `Plus` | sidebar.rs:302-320, :466-475, :478-500 |
| B7 | P1 | Mention row: `attention` bar + `mention_highlight`; mention chips `chip`/`link`; "New" tag; reactions | messages.rs (mention row, :1222 "NEW"); markdown.rs chips |
| B8 | P1 | Member group header: count right-aligned, no em dash | members.rs:127-131; friends.rs:199 |
| B9 | P1 | Composer E1 radius container; `Plus`/`Smile` vectors; attach tray icons | composer.rs:262-290, :650-660, :886-985 |
| B10 | P1 | Header: channel icon + heading + rule divider + topic; icon buttons per §9 | messages.rs:870-930 |
| B11 | P2 | `system_icon` vectors | messages.rs:2629-2637 |
| B12 | P2 | Call stage buttons as vectors | call.rs:340-380 |
| B13 | P2 | Title bar location icon 16 | ui/chrome.rs / sidebar.rs title bar |

### (C) Settings, modals, popouts, login

| # | P | Change | Files |
|---|---|---|---|
| C1 | P0 | Profile popout: width 340, role chips wrap per chip, activity cards §11.2 with art sources | profile.rs:292-345, :420-440, :490-570 |
| C2 | P0 | Activity cards in Active now (one container per person) and in the full profile | friends.rs:321-360; profile_modal.rs |
| C3 | P1 | Settings: sentence-case labels, setting rows, close button, My Account single container with one primary | user_settings.rs:674, :483; user_settings/account.rs:128; user_settings/*.rs |
| C4 | P1 | Plugins as divided rows with segmented filter and "turned on" count | user_settings/plugins.rs:73-170 |
| C5 | P1 | Create channel: radio rows in one group, painted channel icons, copy; server dialogs one primary | server_menu.rs:775-905, :1060-1145; server_settings.rs:1231 |
| C6 | P1 | Login: flat background, panel, footer copy | login.rs:172-200, :298-302 |
| C7 | P1 | Copy pass per §13 (menus, friends, profile, settings, dialogs) | server_menu.rs, friends.rs, profile.rs, messages.rs menus, sidebar.rs menus, user_settings/*.rs |
| C8 | P1 | Remaining glyphs from §8.5 in settings, pickers (category rail `CATEGORY_ICONS`, picker headers no caps :685), security, lists, connections, cosmetics (↗), status | pickers.rs:50,685,756; user_settings/{security,lists,connections,status,cosmetics,theme_editor,keybinds,app}.rs; cosmetics.rs:960-975 |
| C9 | P2 | Profile banner fallback, badges hidden until loaded, no section hairlines, mutuals as links | profile.rs; cosmetics.rs:832 |
| C10 | P2 | Empty states §9 (DM start, Friends, search, inbox, plugins) | messages.rs:1201; friends.rs; search.rs; inbox.rs; user_settings/plugins.rs |
| C11 | — | Desktop copy §12 (tray, Windows settings rows, installer) | owned by the desktop work |

## 15. Verification

Use a scratch copy of `tests/fixtures/home` with `RUSTCORD_HOME`, `RUSTCORD_OFFLINE=1` and
`RUSTCORD_WINDOW_SIZE=1180x760`. Take screenshots in Ember Dark, Paper Light, High Contrast and
one gradient theme:
- `RUSTCORD_OPEN=members` (member art slots), `friends`, `card:<id>`, `profile`
  (my popout + activity), `voice:<id>`, `muted`, `status:dnd`
- `settings:my-account`, `settings:appearance`, `settings:plugins`
- `modal:create-channel`, `server-menu`
- `RUSTCORD_LOGIN_TAB=email`
- Motion: mid-animation with `RUSTCORD_MOTION_SCALE=10` (and `RUSTCORD_SCREENSHOT_FRAME`)

Zoom 3× into:
- the 20 px channel icons (text, voice, locked, NSFW, rules)
- the heat bar mid-animation and at rest
- the member art slot with and without art
- the user panel line 2
- a menu with a danger item
- the focus ring after Tab

Then run `cargo test`. Specifically:
- `builtin_names_are_unique_and_default_is_ember`
- `invalid_color_is_rejected` (update its hex)
- `presets_are_readable`
- `flat_themes_keep_their_fills`
- the font fallback tests
- the theme_editor size tests (they use 14.5)
- `sidebar::tests::glyphs` (it goes away with `channel_glyph`)

Also run `cargo clippy --all-targets` with zero warnings.
