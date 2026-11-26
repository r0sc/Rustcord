# Rustcord cosmetics notes (profile cosmetics, identity, Nitro themes; user API, Nov 2026)

These notes extend FEATURES_NOTES §2.6 (profiles) and §3.2 (own account) without repeating them. They cover the user request: custom window close button, guild (clan) tags, game collection, avatar decoration, nameplate, profile effect, profile frame ("Rahmen"), display-name style, profile theme, badges, connections, client-side cosmetics without Nitro, and free Nitro app themes.

**Source tags:**
- **[UD]** Userdoccers raw MDX: `github.com/discord-userdoccers/discord-userdoccers` → `pages/resources/{user,guild,collectibles,connected-accounts,widgets,game}.mdx` and `pages/reference.mdx`.
- **[WEB]** Web client.
  - wavedevgit `discord-client-datamining` `build/chunks/*.js`.
  - Canary dump `Wumpus-Central/discrapper-canary@4790bc6` (2026-11-12): `chunks/*.js`, `css/*.css`, `data/routes.json`.
- **[MOB]** Mobile client, de-minified: `Wumpus-Central/discord-mobile-datamining/discord_app/...`.
- **[3P]** Other clients: yapfast (Rust), linkcord (Go), fastcord.
- **[VC]** Vencord.
- **[ED]** I checked this myself on 2026-11-13, against the code file or a live CDN/API request (unauthenticated; no token was used).
- **UNCONFIRMED** Not checked.

---

## 0. Editor's corrections and cross-cutting rules

### 0.1 Contradictions in the drafts, resolved

| # | Draft claim | Resolution |
|---|---|---|
| 1 | Dark gradient presets use base theme "dark". | **Wrong.** In chunk `385803.js` they use `_ = l.NJ.DARK`. `812743.js` defines `NJ = {ASH:"dark", LIGHT:"light", ONYX:"midnight", DARK:"darker"}`. So the base is **"darker"** (proto `Theme` 3 DARKER, UI name "Dark") [ED]. |
| 2 | Picker order is "8–15, 17–21, 22, 16". | **Wrong.** The array order is `I = [...light 0–7, ...E, BLURPLE_TWILIGHT(22), EASTER_EGG(16)]` with `E = 8,9,10,11,12,13,14,15,17,18,20,21,19`. The visible list filters out Easter Egg (`Qm: id !== EASTER_EGG`). Hexagon (23) is only reachable through the id lookup (`ag = keyBy([...I, h])`) [ED]. |
| 3 | `/guild-tag-badges/` is an alias of `/clan-badges/`. | **Reversed.** [UD] says `/clan-badges/` is the backward-compat alias of `/guild-tag-badges/`. The web client still builds `/clan-badges/{g}/{badge}.png?size=` (`685073.js` `f()`). Both work; use either [ED]. |
| 4 | The tag is hidden only when member flag bit 10 is set. | **Too narrow.** Web (`685073.js` `h()` → `i.TR(member)`) and mobile (`GuildTagUtils` → `AutomodPermissionUtils.hasAutomodQuarantinedProfile`) hide the tag when **any** automod-quarantine flag is set: `1<<7` NAME, `1<<8` BIO (retired), `1<<10` GUILD_TAG [ED]. |
| 5 | Per-guild decoration set endpoint is UNCONFIRMED. | **Resolved** [UD guild]. `PATCH /guilds/{g}/members/@me` accepts `avatar_decoration_sku_id`, `collectibles: {nameplate: {sku_id}}`, `display_name_font_id`, `display_name_effect_id` and `display_name_colors` (max 2). All are premium-only. |
| 6 | Guild member overrides only the decoration. | **Incomplete** [UD guild]. The guild member object also has `collectibles?` (nameplate) and `display_name_styles?`. Use per-guild precedence for all three. |
| 7 | Palettes: crimson…white, plus black. | [UD] also lists **`none`**, which means no gradient. `black` exists only in client code [MOB, ED]. |
| 8 | Widget `comment` max 200 characters, and a fixed widget display order. | Not in [UD]. Both come only from [MOB]; treat them as UNCONFIRMED for the web. |
| 9 | Game icon fallback via `GET /applications/public`. | **Unnecessary.** The `GET /games` objects already carry `icon_hash` [UD game]. Build `app-icons/{id}/{icon_hash}.png` and skip the second call. |
| 10 | `user_profile.collectibles[]` exists (not in UD). | **Confirmed** in `parseUserProfileCollectibles.tsx` [ED]. `expires_at` is passed to `new Date()`, so it is an ISO string or milliseconds. For the effect, the client converts it to seconds. |
| 11 | `/user-profile-effects` is still used. | The web `routes.json` has no such route (only `COLLECTIBLES_PRODUCTS`, `COLLECTIBLES_CATEGORIES(_V2)`, `COLLECTIBLES_SHOP`, `COLLECTIBLES_SEARCH`) [ED]. Use it only as a fallback. |
| 12 | Surface tint is "~80% / 60% alpha". | **Replaced** by the exact per-level table in §13.4. I spot-checked `css/419070.css`: preset `high` = `mult*0.75` (dark) and `mult*0.7` (light); custom `high` = `0.45 + M*0.4*0.75` [ED]. |
| 13 | Connection `domain` gives `https://{id}/`. | Confirmed in mobile `Platforms.tsx`: an override after `getPlatformUserUrl: null`. All URL parts are `encodeURIComponent`'d; Bluesky then restores `:` [ED]. |

**Also verified [ED]:**
- `GET /collectibles-products/{sku}` works **without a token** (HTTP 200), with shape `{sku_id, name, summary, styles{background_colors,button_colors,confetti_colors}, items[{type, sku_id, asset, label, palette?}], type, premium_type, category_sku_id, …}`.
- All 27 badge icon hashes in §9 return 200 (a random hash returns 404).
- The nameplate is **672×126** (16:3), decoration presets are **288×288**, badge icons are **72×72**.
- `media/v1/collectibles-shop/{sku}/static|animated` works for decorations **and** nameplates (APNG for `animated`).

### 0.2 German UI → feature → data

| German | Feature | Per-user data (every partial user) | Profile-only data |
|---|---|---|---|
| Clan-Tag / Server-Tag | Guild tag | `primary_guild` (old name `clan`) | – |
| Avatar-Deko | Avatar decoration | `avatar_decoration_data` | – |
| Namensschild | Nameplate | `collectibles.nameplate` | – |
| Anzeigename-Stil | Display name style | `display_name_styles` | – |
| Profileffekt | Profile effect | – | `user_profile.collectibles[] type 1` / `user_profile.profile_effect` |
| Rahmen (Profilrahmen) | Profile frame, collectible type 3 (German label UNCONFIRMED) | – | `user_profile.collectibles[] type 3` |
| Profilschema / Profilthema | Profile theme | – | `user_profile.theme_colors` |
| Abzeichen | Badges | `public_flags` (fallback) | `badges[]`, `guild_badges[]` |
| Verknüpfungen | Connections | – | `connected_accounts[]`; own: `GET /users/@me/connections` |
| Spielesammlung | Most likely the profile **game widgets** (German label UNCONFIRMED; it could also be "Kürzlich gespielte Spiele", the activity tab) | – | `widgets[]` |
| Nitro-Designs | Client gradient themes | settings proto `appearance.client_theme_settings` | – |

- **Guild overrides** [UD guild]: `member.avatar_decoration_data`, `member.collectibles`, `member.display_name_styles`, and `guild_member_profile.{theme_colors, profile_effect}`. The guild value wins over the user value.
- **Partial users** come from READY `users[]`, MESSAGE_CREATE `author`, member list items and USER_UPDATE. Nothing on them needs a profile fetch.

### 0.3 Rustcord model gaps (current code)

- `model::User` (src/model.rs:182) has none of `public_flags`, `primary_guild`, `clan`, `avatar_decoration_data`, `collectibles`, `display_name_styles`. Unknown keys are dropped, so **add them first**, all as `Option<…>`.
- `model::Member` (src/model.rs:214) lacks `flags`, `avatar_decoration_data`, `collectibles`, `display_name_styles`.
- `ProfileFields` lacks `profile_effect`, `collectibles`.
- `UserProfile` lacks `widgets`.
- `ConnectedAccount` lacks `metadata`.
- `ProfileBadge`, `badges` and `theme_colors` already exist.
- Image cache (`src/cache/images.rs`) already decodes APNG, GIF and animated WebP. Every asset below is APNG or PNG, so there is **no new decoder** (WebM is never needed; see §4).
- `api::cdn::cdn_size` (powers of two, 16–4096) produces only valid CDN sizes, so reuse it.

```rust
#[derive(Clone, Debug, Default, Serialize, Deserialize)] #[serde(default)]
pub struct PrimaryGuild { pub identity_guild_id: Option<Id>, pub identity_enabled: Option<bool>,
                          pub tag: Option<String>, pub badge: Option<String> }
#[derive(Clone, Debug, Default, Serialize, Deserialize)] #[serde(default)]
pub struct DecorationData { pub asset: String, pub sku_id: Option<Id>, pub expires_at: Option<i64> /* unix s */ }
#[derive(Clone, Debug, Default, Serialize, Deserialize)] #[serde(default)]
pub struct Nameplate { pub asset: String, pub sku_id: Option<Id>, pub palette: Option<String>,
                       pub label: Option<String>, pub expires_at: Option<i64> }
#[derive(Clone, Debug, Default, Serialize, Deserialize)] #[serde(default)]
pub struct Collectibles { pub nameplate: Option<Nameplate> }
#[derive(Clone, Debug, Default, Serialize, Deserialize)] #[serde(default)]
pub struct NameStyle { pub font_id: Option<u32>, pub effect_id: Option<u32>, pub colors: Vec<u32> }
// User / Member: + public_flags: u64 (User only), primary_guild/clan: Option<PrimaryGuild>,
//   avatar_decoration_data: Option<DecorationData>, collectibles: Option<Collectibles>,
//   display_name_styles: Option<NameStyle>, flags: u64 (Member only)
```

### 0.4 Without Nitro: client-side cosmetics (the user's main request)

**Store.** Keep a `LocalCosmetics` in Rustcord's config (`src/config.rs`). Only `{sku_id, asset}` pairs plus a few scalars:

```rust
#[derive(Clone, Debug, Default, Serialize, Deserialize)] #[serde(default)]
pub struct LocalCosmetics {
    pub decoration: Option<DecorationData>, pub nameplate: Option<Nameplate>,
    pub effect_sku: Option<Id>, pub frame_sku: Option<Id>,
    pub name_style: Option<NameStyle>, pub theme_colors: Option<[u32; 2]>,
    pub client_theme: Option<ClientTheme>,          // §13
}
```

**Apply.**
- Apply the overrides **only when rendering `user.id == me`**, in every surface listed per feature.
- Precedence is local → guild member → user.
- An `Option` set to `None` means "use the server value". Add an explicit "hide" choice that stores an empty `asset` / `colors`.

**Hard rules:**
- **Never PATCH a Nitro-only field or an unowned SKU.** The server rejects it (403 or 50035; the exact code is UNCONFIRMED), and repeated rejected writes are abnormal client behaviour.
- Offer "Also apply to account" only when `premium_type > 0`, or when the SKU is in `GET /users/@me/collectibles-purchases`.
- Vencord FakeNitro does the same: local dispatch only, no server write [VC `src/plugins/fakeNitro/index.tsx`].

**Labels.** In the settings page, mark local items "Nur für dich sichtbar" (only visible to you).

**Free features with real writes:**

| Feature | Endpoint |
|---|---|
| Guild tag adoption | `PUT /users/@me/clan` |
| Game widgets | `PUT /users/@me/widgets` |
| Connection visibility | `PATCH /users/@me/connections/…` |
| Claiming free/reward collectibles | `PUT /users/@me/claim-premium-collectibles-product` (Nitro claim) |

### 0.5 CDN rules

- **Sizes** [UD reference]: powers of two 16–4096 plus 20, 22, 24, 28, 40, 44, 48, 56, 60, 80, 96, 100, 160, 240, 300, 320, 480, 600, 640, 1280, 1536, 3072. Web: `size = round_up_to_allowed(px * devicePixelRatio)`.
- Boolean query values must be the literal strings `true` / `false`.
- **`passthrough=true`** returns the original Discord quality and format (APNG for decorations).
- `/assets/…` paths ignore `size`.

---

## 1. Window chrome: own close button (not researched by the drafts; egui-only)

- Current state: `src/main.rs:51` builds `ViewportBuilder` with OS decorations, so Windows draws its title bar above Rustcord's own 32 px `Panel::top("title_bar")` (`src/ui/mod.rs:859` → `sidebar::title_bar`).
- **Builder change:** add `.with_decorations(false)` to the builder (EGUI_0_37_NOTES line 114).
- **Title bar row:**
  - Primary-drag on empty area → `ctx.send_viewport_cmd(ViewportCommand::StartDrag)`.
  - Double-click → `ViewportCommand::Maximized(!ctx.input(|i| i.viewport().maximized.unwrap_or(false)))`.
- **Right side, after Inbox/Help:** three buttons, each full bar height (32 px) by about 40–46 px wide (UNCONFIRMED; Windows 11 uses 46 px).
  - Minimize → `ViewportCommand::Minimized(true)`.
  - Maximize/restore → `Maximized(!max)`.
  - Close → `ViewportCommand::Close` (hover fill `#C42B1C`, white glyph).
  - Draw the glyphs with `painter.line_segment` at 10 px. No icon assets are needed.
- **Resize** of an undecorated window on Windows: winit may drop the edge hit-test (UNCONFIRMED; test it).
  - If it does, add 5 px invisible rects on the edges and corners.
  - On `primary_pressed` there, send `ViewportCommand::BeginResize(ResizeDirection::{North,…})`.
  - Set `cursor_icon` to `ResizeHorizontal` / `ResizeVertical` / `ResizeNwSe` / `ResizeNeSw`.
- Known losses: no Windows snap-layout flyout on the maximize button, and Aero-snap by dragging still works through `StartDrag`. Keep a config toggle `native_titlebar: bool` (default false) as an escape hatch.

---

## 2. Guild tags (Server-/Clan-Tag)

**Object** [UD user]:
```json
"primary_guild": {"identity_guild_id":"80351110224678913","identity_enabled":true,"tag":"MEOW","badge":"7d1734ae5a615e82bc7a4033b98fade8"}
```
- `tag` is at most 4 characters.
- `tag` and `badge` are `null` until the user re-affirms the tag after the guild changed it.
- `identity_guild_id` is only set when `identity_enabled != false`.
- Read `primary_guild.or(clan)`. Whether `clan` is still sent is UNCONFIRMED.

**Show rule** [WEB `685073.js` `h()`, MOB `GuildTagUtils`, ED]:
```
show = pg.identity_enabled == true && pg.identity_guild_id.is_some() && pg.tag.is_some()
       && !(in guild context && member.flags & ((1<<7)|(1<<8)|(1<<10)) != 0)
```

**Badge URL** [ED]:
- `https://cdn.discordapp.com/clan-badges/{identity_guild_id}/{badge}.png?size={S}`. The canonical path is `/guild-tag-badges/…`; the two are aliases.
- Drawn → requested size [MOB `GuildTagConstants`]: 12→16, 14→16, 16→16, 24→24, 32→32, 36→40. The default drawn size is **12 px**.

**Chip** [MOB `guild_tag/native/GuildTag.tsx`]:
- Fill `BACKGROUND_MOD_STRONG` (Rustcord: text colour at about 12% alpha), radius 4, padding 1 px vertical × 4 px horizontal.
- Contents: the 12 px badge, a gap of about 2–4 px, then the tag text (about 12 px semibold, one line, ellipsis). Font size and gap are UNCONFIRMED.

**Where it shows:**
- After the display name in the message header (before the APP/BOT tag).
- Member list row and DM list row.
- Profile popout next to the username.
- Voice channel user rows.

**Click → tag card** (built: `ui/guild_card.rs`; endpoints, join / apply and the button rules
re-checked against [UD]/[MOB] in `docs/GUILD_CARD_NOTES.md`, which wins over this draft):
- `GET /guilds/{id}/profile`. Works for members, or when the guild is discoverable/public. A 403 otherwise means: show tag and badge only.
- Response (draft [UD discovery], not re-checked): `{id, name, icon_hash, member_count, online_count, description, tag, badge (int 0–40 = badge kind), badge_color_primary, badge_color_secondary, badge_hash, traits[{emoji_id,emoji_name,emoji_animated,label,position}], visibility, custom_banner_hash, premium_tier, premium_subscription_count, game_application_ids[]}`.
- Images:
  - Banner: `discovery-splashes/{id}/{custom_banner_hash}.png?size=480` [UD reference row confirmed].
  - Icon: `icons/{id}/{icon_hash}.png?size=128`.
- Card button "Adopt tag" appears only when I am a member and the guild has the `GUILD_TAGS` feature (`685073.js` `d()`).

**Adopt or drop** (free, not Nitro) [UD user, WEB route `USER_SET_GUILD_IDENTITY`, ED]:
- Adopt: `PUT /users/@me/clan {"identity_guild_id":"<g>","identity_enabled":true}` → user object, plus USER_UPDATE.
- Drop: `{"identity_enabled":false}`.
- Mobile 2026 alternative: `PATCH /users/@me {"primary_guild_id":"<g>"}` [MOB `UserSettingsAccountActionCreators`]. Prefer the PUT.
- The tag picker lists my guilds whose `features ∋ GUILD_TAGS` and whose `profile.tag != null`.

---

## 3. Avatar decorations (Avatar-Deko)

**Object** [UD]:
```json
"avatar_decoration_data": {"sku_id":"1144058844004233369","asset":"a_fed43ab12698df65902ba06727e20c0e","expires_at":null}
```
- `expires_at` is Unix **seconds**. Hide the decoration when `expires_at*1000 < now`.

**URL** [WEB `486020.js` fn `M`, verbatim logic, ED]:
- `https://cdn.discordapp.com/avatar-decoration-presets/{asset}.png?size={cdn(drawPx*dpr)}&passthrough={canAnimate}`.
- `passthrough=true` gives APNG, animated when `asset` starts with `a_`.
- `passthrough=false` gives a static PNG. `.webp` also works (static).
- If `asset` starts with `data:` or `blob:`, use it as is (local preview).
- Alternative needing only the SKU: `https://cdn.discordapp.com/media/v1/collectibles-shop/{sku_id}/static|animated` [MOB `CollectiblesAssetUtils`, ED].

**Geometry** [MOB `AvatarDecorationConstants`: `DECORATION_TO_AVATAR_RATIO = 1.2`]:
- The decoration square is `1.2 × avatar`, centred on the avatar (10% overhang on each side), drawn **over** the avatar.
- The status dot is drawn on top of the decoration (web layer order UNCONFIRMED).
- Requested sizes: avatar 32 → 38.4 (request 48 with dpr 1, or `cdn_size` → 64); 40 → 48; 80 → 96.
- Do not let the overhang shift layout. Paint it outside the avatar rect, with no extra allocation.

**Animation:**
- Animate on hover in lists, and always in an open profile (UNCONFIRMED).
- Keep it static in the low-RAM or performance mode.
- Rustcord: decoded at the drawn physical size (snapped to pixels), every frame kept; over the 4 MiB animation budget the side drops (to half at most) before frames do; frame delays ≤ 10 ms play as 100 ms like browsers.

**Where:** message avatars, member list, DM list, user panel, profile popout/modal, voice user list.

**Set (owned SKU or Nitro only):**
- `PATCH /users/@me {"avatar_decoration_sku_id":"<sku>"|null}`.
- Per guild: `PATCH /guilds/{g}/members/@me {"avatar_decoration_sku_id":…}` [UD guild].

---

## 4. Nameplates (Namensschild)

**Object** [UD]:
```json
"collectibles":{"nameplate":{"asset":"nameplates/nameplatetest/angel/","palette":"bubble_gum","label":"It's angel time","sku_id":"1344802364934062152","expires_at":null}}
```
- `asset` ends with `/`. `expires_at` is Unix seconds.

**Assets** [UD reference fn 2/6, WEB `763673.js`, ED]:
- Legacy path: `https://cdn.discordapp.com/assets/collectibles/{asset}{file}`, where `file` is one of:
  - `static.png` (672×126)
  - `img.png` (APNG)
  - `asset.webm`

  `size` is ignored on this path.
- The web client uses `static.png` at rest. Its animated asset is `asset.webm`, **except** with reduced motion, on Linux or on Safari, where it uses `img.png`. **Rustcord: always use `img.png`** (APNG, already decodable).
- Same files by SKU: `media/v1/collectibles-shop/{sku_id}/static` and `/animated` [ED].

**Palettes** [MOB `collectibles/nameplates/constants.tsx`, ED]. For an unknown key or `none` [UD], draw no gradient.

| key | dark theme | light theme |
|---|---|---|
| crimson | #900007 | #E7040F |
| berry | #893A99 | #B11FCF |
| sky | #0080B7 | #56CCFF |
| teal | #086460 | #7DEED7 |
| forest | #2D5401 | #6AA624 |
| bubble_gum | #DC3E97 | #F957B3 |
| violet | #730BC8 | #972FED |
| cobalt | #0131C2 | #4278FF |
| clover | #047B20 | #63CD5A |
| lemon | #F6CD12 | #FED400 |
| white | #FFFFFF | #FFFFFF |
| black (client only) | #000000 | #000000 |

**Render** [MOB `nameplates/native/Nameplate.tsx`, `nameplates/utils.tsx`]: a layer behind the row content, filling the row rect and clipped to the row radius.
1. A horizontal gradient from `palette` at alpha 0 on the left to alpha 0x4D (0.30) on the right. In egui this is a 4-vertex mesh.
2. The image at 100% row height, **right-aligned**, aspect 16:3.
3. Static at rest. Animated on hover or when the row is selected.
4. Muted or unfocused rows: image opacity 0.1 and no gradient.

**User panel** (`cosmetics::nameplate_cover`): the art covers the whole account row edge to edge (cover-fit 16:3, clipped to the panel's corners; under the voice section only the lower ones), under a 20 % veil of the panel colour, an 80 % scrim behind the name and status that clears 48 px after the text ends, and a 50 % scrim behind the buttons (`buttons_scrim`). Dark and light themes alike. Earlier the scrim covered 60 % of the width (and light themes faded the art in only after the text at 42 %), so the art seemed to fill only the right side.

**Where** (placements `account`, `channel`, `member_list`, `preview`): user panel account row, DM list rows, member list rows. **Not** in messages.

**Set (owned or Nitro):**
- `PATCH /users/@me {"nameplate_sku_id":"<sku>"|null}`.
- Per guild: `PATCH /guilds/{g}/members/@me {"collectibles":{"nameplate":{"sku_id":"<sku>"}}}` [UD guild].

---

## 5. Profile effects (Profileffekt)

**Which effect a user has.** Read both shapes; check `guild_member_profile` first.
- 2026 shape [MOB `parseUserProfileCollectibles.tsx`, ED]: `user_profile.collectibles: [{"sku_id","type":1|3,"expires_at"}]`.
  - `type` 1 = effect, 3 = frame.
  - `expires_at` goes through `new Date()`, so it is ISO or ms. Accept a string or a number.
- Older shape [UD]: `user_profile.profile_effect {id, expires_at}`.

**Config lookup:**
1. `GET /collectibles-products/{sku}` (unauthenticated [UD, ED]). Take the `items[]` entry with `type==1`.
2. Fallback only: `GET /user-profile-effects` → `{"profile_effect_configs":[{id, sku_id, …same fields}]}`.
   - This route is not in the web routes anymore [ED]. Match on `id` **or** `sku_id`.
   - Whether `profile_effect.id == sku_id` is UNCONFIRMED. The new shape is SKU-based, so prefer it.

**Item** [UD collectibles]:
```json
{"type":1,"sku_id":"…","title":"…","description":"…","accessibilityLabel":"…","animationType":2,
 "thumbnailPreviewSrc":"…/thumbnail.png","reducedMotionSrc":"…/reducedMotion.png","staticFrameSrc":"…",
 "effects":[{"src":"…/intro.png","loop":false,"width":450,"height":880,"duration":2880,"start":0,"loopDelay":0,
             "position":{"x":0,"y":0},"zIndex":100,"randomizedSources":[{"src":"…"}]}]}
```
- Layers are APNG.
- A relative `src` gets the prefix `https://cdn.discordapp.com/` [3P yapfast].
- `animationType`: 0 unspecified, 1 PERSISTENT, 2 INTERMITTENT.

**Render** [MOB `profile_effects/native/ProfileEffectUtils.tsx`, `getAssetWHRatio.tsx`]:
- Sort layers by `zIndex`, ascending. Start the clock at **t = −500 ms** when the card opens.
- A layer is visible when:
  - `t >= start`, **and**
  - non-looping: `t <= start + duration`;
  - looping with `loopDelay > 0`: `(t - start) % (duration + loopDelay) <= duration`;
  - looping otherwise: always.
- Layer rect: `w = cardW`, `h = cardW * height/width` (default 450×880). `position` is scaled by `cardW/450` [3P linkcord].
- Draw **over the whole card** (banner included), clipped to the card radius.
- APNG frame time: use `(t - start) mod anim_len` for the frame index, and restart the decoder clock when a layer becomes visible.
- `randomizedSources`: pick one index below the shortest array length, once per open, and use it for every layer.
- Reduced motion or performance mode: draw `reducedMotionSrc` (or `staticFrameSrc`) only.
- Rustcord (cosmetics.rs `paint_effect`): effects animate on hover (unless "Play animated avatars, emoji and GIFs" is on), so the clock starts when the hover starts and every layer has loaded (the still image shows until then; no −500 ms), and resets when the hover ends: every hover plays the intro from its first frame. Each layer window rewinds its stream to frame 0 (`ImageCache::rewind`).
- Rustcord: effect layers and profile banners stream (cache/images.rs `stream_frames`): the APNG / GIF stays compressed, a thread decodes each frame just before it is due (2 ahead; own compositor on the `png` / `gif` crates, ~1 ms an effect frame), one texture per layer at the asset's 1× size (2× assets box-halved), paused while not drawn, freed when the card closes.

**Where:** profile popout and full profile modal. Picker preview uses `thumbnailPreviewSrc`.

**Set (owned or Nitro):**
- `PATCH /users/@me/profile {"collectibles_sku_ids":["<effectSku>","<frameSku>"]}`. Send the **full** list; leave an item out to unequip it [MOB `ProfileUpdateRequestUtils.tsx`, ED].
- Legacy: `{"profile_effect_id":"…"}` [UD].

---

## 6. Profile frames ("Rahmen", collectible type 3)

**Item** [UD collectibles + MOB `collectibles/records/ProfileFrameRecord.tsx`]:
```json
{"type":3,"sku_id":"…","label":"…","inner_width":1200,"overflow_top":304,"overflow_bottom":0,"overflow_horizontal":40,
 "layers":[{"id":"<asset id>","type":"staple|rail|border","order":"front|back","anchor":"top|bottom|center","responsive":false}]}
```
[UD] types `type`, `order` and `anchor` as integers with no enum. The client uses strings [MOB `shared-constants/ProfileFrameLayer*.tsx`]. Deserialize both (`#[serde(untagged)]` int|string). The int mapping is UNCONFIRMED; assume declaration order: 0 staple / 1 rail / 2 border, 0 front / 1 back, 0 top / 1 bottom / 2 center.

**Asset:** `https://cdn.discordapp.com/media/v1/collectibles-shop/{sku_id}/{layer.id}/static?width={W}&height={H}`. Only `static` is used. Rustcord decodes it at the full drawn width (≤ 768 px).

**Layout** [MOB `profile_frames/native/ProfileFrame.tsx`, `scaleProfileFrame.tsx`]:
1. `k = cardW / inner_width`. Then `ot = overflow_top*k`, `ob = overflow_bottom*k`, `oh = overflow_horizontal*k`.
2. Each layer spans `x ∈ [-oh, cardW+oh]`, so `W = cardW + 2*oh` and `H = W * imgH/imgW`.
3. z-order: `back` under the card content, `front` over it (front z 3, back z 2; the content's position between them is UNCONFIRMED).
4. Layer types:
   - `staple`: top anchor puts the layer's top edge at `y = -ot`; bottom anchor puts its bottom edge at `y = cardH + ob` (CSS `bottom: -ob`).
   - `rail`: full card height, image at start, centre or end per `anchor`. If `responsive && cardW/cardH >= imgRatio`, skip it (minimum aspect 0.625).
   - `border`: tile the image vertically `ceil(cardH/H)` times.

**Where:** profile popout and full profile (exact web surfaces UNCONFIRMED).

**Set:** via `collectibles_sku_ids` (§5).

---

## 7. Display name styles (Anzeigename-Stil)

**Object** [UD]: `"display_name_styles":{"font_id":12,"effect_id":2,"colors":[2797222,16762000]}`. Colours are `0xRRGGBB`.

**Fonts** [UD ED]:
- Picker order [MOB]: 11, 12, 3, 4, 6, 7, 8, 10, 13, 14, 15, 16.
- Retired ids 1 BANGERS, 2 BIO_RHYME, 5 COMPAGNON, 9 RIBES render with the default font.

| id | UD enum | UI name | public font (licence) | weight [3P] |
|---|---|---|---|---|
| 11 | DEFAULT | – | gg sans → Rustcord UI font | – |
| 3 | CHERRY_BOMB | Sakura | Cherry Bomb One (OFL) | 400 |
| 4 | CHICLE | Jellybean | Chicle (OFL) | 400 |
| 6 | MUSEO_MODERNO | Modern | MuseoModerno (OFL) | 500 |
| 7 | NEO_CASTEL | Medieval | Néo-Castel (licence unknown) | 400 |
| 8 | PIXELIFY | 8Bit | Pixelify Sans (OFL) | 500 |
| 10 | SINISTRE | Vampyre | Sinistre (licence unknown) | 548 |
| 12 | ZILLA_SLAB | Tempo | Zilla Slab (OFL) | 600 |
| 13 | PLAYPEN_SANS | Monkey Bars | Playpen Sans (OFL) | 700 |
| 14 | ORBITRON | Mainframe | Orbitron (OFL) | 700 |
| 15 | NEW_ROCKER | Headbang | New Rocker (OFL) | 400 |
| 16 | KALAM | Journal | Kalam (OFL) | 700 |
| 17 | – | HEXAGON (campaign) | UNCONFIRMED (not in UD) | – |

Font loading (lazy): register the font `egui::FontFamily::Name("dn{id}")` only when a TTF exists under `assets/fonts/display_name/{id}.ttf` (bundled OFL files), otherwise fall back to the UI font. Skip 7 and 10 until their licences are clear.

**Effects** [UD ED: names, colour counts, fallbacks; drawing notes approximate; constants from MOB `DisplayNameStylesConstants.tsx`]:

| id | name | colours | how to draw (egui) |
|---|---|---|---|
| 1 | SOLID | 1 | Plain colour. |
| 2 | GRADIENT | 1–5 | Horizontal gradient c0→c1 across the galley width. Paint the galley in white, then tint per vertex by x. |
| 3 | NEON | 1 | Light core, plus 2–3 offset or blurred copies in the colour at low alpha underneath. |
| 4 | TOON | 1 | Vertical light→dark gradient, plus a dark outline (8 offset copies at ±1 px). |
| 5 | POP | 1 | Shadow copy in the colour at +1/+2 px, text on top. |
| 6 | GLOW | 1–5 | [UD] "falls back to gradient". |
| 7 | PRISM | 1–5 | Scrolling 5-colour gradient (animated). Gradient width at least 38 px. |
| 8 | GUMMY | 1–5 | 4-colour per-letter cycle with squash and stretch. HSL S 0.78, L 0.72. |
| 1001–1004 | TEST_1–4 | 1 | Same as solid. |
| other | – | – | Same as solid. |

**Presets** [MOB]:
- Solid: 24E36D, 1C98EB, C32AFF, DA0149, F93722, C6A615.
- Gradient pairs: 2AAEA6/FFC490, E45EFF/267DE0, 7E4FCB/67CB98, EFB4AA/E46CE2, F5983C/E55AE7.
- Gummy (4 colours each):
  - E9A9D5 A9DDF0 B9A7EE B9EACB
  - FF6FC2 F54FA8 FF9BD6 D96AB7
  - FFB57A FF936F FF7C78 F06B8A
  - C4ED65 98DE60 6CCD63 45BC66
  - 7CB8FF 8EE0D0 9D94F5 66CFF0
  - D77BFF B56BFF E45CE5 8B6CFF
  - D83A5F C94BCB 8B5CFF B06CFF
- Prism (5 colours each):
  - FE9242 35ED7E FFE047 FF4CD2 5865F2
  - 7EE7D8 7CCBFF 7F8CFF B56DFF F38AD8
  - 7A0906 C81E12 F45A18 FF9500 FFD447

**Readability** (yapfast's reading of the web code, UNCONFIRMED):
- Lighten (on dark) or darken (on light) by 0.05 HSL L, at most 10 times, until the WCAG contrast is ≥ 3.0 (gradient and prism: ≥ 2.5).
- Backgrounds: dark #121214, light #FBFBFB.

**Where:** message author, member list, DM list, user panel, profile popout/modal. Whether it overrides the role colour in chat is UNCONFIRMED (safe default: style wins where the user is shown; role colour stays for @mentions).

**Set (Nitro):**
- `PATCH /users/@me {"display_name_font_id":12,"display_name_effect_id":2,"display_name_colors":[2797222,16762000]}`. At most 2 colours through the API; `null` on all three clears it.
- Per guild: the same keys on `PATCH /guilds/{g}/members/@me` [UD guild].

---

## 8. Profile theme (Profilschema)

**Data:** `user_profile.theme_colors: [primary, accent]` (ints, nullable). Draw it only when both are present. The guild profile overrides it (already in `UserProfile::theme_colors()`).

**Card rendering:**
- Background: vertical gradient from `primary` (top) to `accent` (bottom), drawn as a 4-vertex mesh, clipped to the card radius.
- Card mode [MOB `useProfileTheme.tsx`]: **dark** if darkness(primary) > 0.5, otherwise light. darkness = 1 − (0.299r + 0.587g + 0.114b)/255 (formula UNCONFIRMED).
- Overlays on the gradient [MOB tokens], black in dark mode and white in light mode:
  - Body: 0.60.
  - Section boxes: 0.45.
  - Note box: 0.30.
  - Role pills: dark `PRIMARY_660` at 0.5 / white at 0.5.
- Web text and surface tint: the same `Zk` derivation as §13.3, with `textMixAmount = 25` (class `custom-user-profile-theme`, chunk `991988.js`).

**Set (Nitro):** `PATCH /users/@me/profile {"theme_colors":[1052945,6631526]}`. Otherwise use local `LocalCosmetics.theme_colors` (self only).

---

## 9. Badges (Abzeichen)

**Profile:**
- `badges[] {id, description, icon, link?}`. In a guild context, append `guild_badges[]`.
- `private:true` profiles have none.
- Icon: `https://cdn.discordapp.com/badge-icons/{icon}.png` (72×72 [ED]; `size` not needed). Draw at 22 px (web size UNCONFIRMED), gap 4, wrapped in a row with a 4 px radius background.
- Tooltip = `description`. Click (Enter / Space) opens Rustcord's info popover, never the
  `link` (`cosmetics::badge_info`, own copy; `docs/GUILD_CARD_NOTES.md` §7). The icon loads at
  72 px for the popover's 40 px copy.

**Local description rewrites** [MOB `UserProfileStore.tsx`]:
- `premium` → "Subscriber since {premium_since:date}".
- `guild_booster_lvl*` → "Server boosting since {premium_guild_since:date}".

**Known ids** (XYZenix gist; **every hash returned 200 [ED]**):

| id | icon |
|---|---|
| staff | 5e74e9b61934fc1f67c65515d1f7e60d |
| partner | 3f9748e53446a137a052f3454e2de41e |
| certified_moderator | fee1624003e2fee35cb398e125dc479b |
| hypesquad | bf01d1073931f921909045f3a39fd264 |
| hypesquad_house_1 / _2 / _3 | 8a88d63823d8a71cd5e390baa45efa02 / 011940fd013da3f7fb926e4a1cd2e618 / 3aa41de486fa12454c3761e8e223442e |
| bug_hunter_level_1 / _2 | 2717692c7dca7289b35297368a940dd0 / 848f79194d4be5ff5f81505cbd0ce1e6 |
| active_developer | 6bdc42827a38498929a4920da12695d9 |
| verified_developer | 6df5892e0f35b051f8b61eace34f4967 |
| early_supporter | 7060786766c9c840eb3019e725d2b358 |
| premium | 43aa87a32b12a410943ab55ccf290b34 |
| guild_booster_lvl1…9 | 51040c70d4f20a921ad6674ff86fc95c, 0e4080d1d333bc7ad29ef6528b6f2fb7, 72bed924410c304dbe3d00a6e593ff59, df199d2050d3ed4ebf84d64ae83989f8, 996b3e870e8a22ce519b3a50e6bdd52f, 991c9f39ee33d7537d9f408c3e53141e, cb3ae83c15e970e8f3d410bc62cb8b99, 7142225d31238f6387d9f09efaa02759, ec92202290b48d0879b7413d2dde3bab |
| legacy_username | 6de6d34650760ba5551a79732e98ed60 |
| quest_completed | 7d9ae358c8c5e118768335dbe68b4fb8 |
| orb_profile_badge | 83d8a1eb09a8d64e59233eec5d4d5c2d |
| bot_commands | 6f9e37f9029ff57aef81db857890005e |
| automod | f2459b691ac7453ed6039bbcfaccbfcd |

The gist also lists `premium_tenure_{1,3,6,12,24,36,60,72}_month[_v2]` with their own hashes (not checked).

**Fallback without a profile fetch** (mini-badges in hover cards, from `public_flags` bit → id):
- 0 → staff, 1 → partner, 2 → hypesquad, 3 → bug_hunter_level_1
- 6, 7, 8 → hypesquad_house_1, 2, 3
- 9 → early_supporter, 14 → bug_hunter_level_2, 17 → verified_developer, 18 → certified_moderator
- 22 → active_developer (UD marks bit 22 as removed; UNCONFIRMED)

**No local fake badges.** Badges are earned status, not cosmetics, so this was deliberately skipped.

---

## 10. Connections (Verknüpfungen)

**Profile:** `connected_accounts[] {type, id, name, verified, metadata?}`. Partial connections are always visibility 1 [UD]. Drop types without a client platform entry: [UD] lists `soundcloud` and `playstation-stg`, but there is no mobile platform for soundcloud [ED].

**Own connections** [UD connected-accounts, WEB routes `CONNECTIONS*`, ED]:

| op | request | result |
|---|---|---|
| list | `GET /users/@me/connections` (READY `connected_accounts` has the same) | `[{id, type, name, verified, metadata?, metadata_visibility, revoked, integrations[], friend_sync, show_activity, two_way_link, visibility}]` |
| modify | `PATCH /users/@me/connections/{type}/{id}` with any of `{"visibility":0\|1, "show_activity":bool, "metadata_visibility":0\|1, "friend_sync":bool, "name":str}` | connection |
| delete | `DELETE /users/@me/connections/{type}/{id}` | 204. Also leaves guilds joined through its integrations, so show a warning dialog |
| refresh | `POST /users/@me/connections/{type}/{id}/refresh` | 204 |
| add | `GET /connections/{type}/authorize` → `{url}`; open it in the browser. The callback `POST /connections/{type}/callback {code,state}` is done by discord.com in the browser | – |
| event | USER_CONNECTIONS_UPDATE (no payload worth merging) → re-GET | – |

**Profile URLs** [MOB `lib/Platforms.tsx`, ED]. Wrap every placeholder in `encodeURIComponent` (Rust: `urlencoding` or a hand-written percent-encode).

| type | URL |
|---|---|
| twitch | `https://www.twitch.tv/{name}` |
| youtube | `https://www.youtube.com/channel/{id}` |
| bluesky | `https://bsky.app/profile/{id}` (then replace `%3A` with `:`) |
| steam | `https://steamcommunity.com/profiles/{id}` |
| reddit | `https://www.reddit.com/u/{name}` |
| twitter | `https://x.com/{name}` |
| twitter_legacy | `https://twitter.com/{name}` |
| spotify | `https://open.spotify.com/user/{id}` |
| github | `https://github.com/{name}` |
| roblox | `https://roblox.com/users/{id}/profile` |
| ebay | `https://www.ebay.com/usr/{name}` |
| tiktok | `https://www.tiktok.com/@{name}` |
| instagram | `https://www.instagram.com/{name}` |
| letterboxd | `https://letterboxd.com/{name}/` |
| mastodon | `{id}` (already a URL; only open `https://` values) |
| domain | `https://{id}/` |
| skype | `skype:{id}?userinfo` (do not open; copy instead) |

- **No URL** (clicking copies the name): battlenet, bungie, leagueoflegends, riotgames, facebook, xbox, samsung, playstation, epicgames, paypal, crunchyroll, amazon-music, meta_quest_or_horizon.
- `contacts` never appears on profiles.

**UI:**
- Profile modal: a two-column grid of tiles. Each tile has a 24 px platform icon, the name, a ✓ when `verified`, and an external-link icon when a URL exists.
- Reddit metadata: show `total_karma` and `created_at` when present [UD].
- Settings → Connections: list, visibility and show-activity toggles, delete with the warning, and "Add" opening the browser.
- Icons have no stable Discord CDN URL. Bundle monochrome SVGs from Simple Icons (CC0), rasterised once, or a coloured letter disc as a fallback.

---

## 11. Game collection ("Spielesammlung" → profile game widgets)

**Data** [UD widgets, ED]:
```json
"widgets":[{"id":"1455894303866880153","updated_at":"…","data":{"type":"favorite_games",
  "games":[{"game_id":"505134938354352128","comment":"Best game ever!","tags":["expert","open_to_teach"]}]}}]
```

| type | max games | web title | notes |
|---|---|---|---|
| favorite_games | 1 | Favorite game | detailed: shows `comment` and `tags` |
| current_games | 5 | Games in rotation | detailed: shows `tags` |
| played_games | 20 | Games I like | cover grid |
| want_to_play_games | 20 | Want to play | cover grid |
| application | – | game stats card | `data.application_id` instead of `games` |
| personal, clips_gallery | – | new in 2026 [MOB] | shape UNCONFIRMED; skip |

- Tags [UD]. Only one skill tag at a time: noob, learning_the_ropes, casual, getting_good, intermediate, expert, better_than_you.
- Other tags: obsessed, love_it, kind_of_love_it, kind_of_hate_it, rage_quitting, like_it, frustrated, too_easy, looking_for_group, open_to_play, looking_for_tips, open_to_teach, looking_to_discuss.
- Limits from [MOB] only: `comment` ≤ 200 characters; display order personal, clips_gallery, application, favorite, played, current, want_to_play.
- Widgets are hidden on private profiles and shown in the full profile modal only [support FAQ: "desktop and browser"].

**Game metadata** (one batched call per profile) [UD game]:
- `GET /games?game_ids=<id>&game_ids=<id>…`: 1–25 ids, `with_supplemental_data` defaults to true. Whether the key is repeated or comma-separated is UNCONFIRMED; try repeated first.
- Response objects have `id, name, icon_hash?, cover_image_hash?, supplemental_game_data?{cover_image_url?, artwork_urls?, screenshot_urls?}`.
- Cover: `supplemental_game_data.cover_image_url`, portrait 3:4 [MOB].
- Fallback: `https://cdn.discordapp.com/app-icons/{id}/{icon_hash}.png?size=128` on a muted tile with the name.
- Grid: gap 16, small radius, placeholder fill while loading. Cache per `game_id` for the session.

**Edit (free, real write):**
- `PUT /users/@me/widgets {"widgets":[{"id"?, "data":{"type", "games":[{"game_id","comment"?,"tags"?}]}}]}` replaces the whole list; at most one widget per type. The response is the new list.
- Game search for the picker: `GET /users/@me/widgets/suggested-games` → `{suggested_games[], suggested_wishlist_games[]}` (ids → `/games`). Free-text search uses the route `GAMES_SEARCH` `/games/search` (params UNCONFIRMED).

---

## 12. Collectibles catalog (for the free, local picker)

| purpose | request | notes |
|---|---|---|
| full catalog | `GET /collectibles-categories/v2?include_bundles=true&variants_return_style=1` (with the user token, as the Shop page does) | `{categories:[{sku_id,name,summary,logo_url,products[]}], collections, user_discounts?}` |
| shop tab | `GET /collectibles-shop?tab=avatar-decorations\|profile-effects\|nameplates\|profile-frames` | `{categories, shop_blocks, user_discounts?}` |
| search | `GET /shop/search?item_types=NAMEPLATE&limit=100&offset=0` | `{skus[], pagination{offset,limit,total,has_more}}`; resolve each SKU via the products call |
| one product | `GET /collectibles-products/{sku}` (**no token needed** [ED]) | `{sku_id, name, summary, type, premium_type, items[], styles, …}` |
| owned | `GET /users/@me/collectibles-purchases` | decides whether "apply to account" is allowed |

**Product type** [UD]: 0 decoration, 1 effect, 2 nameplate, 3 frame, 1000 bundle (use its `items`), 2000 variants group (use its variants), 3000 external SKU (skip).

**Item fields to keep:**
- Decoration: `{sku_id, asset, label}`.
- Nameplate: `{sku_id, asset, palette, label}`.
- Effect: the full config (§5).
- Frame: `layers` (§6).

**Picker thumbnails:** `media/v1/collectibles-shop/{sku}/static`, and `/animated` on hover.

**Fetch policy:**
- Only when the picker opens. Disk-cache the JSON for 24 h.
- Never paginate the whole shop in the background.
- One categories call is enough. Do not fan out per-product calls (bundles already include their `items`).

---

## 13. Nitro app themes, free and local (Nitro-Designs)

### 13.1 Wire shape (read-only for Nitro users; never written by Rustcord)

- Proto path: `PreloadedUserSettings.appearance(13).client_theme_settings(3)`.
  - `background_gradient_preset_id(2)`: UInt32Value.
  - `custom_user_theme_settings(4)`: `{colors(1) repeated string "#hex" (1–5), gradient_color_stops(2) repeated float (web writes []), gradient_angle(3) int32, base_mix(4) int32 0–100}`.
  - Fields 1 and 3 are deprecated.
- Shared theme in messages: `message.shared_client_theme {colors[1..5], gradient_angle, base_mix, base_theme?}` (UD message). Render it as a card with a free local "Apply" button.
- Rustcord uses legacy JSON settings. Read the proto value only through the existing `src/api/settings_proto.rs` (plan B), as the initial default. **Free users: config only. Never PATCH `client_theme_settings`.**

### 13.2 Preset data

Sources: chunk `385803.js` [ED: ids, order, base, angles, stops, multiplier] and colours from `css/952007.css` (identical to Android `colors.xml`).
- Base: L = light (proto 2), D = **darker** (proto 3; UI "Dark").
- Base and text colours come from §13.3. The real client emitted them for all rows except 16 and 23, which are computed (±1 per channel).

| id | enum | name | base | angle | stops (hex @ %) | base color | text color | mid% |
|---|---|---|---|---|---|---|---|---|
| 0 | MINT_APPLE | Mint Apple | L | 180 | 56b69f 6.15, 63bc61 48.7, 9eca67 93.07 | e8ffcc | 004d3a | 27 |
| 1 | CITRUS_SHERBERT | Citrus Sherbert | L | 180 | f3b336 31.1, ee8558 67.09 | ffeecc | 4d1700 | 27 |
| 2 | RETRO_RAINCLOUD | Retro Raincloud | L | 148.71 | 3a7ca1 5.64, 7f7eb9 26.38, 7f7eb9 49.92, 3a7ca1 73.12 | ccedff | 005280 | 50 |
| 3 | HANAMI | Hanami | L | 38.08 | efaab3 3.56, efd696 35.49, a6daa2 68.78 | d0ffcc | 4d000a | 50 |
| 4 | SUNRISE | Sunrise | L | 154.19 | 9f4175 8.62, c49064 48.07, a6953d 76.04 | fff7cc | 4d002a | 50 |
| 5 | COTTON_CANDY | Cotton Candy | L | 180.14 | f4abb8 8.5, b1c2fc 94.28 | ccd7ff | 4d000e | 50 |
| 6 | LOFI_VIBES | Lofi Vibes | L | 179.52 | a4c0f7 7.08, a9e4e8 34.94, b0e2b8 65.12, cfdfa2 96.23 | f2ffcc | 002a80 | 27 |
| 7 | DESERT_KHAKI | Desert Khaki | L | 38.99 | e7dbd0 12.92, dfd0b2 32.92, e0d6a3 52.11 | ffe4cc | 4d4000 | 50 |
| 8 | SUNSET | Sunset | D | 141.68 | 48288c 27.57, db7f4b 71.25 | 08001a | ffefe5 | 35 |
| 9 | CHROMA_GLOW | Chroma Glow | D | 128.92 | 0eb5bf 3.94, 4c0ce0 26.1, a308a7 39.82, 9a53ff 56.89, 218be0 76.45 | 000e1a | e5feff | 15 |
| 10 | FOREST | Forest | D | 162.27 | 142215 11.2, 2d4d39 29.93, 454c32 48.64, 5a7c58 67.85, a98e4b 83.54 | 001a02 | fff8e5 | 50 |
| 11 | CRIMSON_MOON | Crimson Moon | D | 64.92 | 950909 16.17, 000000 72 | 1a0000 | ffe5e5 | 30 |
| 12 | MIDNIGHT_BLURPLE | Midnight Blurple | D | 48.17 | 5348ca 11.21, 140730 61.92 | 08001a | a299ff | 24 |
| 13 | MARS | Mars | D | 170.82 | 895240 14.61, 8f4343 74.62 | 1a0600 | ffece5 | 50 |
| 14 | DUSK | Dusk | D | 180 | 665069 12.84, 91a3d1 85.99 | 16001a | 99b6ff | 50 |
| 15 | UNDER_THE_SEA | Under the Sea | D | 179.14 | 647962 1.91, 588575 48.99, 6a8482 96.35 | 021a00 | e5fffd | 50 |
| 16 | EASTER_EGG | Easter Egg (hidden) | L | 180 | 7289da 4, 4e5d94 96 | ccd7ff | 001b80 | 50 |
| 17 | RETRO_STORM | Retro Storm | D | 148.71 | 3a7ca1 5.64, 58579a 26.38, 58579a 49.92, 3a7ca1 73.12 | 00101a | 99daff | 61 |
| 18 | NEON_NIGHTS | Neon Nights | D | 180 | 01a89e 0, 7d60ba 50, b43898 100 | 1a0014 | e5fffd | 50 |
| 19 | SEPIA | Sepia | D | 69.98 | 857664 14.14, 5b4421 60.35 | 1a0f00 | fff3e5 | 50 |
| 20 | STRAWBERRY_LEMONADE | Strawberry Lemonade | D | 161.03 | af1a6c 18.79, c26b20 49.76, e7a525 80.72 | 1a000e | fff6e5 | 32 |
| 21 | AURORA | Aurora | D | 239.16 | 062053 10.39, 191fbb 26.87, 13929a 48.31, 218573 64.98, 051a81 92.5 | 00091a | 99abff | 34 |
| 22 | BLURPLE_TWILIGHT | Blurple Twilight | D | 47.61 | 2c3fe7 11.18, 261d83 64.54 | 02001a | 99a3ff | 50 |
| 23 | HEXAGON | Hexagon (campaign; name UNCONFIRMED) | D | 135 | 4645ff 0, af01f3 33.33, ff6698 66.67, f89a03 100 | 00001a | fff5e5 | 35 |

- **Picker order** [ED]: 0–7, then 8, 9, 10, 11, 12, 13, 14, 15, 17, 18, 20, 21, 19, then 22.
- Since it is free and local, Rustcord may also show 16 and 23 at the end under "Hidden".
- Hexagon has `unsupported_overlayOpacityMultiplier: 0.8`; all other presets use 1 [ED].
- The midpoint % has no web CSS consumer; ignore it.
- `f5 = [ASH, DARK, ONYX]`: the dark presets may sit on any dark base. The default is DARK ("darker").

### 13.3 Deriving base and text colours (`Zk`, chunk `345815.js`, chroma-js)

```
(Lc, Dc) = lum(first) > lum(last) ? (first, last) : (last, first)   // WCAG relative luminance
if hue(Dc) is 0 or NaN: Dc = Lc
base_dark  = Dc with HSL s=1, l=0.05        base_light = Lc with s=1, l=0.90
seed       = dark ? Lc.s=1 : Dc.s=1
text.l     = hue(seed) in [200,300] ? (dark ? 0.80 : 0.25) : (dark ? 0.95 : 0.15)
base_amount = 50%, text_amount = 38%   (accessibility: "reduce" → 20%/20%, "disable" → 0%)
```

### 13.4 Layering (web `css/419070.css`, `chunks/162772.js`; spot-checked [ED])

**A. Gradient.** One window-wide `linear-gradient(θdeg, stops)` behind everything (`background-attachment: fixed`). For a W×H window:
```
d   = (sin θ, −cos θ)                       // screen coords, y down; θ=0 points up, 90 right
len = |W·sin θ| + |H·cos θ|
t   = dot(p − centre, d)/len + 0.5          // clamp to the first/last stop outside the stop range; lerp in sRGB
```

**B. Panel level = gradient + one flat overlay.** Black in dark mode, white in light mode. Preset α values below are multiplied by the preset multiplier and clamped to [0, 1].

| level | preset dark α | preset light α | custom dark α | custom light α | Rustcord surface |
|---|---|---|---|---|---|
| lowest | 0.90 | 0.85 | .54+.36M | .51+.34M | small buttons |
| lower | 0.85 | 0.80 | .51+.34M | .48+.32M | header bars |
| low | 0.80 | 0.75 | .48+.32M | .45+.30M | search box, inputs |
| chat | 0.80 | 0.85 | .48+.32M | 0.95·(.6+.4M) | `colors.background` (chat), member list |
| high | 0.75 | 0.70 | .45+.30M | .42+.28M | `colors.sidebar` (channel/DM list) |
| higher = app frame | 0.70 | 0.65 | .42+.28M | .325+.325M | `colors.guild_bar`, 32 px title bar |
| highest | 0.65 | 0.95 | .39+.26M | 1.0·(.8+.2M) | user panel, composer |

**C. Everything else** (popups, modals, cards, menus, text, icons, borders) is tinted in **OKLab**:
- `token' = mix_oklab(token, theme_color, w)` with `w = amt/(100+amt)`.
- Base colour at 50%: w = 1/3. Applies to `colors.surface`, `code_background`, `message_hover` and borders.
- Text colour at 38%: w = 0.2754. Applies to `colors.text`, `text_muted`, `link`.
- `accent`, `danger` and `mention_highlight` stay untinted (UNCONFIRMED; the web mixes interactive tokens with text).
- **Preset app border:** `mix_oklab(border, mix(first,last,0.5), ≈0.194)`.

**D. Test vectors** (computed by the draft author's OKLab code, not captured from the client):
- Sunset on Dark: text #efeff1 → #f3efee, muted #96979e → #b2afb1, surface-high #242429 → #1a1725, app frame #070709 → #07050f.
- Mint Apple on Light: text #2e2e34 → #293736, surface #ffffff → #f7ffee.

### 13.5 Custom themes (Nitro, user-picked 1–5 colours; chunk `162772.js` fn `u`)

**Gradient:** `linear-gradient(gradient_angle deg, c1..cn)` with evenly spaced stops. A single colour is duplicated.

**Derived values:**
- `darkest` / `lightest` = the colours with the lowest and highest luminance.
- `T = 100 − base_mix`, `M = (30 + 0.7T)/100`.
- Dark mode:
  - base amount `60 + 0.4T` %, text amount 30%
  - base = darkest(s=1, l=0.05)
  - text = darkest with l = (100 − 0.1T)/100
- Light mode:
  - base amount `30 − 0.2·base_mix` %, text amount 40%
  - base = lightest(s=1, l=0.9)
  - text = lightest with l = 0.05T/100
- Border amount is 5%.
- Overlay α values: the custom columns in §13.4.

### 13.6 egui recipe for Rustcord

1. **Config:**
   ```
   client_theme: Option<ClientTheme{ preset: Option<u32>, custom: Option<{colors: Vec<String>, angle: i32, base_mix: u8}>, base: "light"|"darker"|"dark"|"midnight" }>
   ```
   The base selects the existing built-in (`lilac` for L, `nightshade` for D) before tinting. `Theme` stays the source of the tokens.
2. **Gradient:** on resize or theme change, CPU-render a texture of about W/4 × H/4 with the §13.4A maths. Paint it once with `ctx.layer_painter(Background)` over the whole screen rect. Bilinear upscaling of a linear gradient is visually lossless and costs about 60 KB of RAM.
3. **Panel fills:** `Color32::from_black_alpha((α*255) as u8)` (dark) or `from_white_alpha` (light) at that panel's level, instead of the opaque `ThemeColors` fill. egui blends in gamma space, as CSS does.
4. **Opaque tokens:** `oklab_mix` (about 20 lines: sRGB → linear → OKLab → lerp → back). Run it once per theme change in `ThemeApplier`, not per frame.
5. **Picker:** Settings → Appearance → a grid of 23 swatches (48 px, radius 8, gradient mesh), plus a custom editor (1–5 colour pickers, angle slider, base_mix slider). It is free for everyone and never sent to the server.

---

## 14. Implementation checklist (build order)

1. [x] (no `native_titlebar` toggle yet) **Window chrome:** `with_decorations(false)`, drag, double-click maximize, min/max/close buttons in `sidebar::title_bar`, edge resize fallback, `native_titlebar` toggle (§1).
2. [x] **Model:** add `public_flags`, `primary_guild|clan`, `avatar_decoration_data`, `collectibles`, `display_name_styles` to `User`. Add `flags`, `avatar_decoration_data`, `collectibles`, `display_name_styles` to `Member`. Add `collectibles`, `profile_effect` to `ProfileFields`. Add `widgets` to `UserProfile` and `metadata` to `ConnectedAccount`. Add serde round-trip tests with the JSON samples above (§0.3).
3. [x] **`api::cdn`:** `guild_tag_badge_url`, `decoration_url(asset, px, animate)`, `nameplate_url(asset, animated)` (→ `img.png`/`static.png`), `collectible_sku_url(sku, static|animated)`, `badge_icon_url`, `app_icon_url`. Add unit tests like the existing ones.
4. [x] (chip only; no tag card, no adopt picker, not in compact chat / voice rows) **Guild tag chip** in messages, member list, DM list, profile and voice rows, with the quarantine-flag rule. Tag card via `GET /guilds/{id}/profile`. Settings picker with `PUT /users/@me/clan` (free).
5. [x] **Decoration overlay** at 1.2× everywhere avatars render; animate on hover.
6. [x] **Nameplate row background** in the member list, DM list and user panel.
7. [x] (fonts not bundled: UI / monospace font) **Display name style renderer:** solid, gradient/glow, pop, neon, toon first; prism and gummy later; fonts lazily from `assets/fonts/display_name/`.
8. [x] **Profile card:** theme_colors gradient and overlays; badges row (tooltip, link, local rewrites); connections grid with the URL table; game widgets with `/games` covers.
9. [x] **Profile effect player** (`/collectibles-products/{sku}`, t0 = −500 ms, visibility rule) and **frame layers**.
10. [x] (stored in `overrides.toml`, not `config.rs`) **`LocalCosmetics`** in config plus the Settings → Profile cosmetics picker (catalog §12, 24 h disk cache). Self-only rendering, labelled "Nur für dich sichtbar". Real PATCH only when Nitro or owned.
11. [x] (presets + editor; picked by name in `settings.theme`) **Free gradient themes:** 23 presets plus custom, config-only (§13).
12. [x] (no "Add" yet) **Settings → Connections:** list, toggles, delete with warning, add via browser.
13. [ ] **Settings → Game widgets editor:** `PUT /users/@me/widgets`, free.

---

## 15. Still UNCONFIRMED

- German labels "Spielesammlung" and "Rahmen" (assumed: profile widgets and profile frames).
- Whether `clan` is still sent alongside `primary_guild`.
- `profile_effect.id == sku_id`.
- Int enum mapping of frame `layers.type`, `order` and `anchor`.
- Web layer order: status dot vs decoration; frame content between back and front layers.
- Display name style vs role colour in chat. Font 17 (HEXAGON) family. The readability-adjust constants.
- Badge draw size on web. Game widget comment limit and display order (mobile only). `/games` query style (repeated vs comma).
- Server error code for unowned or non-Nitro cosmetic writes.
- Whether undecorated egui windows keep edge resize on Windows 11.
- Theme: names of Hexagon and Easter Egg; whether the "darker"/"midnight" `<html>` element also carries `theme-dark`; untinted accent and danger.
- Guild tag card response fields (taken from the drafts' [UD discovery], not re-fetched).

## Sources

- Userdoccers raw MDX:
  - https://github.com/discord-userdoccers/discord-userdoccers/blob/master/pages/resources/user.mdx (primary guild, decoration, nameplate, name styles, `PUT /users/@me/clan`, PATCH params)
  - …/pages/resources/guild.mdx (member flags 7/8/10; member `avatar_decoration_data`/`collectibles`/`display_name_styles`; `PATCH /guilds/{g}/members/@me` params)
  - …/pages/resources/collectibles.mdx (product types, item shapes, palettes incl. `none`, endpoints)
  - …/pages/resources/connected-accounts.mdx
  - …/pages/resources/widgets.mdx
  - …/pages/resources/game.mdx
  - …/pages/reference.mdx (CDN rows and footnotes, sizes, booleans)
- Web client:
  - https://github.com/wavedevgit/discord-client-datamining `build/chunks/486020.js` (`getAvatarDecorationURL`), `763673.js` (nameplate assets), `685073.js` (guild tag)
  - https://github.com/Wumpus-Central/discrapper-canary/tree/4790bc65e87a54218f2d2609f23c15cdc49bc63a `chunks/385803.js`, `812743.js`, `873298.js`, `345815.js`, `162772.js`, `991988.js`, `css/419070.css`, `css/952007.css`, `data/routes.json`
- Mobile client: https://github.com/Wumpus-Central/discord-mobile-datamining `discord_app/modules/user_profile/{ProfileUpdateRequestUtils,utils/parseUserProfileCollectibles,UserProfileStore}.tsx`, `modules/collectibles/{avatar_decorations/AvatarDecorationConstants,nameplates/constants,utils/CollectiblesAssetUtils}.tsx`, `modules/guild_tag/GuildTagUtils.tsx`, `modules/guild_automod/AutomodPermissionUtils.tsx`, `lib/Platforms.tsx`, `actions/UserSettingsAccountActionCreators.tsx`
- Badge ids: https://gist.github.com/XYZenix/c45156b7c883b5301c9028e39d71b479
- Theme base/text capture: https://gist.github.com/Aetherinox/b99ff140df37406da73f602e25f79677
- Vencord FakeNitro: https://github.com/Vendicated/Vencord/blob/main/src/plugins/fakeNitro/index.tsx
- Third-party clients: https://github.com/yapfast/yapfast, https://github.com/ezchr/linkcord, https://github.com/AidenNT/fastcord
- Widgets FAQ: https://support.discord.com/hc/en-us/articles/35344672307607-Profile-Widgets-FAQ
- Live checks (no token): `discord.com/api/v9/collectibles-products/{1144058844004233369,1344802364934062152}`; `cdn.discordapp.com/{badge-icons,avatar-decoration-presets,assets/collectibles,media/v1/collectibles-shop}/…`
