//! Gateway (WebSocket) connection: identify, heartbeat, zombie detection, resume/reconnect
//! with backoff, zlib-stream decompression, outgoing rate limit, and normalization of
//! dispatch payloads into `model::GatewayEvent`.
//!
//! Voice: op 4 is sent here (`UpdateVoiceState`); my VOICE_STATE_UPDATEs and every
//! VOICE_SERVER_UPDATE are also handed to `crate::voice` through a `VoiceFeed`, which opens its
//! own voice connection.
//!
//! OWNERSHIP: gateway implementer (all files under gateway/). Pub signatures below are the
//! contract used by backend.rs.

mod account;
mod calls;
mod conn;
mod inflate;
mod normalize;
mod quests;
pub mod remote_auth;
mod social;
mod stream;
mod threads;

use crate::api::props::ClientProps;
use crate::auth::Token;
use crate::backend::EventSink;
use crate::model::{Id, UserStatus};
use crate::voice::VoiceFeed;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Commands from the backend to a running gateway connection.
#[derive(Debug)]
pub enum GatewayCommand {
    /// Guild subscription (sent as op 37, like the current client; op 14 is deprecated):
    /// member list (range 0-99) + typing for one guild channel.
    SubscribeMemberList { guild_id: Id, channel_id: Id },
    /// Op 4: join (`channel_id` Some), switch, leave (None) or change mute/deaf/camera.
    /// `guild_id` None = DM/group call. Debounced; only the newest pending one is sent,
    /// through the send limiter, and a pending one survives a reconnect.
    UpdateVoiceState { guild_id: Option<Id>, channel_id: Option<Id>, self_mute: bool, self_deaf: bool, self_video: bool },
    /// Op 3 for this session (FEATURES_NOTES §1.3): `status`, `since` 0 (idle: unix ms now),
    /// `afk` (true only for an automatic idle, like the official auto-idle, so mobile pushes go
    /// on), and the **full** activity list, i.e. the current custom status (type 4)
    /// that the gateway keeps from READY / USER_SETTINGS_UPDATE, so op 3 never wipes it.
    /// Max 5 per 20 s: debounced, newest wins, through the send limiter; survives a reconnect.
    UpdatePresence { status: UserStatus, afk: bool },
    /// My detected game and Rustcord's own presence (None: none) for op 3's activity list, after
    /// the custom status (game first). On a change an op 3 with my current status is queued like
    /// `UpdatePresence` (after READY; a new session gets it again after its READY).
    SetActivities { game: Option<crate::activity::GameActivity>, app: Option<crate::activity::AppActivity> },
    /// Op 8 with `query` (§1.7): `{"guild_id":["<g>"],"query","limit","presences":false,
    /// "nonce"}` (nonce ≤ 32 bytes, kept with (guild, query) in a small bounded map). Debounced
    /// per guild (~600 ms, newest query wins). GUILD_MEMBERS_CHUNK with that nonce ->
    /// `BackendEvent::Chat(ChatEvent::MembersFound)`; RATE_LIMITED -> wait `retry_after`, retry
    /// once. Never `query:""`.
    RequestMembers { guild_id: Id, query: String, limit: u8 },
    /// Go Live op (VOICE_NOTES §5.4), each from one user action. Queued in order (at most
    /// `MAX_STREAM_OPS`, oldest dropped), sent through the send limiter, dropped on a re-identify.
    /// STREAM_* answers go to the voice module.
    Stream(StreamOp),
    /// A token-rotating request (password / 2FA change) is on its way: a close 4004 within the
    /// next `TOKEN_WAIT` waits for `SetToken` instead of logging out.
    ExpectToken,
    /// The rotated token: kept for every later IDENTIFY / RESUME; a live connection closes and
    /// re-identifies with it at once.
    SetToken(Token),
    /// Close the connection (code 1000) and end the task.
    Shutdown,
}

/// Main-gateway Go Live ops.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamOp {
    /// Op 18: start my stream in the call I am connected to (`guild_id` None = DM/group call).
    Create { guild_id: Option<Id>, channel_id: Id },
    /// Op 19: end my stream, or stop watching one.
    Delete { stream_key: String },
    /// Op 20: watch a stream.
    Watch { stream_key: String },
    /// Op 22: pause or resume my stream.
    SetPaused { stream_key: String, paused: bool },
}

pub struct GatewayHandle {
    pub tx: mpsc::UnboundedSender<GatewayCommand>,
    pub task: tokio::task::JoinHandle<()>,
}

impl GatewayHandle {
    pub fn send(&self, cmd: GatewayCommand) {
        let _ = self.tx.send(cmd);
    }
}

/// Start the connection loop. Emits `BackendEvent::Conn`, `BackendEvent::Gateway(..)` and,
/// on fatal auth errors (close 4004), `BackendEvent::AuthFailed` (then stops). Voice
/// dispatches for the voice module go to `voice`.
/// Must be called from within the tokio runtime.
pub fn spawn(token: Token, props: Arc<ClientProps>, sink: EventSink, voice: VoiceFeed) -> GatewayHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(conn::run(token, props, sink, voice, rx));
    GatewayHandle { tx, task }
}
