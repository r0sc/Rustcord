//! Chat-essentials state (FEATURES_NOTES Part 1): typing indicators, presences, my status,
//! attachment sends, the pins / search / GIF / jump / reaction-user views and the op 8
//! member-search cache. Pure data + logic like the rest of `state`; every map is bounded.
//!
//! Views: the UI opens one by setting it (e.g. `state.chat.pins = Some(PinsView { channel_id,
//! loading: true, ..Default::default() })`) and sending the matching `ChatCommand`;
//! `AppState::apply_chat` fills it and drops answers that no longer match the open view.

use super::AppState;
use crate::backend::{ChatEvent, SearchOutcome, SearchQuery, SearchScope, UploadFile};
use crate::model::*;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// A typing indicator lasts this long after the last TYPING_START (UD).
pub const TYPING_TIMEOUT: Duration = Duration::from_secs(10);
/// Presence entries kept (~150 B each); far above friends + one member list.
const MAX_PRESENCES: usize = 5000;
/// Op 8 answers kept for autocomplete.
const MEMBER_SEARCH_CACHE: usize = 16;
const MIB: u64 = 1024 * 1024;

#[derive(Debug, Default)]
pub struct ChatState {
    /// channel -> user -> typing until. Only live entries (pruned on every insert and by
    /// `AppState::prune_typing`).
    pub typing: HashMap<Id, HashMap<Id, Typist>>,
    /// Online / idle / dnd users; offline users have no entry. Status is global; activities
    /// are whatever arrived last.
    pub presences: HashMap<Id, UserPresence>,
    /// My chosen status and custom status (None until the gateway reports them).
    pub my_status: Option<UserStatus>,
    pub my_custom_status: Option<CustomStatus>,
    /// Attachment sends by nonce. The UI inserts one next to its `PendingMessage` when it sends
    /// `SendWithFiles`; kept after a failure (Retry re-sends the files), removed on success
    /// and by the UI on Dismiss / cancel.
    pub uploads: HashMap<String, Upload>,
    /// Slowmode: sending in a channel is blocked until then (from `SendCooldown`).
    pub send_cooldowns: HashMap<Id, Instant>,
    pub pins: Option<PinsView>,
    pub search: Option<SearchView>,
    pub gifs: Option<GifView>,
    /// Jump-to-message window (`FetchAround`), separate from the channel history.
    pub around: Option<AroundView>,
    pub reaction_users: Option<ReactionUsersView>,
    /// Who voted for one answer (the voters popover, ui/polls.rs).
    pub poll_voters: Option<PollVotersView>,
    /// Recent op 8 answers, oldest first.
    pub member_search: VecDeque<MemberSearch>,
    /// Marked unread by the user: `mark_read` (automatic acks) skips this channel until the
    /// user opens another channel or sends a message here.
    pub ack_paused: Option<Id>,
}

#[derive(Debug, Clone)]
pub struct Typist {
    pub until: Instant,
    /// Guild nickname from TYPING_START's `member`.
    pub nick: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Upload {
    pub files: Vec<UploadFile>,
    /// From `UploadProgress` (bytes); 0/0 = not started or failed.
    pub sent: u64,
    pub total: u64,
}

#[derive(Debug, Default)]
pub struct PinsView {
    pub channel_id: Id,
    /// Newest pin first; paging appends (`before` = last `pinned_at`).
    pub items: Vec<PinnedMessage>,
    pub has_more: bool,
    pub loading: bool,
    pub error: Option<String>,
    /// CHANNEL_PINS_UPDATE arrived while open: refetch from the start.
    pub stale: bool,
}

#[derive(Debug)]
pub struct SearchView {
    pub scope: SearchScope,
    pub query: SearchQuery,
    /// None while loading.
    pub outcome: Option<SearchOutcome>,
}

#[derive(Debug, Default)]
pub struct GifView {
    /// None = trending.
    pub query: Option<String>,
    /// None while loading.
    pub result: Option<Result<Vec<GifItem>, String>>,
}

#[derive(Debug, Default)]
pub struct AroundView {
    pub channel_id: Id,
    pub message_id: Id,
    /// Sorted ascending; None while loading.
    pub result: Option<Result<Vec<Message>, String>>,
}

#[derive(Debug, Default)]
pub struct ReactionUsersView {
    pub channel_id: Id,
    pub message_id: Id,
    pub emoji: Emoji,
    pub users: Vec<User>,
    /// A full page (100) came back: fetch more with `after` = last user id.
    pub has_more: bool,
    pub loading: bool,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub struct PollVotersView {
    pub channel_id: Id,
    pub message_id: Id,
    pub answer_id: u32,
    pub users: Vec<User>,
    /// A full page (100) came back: fetch more with `after` = last user id.
    pub has_more: bool,
    pub loading: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MemberSearch {
    pub guild_id: Id,
    /// Lowercased.
    pub query: String,
    pub members: Vec<Member>,
}

impl AppState {
    /// Apply a `BackendEvent::Chat`. Returns user-facing text worth a toast.
    pub fn apply_chat(&mut self, ev: ChatEvent) -> Option<String> {
        use ChatEvent as E;
        let c = &mut self.chat;
        match ev {
            E::UploadProgress { nonce, sent, total } => {
                if let Some(u) = c.uploads.get_mut(&nonce) {
                    (u.sent, u.total) = (sent, total);
                }
            }
            E::SendCooldown { channel_id, ms } => {
                let now = Instant::now();
                c.send_cooldowns.retain(|_, until| *until > now);
                c.send_cooldowns.insert(channel_id, now + Duration::from_millis(ms));
            }
            E::PinsLoaded { channel_id, before, result } => {
                let v = c.pins.as_mut().filter(|v| v.channel_id == channel_id)?;
                v.loading = false;
                match result {
                    Ok(page) => {
                        if before.is_none() {
                            v.items.clear();
                            v.stale = false;
                        }
                        v.items.extend(page.items);
                        v.has_more = page.has_more;
                        v.error = None;
                    }
                    Err(e) => v.error = Some(e),
                }
            }
            E::SearchResults { scope, query, result } => {
                if let Some(v) = c.search.as_mut().filter(|v| v.scope == scope && v.query == query) {
                    v.outcome = Some(result);
                }
            }
            E::MembersFound { guild_id, query, members } => {
                for u in members.iter().filter_map(|m| m.user.as_ref()) {
                    self.users.insert(u.id, u.clone());
                }
                let query = query.trim().to_lowercase();
                let cache = &mut self.chat.member_search;
                cache.retain(|s| !(s.guild_id == guild_id && s.query == query));
                cache.push_back(MemberSearch { guild_id, query, members });
                if cache.len() > MEMBER_SEARCH_CACHE {
                    cache.pop_front();
                }
            }
            E::GifResults { query, result } => {
                if let Some(v) = c.gifs.as_mut().filter(|v| v.query == query) {
                    v.result = Some(result);
                }
            }
            E::AroundLoaded { channel_id, message_id, result } => {
                let v = c.around.as_mut().filter(|v| (v.channel_id, v.message_id) == (channel_id, message_id))?;
                let toast = match &result {
                    Err(e) => Some(format!("Could not jump to the message: {e}")),
                    Ok(page) if !page.iter().any(|m| m.id == message_id) => Some("Message not found.".to_owned()),
                    Ok(_) => None,
                };
                v.result = Some(result.map(|mut page| {
                    page.sort_by_key(|m| m.id);
                    page.dedup_by_key(|m| m.id);
                    page
                }));
                return toast;
            }
            E::ReactionUsers { channel_id, message_id, emoji, after, result } => {
                let v = c
                    .reaction_users
                    .as_mut()
                    .filter(|v| (v.channel_id, v.message_id) == (channel_id, message_id) && v.emoji == emoji)?;
                v.loading = false;
                match result {
                    Ok(users) => {
                        if after.is_none() {
                            v.users.clear();
                        }
                        v.has_more = users.len() >= 100;
                        v.users.extend(users);
                        v.error = None;
                    }
                    Err(e) => v.error = Some(e),
                }
            }
            E::PollVoters { channel_id, message_id, answer_id, after, result } => {
                let v = c
                    .poll_voters
                    .as_mut()
                    .filter(|v| (v.channel_id, v.message_id, v.answer_id) == (channel_id, message_id, answer_id))?;
                v.loading = false;
                match result {
                    Ok(users) => {
                        if after.is_none() {
                            v.users.clear();
                        }
                        v.has_more = users.len() >= 100;
                        v.users.extend(users);
                        v.error = None;
                    }
                    Err(e) => v.error = Some(e),
                }
            }
            E::PollVoteFailed { channel_id, message_id, restore, error } => {
                self.set_my_poll_votes(channel_id, message_id, &restore);
                return Some(format!("Could not vote: {error}"));
            }
            // Answered by the UI (ui/mod.rs).
            E::Forwarded { .. } | E::PollPosted { .. } => {}
        }
        None
    }

    /// Channels and DMs a message can be forwarded to (the Forward dialog): text chats I can
    /// send in, no threads. A query is a lowercase substring of the channel's or its server's
    /// name. Newest activity first, DMs and server channels mixed.
    pub fn forward_targets(&self, query: &str, limit: usize) -> Vec<Id> {
        let q = query.trim().to_lowercase();
        let mut found: Vec<(Option<Id>, Id)> = self
            .channels
            .values()
            .filter(|c| c.is_text_chat() && !c.is_thread() && self.can_send(c.id))
            .filter(|c| {
                q.is_empty()
                    || self.channel_name(c.id).to_lowercase().contains(&q)
                    || c.guild_id.and_then(|g| self.guild(g)).is_some_and(|g| g.name.to_lowercase().contains(&q))
            })
            .map(|c| (c.last_message_id, c.id))
            .collect();
        found.sort_by(|a, b| b.cmp(a));
        found.into_iter().take(limit).map(|(_, id)| id).collect()
    }

    /// Display names of who is typing in a channel (earliest first; me, blocked and expired
    /// excluded). Names: guild nick > global name > username.
    pub fn typing_users(&self, channel_id: Id, now: Instant) -> Vec<String> {
        let Some(typists) = self.chat.typing.get(&channel_id) else { return Vec::new() };
        let guild_id = self.channels.get(&channel_id).and_then(|c| c.guild_id);
        let mut live: Vec<(Instant, String)> = typists
            .iter()
            .filter(|(_, t)| t.until > now)
            .filter_map(|(id, t)| {
                let name = match (crate::names::name(*id), &t.nick) {
                    (Some(local), _) => local,
                    (None, Some(n)) => n.clone(),
                    (None, None) => self.member_display_name(guild_id, self.users.get(id)?, None),
                };
                Some((t.until, name))
            })
            .collect();
        live.sort();
        live.into_iter().map(|(_, n)| n).collect()
    }

    /// The line under the composer: "A is typing…", "A and B are typing…",
    /// "A, B, and C are typing…", "Several people are typing…".
    pub fn typing_text(&self, channel_id: Id, now: Instant) -> Option<String> {
        Some(match self.typing_users(channel_id, now).as_slice() {
            [] => return None,
            [a] => format!("{a} is typing…"),
            [a, b] => format!("{a} and {b} are typing…"),
            [a, b, c] => format!("{a}, {b}, and {c} are typing…"),
            _ => "Several people are typing…".to_owned(),
        })
    }

    /// Drops expired typing entries; returns when the next one expires (for
    /// `request_repaint_after` while someone is typing).
    pub fn prune_typing(&mut self, now: Instant) -> Option<Duration> {
        prune_typing(&mut self.chat.typing, now);
        self.chat.typing.values().flat_map(|m| m.values()).map(|t| t.until.saturating_duration_since(now)).min()
    }

    /// Status of any user (offline when unknown).
    pub fn presence(&self, user_id: Id) -> Presence {
        self.chat.presences.get(&user_id).map_or(Presence::Offline, |p| p.status)
    }

    /// Full presence (phone glyph, custom status, activity line); None = offline.
    pub fn presence_of(&self, user_id: Id) -> Option<&UserPresence> {
        self.chat.presences.get(&user_id)
    }

    /// Largest file I may attach in a channel (bytes): `max(my Nitro limit, guild boost
    /// limit)`; DMs use mine. The server has the final word.
    pub fn upload_limit(&self, channel_id: Id) -> u64 {
        let premium = self.me.as_ref().map_or(0, |u| u.premium_type);
        let tier = (self.channels.get(&channel_id).and_then(|c| c.guild_id))
            .and_then(|g| self.guild(g))
            .map_or(0, |g| g.premium_tier);
        max_upload_bytes(premium, tier)
    }

    /// "Mark unread" on `message_id`: moves my read state to just before it and pauses
    /// automatic acks here. Returns `(ack target, mention count)` for `ChatCommand::MarkUnread`.
    pub fn mark_unread(&mut self, channel_id: Id, message_id: Id) -> Option<(Id, u32)> {
        let me = self.me.as_ref()?.id;
        let dm = self.channels.get(&channel_id)?.is_dm_like();
        let msgs = self.messages.get(&channel_id).map_or(&[][..], |e| &e.msgs[..]);
        let i = msgs.partition_point(|m| m.id < message_id);
        // The target need not exist, it only has to be a valid snowflake below the message.
        let target = if i > 0 { msgs[i - 1].id } else { Id(message_id.0.saturating_sub(1)) };
        let mentions = msgs[i..]
            .iter()
            .filter(|m| m.author.id != me && !self.hidden_authors.contains(&m.author.id) && (dm || self.mentions_me(m)))
            .count() as u32;
        let rs = self.read_states.entry(channel_id).or_insert_with(|| ReadState { id: channel_id, ..Default::default() });
        rs.last_message_id = Some(target);
        rs.mention_count = mentions;
        self.chat.ack_paused = Some(channel_id);
        self.dirty = true;
        Some((target, mentions))
    }

    /// Whether `@query` needs an op 8 request: not for an empty query, one already answered,
    /// or one whose answered prefix returned fewer than `limit` members (that set was complete).
    pub fn need_member_search(&self, guild_id: Id, query: &str, limit: u8) -> bool {
        let q = query.trim().to_lowercase();
        !q.is_empty()
            && !self.chat.member_search.iter().any(|s| {
                s.guild_id == guild_id && q.starts_with(&s.query) && (s.query == q || s.members.len() < limit as usize)
            })
    }

    // ----- gateway hooks (called from `apply_gateway` & co.) -------------------------

    pub(super) fn on_typing_start(&mut self, channel_id: Id, user_id: Id, member: Option<Box<Member>>, now: Instant) {
        if self.is_me(user_id) || self.hidden_authors.contains(&user_id) {
            return;
        }
        let nick = member.as_ref().and_then(|m| m.nick.clone()).filter(|n| !n.is_empty());
        if let Some(u) = member.and_then(|m| m.user) {
            self.users.insert(u.id, u);
        }
        prune_typing(&mut self.chat.typing, now);
        self.chat.typing.entry(channel_id).or_default().insert(user_id, Typist { until: now + TYPING_TIMEOUT, nick });
    }

    /// A message from `user_id` ends their typing indicator; my own one ends the ack pause.
    pub(super) fn on_chat_message(&mut self, channel_id: Id, user_id: Id) {
        if let Some(t) = self.chat.typing.get_mut(&channel_id) {
            t.remove(&user_id);
            if t.is_empty() {
                self.chat.typing.remove(&channel_id);
            }
        }
        if self.is_me(user_id) && self.chat.ack_paused == Some(channel_id) {
            self.chat.ack_paused = None;
        }
    }

    pub(super) fn on_presences(&mut self, list: Vec<(Id, UserPresence)>) {
        let p = &mut self.chat.presences;
        for (id, presence) in list {
            if presence.status == Presence::Offline {
                p.remove(&id);
                continue;
            }
            if p.len() >= MAX_PRESENCES
                && !p.contains_key(&id)
                && let Some(k) = p.keys().next().copied()
            {
                // shortcut: evicts an arbitrary user; fine while the cap is far above friends +
                // member-list counts, use LRU if presences of huge guilds are ever stored.
                p.remove(&k);
            }
            p.insert(id, presence);
        }
    }

    pub(super) fn on_pins_update(&mut self, channel_id: Id, last_pin_timestamp: Option<String>) {
        if let Some(ch) = self.channels.get_mut(&channel_id) {
            ch.last_pin_timestamp = last_pin_timestamp;
        }
        if let Some(v) = self.chat.pins.as_mut().filter(|v| v.channel_id == channel_id) {
            v.stale = true;
        }
    }

    /// New gateway session: typing and presences are re-sent from scratch.
    pub(super) fn on_chat_session(&mut self) {
        self.chat.typing.clear();
        self.chat.presences.clear();
    }

    /// Upload bookkeeping of a `SendResult`.
    pub(super) fn on_chat_send_result(&mut self, nonce: &str, ok: bool) {
        if ok {
            self.chat.uploads.remove(nonce);
        } else if let Some(u) = self.chat.uploads.get_mut(nonce) {
            (u.sent, u.total) = (0, 0);
        }
    }
}

fn prune_typing(typing: &mut HashMap<Id, HashMap<Id, Typist>>, now: Instant) {
    for m in typing.values_mut() {
        m.retain(|_, t| t.until > now);
    }
    typing.retain(|_, m| !m.is_empty());
}

/// Per-file limit (FEATURES_NOTES §1.1): 20 MiB default (since 2026-09-03), Nitro Classic /
/// Basic 50, Nitro 500; boost tier 2 -> 50, tier 3 -> 100.
fn max_upload_bytes(premium_type: u8, guild_tier: u8) -> u64 {
    let user = match premium_type {
        2 => 500,
        1 | 3 => 50,
        _ => 20,
    };
    let guild = match guild_tier {
        3 => 100,
        2 => 50,
        _ => 0,
    };
    user.max(guild) * MIB
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::SearchSort;

    const ME: Id = Id(1);
    const OTHER: Id = Id(2);
    const THIRD: Id = Id(3);
    const G: Id = Id(10);
    const TEXT: Id = Id(100);
    const DM: Id = Id(200);

    fn user(id: Id, name: &str) -> User {
        User { id, username: name.into(), ..Default::default() }
    }

    fn state() -> AppState {
        let mut s = AppState { me: Some(user(ME, "me")), ..Default::default() };
        s.guilds.push(Guild { id: G, premium_tier: 2, ..Default::default() });
        s.channels.insert(TEXT, Channel { id: TEXT, guild_id: Some(G), ..Default::default() });
        s.channels.insert(DM, Channel { id: DM, kind: channel_type::DM, recipient_ids: vec![OTHER], ..Default::default() });
        for (id, name) in [(OTHER, "other"), (THIRD, "third"), (Id(4), "fourth"), (Id(5), "fifth")] {
            s.users.insert(id, user(id, name));
        }
        s
    }

    fn msg(id: u64, channel: Id, author: Id) -> Message {
        Message { id: Id(id), channel_id: channel, author: user(author, "a"), ..Default::default() }
    }

    #[test]
    fn typing_shows_the_local_name_over_a_nick() {
        let mut s = state();
        let (t0, id) = (Instant::now(), Id(9_990_120));
        s.users.insert(id, user(id, "alice"));
        let nick = Member { nick: Some("Nick".into()), user: Some(user(id, "alice")), ..Default::default() };
        s.on_typing_start(TEXT, id, Some(Box::new(nick)), t0);
        assert_eq!(s.typing_users(TEXT, t0), ["Nick"]);
        crate::names::with_names(&[crate::names::CustomName { id, name: Some("Ali".into()), ..Default::default() }], || assert_eq!(s.typing_users(TEXT, t0), ["Ali"]));
    }

    #[test]
    fn typing_expires_ignores_me_and_ends_on_message() {
        let mut s = state();
        let t0 = Instant::now();
        let nick = Member { nick: Some("Nick".into()), user: Some(user(OTHER, "other")), ..Default::default() };
        s.on_typing_start(TEXT, OTHER, Some(Box::new(nick)), t0);
        s.on_typing_start(TEXT, ME, None, t0);
        s.on_typing_start(TEXT, THIRD, None, t0 + Duration::from_secs(1));
        assert_eq!(s.typing_users(TEXT, t0), ["Nick", "third"]);
        assert_eq!(s.typing_text(TEXT, t0).as_deref(), Some("Nick and third are typing…"));
        assert_eq!(s.prune_typing(t0), Some(TYPING_TIMEOUT));
        // Another event restarts the 10 s.
        s.on_typing_start(TEXT, OTHER, None, t0 + Duration::from_secs(5));
        assert_eq!(s.typing_users(TEXT, t0 + Duration::from_secs(10)), ["third", "other"]);
        assert_eq!(s.typing_text(TEXT, t0 + Duration::from_secs(11)).as_deref(), Some("other is typing…"));
        s.on_chat_message(TEXT, OTHER);
        assert_eq!(s.typing_text(TEXT, t0), Some("third is typing…".into()));
        assert_eq!(s.prune_typing(t0 + TYPING_TIMEOUT * 2), None);
        assert!(s.chat.typing.is_empty(), "expired channels are dropped");
        // Blocked users never show.
        s.hidden_authors.insert(OTHER);
        s.on_typing_start(TEXT, OTHER, None, t0);
        assert!(s.chat.typing.is_empty());
    }

    #[test]
    fn typing_text_rules() {
        let mut s = state();
        let t0 = Instant::now();
        for (i, id) in [OTHER, THIRD, Id(4)].into_iter().enumerate() {
            s.on_typing_start(DM, id, None, t0 + Duration::from_millis(i as u64));
        }
        assert_eq!(s.typing_text(DM, t0).as_deref(), Some("other, third, and fourth are typing…"));
        s.on_typing_start(DM, Id(5), None, t0);
        assert_eq!(s.typing_text(DM, t0).as_deref(), Some("Several people are typing…"));
        assert_eq!(s.typing_text(TEXT, t0), None);
    }

    #[test]
    fn presences_offline_removes_and_cap_holds() {
        let mut s = state();
        let online = UserPresence { status: Presence::Online, ..Default::default() };
        s.on_presences(vec![(OTHER, UserPresence { status: Presence::Dnd, mobile: true, ..Default::default() }), (THIRD, online.clone())]);
        assert_eq!((s.presence(OTHER), s.presence(Id(99))), (Presence::Dnd, Presence::Offline));
        s.on_presences(vec![(OTHER, UserPresence::default())]);
        assert_eq!(s.presence(OTHER), Presence::Offline);
        assert!(!s.chat.presences.contains_key(&OTHER));
        s.on_presences((0..MAX_PRESENCES as u64 + 10).map(|i| (Id(1000 + i), online.clone())).collect());
        assert_eq!(s.chat.presences.len(), MAX_PRESENCES);
        s.on_chat_session();
        assert!(s.chat.presences.is_empty());
    }

    #[test]
    fn upload_limits() {
        assert_eq!(max_upload_bytes(0, 0), 20 * MIB);
        assert_eq!(max_upload_bytes(3, 0), 50 * MIB);
        assert_eq!(max_upload_bytes(2, 3), 500 * MIB);
        assert_eq!(max_upload_bytes(0, 3), 100 * MIB);
        let mut s = state();
        assert_eq!(s.upload_limit(TEXT), 50 * MIB, "boost tier 2");
        assert_eq!(s.upload_limit(DM), 20 * MIB, "DMs: my own limit");
        s.me.as_mut().unwrap().premium_type = 2;
        assert_eq!(s.upload_limit(TEXT), 500 * MIB);
    }

    #[test]
    fn mark_unread_targets_previous_and_counts_mentions() {
        let mut s = state();
        let mention = |m: Message| Message { mentions: vec![user(ME, "me")], ..m };
        let msgs = vec![msg(10, TEXT, OTHER), msg(20, TEXT, OTHER), mention(msg(30, TEXT, OTHER)), mention(msg(40, TEXT, ME))];
        s.messages.insert(TEXT, super::super::ChannelMessages { msgs, loaded: true, ..Default::default() });
        assert_eq!(s.mark_unread(TEXT, Id(20)), Some((Id(10), 1)));
        assert_eq!(s.read_states[&TEXT].last_message_id, Some(Id(10)));
        assert_eq!(s.read_states[&TEXT].mention_count, 1);
        assert_eq!(s.chat.ack_paused, Some(TEXT));
        // Oldest loaded message: the target is one below it. DMs count every message.
        assert_eq!(s.mark_unread(DM, Id(500)), Some((Id(499), 0)));
        s.messages.insert(DM, super::super::ChannelMessages { msgs: vec![msg(500, DM, OTHER), msg(501, DM, OTHER)], ..Default::default() });
        assert_eq!(s.mark_unread(DM, Id(500)), Some((Id(499), 2)));
        // My message there ends the pause.
        s.on_chat_message(DM, ME);
        assert_eq!(s.chat.ack_paused, None);
    }

    #[test]
    fn member_search_cache_rules() {
        let mut s = state();
        let m = |id| Member { user: Some(user(Id(id), "x")), ..Default::default() };
        assert!(!s.need_member_search(G, "  ", 50));
        assert!(s.need_member_search(G, "Ab", 50));
        s.apply_chat(ChatEvent::MembersFound { guild_id: G, query: "Ab".into(), members: vec![m(7), m(8)] });
        assert!(s.users.contains_key(&Id(7)));
        assert!(!s.need_member_search(G, "ab", 50), "answered");
        assert!(!s.need_member_search(G, "abc", 50), "the shorter answer was complete");
        assert!(s.need_member_search(G, "abc", 2), "the shorter answer hit the limit");
        assert!(s.need_member_search(Id(11), "ab", 50), "other guild");
        for i in 0..MEMBER_SEARCH_CACHE as u64 + 3 {
            s.apply_chat(ChatEvent::MembersFound { guild_id: G, query: format!("q{i}"), members: Vec::new() });
        }
        assert_eq!(s.chat.member_search.len(), MEMBER_SEARCH_CACHE);
        assert!(s.need_member_search(G, "ab", 50), "evicted");
    }

    #[test]
    fn views_take_only_matching_answers() {
        let mut s = state();
        let pin = |id| PinnedMessage { pinned_at: format!("t{id}"), message: msg(id, TEXT, OTHER) };
        s.chat.pins = Some(PinsView { channel_id: TEXT, loading: true, ..Default::default() });
        s.apply_chat(ChatEvent::PinsLoaded { channel_id: DM, before: None, result: Ok(PinsPage { items: vec![pin(1)], has_more: false }) });
        assert!(s.chat.pins.as_ref().unwrap().items.is_empty(), "other channel ignored");
        s.apply_chat(ChatEvent::PinsLoaded { channel_id: TEXT, before: None, result: Ok(PinsPage { items: vec![pin(2)], has_more: true }) });
        s.apply_chat(ChatEvent::PinsLoaded { channel_id: TEXT, before: Some("t2".into()), result: Ok(PinsPage { items: vec![pin(1)], has_more: false }) });
        let v = s.chat.pins.as_ref().unwrap();
        assert_eq!((v.items.len(), v.has_more, v.loading), (2, false, false));
        s.on_pins_update(TEXT, Some("2026-11-12T00:00:00Z".into()));
        assert!(s.chat.pins.as_ref().unwrap().stale);
        assert_eq!(s.channels[&TEXT].last_pin_timestamp.as_deref(), Some("2026-11-12T00:00:00Z"));

        let q = SearchQuery { content: Some("hi".into()), ..Default::default() };
        s.chat.search = Some(SearchView { scope: SearchScope::Guild(G), query: q.clone(), outcome: None });
        let other = SearchQuery { sort: SearchSort::Oldest, ..q.clone() };
        s.apply_chat(ChatEvent::SearchResults { scope: SearchScope::Guild(G), query: other, result: SearchOutcome::Failed("x".into()) });
        assert!(s.chat.search.as_ref().unwrap().outcome.is_none(), "stale query ignored");
        s.apply_chat(ChatEvent::SearchResults { scope: SearchScope::Guild(G), query: q, result: SearchOutcome::Indexing { retry_after_secs: 2.0 } });
        assert!(matches!(s.chat.search.as_ref().unwrap().outcome, Some(SearchOutcome::Indexing { .. })));

        s.chat.around = Some(AroundView { channel_id: TEXT, message_id: Id(5), result: None });
        let toast = s.apply_chat(ChatEvent::AroundLoaded { channel_id: TEXT, message_id: Id(5), result: Ok(vec![msg(6, TEXT, OTHER), msg(4, TEXT, OTHER)]) });
        assert_eq!(toast.as_deref(), Some("Message not found."));
        let ids: Vec<Id> = s.chat.around.as_ref().unwrap().result.as_ref().unwrap().as_ref().unwrap().iter().map(|m| m.id).collect();
        assert_eq!(ids, [Id(4), Id(6)]);

        s.chat.uploads.insert("n".into(), Upload::default());
        s.apply_chat(ChatEvent::UploadProgress { nonce: "n".into(), sent: 5, total: 10 });
        assert_eq!((s.chat.uploads["n"].sent, s.chat.uploads["n"].total), (5, 10));
        s.on_chat_send_result("n", false);
        assert_eq!(s.chat.uploads["n"].sent, 0, "kept for Retry");
        s.on_chat_send_result("n", true);
        assert!(s.chat.uploads.is_empty());
    }

    #[test]
    fn poll_voters_fill_the_open_view_only() {
        let mut s = state();
        let view = |answer_id| PollVotersView { channel_id: TEXT, message_id: Id(5), answer_id, loading: true, ..Default::default() };
        let page = |n: u64| (1..=n).map(|i| user(Id(100 + i), "v")).collect::<Vec<_>>();
        let ev = |answer_id, after, result| ChatEvent::PollVoters { channel_id: TEXT, message_id: Id(5), answer_id, after, result };
        s.chat.poll_voters = Some(view(2));
        // Another answer's page (a stale answer) is dropped.
        s.apply_chat(ev(1, None, Ok(page(3))));
        assert!(s.chat.poll_voters.as_ref().unwrap().users.is_empty() && s.chat.poll_voters.as_ref().unwrap().loading);
        s.apply_chat(ev(2, None, Ok(page(100))));
        let v = s.chat.poll_voters.as_ref().unwrap();
        assert_eq!((v.users.len(), v.has_more, v.loading), (100, true, false));
        // The next page adds; a short one ends the list.
        s.apply_chat(ev(2, Some(Id(200)), Ok(page(5))));
        let v = s.chat.poll_voters.as_ref().unwrap();
        assert_eq!((v.users.len(), v.has_more), (105, false));
        // A first page replaces what was there; an error keeps the users and says why.
        s.apply_chat(ev(2, None, Ok(page(2))));
        assert_eq!(s.chat.poll_voters.as_ref().unwrap().users.len(), 2);
        s.apply_chat(ev(2, None, Err("nope".into())));
        let v = s.chat.poll_voters.as_ref().unwrap();
        assert_eq!((v.users.len(), v.error.as_deref()), (2, Some("nope")));
        s.chat.poll_voters = None;
        assert_eq!(s.apply_chat(ev(2, None, Ok(page(1)))), None);
    }

    #[test]
    fn forward_targets_are_sendable_non_threads_newest_first() {
        let mut s = state();
        let perms = perm::VIEW_CHANNEL | perm::SEND_MESSAGES;
        s.guilds[0].name = "Rust Club".into();
        s.guilds[0].owner_id = Id(999);
        s.guilds[0].roles = vec![Role { id: G, permissions: Perms(perms), ..Default::default() }];
        let ch = |id: u64, kind: u8, last: u64| Channel { id: Id(id), kind, guild_id: Some(G), name: Some(format!("room{id}")), last_message_id: Some(Id(last)), ..Default::default() };
        let denied = Channel { permission_overwrites: vec![Overwrite { id: G, kind: 0, deny: Perms(perm::SEND_MESSAGES), ..Default::default() }], ..ch(303, channel_type::GUILD_TEXT, 60) };
        for c in [
            ch(301, channel_type::GUILD_TEXT, 50),
            ch(302, channel_type::GUILD_TEXT, 70),
            denied,
            Channel { parent_id: Some(Id(301)), ..ch(304, channel_type::PUBLIC_THREAD, 99) },
            ch(305, channel_type::GUILD_FORUM, 98),
        ] {
            s.channels.insert(c.id, c);
        }
        s.channels.get_mut(&TEXT).unwrap().last_message_id = Some(Id(10));
        s.channels.get_mut(&DM).unwrap().last_message_id = Some(Id(90));
        // The DM first (newest), then 302, 301, and the old empty-named TEXT; no 303 (no send), thread or forum.
        assert_eq!(s.forward_targets("", 50), [DM, Id(302), Id(301), TEXT]);
        assert_eq!(s.forward_targets("", 2), [DM, Id(302)]);
        assert_eq!(s.forward_targets("ROOM301", 50), [Id(301)]);
        // The server's name matches all of its channels.
        assert_eq!(s.forward_targets("rust club", 50), [Id(302), Id(301), TEXT]);
        assert_eq!(s.forward_targets("nothing", 50), Vec::<Id>::new());
    }
}
