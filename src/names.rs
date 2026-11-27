//! Custom names (Edit name in a person's menu): a name, a tag and a colour that only this device
//! shows, for anyone. Kept in `Settings::custom_names` (all accounts on this device, like the
//! favourite GIFs) and published to a global so renderers that get no `Settings` can read it, as
//! `config::role_colors` is. Never sent: mentions and autocomplete use ids and the real name.
//! A friend's name is Discord's own friend nickname instead (`SocialCommand::SetFriendNickname`).

use crate::model::Id;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

pub const MAX_NAMES: usize = 500;
pub const NAME_MAX: usize = 32;
pub const TAG_MAX: usize = 16;

/// The colours offered by Edit name (0xRRGGBB, with their words for tooltips).
pub const PALETTE: [(u32, &str); 8] = [
    (0xE5484D, "Red"),
    (0xF2994A, "Orange"),
    (0xE8C547, "Yellow"),
    (0x46B97A, "Green"),
    (0x3BB9C4, "Teal"),
    (0x4A90E2, "Blue"),
    (0x9B6CF0, "Violet"),
    (0xE660A8, "Pink"),
];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomName {
    pub id: Id,
    pub name: Option<String>,
    pub tag: Option<String>,
    pub color: Option<u32>,
}

static GLOBAL: LazyLock<RwLock<HashMap<Id, CustomName>>> = LazyLock::new(Default::default);

/// Hands the list to the readers below. Call after loading and after each change.
pub fn publish(list: &[CustomName]) {
    *GLOBAL.write().unwrap_or_else(|e| e.into_inner()) = list.iter().map(|n| (n.id, n.clone())).collect();
}

fn read<R>(id: Id, f: impl FnOnce(&CustomName) -> Option<R>) -> Option<R> {
    GLOBAL.read().unwrap_or_else(|e| e.into_inner()).get(&id).and_then(f)
}

pub fn name(id: Id) -> Option<String> {
    read(id, |n| n.name.clone())
}

pub fn tag(id: Id) -> Option<String> {
    read(id, |n| n.tag.clone())
}

pub fn color(id: Id) -> Option<u32> {
    read(id, |n| n.color)
}

/// The name to show: my local name, else (in a DM) the friend nickname, else the server nickname,
/// else the display name (global name or username).
pub fn shown(local: Option<&str>, friend: Option<&str>, in_dm: bool, nick: Option<&str>, display: &str) -> String {
    fn pick(s: Option<&str>) -> Option<&str> {
        s.map(str::trim).filter(|s| !s.is_empty())
    }
    pick(local).or(if in_dm { pick(friend) } else { None }).or(pick(nick)).unwrap_or(display).to_owned()
}

/// Stores `entry` (trimmed and cut to the limits); an entry with nothing in it removes the old one.
/// Err is the cap, for a person who has none yet.
pub fn set(list: &mut Vec<CustomName>, mut entry: CustomName) -> Result<(), &'static str> {
    let clean = |s: Option<String>, max: usize| s.map(|s| s.trim().chars().take(max).collect::<String>()).filter(|s| !s.is_empty());
    entry.name = clean(entry.name, NAME_MAX);
    entry.tag = clean(entry.tag, TAG_MAX);
    let at = list.iter().position(|n| n.id == entry.id);
    match (at, entry.name.is_none() && entry.tag.is_none() && entry.color.is_none()) {
        (Some(i), true) => _ = list.remove(i),
        (None, true) => {}
        (Some(i), false) => list[i] = entry,
        (None, false) if list.len() >= MAX_NAMES => return Err("You can give names to up to 500 people. Reset some first."),
        (None, false) => list.push(entry),
    }
    Ok(())
}

/// Runs `f` with exactly `list` published; tests run in parallel and share the global, so they take
/// a lock and use ids no other test uses.
#[cfg(test)]
pub(crate) fn with_names<R>(list: &[CustomName], f: impl FnOnce() -> R) -> R {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    publish(list);
    let out = f();
    publish(&[]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u64, name: &str) -> CustomName {
        CustomName { id: Id(id), name: Some(name.into()), ..Default::default() }
    }

    #[test]
    fn precedence() {
        // Local wins; the friend nickname only in DMs; then server nick, then the display name.
        assert_eq!(shown(Some("Ali"), Some("Al"), true, Some("Nick"), "alice"), "Ali");
        assert_eq!(shown(None, Some("Al"), true, Some("Nick"), "alice"), "Al");
        assert_eq!(shown(None, Some("Al"), false, Some("Nick"), "alice"), "Nick");
        assert_eq!(shown(None, None, false, None, "alice"), "alice");
        // Empty strings are skipped.
        assert_eq!(shown(Some(" "), Some(""), true, Some(""), "alice"), "alice");
    }

    #[test]
    fn set_trims_drops_and_caps() {
        let mut l = Vec::new();
        set(&mut l, entry(9_990_001, "  Ali  ")).unwrap();
        assert_eq!(l[0].name.as_deref(), Some("Ali"));
        set(&mut l, CustomName { id: Id(9_990_001), name: Some("x".repeat(50)), tag: Some("  ".into()), color: Some(1) }).unwrap();
        assert_eq!((l[0].name.as_deref().map(str::len), l[0].tag.clone(), l.len()), (Some(NAME_MAX), None, 1));
        // Nothing left in it: the entry goes.
        set(&mut l, CustomName { id: Id(9_990_001), name: Some(" ".into()), ..Default::default() }).unwrap();
        assert!(l.is_empty());
        let mut full: Vec<CustomName> = (0..MAX_NAMES as u64).map(|i| entry(i + 1, "n")).collect();
        assert!(set(&mut full, entry(9_990_002, "new")).is_err());
        assert!(set(&mut full, entry(1, "changed")).is_ok(), "an existing entry can still change");
    }

    #[test]
    fn the_global_follows_the_list() {
        let list = [CustomName { id: Id(9_990_010), name: Some("Ali".into()), tag: Some("work".into()), color: Some(0xE5484D) }];
        with_names(&list, || {
            assert_eq!((name(Id(9_990_010)).as_deref(), tag(Id(9_990_010)).as_deref(), color(Id(9_990_010))), (Some("Ali"), Some("work"), Some(0xE5484D)));
            assert_eq!(name(Id(9_990_011)), None);
        });
        assert_eq!(name(Id(9_990_010)), None);
    }
}
