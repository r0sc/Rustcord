//! Native plugins: Vencord's useful plugins re-implemented as plain Rust (docs/PLUGINS_NOTES.md).
//! No runtime, no scripting: every plugin is an entry in `META` (id, description, Vencord's
//! default of off, options) plus hook functions that the UI / state code calls at its existing
//! render and event points, each guarded by `enabled(Plugin::X)`. On/off and options live in
//! `<config>/plugins.toml` (one table per plugin, only what differs from the defaults).
//!
//! Rules every plugin keeps (ARCHITECTURE "Normal user client behaviour"): no automation, no
//! background sending, every request user-triggered or what the official client sends for
//! the same action; never fake Nitro or any paid feature; local data stays local and bounded.
//! OWNERSHIP: plugins engineer (`src/plugins/*`, `src/ui/user_settings/plugins.rs`).

pub mod clear_urls;
pub mod message_logger;
pub mod notifier;
pub mod people;
pub mod media;
pub mod menus;
pub mod sidebar;
pub mod chat;
pub mod windows;
pub mod voice_messages;
pub mod compose;
pub mod link_embeds;
pub mod panel;
pub mod idle;
pub mod voice;
pub mod diagnostics;
pub mod translate;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

/// Every plugin, in the order of `META`. A plugin's index is its bit in `ENABLED`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Plugin {
    ShowHiddenChannels,
    PlatformIndicators,
    MoreUserTags,
    BetterRoleDot,
    ImageZoom,
    ReverseImageSearch,
    CopyUserUrls,
    FriendsSince,
    MutualGroupDms,
    PermissionsViewer,
    ServerInfo,
    ShowConnections,
    ShowMeYourName,
    TypingIndicator,
    TypingTweaks,
    UserVoiceShow,
    WhoReacted,
    CallTimer,
    ClearUrls,
    NoBlockedMessages,
    NoReplyMention,
    ReadAllNotificationsButton,
    RelationshipNotifier,
    SilentTyping,
    SilentMessageToggle,
    MessageLogger,
    Translate,
    FavoriteEmojiFirst,
    ViewIcons,
    ValidUser,
    VoiceMessages,
    BetterFolders,
    IrcColors,
    ReplyTimestamp,
    MemberCount,
    ShowTimeoutDuration,
    OpenInApp,
    ViewRaw,
    TextReplace,
    CharacterCounter,
    MessageLinkEmbeds,
    GameActivityToggle,
    AccountPanelServerProfile,
    CustomIdle,
    VcNarrator,
    VolumeBooster,
    OnePingPerDm,
    PinDms,
    ClientDiagnostics,
    QuickReply,
    NotificationLog,
    QuickMention,
    ShowBadgesInChat,
}

/// One option of a plugin (the settings page draws it from this).
#[derive(Debug, Clone, Copy)]
pub struct Opt {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: OptKind,
}

#[derive(Debug, Clone, Copy)]
pub enum OptKind {
    Bool(bool),
    /// Choices, default index.
    Choice(&'static [&'static str], usize),
    /// min, max, default.
    Int(i64, i64, i64),
    /// Default text; `hint` shown in the empty field.
    Text(&'static str, &'static str),
}

#[derive(Debug, Clone, Copy)]
pub struct Meta {
    pub id: Plugin,
    /// Vencord's name: the key in plugins.toml and the card title.
    pub name: &'static str,
    pub description: &'static str,
    /// Vencord turns user plugins off until enabled; so does Rustcord.
    pub default_on: bool,
    pub opts: &'static [Opt],
}

const fn opt(key: &'static str, label: &'static str, kind: OptKind) -> Opt {
    Opt { key, label, kind }
}

const fn meta(id: Plugin, name: &'static str, description: &'static str, opts: &'static [Opt]) -> Meta {
    Meta { id, name, description, default_on: false, opts }
}

use OptKind::{Bool, Choice, Int, Text};
use Plugin as P;

pub const META: &[Meta] = &[
    meta(P::ShowHiddenChannels, "ShowHiddenChannels", "Lists channels you can't view, with a lock. Hover shows their topic and last activity; nothing is loaded.", &[]),
    meta(P::PlatformIndicators, "PlatformIndicators", "Shows whether people are on desktop, mobile, web or a console.", &[
        opt("list", "Member list", Bool(true)),
        opt("messages", "Messages", Bool(true)),
        opt("badges", "Profiles", Bool(true)),
    ]),
    meta(P::MoreUserTags, "MoreUserTags", "Owner, Admin and Mod tags next to names.", &[opt("messages", "In messages", Bool(true)), opt("list", "In the member list", Bool(true))]),
    meta(P::BetterRoleDot, "BetterRoleDot", "Click a role's dot to copy its colour.", &[]),
    meta(P::ImageZoom, "ImageZoom", "Hold the mouse on an image for a magnifier; scroll to zoom.", &[
        opt("zoom", "Zoom", Int(1, 10, 2)),
        opt("size", "Lens size", Int(50, 600, 100)),
        opt("square", "Square lens", Bool(false)),
    ]),
    meta(P::ReverseImageSearch, "ReverseImageSearch", "Search an image with Google Lens, Yandex, SauceNAO and more (message menu).", &[]),
    meta(P::CopyUserUrls, "CopyUserURLs", "Copy a link to someone's profile from their menu.", &[]),
    meta(P::FriendsSince, "FriendsSince", "Shows since when you are friends.", &[]),
    meta(P::MutualGroupDms, "MutualGroupDMs", "Lists the group DMs you share in profiles.", &[]),
    meta(P::PermissionsViewer, "PermissionsViewer", "See anyone's server permissions and where they come from.", &[]),
    meta(P::ServerInfo, "ServerInfo", "Server info from the server menu: owner, dates, counts.", &[]),
    meta(P::ShowConnections, "ShowConnections", "Connected accounts in the profile popout.", &[]),
    meta(P::ShowMeYourName, "ShowMeYourName", "Shows the username next to nicknames.", &[opt("displayNames", "Also next to display names", Bool(false))]),
    meta(P::TypingIndicator, "TypingIndicator", "Typing dots in the channel and DM list.", &[
        opt("includeCurrentChannel", "Include the open channel", Bool(true)),
        opt("includeMutedChannels", "Include muted channels", Bool(false)),
    ]),
    meta(P::TypingTweaks, "TypingTweaks", "Avatars and role colours in the typing line.", &[
        opt("showAvatars", "Avatars", Bool(true)),
        opt("showRoleColors", "Role colours", Bool(true)),
    ]),
    meta(P::UserVoiceShow, "UserVoiceShow", "A speaker next to people who are in a voice channel.", &[
        opt("showInMemberList", "Member list", Bool(true)),
        opt("showInMessages", "Messages", Bool(true)),
        opt("showInUserProfile", "Profiles", Bool(true)),
    ]),
    meta(P::WhoReacted, "WhoReacted", "Avatars of who reacted, after hovering a reaction.", &[]),
    meta(P::CallTimer, "CallTimer", "How long you've been in the call.", &[opt("format", "Format", Choice(&["Stopwatch", "Human"], 0))]),
    meta(P::ClearUrls, "ClearURLs", "Removes tracking parameters from links you send and open.", &[]),
    meta(P::NoBlockedMessages, "NoBlockedMessages", "Hides messages from people you blocked.", &[opt("ignored", "Also ignored users", Bool(true))]),
    meta(P::NoReplyMention, "NoReplyMention", "Replies don't ping by default.", &[opt("userList", "Still ping these user IDs", Text("", "IDs, comma separated"))]),
    meta(P::ReadAllNotificationsButton, "ReadAllNotificationsButton", "A button above the server list that marks every server read.", &[]),
    meta(P::RelationshipNotifier, "RelationshipNotifier", "Tells you when you're unfriended, removed from a server or a group.", &[
        opt("friends", "Friends", Bool(true)),
        opt("servers", "Servers", Bool(true)),
        opt("groups", "Groups", Bool(true)),
        opt("notify", "Desktop notification", Bool(true)),
    ]),
    meta(P::SilentTyping, "SilentTyping", "Others don't see you typing.", &[]),
    meta(P::SilentMessageToggle, "SilentMessageToggle", "A composer button to send without notifications.", &[opt("autoDisable", "Turn off after sending", Bool(true))]),
    meta(P::MessageLogger, "MessageLogger", "Keeps deleted and edited messages visible, greyed. Only in memory, only on this device.", &[
        opt("logDeletes", "Deleted messages", Bool(true)),
        opt("logEdits", "Edit history", Bool(true)),
        opt("ignoreBots", "Ignore bots", Bool(true)),
        opt("ignoreSelf", "Ignore my messages", Bool(false)),
    ]),
    meta(P::Translate, "Translate", "Translate in the message menu shows the message in your language. Only on that click is its text sent: to Google Translate, or to DeepL if you add a key.", &[
        opt("to", "Translate to", Choice(translate::LANGUAGES, 0)),
    ]),
    meta(P::FavoriteEmojiFirst, "FavoriteEmojiFirst", "Your most used emoji first in : suggestions.", &[]),
    meta(P::ViewIcons, "ViewIcons", "Open avatars, banners and server icons in full size.", &[
        opt("format", "Format", Choice(&["webp", "png"], 0)),
        opt("size", "Size", Choice(&["1024", "4096"], 0)),
    ]),
    meta(P::ValidUser, "ValidUser", "Loads unknown mentioned users when you click them.", &[]),
    meta(P::VoiceMessages, "VoiceMessages", "Play voice messages right in the chat.", &[opt("volume", "Volume %", Int(0, 200, 100))]),
    meta(P::BetterFolders, "BetterFolders", "Server folder tweaks.", &[
        opt("closeOthers", "Opening a folder closes the others", Bool(true)),
        opt("forceOpen", "Keep the open server's folder open", Bool(false)),
    ]),
    meta(P::IrcColors, "IrcColors", "Colourful names for people without a role colour.", &[opt("onlyDms", "Only in DMs", Bool(false))]),
    meta(P::ReplyTimestamp, "ReplyTimestamp", "The time of the replied-to message.", &[]),
    meta(P::MemberCount, "MemberCount", "Online and total members above the member list.", &[]),
    meta(P::ShowTimeoutDuration, "ShowTimeoutDuration", "How long someone is still timed out.", &[]),
    meta(P::OpenInApp, "OpenInApp", "Opens Spotify and Steam links in their apps.", &[opt("spotify", "Spotify", Bool(true)), opt("steam", "Steam", Bool(true))]),
    meta(P::ViewRaw, "ViewRaw", "Copy a message as raw JSON (message menu).", &[]),
    meta(P::TextReplace, "TextReplace", "Your own find-and-replace rules, applied to messages you send.", &[]),
    meta(P::CharacterCounter, "CharacterCounter", "How long your message is, out of 2000 characters (4000 with Nitro).", &[]),
    meta(P::MessageLinkEmbeds, "MessageLinkEmbeds", "A preview of linked messages. Ones not loaded yet load when you click.", &[]),
    meta(P::GameActivityToggle, "GameActivityToggle", "A controller button in your user panel that shares or hides your game.", &[]),
    meta(P::AccountPanelServerProfile, "AccountPanelServerProfile", "In a server, your user panel shows your server name and avatar; a click opens that profile.", &[]),
    meta(P::CustomIdle, "CustomIdle", "Idle after a while without input, online again when you're back (Windows and macOS).", &[opt("idleTimeout", "Minutes until idle (0 = never)", Int(0, 60, 10))]),
    meta(P::VcNarrator, "VcNarrator", "Reads out who joins, leaves or moves in your voice channel. Silent in Streamer Mode (Linux needs speech-dispatcher).", &[]),
    meta(P::VolumeBooster, "VolumeBooster", "Turn someone up to 400 % from their menu while you're in voice together.", &[]),
    meta(P::OnePingPerDm, "OnePingPerDM", "Only the first notification of a DM until you've read it.", &[]),
    meta(P::PinDms, "PinDMs", "Pin DMs to the top of the list from their menu.", &[]),
    meta(P::ClientDiagnostics, "ClientDiagnostics", "Settings → Advanced shows frame times, memory and the connection, with a button that copies them as text. Nothing is sent.", &[]),
    meta(P::QuickReply, "QuickReply", "Ctrl+Up / Ctrl+Down pick the message to reply to; Ctrl+Shift+Up / Down pick one of yours to edit. Down past the newest cancels.", &[]),
    meta(P::NotificationLog, "NotificationLog", "The Inbox gets a Recent tab with this session's notifications, also the ones Do Not Disturb, quiet hours or one ping per DM silenced. Kept in memory only.", &[]),
    meta(P::QuickMention, "QuickMention", "A Mention button on messages puts @name into your message box. Nothing is sent until you send it.", &[]),
    meta(P::ShowBadgesInChat, "ShowBadgesInChat", "Small grey Discord badges after names in chat: Staff, Partner, HypeSquad, Bug Hunter, Early Supporter and more.", &[]),
];

pub fn meta_of(p: Plugin) -> &'static Meta {
    &META[p as usize]
}

// ---------------------------------------------------------------------------
// Settings store (plugins.toml)
// ---------------------------------------------------------------------------

/// On/off and option values: one TOML table per plugin, keyed by `Meta::name`. Missing keys
/// take the defaults from `META`, unknown ones are kept (newer versions, typos).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Store {
    table: toml::Table,
}

impl Store {
    pub fn parse(text: &str) -> Self {
        Self { table: text.parse::<toml::Table>().unwrap_or_default() }
    }

    pub fn to_text(&self) -> String {
        toml::to_string_pretty(&self.table).unwrap_or_default()
    }

    fn value(&self, p: Plugin, key: &str) -> Option<&toml::Value> {
        self.table.get(meta_of(p).name)?.as_table()?.get(key)
    }

    pub fn is_on(&self, p: Plugin) -> bool {
        self.value(p, "enabled").and_then(toml::Value::as_bool).unwrap_or(meta_of(p).default_on)
    }

    /// `ENABLED` bits.
    pub fn bits(&self) -> u64 {
        META.iter().filter(|m| self.is_on(m.id)).fold(0, |b, m| b | 1 << m.id as u8)
    }

    pub fn set(&mut self, p: Plugin, key: &str, v: toml::Value) {
        let t = self.table.entry(meta_of(p).name).or_insert_with(|| toml::Value::Table(Default::default()));
        if let Some(t) = t.as_table_mut() {
            t.insert(key.to_owned(), v);
        }
    }

    fn opt(p: Plugin, key: &str) -> OptKind {
        let o = meta_of(p).opts.iter().find(|o| o.key == key);
        debug_assert!(o.is_some(), "{key} is not an option of {p:?}");
        o.map_or(Bool(false), |o| o.kind)
    }

    pub fn bool(&self, p: Plugin, key: &str) -> bool {
        let d = matches!(Self::opt(p, key), Bool(true));
        self.value(p, key).and_then(toml::Value::as_bool).unwrap_or(d)
    }

    pub fn int(&self, p: Plugin, key: &str) -> i64 {
        let (lo, hi, d) = if let Int(lo, hi, d) = Self::opt(p, key) { (lo, hi, d) } else { (0, 0, 0) };
        self.value(p, key).and_then(toml::Value::as_integer).map_or(d, |v| v.clamp(lo, hi))
    }

    pub fn choice(&self, p: Plugin, key: &str) -> usize {
        let (n, d) = if let Choice(list, d) = Self::opt(p, key) { (list.len(), d) } else { (1, 0) };
        self.value(p, key).and_then(toml::Value::as_integer).map_or(d, |v| (v.max(0) as usize).min(n - 1))
    }

    pub fn text(&self, p: Plugin, key: &str) -> String {
        let d = if let Text(d, _) = Self::opt(p, key) { d } else { "" };
        self.value(p, key).and_then(toml::Value::as_str).unwrap_or(d).to_owned()
    }
}

/// Bit `p as u8` set = plugin on. Read on every hook call, so it is an atomic, not the lock.
static ENABLED: AtomicU64 = AtomicU64::new(0);

fn store() -> &'static RwLock<Store> {
    static STORE: OnceLock<RwLock<Store>> = OnceLock::new();
    STORE.get_or_init(|| {
        // Unit tests never read the real config: every plugin at its default.
        let s = if cfg!(test) { Store::default() } else { std::fs::read_to_string(file()).map(|t| Store::parse(&t)).unwrap_or_default() };
        ENABLED.store(s.bits(), Ordering::Relaxed);
        RwLock::new(s)
    })
}

fn file() -> std::path::PathBuf {
    crate::config::paths().config_dir.join("plugins.toml")
}

/// Which plugins' code lives in each `src/plugins/<stem>.rs` (crash reports name them).
const FILES: &[(&str, &[Plugin])] = &[
    ("chat", &[P::TypingTweaks, P::SilentMessageToggle, P::NoReplyMention, P::NoBlockedMessages, P::WhoReacted, P::SilentTyping, P::ReplyTimestamp, P::FavoriteEmojiFirst]),
    ("clear_urls", &[P::ClearUrls]),
    ("compose", &[P::TextReplace, P::CharacterCounter, P::QuickReply]),
    ("idle", &[P::CustomIdle]),
    ("link_embeds", &[P::MessageLinkEmbeds]),
    ("media", &[P::ImageZoom, P::ViewIcons, P::OpenInApp, P::ClearUrls]),
    ("menus", &[P::ViewIcons, P::PermissionsViewer, P::CopyUserUrls, P::ViewRaw, P::Translate, P::ServerInfo, P::ReverseImageSearch]),
    ("message_logger", &[P::MessageLogger]),
    ("notifier", &[P::RelationshipNotifier, P::OnePingPerDm, P::NotificationLog]),
    ("panel", &[P::GameActivityToggle, P::AccountPanelServerProfile]),
    ("people", &[P::UserVoiceShow, P::PlatformIndicators, P::ShowMeYourName, P::MoreUserTags, P::ShowTimeoutDuration, P::IrcColors, P::ValidUser, P::ShowConnections, P::PermissionsViewer, P::MutualGroupDms, P::MemberCount, P::FriendsSince, P::BetterRoleDot, P::ShowBadgesInChat]),
    ("sidebar", &[P::PinDms, P::BetterFolders, P::TypingIndicator, P::CallTimer, P::ShowHiddenChannels, P::ReadAllNotificationsButton]),
    ("translate", &[P::Translate]),
    ("voice", &[P::VolumeBooster, P::VcNarrator]),
    ("voice_messages", &[P::VoiceMessages]),
    ("diagnostics", &[P::ClientDiagnostics]),
];

/// Every plugin whose code lives in the file of a panic location (`src/plugins/people.rs:3:5`,
/// either slash). Empty for any other file.
fn plugins_of_file(location: &str) -> &'static [Plugin] {
    let loc = location.replace('\\', "/");
    let Some((_, rest)) = loc.rsplit_once("src/plugins/") else { return &[] };
    let stem = rest.split(['.', ':']).next().unwrap_or("");
    FILES.iter().find(|(s, _)| *s == stem).map_or(&[], |(_, ps)| ps)
}

/// The plugins of that file that are on now.
pub fn in_file(location: &str) -> Vec<Plugin> {
    plugins_of_file(location).iter().copied().filter(|p| enabled(*p)).collect()
}

/// Names of the plugins in `bits`.
fn names_of(bits: u64) -> Vec<&'static str> {
    META.iter().filter(|m| bits & (1 << m.id as u8) != 0).map(|m| m.name).collect()
}

/// Names of the plugins that are on, for a crash report. Reads the bit mask only (no lock, no
/// file), so it is safe inside a panic hook.
pub fn enabled_names() -> Vec<&'static str> {
    names_of(ENABLED.load(Ordering::Relaxed))
}

/// The hook guard.
pub fn enabled(p: Plugin) -> bool {
    store();
    ENABLED.load(Ordering::Relaxed) & (1 << p as u8) != 0
}

fn read<R>(f: impl FnOnce(&Store) -> R) -> R {
    f(&store().read().unwrap_or_else(|e| e.into_inner()))
}

pub fn opt_bool(p: Plugin, key: &str) -> bool {
    read(|s| s.bool(p, key))
}

pub fn opt_int(p: Plugin, key: &str) -> i64 {
    read(|s| s.int(p, key))
}

pub fn opt_choice(p: Plugin, key: &str) -> usize {
    read(|s| s.choice(p, key))
}

pub fn opt_text(p: Plugin, key: &str) -> String {
    read(|s| s.text(p, key))
}

/// A value a plugin keeps itself, outside `opts` (TextReplace rules, pinned DMs, volumes).
pub fn opt_raw(p: Plugin, key: &str) -> Option<toml::Value> {
    read(|s| s.value(p, key).cloned())
}

/// Changes a value (the settings page) and writes plugins.toml. "enabled" switches the plugin.
pub fn set(p: Plugin, key: &str, v: toml::Value) -> Result<(), String> {
    put(p, key, v);
    save()
}

/// Changes a value in memory only (a slider while it is dragged); `save` writes it.
pub fn put(p: Plugin, key: &str, v: toml::Value) {
    let mut s = store().write().unwrap_or_else(|e| e.into_inner());
    s.set(p, key, v);
    ENABLED.store(s.bits(), Ordering::Relaxed);
    drop(s);
    if p == Plugin::MessageLogger && !enabled(p) {
        message_logger::clear();
    }
    if p == Plugin::Translate && !enabled(p) {
        translate::clear();
    }
}

/// Writes plugins.toml (not in offline test runs without their own RUSTCORD_HOME).
pub fn save() -> Result<(), String> {
    if crate::config::offline_mode() && std::env::var_os("RUSTCORD_HOME").is_none() {
        return Ok(());
    }
    let text = read(Store::to_text);
    crate::config::write_atomic(&file(), text.as_bytes())
}

/// Test hook `RUSTCORD_PLUGINS=all|<Name>,<Name>`: switches plugins on for this run only
/// (screenshots), without touching plugins.toml.
pub fn apply_env() {
    let Ok(list) = std::env::var("RUSTCORD_PLUGINS") else { return };
    let mut s = store().write().unwrap_or_else(|e| e.into_inner());
    for m in META.iter().filter(|m| list == "all" || list.split(',').any(|n| n.trim().eq_ignore_ascii_case(m.name))) {
        s.set(m.id, "enabled", toml::Value::Boolean(true));
    }
    ENABLED.store(s.bits(), Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Event and frame hooks (called once from ui/mod.rs)
// ---------------------------------------------------------------------------

/// Before a gateway event reaches the state: MessageLogger keeps deleted messages (the event is
/// swallowed) and edit history; RelationshipNotifier reads names that the event removes.
pub fn on_gateway(state: &mut crate::state::AppState, view: &mut crate::ui::ViewState, settings: &crate::config::Settings, ev: crate::model::GatewayEvent) -> Option<crate::model::GatewayEvent> {
    notifier::on_gateway(state, view, settings, &ev);
    voice::narrate(state, settings, &ev);
    message_logger::on_gateway(state, ev)
}

/// Before a chat event reaches the state: MessageLinkEmbeds takes the answer of its own
/// "Load message" fetch (the jump view would drop it).
pub fn on_chat(state: &crate::state::AppState, ev: crate::backend::ChatEvent) -> Option<crate::backend::ChatEvent> {
    link_embeds::on_chat(state, ev)
}

/// Once per frame on the main screen, after the page and its modals: plugin windows, link
/// rewriting (ClearURLs, OpenInApp) for every link opened this frame.
pub fn frame(ctx: &egui::Context, cx: &mut crate::ui::UiCx<'_>) {
    windows::show(ctx, cx);
    chat::sync_reactions(cx.state);
    notifier::watch_confirm(cx.view);
    if enabled(Plugin::ClearUrls) || enabled(Plugin::OpenInApp) {
        ctx.output_mut(|o| {
            for c in &mut o.commands {
                if let egui::OutputCommand::OpenUrl(u) = c {
                    u.url = media::rewrite_link(&u.url);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_is_in_enum_order_and_fits_the_bitmask() {
        assert!(META.len() <= 64);
        for (i, m) in META.iter().enumerate() {
            assert_eq!(m.id as usize, i, "{}", m.name);
            assert!(!m.default_on, "{} defaults to off like Vencord", m.name);
        }
        assert!(META.len() >= 48);
    }

    #[test]
    fn a_panic_location_maps_to_the_plugins_of_its_file() {
        let people = plugins_of_file("src\\plugins\\people.rs:12:5");
        assert!(people.contains(&P::MemberCount) && people.contains(&P::FriendsSince));
        assert_eq!(plugins_of_file("src/plugins/people.rs:12:5"), people);
        assert_eq!(plugins_of_file("C:/x/Rustcord/src/plugins/people.rs:1:1"), people);
        assert!(plugins_of_file("src/ui/mod.rs:1:1").is_empty());
        assert!(plugins_of_file("src/plugins/mod.rs:1:1").is_empty());
        // Nothing is on in tests, so the "on now" filter leaves nothing.
        assert!(in_file("src/plugins/people.rs:12:5").is_empty());
        assert_eq!(names_of(1 << P::FriendsSince as u8 | 1 << P::MemberCount as u8), ["FriendsSince", "MemberCount"]);
    }

    #[test]
    fn settings_persist_through_toml() {
        let mut s = Store::default();
        assert!(!s.is_on(P::ClearUrls));
        assert!(s.bool(P::TypingIndicator, "includeCurrentChannel"));
        assert_eq!(s.int(P::ImageZoom, "zoom"), 2);
        s.set(P::ClearUrls, "enabled", toml::Value::Boolean(true));
        s.set(P::ImageZoom, "zoom", toml::Value::Integer(99));
        s.set(P::CallTimer, "format", toml::Value::Integer(1));
        s.set(P::NoReplyMention, "userList", toml::Value::String("1,2".into()));
        let back = Store::parse(&s.to_text());
        assert_eq!(back, s);
        assert!(back.is_on(P::ClearUrls));
        assert_eq!(back.bits(), 1 << P::ClearUrls as u8);
        // Out of range values are clamped, not trusted.
        assert_eq!(back.int(P::ImageZoom, "zoom"), 10);
        assert_eq!(back.choice(P::CallTimer, "format"), 1);
        assert_eq!(back.text(P::NoReplyMention, "userList"), "1,2");
        // Unknown tables and a broken file never fail the load.
        assert!(!Store::parse("[Foo]\nenabled = true\n").is_on(P::ClearUrls));
        assert_eq!(Store::parse("not toml ["), Store::default());
    }
}
