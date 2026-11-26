//! Discord Quests (docs/QUESTS_NOTES.md): the wire shapes Rustcord reads, the page's state and the
//! video progress rule. Pure data + logic: no I/O, no egui.
//!
//! Rustcord shows quests, accepts and claims them on a click, and reports progress only for a
//! quest video the user really plays in its player (`VideoProgress`). It never sends quest
//! heartbeats: those need the official desktop app's identity (QUESTS_NOTES §0, D2).
//!
//! OWNERSHIP: quests engineer.

use crate::auth::Secret;
use crate::model::{Id, null_default};
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// `location` sent with enroll / claim: QUEST_HOME_DESKTOP (UNCONFIRMED that Quest Home sends it).
pub const LOCATION_QUEST_HOME: u8 = 11;
/// The page fetches again at most this often (no polling: gateway events carry the changes).
pub const REFETCH: Duration = Duration::from_secs(10 * 60);
/// Card notice after a failed video progress send (the watch session stops).
pub const VIDEO_FAILED: &str = "Progress stopped saving. Try the video again later.";
/// Card notice when the player lost its connection (the session ends once its window is closed).
pub const VIDEO_OFFLINE: &str = "The video stopped: no connection. Continue watching when you're back online.";
/// The ids of quests whose "Quest complete" notification is done, in the cache dir (one per line).
pub const NOTIFIED_FILE: &str = "quests-notified.txt";

/// Feature flags (UD Quest Feature).
const FEATURE_FORCE_HLS: u32 = 18;
const FEATURE_DO_NOT_DISPLAY: u32 = 21;

/// What the UI asks the backend (`Command::Quests`); each one is a user click, the page open's
/// fetch, or video progress while the user plays the quest video.
#[derive(Debug)]
pub enum QuestCommand {
    /// GET /quests/@me, then GET /users/@me/virtual-currency/balance -> `Loaded`, `Orbs`.
    Fetch,
    /// POST /quests/{id}/enroll -> `Status` | `Failed`.
    Enroll { quest_id: Id, sealed: Option<String> },
    /// POST /quests/{id}/video-progress -> `VideoSaved` | `Failed { stop_video: true }`.
    VideoProgress { quest_id: Id, timestamp: u32 },
    /// POST /quests/{id}/claim-reward -> `Claimed` | `Failed`.
    Claim { quest_id: Id, platform: u8, sealed: Option<String> },
    /// GET /quests/{id}/reward-code -> `Code`.
    RewardCode { quest_id: Id },
}

/// Answers and gateway dispatches (`BackendEvent::Quests`, `GatewayEvent::Quests`).
#[derive(Clone, Debug)]
pub enum QuestEvent {
    Loaded(Result<QuestsPage, String>),
    Orbs(u64),
    /// Enroll answer or QUESTS_USER_STATUS_UPDATE.
    Status(Box<QuestUserStatus>),
    /// Answer to a video progress send (the watch session may send the next one).
    VideoSaved(Box<QuestUserStatus>),
    /// QUESTS_USER_COMPLETION_UPDATE.
    Blocked(Option<String>),
    Claimed { quest_id: Id, claimed_at: Option<String>, error: Option<String> },
    /// Shared so the event stays `Clone`; the code is zeroized when the last copy drops.
    Code { quest_id: Id, code: Result<Arc<Secret>, String> },
    Failed { quest_id: Id, message: String, stop_video: bool },
}

// ----- wire shapes (subset of UD; unknown fields ignored, nulls become defaults) ---------------

/// `GET /quests/@me`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct QuestsPage {
    /// One quest that does not parse is left out, not the whole page.
    #[serde(deserialize_with = "lenient_list")]
    pub quests: Vec<Quest>,
    #[serde(deserialize_with = "lenient_list")]
    pub excluded_quests: Vec<Excluded>,
    pub quest_enrollment_blocked_until: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Excluded {
    pub id: Id,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Quest {
    pub id: Id,
    pub config: QuestConfig,
    pub user_status: Option<QuestUserStatus>,
    #[serde(deserialize_with = "null_default")]
    pub preview: bool,
    /// Passed through on enroll / claim when present; never invented.
    pub traffic_metadata_sealed: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct QuestConfig {
    #[serde(deserialize_with = "null_default")]
    pub expires_at: String,
    #[serde(deserialize_with = "null_default")]
    pub features: Vec<u32>,
    pub application: QuestApp,
    pub assets: QuestAssets,
    pub colors: QuestColors,
    pub messages: QuestMessages,
    /// v1 `task_config` is discontinued: a quest without v2 says "Open this quest in the Discord app."
    pub task_config_v2: Option<TaskConfig>,
    pub rewards_config: RewardsConfig,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct QuestApp {
    pub id: Id,
    #[serde(deserialize_with = "null_default")]
    pub name: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct QuestAssets {
    pub hero: Option<String>,
    pub game_tile: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct QuestColors {
    #[serde(deserialize_with = "null_default")]
    pub primary: String,
    #[serde(deserialize_with = "null_default")]
    pub secondary: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct QuestMessages {
    #[serde(deserialize_with = "null_default")]
    pub quest_name: String,
    #[serde(deserialize_with = "null_default")]
    pub game_title: String,
    #[serde(deserialize_with = "null_default")]
    pub game_publisher: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct TaskConfig {
    #[serde(deserialize_with = "null_default")]
    pub join_operator: String,
    #[serde(deserialize_with = "null_default")]
    pub tasks: BTreeMap<String, QuestTask>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct QuestTask {
    /// Seconds for duration tasks.
    #[serde(deserialize_with = "null_default")]
    pub target: u32,
    pub assets: TaskAssets,
    pub messages: TaskMessages,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct TaskAssets {
    pub video: Option<VideoAsset>,
    pub video_low_res: Option<VideoAsset>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct VideoAsset {
    #[serde(deserialize_with = "null_default")]
    pub url: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct TaskMessages {
    pub video_title: Option<String>,
    pub task_description: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RewardsConfig {
    #[serde(deserialize_with = "lenient_list")]
    pub rewards: Vec<QuestReward>,
    pub rewards_expire_at: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub platforms: Vec<u8>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct QuestReward {
    /// 1 code, 2 in-game, 3 collectible, 4 Orbs, 5 Nitro trial.
    #[serde(rename = "type", deserialize_with = "null_default")]
    pub kind: u8,
    pub messages: RewardMessages,
    pub orb_quantity: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RewardMessages {
    #[serde(deserialize_with = "null_default")]
    pub name: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct QuestUserStatus {
    pub quest_id: Id,
    pub enrolled_at: Option<String>,
    pub completed_at: Option<String>,
    pub claimed_at: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub progress: BTreeMap<String, TaskProgress>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct TaskProgress {
    #[serde(deserialize_with = "null_default")]
    pub value: f64,
}

/// A list whose bad entries are skipped (null = empty).
fn lenient_list<'de, D: Deserializer<'de>, T: DeserializeOwned>(d: D) -> Result<Vec<T>, D::Error> {
    let raw = Option::<Vec<serde_json::Value>>::deserialize(d)?.unwrap_or_default();
    Ok(raw.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect())
}

// ----- logic --------------------------------------------------------------------------------------

/// What a card's task asks for (QUESTS_NOTES §2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskKind {
    Video,
    Desktop,
    Stream,
    Activity,
    Mobile,
    Console,
    Achievement,
    Other,
}

/// Where a quest shows on the page; `Hidden` ones don't.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Claimable,
    InProgress,
    Available,
    Claimed,
    Hidden,
}

impl Phase {
    /// The page's sections, in order.
    pub const SECTIONS: [Phase; 4] = [Phase::Claimable, Phase::InProgress, Phase::Available, Phase::Claimed];
}

/// Card task when an "or" quest lists several: the first of these present, else the first key.
const TASK_ORDER: [(&str, TaskKind); 9] = [
    ("WATCH_VIDEO", TaskKind::Video),
    ("PLAY_ON_DESKTOP", TaskKind::Desktop),
    ("STREAM_ON_DESKTOP", TaskKind::Stream),
    ("PLAY_ACTIVITY", TaskKind::Activity),
    ("ACHIEVEMENT_IN_GAME", TaskKind::Achievement),
    ("ACHIEVEMENT_IN_ACTIVITY", TaskKind::Achievement),
    ("PLAY_ON_XBOX", TaskKind::Console),
    ("PLAY_ON_PLAYSTATION", TaskKind::Console),
    ("WATCH_VIDEO_ON_MOBILE", TaskKind::Mobile),
];

pub fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|t| t.with_timezone(&Utc))
}

impl Quest {
    fn status(&self) -> Option<&QuestUserStatus> {
        self.user_status.as_ref()
    }

    pub fn phase(&self, now: DateTime<Utc>) -> Phase {
        let c = &self.config;
        if self.preview || c.features.contains(&FEATURE_DO_NOT_DISPLAY) {
            return Phase::Hidden;
        }
        let claimed = self.status().is_some_and(|s| s.claimed_at.is_some());
        let past = |t: Option<&str>| t.and_then(parse_time).is_some_and(|t| t <= now);
        let expires = Some(c.expires_at.as_str()).filter(|s| !s.is_empty());
        if self.completed() && !claimed {
            // The reward has its own deadline; without one the quest's end is it.
            return if past(c.rewards_config.rewards_expire_at.as_deref().or(expires)) { Phase::Hidden } else { Phase::Claimable };
        }
        if past(expires) {
            Phase::Hidden
        } else if claimed {
            Phase::Claimed
        } else if self.enrolled() {
            Phase::InProgress
        } else {
            Phase::Available
        }
    }

    /// The card's task (QUESTS_NOTES §2 order); None without `task_config_v2` or tasks.
    pub fn task(&self) -> Option<(TaskKind, &QuestTask)> {
        let tasks = &self.config.task_config_v2.as_ref()?.tasks;
        TASK_ORDER.iter().find_map(|(k, kind)| tasks.get(*k).map(|t| (*kind, t))).or_else(|| tasks.values().next().map(|t| (TaskKind::Other, t)))
    }

    /// (value, target) in seconds or counts: "or" quests the furthest task, "and" quests the least.
    pub fn progress(&self) -> (u32, u32) {
        let Some(tc) = &self.config.task_config_v2 else { return (0, 0) };
        let value = |k: &str| self.status().and_then(|s| s.progress.get(k)).map_or(0.0, |p| p.value);
        let ratio = |&(v, t): &(u32, u32)| if t == 0 { 0.0 } else { f64::from(v) / f64::from(t) };
        let pairs = tc.tasks.iter().map(|(k, t)| ((value(k).max(0.0) as u32).min(t.target), t.target));
        let pick = if tc.join_operator == "and" {
            pairs.min_by(|a, b| ratio(a).total_cmp(&ratio(b)))
        } else {
            pairs.max_by(|a, b| ratio(a).total_cmp(&ratio(b)))
        };
        pick.unwrap_or((0, 0))
    }

    /// Server progress of the video task (seconds).
    pub fn video_value(&self) -> u32 {
        self.status().and_then(|s| s.progress.get("WATCH_VIDEO")).map_or(0, |p| p.value.max(0.0) as u32)
    }

    /// The MP4 the in-app player may play (QUESTS_NOTES §1.5); None if HLS-only or not a
    /// `WATCH_VIDEO` card.
    pub fn video_url(&self) -> Option<String> {
        if self.config.features.contains(&FEATURE_FORCE_HLS) {
            return None;
        }
        let (TaskKind::Video, t) = self.task()? else { return None };
        [&t.assets.video_low_res, &t.assets.video].into_iter().flatten().find_map(|v| crate::api::cdn::quest_video_url(self.id, &v.url))
    }

    /// The claim's `platform`: none or one listed -> it (0 = cross-platform); several -> None
    /// (the user picks).
    pub fn claim_platform(&self) -> Option<u8> {
        match self.config.rewards_config.platforms.as_slice() {
            [] => Some(0),
            [p] => Some(*p),
            _ => None,
        }
    }

    pub fn enrolled(&self) -> bool {
        self.status().is_some_and(|s| s.enrolled_at.is_some())
    }

    pub fn completed(&self) -> bool {
        self.status().is_some_and(|s| s.completed_at.is_some())
    }
}

/// Fetch state of the page.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Load {
    #[default]
    NotAsked,
    Loading,
    Failed(String),
    Loaded(Instant),
}

/// Everything the Quests page shows (`AppState::quests`; not cached, dropped on logout).
/// `Debug` shows no reward code (`Secret` is redacted).
#[derive(Debug, Default)]
pub struct QuestState {
    pub quests: Vec<Quest>,
    pub load: Load,
    pub blocked_until: Option<String>,
    pub orbs: Option<u64>,
    /// Quests with a click on its way (their button is disabled until the answer).
    pub busy: HashSet<Id>,
    /// Card notices (errors); the next click on that card clears its notice.
    pub notices: HashMap<Id, String>,
    /// Reward codes the user asked to see; never cached or logged.
    pub codes: HashMap<Id, Arc<Secret>>,
    /// Quests whose video failed in the player: their card falls back to the official-app row.
    pub no_video: HashSet<Id>,
    /// An event changed the list since the last notification check (`ui::quests::completed`).
    pub changed: bool,
    /// Quests whose "Quest complete" notification is done (or suppressed); None until read from
    /// `NOTIFIED_FILE`.
    pub notified: Option<BTreeSet<Id>>,
}

impl QuestState {
    /// Applies an answer or dispatch; returns (quest, ok) for the answer to a video send.
    pub fn apply(&mut self, ev: QuestEvent) -> Option<(Id, bool)> {
        self.changed = true;
        match ev {
            QuestEvent::Loaded(Ok(page)) => {
                let excluded: HashSet<Id> = page.excluded_quests.iter().map(|e| e.id).collect();
                self.quests = page.quests.into_iter().filter(|q| !excluded.contains(&q.id)).collect();
                self.blocked_until = page.quest_enrollment_blocked_until;
                self.load = Load::Loaded(Instant::now());
            }
            // A failed background refetch keeps the list on screen and tries again later.
            QuestEvent::Loaded(Err(_)) if !self.quests.is_empty() => self.load = Load::Loaded(Instant::now()),
            QuestEvent::Loaded(Err(e)) => self.load = Load::Failed(e),
            QuestEvent::Orbs(n) => self.orbs = Some(n),
            QuestEvent::Status(s) => self.set_status(*s),
            QuestEvent::VideoSaved(s) => {
                let id = s.quest_id;
                self.set_status(*s);
                return Some((id, true));
            }
            QuestEvent::Blocked(until) => self.blocked_until = until,
            QuestEvent::Claimed { quest_id, claimed_at, error } => {
                self.busy.remove(&quest_id);
                match claimed_at {
                    Some(at) => {
                        if let Some(s) = self.quest_mut(quest_id).and_then(|q| q.user_status.as_mut()) {
                            s.claimed_at = Some(at);
                        }
                    }
                    None => _ = self.notices.insert(quest_id, error.unwrap_or_else(|| "Discord didn't give the reward. Try again later.".into())),
                }
            }
            QuestEvent::Code { quest_id, code } => {
                self.busy.remove(&quest_id);
                match code {
                    Ok(c) => _ = self.codes.insert(quest_id, c),
                    Err(e) => _ = self.notices.insert(quest_id, e),
                }
            }
            QuestEvent::Failed { quest_id, message, stop_video } => {
                self.busy.remove(&quest_id);
                self.notices.insert(quest_id, message);
                if stop_video {
                    return Some((quest_id, false));
                }
            }
        }
        None
    }

    fn quest_mut(&mut self, id: Id) -> Option<&mut Quest> {
        self.quests.iter_mut().find(|q| q.id == id)
    }

    /// A status for a listed quest replaces its own; others are ignored (the next fetch lists them).
    fn set_status(&mut self, s: QuestUserStatus) {
        let id = s.quest_id;
        if let Some(q) = self.quest_mut(id) {
            q.user_status = Some(s);
            self.busy.remove(&id);
        }
    }

    /// Never fetched, or the last fetch is `REFETCH` old (a failed one waits for Retry).
    pub fn should_fetch(&self, now: Instant) -> bool {
        match self.load {
            Load::NotAsked => true,
            Load::Loaded(at) => now.saturating_duration_since(at) >= REFETCH,
            Load::Loading | Load::Failed(_) => false,
        }
    }

    /// Rewards waiting to be claimed (the Quests row badge).
    pub fn claimable(&self) -> usize {
        let now = Utc::now();
        self.quests.iter().filter(|q| q.phase(now) == Phase::Claimable).count()
    }

    /// The page's non-empty sections in order, quests in server order.
    pub fn sections(&self, now: DateTime<Utc>) -> Vec<(Phase, Vec<&Quest>)> {
        Phase::SECTIONS
            .into_iter()
            .map(|p| (p, self.quests.iter().filter(|q| q.phase(now) == p).collect::<Vec<_>>()))
            .filter(|(_, qs)| !qs.is_empty())
            .collect()
    }

    pub fn quest(&self, id: Id) -> Option<&Quest> {
        self.quests.iter().find(|q| q.id == id)
    }
}

/// Listed quests whose reward now waits (Claimable) and that `notified` doesn't hold; adds them.
/// Prunes `notified` to the listed quests first.
pub fn newly_claimable(quests: &[Quest], notified: &mut BTreeSet<Id>, now: DateTime<Utc>) -> Vec<Id> {
    notified.retain(|id| quests.iter().any(|q| q.id == *id));
    quests.iter().filter(|q| q.phase(now) == Phase::Claimable && notified.insert(q.id)).map(|q| q.id).collect()
}

// ----- video progress (QUESTS_NOTES §3) -----------------------------------------------------------

/// UD: video progress is sent every 10-15 s of playback.
const SEND_EVERY: u32 = 10;
/// A gap between ticks (suspended, hung) counts at most this.
const MAX_TICK_DT: Duration = Duration::from_secs(2);

/// Progress of one quest video session. Reports only what the player really played (its own
/// `played_ms` counter), never more than the real time between ticks in which that counter grew,
/// since the session started from the server's value. One request in flight; stops for good
/// after a failed one.
#[derive(Debug)]
pub struct VideoProgress {
    /// Server value when the session opened (seconds).
    start: f64,
    /// Real seconds in which the player's counter grew.
    played: f64,
    last_tick: Option<Instant>,
    /// The player's `played_ms` at the last tick (a new player starts at 0).
    player_ms: u64,
    last_pos: f64,
    /// Highest timestamp sent (starts at the server value).
    last_sent: u32,
    /// Highest timestamp the server confirmed (starts at the server value).
    acked: u32,
    target: u32,
    in_flight: bool,
    stopped: bool,
}

impl VideoProgress {
    pub fn new(server_value: u32, target: u32) -> Self {
        let start = server_value.min(target);
        Self { start: f64::from(start), played: 0.0, last_tick: None, player_ms: 0, last_pos: 0.0, last_sent: start, acked: start, target, in_flight: false, stopped: false }
    }

    /// One tick: `pos` = position of the picture (s), `played_ms` = the player's own counter.
    /// The wall time since the last tick counts only if that counter grew. Returns a timestamp
    /// to send.
    pub fn tick(&mut self, pos: f64, played_ms: u64, now: Instant) -> Option<u32> {
        let dt = self.last_tick.map_or(Duration::ZERO, |t| now.saturating_duration_since(t)).min(MAX_TICK_DT);
        self.last_tick = Some(now);
        if played_ms > self.player_ms {
            self.played += dt.as_secs_f64();
        }
        self.player_ms = played_ms;
        self.last_pos = pos;
        let ts = self.allowed(pos);
        let due = ts >= self.last_sent + SEND_EVERY || (ts == self.target && ts > self.last_sent);
        self.take(due, ts)
    }

    /// Modal closing: the last whole seconds, if any are unsent.
    pub fn close(&mut self) -> Option<u32> {
        let ts = self.allowed(self.last_pos);
        let out = self.take(ts > self.last_sent, ts);
        self.stopped = true;
        out
    }

    /// The answer to the last send: `false` stops this session (no retry).
    pub fn answered(&mut self, ok: bool) {
        self.in_flight = false;
        self.stopped |= !ok;
        if ok {
            self.acked = self.last_sent;
        }
    }

    /// Seconds the server confirmed holding (a send still on its way doesn't count).
    pub fn saved(&self) -> u32 {
        self.acked
    }

    pub fn target(&self) -> u32 {
        self.target
    }

    /// A send is on its way.
    pub fn in_flight(&self) -> bool {
        self.in_flight
    }

    /// A send failed: this session sends no more.
    pub fn stopped(&self) -> bool {
        self.stopped
    }

    /// The frame position, capped by real play time and the target.
    fn allowed(&self, pos: f64) -> u32 {
        (pos.min(self.start + self.played).max(0.0) as u32).min(self.target)
    }

    fn take(&mut self, due: bool, ts: u32) -> Option<u32> {
        (due && !self.in_flight && !self.stopped).then(|| {
            self.in_flight = true;
            self.last_sent = ts;
            ts
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        parse_time(s).unwrap()
    }

    fn quest(json: &str) -> Quest {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn demo_json_parses() {
        let page: QuestsPage = serde_json::from_str(crate::ui::quests::DEMO_JSON).unwrap();
        assert_eq!(page.quests.len(), 6);
        let q = &page.quests[0];
        assert_eq!((q.id, q.config.messages.quest_name.as_str()), (Id(101), "Finish three sketches"));
        assert_eq!(q.config.rewards_config.rewards[0].orb_quantity, Some(700));
        // Unknown fields, nulls, a bad quest (skipped) and one without task_config_v2.
        let odd = r#"{"quests":[{"id":"1","config":{"features":null,"new_thing":{"x":1},"messages":{"quest_name":null}},"user_status":null,"preview":null},
            {"id":"2","config":"not an object"}],"excluded_quests":null}"#;
        let page: QuestsPage = serde_json::from_str(odd).unwrap();
        assert_eq!(page.quests.len(), 1);
        assert!(page.quests[0].task().is_none());
        assert_eq!(page.quests[0].progress(), (0, 0));
        let now = at("2026-11-14T00:00:00+00:00");
        let hidden = |extra: &str| quest(&format!(r#"{{"id":"3","config":{{"expires_at":"2026-12-01T00:00:00+00:00"{extra}}}}}"#)).phase(now);
        assert_eq!(hidden(""), Phase::Available);
        assert_eq!(hidden(r#","features":[21]"#), Phase::Hidden);
        let preview = quest(r#"{"id":"3","preview":true,"config":{"expires_at":"2026-12-01T00:00:00+00:00"}}"#);
        assert_eq!(preview.phase(now), Phase::Hidden);
    }

    fn with_status(status: &str, rewards_expire: &str) -> Quest {
        quest(&format!(
            r#"{{"id":"7","config":{{"expires_at":"2026-11-20T00:00:00+00:00","rewards_config":{{"rewards_expire_at":{rewards_expire}}}}},"user_status":{{"quest_id":"7",{status}}}}}"#
        ))
    }

    #[test]
    fn phases_and_sections() {
        let now = at("2026-11-14T00:00:00+00:00");
        let later = at("2026-11-25T00:00:00+00:00");
        let e = r#""enrolled_at":"2026-11-10T00:00:00+00:00""#;
        let c = r#""completed_at":"2026-11-11T00:00:00+00:00""#;
        let cl = r#""claimed_at":"2026-11-12T00:00:00+00:00""#;
        assert_eq!(with_status("\"progress\":{}", "null").phase(now), Phase::Available);
        assert_eq!(with_status(e, "null").phase(now), Phase::InProgress);
        assert_eq!(with_status(&format!("{e},{c}"), "null").phase(now), Phase::Claimable);
        assert_eq!(with_status(&format!("{e},{c},{cl}"), "null").phase(now), Phase::Claimed);
        // Past the quest's end: hidden, unless the reward still waits.
        assert_eq!(with_status(e, "null").phase(later), Phase::Hidden);
        assert_eq!(with_status(&format!("{e},{c},{cl}"), "null").phase(later), Phase::Hidden);
        assert_eq!(with_status(&format!("{e},{c}"), "\"2026-12-28T00:00:00+00:00\"").phase(later), Phase::Claimable);
        assert_eq!(with_status(&format!("{e},{c}"), "\"2026-11-24T00:00:00+00:00\"").phase(later), Phase::Hidden);
        assert_eq!(with_status(&format!("{e},{c}"), "null").phase(later), Phase::Hidden, "no reward deadline: the quest's end");

        let mut s = QuestState::default();
        s.apply(QuestEvent::Loaded(Ok(serde_json::from_str(crate::ui::quests::DEMO_JSON).unwrap())));
        let sections = s.sections(now);
        let order: Vec<Phase> = sections.iter().map(|(p, _)| *p).collect();
        assert_eq!(order, Phase::SECTIONS);
        let ids: Vec<Vec<u64>> = sections.iter().map(|(_, qs)| qs.iter().map(|q| q.id.0).collect()).collect();
        assert_eq!(ids, [vec![104], vec![101, 102], vec![103, 105], vec![106]]);
        // Excluded quests are dropped.
        let page = r#"{"quests":[{"id":"1"},{"id":"2"}],"excluded_quests":[{"id":"2","replacement_id":"9"}],"quest_enrollment_blocked_until":"2026-11-20T00:00:00+00:00"}"#;
        s.apply(QuestEvent::Loaded(Ok(serde_json::from_str(page).unwrap())));
        assert_eq!((s.quests.len(), s.blocked_until.as_deref()), (1, Some("2026-11-20T00:00:00+00:00")));
    }

    #[test]
    fn task_choice_and_progress() {
        let q = quest(
            r#"{"id":"1","config":{"task_config_v2":{"join_operator":"or","tasks":{
                "PLAY_ON_DESKTOP":{"target":900},"WATCH_VIDEO":{"target":180}}}},
              "user_status":{"quest_id":"1","progress":{"WATCH_VIDEO":{"value":36},"PLAY_ON_DESKTOP":{"value":540}}}}"#,
        );
        assert_eq!(q.task().map(|(k, t)| (k, t.target)), Some((TaskKind::Video, 180)));
        assert_eq!(q.progress(), (540, 900), "or: the furthest task");
        let and = quest(&serde_json::to_string(&serde_json::json!({"id":"1","config":{"task_config_v2":{"join_operator":"and","tasks":{
            "PLAY_ON_DESKTOP":{"target":900},"WATCH_VIDEO":{"target":180}}}},
            "user_status":{"quest_id":"1","progress":{"WATCH_VIDEO":{"value":36},"PLAY_ON_DESKTOP":{"value":540}}}})).unwrap());
        assert_eq!(and.progress(), (36, 180), "and: the least done task");
        let zero = quest(r#"{"id":"1","config":{"task_config_v2":{"tasks":{"SOMETHING_NEW":{"target":0}}}}}"#);
        assert_eq!(zero.task().map(|(k, _)| k), Some(TaskKind::Other));
        assert_eq!(zero.progress(), (0, 0), "target 0 does not divide by zero");
        let over = quest(r#"{"id":"1","config":{"task_config_v2":{"tasks":{"WATCH_VIDEO":{"target":60}}}},"user_status":{"progress":{"WATCH_VIDEO":{"value":75.5}}}}"#);
        assert_eq!(over.progress(), (60, 60), "capped at the target");
    }

    #[test]
    fn claim_platform() {
        let q = |p: &str| quest(&format!(r#"{{"id":"1","config":{{"rewards_config":{{"platforms":{p}}}}}}}"#));
        assert_eq!(q("[]").claim_platform(), Some(0));
        assert_eq!(q("[0]").claim_platform(), Some(0));
        assert_eq!(q("[4]").claim_platform(), Some(4));
        assert_eq!(q("[1,2,4]").claim_platform(), None);
    }

    #[test]
    fn video_url_rules() {
        let q = |features: &str, assets: &str| {
            quest(&format!(r#"{{"id":"101","config":{{"features":{features},"task_config_v2":{{"tasks":{{"WATCH_VIDEO":{{"target":180,"assets":{assets}}}}}}}}}}}"#))
        };
        let mp4 = r#"{"video":{"url":"https://cdn.discordapp.com/quests/101/trailer.mp4"}}"#;
        assert_eq!(q("[]", mp4).video_url().as_deref(), Some("https://cdn.discordapp.com/quests/101/trailer.mp4"));
        assert_eq!(q("[18]", mp4).video_url(), None, "forced HLS");
        assert_eq!(q("[]", r#"{"video_hls":{"url":"https://cdn.discordapp.com/x.m3u8"}}"#).video_url(), None);
        let low = r#"{"video_low_res":{"url":"low.mp4"},"video":{"url":"full.mp4"}}"#;
        assert_eq!(q("[]", low).video_url().as_deref(), Some("https://cdn.discordapp.com/quests/101/low.mp4"));
        assert_eq!(q("[]", r#"{"video":{"url":"https://evil.example/x.mp4"}}"#).video_url(), None);
    }

    #[test]
    fn apply_events() {
        let mut s = QuestState::default();
        s.apply(QuestEvent::Loaded(Ok(serde_json::from_str(crate::ui::quests::DEMO_JSON).unwrap())));
        assert!(matches!(s.load, Load::Loaded(_)));
        // A failed refetch keeps the list (and refetches later); only a first fetch fails the page.
        s.apply(QuestEvent::Loaded(Err("x".into())));
        assert!(matches!(s.load, Load::Loaded(_)) && s.quests.len() == 6);
        let mut first = QuestState::default();
        first.apply(QuestEvent::Loaded(Err("x".into())));
        assert_eq!(first.load, Load::Failed("x".into()));
        s.busy.insert(Id(103));
        let status = QuestUserStatus { quest_id: Id(103), enrolled_at: Some("2026-11-14T10:00:00+00:00".into()), ..Default::default() };
        assert_eq!(s.apply(QuestEvent::Status(Box::new(status))), None);
        assert!(s.quest(Id(103)).unwrap().enrolled() && !s.busy.contains(&Id(103)));
        // Unknown quest: ignored.
        let before = s.quests.len();
        s.apply(QuestEvent::Status(Box::new(QuestUserStatus { quest_id: Id(999), ..Default::default() })));
        assert_eq!(s.quests.len(), before);
        // Video answers are reported to the watch session.
        let saved = QuestUserStatus { quest_id: Id(101), ..Default::default() };
        assert_eq!(s.apply(QuestEvent::VideoSaved(Box::new(saved))), Some((Id(101), true)));
        let failed = QuestEvent::Failed { quest_id: Id(101), message: VIDEO_FAILED.into(), stop_video: true };
        assert_eq!(s.apply(failed), Some((Id(101), false)));
        assert_eq!(s.notices.get(&Id(101)).map(String::as_str), Some(VIDEO_FAILED));
        // Codes are kept, never shown by Debug.
        s.apply(QuestEvent::Code { quest_id: Id(106), code: Ok(Arc::new(Secret::new("RC-SECRET-CODE".into()))) });
        assert_eq!(s.codes.get(&Id(106)).map(|c| c.expose()), Some("RC-SECRET-CODE"));
        assert!(!format!("{s:?}").contains("RC-SECRET-CODE"));
        // Claim: the answer's time, or its error as the card notice.
        s.apply(QuestEvent::Claimed { quest_id: Id(104), claimed_at: Some("2026-11-14T12:00:00+00:00".into()), error: None });
        assert!(s.quest(Id(104)).unwrap().status().unwrap().claimed_at.is_some());
        s.apply(QuestEvent::Claimed { quest_id: Id(105), claimed_at: None, error: None });
        assert_eq!(s.notices[&Id(105)], "Discord didn't give the reward. Try again later.");
        s.apply(QuestEvent::Orbs(1240));
        s.apply(QuestEvent::Blocked(Some("2026-11-20T00:00:00+00:00".into())));
        assert_eq!((s.orbs, s.blocked_until.is_some()), (Some(1240), true));
    }

    #[test]
    fn notify_once() {
        let now = at("2026-11-14T00:00:00+00:00");
        let mut s = QuestState::default();
        assert!(!s.changed);
        s.apply(QuestEvent::Loaded(Ok(serde_json::from_str(crate::ui::quests::DEMO_JSON).unwrap())));
        assert!(s.changed, "events mark the list as changed");
        let mut set = BTreeSet::new();
        assert_eq!(newly_claimable(&s.quests, &mut set, now), [Id(104)]);
        assert!(newly_claimable(&s.quests, &mut set, now).is_empty(), "told once");
        // 101 completes: told once more.
        let done = QuestUserStatus {
            quest_id: Id(101),
            enrolled_at: Some("2026-11-10T10:00:00+00:00".into()),
            completed_at: Some("2026-11-14T00:00:00+00:00".into()),
            ..Default::default()
        };
        s.apply(QuestEvent::VideoSaved(Box::new(done)));
        assert_eq!(newly_claimable(&s.quests, &mut set, now), [Id(101)]);
        assert!(newly_claimable(&s.quests, &mut set, now).is_empty());
        // A claimed quest (106) never shows up; a quest that left the list is pruned.
        assert!(!set.contains(&Id(106)));
        set.insert(Id(999));
        newly_claimable(&s.quests, &mut set, now);
        assert!(!set.contains(&Id(999)));
    }

    #[test]
    fn fetch_gate() {
        let t0 = Instant::now();
        let mut s = QuestState::default();
        assert!(s.should_fetch(t0));
        s.load = Load::Loading;
        assert!(!s.should_fetch(t0));
        s.load = Load::Failed("x".into());
        assert!(!s.should_fetch(t0), "a failed fetch waits for Retry");
        s.load = Load::Loaded(t0);
        assert!(!s.should_fetch(t0 + Duration::from_secs(599)));
        assert!(s.should_fetch(t0 + REFETCH));
    }

    /// Drives `tick` at 30 fps from `t` for `secs`: +33 ms wall and +33 `played_ms` per tick, the
    /// position advancing with play from `pos`; returns the sends, the end time and position.
    fn play(v: &mut VideoProgress, t: Instant, pos: f64, secs: f64, ok: bool) -> (Vec<u32>, Instant, f64) {
        let mut sent = Vec::new();
        let frames = (secs * 30.0).round() as u32;
        let (mut now, mut p) = (t, pos);
        for _ in 0..frames {
            now += Duration::from_millis(33);
            p += 0.033;
            if let Some(ts) = v.tick(p, v.player_ms + 33, now) {
                sent.push(ts);
                v.answered(ok);
            }
        }
        (sent, now, p)
    }

    #[test]
    fn video_progress_reports_only_real_play() {
        let t0 = Instant::now();
        let mut v = VideoProgress::new(0, 180);
        v.tick(0.0, 0, t0);
        let (sent, t, pos) = play(&mut v, t0, 0.0, 25.0, true);
        assert_eq!(sent, [10, 20]);
        // A forward seek to 170 s credits nothing beyond the real play time.
        let pm = v.player_ms;
        assert_eq!(v.tick(170.0, pm + 33, t + Duration::from_millis(33)), None);
        assert!(v.allowed(170.0) <= 26);
        // A 60 s gap with the counter +60 s (suspend, hang) credits at most 2 s.
        let before = v.played;
        v.tick(pos, pm + 60_033, t + Duration::from_secs(60));
        assert!(v.played - before <= 2.0 + 1e-9);
        // Ticks with an unchanged counter (paused) credit nothing.
        let (before, pm) = (v.played, v.player_ms);
        for i in 1..=30 {
            v.tick(pos, pm, t + Duration::from_secs(60) + Duration::from_millis(33 * i));
        }
        assert_eq!(v.played, before);

        // Resumed from the server's 150 s: sends 160, 170, then the target, never above.
        let mut v = VideoProgress::new(150, 180);
        v.tick(150.0, 0, t0);
        let (sent, ..) = play(&mut v, t0, 150.0, 40.0, true);
        assert_eq!(sent, [160, 170, 180]);
        assert!(sent.windows(2).all(|w| w[1] > w[0]));

        // One send in flight: nothing more until it is answered.
        let mut v = VideoProgress::new(0, 180);
        v.tick(0.0, 0, t0);
        let mut sent = Vec::new();
        let (mut now, mut p) = (t0, 0.0);
        for _ in 0..(30 * 30) {
            now += Duration::from_millis(33);
            p += 0.033;
            sent.extend(v.tick(p, v.player_ms + 33, now));
        }
        assert_eq!(sent, [10], "no answer yet");
        assert!(v.in_flight());
        assert_eq!(v.saved(), 0, "only an answer counts as saved");
        v.answered(true);
        assert_eq!(v.saved(), 10);
        assert_eq!(v.tick(p, v.player_ms + 33, now + Duration::from_millis(33)), Some(29));

        // A failed send stops the session for good; close sends nothing then.
        let mut v = VideoProgress::new(0, 180);
        v.tick(0.0, 0, t0);
        let (sent, ..) = play(&mut v, t0, 0.0, 25.0, false);
        assert_eq!(sent, [10]);
        assert!(v.stopped() && v.close().is_none());
        assert_eq!(v.saved(), 0, "a failed send is not saved");

        // Close sends the last whole seconds once.
        let mut v = VideoProgress::new(0, 180);
        v.tick(0.0, 0, t0);
        let (sent, ..) = play(&mut v, t0, 0.0, 14.0, true);
        assert_eq!(sent, [10]);
        assert_eq!(v.close(), Some(13));
        assert_eq!(v.close(), None);
    }

    /// The same session sends the same timestamps whether it is ticked at 30 fps (modal drawn)
    /// or at 1 Hz (background), including the last partial second at the end.
    #[test]
    fn same_sends_at_30_fps_and_1_hz() {
        let t0 = Instant::now();
        let mut fast = VideoProgress::new(0, 30);
        let mut sent = Vec::new();
        fast.tick(0.0, 0, t0);
        let (mut now, mut p) = (t0, 0.0);
        while p < 29.96 {
            now += Duration::from_millis(33);
            p += 0.033;
            sent.extend(fast.tick(p, fast.player_ms + 33, now));
            fast.answered(true);
        }
        // Ended at the next tick: the position is the end, the counter caught up.
        sent.extend(fast.tick(30.0, 30_000, now + Duration::from_millis(100)));
        assert_eq!(sent, [10, 20, 30]);

        let mut slow = VideoProgress::new(0, 30);
        let mut sent = Vec::new();
        for t in 0..=30u64 {
            sent.extend(slow.tick(t as f64, t * 1000, t0 + Duration::from_secs(t)));
            slow.answered(true);
        }
        assert_eq!(sent, [10, 20, 30]);
    }
}
