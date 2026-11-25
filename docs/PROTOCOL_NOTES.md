# Discord user-client protocol notes

Compiled by research agents from docs.discord.food (Userdoccers), the official developer docs and open-source clients. Items marked UNCONFIRMED were not verified against a fetched source. A critic pass with corrections is appended at the end (it overrides earlier sections where they conflict).


---

# Part 1: REST API, rate limits, CDN

# Discord user-client REST API: implementation notes for a Rust client (serde_json / reqwest)

Sources I fetched: the userdoccers MDX sources (raw GitHub copies of the docs.discord.food pages), the official docs at docs.discord.com, and the source of two open-source user clients: discord.py-self (`http.py`, `tracking.py`, `abc.py`, `gateway.py`) and Abaddon (`src/discord/discord.cpp`). Anything not confirmed by a fetched source is marked **UNCONFIRMED**.

---

## 1. Base URL and API version
Sources: https://docs.discord.food/reference ; https://docs.discord.com/developers/reference

```
Stable: https://discord.com/api/v9      <- use this
Canary: https://canary.discord.com/api  (or X-Debug-Options: canary)
```

| Version | Status | Default |
|---|---|---|
| 10 | Available | |
| 9 | Available | **default for the client** |
| 8, 7 | Deprecated | |
| 6 | Deprecated | default for the API (when no version is in the path) |
| 5, 4, 3 | Discontinued (400) | |

- **Pick v9.** Userdoccers marks it as the official client's default, and both discord.py-self (`INTERNAL_API_VERSION = 9`) and Abaddon (`https://discord.com/api/v9/...`) use it.
- If you leave the version out of the path, requests route to v6. Always put `/v9` in the path.
- One v9/v10 difference matters here: from v10, a PATCH `attachments` array must list every attachment to keep, including new ones.

---

## 2. Request headers
Sources: https://docs.discord.food/reference#client-properties ; discord.py-self `http.py` (lines ~848–901) and `tracking.py` ; Abaddon `discord.cpp` (lines 1403–1415, 2829–2852)

| Header | Needed? | Value / notes |
|---|---|---|
| `Authorization` | **required** | The raw user token with **no prefix** (`Authorization: <token>`). Bots use `Bot <token>` and OAuth uses `Bearer <token>`. |
| `User-Agent` | strongly recommended | Must equal `browser_user_agent` in the super properties. A malformed or improper UA can trigger a Cloudflare block (`{"message":"internal network error","code":40333}`). |
| `X-Super-Properties` | optional but "highly recommended" (anti-abuse; experiments need a recent `client_build_number`) | Base64 of compact JSON (discord.py-self uses `separators=(',',':')`; standard base64 with padding). Without it, Discord parses some fields from the User-Agent. |
| `X-Discord-Locale` | optional | e.g. `en-US`. Locale resolution order: this header, then `Accept-Language`, then the user settings locale. |
| `X-Discord-Timezone` | optional; **not in userdoccers** | IANA zone name such as `Europe/Berlin`. discord.py-self sends it from `tzlocal.get_localzone_name()`. |
| `X-Debug-Options` | optional | Comma list. Only `canary` and `trace` have documented effects. Abaddon sends `bugReporterEnabled`, which is cosmetic. `trace` also needs `X-Client-Trace-ID`; don't use it. |
| `Referer` | optional | discord.py-self sends `https://discord.com/channels/@me`. Abaddon updates it to the current view: `/channels/@me/{dm_id}` or `/channels/{guild_id}/{channel_id}`. |
| `Origin` | optional | `https://discord.com` (discord.py-self) |
| `Accept-Language`, `Sec-Fetch-*`, `Sec-CH-UA*` | optional | Browser fidelity only (discord.py-self) |
| `Content-Type: application/json` | required on JSON bodies | Use multipart only for file uploads. |
| `X-Captcha-Key`, `X-Captcha-Session-Id`, `X-Captcha-Rqtoken` | only when retrying after a CAPTCHA challenge | See §4.9 |
| `X-Failed-Requests` | optional | Number of earlier 5xx failures, sent on retry |
| `X-Audit-Log-Reason` | not relevant here | |

### Super properties JSON
Source: https://docs.discord.food/reference#example-client-properties-web

No field is strictly required. Field types are not verified by the server. These fields are observed in every official client: `os`, `browser`, `browser_user_agent`, `browser_version`, `release_channel`, `system_locale`, `client_build_number`.

Web example (the shape discord.py-self emits):
```json
{"os":"Windows","browser":"Chrome","device":"","system_locale":"en-US",
 "browser_user_agent":"Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36",
 "browser_version":"136.0.0.0","os_version":"10","referrer":"","referring_domain":"",
 "referrer_current":"","referring_domain_current":"","release_channel":"stable",
 "client_build_number":396858,"client_event_source":null,"has_client_mods":false,
 "client_launch_id":"<uuid4>","client_app_state":"unfocused",
 "client_heartbeat_session_id":"<uuid4, regenerated every 30 min>","launch_signature":"<uuid-like>"}
```

**Must it match the gateway IDENTIFY `properties`?**
- The docs only say the same properties are sent in IDENTIFY and in the header. They don't state that a mismatch is rejected.
- In practice, both libraries derive them from one object:
  - discord.py-self builds IDENTIFY `properties` as the super properties plus identify-only extras (`is_fast_connect:false`, `gateway_connect_reasons:"AppSkeleton"`, `installation_id`).
  - Abaddon builds the header from its IDENTIFY properties.
- **Do the same:** one struct, serialized twice.
- Identify-only fields that never go in the header: `installation_id`, `is_fast_connect`, `version`.
- How to obtain a current `client_build_number` is **UNCONFIRMED**/out of scope. discord.py-self scrapes it from the web app and falls back to `9999`.

### Never send the token to CDN or media hosts
- `cdn.discordapp.com`, `media.discordapp.net` and `images-ext-*.discordapp.net` need no auth.
- Use a **separate `reqwest::Client`** with no default `Authorization`. The API client should only build URLs under `https://discord.com/api/`.
- Never fetch URLs taken from message content or embeds with the API client.
- reqwest drops `Authorization` on cross-host redirects (behavior of reqwest's redirect policy). Don't rely on it.

---

## 3. Rate limits
Source: https://docs.discord.food/topics/rate-limits (plus discord.py-self `http.py` lines 1018–1110 for observed edge cases)

### Headers
**User tokens usually get only `Retry-After`, `X-RateLimit-Global` and `X-RateLimit-Scope`, and only on a 429.** The `X-RateLimit-Limit/Remaining/Reset/Bucket` headers are mainly sent to bots and OAuth clients. Your limiter must work without them, but should use them when they are present.

| Header | Type | Meaning |
|---|---|---|
| `X-RateLimit-Limit` | int | Requests allowed in the window |
| `X-RateLimit-Remaining` | int | Requests left |
| `X-RateLimit-Reset` | float epoch seconds | When the bucket resets (may have decimals, e.g. `1470173023.123`) |
| `X-RateLimit-Reset-After` | float seconds | Time until reset |
| `X-RateLimit-Bucket` | string | Opaque bucket ID, **excluding** major parameters |
| `Retry-After` | seconds (429 only) | Parse as f64 |
| `X-RateLimit-Global` | `true` (429 only, global limit) | |
| `X-RateLimit-Scope` | `user` \| `global` \| `shared` (429 only) | `shared` means a per-resource limit; it does **not** count toward the invalid-request limit. |

### 429 body
```json
{ "message": "You are being rate limited.", "retry_after": 65.0, "global": true }
```
Fields: `message: String`, `retry_after: f64` (seconds), `global: bool`, `code?: i32` (special limits).

Edge cases:
- **Prefer the body's `retry_after` over the header.** The docs' own example has `Retry-After: 1337` with `retry_after: 776`.
- `retry_after` may be an integer in JSON. Deserialize as `f64`; serde_json accepts integers into `f64`.
- **Cloudflare 429s have a non-JSON (HTML/text) body.** If `Retry-After` is present, wait. If it is absent, treat it as a ban and **do not retry programmatically**.
  - discord.py-self tells Cloudflare 429s apart by the missing `Via` header.
  - Cloudflare's `retry_after` is sometimes in milliseconds: detect `header == body/1000`.
- A 429 can arrive while `Remaining > 0`: that is a sub-ratelimit. Just obey `retry_after`.

### Global limit
- **50 requests/second per user** (per IP when unauthenticated), independent of per-route limits.

### Buckets and major parameters
- Limits are per route and may depend on the HTTP method. Several routes can share one bucket.
- Major parameters are `channel_id`, `guild_id`, and `webhook_id` (or `webhook_id` + token). Each value gets an independent limit.
- Key buckets as `"{METHOD} {route_template}:{major_param}"`. Once you learn an `X-RateLimit-Bucket` hash, key on `"{hash}:{major}"` (discord.py-self's scheme).
- Templates should collapse minor IDs and the emoji: `PUT /channels/{c}/messages/{m}/reactions/{emoji}/@me` is keyed on `channel_id` only.

Known sub-limits:
- **Edit message:** messages older than 1 hour can be edited at most **3 times per 6 s per channel**.
- **Delete message:** discord.py-self keeps separate bucket keys for messages under 10 s old and messages 14+ days old, citing official docs issues #1092 and #1295. The values are **UNCONFIRMED**.
- **Reactions:** commonly cited as **1 request per 0.25 s per channel**. This is **UNCONFIRMED**; only third-party sites state it. Serialize reaction PUT/DELETE per channel with about 300 ms spacing and let 429s correct you.
- The rate-limits page's "emoji routes are per-guild and quotas may be inaccurate" warning covers **guild emoji management** (`/guilds/{id}/emojis`), not reactions.

### Invalid-request limit (Cloudflare ban)
- **10,000 invalid requests per 10 minutes per IP leads to a 24-hour IP ban.**
- Invalid means status 401, 403 or 429. A 429 with `X-RateLimit-Scope: shared` is exempt.
- How to avoid it:
  - On **401**, stop **all** requests and drop the session or token.
  - Check permissions locally before acting, to avoid **403**s.
  - Never send on an exhausted or known-limited bucket.
  - Don't loop retries on 4xx.
  - Some endpoints have extra, undocumented Cloudflare limits: honor `Retry-After`; if it is absent, don't retry.

### Other retry rules
Source: https://docs.discord.food/topics/errors
- Retry **502, 504, 507, 522, 523 and 524** with backoff, sending `X-Failed-Requests: n`. discord.py-self sleeps `1 + 2*tries`, up to 5 tries.
- **202** means the resource isn't ready. Body: `{"message":..., "code":11xxxx, "retry_after"?: f64}`. If `retry_after` is missing or `0`, wait about 5 s. discord.py-self adds an `attempts=n` query param on each retry.

---

## 4. Endpoints
Base: `https://discord.com/api/v9`

### 4.1 List messages: `GET /channels/{channel_id}/messages`
Source: https://docs.discord.food/resources/message#list-messages

| Query | Type | Notes |
|---|---|---|
| `limit` | int | 1–100, default 50 |
| `before` \| `after` \| `around` | snowflake | Mutually exclusive. If several are sent, only `around` or `before` is used. |

- Response: `Vec<Message>`.
- Order is newest first, including with `after`. This is **UNCONFIRMED** in the docs; discord.py-self treats `data[0]` as the newest.
- Permissions: needs `VIEW_CHANNEL`. Without `READ_MESSAGE_HISTORY` you get an empty array, not an error.
- **`GET /channels/{c}/messages/{m}` (single message) is "not usable by user accounts".** Use `?around={m}&limit=1` and check `data[0].id == m` (what discord.py-self does).

### 4.2 Create message: `POST /channels/{channel_id}/messages`
Sources: https://docs.discord.food/resources/message#create-message ; https://docs.discord.com/developers/resources/message

| Field | Type | Notes |
|---|---|---|
| `content?` | string | Max 2000 characters. The 4000 limit for Nitro is **UNCONFIRMED**. |
| `nonce?` | int \| string | Official docs: max 25 characters. The same nonce in the same channel within a short time sends only the first message. Clients use the **current-time snowflake as a string** (discord.py-self `str(time_snowflake(now))`, Abaddon `Snowflake::FromNow()`). Use it to match the echoed `MESSAGE_CREATE` to a local pending message. |
| `enforce_nonce?` | bool | Official (bot) docs: return the existing message instead of creating a duplicate. Not in the userdoccers user table, so behavior for user accounts is **UNCONFIRMED**. |
| `tts?` | bool | Needs `SEND_TTS_MESSAGES` |
| `flags?` | int | Only `SUPPRESS_EMBEDS` (1<<2), `SUPPRESS_NOTIFICATIONS` (1<<12) and voice message (1<<13) can be set |
| `message_reference?` | object | See below |
| `allowed_mentions?` | object | See below |
| `sticker_ids?` | snowflake[] | Max 3 |
| `attachments?` / `files[n]` | | Multipart; max 10 |
| `mobile_network_type?` | string | **Not documented by userdoccers.** discord.py-self sends `"unknown"` by default (enum: `bluetooth` `cellular` `ethernet` `none` `other` `unknown` `wifi` `wimax`). Optional. |
| `embeds`, `components` | | **Cannot be used by user accounts** |

- At least one of these is required: `content`, `embeds`, `components`, `sticker_ids`, `activity`, `files[n]`, `poll`, `shared_client_theme`, `with_checkpoint`, or a forward `message_reference`.
- Response: a `Message` (also fires `MESSAGE_CREATE`). Max request size is 200 MiB.

**`message_reference` (send):**

| Field | Type | Notes |
|---|---|---|
| `message_id` | snowflake | The only required field for a reply |
| `channel_id?` | snowflake | Optional when sending (validated if given); always present when received |
| `guild_id?` | snowflake | |
| `fail_if_not_exists?` | bool | Default `true`. If `false`, a reply to a deleted message is sent as a normal message. |
| `type?` | int | `0` = DEFAULT (reply), `1` = FORWARD |
| `forward_only?` | `{embed_indices?: int[], attachment_ids?: snowflake[]}` | FORWARD only |

- A reply needs `READ_MESSAGE_HISTORY`. The target must exist and can't be a system message.

**`allowed_mentions`:**

| Field | Type | Notes |
|---|---|---|
| `parse?` | string[] | Any of `"roles"`, `"users"`, `"everyone"` |
| `roles?` | snowflake[] | Max 100 |
| `users?` | snowflake[] | Max 100 |
| `replied_user?` | bool | Default `false` |

- `parse: ["users"]` together with `users: [...]` is a 400 validation error (the same holds for roles).
- Omitting `allowed_mentions` entirely means mentions are parsed from content.
- It is **UNCONFIRMED** whether a reply with `allowed_mentions` omitted pings the author. Bot libraries assume it does. **Always send `allowed_mentions` explicitly on replies.**

Reply **with** a ping:
```json
{"content":"hi","nonce":"1305212345678901248","tts":false,"flags":0,"mobile_network_type":"unknown",
 "message_reference":{"guild_id":"G","channel_id":"C","message_id":"M","type":0},
 "allowed_mentions":{"parse":["users","roles","everyone"],"replied_user":true}}
```
Reply **without** a ping: the same body with `"replied_user": false`.

To suppress all pings: `"allowed_mentions":{"parse":[]}`. Note that `@everyone` still needs `@everyone` in the content plus permission.

### 4.3 Edit message: `PATCH /channels/{channel_id}/messages/{message_id}`
Source: https://docs.discord.food/resources/message#edit-message

- Body: `{"content": "new text"}`. Every field is nullable/optional: `content?: ?string` (max 2000), `allowed_mentions?`, `flags?` (only `SUPPRESS_EMBEDS` may be set), `attachments?` (the list of attachments to keep).
- Only the author can edit content. Others with `MANAGE_MESSAGES` can only change `flags`, and must send all existing flag bits.
- Editing `content` rebuilds `mentions`. If `allowed_mentions` is absent, default parsing applies; the original send's settings are ignored.
- Response: `Message` (also fires `MESSAGE_UPDATE`).
- Messages older than 1 h: at most 3 edits per 6 s per channel.

### 4.4 Delete message: `DELETE /channels/{channel_id}/messages/{message_id}`
- Returns **204** with an empty body, and fires `MESSAGE_DELETE`.
- Deleting another user's message needs `MANAGE_MESSAGES`.
- Bulk delete is **not usable by user accounts**.

### 4.5 Reactions
Sources: https://docs.discord.food/resources/message#create-reaction ; https://docs.discord.com/developers/resources/message ; discord.py-self `http.py` lines ~1497–1580 ; Abaddon `discord.cpp` lines 705–731

| Action | Method and path | Query |
|---|---|---|
| Add | `PUT /channels/{c}/messages/{m}/reactions/{emoji}/@me` | `type=0` (normal) or `type=1` (burst/super) |
| Remove own | `DELETE /channels/{c}/messages/{m}/reactions/{emoji}/{type}/@me` (`type` = `0` or `1`) | |
| Remove own (legacy, official docs, used by Abaddon) | `DELETE .../reactions/{emoji}/@me` | Whether it removes a burst reaction is **UNCONFIRMED** |
| List users | `GET .../reactions/{emoji}` | `after?`, `limit?` (1–100, default 25), `type?` |

- Add and remove return **204**.
- Add needs `READ_MESSAGE_HISTORY`, plus `ADD_REACTIONS` if nobody has used that emoji on the message yet.
- Add fires `MESSAGE_REACTION_ADD`; remove fires `MESSAGE_REACTION_REMOVE`.

**`{emoji}` encoding:**
- **Unicode:** the exact emoji string, percent-encoded as UTF-8, e.g. 👍 → `%F0%9F%91%8D`. Keep variation selectors: ❤️ → `%E2%9D%A4%EF%B8%8F`. Use the exact string from `reaction.emoji.name`.
- **Custom:** `name:id`, e.g. `blobcat:123456789012345678`. There is no `a:` prefix for animated emoji. Abaddon sends the `:` unencoded.
- An unencoded emoji gives `10014 Unknown Emoji`.
- **Rust:** build the path with `url.path_segments_mut().unwrap().extend(["channels", c, "messages", m, "reactions", emoji, "@me"])`. The url crate percent-encodes non-ASCII per segment, so you don't need a new dependency.

**`location` query param:** the official client is believed to send something like `?location=Message&type=0`. This is **UNCONFIRMED** and not needed; discord.py-self only sends `type`.

**Reaction object (received):**
```
{ count: int, count_details: {normal: int, burst: int}, me: bool, me_burst: bool,
  emoji: {id: ?snowflake, name: ?string, animated?: bool}, burst_colors: string[] }
```

### 4.6 Ack (read state): `POST /channels/{channel_id}/messages/{message_id}/ack`
Source: https://docs.discord.food/topics/read-state

| Field | Type | Notes |
|---|---|---|
| `token?` | ?string | Legacy ack token. The new read-state service always returns `null` and ignores what you send. Send `null` or omit it. Abaddon sends `{"token":null}`. |
| `manual?` | bool | `true` when moving the read marker **backwards** ("mark unread"). Without it, clients may ignore the update. |
| `mention_count?` | int | Only together with `manual: true`. The server does not recompute it. |
| `flags?` | int | Read-state flags: `1<<0` IS_GUILD_CHANNEL, `1<<1` IS_THREAD, `1<<2` IS_MENTION_LOW_IMPORTANCE. Send only if your computed value differs from the stored one. If omitted, the stored value is kept. |
| `last_viewed?` | int | **Days** since the Discord epoch: `(now_ms - 1420070400000) / 86_400_000`. Send only if it changed; if omitted, the stored value is kept. |

- Response `200`: `{"token": null}` (a `?string`). Fires `MESSAGE_ACK`.
- The message ID only has to be a valid snowflake, not an existing message.

**When the official client calls it:** not documented (**UNCONFIRMED**). Ack when the channel is open, the window is focused, and the view is scrolled to the bottom. Debounce it, and ack the newest message ID.

Calculating `flags`:
- Set IS_GUILD_CHANNEL if the channel has a `guild_id`.
- Set IS_THREAD for channel types NEWS/PUBLIC/PRIVATE_THREAD.
- Set LOW_IMPORTANCE if every pending mention is a non-ping "all messages" mention.

Local automation you must mirror (the server sends no ack event for these): when a `MESSAGE_CREATE` authored by you arrives, and its type is not `POLL_RESULT`, set the local `last_message_id` to its ID and set `mention_count` to 0.

**Bulk: `POST /read-states/ack-bulk`** returns **204**:
```json
{"read_states":[{"channel_id":"C","message_id":"M","read_state_type":0}]}
```
- `read_state_type` defaults to 0 (CHANNEL). Other types: 1 GUILD_EVENT, 2 NOTIFICATION_CENTER, 3 GUILD_HOME, 4 GUILD_ONBOARDING_QUESTION, 5 MESSAGE_REQUESTS.
- `message_id` must be > 0. There is no `manual` field, so never move a marker backwards with this endpoint.
- `POST /guilds/{id}/ack` ("mark guild read") is deprecated. Use ack-bulk instead, as Abaddon does.

Read-state object (from READY): `id`, `read_state_type?` (default 0), `last_message_id?` **xor** `last_acked_id?`, `mention_count?` **xor** `badge_count?`, `last_pin_timestamp?`, `flags?`, `last_viewed?: ?int`. Ignore unknown types.

### 4.7 Users
Source: https://docs.discord.food/resources/user

**`GET /users/@me`** returns the full user object.
- Query: `with_analytics_token?` (bool, default false).
- Mostly redundant with READY.

**`GET /users/{id}/profile`**, for profile cards only:

| Query | Default |
|---|---|
| `with_mutual_guilds` | `true` |
| `with_mutual_friends` | `false` |
| `with_mutual_friends_count` | `false` |
| `guild_id` | to get the member profile in that guild |
| `type` | `popout` \| `modal` \| `sidebar` \| `account_popout` \| `action_sheet` \| `you_screen` |

- Access is allowed only for: bots, users sharing a guild, friends, friend suggestions, users with an outgoing request to you, or a valid `join_request_id`. Otherwise you get a 403, which counts as invalid.
- Response:
  - `user`: partial user plus `bio`.
  - `user_profile`: `{pronouns, bio?, banner?: ?string, accent_color?: ?int, theme_colors?: ?[int,int], ...}`.
  - `guild_member?`, `guild_member_profile?`, `badges: [{id, description, icon, link?}]`.
  - `mutual_guilds?: [{id, nick: ?string}]`, `connected_accounts`, `premium_type: ?int`, `premium_since: ?ISO8601`.
  - `legacy_username?: ?string`, `private?: bool`.
- **Serde gotcha:** if the target user has blocked you, fields are unexpectedly missing or `null`. Make everything `Option` with `#[serde(default)]`.
- Fetch only on user action and cache it. The specific limits on this endpoint are **UNCONFIRMED**.

### 4.8 Error JSON
Sources: https://docs.discord.food/reference#error-messages ; https://docs.discord.food/topics/errors

```json
{"code":50035,"message":"Invalid Form Body",
 "errors":{"content":{"_errors":[{"code":"BASE_TYPE_MAX_LENGTH","message":"..."}]}}}
```
- `errors` is a recursive map keyed by JSON field. Array indices appear as string keys (`"0"`). Request-level errors use `errors._errors`. Model it as `Option<serde_json::Value>`.
- Example: `{"message":"Invalid authentication token","code":50014}`.
- **Bodies with other shapes that a single error struct won't parse:**
  1. **CAPTCHA:** status 400 with `{"captcha_key":[...],"captcha_sitekey":?string,"captcha_service":"hcaptcha",...}`. There is no `code` or `message`. Detect it as status 400 with a `captcha_key` field present. A lightweight client should show "verification required" and let the user resolve it. Never solve it automatically.
  2. **Cloudflare block:** `{"message":"internal network error","code":40333}`, or an HTML body that isn't JSON at all.
  3. **Anti-abuse block:** 403 with `{"message":"Unknown Message","code":10008}`, despite the misleading message.
  4. **429 body** (§3) and **202 body** (§3).
- Deserialize with `serde_json::from_slice::<serde_json::Value>` first, then branch.

### 4.9 General serde edge cases
- **Snowflakes are JSON strings,** but can come back as integers when you sent a non-bigint, e.g. `not_found` in `GUILD_MEMBERS_CHUNK`. Write a deserializer that accepts both.
- **`nonce`:** int \| string, may be absent. Use `#[serde(untagged)] enum Nonce { S(String), I(u64) }` or a `Value`.
- **`referenced_message`:** if the key is **absent**, the backend didn't attempt to fetch it. If it is **`null`**, the referenced message was deleted. Use `Option<Option<Box<Message>>>` with `#[serde(default, deserialize_with=...)]` or a double-option helper.
- **Emoji:** `id: null` for unicode emoji. `name` can be `null` for a deleted custom emoji.
- **Attachments:** `height`/`width` are optional **and** nullable. `content_scan_version` is missing if the attachment hasn't been scanned yet.
- **Timestamps** look like `"2023-02-17T19:52:19.184000+00:00"`. `edited_timestamp` is nullable.
- **Discriminator:** a string. `"0"` (not `"0000"`) means the user is migrated to the new username system. `global_name` is nullable.
- **Boolean query strings:** the API accepts `true`/`True`/`1`. The **CDN accepts only `true`/`false`**.

---

## 5. CDN and media URLs
Sources: https://docs.discord.food/reference#cdn-formatting ; https://docs.discord.com/developers/reference ; https://docs.discord.food/resources/emoji#emoji-formats ; https://docs.discord.food/topics/cloud-uploads

CDN base: `https://cdn.discordapp.com/`. Hashes beginning with `a_` are animated.

| Asset | Path | Formats |
|---|---|---|
| Guild icon | `/icons/{guild_id}/{icon}.{ext}` | png, jpg, webp, gif |
| User avatar | `/avatars/{user_id}/{avatar}.{ext}` | png, jpg, webp, gif |
| Guild member avatar | `/guilds/{guild_id}/users/{user_id}/avatars/{member_avatar}.{ext}` | png, jpg, webp, gif |
| Guild member banner | `/guilds/{guild_id}/users/{user_id}/banners/{banner}.{ext}` | png, jpg, webp, gif |
| User banner | `/banners/{user_id}/{banner}.{ext}` | png, jpg, webp, gif |
| Default avatar | `/embed/avatars/{index}.png` | png only; `size` ignored |
| Custom emoji | `/emojis/{emoji_id}.{ext}` | png, jpg, webp, gif |
| Role icon | `/roles/{role_id}/icons/{icon}.{ext}` | png, jpg, webp |
| Sticker | `/stickers/{id}.png` (or `.json` for Lottie) | GIF stickers only via `https://media.discordapp.net/stickers/{id}.gif` |
| Attachment | `/attachments/{channel_id}/{message_id}/{attachment_id}/{filename}` | As uploaded; signed |

**Animated assets:** use `.gif`, or `.webp?animated=true` (`animated` only works with WebP). Without `animated=true`, `.webp` returns a still image.

**Emoji:** Discord recommends `.webp`. Use `https://cdn.discordapp.com/emojis/{id}.webp?size=48&quality=lossless`, adding `&animated=true` when `emoji.animated`. Emoji uploaded as WebP/AVIF are only served as WebP.

**Default avatar index:**
- Migrated users (`discriminator == "0"`): `(user_id >> 22) % 6`, giving 0–5.
- Legacy users: `discriminator % 5`, giving 0–4.
- **Write the parentheses.** The userdoccers footnote says `user_id >> 22 % 6`, and in Rust (and JS) `%` binds tighter than `>>`, so that would compute `id >> 4`.

**Query parameters (images only):**

| Param | Values |
|---|---|
| `size` | Any power of two from 16 to 4096, plus 20, 22, 24, 28, 40, 44, 48, 56, 60, 80, 96, 100, 160, 240, 300, 320, 480, 600, 640, 1280, 1536, 3072 |
| `quality` | `lossless` \| `high` (default) \| `low`; WebP and JPG only |
| `keep_aspect_ratio` | bool, default `false` |
| `passthrough` | bool, default `true` |
| `animated` | bool, default `false`; WebP only |

**`url` vs `proxy_url`:**
- An attachment's `url` points at `cdn.discordapp.com`, the original file.
- Its `proxy_url` is the same path on `https://media.discordapp.net/`. The media proxy:
  - resizes and transcodes, taking `width`, `height` (max dimensions), `format` (e.g. `webp`, or `jpg` for a video still), `quality` and `size`;
  - returns **415** for attachments without dimensions (non-image, non-video).
- Embed media `proxy_url` values may live on other hosts such as `https://images-ext-1.discordapp.net` / `images-ext-2`. Whether those accept the same `width`/`height`/`format` params is **UNCONFIRMED**; the docs only say media-proxy URLs appear on other hosts.
- To get back the original CDN URL from a proxy URL, strip `width`, `height`, `quality`, `size` and `format`.
- Thumbnail example: `{proxy_url}` with `format=webp&quality=lossless&width=400&height=300` appended.
- Signed URLs end in a trailing `&`. Append with `Url::query_pairs_mut().append_pair(...)`, never by string concatenation.

**Signed attachment URLs (`ex`, `is`, `hm`):**
- Applies to `/attachments/`, `/ephemeral-attachments/` and `/server-products/`. It also applies on media.discordapp.net, where the signature is still required. Other CDN paths (avatars, icons, emoji) are unsigned and never expire.
- `ex` is a hex unix-seconds expiry and `is` a hex issue time. `hm` is the signature, valid until `ex`.
- In the docs' examples, `ex - is = 0x127500 = 1,209,600 s`, i.e. 14 days. The real lifetime is **UNCONFIRMED**, so read `ex`.
- URLs in API and gateway payloads arrive freshly signed. URLs inside message **content** or embed descriptions are not refreshed.
- **Caching:**
  - Key cached bytes on the URL with `ex`/`is`/`hm` removed. Keep the transform params, since they identify the variant. The content behind an attachment ID is immutable.
  - When `ex` has passed and you need to fetch again: re-list messages, or call `POST /api/v9/attachments/refresh-urls`.
  - Refresh request: `{"attachment_urls":[...]}` with 1–50 URLs, which need not be valid or signed.
  - Refresh response: `{"refreshed_urls":[{"original":"...","refreshed":"..."}]}`.
  - The refresh call goes to the API host, so it is the only one of these calls that carries the token.

---

**Not checked:** the official client's exact `location` query values, the ack debounce timing, the reaction rate-limit numbers, the 4000-character Nitro limit, and the reply-ping default when `allowed_mentions` is omitted (all marked UNCONFIRMED above). The arikawa/Discordo and Dissent sources were not consulted.

**Risks:**
- User tokens get almost no proactive rate-limit headers, so the limiter must be conservative and 429-driven.
- 401/403/429 loops can get the IP banned for 24 h.
- CAPTCHA (400 with `captcha_key`) can appear on any endpoint and must be shown to the user, never solved automatically.


---

# Part 2: Member list, permissions, read states, notification settings

# Discord user client: lazy member list, permissions, read states, notification settings

**Sources and their status (as of 2026-11-11):**
- **userdoccers master** (`pages/*.mdx`, fetched raw from GitHub, served at docs.discord.food). It does not document op 14 or 37 payloads, nor `GUILD_MEMBER_LIST_UPDATE`. In the send-events table, "Update Guild Subscriptions" has no link.
- **Open PR #595 "Guild subscriptions"** (opened 2026-07-04, not merged) has that material: https://github.com/discord-userdoccers/discord-userdoccers/pull/595. I quote it below as **[PR595]**.
- **Issue #461** shows real payloads: https://github.com/discord-userdoccers/discord-userdoccers/issues/461. A maintainer (dolfies) replied that the event is undocumented on purpose because the subscriptions system is "a lot more vast and convoluted".
- **Implementations used as references:**
  - discord.py-self (dolfies, a userdoccers maintainer), tagged **[DPYS]**: https://github.com/dolfies/discord.py-self, files `discord/state.py`, `abc.py`, `gateway.py`, `types/gateway.py`, `read_state.py`, `settings.py`, `message.py`, `guild.py`.
  - ningen and arikawa (used by Dissent/gtkcord4), tagged **[NINGEN]** and **[ARIKAWA]**: https://github.com/diamondburned/ningen (branch v3: `ningen.go`, `states/member/member.go`, `states/mute/mute.go`, `states/read/read.go`) and https://github.com/diamondburned/arikawa/blob/v3/gateway/events.go.

---

## 1. Guild subscriptions: op 14 and op 37

Opcode table (https://docs.discord.food/gateway/opcodes-and-close-codes):
- `14` "Guild Subscriptions **(deprecated)**": update subscriptions for one guild.
- `37` "Guild Subscriptions Bulk": update subscriptions for several guilds.

**Which one the current client uses:** op 37.
- [DPYS] defines `GUILD_SUBSCRIBE = 14  # Deprecated` and `BULK_GUILD_SUBSCRIBE = 37`, and sends everything through op 37.
- arikawa/ningen still send op 14 (`func (*GuildSubscribeCommand) Op() ws.OpCode { return 14 }`). That op 14 is still accepted in Nov 2026 is UNCONFIRMED.

### Per-guild subscription object ([PR595], [DPYS] `BaseGuildSubscribePayload`, every field optional)

| field | type | meaning |
|---|---|---|
| `typing` | bool | Subscribes to the guild itself (typing and the other non-stateful events). Once true, the guild stays subscribed for good and cannot be unsubscribed. |
| `threads` | bool | Thread events for threads you are not in. Setting it sends `THREAD_LIST_SYNC`. |
| `activities` | bool | Embedded-activity events. [DPYS] says "not sure what this does". |
| `member_updates` | bool | `GUILD_MEMBER_ADD`, `UPDATE` and `REMOVE` for other users. |
| `members` | array[snowflake] | Per-user member and presence subscriptions. No cap except payload size. |
| `channels` | map[snowflake → array[[int,int]]] | Member-list ranges per channel. Ranges are inclusive. Limits: max 5 channels, 1–3 ranges per channel. |
| `thread_member_lists` | array[snowflake] | Thread IDs. Produces `THREAD_MEMBER_LIST_UPDATE` `{guild_id, thread_id, members:[{user_id, member, presence}]}`. |

- **op 14 `d`:** the object above plus `guild_id`.
- **op 37 `d`:** `{"subscriptions": { "<guild_id>": <object> , ...}}`.

Only keys present in the payload are updated server-side ([DPYS] comment). `channels` replaces the guild's whole channel map: [DPYS] always sends the full merged map, and sending `{}` unsubscribes from all member lists.

### Range normalisation (server-side, [PR595] pseudocode)

- Number of ranges must be 1–3.
- Each `[s,e]` is reordered to `[min,max]`; `e - s > 300` is an error.
- Each range is expanded to whole 100-item chunks, `[k*100, k*100+99]`, then deduped and sorted.

### Minimum payload for one channel's sidebar

```json
{"op":37,"d":{"subscriptions":{"<GUILD_ID>":{"typing":true,"activities":true,"threads":true,"channels":{"<CHANNEL_ID>":[[0,99]]}}}}}
```

- [DPYS] refuses to send `channels` or `members` unless `typing` is true or pending: "Cannot subscribe to guild without subscribing to typing".
- ningen's op 14 sends `{"guild_id","typing":true,"threads":false,"activities":true,"channels":{...}}`.
- The exact feature set the official client sends is UNCONFIRMED.

### Scrolling

- Always keep `[0,99]`, plus the visible chunk and the next one, for at most 3 ranges. Example: `[[0,99],[200,299],[300,399]]` ([NINGEN] `RequestMemberList`, [DPYS]).
- Re-send only when the visible chunk index changes.

### Behaviour you must handle

- **Guilds over 75k members:** you get no `MESSAGE_*`, `TYPING` or presence events until you send `typing:true`. You still get forced dispatches for messages that mention you, your roles or @everyone.
- **First subscribe to a large guild:** the gateway sends a fresh `GUILD_CREATE` for a guild you already have. Replace your cache; do not add a duplicate.
- **Unsubscribed large guilds** send `CHANNEL_UNREAD_UPDATE` `{guild_id, channel_unread_updates:[{id, last_message_id:?snowflake, last_pin_timestamp?:?ts}]}`, or `PASSIVE_UPDATE_V1`/`V2` if those capabilities are set. Use these to update `channel.last_message_id` for unread state.
- **On a new session (not a resume)** re-send all subscriptions. [DPYS] `_requeue_subscriptions` does this. Whether subscriptions survive a RESUME is UNCONFIRMED.

### Rate limits

- 120 gateway events per connection per 60 s, or you are disconnected (close 4008). Source: https://docs.discord.food/gateway/using-gateway#rate-limiting.
- An op 37 payload must be ≤ 15360 bytes of compact JSON. [DPYS] `MAX_PAYLOAD_SIZE = 15360`, "taken from the client". [PR595] gives a fragmenting algorithm: split by guild when the running length of `json.dumps([guild_id, sub])` exceeds 15360.
- [DPYS] debounces and merges subscription changes with a 0.5 s tick. Do the same.
- Op 8 (Request Guild Members) has its own limit of 1 per guild per session per 30 s. For users it needs `MANAGE_ROLES`, `KICK_MEMBERS` or `BAN_MEMBERS` to list everyone. Use op 37 for the sidebar instead.

---

## 2. GUILD_MEMBER_LIST_UPDATE (dispatch `t:"GUILD_MEMBER_LIST_UPDATE"`)

Sources: [PR595], issue #461, [ARIKAWA] `GuildMemberListUpdateEvent`, [DPYS] `types/gateway.py`.

```json
{"guild_id":"G","id":"everyone","member_count":21,"online_count":5,
 "groups":[{"id":"<ROLE_ID>","count":1},{"id":"online","count":3},{"id":"offline","count":16}],
 "ops":[{"op":"SYNC","range":[0,99],"items":[
    {"group":{"id":"<ROLE_ID>","count":1}},
    {"member":{"user":{...},"roles":["<ROLE_ID>"],"nick":null,"avatar":null,"joined_at":"...","flags":0,
               "presence":{"user":{"id":"..."},"status":"online","client_status":{"desktop":"online"},"activities":[]}}},
    {"group":{"id":"offline","count":16}}, {"member":{...}} ]}]}
```

### Top-level fields

| field | type | notes |
|---|---|---|
| `id` | string | Member-list ID: `"everyone"` or a decimal u32 string. |
| `guild_id` | snowflake | |
| `member_count` | int | [DPYS] only applies it if > 0, so it can be 0. |
| `online_count` | int | Same as `member_count`. |
| `groups` | array of `{id, count}` | The complete current group list, so replace yours. `id` is a role ID, `"online"` or `"offline"`. |
| `ops` | array | Applied in order. Each op's index is relative to the list state after the previous op. |

### Op shapes (exact; `op` is a string)

| op | fields | meaning |
|---|---|---|
| `SYNC` | `range:[int,int]`, `items:[item]` | Replace items `range[0] .. range[0]+items.len()`. `items` can be shorter than 100 at the end of the list. |
| `INVALIDATE` | `range:[int,int]` | You are no longer subscribed to that range. Blank it; do not shift. |
| `INSERT` | `index:int`, `item` | Insert at `index`, shifting later items right. |
| `UPDATE` | `index:int`, `item` | Replace in place. |
| `DELETE` | `index:int` | Remove, shifting later items left. |

`range` is a flat `[start,end]` pair ([ARIKAWA] `Range [2]int`, [DPYS] `Tuple[int,int]`, issue #461). The `[[0,99]]` in PR595's example is a typo.

### Item shape

Exactly one key is present: `{"group":{...}}` or `{"member":{...}}`.

- **`group`:** `{id: string, count?: int}`. Issue #461 shows `{"group":{"id":"ROLE_ID"}}` without `count`, so make `count` an `Option`.
- **`member`:** a guild member object with:
  - a full partial `user` object (not `user_id`);
  - `roles: [snowflake]`, `nick: ?string`, `avatar: ?string`, `joined_at`, `premium_since?`, `deaf`, `mute`, `pending?`, `communication_disabled_until?`, `flags`;
  - `presence`: `{user:{id}, status, client_status, activities}`;
  - `hoisted_role` (string) per [ARIKAWA]. Not in [PR595], so UNCONFIRMED; model as `Option`.
- **INSERT/UPDATE item type:** [DPYS] types these as member-only. [NINGEN] handles generic items, and group inserts when a group becomes non-empty are plausible (UNCONFIRMED). Model `item` as the same enum.

### List semantics ([PR595])

- Indexes are positions in the whole flattened list, with group headers counted.
- Groups appear in order: hoisted roles sorted by role position, then `online` (online members with no hoisted role), then `offline`.
- Members within a group are sorted by `nick`, then `global_name`, then `username`.
- Empty groups are usually omitted.

### List length

- Virtual list length = Σ over `groups` of (1 + `count`).
- If there is no `"offline"` group, offline members are hidden. [NINGEN] `TotalVisible` = `online_count` if there is no offline group, else `member_count`.
- [DPYS] heuristic for when offline members are hidden: `member_count + hoisted_roles + 2 >= 1000`.
- Some guilds have manually overridden groups with no `"online"` group ([DPYS] `handle_manual_override`).

### Serde model sketch

```rust
#[derive(Deserialize)] #[serde(tag="op", rename_all="SCREAMING_SNAKE_CASE")]
enum ListOp { Sync{range:[u32;2], items:Vec<ListItem>}, Invalidate{range:[u32;2]},
              Insert{index:u32,item:ListItem}, Update{index:u32,item:ListItem}, Delete{index:u32},
              #[serde(other)] Unknown }
#[derive(Deserialize)] #[serde(rename_all="lowercase")]          // externally tagged: {"group":{..}} | {"member":{..}}
enum ListItem { Group(ListGroup), Member(Box<ListMember>) }
#[derive(Deserialize)] struct ListGroup { id:String, #[serde(default)] count:Option<u32> }
```

### Member-list ID

Exact algorithm from [DPYS] `abc.py` `member_list_id` / `_is_everyone_member_list`, which matches [PR595]:

```
everyone_ok = (@everyone ROLE's guild-level permissions & VIEW_CHANNEL) != 0
              && no overwrite in channel.permission_overwrites has (deny & VIEW_CHANNEL)
if everyone_ok -> "everyone"
parts = for each overwrite: if allow&VIEW -> "allow:<id>" elif deny&VIEW -> "deny:<id>"   (others skipped)
id = decimal( murmur3_x86_32( utf8( join(",", sort_lexicographic_as_strings(parts)) ), seed=0 ) as u32 )
```

- [DPYS] calls this check "flawed" because it ignores channel-level allows for @everyone, but uses it so the IDs match what the server sends. Channels that are really public but carry a deny get a hash ID whose list is functionally the same.
- [NINGEN]'s `ComputeListID` is older and wrong: it does not sort and skips the @everyone check. Don't copy it.
- **Do you have to compute it?** Not strictly, since the event carries `id`. But you need it to map an open channel to a list, and one list ID covers many channels. Key lists by `(guild_id, list_id)` and look up `list_id(channel)`.
- Any murmur3 x86_32 implementation works (e.g. the `murmur3` crate's `murmur3_32(&mut Cursor::new(bytes), 0)`; check the API).
- Self-check values from memory (UNCONFIRMED): `""` → 0 and `"hello"` → 613153351 (0x248BFA47).

---

## 3. Permission computation

Source: https://docs.discord.food/topics/permissions.

**Representation:**
- Every permission value (`role.permissions`, overwrite `allow`/`deny`, member `permissions`) is a **decimal string**.
- The highest documented bit is currently 1<<53, so parse to u64. userdoccers recommends a bigint long-term; u128 is cheap.
- Overwrite object: `{id, type:int (0=role, 1=member), allow:string, deny:string}`. In v7 and below, `type` was the string `"role"`/`"member"`.

| name | bit | decimal |
|---|---|---|
| ADMINISTRATOR | 1<<3 | 8 |
| MANAGE_CHANNELS | 1<<4 | 16 |
| ADD_REACTIONS | 1<<6 | 64 |
| VIEW_CHANNEL | 1<<10 | 1024 |
| SEND_MESSAGES | 1<<11 | 2048 |
| SEND_TTS_MESSAGES | 1<<12 | 4096 |
| MANAGE_MESSAGES | 1<<13 | 8192 |
| EMBED_LINKS | 1<<14 | 16384 |
| ATTACH_FILES | 1<<15 | 32768 |
| READ_MESSAGE_HISTORY | 1<<16 | 65536 |
| MENTION_EVERYONE | 1<<17 | 131072 |
| CONNECT | 1<<20 | 1048576 |
| MANAGE_ROLES | 1<<28 | 268435456 |
| MANAGE_THREADS | 1<<34 | 17179869184 |
| SEND_MESSAGES_IN_THREADS | 1<<38 | 274877906944 |

**Algorithm (from the docs pseudocode):**

```
base(member, guild):
  if guild.owner_id == me -> ALL
  p = role[guild.id].permissions            // @everyone role id == guild id
  for r in member.roles: p |= role[r].permissions   // skip unknown role ids
  if p & ADMINISTRATOR -> ALL
channel(base, ch):
  if base & ADMINISTRATOR -> ALL
  p = base
  ow[guild_id] (the @everyone overwrite): p &= !deny; p |= allow
  A = OR(allow of ow[r] for r in member.roles); D = OR(deny ...); p &= !D; p |= A
  ow[my_user_id] (member overwrite):        p &= !deny; p |= allow
```

Order: @everyone base → member roles → @everyone overwrite → role overwrites (all denies, then all allows) → member overwrite.

**Implicit rules:**
- No VIEW_CHANNEL means effectively nothing in that channel.
- No SEND_MESSAGES means MENTION_EVERYONE, SEND_TTS_MESSAGES, ATTACH_FILES and EMBED_LINKS are ignored.
- Voice/stage without CONNECT implicitly denies others.
- **Timed out** (`communication_disabled_until` in the future) or quarantined: only VIEW_CHANNEL and READ_MESSAGE_HISTORY remain (quarantined also keeps CHANGE_NICKNAME). The owner and ADMINISTRATOR are exempt.

**Threads:** use the parent channel's permissions, but sending needs SEND_MESSAGES_IN_THREADS instead of SEND_MESSAGES. You need VIEW_CHANNEL on the parent to see any thread. A private thread (12) is visible if you were added or have MANAGE_THREADS.

**Category sync:** this is copying, not inheritance. A synced child holds identical overwrites, so compute only from the channel's own `permission_overwrites`. Never merge in the parent's.

**Where my roles come from:**
- **READY, default user capabilities with DEDUPE_USER_OBJECTS (1<<4):**
  - `merged_members` is `array[array[member]]`, in the same order as `guilds`.
  - Members carry `user_id` instead of `user`. Find `user_id == me.id` and read `roles`.
  - With PRIORITIZED_READY_PAYLOAD (1<<5), each inner array holds only your own member; the rest arrives in READY_SUPPLEMENTAL.
- **READY without DEDUPE:** `guilds[i].members` includes your own member with a `user` object.
- **CLIENT_STATE_V2 (1<<10):** guild scalars such as `owner_id`, `default_message_notifications` and `name` move into `guilds[i].properties`. `roles`, `channels`, `emojis` and `stickers` stay top-level. `channels` can be omitted if you sent cached guild versions, so check `data_mode`.
- **Updates:**
  - `GUILD_MEMBER_UPDATE` `{guild_id, user, roles, nick?, avatar, joined_at, premium_since, communication_disabled_until, flags, ...}` is always sent for yourself. Replace `roles` when `user.id == me`.
  - Also recompute on:
    - `GUILD_ROLE_CREATE` / `GUILD_ROLE_UPDATE` `{guild_id, role}`;
    - `GUILD_ROLE_DELETE` `{guild_id, role_id}`;
    - `CHANNEL_UPDATE` (overwrites);
    - `GUILD_UPDATE` (`owner_id`).
- Role object: `{id, name, permissions:string, position:int, hoist:bool, mentionable:bool, managed, color (deprecated), colors, flags?}`. Roles are sorted by `(position, id)`.

---

## 4. Channel types and sidebar order

Source: https://docs.discord.food/resources/channel.

| type | name | text client |
|---|---|---|
| 0 | GUILD_TEXT | open as chat |
| 1 | DM | open as chat |
| 2 | GUILD_VOICE | open as chat (text-in-voice; has `last_message_id`). The CONNECT requirement for sending is UNCONFIRMED. |
| 3 | GROUP_DM | open as chat |
| 4 | GUILD_CATEGORY | header only |
| 5 | GUILD_NEWS | open as chat |
| 6 | GUILD_STORE | ignore (dead) |
| 10 | NEWS_THREAD | chat, under its parent |
| 11 | PUBLIC_THREAD | chat, under its parent (also forum/media posts) |
| 12 | PRIVATE_THREAD | chat, under its parent |
| 13 | GUILD_STAGE_VOICE | chat. Text chat in stage is UNCONFIRMED from a fetched source. |
| 14 | GUILD_DIRECTORY | not a chat |
| 15 | GUILD_FORUM | not a chat; open as a thread list |
| 16 | GUILD_MEDIA | not a chat; open as a thread list |
| 17 | LOBBY | ignore |
| 18 | EPHEMERAL_DM | ignore |

Ignore unknown types for forward compatibility.

**Sorting** ([DPYS] `Guild.by_category`, which follows the official UI; userdoccers channel docs):
1. Uncategorized channels (`parent_id` null) first.
2. Then categories sorted by `(position, id)`.
3. Inside each group, sort by `(bucket, position, id)`, with `id` compared as u64.
   - Bucket 0: text-like (0, 5, 15, 16).
   - Bucket 2: voice-like (2, 13).
   - Directory (14) has its own bucket (14).
   - Text-like always sits above voice-like.
4. Equal positions break ties by ascending ID. Treat a missing `position` as 0.

Threads are not in this ordering; nest them under `parent_id`. Thread order within a parent is UNCONFIRMED. Hiding categories with no visible children is UNCONFIRMED.

**Visibility:** VIEW_CHANNEL. Reading history needs READ_MESSAGE_HISTORY.

---

## 5. Read states

Sources: https://docs.discord.food/topics/read-state and gateway-events `Ready` / `Message Ack`.

**READY `read_state` comes in two shapes.** Handle both with an untagged enum:
- a plain array;
- with `VERSIONED_READ_STATES` (1<<2), `{entries:[...], partial:bool, version:int}`.

`user_guild_settings` behaves the same way with `VERSIONED_USER_GUILD_SETTINGS` (1<<3).

### Read-state entry

| field | type | notes |
|---|---|---|
| `id` | snowflake | channel ID, guild ID, or your user ID for user features |
| `read_state_type?` | int | Default 0. 0 CHANNEL, 1 GUILD_EVENT, 2 NOTIFICATION_CENTER, 3 GUILD_HOME, 4 GUILD_ONBOARDING_QUESTION, 5 MESSAGE_REQUESTS. |
| `last_message_id?` | snowflake | Only for CHANNEL. Mutually exclusive with `last_acked_id`. |
| `last_acked_id?` | snowflake | Only for non-CHANNEL types. |
| `mention_count?` | int | CHANNEL only. Mutually exclusive with `badge_count`. |
| `badge_count?` | int | non-CHANNEL types |
| `last_pin_timestamp?` | ISO8601 | CHANNEL only |
| `flags?` | int | CHANNEL only. 1<<0 IS_GUILD_CHANNEL, 1<<1 IS_THREAD, 1<<2 IS_MENTION_LOW_IMPORTANCE. |
| `last_viewed?` | ?int | Days since the Discord epoch. |

- Filter to `read_state_type` 0 (or missing) for channels. [DPYS] falls back `last_acked_id ← last_message_id` and `badge ← mention_count`.
- Deserialize snowflakes leniently (string or integer, with 0 meaning none). Integer 0 appearing in practice is UNCONFIRMED, but [NINGEN] treats a zero `LastMessageID` as "no read state".

### Unread rule

- A channel is unread if `channel.last_message_id > rs.last_message_id`. Compare as **u64**, not as strings, because lengths differ.
- Pins: compare `channel.last_pin_timestamp` with `rs.last_pin_timestamp`.
- Mention badge = `rs.mention_count`.
- `channel.last_message_id` can point to a deleted message. In forum/media it is the last thread ID.

### Channels with no read-state entry

Not documented.
- [NINGEN] `ChannelIsUnread` returns read when the entry is missing or `last_message_id` is 0.
- [DPYS] defaults `last_acked_id` to 0.
- I believe the official client guesses the ack ID as `snowflake_from_ms(guild.joined_at)`, i.e. `(unix_ms - 1420070400000) << 22`, so only messages after you joined count as unread. UNCONFIRMED; I could not find a source.
- Safe default: treat as read, or use the join-time guess.

### MESSAGE_ACK dispatch

`{channel_id, message_id, version:int, ack_type?:int, manual?:bool, mention_count?:int, flags?:?int, last_viewed?:?int}`

- Set `rs.last_message_id = message_id`.
- If `mention_count` is present, set it. Docs footnote: "If omitted or `null`, the current value is retained." [DPYS] does the same.
- Update `flags` and `last_viewed` when they are non-null.
- If `manual:true`, the ack can move backwards.
- Related events:
  - `CHANNEL_PINS_ACK` `{channel_id, timestamp, version}`.
  - Non-channel acks: `GUILD_FEATURE_ACK` and `USER_NON_CHANNEL_ACK`.

### Local updates on MESSAGE_CREATE (docs "Channel Read State Automations")

1. Set `channel.last_message_id = msg.id`.
2. If the author is me and `type != 46` (POLL_RESULT): `rs.last_message_id = msg.id` and `mention_count = 0`. Stop.
3. If the author is blocked or ignored: do nothing more.
4. **Private channel (1/3):** if `type != 2` (RECIPIENT_REMOVE) and (the DM is not muted, or the message mentions me), then `mention_count += 1`. Every unmuted DM message counts.
5. **Guild channel:** if the message mentions me, `mention_count += 1`. "Mentions me" counts user mentions, role mentions and @everyone, minus suppressed ones (section 6). Mute does not stop this.
6. Otherwise, if all of the following hold, `mention_count += 1` and set IS_MENTION_LOW_IMPORTANCE on the next ack:
   - the channel is not voice or stage;
   - the channel is not muted;
   - the effective level is ALL_MESSAGES (0);
   - READY `notification_settings.flags` has MENTION_ON_ALL_MESSAGES (1<<5).
7. On `THREAD_CREATE` by me in a forum/media parent: set the parent's `last_message_id` to the thread ID.

### Acking (REST)

`POST /channels/{channel_id}/messages/{message_id}/ack`, body `{"token":null}`. Optional fields:
- `manual`;
- `mention_count` (only with `manual`);
- `flags` (send only if the computed value differs);
- `last_viewed` (days since the Discord epoch).

Ack tokens are now always `null`. Bulk ack: `POST /read-states/ack-bulk` `{"read_states":[{"channel_id","message_id","read_state_type"?}]}`.

---

## 6. user_guild_settings and notify/unread decisions

Source: https://docs.discord.food/resources/user-settings#user-guild-settings-object.

### Entry fields

| field | type | notes |
|---|---|---|
| `guild_id` | ?snowflake | `null` means the entry for DMs and group DMs. The docs example shows a number; deserialize leniently. |
| `muted` | bool | |
| `mute_config` | ?`{end_time?:?ISO8601, selected_time_window?:int}` | `selected_time_window` -1 means indefinite. |
| `message_notifications` | int | 0 ALL_MESSAGES, 1 ONLY_MENTIONS, 2 NO_MESSAGES, 3 INHERIT (only meaningful in channel overrides) |
| `suppress_everyone` | bool | |
| `suppress_roles` | bool | |
| `flags` | int | 1<<11 UNREADS_ALL_MESSAGES, 1<<12 UNREADS_ONLY_MENTIONS, 1<<13 OPT_IN_CHANNELS_OFF, 1<<14 OPT_IN_CHANNELS_ON |
| `channel_overrides` | array | `[{channel_id, muted, mute_config:?obj, message_notifications, collapsed, flags?}]` |
| other | | `hide_muted_channels`, `mobile_push`, `mute_scheduled_events`, `notify_highlights` (0/1/2), `version` |

- **Channel-override `flags` use different bit positions from the guild `flags`:** 1<<9 UNREADS_ONLY_MENTIONS, 1<<10 UNREADS_ALL_MESSAGES, 1<<11 FAVORITED, 1<<12 OPT_IN_ENABLED, 1<<13 NEW_FORUM_THREADS_OFF, 1<<14 NEW_FORUM_THREADS_ON.
- Updates arrive as `USER_GUILD_SETTINGS_UPDATE` with a full entry as payload. Replace the entry and re-index its overrides.

### Effective mute

`muted && !(mute_config.end_time is some && end_time <= now)`. Treat a null `mute_config` or null `end_time` as indefinite. [DPYS] `MuteConfig` and [NINGEN] `muteConfigInvalid` agree.

- **Channel muted** = its override is muted, OR its parent category's override is muted ([NINGEN] `ChannelIsMuted`), OR it is a thread you have not joined ([NINGEN]).
- **Guild muted** = the guild entry's effective mute.

### Effective notification level for a guild channel

1. The channel override's `message_notifications`, if the override exists and the value is ≠ 3.
2. Else the parent category's override, if ≠ 3. Inheriting from the category is UNCONFIRMED.
3. Else the guild entry's `message_notifications`.
4. Else, if there is no entry (defensively also if the value is 3), `guild.default_message_notifications` (0/1, from `properties` under CLIENT_STATE_V2). [NINGEN] `GuildSettings` does this fallback.

**DMs/GDMs:** use the override in the `guild_id:null` entry if present and ≠ 3; otherwise ALL_MESSAGES.

### "Mentions me"

Mirrors [DPYS] `Message._is_self_mentioned` and docs footnote ¹.
- `mentions[]` contains my ID. Pinging replies put the replied-to author here; a non-pinging reply where `referenced_message.author.id == me` is not a mention (UNCONFIRMED from a fetched source).
- OR (`mention_everyone` && !guild_settings.`suppress_everyone`). This covers both @everyone and @here.
- OR (!guild_settings.`suppress_roles` && `mention_roles` ∩ my roles ≠ ∅).
- For the read-state counter, DMs count as mentioned when not muted.

### Should a new message raise a desktop notification?

My synthesis. The exact official logic is UNCONFIRMED.
- **Return false if:**
  - the author is me or blocked;
  - the channel is focused and the window is active;
  - my status is `dnd` (official client behaviour, UNCONFIRMED from a source);
  - I lack VIEW_CHANNEL.
- **DM/GDM:** notify unless the DM override is effectively muted, the override level is 2, or the `null`-guild entry is muted ([DPYS] also checks that last one).
- **Guild:**
  - Effective level 2 (NO_MESSAGES) → false.
  - Mentions me → true, even if the channel or guild is muted. The official mute dialog says mute hides unreads and notifications "unless you are mentioned" (UNCONFIRMED; [NINGEN] differs: when the guild is muted and set to ALL_MESSAGES, it does not notify).
  - Muted (channel, category or guild) → false.
  - Level 0 → true. Level 1 → false.

### Should the channel show as unread (bold)?

- If `mention_count > 0`: show the mention badge; this overrides mute ([NINGEN]).
- Else if the channel is muted: not unread.
- Else unread if `last_message_id > ack`.
- **Guild icon:** if the guild is muted, show only mention badges ([NINGEN] `GuildIsUnread`).
- **Newer unread flags:** override-level UNREADS_* flags take priority over guild-level ones. They only apply when READY `notification_settings.flags` has USE_NEW_NOTIFICATIONS (1<<4); unset means unreads follow `message_notifications`. How that maps for ONLY_MENTIONS is UNCONFIRMED. The legacy behaviour, where any new message makes an unmuted channel unread, is a safe default.

---

Not checked: Abaddon's and Discordo's own code, and the official client's minified JS. Every point marked UNCONFIRMED comes from that gap. The member-list and op 37 shapes rely on an unmerged docs PR plus two independent libraries, so make every member-list field an `Option` and keep an `Unknown` op variant.


---

# Part 3: Message objects and markdown

# Discord message rendering and markdown: implementation reference

Items marked **UNCONFIRMED** are things I know from experience or from a secondary source, but could not verify against a fetched page. All snowflake IDs are JSON strings. All example IDs below are fake.

---

## 1. Message object (fields a client renders)

Sources: https://docs.discord.food/resources/message , https://docs.discord.food/topics/gateway-events (Message Create), https://docs.discord.com/developers/events/gateway-events , https://docs.discord.food/resources/user

### 1.1 Field table

`?` means the key may be absent. `nullable` means the key may be present with value `null`.

| field | type | notes / serde edge cases |
|---|---|---|
| `id` | snowflake str | |
| `channel_id` | snowflake str | |
| `guild_id?` | snowflake | **Gateway only** (MESSAGE_CREATE/UPDATE extra field). It is absent in REST message lists and on ephemeral messages. |
| `author` | partial user | Can be a webhook pseudo-user; see §1.3. |
| `member?` | partial guild member **without `user`** | Gateway only. Absent for webhooks and ephemeral messages. REST GET messages do not include it. |
| `content` | string | Can be `""`: attachment-only messages, forwards, components-v2 messages, and system messages. |
| `timestamp` | ISO8601 | e.g. `"2021-04-20T16:20:30.123000+00:00"`. The fraction may be missing. Parse as RFC3339 (`chrono::DateTime<FixedOffset>`). |
| `edited_timestamp` | ISO8601, **nullable** | |
| `tts` | bool | Always `false` in MESSAGE_UPDATE. |
| `mention_everyone` | bool | |
| `mentions` | array of partial user | In gateway events each entry may also carry an optional **`member`** key (partial member, no `user`). |
| `mention_roles` | array of snowflake str | |
| `mention_channels?` | array | Only for crossposts. Ignore. |
| `attachments` | array of attachment | See §1.4. |
| `embeds` | array of embed | See §3. |
| `reactions?` | array of reaction | **Absent** (not `[]`) when there are no reactions. |
| `nonce?` | **integer OR string** | Use an untagged enum or `serde_json::Value`. |
| `pinned` | bool | |
| `webhook_id?` | snowflake | If present, the author is a webhook. |
| `type` | int | See §2. Treat unknown values as a raw `u8`/`i32`, never as a closed enum. |
| `flags` | int | Bitfield, see §1.2. Use `#[serde(default)]`. |
| `message_reference?` | object | `{type?: int (0=DEFAULT, 1=FORWARD; absent means 0), message_id?, channel_id?, guild_id?}`. `channel_id` is always present on receive. |
| `referenced_message?` | message, **nullable** | Three states: absent (not a reply, or not fetched), `null` (the referenced message was **deleted**), or an object. Model it as `Option<Option<Box<Message>>>` using `serde_with::rust::double_option`. The nested message has no `referenced_message` of its own (one level only). |
| `message_snapshots?` | array of `{message: SnapshotMessage}` | Present for forwards (`message_reference.type == 1`, flag `HAS_SNAPSHOT`). A snapshot has **no `id`, `author` or `channel_id`**. Its fields are `content, timestamp, edited_timestamp(nullable), mentions, mention_roles, attachments, embeds, type, flags, components?, resolved?, sticker_items?, soundboard_sounds?`. It needs its own struct. |
| `call?` | `{participants: [snowflake], ended_timestamp: ISO8601 nullable}` | `ended_timestamp` may be unexpectedly `null`. |
| `interaction?` | deprecated `{id, type, name, user}` | Fallback when `interaction_metadata` is missing. |
| `interaction_metadata?` | object | See §1.5. |
| `thread?` | channel object | The thread started from this message. |
| `components` | array | Docs mark it required. Ignore it, but use `#[serde(default)]` because it is absent in snapshots. |
| `sticker_items?` | `[{id, name, format_type}]` | `format_type`: 1=PNG, 2=APNG, 3=LOTTIE, 4=GIF. |
| `stickers?`, `poll?`, `activity?`, `application?`, `application_id?`, `role_subscription_data?`, `purchase_notification?`, `gift_info?`, `resolved?`, `soundboard_sounds?`, `potions?`, `shared_client_theme?`, `changelog_id?`, `lobby_id?` | various | Use `#[serde(default)]` on everything optional and do **not** use `deny_unknown_fields`, because new fields appear constantly. |
| `channel_type` | int | Gateway extra field. docs.discord.food says required; the official docs say optional. Treat it as `Option`. |
| `metadata?`, `moderation_metadata?` | `map<string,string>` | Gateway extras. Ignore. |

### 1.2 Message flags (bits)

| bit | name |
|---|---|
| `1<<0` | CROSSPOSTED |
| `1<<1` | IS_CROSSPOST |
| `1<<2` | **SUPPRESS_EMBEDS**: hide all embeds |
| `1<<3` | SOURCE_MESSAGE_DELETED |
| `1<<4` | URGENT |
| `1<<5` | HAS_THREAD |
| `1<<6` | **EPHEMERAL** |
| `1<<7` | **LOADING** ("thinking…") |
| `1<<8` | FAILED_TO_MENTION_SOME_ROLES_IN_THREAD |
| `1<<9` | GUILD_FEED_HIDDEN |
| `1<<10` | SHOULD_SHOW_LINK_NOT_DISCORD_WARNING |
| `1<<12` | SUPPRESS_NOTIFICATIONS (@silent) |
| `1<<13` | **IS_VOICE_MESSAGE** |
| `1<<14` | **HAS_SNAPSHOT** (forward) |
| `1<<15` | **IS_COMPONENTS_V2**: `content`/`embeds` are empty and the text lives in components |
| `1<<16` | SENT_BY_SOCIAL_LAYER_INTEGRATION |
| `1<<17` | HIDDEN_SUSPENDED_USER |
| `1<<18` | IS_FIRST_BOOSTER |
| `1<<19` | IS_GUILD_OFFICIAL |

Bit 11 is undefined.

If you ignore components, an IS_COMPONENTS_V2 message renders as empty. Show a placeholder for it.

### 1.3 Author, member and display name

- **Partial user.** Always present: `id`, `username`, `discriminator` (string), `avatar` (nullable). Optional: `global_name` (nullable), `bot?`, `system?`, `public_flags?`, `avatar_decoration_data?` (nullable), `collectibles?`, `display_name_styles?`, `primary_guild?` (nullable). `banner` and `accent_color` are only guaranteed from profile endpoints.
- **Discriminator.**
  - `"0"` means a migrated (pomelo) user. Show `username` without a `#tag`.
  - Legacy users have `"1234"`.
  - Webhooks have `"0000"`. That value is **UNCONFIRMED** from the docs, but it is long-standing behavior.
- **Display name priority:** `member.nick` → `user.global_name` → `user.username`. `nick` may be absent or null.
- **Member fields used for rendering:** `nick?` (nullable), `avatar?` (nullable, per-guild avatar), `roles: [snowflake]`. Name color is the color of the highest-positioned role with a non-zero color, resolved from the guild role cache. Also: `joined_at`, `premium_since?`, `communication_disabled_until?`, `flags`, `pending?`.
- **Avatar URLs.**
  - User avatar: `https://cdn.discordapp.com/avatars/{user_id}/{hash}.webp?size=80`.
  - A hash that starts with `a_` is animated: add `&animated=true` (or use `.gif`).
  - Member avatar: `https://cdn.discordapp.com/guilds/{guild_id}/users/{user_id}/avatars/{hash}.webp`.
  - Default avatar: `https://cdn.discordapp.com/embed/avatars/{i}.png`, where `i = (user_id >> 22) % 6` if `discriminator == "0"`, else `discriminator % 5`.
  - Valid `size` values are powers of two from 16 to 4096.
- **Webhook author:** `webhook_id` is present and `author.id == webhook_id`. Webhooks have no `member`.
- **Bot tag:** `author.bot == true` shows a "BOT"/"APP" tag. The current label wording is **UNCONFIRMED**.

### 1.4 Attachment object

Receive-side fields:

| field | type | notes |
|---|---|---|
| `id` | snowflake | |
| `filename` | string | |
| `title?` | string | |
| `description?` | string | Alt text. |
| `content_type?` | string | **May be absent**. Fall back to the file extension. |
| `size` | int | Bytes. |
| `url` | string | |
| `proxy_url` | string | Docs say required, but "only images and videos have one". Use `#[serde(default)]`. |
| `height?`, `width?` | int, **optional AND nullable** | |
| `content_scan_version?` | int | |
| `placeholder_version?` | int | |
| `placeholder?` | string | Base64 thumbhash for a blur preview. |
| `ephemeral?` | bool | |
| `duration_secs?` | **float** | Voice messages. |
| `waveform?` | string | Base64 bytes, one amplitude 0–255 per sample. Voice messages only. |
| `flags?` | int | See below. |
| `clip_participants?`, `application?` | object (application is nullable) | |

- **Attachment flags:** `1<<0` IS_CLIP, `1<<1` IS_THUMBNAIL, `1<<2` IS_REMIX, `1<<3` **IS_SPOILER**, `1<<4` CONTAINS_EXPLICIT_MEDIA, `1<<5` IS_ANIMATED, `1<<6` CONTAINS_GORE_CONTENT, `1<<7` CONTAINS_SELF_HARM_CONTENT.
- **Spoiler:** treat as spoiler if `flags & 8 != 0` **OR** `filename.starts_with("SPOILER_")`. The filename prefix is how uploads are marked; Discord.Net and Discord4J define the `"SPOILER_"` constant. Render blurred until clicked.
- **URL signing:** attachment URLs are signed and expire, carrying `?ex=<hex>&is=<hex>&hm=<hex>`. Keep the full query string.
- **Resizing:** the media proxy (`https://media.discordapp.net/…`, same path) accepts `width`, `height` and `format` query parameters. Request `proxy_url?width=W&height=H` for thumbnails.

### 1.5 `interaction_metadata`

- **Fields:**
  - `id`
  - `type` (interaction type: 1=PING, 2=APPLICATION_COMMAND, 3=MESSAGE_COMPONENT, 4=AUTOCOMPLETE, 5=MODAL_SUBMIT; values are **UNCONFIRMED** from this fetch, standard API)
  - `name?` (only for APPLICATION_COMMAND; includes subcommands)
  - `command_type?` (1=CHAT_INPUT, 2=USER, 3=MESSAGE, 4=PRIMARY_ENTRY_POINT; **UNCONFIRMED** values)
  - `ephemerality_reason?`
  - `user` (partial user)
  - `authorizing_integration_owners`: **map whose keys are integer strings** (`"0"`, `"1"`) and whose values are snowflakes. Use `HashMap<String, String>`.
  - `original_response_message_id?`, `interacted_message_id?`, `triggering_interaction_metadata?` (recursive), `target_user?`, `target_message_id?`
- **Rendering** (wording **UNCONFIRMED**):
  - For type 20: above the bot message, show "{user} used **/{name}**".
  - For type 23: "{user} used **{name}**".
  - If `name` is missing (component or modal), show no header, or use the deprecated `interaction.name`.
  - With the EPHEMERAL flag: show "Only you can see this".

### 1.6 Reactions

Source: https://docs.discord.food/resources/message (Reaction Object)

```json
{"emoji":{"id":null,"name":"🔥"},"count":3,"count_details":{"burst":1,"normal":2},
 "me":true,"me_burst":false,"burst_colors":["#FF8800","#FFD000"]}
```

- `emoji.id` is null for unicode emoji.
- `emoji.name` **may be null** for a deleted custom emoji. Render it by `id`.
- `emoji.animated?` is optional.
- `burst_colors` holds hex strings and may be empty.
- Reaction type: 0 = NORMAL, 1 = BURST (super reaction).
- **Gateway events.**
  - MESSAGE_REACTION_ADD: `{user_id, channel_id, message_id, message_author_id, guild_id?, member?, emoji, type, burst_colors?}`. The official bot docs also list `burst: bool`. Accept both.
  - MESSAGE_REACTION_REMOVE: `{user_id, channel_id, message_id, guild_id?, emoji, type}`.
  - MESSAGE_REACTION_REMOVE_ALL: `{channel_id, message_id, guild_id?}`.
  - MESSAGE_REACTION_REMOVE_EMOJI: `{…, emoji}`.
  - MESSAGE_REACTION_ADD_MANY needs the `DEBOUNCE_MESSAGE_REACTIONS` capability. Shape: `{channel_id, message_id, guild_id?, reactions:[{users:[snowflake], emoji}]}`.
  - Apply events to the local cache: adjust `count`, `count_details.{normal|burst}`, and `me`/`me_burst` when `user_id == self`.
- The official client shows normal and burst counts of the same emoji as separate pills. **UNCONFIRMED.**
- MESSAGE_UPDATE carries the same extra fields as CREATE. Neither doc says whether it is full or partial, so **merge defensively** and do not wipe fields that are absent.
- MESSAGE_DELETE: `{id, channel_id, guild_id?}`. MESSAGE_DELETE_BULK: `{ids, channel_id, guild_id?}`.

### 1.7 Example MESSAGE_CREATE `d` (illustrative, fake IDs)

```json
{"id":"1100000000000000002","channel_id":"1100000000000000001","guild_id":"1100000000000000000",
 "channel_type":0,"type":19,"flags":0,"tts":false,"pinned":false,"nonce":"1100000000000000003",
 "content":"hi <@1100000000000000009> <:wave:1100000000000000010> <t:1618953630:R>",
 "timestamp":"2021-04-20T16:20:30.123000+00:00","edited_timestamp":null,
 "author":{"id":"1100000000000000008","username":"alice","discriminator":"0","global_name":"Alice","avatar":"a_0123abcd","public_flags":0},
 "member":{"nick":null,"avatar":null,"roles":["1100000000000000011"],"joined_at":"2020-01-01T00:00:00.000000+00:00","deaf":false,"mute":false,"flags":0},
 "mentions":[{"id":"1100000000000000009","username":"bob","discriminator":"0","global_name":null,"avatar":null,"member":{"nick":"Bobby","roles":[]}}],
 "mention_roles":[],"mention_everyone":false,"attachments":[],"embeds":[],"components":[],
 "message_reference":{"type":0,"channel_id":"1100000000000000001","guild_id":"1100000000000000000","message_id":"1100000000000000005"},
 "referenced_message":null}
```

`referenced_message: null` here means the original message was deleted.

### 1.8 Ping highlight for the local user

Highlight the message if any of these hold:
- `mentions` contains self, or
- `mention_everyone` is true, or
- `mention_roles` intersects your roles.

A reply with ping on puts the replied-to author into `mentions`. This is derived from the field semantics; the exact client rule is **UNCONFIRMED**.

---

## 2. Message types and system-message rendering

Source: https://docs.discord.food/resources/message (Message Type table, Rendered Content column)

- `{author}` is the author display name and `{guild}` is the guild name.
- `**…**` means bold.
- The `{…}`-template structure below comes from docs.discord.food. The exact English wording of each phrase is also from docs.discord.food, but I did not verify it character by character against the official client.

| # | name | render |
|---|---|---|
| 0 | DEFAULT | `{content}` |
| 1 | RECIPIENT_ADD | "{author} added {mentions[0]} to the {group/thread}." |
| 2 | RECIPIENT_REMOVE | "{author} removed {mentions[0]} from the {group/thread}." When author == mentions[0] the client says "left". **UNCONFIRMED** |
| 3 | CALL | If the local user is in `call.participants`: "{author} started a call{ended ? ` that lasted {duration}` : ` — Join the call`}." Otherwise: "You missed a call from {author} that lasted {duration}." Duration is `ended_timestamp − timestamp`. |
| 4 | CHANNEL_NAME_CHANGE | content non-empty: "{author} changed the {forum ? "post title" : "channel name"}: **{content}**". Empty: "{author} removed the custom group name." |
| 5 | CHANNEL_ICON_CHANGE | "{author} changed the channel icon." |
| 6 | CHANNEL_PINNED_MESSAGE | "{author} pinned a message to this channel." `message_reference` points to the pinned message. |
| 7 | USER_JOIN | Pick from 13 templates by `unix_ms(timestamp) % 13`. See the list after this table. |
| 8 | PREMIUM_GUILD_SUBSCRIPTION | "{author} just boosted the server{content ? ` **{content}** times`}!" `content` is the boost count as a string. |
| 9/10/11 | …TIER_1/2/3 | Same as 8, plus " {guild} has achieved **Level N!**" |
| 12 | CHANNEL_FOLLOW_ADD | "{author} has added {content} to this channel. Its most important updates will show up here." |
| 13 | (removed: GUILD_STREAM) | |
| 14–17 | GUILD_DISCOVERY_* | Fixed discovery-status text. |
| 18 | THREAD_CREATED | "{author} started a thread: **{content}**. See all threads." |
| 19 | REPLY | `{content}`, with the reply preview from `referenced_message`. |
| 20 | CHAT_INPUT_COMMAND | `{content}` plus the interaction header (§1.5). |
| 21 | THREAD_STARTER_MESSAGE | `referenced_message.content`. If missing: "Sorry, we couldn't load the first message in this thread". |
| 22 | GUILD_INVITE_REMINDER | "Wondering who to invite?\nStart by inviting anyone who can help you build the server!" |
| 23 | CONTEXT_MENU_COMMAND | `{content}` plus the header. |
| 24 | AUTO_MODERATION_ACTION | Special card from `embeds[0]` (`type:"auto_moderation_message"`; `fields` are key/value pairs). |
| 25 | ROLE_SUBSCRIPTION_PURCHASE | "{author} {is_renewal ? "renewed" : "joined"} **{tier_name}** and has been a subscriber of {guild} for {total_months_subscribed} month(s)!" |
| 26 | INTERACTION_PREMIUM_UPSELL | `{content}` |
| 27 | STAGE_START | "{author} started **{content}**" |
| 28 | STAGE_END | "{author} ended **{content}**" |
| 29 | STAGE_SPEAKER | "{author} is now a speaker." |
| 30 | STAGE_RAISE_HAND | "{author} requested to speak." |
| 31 | STAGE_TOPIC | "{author} changed the Stage topic: **{content}**" |
| 32 | GUILD_APPLICATION_PREMIUM_SUBSCRIPTION | "{author} upgraded {application ?? "a deleted application"} to premium for this server!" |
| 36 | GUILD_INCIDENT_ALERT_MODE_ENABLED | "{author} enabled security actions until {content}." |
| 37 | GUILD_INCIDENT_ALERT_MODE_DISABLED | "{author} disabled security actions." |
| 38 | GUILD_INCIDENT_REPORT_RAID | "{author} reported a raid in {guild}." |
| 39 | GUILD_INCIDENT_REPORT_FALSE_ALARM | "{author} reported a false alarm in {guild}." |
| 44 | PURCHASE_NOTIFICATION | "{author} has purchased {purchase_notification.guild_product_purchase.product_name}!" |
| 46 | POLL_RESULT | Special card from `embeds[0]` (`type:"poll_result"`). |
| 49 | CHANNEL_LINKED_TO_LOBBY | `{content}` |
| 65 | VOICE_SESSION | "{author} started a [voice hangout]({channel})." |
| 67 | FRIEND_REQUEST_ACCEPTED | "{author} accepted your friend request." |

Other values that exist: 35, 40–42, 47, 48, 50–55, 58–62, 64, 66, 68. Removed values: 13, 33, 34, 43, 45, 56, 57, 63. For an unknown type, render `content` if it is non-empty, otherwise "Unsupported message type N".

**USER_JOIN templates (index = `timestamp_ms % 13`):**

0. "{author} joined the party."
1. "{author} is here."
2. "Welcome, {author}. We hope you brought pizza."
3. "A wild {author} appeared."
4. "{author} just landed."
5. "{author} just slid into the server."
6. "{author} just showed up!"
7. "Welcome {author}. Say hi!"
8. "{author} hopped into the server."
9. "Everyone welcome {author}!"
10. "Glad you're here, {author}."
11. "Good to see you, {author}."
12. "Yay you made it, {author}!"

**Reply preview (type 19 / any `message_reference.type == 0` with `referenced_message`):**
- Show a small avatar, the name, and a single-line content preview.
- If `referenced_message` is `null`: show "Original message was deleted". **UNCONFIRMED** wording.
- If the key is absent: show "Message could not be loaded". **UNCONFIRMED** wording.

**Forward (`message_reference.type == 1`):** the outer message has `type` 0 and empty `content`. Render `message_snapshots[0].message` with a "Forwarded" label.
Rustcord parses a snapshot into a `Message` (`MessageSnapshot { message }`): the missing `id`, `author` and `channel_id` stay at their defaults, and the view borrows the forward's own id and author. Only the first snapshot is shown. A forward without a snapshot reads "This forwarded message can't be shown."

---

## 3. Embed object

Source: https://docs.discord.food/resources/message (Embed Object). Dissent client source: https://github.com/diamondburned/dissent/blob/main/internal/messages/embed.go and https://github.com/diamondburned/dissent/blob/main/internal/messages/content.go

**Every field is optional.**

| field | type | notes |
|---|---|---|
| `type?` | string | Default is `rich`. Values: `rich, image, video, gifv, link, article, gift, poll_result, post_preview, auto_moderation_message, auto_moderation_notification, safety_policy_notice, safety_system_notification, age_verification_system_notification, application_news(deprecated)`. **Use `String`** or an enum with `#[serde(other)] Unknown`. |
| `title?` | string | Max 256 chars. |
| `description?` | string | Max 4096. Markdown is allowed. |
| `url?` | string | Max 2048. |
| `timestamp?` | ISO8601 | |
| `color?` | int | `0xRRGGBB`. Render the left bar as `format!("#{:06X}", c)`. If absent, use the default gray bar. The client treats `0` as "no color". **UNCONFIRMED** |
| `footer?` | `{text, icon_url?, proxy_icon_url?}` | |
| `image?`, `thumbnail?`, `video?` | embed media | `{url, proxy_url?, height?, width?, flags?, description?, content_type?, placeholder?, placeholder_version?, content_scan_metadata?}` |
| `provider?` | `{name?, url?}` | |
| `author?` | `{name, url?, icon_url?, proxy_icon_url?}` | |
| `fields?` | `[{name, value, inline?: bool}]` | Max 25. `inline` defaults to false. |
| `reference_id?`, `content_scan_version?`, `flags?` | | Embed flags: `1<<4` explicit, `1<<5` content inventory entry, `1<<6` gore, `1<<7` self-harm. |

- Limits: 10 embeds per message, 6000 chars total across embeds.
- **Same-URL dedupe:** multiple embeds sharing the same `url` represent a gallery. The client renders their images as a grid inside the first embed.
- `SUPPRESS_EMBEDS` (flag bit 2) hides all embeds.
- The unfurler does not embed links written as `<https://…>`.

### Rendering by type (Dissent mirrors the official client)

- **`image`**
  - Render only the media, with no card. The image is in **`thumbnail`** (`proxy_url`, `width`, `height`), not in `image`.
  - **Hide the message content when `embeds.len() == 1 && embeds[0].type == "image" && embeds[0].url == content`.** Dissent's comment says "because that's what the official client does".
- **`gifv`** (Tenor/Giphy)
  - Render as **autoplaying, looping, muted video with no controls**, from `video.proxy_url` (mp4). Fall back to `thumbnail.proxy_url` (a static frame). No title or description card.
  - Hiding the content when it is exactly the gifv URL is **UNCONFIRMED**, but it is commonly observed.
- **`video`**
  - If `video.proxy_url` is present, play it inline.
  - YouTube-like embeds have `video.url = "https://www.youtube.com/embed/<id>"` and **no `proxy_url`**. Show `thumbnail` plus a play button that opens `url` externally.
  - Also show provider, author and title.
- **`article`**: card with a **large** thumbnail below the text.
- **`link`/`rich`**: card with a **small** thumbnail (about 80×80) at top-right. `image` renders full-width below.
  - Dissent rule: the thumbnail is "big" if the embed has no text body or the type is one of gifv/image/video/article.
- **Card layout:**
  1. color bar
  2. provider (small)
  3. author (icon + name, link)
  4. title (link to `url`)
  5. description (markdown)
  6. fields: inline fields go up to 3 per row, or 2 when there is a thumbnail. **UNCONFIRMED**
  7. image
  8. footer: `icon text • timestamp`

**Markdown in embeds:**
- `description` and `fields[].value` support full markdown, including masked links.
- `title`, `author.name` and `footer.text` are plain text, or at most limited inline formatting.
- `fields[].name` supports inline markdown but not links.
- This split is **UNCONFIRMED**. The ItzDerock parser enables `link` only in its "extended" mode, intended "for use in webhooks, embeds".

**gifv example** (field shape per the docs; values illustrative):

```json
{"type":"gifv","url":"https://tenor.com/view/x-123","provider":{"name":"Tenor","url":"https://tenor.co"},
 "thumbnail":{"url":"https://media.tenor.com/abc/x.png","proxy_url":"https://images-ext-1.discordapp.net/external/…/x.png","width":498,"height":280,"placeholder":"…","placeholder_version":1,"flags":0},
 "video":{"url":"https://media.tenor.com/abc/x.mp4","proxy_url":"https://images-ext-1.discordapp.net/external/…/x.mp4","width":498,"height":280,"placeholder":"…","placeholder_version":1,"flags":0},
 "content_scan_version":1}
```

**Stickers** (https://docs.discord.food/resources/sticker , https://docs.discord.food/reference):
- PNG/APNG: `https://media.discordapp.net/stickers/{id}.png`.
- GIF: `https://media.discordapp.net/stickers/{id}.gif` (not served from the cdn host).
- LOTTIE: `https://cdn.discordapp.com/stickers/{id}.json`.

**Played in the app** (src/media, docs/MEDIA_DESIGN.md): gifv `video.proxy_url` and `video`
embeds whose `video.proxy_url` is on a Discord media host (muted loop / inline player), and video
uploads from the attachment `url`. Requests carry no token, through the image client and host
list. UNCONFIRMED (no request was sent while building it): `Range` support (206) of
`cdn.discordapp.com` and `images-ext-*` (fallback: a 200 body is read whole up to 32 MiB, else
"Open in browser" / "Download"), and the proxy's still of a video upload at
`proxy_url?format=webp&quality=lossless&width=…&height=…` (fallback: black poster). YouTube and
other `video` embeds without a Discord proxy keep opening the browser.

---

## 4. Discord markdown: rules as implemented

Sources:
- dismark (claims "matching discord's parsing on desktop bug for bug", with regexes partly from discord/SimpleAST): https://github.com/jeremy-rifkin/dismark/blob/main/src/markdown.ts plus its `test/*.ts`
- simple-markdown defaults: https://github.com/ariabuckles/simple-markdown/blob/master/src/index.js
- ItzDerock/discord-markdown-parser: https://github.com/ItzDerock/discord-markdown-parser/tree/main/src
- Community guide (matthewzring): https://gist.github.com/matthewzring/9f7bbfd102003963f9be7dbcf7d40e51

The official support article (support.discord.com, 210298617) returned 403.

### 4.1 Parser model

- The parser walks the input left to right. At each position it tries the rules **in order** and takes the **first** match. Adjacent plain text is merged.
- Discord does **not** separate block and inline scope. Inline formatting can span newlines and code-fence delimiters. For example, in `*foo\n```\nbar*\n```` the italics match first.
- **The whole input is trimmed** before parsing.
- State flags:
  - `at_start_of_line`: true at the start of input and after any `\n`. Header, subtext, list and blockquote rules require it. Inline containers such as italics inherit it, so `*# foo*` gives italics(header).
  - `in_quote` and `in_link`.

**Rule order** (dismark; first match wins):
`escape, bold, underline, italics, strikethrough, spoiler, codeBlock, inlineCode, blockquote, subtext, header, maskedLink, list, text`.

Add Discord tokens before `text`: mentions, custom emoji, timestamps, autolink/url, unicode emoji. In simple-markdown and ItzDerock these have the same order as `strong`.

simple-markdown's own approach is different: `em`, `strong` and `u` share one order and the longest match wins by "quality" (`em`: len+0.2, `strong`: len+0.1, `u`: len). Dismark's fixed order reproduces the observed results.

### 4.2 Rules (regexes from dismark; `s` = dotAll)

| element | regex / rule | notes |
|---|---|---|
| escape | `^\\([^0-9A-Za-z\s])` | Emits the char literally. Inside a masked link's text, `\]` is not an escape. Escapes are evaluated in the link target. |
| **bold** | `^\*\*(.+?)\*\*(?!\*)` (s) | Inner text is parsed recursively. `foo ** a ** bar` **is** bold (spaces allowed). `**foo***` gives bold "foo*". |
| __underline__ | `^__(.+?)__(?!_)` (s) | |
| *italic* | `^\b_((?:__\|\\[\s\S]\|[^\\_])+?)_\b` **or** `^\*(?=\S)((?:\\\*\|\*\*\|\s+(?:[^*\s]\|\\\*\|\*\*)\|[^\s*])+?)(?<!\\)\*(?!\*)` | `_` needs **word boundaries** on both sides, so `snake_case_word` is not italic. `*` must be followed by non-space and closed by `*` preceded by non-space. `foo * a * bar` is not italic, `foo * a *bar` is not italic, `foo *a * bar` **is** italic. Edge cases: `***` gives literal `***`; `****` gives italic `**`; `*****` gives bold `*`. |
| ~~strike~~ | `^~~(.+?)~~` (s) | |
| \|\|spoiler\|\| | `^\|\|(.+?)\|\|` (s) | Content is parsed recursively and can contain code blocks. A spoiler inside a code block is literal. |
| code block | `^```(.+?)```` (s), and the inner text must contain a non-`\n` char | See the language rule after this table. |
| inline code | `^(``?)(.*?)\1` (s), and the inner text must contain a non-backtick char | Single or double backticks. Double backticks allow a literal `` ` `` inside: ``` ``a`b`` ```. Content is not parsed. It may span newlines. Also allowed inside headers, lists and quotes. |
| blockquote | `^(?: *>>> (.+)\| *>(?!>>) ([^\n]+\n?))` (s) | Only at line start, not inside a quote or link. **Requires a space after `>`** and non-empty text: `>foo` is plain, `> ` alone is plain, `>> foo` is plain. `>>> ` quotes **the rest of the message**. `> ` quotes one line; consecutive `> ` lines are separate nodes, so render them visually contiguous. **No nesting**: `> > foo` is a quote containing the text "> foo". Leading spaces are allowed. Quotes can sit inside list items and headers (`- > - foo`). |
| subtext | `^-# (?!-#) *([^\n]+\n?)` | Line start only. **No leading spaces allowed.** Needs a space. `-#foo` and `-# -# foo` are plain. Small, dim text. |
| header | `^ *(#{1,3}) (?!#) *([^\n]+\n?)` | Line start only; leading spaces allowed. 1–3 `#`, then **a mandatory space**. `####` is plain, `#foo` is plain, `# # foo` is plain. Content is trimmed and parsed inline. Ends at the newline. |
| masked link | `[text](url)` | See §4.3. |
| list | `^( *)([+*-]\|(\d+)\.)( +)([^\n]+(?:\n\1 [^\n]+)*\n?)` | Line start only. Markers `-`, `*`, `+` or `N.`, followed by ≥1 space. Continuation lines must be indented by (indent + 1) spaces. Consecutive matches append items. Ordered start number is `max(N, 1)` (`0.` gives 1; `-1. foo` is plain). Nesting: indent the child bullet by 2 spaces (`- foo\n  - bar`), or inline (`- - foo`, `1. 2. 3. foo`). Item bodies are parsed recursively. |
| newline | each `\n` is a **hard line break** | There are no paragraphs or soft wraps. The simple-markdown "two trailing spaces" `br` rule is irrelevant. |
| text | `^.+?(?=[^0-9A-Za-z\sÀ-￿]\|\n\|$)\n?` (s) | |

**Code block language rule:**
- Trim trailing `\n`.
- If there is more than one line **and** the first line matches `^([\w+\-.]+?)\s*$`, that line is the language and the rest is code. Otherwise everything is code.
- So ` ```cpp foo\nbar``` ` has no language.
- Code blocks are allowed mid-line: `foo```x```bar`.
- Content is literal: no mentions, emoji or markdown.
- Discord's original regex: `/^```(?:([\w+\-.]+?)?(\s*\n))?([^\n].*?)\n*```/s`.
- Highlighting uses highlight.js names and aliases (e.g. `rust`, `py`, `diff`, `ansi`); unknown languages render as plain.
- Quirk: ` ````foo```` ` renders as a block containing "`foo" followed by a literal `` ` ``.

### 4.3 Masked links `[text](url)`

Allowed in user messages (desktop, since the 2023 markdown update), bot and webhook content, and embed description and field values. The "since 2023" date is **UNCONFIRMED**; the parsing rules below come from the dismark tests.

- **The target must fully match** `^https?://[^\s<]+[^<.,:;"')\]\s]$`. `ftp://` and bare `google.com` are rejected and the whole thing stays plain text.
- `[foo](<https://x>)` is allowed: angle brackets are stripped. Whitespace inside the parentheses around the URL is allowed. `[foo] (url)` with a space is plain.
- **Anti-phishing:** if the link text contains a URL (`https?://…` matching the URL regex) or a ` ``` ` code block, the whole construct renders as **plain text**.
  - `[https://google.com](https://google.com)` is plain.
  - `[https://google](…)` is plain.
  - `[https://](…)` is OK.
  - `[google.com](https://google.com)` is OK.
- Link text is parsed inline (bold etc. allowed). Header, list and quote rules do not apply inside the link (`in_link`). Masked links cannot nest.
- The official client shows a "leaving Discord" confirmation with the real URL. **UNCONFIRMED** for third-party clients.

### 4.4 Autolinks and embed suppression

- **Bare URL:** `^(https?:\/\/[^\s<]+[^<.,:;"')\]\s])` (simple-markdown `url` rule). Trailing `. , : ; " ' ) ]` are excluded.
- **Angle autolink:** `^<([^: >]+:\/[^ >]+)>`. `<https://x>` renders as a link, and the **unfurler creates no embed** for it.
- Email `<user@domain>` (optional `mailto:`, with `?subject=&body=` headers) and phone `<+123…>` (optional `tel:`/`sms:`) are linkified (docs.discord.food reference).

### 4.5 Not supported (render literally)

Tables, images `![a](u)`, `####`+ headers, setext headers, horizontal rules, HTML, nested blockquotes, `~~~` fences, 4-space indented code, task lists, footnotes.

---

## 5. Inline tokens

Sources: https://docs.discord.food/reference#message-formatting , https://docs.discord.com/developers/reference#message-formatting , ItzDerock `src/utils/regex.ts`

| token | syntax | regex (ItzDerock, adjusted) | render |
|---|---|---|---|
| user | `<@ID>`, `<@!ID>` (deprecated; treat the same) | `^<@!?(\d+)>` | "@{display name}". Resolve from `mentions[]` and its `.member.nick`, then the cache. Unknown: "@unknown-user" (**UNCONFIRMED**). |
| role | `<@&ID>` | `^<@&(\d+)>` | "@{role.name}" tinted with the role color. Unknown: "@deleted-role" (**UNCONFIRMED**). |
| channel | `<#ID>` | `^<#(\d+)>` | "#{name}". Unknown: "#unknown" (**UNCONFIRMED**). |
| game profile | `<@$ID>` | `^<@\$(\d+)>` | Official docs only; render the raw text or a chip. |
| custom emoji | `<:name:ID>` / `<a:name:ID>` | `^<(a)?:(\w{2,32}):(\d+)>` | Image `https://cdn.discordapp.com/emojis/{ID}.webp?size=48`, plus `&animated=true` if `a`. |
| timestamp | `<t:UNIX>` / `<t:UNIX:S>` | `^<t:(-?\d+)(?::([tTdDfFsSR]))?>` | See the next table. An invalid style letter leaves plain text. |
| slash command | `</name:ID>`, `</name sub:ID>`, `</name group sub:ID>` | `^<\/([-_'\p{L}\p{N}\p{sc=Deva}\p{sc=Thai}]{1,32}(?: [same]{1,32}){0,2}):(\d+)>` (unicode) | Clickable "/name sub". |
| guild nav | `<id:customize>`, `<id:browse>`, `<id:guide>`, `<id:home>` (deprecated = guide), `<id:linked-roles>`, `<id:linked-roles:ROLE_ID>` | `^<id:(customize\|browse\|guide\|home\|linked-roles(?::\d+)?)>` | Labels "Channels & Roles", "Browse Channels", "Server Guide", "Linked Roles". Labels are **UNCONFIRMED**. ItzDerock's `<GUILDID:customize>` form is outdated. |
| @everyone / @here | literal `@everyone`, `@here` | `^@everyone`, `^@here` | Mention-styled. Whether it actually pinged comes from `mention_everyone`. |

Notes:
- Tokens match anywhere: `a<@1>b` works.
- Tokens are not parsed inside code. `\<@1>` escapes `<` and renders the raw text.
- Use `\d+` rather than fixed `{17,20}` digit counts. The libraries disagree on the counts; current snowflakes are 17–20 digits.

**Timestamp styles** (seconds; viewer's timezone and locale). The doc examples are en-GB; the en-US column is my own mapping and is **UNCONFIRMED**.

| style | meaning | docs example | en-US |
|---|---|---|---|
| `t` | short time | 16:20 | 4:20 PM |
| `T` | medium time | 16:20:30 | 4:20:30 PM |
| `d` | short date | 20/04/2021 | 04/20/2021 |
| `D` | long date | April 20, 2021 | April 20, 2021 |
| `f` (default) | long date + short time | April 20, 2021 at 16:20 | April 20, 2021 at 4:20 PM |
| `F` | full date + short time | Tuesday, April 20, 2021 at 16:20 | Tuesday, April 20, 2021 at 4:20 PM |
| `s` | short date + short time | 20/04/2021, 16:20 | |
| `S` | short date + medium time | 20/04/2021, 16:20:30 | |
| `R` | relative | "4 years ago" / "in 2 hours" | |

- `s` and `S` are new. Older parsers (ItzDerock) lack them.
- R must re-render periodically. Hover shows the full date (**UNCONFIRMED**).

---

## 6. Unicode emoji, shortcodes and jumbo sizing

Sources:
- https://docs.discord.food/reference ("Unicode Characters or `:NAME:`"; "Standard emoji are currently rendered using Twemoji for Desktop/Android and Apple's native emoji on iOS")
- https://docs.discord.food/resources/emoji
- twemoji `scripts/build.js`: https://github.com/jdecked/twemoji
- Dissent `content.go`

### Unicode emoji in content

- Content carries raw Unicode emoji. The client replaces each emoji sequence with a **Twemoji** image.
- **Filename rule** (twemoji `grabTheRightIcon`): if the sequence contains no U+200D (ZWJ), strip every U+FE0F. Then join the codepoints as lowercase hex with `-`.
  - Example: `❤️` → `2764`. A ZWJ family emoji keeps `fe0f` where present.
- **Asset URL:** `https://cdn.jsdelivr.net/gh/jdecked/twemoji@<ver>/assets/72x72/<cp>.png` or `…/assets/svg/<cp>.svg`. The current version is 17.0.3. The old twitter/maxcdn host is dead. Better: bundle the assets locally.

### Shortcodes

- **Composer:** the official client converts `:name:` to the Unicode character **before sending**. With skin tone, e.g. `:thumbsup::skin-tone-2:`, it appends the modifier. Custom emoji become `<:name:id>` / `<a:name:id>`; when the user cannot use one, the text stays `:name:`. Optionally, emoticons like `:)` convert when the user setting is on.
  - Status: **UNCONFIRMED** from docs. This is standard observed behavior: the server never converts.
  - The shortcode table is client-bundled; there is no official API. A third-party dataset is needed (e.g. emojibase or a Discord-specific emoji JSON). Exact name parity is **UNCONFIRMED**.
- **Received content:** received `content` may still contain `:name:` text from bots and webhooks. docs.discord.food lists `:NAME:` as a valid standard-emoji format (e.g. `:100:`). Render known shortcodes as emoji and leave unknown ones as text.

### Custom emoji

- CDN: `https://cdn.discordapp.com/emojis/{id}.webp?size=N`, plus `&animated=true` for `<a:…>` or `animated:true`. Always request `.webp`.
- In reactions, use `emoji.id`. `name` may be null.

### Jumbo emoji ("wumboji")

When the trimmed `content` consists **only** of emoji (custom tokens and Unicode emoji sequences) and whitespace, with **count ≤ 30**, render them large. Otherwise render inline.

- The 30 cap comes from third-party guides only (makeemoji "Discord emoji sizes" guide; an older MiniTool article says 27). Treat 30 as current and **UNCONFIRMED**.
- Sizes: inline about 1.375em (22 px), jumbo 48 px (3rem). Request `size=48` inline and `size=96` jumbo for HiDPI. **UNCONFIRMED.**
- Dissent shows large text only when the whole content is a single Unicode emoji.

---

## 7. Serde checklist (summary of breakage points)

- **Double options.**
  - `referenced_message`: absent / null / object.
  - `attachments[].width`/`height`: optional + nullable.
  - Nullable: `global_name`, `avatar`, `member.nick`, `member.avatar`, `edited_timestamp`, `call.ended_timestamp`, `emoji.id`, `emoji.name`, `attachment.application`.
- **Number or string:** `nonce`.
- **Map with integer-string keys:** `authorizing_integration_owners`.
- **Must not be closed enums:** `embed.type` and message `type` (unknown values appear regularly).
- **Defaults on absence:** `reactions` (absent = none), `components` (absent in snapshots), `proxy_url`, `content_type`, `flags`, `channel_type`.
- **Separate structs:** snapshot messages (no `id`/`author`), reply-nested messages (no `referenced_message`), and `member` without `user`.
- **Float, not int:** `duration_secs`.
- **Gateway-only fields:** `guild_id` and `member` are absent in REST history. Take `guild_id` from the channel cache instead.

---

**Not checked:**
- I could not fetch the official support markdown article (403), so the client's exact list numbering and limits, masked-link availability by surface, jumbo cap/sizes, fallback strings for unknown mentions and deleted replies, and the shortcode table are unconfirmed.
- I read Discord's markdown rules mainly from dismark (a third-party "bug-for-bug" reimplementation), not from Discord's own code.
- Neither doc says whether MESSAGE_UPDATE is a full or partial message. Merge updates into cached messages; never replace a cached message with one.


---

# Part 4: Gateway lifecycle

# Discord user-account Gateway lifecycle: implementation reference

**Sources.** I read the raw source of the current docs instead of relying on summaries: discord-userdoccers at master (last pushed 2026-11-01) and discord/discord-api-docs at main. I cross-checked against four open-source user clients: Abaddon, Discordo with its arikawa fork, and discord.py-self. Short names used below:
- **UD-gw** = https://docs.discord.food/gateway/using-gateway (raw: github.com/discord-userdoccers/discord-userdoccers/blob/master/pages/gateway/using-gateway.mdx)
- **UD-ev** = https://docs.discord.food/gateway/gateway-events (`pages/gateway/gateway-events.mdx`)
- **UD-cc** = https://docs.discord.food/gateway/opcodes-and-close-codes
- **UD-ref** = https://docs.discord.food/reference (client properties, ID serialization)
- **UD-rs** = https://docs.discord.food/topics/read-state
- **OFF-gw**, **OFF-ev**, **OFF-cc** = github.com/discord/discord-api-docs/blob/main/developers/events/gateway.mdx, `.../events/gateway-events.mdx`, `.../topics/opcodes-and-status-codes.mdx`
- **ABD** = github.com/uowuo/abaddon `src/discord/discord.cpp` and `objects.cpp`
- **DCO** = github.com/ayn2op/discordo `internal/ui/chat/model.go` and `internal/http/properties.go`
- **ARF** = github.com/ayn2op/arikawa (v3) `gateway/events.go` and `discord/channel.go`
- **DPS** = github.com/dolfies/discord.py-self `discord/gateway.py`, `state.py`, `flags.py`, `client.py`, `backoff.py`, `utils.py`, `types/gateway.py`

---

## 1. URL, query params, zlib-stream (UD-gw "Connecting", "Transport Compression"; OFF-gw; ABD; DPS utils.py)

**Getting the URL**
- `GET https://discord.com/api/v9/gateway` (unauthenticated) returns `{"url":"wss://gateway.discord.gg"}`.
- Cache it. Only fetch it again after a connection attempt fails.

**Connect URL:** `wss://gateway.discord.gg/?v=9&encoding=json&compress=zlib-stream`

| param | type | values |
|---|---|---|
| `v` | int | `9`. UD-ref lists API v9 as the default for the client; 10 is also available. |
| `encoding` | string | `json` or `etf` |
| `compress?` | string | `zlib-stream` or `zstd-stream` (optional) |

- When resuming, append the **same query string** to `resume_gateway_url`, for example `wss://gateway-us-east1-b.discord.gg/?v=9&encoding=json&compress=zlib-stream`. OFF-gw: "with the same version and encoding" / "same query parameters".

**zlib-stream framing**
- Use one inflate context per WebSocket connection, shared by every message on that connection. Create a fresh context for every new connection, including resume connections. ABD resets it with `inflateInit2` on reconnect.
- Server messages are compressed bytes, so they arrive as WebSocket **Binary** messages (`tungstenite::Message::Binary`).
- What you send stays plain JSON text frames.
- Buffer incoming bytes until the buffer ends with `00 00 ff ff` (the Z_SYNC_FLUSH suffix). Then inflate the whole buffer with a sync flush and clear it. The result is UTF-8 JSON.
- The stream has a zlib (RFC1950) wrapper, not raw deflate. DPS uses `zlib.decompressobj()`, whose default expects a zlib header. ABD uses auto-detect (`MAX_WBITS+32`). With flate2 this means `Decompress::new(true)`.
- READY needs an output loop: ABD grows its output buffer and loops.
- `zstd-stream`: one decompression context for the whole connection lifetime. Each WebSocket message is one complete Gateway message but does **not** end a zstd frame, so call `ZSTD_decompressStream` until the input is consumed.

**`compress` in Identify**
- It controls legacy per-payload compression (default `false`). UD-gw: "Payload compression will be disabled if you use transport compression", and the two can't be combined.
- **Send `"compress": false`** (or omit it) when you use zlib-stream. The docs' own Identify example, ABD and DPS (`'compress': not self._transport_compression`) all do this.

**Rust sketch** (untested; needs the `flate2` crate, which you don't have yet):
```rust
use flate2::{Decompress, FlushDecompress, Status};
pub struct ZlibStream { z: Decompress, inbuf: Vec<u8>, out: Vec<u8> }
impl ZlibStream {
    pub fn new() -> Self { Self { z: Decompress::new(true), inbuf: Vec::new(), out: Vec::new() } } // one per WS connection
    pub fn push(&mut self, frame: &[u8]) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
        self.inbuf.extend_from_slice(frame);
        if !self.inbuf.ends_with(&[0x00, 0x00, 0xff, 0xff]) { return Ok(None); }
        self.out.clear();
        let mut pos = 0;
        loop {
            if self.out.capacity() - self.out.len() < 32 * 1024 { self.out.reserve(64 * 1024); }
            let before = self.z.total_in();
            let st = self.z.decompress_vec(&self.inbuf[pos..], &mut self.out, FlushDecompress::Sync)?;
            pos += (self.z.total_in() - before) as usize;
            // done once all input is consumed and the decoder left spare output room (nothing pending)
            if (pos == self.inbuf.len() && self.out.len() < self.out.capacity()) || st == Status::StreamEnd { break; }
        }
        self.inbuf.clear();
        Ok(Some(String::from_utf8(std::mem::take(&mut self.out))?))
    }
}
```

---

## 2. Opcodes, Hello, heartbeat (UD-cc; UD-gw "Sending Heartbeats"; UD-ev; OFF-gw)

**Gateway payload envelope**
```json
{"op": 0, "d": {}, "s": 42, "t": "EVENT_NAME"}
```
- `s` and `t` are `null` when `op != 0`. Treat them as optional.
- `d` can be an object, `null` (op 7, op 11), a `bool` (op 9) or an `int`/`null` (op 1).
- Cache the most recent non-null `s`.

| op | name | dir | notes |
|---|---|---|---|
| 0 | Dispatch | recv | |
| 1 | Heartbeat | send/recv | `d` = last seq or `null` |
| 2 | Identify | send | |
| 3 | Presence Update | send | limit 5 per 20 s |
| 4 | Voice State Update | send | |
| 5 | Voice Server Ping | send | |
| 6 | Resume | send | |
| 7 | Reconnect | recv | |
| 8 | Request Guild Members | send | |
| 9 | Invalid Session | recv | |
| 10 | Hello | recv | |
| 11 | Heartbeat ACK | recv | |
| 12 | Guild Sync | — | removed |
| 13 | Call Connect | send | |
| 14 | Guild Subscriptions | send | deprecated |
| 15, 16 | Lobby Connect / Disconnect | — | removed |
| 17 | Lobby Voice States | send | |
| 18–22 | Stream Create / Delete / Watch / Ping / Set Paused | send | |
| 23–27 | | — | removed |
| 28 | Request Forum Unreads | send | |
| 29 | Remote Command | send | |
| 30 | Request Deleted Entity IDs | send | |
| 31 | Request Soundboard Sounds | send | |
| 32, 33 | Speed Test Create / Delete | send | deprecated |
| 34 | Request Last Messages | send | |
| 35 | Search Recent Members | send | |
| 36 | Request Channel Statuses | send | deprecated, superseded by 43 |
| 37 | Guild Subscriptions Bulk | send | |
| 38 | Guild Channels Resync | send | |
| 39 | Request Channel Member Count | send | |
| 40 | QoS Heartbeat | send | |
| 41 | Update Time Spent Session ID | send | |
| 42 | Lobby Voice Server Ping | send | |
| 43 | Request Channel Info | send | |

**Hello (op 10)**
```json
{"op":10,"d":{"heartbeat_interval":41250,"_trace":["[\"gateway-prd-us-east1-c-6w69\",{\"micros\":0.0}]"]},"s":null,"t":null}
```

**Heartbeat timing**
- After Hello, wait `heartbeat_interval * rand(0..1)` (jitter), then send the first heartbeat. After that, send one every `heartbeat_interval` ms.
- Payload: `{"op":1,"d":<last_s or null>}`.
- Built-in tolerance covers latency, so don't send early. Identify may be sent without waiting for Hello.

**Heartbeat ACK:** `{"op":11,"d":null,"s":null,"t":null}`.

**Zombie connection**
- If no ACK arrives before the next heartbeat is due, close with **any code except 1000 or 1001**. DPS uses 4000; ABD uses 1012.
- Then reconnect to `resume_gateway_url` and Resume.

**Server-requested heartbeat:** when you receive op 1, send op 1 immediately without waiting for the interval. The server replies with op 11. Whether to reset your interval timer afterwards is UNCONFIRMED; either choice is harmless.

**Optional heartbeats (UD-ev)**
- QoS heartbeat (op 40), recommended but not required: `{"op":40,"d":{"seq":1337,"qos":{"ver":31,"active":true,"reasons":["foregrounded"]}}}`.
- Op 41 on connect and every 30 min (recommended): `{"op":41,"d":{"initialization_timestamp":<ms>,"session_id":"<uuid = client_heartbeat_session_id>","client_launch_id":"<uuid>"}}`.
- DPS sends op 41 plus a heartbeat right at READY, then an initial empty op 4. DPS says this mirrors the official client (UNCONFIRMED beyond DPS).

**During an outage:** keep heartbeating. If Identify or Resume gets op 9, keep the connection open and Identify again (UD-gw).

---

## 3. IDENTIFY for a user account (UD-ev "Identify"; UD-ref "Client Properties"; ABD; DCO; DPS)

**Identify fields**

| field | type | notes |
|---|---|---|
| `token` | string | raw user token, no prefix |
| `properties` | object | client properties, see below |
| `compress?` | bool | default `false`; send `false` |
| `large_threshold?` | int | 25–250. Default for users is 250: "stop sending non-stateful events for guilds without a subscription". |
| `shard?` | [int, int] | never needed for users |
| `presence?` | update-presence object | for users, send `status:"unknown"` with empty activities unless re-identifying with the last known presence |
| `intents?` | int | **omit** for users ("not recommended"); users get every intent by default |
| `capabilities?` | int | see §4 |
| `client_state?` | object | see below |

**`client_state`**
- `guild_versions` (`map<snowflake,int>`, required inside the object)
- `highest_last_message_id?`
- `read_state_version?` (int)
- `user_guild_settings_version?` (int)
- `user_settings_version?` (int)
- `private_channels_version?` (snowflake)
- `api_code_version?` (int)
- `initial_guild_id?`

A fresh client sends `{"guild_versions":{}}`; the docs example adds `"api_code_version":0`. ABD still sends an older shape (`guild_hashes`, …) and it works.

**Client properties: what is required**
- UD-ref: "no field is truly required" and types are not verified.
- Observed in every official client: `os`, `browser`, `browser_user_agent`, `browser_version`, `client_build_number` (int), `release_channel`, `system_locale`.
- Used to gate experiments: `os`, `client_build_number`, `release_channel`, `client_version`.
- Optional: `os_version`, `device`, `referrer`, `referring_domain`, `referrer_current`, `referring_domain_current`, `client_event_source` (?string, `null`), `has_client_mods` (bool), `client_launch_id` (uuid), `client_heartbeat_session_id` (uuid, rotates every 30 min), `launch_signature` (uuid with "mod" bits masked), `client_app_state`, and the `utm_*` / `search_engine*` / `mp_keyword*` fields.
- Gateway-only (not sent in `X-Super-Properties`): `installation_id`, `is_fast_connect`, `version`.
- Send the same object, base64-encoded, as the `X-Super-Properties` HTTP header. `browser_user_agent` should match your HTTP `User-Agent`.

**Web client properties** (UD-ref "Example Client Properties (Web)"):
```json
{"os":"Windows","browser":"Chrome","device":"","system_locale":"en-US",
 "browser_user_agent":"Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36",
 "browser_version":"136.0.0.0","os_version":"10","referrer":"https://www.reddit.com/","referring_domain":"www.reddit.com",
 "referrer_current":"https://www.google.com/","referring_domain_current":"www.google.com","search_engine_current":"google",
 "mp_keyword_current":"discord","release_channel":"stable","client_build_number":396858,"client_event_source":null,
 "has_client_mods":false,"client_launch_id":"<uuid>","launch_signature":"<uuid>","client_heartbeat_session_id":"<uuid>"}
```
- DCO sends the same keys with empty referrers, plus `is_fast_connect:true` in Identify only.
- `client_build_number` must be recent: the docs warn that many experimental features need a recent build number. How to obtain it is out of scope.

**Full web-style Identify** (UD-ev example, token placeholder):
```json
{"op":2,"d":{"token":"<TOKEN>","properties":{...},"compress":false,
 "presence":{"activities":[],"status":"unknown","since":0,"afk":false},
 "capabilities":1734653,"client_state":{"guild_versions":{},"api_code_version":0}}}
```
- `1734653` decodes to bits 0, 2–10, 12–14, **17, 19, 20**. Bits 17, 19 and 20 are not in the documented capability table: UNCONFIRMED and undocumented.
- Whether today's web client uses zlib-stream or zstd-stream: UNCONFIRMED. DPS prefers zstd when available.

---

## 4. Capabilities (UD-gw "Gateway Capabilities"; UD-ev Ready footnotes)

| bit | value | name | effect on payloads |
|---|---|---|---|
| 0 | 1 | LAZY_USER_NOTES | READY has no `notes` |
| 1 | 2 | NO_AFFINE_USER_IDS | no member/presence sync or PRESENCE_UPDATE for implicit relationships |
| 2 | 4 | VERSIONED_READ_STATES | `read_state` becomes `{entries,partial,version}` instead of an array |
| 3 | 8 | VERSIONED_USER_GUILD_SETTINGS | `user_guild_settings` becomes `{entries,partial,version}` |
| 4 | 16 | DEDUPE_USER_OBJECTS | dedupes READY (details below) |
| 5 | 32 | PRIORITIZED_READY_PAYLOAD | splits READY into READY + READY_SUPPLEMENTAL; **ignored without bit 4** |
| 6 | 64 | MULTIPLE_GUILD_EXPERIMENT_POPULATIONS | `guild_experiments[].populations` becomes an array |
| 7 | 128 | NON_CHANNEL_READ_STATES | read states of other `read_state_type`s appear |
| 8 | 256 | AUTH_TOKEN_REFRESH | READY may carry `auth_token`; swap it in |
| 9 | 512 | USER_SETTINGS_PROTO | removes JSON `user_settings` and USER_SETTINGS_UPDATE; you get base64 protobuf `user_settings_proto` instead |
| 10 | 1024 | CLIENT_STATE_V2 | guild objects nest REST fields under `properties` and gain `data_mode` |
| 11 | 2048 | PASSIVE_GUILD_UPDATE | PASSIVE_UPDATE_V1 replaces CHANNEL_UNREAD_UPDATE and VOICE_STATE_UPDATE for unsubscribed guilds |
| 12 | 4096 | AUTO_CALL_CONNECT | CALL_CREATE for existing calls on connect |
| 13 | 8192 | DEBOUNCE_MESSAGE_REACTIONS | MESSAGE_REACTION_ADD_MANY replaces bursts of MESSAGE_REACTION_ADD |
| 14 | 16384 | PASSIVE_GUILD_UPDATE_V2 | PASSIVE_UPDATE_V2; supersedes bit 11 |
| 15 | 32768 | CHANNEL_OBFUSCATION | obfuscates channels you can't view |
| 16 | 65536 | AUTO_LOBBY_CONNECT | READY gets `lobbies`; no LOBBY_CREATE stream on connect |

**DEDUPE_USER_OBJECTS (bit 4) in detail**
- Every user object in READY moves to top-level `users`; an ID is left in its place.
- Members carry `user_id` instead of `user`.
- Private channels carry `recipient_ids` instead of `recipients`.
- Relationships carry `user_id` instead of `user` (DPS: `relationship.pop('user_id')`).
- Per-guild `members` collapse into top-level `merged_members[][]`.
- Top-level and per-guild `presences` become `merged_presences{friends,guilds[][]}`, whose entries carry `user_id`.

**PRIORITIZED_READY_PAYLOAD (bit 5) in detail**
- READY's `merged_members[i]` holds **only your own member** for each guild.
- The other members, `merged_presences`, and per-guild `voice_states` / `activity_instances` arrive in READY_SUPPLEMENTAL.

**What open-source user clients send**
- ABD: `4605` (bits 0, 2–8, 12).
- DCO: `8701` (bits 0, 2–8, 13).
- DPS: bits 0, 2–10, 12, 14.

**Recommendation: `509` (0x1FD)** = LAZY_USER_NOTES | VERSIONED_READ_STATES | VERSIONED_USER_GUILD_SETTINGS | DEDUPE_USER_OBJECTS | PRIORITIZED_READY_PAYLOAD | MULTIPLE_GUILD_EXPERIMENT_POPULATIONS | NON_CHANNEL_READ_STATES | AUTH_TOKEN_REFRESH.
- This is exactly the bits all three clients have in common.
- It skips CLIENT_STATE_V2, so guild fields stay flat.
- It skips USER_SETTINGS_PROTO, so no protobuf. Whether the deprecated JSON `user_settings` is still fully populated is UNCONFIRMED.
- It skips the passive-update bits, so you get plain CHANNEL_UNREAD_UPDATE.
- It skips debounce, so you don't need to handle ADD_MANY.
- Cost: you must hydrate `users`, merge READY_SUPPLEMENTAL, and persist `auth_token` if it appears. If you want strictly the fewest event types, drop bit 8: `253`.

---

## 5. READY / READY_SUPPLEMENTAL (UD-ev "Ready", "Gateway Guild Object", "Ready Supplemental"; DPS state.py `parse_ready`/`parse_ready_supplemental`; ARF events.go)

**READY fields a chat client needs**
- `v` (int)
- `user`: full private user with `email`, `verified`, `mfa_enabled`, `premium_type`, `flags`, etc. Not deduped.
- `session_id` (string), `resume_gateway_url` (string), `session_type` (`"normal"`)
- `guilds[]`, `merged_members?`, `private_channels[]`, `users?`, `relationships[]`
- `read_state`, `user_guild_settings`, `sessions[]`
- `presences?` (only without DEDUPE)
- `auth_token?` (bit 8)
- `notes?` (absent with bit 0)
- `user_settings?` (absent with bit 9), `user_settings_proto?`
- `country_code`, `geo_ordered_rtc_regions`, `analytics_token`, `auth_session_id_hash`, `api_code_version`
- `required_action?`, `tutorial` (?obj), `connected_accounts`, `guild_join_requests`, `friend_suggestion_count?`
- `experiments`, `guild_experiments`, `explicit_content_scan_version`, `_trace`

**`guilds[]` element: three shapes**

(a) Without CLIENT_STATE_V2: a flat REST guild object (`id`, `name`, `icon`, `owner_id`, `features`, …) plus these gateway extras:
- `joined_at`, `large`, `member_count`
- `channels[]`
- `threads[]` (users only get **joined** threads)
- `roles[]`, `emojis[]`, `stickers[]`
- `stage_instances[]`, `guild_scheduled_events[]`, `premium_subscription_count`
- `voice_states[]` and `activity_instances[]` (omitted with bit 5)
- `members[]` and `presences[]` (only without DEDUPE; for users these hold self, voice users, friends/implicit relationships and open-DM users; guilds over 75k members get self and voice users only)
- `soundboard_sounds`

(b) With CLIENT_STATE_V2:
- REST fields move into `properties` ("an otherwise-normal guild object that is missing the below fields").
- `roles`, `emojis`, `stickers`, `channels`, `member_count`, … stay top-level.
- Adds `data_mode` (`"full"` | `"partial"` | `"unavailable"`), `partial_updates?` and `channel_updates?`.
- Cached sub-arrays are omitted when your version is current.
- `soundboard_sounds` is omitted.
- DPS and ARF both flatten: `guild.update(guild.pop('properties'))`.

(c) Unavailable: `{"id":"…","unavailable":true}`. Geo-restricted: `{"id","unavailable":true,"geo_restricted":true,"name","icon"}`. User guilds start out available; only outage or geo-restricted guilds look like this.

**`merged_members`**
- Type: `array<array<member>>`, index-aligned with `guilds[]`.
- Members have `user_id` and no `user`.
- Whether an unavailable guild gets an empty `[]` slot is UNCONFIRMED. Index-align defensively.

**`private_channels[]`**
- Channel objects with `type` 1 (DM) or 3 (GROUP_DM): `id`, `last_message_id` (?snowflake), `flags`, `is_spam?`, `is_message_request?`, `is_message_request_timestamp?`, `safety_warnings?`.
- Group DMs add `name`, `icon`, `owner_id`, `nicks?`.
- Recipients come as `recipients[]` (full users) **or** `recipient_ids[]` (with DEDUPE); recipients exclude yourself.
- With `private_channels_version` in `client_state` plus bit 5, only changed channels appear; the rest go to Supplemental `lazy_private_channels`.

**`users[]`:** partial users (`id`, `username`, `discriminator` (`"0"` for migrated users), `global_name?`, `avatar?`, `avatar_decoration_data?`, `public_flags`, `bot?`, …).

**`relationships[]`**
- `{id (= target user id), type, nickname (?string), since?, user_ignored, is_spam_request?, stranger_request?, user | user_id}`.
- Types: 0 NONE, 1 FRIEND, 2 BLOCKED, 3 INCOMING_REQUEST, 4 OUTGOING_REQUEST, 5 IMPLICIT.

**`read_state`** (UD-rs)
- An array, or `{"entries":[…],"partial":bool,"version":int}` with bit 2.
- Entry: `id`, `read_state_type?` (default 0 = CHANNEL; 1 GUILD_EVENT, 2 NOTIFICATION_CENTER, 3 GUILD_HOME, 4 GUILD_ONBOARDING_QUESTION, 5 MESSAGE_REQUESTS), `last_message_id?` **xor** `last_acked_id?`, `mention_count?` **xor** `badge_count?`, `last_pin_timestamp?`, `flags?` (1 IS_GUILD_CHANNEL, 2 IS_THREAD, 4 IS_MENTION_LOW_IMPORTANCE), `last_viewed?` (?int, days since the Discord epoch).
- Ignore unknown types.

**`user_guild_settings`**
- An array, or the versioned object with bit 3.
- Entry: `guild_id` (**null** means your DM settings), `channel_overrides[] {channel_id, collapsed, flags?, message_notifications, muted, mute_config}`, `flags`, `hide_muted_channels`, `message_notifications`, `mobile_push`, `mute_scheduled_events`, `muted`, `mute_config ?{end_time ?ts, selected_time_window int (-1 = indefinite)}`, `notify_highlights`, `suppress_everyone`, `suppress_roles`, `version`.

**`sessions[]`:** `{session_id, client_info{client,os,version}, status, activities, hidden_activities, active?}`. DPS: an aggregate session with `session_id == "all"` exists, and at most 15 sessions are listed.

**READY_SUPPLEMENTAL (only with bit 5)**
- `guilds[] {id, voice_states, activity_instances}` (unavailable guilds have only `id`)
- `merged_members[][]` (members other than you)
- `merged_presences {friends[], guilds[][]}`; entries are presences with `user_id`, and friends entries also have `last_modified` (ms)
- `lazy_private_channels[]`
- `disclose[]`
- `game_invites[]`

DPS `zip`s Ready `guilds`, Supplemental `guilds`, Supplemental `merged_members`, Ready `merged_members` and `merged_presences.guilds` **by index**. Relying on that alignment is confirmed by implementation, not by doc text.

**Example READY with caps 509** (illustrative, put together from the docs' field lists, not a captured payload):
```json
{"op":0,"t":"READY","s":1,"d":{"v":9,"user":{"id":"1","username":"me","discriminator":"0","global_name":null,"avatar":null,"verified":true,"email":null,"mfa_enabled":false,"premium_type":0,"flags":0},
 "session_id":"30f32c5d…","resume_gateway_url":"wss://gateway-us-east1-b.discord.gg","session_type":"normal",
 "guilds":[{"id":"10","name":"G","icon":null,"owner_id":"2","joined_at":"2024-01-01T00:00:00+00:00","large":false,"member_count":42,"channels":[],"threads":[],"roles":[],"emojis":[],"stickers":[],"premium_subscription_count":0},{"id":"11","unavailable":true}],
 "merged_members":[[{"user_id":"1","roles":[],"joined_at":"2024-01-01T00:00:00+00:00","nick":null,"avatar":null,"premium_since":null,"deaf":false,"mute":false,"pending":false,"flags":0,"communication_disabled_until":null}],[]],
 "private_channels":[{"id":"20","type":1,"last_message_id":"99","recipient_ids":["2"],"flags":0}],
 "users":[{"id":"2","username":"friend","discriminator":"0","global_name":null,"avatar":null,"public_flags":0}],
 "relationships":[{"id":"2","type":1,"user_id":"2","nickname":null}],
 "read_state":{"version":123,"partial":false,"entries":[{"id":"20","last_message_id":"98","mention_count":0,"flags":0,"last_viewed":3500}]},
 "user_guild_settings":{"version":45,"partial":false,"entries":[{"guild_id":null,"muted":false,"message_notifications":0,"channel_overrides":[],"flags":0,"hide_muted_channels":false,"mobile_push":true,"mute_scheduled_events":false,"mute_config":null,"notify_highlights":0,"suppress_everyone":false,"suppress_roles":false,"version":1}]},
 "sessions":[]}}
```

**Serde helpers**
```rust
#[derive(Deserialize)] #[serde(untagged)]
enum MaybeVersioned<T> { Versioned { entries: Vec<T>, #[serde(default)] partial: bool, version: i64 }, Plain(Vec<T>) }
// snowflakes: accept "123" or 123 (UD-ref "ID Serialization": e.g. GUILD_MEMBERS_CHUNK.not_found echoes numbers)
fn de_sf<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)] #[serde(untagged)] enum S { N(u64), S(String) }
    match S::deserialize(d)? { S::N(n) => Ok(n), S::S(s) => s.parse().map_err(serde::de::Error::custom) }
}
```

---

## 6. Resume, Resumed, Reconnect, Invalid Session (UD-gw "Resuming"; UD-ev; OFF-gw; DPS; ABD)

**Resume (op 6)**
```json
{"op":6,"d":{"token":"<TOKEN>","session_id":"<READY.session_id>","seq":<last non-null s>}}
```
- Open `resume_gateway_url` with the original query string and a new zlib context.
- Wait for Hello, start heartbeating, then send op 6. Do **not** Identify.
- Using the original URL instead "will experience disconnects at a higher rate".

**RESUMED (dispatch)**
- `d = {"_trace":[…]}`.
- Missed dispatches are replayed first, in order, with their `s`. RESUMED marks the end of the replay.
- A replay buffer overflow leads to op 9 instead.

**RECONNECT (op 7)**
- `{"op":7,"d":null}`, usually sent during deploys.
- Close with a non-1000/1001 code (ABD: 1012), reconnect to `resume_gateway_url`, then Resume.

**INVALID SESSION (op 9)**
- `d` is a bool meaning "may be resumable". It can answer Identify (for example, Identify concurrency was exceeded), answer Resume, or arrive unprompted.
- It does **not** by itself mean you need a new WebSocket; you can continue on the same connection.
- `d:false`: clear `session_id`, `seq` and resume URL, then Identify. DPS closes with 1000 and reconnects to the default URL to Identify.
- `d:true`: Resume.
- **Wait time:** legacy official docs said to wait a random **1–5 s** before re-identifying; the current docs dropped this. Source: https://color-tweaks--hackweek-discord-api-docs.netlify.app/docs/topics/gateway. ABD waits 1 s.
- Recommendation: random 1–5 s. Because users have `max_concurrency` = 1 per 5 s, wait at least 5 s if the op 9 answered your own Identify.

**Your own close codes:** closing with **1000 or 1001** invalidates the session and makes you appear offline. Use 1000 only on logout or quit. Any other code, or just dropping TCP, keeps the session resumable for "a few minutes".

---

## 7. Close codes (UD-cc; reconnect column from OFF-cc; resume vs identify split derived from the explanations and DPS `_can_handle_close`)

| code | meaning | action |
|---|---|---|
| 1000 / 1001 from the server (not initiated by you) | | DPS treats this as an "improper close": reconnect and try Resume |
| 1006 / no close frame / TCP reset | | Resume |
| 4000 | Unknown error | Resume |
| 4001 | Unknown opcode | reconnect, Resume (your bug) |
| 4002 | Decode error, or a payload over 15 KiB | reconnect, Resume (your bug) |
| 4003 | Not authenticated / session invalidated | Re-identify |
| 4004 | Authentication failed | **Fatal**: token invalid; stop and ask for a new login |
| 4005 | Already authenticated | Resume (your bug) |
| 4006 | (struck through in UD; voice only) | treat as Re-identify |
| 4007 | Invalid `seq` | Re-identify |
| 4008 | Rate limited | back off, then Resume |
| 4009 | Session timed out | Re-identify |
| 4010 | Invalid shard | Fatal |
| 4011 | Sharding required | Fatal |
| 4012 | Invalid API version | Fatal |
| 4013 | Invalid intent(s) | Fatal |
| 4014 | Disallowed intent(s) | Fatal |
| 4015 | Too many sessions (user limit: 50 active sessions) | Fatal / long backoff (only in UD) |
| 4016 | Console connection request canceled | Fatal (only in UD) |

- DPS never reconnects on 1000 (when it closed first), 4004, 4010, 4011, 4012, 4013 or 4014.
- For any other code it always attempts **RESUME first** and lets the Gateway send op 9 if the session can't be resumed. DPS says "this is apparently what the official Discord client does".

---

## 8. Rate limits and backoff (UD-gw "Rate Limiting", "Sharding", "Identifying"; UD-ev; DPS backoff.py)

- **Sending:** 120 events per connection per 60 s (2/s on average). Going over means **immediate disconnect**; repeat offenders lose API access.
- Whether heartbeats count toward the 120 is UNCONFIRMED. Budget for them anyway.
- **Outbound payload size:** 15 KiB max, otherwise close 4002.
- **Presence (op 3):** 5 per 20 s. UD also notes incoming events pause until the presence propagates.
- **Op 8:** 1 per guild per session per 30 s; exceeding it gets a `RATE_LIMITED` dispatch `{opcode, retry_after (float s), meta{guild_id, nonce?}}`.
- **Identify:** users always have `max_concurrency` = 1 (one Identify per 5 s) and **no** daily session-start limit (the 1000/day limit is bots only). Exceeding concurrency gets op 9.
- **Backoff:** DPS uses exponential backoff with full jitter: `uniform(0, base * 2^exp)`, `base = 1 s`, exp capped at 10 (≤ 1024 s), reset after 2048 s without retries.
  - My recommendation for a desktop client (not from a source): use that but cap at about 60 s.
  - Reset the backoff after READY or RESUMED.
  - Never Identify more often than once per 5 s.

---

## 9. Dispatch events for a text client (UD-ev; OFF-ev where noted; ABD; DPS)

**Large guilds / subscriptions (DPS state.py `GuildSubscriptions` docstring; not documented in UD)**
- You are auto-subscribed to guilds under 75k members at connect.
- For unsubscribed guilds you get **no** MESSAGE_CREATE / UPDATE / DELETE, only CHANNEL_UNREAD_UPDATE or PASSIVE_UPDATE.
- UD's `large_threshold` text (default 250 for users) describes the same mechanism.
- Subscribe to the guild you're viewing with op 37: `{"op":37,"d":{"subscriptions":{"<guild_id>":{"typing":true,"threads":true,"activities":true,"member_updates":false,"members":[],"channels":{"<channel_id>":[[0,99]]},"thread_member_lists":[]}}}}`. Payload shape is from DPS types and is UNCONFIRMED in UD.
- The first subscribe can trigger a GUILD_CREATE, so handle GUILD_CREATE idempotently.

**MESSAGE_CREATE**
- A message object plus `channel_type` (int; UD lists it as present, OFF as optional).
- `guild_id?`: absent in DMs and in ephemeral messages.
- `member?`: partial member **without `user`**; absent in DMs, ephemeral messages and webhook messages.
- `mentions[]`: users, each with an optional partial `member`.
- `metadata?` and `moderation_metadata?`: `map<string,string>`.
- Guild messages: yes, `guild_id` and `member` are included.

**MESSAGE_UPDATE**
- UD and OFF both now say it is a full message object with the same extras, and `tts` is always `false`.
- ABD still treats it as possibly **partial**: it only caches it if `id, channel_id, author, content, timestamp, edited_timestamp, tts, mention_everyone, mention_roles, attachments, embeds, pinned, type` are all present, and otherwise merges onto the cached message.
- Model it as all-`Option` and merge. Partial embed-only updates still occurring is UNCONFIRMED but cheap to handle.

**MESSAGE_DELETE / MESSAGE_DELETE_BULK / MESSAGE_ACK**
- MESSAGE_DELETE: `{id, channel_id, guild_id?}`.
- MESSAGE_DELETE_BULK: `{ids:[sf], channel_id, guild_id?}`.
- MESSAGE_ACK: `{channel_id, message_id, version:int, ack_type?:int, manual?:bool, mention_count?:int, flags?:?int, last_viewed?:?int}`. An omitted or `null` optional field means **keep the current value**.

**Reactions**
- MESSAGE_REACTION_ADD: `{user_id, channel_id, message_id, message_author_id, guild_id?, member?, emoji, type (0 NORMAL, 1 BURST), burst_colors?:["#rrggbb"]}`. OFF also lists `burst: bool` (required) and makes `message_author_id` optional. Use `burst: Option<bool>`, `#[serde(default)] r#type`.
- MESSAGE_REACTION_REMOVE: `{user_id, channel_id, message_id, guild_id?, emoji, type}` (OFF adds `burst`).
- MESSAGE_REACTION_REMOVE_ALL: `{channel_id, message_id, guild_id?}`.
- MESSAGE_REACTION_REMOVE_EMOJI: `{channel_id, message_id, guild_id?, emoji}`.
- MESSAGE_REACTION_ADD_MANY (only with bit 13): `{channel_id, message_id, guild_id?, reactions:[{users:[sf], emoji}]}`.
- Partial emoji: `{id: ?sf, name: ?string, animated?}`. `name` can be **null** for a deleted custom emoji.

**Channels**
- CHANNEL_CREATE: channel object. DM and group DM carry `recipients` as full users (dedupe is READY-only; DPS only rehydrates READY). Optional `origin_channel_id` for a group DM spawned from a DM. Guild channels have `guild_id`.
- CHANNEL_UPDATE: full channel. **Not** sent for `last_message_id` or `status` changes, so track those from MESSAGE_CREATE. It may reference deleted roles or members.
- CHANNEL_DELETE: channel object; **partial** for private channels.

**Guilds**
- GUILD_CREATE: either a gateway guild object (flat; with CLIENT_STATE_V2 you get `properties` and `data_mode`) or an unavailable stub.
  - For users it fires on joining a guild, on a guild becoming available after an outage, and (per DPS) on the first subscription.
  - Its `members` / `presences` carry inline `user` (dedupe does not apply; inferred from DPS, UNCONFIRMED).
- GUILD_UPDATE: **REST** guild object. No channels, members or threads. Merge, don't replace.
- GUILD_DELETE: `{id, unavailable?, geo_restricted?}`. **If `unavailable` is absent, you left or were removed.** If `true`, it's an outage.

**GUILD_MEMBER_UPDATE**
- `{guild_id, user (partial), nick?:?string, avatar:?string, avatar_decoration_data?, roles:[sf], joined_at, premium_since:?ts, deaf?, mute?, pending?, communication_disabled_until:?ts, flags:int}`.
- Always sent for **your own** member, regardless of intents or subscriptions. For others, only for subscribed members.
- A `communication_disabled_until` in the past means the timeout has expired; no event is sent when it lapses.

**Current user and settings**
- USER_UPDATE: user object for the current user.
- USER_GUILD_SETTINGS_UPDATE: one full user-guild-settings entry (`guild_id` null means DM settings).

**CHANNEL_UNREAD_UPDATE**
- `{guild_id, channel_unread_updates:[{id, last_message_id:?sf, last_pin_timestamp?:?ts}]}`.
- Periodic, for unsubscribed guilds. Not sent with bits 11 or 14 (except for directory channels).
- PASSIVE_UPDATE_V2 equivalent: `{guild_id, updated_channels, updated_voice_states, removed_voice_states, updated_members}`.

**SESSIONS_REPLACE**
- `d` is an **array** of session objects.
- Safe to ignore for chat. It's the only way to learn your own presence (PRESENCE_UPDATE never fires for yourself); look at the `session_id:"all"` entry (DPS).

**PRESENCE_UPDATE** (UD warning): `user` is guaranteed to have only `id`, and "types of the fields are not validated". Make everything optional and lenient.

**TYPING_START:** `{channel_id, guild_id?, user_id, timestamp (int s), member?}`.

**General serde rules**
- Don't use `deny_unknown_fields` anywhere.
- Default every collection.
- Accept snowflakes as either string or number.
- Treat `version` fields as `i64`.
- The Gateway Application `flags` field stops at 31 bits; the full value is in the string `flags_new`.
- Events may be dropped or duplicated (UD-ref "Consistency"), so apply them idempotently.

---

**Not checked:**
- No live payload was captured; the READY example is built from the docs.
- Unconfirmed points: undocumented capability bits 17, 19 and 20; whether heartbeats count toward the 120/60 s limit; whether MESSAGE_UPDATE can still be partial; empty `merged_members` slots for unavailable guilds; the op 37 shape; the web client's choice of zstd vs zlib.

**Risk:** if you include bit 8 (AUTH_TOKEN_REFRESH), you must persist a new `auth_token` from READY, or later requests will use a stale token.




---

# Critic corrections (override earlier parts where they conflict)

# Corrections and additions to the gateway / rest / memberlist_perms / messages_markdown notes

I checked these against the raw sources on 2026-11-11, by grepping the actual files rather than reading summaries:
- **UD** = discord-userdoccers `master`, files under `pages/...mdx`, served at https://docs.discord.food/...
- **OFF** = discord/discord-api-docs `main`, served at https://discord.com/developers/docs/...
- **PR595** = https://github.com/discord-userdoccers/discord-userdoccers/pull/595 (still open and unmerged; last updated 2026-07-04)
- **ABD** = uowuo/abaddon `master`
- **DPS** = dolfies/discord.py-self `master`

---

## A. Contradictions, resolved

### A1. MESSAGE_UPDATE is a full message object
The messages notes say "neither doc says whether it is full or partial". That is wrong. UD says: "Sent when a message is updated. The inner payload is a message object with the extra structure above", and "`tts` will always be `false`".
- Source: https://docs.discord.food/gateway/gateway-events#message-update
- Keep the merge-onto-cache approach anyway. ABD still handles partial updates, and merging costs nothing.

### A2. `merged_members` / `merged_presences.guilds` index alignment is documented
The gateway notes say "confirmed by implementation, not by doc text". That is wrong.
- READY `merged_members`: "Initial members for each of the user's guilds, in the same order as the `guilds` array".
- `merged_presences.guilds`: "in the same order as the `guilds` array in Ready".
- READY_SUPPLEMENTAL `merged_members` uses the same wording, relative to its own `guilds`.
- Supplemental guilds: "Unavailable guilds will only include the `id` field."
- Whether an unavailable guild gets an empty `[]` slot in `merged_members` is still **UNCONFIRMED**. Keep indexing defensively.
- Source: https://docs.discord.food/gateway/gateway-events#ready , #ready-supplemental

### A3. Op 8 rate limit scope
- UD sub-rate-limit table: "Opcode 8 Request Guild Members | 1 request per guild per session per 30 seconds". It does not qualify this. Source: https://docs.discord.food/gateway/using-gateway#rate-limiting
- OFF changelog "Introducing Rate Limit When Requesting All Guild Members": the limit "specifically affects requests for ALL guild members, when developers set `limit` to 0 and use an empty string for `query`"; "1 request per guild per bot every 30 seconds". Source: https://discord.com/developers/docs/change-log
- Resolution: whether `user_ids` / `query` requests from **user** accounts are limited is **UNCONFIRMED**.
  - Batch requests and always set `nonce`.
  - On a `RATE_LIMITED` dispatch `{opcode, retry_after: float s, meta: {guild_id, nonce?}}`, wait `retry_after` and retry once.
  - DPS uses op 8 with `user_ids` from a user client (`state.py` `query_members`).

### A4. Two different "large guild" thresholds both apply
The gateway notes ("auto-subscribed < 75k") and the Identify `large_threshold` text describe two separate mechanisms.
- **`large_threshold`** (UD Identify table): for users, the gateway will "stop sending non-stateful events for guilds without a subscription". Range 25–250, default **250** for users. In practice: no TYPING_START (and similar events) in guilds over 250 members until you send `typing:true`.
- **75k** (PR595 `topics/guild-subscriptions`, unmerged): "Clients in large guilds (guilds with over 75000 members), will no longer receive message and presence events if they are not subscribed".
  - Force-dispatched anyway: messages mentioning you, messages with `mention_everyone:true`, and messages mentioning one of your roles.
  - "Initial subscription to a large guild will dispatch a Guild Create … the client should repopulate its cache".
- Implementation: send op 37 with `typing:true` for whichever guild is open, regardless of its size.

### A5. Op 9 after a failed Resume
The gateway notes say "keep the connection open and Identify again". UD gives two statements:
- Invalid Session alert: "unless the Gateway closes the connection, this event does not signify that the client needs to start a new WebSocket connection." Source: https://docs.discord.food/gateway/gateway-events#invalid-session
- Resuming section: when the replay buffer overflows, the client "will receive an Opcode 9 Invalid Session and should disconnect. After disconnect, the client should create a new connection with the cached URL from Get Gateway, then identify." Source: https://docs.discord.food/gateway/using-gateway#resuming

Resolution, in this order:
1. Clear `session_id`, `seq` and `resume_gateway_url`.
2. Close with 1000. The session is already dead, so the "appear offline" side effect doesn't matter.
3. Reconnect to the **base** URL (`wss://gateway.discord.gg/?v=9&encoding=json&compress=zlib-stream`), not `resume_gateway_url`.
4. Identify, after a 1–5 s random delay.

DPS does exactly this.

### A6. Sticker URL
The messages notes put PNG stickers on `media.discordapp.net`. UD documents:
- `https://cdn.discordapp.com/stickers/{id}.png` for `format_type` 1 (PNG) and 2 (APNG);
- `.json` for 3 (LOTTIE);
- only GIF (4) on `https://media.discordapp.net/stickers/{id}.gif`: "GIF stickers are not available through the CDN".
- Source: https://docs.discord.food/reference#cdn-formatting

### A7. CDN `size` values
The messages notes allow only powers of two. UD: "any power of two between 16 and 4096, and additionally 20, 22, 24, 28, 40, 44, 48, 56, 60, 80, 96, 100, 160, 240, 300, 320, 480, 600, 640, 1280, 1536, 3072". The rest notes are correct. Source: https://docs.discord.food/reference#cdn-formatting

### A8. Guild member `joined_at` nullability
- UD guild member object: `joined_at` is `ISO8601 timestamp`, non-null.
- OFF guild member object and GUILD_MEMBER_UPDATE: `joined_at` is `?ISO8601 timestamp`.
- Use `Option<String>` everywhere: in `merged_members`, the member list, MESSAGE_CREATE `member`, and GUILD_MEMBER_UPDATE.
- Sources: https://discord.com/developers/docs/resources/guild#guild-member-object ; https://discord.com/developers/docs/events/gateway-events#guild-member-update

### A9. DEDUPE field name for private channels
- The UD capability table text says "`user_id` or `recipient_id`".
- The READY footnote ^6^ says "`recipient_ids` in private channel objects".
- Model `recipient_ids: Vec<Snowflake>` as `#[serde(default)]`. If you want to be paranoid, also accept `recipient_id`.
- Source: https://docs.discord.food/gateway/using-gateway#list-of-capabilities vs https://docs.discord.food/gateway/gateway-events#ready

### A10. Points I checked that need no change, or narrower wording
- **Capability bits:** the UD table stops at `1<<16` (AUTO_LOBBY_CONNECT), and the Identify example still uses `"capabilities": 1734653`. Bits 17, 19 and 20 remain **UNCONFIRMED**.
- **Identify before Hello:** confirmed. "The client does not have to wait for the Opcode 10 Hello event before identifying" (using-gateway).
- **Close code 4015:** confirmed. Users are "limited to a maximum of 50 active Gateway sessions at a time" (using-gateway).
- **Op 3:** "Clients may only update their presence 5 times per 20 seconds" comes from gateway-events (Update Presence). It is not in the sub-rate-limit table.
- **Delete own reaction:**
  - Route: `DELETE /channels/{c}/messages/{m}/reactions/{emoji}/{reaction_type}/@me`, returns 204.
  - Others' reactions: `.../{emoji}/{reaction_type}/{user.id}`, needs MANAGE_MESSAGES.
  - Reaction type enum: `0 NORMAL`, `1 BURST`.
  - Create and list both take `type?` (default NORMAL).
  - Source: https://docs.discord.food/resources/message#delete-own-reaction
- **Content limit:** UD says `content` is "up to 2000 characters". There is no 4000/Nitro mention anywhere in UD, so 4000 stays **UNCONFIRMED**.
- **`nonce`:** the "max 25 characters" limit comes only from OFF. UD footnote: "Sending multiple messages in the same channel with the same nonce in a short period of time will result in only the first message being sent."
- **`replied_user`:** UD and OFF both say "default false". That describes the field when `allowed_mentions` is present. Behaviour when `allowed_mentions` is omitted is still **UNCONFIRMED**, so keep sending it explicitly.
- **`referenced_message`** (OFF footnote [4]): "only returned for messages with a `type` of 19 (REPLY), 21 (THREAD_STARTER_MESSAGE), or 23 (CONTEXT_MENU_COMMAND)". Absent on other types, so don't treat absence as "not loaded" for pins or forwards. Source: https://discord.com/developers/docs/resources/message#message-object
- **Member-list murmur3 self-check:** confirmed by a local x86_32 implementation with seed 0: `""` gives `0`, `"hello"` gives `613153351` (`0x248BFA47`). This matches PR595, which uses `mmh3.hash(','.join(sorted(overwrites)), signed=False)` and `is_accessible_by_everyone` exactly as the memberlist notes describe.
- **Member-list sort key** (PR595): `nick` → user `display_name` (= `global_name`) → `username`. Group `id` type is `snowflake | string`.

---

## B. Missing facts a text client needs

### B1. Guild channels in READY / GUILD_CREATE can lack `guild_id`
OFF channel object: `guild_id?` is "the id of the guild (may be missing for some channel objects received over gateway guild dispatches)".
- When ingesting `guilds[i].channels[]` and `threads[]` from READY or GUILD_CREATE, set `channel.guild_id = guild.id` yourself.
- Read-state `IS_GUILD_CHANNEL` (`flags` on ack) and all permission lookups depend on this. UD's rule is "If the channel has a `guild_id`, set IS_GUILD_CHANNEL".
- Sources: https://discord.com/developers/docs/resources/channel#channel-object ; https://docs.discord.food/topics/read-state#calculating-flags

### B2. READY `notification_settings` (missing from the gateway notes' READY field list)
- READY has `notification_settings: {flags: int}`. Example `{"flags":16}`.
- Flags:
  - `1<<4` **USE_NEW_NOTIFICATIONS**: "separate unreads from the overall message notification level".
  - `1<<5` **MENTION_ON_ALL_MESSAGES**: "increment the mention count on all messages in channels with a message notification level of ALL_MESSAGES".
- Updated by the `NOTIFICATION_SETTINGS_UPDATE` dispatch, whose payload is the same object.
- Edit endpoint: `PATCH /users/@me/notification-settings` `{flags?: ?int}`.
- Unread-flag semantics (doc text): override flags `UNREADS_ONLY_MENTIONS` (1<<9) and `UNREADS_ALL_MESSAGES` (1<<10), and guild flags `UNREADS_ALL_MESSAGES` (1<<11) and `UNREADS_ONLY_MENTIONS` (1<<12). "When these flags are unset, unreads follow the `message_notifications` field."
- So, with USE_NEW_NOTIFICATIONS on and no UNREADS_* flag, an ONLY_MENTIONS channel turns unread only on mentions. That is my reading of the docs; the exact client behaviour is **UNCONFIRMED**.
- With the flag off, any new message marks an unmuted channel unread (legacy behaviour).
- Other READY fields the notes omit, all ignorable for chat: `static_client_session_id`, `auth`, `consents`, `game_relationships`, `linked_users`, `regional_feature_config`.
- Sources: https://docs.discord.food/gateway/gateway-events#ready ; https://docs.discord.food/resources/user-settings#notification-settings-object

### B3. Guild order in the sidebar
- With caps 509 (no `USER_SETTINGS_PROTO`), READY carries the JSON `user_settings` (marked "deprecated").
- Its `guild_folders` is `[{id: ?int, name: ?string, color: ?int, guild_ids: [snowflake]}]`.
- UD: "If `id` is `null` and `guild_ids` is a single element array, this folder is not displayed and instead represents a single guild's position."
- `guild_positions` is no longer documented.
- **Evidence the JSON field is still populated:** ABD identifies with caps 4605 (no bit 9) and builds its guild order from `user_settings.guild_folders` (`discord.cpp` `GetUserSortedGuilds`).
- **Guilds missing from all folders:** ABD puts them first, sorted by guild ID descending, then the folder order (skipping guild IDs you're no longer in). Whether the official client does the same is **UNCONFIRMED**.
- **USER_SETTINGS_UPDATE** "is a partial user settings object with only the modified fields serialized", so merge per field. It is not sent with bit 9.
- REST fallback: `GET /users/@me/settings` (deprecated).
- Sources: https://docs.discord.food/resources/user-settings#guild-folder-structure ; https://docs.discord.food/gateway/gateway-events#user-settings-update ; https://github.com/uowuo/abaddon/blob/master/src/discord/discord.cpp

### B4. Channel visibility filters (beyond VIEW_CHANNEL)
- `user_guild_settings.hide_muted_channels: bool`: hide muted channels.
- Guild settings `flags`:
  - `1<<13` OPT_IN_CHANNELS_OFF: "show all guild channels".
  - `1<<14` OPT_IN_CHANNELS_ON: "hide non-opted in channels".
- Channel override `flags`: `1<<12` OPT_IN_ENABLED ("Channel is shown in the UI"), `1<<11` FAVORITED.
- Exact opt-in rules, such as categories and channels with mentions, are **UNCONFIRMED**. Safe default: ignore opt-in and show every viewable channel.
- Source: https://docs.discord.food/resources/user-settings#user-guild-settings-flags

### B5. Open or create a DM: `POST /users/@me/channels`
- Body `{"recipients":["<user_id>"]}`. `recipient_id` is deprecated.
- One recipient creates the DM, or returns the existing one ("most recent" if there are several). Zero or several recipients make a group DM.
- Returns a private channel object and fires CHANNEL_CREATE.
- Limit: "Users may only create 10 new DMs to non-bot users in a 10-minute window". A DM only counts once the first message is sent.
- List existing DMs: `GET /users/@me/channels`.
- Source: https://docs.discord.food/resources/channel#create-private-channel

### B6. Typing
- `POST /channels/{channel_id}/typing`. No body.
- Returns **204** on success, or **200** `{"message_send_cooldown_ms"?: number, "thread_create_cooldown_ms"?: number}` when you're slowmode-limited.
- "Official clients expire a typing indicator 10 seconds after the last Typing Start Gateway event."
- How often to resend while composing is **UNCONFIRMED**. Resending at most every ~8 s while the user types is safe.
- TYPING_START in guilds over 250 members needs `typing:true` (see A4).
- Source: https://docs.discord.food/resources/channel#trigger-typing-indicator

### B7. Slowmode and send preconditions
- **Channel `rate_limit_per_user?`:** int seconds, max 21600. Exempt: users with MANAGE_MESSAGES or MANAGE_CHANNELS (UD channel doc), and `BYPASS_SLOWMODE` `1<<52` (UD permissions table).
- **Other new bits:**
  - `PIN_MESSAGES` `1<<51`.
  - `MANAGE_OFFICIAL_MESSAGES` `1<<53`.
  - Note: UD prints the hex for `1<<45` as `0x0000040000000000`, which is the 1<<42 value. Trust the shift column.
- **Send-path JSON error codes** (OFF):

| code | meaning |
|---|---|
| 0 | general / 401 Unauthorized |
| 10003 | unknown channel |
| 10008 | unknown message |
| 10014 | unknown emoji |
| 20016 | slowmode |
| 20028 | channel write rate limit |
| 30046 | too many edits of messages older than 1 h |
| 40002 | verify account |
| 40333 | Cloudflare |
| 50001 | missing access |
| 50006 | empty message |
| 50007 | cannot DM user |
| 50008 | non-text channel |
| 50013 | missing permissions |
| 50014 | invalid token |
| 50035 | invalid form body |
| 160002 | reply without READ_MESSAGE_HISTORY |
| 160005 | thread locked |
| 200000 | AutoMod blocked |
| 240000 | harmful link blocked |

- Sources: https://docs.discord.food/resources/channel#channel-object ; https://docs.discord.food/topics/permissions ; https://discord.com/developers/docs/topics/opcodes-and-status-codes#json

### B8. Member data for authors in REST history
- REST `GET /channels/{c}/messages` has no `member` and no `guild_id`. Those are gateway-only extra fields. So nicknames and role colours for history authors need a separate lookup.
- **Preferred: op 8.**
  ```json
  {"op":8,"d":{"guild_id":["<gid>"],"user_ids":["<uid>", "..."],"presences":false,"nonce":"<≤32 bytes>"}}
  ```
  - `user_ids` max 100. `guild_id` may be an array for users. One of `query` / `user_ids` is required.
  - The response is one or more **GUILD_MEMBERS_CHUNK** dispatches: `{guild_id, members[], chunk_index, chunk_count, not_found?: array (IDs may come back as JSON numbers), presences?, nonce?}`.
- **Fallback:** `GET /guilds/{g}/members/{u}` (no user-account restriction in UD). It is one request per user, so keep it rare.
- Rate limit: see A3.
- Sources: https://docs.discord.food/gateway/gateway-events#request-guild-members , #guild-members-chunk ; https://docs.discord.food/resources/guild#get-guild-member

### B9. Threads in the sidebar
- READY gives users only **joined** threads.
- The full active list for a guild comes from **THREAD_LIST_SYNC** after subscribing with op 37 `threads:true`. UD: "For user accounts, only joined threads are synced, and the full list is sent through this event on subscription."
- THREAD_LIST_SYNC: `{guild_id, channel_ids?: [sf] (if omitted, the whole guild was synced; listed channels with no threads mean clear them), threads: [channel], members: [thread member]}`.
- THREAD_CREATE: channel object plus `newly_created?: bool`, plus `member?` when you're added to a private thread.
- THREAD_MEMBERS_UPDATE: `{id, guild_id, member_count (capped at 50), added_members?: [thread member + member + ?presence], removed_member_ids?}`.
- Source: https://docs.discord.food/gateway/gateway-events#thread-list-sync

### B10. Other dispatches a text client should handle (UD gateway-events)
- **CHANNEL_PINS_UPDATE** `{guild_id?, channel_id, last_pin_timestamp?: ?ts}`. Not sent when a pinned message is deleted.
- **CHANNEL_PINS_ACK** `{channel_id, timestamp, version}`. Ack pins with `POST /channels/{c}/pins/ack` (204).
- **CHANNEL_RECIPIENT_ADD** `{channel_id, user (partial), nick?}` and **CHANNEL_RECIPIENT_REMOVE** `{channel_id, user}`, for group DMs.
- **RELATIONSHIP_ADD**: full relationship object plus `should_notify?`. Type changes, such as request to friend, arrive only through ADD.
- **RELATIONSHIP_REMOVE** and **RELATIONSHIP_UPDATE** carry a *partial* relationship `{id, type, nickname, since?, stranger_request?, user_ignored}` with no `user`.
- **"Blocked or ignored"** for the read-state rules means relationship `type==2` OR `user_ignored==true`.
- **GUILD_MEMBER_ADD**: for users, sent only for yourself.
- **GUILD_MEMBER_REMOVE** `{guild_id, user}`. PR595 adds: if `user` contains only `id`, it signals that the user is not a member. It is not necessarily a leave event.
- **GUILD_ROLE_DELETE** `{guild_id, role_id}`.
- **CHANNEL_UPDATE_PARTIAL**: OAuth2 only, so ignore it.

### B11. Subscriptions across RESUME
- DPS re-sends every subscription only on a new READY (`_delay_ready` → `_requeue_subscriptions`). `parse_resumed` does nothing.
- So subscriptions are assumed to survive RESUME. This is implementation evidence, not doc text.
- Source: https://github.com/dolfies/discord.py-self/blob/master/discord/state.py

### B12. Notifications: `@silent`
- Message flag `1<<12` SUPPRESS_NOTIFICATIONS: "Message will not trigger push and desktop notifications". So: no desktop notification.
- UD's read-state automations do not exclude these messages, so still apply the `mention_count` increment. That is inference from the docs, **UNCONFIRMED**.
- Source: https://docs.discord.food/resources/message#message-flags

### B13. Small formulas and facts
- **Snowflake to time:** `ms = (id >> 22) + 1420070400000`.
- **Join-time ack guess:** `((joined_ms - 1420070400000) << 22)`. This is the inverse of the formula above; the use itself is still UNCONFIRMED.
- **`last_viewed`** = days since `1420070400000`.
- **Unread rule (UD):** compare `channel.last_message_id` with `read_state.last_message_id`, and `channel.last_pin_timestamp` with `read_state.last_pin_timestamp`. Treat a missing read state as read, or use the join-time guess (UNCONFIRMED).
- **DM list order:** sort by `last_message_id`, falling back to `id`, descending. **UNCONFIRMED** (no source).
- **Message requests:** hide or segregate DMs with `is_message_request:true` / `is_spam:true` (private-channel fields).
- **`GUILD_DELETE`:** `unavailable` absent means you were removed. Don't drop a guild when `unavailable:true`.

### B14. Optional: op 34 Request Last Messages
- Payload: `{"op":34,"d":{"guild_id":"<gid>","channel_ids":["<cid>", "..."]}}`, max 100 channels.
- Response: `LAST_MESSAGES {guild_id, messages[]}`.
- Useful for previews in unsubscribed guilds without REST calls. You must be a member of the guild.
- Source: https://docs.discord.food/gateway/gateway-events#request-last-messages

---

Not checked: Dissent/Discordo/arikawa sources were not opened; no live gateway capture. Still **UNCONFIRMED**:
- the op 8 limit for `user_ids` requests;
- capability bits 17, 19 and 20;
- 4000-char Nitro content;
- the reply-ping default with `allowed_mentions` omitted;
- the official client's handling of guilds not in any folder;
- opt-in channel rules;
- DM ordering.

Risk: the member-list and op 37 material rests on an unmerged PR (#595) plus library code, so keep every member-list field `Option` and keep an `Unknown` op variant.

