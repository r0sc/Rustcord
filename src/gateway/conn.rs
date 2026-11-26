//! Connection loop: connect, HELLO/heartbeat, IDENTIFY/RESUME, zombie detection, close-code
//! handling, reconnect backoff and the outgoing send limit.

use super::inflate::ZlibStream;
use super::normalize::{self, Head, StatusPatch};
use super::GatewayCommand;
use crate::api::props::ClientProps;
use crate::auth::Token;
use crate::backend::{BackendEvent, ChatEvent, ConnState, EventSink};
use crate::model::{CustomStatus, GatewayEvent, Id, Member, UserStatus};
use crate::voice::VoiceFeed;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::collections::VecDeque;
use std::hash::{BuildHasher, RandomState};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{sleep_until, timeout, Instant};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::http::header::{self, HeaderValue};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

const GATEWAY_URL: &str = "wss://gateway.discord.gg";
const QUERY: &str = "/?encoding=json&v=9&compress=zlib-stream";
/// 253 = LAZY_USER_NOTES | VERSIONED_READ_STATES | VERSIONED_USER_GUILD_SETTINGS |
/// DEDUPE_USER_OBJECTS | PRIORITIZED_READY_PAYLOAD | MULTIPLE_GUILD_EXPERIMENT_POPULATIONS |
/// NON_CHANNEL_READ_STATES, + 4096 AUTO_CALL_CONNECT (a CALL_CREATE for every ongoing DM call
/// after READY; READY itself is unchanged) = 4349. The notes' 509 adds AUTH_TOKEN_REFRESH, which
/// would hand us a new token that the REST client and keyring have no way to pick up.
const CAPABILITIES: u64 = 4349;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const HELLO_TIMEOUT: Duration = Duration::from_secs(30);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
/// Users have max_concurrency 1: one IDENTIFY per 5 s.
const IDENTIFY_SPACING: Duration = Duration::from_secs(5);
/// Gateway allows 120 sends per 60 s; keep headroom.
const SEND_LIMIT: usize = 110;
const SEND_WINDOW: Duration = Duration::from_secs(60);
const MAX_BACKOFF_SECS: u64 = 60;
/// Coalesces fast channel switching into one op 37.
const SUBSCRIBE_DEBOUNCE: Duration = Duration::from_millis(300);
/// Coalesces mute/deafen toggling into one op 4 (they share the 120 / 60 s budget).
const VOICE_DEBOUNCE: Duration = Duration::from_millis(250);
/// Coalesces status clicks into one op 3. The server allows 5 per 20 s; keep headroom.
const PRESENCE_DEBOUNCE: Duration = Duration::from_millis(250);
const PRESENCE_LIMIT: usize = 4;
const PRESENCE_WINDOW: Duration = Duration::from_secs(20);
/// Longest a PRESENCE_UPDATE waits for a UI pass (status dots lag by at most this).
const PRESENCE_REPAINT: Duration = Duration::from_secs(1);
/// `@` autocomplete: one op 8 per pause in typing (~600 ms, NIN).
const MEMBERS_DEBOUNCE: Duration = Duration::from_millis(600);
/// Sent op 8s whose GUILD_MEMBERS_CHUNK may still arrive.
const MEMBER_REQUESTS: usize = 8;
/// Names are at most 32 characters, so a longer query matches nobody.
const MAX_MEMBER_QUERY: usize = 32;
/// Longest RATE_LIMITED wait before the single op 8 retry (the documented limit is 30 s); a
/// longer one drops the query.
const MAX_MEMBERS_RETRY: Duration = Duration::from_secs(30);
/// Go Live ops waiting to be sent (each is a click; more means the connection is stuck).
const MAX_STREAM_OPS: usize = 8;
/// Non-1000 close code: keeps the session resumable.
const CLOSE_RESUMABLE: u16 = 4000;
/// IDENTIFYs in a row without a READY before giving up: each one is a session start, and the
/// backoff alone would still allow over a thousand per day.
const MAX_IDENTIFIES_WITHOUT_READY: u32 = 5;

/// How a connection ended.
enum End {
    Shutdown,
    AuthFailed,
    Fatal(String),
    Reconnect { resume: bool, min_wait: Duration },
}

impl End {
    fn resume() -> End {
        End::Reconnect { resume: true, min_wait: Duration::ZERO }
    }

    /// Turns a reconnect into a stop once too many IDENTIFYs went without a READY.
    fn limit_identifies(self, identifies_without_ready: u32) -> End {
        match self {
            End::Reconnect { .. } if identifies_without_ready >= MAX_IDENTIFIES_WITHOUT_READY => End::Fatal(
                "Could not start a session with the chat server after several attempts. Check your connection, then log in again."
                    .into(),
            ),
            end => end,
        }
    }
}

#[derive(Debug, PartialEq)]
enum CloseAction {
    Resume,
    Reidentify,
    AuthFailed,
    Fatal(&'static str),
}

fn close_action(code: u16) -> CloseAction {
    match code {
        4004 => CloseAction::AuthFailed,
        4003 | 4006 | 4007 | 4009 => CloseAction::Reidentify,
        4010 => CloseAction::Fatal("invalid shard"),
        4011 => CloseAction::Fatal("sharding required"),
        4012 => CloseAction::Fatal("invalid API version"),
        4013 => CloseAction::Fatal("invalid intents"),
        4014 => CloseAction::Fatal("disallowed intents"),
        4015 => CloseAction::Fatal("too many active sessions"),
        4016 => CloseAction::Fatal("connection request canceled"),
        // 1000/1001 from the server, 1006, 4000-4002, 4005, 4008 and anything unknown.
        _ => CloseAction::Resume,
    }
}

/// Uniform in [0, 1). Only for jitter.
fn jitter() -> f64 {
    let bits = RandomState::new().hash_one(std::time::SystemTime::now());
    (bits >> 11) as f64 / (1u64 << 53) as f64
}

/// Reconnect delay for the n-th consecutive attempt (n >= 1): within [cap/2, cap], cap doubling
/// from 1 s up to 60 s. `r` is the jitter in [0, 1).
fn backoff(attempt: u32, r: f64) -> Duration {
    let cap = (1u64 << attempt.saturating_sub(1).min(16)).min(MAX_BACKOFF_SECS) as f64;
    Duration::from_secs_f64(cap * (0.5 + 0.5 * r))
}

/// The upgrade request a browser sends: the same User-Agent, Origin and language as the REST
/// client and the IDENTIFY `browser_user_agent`, so the fingerprint stays consistent.
pub(super) fn upgrade_request(url: &str, props: &ClientProps) -> Result<Request, tokio_tungstenite::tungstenite::Error> {
    let mut req = url.into_client_request()?;
    let h = req.headers_mut();
    h.insert(header::USER_AGENT, HeaderValue::from_str(props.user_agent())?);
    h.insert(header::ORIGIN, HeaderValue::from_static("https://discord.com"));
    h.insert(header::ACCEPT_LANGUAGE, HeaderValue::from_str(&props.accept_language())?);
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    h.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    Ok(req)
}

/// Sliding-window limit on outgoing payloads. Heartbeats always pass but are counted.
struct SendLimiter {
    sent: VecDeque<Instant>,
    limit: usize,
    window: Duration,
}

impl Default for SendLimiter {
    /// The connection-wide limit.
    fn default() -> Self {
        SendLimiter::new(SEND_LIMIT, SEND_WINDOW)
    }
}

impl SendLimiter {
    fn new(limit: usize, window: Duration) -> Self {
        SendLimiter { sent: VecDeque::new(), limit, window }
    }

    /// Records a send, or returns when the next non-heartbeat send is allowed.
    fn acquire(&mut self, now: Instant, heartbeat: bool) -> Result<(), Instant> {
        while self.sent.front().is_some_and(|t| now.duration_since(*t) >= self.window) {
            self.sent.pop_front();
        }
        if !heartbeat && self.sent.len() >= self.limit {
            return Err(self.sent[self.sent.len() - self.limit] + self.window);
        }
        self.sent.push_back(now);
        Ok(())
    }
}

enum SendError {
    Limited(Instant),
    Ws,
}

/// One WebSocket connection.
struct Link {
    ws: Ws,
    limiter: SendLimiter,
    interval: Option<Duration>,
    next_beat: Instant,
    acked: bool,
    /// READY or RESUMED arrived on this connection.
    ready: bool,
    /// When to (re)try sending the wanted member-list subscription.
    flush_at: Option<Instant>,
}

impl Link {
    async fn send(&mut self, text: String, heartbeat: bool) -> Result<(), SendError> {
        self.limiter.acquire(Instant::now(), heartbeat).map_err(SendError::Limited)?;
        self.ws.send(WsMessage::text(text)).await.map_err(|e| {
            eprintln!("gateway: send failed: {e}");
            SendError::Ws
        })
    }

    async fn heartbeat(&mut self, seq: Option<u64>) -> Result<(), SendError> {
        self.send(json!({"op": 1, "d": seq}).to_string(), true).await
    }

    async fn close(&mut self, code: u16) {
        let frame = CloseFrame { code: CloseCode::from(code), reason: Default::default() };
        let _ = timeout(CLOSE_TIMEOUT, self.ws.close(Some(frame))).await;
    }

    /// Wake the flush at `t` at the latest. Earliest wins: every pending op re-arms its own later
    /// time when flushed early, so a 30 s op 8 retry or an op 3 limit never holds back op 37/op 4.
    fn flush_by(&mut self, t: Instant) {
        self.flush_at = Some(self.flush_at.map_or(t, |f| f.min(t)));
    }
}

/// An op 8 `query` request (§1.7).
#[derive(Debug)]
struct MemberQuery {
    guild_id: Id,
    query: String,
    limit: u8,
    /// Not before (debounce / RATE_LIMITED wait).
    due: Instant,
    /// Already re-sent once after RATE_LIMITED.
    retried: bool,
}

/// A sent op 8 waiting for its GUILD_MEMBERS_CHUNK(s).
#[derive(Debug)]
struct MemberRequest {
    nonce: String,
    q: MemberQuery,
    found: Vec<Member>,
}

struct Gateway {
    token: Token,
    props: Arc<ClientProps>,
    sink: EventSink,
    voice: VoiceFeed,
    cmds: mpsc::UnboundedReceiver<GatewayCommand>,
    /// (session_id, resume_gateway_url) from READY.
    session: Option<(String, String)>,
    seq: Option<u64>,
    my_id: Id,
    attempt: u32,
    last_identify: Option<Instant>,
    /// IDENTIFYs sent since the last READY.
    identifies_without_ready: u32,
    /// Member list the UI wants (guild, channel) and the one the server has for this session.
    want_sub: Option<(Id, Id)>,
    server_sub: Option<(Id, Id)>,
    /// Op 4 payload not sent yet (newest wins).
    want_voice: Option<String>,
    /// My status and custom status as last reported (READY, SESSIONS_REPLACE,
    /// USER_SETTINGS_UPDATE) or sent; op 3 always carries the custom status along.
    my_status: UserStatus,
    my_custom: Option<CustomStatus>,
    /// When `my_custom` expires (only user settings say); op 3 drops it after that.
    my_custom_expires: Option<chrono::DateTime<chrono::Utc>>,
    /// The saved status per user settings (READY `user_settings`, USER_SETTINGS_UPDATE), never
    /// from SESSIONS_REPLACE or our own op 3: the baseline for "changed on another client".
    settings_status: Option<UserStatus>,
    settings_custom: Option<Option<CustomStatus>>,
    /// My detected game (`SetActivities`): op 3 carries it next to the custom status.
    my_game: Option<crate::activity::GameActivity>,
    /// Rustcord's own presence (`SetActivities`): op 3 carries it after the game.
    my_app: Option<crate::activity::AppActivity>,
    /// Op 3 not sent yet (newest wins), and its `afk`.
    want_presence: Option<UserStatus>,
    afk: bool,
    presence_limiter: SendLimiter,
    /// Op 8 not sent yet (newest wins), and sent ones awaiting their chunks (oldest first).
    want_members: Option<MemberQuery>,
    member_requests: VecDeque<MemberRequest>,
    next_nonce: u32,
    /// Go Live op payloads not sent yet, in order.
    want_stream: VecDeque<String>,
    /// `ExpectToken`: until then a close 4004 waits for the rotated token (`SetToken`).
    token_due: Option<Instant>,
}

/// How long after `ExpectToken` a 4004 waits for the new token (the REST timeout plus slack).
const TOKEN_WAIT: Duration = Duration::from_secs(40);

pub(super) async fn run(
    token: Token,
    props: Arc<ClientProps>,
    sink: EventSink,
    voice: VoiceFeed,
    cmds: mpsc::UnboundedReceiver<GatewayCommand>,
) {
    let mut gw = Gateway::new(token, props, sink, voice, cmds);
    loop {
        gw.sink.send(BackendEvent::Conn(ConnState::Connecting));
        let end = match gw.connect_once().await.limit_identifies(gw.identifies_without_ready) {
            // A password / 2FA change ended the old token before its successor arrived.
            End::AuthFailed if gw.token_due.is_some() => match gw.await_token().await {
                Some(true) => End::Reconnect { resume: false, min_wait: Duration::ZERO },
                Some(false) => End::AuthFailed,
                None => End::Shutdown,
            },
            end => end,
        };
        let min_wait = match end {
            End::Shutdown => return,
            End::AuthFailed => {
                gw.sink.send(BackendEvent::Conn(ConnState::Offline));
                gw.sink.send(BackendEvent::AuthFailed("The login token was rejected. Please log in again.".into()));
                return;
            }
            End::Fatal(msg) => {
                gw.sink.send(BackendEvent::Conn(ConnState::Offline));
                gw.sink.send(BackendEvent::ActionFailed(msg));
                return;
            }
            End::Reconnect { resume, min_wait } => {
                if !resume {
                    gw.session = None;
                    gw.seq = None;
                }
                min_wait
            }
        };
        gw.attempt += 1;
        let mut delay = backoff(gw.attempt, jitter()).max(min_wait);
        if gw.session.is_none()
            && let Some(t) = gw.last_identify
        {
            delay = delay.max(IDENTIFY_SPACING.saturating_sub(t.elapsed()));
        }
        let in_secs = delay.as_secs_f64().ceil() as u32;
        gw.sink.send(BackendEvent::Conn(ConnState::Reconnecting { attempt: gw.attempt, in_secs }));
        if !gw.wait(delay).await {
            return;
        }
    }
}

impl Gateway {
    fn new(token: Token, props: Arc<ClientProps>, sink: EventSink, voice: VoiceFeed, cmds: mpsc::UnboundedReceiver<GatewayCommand>) -> Self {
        Gateway {
            token,
            props,
            sink,
            voice,
            cmds,
            session: None,
            seq: None,
            my_id: Id(0),
            attempt: 0,
            last_identify: None,
            identifies_without_ready: 0,
            want_sub: None,
            server_sub: None,
            want_voice: None,
            my_status: UserStatus::Online,
            my_custom: None,
            my_custom_expires: None,
            settings_status: None,
            settings_custom: None,
            my_game: None,
            my_app: None,
            want_presence: None,
            afk: false,
            presence_limiter: SendLimiter::new(PRESENCE_LIMIT, PRESENCE_WINDOW),
            want_members: None,
            member_requests: VecDeque::new(),
            next_nonce: 0,
            want_stream: VecDeque::new(),
            token_due: None,
        }
    }

    /// After a 4004 while a rotated token is expected: Some(true) once `SetToken` arrived,
    /// Some(false) if it did not in time, None on shutdown.
    async fn await_token(&mut self) -> Option<bool> {
        let until = self.token_due.take()?;
        let old = self.token.clone();
        loop {
            if self.token != old {
                return Some(true);
            }
            tokio::select! {
                _ = sleep_until(until) => return Some(false),
                cmd = self.cmds.recv() => if !self.note_cmd(cmd) { return None },
            }
        }
    }

    /// READY bookkeeping. `my_id` is only set here, so non-zero = this READY replaces an earlier
    /// session: the voice task ends the call (`VoiceFeed::session`), and an op 4 queued for the
    /// old session must not join or leave on this one. A join queued before the first READY stays.
    fn on_new_session(&mut self, me: Id) {
        if self.my_id != Id(0) {
            self.want_voice = None;
            self.want_stream.clear();
        }
        self.my_id = me;
    }

    /// Records a command received while not connected. Returns false on shutdown.
    fn note_cmd(&mut self, cmd: Option<GatewayCommand>) -> bool {
        match cmd {
            Some(GatewayCommand::SubscribeMemberList { guild_id, channel_id }) => {
                self.want_sub = Some((guild_id, channel_id));
                true
            }
            Some(GatewayCommand::UpdateVoiceState { guild_id, channel_id, self_mute, self_deaf, self_video }) => {
                self.want_voice = Some(voice_state_payload(guild_id, channel_id, self_mute, self_deaf, self_video));
                true
            }
            Some(GatewayCommand::UpdatePresence { status, afk }) => {
                (self.want_presence, self.afk) = (Some(status), afk);
                true
            }
            Some(GatewayCommand::SetActivities { game, app }) => {
                // Before READY `my_status` is only the default; READY queues the op 3 then.
                if (&game, &app) != (&self.my_game, &self.my_app) && self.session.is_some() {
                    self.want_presence = Some(self.want_presence.unwrap_or(self.my_status));
                }
                (self.my_game, self.my_app) = (game, app);
                true
            }
            Some(GatewayCommand::RequestMembers { guild_id, query, limit }) => {
                // Never `query:""`: that is the rate-limited "all members" form (moderators only).
                // Trimmed, so the wire query and the `MembersFound` echo match the UI's cache key.
                let query = query.trim().to_owned();
                let len = query.chars().count();
                if (1..=MAX_MEMBER_QUERY).contains(&len) {
                    let due = Instant::now() + MEMBERS_DEBOUNCE;
                    self.want_members = Some(MemberQuery { guild_id, query, limit: limit.clamp(1, 100), due, retried: false });
                }
                true
            }
            Some(GatewayCommand::Stream(op)) => {
                if self.want_stream.len() >= MAX_STREAM_OPS {
                    self.want_stream.pop_front();
                }
                self.want_stream.push_back(super::stream::payload(&op));
                true
            }
            Some(GatewayCommand::ExpectToken) => {
                self.token_due = Some(Instant::now() + TOKEN_WAIT);
                true
            }
            Some(GatewayCommand::SetToken(token)) => {
                (self.token, self.token_due) = (token, None);
                true
            }
            Some(GatewayCommand::Shutdown) | None => false,
        }
    }

    async fn wait(&mut self, delay: Duration) -> bool {
        let until = Instant::now() + delay;
        loop {
            tokio::select! {
                _ = sleep_until(until) => return true,
                cmd = self.cmds.recv() => if !self.note_cmd(cmd) { return false },
            }
        }
    }

    fn can_resume(&self) -> bool {
        self.session.is_some() && self.seq.is_some()
    }

    async fn connect_once(&mut self) -> End {
        let base = match &self.session {
            Some((_, url)) if self.seq.is_some() => url.as_str(),
            _ => GATEWAY_URL,
        };
        let url = format!("{}{QUERY}", base.trim_end_matches('/'));
        let req = match upgrade_request(&url, &self.props) {
            Ok(req) => req,
            Err(e) => {
                eprintln!("gateway: bad upgrade request: {e}");
                return End::resume();
            }
        };
        let connect = timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(req));
        tokio::pin!(connect);
        let ws = loop {
            tokio::select! {
                r = &mut connect => break r,
                cmd = self.cmds.recv() => if !self.note_cmd(cmd) { return End::Shutdown },
            }
        };
        let ws = match ws {
            Ok(Ok((ws, _))) => ws,
            Ok(Err(e)) => {
                eprintln!("gateway: connect failed: {e}");
                return End::resume();
            }
            Err(_) => {
                eprintln!("gateway: connect timed out");
                return End::resume();
            }
        };

        let mut link = Link {
            ws,
            limiter: SendLimiter::default(),
            interval: None,
            next_beat: Instant::now() + HELLO_TIMEOUT,
            acked: true,
            ready: false,
            flush_at: None,
        };
        let mut zlib = ZlibStream::new();
        loop {
            let flush_at = link.flush_at;
            tokio::select! {
                frame = link.ws.next() => {
                    let end = match frame {
                        Some(Ok(WsMessage::Text(t))) => self.on_payload(&mut link, t.as_bytes()).await,
                        Some(Ok(WsMessage::Binary(b))) => match zlib.push(&b) {
                            Ok(Some(json)) => self.on_payload(&mut link, &json).await,
                            Ok(None) => None,
                            Err(e) => {
                                eprintln!("gateway: decompression failed: {e}");
                                link.close(CLOSE_RESUMABLE).await;
                                Some(End::resume())
                            }
                        },
                        Some(Ok(WsMessage::Close(frame))) => {
                            let code = frame.as_ref().map_or(1005, |f| u16::from(f.code));
                            let reason = frame.as_ref().map(|f| f.reason.to_string()).unwrap_or_default();
                            eprintln!("gateway: closed by server ({code} {reason})");
                            Some(match close_action(code) {
                                CloseAction::Resume => End::resume(),
                                CloseAction::Reidentify => End::Reconnect { resume: false, min_wait: Duration::ZERO },
                                CloseAction::AuthFailed => End::AuthFailed,
                                CloseAction::Fatal(what) => End::Fatal(format!(
                                    "Disconnected from the chat server ({code}: {what}). Log in again to reconnect."
                                )),
                            })
                        }
                        Some(Ok(_)) => None, // ping/pong are answered by tungstenite
                        Some(Err(e)) => {
                            eprintln!("gateway: connection error: {e}");
                            Some(End::resume())
                        }
                        None => Some(End::resume()),
                    };
                    if let Some(end) = end {
                        return end;
                    }
                }
                _ = sleep_until(link.next_beat) => {
                    let Some(interval) = link.interval else {
                        eprintln!("gateway: no HELLO received");
                        link.close(CLOSE_RESUMABLE).await;
                        return End::resume();
                    };
                    if !link.acked {
                        eprintln!("gateway: heartbeat not acknowledged, reconnecting");
                        link.close(CLOSE_RESUMABLE).await;
                        return End::resume();
                    }
                    link.acked = false;
                    if link.heartbeat(self.seq).await.is_err() {
                        return End::resume();
                    }
                    link.next_beat = Instant::now() + interval;
                }
                _ = sleep_until(flush_at.unwrap_or(link.next_beat)), if flush_at.is_some() => {
                    if let Some(end) = self.flush_sub(&mut link).await {
                        return end;
                    }
                    if let Some(end) = self.flush_voice(&mut link).await {
                        return end;
                    }
                    if let Some(end) = self.flush_presence(&mut link).await {
                        return end;
                    }
                    if let Some(end) = self.flush_members(&mut link).await {
                        return end;
                    }
                    if let Some(end) = self.flush_stream(&mut link).await {
                        return end;
                    }
                }
                cmd = self.cmds.recv() => {
                    let debounce = match &cmd {
                        Some(GatewayCommand::SubscribeMemberList { .. }) => SUBSCRIBE_DEBOUNCE,
                        Some(GatewayCommand::UpdateVoiceState { .. }) => VOICE_DEBOUNCE,
                        Some(GatewayCommand::UpdatePresence { .. } | GatewayCommand::SetActivities { .. }) => PRESENCE_DEBOUNCE,
                        Some(GatewayCommand::RequestMembers { .. }) => MEMBERS_DEBOUNCE,
                        Some(GatewayCommand::Stream(_) | GatewayCommand::ExpectToken) => Duration::ZERO,
                        Some(GatewayCommand::SetToken(_)) => {
                            // The old token is void: a fresh IDENTIFY with the new one.
                            self.note_cmd(cmd);
                            link.close(1000).await;
                            return End::Reconnect { resume: false, min_wait: Duration::ZERO };
                        }
                        Some(GatewayCommand::Shutdown) | None => {
                            link.close(1000).await;
                            return End::Shutdown;
                        }
                    };
                    self.note_cmd(cmd);
                    if link.ready {
                        link.flush_by(Instant::now() + debounce);
                    }
                }
            }
        }
    }

    /// Handles one complete JSON payload. A payload that fails to parse is logged and skipped.
    async fn on_payload(&mut self, link: &mut Link, bytes: &[u8]) -> Option<End> {
        let head: Head = match serde_json::from_slice(bytes) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("gateway: unreadable payload: {e}");
                return None;
            }
        };
        if let Some(s) = head.s {
            self.seq = Some(s);
        }
        match head.op {
            0 => return self.on_dispatch(link, head.t.as_deref().unwrap_or(""), bytes).await,
            1 => {
                if link.heartbeat(self.seq).await.is_err() {
                    return Some(End::resume());
                }
            }
            7 => {
                link.close(CLOSE_RESUMABLE).await;
                return Some(End::resume());
            }
            9 => {
                let resumable = normalize::data::<Option<bool>>(bytes).ok().flatten().unwrap_or(false);
                let min_wait = Duration::from_secs_f64(1.0 + 4.0 * jitter());
                eprintln!("gateway: invalid session (resumable: {resumable})");
                if resumable && self.can_resume() {
                    link.close(CLOSE_RESUMABLE).await;
                    return Some(End::Reconnect { resume: true, min_wait });
                }
                // The session is dead anyway, so 1000 costs nothing; re-identify on the base URL.
                link.close(1000).await;
                return Some(End::Reconnect { resume: false, min_wait });
            }
            10 => {
                #[derive(serde::Deserialize)]
                struct Hello {
                    heartbeat_interval: u64,
                }
                let ms = normalize::data::<Hello>(bytes).map_or(41_250, |h| h.heartbeat_interval).max(1_000);
                let interval = Duration::from_millis(ms);
                link.interval = Some(interval);
                link.acked = true;
                link.next_beat = Instant::now() + interval.mul_f64(jitter());
                let payload = if self.can_resume() {
                    self.resume_payload()
                } else {
                    self.seq = None;
                    self.last_identify = Some(Instant::now());
                    self.identifies_without_ready += 1;
                    self.identify_payload()
                };
                if link.send(payload, false).await.is_err() {
                    return Some(End::resume());
                }
            }
            11 => link.acked = true,
            _ => {}
        }
        None
    }

    async fn on_dispatch(&mut self, link: &mut Link, t: &str, bytes: &[u8]) -> Option<End> {
        match t {
            "READY" => match normalize::ready(bytes) {
                Ok(r) => {
                    self.identifies_without_ready = 0;
                    let url = r.resume_url.unwrap_or_else(|| GATEWAY_URL.to_owned());
                    self.on_new_session(r.data.user.id);
                    self.voice.session(self.my_id, r.session_id.clone());
                    self.session = Some((r.session_id, url));
                    self.server_sub = None; // new session: nothing subscribed yet
                    self.sink.send(BackendEvent::Gateway(GatewayEvent::Ready(Box::new(r.data))));
                    if let Some(settings) = r.settings {
                        self.sink.send(BackendEvent::Gateway(GatewayEvent::UserSettingsProto { settings, partial: false }));
                    }
                    // A new session starts without my activities: send them again with my real status.
                    let known = r.status.status.is_some();
                    self.on_status(r.status, false);
                    if known && (self.my_game.is_some() || self.my_app.is_some()) && self.want_presence.is_none() {
                        (self.want_presence, self.afk) = (Some(self.my_status), false);
                    }
                    self.connected(link);
                }
                Err(e) => {
                    // READY is the account's own state, so a retry would fail the same way and
                    // only burn another session start: stop instead.
                    eprintln!("gateway: failed to parse READY: {e}");
                    link.close(1000).await;
                    return Some(End::Fatal(
                        "Could not read the session data from the server. Please update Rustcord or log in again.".into(),
                    ));
                }
            },
            "RESUMED" => {
                self.sink.send(BackendEvent::Gateway(GatewayEvent::Resumed));
                self.connected(link);
            }
            "USER_SETTINGS_UPDATE" | "SESSIONS_REPLACE" => {
                let follow = t == "USER_SETTINGS_UPDATE";
                let patch = if follow {
                    normalize::settings_update(bytes)
                } else {
                    normalize::sessions_replace(bytes, self.session.as_ref().map_or("", |(id, _)| id.as_str()))
                };
                match patch {
                    Ok(p) => {
                        if let Some(at) = self.on_status(p, follow) {
                            link.flush_by(at);
                        }
                    }
                    Err(e) => eprintln!("gateway: failed to parse {t}: {e}"),
                }
                // My activity (user panel, my card) when no member list carries me.
                if !follow && let Ok(Some(p)) = normalize::my_presence(bytes) {
                    self.sink.send(BackendEvent::Gateway(GatewayEvent::PresenceUpdate(vec![(self.my_id, p)])));
                }
            }
            "GUILD_MEMBERS_CHUNK" => match normalize::members_chunk(bytes) {
                Ok(c) => self.on_members_chunk(c),
                Err(e) => eprintln!("gateway: failed to parse {t}: {e}"),
            },
            "RATE_LIMITED" => match normalize::rate_limited(bytes) {
                Ok(r) => {
                    if let Some(at) = self.on_rate_limited(r) {
                        link.flush_by(at);
                    }
                }
                Err(e) => eprintln!("gateway: failed to parse {t}: {e}"),
            },
            _ => match normalize::dispatch(t, bytes, self.my_id) {
                Ok(events) => {
                    for ev in events {
                        // Carry a voice/stream token or only matter to Go Live: the voice module
                        // needs them, the UI never sees them.
                        if matches!(
                            ev,
                            GatewayEvent::VoiceServerUpdate { .. }
                                | GatewayEvent::StreamCreate { .. }
                                | GatewayEvent::StreamServerUpdate { .. }
                                | GatewayEvent::StreamDelete { .. }
                        ) {
                            self.voice.send(ev);
                            continue;
                        }
                        if let GatewayEvent::VoiceStateUpdate(v) = &ev
                            && v.user_id == self.my_id
                        {
                            self.voice.send(ev.clone());
                        }
                        // Mostly rich-presence churn: at most one UI pass per second for it.
                        if matches!(ev, GatewayEvent::PresenceUpdate(_)) {
                            self.sink.send_after(BackendEvent::Gateway(ev), PRESENCE_REPAINT);
                        } else {
                            self.sink.send(BackendEvent::Gateway(ev));
                        }
                    }
                }
                Err(e) => eprintln!("gateway: failed to parse {t}: {e}"),
            },
        }
        None
    }

    fn connected(&mut self, link: &mut Link) {
        link.ready = true;
        self.attempt = 0;
        self.sink.send(BackendEvent::Conn(ConnState::Connected));
        if (self.want_sub.is_some() && self.want_sub != self.server_sub)
            || self.want_voice.is_some()
            || self.want_presence.is_some()
            || self.want_members.is_some()
            || !self.want_stream.is_empty()
        {
            link.flush_at = Some(Instant::now());
        }
    }

    fn emit_status(&self) {
        let (status, custom_status) = (self.my_status, self.my_custom.clone());
        self.sink.send(BackendEvent::Gateway(GatewayEvent::MyStatus { status, custom_status }));
    }

    /// Merges what the server says about my status and tells the UI. `follow`: the user changed
    /// it on another client (USER_SETTINGS_UPDATE), so this session follows with an op 3
    /// (FEATURES_NOTES §3.1.2); returns when to flush it. "Changed" is judged against the saved
    /// settings, not `my_status`: another client's op 3 (SESSIONS_REPLACE) arrives first.
    fn on_status(&mut self, p: StatusPatch, follow: bool) -> Option<Instant> {
        if p.status.is_none() && p.custom.is_none() {
            return None;
        }
        let mut changed = false;
        if p.settings {
            if let Some(s) = p.status {
                changed |= self.settings_status.replace(s) != Some(s);
            }
            if let Some(c) = &p.custom {
                changed |= self.settings_custom.replace(c.clone()).as_ref() != Some(c);
            }
        }
        if let Some(s) = p.status {
            self.my_status = s;
        }
        if let Some(c) = p.custom {
            // A session list carries no expiry: keep the known one while the status is the same.
            if p.settings || c != self.my_custom {
                self.my_custom_expires = p.expires;
            }
            self.my_custom = c;
        }
        self.emit_status();
        (follow && changed).then(|| {
            (self.want_presence, self.afk) = (Some(self.my_status), false);
            Instant::now() + PRESENCE_DEBOUNCE
        })
    }

    /// Forgets my custom status once it has expired, so op 3 does not publish it again (whether
    /// the server clears it is UNCONFIRMED, FEATURES_NOTES §1.3).
    fn drop_expired_custom(&mut self, now: chrono::DateTime<chrono::Utc>) {
        if self.my_custom_expires.is_some_and(|t| t <= now) {
            self.my_custom = None;
            self.my_custom_expires = None;
        }
    }

    /// Collects an op 8 answer; the last chunk goes to the UI.
    fn on_members_chunk(&mut self, c: normalize::MembersChunk) {
        let Some(i) = self.member_requests.iter().position(|r| c.nonce.as_deref() == Some(r.nonce.as_str())) else { return };
        self.member_requests[i].found.extend(c.members);
        if c.chunk_index + 1 >= c.chunk_count
            && let Some(r) = self.member_requests.remove(i)
        {
            self.sink.send(BackendEvent::Chat(ChatEvent::MembersFound { guild_id: r.q.guild_id, query: r.q.query, members: r.found }));
        }
    }

    /// RATE_LIMITED for an op 8: retry once after `retry_after`, unless a newer query is
    /// pending. Returns when to flush the retry.
    fn on_rate_limited(&mut self, r: normalize::RateLimited) -> Option<Instant> {
        eprintln!("gateway: op {} rate limited ({:.1} s)", r.opcode, r.retry_after);
        if r.opcode != 8 {
            return None;
        }
        let i = match &r.meta.nonce {
            Some(n) => self.member_requests.iter().position(|x| &x.nonce == n),
            None => self.member_requests.len().checked_sub(1),
        };
        let req = self.member_requests.remove(i?)?;
        // A longer wait is dropped, not cut short: an early retry is only limited again.
        if req.q.retried || self.want_members.is_some() || !(0.0..=MAX_MEMBERS_RETRY.as_secs_f64()).contains(&r.retry_after) {
            return None;
        }
        let wait = Duration::from_secs_f64(r.retry_after);
        let due = Instant::now() + wait;
        self.want_members = Some(MemberQuery { due, retried: true, ..req.q });
        Some(due)
    }

    /// Sends the pending op 3, if any.
    async fn flush_presence(&mut self, link: &mut Link) -> Option<End> {
        let status = self.want_presence?;
        if !link.ready {
            return None;
        }
        // A slot taken here is wasted if the connection limit below defers the send; that only
        // delays a later op 3.
        if let Err(at) = self.presence_limiter.acquire(Instant::now(), false) {
            link.flush_by(at);
            return None;
        }
        // The UI hears of a dropped custom status with the `emit_status` after the send.
        self.drop_expired_custom(chrono::Utc::now());
        match link.send(presence_payload(status, self.afk, self.my_custom.as_ref(), self.my_game.as_ref(), self.my_app.as_ref(), unix_ms()), false).await {
            Ok(()) => {
                self.want_presence = None;
                self.my_status = status;
                self.emit_status();
            }
            Err(SendError::Limited(at)) => link.flush_by(at),
            Err(SendError::Ws) => return Some(End::resume()),
        }
        None
    }

    /// Sends the pending op 8, if any, once it is due.
    async fn flush_members(&mut self, link: &mut Link) -> Option<End> {
        let q = self.want_members.as_ref()?;
        if !link.ready {
            return None;
        }
        if q.due > Instant::now() {
            link.flush_by(q.due);
            return None;
        }
        self.next_nonce = self.next_nonce.wrapping_add(1);
        let nonce = format!("rc{}", self.next_nonce);
        match link.send(members_payload(q, &nonce), false).await {
            Ok(()) => {
                let q = self.want_members.take()?;
                if self.member_requests.len() >= MEMBER_REQUESTS {
                    self.member_requests.pop_front();
                }
                self.member_requests.push_back(MemberRequest { nonce, q, found: Vec::new() });
            }
            Err(SendError::Limited(at)) => link.flush_by(at),
            Err(SendError::Ws) => return Some(End::resume()),
        }
        None
    }

    /// Sends the pending op 4, if any.
    async fn flush_voice(&mut self, link: &mut Link) -> Option<End> {
        let payload = self.want_voice.clone()?;
        if !link.ready {
            return None;
        }
        match link.send(payload, false).await {
            Ok(()) => self.want_voice = None,
            Err(SendError::Limited(at)) => link.flush_by(at),
            Err(SendError::Ws) => return Some(End::resume()),
        }
        None
    }

    /// Sends the pending Go Live ops, in order.
    async fn flush_stream(&mut self, link: &mut Link) -> Option<End> {
        while let Some(payload) = self.want_stream.front().cloned() {
            if !link.ready {
                return None;
            }
            match link.send(payload, false).await {
                Ok(()) => _ = self.want_stream.pop_front(),
                Err(SendError::Limited(at)) => {
                    link.flush_by(at);
                    return None;
                }
                Err(SendError::Ws) => return Some(End::resume()),
            }
        }
        None
    }

    /// Sends op 37 for the wanted member list (and drops the previous guild's list).
    async fn flush_sub(&mut self, link: &mut Link) -> Option<End> {
        link.flush_at = None;
        let (guild_id, channel_id) = self.want_sub?;
        if !link.ready || self.server_sub == self.want_sub {
            return None;
        }
        let mut subs = serde_json::Map::new();
        if let Some((old_guild, _)) = self.server_sub.filter(|(g, _)| *g != guild_id) {
            subs.insert(old_guild.to_string(), json!({"channels": {}}));
        }
        subs.insert(
            guild_id.to_string(),
            json!({
                "typing": true,
                "activities": true,
                "threads": true,
                "channels": { channel_id.to_string(): [[0, 99]] },
            }),
        );
        let payload = json!({"op": 37, "d": {"subscriptions": subs}}).to_string();
        match link.send(payload, false).await {
            Ok(()) => self.server_sub = self.want_sub,
            Err(SendError::Limited(at)) => link.flush_at = Some(at),
            Err(SendError::Ws) => return Some(End::resume()),
        }
        None
    }

    fn identify_payload(&self) -> String {
        json!({
            "op": 2,
            "d": {
                "token": self.token.expose(),
                "capabilities": CAPABILITIES,
                "properties": &*self.props,
                "presence": {"status": "unknown", "since": 0, "activities": [], "afk": false},
                "compress": false,
                "client_state": {"guild_versions": {}, "api_code_version": 0},
            }
        })
        .to_string()
    }

    fn resume_payload(&self) -> String {
        let session_id = self.session.as_ref().map_or("", |(id, _)| id.as_str());
        json!({
            "op": 6,
            "d": {"token": self.token.expose(), "session_id": session_id, "seq": self.seq},
        })
        .to_string()
    }
}

/// Op 4 Update Voice State. `flags` 0: no clips (VOICE_NOTES §1.1).
// shortcut: no preferred_region(s) (READY geo_ordered_rtc_regions); the server picks one.
fn voice_state_payload(guild_id: Option<Id>, channel_id: Option<Id>, self_mute: bool, self_deaf: bool, self_video: bool) -> String {
    json!({"op": 4, "d": {
        "guild_id": guild_id, "channel_id": channel_id,
        "self_mute": self_mute, "self_deaf": self_deaf, "self_video": self_video, "flags": 0,
    }})
    .to_string()
}

/// Op 3 (FEATURES_NOTES §1.3). It replaces the session's activity list, so the custom status
/// (type 4), my detected game (type 0, like the official client's: name, application id, start
/// time) and Rustcord's own presence (type 0, left out while Invisible) go along, in that order.
/// `since` = when idle started (unix ms), else 0.
fn presence_payload(
    status: UserStatus,
    afk: bool,
    custom: Option<&CustomStatus>,
    game: Option<&crate::activity::GameActivity>,
    app: Option<&crate::activity::AppActivity>,
    now_ms: u64,
) -> String {
    let activities: Vec<serde_json::Value> = custom
        .map(|c| {
            let mut a = json!({"type": 4, "name": "Custom Status"});
            if let Some(text) = &c.text {
                a["state"] = json!(text);
            }
            if let Some(e) = &c.emoji {
                a["emoji"] = match e.id {
                    Some(id) => json!({"id": id, "name": e.name, "animated": e.animated}),
                    None => json!({"name": e.name}),
                };
            }
            a
        })
        .into_iter()
        .chain(game.map(|g| {
            let mut a = json!({"type": 0, "name": g.name, "timestamps": {"start": g.start_ms}});
            if let Some(id) = g.application_id {
                a["application_id"] = json!(id);
            }
            a
        }))
        .chain(app.filter(|_| status != UserStatus::Invisible).map(|g| {
            let mut a = json!({
                "type": 0,
                "name": crate::config::APP_NAME,
                "details": crate::activity::APP_DETAILS,
                "state": format!("Version {}", env!("CARGO_PKG_VERSION")),
                "timestamps": {"start": g.start_ms},
            });
            // Buttons need the application: others' clients fetch the link by its id.
            if let Some(id) = g.application_id {
                a["application_id"] = json!(id);
                a["buttons"] = json!([crate::activity::APP_BUTTON.0]);
                a["metadata"] = json!({"button_urls": [crate::activity::APP_BUTTON.1]});
            }
            a
        }))
        .collect();
    let since = if status == UserStatus::Idle { now_ms } else { 0 };
    json!({"op": 3, "d": {"status": status.as_str(), "since": since, "activities": activities, "afk": afk}}).to_string()
}

/// Op 8 with `query` (§1.7); never presences.
fn members_payload(q: &MemberQuery, nonce: &str) -> String {
    json!({"op": 8, "d": {"guild_id": [q.guild_id], "query": q.query, "limit": q.limit, "presences": false, "nonce": nonce}})
        .to_string()
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_state_payloads() {
        let join: serde_json::Value = serde_json::from_str(&voice_state_payload(Some(Id(1)), Some(Id(2)), true, false, false)).unwrap();
        assert_eq!(
            join,
            json!({"op":4,"d":{"guild_id":"1","channel_id":"2","self_mute":true,"self_deaf":false,"self_video":false,"flags":0}})
        );
        let leave: serde_json::Value = serde_json::from_str(&voice_state_payload(None, None, false, false, false)).unwrap();
        assert_eq!((&leave["d"]["guild_id"], &leave["d"]["channel_id"]), (&json!(null), &json!(null)));
    }

    #[tokio::test]
    async fn reidentify_drops_a_queued_op4() {
        let sink = EventSink::new(std::sync::mpsc::channel().0, egui::Context::default());
        let voice = crate::voice::VoiceManager::spawn(sink.clone(), Arc::default()).feed();
        let props = Arc::new(ClientProps::new(&crate::config::Settings::default()));
        let token = Token::parse(&"t".repeat(30)).unwrap();
        let mut gw = Gateway::new(token, props, sink, voice, mpsc::unbounded_channel().1);
        let join = || {
            Some(GatewayCommand::UpdateVoiceState { guild_id: Some(Id(1)), channel_id: Some(Id(2)), self_mute: false, self_deaf: false, self_video: false })
        };
        assert!(gw.note_cmd(join()));
        gw.on_new_session(Id(9));
        assert!(gw.want_voice.is_some(), "a join before the first READY is still sent");
        gw.note_cmd(join());
        gw.on_new_session(Id(9));
        assert!(gw.want_voice.is_none(), "re-identify: the old session's op 4 is dropped");
        assert_eq!(gw.my_id, Id(9));
    }

    fn gateway() -> (Gateway, std::sync::mpsc::Receiver<BackendEvent>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let sink = EventSink::new(tx, egui::Context::default());
        let voice = crate::voice::VoiceManager::spawn(sink.clone(), Arc::default()).feed();
        let props = Arc::new(ClientProps::new(&crate::config::Settings::default()));
        let token = Token::parse(&"t".repeat(30)).unwrap();
        (Gateway::new(token, props, sink, voice, mpsc::unbounded_channel().1), rx)
    }

    #[tokio::test]
    async fn a_rotated_token_answers_a_4004_while_expected() {
        let (mut gw, _rx) = gateway();
        let (tx, cmds) = mpsc::unbounded_channel();
        gw.cmds = cmds;
        let new = Token::parse(&"n".repeat(30)).unwrap();
        assert!(gw.note_cmd(Some(GatewayCommand::ExpectToken)));
        tx.send(GatewayCommand::SetToken(new.clone())).unwrap();
        assert_eq!(gw.await_token().await, Some(true));
        assert!(gw.token == new && gw.token_due.is_none());
        // Expected but never sent in time: the 4004 stands.
        gw.token_due = Some(Instant::now());
        assert_eq!(gw.await_token().await, Some(false));
        gw.token_due = Some(Instant::now() + TOKEN_WAIT);
        tx.send(GatewayCommand::Shutdown).unwrap();
        assert_eq!(gw.await_token().await, None);
    }

    #[test]
    fn presence_and_members_payloads() {
        let v = |s: String| serde_json::from_str::<serde_json::Value>(&s).unwrap();
        let custom = CustomStatus { text: Some("brb".into()), emoji: Some(crate::model::Emoji { name: Some("🍕".into()), ..Default::default() }) };
        assert_eq!(
            v(presence_payload(UserStatus::Dnd, false, Some(&custom), None, None, 5)),
            json!({"op":3,"d":{"status":"dnd","since":0,"afk":false,"activities":[
                {"type":4,"name":"Custom Status","state":"brb","emoji":{"name":"🍕"}}]}})
        );
        let custom = CustomStatus { text: None, emoji: Some(crate::model::Emoji { id: Some(Id(9)), name: Some("pepe".into()), animated: true }) };
        let idle = v(presence_payload(UserStatus::Idle, true, Some(&custom), None, None, 1_700_000_000_000));
        assert_eq!((&idle["d"]["since"], &idle["d"]["status"], &idle["d"]["afk"]), (&json!(1_700_000_000_000u64), &json!("idle"), &json!(true)));
        assert_eq!(idle["d"]["activities"][0], json!({"type":4,"name":"Custom Status","emoji":{"id":"9","name":"pepe","animated":true}}));
        assert_eq!(v(presence_payload(UserStatus::Invisible, false, None, None, None, 5))["d"]["activities"], json!([]));
        // My game goes next to the custom status, which it never replaces.
        let game = crate::activity::GameActivity { name: "Factorio".into(), application_id: Some(Id(427520)), start_ms: 1_700_000_000_000 };
        let custom = CustomStatus { text: Some("brb".into()), emoji: None };
        assert_eq!(
            v(presence_payload(UserStatus::Online, false, Some(&custom), Some(&game), None, 5))["d"]["activities"],
            json!([{"type":4,"name":"Custom Status","state":"brb"},
                   {"type":0,"name":"Factorio","application_id":"427520","timestamps":{"start":1_700_000_000_000u64}}])
        );
        let added = crate::activity::GameActivity { application_id: None, ..game.clone() };
        assert_eq!(v(presence_payload(UserStatus::Dnd, false, None, Some(&added), None, 5))["d"]["activities"], json!([{"type":0,"name":"Factorio","timestamps":{"start":1_700_000_000_000u64}}]));
        // Rustcord's own presence comes last, with fixed texts; the Application ID only when set; never while invisible.
        let app = crate::activity::AppActivity { application_id: None, start_ms: 1_700_000_001_000 };
        let own = json!({"type":0,"name":"Rustcord","details":"Third-party client, built in Rust","state":concat!("Version ", env!("CARGO_PKG_VERSION")),"timestamps":{"start":1_700_000_001_000u64}});
        let list = v(presence_payload(UserStatus::Online, false, Some(&custom), Some(&game), Some(&app), 5))["d"]["activities"].clone();
        assert_eq!((list[0]["type"].clone(), list[1]["name"].clone(), list[2].clone()), (json!(4), json!("Factorio"), own.clone()));
        let with_id = crate::activity::AppActivity { application_id: Some(Id(123456789012345678)), ..app.clone() };
        let own_with_id = v(presence_payload(UserStatus::Idle, false, None, None, Some(&with_id), 5))["d"]["activities"][0].clone();
        assert_eq!(
            [&own_with_id["application_id"], &own_with_id["buttons"], &own_with_id["metadata"]],
            [&json!("123456789012345678"), &json!(["Get Rustcord"]), &json!({"button_urls": ["https://getrustcord.netlify.app/"]})],
            "the button goes along with an application id only (own above has none)"
        );
        assert_eq!(v(presence_payload(UserStatus::Invisible, false, None, Some(&game), Some(&app), 5))["d"]["activities"].as_array().map(Vec::len), Some(1));

        let q = MemberQuery { guild_id: Id(10), query: "ab".into(), limit: 50, due: Instant::now(), retried: false };
        assert_eq!(
            v(members_payload(&q, "rc1")),
            json!({"op":8,"d":{"guild_id":["10"],"query":"ab","limit":50,"presences":false,"nonce":"rc1"}})
        );
    }

    #[tokio::test]
    async fn stream_ops_queue_in_order_bounded_and_die_with_the_session() {
        let (mut gw, _rx) = gateway();
        for i in 0..MAX_STREAM_OPS + 2 {
            assert!(gw.note_cmd(Some(GatewayCommand::Stream(super::super::StreamOp::Watch { stream_key: format!("call:1:{i}") }))));
        }
        assert_eq!(gw.want_stream.len(), MAX_STREAM_OPS);
        let first: serde_json::Value = serde_json::from_str(&gw.want_stream[0]).unwrap();
        assert_eq!(first, json!({"op":20,"d":{"stream_key":"call:1:2"}}), "oldest dropped, in order");
        gw.on_new_session(Id(9));
        assert_eq!(gw.want_stream.len(), MAX_STREAM_OPS, "queued before the first READY: still sent");
        gw.on_new_session(Id(9));
        assert!(gw.want_stream.is_empty(), "re-identify: the old session's streams are gone");
    }

    #[tokio::test]
    async fn member_requests_and_chunks() {
        let (mut gw, rx) = gateway();
        let req = |query: &str, limit| Some(GatewayCommand::RequestMembers { guild_id: Id(10), query: query.into(), limit });
        gw.note_cmd(req("  ", 50));
        gw.note_cmd(req(&"x".repeat(MAX_MEMBER_QUERY + 1), 50));
        assert!(gw.want_members.is_none(), "empty and too long queries are never sent");
        gw.note_cmd(req("a", 50));
        gw.note_cmd(req(" ab ", 0));
        let q = gw.want_members.take().unwrap();
        assert_eq!((q.query.as_str(), q.limit), ("ab", 1), "newest wins, limit clamped");

        // Two chunks for one nonce -> one MembersFound; unknown nonces are ignored.
        gw.member_requests.push_back(MemberRequest { nonce: "rc1".into(), q, found: Vec::new() });
        let member = |id| Member { user: Some(crate::model::User { id: Id(id), ..Default::default() }), ..Default::default() };
        let chunk = |nonce: &str, i, members| normalize::MembersChunk { members, chunk_index: i, chunk_count: 2, nonce: Some(nonce.into()) };
        gw.on_members_chunk(chunk("rc9", 0, vec![member(1)]));
        gw.on_members_chunk(chunk("rc1", 0, vec![member(2)]));
        assert!(rx.try_recv().is_err());
        gw.on_members_chunk(chunk("rc1", 1, vec![member(3)]));
        let Ok(BackendEvent::Chat(ChatEvent::MembersFound { guild_id, query, members })) = rx.try_recv() else { panic!() };
        assert_eq!((guild_id, query.as_str(), members.len()), (Id(10), "ab", 2));
        assert!(gw.member_requests.is_empty());
    }

    #[tokio::test]
    async fn rate_limited_op8_retries_once() {
        let (mut gw, _rx) = gateway();
        let q = |retried| MemberQuery { guild_id: Id(10), query: "ab".into(), limit: 50, due: Instant::now(), retried };
        let limited = |nonce: Option<&str>| normalize::RateLimited {
            opcode: 8,
            retry_after: 2.0,
            meta: normalize::RateLimitMeta { nonce: nonce.map(Into::into) },
        };
        gw.member_requests.push_back(MemberRequest { nonce: "rc1".into(), q: q(false), found: Vec::new() });
        let at = gw.on_rate_limited(limited(Some("rc1"))).unwrap();
        assert!(at >= Instant::now() + Duration::from_secs(1));
        let w = gw.want_members.take().unwrap();
        assert!(w.retried && w.due == at && gw.member_requests.is_empty());

        gw.member_requests.push_back(MemberRequest { nonce: "rc2".into(), q: q(true), found: Vec::new() });
        assert_eq!(gw.on_rate_limited(limited(None)), None, "the retry is not retried");
        assert!(gw.want_members.is_none() && gw.member_requests.is_empty());
        gw.member_requests.push_back(MemberRequest { nonce: "rc3".into(), q: q(false), found: Vec::new() });
        assert_eq!(gw.on_rate_limited(limited(Some("rc4"))), None, "unknown nonce");
        assert_eq!(gw.on_rate_limited(normalize::RateLimited { opcode: 3, ..limited(Some("rc3")) }), None);
        assert_eq!(gw.member_requests.len(), 1);
        let long = normalize::RateLimited { retry_after: 45.0, ..limited(Some("rc3")) };
        assert_eq!(gw.on_rate_limited(long), None, "a wait over 30 s drops the query");
        assert!(gw.want_members.is_none() && gw.member_requests.is_empty());
    }

    #[tokio::test]
    async fn status_merge_and_follow() {
        let (mut gw, rx) = gateway();
        let custom = Some(CustomStatus { text: Some("hi".into()), emoji: None });
        let settings = |status, custom| StatusPatch { status, custom, settings: true, ..Default::default() };
        let sessions = |status| StatusPatch { status: Some(status), ..Default::default() };
        assert_eq!(gw.on_status(StatusPatch { settings: true, ..Default::default() }, true), None);
        assert!(rx.try_recv().is_err(), "nothing to report");
        // READY: no op 3.
        assert_eq!(gw.on_status(settings(Some(UserStatus::Dnd), Some(custom.clone())), false), None);
        let Ok(BackendEvent::Gateway(GatewayEvent::MyStatus { status, custom_status })) = rx.try_recv() else { panic!() };
        assert_eq!((status, &custom_status), (UserStatus::Dnd, &custom));
        // A settings echo without change: no op 3.
        assert_eq!(gw.on_status(settings(Some(UserStatus::Dnd), None), true), None);
        assert!(gw.want_presence.is_none());
        // Changed on another client: follow, keeping the custom status.
        assert!(gw.on_status(settings(Some(UserStatus::Idle), None), true).is_some());
        assert_eq!((gw.want_presence, &gw.my_custom), (Some(UserStatus::Idle), &custom));
        gw.want_presence = None;
        assert!(gw.on_status(settings(None, Some(None)), true).is_some(), "custom status cleared");
        assert_eq!(gw.want_presence, Some(UserStatus::Idle));
        // SESSIONS_REPLACE never sends op 3.
        gw.want_presence = None;
        assert_eq!(gw.on_status(sessions(UserStatus::Online), false), None);
        assert_eq!((gw.my_status, gw.want_presence), (UserStatus::Online, None));
        // Another client's op 3 (SESSIONS_REPLACE) before its settings write: still followed.
        assert_eq!(gw.on_status(sessions(UserStatus::Dnd), false), None);
        assert!(gw.on_status(settings(Some(UserStatus::Dnd), None), true).is_some());
        assert_eq!(gw.want_presence, Some(UserStatus::Dnd));
        // The user's choice is kept until sent.
        gw.note_cmd(Some(GatewayCommand::UpdatePresence { status: UserStatus::Idle, afk: true }));
        assert_eq!((gw.want_presence, gw.afk), (Some(UserStatus::Idle), true));
        // A follow is never afk.
        assert!(gw.on_status(settings(Some(UserStatus::Online), None), true).is_some());
        assert!(!gw.afk);
        gw.note_cmd(Some(GatewayCommand::UpdatePresence { status: UserStatus::Invisible, afk: false }));
        assert_eq!(gw.want_presence, Some(UserStatus::Invisible));
    }

    #[tokio::test]
    async fn activity_changes_queue_one_op3_with_my_status() {
        let (mut gw, _rx) = gateway();
        let game = Some(crate::activity::GameActivity { name: "Factorio".into(), application_id: None, start_ms: 1 });
        let set = |game: Option<crate::activity::GameActivity>, app| Some(GatewayCommand::SetActivities { game, app });
        // Before READY my status is unknown: only remembered.
        gw.note_cmd(set(game.clone(), None));
        assert_eq!((gw.want_presence, &gw.my_game), (None, &game));
        gw.session = Some(("s".into(), GATEWAY_URL.into()));
        gw.my_status = UserStatus::Dnd;
        gw.note_cmd(set(game.clone(), None));
        assert_eq!(gw.want_presence, None, "no change, no op 3");
        gw.note_cmd(set(None, None));
        assert_eq!((gw.want_presence, &gw.my_game), (Some(UserStatus::Dnd), &None));
        // Rustcord's own presence alone also queues one op 3, and the same pair again none.
        gw.want_presence = None;
        let app = Some(crate::activity::AppActivity { application_id: None, start_ms: 2 });
        gw.note_cmd(set(None, app.clone()));
        assert_eq!((gw.want_presence, &gw.my_app), (Some(UserStatus::Dnd), &app));
        gw.want_presence = None;
        gw.note_cmd(set(None, app.clone()));
        assert_eq!(gw.want_presence, None);
        // A status picked meanwhile is kept.
        gw.note_cmd(Some(GatewayCommand::UpdatePresence { status: UserStatus::Idle, afk: true }));
        gw.note_cmd(set(game, app));
        assert_eq!((gw.want_presence, gw.afk), (Some(UserStatus::Idle), true));
    }

    #[tokio::test]
    async fn expired_custom_status_is_not_republished() {
        let (mut gw, _rx) = gateway();
        let now = chrono::Utc::now();
        let custom = Some(CustomStatus { text: Some("brb".into()), emoji: None });
        let expires = Some(now + chrono::TimeDelta::minutes(30));
        gw.on_status(StatusPatch { custom: Some(custom.clone()), expires, settings: true, ..Default::default() }, false);
        // A session list with the same custom status keeps the expiry; a different one has none.
        gw.on_status(StatusPatch { custom: Some(custom.clone()), ..Default::default() }, false);
        assert_eq!(gw.my_custom_expires, expires);
        gw.drop_expired_custom(now);
        assert_eq!(gw.my_custom, custom, "not expired yet");
        gw.drop_expired_custom(now + chrono::TimeDelta::minutes(31));
        assert_eq!((gw.my_custom.as_ref(), gw.my_custom_expires), (None, None));
        let other = Some(CustomStatus { text: Some("x".into()), emoji: None });
        gw.on_status(StatusPatch { custom: Some(custom), expires, settings: true, ..Default::default() }, false);
        gw.on_status(StatusPatch { custom: Some(other.clone()), ..Default::default() }, false);
        gw.drop_expired_custom(now + chrono::TimeDelta::minutes(31));
        assert_eq!(gw.my_custom, other, "no known expiry");
    }

    #[test]
    fn close_codes() {
        assert_eq!(close_action(4004), CloseAction::AuthFailed);
        for c in [4010, 4011, 4012, 4013, 4014, 4015, 4016] {
            assert!(matches!(close_action(c), CloseAction::Fatal(_)), "{c}");
        }
        for c in [4003, 4006, 4007, 4009] {
            assert_eq!(close_action(c), CloseAction::Reidentify, "{c}");
        }
        for c in [1000, 1001, 1005, 1006, 4000, 4001, 4002, 4005, 4008, 4999] {
            assert_eq!(close_action(c), CloseAction::Resume, "{c}");
        }
    }

    #[test]
    fn backoff_bounds() {
        let r_max = 1.0 - f64::EPSILON;
        assert!(backoff(1, 0.0) >= Duration::from_millis(500));
        assert!(backoff(1, r_max) <= Duration::from_secs(1));
        assert!(backoff(2, r_max) <= Duration::from_secs(2));
        assert!(backoff(3, 0.0) >= Duration::from_secs(2));
        let mut prev_cap = Duration::ZERO;
        for attempt in 1..200 {
            let (lo, hi) = (backoff(attempt, 0.0), backoff(attempt, r_max));
            assert!(lo <= hi && hi <= Duration::from_secs(MAX_BACKOFF_SECS));
            assert!(hi >= prev_cap, "monotonic cap");
            prev_cap = hi;
        }
        assert!(backoff(100, 0.0) >= Duration::from_secs(30));
        let j = jitter();
        assert!((0.0..1.0).contains(&j));
    }

    #[test]
    fn identify_limit() {
        let reconnect = || End::Reconnect { resume: false, min_wait: Duration::ZERO };
        for n in 0..MAX_IDENTIFIES_WITHOUT_READY {
            assert!(matches!(reconnect().limit_identifies(n), End::Reconnect { .. }), "{n}");
        }
        assert!(matches!(reconnect().limit_identifies(MAX_IDENTIFIES_WITHOUT_READY), End::Fatal(_)));
        assert!(matches!(End::resume().limit_identifies(99), End::Fatal(_)));
        assert!(matches!(End::Shutdown.limit_identifies(99), End::Shutdown));
        assert!(matches!(End::AuthFailed.limit_identifies(99), End::AuthFailed));
    }

    #[test]
    fn upgrade_request_looks_like_browser() {
        let props = ClientProps::new(&crate::config::Settings::default());
        let req = upgrade_request(&format!("{GATEWAY_URL}{QUERY}"), &props).unwrap();
        let h = req.headers();
        assert_eq!(h[header::USER_AGENT], props.user_agent());
        assert_eq!(h[header::ORIGIN], "https://discord.com");
        assert_eq!(h[header::ACCEPT_LANGUAGE], "en-US,en;q=0.9");
        assert_eq!(h[header::CACHE_CONTROL], "no-cache");
        assert_eq!(h[header::PRAGMA], "no-cache");
        assert!(h.contains_key("sec-websocket-key"), "tungstenite defaults kept");
        assert_eq!(req.uri().path_and_query().unwrap().as_str(), QUERY);
    }

    #[test]
    fn send_limiter_window() {
        let mut l = SendLimiter::default();
        let t0 = Instant::now();
        for _ in 0..SEND_LIMIT {
            assert!(l.acquire(t0, false).is_ok());
        }
        let retry = l.acquire(t0, false).unwrap_err();
        assert_eq!(retry, t0 + SEND_WINDOW);
        assert!(l.acquire(t0, true).is_ok(), "heartbeats always pass");
        assert!(l.acquire(t0 + Duration::from_secs(59), false).is_err());
        assert!(l.acquire(t0 + SEND_WINDOW, false).is_ok());

        let mut p = SendLimiter::new(PRESENCE_LIMIT, PRESENCE_WINDOW);
        for i in 0..PRESENCE_LIMIT as u64 {
            assert!(p.acquire(t0 + Duration::from_secs(i), false).is_ok());
        }
        assert_eq!(p.acquire(t0 + Duration::from_secs(5), false), Err(t0 + PRESENCE_WINDOW));
    }
}
