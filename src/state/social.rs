//! Social state (FEATURES_NOTES Part 2): relationships, thread bookkeeping, the thread/forum,
//! mentions and Add Friend views, and the profile cache. Pure data + logic like the rest of
//! `state`; every collection is bounded.
//!
//! Threads are channels: they live in `AppState::channels` (messages, permissions, read states
//! and selection work unchanged), `Channel::member.is_some()` = joined. Joined active threads
//! come from READY / GUILD_CREATE, the rest of the guild's active ones from THREAD_LIST_SYNC
//! (capped), archived ones are dropped unless open.
//!
//! Views: the UI opens one by setting it (e.g. `state.social.threads =
//! Some(ThreadsView::new(channel_id, list.clone()))`) and sending the matching command;
//! `AppState::apply_social` fills it and drops answers that no longer match.

/// Thread/forum permission gates, new-post checks, thread message counts.
mod threads;

use super::{AppState, NOTIFY_ALL, NOTIFY_NOTHING, is_thread};
use crate::backend::{ArchivedThreads, FriendReply, FriendTarget, MentionFilter, SocialEvent, ThreadList, ThreadQuery};
use crate::model::*;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

/// Profiles kept (LRU).
pub const PROFILE_CACHE: usize = 64;
/// A cached profile (or error) is shown without refetching for this long.
pub const PROFILE_TTL: Duration = Duration::from_secs(300);
/// Discord's own limits are 1,000 friends + 5,000 blocked + pending.
const MAX_RELATIONSHIPS: usize = 10_000;
/// Threads I have not joined kept in `channels` (THREAD_LIST_SYNC, opened from a list).
const MAX_UNJOINED_THREADS: usize = 1_000;
/// Threads (and first messages) one list view holds; paging stops there.
const MAX_VIEW_THREADS: usize = 500;
const MAX_MENTIONS: usize = 500;
/// Thread search page size; the search offset limit.
const THREAD_PAGE: u32 = 25;
const MAX_SEARCH_OFFSET: u32 = 9_975;
/// Products / games kept for the cosmetics display.
const MAX_COSMETIC_CACHE: usize = 256;
/// Previews one `FetchRequestPreviews` asks for (the API takes 1-25).
const PREVIEW_PAGE: usize = 25;

#[derive(Debug, Default)]
pub struct SocialState {
    /// By the other user's id. User objects live in `AppState::users`.
    pub relationships: HashMap<Id, Relationship>,
    /// Thread browser or forum post list (one at a time).
    pub threads: Option<ThreadsView>,
    /// Inbox "Mentions" tab.
    pub mentions: Option<MentionsView>,
    /// Add Friend tab: the name sent and its answer.
    pub add_friend: Option<AddFriendView>,
    /// Set by an 80013 answer: ask "accept a request from someone you don't share a server
    /// with?", then send `AcceptFriend { confirm_stranger: true }` (and clear this either way).
    pub confirm_stranger: Option<Id>,
    /// Profile cache, least recently requested first.
    pub profiles: VecDeque<ProfileEntry>,
    /// Collectibles catalog for the cosmetics editor (`FetchCollectibles` or its saved copy);
    /// None until the editor first opens.
    pub collectibles: Option<Result<Vec<crate::api::collectibles::CatalogItem>, String>>,
    /// A `FetchCollectibles` is on its way (`collectibles` is None then too).
    pub catalog_requested: bool,
    /// My owned collectibles (`FetchOwnedCollectibles`): the ones I may equip for real.
    pub owned_collectibles: Option<Result<Vec<crate::api::collectibles::CatalogItem>, String>>,
    /// Cosmetics display (ui/cosmetics.rs): collectibles products (effect / frame configs) and
    /// games by id. A key = requested once this session; None = loading or failed (not retried).
    pub products: HashMap<Id, Option<Box<CollectibleProduct>>>,
    pub games: HashMap<Id, Option<Game>>,
    /// Settings → Connections: my connections; None until the page first loads them.
    pub connections: Option<Result<Vec<ConnectedAccount>, String>>,
    /// The message requests page's previews; Some while the page is open (`ui/requests.rs`).
    pub request_previews: Option<RequestPreviews>,
    /// Requests being accepted or ignored (a command is on its way).
    pub request_busy: HashSet<Id>,
}

/// Trigger messages of the requests page. Asked a page at a time, only on a click.
#[derive(Debug, Default)]
pub struct RequestPreviews {
    /// By request channel.
    pub messages: HashMap<Id, Message>,
    /// Channels a `FetchRequestPreviews` was sent for (answered or on its way).
    pub asked: HashSet<Id>,
    pub loading: bool,
    pub error: Option<String>,
}

#[derive(Debug)]
pub struct ThreadsView {
    pub channel_id: Id,
    /// The newest page loaded (or requested, for the first one).
    pub list: ThreadList,
    /// In API order, deduplicated, ≤ 500. Joined state: prefer `AppState::channels[id].member`
    /// when the thread is known there (it follows the gateway).
    pub threads: Vec<Channel>,
    /// Forum posts' first messages (`first_message(thread_id)`).
    pub first_messages: Vec<Message>,
    pub has_more: bool,
    /// Thread search only.
    pub total_results: Option<u32>,
    pub loading: bool,
    pub error: Option<String>,
}

impl ThreadsView {
    /// A view waiting for its first page.
    pub fn new(channel_id: Id, list: ThreadList) -> Self {
        Self { channel_id, list, threads: Vec::new(), first_messages: Vec::new(), has_more: false, total_results: None, loading: true, error: None }
    }

    /// The list for the next page (set `loading` and send `FetchThreads` with it); None at the end
    /// or while a page is loading.
    pub fn next_page(&self) -> Option<ThreadList> {
        if !self.has_more || self.loading {
            return None;
        }
        Some(match &self.list {
            ThreadList::Active { offset } => ThreadList::Active { offset: next_offset(*offset)? },
            ThreadList::Forum(q) => ThreadList::Forum(ThreadQuery { offset: next_offset(q.offset)?, ..q.clone() }),
            ThreadList::Archived { kind, .. } => {
                let last = self.threads.last()?;
                let before = match kind {
                    ArchivedThreads::Joined => last.id.to_string(),
                    ArchivedThreads::Public | ArchivedThreads::Private => last.thread_metadata.as_ref()?.archive_timestamp.clone()?,
                };
                ThreadList::Archived { kind: *kind, before: Some(before) }
            }
        })
    }

    pub fn first_message(&self, thread_id: Id) -> Option<&Message> {
        self.first_messages.iter().find(|m| m.id == thread_id)
    }
}

fn next_offset(offset: u32) -> Option<u32> {
    Some(offset + THREAD_PAGE).filter(|o| *o <= MAX_SEARCH_OFFSET)
}

/// Same list, maybe another page.
fn same_listing(a: &ThreadList, b: &ThreadList) -> bool {
    match (a, b) {
        (ThreadList::Active { .. }, ThreadList::Active { .. }) => true,
        (ThreadList::Archived { kind: x, .. }, ThreadList::Archived { kind: y, .. }) => x == y,
        (ThreadList::Forum(x), ThreadList::Forum(y)) => {
            (&x.name, &x.tags, x.match_all_tags, x.archived, x.sort) == (&y.name, &y.tags, y.match_all_tags, y.archived, y.sort)
        }
        _ => false,
    }
}

fn is_first_page(l: &ThreadList) -> bool {
    match l {
        ThreadList::Active { offset } => *offset == 0,
        ThreadList::Forum(q) => q.offset == 0,
        ThreadList::Archived { before, .. } => before.is_none(),
    }
}

#[derive(Debug, Default)]
pub struct MentionsView {
    pub filter: MentionFilter,
    /// Newest first, ≤ 500.
    pub messages: Vec<Message>,
    pub has_more: bool,
    pub loading: bool,
    pub error: Option<String>,
}

impl MentionsView {
    pub fn new(filter: MentionFilter) -> Self {
        Self { filter, loading: true, ..Default::default() }
    }

    /// `before` for the next page; None at the end or while loading.
    pub fn next_before(&self) -> Option<Id> {
        self.messages.last().map(|m| m.id).filter(|_| self.has_more && !self.loading)
    }
}

#[derive(Debug, Default)]
pub struct AddFriendView {
    /// As typed and sent.
    pub username: String,
    /// None while sending.
    pub result: Option<Result<(), String>>,
}

#[derive(Debug)]
pub struct ProfileEntry {
    pub user_id: Id,
    pub guild_id: Option<Id>,
    /// When the last request went out / its answer arrived.
    pub fetched: Instant,
    pub loading: bool,
    /// The last answer; kept (shown) while a refetch runs. None = never answered.
    pub result: Option<Result<Box<UserProfile>, String>>,
}

/// Friends page tabs (§2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FriendsTab {
    /// Friends who are not offline.
    Online,
    All,
    /// Incoming and outgoing requests (spam excluded).
    Pending,
    /// Incoming requests filed as spam.
    Spam,
    /// Blocked or ignored.
    Blocked,
}

impl AppState {
    /// Apply a `BackendEvent::Social`. Returns user-facing text worth a toast.
    pub fn apply_social(&mut self, ev: SocialEvent) -> Option<String> {
        use SocialEvent as E;
        match ev {
            E::ThreadsLoaded { channel_id, list, result } => {
                let v = self.social.threads.as_mut().filter(|v| v.channel_id == channel_id && same_listing(&v.list, &list))?;
                v.loading = false;
                match result {
                    Ok(page) => {
                        if is_first_page(&list) {
                            v.threads.clear();
                            v.first_messages.clear();
                        }
                        let known: HashSet<Id> = v.threads.iter().map(|t| t.id).collect();
                        let room = MAX_VIEW_THREADS.saturating_sub(v.threads.len());
                        v.threads.extend(page.threads.into_iter().filter(|t| !known.contains(&t.id)).take(room));
                        let shown: HashSet<Id> = v.threads.iter().map(|t| t.id).collect();
                        let have: HashSet<Id> = v.first_messages.iter().map(|m| m.id).collect();
                        v.first_messages.extend(page.first_messages.into_iter().filter(|m| shown.contains(&m.id) && !have.contains(&m.id)));
                        v.has_more = page.has_more && v.threads.len() < MAX_VIEW_THREADS;
                        v.total_results = page.total_results;
                        v.list = list;
                        v.error = None;
                    }
                    Err(e) => v.error = Some(e),
                }
            }
            E::ChannelOpened(Ok(ch)) => {
                let id = self.adopt_channel(ch);
                self.select_channel(id);
                self.cap_threads();
            }
            E::ChannelOpened(Err(e)) => return Some(e),
            E::FriendResult { target: FriendTarget::Username(name), result } => {
                let result = result.map(drop);
                match self.social.add_friend.as_mut().filter(|v| v.username == name) {
                    Some(v) => v.result = Some(result),
                    None => return result.err(),
                }
            }
            E::FriendResult { target: FriendTarget::User(id), result } => match result {
                Ok(FriendReply::ConfirmStranger) => self.social.confirm_stranger = Some(id),
                Ok(FriendReply::Done) => {}
                Err(e) => return Some(e),
            },
            E::MentionsLoaded { filter, before, limit, result } => {
                let v = self.social.mentions.as_mut().filter(|v| v.filter == filter)?;
                v.loading = false;
                match result {
                    Ok(page) => {
                        if before.is_none() {
                            v.messages.clear();
                        }
                        let full = page.len() >= limit.max(1) as usize;
                        let room = MAX_MENTIONS.saturating_sub(v.messages.len());
                        v.messages.extend(page.into_iter().take(room));
                        v.has_more = full && v.messages.len() < MAX_MENTIONS;
                        v.error = None;
                    }
                    Err(e) => v.error = Some(e),
                }
            }
            E::ProfileLoaded { user_id, guild_id, result } => {
                let now = Instant::now();
                let p = &mut self.social.profiles;
                match p.iter_mut().find(|e| (e.user_id, e.guild_id) == (user_id, guild_id)) {
                    Some(e) => (e.loading, e.fetched, e.result) = (false, now, Some(result)),
                    // Evicted while loading: still worth keeping.
                    None => push_profile(p, ProfileEntry { user_id, guild_id, fetched: now, loading: false, result: Some(result) }),
                }
            }
            E::CollectiblesLoaded(result) => (self.social.collectibles, self.social.catalog_requested) = (Some(result), false),
            E::OwnedCollectiblesLoaded(result) => self.social.owned_collectibles = Some(result),
            E::ProductLoaded { sku_id, result } => {
                self.social.products.insert(sku_id, result.ok().map(Box::new));
            }
            E::GamesLoaded { ids, result } => {
                for g in result.unwrap_or_default() {
                    self.social.games.insert(g.id, Some(g));
                }
                for id in ids {
                    self.social.games.entry(id).or_insert(None);
                }
            }
            E::ConnectionsLoaded(result) => self.social.connections = Some(result),
            E::RequestPreviewsLoaded { channel_ids, result } => {
                let v = self.social.request_previews.as_mut()?;
                v.loading = false;
                match result {
                    Ok(page) => {
                        v.error = None;
                        v.messages.extend(page.into_iter().filter(|m| channel_ids.contains(&m.channel_id)).map(|m| (m.channel_id, m)));
                    }
                    Err(e) => {
                        // Retry asks again.
                        for id in &channel_ids {
                            v.asked.remove(id);
                        }
                        v.error = Some(e);
                    }
                }
            }
            E::MessageRequestsDone { channel_ids, accepted, result } => {
                for id in &channel_ids {
                    self.social.request_busy.remove(id);
                }
                match result {
                    Err(e) => {
                        let what = match (accepted, channel_ids.len() > 1) {
                            (true, _) => "accept the message request",
                            (false, false) => "ignore the message request",
                            (false, true) => "ignore the message requests",
                        };
                        return Some(format!("Could not {what}: {e}"));
                    }
                    Ok(()) if accepted => {
                        for id in &channel_ids {
                            if let Some(ch) = self.channels.get_mut(id) {
                                (ch.is_message_request, ch.is_spam) = (false, false);
                            }
                        }
                        self.dirty = true;
                    }
                    Ok(()) => {
                        // shortcut: a CHANNEL_UPDATE arriving after this local removal re-adds the channel until its CHANNEL_DELETE; add a rejected-id guard if that shows live.
                        for id in channel_ids {
                            self.apply_gateway(GatewayEvent::ChannelDelete { id, guild_id: None }, None);
                        }
                    }
                }
            }
        }
        None
    }

    /// A profile shown needs product `sku_id` (effect / frame): true = send `FetchProduct` now
    /// (first time this session). The caches are small; past `MAX_COSMETIC_CACHE` they restart.
    pub fn request_product(&mut self, sku_id: Id) -> bool {
        let p = &mut self.social.products;
        if p.contains_key(&sku_id) {
            return false;
        }
        if p.len() >= MAX_COSMETIC_CACHE {
            p.clear();
        }
        p.insert(sku_id, None);
        true
    }

    /// The games of `ids` not requested yet (≤ 25, for one `FetchGames`), marked requested.
    pub fn request_games(&mut self, ids: impl IntoIterator<Item = Id>) -> Vec<Id> {
        let g = &mut self.social.games;
        if g.len() >= MAX_COSMETIC_CACHE {
            g.clear();
        }
        let mut new: Vec<Id> = Vec::new();
        for id in ids {
            if new.len() < crate::api::cosmetics::MAX_GAME_IDS && !id.is_zero() && !g.contains_key(&id) && !new.contains(&id) {
                new.push(id);
            }
        }
        for &id in &new {
            g.insert(id, None);
        }
        new
    }

    /// Call when a profile popout opens. Returns whether to send `FetchProfile` now (not cached
    /// within `PROFILE_TTL`, not already loading); marks the entry most recently used.
    pub fn request_profile(&mut self, user_id: Id, guild_id: Option<Id>, now: Instant) -> bool {
        let p = &mut self.social.profiles;
        let mut e = match p.iter().position(|e| (e.user_id, e.guild_id) == (user_id, guild_id)) {
            Some(i) => p.remove(i).expect("index from position"),
            None => ProfileEntry { user_id, guild_id, fetched: now, loading: false, result: None },
        };
        let age = now.saturating_duration_since(e.fetched);
        // Every FetchProfile is answered (or the state is reset), so a loading entry never retries.
        let fetch = !e.loading && (e.result.is_none() || age >= PROFILE_TTL);
        if fetch {
            (e.loading, e.fetched) = (true, now);
        }
        push_profile(p, e);
        fetch
    }

    pub fn profile(&self, user_id: Id, guild_id: Option<Id>) -> Option<&ProfileEntry> {
        self.social.profiles.iter().find(|e| (e.user_id, e.guild_id) == (user_id, guild_id))
    }

    /// The cached 1:1 DM with a user ("Message" button: open it instead of `OpenDm`).
    pub fn dm_with(&self, user_id: Id) -> Option<Id> {
        let is_dm = |id: &Id| self.channels.get(id).is_some_and(|c| c.kind == channel_type::DM && c.recipient_ids.contains(&user_id));
        self.dm_order.iter().copied().find(is_dm)
    }

    /// User ids of a Friends tab, sorted by `friend_name` (case-insensitive).
    pub fn friends_tab(&self, tab: FriendsTab) -> Vec<Id> {
        use relationship_type::*;
        let mut v: Vec<(String, Id)> = (self.social.relationships.values())
            .filter(|r| match tab {
                FriendsTab::Online => r.kind == FRIEND && self.presence(r.id) != Presence::Offline,
                FriendsTab::All => r.kind == FRIEND,
                FriendsTab::Pending => matches!(r.kind, INCOMING | OUTGOING) && !r.is_spam_request,
                FriendsTab::Spam => r.kind == INCOMING && r.is_spam_request,
                FriendsTab::Blocked => r.kind == BLOCKED || r.user_ignored,
            })
            .map(|r| (self.friend_name(r.id).to_lowercase(), r.id))
            .collect();
        v.sort();
        v.into_iter().map(|(_, id)| id).collect()
    }

    /// Friend nickname > global name > username (> the id while the user is unknown).
    pub fn friend_name(&self, user_id: Id) -> String {
        let nick = self.social.relationships.get(&user_id).and_then(|r| r.nickname.as_deref()).filter(|n| !n.is_empty());
        match (nick, self.users.get(&user_id)) {
            (Some(n), _) => n.to_owned(),
            (None, Some(u)) => u.display_name().to_owned(),
            (None, None) => user_id.to_string(),
        }
    }

    /// Friends badge on Home: incoming requests, spam excluded.
    pub fn pending_count(&self) -> usize {
        self.social.relationships.values().filter(|r| r.kind == relationship_type::INCOMING && !r.is_spam_request).count()
    }

    /// Threads under each channel of a guild's sidebar: joined and not archived, plus the
    /// open thread (temporary row). Parent id -> thread ids, latest activity first.
    pub fn sidebar_threads(&self, guild_id: Id) -> HashMap<Id, Vec<Id>> {
        self.threads_by_parent(|c| c.guild_id == Some(guild_id) && (c.member.is_some() && !c.is_archived() || self.selected_channel == Some(c.id)))
            .into_iter()
            .collect()
    }

    /// Known active threads of one channel, joined or not (the thread browser's local "Active"
    /// list before / besides `ThreadList::Active`), latest activity first.
    pub fn active_threads(&self, parent_id: Id) -> Vec<Id> {
        self.threads_by_parent(|c| c.parent_id == Some(parent_id) && !c.is_archived()).remove(&parent_id).unwrap_or_default()
    }

    /// Open a thread from a list view (it may be unjoined or archived): adds it to `channels`
    /// and selects it.
    #[allow(dead_code, reason = "the UI opens listed threads in the panel instead (threads::open_in_panel)")]
    pub fn open_thread(&mut self, thread: Channel) {
        let id = self.adopt_channel(thread);
        self.select_channel(id);
        self.cap_threads();
    }

    /// Inbox "Unreads": readable unread channels (joined threads only; muted guilds only with
    /// mentions), newest activity first.
    pub fn unread_channels(&self) -> Vec<Id> {
        let mut v: Vec<&Channel> = (self.read_states.keys())
            .filter_map(|id| self.channels.get(id))
            .filter(|c| c.is_text_chat() && !inactive_thread(c) && !c.is_message_request && !c.is_spam)
            .filter(|c| self.is_unread(c.id) && self.can_view(c.id))
            .filter(|c| self.mention_count(c.id) > 0 || !c.guild_id.is_some_and(|g| self.is_guild_muted(g)))
            .collect();
        v.sort_by_key(|c| Reverse(c.last_message_id));
        v.into_iter().map(|c| c.id).collect()
    }

    /// Explicit "Mark all read" / "Mark as read": marks locally (resuming automatic acks) and
    /// returns the `(channel, message)` acks for `SocialCommand::MarkAllRead`. Send them only
    /// when `settings.send_read_acks` is on; the local mark happens either way.
    pub fn mark_all_read(&mut self, channel_ids: &[Id]) -> Vec<(Id, Id)> {
        if self.chat.ack_paused.is_some_and(|c| channel_ids.contains(&c)) {
            self.chat.ack_paused = None;
        }
        channel_ids.iter().filter_map(|&c| self.mark_read(c).map(|m| (c, m))).collect()
    }

    /// A DM under Message Requests or Spam (§2.4): no Home badge, no notification.
    pub fn is_message_request(&self, channel_id: Id) -> bool {
        self.channels.get(&channel_id).is_some_and(|c| c.is_message_request || c.is_spam)
    }

    /// DMs of the Message Requests entry (`spam` false) or the Spam entry, newest first. The DM
    /// list skips both (`is_message_request`).
    pub fn message_requests(&self, spam: bool) -> Vec<Id> {
        let listed = |c: &Channel| if spam { c.is_spam } else { c.is_message_request && !c.is_spam };
        self.dm_order.iter().copied().filter(|id| self.channels.get(id).is_some_and(listed)).collect()
    }

    /// `(requests, spam)` counts of the Message Requests entry.
    pub fn request_counts(&self) -> (usize, usize) {
        let spam = self.message_requests(true).len();
        (self.message_requests(false).len(), spam)
    }

    /// The next page of previews to ask for (`FetchRequestPreviews`): the first 25 listed
    /// requests not asked yet; marks them asked and the view loading. None when the page is
    /// closed, a page is on its way, or everything was asked.
    pub fn begin_request_previews(&mut self, spam: bool) -> Option<Vec<Id>> {
        let ids = self.message_requests(spam);
        let v = self.social.request_previews.as_mut().filter(|v| !v.loading)?;
        let page: Vec<Id> = ids.into_iter().filter(|id| !v.asked.contains(id)).take(PREVIEW_PAGE).collect();
        if page.is_empty() {
            return None;
        }
        v.asked.extend(page.iter().copied());
        (v.loading, v.error) = (true, None);
        Some(page)
    }

    /// Some listed request has no preview asked for yet ("Load more previews").
    pub fn more_request_previews(&self, spam: bool) -> bool {
        let Some(v) = self.social.request_previews.as_ref().filter(|v| !v.loading) else { return false };
        self.message_requests(spam).iter().any(|id| !v.asked.contains(id))
    }

    /// Claims `ids` for an accept / ignore: false when none or one is already on its way.
    pub fn begin_request_action(&mut self, ids: &[Id]) -> bool {
        let busy = &mut self.social.request_busy;
        if ids.is_empty() || ids.iter().any(|id| busy.contains(id)) {
            return false;
        }
        busy.extend(ids.iter().copied());
        true
    }

    /// A request's preview text (one line) and its ISO timestamp.
    pub fn request_preview(&self, channel_id: Id) -> Option<(String, &str)> {
        let m = self.social.request_previews.as_ref()?.messages.get(&channel_id)?;
        Some((super::notification_body(m).split_whitespace().collect::<Vec<_>>().join(" "), &m.timestamp))
    }

    // ----- hooks (called from `apply_gateway` & co.) ------------------------------------

    /// `notification_level` of a thread (§2.1): my thread member flags (8 nothing, 4 mentions,
    /// 2 all), else its parent's effective level (so the category's too). None otherwise.
    pub(super) fn thread_notification_level(&self, ch: &Channel) -> Option<u8> {
        if !is_thread(ch.kind) {
            return None;
        }
        let flags = ch.member.as_ref().map_or(0, |m| m.flags);
        let own = [(8, NOTIFY_NOTHING), (4, 1), (2, NOTIFY_ALL)].into_iter().find(|(bit, _)| flags & bit != 0);
        own.map(|(_, level)| level).or_else(|| self.thread_parent(ch).map(|p| self.notification_level(p)))
    }

    /// `is_channel_muted` of a thread: also muted with its parent channel (or that one's category).
    pub(super) fn thread_parent_muted(&self, ch: &Channel) -> bool {
        is_thread(ch.kind) && self.thread_parent(ch).is_some_and(|p| self.is_channel_muted(p.id))
    }


    /// READY relationships (empty from the disk cache).
    pub(super) fn set_relationships(&mut self, list: Vec<Relationship>) {
        self.social.relationships.clear();
        for r in list {
            self.on_relationship(r);
        }
    }

    /// RELATIONSHIP_ADD (full; every type change arrives as one) / RELATIONSHIP_UPDATE (partial:
    /// nickname, user_ignored, since).
    pub(super) fn on_relationship(&mut self, mut r: Relationship) {
        if let Some(u) = r.user.take() {
            self.users.insert(u.id, u);
        }
        let map = &mut self.social.relationships;
        if let Some(old) = map.get_mut(&r.id) {
            if r.kind == 0 || r.kind == old.kind {
                old.nickname = r.nickname;
                old.user_ignored = r.user_ignored;
                if r.since.is_some() {
                    old.since = r.since;
                }
            } else {
                *old = r;
            }
        } else if map.len() < MAX_RELATIONSHIPS {
            map.insert(r.id, r);
        }
    }

    /// The social `GatewayEvent`s (threads, recipients, mentions, notes).
    pub(super) fn on_social_gateway(&mut self, ev: GatewayEvent) {
        match ev {
            GatewayEvent::ThreadUpsert(t) => {
                self.upsert_thread(*t);
                self.cap_threads();
            }
            GatewayEvent::ThreadDelete { id } => {
                self.remove_channel_data(id);
                self.fix_selection();
            }
            GatewayEvent::ThreadListSync { guild_id, channel_ids, threads } => {
                let incoming: HashSet<Id> = threads.iter().map(|t| t.id).collect();
                let in_scope = |c: &Channel| channel_ids.as_ref().is_none_or(|ids| c.parent_id.is_some_and(|p| ids.contains(&p)));
                let gone: Vec<Id> = (self.channels.values())
                    .filter(|c| c.guild_id == Some(guild_id) && is_thread(c.kind) && in_scope(c))
                    .filter(|c| !incoming.contains(&c.id) && self.selected_channel != Some(c.id) && !kept_archived(c))
                    .map(|c| c.id)
                    .collect();
                for id in gone {
                    self.remove_channel_data(id);
                }
                for t in threads {
                    self.upsert_thread(Channel { guild_id: Some(guild_id), ..t });
                }
                self.cap_threads();
            }
            GatewayEvent::ThreadMemberUpdate { thread_id, member } => {
                if let Some(c) = self.channels.get_mut(&thread_id) {
                    c.member = Some(member);
                }
            }
            GatewayEvent::ThreadMembersUpdate { thread_id, member_count, joined, left } => {
                // A private thread is visible only to members and MANAGE_THREADS (§2.1); opening
                // it after leaving would only earn a 403. The open one stays (read-only).
                let private = self.channels.get(&thread_id).is_some_and(|c| c.kind == channel_type::PRIVATE_THREAD);
                if left && joined.is_none() && private && !self.can_manage_threads(thread_id) && self.selected_channel != Some(thread_id) {
                    self.remove_channel_data(thread_id);
                } else if let Some(c) = self.channels.get_mut(&thread_id) {
                    c.member_count = Some(member_count);
                    if joined.is_some() {
                        c.member = joined;
                    } else if left {
                        c.member = None;
                    }
                }
            }
            GatewayEvent::ChannelRecipientAdd { channel_id, user } => {
                if let Some(c) = self.channels.get_mut(&channel_id)
                    && !c.recipient_ids.contains(&user.id)
                {
                    c.recipient_ids.push(user.id);
                }
                self.users.insert(user.id, user);
            }
            GatewayEvent::ChannelRecipientRemove { channel_id, user_id } => {
                if self.is_me(user_id) {
                    self.remove_channel_data(channel_id);
                    self.fix_selection();
                } else if let Some(c) = self.channels.get_mut(&channel_id) {
                    c.recipient_ids.retain(|r| *r != user_id);
                }
            }
            GatewayEvent::RecentMentionDelete { message_id } => {
                if let Some(v) = self.social.mentions.as_mut() {
                    v.messages.retain(|m| m.id != message_id);
                }
            }
            GatewayEvent::UserNoteUpdate { user_id, note } => {
                for e in self.social.profiles.iter_mut().filter(|e| e.user_id == user_id) {
                    if let Some(Ok(p)) = &mut e.result {
                        p.note = Some(note.clone()).filter(|n| !n.is_empty());
                    }
                }
            }
            _ => {}
        }
    }

    // ----- private helpers ---------------------------------------------------------------

    /// A thread's parent channel (never another thread, so the lookups above cannot loop).
    fn thread_parent(&self, ch: &Channel) -> Option<&Channel> {
        ch.parent_id.and_then(|p| self.channels.get(&p)).filter(|p| !is_thread(p.kind))
    }

    /// THREAD_CREATE / THREAD_UPDATE / a synced thread. Archived ones leave the state unless open
    /// or joined (membership and read state must survive an unarchive: its THREAD_UPDATE lacks
    /// `member`, THREAD_MEMBER_UPDATE follows).
    fn upsert_thread(&mut self, t: Channel) {
        let joined = t.member.is_some() || self.channels.get(&t.id).is_some_and(|c| c.member.is_some());
        if t.is_archived() && !joined && self.selected_channel != Some(t.id) {
            self.remove_channel_data(t.id);
            return;
        }
        // A forum's `last_message_id` is its newest post (no CHANNEL_UPDATE for it).
        let parent = t.parent_id.and_then(|p| self.channels.get_mut(&p));
        if let Some(p) = parent.filter(|p| matches!(p.kind, channel_type::GUILD_FORUM | channel_type::GUILD_MEDIA)) {
            p.last_message_id = p.last_message_id.max(Some(t.id));
        }
        self.adopt_channel(t);
    }

    /// Inserts a channel from REST / thread events: keeps my thread membership when the
    /// payload lacks it, fills a thread's `guild_id` from its parent. Returns its id.
    fn adopt_channel(&mut self, mut ch: Channel) -> Id {
        if ch.member.is_none() {
            ch.member = self.channels.get(&ch.id).and_then(|old| old.member.clone());
        }
        if ch.guild_id.is_none() && !ch.is_dm_like() {
            ch.guild_id = ch.parent_id.and_then(|p| self.channels.get(&p)).and_then(|p| p.guild_id);
        }
        let (id, dm) = (ch.id, ch.is_dm_like());
        self.insert_channel(ch);
        if dm {
            self.sort_dm_order();
        }
        id
    }

    /// Drops the least recently active unjoined threads beyond `MAX_UNJOINED_THREADS` (never
    /// the open one).
    fn cap_threads(&mut self) {
        let mut unjoined: Vec<(Id, Id)> = (self.channels.values())
            .filter(|c| is_thread(c.kind) && c.member.is_none() && self.selected_channel != Some(c.id))
            .map(|c| (activity(c), c.id))
            .collect();
        if unjoined.len() <= MAX_UNJOINED_THREADS {
            return;
        }
        unjoined.sort_unstable();
        let excess = unjoined.len() - MAX_UNJOINED_THREADS;
        for (_, id) in unjoined.into_iter().take(excess) {
            self.remove_channel_data(id);
        }
    }

    /// Viewable threads matching `pred`, grouped by parent, latest activity first.
    fn threads_by_parent(&self, pred: impl Fn(&Channel) -> bool) -> HashMap<Id, Vec<Id>> {
        let mut out: HashMap<Id, Vec<&Channel>> = HashMap::new();
        for c in self.channels.values().filter(|c| is_thread(c.kind) && pred(c)) {
            if let Some(p) = c.parent_id.filter(|_| self.can_view(c.id)) {
                out.entry(p).or_default().push(c);
            }
        }
        out.into_iter()
            .map(|(p, mut v)| {
                v.sort_by_key(|c| Reverse(activity(c)));
                (p, v.into_iter().map(|c| c.id).collect())
            })
            .collect()
    }
}

/// A thread I have not joined, or an archived one: no badge, unread entry or disk cache (§2.1).
pub(super) fn inactive_thread(c: &Channel) -> bool {
    is_thread(c.kind) && (c.member.is_none() || c.is_archived())
}

/// `should_notify` hook: message requests and spam DMs never alert (§2.4); unjoined or archived
/// threads only for a mention (it adds me to the thread).
pub(super) fn silenced(c: &Channel, mentioned: bool) -> bool {
    c.is_message_request || c.is_spam || (inactive_thread(c) && !mentioned)
}

/// A joined archived thread: kept until READY (see `upsert_thread`).
fn kept_archived(c: &Channel) -> bool {
    c.is_archived() && c.member.is_some()
}

/// Newest message (or creation) of a thread.
fn activity(c: &Channel) -> Id {
    c.last_message_id.map_or(c.id, |m| m.max(c.id))
}

fn push_profile(p: &mut VecDeque<ProfileEntry>, e: ProfileEntry) {
    p.push_back(e);
    while p.len() > PROFILE_CACHE {
        p.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::ThreadSort;

    const ME: Id = Id(1);
    const OTHER: Id = Id(2);
    const THIRD: Id = Id(3);
    const G: Id = Id(10);
    const TEXT: Id = Id(100);
    const FORUM: Id = Id(101);
    const DM: Id = Id(200);

    fn user(id: Id, name: &str) -> User {
        User { id, username: name.into(), ..Default::default() }
    }

    fn state() -> AppState {
        let mut s = AppState { me: Some(user(ME, "me")), ..Default::default() };
        // Owner: every permission, so visibility is not what these tests are about.
        s.guilds.push(Guild { id: G, owner_id: ME, ..Default::default() });
        s.channels.insert(TEXT, Channel { id: TEXT, guild_id: Some(G), ..Default::default() });
        s.channels.insert(FORUM, Channel { id: FORUM, kind: channel_type::GUILD_FORUM, guild_id: Some(G), ..Default::default() });
        s.channels.insert(DM, Channel { id: DM, kind: channel_type::DM, recipient_ids: vec![OTHER], ..Default::default() });
        s.dm_order.push(DM);
        s.users.insert(OTHER, user(OTHER, "zed"));
        s.users.insert(THIRD, user(THIRD, "amy"));
        s
    }

    fn thread(id: u64, parent: Id, joined: bool) -> Channel {
        Channel {
            id: Id(id),
            kind: channel_type::PUBLIC_THREAD,
            guild_id: Some(G),
            parent_id: Some(parent),
            member: joined.then(ThreadMember::default),
            thread_metadata: Some(ThreadMetadata::default()),
            ..Default::default()
        }
    }

    fn archived(mut t: Channel, at: &str) -> Channel {
        t.thread_metadata = Some(ThreadMetadata { archived: true, archive_timestamp: Some(at.into()), ..Default::default() });
        t
    }

    fn rel(id: Id, kind: u8) -> Relationship {
        Relationship { id, kind, ..Default::default() }
    }

    fn msg(id: u64) -> Message {
        Message { id: Id(id), channel_id: TEXT, author: user(OTHER, "zed"), ..Default::default() }
    }

    fn gw(s: &mut AppState, ev: GatewayEvent) {
        s.apply_gateway(ev, None);
    }

    #[test]
    fn relationships_merge_tabs_and_badge() {
        let mut s = state();
        let incoming = Relationship { user: Some(user(Id(4), "Bob")), ..rel(Id(4), 3) };
        s.set_relationships(vec![rel(OTHER, 1), rel(THIRD, 1), incoming, Relationship { is_spam_request: true, ..rel(Id(5), 3) }]);
        assert_eq!(s.users[&Id(4)].username, "Bob", "user objects move into users");
        assert!(s.social.relationships[&Id(4)].user.is_none());
        assert_eq!(s.friends_tab(FriendsTab::All), [THIRD, OTHER], "amy before zed");
        assert_eq!(s.friends_tab(FriendsTab::Pending), [Id(4)]);
        assert_eq!(s.friends_tab(FriendsTab::Spam), [Id(5)]);
        assert_eq!(s.pending_count(), 1);
        // Online: presence decides.
        s.on_presences(vec![(OTHER, UserPresence { status: Presence::Idle, ..Default::default() })]);
        assert_eq!(s.friends_tab(FriendsTab::Online), [OTHER]);
        // UPDATE (partial): nickname + ignore, `since` kept when absent; the nickname sorts.
        s.on_relationship(Relationship { since: Some("2020".into()), ..rel(OTHER, 1) });
        gw(&mut s, GatewayEvent::RelationshipUpsert(Relationship { nickname: Some("Al".into()), user_ignored: true, ..rel(OTHER, 1) }));
        let r = &s.social.relationships[&OTHER];
        assert_eq!((r.nickname.as_deref(), r.since.as_deref(), r.user_ignored), (Some("Al"), Some("2020"), true));
        assert_eq!(s.friends_tab(FriendsTab::All), [OTHER, THIRD]);
        assert_eq!(s.friends_tab(FriendsTab::Blocked), [OTHER], "ignored friends are listed as blocked too");
        // A type change (ADD) replaces the entry: the spam request becomes a friend.
        gw(&mut s, GatewayEvent::RelationshipUpsert(rel(Id(5), 1)));
        assert!(!s.social.relationships[&Id(5)].is_spam_request);
        gw(&mut s, GatewayEvent::RelationshipRemove { id: Id(4) });
        assert_eq!(s.pending_count(), 0);
        assert_eq!(s.friend_name(Id(99)), "99");
    }

    #[test]
    fn thread_events_keep_membership_and_drop_archived() {
        let mut s = state();
        gw(&mut s, GatewayEvent::ThreadUpsert(Box::new(thread(300, TEXT, true))));
        // THREAD_UPDATE has no `member`: still joined.
        gw(&mut s, GatewayEvent::ThreadUpsert(Box::new(Channel { name: Some("renamed".into()), ..thread(300, TEXT, false) })));
        assert!(s.channels[&Id(300)].member.is_some());
        assert_eq!(s.channels[&Id(300)].name.as_deref(), Some("renamed"));
        gw(&mut s, GatewayEvent::ThreadMembersUpdate { thread_id: Id(300), member_count: 4, joined: None, left: true });
        assert!(s.channels[&Id(300)].member.is_none());
        assert_eq!(s.channels[&Id(300)].member_count, Some(4));
        gw(&mut s, GatewayEvent::ThreadMemberUpdate { thread_id: Id(300), member: ThreadMember { flags: 2, ..Default::default() } });
        assert_eq!(s.channels[&Id(300)].member.as_ref().map(|m| m.flags), Some(2));
        // Archived: gone unless joined or open.
        gw(&mut s, GatewayEvent::ThreadUpsert(Box::new(archived(thread(300, TEXT, false), "t"))));
        assert!(s.channels[&Id(300)].is_archived() && s.channels[&Id(300)].member.is_some(), "joined: kept");
        gw(&mut s, GatewayEvent::ThreadUpsert(Box::new(thread(302, TEXT, false))));
        gw(&mut s, GatewayEvent::ThreadUpsert(Box::new(archived(thread(302, TEXT, false), "t"))));
        assert!(!s.channels.contains_key(&Id(302)));
        gw(&mut s, GatewayEvent::ThreadUpsert(Box::new(thread(301, TEXT, true))));
        s.select_channel(Id(301));
        gw(&mut s, GatewayEvent::ThreadUpsert(Box::new(archived(thread(301, TEXT, true), "t"))));
        assert!(s.channels[&Id(301)].is_archived(), "the open thread stays, read-only");
        gw(&mut s, GatewayEvent::ThreadDelete { id: Id(301) });
        assert!(!s.channels.contains_key(&Id(301)));
        assert_ne!(s.selected_channel, Some(Id(301)));
        // A new forum post is the forum's newest "message".
        gw(&mut s, GatewayEvent::ThreadUpsert(Box::new(thread(400, FORUM, false))));
        assert_eq!(s.channels[&FORUM].last_message_id, Some(Id(400)));
    }

    #[test]
    fn unarchived_joined_thread_survives_the_cap() {
        let mut s = state();
        let t = |archive: bool| {
            let t = Channel { last_message_id: Some(Id(299)), ..thread(300, TEXT, false) };
            if archive { archived(t, "t") } else { t }
        };
        let sync = |extra: Vec<Channel>| {
            let mut threads: Vec<Channel> = (0..MAX_UNJOINED_THREADS as u64).map(|i| thread(1000 + i, TEXT, false)).collect();
            threads.extend(extra);
            GatewayEvent::ThreadListSync { guild_id: G, channel_ids: None, threads }
        };
        gw(&mut s, sync(vec![thread(300, TEXT, true)]));
        s.read_states.insert(Id(300), ReadState { id: Id(300), last_message_id: Some(Id(1)), ..Default::default() });
        gw(&mut s, GatewayEvent::ThreadUpsert(Box::new(t(true))));
        // Archived: kept (also through a sync, which lists active threads only), but quiet.
        gw(&mut s, sync(vec![]));
        assert!(s.channels[&Id(300)].member.is_some());
        assert!(!s.sidebar_threads(G).get(&TEXT).is_some_and(|v| v.contains(&Id(300))));
        assert_eq!((s.guild_unread(G), s.unread_channels()), ((false, 0), vec![]), "no badge while archived");
        // Unarchive: THREAD_UPDATE without `member` (the oldest activity of 1,001 threads), then
        // THREAD_MEMBER_UPDATE.
        gw(&mut s, GatewayEvent::ThreadUpsert(Box::new(t(false))));
        gw(&mut s, GatewayEvent::ThreadMemberUpdate { thread_id: Id(300), member: ThreadMember { flags: 2, ..Default::default() } });
        assert_eq!(s.channels[&Id(300)].member.as_ref().map(|m| m.flags), Some(2), "still joined");
        assert!(s.sidebar_threads(G)[&TEXT].contains(&Id(300)));
        assert_eq!(s.guild_unread(G), (true, 0));
        assert_eq!(s.channels.values().filter(|c| is_thread(c.kind) && c.member.is_none()).count(), MAX_UNJOINED_THREADS);
    }

    #[test]
    fn thread_notifications_follow_membership_flags_and_parent() {
        const CAT: Id = Id(90);
        let mut s = state();
        s.channels.insert(CAT, Channel { id: CAT, kind: channel_type::GUILD_CATEGORY, guild_id: Some(G), ..Default::default() });
        s.channels.get_mut(&TEXT).unwrap().parent_id = Some(CAT);
        for t in [thread(300, TEXT, true), thread(301, TEXT, false)] {
            gw(&mut s, GatewayEvent::ThreadUpsert(Box::new(t)));
        }
        let mut next = 1000;
        let mut notify = |s: &mut AppState, thread: u64, mention: bool| {
            next += 1;
            let mentions = if mention { vec![user(ME, "me")] } else { vec![] };
            let m = Message { channel_id: Id(thread), mentions, ..msg(next) };
            s.apply_gateway(GatewayEvent::MessageCreate(Box::new(m)), None).is_some()
        };
        // Guild default ALL: joined threads alert, unjoined ones only for a mention.
        assert!(notify(&mut s, 300, false));
        assert!(!notify(&mut s, 301, false));
        assert!(notify(&mut s, 301, true), "a mention adds me to the thread");
        // Thread member flags: 8 nothing (not even mentions), 4 only mentions.
        let flags = |s: &mut AppState, flags| {
            gw(s, GatewayEvent::ThreadMemberUpdate { thread_id: Id(300), member: ThreadMember { flags, ..Default::default() } });
        };
        flags(&mut s, 8 | 1);
        assert!(!notify(&mut s, 300, false) && !notify(&mut s, 300, true));
        flags(&mut s, 4);
        assert!(!notify(&mut s, 300, false) && notify(&mut s, 300, true));
        // No flags: the parent's effective level, i.e. a "Nothing" or muted category too.
        flags(&mut s, 0);
        let category = |level, muted| GuildSettings {
            guild_id: Some(G),
            message_notifications: 3,
            channel_overrides: vec![ChannelOverride { channel_id: CAT, muted, mute_config: None, message_notifications: level }],
            ..Default::default()
        };
        s.guild_settings.insert(Some(G), category(2, false));
        assert!(!notify(&mut s, 300, false));
        s.guild_settings.insert(Some(G), category(3, true));
        assert!(s.is_channel_muted(Id(300)));
        assert!(!notify(&mut s, 300, false));
        s.read_states.insert(Id(300), ReadState { id: Id(300), last_message_id: Some(Id(1)), ..Default::default() });
        assert!(!s.is_unread(Id(300)) && s.guild_unread(G) == (false, 0));
        s.guild_settings.remove(&Some(G));
        assert!(s.is_unread(Id(300)) && notify(&mut s, 300, false));
    }

    #[test]
    fn left_private_thread_is_dropped_without_manage_threads() {
        let private = |id| Channel { kind: channel_type::PRIVATE_THREAD, ..thread(id, TEXT, true) };
        let left = |id| GatewayEvent::ThreadMembersUpdate { thread_id: Id(id), member_count: 1, joined: None, left: true };
        let mut s = state();
        s.guilds[0].owner_id = Id(99);
        s.guilds[0].roles = vec![Role { id: G, permissions: Perms(perm::VIEW_CHANNEL), ..Default::default() }];
        for id in [310, 311, 312] {
            gw(&mut s, GatewayEvent::ThreadUpsert(Box::new(private(id))));
        }
        gw(&mut s, GatewayEvent::ThreadUpsert(Box::new(thread(313, TEXT, true))));
        s.select_channel(Id(311));
        for id in [310, 311, 313] {
            gw(&mut s, left(id));
        }
        assert!(!s.channels.contains_key(&Id(310)), "a 403 if opened");
        assert!(s.channels[&Id(311)].member.is_none(), "the open one stays");
        assert!(s.channels[&Id(313)].member.is_none(), "public: still viewable");
        assert!(s.active_threads(TEXT).contains(&Id(313)) && !s.active_threads(TEXT).contains(&Id(310)));
        // MANAGE_THREADS still sees it.
        s.guilds[0].owner_id = ME;
        gw(&mut s, left(312));
        assert!(s.channels[&Id(312)].member.is_none());
    }

    #[test]
    fn message_requests_are_listed_apart() {
        let mut s = state();
        let ch = |json: &str| serde_json::from_str::<Channel>(json).unwrap();
        let request = ch(r#"{"id":"201","type":1,"recipient_ids":["3"],"last_message_id":"70","is_message_request":true,"is_message_request_timestamp":"2026-11-01T00:00:00+00:00","is_spam":false}"#);
        let spam = ch(r#"{"id":"202","type":1,"recipient_ids":["4"],"last_message_id":"80","is_message_request":true,"is_spam":true}"#);
        let friend = Channel { last_message_id: Some(Id(60)), ..s.channels[&DM].clone() };
        assert_eq!(serde_json::to_value(&request).unwrap()["is_message_request"], true, "cached");
        assert!(serde_json::to_value(&friend).unwrap().get("is_spam").is_none());
        let read_states = [DM, Id(201), Id(202)].map(|id| ReadState { id, last_message_id: Some(Id(1)), ..Default::default() }).into();
        s.apply_ready(ReadyData { user: user(ME, "me"), private_channels: vec![friend, request, spam], read_states, ..Default::default() });
        assert_eq!((s.message_requests(false), s.message_requests(true)), (vec![Id(201)], vec![Id(202)]));
        assert!(s.is_message_request(Id(201)) && s.is_message_request(Id(202)) && !s.is_message_request(DM));
        assert_eq!(s.dm_unread_count(), 1, "requests are not on the Home badge");
        assert_eq!(s.request_counts(), (1, 1));
        assert!(!s.unread_channels().contains(&Id(201)));
        let from = |id, channel, author| Message { id: Id(id), channel_id: channel, author: user(author, "x"), ..Default::default() };
        assert!(s.apply_gateway(GatewayEvent::MessageCreate(Box::new(from(90, Id(201), THIRD))), None).is_none());
        assert!(s.apply_gateway(GatewayEvent::MessageCreate(Box::new(from(91, DM, OTHER))), None).is_some());
    }

    fn requests_state() -> AppState {
        let mut s = state();
        for (id, spam) in [(201, false), (202, true)] {
            let ch = Channel { id: Id(id), kind: channel_type::DM, recipient_ids: vec![THIRD], is_message_request: true, is_spam: spam, ..Default::default() };
            gw(&mut s, GatewayEvent::ChannelCreate(Box::new(ch)));
        }
        s
    }

    fn done(ids: &[u64], accepted: bool, result: Result<(), String>) -> SocialEvent {
        SocialEvent::MessageRequestsDone { channel_ids: ids.iter().map(|&i| Id(i)).collect(), accepted, result }
    }

    #[test]
    fn message_request_answers() {
        let mut s = requests_state();
        assert!(s.begin_request_action(&[Id(201)]) && !s.begin_request_action(&[Id(201)]) && !s.begin_request_action(&[]));
        assert!(s.apply_social(done(&[201], true, Ok(()))).is_none());
        let c = &s.channels[&Id(201)];
        assert!(!c.is_message_request && !c.is_spam && s.social.request_busy.is_empty());
        assert!(!s.is_message_request(Id(201)));
        s.select_channel(Id(202));
        assert!(s.begin_request_action(&[Id(202)]));
        assert!(s.apply_social(done(&[202], false, Ok(()))).is_none());
        assert!(!s.channels.contains_key(&Id(202)) && !s.dm_order.contains(&Id(202)) && s.selected_channel.is_none());
        assert!(s.social.request_busy.is_empty());
        let mut s = requests_state();
        assert!(s.begin_request_action(&[Id(201)]));
        assert_eq!(s.apply_social(done(&[201], true, Err("x".into()))).as_deref(), Some("Could not accept the message request: x"));
        assert!(s.channels[&Id(201)].is_message_request && s.social.request_busy.is_empty());
        assert_eq!(s.apply_social(done(&[201], false, Err("x".into()))).as_deref(), Some("Could not ignore the message request: x"));
        assert_eq!(s.apply_social(done(&[201, 202], false, Err("x".into()))).as_deref(), Some("Could not ignore the message requests: x"));
    }

    #[test]
    fn message_request_previews() {
        let mut s = state();
        assert!(s.begin_request_previews(false).is_none(), "no view");
        for i in 0..30 {
            let ch = Channel { id: Id(500 + i), kind: channel_type::DM, recipient_ids: vec![THIRD], is_message_request: true, last_message_id: Some(Id(1000 + i)), ..Default::default() };
            gw(&mut s, GatewayEvent::ChannelCreate(Box::new(ch)));
        }
        s.social.request_previews = Some(Default::default());
        let first = s.begin_request_previews(false).unwrap();
        assert_eq!(first.len(), 25);
        assert_eq!(first[0], Id(529), "newest first");
        assert!(s.begin_request_previews(false).is_none(), "loading");
        let msg = |c: u64, content: &str| Message { id: Id(1), channel_id: Id(c), content: content.into(), timestamp: "2026-11-01T00:00:00+00:00".into(), ..Default::default() };
        let loaded = |ids: Vec<Id>, result| SocialEvent::RequestPreviewsLoaded { channel_ids: ids, result };
        // 999 was not asked about.
        s.apply_social(loaded(first.clone(), Ok(vec![msg(529, "a\n\nb"), msg(999, "x")])));
        assert_eq!(s.request_preview(Id(529)), Some(("a b".to_owned(), "2026-11-01T00:00:00+00:00")));
        assert!(!s.social.request_previews.as_ref().unwrap().messages.contains_key(&Id(999)));
        assert!(s.more_request_previews(false));
        assert_eq!(s.begin_request_previews(false).unwrap().len(), 5);
        assert!(!s.more_request_previews(false), "loading");
        s.apply_social(loaded(vec![Id(500)], Err("boom".into())));
        let v = s.social.request_previews.as_ref().unwrap();
        assert_eq!(v.error.as_deref(), Some("boom"));
        assert!(!v.asked.contains(&Id(500)) && !v.loading);
        assert_eq!(s.begin_request_previews(false), Some(vec![Id(500)]), "retry asks again");
        assert!(s.begin_request_previews(false).is_none());
        s.social.request_previews = None;
        assert!(s.apply_social(loaded(vec![Id(500)], Err("late".into()))).is_none());
        assert!(s.social.request_previews.is_none());
    }

    #[test]
    fn thread_list_sync_replaces_scope_and_caps() {
        let mut s = state();
        s.channels.insert(Id(102), Channel { id: Id(102), guild_id: Some(Id(11)), ..Default::default() });
        let other_guild = Channel { guild_id: Some(Id(11)), ..thread(350, Id(102), false) };
        for t in [thread(300, TEXT, true), thread(301, TEXT, false), thread(302, FORUM, false), other_guild] {
            gw(&mut s, GatewayEvent::ThreadUpsert(Box::new(t)));
        }
        // Scoped to FORUM: 302 is gone, TEXT's threads untouched.
        gw(&mut s, GatewayEvent::ThreadListSync { guild_id: G, channel_ids: Some(vec![FORUM]), threads: vec![] });
        assert!(!s.channels.contains_key(&Id(302)) && s.channels.contains_key(&Id(301)));
        // Whole guild: the set is replaced (members from the sync), the other guild untouched.
        let synced = Channel { guild_id: None, ..thread(303, TEXT, true) };
        gw(&mut s, GatewayEvent::ThreadListSync { guild_id: G, channel_ids: None, threads: vec![thread(300, TEXT, false), synced] });
        assert!(!s.channels.contains_key(&Id(301)));
        assert!(s.channels[&Id(300)].member.is_some(), "joined state kept");
        assert_eq!(s.channels[&Id(303)].guild_id, Some(G));
        assert!(s.channels.contains_key(&Id(350)));
        // Unjoined threads are capped, oldest activity first; joined ones never count.
        let many = (0..MAX_UNJOINED_THREADS as u64 + 5).map(|i| thread(1000 + i, TEXT, false)).collect();
        gw(&mut s, GatewayEvent::ThreadListSync { guild_id: G, channel_ids: None, threads: many });
        let unjoined = s.channels.values().filter(|c| is_thread(c.kind) && c.member.is_none()).count();
        assert_eq!(unjoined, MAX_UNJOINED_THREADS);
        assert!(!s.channels.contains_key(&Id(1000)) && s.channels.contains_key(&Id(1004 + MAX_UNJOINED_THREADS as u64)));
        assert!(!s.channels.contains_key(&Id(300)), "not in the last sync");
    }

    #[test]
    fn sidebar_and_active_threads() {
        let mut s = state();
        let busy = Channel { last_message_id: Some(Id(900)), ..thread(301, TEXT, true) };
        for t in [thread(300, TEXT, true), busy, thread(302, TEXT, false), thread(303, FORUM, true)] {
            gw(&mut s, GatewayEvent::ThreadUpsert(Box::new(t)));
        }
        let side = s.sidebar_threads(G);
        assert_eq!(side[&TEXT], [Id(301), Id(300)], "joined only, latest activity first");
        assert_eq!(side[&FORUM], [Id(303)]);
        assert_eq!(s.active_threads(TEXT), [Id(301), Id(302), Id(300)]);
        // Opening an unjoined thread gives it a temporary row.
        s.open_thread(thread(304, TEXT, false));
        assert_eq!(s.selected_channel, Some(Id(304)));
        assert!(s.sidebar_threads(G)[&TEXT].contains(&Id(304)));
        // Unjoined threads are not cached.
        let snap = s.to_snapshot();
        let cached: Vec<Id> = snap.guilds.iter().flat_map(|g| &g.channels).map(|c| c.id).collect();
        assert!(cached.contains(&Id(300)) && !cached.contains(&Id(302)) && !cached.contains(&Id(304)));
    }

    #[test]
    fn thread_views_page_and_ignore_other_lists() {
        let mut s = state();
        let q = ThreadQuery { tags: vec![Id(7)], sort: ThreadSort::Created, ..Default::default() };
        s.social.threads = Some(ThreadsView::new(FORUM, ThreadList::Forum(q.clone())));
        let page = |ids: &[u64], more: bool| ThreadPage {
            threads: ids.iter().map(|&i| thread(i, FORUM, false)).collect(),
            has_more: more,
            total_results: Some(30),
            first_messages: ids.iter().map(|&i| msg(i)).collect(),
        };
        let other = ThreadList::Forum(ThreadQuery { sort: ThreadSort::LastActivity, ..q.clone() });
        s.apply_social(SocialEvent::ThreadsLoaded { channel_id: FORUM, list: other, result: Ok(page(&[1], false)) });
        assert!(s.social.threads.as_ref().unwrap().threads.is_empty(), "another sort is another list");
        s.apply_social(SocialEvent::ThreadsLoaded { channel_id: FORUM, list: ThreadList::Forum(q.clone()), result: Ok(page(&[1, 2], true)) });
        let v = s.social.threads.as_ref().unwrap();
        let next = v.next_page().unwrap();
        assert_eq!(next, ThreadList::Forum(ThreadQuery { offset: 25, ..q.clone() }));
        assert_eq!(v.first_message(Id(2)).map(|m| m.id), Some(Id(2)));
        // Page 2 overlaps (posts moved): duplicates dropped.
        s.apply_social(SocialEvent::ThreadsLoaded { channel_id: FORUM, list: next, result: Ok(page(&[2, 3], false)) });
        let v = s.social.threads.as_ref().unwrap();
        assert_eq!(v.threads.iter().map(|t| t.id.0).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!((v.first_messages.len(), v.has_more, v.next_page()), (3, false, None));

        // Archived lists page by archive time, or by id for joined private threads.
        let list = ThreadList::Archived { kind: ArchivedThreads::Public, before: None };
        s.social.threads = Some(ThreadsView::new(TEXT, list.clone()));
        let t = archived(thread(9, TEXT, false), "2026-01-01T00:00:00+00:00");
        let res = Ok(ThreadPage { threads: vec![t], has_more: true, ..Default::default() });
        s.apply_social(SocialEvent::ThreadsLoaded { channel_id: TEXT, list, result: res });
        let before = Some("2026-01-01T00:00:00+00:00".to_owned());
        assert_eq!(s.social.threads.as_ref().unwrap().next_page(), Some(ThreadList::Archived { kind: ArchivedThreads::Public, before }));
        let v = ThreadsView { list: ThreadList::Archived { kind: ArchivedThreads::Joined, before: None }, ..ThreadsView::new(TEXT, ThreadList::Active { offset: 0 }) };
        let v = ThreadsView { threads: vec![thread(9, TEXT, true)], has_more: true, loading: false, ..v };
        assert_eq!(v.next_page(), Some(ThreadList::Archived { kind: ArchivedThreads::Joined, before: Some("9".into()) }));
        let end = ThreadsView { list: ThreadList::Active { offset: MAX_SEARCH_OFFSET }, has_more: true, loading: false, ..ThreadsView::new(TEXT, ThreadList::Active { offset: 0 }) };
        assert_eq!(end.next_page(), None, "the search cannot page further");
        // Errors are kept on the view.
        s.apply_social(SocialEvent::ThreadsLoaded { channel_id: TEXT, list: ThreadList::Archived { kind: ArchivedThreads::Public, before: Some("x".into()) }, result: Err("nope".into()) });
        assert_eq!(s.social.threads.as_ref().unwrap().error.as_deref(), Some("nope"));
    }

    #[test]
    fn opened_channels_are_added_and_selected() {
        let mut s = state();
        let group = Channel { id: Id(201), kind: channel_type::GROUP_DM, recipients: vec![user(THIRD, "amy")], ..Default::default() };
        assert_eq!(s.apply_social(SocialEvent::ChannelOpened(Ok(group))), None);
        assert_eq!((s.selected_channel, s.selected_guild), (Some(Id(201)), None));
        assert_eq!(s.dm_order[0], Id(201));
        assert_eq!(s.channels[&Id(201)].recipient_ids, [THIRD]);
        // A thread from REST without guild_id takes its parent's.
        s.apply_social(SocialEvent::ChannelOpened(Ok(Channel { guild_id: None, ..thread(300, TEXT, true) })));
        assert_eq!((s.selected_channel, s.selected_guild), (Some(Id(300)), Some(G)));
        assert_eq!(s.apply_social(SocialEvent::ChannelOpened(Err("Could not open: x".into()))).as_deref(), Some("Could not open: x"));
        assert_eq!(s.dm_with(OTHER), Some(DM));
        assert_eq!(s.dm_with(THIRD), None, "a group is not the DM");
        // Recipients come and go; my own removal drops the group.
        gw(&mut s, GatewayEvent::ChannelRecipientAdd { channel_id: Id(201), user: user(Id(6), "new") });
        assert_eq!(s.channels[&Id(201)].recipient_ids, [THIRD, Id(6)]);
        assert!(s.users.contains_key(&Id(6)));
        gw(&mut s, GatewayEvent::ChannelRecipientRemove { channel_id: Id(201), user_id: THIRD });
        assert_eq!(s.channels[&Id(201)].recipient_ids, [Id(6)]);
        gw(&mut s, GatewayEvent::ChannelRecipientRemove { channel_id: Id(201), user_id: ME });
        assert!(!s.channels.contains_key(&Id(201)) && !s.dm_order.contains(&Id(201)));
    }

    #[test]
    fn friend_results() {
        let mut s = state();
        s.social.add_friend = Some(AddFriendView { username: "zed".into(), result: None });
        let sent = |name: &str, result| SocialEvent::FriendResult { target: FriendTarget::Username(name.into()), result };
        assert_eq!(s.apply_social(sent("zed", Err("No user with that name.".into()))), None, "shown in the tab");
        assert_eq!(s.social.add_friend.as_ref().unwrap().result, Some(Err("No user with that name.".into())));
        assert_eq!(s.apply_social(sent("old", Err("x".into()))).as_deref(), Some("x"), "tab moved on: toast");
        let accept = |result| SocialEvent::FriendResult { target: FriendTarget::User(OTHER), result };
        assert_eq!(s.apply_social(accept(Ok(FriendReply::ConfirmStranger))), None);
        assert_eq!(s.social.confirm_stranger, Some(OTHER));
        assert_eq!(s.apply_social(accept(Err("captcha".into()))).as_deref(), Some("captcha"));
    }

    #[test]
    fn mentions_view_pages_and_deletes() {
        let mut s = state();
        let filter = MentionFilter { guild_id: Some(G), ..Default::default() };
        s.social.mentions = Some(MentionsView::new(filter));
        let loaded = |filter, before, ids: &[u64]| SocialEvent::MentionsLoaded { filter, before, limit: 2, result: Ok(ids.iter().map(|&i| msg(i)).collect()) };
        s.apply_social(loaded(MentionFilter::default(), None, &[9, 8]));
        assert!(s.social.mentions.as_ref().unwrap().loading, "other filter ignored");
        s.apply_social(loaded(filter, None, &[9, 8]));
        assert_eq!(s.social.mentions.as_ref().unwrap().next_before(), Some(Id(8)));
        s.apply_social(loaded(filter, Some(Id(8)), &[7]));
        let v = s.social.mentions.as_ref().unwrap();
        assert_eq!((v.messages.len(), v.has_more, v.next_before()), (3, false, None));
        gw(&mut s, GatewayEvent::RecentMentionDelete { message_id: Id(8) });
        assert_eq!(s.social.mentions.as_ref().unwrap().messages.iter().map(|m| m.id.0).collect::<Vec<_>>(), [9, 7]);
    }

    #[test]
    fn profile_cache_ttl_inflight_and_lru() {
        let mut s = state();
        let t0 = Instant::now();
        assert!(s.request_profile(OTHER, Some(G), t0));
        assert!(!s.request_profile(OTHER, Some(G), t0), "already loading");
        assert!(s.request_profile(OTHER, None, t0), "per guild");
        let profile = UserProfile { user: user(OTHER, "zed"), ..Default::default() };
        s.apply_social(SocialEvent::ProfileLoaded { user_id: OTHER, guild_id: Some(G), result: Ok(Box::new(profile)) });
        let e = s.profile(OTHER, Some(G)).unwrap();
        assert!(!e.loading && matches!(e.result, Some(Ok(_))));
        let later = Instant::now();
        assert!(!s.request_profile(OTHER, Some(G), later + Duration::from_secs(10)), "cached");
        assert!(s.request_profile(OTHER, Some(G), later + PROFILE_TTL), "stale");
        assert!(matches!(s.profile(OTHER, Some(G)).unwrap().result, Some(Ok(_))), "old answer shown while refetching");
        assert!(!s.request_profile(OTHER, None, t0 + PROFILE_TTL * 2), "a running request is never sent twice");
        gw(&mut s, GatewayEvent::UserNoteUpdate { user_id: OTHER, note: "likes cats".into() });
        let note = |s: &AppState| s.profile(OTHER, Some(G)).and_then(|e| e.result.as_ref()?.as_ref().ok()?.note.clone());
        assert_eq!(note(&s).as_deref(), Some("likes cats"));
        gw(&mut s, GatewayEvent::UserNoteUpdate { user_id: OTHER, note: String::new() });
        assert_eq!(note(&s), None);
        for i in 0..PROFILE_CACHE as u64 {
            s.request_profile(Id(1000 + i), None, t0);
        }
        assert_eq!(s.social.profiles.len(), PROFILE_CACHE);
        assert!(s.profile(OTHER, Some(G)).is_none(), "least recently requested evicted");
    }

    #[test]
    fn unreads_and_mark_all_read() {
        let mut s = state();
        let rs = |id, last| ReadState { id, last_message_id: Some(Id(last)), ..Default::default() };
        s.channels.get_mut(&TEXT).unwrap().last_message_id = Some(Id(50));
        s.channels.get_mut(&DM).unwrap().last_message_id = Some(Id(60));
        let joined = Channel { last_message_id: Some(Id(70)), ..thread(300, TEXT, true) };
        let unjoined = Channel { last_message_id: Some(Id(80)), ..thread(301, TEXT, false) };
        for t in [joined, unjoined] {
            gw(&mut s, GatewayEvent::ThreadUpsert(Box::new(t)));
        }
        for (id, last) in [(TEXT, 40), (DM, 10), (Id(300), 1), (Id(301), 1)] {
            s.read_states.insert(id, rs(id, last));
        }
        assert_eq!(s.unread_channels(), [Id(300), DM, TEXT], "newest first, unjoined thread skipped");
        s.chat.ack_paused = Some(TEXT);
        let acks = s.mark_all_read(&[TEXT, DM]);
        assert_eq!(acks, [(TEXT, Id(50)), (DM, Id(60))]);
        assert_eq!(s.chat.ack_paused, None);
        assert_eq!(s.unread_channels(), [Id(300)]);
        assert!(s.mark_all_read(&[TEXT]).is_empty(), "already read");
    }
}
