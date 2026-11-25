//! Keyword alerts and quiet hours (docs/FEATURES_NOTES.md §2.7). Local only; it reads gateway
//! events the client already gets and sends nothing.

use chrono::{DateTime, Datelike, Local, Timelike};
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
}
