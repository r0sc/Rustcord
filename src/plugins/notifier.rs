//! RelationshipNotifier (PLUGINS_NOTES S43): a toast (and a desktop notification) when someone
//! removes me as a friend, or I'm removed from a server or a group DM. My own removals (asked
//! in a confirm dialog: Remove Friend, Leave Server, Leave Group) are not announced.
//! OnePingPerDM (S44): one desktop notification per DM until it is read.
//! NotificationLog: the Inbox's Recent tab keeps this session's notifications, also the ones
//! Do Not Disturb, quiet hours or OnePingPerDM silenced (memory only, 50 rows).

use super::{Plugin, enabled, opt_bool};
use crate::backend::{AccountCommand, Command, SocialCommand};
use crate::model::{GatewayEvent, Id, channel_type, relationship_type};
use crate::state::{AppState, Notification};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A removal asked for in a confirm dialog counts as mine for this long.
const MINE_FOR: Duration = Duration::from_secs(120);

static MINE: Mutex<Vec<(Id, Instant)>> = Mutex::new(Vec::new());

/// Called every frame: remembers the target of a removal the user is being asked about (the
/// dialog's command is mine whether or not it is confirmed; a stranger's removal of the same
/// thing in the next two minutes is not announced: rare and harmless).
pub fn watch_confirm(view: &crate::ui::ViewState) {
    if !enabled(Plugin::RelationshipNotifier) {
        return;
    }
    let target = match view.confirm.as_ref().map(|c| &c.then) {
        Some(Command::Social(SocialCommand::RemoveRelationship { user_id, .. })) => *user_id,
        Some(Command::Social(SocialCommand::CloseDm { channel_id, .. })) => *channel_id,
        Some(Command::Account(AccountCommand::LeaveGuild { guild_id })) => *guild_id,
        _ => return,
    };
    let mut mine = MINE.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    mine.retain(|(_, t)| now.duration_since(*t) < MINE_FOR);
    match mine.iter_mut().find(|(id, _)| *id == target) {
        Some(e) => e.1 = now,
        None => mine.push((target, now)),
    }
}

fn is_mine(id: Id) -> bool {
    MINE.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|(m, t)| *m == id && t.elapsed() < MINE_FOR)
}

/// The notice for an event, read from the state before the event changes it.
fn notice(state: &AppState, ev: &GatewayEvent) -> Option<String> {
    match ev {
        GatewayEvent::RelationshipRemove { id } if opt_bool(Plugin::RelationshipNotifier, "friends") => {
            let r = state.social.relationships.get(id)?;
            (r.kind == relationship_type::FRIEND).then(|| format!("{} removed you as a friend.", state.friend_name(*id)))
        }
        GatewayEvent::GuildDelete { id, unavailable: false } if opt_bool(Plugin::RelationshipNotifier, "servers") => {
            Some(format!("You were removed from the server {}.", state.guild(*id)?.name))
        }
        GatewayEvent::ChannelDelete { id, .. } if opt_bool(Plugin::RelationshipNotifier, "groups") => {
            let c = state.channels.get(id).filter(|c| c.kind == channel_type::GROUP_DM)?;
            Some(format!("You were removed from the group {}.", c.name.clone().unwrap_or_else(|| state.channel_name(*id))))
        }
        _ => None,
    }
}

/// OnePingPerDM: per DM, my read position when it last notified. A DM notifies again only once
/// it was read since (the position moved).
static PINGED: Mutex<Option<HashMap<Id, Option<Id>>>> = Mutex::new(None);

/// Whether a message notification for `channel_id` may show (`ui/mod.rs`, after the state made
/// it): with OnePingPerDM only the first per DM until that DM is read.
pub fn one_ping(state: &AppState, channel_id: Id) -> bool {
    if !enabled(Plugin::OnePingPerDm) || !state.channels.get(&channel_id).is_some_and(|c| matches!(c.kind, channel_type::DM | channel_type::GROUP_DM)) {
        return true;
    }
    let read = state.read_states.get(&channel_id).and_then(|r| r.last_message_id);
    gate(PINGED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new), channel_id, read)
}

fn gate(pinged: &mut HashMap<Id, Option<Id>>, ch: Id, read: Option<Id>) -> bool {
    if pinged.get(&ch) == Some(&read) {
        return false;
    }
    pinged.insert(ch, read);
    true
}

// ---------------------------------------------------------------------------
// NotificationLog
// ---------------------------------------------------------------------------

/// Rows the log keeps; the oldest drop out.
pub const LOG_MAX: usize = 50;

/// Why a notification made no sound and no toast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Silenced {
    DoNotDisturb,
    QuietHours,
    OnePing,
}

impl Silenced {
    pub fn label(self) -> &'static str {
        match self {
            Silenced::DoNotDisturb => "Do Not Disturb",
            Silenced::QuietHours => "Quiet hours",
            Silenced::OnePing => "One ping per DM",
        }
    }
}

/// What silences a notification, in the order the checks run (the later ones are asked only when
/// the earlier ones pass: OnePingPerDM remembers every DM it lets through).
pub fn silenced_by(dnd: bool, quiet: impl FnOnce() -> bool, one_ping_ok: impl FnOnce() -> bool) -> Option<Silenced> {
    if dnd {
        Some(Silenced::DoNotDisturb)
    } else if quiet() {
        Some(Silenced::QuietHours)
    } else if !one_ping_ok() {
        Some(Silenced::OnePing)
    } else {
        None
    }
}

/// One row of the Recent tab.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub channel_id: Id,
    pub message_id: Id,
    /// The sender's avatar; none while Streamer Mode hides personal information.
    pub author_id: Option<Id>,
    pub title: String,
    pub body: String,
    pub at_ms: u64,
    pub silenced: Option<Silenced>,
}

/// The row for `n`, or None with the plugin off or for a notification without a message (a ringing
/// call). Streamer Mode's personal-information hiding gives the same generic text as the desktop
/// notification would.
pub fn entry(n: &Notification, settings: &crate::config::Settings, silenced: Option<Silenced>) -> Option<Entry> {
    let message_id = n.message_id.filter(|_| enabled(Plugin::NotificationLog))?;
    let hide = settings.streamer.hides_personal();
    let (title, body) = if hide { (crate::config::APP_NAME.to_owned(), "New message".to_owned()) } else { (n.title.clone(), n.body.clone()) };
    Some(Entry { channel_id: n.channel_id, message_id, author_id: n.author_id.filter(|_| !hide), title, body, at_ms: crate::activity::now_ms(), silenced })
}

/// Newest first, at most `LOG_MAX`.
pub fn log(list: &mut VecDeque<Entry>, e: Entry) {
    list.push_front(e);
    list.truncate(LOG_MAX);
}

pub fn on_gateway(state: &AppState, view: &mut crate::ui::ViewState, settings: &crate::config::Settings, ev: &GatewayEvent) {
    if !enabled(Plugin::RelationshipNotifier) {
        return;
    }
    let target = match ev {
        GatewayEvent::RelationshipRemove { id } | GatewayEvent::GuildDelete { id, .. } | GatewayEvent::ChannelDelete { id, .. } => *id,
        _ => return,
    };
    if is_mine(target) {
        return;
    }
    let Some(text) = notice(state, ev) else { return };
    // Through `notification` so Streamer Mode silences or anonymizes it like a message's.
    if opt_bool(Plugin::RelationshipNotifier, "notify")
        && let Some((title, body)) = settings.notification(crate::config::APP_NAME, &text)
    {
        crate::notify::show(title, body);
    }
    view.toast(text, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_keeps_50_newest_first() {
        let entry = |i: u64| Entry { channel_id: Id(1), message_id: Id(i), author_id: None, title: String::new(), body: String::new(), at_ms: i, silenced: None };
        let mut list = VecDeque::new();
        for i in 0..60 {
            log(&mut list, entry(i));
        }
        assert_eq!((list.len(), list.front().map(|e| e.at_ms), list.back().map(|e| e.at_ms)), (LOG_MAX, Some(59), Some(10)));
    }

    #[test]
    fn the_reason_follows_the_check_order_and_asks_lazily() {
        let (yes, no) = (|| true, || false);
        assert_eq!(silenced_by(true, yes, no), Some(Silenced::DoNotDisturb));
        assert_eq!(silenced_by(false, yes, no), Some(Silenced::QuietHours));
        assert_eq!(silenced_by(false, no, no), Some(Silenced::OnePing));
        assert_eq!(silenced_by(false, no, yes), None);
        // OnePingPerDM is asked only when nothing earlier silenced it (it remembers what it lets through).
        let asked = std::cell::Cell::new(false);
        assert_eq!(silenced_by(true, no, || asked.replace(true)), Some(Silenced::DoNotDisturb));
        assert_eq!(silenced_by(false, yes, || asked.replace(true)), Some(Silenced::QuietHours));
        assert!(!asked.get());
        assert_eq!(Silenced::OnePing.label(), "One ping per DM");
    }

    #[test]
    fn a_row_needs_the_plugin_a_message_and_respects_streamer_mode() {
        let n = Notification { channel_id: Id(5), message_id: Some(Id(9)), title: "Ann (#general)".into(), body: "hello".into(), author_id: Some(Id(3)), mentioned: false };
        let mut settings = crate::config::Settings::default();
        // Off by default: no row.
        assert!(entry(&n, &settings, None).is_none());
        super::super::put(Plugin::NotificationLog, "enabled", toml::Value::Boolean(true));
        let e = entry(&n, &settings, Some(Silenced::QuietHours)).unwrap();
        assert_eq!((e.message_id, e.author_id, e.title.as_str(), e.silenced), (Id(9), Some(Id(3)), "Ann (#general)", Some(Silenced::QuietHours)));
        // A ringing call has no message.
        assert!(entry(&Notification { message_id: None, ..n.clone() }, &settings, None).is_none());
        settings.streamer.enabled = true;
        settings.streamer.hide_personal_info = true;
        let hidden = entry(&n, &settings, None).unwrap();
        assert_eq!((hidden.title.as_str(), hidden.body.as_str(), hidden.author_id), ("Rustcord", "New message", None));
        super::super::put(Plugin::NotificationLog, "enabled", toml::Value::Boolean(false));
    }

    #[test]
    fn one_ping_per_dm_until_read() {
        let mut pinged = HashMap::new();
        let (dm, other) = (Id(1), Id(2));
        // First message: notifies; more while unread: silent.
        assert!(gate(&mut pinged, dm, Some(Id(10))));
        assert!(!gate(&mut pinged, dm, Some(Id(10))));
        assert!(!gate(&mut pinged, dm, Some(Id(10))));
        // Other DMs are separate; a never-read DM counts too.
        assert!(gate(&mut pinged, other, None));
        assert!(!gate(&mut pinged, other, None));
        // Read since (position moved): the next one notifies again.
        assert!(gate(&mut pinged, dm, Some(Id(15))));
        assert!(!gate(&mut pinged, dm, Some(Id(15))));
        // Off by default: everything notifies.
        assert!(one_ping(&AppState::default(), dm));
    }
}
