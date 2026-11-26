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
use crate::model::{Id, User};
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
                if matches!(ev, RpcEvent::Closed { .. }) {
                    games.remove(&conn);
                }
                throttle.push(ev, Instant::now()).into_iter().for_each(send);
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

        // Handshake: READY waits for the user.
        let mut game = client(&base, 0).await;
        send(
            &mut game,
            frame::HANDSHAKE,
            json!({"v": 1, "client_id": "1575590836601753835"}),
        )
        .await;
        assert!(matches!(
            next(&rx1).await,
            RpcEvent::Connected {
                conn: 1,
                app: Id(1575590836601753835)
            }
        ));
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
                conn: 1,
                activity: Some(_),
                ..
            }
        ));
        send(
            &mut game,
            frame::FRAME,
            json!({"cmd":"SUBSCRIBE","evt":"ACTIVITY_JOIN","nonce":"2"}),
        )
        .await;
        read_frame(&mut game).await;
        assert!(
            matches!(next(&rx1).await, RpcEvent::Subscribed { conn: 1, ref events } if events == &["ACTIVITY_JOIN"])
        );
        send(&mut game, frame::PING, json!({"nonce": "p"})).await;
        assert_eq!(read_frame(&mut game).await.op, frame::PONG);
        // Closing the pipe: the activity goes (a change, so after the throttle).
        drop(game);
        assert!(matches!(next(&rx1).await, RpcEvent::Closed { conn: 1 }));

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
        assert!(pipe::connect(&base, 0).await.is_err(), "the pipe is gone");
    }
}
