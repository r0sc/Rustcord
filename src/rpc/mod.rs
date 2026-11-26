//! The local game connection: Rustcord answers on Discord's IPC pipe (`discord-ipc-N`), so a
//! game's own rich presence shows on my profile (docs/RPC_DESIGN.md). The server runs on its own
//! thread (a current-thread runtime), one task per game; `proto.rs` is the pure protocol,
//! `frame.rs` the framing, `activity.rs` the checks on what a game sends, `pipe.rs` the OS
//! listener. Games change their activity faster than op 3 may follow, so what reaches the UI is
//! throttled here (§3.3). It never touches the network.
//!
//! OWNERSHIP: activities engineer.

mod activity;
mod frame;
mod pipe;
mod proto;

pub use activity::{Activity, Presence, Privacy};
pub use pipe::BASE;

use crate::backend::{BackendEvent, CommandSender, EventSink};
use crate::auth::Secret;
use crate::model::{Activity as Card, Id, User};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

pub type ConnId = u64;

/// At most one `RpcEvent::Activity` batch reaches the UI per this (op 3 allows 5 per 20 s).
const THROTTLE: Duration = Duration::from_secs(4);
/// The ninth game is turned away (CLOSE 4002).
const MAX_CONNS: usize = 8;
/// A game's image URLs wait this long for the media proxy, then go without.
pub const IMAGE_WAIT: Duration = Duration::from_secs(5);

/// Who READY says is logged in.
#[derive(Clone, Debug, PartialEq)]
pub struct RpcUser {
    pub id: Id,
    pub username: String,
    pub discriminator: String,
    pub global_name: Option<String>,
    pub avatar: Option<String>,
}

impl RpcUser {
    pub fn of(u: &User) -> RpcUser {
        RpcUser {
            id: u.id,
            username: u.username.clone(),
            discriminator: u.discriminator.clone().unwrap_or_else(|| "0".into()),
            global_name: u.global_name.clone(),
            avatar: u.avatar.clone(),
        }
    }

    fn json(&self) -> Value {
        json!({"id": self.id, "username": self.username, "discriminator": self.discriminator, "global_name": self.global_name, "avatar": self.avatar, "bot": false})
    }
}

/// Tests of other modules: a presence from a game's `activity` JSON, all privacy on.
#[cfg(test)]
pub fn test_presence(app: Id, name: &str, activity: Value) -> Presence {
    Presence {
        app,
        name: name.into(),
        privacy: Privacy {
            friends: true,
            voice: true,
        },
        activity: Box::new(activity::parse(&activity).unwrap()),
    }
}

/// UI -> server.
#[derive(Debug)]
pub enum RpcCommand {
    /// The logged-in user (None: logged out, every game is closed).
    Me(Option<RpcUser>),
    /// The setting: listen for games, or close them all and stop.
    Enable(bool),
    /// My click on Join: the join secret goes to every game of `app` that listens (§2.5).
    Dispatch { app: Id, secret: Secret },
}

/// UI -> backend (`Command::RpcRequest`): the requests the game connection causes.
#[derive(Debug)]
pub enum RpcRequest {
    /// The newest game's image URLs (1-2) -> `RpcEvent::Images`.
    ProxyImages { app: Id, urls: Vec<String> },
    /// Join (§4.1; `invite` = channel and message of a rich-presence invite, §4.3): the secret GET,
    /// then `RpcCommand::Dispatch`. An error is an `ActionFailed` toast.
    Join { user: Id, session_id: String, app: Id, invite: Option<(Id, Id)> },
    /// Ask to Join (§4.2): the DM with `user`, then the join request message.
    AskToJoin { user: Id, app: Id, party_id: String, session_id: String, nonce: String },
}

/// Server -> UI (`BackendEvent::Rpc`).
#[derive(Debug)]
pub enum RpcEvent {
    /// The pipe index taken, None: all ten are in use.
    Listening(Option<u8>),
    Connected {
        conn: ConnId,
        app: Id,
    },
    Subscribed {
        conn: ConnId,
        events: Vec<&'static str>,
    },
    /// A game set (or cleared) its activity; `at` orders the games' changes.
    Activity {
        conn: ConnId,
        activity: Option<Box<Activity>>,
        at: Instant,
    },
    Closed {
        conn: ConnId,
    },
    /// A `/applications/public` answer (from the backend, not the server): None = unknown.
    AppName(Id, Option<String>),
    /// The answer to `ProxyImages` (from the backend): each asked URL with its `mp:` path, None =
    /// failed, so that image goes without.
    Images(Id, Vec<(String, Option<String>)>),
}

/// What a card's Join / Ask to Join button shows; `hover` explains a disabled one.
#[derive(Debug, PartialEq)]
pub struct JoinButton {
    pub label: &'static str,
    pub enabled: bool,
    pub hover: Option<String>,
}

/// One game's connection as the UI knows it.
#[derive(Debug)]
pub struct ConnInfo {
    pub app: Id,
    pub events: Vec<&'static str>,
    activity: Option<Box<Activity>>,
    set_at: Option<Instant>,
}

/// `AppState::rpc`.
#[derive(Debug, Default)]
pub struct RpcState {
    /// The setting as last sent to the server.
    on: bool,
    /// The user last sent to the server.
    me: Option<Id>,
    /// The pipe index taken, once the server reported it.
    pub listening: Option<u8>,
    /// The server found all ten pipes in use.
    pub none_free: bool,
    pub conns: HashMap<ConnId, ConnInfo>,
    /// Application names from `/applications/public`; None: asked, not known (or not answered).
    names: HashMap<Id, Option<String>>,
    /// Image URLs given to the media proxy, per application (§3.4).
    proxy_asked: HashSet<(Id, String)>,
    /// The proxy's answers: the path, None = failed.
    proxied: HashMap<(Id, String), Option<String>>,
    /// My gateway session id (READY), which Ask to Join names.
    pub session_id: String,
    /// (user, party) I asked to join this session.
    asked: HashSet<(Id, String)>,
}

impl RpcState {
    pub fn apply(&mut self, ev: RpcEvent) {
        match ev {
            RpcEvent::Listening(i) => (self.listening, self.none_free) = (i, i.is_none()),
            RpcEvent::Connected { conn, app } => {
                self.conns.insert(
                    conn,
                    ConnInfo {
                        app,
                        events: Vec::new(),
                        activity: None,
                        set_at: None,
                    },
                );
            }
            RpcEvent::Subscribed { conn, events } => {
                if let Some(c) = self.conns.get_mut(&conn) {
                    c.events = events;
                }
            }
            RpcEvent::Activity { conn, activity, at } => {
                if let Some(c) = self.conns.get_mut(&conn) {
                    (c.activity, c.set_at) = (activity, Some(at));
                }
            }
            RpcEvent::Closed { conn } => {
                self.conns.remove(&conn);
            }
            RpcEvent::AppName(id, name) => {
                self.names.insert(id, name);
            }
            RpcEvent::Images(app, found) => self.proxied.extend(found.into_iter().map(|(url, path)| ((app, url), path))),
        }
    }

    /// The newest game's image URLs that were never asked about, now marked as asked: the
    /// caller sends the one request (a URL goes to the proxy once per session).
    pub fn proxy_wanted(&mut self) -> Option<(Id, Vec<String>)> {
        let (app, a) = self.newest()?;
        let new: Vec<String> = a.image_urls().into_iter().filter(|u| !self.proxy_asked.contains(&(app, (*u).to_owned()))).map(str::to_owned).collect();
        self.proxy_asked.extend(new.iter().map(|u| (app, u.clone())));
        (!new.is_empty()).then_some((app, new))
    }

    /// `url`'s `mp:` path as the proxy gave it.
    pub fn proxy_path(&self, app: Id, url: &str) -> Option<String> {
        self.proxied.get(&(app, url.to_owned())).cloned().flatten()
    }

    /// False while the newest game's images wait for the proxy: its op 3 change is held.
    pub fn images_ready(&self) -> bool {
        self.newest().is_none_or(|(app, a)| a.image_urls().iter().all(|u| self.proxied.contains_key(&(app, (*u).to_owned()))))
    }

    /// A game of `app` is connected and listens for `ACTIVITY_JOIN`.
    pub fn can_join(&self, app: Id) -> bool {
        self.on && self.conns.values().any(|c| c.app == app && c.events.contains(&"ACTIVITY_JOIN"))
    }

    /// The button of an activity card whose `join_label` is `label` (§4.1, §4.2); `online`: Rustcord
    /// is connected.
    pub fn join_button(&self, label: &'static str, online: bool, user: Id, a: &Card) -> JoinButton {
        let rich = a.rich.as_deref();
        let app = rich.and_then(|r| r.application_id);
        let off = |label, hover: &str| JoinButton { label, enabled: false, hover: Some(hover.to_owned()) };
        if !online {
            return off(label, "You're offline.");
        }
        if label != "Join" {
            let asked = rich.and_then(|r| r.party_id.clone()).map(|p| self.asked.contains(&(user, p)));
            return match (app, asked) {
                (Some(_), Some(false)) if !self.session_id.is_empty() => JoinButton { label, enabled: true, hover: None },
                (Some(_), Some(true)) => off("Request sent", "They answer in their game."),
                _ => off(label, "This activity can't be asked to join."),
            };
        }
        match (app, rich.is_some_and(|r| r.session_id.is_some())) {
            (Some(app), true) if self.can_join(app) => JoinButton { label, enabled: true, hover: None },
            (Some(_), true) if !self.on => off(label, "Turn on \"Let games show their activity\" in Settings to join."),
            (Some(_), true) => off(label, &format!("Start {} to join.", a.name)),
            _ => off(label, "This activity can't be joined from here."),
        }
    }

    /// A click on that button: its one request, none when it is disabled. Ask to Join is marked
    /// as asked, so a second click sends nothing. `nonce` makes the message's nonce.
    pub fn join_click(&mut self, cmd: &CommandSender, label: &'static str, online: bool, user: Id, a: &Card, nonce: impl FnOnce() -> String) {
        let Some(r) = a.rich.as_deref().filter(|_| self.join_button(label, online, user, a).enabled) else { return };
        let Some(app) = r.application_id else { return };
        let req = match (label, &r.session_id, &r.party_id) {
            ("Join", Some(session_id), _) => RpcRequest::Join { user, session_id: session_id.clone(), app, invite: None },
            (_, _, Some(party_id)) if label != "Join" => {
                self.asked.insert((user, party_id.clone()));
                RpcRequest::AskToJoin { user, app, party_id: party_id.clone(), session_id: self.session_id.clone(), nonce: nonce() }
            }
            _ => return,
        };
        cmd.send(crate::backend::Command::RpcRequest(req));
    }

    /// The "Join" of an invite message of `author` for game `app` (§4.3): `live` is the author's
    /// current activity, which must still be of that game with a session, else the invite is stale.
    pub fn invite_button(&self, online: bool, author: Id, app: Id, live: Option<&Card>) -> JoinButton {
        let live = live.filter(|a| a.rich.as_deref().is_some_and(|r| r.application_id == Some(app) && r.session_id.is_some()));
        let Some(a) = live else {
            return JoinButton { label: "Join", enabled: false, hover: Some(if online { "This invite has expired." } else { "You're offline." }.into()) };
        };
        self.join_button("Join", online, author, a)
    }

    /// A click on that button: its one request (the join, with the invite's channel and message), none when disabled.
    pub fn invite_click(&self, cmd: &CommandSender, online: bool, author: Id, app: Id, live: Option<&Card>, invite: (Id, Id)) {
        let session_id = live.and_then(|a| a.rich.as_deref()?.session_id.clone());
        if let (true, Some(session_id)) = (self.invite_button(online, author, app, live).enabled, session_id) {
            cmd.send(crate::backend::Command::RpcRequest(RpcRequest::Join { user: author, session_id, app, invite: Some(invite) }));
        }
    }

    /// What survives a logout: the server's state, not the session's games.
    pub fn after_logout(self) -> Self {
        Self {
            on: self.on,
            me: self.me,
            listening: self.listening,
            none_free: self.none_free,
            ..Default::default()
        }
    }

    /// The game whose activity was set last, and that activity. Nothing while the setting is off.
    pub fn newest(&self) -> Option<(Id, &Activity)> {
        let on = self.on.then(|| self.conns.values());
        on?.filter_map(|c| {
            c.activity
                .as_ref()
                .zip(c.set_at)
                .map(|(a, at)| (at, c.app, &**a))
        })
        .max_by_key(|(at, ..)| *at)
        .map(|(_, app, a)| (app, a))
    }

    /// Application `app`'s name, if answered (None while unknown or not asked).
    pub fn name(&self, app: Id) -> Option<&str> {
        self.names.get(&app).and_then(|n| n.as_deref())
    }

    /// Marks `app` as asked: the caller sends the one request. False when it was asked before.
    pub fn ask_name(&mut self, app: Id) -> bool {
        let new = !self.names.contains_key(&app);
        if new {
            self.names.insert(app, None);
        }
        new
    }

    /// Each frame: the setting and the logged-in user to the server, when they changed.
    pub fn sync(&mut self, enable: bool, me: Option<&User>, cmd: &CommandSender) {
        if enable != self.on {
            self.on = enable;
            if !enable {
                (self.listening, self.none_free) = (None, false);
            }
            cmd.send(crate::backend::Command::Rpc(RpcCommand::Enable(enable)));
        }
        if me.map(|m| m.id) != self.me {
            self.me = me.map(|m| m.id);
            cmd.send(crate::backend::Command::Rpc(RpcCommand::Me(
                me.map(RpcUser::of),
            )));
        }
    }
}

/// Handle of the server thread; it ends with the last handle.
#[derive(Clone)]
pub struct Service(UnboundedSender<RpcCommand>);

impl Service {
    /// Starts the server thread, off until `Enable(true)`. `base` is the pipe name (`BASE`).
    pub fn spawn(sink: EventSink, base: String) -> Service {
        let (tx, rx) = unbounded_channel();
        let run = move || match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt.block_on(serve(rx, sink, base)),
            Err(e) => eprintln!("rustcord: game connection unavailable: {e}"),
        };
        if let Err(e) = std::thread::Builder::new()
            .name("rustcord-rpc".into())
            .spawn(run)
        {
            eprintln!("rustcord: game connection unavailable: {e}");
        }
        Service(tx)
    }

    pub fn send(&self, cmd: RpcCommand) {
        let _ = self.0.send(cmd);
    }
}

/// What the server tells one game's task.
enum ToConn {
    Me(Option<RpcUser>),
    /// CLOSE 1000 with this message.
    Close(&'static str),
    /// ACTIVITY_JOIN, for a game of `app` that subscribed.
    Join { app: Id, secret: Secret },
}

/// Holds back the activity changes (and closes of a game that had one) so that at most one
/// batch passes per `THROTTLE`; per game the latest wins.
#[derive(Default)]
struct Throttle {
    last: Option<Instant>,
    pending: Vec<RpcEvent>,
    /// Games that have an activity set.
    active: HashSet<ConnId>,
}

impl Throttle {
    fn conn(ev: &RpcEvent) -> Option<ConnId> {
        match ev {
            RpcEvent::Activity { conn, .. } | RpcEvent::Closed { conn } => Some(*conn),
            _ => None,
        }
    }

    /// `ev` goes to the UI now or is held; returns what to send now.
    fn push(&mut self, ev: RpcEvent, now: Instant) -> Vec<RpcEvent> {
        let Some(conn) = Self::conn(&ev) else {
            return vec![ev];
        };
        match &ev {
            RpcEvent::Activity { activity, .. } if activity.is_some() => {
                _ = self.active.insert(conn)
            }
            RpcEvent::Activity { .. } => _ = self.active.remove(&conn),
            // A game that never set one is no change to anyone's presence.
            _ if !self.active.remove(&conn) => {
                self.pending.retain(|p| Self::conn(p) != Some(conn));
                return vec![ev];
            }
            _ => {}
        }
        self.pending.retain(|p| Self::conn(p) != Some(conn));
        self.pending.push(ev);
        self.take(now)
    }

    /// The held events, once `THROTTLE` has passed since the last batch.
    fn take(&mut self, now: Instant) -> Vec<RpcEvent> {
        if self.pending.is_empty() || self.last.is_some_and(|t| now < t + THROTTLE) {
            return Vec::new();
        }
        self.last = Some(now);
        std::mem::take(&mut self.pending)
    }

    /// When the held events may go.
    fn due(&self) -> Option<Instant> {
        (!self.pending.is_empty()).then(|| self.last.map_or_else(Instant::now, |t| t + THROTTLE))
    }

    /// Switched off: nothing is held, and the games' closes pass at once.
    fn clear(&mut self) {
        self.pending.clear();
        self.active.clear();
    }
}

/// The server: commands in, games' events through the throttle out.
async fn serve(mut cmds: UnboundedReceiver<RpcCommand>, sink: EventSink, base: String) {
    let send = |ev| sink.send(BackendEvent::Rpc(ev));
    let (up_tx, mut up) = unbounded_channel::<(ConnId, RpcEvent)>();
    let mut listener: Option<pipe::Listener> = None;
    let mut games: HashMap<ConnId, UnboundedSender<ToConn>> = HashMap::new();
    let mut greeted = std::collections::HashSet::new();
    let (mut me, mut next_id, mut throttle) = (None::<RpcUser>, 0, Throttle::default());
    loop {
        let wake = throttle.due();
        tokio::select! {
            cmd = cmds.recv() => match cmd {
                None => break,
                Some(RpcCommand::Me(user)) => {
                    me.clone_from(&user);
                    games.values().for_each(|g| _ = g.send(ToConn::Me(user.clone())));
                }
                Some(RpcCommand::Enable(true)) if listener.is_none() => {
                    listener = pipe::Listener::bind(&base).await;
                    send(RpcEvent::Listening(listener.as_ref().map(pipe::Listener::index)));
                }
                Some(RpcCommand::Enable(false)) => {
                    // Dropping the listener frees the pipe; each game's task reports its own close.
                    listener = None;
                    throttle.clear();
                    games.values().for_each(|g| _ = g.send(ToConn::Close("The game connection is off")));
                }
                Some(RpcCommand::Enable(true)) => {}
                Some(RpcCommand::Dispatch { app, secret }) => {
                    games.values().for_each(|g| _ = g.send(ToConn::Join { app, secret: secret.clone() }));
                }
            },
            stream = async { listener.as_mut().unwrap().accept().await }, if listener.is_some() => match stream {
                Ok(stream) if games.len() < MAX_CONNS => {
                    next_id += 1;
                    let (tx, rx) = unbounded_channel();
                    games.insert(next_id, tx);
                    tokio::spawn(game(stream, next_id, me.clone(), rx, up_tx.clone()));
                }
                Ok(stream) => _ = tokio::spawn(turn_away(stream)),
                Err(e) => {
                    eprintln!("rustcord: game connection stopped: {e}");
                    listener = None;
                    send(RpcEvent::Listening(None));
                }
            },
            Some((conn, ev)) = up.recv() => {
                // Only a handshake makes a game: a connection that closes before one (another
                // server's probe of this socket on Unix, §5) is not reported.
                let report = match ev {
                    RpcEvent::Connected { .. } => greeted.insert(conn),
                    RpcEvent::Closed { .. } => {
                        games.remove(&conn);
                        greeted.remove(&conn)
                    }
                    _ => true,
                };
                if report {
                    throttle.push(ev, Instant::now()).into_iter().for_each(send);
                }
            }
            () = tokio::time::sleep_until(wake.unwrap_or_else(Instant::now).into()), if wake.is_some() => {
                throttle.take(Instant::now()).into_iter().for_each(send);
            }
        }
    }
}

/// The ninth game: CLOSE 4002, then gone.
async fn turn_away(mut stream: pipe::Stream) {
    let _ = stream
        .write_all(&proto::Out::close(4002, "Too many connections").frames[0].encode())
        .await;
}

/// One game: bytes in, `proto::Conn`, frames out. Ends when the game or the server closes it,
/// and says so.
async fn game(
    mut stream: pipe::Stream,
    id: ConnId,
    user: Option<RpcUser>,
    mut from_server: UnboundedReceiver<ToConn>,
    up: UnboundedSender<(ConnId, RpcEvent)>,
) {
    let mut conn = proto::Conn::new(id, user);
    let (mut buf, mut chunk) = (Vec::new(), [0u8; 4096]);
    // Writes the frames, passes the events on; false once the pipe should close.
    async fn finish(
        out: proto::Out,
        stream: &mut pipe::Stream,
        id: ConnId,
        up: &UnboundedSender<(ConnId, RpcEvent)>,
    ) -> bool {
        out.events.into_iter().for_each(|e| _ = up.send((id, e)));
        for f in &out.frames {
            if stream.write_all(&f.encode()).await.is_err() {
                return false;
            }
        }
        !out.close
    }
    'run: loop {
        tokio::select! {
            read = stream.read(&mut chunk) => {
                match read {
                    Ok(n) if n > 0 => buf.extend_from_slice(&chunk[..n]),
                    _ => break,
                }
                loop {
                    let out = match frame::decode(&mut buf) {
                        Ok(Some(f)) => conn.on_frame(f, Instant::now()),
                        Ok(None) => break,
                        Err(message) => proto::Out::close(1003, message),
                    };
                    if !finish(out, &mut stream, id, &up).await {
                        break 'run;
                    }
                }
            }
            msg = from_server.recv() => {
                let out = match msg {
                    Some(ToConn::Me(user)) if user.is_some() => conn.set_user(user),
                    Some(ToConn::Me(None)) => proto::Out::close(1000, "Logged out"),
                    Some(ToConn::Close(message)) => proto::Out::close(1000, message),
                    Some(ToConn::Join { app, secret }) => proto::Out { frames: conn.join(app, &secret).into_iter().collect(), ..Default::default() },
                    _ => break,
                };
                if !finish(out, &mut stream, id, &up).await {
                    break;
                }
            }
        }
    }
    let _ = up.send((id, RpcEvent::Closed { conn: id }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn at(t0: Instant, secs: u64) -> Instant {
        t0 + Duration::from_secs(secs)
    }

    fn set(conn: ConnId, some: bool, t: Instant) -> RpcEvent {
        let activity = some.then(|| Box::new(activity::parse(&json!({"state": "hello"})).unwrap()));
        RpcEvent::Activity {
            conn,
            activity,
            at: t,
        }
    }

    fn conns(evs: &[RpcEvent]) -> Vec<(&'static str, ConnId)> {
        evs.iter()
            .map(|e| match e {
                RpcEvent::Activity { conn, .. } => ("activity", *conn),
                RpcEvent::Closed { conn } => ("closed", *conn),
                _ => ("other", 0),
            })
            .collect()
    }

    #[test]
    fn throttle_passes_one_batch_per_window_latest_per_game() {
        let (t0, mut t) = (Instant::now(), Throttle::default());
        assert_eq!(
            conns(&t.push(set(1, true, t0), t0)),
            [("activity", 1)],
            "the first passes at once"
        );
        assert!(t.push(set(1, false, t0), at(t0, 1)).is_empty());
        assert!(
            t.push(set(1, true, t0), at(t0, 2)).is_empty(),
            "latest of a game wins"
        );
        assert!(t.push(set(2, true, t0), at(t0, 3)).is_empty());
        assert_eq!(t.due(), Some(at(t0, 4)));
        assert!(t.take(at(t0, 3)).is_empty());
        let batch = t.take(at(t0, 4));
        assert_eq!(conns(&batch), [("activity", 1), ("activity", 2)]);
        assert!(matches!(
            &batch[0],
            RpcEvent::Activity {
                activity: Some(_),
                ..
            }
        ));
        assert_eq!(t.due(), None);
        // A pipe close of a game with an activity is a change; one without passes at once.
        assert!(t.push(RpcEvent::Closed { conn: 2 }, at(t0, 5)).is_empty());
        assert_eq!(
            conns(&t.push(RpcEvent::Closed { conn: 9 }, at(t0, 5))),
            [("closed", 9)]
        );
        assert_eq!(conns(&t.take(at(t0, 8))), [("closed", 2)]);
        // Other events are never held; switching off lets closes through.
        assert!(matches!(
            t.push(RpcEvent::Listening(Some(0)), at(t0, 8))[..],
            [RpcEvent::Listening(Some(0))]
        ));
        t.push(set(3, true, t0), at(t0, 9));
        t.push(set(3, true, t0), at(t0, 10));
        t.clear();
        assert_eq!(
            conns(&t.push(RpcEvent::Closed { conn: 3 }, at(t0, 10))),
            [("closed", 3)]
        );
        assert_eq!(t.due(), None);
    }

    #[test]
    fn state_follows_the_events_and_picks_the_newest() {
        let (t0, mut s) = (Instant::now(), RpcState::default());
        s.apply(RpcEvent::Connected {
            conn: 1,
            app: Id(10),
        });
        s.apply(RpcEvent::Connected {
            conn: 2,
            app: Id(20),
        });
        s.apply(set(1, true, at(t0, 2)));
        s.apply(set(2, true, at(t0, 1)));
        assert!(s.newest().is_none(), "off until the setting is on");
        s.on = true;
        assert_eq!(s.newest().map(|n| n.0), Some(Id(10)));
        s.apply(set(1, false, at(t0, 3)));
        assert_eq!(s.newest().map(|n| n.0), Some(Id(20)));
        s.apply(RpcEvent::Subscribed {
            conn: 2,
            events: vec!["ACTIVITY_JOIN"],
        });
        assert_eq!(s.conns[&2].events, ["ACTIVITY_JOIN"]);
        s.apply(RpcEvent::Closed { conn: 2 });
        assert!(s.newest().is_none() && s.conns.len() == 1);
        s.apply(set(7, true, t0));
        assert_eq!(
            s.conns.len(),
            1,
            "an activity of an unknown game is ignored"
        );
        s.apply(RpcEvent::Listening(None));
        assert!(s.none_free);
        // Names: asked once, answered once.
        assert!(s.ask_name(Id(10)) && !s.ask_name(Id(10)));
        assert_eq!(s.name(Id(10)), None);
        s.apply(RpcEvent::AppName(Id(10), Some("Game".into())));
        assert_eq!(s.name(Id(10)), Some("Game"));
        let s = s.after_logout();
        assert!(s.on && s.conns.is_empty() && s.names.is_empty() && s.none_free);
    }

    fn sink() -> (EventSink, mpsc::Receiver<BackendEvent>) {
        let (tx, rx) = mpsc::channel();
        (EventSink::new(tx, egui::Context::default()), rx)
    }

    /// The next RPC event, within 5 s.
    async fn next(rx: &mpsc::Receiver<BackendEvent>) -> RpcEvent {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            match rx.try_recv() {
                Ok(BackendEvent::Rpc(ev)) => return ev,
                Ok(_) => {}
                Err(_) => assert!(Instant::now() < end, "no event in time"),
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    async fn read_frame<S: tokio::io::AsyncRead + Unpin>(s: &mut S) -> frame::Frame {
        let (mut buf, mut chunk) = (Vec::new(), [0u8; 4096]);
        loop {
            if let Some(f) = frame::decode(&mut buf).unwrap() {
                return f;
            }
            let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut chunk))
                .await
                .expect("no frame in time")
                .unwrap();
            assert!(n > 0, "closed");
            buf.extend_from_slice(&chunk[..n]);
        }
    }

    async fn client(
        base: &str,
        index: u8,
    ) -> impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin {
        // The pipe may be busy for a moment between two connects.
        for _ in 0..200 {
            if let Ok(c) = pipe::connect(base, index).await {
                return c;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("could not connect to {base}-{index}");
    }

    async fn send<S: tokio::io::AsyncWrite + Unpin>(s: &mut S, op: u32, body: Value) {
        s.write_all(&frame::Frame::json(op, &body).encode())
            .await
            .unwrap();
    }

    fn me() -> RpcUser {
        RpcUser {
            id: Id(7),
            username: "ann".into(),
            discriminator: "0".into(),
            global_name: None,
            avatar: None,
        }
    }

    fn card(app: Option<Id>, session: Option<&str>, party: Option<&str>) -> Card {
        let rich = crate::model::RichPresence { application_id: app, session_id: session.map(Into::into), party_id: party.map(Into::into), ..Default::default() };
        Card { kind: 0, name: "Halo".into(), rich: Some(Box::new(rich)) }
    }

    fn sent_cmds(rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::backend::Command>) -> Vec<String> {
        std::iter::from_fn(|| rx.try_recv().ok()).map(|c| format!("{c:?}")).collect()
    }

    #[test]
    fn the_join_button_follows_the_game_and_the_setting() {
        let mut s = RpcState { on: true, session_id: "mine".into(), ..Default::default() };
        let a = card(Some(Id(8)), Some("sess"), Some("p1"));
        let join = |s: &RpcState, online, a: &Card| s.join_button("Join", online, Id(2), a);
        let hover = |b: JoinButton| (b.enabled, b.hover);
        assert_eq!(hover(join(&s, false, &a)), (false, Some("You're offline.".into())));
        assert_eq!(hover(join(&s, true, &a)), (false, Some("Start Halo to join.".into())), "the game is not connected");
        s.apply(RpcEvent::Connected { conn: 1, app: Id(8) });
        assert_eq!(hover(join(&s, true, &a)).1.as_deref(), Some("Start Halo to join."), "connected but not listening");
        s.apply(RpcEvent::Connected { conn: 2, app: Id(9) });
        s.apply(RpcEvent::Subscribed { conn: 2, events: vec!["ACTIVITY_JOIN"] });
        assert!(!join(&s, true, &a).enabled, "another game listens");
        s.apply(RpcEvent::Subscribed { conn: 1, events: vec!["ACTIVITY_JOIN"] });
        assert_eq!(hover(join(&s, true, &a)), (true, None));
        assert!(s.can_join(Id(8)) && !s.can_join(Id(7)));
        for (a, why) in [(card(Some(Id(8)), None, None), "no session"), (card(None, Some("sess"), None), "no application")] {
            assert_eq!(hover(join(&s, true, &a)), (false, Some("This activity can't be joined from here.".into())), "{why}");
        }
        s.on = false;
        assert_eq!(hover(join(&s, true, &a)), (false, Some("Turn on \"Let games show their activity\" in Settings to join.".into())));
        assert!(!s.can_join(Id(8)));
    }

    #[test]
    fn a_click_sends_one_request_and_ask_to_join_only_once() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cmd = CommandSender(tx);
        let mut s = RpcState { on: true, session_id: "mine".into(), ..Default::default() };
        let a = card(Some(Id(8)), Some("sess"), Some("p1"));
        s.apply(RpcEvent::Connected { conn: 1, app: Id(8) });
        s.apply(RpcEvent::Subscribed { conn: 1, events: vec!["ACTIVITY_JOIN"] });
        // Join: nothing offline or while disabled, one request per click.
        s.join_click(&cmd, "Join", false, Id(2), &a, || "n".into());
        s.join_click(&cmd, "Join", true, Id(2), &card(Some(Id(8)), None, None), || "n".into());
        assert!(sent_cmds(&mut rx).is_empty());
        s.join_click(&cmd, "Join", true, Id(2), &a, || "n".into());
        assert_eq!(sent_cmds(&mut rx), [r#"RpcRequest(Join { user: 2, session_id: "sess", app: 8, invite: None })"#]);
        // Ask to Join: offline nothing; then once per (user, party), and the button says so.
        s.join_click(&cmd, "Ask to Join", false, Id(2), &a, || "n".into());
        assert!(sent_cmds(&mut rx).is_empty());
        assert!(s.join_button("Ask to Join", true, Id(2), &a).enabled);
        s.join_click(&cmd, "Ask to Join", true, Id(2), &a, || "n".into());
        s.join_click(&cmd, "Ask to Join", true, Id(2), &a, || "n2".into());
        assert_eq!(sent_cmds(&mut rx), [r#"RpcRequest(AskToJoin { user: 2, app: 8, party_id: "p1", session_id: "mine", nonce: "n" })"#]);
        let b = s.join_button("Ask to Join", true, Id(2), &a);
        assert_eq!((b.label, b.enabled), ("Request sent", false));
        s.join_click(&cmd, "Ask to Join", true, Id(3), &a, || "n3".into());
        assert_eq!(sent_cmds(&mut rx).len(), 1, "another user's party is a new request");
        // No party, no application or no session of mine: disabled. The game need not run.
        for a in [card(Some(Id(8)), None, None), card(None, None, Some("p")), card(Some(Id(5)), None, Some("p"))] {
            assert_eq!(s.join_button("Ask to Join", true, Id(4), &a).enabled, a.rich.as_ref().is_some_and(|r| r.application_id.is_some() && r.party_id.is_some()));
        }
        assert!(!RpcState::default().join_button("Ask to Join", true, Id(4), &a).enabled, "no gateway session yet");
        assert!(s.after_logout().join_button("Ask to Join", true, Id(2), &a).hover.is_some(), "a new login starts fresh");
    }

    #[test]
    fn an_invite_joins_only_while_the_host_is_still_in_the_activity() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cmd = CommandSender(tx);
        let mut s = RpcState { on: true, ..Default::default() };
        let live = card(Some(Id(8)), Some("sess"), Some("p1"));
        let b = |s: &RpcState, online, live: Option<&Card>| {
            let b = s.invite_button(online, Id(2), Id(8), live);
            (b.label, b.enabled, b.hover)
        };
        let off = |h: &str| ("Join", false, Some(h.to_owned()));
        assert_eq!(b(&s, false, Some(&live)), off("You're offline."));
        assert_eq!(b(&s, true, None), off("This invite has expired."), "no activity any more");
        for gone in [card(Some(Id(9)), Some("sess"), None), card(Some(Id(8)), None, None), card(None, Some("sess"), None)] {
            assert_eq!(b(&s, true, Some(&gone)), off("This invite has expired."));
        }
        assert_eq!(b(&s, true, Some(&live)), off("Start Halo to join."));
        s.apply(RpcEvent::Connected { conn: 1, app: Id(8) });
        s.apply(RpcEvent::Subscribed { conn: 1, events: vec!["ACTIVITY_JOIN"] });
        assert_eq!(b(&s, true, Some(&live)), ("Join", true, None));
        s.on = false;
        assert_eq!(b(&s, true, Some(&live)), off("Turn on \"Let games show their activity\" in Settings to join."));
        s.on = true;
        // Only a click sends, once, with the invite's channel and message and the host's session.
        s.invite_click(&cmd, true, Id(2), Id(8), None, (Id(3), Id(4)));
        s.invite_click(&cmd, false, Id(2), Id(8), Some(&live), (Id(3), Id(4)));
        assert!(sent_cmds(&mut rx).is_empty());
        s.invite_click(&cmd, true, Id(2), Id(8), Some(&live), (Id(3), Id(4)));
        assert_eq!(sent_cmds(&mut rx), [r#"RpcRequest(Join { user: 2, session_id: "sess", app: 8, invite: Some((3, 4)) })"#]);
    }

    #[tokio::test]
    async fn dispatch_reaches_only_a_listening_game_of_that_app() {
        let base = format!("rustcord-test-ipc-join-{}", std::process::id());
        let (sink, rx) = sink();
        let svc = Service::spawn(sink, base.clone());
        svc.send(RpcCommand::Enable(true));
        assert!(matches!(next(&rx).await, RpcEvent::Listening(Some(0))));
        svc.send(RpcCommand::Me(Some(me())));
        // (client_id, subscribes): the listener, another app's game, and the same app without a subscription.
        let mut games = Vec::new();
        for (app, listens) in [("1575590836601753835", true), ("1575590836601753836", true), ("1575590836601753835", false)] {
            let mut g = client(&base, 0).await;
            send(&mut g, frame::HANDSHAKE, json!({"v": 1, "client_id": app})).await;
            read_frame(&mut g).await;
            if listens {
                send(&mut g, frame::FRAME, json!({"cmd":"SUBSCRIBE","evt":"ACTIVITY_JOIN","nonce":"s"})).await;
                read_frame(&mut g).await;
            }
            games.push(g);
        }
        svc.send(RpcCommand::Dispatch { app: Id(1575590836601753835), secret: Secret::new("j-1a2b3c".into()) });
        let got = serde_json::from_slice::<Value>(&read_frame(&mut games[0]).await.body).unwrap();
        assert_eq!(got, json!({"cmd":"DISPATCH","evt":"ACTIVITY_JOIN","data":{"secret":"j-1a2b3c"},"nonce":null}));
        // The others hear nothing: their next frame is the answer to a PING sent after the dispatch.
        for g in &mut games[1..] {
            send(g, frame::PING, json!({"nonce": "p"})).await;
            assert_eq!(read_frame(g).await.op, frame::PONG);
        }
        svc.send(RpcCommand::Enable(false));
    }

    #[tokio::test]
    async fn a_game_talks_to_the_server_over_a_pipe() {
        let base = format!("rustcord-test-ipc-{}", std::process::id());
        let ((sink1, rx1), (sink2, rx2)) = (sink(), sink());
        let (one, two) = (
            Service::spawn(sink1, base.clone()),
            Service::spawn(sink2, base.clone()),
        );
        one.send(RpcCommand::Enable(true));
        assert!(matches!(next(&rx1).await, RpcEvent::Listening(Some(0))));
        two.send(RpcCommand::Enable(true));
        assert!(
            matches!(next(&rx2).await, RpcEvent::Listening(Some(1))),
            "a second server takes the next pipe"
        );

        // A connection that closes before its handshake (a probe) is not reported, on every OS.
        drop(client(&base, 0).await);
        // Handshake: READY waits for the user.
        let mut game = client(&base, 0).await;
        send(
            &mut game,
            frame::HANDSHAKE,
            json!({"v": 1, "client_id": "1575590836601753835"}),
        )
        .await;
        // The first event: `two`'s probe of pipe 0 (Unix) took an id but is not reported.
        let id = match next(&rx1).await {
            RpcEvent::Connected { conn, app: Id(1575590836601753835) } => conn,
            e => panic!("expected Connected, got {e:?}"),
        };
        one.send(RpcCommand::Me(Some(me())));
        let ready = read_frame(&mut game).await;
        assert_eq!(
            serde_json::from_slice::<Value>(&ready.body).unwrap()["data"]["user"]["username"],
            "ann"
        );
        // SET_ACTIVITY: answered at once, then reported.
        send(&mut game, frame::FRAME, json!({"cmd":"SET_ACTIVITY","nonce":"1","args":{"pid":1,"activity":{"state":"In a match","timestamps":{"start":1764183000}}}})).await;
        let answer = serde_json::from_slice::<Value>(&read_frame(&mut game).await.body).unwrap();
        assert_eq!(
            (&answer["cmd"], &answer["data"]["state"]),
            (&json!("SET_ACTIVITY"), &json!("In a match"))
        );
        assert!(matches!(
            next(&rx1).await,
            RpcEvent::Activity {
                conn,
                activity: Some(_),
                ..
            } if conn == id
        ));
        send(
            &mut game,
            frame::FRAME,
            json!({"cmd":"SUBSCRIBE","evt":"ACTIVITY_JOIN","nonce":"2"}),
        )
        .await;
        read_frame(&mut game).await;
        assert!(
            matches!(next(&rx1).await, RpcEvent::Subscribed { conn, ref events } if conn == id && events == &["ACTIVITY_JOIN"])
        );
        send(&mut game, frame::PING, json!({"nonce": "p"})).await;
        assert_eq!(read_frame(&mut game).await.op, frame::PONG);
        // Closing the pipe: the activity goes (a change, so after the throttle).
        drop(game);
        assert!(matches!(next(&rx1).await, RpcEvent::Closed { conn } if conn == id));

        // A bad frame closes with 1003; logging out closes with 1000.
        let mut bad = client(&base, 0).await;
        bad.write_all(&[9, 0, 0, 0, 0, 0, 0, 0]).await.unwrap();
        let close = read_frame(&mut bad).await;
        assert_eq!(
            (
                close.op,
                serde_json::from_slice::<Value>(&close.body).unwrap()["code"].clone()
            ),
            (frame::CLOSE, json!(1003))
        );
        let mut game = client(&base, 0).await;
        send(
            &mut game,
            frame::HANDSHAKE,
            json!({"v": 1, "client_id": "1575590836601753835"}),
        )
        .await;
        read_frame(&mut game).await;
        one.send(RpcCommand::Me(None));
        let close = read_frame(&mut game).await;
        assert_eq!(
            (
                close.op,
                serde_json::from_slice::<Value>(&close.body).unwrap()["code"].clone()
            ),
            (frame::CLOSE, json!(1000))
        );
        // Eight games fit, the ninth is turned away; switching off closes them all and frees the pipe.
        one.send(RpcCommand::Me(Some(me())));
        let mut held = Vec::new();
        for _ in 0..MAX_CONNS {
            let mut g = client(&base, 0).await;
            send(
                &mut g,
                frame::HANDSHAKE,
                json!({"v": 1, "client_id": "1575590836601753835"}),
            )
            .await;
            read_frame(&mut g).await;
            held.push(g);
        }
        let mut ninth = client(&base, 0).await;
        let close = read_frame(&mut ninth).await;
        assert_eq!(
            serde_json::from_slice::<Value>(&close.body).unwrap()["code"],
            4002
        );
        one.send(RpcCommand::Enable(false));
        for g in &mut held {
            assert_eq!(read_frame(g).await.op, frame::CLOSE);
        }
        // The listening instance closes once its cancelled connect completes on the server's runtime, a moment later under load.
        let gone = async {
            while pipe::connect(&base, 0).await.is_ok() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        assert!(
            tokio::time::timeout(Duration::from_secs(5), gone).await.is_ok(),
            "the pipe is gone"
        );
    }
}
