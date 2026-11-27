# Rustcord design spec

Binding for all UI work. It replaces the "Visual direction" paragraph of `docs/ARCHITECTURE.md`.
Layout stays Discord web 1:1 (rail, channel card, chat, member list, floating user panel,
settings, menus, popouts). The look is Rustcord's own: **Nightshade**.

History: in 0.2 this look replaced the first one, which was blued steel with orange for the
brand and a glowing bar on the rail. The old built-in names "Ember Dark" and "Paper Light" still
load: `theme::renamed` maps them to Nightshade and Lilac. The new look takes the
floating-layer idea from Illegalcord's interface and has its own purple, its own logo and its own
signature, the speech corner.

---

## 1. Concept

**Nightshade: near-black layers that float. One purple, and it always means you.**

| Hue | Token(s) | Its only jobs |
|---|---|---|
| purple | `accent`, `accent_ink`, `chip`, `selection`, `mention_highlight` | the logo and home tile; the selected rail pill; one primary button per view; on-states (switch, checkbox, radio); focus ring, caret, selected text; mentions of me (row tint + bar), mention and channel chips, app tags; my reactions and poll picks; the icon of the selected settings nav row; progress bars |
| blue | `link` | links, info callouts, the "jumped to" flash |
| green | `positive*` | online, voice connected, speaking ring, Join / Accept call, "Update available", my activity icon |
| amber | `attention` | idle, "Connecting…", warning callouts |
| pink | `live` | streaming (dot with a play cut-out, streaming icon) |
| red | `danger*` | destructive actions, count badges, "New", "Live" tag, Do not disturb, errors |

Rules:
1. **Discord's layout, our material.** No control in the main window moves or resizes, except
   where the user asks for it: focus mode folds the rail and the channel list away (§10.9); the tab
   bar (while two or more tabs are open) and the split view take room in the page (§10.10). (Exception: server folders and channel categories fold open and shut, §7; paint only, the layout changes in one step.)
2. **Purple budget.** One big purple fill per view: the primary button. Everything else purple
   is small (a 4 px pill, a 2 px ring or bar, an 18 px icon, a 40x24 switch, a caret) or soft (a
   tint). Hover is never purple, except the home tile, which is the logo. List selection
   (channels, DMs, members, settings nav fill) is never purple.
3. **Colour plus a word or a shape.** Switches show a check or a cross. Status dots keep their
   shapes. The island says "Voice connected" or "Connecting…".
4. **Light comes from above.** Raised layers have a 1 px lit top edge on dark themes. The logo is
   lit from the top left. No other gradients in the UI. No glows.
5. **Calm by default, loud for exceptions.** Grey panes; colour means something happened.
6. **Motion only answers an action** and always stops (0 % idle CPU). Exception (approved): the
   user panel's music section eases in and out when the computer starts or stops playing, which
   answers the user's own action in another app, not one inside Rustcord. Exceptions (approved): a message that arrives in a calm, followed chat rises in (§7).

---


## 2. Colour

### 2.1 `ThemeColors` (the TOML contract: no field added, renamed or removed)

| Field | Nightshade (dark, default) | Lilac (light) | High Contrast | Used for |
|---|---|---|---|---|
| `guild_bar` | `#08070B` | `#E7E5EE` | `#000000` | title bar, rail, behind the channel card, around the settings panel, login and update pages |
| `sidebar` | `#0E0D13` | `#F0EEF5` | `#000000` | channel card, member list, settings nav |
| `background` | `#121118` | `#FBFAFD` | `#0A0A0A` | chat, Friends, settings page side |
| `message_hover` | `#17161E` | `#F4F2F8` | `#222222` | message row hover |
| `surface` | `#1C1A24` | `#EBE8F2` | `#161616` | composer, user panel, embeds, rail tiles, island; dark: all floats (`raised` = `surface`) |
| `code_background` | `#0A090E` | `#F2F0F6` | `#1C1C1C` | wells: inputs, code, search, segmented track |
| `text` | `#F0EEF6` | `#17151D` | `#FFFFFF` | text |
| `text_muted` | `#A8A5B6` | `#5B5869` | `#D6D6D6` | secondary text, icons at rest |
| `accent` | `#A42BEA` | `#8E15D5` | `#DDA3FF` | the purple primary |
| `accent_text` | `#FFFFFF` | `#FFFFFF` | `#000000` | label, knob and check on `accent` |
| `link` | `#6CB4FF` | `#0A63C6` | `#66E0FF` | links |
| `danger` | `#FC6D72` | `#C42838` | `#FF5C5C` | destructive text and icons, errors |
| `mention_highlight` | `#241431` | `#F0E3F9` | `#34293B` | row tint of messages that mention me or match a keyword alert |

`ThemeFont` and `ThemeLayout` defaults stay: size 15 (HC 16), `corner_radius` 8 (HC 4).

The purple: hue 278 (HSL) in all three presets. Discord blurple `#5865F2` is hue 235, CIELAB
distance 37.6 (dark) / 36.7 (light): it reads purple, never blurple. Twitch `#9146FF` is 11.4 /
14.0 away (same family, clearly not the same colour). White on `#A42BEA` is 5.11:1.

The neutrals: hue about 255, very low chroma (R one step above G, B a few steps above): a violet
undertone you only see next to a pure grey. CIELAB L*: frame 2.1, sidebar 3.9, chat 5.4, hover
7.7, surface 9.9, well 2.6. Illegalcord's own panes measure about L* 2-4 (nav `#070709`, page
`#0C0C0E`), so this sits in the same place.

### 2.2 `Tokens` (src/theme/tokens.rs): every field

"x @ n %" = colour x at that opacity (translucent, works on gradient themes). Composite = over
the pane in brackets.

| Token | Nightshade | Lilac | High Contrast | Formula / job |
|---|---|---|---|---|
| `mode` | Dark | Light | HighContrast | `Mode::of`, unchanged (muted/bg 7.79, 6.63, 13.62) |
| `raised` | `#1C1A24` | `#FFFFFF` | `#161616` | unchanged rule: light = white, else `surface` |
| `hover` | text @ 6 % (`#1F1E25` on chat) | text @ 6 % | text @ 12 % | unchanged |
| `selected` | text @ 10 % (`#25242A` on sidebar) | text @ 8 % (`#DFDDE4` on sidebar) | text @ 18 % | unchanged |
| `rule` | text @ 10 % (`#28272E` on chat) | text @ 12 % (`#E0DFE2`) | muted @ 45 % | unchanged |
| `edge` | `#83808F` | `#83818E` | `#ADADAD` | unchanged: `mix(background, text_muted, 0.75)`, HC 0.8 |
| **`accent`** | `#A42BEA` | `#8E15D5` | `#DDA3FF` | `c.accent.to_color32()`: on-state fills for helpers that only have `Tokens` (menus) |
| **`on_accent`** | `#FFFFFF` | `#FFFFFF` | `#000000` | `c.accent_text.to_color32()`: knob and check on `accent` |
| **`accent_ink`** | runtime ≈ `#BE67F0` | runtime ≈ `#8B15D0` | runtime ≈ `#DEA6FF` | purple as text, glyph, ring or bar. See formula below |
| `focus` | = `accent_ink` | = `accent_ink` | = `accent_ink` | focus ring (was `link`; HC was `accent`) |
| **`sheen`** | white @ 6 % (`Color32::from_white_alpha(15)`) | `TRANSPARENT` | `TRANSPARENT` | the lit top edge of raised layers (§6.2) |
| `positive` | `#50C76A` | `#1D8B3D` | `#4FE07A` | unchanged |
| `positive_text` | `#50C76A` | `#00792B` | `#4FE07A` | unchanged |
| `positive_fill` / `positive_label` | `#25803F` / white | `#25803F` / white | `#4FE07A` / black | unchanged |
| `attention` | `#EBC166` | `#A86B00` | `#FFD400` | unchanged values ("amber" in comments). Idle, "Connecting…", warnings. No longer the mention bar |
| `live` | **`#F062C0`** | **`#B0237F`** | **`#FF8AD8`** | streaming (was purple). Hue 320: 53.8 ΔE from the accent, 48.0 from `danger` |
| `danger_fill` / `danger_label` | `#CC3D3F` / white | `#CC2827` / white | `#FF5C5C` / black | unchanged |
| `chip` | accent @ 16 % (`#29153A` on chat) | accent @ 12 % (`#EEDFF8`) | accent @ 22 % (`#382C40`) | **from `accent`** (was `link`) |
| `selection` | accent @ 40 % (`#4C1B6C`) | accent @ 24 % (`#E1C3F3`) | accent @ 40 % (`#5E476C`) | **from `accent`** (was `link`) |
| `scrim` | black @ **64 %** | black @ 40 % | black @ 80 % | dark 60 → 64 % (darker panes) |
| `media_scrim`, `on_media`, `on_media_muted`, `media_hover` | unchanged | unchanged | unchanged | |

`accent_ink` formula (tokens.rs, inside `Tokens::new`):

```rust
// Purple as text: readable (4.5:1) on a chip inside a mention row, the most saturated place it
// sits, and on the floats. readable_on keeps the hue.
let chip_a = k(0.16, 0.12, 0.22);
let chip = |under: Color32| under.blend(accent.gamma_multiply(chip_a));
let row = c.background.to_color32().blend(c.mention_highlight.to_color32()); // opaque for gradient themes too
let accent_ink = readable_on(readable_on(accent, chip(row)), raised);
```

`ATTENTION` (tokens.rs) stays for `attention`; comment it as amber.

### 2.3 Contrast (WCAG 2; text ≥ 4.5, components ≥ 3)

| Pair | Nightshade | Lilac | High Contrast |
|---|---|---|---|
| text on chat / frame / sidebar / surface / raised | 16.32 / 17.47 / 16.82 / 14.94 / 14.94 | 17.39 / 14.50 / 15.72 / 14.94 / 18.08 | 19.8 / 21.0 / 21.0 / 18.1 / 18.1 |
| muted on chat / sidebar / surface / selected row | 7.79 / 8.03 / 7.13 / 6.39 | 6.63 / 6.00 / 5.70 / 5.12 | 13.62 / 14.45 / 12.45 / 9.34 |
| `accent_text` on `accent`: rest / hover (+6 % white) / press (+12 % black) | 5.11 / 4.72 / 6.26 | 6.53 / 5.98 / 7.85 | 10.76 / 11.26 / 8.30 |
| `accent` vs chat / sidebar / frame / surface (switch track, pill, radio) | 3.67 / 3.79 / 3.93 / 3.36 | 6.27 / 5.67 / 5.23 / 5.39 | 10.14 / 10.76 / 10.76 / 9.27 |
| `accent_ink` on chat / sidebar / raised | 5.71 / 5.88 / 5.22 | 6.48 / 5.86 / 6.74 | 10.37 / 11.0 / 9.48 |
| `accent_ink` on chip over chat / chip over mention row / chip over raised | 5.06 / 4.52 / 4.58 | 5.31 / 4.51 / 5.49 | 6.87 / 4.50 / 5.99 |
| `accent_ink` on selected row (nav icon) / on mention row (bar) | 4.68 / 5.24 | 5.01 / 5.48 | 7.12 / 7.21 |
| text on selection / on mention row; muted on mention row | 10.87 / 14.98; 7.15 | 11.45 / 14.69; 5.60 | 8.10 / 13.76; 9.47 |
| link on chat / raised | 8.59 / 7.86 | 5.59 / 5.81 | 12.89 / 11.78 |
| danger on chat / raised (test: ≥ 4.5 on menus) | 6.76 / 6.19 | 5.45 / 5.66 | 6.54 / 5.98 |
| label on `danger_fill` / `positive_fill` | 4.88 / 4.96 | 5.38 / 4.96 | 6.94 / 12.28 |
| `edge` vs chat / surface / well | 4.87 / 4.46 / 5.15 | 3.67 / 3.16 / 3.38 | 8.82 / 8.06 / 7.59 |
| switch-off knob vs `edge` track (knob carries the ×) | 3.35 (text knob) | 3.82 (white knob) | 2.24, as today (track vs pane 8.8) |
| dots on sidebar: positive / attention / live / danger | 8.95 / 11.38 / 6.64 / 6.97 | 3.79 / 3.83 / 5.38 / 4.92 | 12.28 / 14.67 / 9.88 / 6.94 |
| `positive_text` on chat / surface | 8.68 / 7.95 | 5.36 / 4.60 | 11.57 / 10.58 |
| pane steps frame:sidebar, sidebar:chat, chat:surface | 1.04, 1.03, 1.09 | 1.08, 1.11, 1.16 | 1.00, 1.06, 1.09 |

Notes:
- The dark pane steps are tiny in WCAG terms (near-black squeezes the formula) but 1.5-2 L* each,
  like Illegalcord. Hairlines carry the structure (§6.3).
- The dark primary hover is the tight spot (4.72). Hover lightens by exactly 6 %; 8 % would drop
  to 4.58. Do not lighten the dark accent without re-running the tests.
- `accent` on the dark `surface` is 3.36 (a switch inside a modal): fine; the knob glyph also
  carries the state.
- The "Connecting…" word in the island and voice panel uses `readable_on(attention, fill)`:
  plain amber on white is 4.40.

### 2.4 Gradient and custom themes

- `Theme::from_gradient` and `retint` (gradient.rs) start from `nightshade()` /
  `lilac()`. The 23 gradient presets keep their names and get the purple through `..base`.
- `retint` mention tint: `with_alpha(base.accent, 0x21)` instead of `ATTENTION` (13 % purple).
- Custom TOML themes: unchanged contract. Missing colours now default to Nightshade
  (`#[serde(default)]` → `ThemeColors::default()`). All tokens are derived, so custom themes get
  `accent_ink`, `sheen` and friends.
- Known limit: a few gradient presets with purple stops (Hexagon, Chroma Glow) may put the
  switch track near 3:1 on parts of the gradient. Check them in the screenshots (§16); the knob
  glyph carries the state either way.

### 2.5 Preset names and migration of saved values

| Old | New |
|---|---|
| Ember Dark (default) | **Nightshade** (default) |
| Paper Light | **Lilac** |
| High Contrast | High Contrast |
| 23 gradient presets | names unchanged |

What users have saved:
- `settings.toml` holds `theme = "Ember Dark"` (default), `"Paper Light"`, `"High Contrast"`, a
  gradient name, or a user theme name / file stem (config.rs).
- The themes folder may hold user files such as "Ember Dark (custom)" (import of a built-in
  name) or "Ember Dark custom" (theme editor default save name). They are the user's own files.
  They keep their names and their old colours, and keep loading by name.

Migration (one place, `src/theme/mod.rs`):

```rust
/// Built-in names before the 0.2 redesign, for settings files that still name them.
pub fn renamed(name: &str) -> &str {
    match name {
        "Ember Dark" => "Nightshade",
        "Paper Light" => "Lilac",
        n => n,
    }
}
```

`Theme::find(name)`: built-ins by `name` → user themes by name or stem → built-ins by
`renamed(name)` → default. The rename is only a fallback, so a user theme that is really called
"Ember Dark" (possible now that the name is free) still wins. `settings.toml` is not rewritten;
it keeps the old string until the user picks a theme. Without this map a Lilac user would land on
the dark default.

Edge case (accepted): a user theme named exactly "Nightshade" or "Lilac" is now shadowed by the
built-in (existing rule in `load_user_themes`: taken names are skipped).

---

## 3. Type

Font unchanged: gg sans fetched at runtime (`use_discord_font`), else Segoe UI / Semibold / Bold
(Linux Noto/DejaVu, macOS Arial). Nothing bundled (no new font is bundled).

B = `theme.font.size` (15; HC 16).

| Role | Size | Weight | Tracking | Used for |
|---|---|---|---|---|
| display | **B+13 = 28** (was B+9) | 700 | **−0.6 px** | login title, update screen title, full-profile name, DM start name |
| title | B+5 = 20 | **700** (was 600) | **−0.3 px** | settings page title, modal title, profile popout name |
| heading | B+1 = 16 | 600 | 0 | chat header, server header, settings section heading |
| body | B = 15 | 400; names 500 | 0 | messages, rows, settings nav rows |
| ui | B−1 = 14 | 500 controls, 400 inputs | 0 | buttons, menus, tooltips |
| label | B−2 = 13 | 600, muted | 0 | categories, member groups, nav groups, island state word |
| meta | B−3 = 12 | 400 | 0 | timestamps, hints, "Edit profile", island server name |
| badge | B−4 = 11 | 700 | 0 | counts, "New", "Live" |

Two helpers on `Pal` (messages.rs), so tracking lives in one place:

```rust
/// Title role (B+5, 700, -0.3 px): settings page titles, modal titles, popout names.
pub fn title_text(&self, text: &str) -> RichText {
    RichText::new(text).font(FontId::new(self.title(), crate::theme::bold_family())).extra_letter_spacing(-0.3).color(self.text)
}
/// Display role (B+13, 700, -0.6 px): login and update titles, full-profile name.
pub fn display_text(&self, text: &str) -> RichText { /* same with self.display(), -0.6 */ }
```

Rules: tracking only on display and title. Sentence case, no all caps, no coloured headings.

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


---

## 5. Radius

| Tier | Helper | r = 8 | HC | Elements |
|---|---|---|---|---|
| chip | r / 2 | 4 | 2 | chips, tags, checkbox, inline code |
| control | `pal.radius` | 8 | 4 | buttons, inputs, rows, settings nav rows, segments, embeds, call buttons |
| card | `card_radius()` 1.5r | 12 | 6 | composer, user panel, menus, tooltips, autocomplete, hover toolbar, callouts |
| float | **`float_radius()` 2r** | 16 | 8 | profile popout (+ its preview on My account), inbox, pins, emoji/GIF picker, toasts |
| layer | **`modal_radius()` 2.5r (was 2r)** | 20 | 10 | modals and confirms, settings panel, quick switcher, login card, full profile, the drop overlay card |
| full | | pill | pill | avatars, switches, badges, status dots, the island, the call dock |
| fixed | | 12 | 12 | rail tiles (`ICON_R`) and the channel card corner (Discord's values) |
| speech corner | | 2 | 2 | bottom-left corner of the selected rail tile (§10.1) |

Callers to switch (everything else keeps its helper):
- `float_radius()`: profile.rs, user_settings/account.rs, inbox.rs, pins.rs,
  pickers.rs. Toasts already use `corner_radius * 2.0` (= float).
- `modal_radius()` callers stay (dialog mod.rs, login.rs, profile_modal.rs,
  switcher.rs, composer.rs) and grow to 20 by the helper change.
- apply.rs: `window_corner_radius = radius(2.5)`; `menu_corner_radius` stays `radius(1.5)` (egui
  shares it between menus and tooltips; 16 would make one-line tooltips pills).


---

## 6. Elevation

### 6.1 Levels (apply.rs `visuals` and our own floats)

| Level | What | Fill | Edge | Sheen | Shadow (`egui::Shadow`) |
|---|---|---|---|---|---|
| E0 pane | title bar, rail, channel card, chat, member list | pane | `rule` where panes meet (§6.3) | no | none |
| E1 card | composer, user panel, island, settings option groups | `surface` | 1 px `rule` (composer: none; HC `rule`) | yes | none (the island: the contact shadow only, in the E2 colour) |
| E2 float | menus, tooltips, popouts, pickers, autocomplete, hover toolbar, toasts | `raised` | 1 px `rule` (HC 1.5 px muted) | our own floats only | dark `[0,12]` blur 32 black 55 %; light `[0,12]` blur 32 black 12 %; plus a contact shadow `[0,1]` blur 3 in that colour at 60 % |
| E3 layer | modals, settings panel, quick switcher, login card, incoming call card | `raised` (settings: two pane fills) | 1 px `rule` (HC 1.5 px muted) | yes | dark `[0,24]` blur 64 black 65 %; light `[0,24]` blur 64 black 18 %; plus a contact shadow `[0,1]` blur 3 in that colour at 60 % |

HC: no shadows, no sheen (as today: `Shadow::NONE`, the 1.5 px muted edge).

The contact shadow (`messages::contact`) goes under every float whose frame has a shadow, egui's menus and tooltips too (`messages::contact_shadows`, run by `motion::end_pass`), and under the call island. Static paint. HC: none.

### 6.2 The sheen (lit top edge)

One helper in `src/ui/messages.rs`, next to `focus_ring`:

```rust
/// The lit top edge of a raised layer (`sheen`): 1 px, 1 px under the top edge (below a 1 px
/// outline), from `radius` in on both ends. Nothing on light and high-contrast themes.
pub(super) fn sheen(p: &egui::Painter, rect: Rect, radius: f32, tk: &crate::theme::Tokens) {
    if tk.sheen.a() > 0 {
        p.hline(rect.left() + radius..=rect.right() - radius, rect.top() + 1.5, Stroke::new(1.0, tk.sheen));
    }
}
```

Static paint, no frames. Where: user panel, composer, island, dialogs (`dialog_closable`: the
frame's outer rect is `ui.min_rect().expand(24.0)` after the content), toasts, profile popout,
inbox popout, settings panel, login card, call dock. Not on egui's own menus and tooltips (no
hook): they get fill, rule and shadow only.

### 6.3 Hairlines

`rule`, 1 px. Where: the channel card corner line (exists), the chat header bottom (exists),
**new: the left edge of the right column** (member list, thread panel, search results; one
`vline` in `main_layout` like the channel list's at), float outlines, the settings
panel outline and its nav | page divider, menu separators (inset 8), date separators. Nowhere
else.

---

## 7. Motion (all through `src/ui/motion.rs`; ux-motion principle numbers in brackets)

Tokens unchanged: FAST 0.12, BASE 0.20, LARGE 0.30, OUT 0.5, GROW 0.96, POP 1.12, STAGGER 0.04.

| What | In | Out | Helper | Principle |
|---|---|---|---|---|
| Rail pill height + colour (white → purple) + speech corner 12 → 2 | FAST, cubic in-out, retargets mid-way | FAST | `value` (existing; one value drives all three) | 5, 7, 17 |
| Rail and home tile morph (home: surface → purple) | FAST | FAST | existing `morph` | 8 |
| Switch knob + track `edge` → `accent` | FAST slide, 15 % stretch | same | `slide` (existing) | 1, 5 |
| Checkbox, radio, nav selection, hover colours, page switch, quick switcher | instant | instant | | 20 |
| Settings panel (with backdrop and scrim) | LARGE: fade + 0.98 → 1 about the panel centre | instant | `anim(.., LARGE, 0.0)` + `about` (existing) | 8, 18 |
| Backdrop picture | appears under the already fading scrim (ready by frame 2) | dropped at once | | 16 |
| Call island appear / leave | BASE: fade + scale 0.92 → 1 about its centre; the label fades out | BASE x OUT fade; the label fades in | `anim(.., BASE, BASE * OUT)` | 5, 18 |
| Call island connects | dot `attention` → `positive` over FAST + one pop 1 → 1.12 → 1 over BASE; the width glides over BASE (`island_width`) | | `anim`, `pop`, `value` + `forget` | 5, 7, 9, 19 |
| Focus mode panes (rail, channel list, user panel) | returning: LARGE ease-out | leaving: BASE ease-in; content slides left with its narrowing column | `anim(.., LARGE, BASE)` | 5, 8, 17, 18 |
| Tab bar appears (second tab) | FAST: height 0 → 40 ease-out, tabs ride its bottom edge | at once | `anim(.., FAST, 0.0)` | 5, 8, 18 |
| New tab | FAST: fade + grow 0.96 → 1 about its centre (`scaled`) | closes at once; the tabs to its right glide left over FAST in-out | `anim`, `value` on a slot index | 5, 11, 18 |
| Tab already open | pop 1 → 1.12 → 1 over BASE | | `pop` | 9 |
| Tab drag | follows the pointer 1:1; the passed tab glides aside over FAST in-out; settles from where it was let go | | `value` + `set_value` | 5, 17 |
| Split pane opens | BASE: fade + 16 px slide from the right, layout final at once | at once | `anim(.., BASE, 0.0)` | 5, 8, 18 |
| Switching tabs, focus change between panes | instant | instant | | 20 |
| Menus, tooltips, popovers, pickers | FAST, grow 0.96 from the trigger | instant | `end_pass` (existing) | 11, 18, 20 |
| Modals | BASE, grow from the click | BASE x OUT ghost | `modal` (existing) | 11, 18 |
| Toasts | BASE, 8 px rise + fade | BASE x OUT | existing | 5, 18 |
| Press (everything using `motion::pressed`) | instant: drawn at **97 %** about the centre, at least 1 px smaller per side | instant | change `pressed` + its test | 16, 20 (house ".97", no frames) |
| Hidden message shown | FAST fade of its content, height instant | – | `anim` | 5, 16, 20 |
| Translation arriving | FAST fade of the text, height instant | instant (Show original) | `anim` | 5, 16, 18 |
| My reaction | pop 1 → 1.12 → 1 over BASE | | `pop` (existing) | 9 |
| Login error shake, update pill | LARGE | | existing | 9 |
| Music section of the user panel appears / leaves | BASE: height 0 → 41 and opacity 0 → 1, ease-out; the row is laid out at its final size against the section's bottom edge and clipped, so the panel's rising top edge reveals it | BASE x OUT, the same curve backwards | `anim(.., BASE, BASE * OUT)` | 5, 8, 11, 18 |
| Server folder, channel category open / close | BASE ease-out: the rows slide out from under the folder tile or header, rows below follow | BASE x OUT ease-in | `Fold` + `anim` | 5, 11, 17, 18 |
| A message arriving in the open chat | BASE: 6 px rise + fade, ease-out; only the only new one, over 1 s after the last arrival, window focused, chat at the bottom | | `since`, `shifted` | 5, 20 |

Folding is paint only: opening lays the rows out at once, closing keeps them laid out until shut, so hit-testing and drop targets never slide.

Ending cleanly (0 % idle CPU):
- Every row above ends and stops asking for frames. Nothing loops: no glow, no pulse, no
  shimmer, no animated gradient. Existing loops stay: typing dots, incoming ring, spinners.
- The sheen, the speech corner and the backdrop are static paint.
- The island repaints only while it fades, glides or pops. CallTimer (when on) keeps its existing 1 Hz tick.
- Reduced motion (setting or Windows "Animation effects" off): every duration 0, final states
  at once.

### 7.1 Sound

Rustcord's own sounds are synthesized in code (`src/sounds/synth.rs`), never files. They follow
the same ux-motion principles as the motion above. Exception (approved): the user's own WAV or
Ogg Opus file can replace the message, mention and incoming call sound, everywhere or for one
server, channel or DM (Settings > Notifications > Custom sounds). Rustcord ships no sound files;
without one, the synthesized sound plays. A file over 5 MB is refused, only its first 10 s play
(10 ms linear end fade), it is read and decoded locally and never uploaded; a looping ring loops
the file (still capped at 60 s and stopped within one pass of Accept / Decline).

- Families: bell for chat, rings and streams; knock (bell plus a soft tick) for voice
  presence; click for switches. Pitches are D major pentatonic (D4 to F#6).
- Switch sounds are audible for 150 ms or less (8, snappy); presence sounds 0.3-0.8 s; only rings
  last longer. Every out/off sound is shorter than its in/on pair (18). Push to talk is the
  shortest and quietest and the message ping short and soft (20). Only the incoming ring is loud
  (15). Disconnect resolves down to D4 (19). Attack is eased, decay exponential, linear only in
  the 10 ms end fade (5). A ring stops within one pass of Accept or Decline, with a 20 ms fade
  (17). The sound starts within about 100 ms (16).
- A sound never stands alone (7): each backs up something visible (call popup, notification,
  mute icons, speaking ring, LIVE tag).
- Reduced motion does not apply; Settings > Notifications > Sounds does.

---|---|
| rail pill | 150 ms (existing `ANIM`) |
| switch knob, segmented selection, category chevron (0 ↔ −90°) | 120 ms |
| modal open | 140 ms fade + scale 0.97 → 1 |
| menu and popout open | 100 ms fade |

- No hover tweens: hover changes colour instantly.
- Repaints only through `request_repaint_after`.
- `reduced_motion` sets every duration to 0. Static states stay.

---


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
- `Forward`: the `Reply` strokes mirrored (a curved arrow right).
- `Poll`: three filled bars, radius 2: (3.5,4)-(20.5,8), (3.5,10)-(14.5,14), (3.5,16)-(17.5,20).
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


### 8.6 Five new icons (src/ui/icons.rs; 24 grid, stroke 1.75, round ends; add to `Icon` and `ALL`)

Written with the pen helpers already in icons.rs (`k.closed`, `k.ring`, `k.line`, `k.disc`,
`k.rrect_line`, `arc(x, y, r, a0, a1)` in degrees, 0 = right, 90 = down):

```rust
Palette => {
    // Open ring with a thumb dip at the lower right, three paint dots.
    let mut pts = arc(12.0, 12.0, 9.0, 75.0, 375.0);
    pts.extend([(16.5, 14.5), (14.5, 16.5)]);
    k.closed(&pts);
    k.disc(7.75, 11.5, 1.5);
    k.disc(10.5, 7.25, 1.5);
    k.disc(15.25, 7.75, 1.5);
}
Accessibility => {
    k.ring(12.0, 12.0, 9.0);
    k.disc(12.0, 7.25, 1.6);
    k.line(&[(7.5, 10.0), (12.0, 11.0), (16.5, 10.0)]);
    k.line(&[(12.0, 11.0), (12.0, 14.0)]);
    k.line(&[(9.5, 18.0), (12.0, 14.0), (14.5, 18.0)]);
}
Keyboard => {
    k.rrect_line(2.5, 6.0, 21.5, 18.0, 2.5);
    for x in [6.5, 10.0, 13.5, 17.0] { k.disc(x, 10.0, 1.0); }
    k.line(&[(8.5, 14.5), (15.5, 14.5)]);
}
Puzzle => {
    // One piece: knobs on the top and right edges.
    let mut pts = vec![(4.5, 8.5), (8.48, 8.5)];
    pts.extend(arc(10.25, 6.75, 2.5, 135.0, 405.0));
    pts.extend([(16.5, 8.5), (16.5, 11.98)]);
    pts.extend(arc(18.25, 13.75, 2.5, 225.0, 495.0));
    pts.extend([(16.5, 19.5), (4.5, 19.5)]);
    k.closed(&pts);
}
```

`Next` (the music controls), after `Pause` in the Media group: the `Play` triangle plus a bar.

```rust
Next => {
    k.fill(&[(5.0, 5.0), (15.5, 12.0), (5.0, 19.0)]);
    k.rrect(16.5, 5.0, 19.5, 19.0, 1.0);
}
```

Check with `RUSTCORD_NAV_DEMO=icons` (the icon sheet) at 16, 18 and 20 px.

### 8.7 Two badge icons (ShowBadgesInChat)

`Bug` and `Code`, added after `Puzzle` in `Icon` and `ALL`, same pen rules as §8.6:

```rust
Bug => {
    k.rrect_line(8.0, 8.5, 16.0, 20.0, 4.0);          // body
    k.line(&arc(12.0, 8.5, 3.0, 180.0, 360.0));      // head
    k.line(&[(12.0, 11.5), (12.0, 20.0)]);           // wing split
    for (y0, y1) in [(12.0, 10.5), (15.0, 15.0), (18.0, 19.5)] {
        k.line(&[(8.0, y0), (4.5, y1)]);
        k.line(&[(16.0, y0), (19.5, y1)]);
    }
    k.line(&[(10.0, 5.75), (8.5, 3.5)]);
    k.line(&[(14.0, 5.75), (15.5, 3.5)]);
}
Code => {
    k.line(&[(8.0, 6.5), (3.0, 12.0), (8.0, 17.5)]);
    k.line(&[(16.0, 6.5), (21.0, 12.0), (16.0, 17.5)]);
    k.line(&[(13.75, 4.5), (10.25, 19.5)]);
}
```

Which glyph a `public_flags` badge gets (all monochrome, `text_muted`, never purple):

| Badge | Icon |
|---|---|
| staff | `IdCard` |
| partner | `Star` |
| hypesquad, houses 1 to 3 | `Flag` |
| bug hunter 1 and 2 | `Bug` |
| early supporter | `Heart` |
| verified developer, active developer | `Code` |
| certified moderator | `Shield` |

`Verified` and `Partnered` are not used: they cut their marks out in the background colour,
and the author line doesn't know the row's current fill.

---

## 9. Components

Sizes are today's unless stated. Colours are tokens from §2.

### 9.1 Buttons (`messages::button`)

32 tall (40 in modal footers and on login), padding 16, min width 64, label ui 14/500, radius r.

| Kind | Rest | Hover | Press | Disabled |
|---|---|---|---|---|
| Primary | `accent` + `accent_text` | `accent.lerp_to_gamma(WHITE, 0.06)` | `accent.lerp_to_gamma(BLACK, 0.12)` | `selected` fill, muted label |
| Secondary | egui weak fills (`selected`, +4 %, +8 %) | | | text @ 45 % |
| Danger | `danger_fill` + `danger_label` | +10 % black | +18 % black | as secondary |
| Positive | `positive_fill` + `positive_label` | +10 % black | +18 % black | as secondary |
| Ghost | none, `text` | `hover` | `selected` | muted @ 60 % |
| Icon (`button_with`) | 32 hit, 20 icon muted | `hover` fill, icon `hot` | `selected` | on: `selected` fill + icon `text`, never purple |

The same Primary fills everywhere: "Add friend" tab (friends.rs: hover 0.06), login
`primary_button`, "Watch stream" (call.rs).

### 9.2 Switch (`messages::switch`)

40x24 pill, knob 18. Off: track `edge`, knob `text` (light: white) with a 1.5 px × in the track
colour. **On: track `accent`, knob `on_accent` with a check in `accent`.** The track lerps
`edge` → `accent` with the knob travel. Motion unchanged (FAST slide, 15 % stretch).
Code: `tk.positive` → `pal.accent`, `tk.on_positive` → `pal.accent_text`.

### 9.3 Checkbox and radio

- `paint_checkbox`: off 1.5 px `edge` ring; **on `tk.accent` fill + 2 px `tk.on_accent`
  check**. Radius chip.
- `paint_radio`: off 1.5 px `edge` ring; **on 2 px `accent` ring + 8 px `accent` dot**
  (was `text`).
- Selected-check badges: theme editor swatch (theme_editor.rs) and cosmetics picker
  (cosmetics.rs): `tk.accent` disc with a `tk.on_accent` check (were green).

### 9.4 Inputs

Well `code_background`, 1 px `edge`, radius r, padding 12; 40 tall in forms, 32 in search.
Hover: border `text_muted`. Focus: 2 px `focus` (purple ink) replaces the border (apply.rs
already reads `tk.focus`). **Caret: 2 px `accent_ink`** (apply.rs). Error: `danger`
border + a 12 px `danger` line with `Alert`.

### 9.5 Segmented control, tabs, menus, tooltips

Unchanged in shape. Segments and Friends tabs stay neutral (`selected` fill): a view switch is not
"you". Menus: E2, radius 12, items 32, `hover` fill, danger items `danger_fill` on hover, toggle
items use the purple checkbox. Tooltips: `raised` + `rule` + E2, radius r (light tooltips stay
white, no inversion).

Edit name (person menus, `friends::name_submenu`): a submenu form 240 wide, like "Change
nickname": "Name" (meta, `text_muted`) and a field (32 chars), "Tag" and a field (16 chars), "Colour"
with nine 20 px discs 4 apart (None: 1.5 px `edge` ring with a slash; picked: 2 px `focus` ring), one
meta line, Ghost "Reset" and Primary "Save". For a friend the name is Discord's friend nickname.

### 9.6 Popouts and modals

- Profile popout (profile.rs): radius float 16, `raised`, `rule`, E2, sheen, display name via
  `title_text`. Footer Primary "Message".
  Role pills: the colour dot, the role icon 14 when the role has one (4 px after the dot; the pill
  grows by 18), the name.
- Inbox, pins, pickers: radius float 16 (§5); inbox gets the sheen.
- Modals (`dialog_closable`, mod.rs): E3, radius 20, padding 24, widths 440 / 520 / 640,
  title via `pal.title_text`, sheen, `Close` 32 top right. Footer: Ghost "Cancel" + one Primary
  or Danger (unchanged).
- Quick switcher: radius 20 via `modal_radius()` (no motion, unchanged).

### 9.7 Toasts (`show_toasts`, mod.rs)

`surface`, **1 px `rule`, E2 shadow, sheen**, radius 16, padding 12x16, max 360, bottom
right of the chat. 8 px dot: `danger` errors, `positive` done, muted info. Never purple.

### 9.8 Badges, chips, reactions, polls

| Thing | Look |
|---|---|
| Count badge | `danger_fill` pill 16 tall, 11/700, 4 px ring in the parent fill (unchanged) |
| "New" / "Live" tags | `danger_fill`, chip radius (unchanged) |
| Mention / channel / @everyone / @here / slash pills (markdown.rs `pill`) | `chip` fill (purple), **`accent_ink` text** (was `link`). **Hover: the fill stays; the label gets a 1 px underline in `accent_ink`** (2 px in from both ends, 1 px above the bottom). The old hover fill (fg @ 28 %) drops ink to 4.4:1 |
| App / bot tag | `chip` + `accent_ink` |
| My reaction (`reaction_chip`) | `chip` fill + 1 px **`accent_ink`** border; count in **`text`** (was `link`). Hovered mine: `chip.blend(hover)`. Others unchanged |
| My poll pick (polls.rs) | `chip` fill + 1 px `accent_ink` border + `accent_ink` check (were `link`) |
| Role chip, status chip | unchanged |

### 9.9 Callout (from Illegalcord, in our look)

`messages::callout(ui, pal, kind, text)` with `enum Callout { Info, Warn, Danger }`, about 25
lines next to `button`.
- Full width, radius card (12), padding 12 top/bottom, 12 right; a 16 px icon at x+12, text from
  x+36, ui 14 `text`, wrapped.
- Colour per kind: Info `link` + `Icon::Info`; Warn `attention` + `Icon::Alert`; Danger `danger` +
  `Icon::Alert`. Never purple (purple is "you", not "info").
- Fill: kind colour @ 8 %. Border: 1 px kind colour @ 70 % on dark, 100 % on light, 1.5 px 100 %
  on HC. (Dark border vs chat: info 4.74, warn 5.87, danger 3.85. Light: 5.59, 3.9, 5.45.)
- Icon on fill ≥ 3.85; text on fill ≥ 14.
- First use: Settings → Advanced, the hardware acceleration hint becomes an Info callout with the
  same words (user_settings/app.rs).

### 9.10 Chat

- Mention / keyword row (messages.rs): `mention_highlight` tint + 2 px **`accent_ink`**
  bar (was `attention`). Hovered: tint + `hover` (unchanged). A reply to me gets it too.
- Jumped-to flash: `link` (unchanged: it is navigation).
- A new message in a calm chat rises in (§7); busy chats stay still.
- Composer (composer.rs): E1 `surface`, radius 12, no border (HC `rule`), **sheen**.
- Embeds, code, attachments, hover toolbar, separators, "New": unchanged.

### 9.11 Main window panes and lists

- Channel card, channel rows, DM rows, categories, voice users: unchanged. No purple in rows.
- Member list (and thread panel, search results): new 1 px `rule` on the left edge (§6.3).
  Streaming dot and icon in pink `live` (token change, no code).
- User panel (sidebar.rs `user_panel`): E1 `surface`, radius 12, 56 tall, 8 margin,
  **1 px `rule` outline on every theme** (today light/HC only), **sheen**. Content
  unchanged. Voice section words: "Voice connected" `positive_text`, "Connecting…"
  `readable_on(attention, panel fill)`.
- "Read All" pill (plugins/sidebar.rs): hover becomes `selected` fill + `text` label
  (was `accent` / `accent_text`: hover is never purple).
- Message-link embed bar (plugins/link_embeds.rs): `edge` (was `accent`).
- Mic meter (user_settings/voice.rs): below threshold `tk.attention` @ 70 %, above
  `tk.positive` @ 70 % (were raw hex).

### 9.12 Login and update screens

- Login card (login.rs): radius 20 (helper), `raised`, `rule`, E3, **sheen**, padding as
  today. Title "Sign in to Rustcord" via `pal.display_text`. Raster logo (rustcord-128.png). Primary button fills per §9.1.
- Update screen (update.rs): title via `display_text`; the 320x4 progress bar fills with
  **`accent`** on an `edge` @ 50 % track (progress is "now"). Title-bar pill "Update available"
  stays green.

### 9.13 Installer (`src/bin/rustcord-setup/ui.rs`)

Constants: `FRAME #08070B`, `BACKGROUND #121118`, `SURFACE #1C1A24`, `TEXT #F0EEF6`,
`MUTED #A8A5B6`, `ACCENT #A42BEA`, `ACCENT_TEXT #FFFFFF`, `EDGE #83808F`, `DANGER_FILL #CC3D3F`,
`FOCUS #BE67F0`, `CLOSE_RED #C42B1C` (unchanged). Switch and checkbox
on-state: `ACCENT` + `ACCENT_TEXT`. **Delete `POSITIVE` and `ON_POSITIVE`** (unused afterwards;
CI runs clippy with `-D warnings`). Header comment: "DESIGN.md §1 (Nightshade)"; doc
comment follows.

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
 #RW = the pill and the speech corner on the selected server
```

### 10.1 Rail and speech corner (signature)

The selected server tile gets one sharp corner, bottom left, next to the purple pill. The tile
then reads like a chat bubble the pill speaks from. The logo has the same corner. One shape rule,
two places. It is cheap (no mesh). It survives Saturation 0 % and HC.

```
 hover (20 px white pill)      selected (40 px purple pill)
 |                             |
 |  [  RW  ]  all corners 12   █  [  RW  ]  nw/ne/se 12
 █  [      ]                   █  [      ]
 |                             █  [_     ]  sw 2: the speech corner
```

In `sidebar.rs` (replaces `heat_bar`):

```rust
/// The rail pill (docs/DESIGN.md §10.1): `text` up to 20 px tall, then toward `accent` on the way
/// to the selected 40 px. One flat rect in every theme.
fn rail_pill(p: &egui::Painter, pill: Rect, text: Color32, accent: Color32) {
    let t = pill_t(pill.height());
    p.rect_filled(pill, CornerRadius { ne: 4, se: 4, ..Default::default() }, text.lerp_to_gamma(accent, t));
}
fn pill_t(h: f32) -> f32 {
    ((h - 20.0) / 20.0).clamp(0.0, 1.0)
}
/// A rail tile's corners: `ICON_R` everywhere, the bottom-left tightening to 2 as the pill turns
/// selected (the speech corner, §10.1).
fn tile_corners(t: f32) -> CornerRadius {
    let r = ICON_R as u8;
    CornerRadius { nw: r, ne: r, se: r, sw: egui::lerp(ICON_R..=2.0, t).round() as u8 }
}
```

- Order: today `decorate` runs after the icon is painted. Move the pill tween
  (`motion::value(ctx, resp.id.with("pill"), target, FAST)`, same id) up into `icon_slot`, return
  the height, paint the tile with `tile_corners(pill_t(h))`, then `decorate` paints the pill, the
  badge and the focus ring with that height.
- `paint_guild_icon(.., radius: f32, ..)` takes `CornerRadius` instead (callers: rail tiles pass `tile_corners(t)`, folder minis and the title bar's 16 px
  icon pass `CornerRadius::same(..)` as today). `egui::Image::corner_radius` takes it as is.
- Every tile that can carry the 40 px pill uses `tile_corners`: home, servers, servers inside an
  open folder, a closed folder holding the selected server. The faded drag placeholder and
  the focus ring (`corners` + 2 on each corner) follow the same corners.
- Home tile (`home_button`): fill `surface.lerp_to_gamma(accent, morph)`, the R in
  `accent_ink.lerp_to_gamma(accent_text, morph)` (existing `morph`: hover or selected). This is the
  one place hover may be purple: the tile is the logo. Selected also gets the speech corner.
- Unread (8 px) and hover (20 px) pills stay `text` white; hover never changes the shape.
- Add server, badges, folders, drag: unchanged.

---


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
- Edit name: a name, colour and tag only this device shows (`src/names.rs`). The colour replaces the role colour (through `readable_on_pane`); the tag is a neutral chip (`selected` fill, `text` label, badge 11/700, chip radius, 4 px padding, as tall as the Bot chip) after the guild tag in cozy rows and before the name in compact rows; hovering the name shows the real name and `@username`.
- Avatar 40, author body 15/600 in the role colour, the server tag, the role icon 20 (the highest role with one; `ROLE_ICONS` servers; cozy only, like Discord), Bot, time meta.
- ShowBadgesInChat (plugin): 16 `text_muted` badge glyphs in the decorations after the time (§8.7), 4 apart, tooltip with the badge title.
- Body 15 with line height 21. 16 between groups. Compact mode unchanged.

**Hidden message** (Settings > Content & social > Hidden words): a message from someone else whose text, embed title or description, or forwarded text has one of my words folds into one 20 px line instead of the whole message. `EyeOff` 16 `text_muted` in the avatar column (cozy: x + 20, compact: x + 8), then "Hidden message" (meta 12, `text_muted`), 8 px, "Show" (meta 12/600, `link`). The whole line is one button: `hover` fill, pointing hand, focus ring, Enter or Space shows it; no hover bar or context menu on it. It ends its group (the next shown message of that author gets its header again). Shown, the message draws normally until the channel is left; its uploaded pictures and videos and its embed pictures sit under the spoiler cover labelled "Hidden", also with "Show spoiler content: Always". A reply to a hidden message quotes "Hidden message". My own messages are never hidden.

**Translated** (Translate plugin, after the user picked Translate in the message menu): the translation takes the message text's place (same markdown, "(edited)" stays after it). Under it a 18 px meta line: `Globe` 14 `text_muted`, "Translating…" while waiting; "Translated from German" and, 8 px on, the link "Show original" (meta 12, `link`) when done; on failure `Info` 14 `danger`, the reason in `danger` meta, then the links "Try again" and "Dismiss". Static, no spinner. The text fades in on arrival (FAST); everything else is instant.

**Mentions of me:** `mention_highlight` row fill plus a 2 px `attention` bar on the left
(not accent). A message that matches one of my keyword alerts (Settings > Notifications, in scope, history included) takes the same look.

**Unread divider:** 1 px `danger` line with the "New" tag on the right.

**Hover toolbar:** E2, icon buttons 32.

**Reactions:**
- Pill 24 tall, `surface` fill, count meta.
- Mine: `chip` fill with a 1 px `link` border.

**Embeds:** `surface`, a 4 px left bar in the embed colour, radius control.

**Polls** (ui/polls.rs):
- Card: `surface`, container radius, 16 padding, at most 440 wide (High Contrast: a 1 px `rule`
  edge). Question heading 16/600.
- Answers are rows 40 tall, 8 apart, control radius, body 15/500, one line. Vote view: `hover`
  fill (`selected` when hovered or picked), a radio (one answer) or checkbox (several) on the
  right, the hint "Select one answer" above. Results view: a `hover` track and a bar from the
  left (`chip` for my answers, else `selected`), my answers with a 1 px `link` border and a
  `Check` 16 before the percent (ui 14/600); the winner of a closed poll is 600.
- Footer, 12 above: votes and time left in meta `text_muted`; buttons 32 tall on the right. Vote
  is **Secondary**, not Primary (one primary per view); Show results, Back to voting and
  Remove vote are Ghost.
- Voters popover (a click on a result row with votes): E2 `raised`, 1 px `rule`, container radius,
  280 wide, 8 padding, below the row (above when short of room). Header: the answer in the label
  role (600, muted) and the vote count in meta; rows 32 tall with an avatar 24 and the name body
  15/500, `hover` fill, a click opens the profile; "Show more" (Ghost) pages by 100, a failure
  shows the error and a small Retry.
- Create dialog (520 wide, composer "+" menu, needs SEND_POLLS): Question (300), Answers (55 each,
  2 to start, up to 10, a Close button on each from the third on, Ghost "Add answer"), Duration
  (1 hour to 2 weeks, default 24 hours), the "Allow multiple answers" switch, and the one Primary
  button "Post". Answers can't carry an emoji here.
- **Poll result** (the message Discord posts when a poll ends): `surface`, control radius, 12
  padding, at most 440 wide. `Poll` 16 and "Poll closed" in meta `text_muted`, the question body
  15/600, then `Check` 16 with the winner (ui 14/500) and "9 votes, 45%" in meta, or "The poll ended
  in a tie." / "No one voted.". With a reference to the poll the whole card is a button
  (`surface` blended with `hover`, tooltip "View poll") that jumps to the poll.
- Motion: the bars grow 0 -> share over BASE only after my own Vote or Show results; other
  people's votes and the end of the poll change them without a tween; rows draw 1 px smaller while
  held.

**Forwarded:**
- The forwarded message sits in a frame with the quote bar (3 px wide, radius 2, `text_muted`
  at 45 %), 12 px from the left.
- First line: `Forward` 14 and "Forwarded", meta italic `text_muted`. Then its text, media, and
  an origin line: "#channel" in meta `link` (a click jumps to the message), 8 px, the time in
  meta `text_muted`. No middle dot.

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

Edit name's colour replaces the role colour on a member row and its tag chip follows the name (its width is taken before the name is cut); the name's tooltip shows the real name.

**Group header:**
- Label role "Maintainers" on the left. The count is meta `text_muted`, right-aligned, so counts
  form a column.
- No em dash (members.rs:127 `group_row`).

**Row:** 42 tall, avatar 32 with status, name body 15/500 in the role colour, line 2 meta
`text_muted`. Role icon 16 after the name and server tag, 4 px apart; not on offline rows.

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

**Now playing section**, the top-most section of the panel (Windows and Linux; Settings > Appearance
> Layout > "Show music controls", on by default; `RUSTCORD_NAV_DEMO=music[:paused]` shows a demo track):

```
| [art] Rewriting it in Rust                    [ ❚❚ ]  [ ▶❙ ] |   40 tall + 1 px `rule` hairline
|       Ferris and the Crabs                                    |
```

- Shown while the computer's media session plays, or for 60 s after it paused (so Play stays
  reachable); nothing playing, no title or the setting off: hidden. Never sent anywhere.
- Art 24x24 at x+12, chip radius (`radius / 2`); without a picture a `code_background` tile with
  `Listening` (16) in `text_muted`. Text from x+44: the title in label role (`text`), the artist in
  meta (`text_muted`), each one line with "…"; tooltip "{title} by {artist}".
- Buttons from 8 px inside the right edge, 4 px apart, 32 px hit area, 20 px icon, `text_muted` at
  rest, `text` on hover with the 8 % tint, 97 % while held: Play / Pause ("Play", "Pause") and
  `Next` ("Next track"; disabled at 40 % with "This player can't skip"). Purple: none.
- Appears and leaves over BASE / BASE x OUT (§7); music playing at start shows at once.
- The row's own events wake the UI; no seek bar, no timeline, no frames while a track just plays.

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
  Its popout has the tabs Mentions, Unreads and Keywords (this session's keyword-alert matches as the same cards, the matched word marked; empty states "Get notified when words you care about come up." with a secondary "Set up keyword alerts" button, and "No keyword alerts yet. They stay here until you quit Rustcord.").
- Update button (ui/update.rs, docs/UPDATER_DESIGN.md §15.2), only while a new version is
  available or ready: a pill 24 tall, 8 left of `Inbox` (8 from the end where there is no
  inbox: login, settings), radius control, padding 8. `Download` 16 in `positive`, 6 gap, label
  13/600 in `positive_text`: "Update available" / "Update ready". Hover: `hover` fill; focus
  ring; the tooltip names the action ("Restart Rustcord to install 0.1.1"). While a call runs,
  "Update ready" is disabled (icon and label `text_muted`, tooltip "Installs after your call
  ends"). Green, not purple: it is a state, not the view's primary action.
- **Inbox tabs:** Mentions, Unreads, Keywords, Saved, Recent. Saved (messages kept on this device,
  `src/saved.rs`) shows newest first the same cards as Mentions, each under its own 16 px place
  line; a reminder shows right-aligned there as `Clock` 14 and the time, both `text_muted`. Hover
  bar: `x` Remove, "Remind" (menu: In 1 hour, This evening, Tomorrow, Custom...), "Jump". A row
  that is not loaded says "Loading message..."; it loads only while on screen, one at a time.
  Gone: "This message is no longer available." Empty: "Right-click a message and select Save
  message to keep it here." "Remind me" > "Custom..." is a 440 dialog: Day combo (200) + Time
  combo (96, 30-minute steps), danger line "Pick a time later than now.", Primary "Set reminder".
  No new motion (menus, dialog and toasts as in section 7).

---


### 10.7 Call island (title bar centre)

Lives in `sidebar.rs` (new fn `call_island(ui, cx, row) -> f32`, about 120 lines), called from
`sidebar::title_bar`. No new module.

- **When:** while `cx.state.voice.channel_id` is `Some` (connecting or connected, server voice or
  DM call), also over settings. It takes the place of the centred location label. Cross-fade: the
  island draws with opacity t and scale `lerp(0.92, 1, t)` about its centre, the label with
  opacity 1 − t; t = `motion::anim(ctx, id, in_call, BASE, BASE * OUT)`. When t = 0 only the
  label draws.
- **Pill:** 28 tall (2 px above and below in the 32 px bar), radius 14, fill `raised`, 1 px `rule`
  (HC 1.5 px muted), sheen. Centred on the window. Width = its content, min 160, max
  `min(420, row.width() − 240)` (keeps clear of the inbox button and the update pill). If even
  160 does not fit, no island (label stays).
- **Content, left to right:** 12 pad; 8 px dot (`positive` connected, `attention` connecting; on connect it blends over FAST and pops once 1 → 1.12 → 1 over BASE);
  8; the state word, label 13/600: "Voice connected" in `positive_text` or "Connecting…" in
  `readable_on(attention, raised)`, through `plugins::sidebar::call_timer(ctx, state, word)` so
  the CallTimer plugin shows the time here too; 8; channel name ui 14/600 `text`; 6; server name
  meta 12 muted (none for DM calls); 10; two 24 px round icon buttons 4 px apart, 16 px icons;
  2 pad.
  - Mute: `Mic` / `MicOff`. Muted: `danger` icon on `danger` @ 12 %. Calls
    `call::toggle_mute(cx, true)` (make it `pub(super)`, call.rs). Tooltip "Mute" / "Unmute".
  - Disconnect: `Hangup` in `danger`; hover `danger_fill` with `danger_label` icon. Sends
    `Command::LeaveVoice` (as voice_panel). Tooltip "Disconnect".
  - Too narrow: drop the server name first, then end the channel name with "…". The state word
    stays.
- **Width:** glides over BASE (cubic in-out, `island_width`) when the state word, the channel or whether the server name fits changes; a CallTimer tick or a window resize changes it at once.
- **The text part** (dot to server name) is one button: hover `hover` over that part of the pill;
  click selects the call's channel and closes settings (same as voice_panel);
  tooltip "Open {channel}"; accessible name "Voice connected, {channel}, {server}"; focus ring.
- No pulsing, no speaking halo. With CallTimer on, the existing 1 Hz tick of the user panel also
  updates the island (no extra repaint source).
- The title bar drag area is registered first (chrome.rs), so the island's widgets win over
  it, like the inbox button.

### 10.8 Call stage

Same in every theme, like a video player. Violet-black, no brown:

| Constant | Today | New | Check |
|---|---|---|---|
| `STAGE` | rgb(14,12,12) | `#050407` | |
| `TILE` | rgb(38,34,33) | `#1C1A24` | 1.19:1 on stage (same step as today) |
| `BUTTON` | rgb(56,51,50) | `#2A2833` | FG on it 12.6 |
| `FG` | gray 238 | `#F0EEF6` | 14.94 on tile |
| `FG_MUTED` | gray 170 | `#A8A5B6` | 8.49 on stage, 6.02 on button |
| `DOCK` (new, call.rs) | | `#111017` | FG_MUTED 7.86 on it |

- **Dock** (`controls`, call.rs): paint behind the existing control row, before the
  buttons: a pill around the buttons' bounding rect, 8 px wider on each side and 56 tall, fill
  `DOCK`, 1 px white @ 8 %, sheen white @ 6 %, shadow `[0,12]` blur 32 black 60 %. Buttons, order,
  sizes and positions stay.
- Speaking ring: 2 px `positive` (unchanged). Muted / camera off: `danger` icon on `danger` @ 12 %.
  Hang up `danger_fill`. "Live" pill `danger_fill`. "Watch stream" Primary. Tile tint toward the
  user's colour (30 %) unchanged.
- DM call area and incoming card (dm_call.rs): the same constants.

### 10.9 Focus mode

One key (Keybinds > Navigation > "Focus mode", default Ctrl+Shift+F) folds the rail (72), the
channel list card (its width) and the floating user panel away to the left; the same key brings
them back. Not persisted: every start shows the lists.

- The page takes the full width under the title bar. In the chat, the message list and the
  composer form a column centred in the page, at most 800 px wide (`FOCUS_CHAT_W`); the header,
  the member list and the search results keep their width.
- Motion: one value (`anim(.., LARGE, BASE)`): the columns narrow while their content slides
  left at full width (never squeezed), clipped to the column; the chat column widens and
  narrows with them (`focus_column_w`). Interruptible, instant with reduced motion, frames
  only while it moves. At rest in focus mode the panes are not created at all.
- The centring wrapper is always present (also at rest), else the chat's widget ids, and with
  them its scroll position, would change when the mode turns on.
- Toast, once per session: "Focus mode is on. Press Ctrl+Shift+F to bring the lists back."
- The title bar, Inbox, call island, quick switcher and navigation keys work as before.

### 10.10 Tabs and split view

Session only; every tab and split comes from a click. Middle-click a channel or DM row (or its
menu item "Open in new tab", hidden for the open channel) opens a background tab right of the
active one; with none yet the open channel becomes the first tab. At most 8 (toast "You can open
up to 8 tabs. Close one first."). An open tab pops once.

- **Tab bar:** only with two or more tabs, above the chat header, spanning the page and the member
  list or search results (not the split pane). 40 tall, `background`, padding 8, tabs 32 tall, 4
  apart, width `clamp((bar - 16 - 4(n-1)) / n, 48, 200)`. A tab: control radius, a 16 px channel
  icon (`text_muted`) or DM avatar at x+8, the name at x+32 in ui 14/500 (`text_muted` at rest,
  `text` active or hovered, 600 when unread), a mention badge (`danger_fill`) at the right, and
  on hover and on the active tab `Close` 16 in a 20 px hit square (`hover` circle on hover). Active:
  `selected` fill (never purple); hover: `hover`. Tooltip "#general in Rust Workshop" / "@Alice".
  Narrow tabs show the icon only.
- **Actions:** click selects, middle-click or the close button or Delete closes (the active tab
  closes to its right neighbour, else the left one), drag reorders, right-click: "Open in split
  view" (non-thread text channels, not the open one), "Close tab", "Close other tabs". Opening a
  channel from anywhere replaces the active tab's channel (a channel that already is a tab
  activates that tab); tabs of channels that disappear drop out.
- **Split view:** "Open in split view" on a channel or DM row or a tab. The channel shows right of
  the open one, width `floor((window - rail - list) / 2)`; the member list hides meanwhile and
  the thread panel excludes it (opening one closes the other). Windows under 1000 px: the split
  pane takes the page. Left edge a 1 px `rule`; header 48 + 1 px `rule` like the thread panel's
  (margin 16 / 12, channel icon 20 or DM avatar 24, name heading 16/600, `Close` button 32 "Close
  split view"), then the full chat of that channel with its own scroll and draft.
- **Focus:** the pane last pressed in, or where the keyboard focus moved, holds it; the other
  header's title and icon turn `text_muted`. Only the focused pane counts as read (acks, no
  notification for its new messages) and only its composer sends typing.

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


---

## 12. Popouts, settings, login, desktop surfaces

### 12.1 Settings shell: a floating panel

#### 12.1.1 What it looks like

```
+-title bar (frame; live: drag, caption buttons, the island)------------------------------+
| frozen, blurred picture of the app, under the scrim                                     |
|   +-panel: E3, radius 20, inset 16 top / 24 sides and bottom, max 1180 wide---------+   |
|   | nav 248 (sidebar fill)        | page (background fill)                      [x] |   |
|   |  (av) rosc                    |                                                 |   |
|   |       Edit profile  (pen)     |      Appearance        <- title 20/700, -0.3    |   |
|   |  [(q) Search settings     ]   |                                                 |   |
|   |  User settings                |      Theme             <- heading 16/600        |   |
|   |  [(P) My account          ]<- |      ( ) ( ) ( ) ...                            |   |
|   |   (ID) Profiles               |                                                 |   |
|   |   ...                         |      column ≤ 740, centred, ≥ 40 each side      |   |
|   |   (->) Log out    (danger)    |                                                 |   |
|   +-------------------------------+-------------------------------------------------+   |
+-----------------------------------------------------------------------------------------+
```

#### 12.1.2 Backdrop: a frozen, blurred picture (no live app under settings)

Why not the live app: drawing the chat under the panel would let `messages::show` consume Esc
and call `mark_read` (an ack a normal client would not send), and app-wide shortcuts would fire.
Today's auto-acks are already gated on `!chat_hidden()` (mod.rs); keep it that way.

Flow, in `main_layout` (ui/mod.rs), state in `ViewState`:
`settings_backdrop: Option<egui::TextureHandle>`, `backdrop_asked: bool`,
`backdrop_slot: Arc<Mutex<Option<egui::ColorImage>>>`.

1. The first frame `chat_hidden()` is true and `backdrop_asked` is false: draw the app exactly as
   when settings are closed (fall through to the normal layout), set `backdrop_asked = true`,
   and call `ctx.request_screenshot(callback)` (egui 0.37 API, already used at mod.rs).
   Request a repaint. Opening waits one frame (~16 ms), well inside 100 ms.
2. The callback: copy to an `image::RgbaImage`, `image::imageops::thumbnail` to 1/8 of each side,
   then `image::imageops::blur` with sigma 2.5, store the small `ColorImage` in the slot, drop the
   big buffers, `request_repaint`. (`image` is already a dependency.)
3. Following frames: if the slot holds an image, `ctx.load_texture("rc_settings_backdrop", img,
   TextureOptions::LINEAR)` into `settings_backdrop`. Draw it over the central rect (the part of
   the picture under the title bar: uv y from `TITLE_BAR_H / screen height` to 1), then the
   `scrim` (`tk.scrim`), then the panel. No picture yet (or none at all): the `guild_bar` colour,
   then the scrim. That is also the defined fallback.
4. `chat_hidden()` false again: drop the texture and reset `backdrop_asked`.
5. Theme change while open (`cx.view.theme_dirty`): drop the texture, keep `backdrop_asked`
   true (no new capture until the next opening): the panel floats over the plain frame colour.
6. Window resize while open: the picture stretches (it is blurred; no special code).

Cost: one capture per opening. Transient memory: the full frame (8 MB at 1080p, 33 MB at 4K)
plus its copy, freed in the callback. The kept texture is ≤ 0.5 MB. Nothing repaints afterwards.
Switching from user settings to server settings keeps the picture.

#### 12.1.3 Panel geometry and paint (`settings_layer`, mod.rs)

- `content` = the central rect under the title bar. Inset: top 16, left / right / bottom 24
  (windows narrower than 960 px: 12 everywhere). Width at most 1180, centred.
- Paint: E3 shadow (`style.visuals.window_shadow.as_shape(panel, 20)`); nav part fill
  `colors.sidebar` with corners `nw`/`sw` 20; page part `colors.background` with `ne`/`se` 20;
  1 px `rule` outline over the whole panel (HC 1.5 px muted); a 1 px `rule` vline between nav and
  page; sheen on the top edge. Opaque theme colours in every theme (gradient themes too: the
  window gradient cannot follow the rounded corners; the scroll fades use the flat branch).
- Nav column: **248 wide** (208 under 960 px), padding 12 left/right, 16 top, 24 bottom.
- Page: scroll area right of the nav. Column width `min(740, page_w − 80)`, centred:
  left padding `max(40, (page_w − col_w) / 2)`; top 40, bottom 64.
- Close: a 32 px ghost `vector_button(Icon::Close, "Close settings (Esc)")`, 16 px from the
  panel's top-right corner. The 60 px tools column and the round 36 px ring button go away.
- Esc and × close (unchanged logic). A click on the scrim does nothing.
- Motion (unchanged helper): the whole layer fades in and grows 0.98 → 1 about the panel centre
  over LARGE; closing is instant. Reduced motion: instant.
- The CentralPanel frame under it: `fill.frame` (the picture covers it).
- Keep: `pickers::close_if_open`, `inbox_open = false` and `settings_themes` handling exactly as
  today: `cx.view.settings_themes = None` must only run while settings are closed.

#### 12.1.4 Nav (`user_settings::nav`, `nav_item` / `nav_heading` mod.rs)

- **User header** (new, top of the user-settings nav), 56 tall, radius r, hover `hover`: avatar
  40 with its status dot (ring in the nav fill) at x+8; name body 15/600 `text` at x+56; under it
  "Edit profile" meta 12/500 muted with a 12 px `Icon::Edit` 4 px after the words. Click: opens
  Profiles. Focusable, focus ring; accessible name "Edit profile, {name}". 8 px below it, the
  search field.
- **Search field**: 32 tall well, hint "Search settings", text from x+32, a 16 px `Icon::Search`
  in muted at x+10 (`TextEdit::margin(left 32, right 8, top 6, bottom 6)`, icon painted over).
- **Group heading**: label role 13/600 muted, 16 above (12 for the first), 4 below. **No
  `ui.separator()` between groups** (remove and).
- **Nav row** (`nav_item(ui, theme, icon: Icon, label, selected, danger)`): **36 tall**, radius r,
  2 px apart. 18 px icon at x+10, label body 15/500 at x+38.
  Rest: icon and label muted. Hover: `hover` fill, `text`. Selected: `selected` fill, label
  `text`, **icon `accent_ink`** (the only purple in the nav). Danger (Log out, Delete channel):
  icon and label `danger`. Focus ring as today.
- Icons per user-settings page (`Page::icon()`, 4 new icons in §8.6):

| Page | Icon | Page | Icon |
|---|---|---|---|
| My account | `Person` | Voice & video | `Mic` |
| Profiles | `IdCard` | Clips | `Video` |
| Profile cosmetics | `Star` | Chat | `Message` |
| Content & social | `Shield` | Notifications | `Bell` |
| Data & privacy | `Lock` | Keybinds | `Keyboard` (new) |
| Family Center | `Members` | Language | `Globe` |
| Authorized apps | `Key` | Windows / Linux / macOS | `Desktop` |
| Devices | `Phone` | Streamer mode | `Streaming` |
| Connections | `Link` | Advanced | `Gear` |
| Friend requests | `PersonAdd` | Activity privacy | `Eye` |
| Nitro & billing | `Card` | Registered games | `Playing` |
| Appearance | `Palette` (new) | Game overlay | `Screen` |
| Accessibility | `Accessibility` (new) | Plugins | `Puzzle` (new) |
| Local profile | `File` | Theme editor | `Edit` |
| Log out | `Leave` (danger) | | |

- Server settings (server_settings.rs), same rows, no user header: Overview `Gear`,
  Roles `Shield`, Emoji `Smile`, Members `Members`, Invites `Link`, Bans `Ban`; channel pages:
  Overview `Gear`, Permissions `Lock`, Delete channel / category `Trash` (danger).
- Page title (`page_title`, mod.rs): `pal.title_text`. Sections and setting rows: unchanged
  (flat rows with `rule` between them, as in Illegalcord's own settings list; no card around every
  group).


#### 12.1.5 Quick actions

Not in this redesign. Illegalcord's "Quick actions" is one of its plugins; the plugin step owns
it. If it is built later, use Secondary buttons 40 tall with a 20 px icon, in a wrap grid with
8 px gaps, inside one E1 card (radius 12, padding 8).

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

### 12.2 Settings pages
- Nav groups in the label role, sentence case: "User settings", "App settings".
- Selected nav row: `selected` fill only.
- Page title in the title role.
- Section heading in the heading role with 32 above. No rule under it (ui/mod.rs:1292 `section`).
- Notifications page order: Desktop notifications, Flash the taskbar, Quiet hours, (Keyword alerts), Sounds, Custom sounds, Discord's synced groups, Servers. "Quiet hours": a switch row; when on, "From" [time] "to" [time] (two 96 px combos in 30-minute steps), then seven day check rows (Mon to Sun). An error line in `danger`, meta size, under them: "Pick an end time different from the start." or "Pick at least one day."
- Account popout: directly under the status row a pause row ("Pause notifications", "Paused until 14:30" or "Quiet hours until 07:00"), a bell (muted: bell off) at x+16 and the submenu arrow. Its flyout (same helper as the status flyout, 188 px, 32 px items): "For 1 hour", "Until tomorrow", and "Resume notifications" only while paused. A pick toasts the result and closes the popout.
- Status flyout presets: under the four statuses a separator, then the presets (32 px rows, emoji at x+14, text at x+30, check while it is my status, × on hover "Remove preset", tooltip "Clears after …"); a click sets it and closes the popout. Set a custom status: "Save as preset" (secondary) at the left of the footer.
- Notifications > Custom sounds: section "Custom sounds" with a hint ("Your own WAV or Ogg Opus files. Only the first 10 seconds play. Where there's none, Rustcord's sound plays."), a "Where" row (220 px combo: Everywhere, then every place with a file by name, plus the place a menu opened; meta "Right-click a server, channel or DM and pick Custom sounds to add it."), then a setting row each for Message, Mention and Incoming call (the call only for Everywhere and DMs). Meta: the file's name, else "Rustcord's sound" (Everywhere) or "Same as everywhere". Controls right to left: 28 px play button, 8 px, × "Remove this file" (only while a file is set), 8 px, secondary "Choose file" (WAV or Ogg Opus; toasts "This file is over 5 MB. Pick a shorter sound." / "Rustcord can't play this file. Pick a WAV or Ogg Opus file."). The context menus of servers, channels and DMs have a "Custom sounds" item (no icon, after Notifications) that opens it for that place.
- Notifications > Keyword alerts: section "Keyword alerts" with a hint, then a 320 px field ("Add a word or phrase", Enter adds and keeps focus) and a secondary "Add" button; errors in `danger` meta under it. One setting row per keyword (the word as title, a "Whole word" check and a × on the right). Once there is a keyword: an "All servers" switch row; off, a row per server or channel (× on the right) and an add row (Server combo, Channel combo with "Whole server", "Add").
- Content & social > Hidden words: like Keyword alerts without the places (section "Hidden words" first, a hint, the 320 px field + Add, rows with Whole word and ×). Local only; the words apply everywhere, DMs included.
- Notifications > Sounds: section "Sounds", the "Play sounds" switch row, a "Volume" label with a slider (a release plays the message sound), then a row per sound (title, optional meta) with a 28 px play button, 8 px, and a switch on the right. Play buttons are disabled while deafened (a hint says so); the rows are disabled while Play sounds is off.
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
- **Tray menu:** "Open Rustcord", Mute, Deafen, "Pause notifications for 1 hour", "Pause notifications until tomorrow", "Resume notifications" (greyed unless paused), "Quit Rustcord". Tooltip "Rustcord".
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
- **Settings > Windows / Linux / macOS > App lock** (between Window and Updates; crate `lock`): a
  screen lock, not encryption. Switch row "Lock Rustcord with a PIN" ("Asks for your PIN when
  Rustcord starts. It only hides the window: messages and images cached on this computer are not
  encrypted."); on opens the "Set a PIN" dialog (440; "PIN" and "Repeat PIN" 40 px password
  fields, digits only, 4 to 16; errors in `danger` meta: "Use 4 to 16 digits.", "The two PINs are
  different."; "Set PIN" primary) and stays off until a PIN is saved; off removes it ("App lock
  is off"). While on: "Unlock with Windows Hello" (Windows only; disabled with "Windows Hello
  isn't set up on this PC."), "Lock after" (Never, 1 minute, 5 minutes, 15 minutes, 30 minutes,
  1 hour), "Lock when hidden in the tray" (macOS: "menu bar"). Test hooks: `RUSTCORD_NAV_DEMO=lock`,
  `lock:wrong`, `lock:hello`, `lock:forgot`, `lock-setup`, `lock-rows` (unlocked, for the rows).
- **Lock screen** (ui/lock_screen.rs): like the update screen, it replaces the main window under
  the 32 px title bar (Rustcord's name there): flat `guild_bar` page, a centred 320 px column, no
  card. Logo 56; 24; "Rustcord is locked" in the title role; 8; "Enter your PIN." `text_muted`;
  24; the PIN field (320 x 40, `code_background`, 1 px `edge`, 2 px focus ring, focused when the
  screen appears, Enter unlocks); 8; the error in `danger` meta ("Wrong PIN. Try again.", "Too
  many tries. Wait 30 seconds, then try again.", "Rustcord can't find your PIN. Sign out to
  remove the lock." with a secondary "Try again", "Windows Hello didn't unlock Rustcord."); 16;
  "Unlock" (primary, 320 x 40); 8; "Use Windows Hello" (secondary, Windows with Hello on); 16;
  "Forgot your PIN?" in the link colour, which opens "Sign out and remove the lock?" (danger
  "Sign out": the PIN is removed, the lock turned off, the account signed out and the cache
  cleared; nobody is ever locked out). Locking and unlocking are instant (a fade would show the
  private screen longer); a wrong PIN shakes the column like the login card (3 swings of 6 px,
  LARGE, none with Reduced motion). No frames while idle: one wake at the end of the 30 s
  pause. While locked: notifications show "Rustcord" / "New message", no read acks, keybinds
  off, sounds still play. It locks at start, after the idle time (keys, clicks, wheel and text
  only) and when hidden to the tray.
- **Installer:** title "Rustcord setup"; heading "Install Rustcord"; option rows use the same
  sentence case; buttons "Install" (primary) and "Cancel". Done: "Rustcord is installed" with
  "Open Rustcord".
- **Uninstaller:** "Remove Rustcord".
- **Update screen** (ui/update.rs, copy and states: docs/UPDATER_DESIGN.md §15.1): replaces the
  login / main window under the 32 px title bar (Rustcord's name there, as on login). Flat
  `guild_bar` page, a centred column, no card: logo 56; 24; title in the title role, `text`
  ("Downloading update 0.1.1", "Installing update", "Restarting", "Update failed"); 16; a
  320 × 4 progress bar, radius 2, track `edge` @ 50 %, fill `accent`; 8; one meta
  line `text_muted` ("7.3 MB of 12.0 MB"); 24; buttons 40 tall, ghost "Not now" left of the
  one primary ("Install", "Continue"). A failure shows `Alert` 16 and the message in `danger`
  (ui role) in the bar's place. Enter = the primary, Esc = "Not now" / "Continue". Static: it
  repaints only when the updater reports progress.
- **Settings > Windows / Linux / macOS > Updates:** switch row "Install updates automatically"
  (description by install kind; disabled with its reason on copies that can't update
  themselves and in development builds), then a row "Version 0.1.0" whose description says
  what the updater is doing, with a secondary "Check for updates".

**Quick switcher** (switcher.rs, Ctrl+K; docs/FEATURES_NOTES.md §4.9)
- A modal under the title bar, 520 wide (narrow windows: window − 80, at least 200), radius 2r,
  `raised` fill, 16 px padding, scrim behind. It has no motion: no fade, no grow, no ghost.
- Input 48 tall, title-role text, `code_background` well, radius r, the 2 px `focus` ring in place
  of a border while focused. A 20 px `Search` icon in `text_muted` 12 px from the left; hint "Jump to a channel, server or person".
- 8 px, then the list: rows 32 tall, 2 apart, full width, radius r; up to 8 rows (3 to 8 by window
  height), so typing never resizes the modal. Selected row `selected`, hovered `hover`, both at once.
- Row: 20 px icon (server icon, channel kind icon, avatar), name in body 500 (600 when unread),
  on the right the server name (channels) or the username (people) in the meta role `text_muted`
  (at most 40 % wide, no middle dots), then a mention badge.
- Up / Down move the selection (wrapping), Enter opens it, a click opens it, Esc or a click on the
  scrim closes. Empty text lists recent and unread channels; no hits say "No matches. Try fewer letters."
- Footer: 8 px, a 1 px `rule` hairline, 8 px, then "Start with @ for people, # for text
  channels, ! for voice channels or * for servers." in meta `text_muted`.

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


---

## 14. Logo: the speech-corner R

A new mark, not a recolour. A geometric R: the stem's top is round (radius 48), its foot is sharp,
the leg leaves the bowl at an angle. The tile has three corners at 112.64 (the old icon's radius,
macOS-like) and one speech corner, bottom left, at 24 (5 % of the side, the same share as the
rail's 2 px on 40 px). Dark tile lit from above, purple R lit from the top left. The tray mark is
the inverse: a purple tile with the R cut out. Previews of exactly these files (rendered with
headless Edge) were checked at 256, 64, 48, 32 and 16 px on dark and light taskbars:
`C:\Users\uint65_t\AppData\Local\Temp\claude\0168895bbdad278c\12bb50f8-cf50-40cc-b54a-78fc768ccc96\scratchpad\redesign\judge-tools\preview.png`.

### 14.1 Geometry (512 grid, y down)

Tile with the speech corner:
```
M112.64 0H399.36A112.64 112.64 0 0 1 512 112.64V399.36A112.64 112.64 0 0 1 399.36 512H24A24 24 0 0 1 0 488V112.64A112.64 112.64 0 0 1 112.64 0Z
```

R, big (≥ 48 px; stroke 68): one outline (stem, bowl and leg as a union) plus the counter hole.
Stem x 136-204; bowl y 112-292 with its right side a half circle r 90 about (278, 202); counter
x 204-300, y 180-224, right side r 22; leg from the bowl bottom to the baseline, bottom edge
x 310-394 at y 400, slope (82, 108). The leg's right edge meets the bowl's arc at (308.07, 286.83).
```
M136 400V160A48 48 0 0 1 184 112H278A90 90 0 0 1 308.07 286.83L394 400H310L228 292H204V400ZM204 180H278A22 22 0 0 1 278 224H204Z
```

R, small (≤ 32 px; stroke 80, smaller counter, bigger in the tile): stem x 124-204; bowl y
104-296, r 96 about (276, 200); counter y 184-216, r 16; leg bottom x 304-400 at y 408, slope
(88, 112); junction (307.75, 290.6).
```
M124 408V152A48 48 0 0 1 172 104H276A96 96 0 0 1 307.75 290.6L400 408H304L216 296H204V408ZM204 184H276A16 16 0 0 1 276 216H204Z
```

### 14.2 `assets/logo.svg` (app icon ≥ 48 px, README, installer, 512 / 1024 PNG for the rich presence art)

```svg
<svg xmlns="http://www.w3.org/2000/svg" width="512" height="512" viewBox="0 0 512 512">
<defs>
<linearGradient id="tile" gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="0" y2="512">
<stop offset="0" stop-color="#221D30"/><stop offset="1" stop-color="#15121D"/>
</linearGradient>
<radialGradient id="light" gradientUnits="userSpaceOnUse" cx="256" cy="176" r="300">
<stop offset="0" stop-color="#A42BEA" stop-opacity="0.20"/><stop offset="1" stop-color="#A42BEA" stop-opacity="0"/>
</radialGradient>
<linearGradient id="edge" gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="0" y2="512">
<stop offset="0" stop-color="#FFFFFF" stop-opacity="0.12"/><stop offset="1" stop-color="#FFFFFF" stop-opacity="0.12"/>
</linearGradient>
<linearGradient id="glyph" gradientUnits="userSpaceOnUse" x1="136" y1="112" x2="394" y2="400">
<stop offset="0" stop-color="#E6CCFF"/><stop offset="0.5" stop-color="#BE6BF7"/><stop offset="1" stop-color="#A42BEA"/>
</linearGradient>
</defs>
<path fill="url(#tile)" d="M112.64 0H399.36A112.64 112.64 0 0 1 512 112.64V399.36A112.64 112.64 0 0 1 399.36 512H24A24 24 0 0 1 0 488V112.64A112.64 112.64 0 0 1 112.64 0Z"/>
<path fill="url(#light)" d="M112.64 0H399.36A112.64 112.64 0 0 1 512 112.64V399.36A112.64 112.64 0 0 1 399.36 512H24A24 24 0 0 1 0 488V112.64A112.64 112.64 0 0 1 112.64 0Z"/>
<path fill="none" stroke="url(#edge)" stroke-width="2" d="M112.64 1H399.36A111.64 111.64 0 0 1 511 112.64V399.36A111.64 111.64 0 0 1 399.36 511H24A23 23 0 0 1 1 488V112.64A111.64 111.64 0 0 1 112.64 1Z"/>
<path fill="url(#glyph)" fill-rule="evenodd" d="M136 400V160A48 48 0 0 1 184 112H278A90 90 0 0 1 308.07 286.83L394 400H310L228 292H204V400ZM204 180H278A22 22 0 0 1 278 224H204Z"/>
</svg>
```

Contrast: the R's darkest stop `#A42BEA` on the tile's darkest `#15121D`: 3.6:1; its lightest
on the lightest tile: > 10:1.

### 14.3 `assets/logo-small.svg` (16, 20, 24, 32 px PNGs and ICO frames)

```svg
<svg xmlns="http://www.w3.org/2000/svg" width="512" height="512" viewBox="0 0 512 512">
<defs>
<linearGradient id="rim" gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="0" y2="512">
<stop offset="0" stop-color="#1D1829"/><stop offset="1" stop-color="#0E0C15"/>
</linearGradient>
<linearGradient id="tile" gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="0" y2="512">
<stop offset="0" stop-color="#302848"/><stop offset="1" stop-color="#1D182C"/>
</linearGradient>
<linearGradient id="glyph" gradientUnits="userSpaceOnUse" x1="124" y1="104" x2="400" y2="408">
<stop offset="0" stop-color="#F4EBFF"/><stop offset="0.55" stop-color="#D2A6FF"/><stop offset="1" stop-color="#B567F5"/>
</linearGradient>
</defs>
<!-- The dark rim is a fill, not a stroke over the tile: its anti-aliased edge stays dark (a
     stroke over the lighter tile left light corner pixels, a light ring at 16 / 32 px). -->
<path fill="url(#rim)" d="M112.64 0H399.36A112.64 112.64 0 0 1 512 112.64V399.36A112.64 112.64 0 0 1 399.36 512H24A24 24 0 0 1 0 488V112.64A112.64 112.64 0 0 1 112.64 0Z"/>
<path fill="url(#tile)" d="M112.64 32H399.36A80.64 80.64 0 0 1 480 112.64V399.36A80.64 80.64 0 0 1 399.36 480H48A16 16 0 0 1 32 464V112.64A80.64 80.64 0 0 1 112.64 32Z"/>
<path fill="url(#glyph)" fill-rule="evenodd" d="M124 408V152A48 48 0 0 1 172 104H276A96 96 0 0 1 307.75 290.6L400 408H304L216 296H204V408ZM204 184H276A16 16 0 0 1 276 216H204Z"/>
</svg>
```

Contrast: the R's darkest stop `#B567F5` on the tile `#1D182C`: about 4.9:1.

### 14.4 `assets/logo-mono.svg` (Windows tray, macOS menu bar template)

The tile and the R in one even-odd path: the R becomes a hole, its counter stays filled.

```svg
<svg xmlns="http://www.w3.org/2000/svg" width="512" height="512" viewBox="0 0 512 512">
<path fill="#A42BEA" fill-rule="evenodd" d="M112.64 0H399.36A112.64 112.64 0 0 1 512 112.64V399.36A112.64 112.64 0 0 1 399.36 512H24A24 24 0 0 1 0 488V112.64A112.64 112.64 0 0 1 112.64 0ZM124 408V152A48 48 0 0 1 172 104H276A96 96 0 0 1 307.75 290.6L400 408H304L216 296H204V408ZM204 184H276A16 16 0 0 1 276 216H204Z"/>
</svg>
```

On a dark taskbar (`#202020`) 3.19:1, on a light one (`#F3F3F3`) 4.60:1. The macOS template uses
only the alpha (a tile with an R-shaped hole).

### 14.5 Generated files

`cargo run --example export_icons` (examples/export_icons.rs, unchanged) writes
`assets/icons/rustcord-{16,32,48,64,128,256,512,1024}.png`, `assets/icons/rustcord-tray-36.png`,
`assets/rustcord.ico`, `assets/rustcord-tray.ico`. `build.rs` embeds the ICOs; main.rs (64 px
window icon), login.rs / update.rs (128), the installer (128), notify/linux.rs and macos.rs pick
the PNGs up; macOS `.icns` and Linux hicolor icons are built from them at package time.

### 14.6 In the app (`sidebar::logo_mark`)

New signature, drawn from five primitives (shorter than today's band shape):

```rust
/// Rustcord's mark (assets/logo.svg, §14): `tile` with `corners`, the R in `glyph`, its counter
/// cut in `tile`. Below 32 px the bolder small R (logo-small.svg).
pub(super) fn logo_mark(p: &egui::Painter, rect: Rect, corners: CornerRadius, tile: Color32, glyph: Color32)
```

k = `rect.width() / 512`; points are `rect.min + (x, y) * k`; radii `(r * k).round() as u8`.

| Primitive | Big (≥ 32 px) | Small (< 32 px) |
|---|---|---|
| 1. tile | `rect_filled(rect, corners, tile)` | same |
| 2. bowl | rect (136,112)-(368,292), `CornerRadius { nw: 48, ne: 90, se: 90, sw: 0 }`, glyph | rect (124,104)-(372,296), `{ nw: 48, ne: 96, se: 96, sw: 0 }` |
| 3. stem | rect (136,112)-(204,400), `{ nw: 48 }`, glyph | rect (124,104)-(204,408), `{ nw: 48 }` |
| 4. leg | `convex_polygon` (200.67,256) (284.67,256) (394,400) (310,400), glyph | (187.71,260) (283.71,260) (400,408) (304,408) |
| 5. counter | rect (204,180)-(300,224), `{ ne: 22, se: 22 }`, **tile** | rect (204,184)-(292,216), `{ ne: 16, se: 16 }` |

The leg polygon starts inside the bowl's lower stroke (y 256 / 260), so the union matches the
SVG outline with no seam, and it never touches the counter.

| Place | Rect | Corners | Tile | Glyph |
|---|---|---|---|---|
| Home tile (rail) | the 40 px icon | `tile_corners(t)` | `surface.lerp_to_gamma(accent, morph)` | `accent_ink.lerp_to_gamma(accent_text, morph)` |
| Title bar on DM home (sidebar.rs) | 16 px | `{ nw: 4, ne: 4, se: 4, sw: 1 }` | `accent` | `accent_text` |
| `chrome::app_label` (login, update title bar, chrome.rs) | 16 px | same | `accent` | `accent_text` |

The glyph at 40 px is 22.5 px tall (today's mark: about 22).


---

## 15. Change list

What the Nightshade redesign changed, in the order it was built. Owners follow the module table
in `docs/ARCHITECTURE.md`.

| # | Change | Where |
|---|---|---|
| 1 | Theme: Nightshade and Lilac with a purple primary; `theme::renamed` for old settings; new tokens (`accent`, `on_accent`, `accent_ink`, `sheen`); pink `live`; gradient bases | `src/theme/`, `src/config.rs` |
| 2 | Shared widgets: purple on-states, radius tiers (float 2r, layer 2.5r), lit edges, bold tight titles, callout, press at 97 % | `ui/messages.rs`, `ui/motion.rs`, `theme/apply.rs`, `ui/mod.rs` |
| 3 | Chat: purple mention rows and chips, my reactions and poll picks | `ui/messages.rs`, `ui/markdown.rs`, `ui/polls.rs`, `ui/composer.rs` |
| 4 | Rail: the speech corner, a purple pill, the new mark | `ui/sidebar.rs`, `ui/chrome.rs` |
| 5 | Call island in the title bar | `ui/sidebar.rs`, `ui/call.rs` |
| 6 | Call stage: violet-black stage and a floating dock | `ui/call.rs`, `ui/dm_call.rs` |
| 7 | Four new icons (palette, accessibility, keyboard, puzzle) | `ui/icons.rs` |
| 8 | Settings: a floating panel over a frozen, blurred picture of the app | `ui/mod.rs` |
| 9 | Settings nav: user header, search icon, a line icon per row | `ui/user_settings.rs`, `ui/server_settings.rs` |
| 10 | Theme editor says "Primary" | `ui/user_settings/theme_editor.rs` |
| 11 | Login, update screen, friends, mic meter, demo copy | `ui/login.rs`, `ui/update.rs`, `ui/friends.rs` |
| 12 | Logo: the speech-corner R | `assets/` |
| 13 | Installer colours | `src/bin/rustcord-setup/ui.rs` |

### 15.1 What must not change

- Main-window layout and sizes (user-asked exceptions: focus mode, §10.9; tabs and split view, §10.10): title bar 32; rail 72, tiles 40, 8 apart, `ICON_R` 12; channel
  list 303 (192-360) and its 12 px card corner; chat header 48 (+1); rows 32, DM and member rows
  42, categories 24; member list 264; user panel 56 + 8 margin; composer min 44; message avatar 40
  at x 16, text at x 72, 16 between groups, line height 21; compact mode; menu rows 32; buttons
  32 / 40; inputs 40 / 32; switch 40x24; checkbox / radio 20; icon buttons 32 with 20 icons.
- Status meanings and shapes: online green dot, idle amber crescent, DnD red bar, offline ring,
  mobile phone, streaming dot with a play cut-out (now pink); red badges, red "New", red "Live".
- Discord data colours (a colour given with Edit name replaces that person's role colour on this screen): role colours, profile colours and effects, platform colours, emoji skin
  tones, avatar initial hues, MoreUserTags chips; Windows `CLOSE_RED`; media tokens.
- `ThemeColors` fields and the TOML format; `corner_radius` 8 / HC 4; font size 15 / HC 16; the
  23 gradient presets and their names; the Saturation setting.
- Motion tokens and helpers (only `pressed` changes); reduced motion; sounds (only an internal
  name changes).
- Behaviour: no read acks while settings are open (auto-acks stay gated on `!chat_hidden()`);
  keyboard shortcuts, menu contents, focus order; nothing new is sent to Discord.
- First frame < 1 s, idle RAM < 150 MB, 0 frames when idle.

---

## 16. Verification

Use a scratch copy of `tests/fixtures/home` with `RUSTCORD_HOME`, `RUSTCORD_OFFLINE=1` and
`RUSTCORD_WINDOW_SIZE=1280x800`. `scripts/readme-shots.py` shows the offline screenshot recipe;
`docs/ARCHITECTURE.md` lists the `RUSTCORD_*` hooks. Then `cargo clippy --locked --all-targets --
-D warnings` and `cargo test --locked`.

### 16.1 Screenshots (offline fixture, 1280x800, recipe in `scripts/readme-shots.py`)

D = Nightshade, L = Lilac, H = High Contrast, G = Chroma Glow, X = Hexagon.

| Shot | Env | Themes | Look for |
|---|---|---|---|
| main | `OPEN=members,status:online MSG_DEMO=online` | D L H G X | near-black panes, purple pill + speech corner, mention rows purple, member list hairline |
| rail-hover | `POINTER=36x140` | D L H | white 20 px pill, round tile, tooltip |
| home | `OPEN=friends` | D L H | purple home tile with white R and speech corner; title bar mark; "Add friend" purple |
| chat-states | `MSG_DEMO=hover,new,poll OPEN=status:online` | D L H | hover toolbar E2, "New" red, my poll pick purple |
| composer | `MSG_DEMO=emoji` | D L H | composer sheen (D only), picker radius 16 |
| profile | `OPEN=members,profile` | D L H | popout radius 16, bold tight name, streaming pink |
| menu | `OPEN=server-menu` | D L H | menu radius 12, E2 |
| modal | `OPEN=modal:create-channel` | D L H | radius 20, bold title, sheen, purple radio |
| mid-modal | `+ MOTION_SCALE=10 SCREENSHOT_FRAME=12` | D | grows from the click |
| toast | `OPEN=toast` | D L H | outline, E2, sheen |
| switcher | `OPEN=switcher:gen` | D L H | radius 20 |
| settings | `OPEN=settings:appearance` | D L H G X | floating panel, blurred app behind, nav icons, purple switch, selected icon purple |
| settings-nav | `OPEN=settings:my-account` | D L H | user header, search icon |
| plugins | `OPEN=settings:plugins PLUGINS=all` | D L H | rows, switches |
| settings-narrow | `OPEN=settings:appearance WINDOW_SIZE=900x640` | D | 12 px inset, 208 nav |
| server-settings | `OPEN=server-settings` | D L H | same shell, server icons per row |
| advanced | `OPEN=settings:advanced` | D L H | info callout |
| voice | `OPEN=voice:1565443437363200014` | D L H | island: green dot, "Voice connected", Lounge, Rust Workshop, mute, disconnect |
| connecting | `OPEN=voice:…14,channel:…14 CALL_DEMO=connecting` | D L | "Connecting…" readable on light |
| muted | `OPEN=voice:…14,muted` | D L H | red mic in the island and the user panel |
| island-narrow | `+ WINDOW_SIZE=760x600` | D | server name dropped |
| settings+island | `OPEN=voice:…14,settings:appearance` | D | island above the panel |
| call | `OPEN=voice:…14,channel:…14 CALL_DEMO=people,focus SCREENSHOT_FRAME=90` | D L | stage violet-black, dock |
| ring | `OPEN=ring:1565443437363200027` | D L H | incoming card |
| login | empty home, `LOGIN_TAB=email` | D L H | new raster logo, radius 20, display title |
| update | `UPDATE_DEMO=download` | D L H | purple progress |
| icons | `NAV_DEMO=icons` | D | four new icons |
| focus | `NAV_DEMO=focus:6` | D L H | purple focus ring |
| installer | `RUSTCORD_SETUP_SCREEN=install,done` | (fixed) | Nightshade colours, purple checkbox |

### 16.2 Contrast

- Unit tests assert every rule in §2.3 that code can check: `accent_text` on
  `accent` and on the hover fill ≥ 4.5; `accent` ≥ 3 on `background`; `accent_ink` ≥ 4.5 on chip
  over the mention row and on `raised`; `edge` ≥ 3 on `background` and `surface`; `focus` ≥ 3 on
  `sidebar`; danger ≥ 4.5 on `raised`; fills' labels ≥ 4.5; gradient `presets_are_readable` and
  `names_read_on_every_point_of_a_gradient_pane` still pass.
- By eye in the G and X shots: switch track and pill still visible on the purple gradients.
- `is_high_contrast`: Nightshade 7.79 and Lilac 6.63 stay under 9; High Contrast 13.62 over.

### 16.3 Performance

- Idle: `RUSTCORD_FRAME_STATS=1` (no `RUSTCORD_SCREENSHOT`), wait 5 s, frames per 2 s = 0 in each
  state: main window; settings open (backdrop shown); in a voice call (island shown, CallTimer
  off); after hovering the rail then moving away; after the island fades out (leave the call).
- First frame < 1 s: `RUSTCORD_TRACE_STARTUP=<file>`.
- RAM < 150 MB: `RUSTCORD_MEMSTATS=<file>` idle, and with settings open at 1280x800 and at a
  2560x1440 window (the capture spike is transient; the steady state adds ≤ 0.5 MB).
- Opening settings answers within 100 ms (one capture frame + LARGE fade starts).

### 16.4 Must not change (see §15.1)

Compare `main`, `chat-states` and `call` shots of 7c32079 and the final build with the colours
ignored: every rect and text position in the main window is the same (§15.1). Run the full test
suite and the survey's other screenshot hooks once in Dark: no layout moved, nothing new is sent
(offline mode logs no requests).

### 16.5 Old look gone

Search `src`, `docs`, `README.md`, `scripts` and `site` for the first look's colour and logo
names (the two old theme names, the orange, the glowing bar and the metalworking words; use a
whole-word search so "member" and "November" stay out). The only hits are `theme::renamed`, its
test, config tests that parse old settings files, and the history notes in this file.

---

## 17. Aura: people's colours (ui/aura.rs)

Each person brings their own colour; the chrome stays Nightshade and purple stays "you".
Setting: Appearance > People's colours > Strength: **Bold** (default), Subtle, Off. High Contrast
has none.

| | |
|---|---|
| Colour, in order | Nitro profile theme primary; profile accent (banner) colour; the avatar's dominant colour (12 hue bins weighted by chroma², heaviest bin's mean, from the 64 px avatar already in the disk cache, decoded once on a worker thread); none (today's look). Default avatars give none. Servers: the icon's dominant colour. |
| Normalised | HSV saturation 45-72 %, value 38-72 %; greys (s < 12 %) give none; a hue within 20° of the accent's 278° is set to 18 % saturation. Dark app themes: luma capped at 0.42 so the card stays dark. |
| Popout, full profile | the aura as a profile theme (`cosmetics::theme_*`: gradient, contrast-checked text, avatar ring); dark themes: colour over 55 % darker, light themes: 72 % / 55 % toward white. Unlike a Nitro theme: the normal 1 px edge and sheen, no gradient outline. Subtle: colour half toward grey. |
| Member and DM rows | hovered (75 %) or popout open / DM selected (100 %): the aura from the left edge fading out at 75 % of the width, over the row fill, at 55 % (light themes 35 %, Subtle half). The status-dot ring takes the wash colour. At rest: unchanged. |
| User panel | my aura the same way, at full strength. |
| Server | a band under the server name, the icon's aura fading out downward over 150 px, in the card's 12 px corner. |
| Paint | one shared two-texel ramp texture per direction, tinted per shape: no texture per colour, no new traffic. |
| Motion | none of its own: rows wash instantly on hover (ux-motion: frequent, no delay). A colour decoded while its card is open appears on the next frame. |
