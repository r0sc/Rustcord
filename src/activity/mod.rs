//! Activities like the official desktop client: game detection (a background thread lists
//! processes, `scan.rs`, and matches them against the detectable-games list, `detect.rs`, plus
//! the games I added), my game, and Rustcord's own "Playing Rustcord" (a local switch), in op 3 while "Share detected activity" (settings 11.3) is on,
//! and a picture for everyone's activity: rich-presence art, Spotify album art, else the
//! application's icon, else the type icon (drawn by the UI).
//!
//! Traffic, all through the rate limiter: `GET /games/detectable` at most once a day (saved
//! compact on disk), `GET /applications/public` once per application id that is shown without
//! art (batched; answers saved on disk), and one op 3 per change of the game I share or of Rustcord's own presence. The
//! Registered Games list stays on this device.
//!
//! OWNERSHIP: activities engineer.

mod detect;
mod scan;

pub use detect::Table;
pub use scan::{Service, Window, table_path};
// The game overlay's foreground watcher asks the same two questions as the scan (ui/overlay).
#[cfg(windows)]
pub(crate) use scan::{image as process_image, processes};

use crate::api::cdn;
use crate::backend::{Command, CommandSender, ConnState};
use crate::model::{Activity, Id, RichPresence};
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

pub use detect::{file_name, stale as table_stale};

/// Past this many answered app icons the memory cache starts over.
const MAX_ICONS: usize = 2000;
/// App icons asked for but not sent yet (rows shown while connected).
const MAX_WANTED: usize = 100;
/// A button whose URLs come later than this after the click is not opened any more.
const OPEN_WITHIN: std::time::Duration = std::time::Duration::from_secs(5);

/// Whose rich-presence buttons: (user, presence session, application).
pub type ButtonKey = (Id, String, Id);

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// My game as op 3 carries it (type 0).
#[derive(Clone, Debug, PartialEq)]
pub struct GameActivity {
    pub name: String,
    pub application_id: Option<Id>,
    /// When the game's process started (unix ms).
    pub start_ms: u64,
}

impl GameActivity {
    /// As a presence shows it.
    pub fn to_activity(&self) -> Activity {
        let rich = RichPresence { start: Some(self.start_ms), application_id: self.application_id, ..Default::default() };
        Activity { kind: 0, name: self.name.clone(), rich: Some(Box::new(rich)) }
    }
}

/// What op 3 says about Rustcord itself (type 0, after my game): see `FEATURES_NOTES` §1.3.
pub const APP_DETAILS: &str = "Third-party client, built in Rust";

/// When this process started (unix ms): a reconnect keeps the timer.
static STARTED: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

/// "Playing Rustcord" as op 3 carries it. Nothing personal: version and uptime only.
#[derive(Clone, Debug, PartialEq)]
pub struct AppActivity {
    /// Only when the user set one (Settings > Activity privacy): its icon becomes the picture.
    pub application_id: Option<Id>,
    pub start_ms: u64,
}

impl AppActivity {
    /// As a presence shows it.
    pub fn to_activity(&self) -> Activity {
        let state = format!("Version {}", env!("CARGO_PKG_VERSION"));
        let rich = RichPresence {
            details: Some(APP_DETAILS.into()),
            state: Some(state),
            start: Some(self.start_ms),
            application_id: self.application_id,
            ..Default::default()
        };
        Activity { kind: 0, name: crate::config::APP_NAME.into(), rich: Some(Box::new(rich)) }
    }
}

/// Whether `a` is Rustcord's own presence (not a game that happens to be named Rustcord).
pub fn is_rustcord(a: &Activity) -> bool {
    a.kind == 0 && a.name == crate::config::APP_NAME && a.rich.as_ref().is_some_and(|r| r.details.as_deref() == Some(APP_DETAILS))
}

/// A program I added on the Registered Games page ("Add it!").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AddedGame {
    pub name: String,
    /// `detect::normalize`d full path.
    pub exe: String,
}

#[derive(Debug)]
pub enum ActivityCommand {
    /// My game and Rustcord's own presence in op 3 (None: not shown), keeping my status and
    /// custom status. The game comes first.
    SetActivities { game: Option<GameActivity>, app: Option<AppActivity> },
    /// `GET /applications/public` for these ids (≤ 25) -> `ActivityEvent::Apps`.
    FetchApps(Vec<Id>),
    /// Someone's button URLs, after a click on one -> `ActivityEvent::Buttons`.
    FetchButtons(ButtonKey),
    /// Save my added games (local only) and scan again.
    SetAdded(Vec<AddedGame>),
    /// The windows the "Add it!" picker offers -> `ActivityEvent::Windows`.
    ListWindows,
}

#[derive(Debug)]
pub enum ActivityEvent {
    /// The games list (from disk, or fetched).
    Table(Arc<Table>),
    /// The game I am playing changed.
    Detected(Option<GameActivity>),
    /// My added games (at start).
    Added(Vec<AddedGame>),
    /// App icon hashes (None: none, or unknown app).
    Apps(Vec<(Id, Option<String>)>),
    /// Button URLs (None: the request failed, so a later click asks again).
    Buttons(ButtonKey, Option<Vec<String>>),
    Windows(Vec<Window>),
}

/// `AppState::activity`.
#[derive(Debug, Default)]
pub struct ActivityState {
    pub table: Option<Arc<Table>>,
    /// The game running now (shared or not).
    pub detected: Option<GameActivity>,
    pub added: Vec<AddedGame>,
    /// The "Add it!" picker's list, once answered.
    pub windows: Option<Vec<Window>>,
    /// `/applications/public` answers; None = no icon, or asked and not answered yet.
    icons: HashMap<Id, Option<String>>,
    /// Asked for by drawn rows, sent by `logic`.
    wanted: Vec<Id>,
    /// Rustcord's own presence as the settings want it now (set each frame, also offline).
    pub app: Option<AppActivity>,
    /// What my session's presence carries (the last `SetActivities`).
    sent: (Option<GameActivity>, Option<AppActivity>),
    /// Fetched button URLs.
    button_urls: HashMap<ButtonKey, Vec<String>>,
    /// The button clicked while its URLs are on their way (also: asked, do not ask again).
    opening: Option<(ButtonKey, usize, std::time::Instant)>,
}

impl ActivityState {
    /// Returns a clicked button's URL to open now, once its answer came in time.
    pub fn apply(&mut self, ev: ActivityEvent) -> Option<String> {
        match ev {
            ActivityEvent::Table(t) => self.table = Some(t),
            ActivityEvent::Detected(g) => self.detected = g,
            ActivityEvent::Added(a) => self.added = a,
            ActivityEvent::Apps(list) => {
                if self.icons.len() + list.len() > MAX_ICONS {
                    self.icons.clear();
                }
                self.icons.extend(list);
            }
            ActivityEvent::Windows(w) => self.windows = Some(w),
            ActivityEvent::Buttons(key, urls) => {
                let click = self.opening.take_if(|(k, ..)| *k == key);
                let urls = urls?;
                let open = click.filter(|(.., at)| at.elapsed() < OPEN_WITHIN).and_then(|(_, i, _)| urls.get(i).cloned());
                if self.button_urls.len() >= MAX_ICONS {
                    self.button_urls.clear();
                }
                self.button_urls.insert(key, urls);
                return open;
            }
        }
        None
    }

    /// What survives a logout: this device's games, not the account's session or its friends'
    /// app icons.
    pub fn after_logout(self) -> Self {
        Self { table: self.table, detected: self.detected, added: self.added, ..Default::default() }
    }

    /// Application `id`'s icon hash from the games list or an answer; else it is asked for
    /// (when `ask`). Returns (hash, newly asked).
    fn app_icon(&mut self, id: Id, ask: bool) -> (Option<String>, bool) {
        if let Some(h) = self.table.as_ref().and_then(|t| t.icon(id)) {
            return (Some(h), false);
        }
        match self.icons.get(&id) {
            Some(h) => (h.clone(), false),
            None if ask && !self.wanted.contains(&id) && self.wanted.len() < MAX_WANTED => {
                self.wanted.push(id);
                (None, true)
            }
            None => (None, false),
        }
    }
}

/// "Share detected activity" (settings proto 11.3, default on).
pub fn sharing(state: &AppState) -> bool {
    state.account.settings.proto_varint(11, &[3], true).is_none_or(|v| v != 0)
}

/// My detected game while I share it, as others see it.
pub fn my_game(state: &AppState) -> Option<Activity> {
    state.activity.detected.as_ref().filter(|_| sharing(state)).map(GameActivity::to_activity)
}

/// Someone's activity as rows and cards show it: their presence's; mine falls back to my shared
/// game, then Rustcord's own (my presence echo lags, and offline it never comes).
pub fn activity_of(state: &AppState, user: Id) -> Option<Cow<'_, Activity>> {
    if let Some(a) = state.presence_of(user).and_then(|p| p.activity.as_ref()) {
        return Some(Cow::Borrowed(a));
    }
    state.me.as_ref().filter(|m| m.id == user).and_then(|_| my_game(state).or_else(|| state.activity.app.as_ref().map(AppActivity::to_activity))).map(Cow::Owned)
}

/// A click on custom button `i` of `user`'s activity `a`: its URL when known (my own, or fetched
/// before), else one metadata request (while connected) whose answer opens it. The caller opens
/// web links only.
pub fn button_url(state: &mut AppState, cmd: &CommandSender, user: Id, a: &Activity, i: usize) -> Option<String> {
    let r = a.rich.as_deref()?;
    if let Some(u) = r.button_urls.get(i) {
        return Some(u.clone());
    }
    let key = (user, r.session_id.clone()?, r.application_id?);
    if let Some(urls) = state.activity.button_urls.get(&key) {
        return urls.get(i).cloned();
    }
    let asked = state.activity.opening.as_ref().is_some_and(|(k, ..)| *k == key);
    if online(state) {
        if !asked {
            cmd.send(Command::Activity(ActivityCommand::FetchButtons(key.clone())));
        }
        state.activity.opening = Some((key, i, std::time::Instant::now()));
    }
    None
}

fn online(state: &AppState) -> bool {
    state.conn == ConnState::Connected && !crate::config::offline_mode()
}

/// The picture of `a` for a `px` square: its rich-presence image (Spotify album art included),
/// else its application's icon; None = draw the type icon. An unknown application is asked
/// for once (batched by `logic` on the next frame).
pub fn art_url(ctx: &egui::Context, state: &mut AppState, a: &Activity, px: u32) -> Option<String> {
    if let Some(url) = cdn::activity_image_url(a, false, px) {
        return Some(url);
    }
    let id = a.rich.as_ref()?.application_id?;
    let ask = online(state);
    let (hash, asked) = state.activity.app_icon(id, ask);
    if asked {
        ctx.request_repaint();
    }
    cdn::app_icon_url(id, &hash?, px)
}

/// Each frame (`App::logic`, also while minimized), only while connected (READY's settings are
/// in by then): one op 3 when the game to share or Rustcord's presence changed, and the batched
/// app-icon request.
pub fn logic(state: &mut AppState, settings: &crate::config::Settings, cmd: &CommandSender) {
    let p = &settings.rustcord_presence;
    state.activity.app = p.enabled.then(|| AppActivity { application_id: p.application_id, start_ms: *STARTED.get_or_init(now_ms) });
    if !online(state) {
        return;
    }
    let want = (state.activity.detected.clone().filter(|_| sharing(state)), state.activity.app.clone());
    if want != state.activity.sent {
        state.activity.sent = want.clone();
        cmd.send(Command::Activity(ActivityCommand::SetActivities { game: want.0, app: want.1 }));
    }
    let a = &mut state.activity;
    if !a.wanted.is_empty() {
        let ids: Vec<Id> = a.wanted.drain(..a.wanted.len().min(crate::api::apps::MAX_APP_IDS)).collect();
        // Asked once: an unknown app or a failed request is not asked again this session.
        a.icons.extend(ids.iter().map(|&id| (id, None)));
        cmd.send(Command::Activity(ActivityCommand::FetchApps(ids)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::settings_proto::SettingValue;

    fn sent(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Command>) -> Vec<String> {
        std::iter::from_fn(|| rx.try_recv().ok()).map(|c| format!("{c:?}")).collect()
    }

    fn set(game: Option<&GameActivity>, app: Option<AppActivity>) -> String {
        format!("{:?}", Command::Activity(ActivityCommand::SetActivities { game: game.cloned(), app }))
    }

    #[test]
    fn op3_once_per_change_and_only_while_shared() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cmd = CommandSender(tx);
        let mut s = AppState::default();
        let mut cfg = crate::config::Settings::default();
        let game = GameActivity { name: "Factorio".into(), application_id: Some(Id(7)), start_ms: 5 };
        s.activity.apply(ActivityEvent::Detected(Some(game.clone())));
        logic(&mut s, &cfg, &cmd);
        assert!(sent(&mut rx).is_empty(), "not connected: READY's settings may not be in");
        s.conn = ConnState::Connected;
        logic(&mut s, &cfg, &cmd);
        let app = s.activity.app.clone();
        assert!(app.is_some(), "on by default");
        assert_eq!(sent(&mut rx), [set(Some(&game), app.clone())]);
        logic(&mut s, &cfg, &cmd);
        assert!(sent(&mut rx).is_empty(), "no change, no op 3");
        // Sharing switched off: the game goes, once; Rustcord stays.
        s.account.settings.set_local(11, &[(vec![3], SettingValue::Wrapped(0))]);
        assert!(my_game(&s).is_none());
        logic(&mut s, &cfg, &cmd);
        logic(&mut s, &cfg, &cmd);
        assert_eq!(sent(&mut rx), [set(None, app.clone())]);
        // While off, a new game sends nothing.
        s.activity.apply(ActivityEvent::Detected(Some(GameActivity { name: "Celeste".into(), ..game.clone() })));
        logic(&mut s, &cfg, &cmd);
        assert!(sent(&mut rx).is_empty());
        // Rustcord's switch off: one op 3 without it; an Application ID: one with it.
        cfg.rustcord_presence.enabled = false;
        logic(&mut s, &cfg, &cmd);
        logic(&mut s, &cfg, &cmd);
        assert_eq!(sent(&mut rx), [set(None, None)]);
        cfg.rustcord_presence.enabled = true;
        cfg.rustcord_presence.application_id = Some(Id(123456789012345678));
        logic(&mut s, &cfg, &cmd);
        assert_eq!(sent(&mut rx), [set(None, Some(AppActivity { application_id: Some(Id(123456789012345678)), ..app.clone().unwrap() }))]);
        // A new login starts without a presence: sent again once.
        s.account.settings.set_local(11, &[(vec![3], SettingValue::Wrapped(1))]);
        s.activity = std::mem::take(&mut s.activity).after_logout();
        assert!(s.activity.table.is_none() && s.activity.detected.is_some());
        logic(&mut s, &cfg, &cmd);
        assert_eq!(sent(&mut rx).len(), 1);
        // Offline: nothing is sent, but my own views still know the activity.
        s.conn = ConnState::Offline;
        s.activity.sent = Default::default();
        logic(&mut s, &cfg, &cmd);
        assert!(sent(&mut rx).is_empty() && s.activity.app.is_some());
    }

    #[test]
    fn rustcord_activity_is_recognised_and_my_fallback() {
        let app = AppActivity { application_id: None, start_ms: 9 };
        let a = app.to_activity();
        assert!(is_rustcord(&a));
        let rich = a.rich.as_deref().unwrap();
        assert_eq!((rich.details.as_deref(), rich.state.as_deref(), rich.start), (Some(APP_DETAILS), Some(concat!("Version ", env!("CARGO_PKG_VERSION"))), Some(9)));
        let game = GameActivity { name: "Rustcord".into(), application_id: None, start_ms: 1 }.to_activity();
        assert!(!is_rustcord(&game), "a game named Rustcord is not it");
        let mut s = AppState::default();
        s.me = Some(crate::model::User { id: Id(1), ..Default::default() });
        assert!(activity_of(&s, Id(1)).is_none());
        s.activity.app = Some(app.clone());
        assert_eq!(activity_of(&s, Id(1)).as_deref(), Some(&a));
        assert!(activity_of(&s, Id(2)).is_none(), "only mine falls back");
        s.activity.apply(ActivityEvent::Detected(Some(GameActivity { name: "Celeste".into(), application_id: None, start_ms: 1 })));
        assert_eq!(activity_of(&s, Id(1)).map(|a| a.name.clone()).as_deref(), Some("Celeste"));
    }

    #[test]
    fn button_urls_are_fetched_on_a_click_and_opened_once() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cmd = CommandSender(tx);
        let mut s = AppState::default();
        let rich = RichPresence { buttons: vec!["Watch".into(), "Site".into()], session_id: Some("abc".into()), application_id: Some(Id(8)), ..Default::default() };
        let a = Activity { kind: 0, name: "Red Eclipse".into(), rich: Some(Box::new(rich)) };
        let key: ButtonKey = (Id(2), "abc".into(), Id(8));
        assert_eq!(button_url(&mut s, &cmd, Id(2), &a, 1), None);
        assert!(sent(&mut rx).is_empty(), "offline: nothing is asked");
        s.conn = ConnState::Connected;
        assert_eq!(button_url(&mut s, &cmd, Id(2), &a, 0), None);
        assert_eq!(button_url(&mut s, &cmd, Id(2), &a, 1), None);
        assert_eq!(sent(&mut rx), [format!("{:?}", Command::Activity(ActivityCommand::FetchButtons(key.clone())))], "asked once");
        let urls = vec!["https://twitch.tv/x".to_owned(), "https://x.example".to_owned()];
        assert_eq!(s.activity.apply(ActivityEvent::Buttons(key.clone(), Some(urls.clone()))).as_deref(), Some("https://x.example"), "the last click opens");
        assert_eq!(s.activity.apply(ActivityEvent::Buttons(key.clone(), Some(urls))), None, "once");
        assert_eq!(button_url(&mut s, &cmd, Id(2), &a, 0).as_deref(), Some("https://twitch.tv/x"), "known now");
        assert!(sent(&mut rx).is_empty());
        // A failed request is asked again on the next click; a late answer opens nothing.
        let other: ButtonKey = (Id(3), "abc".into(), Id(8));
        assert_eq!(button_url(&mut s, &cmd, Id(3), &a, 0), None);
        assert_eq!(s.activity.apply(ActivityEvent::Buttons(other.clone(), None)), None);
        assert_eq!(button_url(&mut s, &cmd, Id(3), &a, 0), None);
        assert_eq!(sent(&mut rx).len(), 2);
        s.activity.opening.as_mut().unwrap().2 -= OPEN_WITHIN;
        assert_eq!(s.activity.apply(ActivityEvent::Buttons(other, Some(vec!["https://y.example".into()]))), None, "too late");
        // My own presence carries its URLs; without a session or an app id nothing is asked.
        let mine = Activity { rich: Some(Box::new(RichPresence { buttons: vec!["Get".into()], button_urls: vec!["https://own.example".into()], ..Default::default() })), ..Default::default() };
        assert_eq!(button_url(&mut s, &cmd, Id(1), &mine, 0).as_deref(), Some("https://own.example"));
        assert_eq!(button_url(&mut s, &cmd, Id(1), &mine, 1), None);
        assert!(sent(&mut rx).is_empty());
    }

    #[test]
    fn app_icons_are_asked_once_in_batches() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cmd = CommandSender(tx);
        let mut s = AppState::default();
        // Rustcord's own presence is not under test here.
        let off = crate::config::Settings { rustcord_presence: crate::config::RustcordPresence { enabled: false, ..Default::default() }, ..Default::default() };
        s.conn = ConnState::Connected;
        let ctx = egui::Context::default();
        let game = |id: u64| GameActivity { name: "x".into(), application_id: Some(Id(id)), start_ms: 0 }.to_activity();
        for id in [1, 2, 1, 3, 2] {
            assert_eq!(art_url(&ctx, &mut s, &game(id), 64), None);
        }
        logic(&mut s, &off, &cmd);
        assert_eq!(sent(&mut rx), ["Activity(FetchApps([1, 2, 3]))"]);
        for id in [1, 2, 3] {
            assert_eq!(art_url(&ctx, &mut s, &game(id), 64), None);
        }
        logic(&mut s, &off, &cmd);
        assert!(sent(&mut rx).is_empty(), "asked: not again, even before the answer");
        s.activity.apply(ActivityEvent::Apps(vec![(Id(1), Some("abc".into())), (Id(2), None)]));
        assert_eq!(art_url(&ctx, &mut s, &game(1), 64).as_deref(), Some("https://cdn.discordapp.com/app-icons/1/abc.png?size=64"));
        // The games list answers without a request.
        let json = r#"[{"id":"9","name":"x","icon_hash":"0123456789abcdef0123456789abcdef"}]"#;
        s.activity.apply(ActivityEvent::Table(Arc::new(Table::from_json(json.as_bytes()).unwrap())));
        assert!(art_url(&ctx, &mut s, &game(9), 64).is_some_and(|u| u.contains("/app-icons/9/0123456789abcdef0123456789abcdef.png")));
        // Rich-presence art first; offline nothing is asked.
        let mut spotify = game(4);
        spotify.rich.as_mut().unwrap().large_image = Some("spotify:ab67616d0000b2734f8ef2c2c1b3a9a1c4d5e6f7".into());
        assert!(art_url(&ctx, &mut s, &spotify, 64).is_some_and(|u| u.starts_with("https://i.scdn.co/image/")));
        s.conn = ConnState::Offline;
        assert_eq!(art_url(&ctx, &mut s, &game(5), 64), None);
        s.conn = ConnState::Connected;
        logic(&mut s, &off, &cmd);
        assert!(sent(&mut rx).is_empty());
    }
}
