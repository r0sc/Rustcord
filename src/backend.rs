//! UI <-> network boundary.
//!
//! The UI thread owns all application state (`state::AppState`). It sends `Command`s to the
//! backend (a tokio task) and drains `BackendEvent`s once per frame. The backend never
//! touches UI state; it only performs I/O and reports results. Every event send wakes the
//! UI via `egui::Context::request_repaint_after` (`EventSink`).
//!
//! OWNERSHIP: the enums/structs here are a shared contract (integrator only).
//! `spawn`/`run` bodies belong to the api/ implementer.

use crate::auth::{Secret, Token};
use crate::model::{
    Ban, Channel, Emoji, GatewayEvent, GifItem, GuildFolder, Id, Invite, Member, Message, Overwrite, PinsPage, SearchPage, ThreadPage,
    User, UserProfile, UserStatus,
};
use std::path::PathBuf;
use std::sync::mpsc;
use tokio::sync::mpsc as tmpsc;

/// What the UI asks the backend to do. All requests are user-initiated (or the
/// official-client-equivalent read ack); the backend never sends anything on its own.
#[derive(Debug)]
pub enum Command {
    /// Validate the token (GET /users/@me), store it in the keyring on success, then
    /// connect the gateway. `remember=false` skips the keyring (token kept in memory only).
    Login { token: Token, remember: bool },
    /// Token already came from the keyring at startup: connect without re-validating.
    Resume { token: Token },
    /// Email/phone + password login (POST /auth/login). Answered with `LoginStep`s; on
    /// success the received token goes through the same path as `Login`.
    /// `captcha`: the answer to the captcha Discord asked for (set by the backend, never the UI).
    LoginPassword { login: String, password: Secret, remember: bool, captcha: Option<CaptchaAnswer> },
    /// Answer the 2FA challenge of the last `LoginPassword`.
    SubmitMfa { method: MfaMethod, code: Secret, captcha: Option<CaptchaAnswer> },
    /// Text me the 2FA code (only offered when `MfaRequired` lists `Sms`).
    SendMfaSms { captcha: Option<CaptchaAnswer> },
    /// Backend only: buy the token for an approved QR login's ticket.
    QrExchange { ticket: Secret, keys: Arc<RaKeys>, remember: bool, captcha: Option<CaptchaAnswer> },
    /// How the captcha in the login card ended (`LoginStep::Captcha`): the widget's token, the card's
    /// Cancel or the 10-minute cap, or no webview.
    Captcha(CaptchaReply),
    /// Start a QR-code (remote auth) login; replaces a running one.
    StartQrLogin { remember: bool },
    /// Stop the QR login (other tab chosen / login screen left).
    CancelQrLogin,
    /// Disconnect, drop the token from memory and delete it from the keyring.
    Logout,
    FetchMessages { channel_id: Id, before: Option<Id>, limit: u8 },
    SendMessage { channel_id: Id, content: String, nonce: String, reply: Option<ReplyTo> },
    EditMessage { channel_id: Id, message_id: Id, content: String },
    DeleteMessage { channel_id: Id, message_id: Id },
    AddReaction { channel_id: Id, message_id: Id, emoji: Emoji },
    RemoveReaction { channel_id: Id, message_id: Id, emoji: Emoji },
    /// Mark read up to `message_id` (POST .../ack). The UI debounces this.
    Ack { channel_id: Id, message_id: Id },
    /// `Ack` for a thread (types 10/11/12): same call with read-state `flags` 3.
    AckThread { channel_id: Id, message_id: Id },
    /// Gateway op 37 guild subscription: member sidebar + typing for the viewed guild channel.
    SubscribeMemberList { guild_id: Id, channel_id: Id },
    /// Fetch (disk cache, then network) and decode an image. `max_side` is the largest
    /// pixel dimension the UI will draw it at; decoding downsizes to that.
    LoadImage { url: String, max_side: u32 },
    // Voice (answered with `BackendEvent::Voice`). Only ever sent on a user click.
    /// Join or switch to a voice channel; `guild_id` None = DM/group call (also accepts an
    /// incoming call).
    JoinVoice { guild_id: Option<Id>, channel_id: Id },
    LeaveVoice,
    /// DM call I started (VOICE_NOTES §1.4), sent once its CALL_CREATE arrived:
    /// `POST /channels/{id}/call/ring` (`recipients` None = everyone); `check_ringable` (1:1 DM
    /// with a non-friend) asks `GET /channels/{id}/call` first and rings only if `ringable`.
    /// Failures: `ActionFailed`.
    RingCall { channel_id: Id, recipients: Option<Vec<Id>>, check_ringable: bool },
    /// `POST /channels/{id}/call/stop-ringing`; `recipients` None = me (decline an incoming call).
    StopRinging { channel_id: Id, recipients: Option<Vec<Id>> },
    SetSelfMute(bool),
    /// Deafen also mutes.
    SetSelfDeaf(bool),
    /// cpal device names (from `VoiceEvent::Devices`); None = system default.
    SetVoiceDevices { input: Option<String>, output: Option<String> },
    /// Voice activity (threshold) or push-to-talk.
    SetVoiceInputMode(crate::voice::InputMode),
    /// Answered with `VoiceEvent::Devices`.
    ListVoiceDevices,
    /// Camera, screen share, Go Live (docs/VIDEO_DESIGN.md); answered with
    /// `BackendEvent::Voice(VoiceEvent::Video(..))`. Frames: `BackendHandle::video`.
    Video(crate::voice::video::VideoCommand),
    /// Chat essentials (uploads, typing, status, pins, search, GIFs, message extras); answered
    /// with `BackendEvent::Chat`. docs/CHAT_ESSENTIALS_DESIGN.md.
    Chat(ChatCommand),
    /// Threads, forums, friends, DMs, inbox, profiles; answered with `BackendEvent::Social`
    /// (or `ActionFailed`). docs/SOCIAL_DESIGN.md.
    Social(SocialCommand),
    /// Settings proto, own profile, server administration, invites, notification settings;
    /// answered with `BackendEvent::Account`. docs/ACCOUNT_SERVER_DESIGN.md.
    Account(AccountCommand),
    /// Game detection and activity icons (`crate::activity`); answered with
    /// `BackendEvent::Activity`.
    Activity(crate::activity::ActivityCommand),
    /// Quests list, accept, claim, reward code, video progress (`crate::quests`); answered
    /// with `BackendEvent::Quests`. docs/QUESTS_NOTES.md.
    Quests(crate::quests::QuestCommand),
}

#[derive(Debug, Clone)]
pub struct ReplyTo {
    pub message_id: Id,
    pub guild_id: Option<Id>,
    /// Whether the replied-to author gets pinged.
    pub mention_author: bool,
}

/// Chat-essentials requests (FEATURES_NOTES Part 1). Each one is the direct result of a user
/// action (typing: a keystroke that changes non-empty composer text).
#[derive(Debug)]
pub enum ChatCommand {
    /// Cloud upload (§1.1) then the message: `UploadProgress`* then `SendResult` with this
    /// nonce, like `SendMessage` (keep a `PendingMessage` + `AppState::chat.uploads` entry).
    /// `content` may be empty. 1..=10 files; the UI pre-checks sizes (`AppState::upload_limit`).
    SendWithFiles { channel_id: Id, content: String, nonce: String, reply: Option<ReplyTo>, files: Vec<UploadFile> },
    /// The user removed a sending upload: stop it and delete what was uploaded. No answer.
    CancelUpload { nonce: String },
    /// "I'm typing" (§1.2). Send on every keystroke that changes non-empty text (not `/…`,
    /// not while editing, only with send permission); the backend sends at most one
    /// `POST /typing` per channel per 8 s and resets that on send. A slowmode answer comes back
    /// as `ChatEvent::SendCooldown`.
    Typing { channel_id: Id },
    /// Op 3 for this session (§1.3), keeping the current custom status. Not persisted: the
    /// settings write (`PATCH /users/@me/settings` / proto) belongs to the settings block.
    /// `afk`: an automatic idle (CustomIdle), like the official auto-idle.
    SetStatus { status: UserStatus, afk: bool },
    /// `GET /channels/{c}/messages/pins?before=<pinned_at>&limit=50` -> `PinsLoaded`.
    FetchPins { channel_id: Id, before: Option<String> },
    /// `PUT /channels/{c}/messages/pins/{m}` (PIN_MESSAGES in guilds). Failures: `ActionFailed`.
    PinMessage { channel_id: Id, message_id: Id },
    /// `DELETE /channels/{c}/messages/pins/{m}`. Failures: `ActionFailed`.
    UnpinMessage { channel_id: Id, message_id: Id },
    /// Message search (§1.5) on Enter / page change -> `SearchResults`.
    Search { scope: SearchScope, query: SearchQuery },
    /// `@` autocomplete beyond the local cache (§1.7): op 8 with `query` (never `""`), `limit`
    /// 1..=100. Send when `AppState::need_member_search` says so; the gateway debounces per
    /// guild (newest query wins) -> `MembersFound`.
    SearchMembers { guild_id: Id, query: String, limit: u8 },
    /// GIF picker (§1.8): `None` = trending, `Some(q)` = search (debounce ≥ 500 ms in the UI)
    /// -> `GifResults`.
    Gifs { query: Option<String> },
    /// "Mark unread" (§1.9): `POST …/messages/{message_id}/ack {"manual":true,"mention_count"}`.
    /// Get both values from `AppState::mark_unread`. Failures: `ActionFailed`.
    MarkUnread { channel_id: Id, message_id: Id, mention_count: u32 },
    /// Jump to a message (§1.10): `GET …/messages?around={m}&limit={limit}` -> `AroundLoaded`
    /// (the jump window asks 50; MessageLinkEmbeds 1, the message itself).
    FetchAround { channel_id: Id, message_id: Id, limit: u8 },
    /// Remove embeds (§1.9): `PATCH …/messages/{m} {"flags": flags | 4}`; `flags` = the
    /// message's current flags. Failures: `ActionFailed`.
    SuppressEmbeds { channel_id: Id, message_id: Id, flags: u64 },
    /// Who reacted (§1.9): `GET …/reactions/{emoji}?type=0|1&limit=100&after=` -> `ReactionUsers`.
    #[allow(dead_code, reason = "sent by the who-reacted popout, which is not built yet")]
    FetchReactionUsers { channel_id: Id, message_id: Id, emoji: Emoji, burst: bool, after: Option<Id> },
    /// Forward one message to one channel: `POST /channels/{dest}/messages` with a type 1
    /// `message_reference` to `message_id` in `channel_id`, no content -> `Forwarded`.
    Forward { channel_id: Id, guild_id: Option<Id>, message_id: Id, dest: Id, nonce: String },
    /// Vote: `PUT /channels/{c}/polls/{m}/answers/@me {"answer_ids"}` (`[]` removes my votes).
    /// The UI has already counted it; a failure answers `PollVoteFailed` with `restore`.
    PollVote { channel_id: Id, message_id: Id, answer_ids: Vec<u32>, restore: Vec<u32> },
    /// Who voted for an answer: `GET /channels/{c}/polls/{m}/answers/{a}?limit=100[&after=]`
    /// -> `PollVoters`.
    FetchPollVoters { channel_id: Id, message_id: Id, answer_id: u32, after: Option<Id> },
    /// Create a poll: `POST /channels/{c}/messages` with `poll` and no content -> `PollPosted`
    /// (MESSAGE_CREATE brings the poll).
    SendPoll { channel_id: Id, nonce: String, poll: PollDraft },
}

/// A poll as the create dialog sends it: trimmed, empty answers dropped.
#[derive(Debug, Clone)]
pub struct PollDraft {
    pub question: String,
    pub answers: Vec<String>,
    /// How long it runs, in hours (always sent: the documented default differs).
    pub hours: u32,
    pub multi: bool,
}

/// One attachment of `ChatCommand::SendWithFiles`.
#[derive(Debug, Clone)]
pub struct UploadFile {
    pub source: UploadSource,
    /// Name as shown in chat, without the `SPOILER_` prefix (the backend adds it).
    pub filename: String,
    pub spoiler: bool,
    /// Alt text (≤ 1024 chars).
    pub description: Option<String>,
}

#[derive(Clone)]
pub enum UploadSource {
    /// A file on disk (picker, drag and drop); read by the backend when sending.
    Path(std::path::PathBuf),
    /// In-memory bytes, e.g. a pasted image encoded as PNG (`image.png`).
    Bytes(std::sync::Arc<[u8]>),
}

/// Never prints file contents.
impl std::fmt::Debug for UploadSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UploadSource::Path(p) => f.debug_tuple("Path").field(p).finish(),
            UploadSource::Bytes(b) => write!(f, "Bytes({} bytes)", b.len()),
        }
    }
}

/// Where a search runs. "This channel" in a guild = `Guild` + `SearchQuery::channel_ids`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchScope {
    /// `GET /guilds/{g}/messages/search` (READ_MESSAGE_HISTORY).
    Guild(Id),
    /// `GET /channels/{c}/messages/search`: a DM or group DM.
    Dm(Id),
}

/// Search filters (§1.5); list fields become repeated query keys. The UI parses `from:`,
/// `mentions:`, `has:`, `in:`, `pinned:`, `before:`/`after:` (dates via `Id::from_timestamp_ms`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchQuery {
    /// Free text (≤ 1024 chars).
    pub content: Option<String>,
    pub author_ids: Vec<Id>,
    pub mentions: Vec<Id>,
    /// `image|sound|video|file|sticker|embed|link|poll|snapshot`; `-x` negates.
    pub has: Vec<String>,
    /// Guild scope only (≤ 500).
    pub channel_ids: Vec<Id>,
    pub pinned: Option<bool>,
    /// `after:` (exclusive lower bound).
    pub min_id: Option<Id>,
    /// `before:` (exclusive upper bound); also the cursor past offset 9975.
    pub max_id: Option<Id>,
    /// Page offset in hits: 0, 25, 50, … ≤ 9975.
    pub offset: u32,
    pub sort: SearchSort,
    /// Only honoured when the account may see NSFW content.
    pub include_nsfw: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SearchSort {
    /// `sort_by=timestamp&sort_order=desc`.
    #[default]
    Newest,
    /// `sort_by=timestamp&sort_order=asc`.
    Oldest,
    /// `sort_by=relevance`.
    Relevance,
}

#[derive(Debug, Clone)]
pub enum SearchOutcome {
    Page(SearchPage),
    /// HTTP 202 / code 110000 (index not ready) even after the one delayed retry the backend
    /// makes for the user's search. The UI shows "Indexing…" and may offer a manual retry.
    Indexing {
        #[allow(dead_code, reason = "the search panel offers a manual retry instead")]
        retry_after_secs: f32,
    },
    Failed(String),
}

/// Answers to `ChatCommand`s (inside `BackendEvent::Chat`). Errors are user-facing text.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // a few per user action
pub enum ChatEvent {
    /// Bytes of `SendWithFiles` uploaded so far (advances per finished file).
    UploadProgress { nonce: String, sent: u64, total: u64 },
    /// `POST /typing` answered with a slowmode cooldown: sending is blocked for `ms`.
    SendCooldown { channel_id: Id, ms: u64 },
    PinsLoaded { channel_id: Id, before: Option<String>, result: Result<PinsPage, String> },
    /// Echoes the request so stale answers can be told apart.
    SearchResults { scope: SearchScope, query: SearchQuery, result: SearchOutcome },
    /// Op 8 answer (GUILD_MEMBERS_CHUNK matched by nonce); members carry `user`.
    MembersFound { guild_id: Id, query: String, members: Vec<Member> },
    GifResults { query: Option<String>, result: Result<Vec<GifItem>, String> },
    /// `?around=` page, as received (state sorts).
    AroundLoaded { channel_id: Id, message_id: Id, result: Result<Vec<Message>, String> },
    ReactionUsers { channel_id: Id, message_id: Id, emoji: Emoji, after: Option<Id>, result: Result<Vec<User>, String> },
    /// Answer of `Forward`; the forwarded message arrives as MESSAGE_CREATE.
    Forwarded { dest: Id, result: Result<(), String> },
    /// `PollVote` failed: put my votes back to `restore`.
    PollVoteFailed { channel_id: Id, message_id: Id, restore: Vec<u32>, error: String },
    PollVoters { channel_id: Id, message_id: Id, answer_id: u32, after: Option<Id>, result: Result<Vec<User>, String> },
    /// Answer of `SendPoll`.
    PollPosted { channel_id: Id, result: Result<(), String> },
}

/// Social requests (FEATURES_NOTES Part 2). Each is the direct result of one user action; none
/// is retried on 4xx, captcha answers become a "do it in the official app" error. Actions
/// whose success shows up through the gateway (RELATIONSHIP_*, THREAD_*, CHANNEL_RECIPIENT_*,
/// CHANNEL_DELETE, USER_NOTE_UPDATE, RECENT_MENTION_DELETE) only report failures, as
/// `ActionFailed`.
/// Most message requests one batch ignore takes (`batch-reject`).
pub const MAX_REJECT_BATCH: usize = 50;

#[derive(Debug)]
#[allow(dead_code, reason = "sent by the social UI, which is not built yet")]
pub enum SocialCommand {
    // ----- threads and forums (§2.1, §2.2)
    /// A thread list for the thread browser / forum view -> `ThreadsLoaded` echoing `list`.
    /// Covers "FetchActiveThreads" (`ThreadList::Active`; guild-wide active threads come from
    /// THREAD_LIST_SYNC instead, the per-guild REST list is not usable by users),
    /// "FetchArchivedThreads" (`Archived`) and "FetchForumPosts" (`Forum`).
    FetchThreads { channel_id: Id, list: ThreadList },
    /// `PUT /channels/{t}/thread-members/@me` (unarchived threads only). THREAD_MEMBERS_UPDATE
    /// / THREAD_MEMBER_UPDATE carry the result.
    JoinThread { thread_id: Id },
    /// `DELETE /channels/{t}/thread-members/@me`.
    LeaveThread { thread_id: Id },
    /// From a message: `POST /channels/{c}/messages/{m}/threads` (thread id = message id);
    /// else `POST /channels/{c}/threads` with `type` 11 (public), 12 (`private`, text channels
    /// only) or 10 (`parent_kind` = the parent's `Channel::kind` is an announcement channel);
    /// always sent, the server defaults to private. `name` 1-100 chars;
    /// `auto_archive_minutes` 60/1440/4320/10080 (preselect the parent's
    /// `default_auto_archive_duration`). -> `ChannelOpened` (the new thread is opened).
    CreateThread { channel_id: Id, name: String, from_message: Option<Id>, private: bool, parent_kind: u8, auto_archive_minutes: u32 },
    /// `PATCH /channels/{t} {"archived"}` (unarchive: any member unless locked; archive:
    /// creator or MANAGE_THREADS). THREAD_UPDATE carries the result.
    SetThreadArchived { thread_id: Id, archived: bool },
    /// New forum/media post: cloud uploads against the forum (the `SendWithFiles` pipeline),
    /// then `POST /channels/{forum}/threads` with the first message. Needs SEND_MESSAGES on the
    /// forum; REQUIRE_TAG forums need ≥ 1 tag. -> `ChannelOpened` (the post is opened).
    CreateForumPost { channel_id: Id, post: ForumPost },
    // ----- relationships (§2.3). Only on an explicit click; never in bulk.
    /// Add Friend tab: `POST /users/@me/relationships {"username","discriminator":null}`
    /// (`name#1234` sends the legacy discriminator) -> `FriendResult { Username }`.
    SendFriendRequest { username: String },
    /// Accept an incoming request or add by id (profile / menu): `PUT
    /// /users/@me/relationships/{id} {}`; `confirm_stranger` re-sends after the user confirmed
    /// a stranger request (80013) -> `FriendResult { User }`.
    AcceptFriend { user_id: Id, confirm_stranger: bool, location: ContextLocation },
    /// Decline / cancel / unfriend / unblock: `DELETE /users/@me/relationships/{id}`.
    RemoveRelationship { user_id: Id, location: ContextLocation },
    /// `PUT /users/@me/relationships/{id} {"type":2}` (also ends a friendship).
    BlockUser { user_id: Id, location: ContextLocation },
    // ----- DMs (§2.4). Open a cached DM first (`AppState::dm_with`).
    /// `POST /users/@me/channels {"recipients":[id]}` -> `ChannelOpened`.
    OpenDm { user_id: Id },
    /// `POST /users/@me/channels {"recipients":[…]}` (≤ 9 others, friends) -> `ChannelOpened`.
    CreateGroupDm { recipients: Vec<Id> },
    /// `PUT /channels/{c}/recipients/{u}`. On a group: CHANNEL_RECIPIENT_ADD. On a 1:1 DM this
    /// creates a new group -> `ChannelOpened`.
    AddRecipient { channel_id: Id, user_id: Id },
    /// `DELETE /channels/{c}/recipients/{u}` (group owner).
    RemoveRecipient { channel_id: Id, user_id: Id },
    /// `PATCH /channels/{c} {"name"}`; empty = no name.
    RenameGroup { channel_id: Id, name: String },
    /// Close a 1:1 DM (hidden, history kept) or leave a group: `DELETE /channels/{c}`
    /// (`?silent=true` for a group skips the "left" message). CHANNEL_DELETE follows.
    CloseDm { channel_id: Id, silent: bool },
    // ----- inbox (§2.5)
    /// `GET /users/@me/mentions?limit&roles&everyone[&guild_id][&before]` (last 7 days, newest
    /// first) -> `MentionsLoaded`.
    FetchMentions { filter: MentionFilter, before: Option<Id>, limit: u8 },
    /// Inbox ×: `DELETE /users/@me/mentions/{m}` -> RECENT_MENTION_DELETE.
    DeleteMention { message_id: Id },
    /// "Mark all read": `POST /read-states/ack-bulk` with `(channel, message)` pairs from
    /// `AppState::mark_all_read` (chunked by 100 in the API). Only when `send_read_acks` is on.
    MarkAllRead { acks: Vec<(Id, Id)> },
    // ----- profiles (§2.6)
    /// `GET /users/{u}/profile?with_mutual_guilds=true&with_mutual_friends=true[&guild_id]&type=popout`
    /// plus my note (`GET /users/@me/notes/{u}`, 404 = none) -> `ProfileLoaded`. Only on click,
    /// gated by `AppState::request_profile`.
    FetchProfile { user_id: Id, guild_id: Option<Id> },
    /// `PUT /users/@me/notes/{u} {"note"}` (≤ 256 chars, `""` clears) -> USER_NOTE_UPDATE.
    SetNote { user_id: Id, note: String },
    /// Cosmetics editor (COSMETICS_NOTES §12): `GET /collectibles-categories/v2` (saved for
    /// 24 h) -> `CollectiblesLoaded`. Only when the editor opens without a fresh saved copy.
    FetchCollectibles,
    /// `GET /users/@me/collectibles-purchases` -> `OwnedCollectiblesLoaded`. When the editor opens.
    FetchOwnedCollectibles,
    // ----- profile cosmetics display (COSMETICS_NOTES §5, §6, §10, §11; api/cosmetics.rs)
    /// `GET /collectibles-products/{sku}` (profile effect / frame config) -> `ProductLoaded`.
    /// Once per SKU per session, when a shown profile needs it (`AppState::request_product`).
    FetchProduct { sku_id: Id },
    /// `GET /games?game_ids=…` (≤ 25) for an open profile's widgets -> `GamesLoaded`
    /// (`AppState::request_games`).
    FetchGames { ids: Vec<Id> },
    // ----- message requests (§2.4; api/social/people.rs). Only on a click on the requests page.
    /// `GET /users/@me/message-requests/supplemental-data?channel_ids=…` (1-25) ->
    /// `RequestPreviewsLoaded`.
    FetchRequestPreviews { channel_ids: Vec<Id> },
    /// `PUT /channels/{c}/recipients/@me {"consent_status":2}` -> `MessageRequestsDone { accepted: true }`.
    AcceptMessageRequest { channel_id: Id },
    /// Ignore: one id `DELETE /channels/{c}/recipients/@me`, 2..=50 `PUT
    /// /channels/recipients/@me/batch-reject` (confirm batches first) ->
    /// `MessageRequestsDone { accepted: false }`.
    RejectMessageRequests { channel_ids: Vec<Id> },
    /// `GET /users/@me/connections` -> `ConnectionsLoaded` (Settings → Connections opened).
    FetchConnections,
    /// `PATCH /users/@me/connections/{type}/{id}` with the set toggles, then the list again ->
    /// `ConnectionsLoaded`.
    UpdateConnection { kind: String, id: String, visibility: Option<bool>, show_activity: Option<bool> },
    /// `DELETE /users/@me/connections/{type}/{id}` (confirm first), then the list again.
    RemoveConnection { kind: String, id: String },
}

/// Which thread list `SocialCommand::FetchThreads` loads; echoed in `ThreadsLoaded`.
#[derive(Debug, Clone, PartialEq)]
pub enum ThreadList {
    /// Active threads of a text/news channel (thread search `archived=false`, latest activity
    /// first); next page `offset += 25`.
    Active { offset: u32 },
    /// Archived threads, newest first. `before`: public/private = the last thread's
    /// `archive_timestamp`, joined = the last thread's id (`ThreadsView::next_page`).
    Archived { kind: ArchivedThreads, before: Option<String> },
    /// Forum / media posts (thread search).
    Forum(ThreadQuery),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchivedThreads {
    /// `GET /channels/{c}/threads/archived/public` (READ_MESSAGE_HISTORY).
    Public,
    /// `GET /channels/{c}/threads/archived/private` (+ MANAGE_THREADS).
    Private,
    /// `GET /channels/{c}/users/@me/threads/archived/private`: private threads I joined.
    Joined,
}

/// `GET /channels/{forum}/threads/search` parameters (§2.2).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThreadQuery {
    /// Title search (≤ 100 chars).
    pub name: Option<String>,
    /// Tag filter (repeated `tag=`, ≤ 20).
    pub tags: Vec<Id>,
    /// `tag_setting=match_all` instead of `match_some`.
    pub match_all_tags: bool,
    /// None = active and archived.
    pub archived: Option<bool>,
    pub sort: ThreadSort,
    /// 0, 25, 50, … ≤ 9975.
    pub offset: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(dead_code, reason = "chosen in the forum UI, which is not built yet")]
pub enum ThreadSort {
    /// "Recent activity": `sort_by=last_message_time`.
    #[default]
    LastActivity,
    /// "Date posted": `sort_by=creation_time`.
    Created,
    /// With a `name` search: `sort_by=relevance`.
    Relevance,
}

/// A new forum/media post.
#[derive(Debug, Clone, Default)]
pub struct ForumPost {
    /// Title, 1-100 chars.
    pub name: String,
    /// First message (text, files or both).
    pub content: String,
    /// ≤ 5 ids from the forum's `available_tags` (moderated ones need MANAGE_THREADS).
    pub applied_tags: Vec<Id>,
    /// ≤ `api::upload::MAX_FILES`, pre-checked against `AppState::upload_limit(forum)`.
    pub files: Vec<UploadFile>,
    /// 60/1440/4320/10080; the dialog preselects the forum's
    /// `default_auto_archive_duration.unwrap_or(4320)` (the server does not copy it).
    pub auto_archive_minutes: u32,
}

/// Inbox mention filters; echoed in `MentionsLoaded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MentionFilter {
    /// "This server only".
    pub guild_id: Option<Id>,
    /// Include @role mentions.
    pub roles: bool,
    /// Include @everyone / @here.
    pub everyone: bool,
}

impl Default for MentionFilter {
    fn default() -> Self {
        Self { guild_id: None, roles: true, everyone: true }
    }
}

/// UI location sent as `X-Context-Properties` with relationship calls (FEATURES_NOTES §0.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, reason = "chosen by the social UI, which is not built yet")]
pub enum ContextLocation {
    /// Friends list (Pending / All / Blocked rows).
    Friends,
    /// A user profile popout or modal.
    UserProfile,
    /// A right-click menu.
    ContextMenu,
    /// Inside a DM (header / message request bar).
    DmChannel,
}

/// Whose friend request `FriendResult` answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FriendTarget {
    /// `SendFriendRequest` (as typed).
    Username(String),
    /// `AcceptFriend`.
    User(Id),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FriendReply {
    Done,
    /// 80013: a request from someone without shared context; ask the user, then send
    /// `AcceptFriend { confirm_stranger: true }`.
    ConfirmStranger,
}

/// Answers to `SocialCommand`s (inside `BackendEvent::Social`). Errors are user-facing text.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // a few per user action
pub enum SocialEvent {
    ThreadsLoaded { channel_id: Id, list: ThreadList, result: Result<ThreadPage, String> },
    /// A DM / group / thread / forum post the user asked to create or open; Err is the full
    /// toast text. The state adds the channel and opens it.
    ChannelOpened(Result<Channel, String>),
    FriendResult { target: FriendTarget, result: Result<FriendReply, String> },
    MentionsLoaded { filter: MentionFilter, before: Option<Id>, limit: u8, result: Result<Vec<Message>, String> },
    ProfileLoaded { user_id: Id, guild_id: Option<Id>, result: Result<Box<UserProfile>, String> },
    CollectiblesLoaded(Result<Vec<crate::api::collectibles::CatalogItem>, String>),
    OwnedCollectiblesLoaded(Result<Vec<crate::api::collectibles::CatalogItem>, String>),
    ProductLoaded { sku_id: Id, result: Result<crate::model::CollectibleProduct, String> },
    GamesLoaded { ids: Vec<Id>, result: Result<Vec<crate::model::Game>, String> },
    /// My connections (also after an update / removal).
    ConnectionsLoaded(Result<Vec<crate::model::ConnectedAccount>, String>),
    /// The trigger messages of the asked requests (those that have one).
    RequestPreviewsLoaded { channel_ids: Vec<Id>, result: Result<Vec<Message>, String> },
    /// Accept / ignore answered; Err is the bare error (the state adds the toast prefix).
    MessageRequestsDone { channel_ids: Vec<Id>, accepted: bool, result: Result<(), String> },
}

/// Account, settings and server-administration requests (FEATURES_NOTES Part 3). Each is one
/// user action; destructive ones (leave, delete channel/role/emoji/invite, kick, ban) are sent
/// only after the UI's confirmation. Writes answer `AccountEvent::Done` keyed by `op()`; what
/// changed arrives through the gateway (USER_UPDATE, GUILD_*, CHANNEL_*, GUILD_ROLE_*,
/// GUILD_BAN_*, GUILD_EMOJIS_UPDATE, USER_SETTINGS_PROTO_UPDATE, USER_GUILD_SETTINGS_UPDATE).
/// Never retried on 4xx; a captcha answer becomes "do it in the official app", 401/60003
/// (`ApiError::MfaRequired`) an error without logout. Images are files the user picked,
/// size-checked and sent as data URIs (`api::account::image_data_uri`). Routes and bodies are
/// in the `api/account.rs` / `api/guild_admin.rs` doc comments.
#[derive(Debug)]
#[allow(dead_code, reason = "sent by the account/server UI, which is not built yet")]
pub enum AccountCommand {
    // ----- settings proto (§3.1, §3.9). GET settings-proto/1, edit one top-level field
    // (unknown subfields kept), PATCH only that field; `out_of_date` -> one re-GET + re-PATCH.
    /// Saved status (field 11.1): the PATCH only; the UI sends `ChatCommand::SetStatus` (op 3) first.
    SetPersistentStatus { status: UserStatus },
    /// Custom status (field 11.2): text ≤ 128 chars and/or emoji; both None clears it.
    /// `expires_at_ms` unix ms, None = never. The op 3 follows the USER_SETTINGS_UPDATE echo
    /// (the gateway re-sends presence when the saved custom status changes).
    SetCustomStatus { text: Option<String>, emoji: Option<Emoji>, expires_at_ms: Option<u64> },
    /// Server bar order and folders (field 14.1): the **whole** list, from
    /// `AppState::move_guild` or an edited copy of `account.settings.folders`. Debounce drags
    /// 1-2 s in the UI.
    SetGuildFolders { folders: Vec<GuildFolder> },
    /// Any other synced option (Settings pages, docs/SETTINGS_NOTES.md): these (path, value)
    /// pairs inside top-level field `top`, nothing else. USER_SETTINGS_PROTO_UPDATE echoes it.
    SetSetting { top: u32, values: Vec<(Vec<u32>, crate::api::settings_proto::SettingValue)> },
    // ----- Settings lists (docs/SETTINGS_NOTES.md §1); each opening of the page fetches once.
    /// `GET /auth/sessions` -> `SessionsLoaded` (Devices).
    FetchSessions,
    /// `POST /auth/sessions/logout {session_id_hashes}` (1-64; confirm first), then the list.
    LogoutSessions { id_hashes: Vec<String> },
    /// `GET /oauth2/tokens` -> `AuthorizedAppsLoaded`.
    FetchAuthorizedApps,
    /// `DELETE /oauth2/tokens/{id}` (confirm first), then the list.
    RevokeAuthorizedApp { token_id: Id },
    /// `GET /users/@me/consent` -> `ConsentsLoaded` (Data & Privacy).
    FetchConsents,
    /// `GET /safety-hub/@me` -> `StandingLoaded` (My Account > Account Standing, read-only).
    FetchStanding,
    /// `POST /users/@me/consent {grant, revoke}` ("usage_statistics", "personalization"), then
    /// the consents.
    SetConsents { grant: Vec<String>, revoke: Vec<String> },
    // ----- own account and profile (§3.2). USER_UPDATE refreshes `AppState::me`.
    /// `PATCH /users/@me {"global_name"}` (1-32 chars; None = clear).
    UpdateGlobalName { name: Option<String> },
    /// `PATCH /users/@me {"avatar"}`: a picked image file, None = remove.
    UpdateAvatar { path: Option<PathBuf> },
    /// `PATCH /users/@me/profile {"banner"}` (Nitro): a picked image file, None = remove.
    UpdateBanner { path: Option<PathBuf> },
    /// `PATCH /users/@me/profile {"bio"}` (≤ 190 chars, `""` clears).
    UpdateBio { bio: String },
    /// `PATCH /users/@me/profile {"pronouns"}` (≤ 40 chars, `""` clears).
    UpdatePronouns { pronouns: String },
    /// `PATCH /users/@me/profile {"accent_color"}` (0xRRGGBB, None = clear).
    UpdateAccentColor { color: Option<u32> },
    /// `PATCH /users/@me {"username","password"}` (needs the current password; may need the
    /// 2FA re-check: `AccountEvent::Mfa`).
    UpdateUsername { username: String, password: Secret },
    // ----- Password and Authentication (SETTINGS_NOTES §1 Account, FEATURES_NOTES §0.2, §3.2).
    // A rotated token replaces the session's everywhere: REST, the gateway (re-identify) and
    // the keyring only if this login is remembered; never a file. Commands marked "2FA" may
    // first answer `AccountEvent::Mfa(Some(prompt))`: the backend keeps the command (and the
    // ticket) until `FinishMfa` (then retries it once) or `CancelMfa`.
    /// `PATCH /users/@me {"password","new_password"}` (new: 8-72 chars). 2FA.
    ChangePassword { password: Secret, new_password: Secret },
    /// Authenticator step 1: is the password right (60005)? -> `Done`.
    CheckTotpPassword { password: Secret },
    /// Authenticator step 2 with the secret shown as QR code and the app's code ->
    /// `BackupCodes` (to show once).
    EnableTotp { password: Secret, secret: Secret, code: Secret },
    /// Remove the authenticator app (confirm first). 2FA.
    DisableTotp,
    /// SMS backup on / off (needs the authenticator app and a verified phone). 2FA.
    SetSmsMfa { enable: bool, password: Secret },
    /// `GET …/mfa/webauthn/credentials` -> `SecurityKeysLoaded`.
    FetchSecurityKeys,
    /// Rename a security key, then the list.
    RenameSecurityKey { id: Id, name: String },
    /// Delete a security key (confirm first), then the list. 2FA.
    DeleteSecurityKey { id: Id },
    /// View Backup Codes step 1 (password) -> `CodesNonce`; Discord emails a key.
    BackupCodesChallenge { password: Secret },
    /// Step 2: the emailed key and step 1's nonce (`regenerate_nonce` for new codes) ->
    /// `BackupCodes`.
    ViewBackupCodes { key: Secret, nonce: Secret, regenerate: bool },
    /// Pass the open 2FA prompt with this code (or password), then retry its action once.
    FinishMfa { kind: crate::api::account::MfaKind, code: Secret },
    /// Text the prompt's SMS code -> `Mfa(Some)` with `sms_phone`.
    MfaSms,
    /// Close the prompt: its action and ticket are dropped.
    CancelMfa,
    /// Equip / unequip a collectible, display-name style or profile theme for real
    /// (`CosmeticsPatch`; only owned items, name style / theme only with Nitro: the editor
    /// checks). Only on the editor's "Apply to account" / "Remove" click.
    UpdateCosmetics { patch: crate::api::collectibles::CosmeticsPatch },
    // ----- guilds (§3.3)
    /// `POST /guilds` (name 2-100, optional icon file) -> `Done(Some(guild id))`; the state
    /// opens the guild when its GUILD_CREATE arrives.
    CreateGuild { name: String, icon: Option<PathBuf> },
    /// `DELETE /users/@me/guilds/{g} {"lurking":false}` (not for the owner). GUILD_DELETE follows.
    LeaveGuild { guild_id: Id },
    /// `PATCH /guilds/{g}` with the set fields (MANAGE_GUILD). GUILD_UPDATE follows.
    UpdateGuild { guild_id: Id, patch: GuildPatch },
    // ----- channels (§3.4, MANAGE_CHANNELS)
    /// `POST /guilds/{g}/channels {"name","type","parent_id"}`; `kind` 0/2/4/5/13/15 ->
    /// `Done(Some(channel id))`; the state opens it (text-like kinds) once known.
    CreateChannel { guild_id: Id, name: String, kind: u8, parent_id: Option<Id> },
    /// `PATCH /channels/{c}` with the set fields. CHANNEL_UPDATE follows.
    UpdateChannel { channel_id: Id, patch: ChannelPatch },
    /// `DELETE /channels/{c}` (confirm first). CHANNEL_DELETE follows.
    DeleteChannel { channel_id: Id },
    /// `PATCH /guilds/{g}/channels [...]`: the whole affected sorting bucket, contiguous positions.
    ReorderChannels { guild_id: Id, positions: Vec<ChannelPosition> },
    /// `PUT /channels/{c}/permissions/{overwrite.id}` (MANAGE_ROLES; only bits I have).
    SetPermissionOverwrite { channel_id: Id, overwrite: Overwrite },
    /// `DELETE /channels/{c}/permissions/{target}`.
    DeletePermissionOverwrite { channel_id: Id, target_id: Id },
    // ----- roles (§3.5, MANAGE_ROLES; only roles below my highest role)
    /// `POST /guilds/{g}/roles` -> `Done(Some(role id))`. GUILD_ROLE_CREATE follows.
    CreateRole { guild_id: Id, role: RolePatch },
    /// `PATCH /guilds/{g}/roles/{r}` with the set fields.
    UpdateRole { guild_id: Id, role_id: Id, patch: RolePatch },
    /// `DELETE /guilds/{g}/roles/{r}` (confirm first).
    DeleteRole { guild_id: Id, role_id: Id },
    /// `PATCH /guilds/{g}/roles [{"id","position"}]` (@everyone stays 0).
    ReorderRoles { guild_id: Id, positions: Vec<(Id, i32)> },
    /// `PUT /guilds/{g}/members/{u}/roles/{r}`.
    AddMemberRole { guild_id: Id, user_id: Id, role_id: Id },
    /// `DELETE /guilds/{g}/members/{u}/roles/{r}`.
    RemoveMemberRole { guild_id: Id, user_id: Id, role_id: Id },
    // ----- members and moderation (§3.6). `reason` -> `X-Audit-Log-Reason`.
    /// `PATCH /guilds/{g}/members/@me {"nick"}` (CHANGE_NICKNAME; None/"" = reset).
    SetMyNick { guild_id: Id, nick: Option<String> },
    /// `PATCH /guilds/{g}/members/{u} {"nick"}` (MANAGE_NICKNAMES).
    SetNick { guild_id: Id, user_id: Id, nick: Option<String> },
    /// `DELETE /guilds/{g}/members/{u}` (KICK_MEMBERS; confirm first).
    KickMember { guild_id: Id, user_id: Id, reason: Option<String> },
    /// `PUT /guilds/{g}/bans/{u} {"delete_message_seconds"}` (0..=604800; BAN_MEMBERS; confirm
    /// first). GUILD_BAN_ADD follows.
    BanMember { guild_id: Id, user_id: Id, reason: Option<String>, delete_message_seconds: u32 },
    /// `DELETE /guilds/{g}/bans/{u}`. GUILD_BAN_REMOVE follows.
    Unban { guild_id: Id, user_id: Id },
    /// `GET /guilds/{g}/bans?limit=1000[&after=<user id>]` -> `BansLoaded`.
    FetchBans { guild_id: Id, after: Option<Id> },
    // ----- emoji (§3.7, CREATE/MANAGE_GUILD_EXPRESSIONS)
    /// `POST /guilds/{g}/emojis {"name","image"}` (name `[A-Za-z0-9_]{2,32}`, ≤ 256 KiB after
    /// downscaling). GUILD_EMOJIS_UPDATE follows.
    UploadEmoji { guild_id: Id, name: String, path: PathBuf },
    /// `DELETE /guilds/{g}/emojis/{e}` (confirm first).
    DeleteEmoji { guild_id: Id, emoji_id: Id },
    // ----- invites (§3.8). INVITE_* dispatches never reach users: lists come from REST.
    /// `POST /channels/{c}/invites` (`max_age` s 0..=5184000, 0 = never; `max_uses` 0..=100,
    /// 0 = unlimited) -> `InviteCreated` (`Ok(None)` = invites paused for the guild).
    CreateInvite { channel_id: Id, max_age: u32, max_uses: u32, temporary: bool },
    /// `GET /invites/{code}?with_counts=true&with_expiration=true` -> `InviteResolved`. Only for
    /// invites on screen, gated by `AppState::request_invite`.
    ResolveInvite { code: String },
    /// `POST /invites/{code}` after the user confirmed -> `Done(Some(guild id))` (opened once
    /// GUILD_CREATE arrives). Captcha -> "join in the official app" text with the invite link.
    /// The ids come from the resolved preview (`X-Context-Properties`); None when unknown.
    AcceptInvite { code: String, guild_id: Option<Id>, channel_id: Option<Id>, channel_type: Option<u8> },
    /// `DELETE /invites/{code}` (confirm first).
    DeleteInvite { code: String },
    /// `GET /guilds/{g}/invites` (MANAGE_GUILD) -> `InvitesLoaded`.
    FetchGuildInvites { guild_id: Id },
    // ----- notification settings (§3.10). USER_GUILD_SETTINGS_UPDATE follows.
    /// `PATCH /users/@me/guilds/{g}/settings` (`None` = DMs: `/users/@me/guilds/@me/settings`).
    UpdateGuildNotifications { guild_id: Option<Id>, patch: NotificationPatch },
    /// Same route with `{"channel_overrides":{"<c>":{…}}}`; `guild_id` = the channel's guild
    /// (None for a DM). `suppress_*` are ignored here.
    UpdateChannelOverride { guild_id: Option<Id>, channel_id: Id, patch: NotificationPatch },
    // ----- server tag card (docs/GUILD_CARD_NOTES.md), each on the user's click.
    /// `GET /guilds/{g}/profile` -> `GuildProfileLoaded`. Gated by
    /// `AppState::request_guild_profile`.
    FetchGuildProfile { guild_id: Id },
    /// `PUT /guilds/{g}/members/@me?lurker=false` (discoverable servers) -> `Done(Some(guild))`
    /// (opened once GUILD_CREATE arrives). Captcha -> "join in the official app" text.
    JoinGuild { guild_id: Id },
    /// `GET /guilds/{g}/member-verification` -> `VerificationLoaded`.
    FetchMemberVerification { guild_id: Id },
    /// `PUT /guilds/{g}/requests/@me` with every form field and its `response` -> `Done(Some(g))`
    /// when approved at once (rules only: I am in), `Done(None)` when it waits for review.
    SubmitJoinRequest { guild_id: Id, version: Option<String>, form_fields: Vec<serde_json::Value> },
}

/// Fields of `AccountCommand::UpdateGuild`; None = unchanged.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GuildPatch {
    /// 2-100 chars.
    pub name: Option<String>,
    /// `Some(None)` removes the icon.
    pub icon: Option<Option<PathBuf>>,
    /// ≤ 300 chars (community guilds).
    pub description: Option<Option<String>>,
    pub system_channel_id: Option<Option<Id>>,
    pub afk_channel_id: Option<Option<Id>>,
    /// Seconds: 60, 300, 900, 1800, 3600.
    pub afk_timeout: Option<u32>,
    /// 0 all messages, 1 only mentions.
    pub default_message_notifications: Option<u8>,
    /// 0-4.
    pub verification_level: Option<u8>,
    /// 0-2.
    pub explicit_content_filter: Option<u8>,
}

/// Fields of `AccountCommand::UpdateChannel`; None = unchanged.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChannelPatch {
    pub name: Option<String>,
    /// ≤ 1024 chars (forums 4096); `""` clears.
    pub topic: Option<String>,
    pub nsfw: Option<bool>,
    /// Slowmode seconds 0..=21600.
    pub slowmode: Option<u32>,
    /// `Some(None)` = out of its category.
    pub parent_id: Option<Option<Id>>,
    pub position: Option<i32>,
}

/// One entry of `AccountCommand::ReorderChannels`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelPosition {
    pub id: Id,
    pub position: i32,
    /// None = not sent, `Some(None)` = top level.
    pub parent_id: Option<Option<Id>>,
    /// "Sync permissions" with the new category.
    pub lock_permissions: bool,
}

/// Fields of a role create/update; None = unchanged / server default.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RolePatch {
    pub name: Option<String>,
    pub permissions: Option<u64>,
    /// 0xRRGGBB, 0 = none (sent as `colors.primary_color`).
    pub color: Option<u32>,
    pub hoist: Option<bool>,
    pub mentionable: Option<bool>,
}

/// Notification-settings change (§3.10); None = unchanged.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NotificationPatch {
    /// Some(true) mutes (for `mute_secs`), Some(false) unmutes.
    pub muted: Option<bool>,
    /// With `muted: Some(true)`: 900, 3600, 10800, 28800, 86400 …; None = until unmuted.
    pub mute_secs: Option<u32>,
    /// 0 all, 1 only mentions, 2 nothing; 3 = inherit (channel overrides only).
    pub level: Option<u8>,
    pub suppress_everyone: Option<bool>,
    pub suppress_roles: Option<bool>,
}

/// Which action an `AccountEvent::Done` answers: the command kind plus its main target (see
/// `AccountCommand::op`). The UI keeps it to show progress (`AccountState::begin`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AccountOp {
    pub kind: OpKind,
    /// The guild / channel / role / member / emoji the command is about; None for my own
    /// settings and profile, invite codes and DM notification settings.
    pub target: Option<Id>,
}

/// One per `AccountCommand` variant, same names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpKind {
    SetPersistentStatus,
    SetCustomStatus,
    SetGuildFolders,
    /// Target: the top-level field number.
    SetSetting,
    FetchSessions,
    LogoutSessions,
    FetchAuthorizedApps,
    RevokeAuthorizedApp,
    FetchConsents,
    FetchStanding,
    SetConsents,
    UpdateGlobalName,
    UpdateAvatar,
    UpdateBanner,
    UpdateBio,
    UpdatePronouns,
    UpdateAccentColor,
    UpdateUsername,
    ChangePassword,
    CheckTotpPassword,
    EnableTotp,
    DisableTotp,
    SetSmsMfa,
    FetchSecurityKeys,
    /// Target: the key.
    RenameSecurityKey,
    /// Target: the key.
    DeleteSecurityKey,
    BackupCodesChallenge,
    ViewBackupCodes,
    FinishMfa,
    MfaSms,
    CancelMfa,
    UpdateCosmetics,
    CreateGuild,
    LeaveGuild,
    UpdateGuild,
    CreateChannel,
    UpdateChannel,
    DeleteChannel,
    ReorderChannels,
    SetPermissionOverwrite,
    DeletePermissionOverwrite,
    CreateRole,
    UpdateRole,
    DeleteRole,
    ReorderRoles,
    AddMemberRole,
    RemoveMemberRole,
    SetMyNick,
    SetNick,
    KickMember,
    BanMember,
    Unban,
    FetchBans,
    UploadEmoji,
    DeleteEmoji,
    CreateInvite,
    ResolveInvite,
    AcceptInvite,
    DeleteInvite,
    FetchGuildInvites,
    UpdateGuildNotifications,
    UpdateChannelOverride,
    FetchGuildProfile,
    JoinGuild,
    FetchMemberVerification,
    SubmitJoinRequest,
}

impl AccountCommand {
    /// The key its `AccountEvent::Done` carries.
    pub fn op(&self) -> AccountOp {
        use AccountCommand as C;
        let (kind, target) = match self {
            C::SetPersistentStatus { .. } => (OpKind::SetPersistentStatus, None),
            C::SetCustomStatus { .. } => (OpKind::SetCustomStatus, None),
            C::SetGuildFolders { .. } => (OpKind::SetGuildFolders, None),
            C::SetSetting { top, .. } => (OpKind::SetSetting, Some(Id(u64::from(*top)))),
            C::FetchSessions => (OpKind::FetchSessions, None),
            C::LogoutSessions { .. } => (OpKind::LogoutSessions, None),
            C::FetchAuthorizedApps => (OpKind::FetchAuthorizedApps, None),
            C::RevokeAuthorizedApp { token_id } => (OpKind::RevokeAuthorizedApp, Some(*token_id)),
            C::FetchConsents => (OpKind::FetchConsents, None),
            C::FetchStanding => (OpKind::FetchStanding, None),
            C::SetConsents { .. } => (OpKind::SetConsents, None),
            C::UpdateGlobalName { .. } => (OpKind::UpdateGlobalName, None),
            C::UpdateAvatar { .. } => (OpKind::UpdateAvatar, None),
            C::UpdateBanner { .. } => (OpKind::UpdateBanner, None),
            C::UpdateBio { .. } => (OpKind::UpdateBio, None),
            C::UpdatePronouns { .. } => (OpKind::UpdatePronouns, None),
            C::UpdateAccentColor { .. } => (OpKind::UpdateAccentColor, None),
            C::UpdateUsername { .. } => (OpKind::UpdateUsername, None),
            C::ChangePassword { .. } => (OpKind::ChangePassword, None),
            C::CheckTotpPassword { .. } => (OpKind::CheckTotpPassword, None),
            C::EnableTotp { .. } => (OpKind::EnableTotp, None),
            C::DisableTotp => (OpKind::DisableTotp, None),
            C::SetSmsMfa { .. } => (OpKind::SetSmsMfa, None),
            C::FetchSecurityKeys => (OpKind::FetchSecurityKeys, None),
            C::RenameSecurityKey { id, .. } => (OpKind::RenameSecurityKey, Some(*id)),
            C::DeleteSecurityKey { id } => (OpKind::DeleteSecurityKey, Some(*id)),
            C::BackupCodesChallenge { .. } => (OpKind::BackupCodesChallenge, None),
            C::ViewBackupCodes { .. } => (OpKind::ViewBackupCodes, None),
            C::FinishMfa { .. } => (OpKind::FinishMfa, None),
            C::MfaSms => (OpKind::MfaSms, None),
            C::CancelMfa => (OpKind::CancelMfa, None),
            C::UpdateCosmetics { .. } => (OpKind::UpdateCosmetics, None),
            C::CreateGuild { .. } => (OpKind::CreateGuild, None),
            C::LeaveGuild { guild_id } => (OpKind::LeaveGuild, Some(*guild_id)),
            C::UpdateGuild { guild_id, .. } => (OpKind::UpdateGuild, Some(*guild_id)),
            C::CreateChannel { guild_id, .. } => (OpKind::CreateChannel, Some(*guild_id)),
            C::UpdateChannel { channel_id, .. } => (OpKind::UpdateChannel, Some(*channel_id)),
            C::DeleteChannel { channel_id } => (OpKind::DeleteChannel, Some(*channel_id)),
            C::ReorderChannels { guild_id, .. } => (OpKind::ReorderChannels, Some(*guild_id)),
            C::SetPermissionOverwrite { channel_id, .. } => (OpKind::SetPermissionOverwrite, Some(*channel_id)),
            C::DeletePermissionOverwrite { channel_id, .. } => (OpKind::DeletePermissionOverwrite, Some(*channel_id)),
            C::CreateRole { guild_id, .. } => (OpKind::CreateRole, Some(*guild_id)),
            C::UpdateRole { role_id, .. } => (OpKind::UpdateRole, Some(*role_id)),
            C::DeleteRole { role_id, .. } => (OpKind::DeleteRole, Some(*role_id)),
            C::ReorderRoles { guild_id, .. } => (OpKind::ReorderRoles, Some(*guild_id)),
            C::AddMemberRole { user_id, .. } => (OpKind::AddMemberRole, Some(*user_id)),
            C::RemoveMemberRole { user_id, .. } => (OpKind::RemoveMemberRole, Some(*user_id)),
            C::SetMyNick { guild_id, .. } => (OpKind::SetMyNick, Some(*guild_id)),
            C::SetNick { user_id, .. } => (OpKind::SetNick, Some(*user_id)),
            C::KickMember { user_id, .. } => (OpKind::KickMember, Some(*user_id)),
            C::BanMember { user_id, .. } => (OpKind::BanMember, Some(*user_id)),
            C::Unban { user_id, .. } => (OpKind::Unban, Some(*user_id)),
            C::FetchBans { guild_id, .. } => (OpKind::FetchBans, Some(*guild_id)),
            C::UploadEmoji { guild_id, .. } => (OpKind::UploadEmoji, Some(*guild_id)),
            C::DeleteEmoji { emoji_id, .. } => (OpKind::DeleteEmoji, Some(*emoji_id)),
            C::CreateInvite { channel_id, .. } => (OpKind::CreateInvite, Some(*channel_id)),
            C::ResolveInvite { .. } => (OpKind::ResolveInvite, None),
            C::AcceptInvite { .. } => (OpKind::AcceptInvite, None),
            C::DeleteInvite { .. } => (OpKind::DeleteInvite, None),
            C::FetchGuildInvites { guild_id } => (OpKind::FetchGuildInvites, Some(*guild_id)),
            C::UpdateGuildNotifications { guild_id, .. } => (OpKind::UpdateGuildNotifications, *guild_id),
            C::UpdateChannelOverride { channel_id, .. } => (OpKind::UpdateChannelOverride, Some(*channel_id)),
            C::FetchGuildProfile { guild_id } => (OpKind::FetchGuildProfile, Some(*guild_id)),
            C::JoinGuild { guild_id } => (OpKind::JoinGuild, Some(*guild_id)),
            C::FetchMemberVerification { guild_id } => (OpKind::FetchMemberVerification, Some(*guild_id)),
            C::SubmitJoinRequest { guild_id, .. } => (OpKind::SubmitJoinRequest, Some(*guild_id)),
        };
        AccountOp { kind, target }
    }

    /// A copy to retry after the 2FA re-check, for the actions Discord may gate (FEATURES_NOTES
    /// §0.2); None = a 60003 answer stays an error.
    fn mfa_retry(&self) -> Option<AccountCommand> {
        use AccountCommand as C;
        Some(match self {
            C::LogoutSessions { id_hashes } => C::LogoutSessions { id_hashes: id_hashes.clone() },
            C::RevokeAuthorizedApp { token_id } => C::RevokeAuthorizedApp { token_id: *token_id },
            C::UpdateUsername { username, password } => C::UpdateUsername { username: username.clone(), password: password.clone() },
            C::ChangePassword { password, new_password } => C::ChangePassword { password: password.clone(), new_password: new_password.clone() },
            C::DisableTotp => C::DisableTotp,
            C::SetSmsMfa { enable, password } => C::SetSmsMfa { enable: *enable, password: password.clone() },
            C::DeleteSecurityKey { id } => C::DeleteSecurityKey { id: *id },
            _ => return None,
        })
    }
}

/// The 2FA re-check a gated action waits for (`AccountEvent::Mfa`).
#[derive(Debug, Clone, PartialEq)]
pub struct MfaPrompt {
    /// The waiting action (its progress entry stays `Pending`).
    pub op: AccountOp,
    /// Ways the ticket can be passed, in Discord's order.
    pub methods: Vec<crate::api::account::MfaKind>,
    /// Masked phone the SMS code went to (after `MfaSms`).
    pub sms_phone: Option<String>,
    /// The last code was wrong, or the SMS could not be sent.
    pub error: Option<String>,
    /// UI: `FinishMfa` / `MfaSms` sent, waiting (a new prompt from the backend clears it).
    pub busy: bool,
}

/// Answers to `AccountCommand`s (inside `BackendEvent::Account`). Errors are full user-facing
/// text ("Could not …: reason").
#[derive(Debug)]
pub enum AccountEvent {
    /// Every write. `Ok(Some(id))`: the created guild / channel / role, or the guild an
    /// accepted invite joined; `Ok(None)` otherwise.
    Done { op: AccountOp, result: Result<Option<Id>, String> },
    /// `Ok(None)` = invites are paused for this guild (HTTP 204).
    InviteCreated { channel_id: Id, result: Result<Option<Invite>, String> },
    InviteResolved { code: String, result: Result<Invite, String> },
    /// One page (≤ 1000, by user id); `after` echoes the request.
    BansLoaded { guild_id: Id, after: Option<Id>, result: Result<Vec<Ban>, String> },
    InvitesLoaded { guild_id: Id, result: Result<Vec<Invite>, String> },
    /// Devices (after a fetch or a logout).
    SessionsLoaded(Result<Vec<crate::model::AuthSession>, String>),
    /// Authorized Apps (after a fetch or a revoke).
    AuthorizedAppsLoaded(Result<Vec<crate::model::AuthorizedApp>, String>),
    /// Data & Privacy consents (after a fetch or a change).
    ConsentsLoaded(Result<crate::model::Consents, String>),
    /// Account Standing: `account_standing.state` (100 all good, 200 limited, 300 very
    /// limited, 400 at risk, 500 suspended).
    StandingLoaded(Result<u16, String>),
    /// Security keys (after a fetch, rename or delete).
    SecurityKeysLoaded(Result<Vec<crate::api::account::SecurityKey>, String>),
    /// Backup codes to show once (`EnableTotp`, `ViewBackupCodes`); `op` as in `Done`.
    BackupCodes { op: AccountOp, result: Result<Vec<crate::api::account::BackupCode>, String> },
    /// View Backup Codes step 1: the nonces for step 2.
    CodesNonce { op: AccountOp, result: Result<crate::api::account::CodesNonce, String> },
    /// Some: an action waits for the 2FA re-check (show / update the prompt); None: close it.
    Mfa(Option<MfaPrompt>),
    /// `FetchGuildProfile`; `Ok(None)` = the server keeps its profile private (403 / 404).
    GuildProfileLoaded { guild_id: Id, result: Result<Option<crate::model::GuildProfile>, String> },
    /// `FetchMemberVerification`.
    VerificationLoaded { guild_id: Id, result: Result<crate::model::MemberVerification, String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MfaMethod {
    /// Authenticator app code.
    Totp,
    Sms,
    /// One of the account's backup codes.
    Backup,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoginStep {
    /// Discord asked for a captcha: the login card shows hCaptcha's widget for this sitekey (and
    /// rqdata) and the login waits for the user.
    Captcha { sitekey: String, rqdata: Option<String> },
    /// Password accepted; a second factor is needed (methods the account allows).
    MfaRequired { methods: Vec<MfaMethod> },
    /// SMS code sent to this (masked) phone number.
    SmsSent { phone: String },
    /// Show this URL as a QR code (`https://discord.com/ra/<fingerprint>`).
    QrCode { url: String },
    /// The code was scanned; waiting for the confirmation on the phone.
    QrScanned { username: String, avatar_url: Option<String> },
    /// QR code expired or the phone cancelled; the UI offers a new code.
    QrExpired,
    /// The attempt failed (wrong password/code, captcha required, network…). User-facing text.
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnState {
    /// No token / logged out.
    Offline,
    Connecting,
    Connected,
    /// Waiting before the next reconnect attempt.
    Reconnecting { attempt: u32, in_secs: u32 },
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // a few events per frame; boxing buys nothing
pub enum BackendEvent {
    Conn(ConnState),
    /// Login validated; gateway is connecting.
    LoggedIn,
    /// Token rejected (401 / gateway close 4004). The UI shows the login screen.
    AuthFailed(String),
    /// Progress of an email/password or QR login (login screen only).
    LoginStep(LoginStep),
    Gateway(GatewayEvent),
    MessagesLoaded { channel_id: Id, before: Option<Id>, result: Result<Vec<Message>, String> },
    /// Result of SendMessage; on success the MESSAGE_CREATE also arrives via the gateway.
    SendResult { channel_id: Id, nonce: String, result: Result<Message, String> },
    /// Edit/delete/reaction/ack failures worth a toast.
    ActionFailed(String),
    ImageLoaded { url: String, result: Result<DecodedImage, String> },
    /// Voice connection status, speaking indicators, devices.
    Voice(crate::voice::VoiceEvent),
    /// Chat-essentials answers; the UI hands them to `AppState::apply_chat`.
    Chat(ChatEvent),
    /// Social answers; the UI hands them to `AppState::apply_social`.
    Social(SocialEvent),
    /// Account/server answers; the UI hands them to `AppState::apply_account`.
    Account(AccountEvent),
    /// The activity thread and its fetches; the UI hands them to `AppState::activity`.
    Activity(crate::activity::ActivityEvent),
    /// Quest answers; the UI hands them to `AppState::quests`.
    Quests(crate::quests::QuestEvent),
}

/// Decoded image ready for texture upload. Static images have exactly one frame.
#[derive(Debug)]
pub struct DecodedImage {
    pub frames: Vec<ImageFrame>,
    /// A streamed animation (profile effects, `cache::images::stream_apng`): `frames` holds the first frame, the rest
    /// arrive here in order, looping, decoded just ahead; dropping it stops the decoder.
    pub stream: Option<std::sync::mpsc::Receiver<ImageFrame>>,
}

#[derive(Debug)]
pub struct ImageFrame {
    pub image: egui::ColorImage,
    /// Display time for animated frames; 0 for static images.
    pub delay_ms: u32,
}

/// Backend -> UI channel that also wakes the UI.
#[derive(Clone)]
pub struct EventSink {
    tx: mpsc::Sender<BackendEvent>,
    ctx: egui::Context,
}

impl EventSink {
    pub fn new(tx: mpsc::Sender<BackendEvent>, ctx: egui::Context) -> Self {
        Self { tx, ctx }
    }

    /// Wakes the UI for one pass, shared by a burst; a plain `request_repaint` would cost two.
    /// The UI adds a settle pass itself when the event moved a widget (`ui::RustcordApp::ui`).
    pub fn send(&self, ev: BackendEvent) {
        self.send_after(ev, Duration::from_millis(1));
    }

    /// Like `send`, but the UI wakes within `delay`; egui keeps the earliest pending wake, so a
    /// burst of these costs one pass.
    pub fn send_after(&self, ev: BackendEvent, delay: Duration) {
        if self.tx.send(ev).is_ok() {
            self.ctx.request_repaint_after_for(delay, egui::ViewportId::ROOT);
        }
    }
}

/// UI -> backend handle. Cheap to clone.
#[derive(Clone)]
pub struct CommandSender(pub tmpsc::UnboundedSender<Command>);

impl CommandSender {
    pub fn send(&self, cmd: Command) {
        let _ = self.0.send(cmd);
    }
}

pub struct BackendHandle {
    pub cmd: CommandSender,
    pub events: mpsc::Receiver<BackendEvent>,
    /// Latest frame of each shown video (camera, Go Live, my previews); read per paint, see
    /// `VideoFrames::latest` and docs/VIDEO_DESIGN.md.
    pub video: Arc<crate::voice::video::VideoFrames>,
}

/// Starts the backend task on `rt`. Must be called once, from the eframe app creator.
pub fn spawn(rt: &tokio::runtime::Handle, ctx: egui::Context) -> BackendHandle {
    let (cmd_tx, cmd_rx) = tmpsc::unbounded_channel();
    let (ev_tx, ev_rx) = mpsc::channel();
    let video = Arc::new(crate::voice::video::VideoFrames::new(ctx.clone()));
    // In-app media (src/media): the same runtime, no new one.
    crate::media::net::init(rt.clone());
    let sink = EventSink::new(ev_tx, ctx);
    // Weak, so the loop still ends when the UI drops its sender.
    rt.spawn(run(cmd_rx, cmd_tx.downgrade(), sink, video.clone()));
    BackendHandle { cmd: CommandSender(cmd_tx), events: ev_rx, video }
}

// ---------------------------------------------------------------------------
// Backend loop (api implementer)
// ---------------------------------------------------------------------------

use crate::api::auth::{
    AuthClient, CAPTCHA_CANCELLED, CAPTCHA_REJECTED, CaptchaAnswer, CaptchaChallenge, EXPIRED, LoginError, LoginOutcome,
};
use crate::captcha::CaptchaReply;
use crate::gateway::remote_auth::RaKeys;
use crate::api::props::ClientProps;
use crate::api::{ApiClient, ApiError};
use crate::gateway::{GatewayCommand, GatewayHandle};
use crate::voice::VoiceManager;
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// Typing indicator resend interval (FEATURES_NOTES §0.1 #8: the remote side shows it 10 s).
pub(crate) const TYPING_RESEND: Duration = Duration::from_secs(8);

/// A logged-in account: REST client + gateway connection.
struct Session {
    api: Arc<ApiClient>,
    gateway: GatewayHandle,
    /// REST tasks of this session. Dropping the set aborts them, so nothing from a
    /// logged-out session reaches the UI.
    tasks: tokio::task::JoinSet<()>,
    /// Only the first 401 of a session is reported as AuthFailed.
    auth_reported: Arc<AtomicBool>,
    voice: VoiceManager,
    /// This login is in the keyring: a rotated token replaces it there (`TokenSwap`).
    remember: bool,
    /// The gated action waiting for its 2FA re-check (`FinishMfa` / `CancelMfa`).
    mfa: Arc<std::sync::Mutex<Option<PendingMfa>>>,
    /// Set by `handle_account` for the `spawn` of the command it handles.
    gate: Option<Gate>,
}

/// A gated action that got 60003: its ticket, the methods, and the command to retry.
struct PendingMfa {
    ticket: Secret,
    methods: Vec<crate::api::account::MfaKind>,
    op: AccountOp,
    cmd: AccountCommand,
    /// Tells this entry apart from a later one (the expiry only drops its own).
    at: Instant,
}

/// The command a `spawn` runs may be gated: its op, a copy to keep for the retry, and (when it
/// is that retry) the re-check to pass first.
struct Gate {
    op: AccountOp,
    retry: AccountCommand,
    finish: Option<MfaFinish>,
}

/// The re-check of a retry: ticket, the ticket's methods (to ask again), and the typed answer.
/// `Instant`: the pending entry's `at`, kept through a wrong-code retry so its TTL still ends it.
type MfaFinish = (Secret, Vec<crate::api::account::MfaKind>, crate::api::account::MfaKind, Secret, Instant);

/// A 60003 ticket lives about 5 minutes: the kept command (maybe holding passwords) goes then.
const MFA_TICKET_TTL: Duration = Duration::from_secs(300);

fn mfa_slot(m: &std::sync::Mutex<Option<PendingMfa>>) -> std::sync::MutexGuard<'_, Option<PendingMfa>> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn mfa_prompt(p: Option<MfaPrompt>) -> BackendEvent {
    BackendEvent::Account(AccountEvent::Mfa(p))
}

/// Where a rotated login token goes (FEATURES_NOTES §3.2): REST, the gateway, the keyring only
/// for a remembered login. Never a file, a log or an error text.
struct TokenSwap {
    gateway: tmpsc::UnboundedSender<GatewayCommand>,
    remember: bool,
    sink: EventSink,
}

impl TokenSwap {
    async fn apply(self, api: Arc<ApiClient>, token: Token) {
        let (gateway, remember) = (self.gateway, self.remember);
        let saved = tokio::task::spawn_blocking(move || swap_token(&api, &gateway, remember, token, crate::auth::save_token)).await;
        if let Some(e) = match saved {
            Ok(r) => r.err(),
            Err(_) => Some("credential store task failed".into()),
        } {
            self.sink.send(BackendEvent::ActionFailed(format!("Your login changed, but the new one could not be remembered ({e}).")));
        }
    }
}

/// Puts `token` in place of the session's: REST client, gateway (`SetToken` re-identifies),
/// and `save` (the keyring) only when `remember`. Returns `save`'s error.
fn swap_token(
    api: &ApiClient,
    gateway: &tmpsc::UnboundedSender<GatewayCommand>,
    remember: bool,
    token: Token,
    save: impl FnOnce(&Token) -> Result<(), String>,
) -> Result<(), String> {
    api.set_token(&token);
    // Gateway first: it holds the old token's 4004 only so long, and the keyring may block.
    let _ = gateway.send(GatewayCommand::SetToken(token.clone()));
    if remember { save(&token) } else { Ok(()) }
}

impl Session {
    fn start(api: Arc<ApiClient>, token: Token, remember: bool, props: &Arc<ClientProps>, sink: &EventSink, voice: &VoiceManager) -> Self {
        let gateway = crate::gateway::spawn(token, props.clone(), sink.clone(), voice.feed());
        voice.attach_gateway(Some(gateway.tx.clone()));
        let (tasks, auth_reported) = (tokio::task::JoinSet::new(), Arc::new(AtomicBool::new(false)));
        Self { api, gateway, tasks, auth_reported, voice: voice.clone(), remember, mfa: Default::default(), gate: None }
    }

    fn token_swap(&self, sink: &EventSink) -> TokenSwap {
        TokenSwap { gateway: self.gateway.tx.clone(), remember: self.remember, sink: sink.clone() }
    }

    /// Ask the gateway to close (ending any call); returns its task so a caller can wait for it.
    fn shutdown(self) -> tokio::task::JoinHandle<()> {
        self.voice.attach_gateway(None);
        self.gateway.send(GatewayCommand::Shutdown);
        self.gateway.task
    }

    /// Run one REST call in its own task (the rate limiter serializes per bucket).
    /// `report` turns the outcome into an event (None = nothing to tell the UI).
    /// 401s become AuthFailed instead. A gated command (`gate`) that gets 60003 is kept for
    /// the 2FA prompt instead of being reported; its retry passes the re-check first.
    fn spawn<T: Send + 'static>(
        &mut self,
        sink: &EventSink,
        call: impl Future<Output = Result<T, ApiError>> + Send + 'static,
        report: impl FnOnce(Result<T, String>) -> Option<BackendEvent> + Send + 'static,
    ) {
        let sink = sink.clone();
        let auth_reported = self.auth_reported.clone();
        let (api, slot, gate) = (self.api.clone(), self.mfa.clone(), self.gate.take());
        self.tasks.spawn(async move {
            let (outcome, gate) = match gate {
                Some(Gate { op, retry, finish: Some((ticket, methods, kind, code, at)) }) => match api.mfa_finish(&ticket, kind, &code).await {
                    Ok(jwt) => {
                        sink.send(mfa_prompt(None));
                        (crate::api::with_mfa(&jwt, call).await, None)
                    }
                    // Wrong code: the ticket still works, ask again.
                    Err((e, Some(c))) if crate::api::account::WRONG_MFA_CODE.contains(&c) => {
                        let prompt = MfaPrompt { op, methods: methods.clone(), sms_phone: None, error: Some(e.to_string()), busy: false };
                        *mfa_slot(&slot) = Some(PendingMfa { ticket, methods, op, cmd: retry, at });
                        sink.send(mfa_prompt(Some(prompt)));
                        return;
                    }
                    Err((e, _)) => {
                        sink.send(mfa_prompt(None));
                        (Err(e), None)
                    }
                },
                gate => (call.await, gate),
            };
            let result = match (outcome, gate) {
                (Ok(v), _) => Ok(v),
                (Err(ApiError::Unauthorized), _) => {
                    if !auth_reported.swap(true, Ordering::Relaxed) {
                        sink.send(BackendEvent::AuthFailed(ApiError::Unauthorized.to_string()));
                    }
                    return;
                }
                (Err(ApiError::MfaRequired(ch)), Some(Gate { op, retry, .. })) if !ch.methods.is_empty() => {
                    let at = Instant::now();
                    let prompt = MfaPrompt { op, methods: ch.methods.clone(), sms_phone: None, error: None, busy: false };
                    *mfa_slot(&slot) = Some(PendingMfa { ticket: ch.ticket, methods: ch.methods, op, cmd: retry, at });
                    sink.send(mfa_prompt(Some(prompt)));
                    // Not passed in time: the ticket is dead, so drop the command (and its passwords).
                    tokio::time::sleep(MFA_TICKET_TTL).await;
                    let expired = {
                        let mut s = mfa_slot(&slot);
                        let mine = s.as_ref().is_some_and(|p| p.at == at);
                        mine && s.take().is_some()
                    };
                    if !expired {
                        return;
                    }
                    sink.send(mfa_prompt(None));
                    Err("The two-factor check timed out. Try again.".to_owned())
                }
                (Err(e), _) => Err(e.to_string()),
            };
            if let Some(ev) = report(result) {
                sink.send(ev);
            }
        });
    }
}

fn failed(what: &'static str) -> impl FnOnce(Result<(), String>) -> Option<BackendEvent> {
    move |r| r.err().map(|e| BackendEvent::ActionFailed(format!("{what}: {e}")))
}

/// Runs a REST call in the session, or (logged out) answers `report` with "Not connected" at
/// once, so every request gets its answer event.
fn rest<T: Send + 'static, F>(
    session: &mut Option<Session>,
    sink: &EventSink,
    call: impl FnOnce(Arc<ApiClient>) -> F,
    report: impl FnOnce(Result<T, String>) -> Option<BackendEvent> + Send + 'static,
) where
    F: Future<Output = Result<T, ApiError>> + Send + 'static,
{
    match session {
        Some(s) => {
            let fut = call(s.api.clone());
            s.spawn(sink, fut, report);
        }
        None => {
            if let Some(ev) = report(Err("Not connected".into())) {
                sink.send(ev);
            }
        }
    }
}

/// At most one `POST /typing` per channel per `TYPING_RESEND`; a sent message resets it.
#[derive(Default)]
struct TypingThrottle {
    last: HashMap<Id, Instant>,
}

impl TypingThrottle {
    /// Records and returns true if a typing request may go out now.
    fn should_send(&mut self, channel_id: Id, now: Instant) -> bool {
        // Only entries inside the window matter, which keeps the map tiny.
        self.last.retain(|_, t| now.duration_since(*t) < TYPING_RESEND);
        if self.last.contains_key(&channel_id) {
            return false;
        }
        self.last.insert(channel_id, now);
        true
    }

    fn reset(&mut self, channel_id: Id) {
        self.last.remove(&channel_id);
    }
}

/// Backend-side chat state that outlives single requests.
#[derive(Default)]
struct ChatBackend {
    typing: TypingThrottle,
    /// Cancel switch per running upload (by nonce); closed ones are pruned on the next upload.
    uploads: HashMap<String, oneshot::Sender<()>>,
}

/// `Command::Chat`. REST calls go through `rest` (answers + 401 handling), gateway ops
/// through `GatewayCommand`. The bodies live in `api/chat.rs`, `api/upload.rs`, `gateway/`.
fn handle_chat(cmd: ChatCommand, session: &mut Option<Session>, sink: &EventSink, props: &ClientProps, chat: &mut ChatBackend) {
    use ChatCommand as C;
    let chat_ev = BackendEvent::Chat;
    match cmd {
        C::SendWithFiles { channel_id, content, nonce, reply, files } => {
            chat.typing.reset(channel_id);
            let (cancel_tx, cancel) = oneshot::channel();
            chat.uploads.retain(|_, tx| !tx.is_closed());
            chat.uploads.insert(nonce.clone(), cancel_tx);
            let progress = {
                let (sink, nonce) = (sink.clone(), nonce.clone());
                move |sent, total| sink.send(chat_ev(ChatEvent::UploadProgress { nonce: nonce.clone(), sent, total }))
            };
            let n = nonce.clone();
            rest(
                session,
                sink,
                move |api| async move {
                    crate::api::upload::send_with_files(&api, channel_id, &content, &n, reply.as_ref(), files, progress, cancel).await
                },
                // Ok(None) = cancelled: the UI already dropped the pending message.
                move |r| match r {
                    Ok(None) => None,
                    Ok(Some(m)) => Some(BackendEvent::SendResult { channel_id, nonce, result: Ok(m) }),
                    Err(e) => Some(BackendEvent::SendResult { channel_id, nonce, result: Err(e) }),
                },
            );
        }
        C::CancelUpload { nonce } => {
            if let Some(tx) = chat.uploads.remove(&nonce) {
                let _ = tx.send(());
            }
        }
        C::Typing { channel_id } => {
            if session.is_some() && chat.typing.should_send(channel_id, Instant::now()) {
                // Typing failures stay silent.
                rest(session, sink, move |api| async move { api.typing(channel_id).await }, move |r| {
                    r.ok().flatten().map(|ms| chat_ev(ChatEvent::SendCooldown { channel_id, ms }))
                });
            }
        }
        C::SetStatus { status, afk } => match session {
            Some(s) => s.gateway.send(GatewayCommand::UpdatePresence { status, afk }),
            None => sink.send(BackendEvent::ActionFailed("Not connected".into())),
        },
        C::FetchPins { channel_id, before } => {
            let b = before.clone();
            rest(session, sink, move |api| async move { api.pins(channel_id, b.as_deref()).await }, move |result| {
                Some(chat_ev(ChatEvent::PinsLoaded { channel_id, before, result }))
            });
        }
        C::PinMessage { channel_id, message_id } => {
            rest(session, sink, move |api| async move { api.pin(channel_id, message_id).await }, failed("Could not pin the message"));
        }
        C::UnpinMessage { channel_id, message_id } => rest(
            session,
            sink,
            move |api| async move { api.unpin(channel_id, message_id).await },
            failed("Could not unpin the message"),
        ),
        C::Search { scope, query } => {
            let q = query.clone();
            rest(session, sink, move |api| async move { api.search(scope, &q).await }, move |r| {
                let result = r.unwrap_or_else(SearchOutcome::Failed);
                Some(chat_ev(ChatEvent::SearchResults { scope, query, result }))
            });
        }
        // Autocomplete: nothing to answer while logged out.
        C::SearchMembers { guild_id, query, limit } => {
            if let Some(s) = session {
                s.gateway.send(GatewayCommand::RequestMembers { guild_id, query, limit });
            }
        }
        C::Gifs { query } => {
            let (q, locale) = (query.clone(), props.system_locale.clone());
            rest(session, sink, move |api| async move { api.gifs(q.as_deref(), &locale).await }, move |result| {
                Some(chat_ev(ChatEvent::GifResults { query, result }))
            });
        }
        C::MarkUnread { channel_id, message_id, mention_count } => rest(
            session,
            sink,
            move |api| async move { api.mark_unread(channel_id, message_id, mention_count).await },
            failed("Could not mark the message unread"),
        ),
        C::FetchAround { channel_id, message_id, limit } => {
            rest(session, sink, move |api| async move { api.messages_around(channel_id, message_id, limit).await }, move |result| {
                Some(chat_ev(ChatEvent::AroundLoaded { channel_id, message_id, result }))
            });
        }
        C::SuppressEmbeds { channel_id, message_id, flags } => rest(
            session,
            sink,
            move |api| async move { api.suppress_embeds(channel_id, message_id, flags).await },
            failed("Could not remove the embeds"),
        ),
        C::FetchReactionUsers { channel_id, message_id, emoji, burst, after } => {
            let e = emoji.clone();
            rest(
                session,
                sink,
                move |api| async move { api.reaction_users(channel_id, message_id, &e, burst, after).await },
                move |result| Some(chat_ev(ChatEvent::ReactionUsers { channel_id, message_id, emoji, after, result })),
            );
        }
        C::Forward { channel_id, guild_id, message_id, dest, nonce } => rest(
            session,
            sink,
            move |api| async move { api.forward(channel_id, guild_id, message_id, dest, &nonce).await },
            move |result| Some(chat_ev(ChatEvent::Forwarded { dest, result })),
        ),
        C::PollVote { channel_id, message_id, answer_ids, restore } => rest(
            session,
            sink,
            move |api| async move { api.poll_vote(channel_id, message_id, &answer_ids).await },
            move |r| r.err().map(|error| chat_ev(ChatEvent::PollVoteFailed { channel_id, message_id, restore, error })),
        ),
        C::FetchPollVoters { channel_id, message_id, answer_id, after } => rest(
            session,
            sink,
            move |api| async move { api.poll_voters(channel_id, message_id, answer_id, after).await },
            move |result| Some(chat_ev(ChatEvent::PollVoters { channel_id, message_id, answer_id, after, result })),
        ),
        C::SendPoll { channel_id, nonce, poll } => {
            chat.typing.reset(channel_id);
            rest(
                session,
                sink,
                move |api| async move { api.send_poll(channel_id, &nonce, &poll).await },
                move |result| Some(chat_ev(ChatEvent::PollPosted { channel_id, result })),
            );
        }
    }
}

/// Answer of a create/open call: the channel to open, or the full error text.
fn opened(what: &'static str) -> impl FnOnce(Result<Channel, String>) -> Option<BackendEvent> {
    move |r| Some(BackendEvent::Social(SocialEvent::ChannelOpened(r.map_err(|e| format!("{what}: {e}")))))
}

/// `Command::Social`: every request goes through `rest` (answers + 401 handling); the calls
/// live in `api/social.rs`.
fn handle_social(cmd: SocialCommand, session: &mut Option<Session>, sink: &EventSink) {
    use SocialCommand as C;
    let ev = BackendEvent::Social;
    match cmd {
        C::FetchThreads { channel_id, list } => {
            let l = list.clone();
            rest(session, sink, move |api| async move { api.threads(channel_id, &l).await }, move |result| {
                Some(ev(SocialEvent::ThreadsLoaded { channel_id, list, result }))
            });
        }
        C::JoinThread { thread_id } => {
            rest(session, sink, move |api| async move { api.join_thread(thread_id).await }, failed("Could not join the thread"));
        }
        C::LeaveThread { thread_id } => {
            rest(session, sink, move |api| async move { api.leave_thread(thread_id).await }, failed("Could not leave the thread"));
        }
        C::CreateThread { channel_id, name, from_message, private, parent_kind, auto_archive_minutes } => rest(
            session,
            sink,
            move |api| async move { api.create_thread(channel_id, &name, from_message, private, parent_kind, auto_archive_minutes).await },
            opened("Could not create the thread"),
        ),
        C::SetThreadArchived { thread_id, archived } => rest(
            session,
            sink,
            move |api| async move { api.set_thread_archived(thread_id, archived).await },
            failed(if archived { "Could not archive the thread" } else { "Could not unarchive the thread" }),
        ),
        C::CreateForumPost { channel_id, post } => rest(
            session,
            sink,
            move |api| async move { crate::api::threads::create_forum_post(&api, channel_id, post).await },
            opened("Could not create the post"),
        ),
        C::SendFriendRequest { username } => {
            let u = username.clone();
            rest(session, sink, move |api| async move { api.send_friend_request(&u).await }, move |result| {
                Some(ev(SocialEvent::FriendResult { target: FriendTarget::Username(username), result }))
            });
        }
        C::AcceptFriend { user_id, confirm_stranger, location } => rest(
            session,
            sink,
            move |api| async move { api.accept_friend(user_id, confirm_stranger, location).await },
            move |result| Some(ev(SocialEvent::FriendResult { target: FriendTarget::User(user_id), result })),
        ),
        C::RemoveRelationship { user_id, location } => rest(
            session,
            sink,
            move |api| async move { api.remove_relationship(user_id, location).await },
            failed("Could not update the relationship"),
        ),
        C::BlockUser { user_id, location } => {
            rest(session, sink, move |api| async move { api.block_user(user_id, location).await }, failed("Could not block the user"));
        }
        C::OpenDm { user_id } => {
            rest(session, sink, move |api| async move { api.open_dm(user_id).await }, opened("Could not open the conversation"));
        }
        C::CreateGroupDm { recipients } => {
            rest(session, sink, move |api| async move { api.create_group_dm(&recipients).await }, opened("Could not create the group"));
        }
        C::AddRecipient { channel_id, user_id } => {
            rest(session, sink, move |api| async move { api.add_recipient(channel_id, user_id).await }, move |r| match r {
                // A 1:1 DM became a new group.
                Ok(Some(ch)) => Some(ev(SocialEvent::ChannelOpened(Ok(ch)))),
                Ok(None) => None,
                Err(e) => Some(BackendEvent::ActionFailed(format!("Could not add the member: {e}"))),
            });
        }
        C::RemoveRecipient { channel_id, user_id } => rest(
            session,
            sink,
            move |api| async move { api.remove_recipient(channel_id, user_id).await },
            failed("Could not remove the member"),
        ),
        C::RenameGroup { channel_id, name } => {
            rest(session, sink, move |api| async move { api.rename_group(channel_id, &name).await }, failed("Could not rename the group"));
        }
        C::CloseDm { channel_id, silent } => rest(
            session,
            sink,
            move |api| async move { api.close_dm(channel_id, silent).await },
            failed("Could not close the conversation"),
        ),
        C::FetchMentions { filter, before, limit } => {
            rest(session, sink, move |api| async move { api.mentions(filter, before, limit).await }, move |result| {
                Some(ev(SocialEvent::MentionsLoaded { filter, before, limit, result }))
            });
        }
        C::DeleteMention { message_id } => {
            rest(session, sink, move |api| async move { api.delete_mention(message_id).await }, failed("Could not remove the mention"));
        }
        C::MarkAllRead { acks } => {
            rest(session, sink, move |api| async move { api.ack_bulk(&acks).await }, failed("Could not mark everything read"));
        }
        C::FetchProfile { user_id, guild_id } => rest(
            session,
            sink,
            move |api| async move {
                let mut p = api.profile(user_id, guild_id).await?;
                // The note is extra: its failure must not hide the profile.
                p.note = api.note(user_id).await.ok().flatten();
                Ok(Box::new(p))
            },
            move |result| Some(ev(SocialEvent::ProfileLoaded { user_id, guild_id, result })),
        ),
        C::SetNote { user_id, note } => {
            rest(session, sink, move |api| async move { api.set_note(user_id, &note).await }, failed("Could not save the note"));
        }
        C::FetchCollectibles => rest(
            session,
            sink,
            |api| async move { api.collectibles_catalog().await },
            move |result| Some(ev(SocialEvent::CollectiblesLoaded(result))),
        ),
        C::FetchOwnedCollectibles => rest(
            session,
            sink,
            |api| async move { api.owned_collectibles().await },
            move |result| Some(ev(SocialEvent::OwnedCollectiblesLoaded(result))),
        ),
        C::FetchProduct { sku_id } => rest(
            session,
            sink,
            move |api| async move { api.collectible_product(sku_id).await },
            move |result| Some(BackendEvent::Social(SocialEvent::ProductLoaded { sku_id, result })),
        ),
        C::FetchGames { ids } => {
            let asked = ids.clone();
            rest(session, sink, move |api| async move { api.games(&asked).await }, move |result| Some(BackendEvent::Social(SocialEvent::GamesLoaded { ids, result })));
        }
        C::FetchRequestPreviews { channel_ids } => {
            let asked = channel_ids.clone();
            rest(session, sink, move |api| async move { api.message_request_previews(&asked).await }, move |result| {
                Some(ev(SocialEvent::RequestPreviewsLoaded { channel_ids, result }))
            });
        }
        C::AcceptMessageRequest { channel_id } => rest(
            session,
            sink,
            move |api| async move { api.accept_message_request(channel_id).await },
            move |result| Some(ev(SocialEvent::MessageRequestsDone { channel_ids: vec![channel_id], accepted: true, result })),
        ),
        C::RejectMessageRequests { channel_ids } => {
            let asked = channel_ids.clone();
            rest(session, sink, move |api| async move { api.reject_message_requests(&asked).await }, move |result| {
                Some(ev(SocialEvent::MessageRequestsDone { channel_ids, accepted: false, result }))
            });
        }
        C::FetchConnections => rest(
            session,
            sink,
            |api| async move { api.connections().await },
            |result| Some(BackendEvent::Social(SocialEvent::ConnectionsLoaded(result))),
        ),
        C::UpdateConnection { kind, id, visibility, show_activity } => rest(
            session,
            sink,
            move |api| async move {
                api.update_connection(&kind, &id, visibility, show_activity).await?;
                api.connections().await
            },
            |result| Some(BackendEvent::Social(SocialEvent::ConnectionsLoaded(result.map_err(|e| format!("Could not change the connection: {e}"))))),
        ),
        C::RemoveConnection { kind, id } => rest(
            session,
            sink,
            move |api| async move {
                api.remove_connection(&kind, &id).await?;
                api.connections().await
            },
            |result| Some(BackendEvent::Social(SocialEvent::ConnectionsLoaded(result.map_err(|e| format!("Could not remove the connection: {e}"))))),
        ),
    }
}

/// Answer of an account write: `Done` keyed by `op`, the error prefixed with `what`.
fn done(op: AccountOp, what: &'static str) -> impl FnOnce(Result<(), String>) -> Option<BackendEvent> {
    move |r| Some(BackendEvent::Account(AccountEvent::Done { op, result: r.map(|()| None).map_err(|e| format!("{what}: {e}")) }))
}

/// `done` for writes that create something (or join a guild): `Done(Ok(Some(id)))`.
fn created(op: AccountOp, what: &'static str) -> impl FnOnce(Result<Id, String>) -> Option<BackendEvent> {
    move |r| Some(BackendEvent::Account(AccountEvent::Done { op, result: r.map(Some).map_err(|e| format!("{what}: {e}")) }))
}

/// A request that rotates the login token (password, authenticator app): the gateway holds a
/// 4004 and REST ignores old-token 401s meanwhile, then `TokenSwap` puts the new token in place.
fn rotate<T: Send + 'static, F>(
    session: &mut Option<Session>,
    sink: &EventSink,
    call: impl FnOnce(Arc<ApiClient>) -> F + Send + 'static,
    report: impl FnOnce(Result<T, String>) -> Option<BackendEvent> + Send + 'static,
) where
    F: Future<Output = Result<(Token, T), ApiError>> + Send + 'static,
{
    let Some(swap) = session.as_ref().map(|s| s.token_swap(sink)) else {
        if let Some(e) = report(Err("Not connected".into())) {
            sink.send(e);
        }
        return;
    };
    rest(
        session,
        sink,
        move |api| async move {
            let _ = swap.gateway.send(GatewayCommand::ExpectToken);
            let (token, value) = {
                let _rotating = api.rotation();
                let (token, value) = call(api.clone()).await?;
                // Before the guard goes: an old-token 401 must still not count as a logout.
                api.set_token(&token);
                (token, value)
            };
            swap.apply(api, token).await;
            Ok(value)
        },
        report,
    );
}

/// `Command::Account`: every request goes through `rest` (answers + 401 handling); the calls
/// live in `api/account.rs` (settings proto, profile, password and 2FA) and
/// `api/guild_admin.rs` (guilds, channels, roles, members, emoji, invites, notification
/// settings). `finish`: this command is the retry of a gated one, after this re-check.
fn handle_account(cmd: AccountCommand, session: &mut Option<Session>, sink: &EventSink, finish: Option<MfaFinish>) {
    let op = cmd.op();
    if let Some(s) = session.as_mut() {
        s.gate = cmd.mfa_retry().map(|retry| Gate { op, retry, finish });
    }
    handle_account_cmd(cmd, op, session, sink);
    // Every arm spawns at most once; nothing may inherit this command's gate.
    if let Some(s) = session.as_mut() {
        s.gate = None;
    }
}

fn handle_account_cmd(cmd: AccountCommand, op: AccountOp, session: &mut Option<Session>, sink: &EventSink) {
    use AccountCommand as C;
    let ev = BackendEvent::Account;
    match cmd {
        // The UI sends the op 3 (`ChatCommand::SetStatus`) just before this.
        C::SetPersistentStatus { status } => {
            rest(session, sink, move |api| async move { api.save_status(status).await }, done(op, "Could not save your status"))
        }
        C::SetCustomStatus { text, emoji, expires_at_ms } => rest(
            session,
            sink,
            move |api| async move { api.save_custom_status(text.as_deref(), emoji.as_ref(), expires_at_ms).await },
            done(op, "Could not save your custom status"),
        ),
        C::SetGuildFolders { folders } => rest(
            session,
            sink,
            move |api| async move { api.save_guild_folders(&folders).await },
            done(op, "Could not save the server list"),
        ),
        C::SetSetting { top, values } => {
            rest(session, sink, move |api| async move { api.save_settings(top, &values).await }, done(op, "Could not save the setting"));
        }
        C::FetchSessions => rest(session, sink, |api| async move { api.auth_sessions().await }, move |r| {
            Some(ev(AccountEvent::SessionsLoaded(r.map_err(|e| format!("Could not load your devices: {e}")))))
        }),
        C::LogoutSessions { id_hashes } => rest(
            session,
            sink,
            move |api| async move {
                api.logout_sessions(&id_hashes).await?;
                api.auth_sessions().await
            },
            move |r| Some(ev(AccountEvent::SessionsLoaded(r.map_err(|e| format!("Could not log out the device: {e}"))))),
        ),
        C::FetchAuthorizedApps => rest(session, sink, |api| async move { api.authorized_apps().await }, move |r| {
            Some(ev(AccountEvent::AuthorizedAppsLoaded(r.map_err(|e| format!("Could not load your authorized apps: {e}")))))
        }),
        C::RevokeAuthorizedApp { token_id } => rest(
            session,
            sink,
            move |api| async move {
                api.revoke_authorized_app(token_id).await?;
                api.authorized_apps().await
            },
            move |r| Some(ev(AccountEvent::AuthorizedAppsLoaded(r.map_err(|e| format!("Could not deauthorize the app: {e}"))))),
        ),
        C::FetchStanding => rest(session, sink, |api| async move { api.account_standing().await }, move |r| {
            Some(ev(AccountEvent::StandingLoaded(r.map_err(|e| format!("Could not load your account standing: {e}")))))
        }),
        C::FetchConsents => rest(session, sink, |api| async move { api.consents().await }, move |r| {
            Some(ev(AccountEvent::ConsentsLoaded(r.map_err(|e| format!("Could not load your privacy settings: {e}")))))
        }),
        C::SetConsents { grant, revoke } => rest(
            session,
            sink,
            move |api| async move {
                api.set_consents(&grant, &revoke).await?;
                api.consents().await
            },
            move |r| Some(ev(AccountEvent::ConsentsLoaded(r.map_err(|e| format!("Could not save your privacy settings: {e}"))))),
        ),
        C::UpdateGlobalName { name } => rest(
            session,
            sink,
            move |api| async move { api.set_global_name(name.as_deref()).await },
            done(op, "Could not change your display name"),
        ),
        C::UpdateAvatar { path } => {
            rest(session, sink, move |api| async move { api.set_avatar(path).await }, done(op, "Could not change your avatar"));
        }
        C::UpdateBanner { path } => {
            rest(session, sink, move |api| async move { api.set_banner(path).await }, done(op, "Could not change your banner"));
        }
        C::UpdateBio { bio } => {
            rest(session, sink, move |api| async move { api.set_bio(&bio).await }, done(op, "Could not save your About Me"));
        }
        C::UpdatePronouns { pronouns } => {
            rest(session, sink, move |api| async move { api.set_pronouns(&pronouns).await }, done(op, "Could not save your pronouns"));
        }
        C::UpdateAccentColor { color } => rest(
            session,
            sink,
            move |api| async move { api.set_accent_color(color).await },
            done(op, "Could not save your banner colour"),
        ),
        C::UpdateCosmetics { patch } => {
            rest(session, sink, move |api| async move { api.update_cosmetics(&patch).await }, done(op, "Could not update your profile"));
        }
        C::UpdateUsername { username, password } => rest(
            session,
            sink,
            move |api| async move { api.set_username(&username, &password).await },
            done(op, "Could not change your username"),
        ),
        C::ChangePassword { password, new_password } => rotate(
            session,
            sink,
            move |api| async move { Ok((api.change_password(&password, &new_password).await?, ())) },
            done(op, "Could not change your password"),
        ),
        C::CheckTotpPassword { password } => {
            rest(session, sink, move |api| async move { api.totp_check_password(&password).await }, done(op, "Could not enable the authenticator app"));
        }
        C::EnableTotp { password, secret, code } => rotate(
            session,
            sink,
            move |api| async move { api.enable_totp(&password, &secret, &code).await },
            move |r| Some(ev(AccountEvent::BackupCodes { op, result: r.map_err(|e| format!("Could not enable the authenticator app: {e}")) })),
        ),
        C::DisableTotp => rotate(
            session,
            sink,
            |api| async move { Ok((api.disable_totp().await?, ())) },
            done(op, "Could not remove the authenticator app"),
        ),
        C::SetSmsMfa { enable, password } => rest(
            session,
            sink,
            move |api| async move { api.set_sms_mfa(enable, &password).await },
            done(op, if enable { "Could not turn on SMS authentication" } else { "Could not remove SMS authentication" }),
        ),
        C::FetchSecurityKeys => rest(session, sink, |api| async move { api.security_keys().await }, move |r| {
            Some(ev(AccountEvent::SecurityKeysLoaded(r.map_err(|e| format!("Could not load your security keys: {e}")))))
        }),
        C::RenameSecurityKey { id, name } => rest(
            session,
            sink,
            move |api| async move {
                api.rename_security_key(id, &name).await?;
                api.security_keys().await
            },
            move |r| Some(ev(AccountEvent::SecurityKeysLoaded(r.map_err(|e| format!("Could not rename the key: {e}"))))),
        ),
        C::DeleteSecurityKey { id } => rest(
            session,
            sink,
            move |api| async move {
                api.delete_security_key(id).await?;
                api.security_keys().await
            },
            move |r| Some(ev(AccountEvent::SecurityKeysLoaded(r.map_err(|e| format!("Could not remove the key: {e}"))))),
        ),
        C::BackupCodesChallenge { password } => rest(
            session,
            sink,
            move |api| async move { api.backup_codes_challenge(&password).await },
            move |r| Some(ev(AccountEvent::CodesNonce { op, result: r.map_err(|e| format!("Could not send the key: {e}")) })),
        ),
        C::ViewBackupCodes { key, nonce, regenerate } => rest(
            session,
            sink,
            move |api| async move { api.backup_codes(&key, &nonce, regenerate).await },
            move |r| Some(ev(AccountEvent::BackupCodes { op, result: r.map_err(|e| format!("Could not load your backup codes: {e}")) })),
        ),
        C::FinishMfa { kind, code } => {
            let pending = session.as_ref().and_then(|s| mfa_slot(&s.mfa).take());
            match pending {
                Some(p) => handle_account(p.cmd, session, sink, Some((p.ticket, p.methods, kind, code, p.at))),
                None => {
                    sink.send(mfa_prompt(None));
                    sink.send(BackendEvent::ActionFailed("That two-factor check expired. Try the action again.".into()));
                }
            }
        }
        C::MfaSms => {
            let pending = session.as_ref().and_then(|s| mfa_slot(&s.mfa).as_ref().map(|p| (p.ticket.clone(), p.methods.clone(), p.op)));
            let Some((ticket, methods, gated)) = pending else { return sink.send(mfa_prompt(None)) };
            rest(session, sink, move |api| async move { api.mfa_send_sms(&ticket).await }, move |r| {
                let (sms_phone, error) = match r {
                    Ok(phone) => (Some(phone), None),
                    Err(e) => (None, Some(format!("Could not send the code: {e}"))),
                };
                Some(mfa_prompt(Some(MfaPrompt { op: gated, methods, sms_phone, error, busy: false })))
            });
        }
        C::CancelMfa => {
            if let Some(s) = session.as_ref() {
                mfa_slot(&s.mfa).take();
            }
        }
        C::CreateGuild { name, icon } => {
            rest(session, sink, move |api| async move { api.create_guild(&name, icon).await }, created(op, "Could not create the server"));
        }
        C::LeaveGuild { guild_id } => {
            rest(session, sink, move |api| async move { api.leave_guild(guild_id).await }, done(op, "Could not leave the server"));
        }
        C::UpdateGuild { guild_id, patch } => rest(
            session,
            sink,
            move |api| async move { api.update_guild(guild_id, patch).await },
            done(op, "Could not save the server settings"),
        ),
        C::CreateChannel { guild_id, name, kind, parent_id } => rest(
            session,
            sink,
            move |api| async move { api.create_channel(guild_id, &name, kind, parent_id).await },
            created(op, "Could not create the channel"),
        ),
        C::UpdateChannel { channel_id, patch } => rest(
            session,
            sink,
            move |api| async move { api.update_channel(channel_id, &patch).await },
            done(op, "Could not save the channel"),
        ),
        C::DeleteChannel { channel_id } => {
            rest(session, sink, move |api| async move { api.delete_channel(channel_id).await }, done(op, "Could not delete the channel"));
        }
        C::ReorderChannels { guild_id, positions } => rest(
            session,
            sink,
            move |api| async move { api.reorder_channels(guild_id, &positions).await },
            done(op, "Could not move the channels"),
        ),
        C::SetPermissionOverwrite { channel_id, overwrite } => rest(
            session,
            sink,
            move |api| async move { api.set_overwrite(channel_id, &overwrite).await },
            done(op, "Could not save the permissions"),
        ),
        C::DeletePermissionOverwrite { channel_id, target_id } => rest(
            session,
            sink,
            move |api| async move { api.delete_overwrite(channel_id, target_id).await },
            done(op, "Could not remove the permissions"),
        ),
        C::CreateRole { guild_id, role } => {
            rest(session, sink, move |api| async move { api.create_role(guild_id, &role).await }, created(op, "Could not create the role"));
        }
        C::UpdateRole { guild_id, role_id, patch } => rest(
            session,
            sink,
            move |api| async move { api.update_role(guild_id, role_id, &patch).await },
            done(op, "Could not save the role"),
        ),
        C::DeleteRole { guild_id, role_id } => {
            rest(session, sink, move |api| async move { api.delete_role(guild_id, role_id).await }, done(op, "Could not delete the role"));
        }
        C::ReorderRoles { guild_id, positions } => rest(
            session,
            sink,
            move |api| async move { api.reorder_roles(guild_id, &positions).await },
            done(op, "Could not move the roles"),
        ),
        C::AddMemberRole { guild_id, user_id, role_id } => rest(
            session,
            sink,
            move |api| async move { api.member_role(guild_id, user_id, role_id, true).await },
            done(op, "Could not add the role"),
        ),
        C::RemoveMemberRole { guild_id, user_id, role_id } => rest(
            session,
            sink,
            move |api| async move { api.member_role(guild_id, user_id, role_id, false).await },
            done(op, "Could not remove the role"),
        ),
        C::SetMyNick { guild_id, nick } => rest(
            session,
            sink,
            move |api| async move { api.set_nick(guild_id, None, nick.as_deref()).await },
            done(op, "Could not change your nickname"),
        ),
        C::SetNick { guild_id, user_id, nick } => rest(
            session,
            sink,
            move |api| async move { api.set_nick(guild_id, Some(user_id), nick.as_deref()).await },
            done(op, "Could not change the nickname"),
        ),
        C::KickMember { guild_id, user_id, reason } => rest(
            session,
            sink,
            move |api| async move { api.kick(guild_id, user_id, reason.as_deref()).await },
            done(op, "Could not kick the member"),
        ),
        C::BanMember { guild_id, user_id, reason, delete_message_seconds } => rest(
            session,
            sink,
            move |api| async move { api.ban(guild_id, user_id, reason.as_deref(), delete_message_seconds).await },
            done(op, "Could not ban the member"),
        ),
        C::Unban { guild_id, user_id } => {
            rest(session, sink, move |api| async move { api.unban(guild_id, user_id).await }, done(op, "Could not revoke the ban"));
        }
        C::FetchBans { guild_id, after } => {
            rest(session, sink, move |api| async move { api.bans(guild_id, after).await }, move |result| {
                Some(ev(AccountEvent::BansLoaded { guild_id, after, result }))
            });
        }
        C::UploadEmoji { guild_id, name, path } => rest(
            session,
            sink,
            move |api| async move { api.upload_emoji(guild_id, &name, path).await },
            done(op, "Could not upload the emoji"),
        ),
        C::DeleteEmoji { guild_id, emoji_id } => {
            rest(session, sink, move |api| async move { api.delete_emoji(guild_id, emoji_id).await }, done(op, "Could not delete the emoji"));
        }
        C::CreateInvite { channel_id, max_age, max_uses, temporary } => rest(
            session,
            sink,
            move |api| async move { api.create_invite(channel_id, max_age, max_uses, temporary).await },
            move |r| Some(ev(AccountEvent::InviteCreated { channel_id, result: r.map_err(|e| format!("Could not create the invite: {e}")) })),
        ),
        C::ResolveInvite { code } => {
            let c = code.clone();
            rest(session, sink, move |api| async move { api.resolve_invite(&c).await }, move |result| {
                Some(ev(AccountEvent::InviteResolved { code, result }))
            });
        }
        C::AcceptInvite { code, guild_id, channel_id, channel_type } => rest(
            session,
            sink,
            move |api| async move { api.accept_invite(&code, guild_id, channel_id, channel_type).await },
            created(op, "Could not join the server"),
        ),
        C::DeleteInvite { code } => {
            rest(session, sink, move |api| async move { api.delete_invite(&code).await }, done(op, "Could not delete the invite"));
        }
        C::FetchGuildInvites { guild_id } => {
            rest(session, sink, move |api| async move { api.guild_invites(guild_id).await }, move |result| {
                Some(ev(AccountEvent::InvitesLoaded { guild_id, result }))
            });
        }
        C::UpdateGuildNotifications { guild_id, patch } => rest(
            session,
            sink,
            move |api| async move { api.update_notifications(guild_id, None, &patch).await },
            done(op, "Could not save the notification settings"),
        ),
        C::UpdateChannelOverride { guild_id, channel_id, patch } => rest(
            session,
            sink,
            move |api| async move { api.update_notifications(guild_id, Some(channel_id), &patch).await },
            done(op, "Could not save the notification settings"),
        ),
        C::FetchGuildProfile { guild_id } => rest(session, sink, move |api| async move { api.guild_profile(guild_id).await }, move |result| {
            Some(ev(AccountEvent::GuildProfileLoaded { guild_id, result }))
        }),
        C::JoinGuild { guild_id } => rest(
            session,
            sink,
            move |api| async move { api.join_guild(guild_id).await.map(|()| guild_id) },
            created(op, "Could not join the server"),
        ),
        C::FetchMemberVerification { guild_id } => rest(session, sink, move |api| async move { api.member_verification(guild_id).await }, move |result| {
            Some(ev(AccountEvent::VerificationLoaded { guild_id, result }))
        }),
        C::SubmitJoinRequest { guild_id, version, form_fields } => rest(
            session,
            sink,
            move |api| async move { api.submit_join_request(guild_id, version, form_fields).await },
            move |r| {
                let result = r.map(|status| (status == "APPROVED").then_some(guild_id)).map_err(|e| format!("Could not send the application: {e}"));
                Some(ev(AccountEvent::Done { op, result }))
            },
        ),
    }
}

/// Acks are automatic: failures stay silent (except 401, handled in `spawn`); nothing while
/// logged out.
fn ack(session: &mut Option<Session>, sink: &EventSink, channel_id: Id, message_id: Id, thread: bool) {
    if let Some(s) = session {
        let api = s.api.clone();
        s.spawn(sink, async move { api.ack(channel_id, message_id, thread).await }, |_| None);
    }
}

/// Pending 2FA challenge of the last `LoginPassword` (Debug-safe: the ticket is a Secret).
#[derive(Debug)]
struct MfaTicket {
    ticket: Secret,
    login_instance_id: Option<String>,
    remember: bool,
}

/// A captcha nobody answers is given up after this (the parked request holds the password).
const CAPTCHA_TIMEOUT: Duration = Duration::from_secs(600);

/// A login request that waits for the user to solve Discord's captcha; its answer goes with the same
/// request again. The task is the 10-minute cap.
struct Parked {
    cmd: Command,
    challenge: CaptchaChallenge,
    /// What to show when the card cannot show the captcha (no webview).
    fallback: String,
    task: tokio::task::JoinHandle<()>,
}

type ParkedSlot = Arc<std::sync::Mutex<Option<Parked>>>;

fn unpark(slot: &ParkedSlot) {
    if let Some(p) = lock(slot).take() {
        p.task.abort();
    }
}

fn set_answer(cmd: &mut Command, answer: CaptchaAnswer) {
    match cmd {
        Command::LoginPassword { captcha, .. }
        | Command::SubmitMfa { captcha, .. }
        | Command::SendMfaSms { captcha }
        | Command::QrExchange { captcha, .. } => *captcha = Some(answer),
        _ => {}
    }
}

/// A login request failed. A captcha the user can solve goes to the login card and parks `retry` (the
/// same request); an answered one that Discord still refuses is final: the next try is the user's click.
fn fail_or_park(slot: &ParkedSlot, again: &tmpsc::WeakUnboundedSender<Command>, sink: &EventSink, e: LoginError, retry: Command, answered: bool) {
    let challenge = match e.captcha {
        Some(ch) if crate::captcha::supported() => *ch,
        _ => return sink.send(login_step(LoginStep::Failed(e.message))),
    };
    if answered {
        return sink.send(login_step(LoginStep::Failed(CAPTCHA_REJECTED.into())));
    }
    let again = again.clone();
    let task = tokio::spawn(async move {
        tokio::time::sleep(CAPTCHA_TIMEOUT).await;
        if let Some(tx) = again.upgrade() {
            let _ = tx.send(Command::Captcha(CaptchaReply::Cancelled));
        }
    });
    let step = LoginStep::Captcha { sitekey: challenge.sitekey.clone(), rqdata: challenge.rqdata.clone() };
    if let Some(old) = lock(slot).replace(Parked { cmd: retry, challenge, fallback: e.message, task }) {
        old.task.abort();
    }
    sink.send(login_step(step));
}

/// A finished email/QR login re-enters the normal token path (validate, keyring, gateway).
fn relogin(again: &tmpsc::WeakUnboundedSender<Command>, token: Token, remember: bool) {
    if let Some(tx) = again.upgrade() {
        let _ = tx.send(Command::Login { token, remember });
    }
}

/// The login screen's unauthenticated client: one `/experiments` fingerprint (fetched by its first
/// request) and Discord's device cookies, shared by the email and QR flows (a new code reuses it).
fn login_client(slot: &mut Option<Arc<AuthClient>>, props: &ClientProps) -> Arc<AuthClient> {
    if let Some(c) = slot {
        return c.clone();
    }
    let c = Arc::new(AuthClient::new(props, Some(crate::config::paths().device_file())));
    *slot = Some(c.clone());
    c
}

fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn login_step(step: LoginStep) -> BackendEvent {
    BackendEvent::LoginStep(step)
}

/// `Command::Activity`: op 3 through the gateway, icon fetches through the limiter, the rest
/// to the activity thread (local only).
fn handle_activity(cmd: crate::activity::ActivityCommand, session: &mut Option<Session>, sink: &EventSink, activity: &crate::activity::Service) {
    use crate::activity::{ActivityCommand as C, ActivityEvent};
    match cmd {
        // Without a session there is no presence to change; the UI sends it again after login.
        C::SetGame(game) => {
            if let Some(s) = session {
                s.gateway.send(GatewayCommand::SetGame(game));
            }
        }
        C::FetchApps(ids) => {
            let (asked, service) = (ids.clone(), activity.clone());
            rest(session, sink, move |api| async move { api.public_apps(&asked).await }, move |r| {
                let found = r.map_err(|e| eprintln!("rustcord: app icons: {e}")).ok()?;
                // Every asked id is answered (None: unknown app), so none is asked again.
                let icons: Vec<(Id, Option<String>)> = ids.iter().map(|&id| (id, found.iter().find(|a| a.id == id).and_then(|a| a.icon.clone()))).collect();
                service.save_apps(icons.clone());
                Some(BackendEvent::Activity(ActivityEvent::Apps(icons)))
            });
        }
        C::SetAdded(games) => activity.set_added(games),
        C::ListWindows => activity.list_windows(),
    }
}

/// `Command::Quests` (docs/QUESTS_NOTES.md): one request per click, the page open's fetch, or one
/// video progress send per 10 s of real play. Logged out: every one is answered "Not connected".
/// Errors are already quest copy (`api::quests`: captcha, 401 without logout).
fn handle_quests(cmd: crate::quests::QuestCommand, session: &mut Option<Session>, sink: &EventSink) {
    use crate::quests::{QuestCommand as C, QuestEvent as E};
    let ev = |e: E| Some(BackendEvent::Quests(e));
    let failed = move |quest_id: Id, stop_video: bool| move |message: String| ev(E::Failed { quest_id, message, stop_video });
    match cmd {
        C::Fetch => {
            rest(session, sink, |api| async move { api.quests_me().await }, move |r| ev(E::Loaded(r)));
            // The header just stays without a balance if this fails.
            rest(session, sink, |api| async move { api.orbs_balance().await }, move |r| {
                r.map_err(|e| eprintln!("rustcord: Orbs balance: {e}")).ok().and_then(|n| ev(E::Orbs(n)))
            });
        }
        C::Enroll { quest_id, sealed } => rest(session, sink, |api| async move { api.quest_enroll(quest_id, sealed.as_deref()).await }, move |r| {
            r.map_or_else(failed(quest_id, false), |s| ev(E::Status(Box::new(s))))
        }),
        C::VideoProgress { quest_id, timestamp } => {
            rest(session, sink, |api| async move { api.quest_video_progress(quest_id, timestamp).await }, move |r| match r {
                Ok(s) => ev(E::VideoSaved(Box::new(s))),
                Err(e) => {
                    eprintln!("rustcord: quest video progress: {e}");
                    failed(quest_id, true)(crate::quests::VIDEO_FAILED.into())
                }
            })
        }
        C::Claim { quest_id, platform, sealed } => rest(session, sink, |api| async move { api.quest_claim(quest_id, platform, sealed.as_deref()).await }, move |r| {
            r.map_or_else(failed(quest_id, false), |a| {
                let error = a.errors.into_iter().map(|e| e.message).find(|m| !m.is_empty());
                ev(E::Claimed { quest_id, claimed_at: a.claimed_at, error })
            })
        }),
        C::RewardCode { quest_id } => rest(session, sink, |api| async move { api.quest_reward_code(quest_id).await }, move |r| {
            ev(E::Code { quest_id, code: r.map(Arc::new) })
        }),
    }
}

/// After a login: fetch the detectable-games list when the saved one is missing or a day old,
/// once per run (a failure waits for the next start), a little after the session's own start.
fn refresh_detectable(session: &mut Option<Session>, sink: &EventSink, activity: &crate::activity::Service, fetched: &mut bool) {
    use crate::activity::{ActivityEvent, Table};
    let path = crate::activity::table_path();
    if *fetched || crate::config::offline_mode() || !crate::activity::table_stale(&path) {
        return;
    }
    *fetched = true;
    let service = activity.clone();
    rest(
        session,
        sink,
        move |api| async move {
            tokio::time::sleep(Duration::from_secs(10)).await;
            let body = api.detectable_games().await?;
            // ~13 MB of JSON: parsed and saved on a blocking thread, the body dropped after.
            let parsed = tokio::task::spawn_blocking(move || {
                let t = Table::from_json(&body)?;
                t.save(&path);
                Ok::<_, String>(Arc::new(t))
            });
            parsed.await.map_err(|e| ApiError::Decode(e.to_string()))?.map_err(ApiError::Decode)
        },
        move |r| match r {
            Ok(t) => {
                service.table(t.clone());
                Some(BackendEvent::Activity(ActivityEvent::Table(t)))
            }
            Err(e) => {
                eprintln!("rustcord: games list: {e}");
                None
            }
        },
    );
}

async fn run(
    mut cmds: tmpsc::UnboundedReceiver<Command>,
    again: tmpsc::WeakUnboundedSender<Command>,
    sink: EventSink,
    video: Arc<crate::voice::video::VideoFrames>,
) {
    let props = Arc::new(ClientProps::new(&crate::config::Settings::load()));
    let images = crate::cache::images::ImageLoader::new();
    // Outlives logins: device and input-mode choices are kept for the next call.
    let voice = VoiceManager::spawn(sink.clone(), video);
    // Game detection (its own thread; local files only) and the games list fetched once a run.
    let activity = crate::activity::Service::spawn(sink.clone());
    let mut detectable_fetched = false;
    let mut session: Option<Session> = None;
    let not_connected = || BackendEvent::ActionFailed("Not connected".into());
    // Login screen state: unauthenticated client (+ fingerprint), 2FA ticket, running QR flow.
    let mut auth_client: Option<Arc<AuthClient>> = None;
    // Shared with the spawned password login, which may finish while other commands run.
    let mfa: Arc<std::sync::Mutex<Option<MfaTicket>>> = Default::default();
    let mut qr: Option<tokio::task::JoinHandle<()>> = None;
    // A login request waiting for the captcha in the card; shared with the tasks that run the requests.
    let parked: ParkedSlot = Default::default();
    // Shown as a toast once the password login's token is accepted.
    let notice: Arc<std::sync::Mutex<Option<&'static str>>> = Default::default();
    let mut chat = ChatBackend::default();

    while let Some(cmd) = cmds.recv().await {
        if let Some(s) = &mut session {
            while s.tasks.try_join_next().is_some() {}
        }
        match cmd {
            Command::Login { token, remember } => {
                if let Some(old) = session.take() {
                    old.shutdown();
                }
                let api = Arc::new(ApiClient::new(token.clone(), props.clone()));
                match api.me().await {
                    Ok(_) => {
                        // LoggedIn first: the UI shows an ActionFailed that arrives while the
                        // login card is busy as a login error, which would hide the toast below.
                        sink.send(BackendEvent::LoggedIn);
                        // Logged in (maybe through another tab): drop leftover login-screen credentials.
                        *lock(&mfa) = None;
                        unpark(&parked);
                        auth_client = None;
                        // Like the web client: a login ends the fingerprint, the device cookies stay.
                        tokio::task::spawn_blocking(|| crate::api::auth::forget_fingerprint(&crate::config::paths().device_file()));
                        if let Some(n) = lock(&notice).take() {
                            sink.send(BackendEvent::ActionFailed(n.into()));
                        }
                        session = Some(Session::start(api, token.clone(), remember, &props, &sink, &voice));
                        refresh_detectable(&mut session, &sink, &activity, &mut detectable_fetched);
                        if remember {
                            let t = token.clone();
                            let saved = tokio::task::spawn_blocking(move || crate::auth::save_token(&t)).await;
                            let err = match saved {
                                Ok(Ok(())) => None,
                                Ok(Err(e)) => Some(e),
                                Err(_) => Some("credential store task failed".to_owned()),
                            };
                            if let Some(e) = err {
                                sink.send(BackendEvent::ActionFailed(format!(
                                    "Logged in, but the login could not be remembered ({e})."
                                )));
                            }
                        }
                    }
                    Err(ApiError::Unauthorized) => {
                        *lock(&notice) = None;
                        sink.send(BackendEvent::AuthFailed("That token was rejected.".into()))
                    }
                    // Not an auth problem (network, 5xx, rate limit): AuthFailed would make the
                    // UI wipe the cache and keyring. The login card shows this inline instead.
                    Err(e) => {
                        *lock(&notice) = None;
                        sink.send(BackendEvent::ActionFailed(format!("Could not log in: {e}")))
                    }
                }
            }
            Command::Resume { token } => {
                if let Some(old) = session.take() {
                    old.shutdown();
                }
                let api = Arc::new(ApiClient::new(token.clone(), props.clone()));
                // Resume runs with the token the keyring held: a rotated one goes back there.
                session = Some(Session::start(api, token, true, &props, &sink, &voice));
                refresh_detectable(&mut session, &sink, &activity, &mut detectable_fetched);
            }
            // One user action = one request, never retried. The password login runs in its own task:
            // its first request may fetch the fingerprint first, which must not stall other commands.
            Command::LoginPassword { login, password, remember, captcha } => {
                unpark(&parked);
                *lock(&mfa) = None;
                let retry = Command::LoginPassword { login: login.clone(), password: password.clone(), remember, captcha: None };
                // Test hook: a captcha without a request to Discord (docs/AUTH_NOTES.md section 0).
                if std::env::var_os("RUSTCORD_CAPTCHA_DEMO").is_some() {
                    match &captcha {
                        None => fail_or_park(&parked, &again, &sink, crate::api::auth::demo_captcha(), retry, false),
                        Some(a) => {
                            let n = crate::api::auth::captcha_headers(a).len();
                            sink.send(login_step(LoginStep::Failed(format!("Captcha demo: Discord would now get the answer in {n} header(s)."))));
                        }
                    }
                    continue;
                }
                let client = login_client(&mut auth_client, &props);
                let (sink, again, mfa, notice, parked) = (sink.clone(), again.clone(), mfa.clone(), notice.clone(), parked.clone());
                tokio::spawn(async move {
                    match client.login(&login, &password, captcha.as_ref()).await {
                        Ok(LoginOutcome::Token { token, update_password }) => {
                            *lock(&notice) =
                                update_password.then_some("Discord asks you to change your password. Do it in the official app.");
                            relogin(&again, token, remember);
                        }
                        Ok(LoginOutcome::Mfa { ticket, login_instance_id, methods }) => {
                            *lock(&mfa) =
                                Some(MfaTicket { ticket, login_instance_id, remember });
                            sink.send(login_step(LoginStep::MfaRequired { methods }));
                        }
                        Err(e) => fail_or_park(&parked, &again, &sink, e, retry, captcha.is_some()),
                    }
                });
            }
            Command::SubmitMfa { method, code, captcha } => {
                let ticket = lock(&mfa).take();
                match (ticket, &auth_client) {
                    (Some(t), Some(client)) => {
                        match client.verify_mfa(method, &code, &t.ticket, t.login_instance_id.as_deref(), captcha.as_ref()).await {
                            Ok(token) => {
                                auth_client = None;
                                relogin(&again, token, t.remember);
                            }
                            Err(e) => {
                                if e.keep_ticket {
                                    *lock(&mfa) = Some(t);
                                }
                                let retry = Command::SubmitMfa { method, code, captcha: None };
                                fail_or_park(&parked, &again, &sink, e, retry, captcha.is_some());
                            }
                        }
                    }
                    _ => sink.send(login_step(LoginStep::Failed(EXPIRED.into()))),
                }
            }
            Command::SendMfaSms { captcha } => {
                let ticket = lock(&mfa).as_ref().map(|t| t.ticket.clone());
                match (ticket, &auth_client) {
                    (Some(ticket), Some(client)) => match client.send_mfa_sms(&ticket, captcha.as_ref()).await {
                        Ok(phone) => sink.send(login_step(LoginStep::SmsSent { phone })),
                        Err(e) => {
                            if !e.keep_ticket {
                                *lock(&mfa) = None;
                            }
                            fail_or_park(&parked, &again, &sink, e, Command::SendMfaSms { captcha: None }, captcha.is_some());
                        }
                    },
                    _ => sink.send(login_step(LoginStep::Failed(EXPIRED.into()))),
                }
            }
            Command::StartQrLogin { remember } => {
                if let Some(old) = qr.take() {
                    old.abort();
                }
                unpark(&parked);
                // Created now so the exchange after the scan has the device's cookies and fingerprint.
                login_client(&mut auth_client, &props);
                let (props, sink, again) = (props.clone(), sink.clone(), again.clone());
                qr = Some(tokio::spawn(async move {
                    if let Some((ticket, keys)) = crate::gateway::remote_auth::run(props, sink).await
                        && let Some(tx) = again.upgrade()
                    {
                        let _ = tx.send(Command::QrExchange { ticket, keys: Arc::new(keys), remember, captcha: None });
                    }
                }));
            }
            // The phone approved: one exchange per ticket, never retried (a captcha is answered once).
            Command::QrExchange { ticket, keys, remember, captcha } => {
                let auth = login_client(&mut auth_client, &props);
                let (sink, again, parked) = (sink.clone(), again.clone(), parked.clone());
                tokio::spawn(async move {
                    match auth.remote_auth_login(&ticket, captcha.as_ref()).await {
                        Ok(encrypted) => match keys.token(&encrypted) {
                            Ok(token) => relogin(&again, token, remember),
                            Err(msg) => sink.send(login_step(LoginStep::Failed(msg))),
                        },
                        Err(e) => {
                            let answered = captcha.is_some();
                            fail_or_park(&parked, &again, &sink, e, Command::QrExchange { ticket, keys, remember, captcha: None }, answered);
                        }
                    }
                });
            }
            Command::Captcha(reply) => {
                let Some(p) = lock(&parked).take() else { continue }; // late or duplicate
                p.task.abort();
                match reply {
                    CaptchaReply::Solved(key) => {
                        let mut cmd = p.cmd;
                        set_answer(&mut cmd, p.challenge.answer(key));
                        if let Some(tx) = again.upgrade() {
                            let _ = tx.send(cmd);
                        }
                    }
                    CaptchaReply::Cancelled => sink.send(login_step(LoginStep::Failed(CAPTCHA_CANCELLED.into()))),
                    CaptchaReply::Unavailable => sink.send(login_step(LoginStep::Failed(p.fallback))),
                }
            }
            Command::CancelQrLogin => {
                if let Some(t) = qr.take() {
                    t.abort();
                }
            }
            Command::Logout => {
                if let Some(t) = qr.take() {
                    t.abort();
                }
                unpark(&parked);
                *lock(&mfa) = None;
                auth_client = None;
                // Like the image cache: the app icons of this account's friends' activities go.
                activity.save_apps(Vec::new());
                if let Some(old) = session.take() {
                    // Gateway first (a 4004 after the token dies would show as AuthFailed on the new
                    // login screen), bounded so no event of the old connection arrives after Offline.
                    // Then end the token at Discord; errors ignored, a logout always completes locally.
                    let api = old.api.clone();
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), old.shutdown()).await;
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), api.logout()).await;
                }
                // Awaited so a following Login cannot race with the delete.
                if let Ok(Err(e)) = tokio::task::spawn_blocking(crate::auth::clear_token).await {
                    sink.send(BackendEvent::ActionFailed(format!("Logged out, but {e}.")));
                }
                sink.send(BackendEvent::Conn(ConnState::Offline));
            }
            Command::FetchMessages { channel_id, before, limit } => match &mut session {
                Some(s) => {
                    let api = s.api.clone();
                    s.spawn(&sink, async move { api.messages(channel_id, before, limit).await }, move |result| {
                        Some(BackendEvent::MessagesLoaded { channel_id, before, result })
                    });
                }
                None => sink.send(BackendEvent::MessagesLoaded { channel_id, before, result: Err("offline".into()) }),
            },
            Command::SendMessage { channel_id, content, nonce, reply } => match &mut session {
                Some(s) => {
                    chat.typing.reset(channel_id);
                    let api = s.api.clone();
                    let n = nonce.clone();
                    s.spawn(
                        &sink,
                        async move { api.send_message(channel_id, &content, &n, reply.as_ref()).await },
                        move |result| Some(BackendEvent::SendResult { channel_id, nonce, result }),
                    );
                }
                // SendResult (not ActionFailed) so the pending message is marked failed.
                None => sink.send(BackendEvent::SendResult { channel_id, nonce, result: Err("Not connected".into()) }),
            },
            Command::EditMessage { channel_id, message_id, content } => match &mut session {
                Some(s) => {
                    let api = s.api.clone();
                    s.spawn(
                        &sink,
                        async move { api.edit_message(channel_id, message_id, &content).await.map(drop) },
                        failed("Could not edit the message"),
                    );
                }
                None => sink.send(not_connected()),
            },
            Command::DeleteMessage { channel_id, message_id } => match &mut session {
                Some(s) => {
                    let api = s.api.clone();
                    s.spawn(
                        &sink,
                        async move { api.delete_message(channel_id, message_id).await },
                        failed("Could not delete the message"),
                    );
                }
                None => sink.send(not_connected()),
            },
            Command::AddReaction { channel_id, message_id, emoji } => match &mut session {
                Some(s) => {
                    let api = s.api.clone();
                    s.spawn(
                        &sink,
                        async move { api.add_reaction(channel_id, message_id, &emoji).await },
                        failed("Could not add the reaction"),
                    );
                }
                None => sink.send(not_connected()),
            },
            Command::RemoveReaction { channel_id, message_id, emoji } => match &mut session {
                Some(s) => {
                    let api = s.api.clone();
                    s.spawn(
                        &sink,
                        async move { api.remove_reaction(channel_id, message_id, &emoji).await },
                        failed("Could not remove the reaction"),
                    );
                }
                None => sink.send(not_connected()),
            },
            Command::Ack { channel_id, message_id } => ack(&mut session, &sink, channel_id, message_id, false),
            Command::AckThread { channel_id, message_id } => ack(&mut session, &sink, channel_id, message_id, true),
            // Automatic on channel switch; nothing to do while logged out.
            Command::SubscribeMemberList { guild_id, channel_id } => {
                if let Some(s) = &session {
                    s.gateway.send(GatewayCommand::SubscribeMemberList { guild_id, channel_id });
                }
            }
            Command::LoadImage { url, max_side } => images.request(url, max_side, sink.clone()),
            // Not logged in: the voice task answers JoinVoice with "Not connected".
            Command::JoinVoice { guild_id, channel_id } => voice.join(guild_id, channel_id),
            Command::LeaveVoice => voice.leave(),
            Command::RingCall { channel_id, recipients, check_ringable } => rest(
                &mut session,
                &sink,
                |api| async move {
                    if check_ringable && !api.call_ringable(channel_id).await? {
                        return Ok(());
                    }
                    api.ring_call(channel_id, recipients.as_deref()).await
                },
                failed("Could not ring"),
            ),
            Command::StopRinging { channel_id, recipients } => rest(
                &mut session,
                &sink,
                |api| async move { api.stop_ringing(channel_id, recipients.as_deref()).await },
                failed("Could not decline the call"),
            ),
            Command::SetSelfMute(on) => voice.set_self_mute(on),
            Command::SetSelfDeaf(on) => voice.set_self_deaf(on),
            Command::SetVoiceDevices { input, output } => voice.set_devices(input, output),
            Command::SetVoiceInputMode(mode) => voice.set_input_mode(mode),
            Command::ListVoiceDevices => voice.list_devices(),
            Command::Video(cmd) => voice.video(cmd),
            Command::Chat(c) => handle_chat(c, &mut session, &sink, &props, &mut chat),
            Command::Social(c) => handle_social(c, &mut session, &sink),
            Command::Account(c) => handle_account(c, &mut session, &sink, None),
            Command::Activity(c) => handle_activity(c, &mut session, &sink, &activity),
            Command::Quests(c) => handle_quests(c, &mut session, &sink),
        }
    }

    if let Some(s) = session.take() {
        s.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_captcha_answer_goes_into_the_parked_request() {
        let answer = || CaptchaChallenge { sitekey: "k".into(), rqdata: None, rqtoken: Some("rq".into()), session_id: None }.answer(Secret::new("tok".into()));
        let has = |c: &Option<CaptchaAnswer>| c.as_ref().is_some_and(|a| a.key.expose() == "tok" && a.rqtoken.as_deref() == Some("rq"));
        let keys = Arc::new(RaKeys::generate().unwrap());
        let mut cmds = vec![
            Command::LoginPassword { login: "a".into(), password: Secret::new("p".into()), remember: true, captcha: None },
            Command::SubmitMfa { method: MfaMethod::Totp, code: Secret::new("1".into()), captcha: None },
            Command::SendMfaSms { captcha: None },
            Command::QrExchange { ticket: Secret::new("t".into()), keys, remember: false, captcha: None },
        ];
        for c in &mut cmds {
            set_answer(c, answer());
            assert!(
                match c {
                    Command::LoginPassword { captcha, .. }
                    | Command::SubmitMfa { captcha, .. }
                    | Command::SendMfaSms { captcha }
                    | Command::QrExchange { captcha, .. } => has(captcha),
                    _ => false,
                },
                "{c:?}"
            );
        }
        // Anything else is left alone.
        let mut logout = Command::Logout;
        set_answer(&mut logout, answer());
        assert!(matches!(logout, Command::Logout));
    }

    #[test]
    fn typing_throttle_per_channel_and_reset() {
        let mut t = TypingThrottle::default();
        let t0 = Instant::now();
        assert!(t.should_send(Id(1), t0));
        assert!(!t.should_send(Id(1), t0 + Duration::from_secs(7)));
        assert!(t.should_send(Id(2), t0), "other channels have their own window");
        assert!(t.should_send(Id(1), t0 + TYPING_RESEND));
        t.reset(Id(1));
        assert!(t.should_send(Id(1), t0 + TYPING_RESEND), "a sent message resets the throttle");
        assert!(!t.should_send(Id(1), t0 + TYPING_RESEND + Duration::from_secs(1)));
        // Old entries are dropped on the way.
        assert!(t.should_send(Id(3), t0 + Duration::from_secs(60)));
        assert_eq!(t.last.len(), 1);
    }

    #[test]
    fn an_event_wakes_one_pass() {
        let ctx = egui::Context::default();
        let pass = || ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        // The first passes settle egui itself (fonts …).
        for _ in 0..5 {
            pass();
        }
        assert!(!ctx.has_requested_repaint());
        let (tx, rx) = mpsc::channel();
        let sink = EventSink::new(tx, ctx.clone());
        sink.send(BackendEvent::ActionFailed("a".into()));
        sink.send(BackendEvent::ActionFailed("b".into()));
        assert!(ctx.has_requested_repaint());
        pass();
        // egui's own settle pass (`request_repaint`) would still be pending here.
        assert!(!ctx.has_requested_repaint());
        assert_eq!(rx.try_iter().count(), 2);
    }

    #[test]
    fn upload_bytes_debug_is_short() {
        let s = UploadSource::Bytes(vec![7u8; 4096].into());
        assert_eq!(format!("{s:?}"), "Bytes(4096 bytes)");
    }

    #[tokio::test]
    async fn chat_commands_offline_answer_or_stay_silent() {
        let (tx, rx) = mpsc::channel();
        let sink = EventSink::new(tx, egui::Context::default());
        let props = ClientProps::new(&crate::config::Settings::default());
        let (mut session, mut chat) = (None, ChatBackend::default());
        let mut run = |c| handle_chat(c, &mut session, &sink, &props, &mut chat);
        run(ChatCommand::Typing { channel_id: Id(1) });
        run(ChatCommand::SearchMembers { guild_id: Id(1), query: "ab".into(), limit: 10 });
        run(ChatCommand::FetchPins { channel_id: Id(1), before: None });
        run(ChatCommand::SendWithFiles { channel_id: Id(1), content: String::new(), nonce: "n".into(), reply: None, files: Vec::new() });
        run(ChatCommand::PinMessage { channel_id: Id(1), message_id: Id(2) });
        run(ChatCommand::Forward { channel_id: Id(1), guild_id: None, message_id: Id(2), dest: Id(3), nonce: "n".into() });
        run(ChatCommand::PollVote { channel_id: Id(1), message_id: Id(2), answer_ids: vec![1], restore: vec![2] });
        run(ChatCommand::FetchPollVoters { channel_id: Id(1), message_id: Id(2), answer_id: 3, after: None });
        let poll = PollDraft { question: "Q?".into(), answers: vec!["a".into(), "b".into()], hours: 24, multi: false };
        run(ChatCommand::SendPoll { channel_id: Id(1), nonce: "n".into(), poll });
        let evs: Vec<BackendEvent> = rx.try_iter().collect();
        assert!(matches!(&evs[0], BackendEvent::Chat(ChatEvent::PinsLoaded { result: Err(_), .. })), "{evs:?}");
        assert!(matches!(&evs[1], BackendEvent::SendResult { result: Err(_), .. }));
        assert!(matches!(&evs[2], BackendEvent::ActionFailed(m) if m.contains("pin")));
        assert!(matches!(&evs[3], BackendEvent::Chat(ChatEvent::Forwarded { dest: Id(3), result: Err(_) })));
        assert!(matches!(&evs[4], BackendEvent::Chat(ChatEvent::PollVoteFailed { restore, .. }) if restore == &[2]));
        assert!(matches!(&evs[5], BackendEvent::Chat(ChatEvent::PollVoters { answer_id: 3, result: Err(_), .. })));
        assert!(matches!(&evs[6], BackendEvent::Chat(ChatEvent::PollPosted { channel_id: Id(1), result: Err(_) })));
        assert_eq!(evs.len(), 7, "typing and member search stay silent offline");
    }

    #[tokio::test]
    async fn account_commands_offline_answer_with_their_op() {
        let (tx, rx) = mpsc::channel();
        let sink = EventSink::new(tx, egui::Context::default());
        let mut session = None;
        let kick = AccountCommand::KickMember { guild_id: Id(1), user_id: Id(2), reason: None };
        let op = kick.op();
        assert_eq!(op, AccountOp { kind: OpKind::KickMember, target: Some(Id(2)) });
        handle_account(kick, &mut session, &sink, None);
        handle_account(AccountCommand::ResolveInvite { code: "abc".into() }, &mut session, &sink, None);
        handle_account(AccountCommand::SetPersistentStatus { status: UserStatus::Dnd }, &mut session, &sink, None);
        let pw = || Secret::new("pw".into());
        handle_account(AccountCommand::ChangePassword { password: pw(), new_password: pw() }, &mut session, &sink, None);
        handle_account(AccountCommand::FinishMfa { kind: crate::api::account::MfaKind::Totp, code: pw() }, &mut session, &sink, None);
        let evs: Vec<BackendEvent> = rx.try_iter().collect();
        assert!(
            matches!(&evs[0], BackendEvent::Account(AccountEvent::Done { op: o, result: Err(e) }) if *o == op && e.starts_with("Could not kick")),
            "{evs:?}"
        );
        assert!(matches!(&evs[1], BackendEvent::Account(AccountEvent::InviteResolved { code, result: Err(_) }) if code == "abc"));
        assert!(matches!(&evs[2], BackendEvent::Account(AccountEvent::Done { op: AccountOp { kind: OpKind::SetPersistentStatus, target: None }, .. })));
        assert!(matches!(&evs[3], BackendEvent::Account(AccountEvent::Done { op: AccountOp { kind: OpKind::ChangePassword, .. }, result: Err(e) }) if e.contains("password")));
        // No pending re-check offline: the prompt closes with a hint.
        assert!(matches!(&evs[4], BackendEvent::Account(AccountEvent::Mfa(None))));
        assert!(matches!(&evs[5], BackendEvent::ActionFailed(m) if m.contains("expired")));
        assert_eq!(evs.len(), 6);
    }

    #[test]
    fn gated_commands_keep_a_retry_copy() {
        let pw = Secret::new("pw".into());
        let gated = [
            AccountCommand::LogoutSessions { id_hashes: vec!["h".into()] },
            AccountCommand::RevokeAuthorizedApp { token_id: Id(3) },
            AccountCommand::ChangePassword { password: pw.clone(), new_password: pw.clone() },
            AccountCommand::DisableTotp,
            AccountCommand::SetSmsMfa { enable: true, password: pw.clone() },
            AccountCommand::DeleteSecurityKey { id: Id(4) },
            AccountCommand::UpdateUsername { username: "u".into(), password: pw },
        ];
        for c in gated {
            let copy = c.mfa_retry().expect("gated");
            assert_eq!(copy.op(), c.op());
        }
        assert!(AccountCommand::FetchSecurityKeys.mfa_retry().is_none());
        assert!(AccountCommand::KickMember { guild_id: Id(1), user_id: Id(2), reason: None }.mfa_retry().is_none());
    }

    #[test]
    fn token_swap_reaches_rest_and_gateway_and_the_keyring_only_when_remembered() {
        let props = Arc::new(ClientProps::new(&crate::config::Settings::default()));
        let old = Token::parse(&"o".repeat(30)).unwrap();
        let new = Token::parse(&"n".repeat(30)).unwrap();
        for remember in [false, true] {
            let api = ApiClient::new(old.clone(), props.clone());
            let (gw, mut gw_rx) = tmpsc::unbounded_channel();
            let mut saved = Vec::new();
            // The keyring is the only store `swap_token` can write to; nothing else is passed in.
            let r = swap_token(&api, &gw, remember, new.clone(), |t| {
                saved.push(t.clone());
                Ok(())
            });
            assert!(r.is_ok());
            assert_eq!(saved, if remember { vec![new.clone()] } else { vec![] }, "remember={remember}");
            assert!(matches!(gw_rx.try_recv(), Ok(GatewayCommand::SetToken(t)) if t == new));
            assert!(api.sends(&new), "REST uses the new token");
            assert!(!format!("{:?}", GatewayCommand::SetToken(new.clone())).contains("nnnn"), "Debug stays redacted");
        }
        let api = ApiClient::new(old, props);
        let (gw, _rx) = tmpsc::unbounded_channel();
        let err = swap_token(&api, &gw, true, new, |_| Err("store locked".into()));
        assert_eq!(err, Err("store locked".into()), "a keyring failure is reported, the swap still happens");
    }

    #[tokio::test]
    async fn quest_commands_offline_answer() {
        use crate::quests::{QuestCommand as C, QuestEvent as E};
        let (tx, rx) = mpsc::channel();
        let sink = EventSink::new(tx, egui::Context::default());
        let mut session = None;
        for c in [
            C::Fetch,
            C::Enroll { quest_id: Id(1), sealed: None },
            C::VideoProgress { quest_id: Id(1), timestamp: 10 },
            C::Claim { quest_id: Id(1), platform: 0, sealed: None },
            C::RewardCode { quest_id: Id(1) },
        ] {
            handle_quests(c, &mut session, &sink);
        }
        let evs: Vec<BackendEvent> = rx.try_iter().collect();
        assert!(matches!(&evs[0], BackendEvent::Quests(E::Loaded(Err(e))) if e == "Not connected"), "{evs:?}");
        assert!(matches!(&evs[1], BackendEvent::Quests(E::Failed { stop_video: false, .. })));
        assert!(matches!(&evs[2], BackendEvent::Quests(E::Failed { stop_video: true, .. })));
        assert!(matches!(&evs[3], BackendEvent::Quests(E::Failed { stop_video: false, .. })));
        assert!(matches!(&evs[4], BackendEvent::Quests(E::Code { code: Err(_), .. })));
        assert_eq!(evs.len(), 5, "the Orbs balance stays silent");
    }
}
