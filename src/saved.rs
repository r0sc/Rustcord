//! Saved messages and reminders (Inbox > Saved). Local only: ids in `Settings::saved`, never the
//! text (it loads one message at a time while its row is on screen) and never Discord's own
//! bookmark or reminder endpoints.

use crate::alerts::at;
use crate::model::Id;
use chrono::{DateTime, Local, TimeDelta, Timelike};
use serde::{Deserialize, Serialize};

pub const MAX_SAVED: usize = 500;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    /// My user id when it was saved: another account on this device never sees it.
    pub owner: Id,
    pub channel_id: Id,
    pub message_id: Id,
    /// Unix seconds; None = no reminder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remind_at: Option<i64>,
}

pub fn position(list: &[Saved], owner: Id, mid: Id) -> Option<usize> {
    list.iter().position(|s| s.owner == owner && s.message_id == mid)
}

/// Saves `mid`, or removes it when it is saved. Ok(true) = now saved (newest first); Err = the cap.
pub fn toggle(list: &mut Vec<Saved>, owner: Id, ch: Id, mid: Id) -> Result<bool, &'static str> {
    if let Some(i) = position(list, owner, mid) {
        list.remove(i);
        return Ok(false);
    }
    insert(list, owner, ch, mid, None).map(|()| true)
}

fn insert(list: &mut Vec<Saved>, owner: Id, ch: Id, mid: Id, at: Option<i64>) -> Result<(), &'static str> {
    if list.iter().filter(|s| s.owner == owner).count() >= MAX_SAVED {
        return Err("You can keep up to 500 saved messages. Remove some in Inbox > Saved.");
    }
    list.insert(0, Saved { owner, channel_id: ch, message_id: mid, remind_at: at });
    Ok(())
}

/// Sets (Some) or removes (None) the reminder; an unsaved message is saved first.
pub fn remind(list: &mut Vec<Saved>, owner: Id, ch: Id, mid: Id, at: Option<i64>) -> Result<(), &'static str> {
    match position(list, owner, mid) {
        Some(i) => {
            list[i].remind_at = at;
            Ok(())
        }
        None if at.is_none() => Ok(()),
        None => insert(list, owner, ch, mid, at),
    }
}

/// My reminders due at `now` (Unix seconds); their reminder is cleared, the message stays.
pub fn take_due(list: &mut [Saved], owner: Id, now: i64) -> Vec<Saved> {
    let mut due: Vec<Saved> = Vec::new();
    for s in list.iter_mut().filter(|s| s.owner == owner && s.remind_at.is_some_and(|t| t <= now)) {
        due.push(s.clone());
        s.remind_at = None;
    }
    due.sort_by_key(|s| s.remind_at);
    due
}

/// The earliest pending reminder of mine.
pub fn next_due(list: &[Saved], owner: Id) -> Option<i64> {
    list.iter().filter(|s| s.owner == owner).filter_map(|s| s.remind_at).min()
}

/// "This evening": 20:00 today, offered only before 19:00.
pub fn evening(now: DateTime<Local>) -> Option<DateTime<Local>> {
    (now.hour() < 19).then(|| at(now.date_naive(), 20 * 60)).flatten()
}

/// `day` 0 = today, n = n days on, at `minute` of the day; None unless it is after `now`.
pub fn custom_at(now: DateTime<Local>, day: u8, minute: u16) -> Option<DateTime<Local>> {
    at(now.date_naive() + chrono::Days::new(day.into()), minute).filter(|t| *t > now)
}

/// The Custom modal's start: the next half hour at least 30 minutes ahead, today; after that tomorrow 08:00.
pub fn default_custom(now: DateTime<Local>) -> (u8, u16) {
    let t = now + TimeDelta::minutes(30);
    let minute = (t.hour() * 60 + t.minute()).div_ceil(30) * 30;
    if t.date_naive() == now.date_naive() && minute < 24 * 60 { (0, minute as u16) } else { (1, 8 * 60) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn s(owner: u64, mid: u64, at: Option<i64>) -> Saved {
        Saved { owner: Id(owner), channel_id: Id(9), message_id: Id(mid), remind_at: at }
    }

    fn local(h: u32, m: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 5, 4, h, m, 0).unwrap()
    }

    #[test]
    fn toggle_twice_and_the_cap() {
        let mut l = Vec::new();
        assert_eq!(toggle(&mut l, Id(1), Id(9), Id(5)), Ok(true));
        assert_eq!(toggle(&mut l, Id(1), Id(9), Id(6)), Ok(true));
        assert_eq!(l[0].message_id, Id(6), "newest first");
        assert_eq!(toggle(&mut l, Id(1), Id(9), Id(5)), Ok(false));
        assert_eq!(l.len(), 1);
        let mut full: Vec<Saved> = (0..MAX_SAVED as u64).map(|i| s(1, 100 + i, None)).collect();
        assert!(toggle(&mut full, Id(1), Id(9), Id(1)).is_err());
        assert_eq!(full.len(), MAX_SAVED);
        assert_eq!(toggle(&mut full, Id(2), Id(9), Id(1)), Ok(true), "the cap is per account");
    }

    #[test]
    fn remind_saves_and_keeps_one_entry() {
        let mut l = Vec::new();
        remind(&mut l, Id(1), Id(9), Id(5), Some(100)).unwrap();
        remind(&mut l, Id(1), Id(9), Id(5), Some(200)).unwrap();
        assert_eq!(l, [s(1, 5, Some(200))]);
        remind(&mut l, Id(1), Id(9), Id(5), None).unwrap();
        assert_eq!(l, [s(1, 5, None)]);
        remind(&mut l, Id(1), Id(9), Id(7), None).unwrap();
        assert_eq!(l.len(), 1, "removing a reminder never saves");
    }

    #[test]
    fn due_reminders() {
        let mut l = vec![s(1, 1, Some(50)), s(2, 2, Some(10)), s(1, 3, Some(500)), s(1, 4, None)];
        assert_eq!(next_due(&l, Id(1)), Some(50));
        assert_eq!(next_due(&l, Id(3)), None);
        let due = take_due(&mut l, Id(1), 100);
        assert_eq!(due.iter().map(|d| d.message_id).collect::<Vec<_>>(), [Id(1)]);
        assert_eq!(l[0].remind_at, None);
        assert_eq!(l[1].remind_at, Some(10), "another account's stays");
        assert_eq!(l[2].remind_at, Some(500));
        assert_eq!(next_due(&l, Id(1)), Some(500));
    }

    #[test]
    fn times() {
        assert_eq!(evening(local(18, 59)), Some(local(20, 0)));
        assert_eq!(evening(local(19, 0)), None);
        assert_eq!(custom_at(local(10, 0), 0, 9 * 60), None);
        assert_eq!(custom_at(local(10, 0), 1, 8 * 60), Some(local(8, 0) + TimeDelta::days(1)));
        assert_eq!(default_custom(local(23, 40)), (1, 480));
        assert_eq!(default_custom(local(10, 10)), (0, 11 * 60));
    }
}
