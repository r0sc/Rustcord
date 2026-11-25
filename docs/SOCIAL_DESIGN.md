# Social design (threads, forums, friends, DMs, inbox, profiles)

Protocol facts: `docs/FEATURES_NOTES.md` Part 2 and §0 (authoritative). Same pattern as
`docs/CHAT_ESSENTIALS_DESIGN.md`: one nested command/event pair, one state submodule. This
page fixes the split between the engineers. Backend status: built, unit-tested (not against the
live API); the UI comes later, see the hook-up guide at the end.

## Data flow
```
UI ──Command::Social(SocialCommand)──► backend::run ──► handle_social (backend.rs, done)
     REST: rest(session, …) ──► ApiClient methods in api/social/people.rs (friends, DMs, inbox,
           profiles) and api/threads.rs (threads, forums)        ──► SocialEvent / ActionFailed
           CreateForumPost  ──► api::threads::create_forum_post (upload pipeline steps 1-2)
UI ──Command::AckThread──► ApiClient::ack(.., thread: true) (read-state flags 3, silent)
gateway dispatches ──► normalize.rs ──► GatewayEvent::{ThreadUpsert, ThreadDelete, ThreadListSync,
     ThreadMemberUpdate, ThreadMembersUpdate, ChannelRecipientAdd, ChannelRecipientRemove,
     RecentMentionDelete, UserNoteUpdate, RelationshipUpsert, RelationshipRemove}
     + READY/GUILD_CREATE threads[] in Guild::channels ──► AppState::apply_gateway
UI drain_events: BackendEvent::Social(ev) ──► AppState::apply_social (state/social.rs) ──► toast?
```
Logged out, `rest` answers at once with "Not connected" (reads get their typed event with `Err`,
actions an `ActionFailed`).

## Contract (additive only; ask the contract owner before changing)
- `backend.rs`: `Command::Social`, `SocialCommand`, `SocialEvent`, `BackendEvent::Social`,
  `ThreadList`, `ArchivedThreads`, `ThreadQuery`, `ThreadSort`, `ForumPost`, `MentionFilter`,
  `ContextLocation`, `FriendTarget`, `FriendReply`, `Command::AckThread`, `MAX_REJECT_BATCH`, the message
  request commands / events (`FetchRequestPreviews`, `AcceptMessageRequest`,
  `RejectMessageRequests`, `RequestPreviewsLoaded`, `MessageRequestsDone`); `handle_social` +
  `opened` (done).
- `model.rs`: `Channel` thread/forum fields (`thread_metadata`, `member`, `member_count`,
  `message_count`, `applied_tags`, `available_tags`, `default_reaction_emoji`,
  `default_sort_order`, `default_forum_layout`, `default_auto_archive_duration`, `flags`),
  `ThreadMetadata`, `ThreadMember`, `ForumTag`, `DefaultReaction`, `Channel::{is_thread,
  is_archived}`, `Relationship` (+ `nickname`, `since`, `is_spam_request`), `relationship_type`,
  `ThreadPage`, `UserProfile` (+ `bio()`, `pronouns()`, `theme_colors()`, `banner()`),
  `ProfileFields`, `ProfileBadge`, `MutualGuild`, `ConnectedAccount`, the nine new
  `GatewayEvent` variants.
- `api/social/people.rs` and `api/threads.rs`: the `ApiClient` method signatures and
  `api::threads::create_forum_post` (routes, bodies, headers and error codes are in their doc
  comments).
- `state/social.rs`: `SocialState` and its views (UI reads fields directly), `FriendsTab`,
  `PROFILE_CACHE`, `PROFILE_TTL`, `AppState::{apply_social, request_profile, profile, dm_with,
  friends_tab, friend_name, pending_count, sidebar_threads, active_threads, open_thread,
  unread_channels, mark_all_read, message_requests, request_counts, begin_request_previews,
  more_request_previews, begin_request_action, request_preview}`; `SocialState.request_previews` /
  `request_busy`, `RequestPreviews`.

## File ownership
| Owner | Files | Status |
|---|---|---|
| contract owner | `backend.rs` (`handle_social`), `model.rs`, `state/social.rs` + its hooks in `state.rs` (`social` field, `replace_core` → `set_relationships`, relationship + social arms of `apply_gateway`, unjoined threads left out of `to_snapshot`), the `BackendEvent::Social` arm in `ui/mod.rs`, this page | done, unit-tested |
| friends/DMs/inbox engineer | `api/social/people.rs` (relationship, DM, inbox, profile, note calls; `mod people;` in `api/social.rs`), `ApiClient::send_coded` in `api/mod.rs` (error `code` for 80013), `gateway/social.rs` (CHANNEL_RECIPIENT_ADD/REMOVE, RECENT_MENTION_DELETE, USER_NOTE_UPDATE; one guard arm in `normalize::dispatch`) | built, unit-tested |
| threads-forums engineer | `api/threads.rs` (thread/forum calls + `create_forum_post`), `gateway/threads.rs` (THREAD_* dispatches; READY / GUILD_CREATE `threads[]` in `normalize::GuildWire`), `state/social/threads.rs` (permission gates, `check_forum_post`, thread `message_count`) + its hooks in `state.rs` (`count_thread_messages` in `apply_gateway`, thread-member mute in `is_channel_muted`, joined threads in the guild badge) | built, unit-tested |
| integrator | `Command::AckThread` + `ApiClient::ack(.., thread)`, `CreateThread::parent_kind` (type 10 under announcement channels), this page's status and hook-up guide | done, unit-tested |

## Per-area rules
- **Every request is one user action.** No bulk, no queue, no automatic retry of a 4xx; 429
  goes through the limiter. A captcha answer (`captcha_key` in a 400) becomes "Discord wants a
  captcha for this action. Complete it in the official app, then try again." in
  `ApiClient`'s error mapping (already there): never solved, bypassed or automated.
- **Threads** (§2.1): threads are channels in `AppState::channels`; `member.is_some()` =
  joined. Joined active ones come from READY/GUILD_CREATE, other active ones from
  THREAD_LIST_SYNC (op 37 already sends `threads: true`). Guild-wide REST active lists are not
  usable by users: per channel use `ThreadList::Active` (thread search `archived=false`).
  Archived threads leave the state unless open. Create without a message always sends `type`
  (server default is private): 10 under an announcement channel (`parent_kind`), else 11 / 12.
  Read acks in threads go out as `Command::AckThread` (flags 3).
- **Forums** (§2.2): `ThreadList::Forum(ThreadQuery)` = thread search (tags, sort, name, offset
  += 25, ≤ 9975; 202 → one delayed retry in the call). New post = cloud upload against the
  forum id, then `POST /channels/{forum}/threads?use_nested_fields=true` with `message`;
  uploads are deleted again when that fails. A new post bumps the forum's `last_message_id`.
- **Relationships** (§2.3): every type change arrives as RELATIONSHIP_ADD (replace);
  RELATIONSHIP_UPDATE is partial (nickname, `user_ignored`, `since`). Calls send
  `X-Context-Properties` for the `ContextLocation`. 80013 → `FriendReply::ConfirmStranger`
  (ask, then `AcceptFriend { confirm_stranger: true }`; a second 80013 after confirming, or one
  on Add Friend by username, is error text); 80000-80008 / 30002 → readable text.
- **DMs** (§2.4): open the cached DM first (`dm_with`); `OpenDm` only otherwise. Adding a
  member to a 1:1 DM creates a new group (answered as `ChannelOpened`). My own
  CHANNEL_RECIPIENT_REMOVE drops the group.
- **Inbox** (§2.5): mentions are REST pages (newest first, 7 days); unreads are local
  (`unread_channels`); "Mark all read" = `mark_all_read` locally + `MarkAllRead { acks }`
  (ack-bulk, chunks of 100, sequential).
- **Profiles** (§2.6): fetch only on click, gated by `request_profile` (per (user, guild),
  TTL 5 min, in-flight dedupe, lost requests retry after 30 s). The note rides along with the
  profile (its failure does not hide the profile); USER_NOTE_UPDATE patches cached profiles.

## Memory bounds (state/social.rs)
Relationships ≤ 10,000 (Discord's own limits). Unjoined threads in `channels` ≤ 1,000 (least
recent activity dropped, never the open one; not written to the disk cache). Thread list view
≤ 500 threads + their first messages. Mentions view ≤ 500. Profiles: LRU of 64. One view each
(threads/forum, mentions, Add Friend).

## Notes for the UI block
- Open a view by setting it, then send the command; stale answers are dropped by key
  (channel + list kind/filters, mention filter, username, (user, guild)).
- Thread send gate is already in `can_send` (SEND_MESSAGES_IN_THREADS). Archived threads are
  read-only (`is_archived()`; sending unarchives unless `locked`).
- Gate thread/forum buttons with `can_create_thread(parent, private)`, `can_create_post(forum)`,
  `can_manage_threads(ch)`, `can_join_thread(&thread)`, `can_set_archived(&thread, archived)`,
  `can_list_archived(ch, kind)`; the New Post dialog offers `post_tags(forum)` and shows
  `check_forum_post(forum, &post)`'s error instead of sending. Joined threads count towards the
  guild badge (muted on their thread member: mentions only); `channels[t].message_count`
  follows MESSAGE_CREATE / DELETE.
- `ThreadsView.threads` are copies from the REST page: for `message_count` and membership
  prefer `channels[id]` when the thread is known there (it follows the gateway).
- `can_set_archived(.., false)` (unarchive) uses SEND_MESSAGES_IN_THREADS on the parent; the
  notes only say "any member unless locked", so this rule is a reading, not a confirmed fact.
- Not covered yet: thread member list (op 37 `thread_member_lists`), thread notification/mute
  settings (reading the mute works), spam reports for message requests, group icon / owner transfer, friend
  nickname / ignore, clear-all requests, For You tab, mutual-friends list call, bulk notes,
  forum `post-data`, THREAD_LIST_SYNC `most_recent_messages` (ignored), the sidebar's
  `auto_archive_duration` hide rule.

## UI hook-up guide
Commands are `Command::Social(SocialCommand::X)`; answers arrive as `BackendEvent::Social` and
are already applied in `ui/mod.rs` `drain_events` (`apply_social`, toast on `Some`). Gateway
events are applied by `apply_gateway`. The UI only sends commands and reads state.

| Feature | UI sends | Comes back as | UI reads |
|---|---|---|---|
| Threads under channels (sidebar) | nothing | READY / `ThreadUpsert` / `ThreadListSync` / member events | `sidebar_threads(guild)` (parent → thread ids, joined + the open one) |
| Thread browser: Active | `social.threads = Some(ThreadsView::new(ch, ThreadList::Active { offset: 0 }))`, `FetchThreads`; local list meanwhile: `active_threads(ch)` | `ThreadsLoaded` | `ThreadsView.threads/has_more/loading/error`; more: `if let Some(l) = v.next_page()` → `v.loading = true`, `FetchThreads { list: l }` |
| Thread browser: Archived | same with `ThreadList::Archived { kind: Public \| Private \| Joined, before: None }` (`Private` needs MANAGE_THREADS) | `ThreadsLoaded` | same; `next_page()` builds `before` |
| Open a thread from a list | `open_thread(channel.clone())` (adds + selects; history loads as for any channel) | – | – |
| Join / leave | `JoinThread` / `LeaveThread` | thread member events; `ActionFailed` on error | `channels[t].member.is_some()` |
| Create thread | `CreateThread { channel_id, name, from_message, private, parent_kind: channels[channel_id].kind, auto_archive_minutes }` (gate: `can_create_thread(parent, private)`; preselect parent's `default_auto_archive_duration`, else 4320; `private` only under text channels with CREATE_PRIVATE_THREADS) | `ChannelOpened` (opened) or toast | – |
| Read acks in a thread | in `ui/mod.rs` `flush_acks`: `if channels[c].is_thread() { Command::AckThread { channel_id: c, message_id } } else { Command::Ack { .. } }` (done) | nothing (silent) | – |
| Archive / unarchive | `SetThreadArchived` | `ThreadUpsert`; `ActionFailed` | `is_archived()`, `thread_metadata.locked` |
| Forum post list | `social.threads = Some(ThreadsView::new(forum, ThreadList::Forum(ThreadQuery { sort, tags, match_all_tags, name, .. })))`, `FetchThreads`; sort default from `default_sort_order` | `ThreadsLoaded` | cards: `threads`, `first_message(id)`, `applied_tags` vs forum `available_tags`, `message_count`, `total_results`; layout from `default_forum_layout` (media = grid) |
| New forum post | `if let Err(e) = state.check_forum_post(forum, &post)` show `e`, else `CreateForumPost { channel_id, post: ForumPost { name, content, applied_tags, files, auto_archive_minutes } }` (preselect the forum's `default_auto_archive_duration`, else 4320) (tags offered: `post_tags(forum)`; REQUIRE_TAG = `flags & 1<<4` → ≥ 1 tag; moderated tags need MANAGE_THREADS; sizes via `upload_limit(forum)`) | `ChannelOpened` (post opened) or toast | – |
| Friends tabs | nothing (READY + RELATIONSHIP_*) | – | `friends_tab(FriendsTab::…)`, `friend_name(id)`, `social.relationships[id]` (`since`, `nickname`), `pending_count()` (Home badge) |
| Add Friend | `social.add_friend = Some(AddFriendView { username, result: None })`, `SendFriendRequest { username }` | `FriendResult { Username }` | `AddFriendView.result` (`Ok` = "request sent", `Err` text incl. captcha) |
| Accept / add by id | `AcceptFriend { user_id, confirm_stranger: false, location }` | `FriendResult { User }`; `ConfirmStranger` → `social.confirm_stranger = Some(id)` | ask, then resend with `confirm_stranger: true`; clear the field either way |
| Decline / cancel / unfriend / unblock | `RemoveRelationship { user_id, location }` | RELATIONSHIP_REMOVE; `ActionFailed` | – |
| Block | `BlockUser { user_id, location }` (confirm first) | RELATIONSHIP_ADD type 2; `ActionFailed` | – |
| Message a user | `match state.dm_with(u) { Some(c) => state.select_channel(c), None => send OpenDm { user_id: u } }` | `ChannelOpened` | – |
| New group DM | `CreateGroupDm { recipients }` (friends only, ≤ 9) | `ChannelOpened` | – |
| Add / remove group member | `AddRecipient` / `RemoveRecipient` (owner) | `ChannelRecipientAdd/Remove`; a 1:1 DM → `ChannelOpened` (new group) | `channels[c].recipient_ids` |
| Rename group | `RenameGroup { channel_id, name }` (`""` = no name) | CHANNEL_UPDATE; `ActionFailed` | `channels[c].name` |
| Close DM / leave group | `CloseDm { channel_id, silent }` (confirm leaving) | CHANNEL_DELETE; `ActionFailed` | – |
| Message requests: previews | `social.request_previews = Some(Default::default())`, then `if let Some(ids) = state.begin_request_previews(spam) { FetchRequestPreviews { channel_ids: ids } }` (≤ 25; only on a click: open, tab, "Load more previews", "Retry") | `RequestPreviewsLoaded` (an error is kept in the view, no toast) | `request_preview(c)`, `more_request_previews(spam)`, `RequestPreviews.loading/error` |
| Message requests: accept | `if state.begin_request_action(&[c]) { AcceptMessageRequest { channel_id: c } }` | `MessageRequestsDone { accepted: true }` clears the flags (CHANNEL_UPDATE does too) | `request_busy`, `can_send(c)` |
| Message requests: ignore | `begin_request_action(&ids)`, then `RejectMessageRequests { channel_ids }` (one id = DELETE, 2..=`MAX_REJECT_BATCH` = batch; confirm batches) | `MessageRequestsDone { accepted: false }` removes the channels locally (a `ChannelDelete`) | `message_requests(spam)`, `request_counts()` |
| Inbox: Mentions | `social.mentions = Some(MentionsView::new(filter))`, `FetchMentions { filter, before: None, limit: 25 }`; more: `before = v.next_before()` | `MentionsLoaded` | `MentionsView.messages/has_more/loading/error` |
| Inbox: remove a mention | `DeleteMention { message_id }` | `RecentMentionDelete` removes it | – |
| Inbox: Unreads | nothing (local) | – | `unread_channels()` |
| Mark all read | `let acks = state.mark_all_read(&ids); if settings.send_read_acks && !acks.is_empty() { MarkAllRead { acks } }` (local mark-read either way) | MESSAGE_ACKs; `ActionFailed` | – |
| Profile popout / modal | `if state.request_profile(u, g, Instant::now()) { FetchProfile { user_id: u, guild_id: g } }` on open | `ProfileLoaded` | `profile(u, g)`: `loading`, `result` (`Ok(p)`: `p.bio()`, `pronouns()`, `banner()`, `theme_colors()`, `mutual_guilds`, `mutual_friends`, `badges`, `connected_accounts`, `note`; `Err` text, e.g. 403) |
| Note | `SetNote { user_id, note }` on blur (≤ 256 chars) | `UserNoteUpdate` patches cached profiles; `ActionFailed` | `profile(..).result….note` |

Removing the `#[allow(dead_code, reason = "…not built yet")]` markers on `Command::Social`,
`SocialCommand`, the thread/forum/profile types and `state/social.rs` is
part of the UI block once they are used.
