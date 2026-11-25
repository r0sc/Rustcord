# Voice design (audio calls, phases 0/A/B/C of VOICE_NOTES)

Protocol facts live in `docs/VOICE_NOTES.md` (authoritative). This page fixes the split of the
code so four people can work in parallel. Video/Go Live: `docs/VIDEO_DESIGN.md`.

## Data flow
```
UI ──Command::JoinVoice/LeaveVoice/SetSelfMute/SetSelfDeaf/SetVoiceDevices/SetVoiceInputMode/ListVoiceDevices──►
backend::run ──VoiceManager (methods)──► voice task (voice/mod.rs, "manager")
                                           │ GatewayCommand::UpdateVoiceState (op 4, debounced 250 ms, send limiter)
main gateway (gateway/conn.rs) ◄───────────┘
   │ my VOICE_STATE_UPDATE + every VOICE_SERVER_UPDATE ──VoiceFeed──► voice task
   │ all VOICE_STATE_UPDATEs (+ READY_SUPPLEMENTAL / GUILD_CREATE voice states) ──► UI ──► AppState::voice_states
voice task: both dispatches in → transport::ConnectInfo → audio::Audio::start + tokio::spawn(transport::run)
transport::run ── wss://{endpoint}/?v=9 (JSON + binary DAVE ops ⇄ dave::Dave)
               ── UDP: send  Captured::Frame → Dave::encrypt_opus → RTP → AEAD → socket
                       recv  socket → AEAD → RTP → SSRC→user → Dave::decrypt → Playback::push
audio thread  ── cpal capture → input volume (soft limit above 100 %) → level / VAD/PTT → Opus encode → Captured (bounded) ; Playback → jitter → Opus decode → mix → cpal
VoiceEvents (EventSink) ──BackendEvent::Voice──► UI: AppState::apply_voice → AppState::voice (+ toast)
```
The voice token only travels gateway → voice task → transport (`auth::Secret`, redacted Debug);
it never reaches the UI thread.

## Tasks and threads
| Where | Kind | Owns |
|---|---|---|
| voice task (`mod.rs`) | 1 tokio task for the whole backend loop | target channel, mute/deaf, prefs, `Call {stop, Audio}`, join deadline |
| `transport::run` | 1 tokio task per connection + child tasks it owns (WS loop, UDP rx, UDP tx) | WebSocket, `UdpSocket`, `Crypt`, `Arc<Mutex<Dave>>`, SSRC→user map |
| audio | 1 std thread "rustcord-audio" per call + cpal callback threads | cpal streams, Opus encoder/decoders, jitter buffers, mixer, PTT polling |
| device listing | `spawn_blocking` per `ListVoiceDevices` | — |

Stop = the manager drops `Call`: the transport's `stop` resolves, `Audio`'s Drop ends its thread.
Every `Connected`/`Ended` from an old connection is ignored (connection counter).

## Manager policy (mod.rs)
- Join: op 4 → `Connecting` → wait for my VOICE_STATE_UPDATE (session_id) and VOICE_SERVER_UPDATE
  (endpoint, token) in any order → connect. 10 s without media → leave ("channel may be full").
- Moved by a moderator: follow the new channel; a new token always means a fresh connection.
- Own state with no channel after ours arrived = kicked → `Disconnected{reason}`, no op 4.
- `End::Kicked` (4014): wait for the gateway (move or kick), deadline 10 s. `End::Rejoin`: one
  fresh op 4. `End::Failed`: op 4 leave + `Disconnected{reason}`. Never any other automatic join.
- Mute/deafen: op 4 (deafen sends `self_mute` too) + `Audio::set_config`; server mute/deaf/
  suppress from my voice state also force `muted`/`deafened`.
- Sessions (`VoiceFeed::session` on every READY): my voice state with another `session_id` and a
  channel = I joined voice on another device → `Disconnected{reason}`, **no** op 4 (it would
  leave there too); with no channel it is ignored. A new READY (re-identify) ends the call
  without op 4 or rejoin. RESUMED changes nothing. Joining the channel I'm already in is a no-op.
- A panic in the transport (or its UDP tasks) ends the call as `End::Failed`, never a hang.

## Buffers (memory stays bounded, ~1–2 MB per call)
| Buffer | Size |
|---|---|
| capture → transport (`Captured`) | `CAPTURE_QUEUE` = 10 frames (200 ms), drop newest when full |
| jitter per SSRC | `JITTER_PACKETS` = 5 packets; unknown SSRC wait ≤ 200 ms |
| decode scratch | 5760 samples × 2 ch, one per call (120 ms Opus) |
| output ring | ≤ 100 ms (4800 × 2 f32) |
| UDP packet buffers | one ≤ 1500 B per direction, reused |
| DAVE old keys | ≤ 10 s (davey) |

## Contract (do not change signatures without the contract owner)
- `voice/mod.rs`: `VoiceEvent`, `InputMode`, `VoiceEvents::send`, `VoiceFeed::{send, session}`, `VoiceManager`.
- `transport.rs`: `ConnectInfo`, `End`, `run(info, media, stop, connected) -> End`.
- `audio.rs`: `AudioConfig`, `Captured`, `MediaLink`, `Playback::{push, remove}`,
  `Audio::{start, set_config}` + Drop, `list_devices`, the constants.
- `dave.rs`: `MAX_PROTOCOL_VERSION`, `Reply`, `Dave::{new, on_session_description, on_clients_connect,
  on_client_disconnect, on_json, on_binary, encrypt_opus, decrypt, privacy_code}`.
- Helpers in `transport.rs` (`Mode`, `discovery_request`, `parse_rtp`, `Crypt`, …) are a suggestion.

## File ownership
| Owner | Files |
|---|---|
| contract owner | `voice/mod.rs`, this page, `model.rs` voice types, gateway op 4/forwarding, `state.rs` voice parts, `backend.rs` voice arms, `Cargo.toml` |
| transport engineer | `voice/transport.rs` (+ private submodules `voice/transport/*`) |
| audio engineer | `voice/audio.rs` (+ `voice/audio/*`) |
| DAVE engineer | `voice/dave.rs` |
Crates are already in `Cargo.toml` (pinned): davey 0.2.0, opus-rs 0.1.42, cpal 0.18.2, aes-gcm 0.10.3,
chacha20poly1305 0.10.1; tokio `net`; windows-sys `Win32_UI_Input_KeyboardAndMouse`. Ask before adding more.

## Rules
Secrets (voice token, `secret_key`, MLS keys) never in logs, Debug output or error text; davey
logs key material through `tracing` at debug/trace, so never install a subscriber for it.
Nothing transmits while muted/suppressed; 5 silence frames before going quiet. Test DAVE-less
media only in a Stage channel. Every non-trivial pure function gets a unit test.
