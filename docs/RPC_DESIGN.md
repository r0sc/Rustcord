# Local game connection (Discord RPC over IPC): design

Status: **approved** (§11: build it, the setting defaults to on, no host side). Built: the pipe and a
game's own rich presence in op 3 (§2, §3, §5, §8.5) with the image proxy (§3.4), and Join / Ask to
Join on activity cards (§4.1, §4.2). Still to come: invite messages (§4.3).

Games and apps on this computer (discord-rpc, the Game SDK, pypresence, music players, …) look
for a local pipe named `discord-ipc-N`, speak a small JSON protocol over it, and use it for two
things:

1. **their own rich presence**: "Playing X · In a match · 2 of 4", their pictures, buttons;
2. **joining**: when a friend clicks "Join" on that presence, the friend's client asks Discord
   for the join secret and hands it to the friend's copy of the game over the same pipe.

Rustcord has no such pipe today, so games show only as "Playing X" (process detection) and the
"Join" / "Ask to Join" buttons on activity cards (`ui/profile.rs` `activity_section`) are
disabled. This document specifies a local RPC server in Rustcord that fills both gaps.

Sources: docs.discord.food (presence, message, RPC, gateway-events, application pages),
Discord's RPC and Game SDK docs, the reference library discord/discord-rpc
(`connection_win.cpp`, `connection_unix.cpp`, `rpc_connection.cpp`, `serialization.cpp`,
`discord_rpc.cpp`), and arRPC 3.7 (OpenAsar/arrpc). Points that no source settles are marked
**unverified** and listed in §10.

## 1. Scope

v1 (this design):

- the pipe server, on by a setting (§8.5)
- the commands games use for presence and joining: handshake, `SET_ACTIVITY`, `SUBSCRIBE` /
  `UNSUBSCRIBE`, PING
- a game's activity in my op 3, next to my detected game and Rustcord's own presence
- "Join" on someone's activity card: one secret GET on the click, then `ACTIVITY_JOIN` to my
  connected game
- "Ask to Join" on someone's card: one DM message on the click
- rich-presence invites in chats (message `activity.type` 1) with a "Join" button; join requests
  (type 5) shown as a line

Later (phase 2, separate OK): the **host side**, i.e. others asking to join *my* game
(`ACTIVITY_JOIN_REQUEST` to my game, the game's `SEND_ACTIVITY_JOIN_INVITE` /
`CLOSE_ACTIVITY_JOIN_REQUEST`, an "Invite" button on a join request), `ACTIVITY_INVITE` /
`ACTIVITY_INVITE_USER` / `ACCEPT_ACTIVITY_INVITE` (Game SDK in-game invites), launching a game
that is not running, spectating.

Never: OAuth over RPC (`AUTHORIZE`, `AUTHENTICATE`), voice/guild/channel RPC commands, a
WebSocket or HTTP RPC port, the Flatpak/Snap subdirectory pipes.

## 2. What a game sends

### 2.1 Transport

| OS | Path | Who picks N |
|---|---|---|
| Windows | `\\.\pipe\discord-ipc-N` (clients write `\\?\pipe\…`, the same pipe) | the game opens N = 0..9 in order and takes the first that opens; on `ERROR_PIPE_BUSY` it waits up to 10 s for the same N |
| Linux, macOS | `<dir>/discord-ipc-N`, `<dir>` = the first **set** of `XDG_RUNTIME_DIR`, `TMPDIR`, `TMP`, `TEMP`, else `/tmp` | the game `connect()`s N = 0..9 and takes the first that accepts |

The reference library treats any other open error on Windows (for example access denied) as
"no Discord" and stops trying.

### 2.2 Frames

Every message is one frame: `op: u32 LE`, `len: u32 LE`, then `len` bytes of UTF-8 JSON.

| op | Name | Direction |
|---|---|---|
| 0 | HANDSHAKE | game → client, once, first |
| 1 | FRAME | both: commands, responses, events |
| 2 | CLOSE | client → game: `{"code", "message"}`, then the pipe closes |
| 3 | PING | either; answered with PONG carrying the same bytes |
| 4 | PONG | |

Rustcord buffers partial reads (arRPC doesn't, and drops a frame split across reads), caps a
frame at **64 KiB** (`MAX_FRAME`; a rich presence is under 2 KiB), and closes on an unknown op,
a second handshake, a FRAME before the handshake or bad JSON: CLOSE `{"code":1003,"message":…}`.

### 2.3 Handshake and READY

The game sends `{"v":1,"client_id":"<application id>"}`.

- `v` not 1 → CLOSE 4004 (`INVALID_VERSION`);
- `client_id` missing or not a snowflake → CLOSE 4000 (`INVALID_CLIENTID`).

Otherwise Rustcord answers, once it knows who is logged in (a game that connects while Rustcord
is logged out waits; discord-rpc waits for READY without a timeout):

```json
{"cmd":"DISPATCH","evt":"READY","nonce":null,"data":{"v":1,
 "config":{"cdn_host":"cdn.discordapp.com","api_endpoint":"//discord.com/api","environment":"production"},
 "user":{"id":"…","username":"…","discriminator":"0","global_name":"…","avatar":"…","bot":false}}}
```

`user` is the logged-in account (discord-rpc reads `id`, `username`, `discriminator`, `avatar`).
It stays on this computer. A logout closes every connection (CLOSE 1000); the next login takes
new ones.

### 2.4 Commands (FRAME, game → Rustcord)

`{"cmd":…, "args":…, "nonce":"…"}`; `SUBSCRIBE` / `UNSUBSCRIBE` carry the event name in `evt`
and no `args`. Every command gets exactly one response with the same `nonce` and `cmd`.

| Command | Args | Response `data` |
|---|---|---|
| `SET_ACTIVITY` | `pid`, `activity` (absent or null: clear) | the activity as sent on (§3), or null |
| `SUBSCRIBE` | `evt`: `ACTIVITY_JOIN`, `ACTIVITY_SPECTATE`, `ACTIVITY_JOIN_REQUEST`, `ACTIVITY_INVITE` | `{"evt":"<name>"}`, `"evt":null`; another event → ERROR 4000 |
| `UNSUBSCRIBE` | same | same |
| `SEND_ACTIVITY_JOIN_INVITE`, `CLOSE_ACTIVITY_JOIN_REQUEST`, `CLOSE_ACTIVITY_REQUEST` | `user_id` | v1: ERROR 4012 (`INVALID_ACTIVITY_JOIN_REQUEST`): no join request is ever pending in v1 (host side is phase 2) |
| anything else (`AUTHORIZE`, `GET_GUILDS`, …) | | ERROR 4002 (`INVALID_COMMAND`), message "Not supported by Rustcord" |

An error response is `{"cmd":<cmd>,"evt":"ERROR","data":{"code":…,"message":…},"nonce":…}`.
discord-rpc reads only errors and events; it ignores successful responses.

The subscriptions are accepted for every event discord-rpc and the Game SDK use, so games that
subscribe to all of them right after READY do not get errors; in v1 only `ACTIVITY_JOIN` is ever
dispatched.

### 2.5 Events (FRAME, Rustcord → game, no nonce)

| Event | When (v1) | `data` |
|---|---|---|
| `ACTIVITY_JOIN` | my click on "Join" (§4.1) or on a "Join" invite (§4.3) | `{"secret":"<join secret>"}` |
| `ACTIVITY_SPECTATE` | never (spectating is deprecated) | |
| `ACTIVITY_JOIN_REQUEST`, `ACTIVITY_INVITE` | phase 2 | |

An event goes only to connections of the matching application (`client_id` = the activity's
`application_id`) that subscribed to it.

## 3. `SET_ACTIVITY` → op 3

### 3.1 What a game sends

What discord-rpc writes (other libraries send a subset or the same plus `buttons` and `type`):

```json
{"cmd":"SET_ACTIVITY","nonce":"1","args":{"pid":4242,"activity":{
  "state":"In a match","details":"Ranked · Map 3",
  "timestamps":{"start":1764183000},
  "assets":{"large_image":"map3","large_text":"Map 3","small_image":"rank_gold","small_text":"Gold"},
  "party":{"id":"party-8f2c","size":[2,4],"privacy":1},
  "secrets":{"join":"j-1a2b3c","spectate":"s-…","match":"m-…"},
  "instance":true}}}
```

### 3.2 Mapping

Rustcord parses this into a typed `rpc::Activity` (strict types, unknown keys ignored), checks
the limits from the presence docs, and answers ERROR 4000 (`INVALID_PAYLOAD`) with the failing
field for anything outside them. The op 3 activity is built from that type only, never by
passing the game's JSON through (arRPC spreads it unchanged).

| Op 3 field | From | Rule |
|---|---|---|
| `type` | `activity.type` | 0, 2, 3 or 5; absent → 0; others → ERROR |
| `name` | the application's name (`/applications/public`, cached like the icons; else the detectable-games list) | the game's own `name` is ignored, like the official client. Until the name is known the activity is held, not sent |
| `application_id` | handshake `client_id` | |
| `details`, `state` | same | 2-128 chars after trimming; shorter is dropped, longer → ERROR |
| `details_url`, `state_url` | same | https only, ≤ 256 |
| `status_display_type` | same | 0, 1 or 2 |
| `timestamps.start` / `.end` | same | **seconds → ms**: a value below 10^11 is seconds and is multiplied by 1000 (discord-rpc sends seconds, some libraries ms); 0 dropped |
| `assets.large_image` / `small_image` | same | an asset key or `mp:…` as is; an `https://` URL goes through the external-assets proxy (§3.4); anything else dropped |
| `assets.large_text` / `small_text` | same | ≤ 128 |
| `party` | `party.id` (≤ 128), `party.size` `[current, max]` (both ≥ 1, current ≤ max) | `privacy` is not sent on, it only sets flags |
| `secrets` | `secrets.join` (≤ 128) only | `spectate` and `match` are deprecated and dropped. Send-only: Discord stores it and gives it out only through the secret endpoint (§4.1) |
| `flags` | computed | `INSTANCE` (1<<0) if `instance`; `JOIN` (1<<1) if a join secret **and** a party id; `PARTY_PRIVACY_FRIENDS` (1<<6) if JOIN, `party.privacy` = 1 (public) and "Friends can join my activities" (proto 8.1, default on); `PARTY_PRIVACY_VOICE_CHANNEL` (1<<7) likewise with "Voice channel members can join" (8.2). **Unverified**: the privacy rule (§10) |
| `buttons` + `metadata.button_urls` | `buttons: [{label, url}]` | ≤ 2, label 1-32, url https ≤ 512 (same as the "Get Rustcord" button). Dropped when a join secret is set (**unverified** whether Discord rejects both; §10) |

Never sent: `id`, `created_at`, `session_id` (server-only), `platform`, `sync_id`, `emoji`.

The response's `data` is this op 3 activity, with the join secret left out (the game knows it).

### 3.3 Which activities op 3 carries

op 3 today: `[custom status, detected game, Rustcord]` (`gateway/conn.rs` `presence_payload`).
With RPC:

- **One** RPC activity: the most recently set non-null one over all connections. A second app
  (a music player next to a game) does not show at the same time (**shortcut**; the official
  client's behaviour with two RPC apps is not documented).
- It replaces the detected game when both have the same application id, or when the detected
  game has no application id (most games are detected by name only) and the RPC activity is type
  0 (playing; a music player's "Listening" does not hide the game). Otherwise both go.
- Order: `[custom status, RPC activity, detected game, Rustcord]`.
- Gated like the detected game: only while "Share my activity" (proto 11.3) is on. Rustcord's own
  presence keeps its own switch; Invisible still hides it.
- A connection's activity goes when it sends `activity: null` or when its pipe closes (the game
  quit or crashed). That is one op 3.

op 3 is limited to **5 updates per 20 s** per session (gateway-events page). User actions
(status, custom status) must not wait behind a chatty game, so game-driven changes are
throttled to **one op 3 per 4 s**, the latest winning. The existing 250 ms
`PRESENCE_DEBOUNCE` stays for everything else.

Code: `ActivityCommand::SetActivities { game, app }` gains `rpc: Option<rpc::Presence>`
(`activity::logic` builds it from `state.rpc`; the throttle lives in the server, `rpc/mod.rs`), and so do
`GatewayCommand::SetActivities` and `presence_payload`. `rpc::Presence::op3()`
builds the JSON; `presence_payload` stays the one place that writes op 3.

### 3.4 Image URLs

The presence docs say activities sent through RPC "automatically handle proxying URLs": the
official client turns each `https://` image into a media-proxy path with
`POST /applications/{app}/external-assets {"urls":[…]}` (1-2 URLs, each ≤ 256) →
`[{"url","external_asset_path"}]`, then sends `mp:<external_asset_path>`.

Rustcord does the same: one POST per new URL pair, through the limiter, cached per
(application, URL) for the session; the activity waits for the answer (≤ 5 s, then goes
without the picture). This is the one request a game can cause without a click; it carries
only the game's image URLs. **Unverified**: whether the endpoint answers a non-official client
(§10). If it doesn't, URL images are dropped and asset keys still work.

## 4. Joining

The click is the only trigger. Nothing below runs on its own, on hover or on a message
arriving, and every request goes through the limiter.

### 4.1 "Join" on someone's activity card

`Activity::join_label` already decides the label: "Join" when the party lets me in (friend +
`PARTY_PRIVACY_FRIENDS`, or same voice channel + `PARTY_PRIVACY_VOICE_CHANNEL`), else "Ask to
Join".

The button is enabled when:
- Rustcord is connected;
- the activity has a `session_id` and an `application_id`;
- one of my RPC connections has that `client_id` and subscribed to `ACTIVITY_JOIN`, i.e. the
  same game runs here and listens.

Otherwise it stays disabled with the reason on hover: "Start {game} to join" when the game isn't
connected, or the existing "Rustcord can't connect to games" text while the setting is off.
Launching the game for the user (the official client can) is phase 2.

The click sends:

```
GET /users/{user}/sessions/{session_id}/activities/{application_id}/1    → {"secret":"…"}
```

(1 = JOIN; the endpoint needs the JOIN flag and, without an invite, a party-privacy flag I
meet). On success Rustcord writes `ACTIVITY_JOIN {"secret"}` to every matching subscribed
connection. On an error a toast says why ("Couldn't join: …"). The secret is never logged,
stored or shown: it lives in a `Secret` from the response to the pipe write.

`session_id` comes from other users, so it gets the same check as the metadata URL
(`api/apps.rs` `metadata_url`: letters and digits, ≤ 64).

### 4.2 "Ask to Join" on someone's card

Enabled when connected and the activity carries a `party.id` (the presence model gets
`party_id`; `gateway/normalize.rs` reads `party.id`). The game does not need to run yet: the
answer can take minutes.

The click: open the DM with them (`Command::OpenDm`, which the existing user menu uses), then

```
POST /channels/{dm}/messages
{"content":"","nonce":"…","application_id":"<app>",
 "activity":{"type":5,"session_id":"<my gateway session id>","party_id":"<their party id>"}}
```

After the click the button reads "Request sent" (disabled) for that activity for this session,
so a second click sends nothing. **Unverified**: whether `session_id` is mine (my READY
`session_id`) or theirs; docs.discord.food only calls it "the session ID associated with this
activity" (§10).

The host's client (official or Rustcord with phase 2) shows the request; if they accept, I get a
rich-presence invite (§4.3).

### 4.3 Invite messages in chats

Messages gain `activity: Option<MessageActivity { kind, party_id }>` and `application_id`
(from `application_id` or `application.id`), parsed leniently in the message model.

- **type 1 (JOIN), an invite**: drawn under the author line as a card: app icon,
  "{author} invited you to join {game}", and a "Join" button. Enabled under the same game rule as
  §4.1, and only while the author's presence still has an activity of that application with a
  `session_id` (invites expire when the host leaves the activity, at most 6 h after sending). The
  click:

  ```
  GET /users/{author}/sessions/{session_id}/activities/{app}/1?channel_id={channel}&message_id={message}
  ```

  then `ACTIVITY_JOIN` as in §4.1. The invite query lets me join a private party. My own
  invites show the card without the button.
- **type 5 (JOIN_REQUEST)**: one muted line, "{author} asked to join your {game}" (mine: "You
  asked to join {game}"). No button in v1: answering is phase 2.
- **type 3 (LISTEN)**, others: the plain message as today.

## 5. Pipe index and the official client

Rustcord takes the **lowest free** index and never takes a pipe someone else holds:

- **Windows**: create `\\.\pipe\discord-ipc-N` with `FILE_FLAG_FIRST_PIPE_INSTANCE`
  (`tokio::net::windows::named_pipe::ServerOptions::first_pipe_instance(true)`). If the name
  exists (Discord, arRPC, another Rustcord) the create fails with access denied → try N+1. No
  probing connection, so nobody else's pipe sees a stray client (arRPC connects and PINGs).
- **Linux, macOS**: for each N, `connect()` first. Accepted → in use, N+1. `ECONNREFUSED` → a
  stale socket file from a crash → remove it and bind there. Not there → bind.
- All ten taken → the server stays off; the setting's description says "Another app is using
  the game connection".

Games use the **first** pipe that opens, so whoever starts first serves the games:

| Started first | Games talk to | Effect |
|---|---|---|
| official Discord (takes 0) | Discord | Rustcord gets 1 and sees no games: the official client shows their presence |
| Rustcord (takes 0) | Rustcord | Discord gets 1 and sees no games |
| only Rustcord | Rustcord | |

That is the same first-come rule the official client, arRPC and Vesktop have with each other.
Both clients being logged into the same account at once is fine: the presences of both
sessions merge.

Rustcord keeps creating the next pipe instance as each game connects, so a second game never
sees `ERROR_PIPE_BUSY`. At most **8** connections at once; the ninth is closed with 4002
(`RATELIMITED`).

## 6. Security

The pipe is local only, per user, and gives a connected program very little:

- **Windows**: the default named-pipe DACL gives full access to the creating user, SYSTEM and
  administrators, and **read** only to Everyone. A program of another user cannot write the
  handshake, and Rustcord writes nothing before one. The pipe also rejects remote clients
  (`reject_remote_clients`, tokio's default): no SMB access from the network. Medium-integrity
  pipes are not writable from low-integrity (sandboxed) processes.
- **Linux, macOS**: the socket is created with mode 0600. `XDG_RUNTIME_DIR` and macOS `TMPDIR`
  are per-user 0700 directories; the `/tmp` fallback is protected by the 0600 mode.
- **No network**: no WebSocket (6463-6472) or HTTP transport, and no bridge port (arRPC's bridge
  listens on all interfaces); a web page cannot reach the pipe.

What a local program **of the same user** can do over it, and why that is acceptable:

| It can | Bound |
|---|---|
| show an activity on my profile | only while "Share my activity" is on, with a real application's name, the limits of §3.2 and the 4 s throttle. The same program could already fake process detection by its exe name |
| read my user id, name and avatar (READY) | public profile data |
| receive a join secret | only for its own `client_id`, only after **my** click on Join for that application |
| make Rustcord send anything else | nothing. No command sends a message, a friend request or an invite; no OAuth; the only request a game can cause is the image proxy (§3.4) |

A same-user program can already read Rustcord's token from disk, so the pipe adds no new trust
boundary on that side. Limits: 64 KiB frames, 8 connections, errors close the pipe.

## 7. Impersonation

Rule (docs/QUESTS_NOTES.md D2, the declined Chrome disguise): Rustcord never pretends to be
the official client toward Discord.

**Toward Discord**, the feature keeps that rule:
- Same identity as today: Rustcord's client properties and user agent, no desktop build number,
  no Electron user agent, nothing taken from Discord's client.
- op 3 with an `application_id`, `party`, `secrets` and `flags` is the user's own presence, the
  fields any client sets. Discord's web client sends the same op 3 when arRPC (Vesktop, Vencord)
  feeds it a game's presence. Rustcord already sends `application_id` for detected games.
- The secret GET, the Ask-to-Join message and the external-assets POST are user-API calls the
  official clients make, each made by Rustcord on the user's own click (the image proxy:
  automatically, §3.4).
- No `AUTHORIZE` / `AUTHENTICATE`: that would mint OAuth tokens for third-party apps behind a
  consent screen Rustcord would have to fake.

**Toward local games** (the open question for the user): Rustcord answers on the pipe name
Discord's client uses, speaks its protocol, and READY says `"api_endpoint":"//discord.com/api"`.
A game cannot tell it isn't the official client. The case for doing it anyway:

- It is an interoperability interface: games ask "is a Discord client of this user running?",
  and Rustcord is one, logged into that user. READY carries the real user; nothing false about
  the account reaches the game or Discord.
- arRPC (in Vesktop and Vencord) has served this pipe for years.
- The added account risk is small: Discord sees a presence with an application id from a
  third-party client, which Rustcord already sends for detected games and web clients send with
  arRPC. Using a third-party client at all is the larger, existing risk.

The case against: a game's developer expects the official client behind the pipe, and Discord's
terms ban third-party clients in general. Rustcord does not hide that it is Rustcord: the
setting says so, and the activity reaches Discord from Rustcord's session.

## 8. Code

### 8.1 Modules

- `src/rpc/mod.rs`: `Service` (start/stop on the backend's tokio runtime), `RpcCommand`,
  `RpcEvent`, `RpcState` (`AppState::rpc`). `OWNERSHIP: activities engineer.`
- `src/rpc/frame.rs`: `Frame { op, body }`, `decode(&mut BytesBuf) -> Result<Option<Frame>,
  Close>` over a growing buffer, `encode`. Pure, no I/O.
- `src/rpc/proto.rs`: per-connection state machine, pure: `Conn::on_frame(Frame) ->
  (Vec<Frame>, Vec<RpcEvent>)` (handshake, READY once the user is known, commands, errors,
  close). All protocol tests run here without a pipe.
- `src/rpc/activity.rs`: `RpcActivity` (serde, from the game) → validated `Activity` (§3.2),
  `to_op3`.
- `src/rpc/pipe.rs`: the listener, `#[cfg(windows)]` named pipe, `#[cfg(unix)]`
  `tokio::net::UnixListener` (§5). Each accepted stream gets a task that reads into the buffer,
  feeds `Conn`, writes frames. The base name is a parameter so tests use
  `rustcord-test-ipc-<pid>-N`, never `discord-ipc-N`.

No new dependencies: tokio `net` (already on) has named pipes and Unix sockets; serde_json and
the `windows-sys` error constants are there.

### 8.2 Channels

- UI → server: `RpcCommand::Me(Option<RpcUser>)` (login, logout), `Dispatch { app, secret }`
  (ACTIVITY_JOIN), `Stop` / `Start` (the setting).
- server → UI (`BackendEvent::Rpc`): `Connected { conn, app }`, `Subscribed { conn, events }`,
  `Activity { conn, activity: Option<Activity>, at }`, `Closed { conn }`, `Listening(Option<u8>)`
  (the index, None: none free).
- UI → backend: `Command::Rpc(RpcRequest::Join { user, session_id, app, invite: Option<(channel,
  message)> })` → secret GET → `RpcCommand::Dispatch`; `RpcRequest::AskToJoin { user, app,
  party_id }` → `OpenDm` + POST; `RpcRequest::ProxyImages { app, urls }` → external-assets.

### 8.3 State

`RpcState { listening: Option<u8>, conns: HashMap<ConnId, Conn { app, events, activity,
set_at }>, asked: HashSet<(Id, String)>, proxied: HashMap<(Id, String), String> }`, plus
`can_join(app) -> bool`. Cleared on logout like `ActivityState::after_logout`.

### 8.4 UI

- `ui/profile.rs` `activity_section`: the Join button as §4.1/§4.2 (the code around the
  current disabled button). No new layout.
- `ui/messages.rs`: the invite card and the request line (§4.3), drawn with the existing
  activity art (`activity::art_url`) and `messages::button`.
- My own activity card shows the game's rich presence like anyone's (via `activity_of` falling
  back to my RPC activity, then my detected game, then Rustcord).

### 8.5 Setting

Settings > Activity privacy, under "Share my activity", one switch (config
`game_connection: bool`):

- **Let games show their activity**: "Games on this computer can show what you're doing in them
  and let friends join you. Rustcord answers them in place of the Discord app." Description when
  no pipe was free: "Another app is using the game connection." Default: §11.

Turning it off closes every connection (CLOSE 1000) and removes the RPC activity (one op 3).

## 9. Tests (offline)

- `frame`: header round trip, a frame split at every byte offset, two frames in one read,
  64 KiB + 1 rejected, unknown op, short header.
- `proto` with fixture payloads: a discord-rpc handshake → READY after `Me`; bad `v` → 4004;
  missing client id → 4000; FRAME before handshake → 1003; PING → PONG with the same body;
  `SET_ACTIVITY` (the §3.1 fixture, a pypresence one with ms timestamps and buttons, a null
  clear) → response and `RpcEvent::Activity`; `SUBSCRIBE ACTIVITY_JOIN` → `{"evt":…}`;
  unknown command → 4002; `SEND_ACTIVITY_JOIN_INVITE` → 4012.
- `activity`: §3.2 rules: seconds → ms, flags (instance, join, privacy × settings), dropped
  buttons with a secret, invalid lengths → ERROR, the op 3 JSON next to the existing
  `presence_payload` tests (order, replacement of the detected game, gated by 11.3).
- `pipe`: a fake pipe client (a tokio named-pipe client / Unix stream) on a test name: handshake,
  READY, SET_ACTIVITY, SUBSCRIBE, a `Dispatch` reaches it as `ACTIVITY_JOIN`; a second server on
  the same name takes N+1; closing the client yields `Closed`.
- join plumbing: a click sends exactly one `Command::Rpc(Join)`; nothing is sent offline, on
  hover or on a message arriving; "Ask to Join" once per activity; the secret is absent from
  `Debug` output.

Nothing in the tests talks to Discord. The live checks of §10 need the user's OK first.

## 10. Unverified (to confirm live, with the user's OK)

1. `PARTY_PRIVACY_*` flags: set by the client when the game's `party.privacy` is public and the
   matching user setting is on. If the official client sets them without `privacy`, friends of
   discord-rpc games see "Ask to Join" through Rustcord instead of "Join".
2. Ask to Join `session_id`: mine (assumed; built that way, **still unverified live**) or the host's.
3. Buttons together with a join secret: rejected or dropped by Discord (assumed: drop them).
4. The external-assets endpoint answering Rustcord.
5. The secret endpoint's errors (none documented): the toast shows Discord's message.
6. Join from a rich-presence invite when the author's presence is not known to me (not a friend,
   no shared server): the button stays disabled.

A first live test: the user runs a discord-rpc game with Rustcord only, checks the presence on
a second account, then clicks Join from the second account's client (official is fine) and back.

## 11. Decisions for the user

1. **Serve local games on Discord's pipe at all?** (§7) Recommendation: yes.
2. **Default of the setting**: on (like the official client: only takes a free pipe; sends
   nothing while "Share my activity" is off) or off (opt-in). Recommendation: on.
3. **Host side** (others asking to join my game, §1): phase 2 later. Recommendation: later.
