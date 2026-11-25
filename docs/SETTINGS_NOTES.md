# Discord user settings: complete inventory for Rustcord

Research date 2026-11-13. Unverified claims are marked **UNCONFIRMED**.

Scope: every Discord settings page and option, with where it is stored and whether Rustcord can implement it. Builds on FEATURES_NOTES §3.1–3.2 (stores, status sync, account/profile PATCH) and COSMETICS_NOTES (profile cosmetics); neither is repeated here.

Field numbers were checked line by line against `PreloadedUserSettings.proto` and `FrecencyUserSettings.proto` (discord-protos master, fetched 2026-11-13).

**Notation**

| Mark | Meaning |
|---|---|
| `P6.17 Bool` | Settings proto type 1, top-level field 6, subfield 17, a BoolValue wrapper. |
| `U32` / `Str` / `F32` | UInt32Value / StringValue / FloatValue wrapper. |
| plain | A non-wrapped proto3 scalar. |
| `J:` | Legacy JSON key. |
| **L** | Client-local: stored in `config::Settings` (TOML, `serde(default)`). |
| ✔ | Implement. |
| ↗ | Hand off to the official app or web. |
| ✗ | Skip. |

## 0. Editor's corrections and decisions

| # | Issue | Resolution |
|---|---|---|
| 1 | FEATURES_NOTES §3.1 says "legacy JSON: use this, proto is plan B" | **Superseded by the code.** Rustcord already writes settings through proto: `api/account.rs::edit_settings` does GET → `settings_proto::edit` → PATCH, with an `out_of_date` retry. Most 2024–2026 options (6.29+, 7.11+, 8.21+, 21, 23) exist **only** in the proto. Use proto for everything. JSON keys are listed only as aliases. |
| 2 | "BoolValue `false` must be an empty submessage" | Too strict. It must be **present**. Both `{}` and `{1:0}` decode as false. The existing `put_wrapped_varint(out, N, 0)` is enough, so Bool/U32/U64 need **no new writer**. New writers needed: plain varint/enum (`key(out,N,0); put_varint(out,v)`, omitted when 0), FloatValue (`{1: fixed32}`, wire type 5), plain float (wire type 5). |
| 3 | `P7.5` / `P7.6` are "fixed64 expiry" | `P7.5 quiet_mode` is a **Bool**. `P7.6 focus_mode_expires_at_ms` is fixed64. |
| 4 | Ash = DARK and Dark = DARKER marked UNCONFIRMED | **Ash = THEME_DARK (1):** Discord: "These days the OG dark theme goes by Ash." **Onyx = THEME_MIDNIGHT (4):** "Onyx, previously known as Midnight on mobile". **Dark = THEME_DARKER (3)** by elimination. |
| 5 | Tray "needs a new dependency" | **Wrong.** `src/notify.rs` already creates a `Shell_NotifyIconW` tray icon. |
| 6 | Aug 2026 renames | **Confirmed** (Aug 4 2026 patch notes): "Activity is now Games & Apps", "Content & Social is now Messaging Permissions", Authorized Apps/Connections and OS-specific pages/keybinds "consolidated". Where they now live is not stated. The "profile can be set to Private" claim comes from third-party sites; the patch notes only mention a per-server private-profile fix. |
| 7 | Omitted notification fields | Added `7.4`, `7.8`, `7.10`, `7.13` and `7.20` (§4 Notifications). |
| 8 | Notation `P3.{gid}.11`, `P23.{app}.2.1` | Exact paths: `3.1[gid].11` (map field 1 of AllGuildSettings) and `23.1[app_id].2.1`. |

### Code needed for proto settings (one helper)

```rust
// api/settings_proto.rs — every proto toggle is one subfield of one top-level field.
pub fn with_sub(current: &str, top: u32, sub: u32, put: impl FnOnce(&mut Vec<u8>)) -> Result<String, String> {
    edit(current, top, &[sub], put)
}
// e.g. compact mode:  with_sub(cur, 6, 17, |o| put_wrapped_varint(o, 17, on as u64))
// plain enum (omit 0): with_sub(cur, 13, 1, |o| if v != 0 { key(o, 1, 0); put_varint(o, v) })
```

- **Reading.** Add one `pub fn get_sub(b64, top, sub) -> Option<…>` built from the existing `parse` / `merged` / `wrapped_varint`. Absent means use the documented default.
- **Debounce.** Each write is GET+PATCH, so debounce about 1 s per top-level field in the UI. UD: frequent writes are "batched" by Discord (10 s).
- **Test vectors.** Verified with Python; add them as unit tests:

| Input | Base64 |
|---|---|
| `{13:{2:1}}` | `agIQAQ==` |
| `{13:{1:2}}` | `agIIAg==` |
| `{6:{17:Bool(true)}}` | `MgWKAQIIAQ==` |
| `{6:{17:{}}}` (false, empty) | `MgOKAQA=` |
| `{6:{17:{1:0}}}` (false, explicit) | `MgWKAQIIAA==` |
| `{19:{1:{}}}` | `mgECCgA=` |

### What Rustcord has today

| Area | What exists |
|---|---|
| Pages (`ui/user_settings.rs` `Page`) | My Account, Profiles, Profile Cosmetics, Connections, Data & Privacy, Local Profile (USER); Appearance, Notifications, Voice & Video (APP) |
| Data & Privacy | Only the local "Send read receipts" |
| Appearance | Theme, member list, gradients |
| Notifications | Desktop notifications, plus per-server levels |
| `config::Settings` | `theme`, `desktop_notifications`, `send_read_acks`, `show_member_list`, `animate_others`, voice in/out/threshold/`voice_input_volume`/PTT/key, `camera`, `frequent_emoji`, `favorite_gifs` |

**Target sidebar** (2026 names):

- **User:** Account (+ Devices, Account Standing, MFA), Profiles, Profile Cosmetics, Content & Social → *Messaging Permissions*, Data & Privacy, Family Center, Authorized Apps + Connections, Devices.
- **App:** Appearance, Accessibility, Voice & Video, Chat, Notifications, Keybinds, Language, Windows, Streamer Mode, Advanced.
- **Games & Apps:** Activity Privacy, Registered Games.
- **Rustcord:** Plugins (PLUGINS_NOTES §1), Local Profile, Theme editor.
- **Billing:** a single link out.

## 1. USER SETTINGS

### Account

Several actions are MFA-gated and need the `/mfa/finish` prompt that ACCOUNT_SERVER_DESIGN lists as missing.

| Option | Storage / endpoint | Status |
|---|---|---|
| Display name, username, password | FEATURES_NOTES §3.2 (store the returned `token`) | ✔ |
| Email (masked, Reveal) | `PATCH /users/@me {email,email_token,password}` plus email verification | ↗ |
| Phone | Verification and captcha | ↗ |
| Authenticator app (enable) | `POST /users/@me/mfa/totp/enable {password}` → 60005 → client makes a 32-char base32 secret and shows QR/secret → `POST {password,secret,code}` → `{token,backup_codes}` | ✔ careful |
| Remove authenticator | `POST /users/@me/mfa/totp/disable` (MFA) → `{token}` | ✔ |
| SMS backup | `POST /users/@me/mfa/sms/enable\|disable {password}` (needs TOTP plus verified phone) | ✔ |
| Security keys / passkeys | `GET/PATCH/DELETE /users/@me/mfa/webauthn/credentials[/{id}]`; adding needs WebAuthn | list/rename/delete ✔, add ✗ |
| Backup codes | `POST /auth/verify/view-backup-codes-challenge {password}` → `{nonce,regenerate_nonce}` plus emailed key → `POST /users/@me/mfa/codes-verification {key,nonce,regenerate}` | ✔ |
| Disable / delete account | `POST /users/@me/disable` / `POST /users/@me/delete {password}` (MFA) | ↗ |
| Account Standing | `GET /safety-hub/@me` → `account_standing.state`: 100 ALL_GOOD, 200 LIMITED, 300 VERY_LIMITED, 400 AT_RISK, 500 SUSPENDED, plus `classifications[]` | ✔ read-only; appeals ↗ |

### Profiles

Profile editing itself is in FEATURES_NOTES §3.2 and COSMETICS_NOTES. Proto items on or near this page:

| Option | Storage | Default | Status |
|---|---|---|---|
| Show legacy username badge | `P8.22 hide_legacy_username` Bool (inverse) | shown | ✔ |
| Profile visibility | `P8.32` plain enum: 0 UNSET (= 3), 1 FRIENDS_ONLY ("Private"), 2 FRIENDS_AND_SMALL_GUILDS (threshold UNCONFIRMED), 3 FRIENDS_AND_ALL_GUILDS | 3 | ✔ (third parties place it under Data & Privacy > Profile Privacy) |
| Show my local time | `P8.31 show_local_time` Bool; meaning UNCONFIRMED | true | ✔ optional |

### Messaging Permissions (was Content & Social)

| Option | Values | Default | Storage |
|---|---|---|---|
| Sensitive media filter | 3 dropdowns: friend DMs / other DMs / servers. Values: 1 SHOW, 2 BLUR, 3 BLOCK (servers only SHOW/BLUR) | friends SHOW, others BLOCK, servers SHOW (UD + third party) | `P6.29 {1 guilds, 2 friend_dm, 3 non_friend_dm}` |
| Graphic media filter | same | same | `P6.32` (same shape) |
| Self-harm content filter | same; UI presence UNCONFIRMED | same | `P6.35` |
| Legacy explicit filter | 0 off, 1 non-friends, 2 all | — | `P6.19 U32` / `J:explicit_content_filter`; write only if 6.29 is absent |
| Keyword filters (profanity / sexual / slurs) | 3 toggles | off | `P6.30 {1,2,3} Bool`. The setting is ✔; hiding needs word lists that are not public (✗). |
| DM spam filter | 1 DISABLED, 2 NON_FRIENDS, 3 FRIENDS_AND_NON_FRIENDS | UNSET = 2 | `P6.27` plain enum (`6.26 U32` is the old one) |
| DMs from server members (new servers) | toggle | on | `P8.4` plain bool (inverse) + `P8.27 Bool`. Per server: `P8.3` repeated fixed64 / `J:restricted_guilds`. "Apply to existing servers" = write every guild ID. |
| Message requests from members | toggle | on | `P8.17 Bool` (inverse); per server `P8.16` |
| Age-restricted servers on iOS | toggle | off | `P6.20` |
| Age-restricted app commands in DMs | toggle | off | `P6.23` |
| Inappropriate-conversation warnings | toggle | on | `P8.23` |
| Friend requests: Everyone / Friends of friends / Server members | checkboxes | all | `P8.11 U32` flags 2 MUTUAL_FRIENDS \| 4 MUTUAL_GUILDS \| 8 NO_RELATION (needs 2\|4), so all = 14 |
| Hide friend-request notes | toggle | off | `P8.33` |
| In-game DMs (Social SDK) | 1 ALL, 2 USERS_WITH_GAME, 3 NONE | UNSET = 1 | `P8.28` plain enum |
| Game-friend DMs in Discord | toggle | on | `P8.26` |
| Safety preset | 1 Balanced, 2 Strict, 3 Relaxed, 4 Custom; UI placement UNCONFIRMED | — | `P21.1` plain enum |
| Hide blocked-profile warning | toggle | off | `P21.2` plain bool |
| Leaderboards default for new servers | 0 / 1 | 0 | `P8.25` plain enum; per server `P3.1[gid].11` plain bool |

- All rows are ✔.
- Enforcing the sensitive-media filter needs Discord's attachment classification. Which flag carries it is UNCONFIRMED; until then, show the setting only.
- **Per-server privacy menu** (server context menu): DMs `8.3`, message requests `8.16`, activity status `8.13`, activity joining `8.15`, leaderboards `3.1[gid].11`. Build these in the same work.

### Data & Privacy

| Option | Storage | Status |
|---|---|---|
| Use data to improve Discord | `GET /users/@me/consent` → `{usage_statistics:{consented}, personalization:{…}}`; `POST /users/@me/consent {grant:[…], revoke:[…]}`; READY `consents` | ✔ |
| Personalize my experience | consent `personalization` | ✔ |
| Marketing toggle (BGR) | storage UNCONFIRMED | skip |
| Record my voice in Clips | `P19.1 allow_voice_recording` Bool (default true) | ✔ |
| Screen-reader detection | `P8.7` plain bool (default false) | ✔ |
| Quests third-party data | `P8.30 quests_3p_data_opted_out` Bool (inverse) | ✔ |
| Ad topic opt-outs | `P8.34` repeated enum (1 REAL_MONEY_GAMING) | ✔ |
| Show my messages from discoverable channels on web pages | `P8.35 swp_message_promotion_opted_out` Bool (inverse); mapping inferred, UNCONFIRMED | ✔ once confirmed |
| `P8.36 personal_access_opted_out` | meaning UNCONFIRMED | skip |
| Help improve spam models | `P8.19 non_spam_retraining_opt_in` | ✔ |
| Find me by phone / email | `P8.12 U32` flags: 2 phone, 4 email | ✔ |
| Contact sync | `P8.10` (mobile) | ✗ |
| Request all my data | `GET /users/@me/harvest` (204 = none, otherwise `{state,status,created_at,completed_at,backends}`); `POST {"backends":[…]}` (empty = all; delivered by email) | ✔ |
| Read receipts (Rustcord) | L `send_read_acks` | exists |

### Family Center

- `P8.21 family_center_enabled_v2` (sidebar toggle) ✔.
- Linking is a mobile QR flow ↗: show "Manage in the Discord mobile app". Endpoints: `/family-center/@me`, `/users/@me/linked-users`.

### Authorized Apps + Connections (merged in 2026)

**Authorized apps:**
- List: `GET /oauth2/tokens` → `[{id, scopes[], application{id,name,icon,description}}]`.
- Deauthorize: `DELETE /oauth2/tokens/{id}` → 204. ✔

**Connections:**
- List: `GET /users/@me/connections` (USER_CONNECTIONS_UPDATE → refetch).
- Toggles: `PATCH /users/@me/connections/{type}/{id}` with `{visibility:0|1}`, `{metadata_visibility}`, `{show_activity}` or `{friend_sync}`. ✔
- Remove: `DELETE`. Confirm first: it also removes you from servers joined through that integration. ✔
- Add: the provider redirects back to discord.com → ↗.
- "Detect accounts on this computer": `P8.8 detect_platform_accounts` Bool (default true). ✔ setting only.

### Devices

- List: `GET /auth/sessions` → `{user_sessions:[{id_hash, approx_last_used_time, client_info{os,platform,location}}]}`.
- This device: READY `auth_session_id_hash`, updated by AUTH_SESSION_CHANGE.
- Log out one or all: `POST /auth/sessions/logout {session_id_hashes:[1..64]}`. MFA-gated. ✔

### Clips

- Only `P19.1` syncs; everything else is L.
- Capture and encode → ✗ for v1.

## 2. BILLING (skip)

Nitro, Boosts, Subscriptions, Gift Inventory, Billing, Shop and Orbs: a link only. `P21.3` spending limit is ✗. `premium_type` gates Nitro-only controls.

## 3. APP SETTINGS

### Appearance

| Option | Values | Default | Storage | Status |
|---|---|---|---|---|
| Theme | **Light 2, Ash 1, Dark 3, Onyx 4**, plus "Sync with computer" (L) | Dark | `P13.1` plain enum / `J:theme` dark/light/darker/midnight | ✔ Map to Rustcord palettes; keep Rustcord themes as extras. |
| Colour themes (Nitro gradients) | preset or custom | — | `P13.3.2 U32` preset ID; `P13.3.4 {1 colors[] hex str, 2 stops[] float, 3 angle i32, 4 base_mix i32}` | Render ✔; writing probably Nitro-gated (UNCONFIRMED) |
| Dark sidebar (Light theme) | toggle | off | `P13.15` plain bool | ✔ |
| UI density | 1 COMPACT, 2 COZY, 3 RESPONSIVE, 4 DEFAULT; Spacious = COZY is UNCONFIRMED | DEFAULT | `P13.12` plain enum | ✔ |
| Message display | Cozy / Compact | Cozy | `P6.17 Bool` / `J:message_display_compact` | ✔ |
| Avatars in compact mode | toggle | UNCONFIRMED | `P6.41 Bool` | ✔ |
| Space between message groups | 0 / 4 / 8 / 16 / 24 px | 16 | L | ✔ |
| Zoom | 50–200 % (Ctrl +/−/0) | 100 % | L | ✔ `ctx.set_zoom_factor` |
| Hour cycle | 0 AUTO, 1 H12, 2 H23 | AUTO | `P13.9` plain enum | ✔ |
| Server theme source | 1 PERSONAL, 2 GUILD | — | `P13.14`; per server `P3.1[gid].13` | later |
| Search shows exact result count | toggle | UNCONFIRMED | `P13.8 Bool` (UI placement UNCONFIRMED) | optional |
| Mobile-only | — | — | `13.4`, `13.6`, `13.7`, `13.10`, `13.11`, `13.13` | ✗ |
| Use Discord font (gg sans) | toggle | on | L (FONTS_NOTES) | ✔ Rustcord-specific |

### Accessibility

| Option | Default | Storage | Status |
|---|---|---|---|
| Chat text size: 12 / 14 / 15 / 16 / 18 / 20 / 24 px | 16 | L | ✔ |
| Saturation 0–100 % (+ apply to custom colours) | 100 | L | ✔ |
| Contrast / high contrast (+ sync with Windows) | off | L | ✔ simple |
| Role colours: in names / next to names (dot) / hide | in names | L | ✔ (BetterRoleDot overlaps) |
| Always underline links | off | L | ✔ |
| Show others' display-name styles | on | L (UNCONFIRMED) | ✔ |
| Reduced motion (+ sync with OS) | sync | L | ✔ |
| Autoplay GIFs while focused | on | `P6.11` / `J:gif_auto_play` | ✔ replaces `animate_others` |
| Play animated emoji | on | `P6.14` | ✔ |
| Stickers: 0 always / 1 on interaction / 2 never | 0 (UNCONFIRMED) | `P6.15 U32` / `J:animate_stickers` | ✔ |
| Allow `/tts` | on | `P6.16` | ✔ with TTS |
| TTS rate | — | L | ✔ with TTS |
| Show send-message button | UNCONFIRMED | L | ✔ |
| Legacy chat input | off | `P6.24` (`6.2` is the inverse, old) | ✔ (Rustcord is effectively legacy) |
| Image descriptions (alt text) | off | `P6.7` | ✔ |
| High dynamic range | — | L | ✗ |

### Voice & Video

All L unless noted. Rustcord already stores device, threshold, PTT and key.

- **Devices:** input and output (default = system); input volume 0–100 %, output 0–200 %; mic test.
- **Input profile:** Voice Isolation / Studio / Custom.
- **Input mode:** Voice Activity / PTT, key, release delay (default ~20 ms, UNCONFIRMED).
- **Sensitivity:** auto (on), manual slider down to −100 dB.
- **Processing:** echo cancellation; noise suppression (Krisp / Standard / None); advanced voice activity; AGC; bypass system processing. Offer only what the voice stack can do.
- **Attenuation:** 0–100 %, "when I speak" / "when others speak".
- **Video:**
  - "Always preview video": `P5.5 Bool` (default false) ✔.
  - Backgrounds `P5.1` blur `{1 use_blur}` / `P5.2` preset / `P5.3 {1 id fixed64, 2 asset_hash}`: ✗ (needs segmentation).
- **Streaming:** "Disable stream previews": `P5.10 Bool` ✔. Quality is L.
- **Soundboard:** volume `P5.9.1` plain float ✔. Soundmoji volume `P5.11 F32` (default 100) ✔. Entrance sound `P3.1[gid].6 {sound_id, guild_id}` (Nitro).
- **Advanced:** codec, QoS, audio subsystem, capture method, debug log, reset. All L; only what is supported.
- **Per-user volume and mute (synced):**
  - Stored in `P16.1` (user) / `P16.2` (stream) maps `<fixed64, {1 muted bool, 2 volume float 0–200, 3 modified_at fixed64, 4 soundboard_muted bool}>`.
  - Write: `PATCH /users/@me/audio-settings/{user|stream}/{user_id} {muted?, volume?, soundboard_muted?}` → 204 + AUDIO_SETTINGS_UPDATE. ✔
  - VolumeBooster values above 200 stay L.

### Chat (was Text & Images)

All ✔ except spell check. They are cheap render gates in the message view.

| Option | Default | Storage |
|---|---|---|
| Show media posted as links | on | `P6.10` / `J:inline_embed_media` |
| Show media uploaded to Discord | on | `P6.9` / `J:inline_attachment_media` |
| Image descriptions | off | `P6.7` |
| Embeds and link previews | on | `P6.12` / `J:render_embeds` |
| Emoji reactions | on | `P6.13` / `J:render_reactions` |
| Convert emoticons | on | `P6.21` / `J:convert_emoticons` |
| Expression suggestions | on | `P6.22` |
| Inline emoji suggestions | UNCONFIRMED | `P6.40` |
| Mention suggestions | on | `P6.34` |
| Stickers / soundmoji / game mentions in autocomplete | off / on / UNCONFIRMED | `P6.28` / `P6.31` / `P6.39` |
| Command suggestions | on | `P6.8` |
| Spoilers | ON_CLICK | `P6.4 Str`: "ON_CLICK" / "IF_MODERATOR" / "ALWAYS" |
| Threads in split view | on | `P6.3` |
| Cross-DM search | off | `P6.36` |
| "Search with…" | Google | `P6.37` plain enum: 1 Google, 2 Bing, 3 DuckDuckGo, 4 Custom; plus `P6.38 Str` URL |
| Default reaction emoji | — | `P6.33` |
| Spell check | — | L (Electron) ✗ |

### Notifications

| Option | Default | Storage | Status |
|---|---|---|---|
| Desktop notifications | on | L | exists |
| Unread badge on taskbar | on | L | ✔ overlay icon |
| Taskbar flashing | on | L | ✔ `FlashWindowEx` |
| Push inactive timeout: 1–10 min | 60 s (UD) | likely `P5.6 afk_timeout U32` seconds / `J:afk_timeout` (mapping UNCONFIRMED) | ✔. After this much idle, also send op 3 `afk:true`. |
| Reaction notifications: 0 all / 1 DMs only / 2 never | 0 | `P7.7` plain enum | ✔ |
| Super-reaction notifications | on | `P7.4 Bool` | ✔ |
| Group-DM reactions on all messages | UNCONFIRMED | `P7.20 Bool` | ✔ |
| In-app notifications | on | `P7.1` | ✔ |
| Game activity notifications: 1 off / 2 on / 3 only games played | UNSET | `P7.8` plain enum; `P7.10 Bool` excludes Steam | ✔ |
| TTS notifications: all / current channel / never | never | L | ✔ with TTS |
| Sounds | on | L | ✔ (for the sounds Rustcord plays) |
| Receive friends' Go Live | on | `P5.7` | ✔ |
| Notify friends / server members when I go live | off / ? | `P7.2` / `P7.26` | ✔ |
| Notify friends when I come online / update my profile | on | `P7.25` / `P7.24` | ✔ |
| Activity notifications | on | `P7.11` voice activity, `P7.12` friend online, `P7.13` friend returning ("resurrection"), `P7.14` anniversaries, `P7.15` game updates, `P7.16` profile updates, `P7.17` server trending, `P7.18` DM reply nudges, `P7.19` summaries, `P7.21` friend gaming, `P7.22` upcoming events, `P7.23` screen-time schedule | ✔ (mostly inbox "For You" and mobile push) |
| Quiet mode / focus mode | off | `P7.5 Bool` / `P7.6` fixed64 expiry ms (UI UNCONFIRMED) | later |
| Custom-status push (mobile) | — | `P7.9` | ✗ |
| Email notifications | — | `GET/PATCH /users/@me/email-settings {"settings":{"categories":{…}}}`. Categories: communication, social, recommendations_and_events, tips, updates_and_announcements, family_center_digest | ✔ |

- **Sounds list** (UNCONFIRMED): a master "disable all" plus per sound: message, mute, unmute, deafen, undeafen, disconnect, PTT on/off, user join/leave/moved, incoming/outgoing ring, stream start/end, viewer join/leave, activity start/end/join.
- **No visible toggle** (skip): `PATCH /users/@me/notification-settings {flags}`.

### Keybinds (L)

- Actions (list UNCONFIRMED): PTT (normal, priority), push to mute, toggle mute / deafen / VAD, streamer mode, back/forward, disconnect, camera, go live, overlay.
- A read-only list of defaults (Ctrl+K, Alt+↑/↓, Shift+Esc, Ctrl+Shift+M/D…).
- ✔ for voice and navigation.
  - Toggles: `RegisterHotKey`.
  - PTT needs key-up: Rustcord stores a VK; `Win32_UI_Input_KeyboardAndMouse` is enabled.

### Language

- Locale: `P12.1 Str` / `J:locale` (default en-US). Store it and send it as `X-Discord-Locale` and the super-properties locale. ✔
- Timezone: `P12.2` Int32Value minutes and `P12.3 Str` IANA name. Written from the OS. ✔
- Translating Rustcord's own UI is separate work.

### Windows (L)

| Option | Implementation |
|---|---|
| Open on startup | HKCU `Software\Microsoft\Windows\CurrentVersion\Run` value. Needs the windows-sys `Win32_System_Registry` feature (a feature, not a crate). |
| Start minimized | startup argument |
| Close button minimizes to tray | the existing tray (`notify.rs`) |
| Install updates automatically | `Settings::auto_update` (default on) -> `update::set_auto`. Off: the check still runs and the title bar says "Update available"; a click downloads. Disabled on copies that can't update themselves (portable, dev, read-only) and while the updater is off (debug builds, `RUSTCORD_*` test hooks). docs/UPDATER_DESIGN.md §15.3 |
| Check for updates | button on the "Version 0.1.0" row (`update::check_now`); the row's description is the updater's state. Same section "Updates" |

All ✔.

### Streamer Mode (L)

All ✔ and cheap:

- enable;
- auto-enable when OBS, XSplit or Streamlabs runs (process scan);
- hide personal info: email, phone, connections, notes, DM previews;
- hide invite links;
- disable sounds;
- disable notifications;
- hide the window from capture: `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)`.

### Advanced

| Option | Storage | Status |
|---|---|---|
| Developer mode ("Copy ID") | `P13.2` plain bool / `J:developer_mode` | ✔ |
| Hardware acceleration | L (restart; renderer switch) | optional |
| Application test mode | — | ✗ |
| RTC panel voice states (debug) | `P9.1` | ✗ |

## 4. GAMES & APPS (was Activity)

### Activity Privacy

| Option | Default | Storage | Status |
|---|---|---|---|
| Share detected activity | on | `P11.3 show_current_game` Bool / `J:show_current_game` | ✔ Gates op 3 activities; also used by GameActivityToggle. |
| Activity status default in new servers | region-dependent | `P8.29` plain enum: 1 OFF, 2 ON_FOR_LARGE_GUILDS, 3 ON (`8.14` is the old v1 enum 0/1/2) | ✔ |
| Per-server activity status | — | `P8.13` repeated fixed64 | ✔ |
| Friends can join your game | on | `P8.1` / `J:allow_activity_party_privacy_friends` | ✔ |
| Voice-channel members can join | on | `P8.2` | ✔ |
| Per-server activity joining | — | `P8.15` | ✔ |
| Recent games on profile | on | `P8.24` | ✔ |
| Per-app activity sharing | — | `P23.1[app_id].2.1` plain bool `disable_application_activity_sharing` (+ `23.1[app].1.2` app DM mobile push) | ✔ |
| Broadcast | — | `P18` | ✗ |

### Registered Games

- Detection list from `GET /games/detectable` (`executables[{os,name}]`, `aliases`) plus `/games/detectable/exclusions`.
- Process scan → op 3 `{type:0, name, application_id, timestamps.start}`. "Add it!" is L.
- ✔ medium effort. A local RPC pipe (`\\.\pipe\discord-ipc-0`) is a separate, larger feature.
- `P10` game library: ✗ obsolete.

### Game Overlay

✔ out-of-process overlay (Windows only), docs/OVERLAY_DESIGN.md. Rustcord's own windows over the game, no injection: voice widget and notifications; page hidden on Linux and macOS.

## 5. Order of work

1. **One page per proto toggle group, using `with_sub`/`get_sub`:**
   - Chat (all of 6.x);
   - Accessibility proto rows;
   - Appearance (13.1/13.2/13.9/13.12/13.15, 6.17/6.41);
   - Messaging Permissions (6.27/6.29/6.30/6.32, 8.3/8.4/8.11/8.16/8.17/8.27/8.28/8.33);
   - Data & Privacy (consents, 19.1, 8.7/8.12/8.19/8.30/8.34);
   - Notifications (7.x, 5.7);
   - Activity Privacy (11.3, 8.1/8.2/8.13/8.15/8.24/8.29);
   - Language (12.1).
2. **REST pages:** Devices, Authorized Apps, Connections toggles/remove, harvest, email settings, Account Standing. TOTP and backup codes come after the MFA prompt.
3. **Local:** text size, zoom, saturation, role-colour style, underline links, reduced motion, sounds, badge/flash, streamer mode, startup/tray, keybinds.
4. **Hand-offs and skips:** as marked.

## 6. Still UNCONFIRMED

- 2026 sidebar placement of merged pages.
- Spacious = COZY.
- Placement of 21.1 and 13.8.
- Meaning of 8.31, 8.35 and 8.36.
- Mapping of 5.6 to the push timeout.
- Keybind and sound lists.
- Some defaults (stickers, 6.40, 6.41).
- Which attachment flag carries the sensitive-media classification.
- Whether connections can be added outside the browser.
- Whether writing 13.3 needs Nitro.

## Sources

- **Protos:**
  - https://raw.githubusercontent.com/discord-userdoccers/discord-protos/master/discord_protos/discord_users/v1/PreloadedUserSettings.proto
  - `…/FrecencyUserSettings.proto`
- **UD (userdoccers):** https://github.com/discord-userdoccers/discord-userdoccers, pages `resources/user-settings`, `user-settings-proto`, `user`, `authentication`, `topics/oauth2`, `connected-accounts`, `family-center`, `safety-hub`, `game`, `gateway/gateway-events`, `topics/read-state`
- **Discord (official):**
  - https://discord.com/blog/discord-patch-notes-august-4-2026 (renames, verified)
  - https://discord.com/blog/improving-mobile-with-squircles-styles-and-spacing (Ash / Onyx, verified)
  - https://discord.com/blog/discord-patch-notes-june-4-2026 (Account nesting; researcher)
- **Third-party (labels only):**
  - https://www.bgr.com/2264019/how-to-protect-privacy-discord-settings/
  - https://www.discords.ai/wiki/discord-settings-client-updates-guide-august-release-overview
  - https://stayonguard.app/guides/block-explicit-content/discord/
  - https://note.com/fuso_wasedr09/n/nc69eaa7a9380
- **Rustcord code:** `src/api/settings_proto.rs` (`edit`, `put_wrapped_varint`), `src/api/account.rs::edit_settings`, `src/ui/user_settings.rs::Page`, `src/config.rs::Settings`, `src/notify.rs`

Not checked: the live client, so exact 2026 labels and some defaults. support.discord.com refuses automated fetches (403). Risk: always write a whole top-level field through `edit()`. A wrong write resets sibling settings on every client the user owns.
