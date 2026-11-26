//! One connection's protocol (docs/RPC_DESIGN.md §2.3, §2.4): what to answer to each frame, and
//! what the UI hears of it. Pure: no I/O, the caller writes `Out::frames` and closes on
//! `Out::close`.

use super::activity::{self, Privacy};
use super::frame::{self, Frame};
use super::{ConnId, RpcEvent, RpcUser};
use crate::model::Id;
use serde_json::{Value, json};
use std::time::Instant;

/// The events a game may subscribe to (all that discord-rpc and the Game SDK ask for).
const EVENTS: [&str; 4] = [
    "ACTIVITY_JOIN",
    "ACTIVITY_SPECTATE",
    "ACTIVITY_JOIN_REQUEST",
    "ACTIVITY_INVITE",
];

/// What to do after a frame.
#[derive(Debug, Default)]
pub struct Out {
    pub frames: Vec<Frame>,
    pub events: Vec<RpcEvent>,
    /// Close the pipe after writing `frames`.
    pub close: bool,
}

impl Out {
    fn frame(frame: Frame) -> Out {
        Out {
            frames: vec![frame],
            ..Default::default()
        }
    }

    /// A CLOSE frame, then the pipe closes.
    pub fn close(code: u32, message: &str) -> Out {
        Out {
            frames: vec![Frame::json(
                frame::CLOSE,
                &json!({"code": code, "message": message}),
            )],
            close: true,
            ..Default::default()
        }
    }
}

pub struct Conn {
    id: ConnId,
    /// The handshake's `client_id`; None until the handshake.
    app: Option<Id>,
    user: Option<RpcUser>,
    ready: bool,
    subscribed: Vec<&'static str>,
}

impl Conn {
    pub fn new(id: ConnId, user: Option<RpcUser>) -> Conn {
        Conn {
            id,
            app: None,
            user,
            ready: false,
            subscribed: Vec::new(),
        }
    }

    pub fn on_frame(&mut self, f: Frame, now: Instant) -> Out {
        match f.op {
            frame::HANDSHAKE => self.handshake(&f.body),
            frame::FRAME if self.app.is_some() => self.command(&f.body, now),
            frame::FRAME => Out::close(1003, "Handshake expected first"),
            frame::CLOSE => Out {
                close: true,
                ..Default::default()
            },
            frame::PING => Out::frame(Frame {
                op: frame::PONG,
                body: f.body,
            }),
            _ => Out::default(),
        }
    }

    /// The logged-in user became known (READY goes out once a handshake is in) or changed.
    pub fn set_user(&mut self, user: Option<RpcUser>) -> Out {
        self.user = user;
        self.ready_out()
    }

    fn ready_out(&mut self) -> Out {
        match (&self.user, self.app, self.ready) {
            (Some(u), Some(_), false) => {
                self.ready = true;
                let config = json!({"cdn_host": "cdn.discordapp.com", "api_endpoint": "//discord.com/api", "environment": "production"});
                let data = json!({"v": 1, "config": config, "user": u.json()});
                Out::frame(Frame::json(
                    frame::FRAME,
                    &json!({"cmd": "DISPATCH", "evt": "READY", "nonce": null, "data": data}),
                ))
            }
            _ => Out::default(),
        }
    }

    fn handshake(&mut self, body: &[u8]) -> Out {
        if self.app.is_some() {
            return Out::close(1003, "Already shook hands");
        }
        let Ok(v) = serde_json::from_slice::<Value>(body) else {
            return Out::close(1003, "Invalid JSON");
        };
        if v["v"].as_u64() != Some(1) {
            return Out::close(4004, "Invalid version");
        }
        let id = match &v["client_id"] {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            _ => String::new(),
        };
        let app = (17..=20)
            .contains(&id.len())
            .then(|| id.parse().ok())
            .flatten()
            .filter(|n| *n != 0);
        let Some(app) = app else {
            return Out::close(4000, "Invalid client ID");
        };
        self.app = Some(Id(app));
        let mut out = self.ready_out();
        out.events.push(RpcEvent::Connected {
            conn: self.id,
            app: Id(app),
        });
        out
    }

    fn command(&mut self, body: &[u8], now: Instant) -> Out {
        let Ok(v) = serde_json::from_slice::<Value>(body) else {
            return Out::close(1003, "Invalid JSON");
        };
        let (cmd, nonce) = (v["cmd"].as_str().unwrap_or(""), &v["nonce"]);
        let answer = |data: Value| {
            Out::frame(Frame::json(
                frame::FRAME,
                &json!({"cmd": cmd, "data": data, "evt": null, "nonce": nonce}),
            ))
        };
        let error = |code: u32, message: &str| {
            Out::frame(Frame::json(
                frame::FRAME,
                &json!({"cmd": cmd, "evt": "ERROR", "data": {"code": code, "message": message}, "nonce": nonce}),
            ))
        };
        match cmd {
            "SET_ACTIVITY" => {
                let act = &v["args"]["activity"];
                let activity = match act {
                    Value::Null => Ok(None),
                    act => activity::parse(act).map(|a| Some(Box::new(a))),
                };
                match activity {
                    Err(e) => error(4000, &format!("Invalid activity: {e}")),
                    Ok(a) => {
                        // Discord's defaults for the privacy flags: the game never sees the settings.
                        let data = a.as_ref().zip(self.app).map_or(Value::Null, |(a, app)| {
                            a.op3(
                                app,
                                "",
                                Privacy {
                                    friends: true,
                                    voice: true,
                                },
                                false,
                            )
                        });
                        let mut out = answer(data);
                        out.events.push(RpcEvent::Activity {
                            conn: self.id,
                            activity: a,
                            at: now,
                        });
                        out
                    }
                }
            }
            "SUBSCRIBE" | "UNSUBSCRIBE" => {
                let Some(evt) = EVENTS.into_iter().find(|e| v["evt"] == *e) else {
                    return error(4000, "Invalid event");
                };
                self.subscribed.retain(|e| *e != evt);
                if cmd == "SUBSCRIBE" {
                    self.subscribed.push(evt);
                }
                let mut out = answer(json!({"evt": evt}));
                out.events.push(RpcEvent::Subscribed {
                    conn: self.id,
                    events: self.subscribed.clone(),
                });
                out
            }
            // The host side is not built: no join request is ever pending.
            "SEND_ACTIVITY_JOIN_INVITE"
            | "CLOSE_ACTIVITY_JOIN_REQUEST"
            | "CLOSE_ACTIVITY_REQUEST" => error(4012, "No such join request"),
            _ => error(4002, "Not supported by Rustcord"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user() -> RpcUser {
        RpcUser {
            id: Id(7),
            username: "ann".into(),
            global_name: Some("Ann".into()),
            avatar: None,
            discriminator: "0".into(),
        }
    }

    fn hello(c: &mut Conn, body: &str) -> Out {
        c.on_frame(
            Frame {
                op: frame::HANDSHAKE,
                body: body.as_bytes().to_vec(),
            },
            Instant::now(),
        )
    }

    fn cmd(c: &mut Conn, body: Value) -> Out {
        c.on_frame(Frame::json(frame::FRAME, &body), Instant::now())
    }

    fn body(f: &Frame) -> Value {
        serde_json::from_slice(&f.body).unwrap()
    }

    fn closes(o: &Out, code: u32) {
        assert!(o.close && o.events.is_empty());
        assert_eq!(
            (o.frames[0].op, body(&o.frames[0])["code"].clone()),
            (frame::CLOSE, json!(code)),
            "{o:?}"
        );
    }

    fn shaken() -> Conn {
        let mut c = Conn::new(3, Some(user()));
        hello(&mut c, r#"{"v":1,"client_id":"1575590836601753835"}"#);
        c
    }

    #[test]
    fn handshake_then_ready_waits_for_the_user() {
        let mut c = Conn::new(3, None);
        let o = hello(&mut c, r#"{"v":1,"client_id":"1575590836601753835"}"#);
        assert!(o.frames.is_empty() && !o.close, "no user yet: wait");
        assert!(matches!(
            o.events[..],
            [RpcEvent::Connected {
                conn: 3,
                app: Id(1575590836601753835)
            }]
        ));
        let o = c.set_user(Some(user()));
        let ready = body(&o.frames[0]);
        assert_eq!(
            (&ready["cmd"], &ready["evt"], &ready["nonce"]),
            (&json!("DISPATCH"), &json!("READY"), &json!(null))
        );
        assert_eq!(
            ready["data"]["user"],
            json!({"id":"7","username":"ann","discriminator":"0","global_name":"Ann","avatar":null,"bot":false})
        );
        assert_eq!(ready["data"]["config"]["api_endpoint"], "//discord.com/api");
        assert!(c.set_user(Some(user())).frames.is_empty(), "READY once");
        // User known first: READY answers the handshake at once; a number client id works too.
        let mut c = Conn::new(4, Some(user()));
        let o = hello(&mut c, r#"{"v":1,"client_id":1575590836601753835}"#);
        assert_eq!((o.frames.len(), o.events.len()), (1, 1));
    }

    #[test]
    fn handshake_errors_close() {
        closes(
            &hello(
                &mut Conn::new(1, None),
                r#"{"v":2,"client_id":"1575590836601753835"}"#,
            ),
            4004,
        );
        closes(
            &hello(
                &mut Conn::new(1, None),
                r#"{"client_id":"1575590836601753835"}"#,
            ),
            4004,
        );
        for bad in [
            r#"{"v":1}"#,
            r#"{"v":1,"client_id":"123"}"#,
            r#"{"v":1,"client_id":"abcdefghijklmnopq"}"#,
            r#"{"v":1,"client_id":true}"#,
            r#"{"v":1,"client_id":"0000000000000000000"}"#,
        ] {
            closes(&hello(&mut Conn::new(1, None), bad), 4000);
        }
        closes(&hello(&mut Conn::new(1, None), "nope"), 1003);
        closes(
            &hello(
                &mut shaken(),
                r#"{"v":1,"client_id":"1575590836601753835"}"#,
            ),
            1003,
        );
        // A FRAME before the handshake; a bad JSON frame after it.
        closes(
            &cmd(&mut Conn::new(1, None), json!({"cmd":"SET_ACTIVITY"})),
            1003,
        );
        closes(
            &shaken().on_frame(
                Frame {
                    op: frame::FRAME,
                    body: b"{".to_vec(),
                },
                Instant::now(),
            ),
            1003,
        );
        assert!(
            shaken()
                .on_frame(
                    Frame {
                        op: frame::CLOSE,
                        body: Vec::new()
                    },
                    Instant::now()
                )
                .close,
            "the game closes"
        );
    }

    #[test]
    fn ping_pong_and_unknown_commands() {
        let o = shaken().on_frame(
            Frame {
                op: frame::PING,
                body: b"{\"nonce\":\"x\"}".to_vec(),
            },
            Instant::now(),
        );
        assert_eq!(
            (o.frames[0].op, &o.frames[0].body[..]),
            (frame::PONG, &b"{\"nonce\":\"x\"}"[..])
        );
        let o = cmd(
            &mut shaken(),
            json!({"cmd":"AUTHORIZE","args":{},"nonce":"n1"}),
        );
        assert_eq!(
            body(&o.frames[0]),
            json!({"cmd":"AUTHORIZE","evt":"ERROR","data":{"code":4002,"message":"Not supported by Rustcord"},"nonce":"n1"})
        );
        for c in [
            "SEND_ACTIVITY_JOIN_INVITE",
            "CLOSE_ACTIVITY_JOIN_REQUEST",
            "CLOSE_ACTIVITY_REQUEST",
        ] {
            let o = cmd(
                &mut shaken(),
                json!({"cmd":c,"args":{"user_id":"1"},"nonce":"n2"}),
            );
            assert_eq!(
                (
                    &body(&o.frames[0])["data"]["code"],
                    &body(&o.frames[0])["nonce"]
                ),
                (&json!(4012), &json!("n2")),
                "{c}"
            );
            assert!(!o.close);
        }
    }

    #[test]
    fn subscribe_and_unsubscribe() {
        let mut c = shaken();
        let o = cmd(
            &mut c,
            json!({"cmd":"SUBSCRIBE","evt":"ACTIVITY_JOIN","nonce":"a"}),
        );
        assert_eq!(
            body(&o.frames[0]),
            json!({"cmd":"SUBSCRIBE","data":{"evt":"ACTIVITY_JOIN"},"evt":null,"nonce":"a"})
        );
        cmd(
            &mut c,
            json!({"cmd":"SUBSCRIBE","evt":"ACTIVITY_INVITE","nonce":"b"}),
        );
        let o = cmd(
            &mut c,
            json!({"cmd":"SUBSCRIBE","evt":"ACTIVITY_JOIN","nonce":"c"}),
        );
        assert!(
            matches!(&o.events[..], [RpcEvent::Subscribed { conn: 3, events }] if events == &["ACTIVITY_INVITE", "ACTIVITY_JOIN"])
        );
        let o = cmd(
            &mut c,
            json!({"cmd":"UNSUBSCRIBE","evt":"ACTIVITY_INVITE","nonce":"d"}),
        );
        assert_eq!(body(&o.frames[0])["cmd"], "UNSUBSCRIBE");
        assert!(
            matches!(&o.events[..], [RpcEvent::Subscribed { events, .. }] if events == &["ACTIVITY_JOIN"])
        );
        let o = cmd(
            &mut c,
            json!({"cmd":"SUBSCRIBE","evt":"VOICE_STATE_CREATE","nonce":"e"}),
        );
        assert_eq!(
            (
                &body(&o.frames[0])["evt"],
                &body(&o.frames[0])["data"]["code"]
            ),
            (&json!("ERROR"), &json!(4000))
        );
        assert!(o.events.is_empty());
    }

    #[test]
    fn set_activity_answers_and_reports() {
        let mut c = shaken();
        // discord-rpc: seconds, a party, secrets.
        let o = cmd(
            &mut c,
            json!({"cmd":"SET_ACTIVITY","nonce":"1","args":{"pid":4242,"activity":{
            "state":"In a match","details":"Ranked","timestamps":{"start":1764183000},
            "party":{"id":"p1","size":[2,4],"privacy":1},"secrets":{"join":"j-1a2b3c"},"instance":true}}}),
        );
        let r = body(&o.frames[0]);
        assert_eq!(
            (&r["cmd"], &r["evt"], &r["nonce"]),
            (&json!("SET_ACTIVITY"), &json!(null), &json!("1"))
        );
        assert_eq!(
            (
                &r["data"]["timestamps"]["start"],
                &r["data"]["application_id"],
                &r["data"]["flags"]
            ),
            (
                &json!(1_764_183_000_000u64),
                &json!("1575590836601753835"),
                &json!(1 | 2 | 64 | 128)
            )
        );
        assert!(
            r["data"].get("secrets").is_none()
                && !o.frames[0].body.windows(8).any(|w| w == b"j-1a2b3c")
        );
        assert!(matches!(
            &o.events[..],
            [RpcEvent::Activity {
                conn: 3,
                activity: Some(_),
                ..
            }]
        ));
        assert!(!format!("{o:?}").contains("j-1a2b3c"));
        // pypresence: ms and buttons.
        let o = cmd(
            &mut c,
            json!({"cmd":"SET_ACTIVITY","nonce":"2","args":{"pid":1,"activity":{"state":"Hi there","timestamps":{"start":1764183000123u64},
            "buttons":[{"label":"Site","url":"https://x.example"}]}}}),
        );
        assert_eq!(body(&o.frames[0])["data"]["buttons"], json!(["Site"]));
        // Cleared by null or by no activity at all.
        for args in [
            json!({"pid":1,"activity":null}),
            json!({"pid":1}),
            json!(null),
        ] {
            let o = cmd(
                &mut c,
                json!({"cmd":"SET_ACTIVITY","nonce":"3","args":args}),
            );
            assert_eq!(body(&o.frames[0])["data"], json!(null));
            assert!(matches!(
                &o.events[..],
                [RpcEvent::Activity { activity: None, .. }]
            ));
        }
        // Invalid: ERROR 4000 naming the field, no event, the pipe stays open.
        let o = cmd(
            &mut c,
            json!({"cmd":"SET_ACTIVITY","nonce":"4","args":{"activity":{"state":"x".repeat(200)}}}),
        );
        let r = body(&o.frames[0]);
        assert_eq!(
            (&r["evt"], &r["data"]["code"], &r["nonce"]),
            (&json!("ERROR"), &json!(4000), &json!("4"))
        );
        assert!(
            r["data"]["message"].as_str().unwrap().contains("state")
                && o.events.is_empty()
                && !o.close
        );
    }
}
