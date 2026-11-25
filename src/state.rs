//! Application state, owned by the UI thread. Pure data + logic, no I/O, no egui.
//!
//! OWNERSHIP: struct fields and pub method signatures are a shared contract (ui/ reads the
//! fields directly). Method bodies belong to the state implementer.

pub mod account;
pub mod calls;
pub mod chat;
mod memstats;
pub mod social;
pub mod video;

use crate::backend::ConnState;
use crate::cache::{SNAPSHOT_VERSION, Snapshot};
use crate::model::*;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

/// Max messages kept per channel in memory (older ones are dropped when scrolling down).
pub const MAX_MESSAGES_PER_CHANNEL: usize = 400;

/// Recently selected channels whose last messages go into the disk cache.
const SNAPSHOT_CHANNELS: usize = 5;
/// Messages per channel kept in the disk cache.
const SNAPSHOT_MESSAGES: usize = 50;
/// Only the first member-list chunk (range 0..99) is subscribed to and kept.
const MEMBER_LIST_KEEP: usize = 100;
/// `last_channel_per_guild` key for the DM home (snowflakes are never 0).
const DM_HOME: Id = Id(0);

const MSG_RECIPIENT_REMOVE: u8 = 2;
const MSG_POLL_RESULT: u8 = 46;
/// Message flag SUPPRESS_NOTIFICATIONS (@silent).
const FLAG_SUPPRESS_NOTIFICATIONS: u64 = 1 << 12;

const NOTIFY_ALL: u8 = 0;
const NOTIFY_NOTHING: u8 = 2;
const NOTIFY_INHERIT: u8 = 3;

#[derive(Debug, Default)]
pub struct ChannelMessages {
    /// Sorted ascending by id (oldest first).
    pub msgs: Vec<Message>,
    /// False once a history fetch returned fewer messages than requested.
    pub has_more_before: bool,
    /// A history request is in flight.
    pub loading: bool,
    /// At least one history fetch completed (or the cache provided messages).
    pub loaded: bool,
    /// Last fetch failed with this error.
    pub error: Option<String>,
}

/// Locally echoed message while POST /messages is in flight.
#[derive(Debug, Clone)]
pub struct PendingMessage {
    pub nonce: String,
    pub channel_id: Id,
    pub content: String,
    pub failed: Option<String>,
    /// Kept so a retry is still sent as a reply.
    pub reply: Option<crate::backend::ReplyTo>,
}

#[derive(Debug, Default)]
pub struct MemberList {
    pub list_id: String,
    pub channel_id: Option<Id>,
    pub member_count: u32,
    pub online_count: u32,
    /// Flat list as sent by the gateway (groups followed by their members).
    pub items: Vec<MemberListItem>,
}

/// One row of a guild's channel sidebar, already filtered (VIEW_CHANNEL) and sorted.
#[derive(Debug, Clone, PartialEq)]
pub enum SidebarItem {
    Category { id: Id, name: String },
    Channel { id: Id },
}

/// Desktop notification produced by applying a MESSAGE_CREATE.
#[derive(Debug, Clone)]
pub struct Notification {
    #[allow(dead_code)] // for click-to-open once notify supports a click callback
    pub channel_id: Id,
    pub title: String,
    pub body: String,
    /// The sender, for the avatar on the overlay toast (None: a ringing call).
    pub author_id: Option<Id>,
}

#[derive(Debug)]
pub struct AppState {
    pub me: Option<User>,
    /// All known users (DM recipients, message authors seen, members).
    pub users: HashMap<Id, User>,
    /// Guilds in sidebar order. `Guild::channels` is empty here; see `channels`.
    pub guilds: Vec<Guild>,
    /// Every guild channel and DM channel by id.
    pub channels: HashMap<Id, Channel>,
    /// DM/group-DM channel ids, most recent activity first.
    pub dm_order: Vec<Id>,
    /// My member object per guild (roles for permissions, nick).
    pub my_members: HashMap<Id, Member>,
    pub read_states: HashMap<Id, ReadState>,
    /// Keyed by guild id; `None` = DM settings.
    pub guild_settings: HashMap<Option<Id>, GuildSettings>,
    pub messages: HashMap<Id, ChannelMessages>,
    pub pending: Vec<PendingMessage>,
    /// Keyed by guild id.
    pub member_lists: HashMap<Id, MemberList>,
    /// `None` = DM home.
    pub selected_guild: Option<Id>,
    pub selected_channel: Option<Id>,
    /// Last selected channel per guild (restored when switching guilds).
    pub last_channel_per_guild: HashMap<Id, Id>,
    pub conn: ConnState,
    /// True once READY (or a usable cache) has been applied.
    pub has_data: bool,
    /// Set whenever something worth persisting to the disk cache changed.
    pub dirty: bool,
    /// Channels whose history was fetched this session (cache data may be stale).
    pub fresh_channels: HashSet<Id>,
    /// Most recently selected channels, newest first (decides which messages are cached).
    pub recent_channels: Vec<Id>,
    /// Per channel: newest message held before READY / the cache load. Messages up to it
    /// may have been deleted meanwhile, so the next latest-page fetch decides about them.
    stale_upto: HashMap<Id, Id>,
    /// Blocked or ignored users: their messages neither notify nor bump mention counts.
    hidden_authors: HashSet<Id>,
    /// Who is in voice: channel id -> user id -> state (empty channels are removed).
    /// Not cached; rebuilt from READY_SUPPLEMENTAL / GUILD_CREATE / VOICE_STATE_UPDATE.
    pub voice_states: HashMap<Id, HashMap<Id, VoiceState>>,
    /// My own voice connection, as reported by the voice module.
    pub voice: VoiceStatus,
    /// DM / group DM calls and ringing (`state/calls.rs`). Not cached.
    pub calls: calls::CallState,
    /// Typing, presences, uploads, pins/search/GIF views (`state/chat.rs`). Not cached.
    pub chat: chat::ChatState,
    /// Relationships, thread/forum/mentions views, profiles (`state/social.rs`). Not cached.
    pub social: social::SocialState,
    /// Settings proto, action progress, bans/invites views, invite previews
    /// (`state/account.rs`). Not cached.
    pub account: account::AccountState,
    /// (open channel, my read position) when it was opened, or READY refreshed the read
    /// states, with unread messages: the NEW divider stays there for the visit although
    /// `mark_read` acks the channel right away. "Mark Unread" moves it.
    pub unread_marker: Option<(Id, Id)>,
    /// Game detection, my added games, activity icons (`crate::activity`). Not cached.
    pub activity: crate::activity::ActivityState,
    /// The Quests page: quests, Orbs, clicks on their way (`crate::quests`). Not cached.
    pub quests: crate::quests::QuestState,
}

/// What `BackendEvent::Voice` reported so far (read by the voice panel).
#[derive(Debug, Default)]
pub struct VoiceStatus {
    /// Channel being joined or connected; None = not in voice.
    pub channel_id: Option<Id>,
    /// Media is flowing (false while joining).
    pub connected: bool,
    /// Users whose speaking indicator is lit (me included).
    pub speaking: HashSet<Id>,
    /// My input level, 0.0..=1.0.
    pub level: f32,
    /// Device names from the last `ListVoiceDevices`.
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    /// Camera, screen share and remote videos (`VoiceEvent::Video`).
    pub video: video::VideoStatus,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            me: None,
            users: HashMap::new(),
            guilds: Vec::new(),
            channels: HashMap::new(),
            dm_order: Vec::new(),
            my_members: HashMap::new(),
            read_states: HashMap::new(),
            guild_settings: HashMap::new(),
            messages: HashMap::new(),
            pending: Vec::new(),
            member_lists: HashMap::new(),
            selected_guild: None,
            selected_channel: None,
            last_channel_per_guild: HashMap::new(),
            conn: ConnState::Offline,
            has_data: false,
            dirty: false,
            fresh_channels: HashSet::new(),
            recent_channels: Vec::new(),
            stale_upto: HashMap::new(),
            hidden_authors: HashSet::new(),
            voice_states: HashMap::new(),
            voice: VoiceStatus::default(),
            calls: calls::CallState::default(),
            chat: chat::ChatState::default(),
            social: social::SocialState::default(),
            account: account::AccountState::default(),
            unread_marker: None,
            activity: Default::default(),
            quests: Default::default(),
        }
    }
}

impl AppState {
    // ----- lifecycle -------------------------------------------------------

    /// Build state from the disk cache (instant startup). Sets `has_data` if it has guilds/DMs.
    pub fn from_snapshot(snap: Snapshot) -> Self {
        let mut s = Self::default();
        s.replace_core(ReadyData {
            user: snap.me.clone().unwrap_or_default(),
            guilds: snap.guilds,
            private_channels: snap.private_channels,
            users: snap.users,
            my_members: snap.my_members,
            read_states: snap.read_states,
            guild_settings: snap.guild_settings,
            // Not cached; READY brings them.
            relationships: Vec::new(),
            auth_session_id_hash: String::new(),
        });
        s.me = snap.me;
        for (id, mut msgs) in snap.recent_messages {
            // Like `select_channel`: only the recent channels keep messages.
            if !s.channels.contains_key(&id) || msgs.is_empty() || s.recent_channels.len() >= SNAPSHOT_CHANNELS {
                continue;
            }
            msgs.sort_by_key(|m| m.id);
            msgs.dedup_by_key(|m| m.id);
            s.stale_upto.insert(id, msgs[msgs.len() - 1].id);
            // Unknown whether older history exists; a scroll-up fetch settles it.
            s.messages.insert(id, ChannelMessages { msgs, has_more_before: true, loaded: true, ..Default::default() });
            s.recent_channels.push(id);
        }
        s.selected_guild = snap.selected_guild;
        s.selected_channel = snap.selected_channel;
        s.last_channel_per_guild = snap.last_channel_per_guild.into_iter().collect();
        s.has_data = !s.guilds.is_empty() || !s.dm_order.is_empty();
        if s.has_data {
            s.fix_selection();
        }
        s.capture_unread_marker();
        s.dirty = false;
        s
    }

    /// Data to persist to the disk cache (guilds+channels, DMs, users referenced by DMs,
    /// read states, settings, selection, last ~50 messages of up to 5 recently viewed channels).
    pub fn to_snapshot(&self) -> Snapshot {
        let mut by_guild: HashMap<Id, Vec<Channel>> = HashMap::new();
        for c in self.channels.values() {
            // Unjoined threads come back with THREAD_LIST_SYNC, archived ones not at all.
            if let Some(g) = c.guild_id.filter(|_| !social::inactive_thread(c)) {
                by_guild.entry(g).or_default().push(c.clone());
            }
        }
        let guilds = self
            .guilds
            .iter()
            .map(|g| {
                let mut g = g.clone();
                g.channels = by_guild.remove(&g.id).unwrap_or_default();
                g.channels.sort_by_key(|c| c.id);
                g
            })
            .collect();
        let private_channels: Vec<Channel> = self.dm_order.iter().filter_map(|id| self.channels.get(id).cloned()).collect();

        let mut candidates: Vec<Id> = Vec::new();
        for id in self.selected_channel.iter().chain(&self.recent_channels) {
            if !candidates.contains(id) {
                candidates.push(*id);
            }
        }
        let recent_messages: Vec<(Id, Vec<Message>)> = candidates
            .into_iter()
            .filter_map(|id| {
                let msgs = &self.messages.get(&id)?.msgs;
                // MessageLogger's deleted messages stay in memory only.
                let kept = msgs[msgs.len().saturating_sub(SNAPSHOT_MESSAGES)..].iter().filter(|m| !crate::plugins::message_logger::is_deleted(m.id));
                let kept: Vec<Message> = kept.cloned().collect();
                (!kept.is_empty()).then_some((id, kept))
            })
            .take(SNAPSHOT_CHANNELS)
            .collect();

        let mut users: HashMap<Id, User> = HashMap::new();
        for id in private_channels.iter().flat_map(|c| &c.recipient_ids) {
            if let Some(u) = self.users.get(id) {
                users.insert(*id, u.clone());
            }
        }
        for m in recent_messages.iter().flat_map(|(_, msgs)| msgs) {
            users.entry(m.author.id).or_insert_with(|| self.users.get(&m.author.id).unwrap_or(&m.author).clone());
        }
        let mut users: Vec<User> = users.into_values().collect();
        users.sort_by_key(|u| u.id);

        let mut my_members: Vec<(Id, Member)> = self.my_members.iter().map(|(g, m)| (*g, m.clone())).collect();
        my_members.sort_by_key(|(g, _)| *g);
        let mut read_states: Vec<ReadState> = self.read_states.values().cloned().collect();
        read_states.sort_by_key(|r| r.id);
        let mut guild_settings: Vec<GuildSettings> = self.guild_settings.values().cloned().collect();
        guild_settings.sort_by_key(|s| s.guild_id);
        let mut last_channel_per_guild: Vec<(Id, Id)> = self.last_channel_per_guild.iter().map(|(g, c)| (*g, *c)).collect();
        last_channel_per_guild.sort();

        Snapshot {
            version: SNAPSHOT_VERSION,
            me: self.me.clone(),
            users,
            guilds,
            private_channels,
            my_members,
            read_states,
            guild_settings,
            selected_guild: self.selected_guild,
            selected_channel: self.selected_channel,
            last_channel_per_guild,
            recent_messages,
        }
    }

    /// Replace everything with a fresh READY (keeps selection if still valid, keeps loaded
    /// messages of channels that still exist, marks them not fresh).
    pub fn apply_ready(&mut self, ready: ReadyData) {
        let had_data = self.has_data;
        self.replace_core(ready);
        let channels = &self.channels;
        self.messages.retain(|id, _| channels.contains_key(id));
        // A request lost with the old connection would otherwise block refetching forever.
        for m in self.messages.values_mut() {
            m.loading = false;
        }
        self.stale_upto = self.messages.iter().filter_map(|(id, e)| Some((*id, e.msgs.last()?.id))).collect();
        self.fresh_channels.clear();
        self.recent_channels.retain(|id| channels.contains_key(id));
        self.pending.retain(|p| channels.contains_key(&p.channel_id));
        let guilds = &self.guilds;
        self.member_lists.retain(|g, _| guilds.iter().any(|x| x.id == *g));
        self.on_chat_session();
        self.has_data = true;
        if !had_data && self.selected_guild.is_none() && self.selected_channel.is_none() {
            self.select_fallback();
        } else {
            self.fix_selection();
        }
        self.capture_unread_marker();
        self.dirty = true;
    }

    /// Apply a gateway event. Returns a notification when a new message should alert the user.
    /// `focused_channel` = channel currently on screen while the window has focus (no alert,
    /// no unread increment for it).
    pub fn apply_gateway(&mut self, ev: GatewayEvent, focused_channel: Option<Id>) -> Option<Notification> {
        self.count_thread_messages(&ev);
        match ev {
            GatewayEvent::Ready(r) => {
                // A new session: the CALL_CREATEs after READY list the calls again.
                self.calls = calls::CallState::default();
                self.apply_ready(*r)
            }
            GatewayEvent::Resumed => return None,
            GatewayEvent::MessageCreate(m) => return self.on_message_create(*m, focused_channel),
            GatewayEvent::MessageUpdate(u) => self.on_message_update(*u),
            GatewayEvent::MessageDelete { id, channel_id } => self.remove_messages(channel_id, &[id]),
            GatewayEvent::MessageDeleteBulk { ids, channel_id } => self.remove_messages(channel_id, &ids),
            GatewayEvent::ReactionAdd { channel_id, message_id, user_id, emoji, burst } => {
                let me = self.is_me(user_id);
                if let Some(m) = self.message_mut(channel_id, message_id) {
                    add_reaction(&mut m.reactions, emoji, burst, me);
                }
            }
            GatewayEvent::ReactionRemove { channel_id, message_id, user_id, emoji, burst } => {
                let me = self.is_me(user_id);
                if let Some(m) = self.message_mut(channel_id, message_id) {
                    remove_reaction(&mut m.reactions, &emoji, burst, me);
                }
            }
            GatewayEvent::ReactionRemoveAll { channel_id, message_id } => {
                if let Some(m) = self.message_mut(channel_id, message_id) {
                    m.reactions.clear();
                }
            }
            GatewayEvent::ReactionRemoveEmoji { channel_id, message_id, emoji } => {
                if let Some(m) = self.message_mut(channel_id, message_id) {
                    m.reactions.retain(|r| !same_emoji(&r.emoji, &emoji));
                }
            }
            GatewayEvent::MessageAck { channel_id, message_id, mention_count } => {
                let rs = self.read_states.entry(channel_id).or_insert_with(|| ReadState { id: channel_id, ..Default::default() });
                // Set unconditionally: a manual "mark unread" elsewhere moves the ack backwards.
                rs.last_message_id = Some(message_id);
                if let Some(n) = mention_count {
                    rs.mention_count = n;
                }
            }
            GatewayEvent::ChannelCreate(ch) | GatewayEvent::ChannelUpdate(ch) => {
                let (dm, id) = (ch.is_dm_like(), ch.id);
                self.insert_channel(*ch);
                if dm {
                    self.sort_dm_order();
                }
                self.fix_selection();
                self.on_account_created(id);
            }
            GatewayEvent::ChannelDelete { id, .. } => {
                self.remove_channel_data(id);
                self.fix_selection();
            }
            GatewayEvent::GuildCreate(g) => {
                let id = g.id;
                self.on_guild_create(*g);
                self.fix_selection();
                self.on_account_created(id);
            }
            GatewayEvent::GuildUpdate(g) => {
                if let Some(old) = self.guild_mut(g.id) {
                    merge_guild(old, *g);
                }
                self.fix_selection();
            }
            GatewayEvent::GuildDelete { id, unavailable } => {
                self.clear_guild_voice(id);
                if unavailable {
                    if let Some(g) = self.guild_mut(id) {
                        g.unavailable = true;
                    }
                } else {
                    self.remove_guild(id);
                }
                self.fix_selection();
            }
            GatewayEvent::GuildRoleUpsert { guild_id, role } => {
                if let Some(g) = self.guild_mut(guild_id) {
                    match g.roles.iter_mut().find(|r| r.id == role.id) {
                        Some(r) => *r = role,
                        None => g.roles.push(role),
                    }
                }
                self.fix_selection();
            }
            GatewayEvent::GuildRoleDelete { guild_id, role_id } => {
                if let Some(g) = self.guild_mut(guild_id) {
                    g.roles.retain(|r| r.id != role_id);
                }
                if let Some(m) = self.my_members.get_mut(&guild_id) {
                    m.roles.retain(|r| *r != role_id);
                }
                self.fix_selection();
            }
            GatewayEvent::GuildEmojisUpdate { guild_id, emojis } => {
                if let Some(g) = self.guild_mut(guild_id) {
                    g.emojis = emojis;
                }
            }
            GatewayEvent::MyMemberUpdate { guild_id, member } => {
                self.my_members.insert(guild_id, member);
                self.fix_selection();
            }
            GatewayEvent::UserUpdate(u) => {
                if self.is_me(u.id) {
                    self.me = Some(u.clone());
                }
                self.users.insert(u.id, u);
            }
            GatewayEvent::GuildSettingsUpdate(s) => {
                self.guild_settings.insert(s.guild_id, s);
            }
            GatewayEvent::MemberListUpdate(u) => {
                self.on_member_list(*u);
                return None;
            }
            GatewayEvent::ChannelLastMessage { updates } => {
                for (id, last) in updates {
                    if let Some(ch) = self.channels.get_mut(&id) {
                        ch.last_message_id = ch.last_message_id.max(last);
                    }
                }
            }
            GatewayEvent::RelationshipUpsert(r) => {
                if r.hides_author() {
                    self.hidden_authors.insert(r.id);
                } else {
                    self.hidden_authors.remove(&r.id);
                }
                self.on_relationship(r);
                return None;
            }
            GatewayEvent::RelationshipRemove { id } => {
                self.hidden_authors.remove(&id);
                self.social.relationships.remove(&id);
                return None;
            }
            ev @ (GatewayEvent::ThreadUpsert(_)
            | GatewayEvent::ThreadDelete { .. }
            | GatewayEvent::ThreadListSync { .. }
            | GatewayEvent::ThreadMemberUpdate { .. }
            | GatewayEvent::ThreadMembersUpdate { .. }
            | GatewayEvent::ChannelRecipientAdd { .. }
            | GatewayEvent::ChannelRecipientRemove { .. }
            | GatewayEvent::RecentMentionDelete { .. }
            | GatewayEvent::UserNoteUpdate { .. }) => self.on_social_gateway(ev),
            // Not cached: nothing to persist.
            ev @ (GatewayEvent::UserSettingsProto { .. } | GatewayEvent::GuildBanAdd { .. } | GatewayEvent::GuildBanRemove { .. }) => {
                self.on_account_gateway(ev);
                return None;
            }
            GatewayEvent::VoiceStateUpdate(v) => {
                self.on_voice_state(*v);
                return None;
            }
            GatewayEvent::CallUpdate { channel_id, ringing } => return self.on_call_update(channel_id, ringing),
            GatewayEvent::CallDelete { channel_id } => {
                self.on_call_delete(channel_id);
                return None;
            }
            // Routed to the voice module by the gateway; nothing to keep here.
            GatewayEvent::VoiceServerUpdate { .. }
            | GatewayEvent::StreamCreate { .. }
            | GatewayEvent::StreamServerUpdate { .. }
            | GatewayEvent::StreamDelete { .. } => return None,
            GatewayEvent::TypingStart { channel_id, user_id, member, .. } => {
                self.on_typing_start(channel_id, user_id, member, std::time::Instant::now());
                return None;
            }
            GatewayEvent::PresenceUpdate(list) => {
                self.on_presences(list);
                return None;
            }
            GatewayEvent::MyStatus { status, custom_status } => {
                (self.chat.my_status, self.chat.my_custom_status) = (Some(status), custom_status);
                return None;
            }
            GatewayEvent::ChannelPinsUpdate { channel_id, last_pin_timestamp, .. } => {
                self.on_pins_update(channel_id, last_pin_timestamp);
            }
            // Not cached: nothing to persist.
            GatewayEvent::Quests(ev) => {
                self.quests.apply(ev);
                return None;
            }
        }
        self.dirty = true;
        None
    }

    /// Apply a `BackendEvent::Voice`. Returns user-facing text worth a toast.
    pub fn apply_voice(&mut self, ev: crate::voice::VoiceEvent) -> Option<String> {
        use crate::voice::VoiceEvent as V;
        let v = &mut self.voice;
        match ev {
            V::Connecting { channel_id } => (v.channel_id, v.connected) = (Some(channel_id), false),
            V::Connected { channel_id } => (v.channel_id, v.connected) = (Some(channel_id), true),
            V::Disconnected { reason } => {
                (v.channel_id, v.connected) = (None, false);
                v.speaking.clear();
                v.video.end_call();
                return reason;
            }
            V::Speaking { user_id, speaking: true } => _ = v.speaking.insert(user_id),
            V::Speaking { user_id, speaking: false } => _ = v.speaking.remove(&user_id),
            V::LocalLevel(level) => v.level = level,
            V::Error(e) => return Some(e),
            V::Devices { inputs, outputs } => (v.inputs, v.outputs) = (inputs, outputs),
            V::Video(ev) => return v.video.apply(ev),
        }
        None
    }

    /// Merge a history page. `before=None` means "latest page" (replaces/merges with cached).
    pub fn apply_messages_loaded(&mut self, channel_id: Id, before: Option<Id>, result: Result<Vec<Message>, String>, requested: u8) {
        let mut page = match result {
            Ok(page) => page,
            Err(e) => {
                let entry = self.messages.entry(channel_id).or_default();
                entry.loading = false;
                entry.error = Some(e);
                return;
            }
        };
        page.sort_by_key(|m| m.id);
        page.dedup_by_key(|m| m.id);
        for m in &page {
            self.users.insert(m.author.id, m.author.clone());
        }
        if let (None, Some(newest)) = (before, page.last())
            && let Some(ch) = self.channels.get_mut(&channel_id)
        {
            ch.last_message_id = ch.last_message_id.max(Some(newest.id));
        }

        let entry = self.messages.entry(channel_id).or_default();
        entry.loading = false;
        entry.loaded = true;
        entry.error = None;
        entry.has_more_before = page.len() >= requested as usize;
        match before {
            None => {
                // Cached messages older than the page may be stale or separated from it by a
                // gap, so the page replaces them. Only messages newer than the page (delivered
                // by the gateway while the request was in flight) are kept, but not ones held
                // from before READY/the cache: those are newer only because they were deleted.
                let keep_after = page.last().map(|m| m.id).max(self.stale_upto.remove(&channel_id));
                page.extend(entry.msgs.drain(..).filter(|m| keep_after.is_some_and(|n| m.id > n)));
                entry.msgs = page;
                cap_oldest(entry);
            }
            Some(_) => {
                for m in page {
                    insert_sorted(&mut entry.msgs, m);
                }
            }
        }
        self.dirty = true;
    }

    /// Outcome of a SendMessage: on error mark the pending entry failed; on success remove
    /// it and insert the message if the gateway has not delivered it yet.
    pub fn apply_send_result(&mut self, channel_id: Id, nonce: &str, result: Result<Message, String>) {
        self.on_chat_send_result(nonce, result.is_ok());
        match result {
            Err(e) => {
                if let Some(p) = self.pending.iter_mut().find(|p| p.nonce == nonce) {
                    p.failed = Some(e);
                }
            }
            Ok(mut msg) => {
                self.pending.retain(|p| p.nonce != nonce);
                // Channel deleted (or guild left) while the POST was in flight.
                let Some(ch) = self.channels.get(&channel_id) else { return };
                // REST messages lack guild_id; without it an unknown channel would become a DM.
                msg.guild_id = ch.guild_id;
                if msg.channel_id.is_zero() {
                    msg.channel_id = channel_id;
                }
                // Treated as focused: my own message never alerts or bumps mentions.
                self.on_message_create(msg, Some(channel_id));
            }
        }
    }

    /// Clear everything (logout).
    pub fn reset(&mut self) {
        // This device's games stay; the session's presence and friends' app icons go.
        let activity = std::mem::take(&mut self.activity).after_logout();
        *self = Self { activity, ..Self::default() };
    }

    // ----- selection ---------------------------------------------------------

    /// Switch guild (None = DMs) and restore its last channel (or the first readable one).
    pub fn select_guild(&mut self, guild: Option<Id>) {
        self.selected_guild = guild;
        let remembered = self
            .last_channel_per_guild
            .get(&guild.unwrap_or(DM_HOME))
            .copied()
            .filter(|c| self.channels.get(c).is_some_and(|ch| ch.guild_id == guild) && self.can_view(*c));
        match remembered.or_else(|| guild.and_then(|g| self.first_readable_channel(g))) {
            Some(c) => self.select_channel(c),
            None => {
                self.selected_channel = None;
                self.dirty = true;
            }
        }
    }

    pub fn select_channel(&mut self, channel: Id) {
        let Some(guild) = self.channels.get(&channel).map(|c| c.guild_id) else { return };
        // Leaving a channel marked unread resumes its automatic acks.
        if self.chat.ack_paused.is_some_and(|c| c != channel) {
            self.chat.ack_paused = None;
        }
        // History paged in is kept only while its channel is open.
        if let Some(prev) = self.selected_channel.filter(|p| *p != channel) {
            self.trim_history(prev);
        }
        let changed = self.selected_channel != Some(channel);
        self.selected_guild = guild;
        self.selected_channel = Some(channel);
        if changed {
            self.capture_unread_marker();
        }
        self.last_channel_per_guild.insert(guild.unwrap_or(DM_HOME), channel);
        self.recent_channels.retain(|c| *c != channel);
        self.recent_channels.insert(0, channel);
        self.recent_channels.truncate(SNAPSHOT_CHANNELS);
        // Bounds memory to the recent channels; a revisit refetches (user-triggered). Threads of
        // the opened channel keep theirs one step longer: a forum's cards show their first messages.
        let (recent, channels) = (&self.recent_channels, &self.channels);
        self.messages.retain(|id, _| recent.contains(id) || channels.get(id).is_some_and(|c| c.is_thread() && c.parent_id == Some(channel)));
        self.fresh_channels.retain(|id| recent.contains(id));
        self.dirty = true;
    }

    /// Sets `unread_marker` from the open channel's read state (before any ack of this visit).
    fn capture_unread_marker(&mut self) {
        self.unread_marker = self.selected_channel.and_then(|ch| {
            let read = self.read_states.get(&ch)?.last_message_id.filter(|r| !r.is_zero())?;
            (self.channels.get(&ch)?.last_message_id? > read).then_some((ch, read))
        });
    }

    /// Drops the oldest messages beyond MAX_MESSAGES_PER_CHANNEL. For the open channel, call
    /// it only while the view is at the bottom, so the history being read stays put.
    pub fn trim_history(&mut self, channel_id: Id) {
        if let Some(e) = self.messages.get_mut(&channel_id) {
            cap_oldest(e);
        }
    }

    // ----- permissions -------------------------------------------------------

    /// Effective permissions of the current user in a channel (all bits for DMs).
    pub fn channel_permissions(&self, channel_id: Id) -> u64 {
        let Some(ch) = self.channels.get(&channel_id) else { return 0 };
        if ch.is_dm_like() {
            return perm::ALL;
        }
        let Some(guild) = ch.guild_id.and_then(|g| self.guild(g)) else { return 0 };
        self.perms_in(guild, self.base_permissions(guild), ch)
    }

    pub fn can_view(&self, channel_id: Id) -> bool {
        self.channel_permissions(channel_id) & perm::VIEW_CHANNEL != 0
    }

    pub fn can_send(&self, channel_id: Id) -> bool {
        let p = self.channel_permissions(channel_id);
        p & perm::VIEW_CHANNEL != 0 && p & perm::SEND_MESSAGES != 0
    }

    pub fn can_react(&self, channel_id: Id) -> bool {
        let p = self.channel_permissions(channel_id);
        p & perm::VIEW_CHANNEL != 0 && p & perm::ADD_REACTIONS != 0
    }

    // ----- sidebar -----------------------------------------------------------

    /// Visible channels of a guild grouped under categories, Discord sort order
    /// (uncategorized first, then categories by position; text-like before voice-like).
    /// Empty categories (no visible children) are omitted.
    pub fn sidebar_channels(&self, guild_id: Id) -> Vec<SidebarItem> {
        self.sidebar_channels_with(guild_id, false)
    }

    /// `sidebar_channels`; with `hidden` also the channels I can't view (the ShowHiddenChannels
    /// plugin lists them locked; display only, never selected).
    pub fn sidebar_channels_with(&self, guild_id: Id, hidden: bool) -> Vec<SidebarItem> {
        let Some(guild) = self.guild(guild_id) else { return Vec::new() };
        let base = self.base_permissions(guild);
        let mut cats: Vec<&Channel> = Vec::new();
        let mut kids: Vec<&Channel> = Vec::new();
        for c in self.channels.values().filter(|c| c.guild_id == Some(guild_id)) {
            if c.kind == channel_type::GUILD_CATEGORY {
                cats.push(c);
            } else if is_sidebar_kind(c.kind) && (hidden || self.perms_in(guild, base, c) & perm::VIEW_CHANNEL != 0) {
                kids.push(c);
            }
        }
        cats.sort_by_key(|c| (c.position, c.id));
        kids.sort_by_key(|c| (sort_bucket(c.kind), c.position, c.id));

        let is_cat = |id: Id| cats.iter().any(|c| c.id == id);
        let mut out: Vec<SidebarItem> = kids
            .iter()
            .filter(|c| !c.parent_id.is_some_and(is_cat))
            .map(|c| SidebarItem::Channel { id: c.id })
            .collect();
        for cat in &cats {
            let children: Vec<SidebarItem> = kids
                .iter()
                .filter(|c| c.parent_id == Some(cat.id))
                .map(|c| SidebarItem::Channel { id: c.id })
                .collect();
            if !children.is_empty() {
                out.push(SidebarItem::Category { id: cat.id, name: cat.name.clone().unwrap_or_default() });
                out.extend(children);
            }
        }
        out
    }

    /// Display name of a channel (DMs: recipient names joined).
    pub fn channel_name(&self, channel_id: Id) -> String {
        let Some(ch) = self.channels.get(&channel_id) else { return "unknown channel".into() };
        if let Some(name) = ch.name.as_deref().filter(|n| !n.is_empty()) {
            return name.to_string();
        }
        if !ch.is_dm_like() {
            return "unnamed".into();
        }
        let names: Vec<&str> = ch
            .recipient_ids
            .iter()
            .filter(|id| !self.is_me(**id))
            .filter_map(|id| self.users.get(id))
            .map(|u| u.display_name())
            .collect();
        if names.is_empty() { "Unnamed conversation".into() } else { names.join(", ") }
    }

    // ----- unread / mentions -------------------------------------------------

    pub fn is_unread(&self, channel_id: Id) -> bool {
        // No read state: treated as read (see PROTOCOL_NOTES part 2 §5).
        let Some(rs) = self.read_states.get(&channel_id) else { return false };
        if rs.mention_count > 0 {
            return true;
        }
        if self.is_channel_muted(channel_id) {
            return false;
        }
        let last = self.channels.get(&channel_id).and_then(|c| c.last_message_id);
        match (last, rs.last_message_id) {
            (Some(last), Some(read)) if !read.is_zero() => last > read,
            _ => false,
        }
    }

    pub fn mention_count(&self, channel_id: Id) -> u32 {
        self.read_states.get(&channel_id).map_or(0, |rs| rs.mention_count)
    }

    /// (any unread non-muted channel, total mentions) for a guild's icon badge.
    /// The UI uses `guild_unreads` (one pass for the whole guild bar).
    #[cfg(test)]
    pub fn guild_unread(&self, guild_id: Id) -> (bool, u32) {
        self.unread_pass(Some(guild_id)).remove(&guild_id).unwrap_or_default()
    }

    /// `guild_unread` of every guild with a badge, in one pass over the read states.
    pub fn guild_unreads(&self) -> HashMap<Id, (bool, u32)> {
        self.unread_pass(None)
    }

    /// Total mentions/unread DMs for the home button badge.
    pub fn dm_unread_count(&self) -> u32 {
        self.dm_order
            .iter()
            .filter(|&&id| !self.is_message_request(id))
            .map(|&id| match self.mention_count(id) {
                0 => self.is_unread(id) as u32,
                n => n,
            })
            .sum()
    }

    pub fn is_channel_muted(&self, channel_id: Id) -> bool {
        let Some(ch) = self.channels.get(&channel_id) else { return false };
        // A joined thread's own mute lives on my thread member.
        if ch.member.as_ref().is_some_and(|m| mute_active(m.muted, &m.mute_config)) || self.thread_parent_muted(ch) {
            return true;
        }
        let Some(s) = self.guild_settings.get(&ch.guild_id) else { return false };
        // The DM entry's own mute silences every DM.
        if ch.guild_id.is_none() && mute_active(s.muted, &s.mute_config) {
            return true;
        }
        // Own override or the parent's (category, or the channel of a thread).
        [Some(ch.id), ch.parent_id]
            .into_iter()
            .flatten()
            .any(|cid| s.channel_overrides.iter().any(|o| o.channel_id == cid && mute_active(o.muted, &o.mute_config)))
    }

    pub fn is_guild_muted(&self, guild_id: Id) -> bool {
        self.guild_settings.get(&Some(guild_id)).is_some_and(|s| mute_active(s.muted, &s.mute_config))
    }

    /// Mark a channel read locally. Returns the message id to ack on Discord's side if the
    /// read state actually advanced.
    pub fn mark_read(&mut self, channel_id: Id) -> Option<Id> {
        // The user marked it unread (`mark_unread`): no automatic ack until they leave or send.
        if self.chat.ack_paused == Some(channel_id) {
            return None;
        }
        let last = self.channels.get(&channel_id)?.last_message_id?;
        let rs = self.read_states.entry(channel_id).or_insert_with(|| ReadState { id: channel_id, ..Default::default() });
        if rs.last_message_id.is_some_and(|r| r >= last) && rs.mention_count == 0 {
            return None;
        }
        let target = rs.last_message_id.map_or(last, |r| r.max(last));
        rs.last_message_id = Some(target);
        rs.mention_count = 0;
        self.dirty = true;
        Some(target)
    }

    // ----- display helpers -----------------------------------------------------

    /// Nick > global name > username.
    pub fn member_display_name(&self, guild_id: Option<Id>, user: &User, member: Option<&Member>) -> String {
        let member = member.or_else(|| guild_id.and_then(|g| self.find_member(g, user.id)));
        match member.and_then(|m| m.nick.as_deref()) {
            Some(nick) if !nick.is_empty() => nick.to_string(),
            _ => user.display_name().to_string(),
        }
    }

    /// Cached member object of a user: mine, or one from the guild's member list.
    pub fn find_member(&self, guild_id: Id, user_id: Id) -> Option<&Member> {
        if self.is_me(user_id) {
            return self.my_members.get(&guild_id);
        }
        self.member_lists.get(&guild_id)?.items.iter().find_map(|it| match it {
            MemberListItem::Member { member, .. } if member.user.as_ref().is_some_and(|u| u.id == user_id) => Some(member),
            _ => None,
        })
    }

    /// Color (0xRRGGBB) of the highest colored role, if any (none while Settings >
    /// Accessibility hides role colours).
    pub fn role_color(&self, guild_id: Id, roles: &[Id]) -> Option<u32> {
        if !crate::config::role_colors() {
            return None;
        }
        // Equal positions: the lower id ranks higher.
        self.guild(guild_id)?
            .roles
            .iter()
            .filter(|r| r.color != 0 && roles.contains(&r.id))
            .max_by_key(|r| (r.position, Reverse(r.id)))
            .map(|r| r.color)
    }

    pub fn guild(&self, guild_id: Id) -> Option<&Guild> {
        self.guilds.iter().find(|g| g.id == guild_id)
    }

    /// Does this message mention me (direct, reply-to-me, role I have, @everyone/@here
    /// unless suppressed)?
    pub fn mentions_me(&self, msg: &Message) -> bool {
        let Some(me) = self.me.as_ref().map(|u| u.id) else { return false };
        // Pinging replies put the replied-to author into `mentions`.
        if msg.mentions.iter().any(|u| u.id == me) {
            return true;
        }
        let guild_id = msg.guild_id.or_else(|| self.channels.get(&msg.channel_id).and_then(|c| c.guild_id));
        let settings = self.guild_settings.get(&guild_id);
        if msg.mention_everyone && !settings.is_some_and(|s| s.suppress_everyone) {
            return true;
        }
        !settings.is_some_and(|s| s.suppress_roles)
            && guild_id.is_some_and(|g| self.my_roles(g).iter().any(|r| msg.mention_roles.contains(r)))
    }

    // ----- private helpers -------------------------------------------------------

    fn is_me(&self, user_id: Id) -> bool {
        self.me.as_ref().is_some_and(|u| u.id == user_id)
    }

    fn guild_mut(&mut self, guild_id: Id) -> Option<&mut Guild> {
        self.guilds.iter_mut().find(|g| g.id == guild_id)
    }

    fn my_roles(&self, guild_id: Id) -> &[Id] {
        self.my_members.get(&guild_id).map_or(&[], |m| &m.roles)
    }

    /// Guild badges (unread, mentions) from one scan of the read states, optionally for one guild.
    fn unread_pass(&self, only: Option<Id>) -> HashMap<Id, (bool, u32)> {
        // Per guild: (guild, base permissions, guild muted), computed once.
        let mut guilds: HashMap<Id, Option<(&Guild, u64, bool)>> = HashMap::new();
        let mut out: HashMap<Id, (bool, u32)> = HashMap::new();
        for rs in self.read_states.values() {
            let Some(ch) = self.channels.get(&rs.id) else { continue };
            let Some(gid) = ch.guild_id.filter(|g| only.is_none_or(|o| o == *g)) else { continue };
            // Only joined, active threads have badges; forum/media/directory channels cannot be
            // opened (or marked read) here, so they must not light up the guild forever.
            if social::inactive_thread(ch) || !ch.is_text_chat() {
                continue;
            }
            let info = *guilds
                .entry(gid)
                .or_insert_with(|| self.guild(gid).map(|g| (g, self.base_permissions(g), self.is_guild_muted(gid))));
            let Some((guild, base, guild_muted)) = info else { continue };
            if self.perms_in(guild, base, ch) & perm::VIEW_CHANNEL == 0 {
                continue;
            }
            let badge = out.entry(gid).or_default();
            badge.1 += rs.mention_count;
            badge.0 |= !guild_muted && !self.is_channel_muted(ch.id) && self.is_unread(ch.id);
        }
        out
    }

    /// Shared by READY and the disk cache: replace guilds, channels, DMs, read states, settings.
    /// Users are merged (message authors seen earlier stay known).
    fn replace_core(&mut self, r: ReadyData) {
        for u in r.users {
            self.users.insert(u.id, u);
        }
        if !r.user.id.is_zero() {
            self.users.insert(r.user.id, r.user.clone());
        }
        self.me = Some(r.user);
        self.channels.clear();
        self.dm_order.clear();
        self.voice_states.clear();
        let guilds: Vec<Guild> = r.guilds.into_iter().map(|g| self.ingest_guild(g)).collect();
        self.guilds = guilds;
        for ch in r.private_channels {
            self.insert_channel(ch);
        }
        self.sort_dm_order();
        self.my_members = r.my_members.into_iter().collect();
        self.read_states = r.read_states.into_iter().filter(|rs| rs.read_state_type == 0).map(|rs| (rs.id, rs)).collect();
        self.guild_settings = r.guild_settings.into_iter().map(|s| (s.guild_id, s)).collect();
        self.hidden_authors = r.relationships.iter().filter(|r| r.hides_author()).map(|r| r.id).collect();
        self.set_relationships(r.relationships);
        self.account.session_hash = r.auth_session_id_hash;
    }

    /// Moves a guild's channels into `self.channels` and its voice states (a complete list,
    /// replacing the guild's old ones) into `self.voice_states`; returns the guild without them.
    fn ingest_guild(&mut self, mut g: Guild) -> Guild {
        for mut ch in std::mem::take(&mut g.channels) {
            // Channels inside guild payloads may lack guild_id.
            ch.guild_id = Some(g.id);
            self.insert_channel(ch);
        }
        let states = std::mem::take(&mut g.voice_states);
        if !g.unavailable {
            self.clear_guild_voice(g.id);
            for v in states {
                self.on_voice_state(VoiceState { guild_id: Some(g.id), ..v });
            }
        }
        g
    }

    /// One VOICE_STATE_UPDATE: a user is in at most one channel per guild (per DM call
    /// scope when `guild_id` is None), so the previous entry goes first.
    fn on_voice_state(&mut self, mut v: VoiceState) {
        // The user object lives in `users` (as for DM recipients), not twice in memory.
        if let Some(u) = v.member.as_mut().and_then(|m| m.user.take()) {
            self.users.insert(u.id, u);
        }
        let (guild, user) = (v.guild_id, v.user_id);
        self.voice_states.retain(|_, users| {
            if users.get(&user).is_some_and(|old| old.guild_id == guild) {
                users.remove(&user);
            }
            !users.is_empty()
        });
        if let Some(ch) = v.channel_id {
            self.voice_states.entry(ch).or_default().insert(user, v);
        }
    }

    fn clear_guild_voice(&mut self, guild_id: Id) {
        self.voice_states.retain(|_, users| {
            users.retain(|_, v| v.guild_id != Some(guild_id));
            !users.is_empty()
        });
    }

    /// Insert or update a channel: DM recipients move into `users`, and fields an update
    /// does not carry (last_message_id, guild_id, recipients) are kept.
    fn insert_channel(&mut self, mut ch: Channel) {
        for u in std::mem::take(&mut ch.recipients) {
            if !ch.recipient_ids.contains(&u.id) {
                ch.recipient_ids.push(u.id);
            }
            self.users.insert(u.id, u);
        }
        if let Some(old) = self.channels.get(&ch.id) {
            ch.last_message_id = ch.last_message_id.max(old.last_message_id);
            if ch.guild_id.is_none() {
                ch.guild_id = old.guild_id;
            }
            if ch.recipient_ids.is_empty() {
                ch.recipient_ids = old.recipient_ids.clone();
            }
        }
        if ch.is_dm_like() && !self.dm_order.contains(&ch.id) {
            self.dm_order.push(ch.id);
        }
        self.channels.insert(ch.id, ch);
    }

    fn sort_dm_order(&mut self) {
        let channels = &self.channels;
        self.dm_order.sort_by_key(|id| Reverse(channels.get(id).map(|c| c.last_message_id.unwrap_or(c.id))));
    }

    fn remove_channel_data(&mut self, id: Id) {
        self.channels.remove(&id);
        self.voice_states.remove(&id);
        self.messages.remove(&id);
        self.read_states.remove(&id);
        self.fresh_channels.remove(&id);
        self.dm_order.retain(|c| *c != id);
        self.recent_channels.retain(|c| *c != id);
        self.last_channel_per_guild.retain(|_, c| *c != id);
        self.pending.retain(|p| p.channel_id != id);
    }

    fn remove_guild(&mut self, id: Id) {
        self.guilds.retain(|g| g.id != id);
        let chans: Vec<Id> = self.channels.values().filter(|c| c.guild_id == Some(id)).map(|c| c.id).collect();
        for c in chans {
            self.remove_channel_data(c);
        }
        self.my_members.remove(&id);
        self.member_lists.remove(&id);
        self.guild_settings.remove(&Some(id));
        self.last_channel_per_guild.remove(&id);
    }

    fn on_guild_create(&mut self, g: Guild) {
        let gid = g.id;
        if !g.channels.is_empty() {
            let vanished: Vec<Id> = self
                .channels
                .values()
                .filter(|c| c.guild_id == Some(gid) && !g.channels.iter().any(|n| n.id == c.id))
                .map(|c| c.id)
                .collect();
            for id in vanished {
                self.remove_channel_data(id);
            }
        }
        let g = self.ingest_guild(g);
        match self.guild_mut(gid) {
            Some(old) if g.unavailable => old.unavailable = true,
            Some(old) => merge_guild(old, g),
            None => self.guilds.push(g),
        }
    }

    /// Repairs the selection after data changed: a vanished guild falls back to the first
    /// guild with a readable channel (or DM home), a vanished/unreadable channel to the
    /// guild's remembered or first readable channel.
    fn fix_selection(&mut self) {
        if self.selected_guild.is_some_and(|g| self.guild(g).is_none()) {
            self.select_fallback();
            return;
        }
        let ok = match self.selected_channel {
            Some(c) => self.channels.get(&c).is_some_and(|ch| ch.guild_id == self.selected_guild) && self.can_view(c),
            // DM home with nothing open is a valid state.
            None => self.selected_guild.is_none(),
        };
        if !ok {
            self.select_guild(self.selected_guild);
        }
    }

    fn select_fallback(&mut self) {
        let first = self.guilds.iter().map(|g| g.id).find(|g| self.first_readable_channel(*g).is_some());
        self.select_guild(first);
    }

    /// First visible text channel in sidebar order, else any visible chat channel.
    fn first_readable_channel(&self, guild_id: Id) -> Option<Id> {
        let ids: Vec<&Channel> = self
            .sidebar_channels(guild_id)
            .into_iter()
            .filter_map(|it| match it {
                SidebarItem::Channel { id } => self.channels.get(&id),
                SidebarItem::Category { .. } => None,
            })
            .collect();
        ids.iter()
            .find(|c| matches!(c.kind, channel_type::GUILD_TEXT | channel_type::GUILD_ANNOUNCEMENT))
            .or_else(|| ids.iter().find(|c| c.is_text_chat()))
            .map(|c| c.id)
    }

    /// Guild-level permissions: everything for the owner or ADMINISTRATOR, else
    /// @everyone (role id == guild id) | my roles.
    fn base_permissions(&self, guild: &Guild) -> u64 {
        let Some(me) = self.me.as_ref().map(|u| u.id) else { return 0 };
        if guild.owner_id == me {
            return perm::ALL;
        }
        let my_roles = self.my_roles(guild.id);
        let p = guild
            .roles
            .iter()
            .filter(|r| r.id == guild.id || my_roles.contains(&r.id))
            .fold(0, |p, r| p | r.permissions.0);
        if p & perm::ADMINISTRATOR != 0 { perm::ALL } else { p }
    }

    /// Channel permissions from precomputed guild base permissions (0 without VIEW_CHANNEL).
    fn perms_in(&self, guild: &Guild, base: u64, ch: &Channel) -> u64 {
        if is_thread(ch.kind) {
            let parent = ch.parent_id.and_then(|p| self.channels.get(&p)).filter(|p| !is_thread(p.kind));
            let Some(parent) = parent else { return 0 };
            let p = self.perms_in(guild, base, parent);
            // Threads gate sending on SEND_MESSAGES_IN_THREADS instead of SEND_MESSAGES.
            return if p & perm::SEND_MESSAGES_IN_THREADS != 0 { p | perm::SEND_MESSAGES } else { p & !perm::SEND_MESSAGES };
        }
        if base & perm::ADMINISTRATOR != 0 {
            return perm::ALL;
        }
        let me = self.me.as_ref().map_or(Id(0), |u| u.id);
        let mut p = apply_overwrites(base, &ch.permission_overwrites, guild.id, self.my_roles(guild.id), me);
        // Timed out (owner/admins returned above): only viewing and reading history remain.
        if self.my_members.get(&guild.id).is_some_and(Member::timed_out) {
            p &= perm::VIEW_CHANNEL | perm::READ_MESSAGE_HISTORY;
        }
        if p & perm::VIEW_CHANNEL == 0 { 0 } else { p }
    }

    fn message_mut(&mut self, channel_id: Id, id: Id) -> Option<&mut Message> {
        let msgs = &mut self.messages.get_mut(&channel_id)?.msgs;
        let i = msgs.binary_search_by_key(&id, |m| m.id).ok()?;
        Some(&mut msgs[i])
    }

    fn remove_messages(&mut self, channel_id: Id, ids: &[Id]) {
        if let Some(e) = self.messages.get_mut(&channel_id) {
            e.msgs.retain(|m| !ids.contains(&m.id));
        }
    }

    fn on_message_create(&mut self, msg: Message, focused: Option<Id>) -> Option<Notification> {
        let cid = msg.channel_id;
        self.on_chat_message(cid, msg.author.id);
        if let Some(nonce) = msg.nonce_str() {
            self.pending.retain(|p| p.nonce != nonce);
        }
        let mine = self.is_me(msg.author.id);
        if !self.channels.contains_key(&cid) {
            // Unknown guild channels (e.g. threads we are not in) have nowhere to show.
            if msg.guild_id.is_some() {
                return None;
            }
            let recipient_ids = if mine { Vec::new() } else { vec![msg.author.id] };
            self.insert_channel(Channel { id: cid, kind: channel_type::DM, recipient_ids, ..Default::default() });
        }
        if !mine {
            self.users.insert(msg.author.id, msg.author.clone());
        }

        let focused = focused == Some(cid);
        // Blocked/ignored authors only move last_message_id (PROTOCOL_NOTES, MESSAGE_CREATE step 3).
        let hidden = !mine && self.hidden_authors.contains(&msg.author.id);
        let mentioned = !mine && !hidden && self.mentions_me(&msg);
        let notification =
            (!mine && !hidden && !focused && self.should_notify(&msg, mentioned)).then(|| self.build_notification(&msg));
        let muted = self.is_channel_muted(cid);

        let ch = self.channels.get_mut(&cid).expect("channel inserted above");
        ch.last_message_id = ch.last_message_id.max(Some(msg.id));
        let is_dm = ch.is_dm_like();
        if is_dm {
            self.dm_order.retain(|c| *c != cid);
            self.dm_order.insert(0, cid);
        }

        if mine {
            if msg.kind != MSG_POLL_RESULT {
                let rs = self.read_states.entry(cid).or_insert_with(|| ReadState { id: cid, ..Default::default() });
                rs.last_message_id = rs.last_message_id.max(Some(msg.id));
                rs.mention_count = 0;
            }
        } else if !focused && !hidden {
            let counts = if is_dm { msg.kind != MSG_RECIPIENT_REMOVE && (!muted || mentioned) } else { mentioned };
            // No read state yet: everything before this message counts as read.
            let rs = self.read_states.entry(cid).or_insert_with(|| ReadState {
                id: cid,
                last_message_id: Some(Id(msg.id.0.saturating_sub(1))),
                ..Default::default()
            });
            if counts {
                rs.mention_count += 1;
            }
        }

        let open = self.selected_channel == Some(cid);
        if let Some(e) = self.messages.get_mut(&cid).filter(|e| e.loaded || e.loading) {
            // Beyond the cap in the open channel = history the user paged in and may be
            // reading; trimming it would jump the view and refetch it (see `trim_history`).
            let paged_back = open && e.msgs.len() > MAX_MESSAGES_PER_CHANNEL;
            if insert_sorted(&mut e.msgs, msg) && !paged_back {
                cap_oldest(e);
            }
        }
        self.dirty = true;
        notification
    }

    /// Desktop-notification decision (PROTOCOL_NOTES part 2 §6); author/focus checked by caller.
    fn should_notify(&self, msg: &Message, mentioned: bool) -> bool {
        let Some(ch) = self.channels.get(&msg.channel_id) else { return false };
        if msg.flags & FLAG_SUPPRESS_NOTIFICATIONS != 0 || social::silenced(ch, mentioned) || !self.can_view(ch.id) {
            return false;
        }
        let level = self.notification_level(ch);
        if level == NOTIFY_NOTHING {
            return false;
        }
        if ch.is_dm_like() {
            return !self.is_channel_muted(ch.id) && (level == NOTIFY_ALL || mentioned);
        }
        if mentioned {
            return true;
        }
        if self.is_channel_muted(ch.id) || ch.guild_id.is_some_and(|g| self.is_guild_muted(g)) {
            return false;
        }
        level == NOTIFY_ALL
    }

    /// Channel override > parent (category) override > guild entry > guild default.
    /// DMs: their override in the DM entry, else all messages. Threads: `thread_notification_level`.
    fn notification_level(&self, ch: &Channel) -> u8 {
        if let Some(level) = self.thread_notification_level(ch) {
            return level;
        }
        if let Some(s) = self.guild_settings.get(&ch.guild_id) {
            for cid in [Some(ch.id), ch.parent_id].into_iter().flatten() {
                if let Some(o) = s.channel_overrides.iter().find(|o| o.channel_id == cid)
                    && o.message_notifications != NOTIFY_INHERIT
                {
                    return o.message_notifications;
                }
            }
            if ch.guild_id.is_some() && s.message_notifications != NOTIFY_INHERIT {
                return s.message_notifications;
            }
        }
        match ch.guild_id {
            Some(g) => self.guild(g).map_or(NOTIFY_ALL, |g| g.default_message_notifications),
            None => NOTIFY_ALL,
        }
    }

    fn build_notification(&self, msg: &Message) -> Notification {
        let ch = self.channels.get(&msg.channel_id);
        let guild_id = ch.and_then(|c| c.guild_id);
        let author = self.member_display_name(guild_id, &msg.author, msg.member.as_ref());
        let title = match ch {
            Some(c) if guild_id.is_some() => {
                let guild = guild_id.and_then(|g| self.guild(g)).map_or("", |g| g.name.as_str());
                format!("{author} (#{}, {guild})", self.channel_name(c.id))
            }
            Some(c) if c.kind == channel_type::GROUP_DM => format!("{author} ({})", self.channel_name(c.id)),
            _ => author,
        };
        Notification { channel_id: msg.channel_id, title, body: notification_body(msg), author_id: Some(msg.author.id) }
    }

    fn on_message_update(&mut self, u: MessageUpdate) {
        let Some(m) = self.message_mut(u.channel_id, u.id) else { return };
        if let Some(v) = u.content {
            m.content = v;
        }
        if u.edited_timestamp.is_some() {
            m.edited_timestamp = u.edited_timestamp;
        }
        if let Some(v) = u.flags {
            m.flags = v;
        }
        if let Some(v) = u.mentions {
            m.mentions = v;
        }
        if let Some(v) = u.mention_roles {
            m.mention_roles = v;
        }
        if let Some(v) = u.mention_everyone {
            m.mention_everyone = v;
        }
        if let Some(v) = u.attachments {
            m.attachments = v;
        }
        if let Some(v) = u.embeds {
            m.embeds = v;
        }
        if let Some(v) = u.pinned {
            m.pinned = v;
        }
    }

    fn on_member_list(&mut self, u: MemberListUpdate) {
        for op in &u.ops {
            let items: &[MemberListItem] = match op {
                MemberListOp::Sync { items, .. } => items,
                MemberListOp::Insert { item, .. } | MemberListOp::Update { item, .. } => std::slice::from_ref(item),
                MemberListOp::Delete { .. } | MemberListOp::Invalidate { .. } => &[],
            };
            for it in items {
                if let MemberListItem::Member { member: Member { user: Some(user), .. }, .. } = it {
                    self.users.insert(user.id, user.clone());
                }
            }
        }
        let selected = self.selected_channel.filter(|c| self.channels.get(c).is_some_and(|ch| ch.guild_id == Some(u.guild_id)));
        let other_list = self.member_lists.get(&u.guild_id).is_none_or(|l| l.list_id != u.list_id);
        // Only a SYNC starts a list; anything else for another list id is a late update for a
        // list we no longer show and must not wipe the current one.
        if other_list && !u.ops.iter().any(|op| matches!(op, MemberListOp::Sync { .. })) {
            return;
        }
        let list = self.member_lists.entry(u.guild_id).or_default();
        if other_list {
            *list = MemberList { list_id: u.list_id, ..Default::default() };
        }
        if selected.is_some() {
            list.channel_id = selected;
        }
        // Counts of 0 can mean "not sent".
        if u.member_count > 0 {
            list.member_count = u.member_count;
        }
        if u.online_count > 0 {
            list.online_count = u.online_count;
        }
        for op in u.ops {
            apply_list_op(&mut list.items, op);
        }
        // `groups` is the complete current group list; header counts follow it.
        for it in &mut list.items {
            if let MemberListItem::Group { id, count } = it
                && let Some((_, c)) = u.groups.iter().find(|(g, _)| g == id)
            {
                *count = *c;
            }
        }
    }
}

// ----- pure helpers ---------------------------------------------------------------

fn is_thread(kind: u8) -> bool {
    matches!(kind, channel_type::ANNOUNCEMENT_THREAD | channel_type::PUBLIC_THREAD | channel_type::PRIVATE_THREAD)
}

/// Non-thread guild channel types shown in the sidebar (unknown/dead types are ignored).
fn is_sidebar_kind(kind: u8) -> bool {
    use channel_type::*;
    matches!(kind, GUILD_TEXT | GUILD_VOICE | GUILD_ANNOUNCEMENT | GUILD_STAGE_VOICE | GUILD_DIRECTORY | GUILD_FORUM | GUILD_MEDIA)
}

/// Sidebar bucket: text-like above voice-like; directories get their own bucket.
fn sort_bucket(kind: u8) -> u8 {
    use channel_type::*;
    match kind {
        GUILD_VOICE | GUILD_STAGE_VOICE => 2,
        GUILD_TEXT | GUILD_ANNOUNCEMENT | GUILD_FORUM | GUILD_MEDIA => 0,
        k => k,
    }
}

/// Overwrite order: @everyone, then all my roles (denies, then allows), then my member entry.
fn apply_overwrites(base: u64, ows: &[Overwrite], everyone: Id, my_roles: &[Id], me: Id) -> u64 {
    let mut p = base;
    if let Some(o) = ows.iter().find(|o| o.id == everyone) {
        p = (p & !o.deny.0) | o.allow.0;
    }
    let (allow, deny) = ows
        .iter()
        .filter(|o| o.kind == 0 && o.id != everyone && my_roles.contains(&o.id))
        .fold((0, 0), |(a, d), o| (a | o.allow.0, d | o.deny.0));
    p = (p & !deny) | allow;
    if let Some(o) = ows.iter().find(|o| o.kind == 1 && o.id == me) {
        p = (p & !o.deny.0) | o.allow.0;
    }
    p
}

/// Effective mute: muted and the mute has no end time or ends in the future.
fn mute_active(muted: bool, cfg: &Option<MuteConfig>) -> bool {
    let end = cfg.as_ref().and_then(|c| c.end_time.as_deref());
    muted
        && end.is_none_or(|t| {
            // An unparsable end time is treated as indefinite.
            chrono::DateTime::parse_from_rfc3339(t)
                .map_or(true, |end| end.timestamp_millis() > chrono::Utc::now().timestamp_millis())
        })
}

/// Inserts keeping ascending id order; returns false (keeping the existing copy) on duplicates.
fn insert_sorted(msgs: &mut Vec<Message>, m: Message) -> bool {
    match msgs.binary_search_by_key(&m.id, |x| x.id) {
        Ok(_) => false,
        Err(i) => {
            msgs.insert(i, m);
            true
        }
    }
}

fn cap_oldest(e: &mut ChannelMessages) {
    if e.msgs.len() > MAX_MESSAGES_PER_CHANNEL {
        e.msgs.drain(..e.msgs.len() - MAX_MESSAGES_PER_CHANNEL);
        e.has_more_before = true;
    }
}

/// Merges a GUILD_UPDATE / repeated GUILD_CREATE onto the cached guild without wiping
/// lists the payload did not carry.
fn merge_guild(old: &mut Guild, new: Guild) {
    if !new.name.is_empty() {
        old.name = new.name;
    }
    old.icon = new.icon;
    if !new.owner_id.is_zero() {
        old.owner_id = new.owner_id;
    }
    if !new.roles.is_empty() {
        old.roles = new.roles;
    }
    if !new.emojis.is_empty() {
        old.emojis = new.emojis;
    }
    old.default_message_notifications = new.default_message_notifications;
    old.premium_tier = new.premium_tier;
    old.features = new.features;
    old.premium_subscription_count = new.premium_subscription_count;
    old.mfa_level = new.mfa_level;
    old.rules_channel_id = new.rules_channel_id;
    old.unavailable = false;
}

/// Custom emoji match by id, unicode emoji by name.
fn same_emoji(a: &Emoji, b: &Emoji) -> bool {
    match (a.id, b.id) {
        (Some(x), Some(y)) => x == y,
        (None, None) => a.name == b.name,
        _ => false,
    }
}

fn add_reaction(reactions: &mut Vec<Reaction>, emoji: Emoji, burst: bool, me: bool) {
    let i = match reactions.iter().position(|r| same_emoji(&r.emoji, &emoji)) {
        Some(i) => i,
        None => {
            reactions.push(Reaction { emoji, ..Default::default() });
            reactions.len() - 1
        }
    };
    let r = &mut reactions[i];
    let mine_already = if burst { r.me_burst } else { r.me };
    if me && mine_already {
        return; // duplicate event (or echo of an optimistic local update)
    }
    r.count += 1;
    if burst {
        r.count_details.burst += 1;
        r.me_burst |= me;
    } else {
        r.count_details.normal += 1;
        r.me |= me;
    }
}

fn remove_reaction(reactions: &mut Vec<Reaction>, emoji: &Emoji, burst: bool, me: bool) {
    let Some(i) = reactions.iter().position(|r| same_emoji(&r.emoji, emoji)) else { return };
    let r = &mut reactions[i];
    let mine_now = if burst { r.me_burst } else { r.me };
    if me && !mine_now {
        return; // already removed
    }
    r.count = r.count.saturating_sub(1);
    if burst {
        r.count_details.burst = r.count_details.burst.saturating_sub(1);
        r.me_burst &= !me;
    } else {
        r.count_details.normal = r.count_details.normal.saturating_sub(1);
        r.me &= !me;
    }
    if r.count == 0 {
        reactions.remove(i);
    }
}

/// Applies one member-list op, keeping only indices 0..MEMBER_LIST_KEEP.
fn apply_list_op(items: &mut Vec<MemberListItem>, op: MemberListOp) {
    match op {
        MemberListOp::Sync { range: (start, _), items: new } => {
            let s = start as usize;
            if s <= items.len() && s < MEMBER_LIST_KEEP {
                items.truncate(s);
                items.extend(new.into_iter().take(MEMBER_LIST_KEEP - s));
            }
        }
        MemberListOp::Insert { index, item } => {
            let i = index as usize;
            if i <= items.len() && i < MEMBER_LIST_KEEP {
                items.insert(i, item);
                items.truncate(MEMBER_LIST_KEEP);
            }
        }
        MemberListOp::Update { index, item } => {
            if let Some(slot) = items.get_mut(index as usize) {
                *slot = item;
            }
        }
        MemberListOp::Delete { index } => {
            if (index as usize) < items.len() {
                items.remove(index as usize);
            }
        }
        // shortcut: no placeholder rows, so the invalidated tail is dropped; fine while only
        // the 100-aligned first chunk is kept.
        MemberListOp::Invalidate { range: (start, _) } => items.truncate(start as usize),
    }
}

/// One-line notification text: user mentions resolved, placeholders for media-only messages.
fn notification_body(msg: &Message) -> String {
    let mut text = msg.content.clone();
    for u in &msg.mentions {
        let name = format!("@{}", u.display_name());
        text = text.replace(&format!("<@{}>", u.id), &name).replace(&format!("<@!{}>", u.id), &name);
    }
    if text.trim().is_empty() {
        text = if !msg.attachments.is_empty() {
            "Sent an attachment".into()
        } else if let Some(s) = msg.sticker_items.first() {
            format!("Sent a sticker: {}", s.name)
        } else if let Some(e) = msg.embeds.first() {
            e.title.clone().or_else(|| e.description.clone()).unwrap_or_default()
        } else if let Some(s) = msg.message_snapshots.first() {
            match notification_body(&s.message) {
                inner if inner.is_empty() => "Forwarded a message".into(),
                inner => format!("Forwarded: {inner}"),
            }
        } else {
            String::new()
        };
    }
    const MAX_CHARS: usize = 200;
    if text.chars().count() > MAX_CHARS {
        text = text.chars().take(MAX_CHARS).collect::<String>() + "…";
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: Id = Id(1);
    const OTHER: Id = Id(2);
    const G: Id = Id(100);
    const ROLE_A: Id = Id(101);
    const ROLE_B: Id = Id(102);
    const CAT: Id = Id(200);
    const TEXT: Id = Id(201);
    const DM: Id = Id(300);
    const BASE: u64 = perm::VIEW_CHANNEL | perm::SEND_MESSAGES | perm::ADD_REACTIONS | perm::READ_MESSAGE_HISTORY;
    const PAST: &str = "2000-01-01T00:00:00+00:00";
    const FUTURE: &str = "2999-01-01T00:00:00+00:00";

    fn user(id: Id, name: &str) -> User {
        User { id, username: name.into(), ..Default::default() }
    }
    fn role(id: Id, perms: u64, position: i32, color: u32) -> Role {
        Role { id, name: format!("role{id}"), color, position, permissions: Perms(perms), ..Default::default() }
    }
    fn chan(id: Id, kind: u8, parent: Option<Id>, position: i32) -> Channel {
        Channel { id, kind, guild_id: Some(G), name: Some(format!("c{id}")), position, parent_id: parent, ..Default::default() }
    }
    fn ow(id: Id, kind: u8, allow: u64, deny: u64) -> Overwrite {
        Overwrite { id, kind, allow: Perms(allow), deny: Perms(deny) }
    }
    fn ready() -> ReadyData {
        ReadyData {
            user: user(ME, "me"),
            guilds: vec![Guild {
                id: G,
                name: "Guild".into(),
                owner_id: Id(999),
                roles: vec![role(G, BASE, 0, 0), role(ROLE_A, 0, 1, 0xff0000), role(ROLE_B, 0, 2, 0)],
                channels: vec![
                    chan(CAT, channel_type::GUILD_CATEGORY, None, 0),
                    // guild_id missing, as in some gateway guild payloads.
                    Channel { guild_id: None, last_message_id: Some(Id(1000)), ..chan(TEXT, 0, Some(CAT), 0) },
                ],
                ..Default::default()
            }],
            private_channels: vec![Channel {
                id: DM,
                kind: channel_type::DM,
                recipients: vec![user(OTHER, "other")],
                last_message_id: Some(Id(500)),
                ..Default::default()
            }],
            my_members: vec![(G, Member { roles: vec![ROLE_A], ..Default::default() })],
            read_states: vec![
                ReadState { id: TEXT, last_message_id: Some(Id(1000)), ..Default::default() },
                ReadState { id: DM, last_message_id: Some(Id(500)), ..Default::default() },
            ],
            ..Default::default()
        }
    }
    fn state() -> AppState {
        let mut s = AppState::default();
        s.apply_ready(ready());
        s
    }
    fn msg(id: u64, channel: Id, author: Id) -> Message {
        let guild_id = (channel != DM).then_some(G);
        Message { id: Id(id), channel_id: channel, guild_id, author: user(author, "author"), content: format!("m{id}"), ..Default::default() }
    }
    fn mention(m: Message) -> Message {
        Message { mentions: vec![user(ME, "me")], ..m }
    }
    fn create(s: &mut AppState, m: Message, focused: Option<Id>) -> Option<Notification> {
        s.apply_gateway(GatewayEvent::MessageCreate(Box::new(m)), focused)
    }
    fn add_channel(s: &mut AppState, ch: Channel) {
        s.apply_gateway(GatewayEvent::ChannelCreate(Box::new(ch)), None);
    }
    fn settings(guild: Option<Id>, level: u8) -> GuildSettings {
        GuildSettings { guild_id: guild, message_notifications: level, ..Default::default() }
    }
    fn mute(end: Option<&str>) -> Option<MuteConfig> {
        Some(MuteConfig { end_time: end.map(String::from) })
    }
    fn override_(channel: Id, level: u8, muted: bool, end: Option<&str>) -> ChannelOverride {
        ChannelOverride { channel_id: channel, muted, mute_config: mute(end), message_notifications: level }
    }
    fn loaded(s: &mut AppState, ch: Id, ids: impl IntoIterator<Item = u64>) {
        let page = ids.into_iter().map(|i| msg(i, ch, OTHER)).collect();
        s.apply_messages_loaded(ch, None, Ok(page), 50);
    }
    fn ids(s: &AppState, ch: Id) -> Vec<u64> {
        s.messages[&ch].msgs.iter().map(|m| m.id.0).collect()
    }

    #[test]
    fn ready_normalizes_and_selects_first_channel() {
        let s = state();
        assert!(s.has_data && s.dirty);
        assert!(s.guilds[0].channels.is_empty());
        assert_eq!(s.channels[&TEXT].guild_id, Some(G));
        assert_eq!(s.channels[&DM].recipient_ids, vec![OTHER]);
        assert!(s.channels[&DM].recipients.is_empty());
        assert_eq!(s.users[&OTHER].username, "other");
        assert_eq!((s.selected_guild, s.selected_channel), (Some(G), Some(TEXT)));
        assert_eq!(s.channel_name(DM), "other");
    }

    #[test]
    fn ready_keeps_valid_selection_and_messages() {
        let mut s = state();
        s.select_channel(DM);
        loaded(&mut s, DM, [400, 450]);
        s.fresh_channels.insert(DM);
        s.messages.get_mut(&DM).unwrap().loading = true;
        s.apply_ready(ready());
        assert_eq!((s.selected_guild, s.selected_channel), (None, Some(DM)));
        assert_eq!(ids(&s, DM), vec![400, 450]);
        assert!(!s.messages[&DM].loading);
        assert!(s.fresh_channels.is_empty());

        // Selected guild channel vanished: back to the guild's first readable channel.
        add_channel(&mut s, chan(Id(202), 0, Some(CAT), 1));
        s.select_channel(Id(202));
        s.apply_ready(ready());
        assert_eq!((s.selected_guild, s.selected_channel), (Some(G), Some(TEXT)));

        // Selected DM vanished: stay on the DM home.
        s.select_channel(DM);
        let mut r = ready();
        r.private_channels.clear();
        s.apply_ready(r);
        assert!(!s.messages.contains_key(&DM));
        assert_eq!((s.selected_guild, s.selected_channel), (None, None));
    }

    #[test]
    fn permissions_owner_admin_and_dm() {
        let mut s = state();
        s.channels.get_mut(&TEXT).unwrap().permission_overwrites = vec![ow(G, 0, 0, perm::VIEW_CHANNEL)];
        assert_eq!(s.channel_permissions(TEXT), 0);
        s.guild_mut(G).unwrap().roles[2].permissions = Perms(perm::ADMINISTRATOR);
        s.my_members.get_mut(&G).unwrap().roles.push(ROLE_B);
        assert_eq!(s.channel_permissions(TEXT), perm::ALL, "admin ignores overwrites");
        s.my_members.get_mut(&G).unwrap().roles.clear();
        assert_eq!(s.channel_permissions(TEXT), 0);
        s.guild_mut(G).unwrap().owner_id = ME;
        assert_eq!(s.channel_permissions(TEXT), perm::ALL, "owner has everything");
        assert_eq!(s.channel_permissions(DM), perm::ALL);
        assert_eq!(s.channel_permissions(Id(12345)), 0);
    }

    #[test]
    fn permission_overwrite_order() {
        let mut s = state();
        s.my_members.get_mut(&G).unwrap().roles = vec![ROLE_A, ROLE_B];
        let set = |s: &mut AppState, ows: Vec<Overwrite>| s.channels.get_mut(&TEXT).unwrap().permission_overwrites = ows;
        // @everyone denies VIEW, one of my roles allows it back.
        set(&mut s, vec![ow(G, 0, 0, perm::VIEW_CHANNEL), ow(ROLE_A, 0, perm::VIEW_CHANNEL, 0)]);
        assert!(s.can_view(TEXT));
        // Role allows are applied after all role denies.
        set(&mut s, vec![ow(ROLE_A, 0, 0, perm::SEND_MESSAGES), ow(ROLE_B, 0, perm::SEND_MESSAGES, 0)]);
        assert!(s.can_send(TEXT));
        // A role deny beats an @everyone allow.
        set(&mut s, vec![ow(G, 0, perm::SEND_MESSAGES, 0), ow(ROLE_A, 0, 0, perm::SEND_MESSAGES)]);
        assert!(!s.can_send(TEXT));
        // The member overwrite beats role allows.
        set(&mut s, vec![ow(ROLE_A, 0, perm::SEND_MESSAGES, 0), ow(ME, 1, 0, perm::SEND_MESSAGES)]);
        assert!(!s.can_send(TEXT) && s.can_view(TEXT));
        // Overwrites for roles I lack or for other members don't apply.
        set(&mut s, vec![ow(Id(555), 0, 0, perm::VIEW_CHANNEL), ow(OTHER, 1, 0, perm::VIEW_CHANNEL)]);
        assert!(s.can_view(TEXT));
        // A member allow re-grants what @everyone and roles denied.
        set(&mut s, vec![ow(G, 0, 0, perm::ADD_REACTIONS), ow(ROLE_A, 0, 0, perm::ADD_REACTIONS), ow(ME, 1, perm::ADD_REACTIONS, 0)]);
        assert!(s.can_react(TEXT));
    }

    #[test]
    fn threads_use_parent_permissions() {
        let mut s = state();
        let thread = Id(250);
        add_channel(&mut s, chan(thread, channel_type::PUBLIC_THREAD, Some(TEXT), 0));
        assert!(s.can_view(thread));
        assert!(!s.can_send(thread), "BASE lacks SEND_MESSAGES_IN_THREADS");
        s.guild_mut(G).unwrap().roles[0].permissions = Perms(BASE | perm::SEND_MESSAGES_IN_THREADS);
        assert!(s.can_send(thread));
        s.channels.get_mut(&TEXT).unwrap().permission_overwrites = vec![ow(G, 0, 0, perm::VIEW_CHANNEL)];
        assert!(!s.can_view(thread));
    }

    #[test]
    fn sidebar_sorting_and_filtering() {
        let mut s = state();
        let hidden = vec![ow(G, 0, 0, perm::VIEW_CHANNEL)];
        for ch in [
            chan(Id(210), channel_type::GUILD_VOICE, None, 0),
            chan(Id(211), channel_type::GUILD_TEXT, None, 5),
            chan(Id(220), channel_type::GUILD_CATEGORY, None, 1),
            chan(Id(205), channel_type::GUILD_CATEGORY, None, 1),
            chan(Id(221), channel_type::GUILD_VOICE, Some(Id(220)), 0),
            chan(Id(223), channel_type::GUILD_TEXT, Some(Id(220)), 2),
            chan(Id(222), channel_type::GUILD_ANNOUNCEMENT, Some(Id(220)), 2),
            chan(Id(206), channel_type::GUILD_TEXT, Some(Id(205)), 0),
            Channel { permission_overwrites: hidden.clone(), ..chan(Id(207), channel_type::GUILD_TEXT, Some(Id(205)), 1) },
            chan(Id(230), channel_type::GUILD_CATEGORY, None, 0),
            Channel { permission_overwrites: hidden, ..chan(Id(231), channel_type::GUILD_TEXT, Some(Id(230)), 0) },
            chan(Id(240), channel_type::PUBLIC_THREAD, Some(Id(206)), 0),
            chan(Id(241), 6, None, 0),
        ] {
            add_channel(&mut s, ch);
        }
        let cat = |id: u64| SidebarItem::Category { id: Id(id), name: format!("c{id}") };
        let ch = |id: u64| SidebarItem::Channel { id: Id(id) };
        assert_eq!(
            s.sidebar_channels(G),
            vec![
                // Uncategorized first, text-like above voice-like regardless of position.
                ch(211),
                ch(210),
                // Categories by (position, id); 230 has no visible child and is omitted.
                cat(200),
                ch(201),
                cat(205),
                ch(206),
                cat(220),
                ch(222),
                ch(223),
                ch(221),
            ]
        );
        assert!(s.sidebar_channels(Id(4242)).is_empty());
    }

    #[test]
    fn unread_and_mentions() {
        let mut s = state();
        assert!(!s.is_unread(TEXT));
        create(&mut s, msg(1001, TEXT, OTHER), None);
        assert!(s.is_unread(TEXT));
        assert_eq!(s.mention_count(TEXT), 0);
        assert_eq!(s.guild_unread(G), (true, 0));
        create(&mut s, mention(msg(1002, TEXT, OTHER)), None);
        assert_eq!(s.mention_count(TEXT), 1);
        assert_eq!(s.guild_unread(G), (true, 1));

        // Muted channel: plain unread hidden, the mention badge stays.
        s.guild_settings.insert(Some(G), GuildSettings { channel_overrides: vec![override_(TEXT, 3, true, None)], ..settings(Some(G), 0) });
        assert!(s.is_unread(TEXT), "mentions override mute");
        assert_eq!(s.guild_unread(G), (false, 1));
        assert_eq!(s.mark_read(TEXT), Some(Id(1002)));
        assert_eq!(s.mark_read(TEXT), None);
        create(&mut s, msg(1003, TEXT, OTHER), None);
        assert!(!s.is_unread(TEXT));
        assert!(s.is_channel_muted(TEXT));

        // A mute whose end time passed is no mute; a future one still is.
        let set_end = |s: &mut AppState, end| s.guild_settings.get_mut(&Some(G)).unwrap().channel_overrides[0].mute_config = mute(Some(end));
        set_end(&mut s, PAST);
        assert!(s.is_unread(TEXT));
        set_end(&mut s, FUTURE);
        assert!(!s.is_unread(TEXT));

        // A muted category mutes its children.
        s.guild_settings.insert(Some(G), GuildSettings { channel_overrides: vec![override_(CAT, 3, true, None)], ..settings(Some(G), 0) });
        assert!(s.is_channel_muted(TEXT) && !s.is_unread(TEXT));

        // Muted guild: the icon shows mention badges only.
        s.guild_settings.insert(Some(G), GuildSettings { muted: true, ..settings(Some(G), 0) });
        assert!(s.is_guild_muted(G) && s.is_unread(TEXT));
        assert_eq!(s.guild_unread(G), (false, 0));

        // My own message marks the channel read.
        create(&mut s, msg(1004, TEXT, ME), None);
        assert!(!s.is_unread(TEXT));
        assert_eq!(s.read_states[&TEXT].last_message_id, Some(Id(1004)));
    }

    #[test]
    fn channel_without_read_state() {
        let mut s = state();
        let ch = Id(202);
        add_channel(&mut s, Channel { last_message_id: Some(Id(5000)), ..chan(ch, 0, Some(CAT), 1) });
        assert!(!s.is_unread(ch), "missing read state counts as read");
        create(&mut s, msg(5001, ch, OTHER), None);
        assert!(s.is_unread(ch));
        assert_eq!(s.read_states[&ch].last_message_id, Some(Id(5000)));
        assert_eq!(s.guild_unread(G), (true, 0));
        // Hidden channels don't badge the guild.
        s.channels.get_mut(&ch).unwrap().permission_overwrites = vec![ow(G, 0, 0, perm::VIEW_CHANNEL)];
        assert_eq!(s.guild_unread(G), (false, 0));
        // Neither do forum channels, which the client cannot open or mark read.
        let forum = Id(203);
        add_channel(&mut s, Channel { last_message_id: Some(Id(6000)), ..chan(forum, channel_type::GUILD_FORUM, None, 2) });
        s.read_states.insert(forum, ReadState { id: forum, last_message_id: Some(Id(5999)), ..Default::default() });
        assert_eq!(s.guild_unread(G), (false, 0));
    }

    #[test]
    fn guild_unreads_matches_per_guild_badges() {
        let mut s = state();
        let g2 = Id(110);
        let guild2 = Guild {
            id: g2,
            name: "Other".into(),
            owner_id: ME,
            channels: vec![Channel { guild_id: Some(g2), ..chan(Id(210), 0, None, 0) }],
            ..Default::default()
        };
        s.apply_gateway(GatewayEvent::GuildCreate(Box::new(guild2)), None);
        s.read_states.insert(Id(210), ReadState { id: Id(210), last_message_id: Some(Id(1)), ..Default::default() });
        create(&mut s, mention(msg(1001, TEXT, OTHER)), None);
        create(&mut s, Message { guild_id: Some(g2), ..msg(1002, Id(210), OTHER) }, None);
        let all = s.guild_unreads();
        assert_eq!(all[&G], (true, 1));
        assert_eq!(all[&g2], (true, 0));
        for g in [G, g2, Id(999)] {
            assert_eq!(all.get(&g).copied().unwrap_or_default(), s.guild_unread(g));
        }
    }

    #[test]
    fn history_cap_spares_the_open_channel() {
        let max = MAX_MESSAGES_PER_CHANNEL as u64;
        let mut s = state(); // TEXT is open
        loaded(&mut s, TEXT, 101..101 + max);
        // Paging back past the cap keeps everything, and so do new messages meanwhile.
        s.apply_messages_loaded(TEXT, Some(Id(101)), Ok((51..101).map(|i| msg(i, TEXT, OTHER)).collect()), 50);
        create(&mut s, msg(5000, TEXT, OTHER), None);
        assert_eq!(s.messages[&TEXT].msgs.len(), MAX_MESSAGES_PER_CHANNEL + 51);
        // Back at the bottom, the view trims it.
        s.trim_history(TEXT);
        assert_eq!(s.messages[&TEXT].msgs.len(), MAX_MESSAGES_PER_CHANNEL);
        assert_eq!(s.messages[&TEXT].msgs[0].id, Id(102));
        // Leaving a paged-back channel trims it too; then it is capped like any other.
        s.apply_messages_loaded(TEXT, Some(Id(102)), Ok((51..102).map(|i| msg(i, TEXT, OTHER)).collect()), 50);
        s.select_channel(DM);
        assert_eq!(s.messages[&TEXT].msgs.len(), MAX_MESSAGES_PER_CHANNEL);
        assert!(s.messages[&TEXT].has_more_before);
        create(&mut s, msg(5001, TEXT, OTHER), None);
        assert_eq!(s.messages[&TEXT].msgs.len(), MAX_MESSAGES_PER_CHANNEL);
    }

    #[test]
    fn only_recent_channels_keep_messages() {
        let mut s = state(); // TEXT is open
        loaded(&mut s, TEXT, [1000]);
        s.fresh_channels.insert(TEXT);
        let others: Vec<Id> = (202..202 + SNAPSHOT_CHANNELS as u64).map(Id).collect();
        for (i, id) in others.iter().enumerate() {
            add_channel(&mut s, chan(*id, 0, Some(CAT), i as i32 + 1));
        }
        for id in &others[..SNAPSHOT_CHANNELS - 1] {
            s.select_channel(*id);
        }
        assert!(s.messages.contains_key(&TEXT) && s.fresh_channels.contains(&TEXT));
        // TEXT falls out of the recent list: dropped, and refetched on a revisit.
        s.select_channel(others[SNAPSHOT_CHANNELS - 1]);
        assert!(!s.messages.contains_key(&TEXT) && !s.fresh_channels.contains(&TEXT));
    }

    #[test]
    fn opening_a_parent_keeps_its_threads_messages_one_step() {
        let mut s = state(); // TEXT is open
        let (forum, post) = (Id(300), Id(301));
        add_channel(&mut s, chan(forum, channel_type::GUILD_FORUM, None, 9));
        add_channel(&mut s, chan(post, channel_type::PUBLIC_THREAD, Some(forum), 0));
        s.select_channel(post);
        loaded(&mut s, post, [post.0]);
        // Five other channels push the post out of the recent list, the last one being its forum.
        let others: Vec<Id> = (202..202 + SNAPSHOT_CHANNELS as u64 - 1).map(Id).collect();
        for (i, id) in others.iter().enumerate() {
            add_channel(&mut s, chan(*id, 0, Some(CAT), i as i32 + 1));
            s.select_channel(*id);
        }
        s.select_channel(forum);
        assert!(!s.recent_channels.contains(&post) && s.messages.contains_key(&post), "the forum's card keeps its starter");
        s.select_channel(TEXT);
        assert!(!s.messages.contains_key(&post), "gone once another channel opens");
    }

    #[test]
    fn dm_messages_count_as_mentions_unless_muted() {
        let mut s = state();
        create(&mut s, msg(600, DM, OTHER), None);
        create(&mut s, msg(601, DM, OTHER), None);
        assert_eq!(s.mention_count(DM), 2);
        assert_eq!(s.dm_unread_count(), 2);
        create(&mut s, msg(602, DM, OTHER), Some(DM));
        assert_eq!(s.mention_count(DM), 2, "focused channel doesn't count");
        create(&mut s, Message { kind: MSG_RECIPIENT_REMOVE, ..msg(603, DM, OTHER) }, None);
        assert_eq!(s.mention_count(DM), 2);
        assert_eq!(s.mark_read(DM), Some(Id(603)));
        s.guild_settings.insert(None, GuildSettings { channel_overrides: vec![override_(DM, 3, true, None)], ..settings(None, 3) });
        create(&mut s, msg(604, DM, OTHER), None);
        assert_eq!(s.mention_count(DM), 0);
        assert_eq!(s.dm_unread_count(), 0);
    }

    #[test]
    fn notification_decisions() {
        let notify = |s: &mut AppState, m: Message, focused: Option<Id>| create(s, m, focused).is_some();
        let mut s = state();

        // DMs notify unless focused, mine, or muted (an expired mute is no mute).
        assert!(notify(&mut s, msg(600, DM, OTHER), None));
        assert!(!notify(&mut s, msg(601, DM, OTHER), Some(DM)));
        assert!(!notify(&mut s, msg(602, DM, ME), None));
        s.guild_settings.insert(None, GuildSettings { channel_overrides: vec![override_(DM, 3, true, Some(FUTURE))], ..settings(None, 3) });
        assert!(!notify(&mut s, msg(603, DM, OTHER), None));
        s.guild_settings.insert(None, GuildSettings { channel_overrides: vec![override_(DM, 3, true, Some(PAST))], ..settings(None, 3) });
        assert!(notify(&mut s, msg(604, DM, OTHER), None));

        // No settings entry: the guild default decides.
        assert!(notify(&mut s, msg(2000, TEXT, OTHER), None));
        s.guild_mut(G).unwrap().default_message_notifications = 1;
        assert!(!notify(&mut s, msg(2001, TEXT, OTHER), None));
        assert!(notify(&mut s, mention(msg(2002, TEXT, OTHER)), None));
        // Guild entry beats the default; INHERIT falls through to it.
        s.guild_settings.insert(Some(G), settings(Some(G), 0));
        assert!(notify(&mut s, msg(2003, TEXT, OTHER), None));
        s.guild_settings.insert(Some(G), settings(Some(G), 3));
        assert!(!notify(&mut s, msg(2004, TEXT, OTHER), None));
        // Channel override beats the guild entry; a category override applies to children.
        s.guild_settings.insert(Some(G), GuildSettings { channel_overrides: vec![override_(TEXT, 0, false, None)], ..settings(Some(G), 1) });
        assert!(notify(&mut s, msg(2005, TEXT, OTHER), None));
        s.guild_settings.insert(Some(G), GuildSettings { channel_overrides: vec![override_(CAT, 0, false, None)], ..settings(Some(G), 1) });
        assert!(notify(&mut s, msg(2006, TEXT, OTHER), None));
        s.guild_settings.insert(Some(G), GuildSettings { channel_overrides: vec![override_(TEXT, 3, false, None)], ..settings(Some(G), 1) });
        assert!(!notify(&mut s, msg(2007, TEXT, OTHER), None));
        // NO_MESSAGES blocks even mentions.
        s.guild_settings.insert(Some(G), settings(Some(G), 2));
        assert!(!notify(&mut s, mention(msg(2008, TEXT, OTHER)), None));
        // Muted guild: silent unless mentioned; an expired mute is no mute.
        s.guild_settings.insert(Some(G), GuildSettings { muted: true, ..settings(Some(G), 0) });
        assert!(!notify(&mut s, msg(2009, TEXT, OTHER), None));
        assert!(notify(&mut s, mention(msg(2010, TEXT, OTHER)), None));
        s.guild_settings.insert(Some(G), GuildSettings { muted: true, mute_config: mute(Some(PAST)), ..settings(Some(G), 0) });
        assert!(notify(&mut s, msg(2011, TEXT, OTHER), None));
        // Muted channel.
        s.guild_settings.insert(Some(G), GuildSettings { channel_overrides: vec![override_(TEXT, 3, true, None)], ..settings(Some(G), 0) });
        assert!(!notify(&mut s, msg(2012, TEXT, OTHER), None));
        // Focused channel, my own message and @silent never alert.
        s.guild_settings.insert(Some(G), settings(Some(G), 0));
        assert!(!notify(&mut s, msg(2013, TEXT, OTHER), Some(TEXT)));
        assert!(!notify(&mut s, mention(msg(2014, TEXT, ME)), None));
        let silent = Message { flags: FLAG_SUPPRESS_NOTIFICATIONS, ..mention(msg(2015, TEXT, OTHER)) };
        let before = s.mention_count(TEXT);
        assert!(!notify(&mut s, silent, None));
        assert_eq!(s.mention_count(TEXT), before + 1, "@silent still counts as a mention");
        // No VIEW_CHANNEL: no alert.
        s.channels.get_mut(&TEXT).unwrap().permission_overwrites = vec![ow(G, 0, 0, perm::VIEW_CHANNEL)];
        assert!(!notify(&mut s, msg(2016, TEXT, OTHER), None));
    }

    #[test]
    fn mentions_me_rules() {
        let mut s = state();
        let everyone = Message { mention_everyone: true, ..msg(1, TEXT, OTHER) };
        let my_role = Message { mention_roles: vec![ROLE_A], ..msg(2, TEXT, OTHER) };
        let other_role = Message { mention_roles: vec![ROLE_B], ..msg(3, TEXT, OTHER) };
        let quiet_reply = Message { referenced_message: Some(Box::new(msg(4, TEXT, ME))), ..msg(5, TEXT, OTHER) };
        assert!(s.mentions_me(&everyone) && s.mentions_me(&my_role) && s.mentions_me(&mention(msg(6, TEXT, OTHER))));
        assert!(!s.mentions_me(&other_role) && !s.mentions_me(&quiet_reply));
        s.guild_settings.insert(Some(G), GuildSettings { suppress_everyone: true, suppress_roles: true, ..settings(Some(G), 1) });
        assert!(!s.mentions_me(&everyone) && !s.mentions_me(&my_role));
        assert!(create(&mut s, Message { id: Id(2100), ..everyone }, None).is_none());
        assert_eq!(s.mention_count(TEXT), 0);
    }

    #[test]
    fn notification_text() {
        let mut s = state();
        let other = User { global_name: Some("Oth".into()), ..user(OTHER, "other") };
        let m = Message { content: format!("hi <@{ME}> and <@!{OTHER}>"), mentions: vec![user(ME, "me"), other], ..msg(600, DM, OTHER) };
        let n = create(&mut s, m, None).unwrap();
        assert_eq!((n.channel_id, n.title.as_str(), n.body.as_str()), (DM, "author", "hi @me and @Oth"));
        let n = create(&mut s, msg(1001, TEXT, OTHER), None).unwrap();
        assert_eq!(n.title, "author (#c201, Guild)");
        let attachment_only = Message { content: String::new(), attachments: vec![Attachment::default()], ..msg(1002, TEXT, OTHER) };
        assert_eq!(create(&mut s, attachment_only, None).unwrap().body, "Sent an attachment");
        let long = Message { content: "x".repeat(500), ..Default::default() };
        assert_eq!(notification_body(&long).chars().count(), 201);
        let snap = |content: &str| Message { message_snapshots: vec![MessageSnapshot { message: Message { content: content.into(), ..Default::default() } }], ..Default::default() };
        assert_eq!(notification_body(&snap("look")), "Forwarded: look");
        assert_eq!(notification_body(&snap("")), "Forwarded a message");
    }

    #[test]
    fn mute_end_time() {
        assert!(mute_active(true, &None));
        assert!(mute_active(true, &mute(None)));
        assert!(mute_active(true, &mute(Some(FUTURE))));
        assert!(!mute_active(true, &mute(Some(PAST))));
        assert!(!mute_active(false, &mute(Some(FUTURE))));
        assert!(mute_active(true, &mute(Some("garbage"))));
    }

    #[test]
    fn message_create_merges_into_loaded_channels_only() {
        let mut s = state();
        create(&mut s, msg(1001, TEXT, OTHER), None);
        assert!(!s.messages.contains_key(&TEXT));
        assert_eq!(s.channels[&TEXT].last_message_id, Some(Id(1001)));
        loaded(&mut s, TEXT, [990, 1000, 1001]);
        let e = &s.messages[&TEXT];
        assert!(e.loaded && !e.loading && !e.has_more_before && e.error.is_none());
        create(&mut s, msg(1003, TEXT, OTHER), None);
        create(&mut s, msg(1002, TEXT, OTHER), None);
        create(&mut s, msg(1003, TEXT, OTHER), None);
        assert_eq!(ids(&s, TEXT), vec![990, 1000, 1001, 1002, 1003]);
        // A fetch in flight also receives live messages.
        s.messages.insert(DM, ChannelMessages { loading: true, ..Default::default() });
        create(&mut s, msg(700, DM, OTHER), None);
        assert_eq!(ids(&s, DM), vec![700]);
    }

    #[test]
    fn message_cap_drops_oldest() {
        let mut s = state();
        loaded(&mut s, TEXT, 1..=MAX_MESSAGES_PER_CHANNEL as u64);
        s.messages.get_mut(&TEXT).unwrap().has_more_before = false;
        create(&mut s, msg(5000, TEXT, OTHER), None);
        let e = &s.messages[&TEXT];
        assert_eq!(e.msgs.len(), MAX_MESSAGES_PER_CHANNEL);
        assert_eq!((e.msgs[0].id, e.msgs.last().unwrap().id), (Id(2), Id(5000)));
        assert!(e.has_more_before);
    }

    #[test]
    fn history_pages() {
        // Messages held from before READY (cache or the previous session) with gaps; 2000 was
        // deleted on the server meanwhile.
        let mut s = AppState::default();
        let stale = vec![msg(10, TEXT, OTHER), msg(990, TEXT, OTHER), msg(2000, TEXT, OTHER)];
        s.messages.insert(TEXT, ChannelMessages { msgs: stale, loaded: true, ..Default::default() });
        s.apply_ready(ready());
        s.messages.get_mut(&TEXT).unwrap().loading = true;
        // Delivered by the gateway while the latest-page request is in flight.
        create(&mut s, msg(2001, TEXT, OTHER), None);
        // The page replaces everything it covers or predates; only the live message survives.
        let page: Vec<Message> = (1000..1050).rev().map(|i| msg(i, TEXT, OTHER)).collect();
        s.apply_messages_loaded(TEXT, None, Ok(page), 50);
        let e = &s.messages[&TEXT];
        assert_eq!(e.msgs.len(), 51);
        assert_eq!((e.msgs[0].id, e.msgs[50].id), (Id(1000), Id(2001)));
        assert!(e.has_more_before && !e.loading);
        assert_eq!(s.channels[&TEXT].last_message_id, Some(Id(2001)));
        // An older page prepends and settles has_more_before.
        s.apply_messages_loaded(TEXT, Some(Id(1000)), Ok(vec![msg(999, TEXT, OTHER), msg(998, TEXT, OTHER)]), 50);
        let e = &s.messages[&TEXT];
        assert_eq!(e.msgs[0].id, Id(998));
        assert!(!e.has_more_before);
        // Errors are recorded and keep the messages.
        s.messages.get_mut(&TEXT).unwrap().loading = true;
        s.apply_messages_loaded(TEXT, Some(Id(998)), Err("boom".into()), 50);
        let e = &s.messages[&TEXT];
        assert_eq!(e.error.as_deref(), Some("boom"));
        assert!(!e.loading);
        assert_eq!(e.msgs.len(), 53);
        // Empty latest page: the channel is empty now.
        s.apply_messages_loaded(TEXT, None, Ok(vec![]), 50);
        let e = &s.messages[&TEXT];
        assert!(e.msgs.is_empty() && !e.has_more_before && e.error.is_none());
    }

    #[test]
    fn pending_and_send_results() {
        let mut s = state();
        loaded(&mut s, TEXT, [1000]);
        let pend = |n: &str| PendingMessage { nonce: n.into(), channel_id: TEXT, content: "hi".into(), failed: None, reply: None };
        s.pending = vec![pend("a"), pend("b"), pend("c")];
        s.apply_send_result(TEXT, "a", Err("nope".into()));
        assert_eq!(s.pending[0].failed.as_deref(), Some("nope"));
        let mine = |id, n: &str| Message { nonce: Some(serde_json::Value::String(n.into())), ..msg(id, TEXT, ME) };
        // The gateway echo carries our nonce and removes the pending entry.
        assert!(create(&mut s, mine(1001, "b"), None).is_none());
        assert_eq!(s.pending.len(), 2);
        // REST result after the gateway copy: no duplicate.
        s.apply_send_result(TEXT, "b", Ok(mine(1001, "b")));
        // REST result first: inserted, read state advanced.
        s.apply_send_result(TEXT, "c", Ok(mine(1002, "c")));
        assert_eq!(s.pending.len(), 1);
        assert_eq!(ids(&s, TEXT), vec![1000, 1001, 1002]);
        assert_eq!(s.read_states[&TEXT].last_message_id, Some(Id(1002)));
        assert!(!s.is_unread(TEXT));
        // The channel vanished while the POST was in flight: no phantom DM appears.
        s.apply_gateway(GatewayEvent::ChannelDelete { id: TEXT, guild_id: Some(G) }, None);
        s.pending = vec![pend("d")];
        let dms = s.dm_order.clone();
        s.apply_send_result(TEXT, "d", Ok(Message { guild_id: None, ..mine(1003, "d") }));
        assert!(s.pending.is_empty() && !s.channels.contains_key(&TEXT));
        assert_eq!(s.dm_order, dms);
    }

    #[test]
    fn message_update_merges_present_fields_and_delete() {
        let mut s = state();
        let embed = Embed { title: Some("t".into()), ..Default::default() };
        let original = Message { embeds: vec![embed], pinned: true, ..msg(1000, TEXT, OTHER) };
        s.apply_messages_loaded(TEXT, None, Ok(vec![original, msg(1001, TEXT, OTHER), msg(1002, TEXT, OTHER)]), 50);
        let upd = MessageUpdate {
            id: Id(1000),
            channel_id: TEXT,
            content: Some("edited".into()),
            edited_timestamp: Some("2026-01-01T00:00:00+00:00".into()),
            ..Default::default()
        };
        s.apply_gateway(GatewayEvent::MessageUpdate(Box::new(upd)), None);
        let m = &s.messages[&TEXT].msgs[0];
        assert_eq!(m.content, "edited");
        assert!(m.edited_timestamp.is_some());
        assert_eq!(m.embeds.len(), 1);
        assert!(m.pinned);
        assert_eq!(m.author.id, OTHER);
        s.apply_gateway(GatewayEvent::MessageDelete { id: Id(1000), channel_id: TEXT }, None);
        s.apply_gateway(GatewayEvent::MessageDeleteBulk { ids: vec![Id(1002), Id(77)], channel_id: TEXT }, None);
        assert_eq!(ids(&s, TEXT), vec![1001]);
    }

    #[test]
    fn reactions() {
        let mut s = state();
        loaded(&mut s, TEXT, [1000]);
        let fire = Emoji { id: None, name: Some("🔥".into()), animated: false };
        let custom = Emoji { id: Some(Id(77)), name: Some("blob".into()), animated: false };
        let add = |user_id, emoji: &Emoji, burst| GatewayEvent::ReactionAdd { channel_id: TEXT, message_id: Id(1000), user_id, emoji: emoji.clone(), burst };
        let rm = |user_id, emoji: &Emoji, burst| GatewayEvent::ReactionRemove { channel_id: TEXT, message_id: Id(1000), user_id, emoji: emoji.clone(), burst };
        let list = |s: &AppState| {
            s.messages[&TEXT].msgs[0]
                .reactions
                .iter()
                .map(|r| (r.emoji.name.clone().unwrap_or_default(), r.count, r.me, r.me_burst))
                .collect::<Vec<_>>()
        };
        s.apply_gateway(add(OTHER, &fire, false), None);
        s.apply_gateway(add(ME, &fire, false), None);
        s.apply_gateway(add(ME, &fire, false), None); // duplicate echo
        assert_eq!(list(&s), vec![("🔥".to_string(), 2, true, false)]);
        // Custom emoji match by id even when name/animated differ.
        s.apply_gateway(add(ME, &custom, true), None);
        s.apply_gateway(add(OTHER, &Emoji { name: None, animated: true, ..custom.clone() }, false), None);
        assert_eq!(list(&s)[1], ("blob".to_string(), 2, false, true));
        let details = &s.messages[&TEXT].msgs[0].reactions[1].count_details;
        assert_eq!((details.burst, details.normal), (1, 1));
        s.apply_gateway(rm(ME, &fire, false), None);
        s.apply_gateway(rm(ME, &fire, false), None); // duplicate
        assert_eq!(list(&s)[0], ("🔥".to_string(), 1, false, false));
        s.apply_gateway(rm(OTHER, &fire, false), None);
        assert_eq!(list(&s).len(), 1, "a reaction reaching 0 is removed");
        s.apply_gateway(GatewayEvent::ReactionRemoveEmoji { channel_id: TEXT, message_id: Id(1000), emoji: custom.clone() }, None);
        assert!(list(&s).is_empty());
        s.apply_gateway(add(OTHER, &fire, false), None);
        s.apply_gateway(GatewayEvent::ReactionRemoveAll { channel_id: TEXT, message_id: Id(1000) }, None);
        assert!(list(&s).is_empty());
        // Unknown messages are ignored.
        s.apply_gateway(add(OTHER, &fire, false), None);
        s.apply_gateway(GatewayEvent::ReactionAdd { channel_id: TEXT, message_id: Id(5), user_id: OTHER, emoji: fire, burst: false }, None);
    }

    fn member(id: u64) -> MemberListItem {
        MemberListItem::Member { member: Member { user: Some(user(Id(id), "m")), ..Default::default() }, status: Presence::Online }
    }
    fn group(id: &str) -> MemberListItem {
        MemberListItem::Group { id: id.into(), count: 1 }
    }
    fn list_update(list_id: &str, ops: Vec<MemberListOp>) -> GatewayEvent {
        let groups = vec![("online".into(), 7)];
        GatewayEvent::MemberListUpdate(Box::new(MemberListUpdate { guild_id: G, list_id: list_id.into(), member_count: 10, online_count: 5, ops, groups }))
    }
    fn list_ids(s: &AppState) -> Vec<String> {
        s.member_lists[&G]
            .items
            .iter()
            .map(|it| match it {
                MemberListItem::Group { id, .. } => id.clone(),
                MemberListItem::Member { member, .. } => member.user.as_ref().unwrap().id.to_string(),
            })
            .collect()
    }

    #[test]
    fn member_list_ops() {
        let mut s = state();
        s.apply_gateway(list_update("everyone", vec![MemberListOp::Sync { range: (0, 99), items: vec![group("online"), member(10), member(11)] }]), None);
        assert_eq!(list_ids(&s), ["online", "10", "11"]);
        assert!(matches!(&s.member_lists[&G].items[0], MemberListItem::Group { count: 7, .. }), "counts follow `groups`");
        assert_eq!(s.member_lists[&G].channel_id, Some(TEXT));
        assert_eq!((s.member_lists[&G].member_count, s.member_lists[&G].online_count), (10, 5));
        assert!(s.users.contains_key(&Id(10)));
        s.apply_gateway(
            list_update(
                "everyone",
                vec![
                    MemberListOp::Insert { index: 1, item: member(12) },
                    MemberListOp::Update { index: 3, item: member(13) },
                    MemberListOp::Delete { index: 2 },
                    MemberListOp::Insert { index: 50, item: member(14) },
                    MemberListOp::Sync { range: (100, 199), items: vec![member(15)] },
                ],
            ),
            None,
        );
        assert_eq!(list_ids(&s), ["online", "12", "13"]);
        s.apply_gateway(list_update("everyone", vec![MemberListOp::Invalidate { range: (0, 99) }]), None);
        assert!(list_ids(&s).is_empty());
        // Only the first 100 rows are kept.
        let many = (0..150).map(|i| member(1000 + i)).collect();
        s.apply_gateway(list_update("everyone", vec![MemberListOp::Sync { range: (0, 99), items: many }]), None);
        assert_eq!(s.member_lists[&G].items.len(), MEMBER_LIST_KEEP);
        s.apply_gateway(list_update("everyone", vec![MemberListOp::Insert { index: 0, item: member(9) }]), None);
        assert_eq!(s.member_lists[&G].items.len(), MEMBER_LIST_KEEP);
        // A different list id starts over with its SYNC; late non-SYNC updates for the old
        // list leave the current one alone.
        s.apply_gateway(list_update("12345", vec![MemberListOp::Sync { range: (0, 99), items: vec![group("offline")] }]), None);
        assert_eq!(list_ids(&s), ["offline"]);
        assert_eq!(s.member_lists[&G].list_id, "12345");
        s.apply_gateway(list_update("everyone", vec![MemberListOp::Invalidate { range: (0, 99) }]), None);
        s.apply_gateway(list_update("everyone", vec![MemberListOp::Insert { index: 0, item: member(16) }]), None);
        assert_eq!(list_ids(&s), ["offline"]);
        assert_eq!(s.member_lists[&G].list_id, "12345");
        assert!(s.users.contains_key(&Id(16)), "members are still cached");
        // No list yet and no SYNC: nothing is created.
        s.member_lists.clear();
        s.apply_gateway(list_update("everyone", vec![MemberListOp::Delete { index: 0 }]), None);
        assert!(s.member_lists.is_empty());
    }

    #[test]
    fn display_names_and_role_colors() {
        let mut s = state();
        let u = User { global_name: Some("Global".into()), ..user(OTHER, "name") };
        assert_eq!(s.member_display_name(Some(G), &u, None), "Global");
        assert_eq!(s.member_display_name(None, &user(OTHER, "name"), None), "name");
        let nick = Member { nick: Some("Nick".into()), ..Default::default() };
        assert_eq!(s.member_display_name(Some(G), &u, Some(&nick)), "Nick");
        // History messages carry no member: fall back to the member list / my member.
        let listed = Member { user: Some(u.clone()), nick: Some("Listed".into()), ..Default::default() };
        let item = MemberListItem::Member { member: listed, status: Presence::Online };
        s.apply_gateway(list_update("everyone", vec![MemberListOp::Sync { range: (0, 99), items: vec![item] }]), None);
        assert_eq!(s.member_display_name(Some(G), &u, None), "Listed");
        s.my_members.get_mut(&G).unwrap().nick = Some("MyNick".into());
        assert_eq!(s.member_display_name(Some(G), &user(ME, "me"), None), "MyNick");

        // Highest positioned colored role wins; uncolored roles are skipped.
        s.apply_gateway(GatewayEvent::GuildRoleUpsert { guild_id: G, role: role(Id(103), 0, 5, 0x00ff00) }, None);
        assert_eq!(s.role_color(G, &[ROLE_A, ROLE_B]), Some(0xff0000));
        assert_eq!(s.role_color(G, &[ROLE_A, Id(103)]), Some(0x00ff00));
        assert_eq!(s.role_color(G, &[ROLE_B]), None);
        s.apply_gateway(GatewayEvent::GuildRoleDelete { guild_id: G, role_id: ROLE_A }, None);
        assert_eq!(s.role_color(G, &[ROLE_A]), None);
        assert!(s.my_members[&G].roles.is_empty());
    }

    #[test]
    fn selection_memory_and_fallbacks() {
        let mut s = state();
        add_channel(&mut s, chan(Id(202), 0, Some(CAT), 1));
        s.select_channel(Id(202));
        s.select_guild(None);
        assert_eq!(s.selected_channel, None, "no DM was selected before");
        s.select_channel(DM);
        s.select_guild(Some(G));
        assert_eq!(s.selected_channel, Some(Id(202)));
        s.select_guild(None);
        assert_eq!(s.selected_channel, Some(DM));
        assert_eq!(s.recent_channels, vec![DM, Id(202), TEXT]);
        s.select_channel(Id(9999));
        assert_eq!(s.selected_channel, Some(DM), "unknown channels are ignored");

        // Deleting the selected channel falls back to the guild's first readable channel.
        s.select_channel(Id(202));
        s.apply_gateway(GatewayEvent::ChannelDelete { id: Id(202), guild_id: Some(G) }, None);
        assert_eq!(s.selected_channel, Some(TEXT));
        // An outage keeps the guild; leaving it goes to DM home (no other guild).
        s.apply_gateway(GatewayEvent::GuildDelete { id: G, unavailable: true }, None);
        assert!(s.guilds[0].unavailable && s.selected_channel == Some(TEXT));
        s.apply_gateway(GatewayEvent::GuildDelete { id: G, unavailable: false }, None);
        assert_eq!((s.selected_guild, s.selected_channel), (None, Some(DM)));
        assert!(!s.channels.contains_key(&TEXT) && !s.read_states.contains_key(&TEXT));
    }

    #[test]
    fn dm_order_follows_activity() {
        let mut r = ready();
        r.private_channels.push(Channel { id: Id(301), kind: channel_type::GROUP_DM, name: Some("group".into()), last_message_id: Some(Id(700)), ..Default::default() });
        r.private_channels.push(Channel { id: Id(302), kind: channel_type::DM, recipient_ids: vec![OTHER], ..Default::default() });
        let mut s = AppState::default();
        s.apply_ready(r);
        assert_eq!(s.dm_order, vec![Id(301), DM, Id(302)]);
        create(&mut s, msg(800, Id(302), OTHER), None);
        assert_eq!(s.dm_order, vec![Id(302), Id(301), DM]);
        // A message in an unknown DM creates it.
        let n = create(&mut s, Message { guild_id: None, ..msg(900, Id(303), OTHER) }, None);
        assert!(n.is_some());
        assert_eq!(s.dm_order[0], Id(303));
        assert_eq!(s.channels[&Id(303)].recipient_ids, vec![OTHER]);
        assert_eq!(s.mention_count(Id(303)), 1);
        // Unknown guild channels are ignored.
        assert!(create(&mut s, msg(901, Id(9999), OTHER), None).is_none());
        assert!(!s.channels.contains_key(&Id(9999)));
    }

    #[test]
    fn blocked_authors_and_unread_updates() {
        let mut s = state();
        let mut r = ready();
        r.relationships = vec![Relationship { id: OTHER, kind: 2, ..Default::default() }];
        s.apply_ready(r);
        assert!(create(&mut s, mention(msg(1001, TEXT, OTHER)), None).is_none());
        assert_eq!(s.mention_count(TEXT), 0);
        assert_eq!(s.channels[&TEXT].last_message_id, Some(Id(1001)));
        s.apply_gateway(GatewayEvent::RelationshipRemove { id: OTHER }, None);
        assert!(create(&mut s, mention(msg(1002, TEXT, OTHER)), None).is_some());
        assert_eq!(s.mention_count(TEXT), 1);
        s.apply_gateway(GatewayEvent::RelationshipUpsert(Relationship { id: OTHER, kind: 1, user_ignored: true, ..Default::default() }), None);
        assert!(create(&mut s, mention(msg(1003, TEXT, OTHER)), None).is_none());
        assert_eq!(s.mention_count(TEXT), 1);
        // CHANNEL_UNREAD_UPDATE only ever raises last_message_id.
        let updates = vec![(TEXT, Some(Id(900))), (DM, Some(Id(600))), (Id(9999), Some(Id(1)))];
        s.apply_gateway(GatewayEvent::ChannelLastMessage { updates }, None);
        assert_eq!(s.channels[&TEXT].last_message_id, Some(Id(1003)));
        assert_eq!(s.channels[&DM].last_message_id, Some(Id(600)));
    }

    #[test]
    fn timeout_leaves_only_view_and_history() {
        let mut s = state();
        let timeout = |t: &str| Member { roles: vec![ROLE_A], communication_disabled_until: Some(t.into()), ..Default::default() };
        s.apply_gateway(GatewayEvent::MyMemberUpdate { guild_id: G, member: timeout(FUTURE) }, None);
        assert!(s.can_view(TEXT) && !s.can_send(TEXT) && !s.can_react(TEXT));
        assert!(s.channel_permissions(TEXT) & perm::READ_MESSAGE_HISTORY != 0);
        s.apply_gateway(GatewayEvent::MyMemberUpdate { guild_id: G, member: timeout(PAST) }, None);
        assert!(s.can_send(TEXT) && s.can_react(TEXT));
    }

    #[test]
    fn message_ack_updates_read_state() {
        let mut s = state();
        create(&mut s, mention(msg(1001, TEXT, OTHER)), None);
        s.apply_gateway(GatewayEvent::MessageAck { channel_id: TEXT, message_id: Id(1001), mention_count: None }, None);
        assert_eq!(s.read_states[&TEXT].last_message_id, Some(Id(1001)));
        assert_eq!(s.mention_count(TEXT), 1, "an omitted mention_count is kept");
        s.apply_gateway(GatewayEvent::MessageAck { channel_id: TEXT, message_id: Id(1001), mention_count: Some(0) }, None);
        assert!(!s.is_unread(TEXT));
    }

    #[test]
    fn channel_guild_and_user_updates() {
        let mut s = state();
        create(&mut s, msg(1001, TEXT, OTHER), None);
        let renamed = Channel { name: Some("renamed".into()), last_message_id: Some(Id(900)), ..chan(TEXT, 0, Some(CAT), 0) };
        s.apply_gateway(GatewayEvent::ChannelUpdate(Box::new(renamed)), None);
        assert_eq!(s.channel_name(TEXT), "renamed");
        assert_eq!(s.channels[&TEXT].last_message_id, Some(Id(1001)), "CHANNEL_UPDATE carries no fresh last_message_id");
        // GUILD_UPDATE merges; an absent role list doesn't wipe roles.
        s.apply_gateway(GatewayEvent::GuildUpdate(Box::new(Guild { id: G, name: "New".into(), owner_id: Id(999), ..Default::default() })), None);
        assert_eq!((s.guilds[0].name.as_str(), s.guilds[0].roles.len()), ("New", 3));
        // The boost mark follows GUILD_UPDATE (features and count are replaced, not kept).
        let marked = Guild { id: G, features: vec!["VERIFIED".into()], premium_tier: 2, premium_subscription_count: 9, ..Default::default() };
        s.apply_gateway(GatewayEvent::GuildUpdate(Box::new(marked)), None);
        assert_eq!((s.guilds[0].features.len(), s.guilds[0].premium_subscription_count), (1, 9));
        s.apply_gateway(GatewayEvent::GuildUpdate(Box::new(Guild { id: G, ..Default::default() })), None);
        assert_eq!((s.guilds[0].features.len(), s.guilds[0].premium_subscription_count, s.guilds[0].name.as_str()), (0, 0, "New"));
        // A repeated GUILD_CREATE replaces the channel list and repairs the selection.
        let g = Guild { id: G, name: "New".into(), owner_id: Id(999), channels: vec![chan(Id(260), 0, None, 0)], ..Default::default() };
        s.apply_gateway(GatewayEvent::GuildCreate(Box::new(g)), None);
        assert!(!s.channels.contains_key(&TEXT) && !s.channels.contains_key(&CAT));
        assert_eq!(s.selected_channel, Some(Id(260)));
        assert_eq!(s.guilds.len(), 1);
        s.apply_gateway(GatewayEvent::GuildCreate(Box::new(Guild { id: Id(110), name: "Other".into(), ..Default::default() })), None);
        assert_eq!(s.guilds.len(), 2);
        s.apply_gateway(GatewayEvent::GuildEmojisUpdate { guild_id: G, emojis: vec![CustomEmoji::default()] }, None);
        assert_eq!(s.guilds[0].emojis.len(), 1);
        s.apply_gateway(GatewayEvent::UserUpdate(user(ME, "renamed_me")), None);
        assert_eq!(s.me.as_ref().unwrap().username, "renamed_me");
        s.apply_gateway(GatewayEvent::GuildSettingsUpdate(settings(Some(G), 2)), None);
        assert_eq!(s.guild_settings[&Some(G)].message_notifications, 2);
    }

    #[test]
    fn snapshot_round_trip() {
        let mut s = state();
        loaded(&mut s, TEXT, 1..=60);
        s.select_channel(DM);
        loaded(&mut s, DM, [400, 450]);
        s.guild_settings.insert(Some(G), settings(Some(G), 1));
        let json = serde_json::to_string(&s.to_snapshot()).unwrap();
        let snap: Snapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(snap.guilds[0].channels.len(), 2);
        let recent: Vec<(Id, usize)> = snap.recent_messages.iter().map(|(id, m)| (*id, m.len())).collect();
        assert_eq!(recent, vec![(DM, 2), (TEXT, SNAPSHOT_MESSAGES)]);
        assert!(snap.users.iter().any(|u| u.id == OTHER));

        let r = AppState::from_snapshot(snap);
        assert!(r.has_data && !r.dirty);
        assert_eq!((r.selected_guild, r.selected_channel), (None, Some(DM)));
        assert_eq!(r.last_channel_per_guild, s.last_channel_per_guild);
        assert_eq!(r.channels.len(), s.channels.len());
        assert_eq!(r.channels[&TEXT].guild_id, Some(G));
        assert!(r.guilds[0].channels.is_empty());
        assert_eq!(r.dm_order, s.dm_order);
        assert_eq!(r.messages[&TEXT].msgs.first().map(|m| m.id), Some(Id(11)));
        assert!(r.messages[&TEXT].loaded && r.messages[&TEXT].has_more_before);
        assert!(r.fresh_channels.is_empty());
        assert_eq!(r.read_states.len(), s.read_states.len());
        assert_eq!(r.guild_settings[&Some(G)].message_notifications, 1);
        assert_eq!(r.my_members[&G].roles, vec![ROLE_A]);
        assert_eq!(r.recent_channels, vec![DM, TEXT]);
        assert_eq!(r.channel_name(DM), s.channel_name(DM));
        assert_eq!(r.sidebar_channels(G), s.sidebar_channels(G));
        assert!(!AppState::from_snapshot(Snapshot::default()).has_data);

        // A snapshot with more channels than the recent list: the first ones keep messages.
        let mut snap = s.to_snapshot();
        for i in 0..SNAPSHOT_CHANNELS as u64 {
            add_channel(&mut s, chan(Id(500 + i), 0, Some(CAT), 9));
            snap.guilds[0].channels.push(s.channels[&Id(500 + i)].clone());
            snap.recent_messages.push((Id(500 + i), vec![msg(600 + i, Id(500 + i), OTHER)]));
        }
        let r = AppState::from_snapshot(snap);
        assert_eq!(r.messages.len(), SNAPSHOT_CHANNELS);
        assert_eq!(r.recent_channels[..3], [DM, TEXT, Id(500)]);
    }

    #[test]
    fn unread_marker_survives_acks_and_follows_ready() {
        let mut s = state();
        assert_eq!(s.selected_channel, Some(TEXT));
        assert_eq!(s.unread_marker, None, "nothing unread");
        // Cache with unread messages: captured before the first ack, kept through acks.
        s.channels.get_mut(&TEXT).unwrap().last_message_id = Some(Id(1003));
        let mut r = AppState::from_snapshot(s.to_snapshot());
        assert_eq!(r.unread_marker, Some((TEXT, Id(1000))));
        assert_eq!(r.mark_read(TEXT), Some(Id(1003)));
        assert_eq!(r.unread_marker, Some((TEXT, Id(1000))));
        // READY with newer messages and the server's read position: recomputed.
        let mut fresh = ready();
        fresh.guilds[0].channels[1].last_message_id = Some(Id(1009));
        fresh.read_states[0].last_message_id = Some(Id(1005));
        r.apply_ready(fresh);
        assert_eq!(r.unread_marker, Some((TEXT, Id(1005))));
        // Re-selecting the open channel keeps it; another channel recaptures (nothing unread).
        r.mark_read(TEXT);
        r.select_channel(TEXT);
        assert_eq!(r.unread_marker, Some((TEXT, Id(1005))));
        r.select_channel(DM);
        assert_eq!(r.unread_marker, None);
        r.apply_ready(ready());
        assert_eq!(r.unread_marker, None, "READY with nothing unread clears it");
    }

    #[test]
    fn fixture_snapshot_loads() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/home/cache/snapshot.json");
        let snap: Snapshot = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let s = AppState::from_snapshot(snap);
        assert!(s.has_data);
        let ch = s.selected_channel.expect("selected channel");
        assert!(s.can_view(ch));
        assert_eq!(s.channels[&ch].guild_id, s.selected_guild);
        assert!(!s.sidebar_channels(s.selected_guild.unwrap()).is_empty());
        assert!(!s.messages[&ch].msgs.is_empty());
        assert!(s.is_unread(ch) && s.mention_count(ch) == 1);
        assert_eq!(s.dm_order.len(), 4);
    }

    #[test]
    fn voice_states_follow_updates_and_guild_payloads() {
        const VC: Id = Id(210);
        const VC2: Id = Id(211);
        let vs = |guild: Option<Id>, ch: Option<Id>, user: Id| VoiceState { guild_id: guild, channel_id: ch, user_id: user, ..Default::default() };
        let upd = |s: &mut AppState, v: VoiceState| s.apply_gateway(GatewayEvent::VoiceStateUpdate(Box::new(v)), None);
        let in_ch = |s: &AppState, ch: Id| -> Vec<Id> {
            let mut ids: Vec<Id> = s.voice_states.get(&ch).map(|m| m.keys().copied().collect()).unwrap_or_default();
            ids.sort();
            ids
        };
        let mut s = state();
        s.dirty = false;

        // Join (user object moves into `users`), move, then leave.
        let mut joined = vs(Some(G), Some(VC), OTHER);
        joined.member = Some(Member { user: Some(user(OTHER, "voice_name")), nick: Some("N".into()), ..Default::default() });
        upd(&mut s, joined);
        assert_eq!(in_ch(&s, VC), [OTHER]);
        assert_eq!(s.users[&OTHER].username, "voice_name");
        assert!(s.voice_states[&VC][&OTHER].member.as_ref().unwrap().user.is_none());
        assert!(!s.dirty, "voice is not cached");
        upd(&mut s, vs(Some(G), Some(VC2), OTHER));
        assert!(!s.voice_states.contains_key(&VC), "empty channels are dropped");
        assert_eq!(in_ch(&s, VC2), [OTHER]);
        // Leaving another guild's channel does not touch this guild.
        upd(&mut s, vs(Some(Id(999)), None, OTHER));
        assert_eq!(in_ch(&s, VC2), [OTHER]);
        upd(&mut s, vs(Some(G), None, OTHER));
        assert!(s.voice_states.is_empty());

        // GUILD_CREATE replaces the guild's list; DM calls are kept apart.
        upd(&mut s, vs(Some(G), Some(VC), ME));
        upd(&mut s, vs(None, Some(DM), OTHER));
        let g = Guild { id: G, name: "Guild".into(), voice_states: vec![vs(None, Some(VC2), OTHER)], ..Default::default() };
        s.apply_gateway(GatewayEvent::GuildCreate(Box::new(g)), None);
        assert!(!s.voice_states.contains_key(&VC));
        assert_eq!(in_ch(&s, VC2), [OTHER]);
        assert_eq!(s.voice_states[&VC2][&OTHER].guild_id, Some(G));
        assert!(s.guilds.iter().all(|g| g.voice_states.is_empty()));
        assert_eq!(in_ch(&s, DM), [OTHER]);

        // Unavailable guild / deleted channel / new READY drop their states.
        s.apply_gateway(GatewayEvent::GuildDelete { id: G, unavailable: true }, None);
        assert!(!s.voice_states.contains_key(&VC2));
        assert_eq!(in_ch(&s, DM), [OTHER]);
        s.apply_gateway(GatewayEvent::ChannelDelete { id: DM, guild_id: None }, None);
        assert!(s.voice_states.is_empty());
        upd(&mut s, vs(Some(G), Some(VC), ME));
        s.apply_ready(ready());
        assert!(s.voice_states.is_empty());
    }

    #[test]
    fn voice_events_update_status() {
        use crate::voice::VoiceEvent as V;
        let mut s = AppState::default();
        assert_eq!(s.apply_voice(V::Connecting { channel_id: TEXT }), None);
        assert_eq!((s.voice.channel_id, s.voice.connected), (Some(TEXT), false));
        s.apply_voice(V::Connected { channel_id: TEXT });
        s.apply_voice(V::Speaking { user_id: OTHER, speaking: true });
        s.apply_voice(V::Speaking { user_id: ME, speaking: true });
        s.apply_voice(V::Speaking { user_id: ME, speaking: false });
        assert!(s.voice.connected && s.voice.speaking.len() == 1);
        assert_eq!(s.apply_voice(V::Error("no mic".into())).as_deref(), Some("no mic"));
        assert_eq!(s.apply_voice(V::Disconnected { reason: Some("kicked".into()) }).as_deref(), Some("kicked"));
        assert!(s.voice.channel_id.is_none() && s.voice.speaking.is_empty());
        assert_eq!(s.apply_voice(V::Disconnected { reason: None }), None);
        s.apply_voice(V::Devices { inputs: vec!["Mic".into()], outputs: vec![] });
        assert_eq!(s.voice.inputs, ["Mic"]);
    }
}
