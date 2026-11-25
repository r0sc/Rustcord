# Rustcord feature notes (user-account API, web-client parity)

Edited 2026-11-11. This merges four research drafts (chat essentials; threads/forums/friends/DMs/inbox; account and server; layout reference), checks them against each other, against the cached sources and against the current Rustcord code, and fixes what was wrong. Voice, video and screen share are in `docs/VOICE_NOTES.md`.

What this file does **not** repeat (already in `docs/PROTOCOL_NOTES.md` / `docs/AUTH_NOTES.md`): login, gateway connect/zlib/resume, READY handling and capabilities table, message history/send/edit/delete/reply/reactions (P1 §4), read-state acks (P1 §4.6, P2 §5), op 37 member list (P2 §1–2), permissions maths (P2 §3), CDN URLs (P1 §5), markdown (P3), corrections A1–A10 and B1–B14.

**Conventions.** UNCONFIRMED = no fetched source states it. INFERENCE = derived by the researcher. "Official client" behaviour that is not marked was stated by a source.

**Live testing.** None. The Claude-in-Chrome extension reported **no connected browser** (checked again during editing) and the built-in browser pane is logged out. Nothing in this file comes from a captured request of the user's web session; every "the web client does X" without a source is UNCONFIRMED.

**Source keys**
| Key | Source |
|---|---|
| UD | discord-userdoccers `master` raw MDX, rendered at https://docs.discord.food/... (cached copies were used for the spot checks below) |
| OFF | discord/discord-api-docs `main`, https://discord.com/developers/docs/... and its change log |
| DPS | dolfies/discord.py-self `master` (`http.py`, `state.py`, `client.py`, `settings.py`, `tracking.py`, `message.py`, `invite.py`) |
| ABD | uowuo/abaddon `src/discord/discord.cpp`, `channellisttree.cpp` |
| DCO | ayn2op/discordo `internal/ui/chat/composer/` |
| NIN | diamondburned/ningen `v3/states/member/member.go` |
| ERR | gist.github.com/Dziurwa14/de2498e5ee28d2089f095aa037957cbb (error codes compiled from UD) |
| BUNDLE | Discord production web assets fetched 2026-11-11 (`https://discord.com/assets/web.f25377cf5b911919.js` + 1,257 CSS chunks) |
| LIVE | unauthenticated `curl` probes of `/api/v9/gifs/*` (6 calls, 2 got 429) |

---

## 0. Editor's corrections and cross-cutting rules

### 0.1 Contradictions and errors in the drafts, resolved

| # | Draft claim | Verdict | Evidence |
|---|---|---|---|
| 1 | Threads draft: "Rustcord caps 509" | **Wrong.** `src/gateway/conn.rs:36` is `CAPABILITIES = 253` (bits 0, 2–7). 253 still contains LAZY_USER_NOTES (bit 0) and PRIORITIZED_READY_PAYLOAD (bit 5), so the statements built on it (no `notes` in READY, presences in READY_SUPPLEMENTAL) hold. Adding AUTO_CALL_CONNECT gives **4349**, not 4605. | code |
| 2 | Chat draft: op 3 `since` = "milliseconds since the client went idle" | **Wrong wording.** It is the **Unix timestamp in ms** when idle started, else 0. | UD gateway-events "Update Presence" table |
| 3 | Threads draft: forum post with files = multipart | Rustcord's reqwest has no `multipart` feature and the chat draft recommends cloud uploads. **Use cloud uploads for forum posts too** (`message.attachments[].uploaded_filename`). UD cloud-uploads says generically "instead of sending the attachment again in a form body … just send the `upload_filename`"; that it works on `POST /channels/{forum}/threads` specifically is UNCONFIRMED (INFERENCE, high confidence). | UD topics/cloud-uploads |
| 4 | 20 MiB default upload limit | **Confirmed**: OFF change log entry dated **September 3, 2026**, "increased from 10 MiB to 20 MiB for users, bots, webhooks…". UD still says 10 MiB (stale). | OFF change log |
| 5 | PIN_MESSAGES cut-over date | OFF has two entries: the Aug-2025 entry says MANAGE_MESSAGES stops pinning on Jan 12 2026; the later entry moves it to **Feb 23 2026**. Use the later one; it is past, so require PIN_MESSAGES (1<<51) now. | OFF change log |
| 6 | READY `user_settings_proto` present with caps 253? | UD READY table: `user_settings` has footnote 4 (omitted with USER_SETTINGS_PROTO); `user_settings_proto` only has footnote 1 ("not for bots"). So **both** arrive with 253 (INFERENCE from footnotes; "if missing, defaults should be used"). | UD gateway-events READY |
| 7 | Threads draft: THREAD_LIST_SYNC needs `typing:true` before `threads:true` (DPS warning) | **Not an issue for Rustcord**: `conn.rs:531-535` already sends `typing`, `activities` and `threads: true` in the same op 37 guild object. | code |
| 8 | Typing resend: 10 s (DCO) vs 8 s | Use **8 s** (matches PROTOCOL_NOTES B6 and keeps the remote 10 s indicator alive). | — |
| 9 | Op 8 rate limit | Already resolved in PROTOCOL_NOTES A3: OFF documents the 1-per-30 s limit only for "all members" (`query:""`, `limit:0`) requests; scope for user `query` requests UNCONFIRMED. | OFF change log |
| 10 | `GET /channels/{c}/messages/{id}` | **Confirmed not usable by user accounts** (UD message.mdx). Use `?around={id}&limit=1` or `limit=50`. Same for `GET /guilds/{g}/threads/active`, `GET /channels/{c}/threads/active`, `GET /channels/{c}/thread-members`. | UD |
| 11 | DAVE mandatory since 2026-03-01, Stage exempt | **Confirmed** (UD voice-connections and OFF change log "Deprecating Non-E2EE Voice Calls"). See VOICE_NOTES. | UD, OFF |
| 12 | Thread create default type | **Confirmed**: `type` defaults to PRIVATE_THREAD (12) on `POST /channels/{c}/threads`. Always send `"type":11` for public. | UD channel.mdx |
| 13 | Identify `status:"unknown"` | **Confirmed** in `conn.rs:554`. | code |
| 14 | Proto test vectors (§3.1.4) | **Re-computed and confirmed** byte for byte with an independent encoder. | — |

### 0.2 Code changes these features need first (all verified in the code)

1. **401 kill-switch logs out 2FA users.** `src/api/mod.rs:198-200` sets `unauthorized = true` on every 401. Discord's MFA re-prompt is also 401: `{"message":"Two factor is required for this operation","code":60003,"mfa":{"ticket":"…","methods":[{"type":"totp","backup_codes_allowed":true},{"type":"sms"},{"type":"password"}]}}`.
   - Fix: read the body first; `401 && code==60003 && mfa` → new `ApiError::MfaRequired{ticket, methods}`; only other 401s set the flag.
   - Flow: `POST /mfa/finish {"ticket","mfa_type":"totp"|"sms"|"backup"|"password","data":"<code>"}` → `{"token":"<JWT>"}` (valid ~5 min). Retry the original request with header `X-Discord-MFA-Authorization: <JWT>`; cache it 5 min. SMS: first `POST /auth/mfa/sms/send {"ticket"}`. Source: https://docs.discord.food/authentication#mfa-verification
   - MFA-gated actions seen in the drafts: change username/password, delete guild, guild settings with `mfa_level` 1, bulk ban.
2. **`send()` cannot set extra headers.** Add `extra: &[(HeaderName, String)]` for `X-Audit-Log-Reason` (percent-encoded UTF-8, ≤512 chars; `percent-encoding` is already a dependency), `X-Discord-MFA-Authorization`, `X-Context-Properties`.
3. **`route_key` (`src/api/ratelimit.rs:46-91`) keeps non-numeric segments literally**, so every invite code, template code, upload filename and stream key becomes its own route. Add arms: `"invites" => "{code}"`, `"templates" => "{code}"`, `"attachments" if !is_id => "{upload}"`, `"streams" => "{stream_key}"`.
4. **Captcha:** detection already exists (`error_from`, 400 + `captcha_key`). Keep it generic: UD says any endpoint may return one. Never solve, never retry; tell the user to do the action in the official app.
5. **No new reqwest features are required.** `stream` is only needed if you want byte-level upload progress (§1.1).

### 0.3 Request headers for user actions

**`X-Context-Properties`** = standard base64 of compact JSON naming the UI location. Not in UD; DPS `tracking.py` calls it "essential for certain actions (e.g. joining guilds, friend requesting)". Never send it to CDN hosts.

| Location JSON | Header value | Used on (DPS) |
|---|---|---|
| `{}` | `e30=` | `POST /users/@me/channels` (1:1 DM) |
| `{"location":"Friends"}` | `eyJsb2NhdGlvbiI6IkZyaWVuZHMifQ==` | relationship calls from the Friends list |
| `{"location":"Add Friend"}` | `eyJsb2NhdGlvbiI6IkFkZCBGcmllbmQifQ==` | `POST /users/@me/relationships` |
| `{"location":"User Profile"}` | `eyJsb2NhdGlvbiI6IlVzZXIgUHJvZmlsZSJ9` | add/accept from a profile |
| `{"location":"ContextMenu"}` | `eyJsb2NhdGlvbiI6IkNvbnRleHRNZW51In0=` | add/remove/block from a context menu |
| `{"location":"DM Channel"}` | `eyJsb2NhdGlvbiI6IkRNIENoYW5uZWwifQ==` | friend actions inside a DM |
| `{"location":"New Group DM"}` | `eyJsb2NhdGlvbiI6Ik5ldyBHcm91cCBETSJ9` | `POST /users/@me/channels` (group) |
| `{"location":"Add Friends to DM"}` | `eyJsb2NhdGlvbiI6IkFkZCBGcmllbmRzIHRvIERNIn0=` | `PUT /channels/{id}/recipients/{uid}` |
| `{"location":"Guild Header"}` / `{"location":"Context Menu"}` | base64 of that JSON | `POST /channels/{id}/invites` |
| `{"location":"Join Guild","location_guild_id":"…","location_channel_id":"…","location_channel_type":0}` (also `"Accept Invite Page"`, or `"Invite Button Embed"` + `location_message_id`) | base64 of that JSON | `POST /invites/{code}` |

Thread endpoints take an analytics `location` instead (optional): create-from-message body `"Message"`; create-without-message body `"Plus Button"` / `"Thread Browser Toolbar"`; join query `?location=Banner|Toolbar Overflow|Sidebar Overflow|Context Menu`; leave query `?location=Toolbar Overflow|Context Menu|Sidebar Overflow`.

### 0.4 Error codes these features branch on (OFF opcodes table + ERR)

| code | meaning | | code | meaning |
|---|---|---|---|---|
| 10006 | unknown invite | | 50045 | file too large |
| 10013 | unknown user | | 50046 | invalid file |
| 20028/20029 | write rate limit (channel/server) | | 50074 | cannot delete community channel |
| 20031 | name has disallowed words | | 50083 | thread is archived |
| 30001 | max guilds (100/200 Nitro) | | 50084 | invalid thread notification settings |
| 30002 | max friends (1000) | | 50146 | uploaded file not found (bad/expired `uploaded_filename`) |
| 30003 | max pins (250) | | 60003 | MFA required (401, see §0.2) |
| 30004 / 30011 | max recipients (10) / max group DMs | | 80000 | incoming friend requests disabled |
| 30005 | max roles (250) | | 80001 | friend request blocked |
| 30008 / 30018 | max emojis / max animated emojis | | 80002 | bots cannot have friends |
| 30013 | max channels (500) | | 80003 | cannot friend yourself |
| 30016 | max invites (1000) | | 80004 | no user with that tag |
| 30033 | max thread participants | | 80005 | no incoming request from that user |
| 30035 | max bans for non-members | | 80006 | need to be friends |
| 30052 | bitrate too high for boost tier | | 80007 | already friends |
| 40002 | verify your account | | 80008 | discriminator required |
| 40005 | request entity too large (also HTTP 413) | | 80013 | must confirm stranger request |
| 40007 | banned from this guild | | 110000 | search index not ready (HTTP 202) |
| 40058 | cannot send in a forum channel | | 160004 | thread already exists for this message |
| 40061 | tag names must be unique | | 160005 / 160006 / 160007 | thread locked / max active threads / max active announcement threads |
| 50007 | cannot send messages to this user | | 400001 | uploads limited for this guild |
| 50024 / 50033 | wrong channel type / invalid recipients | | | |

### 0.5 Normal-client behaviour rules (apply to every section)

- Every write is the direct result of one user click or Enter. No timers that write, no background joins, no bulk friend requests, no retry of 4xx.
- 429: honour `retry_after` through the existing limiter; for long waits (channel rename, username) show a countdown instead of queueing.
- Avoid 403s (they count toward the 10k-per-10-min Cloudflare budget): gate every button on local permission bits, role hierarchy and guild `mfa_level`; fetch profiles only on click; resolve invite codes only when on screen, cached.
- Captcha → hand-off message, never a solver.
- Never send the token to `cdn.discordapp.com`, `media.discordapp.net`, `images-ext-*`, `static.klipy.com` or the Google upload host.

---

# Part 1. Chat essentials

## 1.1 File and image upload (cloud upload; no multipart)

Sources: UD topics/cloud-uploads; DPS `get_attachment_urls` / `upload_to_cloud` / `CloudFile`; OFF reference#uploading-files and change log.

**Step 1. `POST /channels/{c}/attachments`** (API host, token)
```json
{"files":[{"id":"0","filename":"SPOILER_cat.png","file_size":123456}]}
```
- 1–10 entries; `filename` ≤1024 chars; optional `is_clip`, `original_content_type`. Same permissions as an inline upload (ATTACH_FILES + SEND_MESSAGES, or SEND_MESSAGES_IN_THREADS in threads).
- `200 {"attachments":[{"id":"0","upload_url":"https://discord-attachments-uploads-prd.storage.googleapis.com/<uuid>/cat.png?upload_id=…","upload_filename":"<uuid>/cat.png"}]}`. Match on the echoed `id`.

**Step 2. `PUT {upload_url}`**, body = raw bytes.
- **No** `Authorization`, **no** `Content-Type` (DPS: "Google explodes if we do that"; a reqwest body from `Vec<u8>` sets none). `Content-MD5` optional. Any 2xx = done.
- Retry only 500/502/504/507/522/523/524, up to 5×, sleeping `1 + 2*try` s (DPS). This is part of the user's own send, not automation.

**Step 3. `POST /channels/{c}/messages`** (normal JSON send)
```json
{"content":"look","nonce":"<snowflake>","flags":0,"attachments":[
  {"id":"0","filename":"SPOILER_cat.png","uploaded_filename":"<uuid>/cat.png","description":"alt text"}]}
```
- `content` may be empty with attachments. `id` is the string index. `description` = alt text (≤1024). Optional `title`, `is_spoiler`.
- Reply / `allowed_mentions` / nonce rules unchanged (P1 §4.2).

**Cancel / cleanup**
- Unused upload: `DELETE /attachments/{upload_filename}` → 204, API host with token. DPS percent-encodes the `/` as `%2F` (`quote(v, safe='')`); copy that (raw `/` UNCONFIRMED).
- Mid-PUT: drop the future, then the DELETE above. Optional GCS cancel `DELETE {upload_url}` with `Content-Length: 0` → 204.
- Auto-expiry of unsent uploads UNCONFIRMED → always DELETE when the user removes a pending file.

**Progress (optional)**
- One PUT gives no progress. Options: reqwest `stream` feature + `Body::wrap_stream` counting bytes (feature exists in 0.13.5; no new crate in practice, it enables `tokio-util`, already in Cargo.lock), or GCS resumable chunks (`Content-Range: bytes a-b/total`, chunks multiple of 256 KiB, `308` + `Range: bytes=0-N` between, final 200/201). Discord accepting chunked PUTs is UNCONFIRMED. MVP: indeterminate spinner.

**Limits**
| Item | Value | Source |
|---|---|---|
| Files per message | 10 | UD |
| Default per-file | **20 MiB** (since 2026-09-03) | OFF change log |
| `premium_type` 1 Classic / 2 Nitro / 3 Basic | 50 / 500 / 50 MiB | UD |
| Guild `premium_tier` 2 / 3 | 50 / 100 MiB | UD (whether raised with the 20 MiB change: UNCONFIRMED) |
| Effective | `max(user, guild)`; DMs use the user limit | UD/OFF |
| Cloud PUT max | 500 MiB per file | UD |
| Whole request | < 500 MiB (Cloudflare 413); message request ≤ 200 MiB | UD |

Errors: 40005/413, 50045, 50046, 50146, 400001. Pre-check sizes client-side, trust the server.

**Spoiler and alt text**
- Mark: `SPOILER_` filename prefix (DPS always does this) or `is_spoiler:true`.
- Render as spoiler if attachment `flags & (1<<3)` **or** name starts with `SPOILER_` (OFF: "not all spoilered attachments have the prefix"). Also blur `1<<4` explicit, `1<<6` gore, `1<<7` self-harm; `1<<5` = animated.
- Edit existing attachments: `PATCH` message with `attachments` = full keep-list `[{id, description?, is_spoiler?}]`; omitted ones are removed (description/is_spoiler editable since 2026-06-24, OFF).

**Paste and drop (egui 0.37)**
- Clipboard image → `egui::Event::PasteImage(Arc<ColorImage>)`; encode PNG with the existing `image` crate; name `image.png` (official default name UNCONFIRMED).
- Drops: `ctx.input(|i| i.raw.dropped_files)`; picker: `rfd` (already a dependency).
- Official parity (UNCONFIRMED): pasted text over the 2000-char limit is offered as `message.txt`.

**Minimal implementation:** composer holds `Vec<PendingFile{source, name, spoiler, alt}>` (≤10) → on send: one step-1 call for all files, parallel PUTs, step-3 message with the nonce placeholder → on removal/cancel the DELETE.

**Checklist**
- [ ] `PendingFile` list with size pre-check from `premium_type` / guild tier
- [ ] step 1 → PUT (no auth, no content-type, 5xx retry) → step 3
- [ ] `DELETE /attachments/{%2F-encoded}` on remove/cancel
- [ ] spoiler toggle (prefix) + alt-text field; spoiler/explicit blur on render
- [ ] paste image, drag-drop, file picker
- [ ] error mapping 40005/50045/50046/50146/400001

## 1.2 Typing

- **Send:** `POST /channels/{c}/typing`, no body → 204, or `200 {"message_send_cooldown_ms"?, "thread_create_cooldown_ms"?}` when slowmode-limited (start the countdown from it).
- **When** (official cadence UNCONFIRMED): on a keystroke that changes non-empty text, at most once per **8 s**; reset the throttle on send. Not for empty text, text starting with `/`, while editing (UNCONFIRMED), or without SEND_MESSAGES (SEND_MESSAGES_IN_THREADS in threads). No "stop typing" call.
- **Receive:** `TYPING_START {channel_id, guild_id?, user_id, timestamp (unix s), member?}`; `member` gives the display name in guilds. Guilds >250 members need op 37 `typing:true` (already sent).
- **Indicator:** expire 10 s after the **last** event (UD); remove on MESSAGE_CREATE by that user in that channel; ignore self, blocked (`type 2`) and `user_ignored`. Text: `A is typing…`, `A and B are typing…`, `A, B, and C are typing…`, `Several people are typing…`. Names: guild nick → `global_name` → `username`.
- **Minimal:** `HashMap<ChannelId, HashMap<UserId, Instant>>`, pruned in the UI tick with `request_repaint_after(1s)` while non-empty; one `Option<Instant>` throttle per composer.

**Checklist:** [ ] throttled POST [ ] slowmode cooldown from 200 body [ ] TYPING_START store + expiry + MESSAGE_CREATE removal [ ] indicator text rules

## 1.3 Presence and status

**Send: op 3** (UD gateway-events#update-presence)
```json
{"op":3,"d":{"status":"dnd","since":0,"afk":false,"activities":[
  {"type":4,"name":"Custom Status","state":"brb","emoji":{"name":"🍕"}}]}}
```
- `status`: `online|idle|dnd|invisible` (`offline` receive-only; `unknown` lets the gateway use saved settings).
- `since`: **Unix ms when idle started**, else 0. `afk:true` routes pushes to mobile.
- Custom status = activity `type 4`, `name` exactly `"Custom Status"`, text in `state` (≤128), `emoji` `{name}` or `{id,name,animated}` (external custom emoji need Nitro).
- **5 per 20 s**, and the gateway pauses incoming events until the presence propagates → send only on user action. op 3 **replaces** the activity list: always send the full list.

**Persist** so other clients follow (DPS `change_presence`: op 3, then settings edit):
- `PATCH /users/@me/settings {"status":"dnd"}`
- `PATCH /users/@me/settings {"custom_status":{"text":"brb","emoji_name":"🍕","emoji_id":null,"expires_at":"2026-11-12T00:00:00.000Z"}}` (`expires_at` null = never; `"custom_status":null` clears; at most one of `emoji_id`/`emoji_name`).
- Response = full settings; fires `USER_SETTINGS_UPDATE` (partial JSON). Proto path: §3.1.
- **Follow other clients:** on `USER_SETTINGS_UPDATE` with new `status`/`custom_status` → re-send op 3 (DPS `on_internal_settings_update`).
- **Status durations** ("DND for 1 h") exist only in proto `status_expires_at_ms`; JSON has only `custom_status.expires_at`. Who reverts an expired status (server or client) is UNCONFIRMED; MVP: offer only custom-status expiry.
- **Auto-idle** (optional; ~10 min official threshold UNCONFIRMED): op 3 `status:"idle", since:now_ms, afk:true` **without** a settings write (DPS: "should be set to False for idle changes"); restore the saved status on input.

**Receive**
- `PRESENCE_UPDATE {user:{id,…partial}, guild_id?, status, activities[], client_status{desktop?,mobile?,web?,embedded?,vr?}}`. Never for yourself. Users get it for friends, implicit relationships, open-DM users and subscribed guild members. Keep every field optional. Without `guild_id` it is the user-level presence (friends list).
- `READY_SUPPLEMENTAL.merged_presences {friends:[presence+user_id], guilds:[[…]]}` index-aligned with READY guilds (P-notes A2).
- Store globally `HashMap<UserId, Presence>` (status is global; `guild_id` matters only for activities).
- Display: phone glyph if only `client_status.mobile`; custom status = type 4 as `{emoji} {state}`; others "Playing {name}" (0), "Streaming" (1), "Listening to" (2), "Watching" (3), "Competing in" (5).
- `SESSIONS_REPLACE` (and READY `sessions[]`): `[{session_id, client_info{client,os,version}, status, activities, hidden_activities, active?}]`, max 15 plus `session_id:"all"` = what others see. Use `all` for your own effective status.

**Checklist**
- [ ] status picker (online/idle/dnd/invisible) → op 3 + PATCH settings
- [ ] custom-status dialog (text ≤128, emoji, expiry: 30 min / 1 h / 4 h / today / never) → op 3 + PATCH
- [ ] read initial status from READY `user_settings` / `sessions`
- [ ] USER_SETTINGS_UPDATE → re-send op 3; SESSIONS_REPLACE → own status
- [ ] PRESENCE_UPDATE + merged_presences store; status dots, mobile glyph, activity line
- [ ] (optional) auto-idle without settings write

## 1.4 Pins

| Action | Request | Notes |
|---|---|---|
| List | `GET /channels/{c}/messages/pins?before=<ISO8601 pinned_at>&limit=1..50` | `{"items":[{"pinned_at","message"}],"has_more"}`; next page `before = last.pinned_at`. VIEW_CHANNEL; without READ_MESSAGE_HISTORY `items` is empty. Messages have no reactions. |
| List (deprecated) | `GET /channels/{c}/pins` | `[Message]`, max 50 |
| Pin / Unpin | `PUT` / `DELETE /channels/{c}/messages/pins/{m}` | 204 (old `/channels/{c}/pins/{m}` deprecated) |
| Ack pins | `POST /channels/{c}/pins/ack` | 204 → `CHANNEL_PINS_ACK {channel_id, timestamp, version}` |

- Permission: **PIN_MESSAGES (1<<51)** in guilds (MANAGE_MESSAGES alone stopped working 2026-02-23); anyone in DMs/group DMs. Limit 250 (30003).
- Events: `CHANNEL_PINS_UPDATE {guild_id?, channel_id, last_pin_timestamp?}` (not on deletion of a pinned message), `MESSAGE_UPDATE` with `pinned`, system message type 6 with `message_reference`.
- Unread pin dot: `channel.last_pin_timestamp > read_state.last_pin_timestamp` (B13); opening the popout sends the pins ack.

**Checklist:** [ ] pins popout (420 px, §4) with `before` paging [ ] Jump (§1.10) [ ] pin/unpin in context menu gated on bit 51 [ ] refresh on CHANNEL_PINS_UPDATE [ ] pin dot + ack on open

## 1.5 Search

- `GET /guilds/{g}/messages/search` (READ_MESSAGE_HISTORY) and `GET /channels/{c}/messages/search` (DMs/group DMs).
- Query (arrays = **repeated keys**): `content` (≤1024), `author_id` (≤100), `author_type` (`user|bot|webhook`, `-` negates), `mentions`, `mentions_role_id`, `mention_everyone`, `replied_to_user_id`, `replied_to_message_id`, `pinned`, `has` (`image|sound|video|file|sticker|embed|link|poll|snapshot`, `-x` negates), `embed_type`, `embed_provider` (case-sensitive), `link_hostname`, `attachment_filename`, `attachment_extension`, `channel_id` (≤500, guild only), `min_id`/`max_id`, `sort_by` (`timestamp`|`relevance`), `sort_order` (`desc`|`asc`), `offset` (≤9975), `limit` (1–25), `slop` (2), `contents`, `include_nsfw` (needs `nsfw_allowed`).
- `200 {"analytics_id","doing_deep_historical_index","documents_indexed"?, "total_results", "messages":[[Message+{"hit":true}]], "threads"?, "members"?, "channels"?}`. Inner arrays hold the single hit (no context). No `reactions`, no `member` → resolve names via op 8 `user_ids` (B8).
- Paging: `offset += 25` (don't trust `messages.len()`); `total_results` approximate; beyond 10 000 use `max_id = last hit id` with `sort_by=timestamp`.
- **202** `{"message":"Index not yet available. Try again later","code":110000,"documents_indexed":0,"retry_after":2}` → "Indexing…", retry after `retry_after` (or ~5 s if 0), at most ~5 times, as a continuation of the user's search.
- Optional newer API: `POST /guilds/{g}/messages/search/tabs` (also channel and `/users/@me/messages/search/tabs`), body `{tabs:{messages|links|media|files|pins:{…filters…, cursor?}}, channel_ids?, include_nsfw?, track_exact_total_hits?}`. Which one the web client uses now is UNCONFIRMED; GET suffices.
- Rate limits unpublished: search on Enter only.
- **Query syntax** to parse: `from:@u` → `author_id`; `mentions:@u`; `has:link|embed|file|image|video|sound|sticker|poll|forward` (`forward` → `snapshot`, INFERENCE); `in:#c` → `channel_id`; `pinned:true`; `before:`/`after:`/`during:` YYYY-MM-DD → `max_id`/`min_id` via `((ms - 1420070400000) << 22)`; rest → `content`.

**Checklist:** [ ] search box + token parser [ ] GET with repeated keys [ ] results panel (418 px) grouped by channel with Jump [ ] offset pager [ ] 202 retry UI [ ] op 8 name resolution

## 1.6 Emoji picker and `:shortcode:`

**Data**
- No API for unicode emoji. **Recommended new crate: `emojis` 0.9.1** — `(MIT OR Apache-2.0) AND Unicode-3.0`, Unicode 18.0 / gemoji 4.1.0 names, `get`, `get_by_shortcode`, `iter()` in CLDR order, `Group`, skin tones. Measured against the current Cargo.lock: **3 new crates, 0.16 MiB compressed** (`emojis` 125.8 KiB + `phf`/`phf_shared` 0.13.1 duplicates of the 0.14 already in the tree).
- Discord names differ from gemoji in places (🙂 `slight_smile` vs `slightly_smiling_face`, 🤔 `thinking` vs `thinking_face`): keep a small alias table `&[(&str, &str)]`. Exact parity UNCONFIRMED.
- Zero-dependency alternative: generate a table at build time from `emojibase-data` 18.0.0 `en/shortcodes/joypixels.json` (MIT; Discord names historically follow EmojiOne/JoyPixels). Do **not** ship Discord's own bundled emoji JSON (copyright unclear, asset names change).
- Images: Twemoji (P3 §6).

**Autocomplete** (details UNCONFIRMED): `:` + ≥2 of `[A-Za-z0-9_+-]`; prefix matches first, then substring; current guild's custom emoji before other guilds'; Tab/Enter inserts.

**On send** (the server never converts): `:name:` → unicode (`:name::skin-tone-N:` appends the modifier); usable custom → `<:name:id>` / `<a:name:id>`; unusable → plain `:name:`. Emoticons (`:)`) only when user setting `convert_emoticons` is true.

**Guild emoji and usability**
- Data: guild `emojis[]` in READY/GUILD_CREATE; `GUILD_EMOJIS_UPDATE {guild_id, emojis[]}` replaces the list; fallback `GET /guilds/{g}/emojis`; optional `GET /guilds/{g}/top-emojis` → `{items:[{emoji_id, emoji_rank}]}`.
- Rules (Nitro = `premium_type` 1/2/3; Basic's rights UNCONFIRMED): `available:false` → greyed; `roles` non-empty and member has none → disabled (hide vs disable UNCONFIRMED); animated without Nitro → locked; other guild's emoji → Nitro **and** USE_EXTERNAL_EMOJIS (1<<18) in guild channels (Nitro alone in DMs). Same rules for adding a **new** reaction; clicking an existing pill only needs READ_MESSAGE_HISTORY.
- Recents/frequents: local config `HashMap<key, {count, last_used}>`. Server sync (proto type 2 `FrecencyUserSettings` field 6 `emoji_frecency`, field 5 `favorite_emojis`, batch ~daily) is not worth it for MVP.

**Checklist:** [ ] add `emojis` (or build-time table) + alias table [ ] picker: search, rail (frequent, guilds, 8 unicode groups), 40 px grid, skin tone, inspector footer [ ] usability states [ ] `:` autocomplete [ ] conversion on send (skip code spans) [ ] local recents

## 1.7 `@mention` and `#channel` autocomplete

Candidate sources, in order:
1. **Local cache** (READY/SUPPLEMENTAL `merged_members`, op 37 items, GUILD_MEMBERS_CHUNK, MESSAGE_CREATE `member`; recent channel authors first), prefix match on nick/`global_name`/`username`, filtered by VIEW_CHANNEL.
2. **op 8 with `query`:** `{"op":8,"d":{"guild_id":["<g>"],"query":"ab","limit":50,"presences":false,"nonce":"ac:<g>:ab"}}` (`limit` required, ≤100; nonce ≤32 bytes) → `GUILD_MEMBERS_CHUNK {guild_id, members[], chunk_index, chunk_count, not_found?, presences?, nonce?}` matched by nonce. Debounce ~600 ms (NIN); skip a longer query if the shorter one returned fewer than `limit` (DCO); cache by query. Never `query:""` + `limit:0` (needs MANAGE_ROLES/KICK/BAN for users, and is the officially rate-limited form). On `RATE_LIMITED` back off `retry_after`. Whether `query` matches `global_name`: UNCONFIRMED.
3. op 35 Search Recent Members — only recent joiners and one result per query → skip. REST `GET /guilds/{g}/members/search` is **not usable by users**; `POST /guilds/{g}/members-search` needs MANAGE_GUILD → skip.
4. DMs/group DMs: recipients + self.

Roles: `mentionable:true` ones, or all with MENTION_EVERYONE (1<<17) → `<@&id>`. `@everyone`/`@here` as literal text (no ping without MENTION_EVERYONE). Users → `<@id>`. Channels: `#` → viewable text/voice/announcement/stage/forum/media channels and active threads → `<#id>`.

Composer: egui `TextEdit` has no pills. Insert `@DisplayName` and keep a side map span → token, expanded on send outside code (DCO `expandMentions`); or insert raw `<@id>`.

**Checklist:** [ ] popup above composer with header ("MEMBERS"/"CHANNELS"/"EMOJI MATCHING") [ ] local matches immediately [ ] debounced op 8 + nonce merge [ ] roles/@everyone/@here/channels [ ] expansion on send

## 1.8 GIF picker

Provider is **Klipy** (LIVE responses point to `static.klipy.com`; Tenor's API shut down 2026-06-30 per press reports). `provider=tenor` is obsolete.

| Endpoint | Query | Response |
|---|---|---|
| `GET /gifs/trending` | `media_format`, `locale` | `{"categories":[{"name","src"}],"gifs":[GIF]}` |
| `GET /gifs/trending-gifs` | `media_format`, `locale`, `limit` 20–500 | `[GIF]` |
| `GET /gifs/search` | `q`, `media_format`, `locale`, `limit` **20**–500 | `[GIF]` |
| `GET /gifs/suggest` (deprecated) | `q`, `limit` 1–50, `locale` | `["cat hug",…]` |
| `POST /gifs/select` | `{"id","q"}` | 204 analytics — skip |

- GIF (LIVE, `media_format=gif`): `{"id","title","url":"https://klipy.com/gifs/…","src":"…/x.gif","gif_src":"…/y.webp","preview","width","height"}`. Use `media_format=gif` (egui can't play video).
- `limit<20` → 400 `50035`. A second unauthenticated call within ~1 s got 429 `retry_after 9.5`: send normal API headers (token) so you get a per-user bucket, debounce ≥500 ms, honour 429.
- **Send**: plain message with `content = gif.url` (page URL; the server unfurls it into a `gifv` embed; needs EMBED_LINKS). Tenor-era behaviour; for Klipy URLs UNCONFIRMED.
- Favourites: server copy only in proto type 2 `favorite_gifs`; keep local for MVP.

**Checklist:** [ ] trending categories grid [ ] debounced search, masonry of `src` GIFs (cached, animated via the existing `image` gif decoder) [ ] click sends `url` [ ] local favourites [ ] 429 handling

## 1.9 Message context menu and hover actions

| Action | Request | Gate |
|---|---|---|
| Reply | composer state; send `message_reference{message_id,channel_id,guild_id,type:0}` + `allowed_mentions{parse:[users,roles,everyone],replied_user}` (P1 §4.2) | SEND_MESSAGES + READ_MESSAGE_HISTORY; not system messages |
| Edit | `PATCH /channels/{c}/messages/{m} {"content"}`; ↑ in empty composer edits the last own message | author; types 0/19 |
| Delete | `DELETE …/messages/{m}` → 204; confirm dialog, Shift skips it | author or MANAGE_MESSAGES |
| Pin/Unpin | §1.4 | PIN_MESSAGES |
| Add reaction | `PUT …/reactions/{emoji}/@me?type=0` (P1 §4.5) | §1.6 rules |
| View reactions | `GET …/reactions/{emoji}?type=0&limit=100[&after=<user id>]` → partial users; `type=1` burst | READ_MESSAGE_HISTORY |
| Copy text / link / ID | link `https://discord.com/channels/{guild_id or @me}/{channel_id}/{message_id}`; ID only with `user_settings.developer_mode` | — |
| Mark unread | `POST /channels/{c}/messages/{prev_id}/ack {"manual":true,"mention_count":N}` | — |
| Forward | `POST /channels/{dest}/messages {"nonce","message_reference":{"type":1,"message_id":M,"channel_id":C,"guild_id"?:G}}` | VIEW_CHANNEL source, SEND_MESSAGES dest |
| Suppress embeds | `PATCH …/messages/{m} {"flags": old \| 4}` (keep other bits) | author or MANAGE_MESSAGES; only with embeds |
| Create thread | §2.1 | CREATE_PUBLIC_THREADS |

- Link parser: `https://(?:(?:ptb|canary)\.)?discord(?:app)?\.com/channels/(\d+|@me)/(\d+)(?:/(\d+))?`.
- Mark unread: `prev_id` need not exist but must be a valid snowflake → previous loaded id or `clicked_id - 1`. Compute `mention_count` from loaded messages at/after the clicked one that mention you (server doesn't). Locally set `read_state.last_message_id = prev_id` and pause auto-ack for that channel until the user leaves or sends (UNCONFIRMED official rule). Other sessions get `MESSAGE_ACK` with `manual:true`.
- Forward: only DEFAULT/REPLY messages without poll/call/components. Result has flag HAS_SNAPSHOT (1<<14) and `message_snapshots:[{message:{content, timestamp, edited_timestamp, mentions, mention_roles, attachments, embeds, type, flags, components?, sticker_items?}}]` with **no author** → render as a "Forwarded" quote linking to `message_reference`. Optional `forward_only:{attachment_ids?, embed_indices?}`. A typed comment goes as a **separate** message (DPS; same-request content UNCONFIRMED). One POST per destination.
- Ephemeral (`flags & 64`): Copy + Dismiss (local). Failed/pending local messages: Retry + Delete (local).

**Checklist:** [ ] hover toolbar (§4.6) [ ] right-click menu with gates [ ] delete confirm + Shift bypass [ ] reaction list popout [ ] mark unread with auto-ack pause [x] forward picker (one destination, no text sent along) + snapshot rendering [ ] suppress embeds [ ] developer-mode Copy ID

## 1.10 "New messages" bar, Jump to present, jump to message

- `GET /channels/{c}/messages?around={m}&limit=50` returns both sides of `m` (split ~half, UNCONFIRMED); `before`/`after`/`around` are exclusive.
- **On open** (UNCONFIRMED details): snapshot `divider = read_state.last_message_id` (don't move it while acking); unread if `channel.last_message_id > divider`. Load `?limit=50`; draw the red **NEW** divider before the first `id > divider` (only if a message ≤ divider is loaded). Top bar **"N new messages since {time}"** ("50+" if the oldest loaded is still > divider; time = first unread's timestamp or snowflake time of `divider+1`) with **Mark As Read** (ack latest, keep divider). Clicking the bar scrolls to the first unread, or loads `?after={divider}&limit=50` and enters detached mode.
- **Detached** = loaded window lacks `channel.last_message_id` (after jumping to a reply/pin/search hit/link or deep scroll). Show "You're viewing older messages — **Jump To Present**"; MESSAGE_CREATE updates state but is not appended; bottom scroll loads `?after={newest}&limit=50` and re-attaches when a page has <50 or contains the latest. Jump to present = drop window, `?limit=50`, re-attach.
- **Jump to message:** loaded → scroll + highlight ~2 s; else `around` → verify presence → detached unless it includes the latest; missing → toast "Message not found".
- Existing auto-ack (focused, at bottom, debounced) stays; remote `MESSAGE_ACK` can clear the bar.

**Checklist:** [ ] per-channel `{window, detached, divider, highlight}` [ ] NEW divider + bar + Mark As Read [ ] detached bar + Jump To Present [ ] jump-to-message used by reply previews, pins, search, links

## 1.11 Stickers and polls (gap not covered by the drafts)

- **Send sticker:** message with `"sticker_ids":["<id>"]` (max 3; content optional). Usable: current guild's stickers, standard packs; other guilds' need Nitro (UNCONFIRMED exact rule). Data: guild `stickers[]` in READY/GUILD_CREATE, `GUILD_STICKERS_UPDATE {guild_id, stickers[]}` (full replace), standard packs `GET /sticker-packs`. Render via CDN (P-notes A6); Lottie (`format_type` 3) cannot be rendered by egui → show name placeholder.
- **Vote:** `PUT /channels/{c}/polls/{m}/answers/@me {"answer_ids":[1]}` (empty array clears) → 204; events `MESSAGE_POLL_VOTE_ADD/REMOVE {user_id, channel_id, message_id, guild_id?, answer_id}`. Voters: `GET /channels/{c}/polls/{m}/answers/{answer_id}?after=&limit=`. End early (author): `POST /channels/{c}/polls/{m}/expire`. Source: https://docs.discord.food/resources/message (poll endpoints).
- **Create poll:** message with `"poll":{"question":{"text"},"answers":[{"poll_media":{"text","emoji"?}}],"duration":24,"allow_multiselect":false,"layout_type":1}` (`duration` in hours, default 1, max 32 days; question ≤300 chars, answer ≤55 chars, up to 10 answers; UD message.mdx poll create object). Needs SEND_POLLS (1<<49).

**Checklist:** [ ] sticker tab in picker + `sticker_ids` send [ ] poll rendering with results bar, vote/unvote, voter list [ ] create-poll dialog

## 1.12 Apps: slash commands, buttons, selects, modals (gap not covered by the drafts)

Web parity includes using bots. Sources: https://docs.discord.food/interactions/application-commands#endpoints and /interactions/receiving-and-responding#create-interaction; UD gateway-events "Interactions".

- **Command list:** `GET /guilds/{g}/application-command-index` (guild channels), `GET /channels/{c}/application-command-index` (DMs/group DMs), `GET /users/@me/application-command-index` (user-installed apps) → `{applications[], application_commands[], version}`. Fetch when the user types `/` (cache per guild by `version`).
- **Run:** `POST /interactions` (JSON; files via cloud upload `attachments[].uploaded_filename`) → 204:
  ```json
  {"type":2,"application_id":"<app>","guild_id":"<g>","channel_id":"<c>","session_id":"<gateway session_id>","nonce":"<snowflake>",
   "data":{"id":"<cmd id>","type":1,"name":"ping","version":"<cmd version>","options":[{"type":3,"name":"text","value":"hi"}]}}
  ```
  - `type`: 2 APPLICATION_COMMAND, 3 MESSAGE_COMPONENT, 4 AUTOCOMPLETE, 5 MODAL_SUBMIT.
  - Button / select click: `type 3`, `message_id`, `message_flags`, `data:{"component_type":2,"custom_id":"…"}` (selects add `"values":[…]`).
  - Context-menu commands: `data.type` 2 (user) / 3 (message) with `target_id`.
- **Events:** `INTERACTION_CREATE {id, nonce?}` → `INTERACTION_SUCCESS {id, nonce}` or `INTERACTION_FAILURE {id, nonce?, reason_code}` (2 = timeout); `APPLICATION_COMMAND_AUTOCOMPLETE_RESPONSE {choices, nonce}`; `INTERACTION_MODAL_CREATE {id, channel_id, custom_id, application, title, components, nonce}` → show the modal; submit with `type 5`, `data:{"id":"<ID of the interaction that triggered the modal>","custom_id","components":[…values…]}` (component value shape: UD receiving-and-responding "Modal Submit Component Data").
- Show "Bot is thinking…" between CREATE and the bot's MESSAGE_CREATE; on FAILURE show "The application did not respond".
- Ephemeral replies arrive as MESSAGE_CREATE with `flags & 64` to your session only.

**Checklist:** [ ] `/` popup from command index [ ] option entry (string/int/bool/user/channel/role/attachment) [ ] POST /interactions with nonce + session_id [ ] buttons/selects on messages [ ] modal renderer [ ] success/failure states

## 1.13 Voice messages (receive)

Messages with flag IS_VOICE_MESSAGE (1<<13) carry one `.ogg` Opus attachment with `duration_secs` and base64 `waveform`. Rendering the waveform needs nothing new; **playback needs an Opus decoder + audio output** (opus-rs + cpal from VOICE_NOTES phase A) plus an Ogg page parser (~60 lines, no crate). Sending voice messages (record → Opus → cloud upload with `duration_secs`/`waveform`, flag 8192) depends on the same stack. Schedule after voice phase A.

---

# Part 2. Threads, forums, friends, DMs, inbox

Sources: UD `topics/threads`, `resources/channel`, `resources/relationships`, `resources/user`, `resources/notification-center`, `topics/read-state`, `gateway/gateway-events`, `topics/permissions`; DPS `http.py`/`state.py`/`tracking.py`; ABD `discord.cpp`, `channellisttree.cpp`.

## 2.1 Threads

**Model.** Channel types 10 NEWS_THREAD, 11 PUBLIC_THREAD, 12 PRIVATE_THREAD (private only under GUILD_TEXT; news parents create 10). `owner_id` = creator, `parent_id` = text/news/forum/media. Extra fields: `member_count` (approx., caps at 50), `message_count` (excludes starter and deleted), `total_message_sent` (never decremented), `member_ids_preview?`, `applied_tags?`, `member?` (your thread member on some endpoints/events), `thread_metadata {archived, auto_archive_duration 60|1440|4320|10080, archive_timestamp, locked, invitable? (private), create_timestamp? (after 2022-01-09)}`. `nsfw` inherited from the parent.
Thread member `{id?, user_id?, join_timestamp, flags, muted?, mute_config?, member?}` — `id`/`user_id` omitted inside READY/GUILD_CREATE `threads[]`; `muted`/`mute_config` only for you; flags 1 HAS_INTERACTED, 2 ALL_MESSAGES, 4 ONLY_MENTIONS, 8 NO_MESSAGES.

**Permissions.** Parent permissions apply, but sending uses **SEND_MESSAGES_IN_THREADS (1<<38)**; CREATE_PUBLIC_THREADS 1<<35, CREATE_PRIVATE_THREADS 1<<36, MANAGE_THREADS 1<<34. VIEW_CHANNEL on the parent is always needed. Private threads visible only to members or MANAGE_THREADS. Unarchive: any member unless locked (then MANAGE_THREADS). `name`/`archived`/`auto_archive_duration`: creator or MANAGE_THREADS. `locked`, `rate_limit_per_user`: MANAGE_THREADS (creator may set `locked:true`). Delete: MANAGE_THREADS. Archived = read-only (50083). Sending auto-unarchives (unless locked) and auto-joins. Parent `rate_limit_per_user` also limits thread creation.

**Gateway**
- READY `guilds[].threads[]`: only **joined active** threads, each with `member`; set `guild_id` yourself (B1).
- THREAD_LIST_SYNC (B9): after op 37 `threads:true` (already sent). Without `channel_ids` → replace the guild's whole set (DPS deletes threads missing from it). DPS also parses optional `most_recent_messages[]` (UNCONFIRMED in UD).
- THREAD_CREATE: channel + `newly_created?` + `member?`; also sent when you are added to any thread and to moderators → **upsert**; `newly_created:false` = joined/gained access.
- THREAD_UPDATE: full channel; not sent for `last_message_id` (track from MESSAGE_CREATE). `archived` → true: drop from the active set. Unarchive sends THREAD_UPDATE then THREAD_MEMBER_UPDATE to every member.
- THREAD_DELETE `{id, guild_id, parent_id, type}`; also delete its local read state.
- THREAD_MEMBER_UPDATE: your thread member + `guild_id` ("you are a member").
- THREAD_MEMBERS_UPDATE `{id, guild_id, member_count, added_members?, removed_member_ids?}`; for users expect only your own changes (INFERENCE). Joined = your id in `added_members`; left = in `removed_member_ids`.
- Losing access to the parent sends **no** THREAD_DELETE → recompute VIEW_CHANNEL on the parent before showing or acting.
- Thread member sidebar: op 37 `thread_member_lists:[thread_id]` → THREAD_MEMBER_LIST_UPDATE `{guild_id, thread_id, members:[{user_id, member, presence}]}` (REST `/thread-members` list is not usable by users).
- Messages in unjoined threads arrive only while the guild is subscribed with `threads:true`.
- On MESSAGE_CREATE/DELETE(_BULK) in a thread update `last_message_id`, `message_count`, `total_message_sent` locally.

**REST**
| Action | Request | Notes |
|---|---|---|
| Active threads | — | `GET /guilds/{g}/threads/active` and `GET /channels/{c}/threads/active` are **not usable by users**. Use THREAD_LIST_SYNC or thread search with `archived=false`. |
| Archived public | `GET /channels/{c}/threads/archived/public?before=<ISO8601>&limit=2..100` | `{threads, members, has_more}`, `archive_timestamp` desc; READ_MESSAGE_HISTORY; next `before` = last `archive_timestamp` |
| Archived private (all) | `GET /channels/{c}/threads/archived/private` | + MANAGE_THREADS |
| Archived private joined | `GET /channels/{c}/users/@me/threads/archived/private?before=<snowflake>&limit=` | ID desc; READ_MESSAGE_HISTORY only (ABD uses it with archived/public) |
| Search | `GET /channels/{c}/threads/search` | §2.2; works for text channels too |
| Get | `GET /channels/{thread}` | includes your `member` if joined |
| Create from message | `POST /channels/{parent}/messages/{mid}/threads {"name":"1-100","auto_archive_duration":4320,"rate_limit_per_user"?,"location":"Message"}` | thread id = message id (160004 if exists); fires THREAD_CREATE + MESSAGE_UPDATE (parent message gets `thread`, flag HAS_THREAD 1<<5). DPS also sends `"type":11`. |
| Create without message | `POST /channels/{parent}/threads {"name","auto_archive_duration","type":11\|12,"invitable"?,"location":"Plus Button"}` | **`type` defaults to 12 (private) — always send it.** Public posts a type-18 system message in the parent. Then send the first message into the thread. Preselect the parent's `default_auto_archive_duration` in the UI (not auto-copied). |
| Join / leave | `PUT` / `DELETE /channels/{t}/thread-members/@me?location=…` | 204; needs unarchived thread |
| Add / remove member | `PUT` / `DELETE /channels/{t}/thread-members/{uid}` | add: SEND_MESSAGES; remove: MANAGE_THREADS or private-thread creator |
| Notify / mute | `PATCH /channels/{t}/thread-members/@me/settings {"flags"?:2\|4\|8,"muted"?,"mute_config"?:{end_time,selected_time_window}}` | thread member or 204; fires THREAD_MEMBER_UPDATE |
| Rename/archive/lock/tags | `PATCH /channels/{t} {"name"?,"archived"?,"locked"?,"auto_archive_duration"?,"applied_tags"?}` | ABD "Archive" sends `{archived:true, locked:true}` |
| Delete | `DELETE /channels/{t}` | MANAGE_THREADS |

**Starter message:** thread from a message → first message is type 21 THREAD_STARTER_MESSAGE with only `message_reference` + `referenced_message`. Forum post → first message id = thread id. Fetch with `?around={id}&limit=1` or forum `first_messages` / `post-data`.

**Sidebar (partly confirmed):** under each text/news/forum channel show **joined, non-archived** threads (READY + ABD). UD: `auto_archive_duration` "is now repurposed to control how long the thread stays in the channel list" → hide when `now - max(snowflake_time(last_message_id), archive_timestamp) > auto_archive_duration` (formula INFERENCE). Viewing an unjoined thread adds a temporary row under the parent, removed on leave if still unjoined (ABD). Order: `last_message_id` desc (UNCONFIRMED). Thread browser ("Threads" header button): Active (joined + other active from sync) and Archived (public + joined private).

**Read state / notifications:** same CHANNEL read state; ack `flags` = **3** (IS_GUILD_CHANNEL|IS_THREAD) for types 10/11/12 (UD read-state). Badges only for joined threads (safe default, UNCONFIRMED). Level = thread member flags 2/4/8, else inherit the parent's effective level (UNCONFIRMED); `muted` lives on the thread member, not in `user_guild_settings`.

**Checklist**
- [ ] `threads: HashMap<id, Channel>` + `joined` from READY/THREAD_* events; drop archived
- [ ] nested joined threads under parents (spine lines, §4.2), temporary row for unjoined
- [ ] thread view = message view with bit 38 send gate and ack flags 3
- [ ] join/leave, create from message, create via "+", archive/lock/rename
- [ ] thread browser: active + archived paging
- [ ] thread notification/mute menu
- [ ] split-view thread panel (§4.9)

## 2.2 Forum (15) and media (16) channels

**Channel fields:** `topic` = guidelines; `available_tags[] {id, name≤50, moderated, emoji_id?, emoji_name?}` (≤20); `default_reaction_emoji {emoji_id?, emoji_name?}`; `default_sort_order` (null/0 LATEST_ACTIVITY, 1 CREATION_TIME); `default_forum_layout` (0 default, 1 list, 2 grid; media always gallery); `default_tag_setting` (`match_some`|`match_all`); `default_thread_rate_limit_per_user`; `rate_limit_per_user` (limits post creation); `template?`; flags REQUIRE_TAG 1<<4, HIDE_MEDIA_DOWNLOAD_OPTIONS 1<<15; per post PINNED 1<<1 (sorts first, never auto-archives). `last_message_id` = **newest post id** → update on THREAD_CREATE (no CHANNEL_UPDATE). Posts are PUBLIC_THREADs with `applied_tags` (≤5); moderated tags need MANAGE_THREADS.

**List posts: `GET /channels/{forum}/threads/search`**
| param | value |
|---|---|
| `name?` | ≤100 chars, `slop?` (2) |
| `tag?` | repeat per tag id, ≤20 |
| `tag_setting?` | `match_some` (default) / `match_all` |
| `archived?` | true / false / omit for both |
| `sort_by?` | `last_message_time` (default) / `archive_time` / `relevance` / `creation_time` |
| `sort_order?` | `desc` (default) / `asc` |
| `limit` / `offset` | 1–25 / ≤9975 |
| `max_id` / `min_id` | paging alternative with `creation_time` |

→ `{threads[], members[] (yours), has_more, total_results, first_messages?[]}`; READ_MESSAGE_HISTORY. **202** code 110000 → retry after `retry_after`. Map `first_messages` by `message.id == thread.id`. UI: "Recent activity" = `last_message_time`, "Date posted" = `creation_time`.
Alternative for active posts: THREAD_LIST_SYNC + `POST /channels/{forum}/post-data {"thread_ids":[…]}` → `{"threads":{"<id>":{"owner":?member,"first_message":?message}}}` (≤25 ids per call to be safe, UNCONFIRMED max). Which sequence the web client uses is UNCONFIRMED. **Minimal: search only, `archived` omitted, `offset += 25` while `has_more`.**

**Create post: `POST /channels/{forum}/threads`** (DPS adds `?use_nested_fields=true`)
```json
{"name":"…","auto_archive_duration":4320,"applied_tags":["<tag>"],
 "message":{"content":"…","allowed_mentions"?:{…},"sticker_ids"?:[],"flags"?:0,
            "attachments"?:[{"id":"0","filename":"a.png","uploaded_filename":"<uuid>/a.png"}]}}
```
- Attachments: cloud upload first (§1.1, step 1 against the **forum** channel id — INFERENCE; §0.1 #3). Multipart (`payload_json` + `files[n]`, ≤100 MiB) is the documented alternative but needs reqwest `multipart`.
- At least one of content / stickers / files. `embeds`/`components` not allowed for users. Needs **SEND_MESSAGES on the forum** (CREATE_PUBLIC_THREADS ignored); type is always 11. REQUIRE_TAG → ≥1 tag.
- Response: channel + `message`. Events: THREAD_CREATE `newly_created:true`, then MESSAGE_CREATE. Read-state automation: your own post sets the parent's `last_message_id` = thread id and `mention_count` 0.
- `POST /channels/{forum}/messages` → 40058.

**Card contents (UNCONFIRMED, from the UI):** title, tag chips (names/emoji from `available_tags`), author (post-data `owner` or first message author), 2-line first-message snippet (markdown stripped), first image as thumbnail (right in list, large in grid/media), reply count = `message_count`, relative last activity (snowflake time of `last_message_id`), reaction button (`default_reaction_emoji` or top reaction of the first message), pin icon, "NEW"/unread count for joined posts, "Follow" = join. Header: Guidelines, Sort & View, tag filter, search (`name`), New Post.
**Unread:** parent unread = `forum.last_message_id > read_state.last_message_id`. Gateway op 28 "Request Forum Unreads" payload undocumented → skip. Acking the forum when viewing the list: UNCONFIRMED.

**Checklist:** [ ] forum view with list/grid cards (§4.9 sizes) [ ] sort/tag/search params [ ] 202 handling [ ] New Post dialog (name, tags with REQUIRE_TAG and moderated rules, body, cloud attachments) [ ] open post as thread view [ ] media channel = grid only

## 2.3 Friends and relationships

**Object:** `{id (= user id), type, user (partial) | user_id (READY dedupe), nickname?, is_spam_request?, stranger_request?, user_ignored, since?, note? (request note ≤120)}`. Types: 0 NONE, 1 FRIEND, 2 BLOCKED, 3 INCOMING, 4 OUTGOING, 5 IMPLICIT (never listed), 6 SUGGESTION (deprecated). Limits: 1000 friends, 5000 blocked.

**Gateway:** READY `relationships[]` (resolve `user_id` from `users[]`). RELATIONSHIP_ADD = full object + `should_notify?`; **every type change arrives as ADD** → upsert. RELATIONSHIP_UPDATE = partial `{id, type, nickname, since?, stranger_request?, user_ignored}` → merge. RELATIONSHIP_REMOVE → delete. Badge: ADD type 3 → +1; ADD type 1 or REMOVE type 3 → −1 (≥0).

**REST** (all 204; fire RELATIONSHIP_*; send the matching `X-Context-Properties`, §0.3)
| UI action | Request |
|---|---|
| Add Friend tab | `POST /users/@me/relationships {"username":"name","discriminator":null}` (+ optional `"note"` ≤120; legacy tags: discriminator string) |
| Add by id (profile/menu) / accept incoming | `PUT /users/@me/relationships/{id} {}` (type −1 = accept or create). On **80013** ask "accept stranger?" and resend `{"confirm_stranger_request":true}` |
| Decline / cancel / remove / unblock | `DELETE /users/@me/relationships/{id}` |
| Block | `PUT /users/@me/relationships/{id} {"type":2}` (removes friendship) |
| Ignore / unignore | `PUT` / `DELETE /users/@me/relationships/{id}/ignore` |
| Friend nickname | `PATCH /users/@me/relationships/{id} {"nickname"}` |
| Clear incoming / spam requests | `DELETE /users/@me/relationships?relationship_type=3` (body `{"filters":[1]}` for spam only) |
| Mutual friends | `GET /users/{id}/relationships` → partial users |

**Abuse:** UD warns that many friend requests in a short time "may be flagged … or lead to immediate account termination". Only on an explicit click; never bulk, queue or auto-retry; captcha → hand-off.

**Friends page tabs:** Online (type 1 with user-level status ≠ offline; invisible looks offline), All (type 1), Pending (3 + 4 with Accept/Ignore/Cancel; `is_spam_request` in a separate Spam section), Blocked (type 2 + `user_ignored`), Add Friend. Sort by friend nickname > `global_name` > `username`, case-insensitive (UNCONFIRMED). Friends badge = incoming count (spam excluded? UNCONFIRMED). Presence: `merged_presences.friends[]` then PRESENCE_UPDATE without `guild_id`.

**Checklist:** [ ] relationships map + ADD/UPDATE/REMOVE [ ] friend presence map [ ] 4 tabs + Add Friend + Spam [ ] accept/decline/cancel/remove/block/ignore/nickname [ ] 80000–80013 messages [ ] pending badge on Home

## 2.4 DMs, group DMs, message requests

- **List:** READY `private_channels` (`recipient_ids` with dedupe, A9). Group fields: `name?`, `icon?`, `owner_id`, `nicks?[{id,nick}]`, `managed?`, `blocked_user_warning_dismissed?`. Safety: `is_message_request`, `is_message_request_timestamp`, `is_spam`, `safety_warnings[]`. Order: B13.
- **Open DM:** use the cached type-1 channel with that recipient; otherwise `POST /users/@me/channels {"recipients":["<id>"]}` + `X-Context-Properties: e30=` (B5; 10 new DMs per 10 min counted at first message).
- **Group DM:** `POST /users/@me/channels {"recipients":[…]}` + "New Group DM" header; max 10 incl. you (30004/30011); UI offers friends only.
- **Add member:** `PUT /channels/{gdm}/recipients/{uid}` (body `{"nick"?}` or none) + "Add Friends to DM" → 204 + CHANNEL_RECIPIENT_ADD. **On a 1:1 DM this creates a new group** (returns it; CHANNEL_CREATE has `origin_channel_id`).
- **Remove (owner):** `DELETE /channels/{gdm}/recipients/{uid}` → CHANNEL_RECIPIENT_REMOVE. If it names you, drop the channel.
- **Rename / icon / transfer:** `PATCH /channels/{gdm}` `{"name"}` / `{"icon":"data:image/png;base64,…"|null}` / `{"owner":"<uid>"}` → CHANNEL_UPDATE. Icon CDN `https://cdn.discordapp.com/channel-icons/{channel_id}/{icon}.png` (UD's `/channels/{id}/icons/` form UNCONFIRMED); no default → placeholder.
- **Leave group:** `DELETE /channels/{gdm}` (`?silent=true` skips the system message). **Close 1:1 DM:** `DELETE /channels/{dm}` (hides; history kept; re-open returns the same channel). Both fire CHANNEL_DELETE (partial; use `id`).
- **Message requests** (`is_message_request:true`; `is_spam:true` → Spam): accept `PUT /channels/{id}/recipients/@me {"consent_status":2}`; reject `DELETE /channels/{id}/recipients/@me`; batch reject `PUT /channels/recipients/@me/batch-reject {channel_ids≤50}`; previews `GET /users/@me/message-requests/supplemental-data?channel_ids=…` (1–25) → `[{channel_id, message_preview}]`; badge = read state type 5 (newest request id vs `last_acked_id`). Whether replying auto-accepts: UNCONFIRMED.
- Optional: `POST /channels/{id}/blocked-user-warning-dismissal`; `POST /channels/{id}/safety-warnings/ack {warning_ids}`.

**Checklist:** [ ] DM list rows (§4.3) with close × [ ] cached-first open DM [ ] New Group DM picker [ ] group header menu (rename, icon, add, remove, leave, transfer) [ ] Message Requests entry with accept/reject/spam [ ] CHANNEL_RECIPIENT_* handling

## 2.5 Inbox (Mentions, Unreads, For You)

- **Mentions:** `GET /users/@me/mentions?limit=25&roles=true&everyone=true[&guild_id=<g>][&before=<msg_id>]` (limit 1–100; past 7 days; newest first). Remove one: `DELETE /users/@me/mentions/{message_id}` → 204 + RECENT_MENTION_DELETE `{message_id}`. Filters "this server only / @everyone / @roles" → `guild_id` / `everyone` / `roles`. Header "#channel – Server" via the channel cache.
- **Unreads:** computed locally from read states (channels with `last_message_id > ack`, minus muted, newest first). Mark all read: `POST /read-states/ack-bulk {"read_states":[{"channel_id","message_id":<last_message_id>,"read_state_type":0}…]}` → 204; chunk ≤100 (cap UNCONFIRMED); never move markers backwards.
- **For You** (optional; desktop presence UNCONFIRMED): `GET /users/@me/notification-center/items?limit=1..100&with_mentions=false&roles_filter=true&everyone_filter=true&after=<id>` → `{items[], cursor, has_more, limit}`; item `{id, type ("friend_request_accepted", "friend_request_pending", "recent_mention", "reply_mention", "go_live_push", "reaction_sent", …), body, acked, deeplink, other_user?, message?, guild_id?, message_channel_id?, completed}`. Ack `POST …/items/{id}/ack`; bulk `POST …/items/bulk-ack?item_ids=…`; delete `DELETE …/items/{id} {"item_type":"mention"|"regular"}`; badge = read state type 2, ack `POST /users/@me/2/{newest_item_id}/ack {}`. Events NOTIFICATION_CENTER_ITEM_CREATE / _ITEM_DELETE `{id,user_id}` / _ITEMS_ACK `{id}` / _ITEM_COMPLETED.

**Checklist:** [ ] Inbox popout (Ctrl+I) with Mentions (REST + ×) [ ] Unreads (local + ack-bulk) [ ] (later) For You

## 2.6 User profiles and notes

- `GET /users/{id}/profile?with_mutual_guilds=true&with_mutual_friends=true&with_mutual_friends_count=false[&guild_id=<g>]&type=popout` (base in P1 §4.7; DPS sends lowercase strings; exact web flags UNCONFIRMED). Additions: `mutual_friends?`, `mutual_friends_count?`, `guild_member_profile?` (guild `pronouns`, `bio`, `banner`, `theme_colors`), `guild_badges[]`, `premium_guild_since`, `user_profile.profile_effect?`, `widgets?`, `private?` (redacts bio, pronouns, badges, connections, premium dates).
- Display: guild profile field over user field when non-empty. Banner: member banner → user banner → `accent_color` → avatar dominant colour. `theme_colors=[primary, accent]` (0xRRGGBB). `bio` ≤190, `pronouns` ≤40. Badges `https://cdn.discordapp.com/badge-icons/{icon}.png` (+ optional `link`). `connected_accounts[] {type, id, name, verified, metadata?}` (profile URLs built client-side, UNCONFIRMED). `mutual_guilds[{id, nick}]`.
- **Access:** 403 unless shared guild / friend / suggestion / they requested you → fetch **only on click**, cache per (user, guild) for minutes, never for strangers from the inbox.
- **Notes** (READY has none with bit 0): `GET /users/@me/notes/{id}` → `{note, note_user_id, user_id}`, **404 = no note**; bulk `GET /users/@me/notes` → `{user_id: note}`. Set `PUT /users/@me/notes/{id} {"note":"…"}` (≤256; `""` clears; ≤1500 notes) → 204 + USER_NOTE_UPDATE `{id, note}`. Save on blur (UNCONFIRMED).

**Checklist:** [ ] 300 px popout (§4.8) from avatar/name anywhere [ ] cached profile GET [ ] mutual servers/friends tabs in the full modal [ ] note box PUT on blur [ ] "Message @user" box opens/creates the DM

---

# Part 3. Account and server

Sources: UD `resources/user-settings`, `user-settings-proto`, `user`, `guild`, `channel`, `invite`, `emoji`, `guild-template`, `audit-log`, `topics/permissions`, `rate-limits`, `captcha-handling`, `authentication#mfa-verification`; proto https://github.com/discord-userdoccers/discord-protos (`PreloadedUserSettings.proto`); DPS `settings.py`, `client.py`, `http.py`, `invite.py`; ABD `discord.cpp`. Prerequisite: §0.2 (MFA 401, extra headers).

## 3.1 User settings

### 3.1.1 Two stores
| | Legacy JSON (**use this**) | Protobuf (official web client) |
|---|---|---|
| READY | `user_settings` (present with caps 253; omitted only with bit 9) | `user_settings_proto` base64 (present regardless, §0.1 #6) |
| Read | `GET /users/@me/settings` | `GET /users/@me/settings-proto/1` → `{"settings":"<b64>"}` |
| Write | `PATCH /users/@me/settings` with **only changed keys** (server merges) → full object | `PATCH /users/@me/settings-proto/1 {"settings":"<b64>","required_data_version"?}` → `{"settings","out_of_date"?}` |
| Dispatch | `USER_SETTINGS_UPDATE` partial JSON → merge per key (not sent with bit 9) | `USER_SETTINGS_PROTO_UPDATE {"settings":{"type":1,"proto":"<b64>"},"partial":bool}` |
| Danger | none | **whole top-level sub-message must be sent** or its unsent subfields reset |

Both PATCHes fire both events (DPS "changes are reflected in both"). Proto writes are "ratelimited heavily": infrequent → immediate; frequent → batch 10 s; automated → 30 s. With `out_of_date:true` the edit was discarded → re-GET, re-apply, re-PATCH.

Legacy JSON bodies:
```json
PATCH /users/@me/settings {"status":"dnd"}
PATCH /users/@me/settings {"custom_status":{"text":"hi","emoji_name":"❤️","emoji_id":null,"expires_at":"2026-11-12T00:00:00.000Z"}}
PATCH /users/@me/settings {"custom_status":null}
PATCH /users/@me/settings {"guild_folders":[{"id":2183563457,"name":"Work","color":16711680,"guild_ids":["1","2"]},
                                            {"id":null,"name":null,"color":null,"guild_ids":["3"]}]}
PATCH /users/@me/settings {"theme":"dark"}   // dark|light|darker|midnight; also locale, message_display_compact, developer_mode, convert_emoticons …
```
`custom_status.text` ≤128; folder `id`/`color` are JSON integers (`Option<i64>`), `guild_ids` strings; `id:null` + one guild = positioned guild, not a folder.

### 3.1.2 Status ↔ presence sync
See §1.3: op 3 first, then PATCH; on USER_SETTINGS_UPDATE re-send op 3 keeping non-custom activities; auto-idle never writes settings; op 3 never on a timer.

### 3.1.3 Proto field numbers (plan B only)
```
PreloadedUserSettings
  1  versions        {1 client_version u32, 2 server_version u32, 3 data_version u32}
  6  text_and_images {17 message_display_compact, 14 animate_emoji, 11 gif_auto_play, 12 render_embeds, 13 render_reactions,
                      9 inline_attachment_media, 10 inline_embed_media, 4 render_spoilers StringValue, 21 convert_emoticons,
                      41 display_compact_avatars … (BoolValue unless noted)}
  2 inbox, 3 guilds, 4 user_content, 5 voice_and_video, 7 notifications, 8 privacy, 9 debug, 10 game_library
 11  status          {1 status StringValue, 2 custom_status CustomStatus, 3 show_current_game BoolValue,
                      4 status_expires_at_ms fixed64, 5 status_created_at_ms UInt64Value}
       CustomStatus  {1 text string, 2 emoji_id fixed64, 3 emoji_name string, 4 expires_at_ms fixed64, 5 created_at_ms fixed64, 6 label StringValue}
 12  localization    {1 locale StringValue, 2 timezone_offset Int32Value (min), 3 timezone_name StringValue}
 13  appearance      {1 theme enum(0 UNSET,1 DARK,2 LIGHT,3 DARKER,4 MIDNIGHT), 2 developer_mode bool, 9 timestamp_hour_cycle enum(0 AUTO,1 H12,2 H23), 12 ui_density …}
 14  guild_folders   {1 folders repeated GuildFolder, 2 guild_positions repeated fixed64 (deprecated)}
       GuildFolder   {1 guild_ids repeated fixed64, 2 id Int64Value, 3 name StringValue, 4 color UInt64Value}
```
Wrapper types are sub-messages with the value in field 1. **Hand-write the codec (~120 lines), no `prost`:** prost drops unknown fields, and re-encoding a decoded sub-message would reset fields Rustcord doesn't model. Raw model `Vec<(u32, Wire)>`, `enum Wire { Varint(u64), I64(u64), Len(Vec<u8>), I32(u32) }`, groups → error; `repeated fixed64` must accept packed (wt 2) and unpacked (wt 1). Edit = take the current field-11 bytes, drop fields 1/2, push new ones, re-encode, send `{11: …}` only. `USER_SETTINGS_PROTO_UPDATE partial:true` → replace each top-level field present (UNCONFIRMED; DPS uses `MergeFrom`); `partial:false` → replace all.

Test vectors (re-verified):
```
{11:{1:StringValue("dnd")}}                                    5a 07 0a 05 0a 03 64 6e 64  "WgcKBQoDZG5k"
{11:{1:"online", 2:{1:"hi",3:"❤"}}}                            "WhUKCAoGb25saW5lEgkKAmhpGgPinaQ="
{14:{1:[{1:[1,2] packed,2:Int64Value(123),3:"A",4:UInt64Value(0xFF0000)},{1:[3]}]}}
                                                                "cjAKIgoQAQAAAAAAAAACAAAAAAAAABICCHsaAwoBQSIFCICA/AcKCgoIAwAAAAAAAAA="
```

**Checklist:** [ ] JSON settings model with merge on USER_SETTINGS_UPDATE [ ] status/custom status (§1.3) [ ] theme/compact/developer mode/convert_emoticons read + write [ ] (plan B) proto codec with the vectors as unit tests

## 3.2 Own account and profile

**`PATCH /users/@me`** → user object **+ `token`**; fires USER_UPDATE; may return 401/60003.
| field | notes |
|---|---|
| `username` | 2–32, unique; **requires `password`**; needs a live gateway session. Change rate ~2/h (UNCONFIRMED); show the server's 50035 message (`discord_message()` already extracts it) |
| `global_name` | 1–32 or `null` |
| `avatar` | data URI or `null`; animated needs Nitro; `avatar_description?` ("{filename}, added {date}") |
| `password` / `new_password` | invalidates all tokens → **store the returned `token`** in the keyring whenever non-empty |
| `bio`, `pronouns`, `accent_color`, `banner` | accepted here too (banner = Nitro) |
- Username availability while typing: `POST /users/@me/pomelo-attempt {"username"}` → `{"taken"}`; debounce ≥500 ms (official cadence UNCONFIRMED).
- Recent avatars: `GET /users/@me/avatars` (6 `{id, storage_hash, description}`) → reuse with `PATCH /users/@me {"avatar_id"}`.
- **`PATCH /users/@me/profile`** (fires USER_UPDATE): `pronouns` ≤40, `bio` ≤190, `accent_color`; Nitro: `banner`, `theme_colors`, `profile_effect_id`. Per-guild: `PATCH /guilds/{id}/profile/@me` (same fields) and `PATCH /guilds/{id}/members/@me` (nick, guild avatar). Which endpoint the web "Profiles" page uses per field is UNCONFIRMED → name/avatar via `/users/@me`, bio/pronouns/accent via `/users/@me/profile`.
- **Images:** `data:image/png;base64,…` (jpeg/gif/webp too; MIME must match). Size limits undocumented (~8–10 MB reported, UNCONFIRMED) → crop square, resize with the existing `image` crate to 512² (avatar) / 1024×409 (banner), encode PNG; send GIF untouched only with `premium_type > 0`. Errors 50045/50046.

**Checklist:** [ ] My Account page (display name, username with availability + password + MFA, password change with token swap) [ ] Profiles page (avatar crop/upload, bio, pronouns, accent colour) [ ] per-server profile (nick) [ ] MFA prompt dialog (§0.2)

## 3.3 Guild lifecycle

General: `X-Audit-Log-Reason` on audited routes; **act on gateway dispatches**, not REST responses; gate locally on permissions, role hierarchy (sort roles by `(position, id)`; you can only manage roles/members below your highest role and grant permissions you have; the owner bypasses), and **guild `mfa_level==1`**: KICK, BAN, ADMINISTRATOR, MANAGE_CHANNELS/GUILD/MESSAGES/ROLES/WEBHOOKS/EXPRESSIONS/THREADS count as absent unless `user.mfa_enabled` (exact error UNCONFIRMED).

- **Create:** `POST /guilds {"name":"My Server","icon":null|"data:…","channels":[],"system_channel_id":null,"guild_template_code":"2TffvPucqHkN"}` (DPS; "Blank Server" template) → GUILD_CREATE. Name 2–100. From template: `POST /guilds/templates/{code} {"name","icon"?}`; preview `GET /guilds/templates/{code}`. 30001 = cap 100/200.
- **Leave:** `DELETE /users/@me/guilds/{id} {"lurking":false}` → 204, GUILD_DELETE (no `unavailable`), GUILD_MEMBER_REMOVE. Hide for the owner.
- **Delete (owner):** `DELETE /guilds/{id}` (MFA possible; DPS uses legacy `POST /guilds/{id}/delete`).
- **Modify:** `PATCH /guilds/{id}` (MANAGE_GUILD): `name` 2–100, `icon`, `banner`/`splash` (feature-gated), `description` ≤300, `afk_channel_id`, `afk_timeout` ∈ {60,300,900,1800,3600}, `verification_level`, `default_message_notifications` (0 all, 1 mentions), `explicit_content_filter`, `system_channel_id`, `system_channel_flags`, `preferred_locale`, `premium_progress_bar_enabled`, `owner_id` (transfer; plus `code` from `PUT /guilds/{id}/pincode` when the owner has email but no 2FA). 20031 = disallowed words.

## 3.4 Channels

- **Create:** `POST /guilds/{id}/channels` (MANAGE_CHANNELS) `{"name","type":0|2|4|5|13|15,"parent_id"?,"topic"?,"nsfw"?,"rate_limit_per_user"?,"bitrate"?,"user_limit"?,"position"?,"permission_overwrites"?:[{"id","type":0|1,"allow":"0","deny":"1024"}]}`. "Private" toggle = deny VIEW_CHANNEL (1024) for `@everyone` (id = guild id) + allow entries (exact client payload UNCONFIRMED). Limits 500 channels / 50 per category (30013); bitrate cap by boost 96k/128k/256k/384k (30052).
- **Edit:** `PATCH /channels/{id}` (`name`, `topic` ≤4096, `nsfw`, `rate_limit_per_user` ≤21600, `parent_id`, `bitrate`, `user_limit` ≤99, `permission_overwrites` full replace with MANAGE_ROLES). Name/topic edits reportedly 2 per 10 min per channel (bot reports; user limit UNCONFIRMED) → show the `retry_after` countdown, never auto-retry.
- **Delete:** `DELETE /channels/{id}` (confirm); category deletion un-parents children (CHANNEL_UPDATE each); community rules/updates channels → 50074.
- **Reorder:** `PATCH /guilds/{id}/channels [{"id","position","parent_id"?,"lock_permissions"?}]` → 204 + CHANNEL_UPDATEs. Send the whole affected sorting bucket with contiguous positions; text/news/forum sort together, voice/stage together and below; `lock_permissions:true` = "Sync permissions".
- **Overwrites:** `PUT /channels/{id}/permissions/{target} {"type":0|1,"allow","deny"}` / `DELETE …` → 204 + CHANNEL_UPDATE (MANAGE_ROLES; only bits you have).

## 3.5 Roles

- Create `POST /guilds/{id}/roles {"name"?,"permissions"?,"colors"?:{"primary_color":int,"secondary_color":null,"tertiary_color":null},"hoist"?,"mentionable"?,"icon"?,"unicode_emoji"?}` (`color` deprecated; gradients need ENHANCED_ROLE_COLORS; max 250 → 30005) → GUILD_ROLE_CREATE.
- Edit `PATCH /guilds/{id}/roles/{rid}` → GUILD_ROLE_UPDATE; delete `DELETE …` → GUILD_ROLE_DELETE `{guild_id, role_id}`.
- Reorder `PATCH /guilds/{id}/roles [{"id","position"}]` (only below your top role; @everyone = 0).
- Member roles: `PUT`/`DELETE /guilds/{id}/members/{uid}/roles/{rid}` → 204, or `PATCH /guilds/{id}/members/{uid} {"roles":[…full…]}` → GUILD_MEMBER_UPDATE (delivered to users only for subscribed members and own actions).

## 3.6 Members and moderation

- Own nick `PATCH /guilds/{id}/members/@me {"nick"|null}` (CHANGE_NICKNAME; not the deprecated `/members/@me/nick`).
- Others `PATCH /guilds/{id}/members/{uid}`: `nick` (MANAGE_NICKNAMES), `roles` (MANAGE_ROLES), `communication_disabled_until` ISO8601 ≤ now+28 d or null (MODERATE_MEMBERS; **timeout**; admins exempt), `mute`/`deaf`/`channel_id` (voice).
- Kick `DELETE /guilds/{id}/members/{uid}` (KICK_MEMBERS). Ban `PUT /guilds/{id}/bans/{uid} {"delete_message_seconds":0..604800}` → GUILD_BAN_ADD; presets 0/3600/21600/43200/86400/259200/604800 (UNCONFIRMED list). Unban `DELETE …/bans/{uid}`; list `GET /guilds/{id}/bans?limit≤1000&after=<uid>`. Bulk ban: skip.
- Reason textbox on kick/ban/timeout → `X-Audit-Log-Reason`.

## 3.7 Guild emoji upload

`POST /guilds/{id}/emojis {"name":"[A-Za-z0-9_]{2,32}","image":"data:image/png|gif;base64,…","roles":[]}` (CREATE_EXPRESSIONS 1<<43 or MANAGE_EXPRESSIONS 1<<30); **≤256 KiB** (downscale to 128×128 with `image`); slots 50/100/150/250 by tier (static and animated separately), errors 30008/30018. Rename/role-lock `PATCH …/emojis/{eid} {"name"?,"roles"?}`; delete `DELETE …`. GUILD_EMOJIS_UPDATE replaces the list. Per-guild special limits with inaccurate headers → serialize uploads per guild, wait out 429.

## 3.8 Invites

- **Create** `POST /channels/{id}/invites {"max_age":604800,"max_uses":0,"temporary":false,"unique"?,"flags":0,"target_type":null}` (CREATE_INSTANT_INVITE; `max_age` 0–5184000, `max_uses` 0–100; group DM only `max_age` 1–604800; default modal values 7 days / unlimited UNCONFIRMED). **204 = invites paused** for the guild. 30016 = 1000 max.
- **Resolve** (invite embeds) `GET /invites/{code}?with_counts=true&with_expiration=true` → `guild{…}`, `channel{id,name,type}`, `inviter`, `approximate_member_count`, `approximate_presence_count`, `expires_at`, `type` (0 guild, 1 group DM, 2 friend), `flags`. 404/10006 = invalid. Cache per code; resolve only on screen. Regex `(?:https?://)?(?:www\.)?(?:discord(?:app)?\.com/invite|discord\.gg)/([A-Za-z0-9-]+)`.
- **Join** `POST /invites/{code} {"session_id":"<gateway session_id>"}` + `X-Context-Properties` "Join Guild"/"Accept Invite Page"/"Invite Button Embed" (§0.3). Only on click + confirm dialog. **A captcha (400 + `captcha_key`, possibly "You need to update your app to join this server.") is a likely outcome for third-party clients** → show "Join this server in the official app: https://discord.com/invite/CODE" (optionally open via the existing `links` feature); never retry. Success → invite + `new_member`, `show_verification_form`; build state from GUILD_CREATE. Rules screening (`pending`) → "complete in the official app". 40007 banned; 30001 guild cap. UD: joining many guilds quickly "may be flagged … or lead to immediate account termination".
- **Delete** `DELETE /invites/{code}`; **list** `GET /guilds/{id}/invites` (MANAGE_GUILD or VIEW_AUDIT_LOG) / `GET /channels/{id}/invites` (MANAGE_CHANNELS). INVITE_CREATE/DELETE are **not delivered to users** → refresh lists from REST.

## 3.9 Server bar order and folders

- Order = `guild_folders` (JSON) or proto 14.1. `id:null` single-guild entries are plain positions; real folders: `id` (i64), `name` (null → client joins guild names with ", "), `color` (RGB int), `guild_ids`. Guilds in no folder (just joined) render first (ABD; whether the server inserts them: UNCONFIRMED). Skip ids you are not in.
- Edit (drag, create, rename, recolor): rewrite the **whole** list incl. single entries → `PATCH /users/@me/settings {"guild_folders":[…]}`; new folder id = random unused u32 (official generator UNCONFIRMED); debounce drags 1–2 s. Expand/collapse state local (UNCONFIRMED).

## 3.10 Notification settings and mute

`PATCH /users/@me/guilds/{guild_id}/settings` (or `/users/@me/guilds/@me/settings` for DMs), partial body → full entry + USER_GUILD_SETTINGS_UPDATE (replace).
```json
{"muted":true,"mute_config":{"selected_time_window":3600,"end_time":"2026-11-11T23:00:00.000Z"}}
{"muted":true,"mute_config":{"selected_time_window":-1,"end_time":null}}
{"muted":false}
{"message_notifications":1}   // 0 all, 1 mentions, 2 nothing (3 = inherit, overrides only)
{"suppress_everyone":true,"suppress_roles":true,"mute_scheduled_events":true,"mobile_push":false,"hide_muted_channels":true,"notify_highlights":1,"flags":<bits>}
{"channel_overrides":{"<channel_id>":{"muted":true,"mute_config":{…},"message_notifications":3,"collapsed":false}}}
```
- `channel_overrides` in the PATCH is a **map keyed by channel id** (READ shape is an array); send only changed keys.
- **Category collapse syncs across clients:** `{"channel_overrides":{"<category_id>":{"collapsed":true}}}`.
- Presets (UNCONFIRMED): 900, 3600, 10800, 28800, 86400, −1. Bulk across guilds: `PATCH /users/@me/guilds/settings {"guilds":{"<gid>":{…}}}`. Coalesce rapid toggles ~500 ms per guild.

## 3.11 Mark server read

`POST /read-states/ack-bulk` with every channel/thread of the guild where `last_message_id > ack` (`message_id > 0`, chunks ≤100, never backwards). Deprecated `POST /guilds/{id}/ack` still exists. Shortcut Shift+Esc.

## 3.12 Rate limits and abuse sensitivity

| Action | Limit / risk | Handling |
|---|---|---|
| Any request | 50 req/s per user; 10k 401/403/429 per 10 min per IP → 24 h Cloudflare ban (`shared` 429 exempt) | existing limiter; local gating |
| 401 code 60003 | not a dead token | §0.2 |
| `POST /invites/{code}` | captcha likely; mass joins → flags/termination | click + confirm; hand-off on captcha |
| friend requests, DMs to strangers, login | documented captcha endpoints | detect, never solve |
| `POST /guilds` | cap 100/200 | one in flight |
| username / avatar changes | ~2/h, "too fast" errors (UNCONFIRMED) | show server text |
| `PATCH /users/@me/settings-proto/1` | heavy | status immediate; debounce folder drags |
| channel name/topic | ~2/10 min/channel (bots) | countdown |
| emoji routes | per-guild special | serialize |
| 20028/20029 | write rate limit | surface + back off |
| fingerprint | stale `X-Super-Properties` / UA mismatch raises scrutiny (P1 §2) | keep current |
| `USER_REQUIRED_ACTION_UPDATE {required_action}` / 40002 | account gated | banner "finish in the official app", block sends |

## 3.13 Gateway events touched (merge rules in PROTOCOL_NOTES)
GUILD_CREATE/UPDATE/DELETE, CHANNEL_CREATE/UPDATE/DELETE, GUILD_ROLE_*, GUILD_MEMBER_UPDATE/REMOVE, GUILD_BAN_ADD/REMOVE, GUILD_EMOJIS_UPDATE, GUILD_STICKERS_UPDATE, USER_UPDATE, USER_SETTINGS_UPDATE (+ PROTO), USER_GUILD_SETTINGS_UPDATE, USER_NOTE_UPDATE, USER_REQUIRED_ACTION_UPDATE. INVITE_* never arrives.

**Checklist (Part 3)**
- [ ] §0.2 MFA + headers + route_key first
- [ ] server dropdown/context menu (§4.2, §4.11) gated per permission/hierarchy/mfa_level
- [ ] create/join server dialog (Ctrl+Shift+N) with captcha hand-off; leave/delete with confirm
- [ ] server settings layer: Overview, Roles (create/edit/reorder/assign), Emoji, Members (nick/timeout/kick/ban/roles), Invites, Bans; audit reason field
- [ ] channel create/edit/delete/reorder/overwrites; category collapse via `channel_overrides`
- [ ] invite create modal + invite embeds (cached resolve)
- [ ] folders (render + drag + edit) via `guild_folders`
- [ ] notification menus (guild, channel, DM) incl. timed mute
- [ ] Mark As Read (guild) via ack-bulk

---

# Part 4. Layout reference (web client, desktop width, late 2026)

Numbers come from the production BUNDLE (CSS chunks + `web.f25377cf5b911919.js`, fetched 2026-11-11) at **default density, 100 % zoom, 1 rem = 16 px** — VERIFIED unless marked UNCONFIRMED. Class names are `name_HASH` and change per build; match on values. Colours are given by role only: Rustcord keeps its own palette/branding (ember theme, `docs/ARCHITECTURE.md`). The live layout is the **2025-03-25 "refresh"** (smaller squircle icons, floating user panel, resizable sidebar, title bar); the 2025-12-08 patch redesigned User Settings. The user's account may be in experiments such as `refresh-fast-follow-avatars` (44 px icons). Bundle copies: `<local scratch, not in repo>` (`all.css`).

## 4.0 Skeleton and tokens

```
grid-template-areas: "titleBar titleBar titleBar" / "guildsList notice notice" / "guildsList channelsList page"
columns: min-content | min-content | 1fr        rows: 32px | min-content | 1fr
```
| Element | Size |
|---|---|
| Title bar | 32 px (variants 24 / 40) |
| Server bar | 72 px = 40 icon + 2×16 |
| Sidebar total (server bar + channel list) | default **375** → channel list 302 px; resizable **264–432**; 4 px handle on the channel list's right edge; ←/→ 10 px, Home/End = min/max; persist in config |
| Channel list chrome | 1 px left+top border, top-left radius 12 |
| Member list | 264 px (compact 256, cozy 268), 1 px left border |
| Channel header & sidebar header | 49 px incl. 1 px bottom border (aligned) |
| Min window | 800 × 500 |

Density (`.density-compact | default | cozy`): `--space-xs/sm/md/lg/xl/xxl` = 6/10/12/16/20/26 | **8/12/16/20/24/32** | 10/16/20/24/30/40; channel name line-height 20 | 24 | 28; space before a category 8 | 16 | 16. Radii xs 4, sm 8, md 12, lg 16, xl 24, xxl 32. Icons 12/16/18/24/32. Control heights 32/40. Modals 442 / 602 / 800 wide, padding 24×16.
**Minimal:** egui panels with exact widths (72 / 302 / 264 / 32); all spacing through `Theme` density tokens so a density switch is one table.

## 4.1 Server bar
- Width 72; lowest frame colour; scroller padding 4 top / 24 bottom; bottom 16 px fade; stops above the floating user panel.
- Order: Home (DMs) → unread DM avatars with red count (UNCONFIRMED order) → separator 32×1 px radius 1 → guilds/folders → Add a Server (+) → Discover. Item gap 8 (derived).
- Icon 40×40 **squircle** (no hover morph in the refresh; the circle→square morph is pre-2025). egui: `CornerRadius` ≈ 12. Acronym icons: medium text on a subtle background; hover/selected → brand background, white text, 0.15 s. Round buttons 40×40, 20 px glyph. Press = 1 px down.
- **Pill:** 8 px wide at x −4 → **4 px visible**, radius `0 4 4 0`, near-white; height **8** unread / **20** hover / **40** selected; 200 ms ease-out (DM variant 10/24).
- Badge bottom-right: 16 px high, min-width 16, radius 8, bold 12 px, ~4 px cut-out ring (sizes UNCONFIRMED). Tooltip with the server name on hover; right-click menu §4.11.
- Folders: closed 48×48 radius 16, folder colour at 60 % transparency, 2×2 mini icons 19 px with 2 px gap; open = folder glyph tile 40×40 radius 12 plus children inside a 48 px rounded (16) background at 85 % transparency; drop guild on guild creates a folder (green outline).
- "NEW MENTIONS" sticky bar at top/bottom when mentions are scrolled out.
**Minimal:** 72×48 rows, icon at x 16, pill at x 0 with `ctx.animate_value_with_time(id, h, 0.2)`, badge with background ring, `on_hover_text`, `context_menu`, `dnd_*` for reorder; folders later.

## 4.2 Channel sidebar
- **Header** 49 px: server-name dropdown pill (32 high, radius 8, chevron) + 32×32 invite button. Banner servers get a taller header. Dropdown items (UNCONFIRMED order): Invite People, Server Settings ▸, Create Channel, Create Category, Create Event, App Directory | Notification Settings ▸, Privacy Settings | Edit Per-server Profile, Hide Muted Channels ☐ | **Leave Server** (red), Copy Server ID (dev mode); each only with permission.
- **Menus:** radius 8, 1 px border, raised surface, 8 px padding, min-width ~188; items min-height 32, padding 8, radius 2; separators 1 px with 8 px margin; danger items red.
- **Category:** 16 px space above; row 24 px, padding 16 left / 8 right; name in case given (no uppercase), 12 px chevron rotating −90° in 0.1 s; "+" on hover; muted = opacity 0.5. Collapsed keeps the selected channel and occupied voice channels visible (UNCONFIRMED). Collapse syncs via §3.10.
- **Channel row:** margin-left 8, 1 px top/bottom → **34 px pitch**; link radius 8, padding 4×8, icon 20, gap 8 → **32 px row**.
  | state | name | icon | background | weight |
  |---|---|---|---|---|
  | normal | dim channel colour | channel-icon | — | regular |
  | hover | hover text | — | hover tint | regular |
  | unread | strong | subtle | — | 500–600 |
  | selected | strong | strong | selected tint | 500–600 |
  | muted | muted, opacity 0.5 | muted | — | regular |
  Unread pill 4×8, radius `0 4 4 0`, ~1 px from the list's left edge. Mention badge right. Hover icons (Create Invite, Edit Channel, 16 px, gap 4) replace the badge; always on the selected row. Status/subtitle second line indented 28 px.
- **Voice users** under voice channels: 32 px rows, 24 px avatar (large: 38/30), gap 8, radius 8; right icons muted/deafened/LIVE/video (gap 4); speaking = green ring; indent ~36 (UNCONFIRMED); connected channel = green 2 px left bar.
- **Threads:** left margin 36, curved spine lines (x offset 24 at default density), own unread pill; only joined active threads (§2.1).

## 4.3 DM home sidebar
- Top bar 49 px: input-looking button "Find or start a conversation" (padding 10×12, radius 8, 14 px muted) → quick switcher filtered to DMs (UNCONFIRMED).
- Rows: **Friends** (pending badge), Message Requests when present (Nitro/Shop: skip). Same row component as channels.
- "Direct Messages" header 48 px with "+" (Create DM/group, Ctrl+Shift+T).
- DM row: **42 px** (+1 px padding), margin-left 8, inner padding 8; **32 px avatar** with status dot (10 px, 3 px ring), gap 12; name + subtext (activity/custom status/member count, −2 px overlap); close × 16 px at ~0.7 opacity on hover/focus only.
- Friends page: tabs Online / All / Pending / Blocked / Add Friend; "Active Now" column (UNCONFIRMED).

## 4.4 User panel and voice panel
- Floating container: bottom 8, left 8, width = sidebar total − 16 (spans server bar + channel list), 1 px border, radius 8. Stack: activity → voice connection → account.
- **Account row** 56 px, padding 12, gap 8: clickable area with **32 px avatar** + dot, name 14 px medium (line 22), second line custom status / username on hover / status; right: **Mute**, **Deafen**, **Settings** (gap 8; red with slash when active; device chevrons UNCONFIRMED). Click avatar → account popout (status picker §1.3).
- **Voice panel** (in a call): padding 12, gap 12; row 1 (32 px) green "Voice Connected" + signal bars, "Channel / Server" link, noise-suppression + disconnect icons; row 2 equal-width buttons (32 px, radius 8, 20 px icon, gap 8): Camera, Screen share, Activities, Soundboard; active = green tint. See VOICE_NOTES.

## 4.5 Title bar and chat header
- **Title bar** 32 px: centred location icon + name; right (gap 12, padding 12): **Inbox** (Ctrl+I), Help; window controls on desktop. Inbox/Help moved here from the channel header in the refresh.
- **Chat header** 49 px, padding 8 (left 16): type icon 20–24, name 16 px semibold, 1×24 divider (4 px margins), topic 14 px muted one line with right fade (click → topic modal). Threads/forum posts: breadcrumb parent › thread (10 px caret).
- Toolbar (gap 8, 32×32 buttons, 20 px icons): **Threads**, **Notification bell** (All / Only @mentions / Nothing / Mute ▸), **Pins** (Ctrl+P), **Member list toggle** (Ctrl+U), **Search** box (32 px, radius 8, width `clamp(64px, 100cqi − 320px, 244px)`). DMs: call, video, pins, add friends, profile toggle. Icon badge: 8 px dot top-right (red important, brand for unread pins).

## 4.6 Message list
- **Cozy group:** row padding 2 px; content left offset **72** (16 + 40 + 16); right padding 24; group start margin-top **17 px** (1.0625 rem, user-settable), min-height 44. Avatar 40 px circle at x 16, top +2 (click → popout). Header line 22 px: name 16 px medium in top role colour (underline on hover), role icon 20, BOT/APP tag, timestamp 12 px muted, 4 px left margin ("Today at 14:03"; tooltip full date). Follow-ups: no header; hover shows short time in the 56 px gutter, right-aligned. Content 16 px / line 22. "(edited)" 10 px muted (UNCONFIRMED size). Grouping: same author within 7 min, no reply, same day (UNCONFIRMED threshold).
- **Row backgrounds:** hover tint (right corners 4); mention-of-me warm tint + **2 px warning bar**; reply target brand tint + 2 px bar; jumped-to highlight flashes then fades; ephemeral brand tint; AutoMod red bar.
- **Reply reference:** 18 px line, 14 px text, 4 px below; 16 px avatar, coloured name (@ if pinged), one-line preview; curved spine from x ≈ 36; click → jump (§1.10); "Original message was deleted" italic.
- **Hover toolbar:** top −25 px, right 0, padding 0 14 0 32; pill radius 8, 1 px border, 2 px padding, raised bg + low shadow; buttons 24×24 (20 icon), radius 6; 1×24 separators. Order (UNCONFIRMED): 3 quick reactions, Add Reaction, Reply / Edit (own), Forward, Create Thread, More. **Shift** reveals Delete-without-confirm, Pin, Mark Unread, Copy ID (dev).
- **Context menu** (UNCONFIRMED order): reaction row, Add Reaction ▸, View Reactions | Edit (own), Reply, Forward, Create Thread, Copy Text, Apps ▸ | Pin, Mark Unread (Alt+Enter), Copy Message Link, Speak Message | **Delete** (red), Report | Copy Message ID (dev).
- **System messages:** 72 px offset, 16 px glyph in the gutter, muted text, bold clickable names, min-height 22.
- **Dividers:** NEW = 1 px notification-red line inset 16 / 14, red "NEW" tag (13 px high, bold 10 px uppercase, right radius 4) above the first unread. Date = 1 px subtle line, centred 12 px semibold label (padding 2×4, radius 8), 24 px above / 8 below; combined when they coincide.
- **Bars:** new-messages bar 32 px, brand bg, white text, bottom radius 8, padding 16, "N new messages since …" + **Mark As Read**; detached bar 28 px above the composer "You're viewing older messages" + **Jump To Present** (newer builds: round 36 px button bottom-right, 16 px inset).
- **Reactions:** padding-top 2, wrap, 4 px gaps; pill radius 8, 1 px border, padding ~2×6, emoji 20, medium count; "me" = brand tint/border/text; add-reaction ghost at row end on hover; tooltip "X, Y and 3 others reacted with :e:".
- **Embeds:** max width **516**; radius 4, raised bg, 1 px border, **4 px left colour bar**; padding ~2/16/12/16; provider, author (24 px icon), title (16 semibold, link colour with URL), description 14 px, fields (≤3 inline), image (≤400×300) or 80×80 thumbnail, footer (20 px icon, 12 px). Suppress × on hover (§1.9).
- **Attachments:** images/video radius 8, max ~550×350, mosaic gap 4 (UNCONFIRMED max); files card ~432 px (icon, name link, size, download); spoilers blurred with "SPOILER" pill.
- **Lightbox:** dark scrim; width `round(min(100vw−110, 177.78vh−551, 1900))`, min 500; Open in Browser, download; ←/→ steps; Esc/scrim closes.
**Minimal:** reversed virtualised `ScrollArea` (`stick_to_bottom`), full-row hover rect, toolbar as an `Area` at `row.right_top() + (−14, −25)`.

## 4.7 Composer
- Form: horizontal padding 8, margin-top −16 (overlaps an 8 px fade), bottom margin 8.
- Box: 1 px border, radius 8, input bg; inner min-height **56** growing to **50 vh** then scrolling; "+" attach button left (Upload a File, Create Thread, Create Poll, Use Apps; inner padding 15); text 16/22 with 17 px vertical padding; placeholder "Message #channel" / "Message @user"; right buttons (row 56, gap 8, 20 px icons): GIF, Stickers, Emoji (Gift: skip; Apps and voice-message buttons are newer).
- Character counter near the limit (2000; 4000 Nitro): 12 px monospace, 14 right / 12 bottom, red when over; box grows 32 px. Error = red 2 px ring.
- Above the box: **reply bar** "Replying to **Name**" with **@ ON/OFF** (`allowed_mentions.replied_user`) and ⓧ; stacked bars square the composer's top corners; Esc cancels. **Typing line** 24 px, top −24, 16 px left padding, three dots + 12 px text with bold names. Slowmode clock + countdown right-aligned on that line (UNCONFIRMED placement); send disabled while > 0. **Upload previews** strip inside the box (Spoiler, Edit, Remove per card).
- **Autocomplete popup:** directly above the composer, same width, raised bg, radius 8, ≤10 rows + header; triggers `@`, `#`, `:` (≥2 chars), `/`; ↑/↓, Tab/Enter, Esc.
**Minimal:** multiline `TextEdit` (desired rows 1, `max_height(0.5 × screen)`) in an 8 px rounded frame; 24 px typing strip; reply bar `Frame`; autocomplete `Area` anchored at the composer's `left_top` growing upward.

## 4.8 Member list and popouts
- **Member list:** 264 px; group headers padding 20 top / 4 bottom / 16 left ("Moderators — 3", "Online — 120", "Offline — 900"; small semibold muted); rows **42 px** (+1), margin-left 8, padding 8, radius 8, **32 px avatar** + dot (mobile glyph), gap 12, name 16 px medium in role colour (crown, BOT tag, role icon), subtext 12 px activity/custom status; offline = avatar opacity **0.3** + muted name (full on hover); click → popout left of the list.
- **Profile popout:** **300 px**, radius 8, max height 100vh−20; banner **105 px**; avatar **80 px** (dot 16, ring 6, offset 4) overlapping the banner 16 px from the left; custom-status bubble at left 105, radius 16, ≤2 lines; body padding 16: display name 20 px bold, username + pronouns, badges, About Me, Member Since, role pills with colour dots, Note, "Message @user" input. Flips to stay in the window. Full modal banner 210 (v2: 140 banner, 120 avatar).
- **Status-dot sizes** (BUNDLE): avatar 16 → dot 6 / ring 2; 24 → 8/3; 32 → 10/3; 40 → 12/4; 80 → 16/6 (offset 4); 120 → 24/8 (offset 8).

## 4.9 Layers, pickers and panels
- **Settings (user and server):** full-screen layer; left region ~30 % right-aligned with a 264 px sidebar column (192 content, 60 px vertical padding), section headings ("USER SETTINGS", "APP SETTINGS", …; casing after the Dec-2025 redesign UNCONFIRMED), 32 px rows radius 4–8, red Log Out at the bottom; right region ~70 % with a 460–740 px content column (padding 60/40/80); 60 px tools column with round × and "ESC" under it, 60 px from the top. Server settings: Overview, Roles, Emoji, Stickers, Members, Invites, Bans, AutoMod, Audit Log, Integrations, …, Delete Server (red). The newer "sidenav" (40 px padding, max 800) + settings search exist; the classic layout is acceptable.
- **Expression picker:** anchored above the composer's right edge (24 px in); **486 px** tall (396 in emoji-only reaction mode), ~498 wide; raised bg, 1 px border, radius 8, high shadow. Tabs **GIFs | Stickers | Emoji** at 16 px from the top (row 30, 14 px semibold, padding 8×12, radius 8). Emoji tab: search (focused on open) + skin-tone selector; 48 px left rail (Frequently Used, 32 px server icons with 8 px margin, 8 unicode category glyphs 24 px); grid cells **40** (48 large), sticky section headers with collapse; inspector footer ~48 px with large preview + `:name:` + source server; click inserts, Shift-click keeps open; locked emoji greyed. GIF tab: category tiles 2 columns (Favorites, Trending, words over darkened previews), results masonry ~160 px columns, star toggle on hover. Sticker tab: 96 px (80 small) cells, 32 px rail.
- **Pins popout:** **420 px**, max 80 vh, min 200, radius 8, under the pin button right-aligned; "Pinned Messages" header; cozy cards with hover Jump (24 px, radius 3) and × unpin.
- **Inbox popout:** For You / Unreads / Mentions; grouped by channel with mark-read checks; min height 320; width ~480–600 (UNCONFIRMED).
- **Search results:** right panel **418 px** replacing the member list (1 px left border); header with count, Sort (New/Old/Relevant), tabs Messages/Media/Files/Pins; grouped by channel; cozy cards with hover Jump; pager at the bottom.
- **Thread panel:** right-docked chat with 1 px borders, top-left radius 12, own header (name, mute, settings, ×) and composer; 8 px resize handle on its left; floats with `0 8px 40px` shadow on narrow windows.
- **Forum:** top "New Post" bar + Sort & View + tag pills; list cards radius 12, 1 px border, padding 12×10, hover raised bg + stronger border + low shadow; grid = media tiles (columns UNCONFIRMED); a post opens as the thread panel.
- **Quick switcher** (Ctrl+K): centred modal radius 8, padding 12×20, ~570 wide (UNCONFIRMED); input 70 px tall, 22 px font; results scroller 236 px ("Unread Channels", recents); prefixes `@` users, `#` text, `!` voice, `*` servers; protip footer (10 px padding, 1 px top border).
- Generic modal: 442/602/800, padding 24×16, radius 8–12, dark scrim, Esc closes.

## 4.10 Keyboard shortcuts (bind strings from the BUNDLE; `mod` = Ctrl)

| Keys | Action |
|---|---|
| Ctrl+K (also Ctrl+T) | Quick switcher |
| Ctrl+/ | Shortcut help |
| Alt+↑/↓ · Alt+Shift+↑/↓ · Ctrl+Shift+Alt+↑/↓ | prev/next channel · unread channel · channel with mention |
| Ctrl+Alt+↑/↓ (also Ctrl+Shift+[ / ], Ctrl+Shift+Tab) | prev/next server (mapping UNCONFIRMED) |
| Ctrl+Alt+←/→ | back / forward (mapping UNCONFIRMED) |
| Esc · Shift+Esc | mark channel read (after closing layers / cancelling reply-edit) · mark server read |
| Shift+PageUp · PageUp/PageDown | oldest unread · scroll |
| ↑ in empty composer | edit last message |
| any printable key | focus composer |
| Ctrl+E / Ctrl+G / Ctrl+S | emoji / GIF / sticker picker |
| Ctrl+I · Ctrl+P · Ctrl+U · Ctrl+F | inbox · pins · member list · search |
| Ctrl+Shift+U · Ctrl+Shift+N · Ctrl+Shift+T · Ctrl+Shift+Alt+N · Ctrl+Shift+L | upload · create/join server · create DM group · create thread · copy channel link |
| Ctrl+Shift+H, F1 | help |
| Ctrl+Shift+M / Ctrl+Shift+D · Ctrl+Enter | toggle mute / deafen · accept call |
| Shift+F10 | context menu of the focused item |
| Ctrl+Plus / Minus / 0 | zoom |
| focused message: e, r, p, f, +, Backspace, Alt+Enter, Ctrl+C | edit, reply, pin, forward, react, delete, mark unread, copy (mapping UNCONFIRMED) |

egui: `ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::ALT, Key::ArrowUp)))` when no text field has focus; Esc and Ctrl+K work everywhere.

## 4.11 Server icon context menu (UNCONFIRMED order)
Mark As Read | Invite to Server | Mute Server ▸ (15 min / 1 h / 3 h / 8 h / 24 h / Until I turn it back on), Notification Settings ▸, Hide Muted Channels | Server Settings ▸, Privacy Settings, Edit Per-server Profile | Create Channel, Create Category, Create Event | **Leave Server** (red) | Copy Server ID (dev mode).

**Layout checklist**
- [ ] skeleton widths/heights + resizable sidebar (264–432, persisted)
- [ ] density tokens in `Theme`
- [ ] server bar pills/badges/squircles; folders
- [ ] channel rows/categories/voice users/threads
- [ ] floating user panel + voice panel
- [ ] title bar (Inbox, Help) + chat header toolbar
- [ ] message list geometry, hover toolbar, dividers, bars, reactions, embeds, attachments, lightbox
- [ ] composer with reply/typing/upload strips and autocomplete
- [ ] member list, profile popout, settings layer, pickers, pins/inbox/search/thread/forum/quick switcher
- [ ] shortcut table

---

# Part 5. Still UNCONFIRMED (a live capture of the web client would settle these)

- Typing resend interval; whether typing is sent while editing.
- `/attachments/{upload_filename}` with raw `/`; chunked GCS PUT acceptance; boost-tier limits after the 20 MiB change; cloud uploads on forum-post creation.
- Discord vs gemoji shortcode differences; Nitro Basic emoji rights; external sticker rules.
- Official op 8 `limit`/debounce; whether `query` matches `global_name`; whether user `query` requests are rate-limited.
- Klipy `url` unfurling as `gifv`.
- Mark-unread target id and the auto-ack pause rule; new-messages bar wording and time anchor; grouping threshold (7 min).
- Forward + content in one request (Rustcord sends none).
- Forwarding a message that is itself a forward (Rustcord hides the action).
- Search GET vs `/search/tabs` in today's client; forum listing sequence (search vs sync + post-data); op 28 payload; forum ack on view.
- Profile popout query flags; Inbox Unreads loading; ack-bulk chunk cap; group DM icon path variant.
- Thread order in the sidebar; thread notification inheritance.
- Which endpoint the Profiles page uses per field; username/avatar change limits; image size limits.
- Status expiry ownership; mute presets; invite defaults; ban-history presets; partial proto merge semantics.
- Context-menu orders, a few font sizes, settings/inbox/quick-switcher widths.

Live capture was not possible (no connected Chrome extension; the built-in browser is logged out). Any capture must stay read-only on the user's account (open views, read network logs), never send, join, ring or edit.

## Sources
- UD: https://docs.discord.food/topics/cloud-uploads , /resources/message , /resources/channel , /resources/user , /resources/user-settings , /resources/user-settings-proto , /resources/relationships , /resources/notification-center , /resources/guild , /resources/invite , /resources/emoji , /resources/guild-template , /resources/presence , /resources/integration#search-gifs , /topics/threads , /topics/read-state , /topics/permissions , /topics/rate-limits , /topics/captcha-handling , /authentication#mfa-verification , /gateway/gateway-events , /interactions/application-commands , /interactions/receiving-and-responding (raw: https://github.com/discord-userdoccers/discord-userdoccers/tree/master/pages)
- Protos: https://github.com/discord-userdoccers/discord-protos
- OFF: https://discord.com/developers/docs/change-log (Default File Upload Limit Increase 2026-09-03; Attachment Editing and is_spoiler; PIN_MESSAGES Feb 2026; Rate Limit for Request Guild Members; Deprecating Non-E2EE Voice Calls), https://discord.com/developers/docs/topics/opcodes-and-status-codes , /reference#uploading-files , /resources/message
- DPS https://github.com/dolfies/discord.py-self ; ABD https://github.com/uowuo/abaddon ; DCO https://github.com/ayn2op/discordo ; NIN https://github.com/diamondburned/ningen ; ERR gist above
- Emoji: https://crates.io/crates/emojis , https://github.com/rossmacarthur/emojis , https://www.npmjs.com/package/emojibase-data
- GIF provider change: https://piunikaweb.com/2026/01/14/discord-gif-search-change-tenor-api-shutdown/ , https://www.shacknews.com/article/149852/google-tenor-api-service-discontinued-twitter-discord-whatsapp-bluesky
- Layout: BUNDLE (CSS chunk list in the layout draft), https://discord.com/blog/discord-patch-notes-december-8-2025 , https://betanews.com/2025/03/25/discord-game-overlay-update-desktop-customization-pc-gaming , https://constrat.github.io/revert-discord/revert.css (pre-2025 values)
- Crate sizes: crates.io API `crate_size`, resolved against the current `Cargo.lock` with `cargo update --workspace` (index only) and `scratchpad\tools\depsize.py`.
