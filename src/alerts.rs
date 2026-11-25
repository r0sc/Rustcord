//! Keyword alerts and quiet hours (docs/FEATURES_NOTES.md §2.7). Local only; it reads gateway
//! events the client already gets and sends nothing.

use chrono::{DateTime, Datelike, Days, Local, NaiveDate, TimeDelta, TimeZone, Timelike};
use crate::model::{GatewayEvent, Id, Message};
use crate::state::{AppState, Notification};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

pub const MAX_KEYWORDS: usize = 50;
pub const MAX_KEYWORD_CHARS: usize = 100;
pub const MAX_PLACES: usize = 100;
pub const MAX_HITS: usize = 50;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keyword {
    /// Trimmed, lowercase.
    pub text: String,
    pub whole_word: bool,
}

impl Default for Keyword {
    fn default() -> Self {
        Self { text: String::new(), whole_word: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeywordAlerts {
    pub keywords: Vec<Keyword>,
    /// true: every server; false: only `places`.
    pub everywhere: bool,
    /// Server or channel ids (a channel covers its threads), used while !everywhere.
    pub places: Vec<Id>,
}

impl Default for KeywordAlerts {
    fn default() -> Self {
        Self { keywords: Vec::new(), everywhere: true, places: Vec::new() }
    }
}

/// A message that matched a keyword, kept for the inbox's Keywords tab.
#[derive(Debug, Clone)]
pub struct Hit {
    pub msg: Message,
    #[allow(dead_code, reason = "read by the Keywords tab")]
    pub keyword: String,
}

impl KeywordAlerts {
    /// Trims and lowercases, drops empties and duplicates (hand-edited files).
    pub fn normalize(&mut self) {
        let mut seen = std::collections::HashSet::new();
        for k in &mut self.keywords {
            k.text = k.text.trim().to_lowercase();
        }
        self.keywords.retain(|k| !k.text.is_empty() && seen.insert(k.text.clone()));
        self.keywords.truncate(MAX_KEYWORDS);
        self.places.truncate(MAX_PLACES);
    }

    /// Adds `text` (trimmed, lowercased, whole word). Err is the inline error copy.
    pub fn add(&mut self, text: &str) -> Result<(), &'static str> {
        let text: String = text.trim().to_lowercase().chars().take(MAX_KEYWORD_CHARS).collect();
        if text.is_empty() {
            return Err("Type a word or phrase first.");
        }
        if self.keywords.iter().any(|k| k.text == text) {
            return Err("That's already on your list.");
        }
        if self.keywords.len() >= MAX_KEYWORDS {
            return Err("You can add up to 50 keywords.");
        }
        self.keywords.push(Keyword { text, whole_word: true });
        Ok(())
    }

    /// In scope: everywhere, or one of the places is the server, the channel or the thread's parent.
    pub fn covers(&self, guild: Id, channel: Id, parent: Option<Id>) -> bool {
        self.everywhere || self.places.iter().any(|&p| p == guild || p == channel || Some(p) == parent)
    }

    /// The first keyword in `text`.
    pub fn find(&self, text: &str) -> Option<&Keyword> {
        if self.keywords.is_empty() || text.is_empty() {
            return None;
        }
        let hay = text.to_lowercase();
        self.keywords.iter().find(|k| !k.text.is_empty() && occurs(&hay, &k.text, k.whole_word))
    }
}

/// Every start position (not `match_indices`: an overlapping earlier hit must not hide a whole-word one).
fn occurs(hay: &str, needle: &str, whole: bool) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut from = 0;
    while let Some(i) = hay[from..].find(needle) {
        let (s, e) = (from + i, from + i + needle.len());
        if !whole || (!hay[..s].chars().next_back().is_some_and(word) && !hay[e..].chars().next().is_some_and(word)) {
            return true;
        }
        from = s + hay[s..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// The keyword `msg` matches for me: a server message (DMs never), not mine, not from a blocked
/// or ignored user, in scope.
pub fn matched<'a>(a: &'a KeywordAlerts, state: &AppState, msg: &Message) -> Option<&'a Keyword> {
    if a.keywords.is_empty() {
        return None;
    }
    let ch = state.channels.get(&msg.channel_id)?;
    let guild = ch.guild_id?;
    if state.me.as_ref().is_some_and(|u| u.id == msg.author.id) || state.hides_author(msg.author.id) || !a.covers(guild, ch.id, ch.parent_id) {
        return None;
    }
    a.find(&msg.content)
}

/// A gateway event before `AppState::apply_gateway` takes it.
/// - A new matching server message goes to the front of `hits` (deduped by id, capped at MAX_HITS).
/// - It is returned as (notification, message id) unless its channel is focused or it is @silent.
/// - Edits update a listed message's text; deletes remove it from the list.
pub fn on_gateway(hits: &mut VecDeque<Hit>, a: &KeywordAlerts, state: &AppState, ev: &GatewayEvent, focused: Option<Id>) -> Option<(Notification, Id)> {
    match ev {
        GatewayEvent::MessageCreate(m) => {
            let k = matched(a, state, m)?;
            remember(hits, Hit { msg: (**m).clone(), keyword: k.text.clone() });
            (focused != Some(m.channel_id) && m.flags & crate::state::FLAG_SUPPRESS_NOTIFICATIONS == 0).then(|| (notification(state, m, &k.text), m.id))
        }
        GatewayEvent::MessageUpdate(u) => {
            if let (Some(h), Some(c)) = (hits.iter_mut().find(|h| h.msg.id == u.id), &u.content) {
                h.msg.content = c.clone();
            }
            None
        }
        GatewayEvent::MessageDelete { id, .. } => {
            hits.retain(|h| h.msg.id != *id);
            None
        }
        GatewayEvent::MessageDeleteBulk { ids, .. } => {
            hits.retain(|h| !ids.contains(&h.msg.id));
            None
        }
        _ => None,
    }
}

pub fn remember(hits: &mut VecDeque<Hit>, hit: Hit) {
    hits.retain(|h| h.msg.id != hit.msg.id);
    hits.push_front(hit);
    hits.truncate(MAX_HITS);
}

/// Title `"release" in #general (Rust Workshop)`, body `Ann: we cut the release today`.
fn notification(state: &AppState, msg: &Message, keyword: &str) -> Notification {
    let guild_id = state.channels.get(&msg.channel_id).and_then(|c| c.guild_id);
    let author = state.member_display_name(guild_id, &msg.author, msg.member.as_ref());
    let guild = guild_id.and_then(|g| state.guild(g)).map_or("", |g| g.name.as_str());
    Notification {
        channel_id: msg.channel_id,
        title: format!("\"{keyword}\" in #{} ({guild})", state.channel_name(msg.channel_id)),
        body: format!("{author}: {}", crate::state::notification_body(msg)),
        author_id: Some(msg.author.id),
        mentioned: false,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuietHours {
    pub enabled: bool,
    /// Bit 0 Monday .. bit 6 Sunday: the days a quiet period starts on.
    pub days: u8,
    /// Local minutes after midnight (30-minute steps in the UI). start == end: never.
    pub start: u16,
    pub end: u16,
    /// Paused by hand (tray, account menu) until this Unix time (s).
    pub paused_until: Option<i64>,
}

impl Default for QuietHours {
    fn default() -> Self {
        Self { enabled: false, days: 0x7F, start: 22 * 60, end: 7 * 60, paused_until: None }
    }
}

impl QuietHours {
    /// Is a quiet period running at `minute` of a `weekday` (0 = Monday)? An overnight period
    /// belongs to the day it starts on.
    pub fn scheduled(&self, weekday: u32, minute: u16) -> bool {
        if !self.enabled || self.start == self.end {
            return false;
        }
        let on = |d: u32| self.days & (1 << (d % 7)) != 0;
        if self.start < self.end {
            on(weekday) && (self.start..self.end).contains(&minute)
        } else {
            (on(weekday) && minute >= self.start) || (on((weekday + 6) % 7) && minute < self.end)
        }
    }

    pub fn paused(&self, now: i64) -> bool {
        self.paused_until.is_some_and(|t| now < t)
    }

    pub fn quiet_at(&self, now: DateTime<Local>) -> bool {
        self.paused(now.timestamp()) || self.scheduled(now.weekday().num_days_from_monday(), (now.hour() * 60 + now.minute()) as u16)
    }

    /// Rustcord's notifications and alerting sounds are silent now (the one call other code makes).
    pub fn quiet_now(&self) -> bool {
        self.quiet_at(Local::now())
    }

    /// When the current quiet ends: the later of the pause and the scheduled period (both
    /// contain now, so their union is contiguous); None when not quiet.
    pub fn until(&self, now: DateTime<Local>) -> Option<DateTime<Local>> {
        let minute = (now.hour() * 60 + now.minute()) as u16;
        let paused = self.paused_until.filter(|&t| now.timestamp() < t).and_then(|t| Local.timestamp_opt(t, 0).single());
        let sched = self
            .scheduled(now.weekday().num_days_from_monday(), minute)
            .then(|| at(if minute >= self.end { now.date_naive() + Days::new(1) } else { now.date_naive() }, self.end))
            .flatten();
        paused.max(sched)
    }
}

fn at(day: NaiveDate, minute: u16) -> Option<DateTime<Local>> {
    Local.from_local_datetime(&day.and_hms_opt((minute / 60).into(), (minute % 60).into(), 0)?).earliest()
}

/// "Until tomorrow": 08:00 the next morning (before 05:00: today's 08:00).
pub fn tomorrow_morning(now: DateTime<Local>) -> DateTime<Local> {
    let day = if now.hour() < 5 { now.date_naive() } else { now.date_naive() + Days::new(1) };
    at(day, 8 * 60).unwrap_or(now + TimeDelta::hours(8))
}

/// "14:30", "08:00 tomorrow", later "Mon 07:00" (24-hour, as the chat's times).
pub fn time_label(t: DateTime<Local>, now: DateTime<Local>) -> String {
    let days = (t.date_naive() - now.date_naive()).num_days();
    match days {
        0 => t.format("%H:%M").to_string(),
        1 => t.format("%H:%M tomorrow").to_string(),
        _ => t.format("%a %H:%M").to_string(),
    }
}

/// The tray's and the account menu's pause: until `until`, or resumed (None). Returns the toast.
pub fn pause(q: &mut QuietHours, until: Option<DateTime<Local>>, now: DateTime<Local>) -> String {
    q.paused_until = until.map(|t| t.timestamp());
    match (until, q.until(now)) {
        (Some(t), _) => format!("Notifications paused until {}", time_label(t, now)),
        (None, Some(t)) => format!("Notifications resumed. Quiet hours last until {}.", time_label(t, now)),
        (None, None) => "Notifications resumed".into(),
    }
}

pub fn hhmm(minute: u16) -> String {
    format!("{:02}:{:02}", minute / 60, minute % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_same_day_and_overnight() {
        let day = QuietHours { enabled: true, days: 0b0000001, start: 9 * 60, end: 17 * 60, paused_until: None };
        assert!(day.scheduled(0, 9 * 60) && day.scheduled(0, 16 * 60 + 59));
        assert!(!day.scheduled(0, 17 * 60) && !day.scheduled(0, 8 * 60) && !day.scheduled(1, 12 * 60));
        let night = QuietHours { start: 22 * 60, end: 7 * 60, ..day.clone() };
        assert!(night.scheduled(0, 23 * 60) && night.scheduled(1, 6 * 60 + 59));
        assert!(!night.scheduled(1, 7 * 60) && !night.scheduled(0, 6 * 60) && !night.scheduled(6, 23 * 60));
        assert!(!QuietHours { start: 60, end: 60, ..night.clone() }.scheduled(0, 60));
        assert!(!QuietHours { enabled: false, ..night }.scheduled(0, 23 * 60));
    }

    #[test]
    fn pause_expires() {
        let q = QuietHours { paused_until: Some(100), ..Default::default() };
        assert!(q.paused(99) && !q.paused(100));
        assert_eq!(hhmm(7 * 60 + 30), "07:30");
    }

    fn alerts(words: &[(&str, bool)]) -> KeywordAlerts {
        KeywordAlerts { keywords: words.iter().map(|&(t, w)| Keyword { text: t.into(), whole_word: w }).collect(), ..Default::default() }
    }

    #[test]
    fn find_is_case_insensitive_and_whole_word() {
        let whole = alerts(&[("rust", true)]);
        assert!(whole.find("I love RUST!").is_some());
        assert!(whole.find("trust").is_none() && whole.find("rust_lang").is_none());
        assert!(alerts(&[("rust", false)]).find("trust").is_some());
        assert!(alerts(&[("c++", true)]).find("c++ is fun").is_some());
        assert!(alerts(&[("c++", true)]).find("c++11").is_none());
        assert!(alerts(&[("new release", true)]).find("The New  Release? no. a new release!").is_some());
        // The overlapping earlier hit ("ba a a" at 1) must not hide the whole-word one.
        assert!(alerts(&[("a a", true)]).find("ba a a").is_some());
        assert!(alerts(&[("rust", true)]).find("").is_none());
    }

    #[test]
    fn add_validates() {
        let mut a = KeywordAlerts::default();
        assert_eq!(a.add("   "), Err("Type a word or phrase first."));
        assert_eq!(a.add("  Rust "), Ok(()));
        assert_eq!(a.add("RUST"), Err("That's already on your list."));
        assert_eq!(a.keywords, [Keyword { text: "rust".into(), whole_word: true }]);
        for i in 1..MAX_KEYWORDS {
            a.add(&format!("w{i}")).unwrap();
        }
        assert_eq!(a.add("one more"), Err("You can add up to 50 keywords."));
        let mut h = alerts(&[("  Rust ", true), ("", true), ("rust", false), ("Go", true)]);
        h.normalize();
        assert_eq!(h.keywords.iter().map(|k| k.text.as_str()).collect::<Vec<_>>(), ["rust", "go"]);
    }

    const ME: Id = Id(1);
    const ANN: Id = Id(2);
    const G: Id = Id(100);
    const TEXT: Id = Id(201);
    const THREAD: Id = Id(202);
    const OTHER_TEXT: Id = Id(203);
    const DM: Id = Id(300);

    fn world() -> AppState {
        use crate::model::{Channel, Guild, User};
        let mut s = AppState::default();
        s.me = Some(User { id: ME, username: "me".into(), ..Default::default() });
        s.guilds.push(Guild { id: G, name: "Rust Workshop".into(), ..Default::default() });
        let ch = |id, kind, guild_id, parent_id, name: &str| Channel { id, kind, guild_id, parent_id, name: Some(name.into()), ..Default::default() };
        for c in [ch(TEXT, 0, Some(G), None, "general"), ch(THREAD, 11, Some(G), Some(TEXT), "t"), ch(OTHER_TEXT, 0, Some(G), None, "other"), ch(DM, 1, None, None, "dm")] {
            s.channels.insert(c.id, c);
        }
        s
    }

    fn say(id: u64, channel: Id, author: Id, text: &str) -> Message {
        let user = crate::model::User { id: author, username: if author == ME { "me" } else { "Ann" }.into(), ..Default::default() };
        Message { id: Id(id), channel_id: channel, author: user, content: text.into(), ..Default::default() }
    }

    #[test]
    fn covers_servers_channels_and_threads() {
        let mut a = KeywordAlerts::default();
        assert!(a.covers(G, TEXT, None), "everywhere");
        a.everywhere = false;
        assert!(!a.covers(G, TEXT, None));
        a.places = vec![G];
        assert!(a.covers(G, TEXT, None) && !a.covers(Id(101), Id(9), None));
        a.places = vec![TEXT];
        assert!(a.covers(G, TEXT, None) && a.covers(G, THREAD, Some(TEXT)) && !a.covers(G, OTHER_TEXT, None));
    }

    #[test]
    fn matched_skips_me_dms_and_out_of_scope() {
        let s = world();
        let mut a = alerts(&[("release", true)]);
        assert!(matched(&a, &s, &say(1, TEXT, ANN, "the release")).is_some());
        assert!(matched(&a, &s, &say(2, THREAD, ANN, "the release")).is_some());
        assert!(matched(&a, &s, &say(3, TEXT, ME, "the release")).is_none(), "mine");
        assert!(matched(&a, &s, &say(4, DM, ANN, "the release")).is_none(), "DM");
        assert!(matched(&a, &s, &say(5, Id(999), ANN, "the release")).is_none(), "unknown channel");
        assert!(matched(&a, &s, &say(6, TEXT, ANN, "nothing")).is_none());
        a.everywhere = false;
        a.places = vec![TEXT];
        assert!(matched(&a, &s, &say(7, THREAD, ANN, "the release")).is_some());
        assert!(matched(&a, &s, &say(8, OTHER_TEXT, ANN, "the release")).is_none(), "out of scope");
    }

    #[test]
    fn on_gateway_lists_notifies_and_forgets() {
        let s = world();
        let a = alerts(&[("release", true)]);
        let mut hits = VecDeque::new();
        let create = |m: Message| GatewayEvent::MessageCreate(Box::new(m));
        let (n, id) = on_gateway(&mut hits, &a, &s, &create(say(10, TEXT, ANN, "we cut the release today")), None).unwrap();
        assert_eq!((n.title.as_str(), n.body.as_str(), id), ("\"release\" in #general (Rust Workshop)", "Ann: we cut the release today", Id(10)));
        assert!(!n.mentioned && n.author_id == Some(ANN) && n.channel_id == TEXT);
        // Focused or @silent: listed, no notification.
        assert!(on_gateway(&mut hits, &a, &s, &create(say(11, TEXT, ANN, "release")), Some(TEXT)).is_none());
        let silent = Message { flags: crate::state::FLAG_SUPPRESS_NOTIFICATIONS, ..say(12, TEXT, ANN, "release") };
        assert!(on_gateway(&mut hits, &a, &s, &create(silent), None).is_none());
        assert_eq!(hits.iter().map(|h| h.msg.id.0).collect::<Vec<_>>(), [12, 11, 10]);
        // An edit changes the text and never notifies.
        let edit = GatewayEvent::MessageUpdate(Box::new(crate::model::MessageUpdate { id: Id(11), channel_id: TEXT, content: Some("edited".into()), ..Default::default() }));
        assert!(on_gateway(&mut hits, &a, &s, &edit, None).is_none());
        assert_eq!(hits[1].msg.content, "edited");
        on_gateway(&mut hits, &a, &s, &GatewayEvent::MessageDelete { id: Id(12), channel_id: TEXT }, None);
        on_gateway(&mut hits, &a, &s, &GatewayEvent::MessageDeleteBulk { ids: vec![Id(10), Id(99)], channel_id: TEXT }, None);
        assert_eq!(hits.iter().map(|h| h.msg.id.0).collect::<Vec<_>>(), [11]);
        // Capped at 50, newest first, deduped by id.
        for i in 100..160 {
            on_gateway(&mut hits, &a, &s, &create(say(i, TEXT, ANN, "release")), None);
        }
        on_gateway(&mut hits, &a, &s, &create(say(150, TEXT, ANN, "release")), None);
        assert_eq!((hits.len(), hits[0].msg.id), (MAX_HITS, Id(150)));
        assert_eq!(hits.iter().filter(|h| h.msg.id == Id(150)).count(), 1);
    }

    /// A January date (no DST change).
    fn jan(day: u32, h: u32, m: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2027, 1, day, h, m, 0).single().unwrap()
    }

    #[test]
    fn until_and_pause() {
        // 2027-01-04 is a Monday.
        let night = QuietHours { enabled: true, days: 0b0000001, start: 22 * 60, end: 7 * 60, paused_until: None };
        assert_eq!(night.until(jan(4, 23, 0)), Some(jan(5, 7, 0)));
        assert_eq!(night.until(jan(5, 6, 0)), Some(jan(5, 7, 0)));
        assert_eq!(night.until(jan(5, 12, 0)), None);
        // A pause later than the window wins; an earlier one does not shorten it.
        let long = QuietHours { paused_until: Some(jan(5, 9, 0).timestamp()), ..night.clone() };
        assert_eq!(long.until(jan(4, 23, 0)), Some(jan(5, 9, 0)));
        let short = QuietHours { paused_until: Some(jan(5, 1, 0).timestamp()), ..night.clone() };
        assert_eq!(short.until(jan(4, 23, 0)), Some(jan(5, 7, 0)));
        let mut q = QuietHours::default();
        assert_eq!(pause(&mut q, Some(jan(4, 15, 30)), jan(4, 14, 30)), "Notifications paused until 15:30");
        assert!(q.quiet_at(jan(4, 15, 0)) && !q.quiet_at(jan(4, 15, 30)));
        assert_eq!(pause(&mut q, None, jan(4, 15, 0)), "Notifications resumed");
        let mut n = QuietHours { paused_until: Some(1), ..night };
        assert_eq!(pause(&mut n, None, jan(4, 23, 0)), "Notifications resumed. Quiet hours last until 07:00 tomorrow.");
    }

    #[test]
    fn tomorrow_morning_and_labels() {
        assert_eq!(tomorrow_morning(jan(4, 23, 0)), jan(5, 8, 0));
        assert_eq!(tomorrow_morning(jan(5, 2, 0)), jan(5, 8, 0));
        assert_eq!(tomorrow_morning(jan(5, 5, 0)), jan(6, 8, 0));
        let now = jan(4, 12, 0);
        assert_eq!(time_label(jan(4, 14, 30), now), "14:30");
        assert_eq!(time_label(jan(5, 8, 0), now), "08:00 tomorrow");
        assert_eq!(time_label(jan(6, 7, 0), now), "Wed 07:00");
    }
}
