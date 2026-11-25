//! Keyword alerts and quiet hours (docs/FEATURES_NOTES.md §2.7). Local only; it reads gateway
//! events the client already gets and sends nothing.

use chrono::{DateTime, Datelike, Days, Local, NaiveDate, TimeDelta, TimeZone, Timelike};
use serde::{Deserialize, Serialize};

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
