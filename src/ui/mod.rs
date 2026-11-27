//! egui front-end. `RustcordApp` owns all state; sub-modules render pieces of it through
//! a `UiCx` bundle of borrows.
//!
//! OWNERSHIP: ui-shell implementer (this file, login.rs, sidebar.rs, members.rs). `UiCx`,
//! `ViewState` fields, `Modal`, `Confirm`, the shared dialog / settings-layer helpers below and
//! the sub-module `pub fn` signatures are a shared contract between the ui implementers; add
//! fields freely, don't remove/rename. Which file owns which surface: docs/SOCIAL_UI_DESIGN.md.

pub mod aura;
pub mod autocomplete;
pub mod call;
pub mod chrome;
pub mod composer;
pub mod cosmetics;
pub mod crash_card;
pub mod dm_call;
pub mod forum;
pub mod forward;
pub mod frame_stats;
pub mod friends;
pub mod guild_card;
pub mod icons;
pub mod images;
pub mod inbox;
pub mod lock_screen;
mod login;
pub mod markdown;
pub mod members;
pub mod messages;
pub mod motion;
pub mod overlay;
pub mod pickers;
pub mod pins;
pub mod polls;
pub mod profile;
pub mod profile_modal;
pub mod quests;
pub mod requests;
pub mod screen_share;
pub mod search;
pub mod server_menu;
pub mod server_settings;
pub mod sidebar;
pub mod switcher;
pub mod threads;
pub mod update;
pub mod user_settings;
pub mod viewer;

use crate::auth::Token;
use crate::backend::{BackendEvent, BackendHandle, ChatEvent, Command, CommandSender, ConnState};
use crate::cache::Snapshot;
use crate::config::Settings;
use crate::model::{GatewayEvent, Id, Presence};
use crate::state::AppState;
use crate::theme::{Theme, ThemeApplier, ThemeColors};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Width of the server icon column.
pub const GUILD_BAR_W: f32 = 72.0;
/// Narrower windows hide the member list so the message area keeps ~450 px.
pub const MEMBER_LIST_MIN_WINDOW_W: f32 = 1000.0;
/// How long a toast stays before it fades out.
const TOAST_FOR: Duration = Duration::from_secs(5);
const MEMBER_LIST_W: f32 = 264.0;
/// Title bar above all columns.
pub(crate) const TITLE_BAR_H: f32 = 32.0;
/// Channel list width: default, and the range its resize handle allows (the sidebar, server
/// bar included, is 375 px and 264..=432 px wide).
const CHANNEL_LIST_W: f32 = 303.0;
const CHANNEL_LIST_MIN_W: f32 = 192.0;
const CHANNEL_LIST_MAX_W: f32 = 360.0;
/// The message area keeps this much width; the channel list gives way first.
const MESSAGES_MIN_W: f32 = 450.0;
/// Thread split panel (threads.rs) right of its parent's chat; narrower windows show it in the
/// chat's place.
pub const THREAD_PANEL_W: f32 = 400.0;
/// Minimum time between two read acks of the same channel.
const ACK_DEBOUNCE: Duration = Duration::from_secs(3);

/// Borrow bundle handed to every UI module.
pub struct UiCx<'a> {
    pub state: &'a mut AppState,
    pub view: &'a mut ViewState,
    pub images: &'a mut images::ImageCache,
    pub theme: &'a mut Theme,
    pub settings: &'a mut Settings,
    pub cmd: &'a CommandSender,
    /// Local-only banner/avatar (edit, then call `crate::overrides::publish`).
    pub overrides: &'a mut crate::overrides::LocalOverrides,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Login,
    Main,
}

pub struct Toast {
    pub text: String,
    pub error: bool,
    pub until: Instant,
}

#[derive(Debug, Clone, Copy)]
pub struct ProfileTarget {
    pub user_id: Id,
    pub guild_id: Option<Id>,
    /// Screen position to anchor the card at.
    pub anchor: egui::Pos2,
    /// Corner of the card placed at `anchor` (LEFT_BOTTOM grows the card upward).
    pub pivot: egui::Align2,
}

/// UI-only state (never persisted, never sent anywhere).
pub struct ViewState {
    pub screen: Screen,
    pub login_input: String,
    pub login_remember: bool,
    pub login_busy: bool,
    pub login_error: Option<String>,
    /// Discord's captcha the login card shows (`LoginStep::Captcha`: sitekey, rqdata); the login waits for it.
    pub login_captcha: Option<(String, Option<String>)>,
    /// Where the card drew it this pass (screen points) and whether all of it is in view;
    /// `captcha::Inline::sync` takes it.
    pub login_captcha_slot: Option<(egui::Rect, bool)>,
    /// Composer drafts per channel.
    pub drafts: HashMap<Id, String>,
    /// (channel, message, ping author)
    pub reply_to: Option<(Id, Id, bool)>,
    /// (channel, message) being edited inline.
    pub editing: Option<(Id, Id)>,
    pub edit_text: String,
    /// (message id, spoiler index) revealed by click. Attachments use index 1000+i.
    pub revealed_spoilers: HashSet<(Id, usize)>,
    pub toasts: Vec<Toast>,
    /// The crash report card (src/crash.rs, ui/crash_card.rs).
    pub crash: Option<crate::crash::Report>,
    pub settings_open: bool,
    pub profile_card: Option<ProfileTarget>,
    /// (channel, message) whose reaction picker is open.
    pub reaction_picker: Option<(Id, Id)>,
    /// Message pending delete confirmation (channel, message).
    pub confirm_delete: Option<(Id, Id)>,
    /// Set by settings when the theme changed and must be re-applied.
    pub theme_dirty: bool,
    pub window_focused: bool,
    /// Channel for which the last member-list subscription / history fetch was issued.
    pub(crate) last_selected: Option<Id>,
    pub(crate) last_ack: HashMap<Id, Instant>,
    /// Request to scroll the message list to the bottom on next frame.
    pub scroll_to_bottom: bool,
    /// Message to scroll to (jump to reply).
    pub scroll_to_message: Option<Id>,
    /// Channel categories collapsed in the sidebar (kept for the session).
    pub collapsed_categories: HashSet<Id>,
    /// Login screen: show the token in clear text.
    pub login_show_token: bool,
    /// Login screen: active tab, QR login and email/2FA state (secret buffers wiped on drop).
    pub login_form: login::LoginForm,
    /// Channel whose member list (op 37) was subscribed in the current gateway session.
    pub(crate) member_list_sub: Option<Id>,
    /// Read acks waiting for the per-channel debounce (channel -> message).
    pub(crate) pending_acks: HashMap<Id, Id>,
    /// When `AppState::conn` last changed (reconnect countdown).
    pub(crate) conn_since: Instant,
    /// Themes listed on the settings page; loaded when it opens, dropped when it closes.
    pub(crate) settings_themes: Option<Vec<Theme>>,
    /// The blurred, frozen picture of the app behind the settings panel (§7.2); dropped when
    /// settings close or the theme changes.
    pub(crate) settings_backdrop: Option<egui::TextureHandle>,
    /// The picture was asked for in this opening of settings (reset when they close).
    pub(crate) backdrop_asked: bool,
    /// Where the screenshot callback leaves the small blurred picture for the next frame.
    pub(crate) backdrop_slot: Arc<std::sync::Mutex<Option<egui::ColorImage>>>,
    /// DMs closed in the sidebar (channel -> its last message then); local only, a newer
    /// message or opening the DM lists it again.
    pub hidden_dms: HashMap<Id, Option<Id>>,
    /// Text of the DM list's filter field.
    pub dm_filter: String,
    /// User panel mute / deafen switches (sent to the voice module on change).
    pub self_mute: bool,
    pub self_deaf: bool,
    /// Channel list width set with its resize handle (None = automatic). Not persisted yet.
    pub channel_list_w: Option<f32>,
    /// Guilds whose muted channels are hidden in the sidebar ("Hide Muted Channels").
    pub hide_muted: HashSet<Id>,
    /// User settings layer (full screen while `settings_open`): the page shown.
    pub settings_page: user_settings::Page,
    /// Server settings layer (full screen): guild and page. `Page::Channel` is a channel's
    /// settings (Edit Channel).
    pub server_settings: Option<(Id, server_settings::Page)>,
    /// The one open modal (`show_modals` draws it); `confirm` stacks above it.
    pub modal: Option<Modal>,
    /// Yes/no question before a destructive action (`ViewState::confirm`), above any modal.
    pub confirm: Option<Confirm>,
    /// Thread shown in the split panel right of its parent's chat (threads.rs). The shell
    /// loads its history, acks it and closes it once its parent is no longer the open channel.
    pub thread_panel: Option<Id>,
    /// Inbox popout open (inbox.rs, Ctrl+I).
    pub inbox_open: bool,
    /// Focus mode (Keybinds): the server bar and channel list are folded away and the chat is centred. Session only.
    pub focus_mode: bool,
    /// The "Focus mode is on" toast was shown this session.
    pub focus_hint_shown: bool,
    // Module-private UI state: one struct per module, defined and owned by that module's file
    // (add fields there, not here). All of it is dropped on logout with the rest of the view.
    pub friends_ui: friends::FriendsUi,
    pub inbox_ui: inbox::InboxUi,
    pub threads_ui: threads::ThreadsUi,
    pub forum_ui: forum::ForumUi,
    pub forward_ui: forward::ForwardUi,
    pub polls_ui: polls::PollsUi,
    pub profile_modal_ui: profile_modal::ProfileModalUi,
    pub server_menu_ui: server_menu::ServerMenuUi,
    pub server_settings_ui: server_settings::ServerSettingsUi,
    pub user_settings_ui: user_settings::UserSettingsUi,
    pub call_ui: call::CallUi,
    pub quests_ui: quests::QuestsUi,
    pub requests_ui: requests::RequestsUi,
    pub media_ui: viewer::MediaUi,
    pub switcher_ui: switcher::SwitcherUi,
}

/// A modal dialog (`ViewState::modal`, drawn by `show_modals` through the owning module).
/// Draft text and other form state live in that module's `*Ui` struct; the variant only names
/// the target. A new variant is a contract change (mod.rs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modal {
    /// threads.rs: Create Thread from a message (`from_message`) or the header.
    CreateThread { channel_id: Id, from_message: Option<Id> },
    /// forum.rs: New Post in a forum / media channel.
    NewForumPost { forum_id: Id },
    /// profile_modal.rs: the full profile (View Full Profile).
    Profile { user_id: Id, guild_id: Option<Id> },
    /// friends.rs: New Group DM picker, or Add Friends to DM for `channel_id`.
    GroupDm { channel_id: Option<Id> },
    /// server_menu.rs: Create Channel / Create Category (`category`) under `parent_id`.
    CreateChannel { guild_id: Id, parent_id: Option<Id>, category: bool },
    /// server_menu.rs: Invite People (link for `channel_id`).
    Invite { guild_id: Id, channel_id: Id },
    /// server_menu.rs: Add a Server (join by invite link / create my own).
    JoinServer,
    /// server_menu.rs: kick or ban with a reason (and the ban's delete-messages preset).
    Moderate { guild_id: Id, user_id: Id, ban: bool },
    /// user_settings.rs: Set Custom Status.
    CustomStatus,
    /// server_menu.rs: a server's Privacy Settings (DMs, message requests, activity).
    PrivacySettings { guild_id: Id },
    /// screen_share.rs: the screen share picker (Go Live).
    ScreenShare,
    /// server_menu.rs: apply to join a server (its questions), or accept its rules after a join.
    JoinApplication { guild_id: Id },
    /// quests.rs: the quest video (its progress counts while it plays there).
    QuestVideo { quest_id: Id },
    /// quests.rs: the platform a quest reward is claimed for.
    QuestClaim { quest_id: Id },
    /// viewer.rs: the media lightbox (its items live in `ViewState::media_ui`).
    Media,
    /// forward.rs: forward a message to one channel or DM.
    Forward { channel_id: Id, message_id: Id },
    /// inbox.rs: the Custom reminder for a message ("Remind me").
    Remind { channel_id: Id, message_id: Id },
    /// polls.rs: create a poll in `channel_id` (its draft lives in `ViewState::polls_ui`).
    CreatePoll { channel_id: Id },
    /// switcher.rs: the quick switcher (Ctrl+K).
    QuickSwitcher,
}

/// A pending confirmation: `show_modals` asks, `then` goes out on the danger button.
pub struct Confirm {
    pub title: String,
    pub body: String,
    /// Label of the confirm button ("Leave Server", "Block").
    pub button: String,
    /// Danger colour (destructive); false = accent (e.g. accepting a stranger's request).
    pub danger: bool,
    pub then: Command,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            screen: Screen::Login,
            login_input: String::new(),
            login_remember: true,
            login_busy: false,
            login_error: None,
            login_captcha: None,
            login_captcha_slot: None,
            drafts: HashMap::new(),
            reply_to: None,
            editing: None,
            edit_text: String::new(),
            revealed_spoilers: HashSet::new(),
            toasts: Vec::new(),
            crash: None,
            settings_open: false,
            profile_card: None,
            reaction_picker: None,
            confirm_delete: None,
            theme_dirty: false,
            window_focused: true,
            last_selected: None,
            last_ack: HashMap::new(),
            scroll_to_bottom: true,
            scroll_to_message: None,
            collapsed_categories: HashSet::new(),
            login_show_token: false,
            login_form: login::LoginForm::default(),
            member_list_sub: None,
            pending_acks: HashMap::new(),
            conn_since: Instant::now(),
            settings_themes: None,
            settings_backdrop: None,
            backdrop_asked: false,
            backdrop_slot: Arc::default(),
            hidden_dms: HashMap::new(),
            dm_filter: String::new(),
            self_mute: false,
            self_deaf: false,
            channel_list_w: None,
            hide_muted: HashSet::new(),
            settings_page: user_settings::Page::default(),
            server_settings: None,
            modal: None,
            confirm: None,
            thread_panel: None,
            inbox_open: false,
            focus_mode: false,
            focus_hint_shown: false,
            friends_ui: Default::default(),
            inbox_ui: Default::default(),
            threads_ui: Default::default(),
            forum_ui: Default::default(),
            forward_ui: Default::default(),
            polls_ui: Default::default(),
            profile_modal_ui: Default::default(),
            server_menu_ui: Default::default(),
            server_settings_ui: Default::default(),
            user_settings_ui: Default::default(),
            call_ui: Default::default(),
            quests_ui: Default::default(),
            requests_ui: Default::default(),
            media_ui: Default::default(),
            switcher_ui: Default::default(),
        }
    }
}

impl ViewState {
    pub fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toasts.push(Toast { text: text.into(), error, until: Instant::now() + TOAST_FOR });
    }

    /// Open the profile card for a user (clicking the same user again closes it).
    /// Use this from every avatar / name / mention click.
    pub fn open_profile(&mut self, user_id: Id, guild_id: Option<Id>, anchor: egui::Pos2) {
        if self.profile_card.is_some_and(|p| p.user_id == user_id) {
            self.profile_card = None;
        } else {
            self.profile_card = Some(ProfileTarget { user_id, guild_id, anchor, pivot: egui::Align2::LEFT_TOP });
        }
    }

    /// `open_profile` for a card that grows upward from `anchor` (its bottom-left corner).
    pub fn open_profile_above(&mut self, user_id: Id, guild_id: Option<Id>, anchor: egui::Pos2) {
        self.open_profile(user_id, guild_id, anchor);
        if let Some(p) = &mut self.profile_card {
            p.pivot = egui::Align2::LEFT_BOTTOM;
        }
    }

    /// Ask before sending `then` (leave server, delete channel, kick, block, …): a dialog with
    /// `title`, `body`, Cancel and a danger-coloured `button`.
    pub fn confirm(&mut self, title: impl Into<String>, body: impl Into<String>, button: impl Into<String>, then: Command) {
        self.confirm = Some(Confirm { title: title.into(), body: body.into(), button: button.into(), danger: true, then });
    }

    /// A modal or confirmation is open: Esc and clicks belong to it, not to the page below.
    pub fn dialog_open(&self) -> bool {
        self.modal.is_some() || self.confirm.is_some()
    }

    /// A full-screen settings layer hides the chat (no read acks, no member list).
    pub fn chat_hidden(&self) -> bool {
        self.settings_open || self.server_settings.is_some() || crate::lock::locked()
    }
}

pub struct RustcordApp {
    rt: Option<tokio::runtime::Runtime>,
    backend: BackendHandle,
    state: AppState,
    view: ViewState,
    images: images::ImageCache,
    theme: Theme,
    theme_applier: ThemeApplier,
    settings: Settings,
    overrides: crate::overrides::LocalOverrides,
    /// My user id and premium type the overrides were last published for.
    overrides_me: Option<(Id, u8)>,
    started: Instant,
    frame_no: u64,
    last_cache_save: Instant,
    /// Backend events were applied since the last shown pass (see `settle_after_events`).
    event_pass: bool,
    /// A key or pointer button was pressed since start (the updater's start screen waits for none).
    interacted: bool,
    /// The in-game overlay (Windows; a stub elsewhere): `overlay/live.rs`.
    overlay: overlay::Overlay,
    /// Discord's captcha inside the login card (Windows, macOS; a stub elsewhere): `src/captcha.rs`.
    captcha: crate::captcha::Inline,
    /// Voice, mute and stream sounds come from what changed between two passes.
    cues: crate::sounds::Cues,
}

impl RustcordApp {
    pub fn new(
        ctx: &egui::Context,
        rt: tokio::runtime::Runtime,
        backend: BackendHandle,
        settings: Settings,
        snapshot: Option<Snapshot>,
        token: Option<Token>,
        started: Instant,
    ) -> Self {
        let theme = Theme::find(&settings.theme);
        crate::theme::init_fonts(ctx, settings.use_discord_font);
        let mut theme_applier = ThemeApplier::default();
        theme_applier.apply(ctx, &theme);

        let state = snapshot.map(AppState::from_snapshot).unwrap_or_default();
        let mut view = ViewState { crash: crate::crash::pending(), ..Default::default() };
        let offline = crate::config::offline_mode();
        user_settings::restore_voice(&settings, &backend.cmd);
        if let Some(token) = token {
            // Cached UI immediately; connect in the background.
            view.screen = Screen::Main;
            backend.cmd.send(Command::Resume { token });
        } else if offline && state.has_data {
            view.screen = Screen::Main;
        }
        // Test hook: RUSTCORD_LOGIN_TAB=qr|email|token|mfa opens that login tab (before the first
        // frame, so the QR tab does not start a login when another tab is wanted).
        if let Ok(tab) = std::env::var("RUSTCORD_LOGIN_TAB") {
            login::open_test_tab(&mut view, &tab);
        }

        let images = images::ImageCache::new(backend.cmd.clone());
        crate::plugins::apply_env();
        Self {
            rt: Some(rt),
            backend,
            state,
            view,
            images,
            theme,
            theme_applier,
            settings,
            overrides: crate::overrides::LocalOverrides::load(),
            overrides_me: None,
            started,
            frame_no: 0,
            overlay: Default::default(),
            captcha: Default::default(),
            cues: Default::default(),
            last_cache_save: Instant::now(),
            event_pass: false,
            interacted: false,
        }
    }

    /// Voice, mute, push-to-talk and stream sounds (changes since the last pass) and the call
    /// rings (recomputed from state). Plays nothing outside the main screen; no repaint.
    fn sound_cues(&mut self) {
        let main = self.view.screen == Screen::Main;
        let deaf = crate::sounds::deafened(&self.state, self.view.self_deaf);
        let dnd = crate::sounds::dnd(&self.state);
        if main {
            let snap = crate::sounds::Snap::of(&self.state, self.view.self_mute, self.view.self_deaf, self.settings.push_to_talk);
            if let Some(sound) = self.cues.step(snap, std::time::Instant::now()) {
                crate::sounds::play(sound, &self.settings, deaf, dnd);
            }
        } else {
            self.cues = Default::default();
        }
        let ring = main.then(|| crate::sounds::ring(&self.state)).flatten();
        // Quiet hours silence the incoming ring only; the popup stays. The clock is read only while one rings.
        let quiet = matches!(ring, Some(crate::sounds::Sound::IncomingRing)) && self.settings.quiet_hours.quiet_now();
        crate::sounds::set_ring(ring, self.state.incoming_call(), &self.settings, deaf, dnd || quiet);
    }

    fn drain_events(&mut self, ctx: &egui::Context) {
        let focused_channel = if self.view.window_focused && !self.view.chat_hidden() {
            self.state.selected_channel
        } else {
            None
        };
        while let Ok(ev) = self.backend.events.try_recv() {
            self.event_pass = true;
            match ev {
                BackendEvent::Conn(c) => {
                    if c != self.state.conn {
                        self.view.conn_since = Instant::now();
                    }
                    self.state.conn = c;
                }
                BackendEvent::LoggedIn => {
                    login::on_logged_in(ctx, &mut self.view, &self.backend.cmd);
                    self.view.screen = Screen::Main;
                }
                BackendEvent::LoginStep(step) => login::on_step(&mut self.view, step),
                BackendEvent::AuthFailed(msg) => {
                    self.state.reset();
                    self.images.clear();
                    self.view = ViewState { login_error: Some(msg), ..ViewState::default() };
                    // Drops the session and deletes the rejected token from the keyring.
                    self.backend.cmd.send(Command::Logout);
                    std::thread::spawn(crate::cache::clear_all);
                }
                BackendEvent::Gateway(ev) => {
                    if matches!(ev, GatewayEvent::Ready(_)) {
                        // New session: op 37 subscriptions only survive a RESUME.
                        self.view.member_list_sub = None;
                    }
                    // Plugins (MessageLogger, RelationshipNotifier) see it first; may swallow it.
                    let Some(ev) = crate::plugins::on_gateway(&mut self.state, &mut self.view, &self.settings, ev) else { continue };
                    // Keyword alerts see the message before the state takes it (crate::alerts).
                    let keyword = crate::alerts::on_gateway(&mut self.view.inbox_ui.keyword_hits, &self.settings.keyword_alerts, &self.state, &ev, focused_channel);
                    let jump = keyword.as_ref().map(|(n, id)| (n.channel_id, *id));
                    // Do Not Disturb and quiet hours: unread state still updates, but no toast, sound or taskbar flash.
                    // Discord's rules first; a keyword alert speaks where they stay silent (muted, mentions only).
                    if let Some(n) = self.state.apply_gateway(ev, focused_channel).or(keyword.map(|k| k.0)) {
                        // Plugins (OnePingPerDM): one per DM until it is read.
                        let silenced = crate::plugins::notifier::silenced_by(
                            self.state.chat.my_status == Some(crate::model::UserStatus::Dnd),
                            || self.settings.quiet_hours.quiet_now(),
                            || crate::plugins::notifier::one_ping(&self.state, n.channel_id),
                        );
                        // Plugins (NotificationLog): the Inbox's Recent tab also lists the silenced ones.
                        if let Some(e) = crate::plugins::notifier::entry(&n, &self.settings, silenced) {
                            crate::plugins::notifier::log(&mut self.view.inbox_ui.recent, e);
                        }
                        if silenced.is_some() {
                            continue;
                        }
                        // A ringing call (no author) rings from `sound_cues` instead; my own messages never get here.
                        if n.author_id.is_some() {
                            let deaf = crate::sounds::deafened(&self.state, self.view.self_deaf);
                            let sound = if n.mentioned { crate::sounds::Sound::Mention } else { crate::sounds::Sound::Message };
                            crate::sounds::play_in(sound, &self.settings, deaf, false, &crate::sounds::places(&self.state, n.channel_id));
                        }
                        // Off, or Streamer Mode: none, or without sender and text.
                        if let Some((title, body)) = self.settings.notification(&n.title, &n.body) {
                            // A game is in front: the overlay shows it instead of the desktop notification.
                            let author = n.author_id.filter(|_| !self.settings.hides_sender());
                            if !self.overlay.notify(&self.state, &self.settings, title, body, author) {
                                crate::notify::show(title, body);
                                self.view.quests_ui.notified_last = false;
                                self.view.inbox_ui.notified = jump;
                            }
                        }
                        if !self.view.window_focused && self.settings.flash_taskbar {
                            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                                egui::UserAttentionType::Informational,
                            ));
                        }
                    }
                }
                BackendEvent::MessagesLoaded { channel_id, before, result } => {
                    if let Err(e) = &result {
                        self.view.toast(format!("Could not load messages: {e}"), true);
                    }
                    self.state.apply_messages_loaded(channel_id, before, result, messages::PAGE_SIZE);
                }
                BackendEvent::SendResult { channel_id, nonce, result } => {
                    if let Err(e) = &result {
                        self.view.toast(format!("Message not sent: {e}"), true);
                    }
                    self.state.apply_send_result(channel_id, &nonce, result);
                }
                // A login that fails for a non-auth reason (network) must not spin forever.
                BackendEvent::ActionFailed(e) if self.view.screen == Screen::Login && self.view.login_busy => {
                    self.view.login_busy = false;
                    self.view.login_error = Some(e);
                }
                BackendEvent::ActionFailed(e) => self.view.toast(e, true),
                BackendEvent::ImageLoaded { url, result } => self.images.on_loaded(ctx, url, result),
                // Voice status lives in AppState for the voice panel; failures become toasts.
                BackendEvent::Voice(ev) => {
                    if let Some(msg) = self.state.apply_voice(ev) {
                        self.view.toast(msg, true);
                    }
                }
                // The Forward and Create poll dialogs answer their own requests.
                BackendEvent::Chat(ChatEvent::Forwarded { dest, result }) => forward::answered(&mut self.view, &self.state, dest, result),
                BackendEvent::Chat(ChatEvent::PollPosted { channel_id, result }) => polls::posted(&mut self.view, channel_id, result),
                // Chat essentials (pins, search, GIFs, uploads…) live in AppState::chat.
                BackendEvent::Chat(ev) => {
                    // Plugins (MessageLinkEmbeds) take the answers to their own fetches.
                    let Some(ev) = crate::plugins::on_chat(&self.state, ev) else { continue };
                    if let Some(msg) = self.state.apply_chat(ev) {
                        self.view.toast(msg, true);
                    }
                }
                // Threads, friends, DMs, inbox, profiles live in AppState::social.
                BackendEvent::Social(ev) => {
                    if let Some(msg) = self.state.apply_social(ev) {
                        self.view.toast(msg, true);
                    }
                }
                // Settings proto, profile, server admin, invites live in AppState::account.
                BackendEvent::Account(ev) => {
                    if let Some(msg) = self.state.apply_account(ev) {
                        self.view.toast(msg, true);
                    }
                }
                // Game detection, added games, app icons, button links (crate::activity).
                BackendEvent::Activity(ev) => {
                    if let Some(url) = self.state.activity.apply(ev).filter(|u| markdown::is_web_scheme(u)) {
                        ctx.open_url(egui::OpenUrl::new_tab(url));
                    }
                }
                // Games on the local pipe, application names (crate::rpc).
                BackendEvent::Rpc(ev) => self.state.rpc.apply(ev),
                // Quests page; a video progress answer goes on to the video modal's session.
                BackendEvent::Quests(ev) => {
                    if let Some((quest_id, ok)) = self.state.quests.apply(ev) {
                        quests::on_video_answer(&mut self.view, quest_id, ok);
                    }
                }
            }
        }
    }

    /// An event wakes one pass (`EventSink::send`), but hover, the chat's stick-to-bottom and
    /// area sizes follow the widget rects of the pass before: one more pass, as egui's
    /// `request_repaint` used to give every event, only when this one moved a widget.
    fn settle_after_events(&mut self, ctx: &egui::Context) {
        if !self.event_pass || ctx.will_discard() {
            return;
        }
        self.event_pass = false;
        if !ctx.viewport(|vp| same_widgets(&vp.prev_pass.widgets, &vp.this_pass.widgets)) {
            ctx.request_repaint_after(Duration::from_millis(1));
        }
    }

    /// Side effects of the current selection: history fetch, member list, read acks.
    fn selection_effects(&mut self, ctx: &egui::Context) {
        let online = self.state.conn == ConnState::Connected;
        if let Some(ch) = self.state.selected_channel {
            if self.view.last_selected != Some(ch) {
                self.view.last_selected = Some(ch);
                self.view.scroll_to_bottom = true;
                self.view.scroll_to_message = None;
                self.view.reaction_picker = None;
                self.view.confirm_delete = None;
                self.view.profile_card = None;
            }
            // Always subscribed, even with the member list hidden: op 37 also sets typing:true,
            // without which large guilds send no live messages/typing. Tracked separately from
            // `last_selected` so it is (re)sent once we are online or after a new session.
            if let Some(guild_id) = self.state.channels.get(&ch).and_then(|c| c.guild_id)
                && online && self.view.member_list_sub != Some(ch)
            {
                self.view.member_list_sub = Some(ch);
                self.backend.cmd.send(Command::SubscribeMemberList { guild_id, channel_id: ch });
            }
        }
        // The thread split panel belongs to its parent: it closes when another channel opens.
        if let Some(t) = self.view.thread_panel
            && self.state.channels.get(&t).and_then(|c| c.parent_id).is_none_or(|p| self.state.selected_channel != Some(p))
        {
            self.view.thread_panel = None;
        }
        // History and read acks for what is on screen: the open channel and the thread panel.
        for ch in [self.state.selected_channel, self.view.thread_panel].into_iter().flatten() {
            // Forum / media channels have no message history (forum.rs lists their posts).
            let chat = self.state.channels.get(&ch).is_some_and(crate::model::Channel::is_text_chat);
            if online && chat && !self.state.fresh_channels.contains(&ch) && self.state.can_view(ch) {
                let entry = self.state.messages.entry(ch).or_default();
                if !entry.loading {
                    entry.loading = true;
                    self.state.fresh_channels.insert(ch);
                    self.backend.cmd.send(Command::FetchMessages { channel_id: ch, before: None, limit: messages::PAGE_SIZE });
                }
            }
            // No is_unread gate: muted channels are acked on view too; mark_read dedups.
            if self.view.window_focused && !self.view.chat_hidden()
                && let Some(mid) = self.state.mark_read(ch)
                && self.settings.send_read_acks
            {
                self.view.pending_acks.insert(ch, mid);
            }
        }
        self.flush_acks(ctx, online);
    }

    /// Sends queued read acks, at most one per channel every `ACK_DEBOUNCE`. Debounced acks
    /// stay queued (instead of being dropped) so the newest read position always reaches
    /// the server.
    fn flush_acks(&mut self, ctx: &egui::Context, online: bool) {
        let ViewState { pending_acks, last_ack, .. } = &mut self.view;
        if !self.settings.send_read_acks {
            pending_acks.clear();
            return;
        }
        if !online || pending_acks.is_empty() {
            return;
        }
        let now = Instant::now();
        let (cmd, channels) = (&self.backend.cmd, &self.state.channels);
        pending_acks.retain(|&channel_id, &mut message_id| {
            if last_ack.get(&channel_id).is_some_and(|t| now.duration_since(*t) < ACK_DEBOUNCE) {
                return true;
            }
            last_ack.insert(channel_id, now);
            // Threads ack with read-state flags 3 (FEATURES_NOTES §2.1).
            let thread = channels.get(&channel_id).is_some_and(crate::model::Channel::is_thread);
            cmd.send(if thread { Command::AckThread { channel_id, message_id } } else { Command::Ack { channel_id, message_id } });
            false
        });
        if !pending_acks.is_empty() {
            ctx.request_repaint_after(ACK_DEBOUNCE);
        }
    }

    fn maybe_save_cache(&mut self, force: bool) {
        if !self.state.has_data || crate::config::offline_mode() {
            return;
        }
        // Most churn (last_message_id, read drift) is rebuilt by READY anyway, and on_exit
        // saves synchronously, so a long interval keeps snapshot cost and disk writes low.
        if force || (self.state.dirty && self.last_cache_save.elapsed() > Duration::from_secs(300)) {
            self.state.dirty = false;
            self.last_cache_save = Instant::now();
            let snap = self.state.to_snapshot();
            let write = move || {
                if let Err(e) = crate::cache::save_snapshot(&snap) {
                    eprintln!("rustcord: cache save failed: {e}");
                }
            };
            if force { write() } else { std::thread::spawn(write); }
        }
    }

    fn show_toasts(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        // A toast rises and fades in over BASE, stays TOAST_FOR, then fades out over BASE x OUT (§7).
        let (enter, leave) = (motion::secs(ctx, motion::BASE), motion::secs(ctx, motion::BASE * motion::OUT));
        self.view.toasts.retain(|t| t.until + Duration::from_secs_f32(leave) > now);
        if self.view.toasts.is_empty() {
            return;
        }
        let mut moving = false;
        let c = &self.theme.colors;
        // Quiet cards (popup style); errors are marked by a small danger dot, not a red fill.
        let card = egui::Frame::popup(&ctx.global_style())
            .fill(c.surface.to_color32())
            .corner_radius(self.theme.layout.corner_radius * 2.0)
            .inner_margin(egui::Margin { left: 12, right: 16, top: 8, bottom: 8 });
        let (toast_r, tk) = (self.theme.layout.corner_radius * 2.0, self.theme.tokens());
        // Inside the message area, not straddling the member list's (or thread panel's) edge.
        let right_column = match self.view.screen {
            Screen::Main => right_column_w(&self.view, &self.settings, &self.state, ctx.content_rect().width()),
            Screen::Login => 0.0,
        };
        let right = right_column + 16.0;
        // 8 px above the composer's top when it is on screen this pass, else 80 px from the bottom.
        let pass = ctx.cumulative_pass_nr();
        let above = ctx.data(|d| d.get_temp::<(f32, u64)>(egui::Id::unique("rc_composer_top"))).filter(|c| c.1 == pass).map(|c| ctx.content_rect().bottom() - c.0 + 8.0);
        egui::Area::new(egui::Id::unique("toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-right, -above.unwrap_or(80.0)))
            .order(egui::Order::Foreground)
            .interactable(false)
            .fade_in(false)
            .show(ctx, |ui| {
                motion::own(ctx, ui.layer_id());
                ui.set_max_width(360.0);
                ui.spacing_mut().item_spacing.y = 8.0;
                for t in &self.view.toasts {
                    let dot = if t.error { c.danger } else { c.text_muted }.to_color32();
                    let age = (now + TOAST_FOR).saturating_duration_since(t.until).as_secs_f32();
                    let gone = now.saturating_duration_since(t.until).as_secs_f32();
                    let a = if enter > 0.0 { motion::ease((age / enter).min(1.0)) } else { 1.0 };
                    let b = if leave > 0.0 { 1.0 - egui::emath::easing::cubic_in((gone / leave).min(1.0)) } else { 1.0 };
                    moving |= a < 1.0 || gone > 0.0;
                    ui.scope(|ui| {
                        ui.multiply_opacity(a * b);
                        ui.with_visual_transform(egui::emath::TSTransform::from_translation(egui::vec2(0.0, 8.0 * (1.0 - a))), |ui| {
                            let out = card.show(ui, |ui| {
                                ui.horizontal_top(|ui| {
                                    ui.spacing_mut().item_spacing.x = 8.0;
                                    let h = ui.text_style_height(&egui::TextStyle::Body);
                                    let (r, _) = ui.allocate_exact_size(egui::vec2(8.0, h), egui::Sense::hover());
                                    ui.painter().circle_filled(r.center(), 3.5, dot);
                                    ui.add(egui::Label::new(egui::RichText::new(&t.text).color(c.text.to_color32())).wrap());
                                });
                            });
                            messages::sheen(ui.painter(), out.response.rect, toast_r, &tk);
                        });
                    });
                }
            });
        if moving {
            ctx.request_repaint();
        } else if let Some(next) = self.view.toasts.iter().map(|t| t.until).filter(|u| *u > now).min() {
            ctx.request_repaint_after(next - now);
        }
    }

    fn startup_hooks(&mut self, ctx: &egui::Context) {
        self.frame_no += 1;
        if self.frame_no == 1 {
            crate::trace_startup("first_frame", self.started);
            // Test hook: RUSTCORD_WINDOW_SIZE=<w>x<h> resizes the window (layout screenshots).
            if let Some(size) = std::env::var("RUSTCORD_WINDOW_SIZE").ok().as_deref().and_then(parse_size) {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
            }
            // Test hook: RUSTCORD_OPEN=settings|profile|toast|members|muted|deafened|channel:<id>|voice:<id>|status:<s>
            // opens the settings page / my account popout (profile card + status picker) / a sample error
            // toast / a demo member list for the open guild (cached users, two in a role group, mixed
            // presence with a custom status, a game and a phone) / mutes / deafens / that channel (DM ids
            // show the DM list) / shows me connected (and speaking) in that voice channel, without
            // joining / sets my status (online|idle|dnd|invisible) with a sample custom status. A comma
            // list combines them, applied in order (e.g. `channel:<id>,toast`).
            // Social / server UI (docs/SOCIAL_UI_DESIGN.md): `settings:<page>` (user settings page),
            // `server-settings[:<page>]` (open guild), `friends` (Friends page with demo
            // relationships from cached users), `inbox`, `threads` (Threads popout), `thread:<id>`
            // (thread panel; selects its parent), `card:<user id>` (that user's profile card),
            // `modal:<name>` (`demo_modal`), `confirm` (Leave Server question for the open guild).
            // DM calls (dm_call.rs `demo`): `call:<dm id>` (a call going on there), `ring:<dm id>` (it rings me).
            // `switcher[:<query>]` (the quick switcher with that text typed in; no comma in it).
            // None of them sends a request.
            if sidebar::nav_demo("keywords") {
                inbox::demo(&self.state, &mut self.view, &mut self.settings);
            }
            if sidebar::nav_demo("recent") {
                inbox::demo_recent(&self.state, &mut self.view);
            }
            let open = std::env::var("RUSTCORD_OPEN").unwrap_or_default();
            for open in open.split(',') {
                match open.trim() {
                    "settings" => self.view.settings_open = true,
                    s if let Some(page) = s.strip_prefix("settings:").and_then(user_settings::Page::from_name) => {
                        (self.view.settings_open, self.view.settings_page) = (true, page);
                    }
                    s if let Some(q) = s.strip_prefix("settings-search:") => {
                        self.view.settings_open = true;
                        user_settings::demo_search(ctx, q);
                    }
                    s if let Some(page) = s.strip_prefix("server-settings") && let Some(g) = self.state.selected_guild => {
                        let page = server_settings::Page::from_name(page.trim_start_matches(':')).unwrap_or_default();
                        self.view.server_settings = Some((g, page));
                    }
                    s if let Some(q) = s.strip_prefix("switcher").map(|q| q.trim_start_matches(':')) => switcher::open(&mut self.view, q),
                    "friends" => friends::demo(&mut self.state),
                    "inbox" => self.view.inbox_open = true,
                    "inbox:saved" => (self.view.inbox_open, self.view.inbox_ui.tab) = (true, inbox::Tab::Saved),
                    "threads" => egui::Popup::open_id(ctx, threads::popup_id()),
                    s if let Some(t) = s.strip_prefix("thread:").and_then(|id| id.parse().ok()).map(Id) => {
                        if let Some(parent) = self.state.channels.get(&t).and_then(|c| c.parent_id) {
                            self.state.select_channel(parent);
                        }
                        self.view.thread_panel = Some(t);
                    }
                    s if let Some(u) = s.strip_prefix("card:").and_then(|id| id.parse().ok()).map(Id) => {
                        self.view.open_profile(u, self.state.selected_guild, egui::pos2(GUILD_BAR_W + CHANNEL_LIST_W + 16.0, 120.0));
                    }
                    s if let Some(name) = s.strip_prefix("modal:") => self.view.modal = demo_modal(&self.state, name),
                    "confirm" if let Some(g) = self.state.selected_guild => server_menu::ask_leave(&mut self.view, &self.state, g),
                    "profile" if let Some(me) = &self.state.me => {
                        // Where the user panel avatar opens it: 8 px above the floating panel.
                        let top = ctx.content_rect().bottom() - 8.0 - sidebar::user_panel_height(&self.state, &self.view, &self.theme);
                        self.view.open_profile_above(me.id, self.state.selected_guild, egui::pos2(8.0, top - 8.0));
                    }
                    s if let Some(id) = s.strip_prefix("voice:").and_then(|id| id.parse().ok()) => {
                        let (id, me) = (Id(id), self.state.me.as_ref().map(|u| u.id));
                        let v = &mut self.state.voice;
                        (v.channel_id, v.connected) = (Some(id), true);
                        v.speaking.extend(me);
                        // Listed under the channel too, as the gateway would report it.
                        if let Some(me) = me {
                            let guild_id = self.state.channels.get(&id).and_then(|c| c.guild_id);
                            let vs = crate::model::VoiceState { guild_id, channel_id: Some(id), user_id: me, ..Default::default() };
                            self.state.voice_states.entry(id).or_default().insert(me, vs);
                        }
                    }
                    "toast" => self.view.toast("Message not sent: the connection was lost", true),
                    "muted" => {
                        self.view.self_mute = true;
                        self.backend.cmd.send(Command::SetSelfMute(true));
                    }
                    "deafened" => {
                        self.view.self_deaf = true;
                        self.backend.cmd.send(Command::SetSelfDeaf(true));
                    }
                    "members" if let Some(g) = self.state.selected_guild => {
                        use crate::model::{Member, MemberListItem};
                        let mut users: Vec<_> = self.state.me.iter().chain(self.state.users.values()).cloned().collect();
                        users.sort_by_key(|u| u.id.0);
                        users.dedup_by_key(|u| u.id.0);
                        let (online, offline) = users.split_at(users.len().min(4));
                        // The first two online users form a group of the guild's first hoisted role.
                        let role = self.state.guild(g).and_then(|g| g.roles.iter().find(|r| r.hoist)).map(|r| r.id);
                        let role_group = role.map(|r| r.0.to_string()).unwrap_or_default();
                        let (staff, online) = online.split_at(if role.is_some() { online.len().min(2) } else { 0 });
                        let mut presence = [Presence::Online, Presence::Idle, Presence::Dnd, Presence::Online].into_iter();
                        let mut items = Vec::new();
                        for (id, group) in [(role_group.as_str(), staff), ("online", online), ("offline", offline)] {
                            if group.is_empty() {
                                continue;
                            }
                            items.push(MemberListItem::Group { id: id.into(), count: group.len() as u32 });
                            for u in group {
                                let status = if id == "offline" { Presence::Offline } else { presence.next().unwrap_or_default() };
                                let roles = if id == role_group { role.into_iter().collect() } else { Vec::new() };
                                let member = Member { user: Some(u.clone()), roles, ..Default::default() };
                                items.push(MemberListItem::Member { member, status });
                            }
                        }
                        members::demo_presences(&mut self.state, &items);
                        // The whole (demo) server is listed: role counts show (server_settings.rs).
                        let (member_count, online_count) = (users.len() as u32, online.len() as u32 + staff.len() as u32);
                        self.state.member_lists.insert(g, crate::state::MemberList { items, member_count, online_count, ..Default::default() });
                    }
                    s if let Some(status) = s.strip_prefix("status:") => sidebar::demo_status(&mut self.state, status),
                    s if let Some(id) = s.strip_prefix("channel:").and_then(|id| id.parse().ok()) => {
                        self.state.select_channel(Id(id));
                    }
                    s if dm_call::demo(&mut self.state, s) => {}
                    // quests.rs `demo`: quests[:blocked|empty|error|loading|watch|watch-play|bg|complete|claim].
                    s if quests::demo(&mut self.state, &mut self.view, s) => {}
                    // requests.rs `demo`: requests[:spam|ticked|confirm|open|spam-open].
                    s if requests::demo(&mut self.state, &mut self.view, s) => {}
                    _ => {}
                }
            }
        }
        // Test hook: RUSTCORD_SCREENSHOT=<file.png> saves a screenshot after a few frames and exits;
        // RUSTCORD_SCREENSHOT_FRAME=<n> (default 30, >= 50 ms each) waits longer, e.g. for a QR code.
        if self.frame_no <= 1000
            && let Some(path) = std::env::var_os("RUSTCORD_SCREENSHOT")
        {
            let at = std::env::var("RUSTCORD_SCREENSHOT_FRAME").ok().and_then(|s| s.parse().ok()).unwrap_or(30);
            if self.frame_no == at {
                let ctx2 = ctx.clone();
                ctx.request_screenshot(move |img| {
                    let [w, h] = img.size;
                    let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                    if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, rgba) {
                        let _ = buf.save(&path);
                    }
                    // A game-overlay demo closes the window itself, after its panes' own screenshots.
                    if !overlay::demo_active() {
                        ctx2.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            }
            if self.frame_no < at + 10 {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }
    }
}

impl eframe::App for RustcordApp {
    /// One clear for every viewport: the overlay windows need it transparent, and the root's panels cover the root.
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    /// Runs before every `ui` call and also while the window is hidden/minimized, so
    /// network events and notifications are processed even when nothing is drawn.
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        frame_stats::begin(ctx, frame.info().cpu_usage);
        let _t = frame_stats::scope("logic");
        self.view.window_focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        self.drain_events(ctx);
        // Tray menu clicks, closing to the tray, `--minimized` (also while hidden in the tray).
        let clicked = user_settings::logic(ctx, &mut self.view, &self.backend.cmd, &mut self.settings);
        if self.view.screen == Screen::Main {
            if clicked {
                quests::notification_clicked(&mut self.state, &mut self.view);
                inbox::notification_clicked(&mut self.view);
            }
            self.selection_effects(ctx);
            // Plugins (CustomIdle): auto-idle, also while minimized.
            crate::plugins::idle::logic(ctx, &self.state, &self.backend.cmd);
            // My detected game and Rustcord's own presence in op 3, batched app-icon requests.
            crate::activity::logic(&mut self.state, &self.settings, &self.backend.cmd);
            // People who updated from 0.1.4 hear once that Rustcord now shows as their activity.
            if self.state.conn == ConnState::Connected && !self.settings.rustcord_presence.notice_seen {
                self.settings.rustcord_presence.notice_seen = true;
                self.view.toast("Rustcord now shows as your activity. Change it in Settings > Activity privacy.", false);
                let _ = self.settings.save();
            }
            // A quest video the user started plays on and reports progress, also minimized.
            quests::logic(ctx, &mut self.state, &mut self.view, &self.backend.cmd);
            // Like messages: not in Do Not Disturb, only with desktop notifications on, and the
            // overlay shows it in a game.
            let dnd_off = self.state.chat.my_status != Some(crate::model::UserStatus::Dnd);
            // Saved messages whose reminder is due: the same gates as "Quest complete".
            for (body, jump) in inbox::reminders(ctx, &self.state, &mut self.view, &mut self.settings) {
                if let Some((title, text)) = self.settings.notification("Reminder", &body)
                    && dnd_off
                    && !self.settings.quiet_hours.quiet_now()
                    && !self.overlay.notify(&self.state, &self.settings, title, text, None) {
                    crate::notify::show(title, text);
                    self.view.quests_ui.notified_last = false;
                    self.view.inbox_ui.notified = Some(jump);
                }
            }
            for body in quests::completed(&mut self.state, &self.view) {
                if dnd_off
                    && self.settings.notification("", "").is_some()
                    && !self.settings.quiet_hours.quiet_now()
                    && !self.overlay.notify(&self.state, &self.settings, "Quest complete", &body, None) {
                    crate::notify::show("Quest complete", &body);
                    self.view.quests_ui.notified_last = true;
                    self.view.inbox_ui.notified = None;
                }
            }
        }
        self.maybe_save_cache(false);
        // My id and Nitro (local cosmetics apply only without it).
        let me = self.state.me.as_ref().map(|u| (u.id, u.premium_type));
        if me != self.overrides_me {
            self.overrides_me = me;
            crate::overrides::publish(self.state.me.as_ref(), &self.overrides);
        }
        images::mem_hooks(ctx, &mut self.state, &mut self.images);
        // Videos shown in the last pass stay decoded, the rest are hidden (also while minimized).
        call::sync(&mut self.view, &mut self.state, &self.backend.cmd, &self.backend.video);
        dm_call::logic(&mut self.state, &self.backend.cmd, &self.settings.camera);
        self.sound_cues();
        self.overlay.logic(ctx, &self.state, &self.view, &self.settings, &self.theme, &mut self.images);
        let pressed = |e: &egui::Event| matches!(e, egui::Event::PointerButton { pressed: true, .. } | egui::Event::Key { pressed: true, .. });
        self.interacted |= ctx.input(|i| i.events.iter().any(pressed));
        let hidden = user_settings::in_tray(ctx);
        crate::update::sync_ui(hidden, update::busy(&self.state, &self.view, hidden), self.interacted);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // macOS: the native traffic lights, centred in the title bar row (ui/chrome.rs).
        #[cfg(target_os = "macos")]
        frame.set_traffic_lights_position(&ctx, egui::Rangef::new(0.0, TITLE_BAR_H), 12.0);
        if self.view.theme_dirty {
            self.view.theme_dirty = false;
            self.theme_applier.apply(&ctx, &self.theme);
            // The picture shows the old colours: the panel floats over the plain frame colour.
            self.view.settings_backdrop = None;
            drop(self.view.backdrop_slot.lock().map(|mut s| s.take()));
        }

        let mut cx = UiCx {
            state: &mut self.state,
            view: &mut self.view,
            images: &mut self.images,
            theme: &mut self.theme,
            settings: &mut self.settings,
            cmd: &self.backend.cmd,
            overrides: &mut self.overrides,
        };
        // Zoom, reduced motion, global keybinds, capture exclusion.
        user_settings::frame(&ctx, &mut cx);
        let up = update::status();
        match cx.view.screen {
            // The update screen covers the login and the main window alike.
            _ if up.screen => {
                title_bar(ui, &mut cx, &up);
                update::screen(ui, &cx, &up);
            }
            Screen::Login => {
                title_bar(ui, &mut cx, &up);
                login::show(ui, &mut cx);
            }
            // App lock: the lock screen instead of the window (nothing of the chat is drawn).
            Screen::Main if crate::lock::locked() => {
                title_bar(ui, &mut cx, &up);
                lock_screen::screen(ui, &mut cx);
            }
            Screen::Main => {
                // Gradient themes: the window gradient under the translucent panels.
                self.theme_applier.paint_background(&ctx);
                // Taken before the page draws: the composer / message editor would read the same
                // Ctrl+Enter as Enter and send the draft.
                let accept = cx.state.incoming_call().is_some()
                    && user_settings::shortcut(cx.settings, user_settings::Bind::AcceptCall).is_some_and(|k| ctx.input_mut(|i| i.consume_shortcut(&k)));
                main_layout(ui, &mut cx, &up);
                let _t = frame_stats::scope("popups");
                profile::show_popup(&ctx, &mut cx);
                show_modals(&ctx, &mut cx);
                viewer::end_frame(&ctx, &mut cx);
                crate::plugins::frame(&ctx, &mut cx);
                crash_card::show(&ctx, &mut cx);
                dm_call::incoming(&ctx, &mut cx, accept);
            }
        }

        // The captcha webview follows the login card's slot (a native child of this window).
        self.captcha.sync(&ctx, frame, &mut self.view, &self.theme, &self.backend.cmd);
        // Clicks and keybinds handled above (`logic` runs before them).
        self.sound_cues();
        frame_stats::time("end", || {
            self.show_toasts(&ctx);
            self.images.end_frame(&ctx);
            self.startup_hooks(&ctx);
            chrome::resize_cursor(&ctx);
            text_field_focus_ring(&ctx);
            self.settle_after_events(&ctx);
            motion::end_pass(&ctx);
        });
        frame_stats::end(&ctx);
    }

    /// Test hook: RUSTCORD_POINTER=<x>x<y> keeps the pointer at that point (logical px), so
    /// screenshots show hover states and tooltips; RUSTCORD_CLICK=left|right also clicks there
    /// once (frames 10-11), e.g. to open a context menu. With RUSTCORD_SCREENSHOT the window
    /// counts as focused (text focus, completion popups, read acks), whatever is in front.
    /// RUSTCORD_NAV_DEMO=focus:<n> drops the focus and presses Tab n times (Shift+Tab for a
    /// negative n: from the last control, e.g. an open menu's), one per frame from frame 14, to
    /// show the focus ring wherever it lands. RUSTCORD_NAV_DEMO=press:<shortcut> presses that
    /// combination once at frame 14 (Focus stays where it is, so it goes through a focused composer).
    /// RUSTCORD_POINTER_AT=<frame>:<x>x<y> moves the pointer there from that frame on (a hover that
    /// starts after a card opened, as from the member list).
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        // Runs for every viewport's pass; the pointer, click and focus hooks below are the root's.
        if raw.viewport_id != egui::ViewportId::ROOT {
            return;
        }
        chrome::resize_press(ctx, raw);
        static SHOT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *SHOT.get_or_init(|| std::env::var_os("RUSTCORD_SCREENSHOT").is_some()) {
            raw.focused = true;
            if let Some(v) = raw.viewports.get_mut(&raw.viewport_id) {
                v.focused = Some(true);
            }
        }
        if let Some(n) = sidebar::nav_demo_value("focus").and_then(|v| v.parse::<i32>().ok())
            && (14..14 + u64::from(n.unsigned_abs())).contains(&self.frame_no)
        {
            let modifiers = if n < 0 { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE };
            if self.frame_no == 14 {
                ctx.memory_mut(|m| m.stop_text_input());
            }
            for pressed in [true, false] {
                raw.events.push(egui::Event::Key { key: egui::Key::Tab, physical_key: None, pressed, repeat: false, modifiers });
            }
        }
        if self.frame_no == 14
            && let Some(k) = sidebar::nav_demo_value("press").and_then(user_settings::parse)
        {
            for pressed in [true, false] {
                raw.events.push(egui::Event::Key { key: k.logical_key, physical_key: None, pressed, repeat: false, modifiers: k.modifiers });
            }
        }
        static POINTER: std::sync::OnceLock<Option<egui::Pos2>> = std::sync::OnceLock::new();
        let pointer = POINTER.get_or_init(|| std::env::var("RUSTCORD_POINTER").ok().as_deref().and_then(parse_size).map(|v| v.to_pos2()));
        static LATER: std::sync::OnceLock<Option<(u64, egui::Pos2)>> = std::sync::OnceLock::new();
        let later = LATER.get_or_init(|| {
            let v = std::env::var("RUSTCORD_POINTER_AT").ok()?;
            let (frame, at) = v.split_once(':')?;
            Some((frame.parse().ok()?, parse_size(at)?.to_pos2()))
        });
        let pointer = &match *later {
            Some((frame, p)) if self.frame_no >= frame => Some(p),
            _ => *pointer,
        };
        // The real mouse would fight the hook (Windows sends moves while the cursor rests on the window).
        if pointer.is_some() {
            raw.events.retain(|e| !matches!(e, egui::Event::PointerMoved(_) | egui::Event::PointerGone));
        }
        // Only when it moved away: every move event restarts the tooltip delay.
        if let Some(p) = *pointer
            && ctx.input(|i| i.pointer.latest_pos()) != Some(p)
        {
            raw.events.push(egui::Event::PointerMoved(p));
        }
        if let (Some(pos), 10 | 11) = (*pointer, self.frame_no) {
            let button = match std::env::var("RUSTCORD_CLICK").as_deref() {
                Ok("left") => egui::PointerButton::Primary,
                Ok("right") => egui::PointerButton::Secondary,
                _ => return,
            };
            let pressed = self.frame_no == 10;
            raw.events.push(egui::Event::PointerButton { pos, button, pressed, modifiers: egui::Modifiers::default() });
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.maybe_save_cache(true);
        crate::notify::shutdown();
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

/// The focused text field's frame in the focus colour (docs/DESIGN.md §9 Inputs). egui strokes it
/// with `selection.stroke`, which also colours selected text and selected labels, so that stays
/// `text` (theme/apply.rs) and the stroke is recoloured here, after the pass painted it. Fields
/// with their own frame (`Frame::NONE`) draw no such stroke and keep their own ring.
fn text_field_focus_ring(ctx: &egui::Context) {
    let Some(field) = ctx.memory(|m| m.focused()).and_then(|id| ctx.read_response(id)) else { return };
    let sel = ctx.global_style().visuals.selection.stroke;
    let focus = crate::theme::applied_tokens(ctx).focus;
    let near = |a: egui::Pos2, b: egui::Pos2| a.distance(b) < 1.0;
    ctx.graphics_mut(|g| {
        let Some(list) = g.get_mut(field.layer_id) else { return };
        let frames: Vec<usize> = list
            .all_entries()
            .enumerate()
            .filter(|(_, c)| matches!(&c.shape, egui::Shape::Rect(r) if r.stroke == sel && near(r.rect.min, field.rect.min) && near(r.rect.max, field.rect.max)))
            .map(|(i, _)| i)
            .collect();
        for i in frames {
            list.mutate_shape(egui::layers::ShapeIdx(i), |c| {
                if let egui::Shape::Rect(r) = &mut c.shape {
                    r.stroke.color = focus;
                }
            });
        }
    });
}

/// Same widgets at the same rects in the same paint order. `WidgetRects`' own `==` also
/// compares the empty layer lists a recycled pass keeps, which differ between the two passes.
fn same_widgets(a: &egui::WidgetRects, b: &egui::WidgetRects) -> bool {
    let layers = |w: &egui::WidgetRects| w.layers().filter(|(_, ws)| !ws.is_empty()).count();
    layers(a) == layers(b) && a.layers().all(|(l, ws)| ws.iter().eq(b.get_layer(*l)))
}

/// The 32 px title bar of the frameless window (`chrome`: drag, window buttons) with the
/// location (main screen) or Rustcord's name (login, update screen), and the updater's button.
fn title_bar(ui: &mut egui::Ui, cx: &mut UiCx<'_>, up: &crate::update::Status) {
    let c = cx.theme.colors.clone();
    egui::Panel::top("title_bar")
        .exact_size(TITLE_BAR_H)
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::new().fill(cx.theme.panel_fills().frame.to_color32()))
        .show(ui, |ui| {
            chrome::title_bar(ui, &c, |ui| {
                if up.screen || cx.view.screen == Screen::Login || crate::lock::locked() {
                    chrome::app_label(ui, cx.theme);
                } else {
                    sidebar::title_bar(ui, cx);
                }
                if !up.screen {
                    // sidebar::title_bar shows the inbox button outside the settings layers.
                    let inbox = cx.view.screen == Screen::Main && !cx.view.chat_hidden();
                    update::title_button(ui, cx, up, inbox);
                }
            })
        });
}

/// Title bar over the columns: server bar | channel list card | page | right column; the
/// floating user panel spans the first two. The page is the Friends page (DM home with no DM
/// open), a forum's post list or the chat; the right column the search results, the thread
/// panel or the member list. User and server settings cover everything under the title bar.
/// The channel list (303 px, resizable) narrows first so the page keeps ~450 px; narrow windows
/// drop the member list and show the thread panel in the chat's place (minimum window 720 px).
fn main_layout(ui: &mut egui::Ui, cx: &mut UiCx<'_>, up: &crate::update::Status) {
    requests::sync(cx.state, cx.view);
    threads::track(cx, ui.ctx().pixels_per_point());
    call::demo(cx);
    let c = cx.theme.colors.clone();
    // Panel backgrounds: the theme colours, or overlays over the window gradient.
    let fill = cx.theme.panel_fills();
    let width = ui.available_width();
    let right = right_column_w(cx.view, cx.settings, cx.state, width);
    let (channels_w, max_w) = channel_list_width(width, right, cx.view.channel_list_w);
    let edge = sidebar::line(&c);
    frame_stats::time("title", || title_bar(ui, cx, up));
    // The settings panel fades in and settles 0.98 -> 1 over LARGE (§7); closing is instant.
    let hidden = cx.view.chat_hidden();
    let shown = motion::anim(ui.ctx(), egui::Id::unique("rc_settings_layer"), hidden, motion::LARGE, 0.0);
    if hidden {
        pickers::close_if_open(ui.ctx(), cx);
        cx.view.inbox_open = false;
        if !cx.view.backdrop_asked {
            // One frame draws the app as usual (below) and takes its picture; no read ack comes
            // of it (the auto-ack gate in `RustcordApp::sync` stays on `chat_hidden`).
            cx.view.backdrop_asked = true;
            capture_backdrop(ui.ctx(), cx.view.backdrop_slot.clone());
        } else {
            if cx.view.settings_backdrop.is_none()
                && let Some(img) = cx.view.backdrop_slot.lock().ok().and_then(|mut s| s.take())
            {
                cx.view.settings_backdrop = Some(ui.ctx().load_texture("rc_settings_backdrop", img, egui::TextureOptions::LINEAR));
            }
            // Lighter than a modal's scrim: the frozen picture of the app should stay readable behind the panel.
            let tk = cx.theme.tokens();
            let scrim = match tk.mode {
                crate::theme::Mode::Dark => egui::Color32::from_black_alpha(89),
                crate::theme::Mode::Light => egui::Color32::from_black_alpha(61),
                crate::theme::Mode::HighContrast => tk.scrim,
            };
            let picture = cx.view.settings_backdrop.as_ref().map(|t| t.id());
            let page = egui::Frame::new().fill(fill.frame.to_color32());
            // The wrapper stays at rest too: skipping it would change the widget ids under it.
            let layer = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
                let rect = ui.max_rect();
                if let Some(id) = picture {
                    // The part of the picture under the title bar; it is there at once, the scrim fades in over it.
                    let top = TITLE_BAR_H / (rect.height() + TITLE_BAR_H);
                    ui.painter().image(id, rect, egui::Rect::from_min_max(egui::pos2(0.0, top), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                }
                ui.painter().rect_filled(rect, 0.0, scrim.gamma_multiply(shown));
                ui.with_visual_transform(motion::about(rect.center(), egui::lerp(0.98..=1.0, shown)), |ui| {
                    ui.multiply_opacity(shown);
                    add(ui);
                });
            };
            match cx.view.server_settings.filter(|_| !cx.view.settings_open) {
                Some((g, p)) => egui::CentralPanel::default().frame(page).show(ui, |ui| layer(ui, &mut |ui| server_settings::show(ui, cx, g, p))),
                None => egui::CentralPanel::default().frame(page).show(ui, |ui| layer(ui, &mut |ui| user_settings::show(ui, cx))),
            };
            return;
        }
    } else {
        cx.view.settings_backdrop = None;
        cx.view.backdrop_asked = false;
        drop(cx.view.backdrop_slot.lock().map(|mut s| s.take()));
        cx.view.settings_themes = None;
    }
    // Focus mode: 1 = the lists are shown. They fold away to the left (BASE ease-in) and come back (LARGE ease-out).
    let lists = motion::anim(ui.ctx(), egui::Id::unique("rc_focus"), !cx.view.focus_mode, motion::LARGE, motion::BASE);
    let origin = ui.available_rect_before_wrap();
    // Content keeps its full width and slides left with its narrowing column, clipped to it.
    let slid = |ui: &egui::Ui, w: f32| {
        let r = ui.max_rect();
        egui::Rect::from_min_size(egui::pos2(r.right() - w, r.top()), egui::vec2(w, r.height()))
    };
    let _t = frame_stats::scope("rail");
    if lists > 0.0 {
        egui::Panel::left("guild_bar")
            .exact_size(GUILD_BAR_W * lists)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(fill.frame.to_color32()))
            .show(ui, |ui| {
                let (clip, full) = (ui.max_rect(), slid(ui, GUILD_BAR_W));
                ui.scope_builder(egui::UiBuilder::new().max_rect(full), |ui| {
                    ui.set_clip_rect(clip.intersect(ui.clip_rect()));
                    sidebar::guild_bar(ui, cx)
                });
            });
    }
    drop(_t);
    let _t = frame_stats::scope("channels");
    // On the server-bar colour, which shows around the card's rounded corner.
    let list = if lists > 0.0 {
        egui::Panel::left("channel_list")
            .exact_size(channels_w * lists)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(fill.frame.to_color32()))
            .show(ui, |ui| {
                let (clip, full) = (ui.max_rect(), slid(ui, channels_w));
                ui.scope_builder(egui::UiBuilder::new().max_rect(full), |ui| {
                    ui.set_clip_rect(clip.intersect(ui.clip_rect()));
                    sidebar::paint_card(ui.painter(), full, &ThemeColors { sidebar: fill.card, ..c.clone() });
                    sidebar::channel_list(ui, cx)
                });
            })
            .response
            .rect
    } else {
        egui::Rect::from_min_size(origin.min, egui::vec2(0.0, origin.height()))
    };
    drop(_t);
    if lists > 0.0 {
        ui.painter().vline(list.right() - 0.5, list.y_range(), edge);
    }

    let page = egui::Frame::new().fill(fill.chat.to_color32());
    let _t = frame_stats::scope("page");
    if friends::is_open(cx.state) {
        pickers::close_if_open(ui.ctx(), cx);
        // The DM home: Friends, or Quests / Message requests (the rows under Friends).
        let (quests, requests) = (cx.view.quests_ui.open, cx.view.requests_ui.open);
        egui::CentralPanel::default().frame(page).show(ui, |ui| {
            if quests {
                quests::show(ui, cx)
            } else if requests {
                requests::show(ui, cx)
            } else {
                friends::show(ui, cx)
            }
        });
    } else if call::is_open(cx.state) && !dm_call::in_chat(cx.state) {
        // The open channel is my call: the stage (call.rs), with its chat column (`right`). A DM
        // call stays above the DM's chat (dm_call.rs).
        call::show(ui, cx, right);
    } else {
        let thread = cx.view.thread_panel;
        let searching = search::is_open(cx.state);
        // The thread panel has its own header: full height, beside the chat's.
        if let Some(t) = thread.filter(|_| right > 0.0 && !searching) {
            let r = egui::Panel::right("thread_panel")
                .exact_size(THREAD_PANEL_W)
                .resizable(false)
                .show_separator_line(false)
                .frame(page)
                .show(ui, |ui| threads::panel(ui, cx, t))
                .response
                .rect;
            ui.painter().vline(r.left() + 0.5, r.y_range(), edge);
        }
        // Spans the page and the member list / search results, like the web client's title section.
        frame_stats::time("+header", || messages::chat_header(ui, cx));
        // Open search results take the member list's place, so does the thread panel.
        if searching {
            search::side_panel(ui, cx);
        } else if thread.is_none() && right > 0.0 {
            let _t = frame_stats::scope("+members");
            let r = egui::Panel::right("member_list")
                .exact_size(MEMBER_LIST_W)
                .resizable(false)
                .show_separator_line(false)
                .frame(egui::Frame::new().fill(fill.side.to_color32()))
                .show(ui, |ui| members::show(ui, cx))
                .response
                .rect;
            ui.painter().vline(r.left() + 0.5, r.y_range(), edge);
        }
        let _t = frame_stats::scope("+chat");
        egui::CentralPanel::default().frame(page).show(ui, |ui| match (thread, forum::open_forum(cx.state)) {
            // Too narrow for a split: the thread panel takes the page.
            (Some(t), _) if right == 0.0 => threads::panel(ui, cx, t),
            (_, Some(f)) => forum::show(ui, cx, f),
            _ => focus_column(ui, lists, |ui| messages::show(ui, cx)),
        });
    }
    drop(_t);
    // The card's top hairline continues over the message area and the member list.
    ui.painter().hline(list.right()..=ui.max_rect().right(), list.top() + 0.5, edge);
    if lists >= 1.0 {
        channel_list_handle(ui, cx, list, max_w);
    }
    let shift = (1.0 - lists) * (GUILD_BAR_W + channels_w);
    frame_stats::time("user panel", || sidebar::user_panel(ui.ctx(), cx, GUILD_BAR_W + channels_w, shift));
}

/// Widest the chat column gets in focus mode.
const FOCUS_CHAT_W: f32 = 800.0;

/// Width of the chat column on a page `full` wide while the lists are `t` shown (1 = all): the
/// column narrows continuously from the page's width to at most `FOCUS_CHAT_W`.
fn focus_column_w(full: f32, t: f32) -> f32 {
    egui::lerp(full.min(FOCUS_CHAT_W)..=full, t.clamp(0.0, 1.0))
}

/// The chat centred in a column of `focus_column_w`. Always wraps, also at rest: a wrapper that
/// appeared only in focus mode would change the widget ids under it and reset the scroll.
fn focus_column(ui: &mut egui::Ui, t: f32, add: impl FnOnce(&mut egui::Ui)) {
    let page = ui.max_rect();
    let w = focus_column_w(page.width(), t);
    let rect = egui::Rect::from_center_size(page.center(), egui::vec2(w, page.height()));
    ui.scope_builder(egui::UiBuilder::new().id_salt("focus_column").max_rect(rect), add);
}

/// (width, current maximum) of the channel list in a window `width` wide with a `right` px
/// right column: 303 px or the width the user dragged it to, narrowed so the message area
/// keeps 450 px, never under 192 px.
fn channel_list_width(width: f32, right: f32, user: Option<f32>) -> (f32, f32) {
    let room = width - GUILD_BAR_W - MESSAGES_MIN_W - right;
    let max_w = room.clamp(CHANNEL_LIST_MIN_W, CHANNEL_LIST_MAX_W);
    (user.unwrap_or(CHANNEL_LIST_W).clamp(CHANNEL_LIST_MIN_W, max_w), max_w)
}

/// 4 px resize handle on the channel list's right edge: drag it, or focus it (Tab) and use
/// ←/→ (10 px), Home/End. Registered after the message area so it wins the hover there.
fn channel_list_handle(ui: &mut egui::Ui, cx: &mut UiCx<'_>, list: egui::Rect, max_w: f32) {
    let rect = egui::Rect::from_x_y_ranges(list.right() - 2.0..=list.right() + 2.0, list.y_range());
    let resp = ui.interact(rect, ui.make_persistent_id("channel_list_resize"), egui::Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Splitter, true, "Resize channel list"));
    let mut w = resp.interact_pointer_pos().filter(|_| resp.dragged()).map(|p| p.x - list.left());
    if resp.has_focus() {
        let filter = egui::EventFilter { horizontal_arrows: true, ..Default::default() };
        ui.memory_mut(|m| m.set_focus_lock_filter(resp.id, filter));
        let keys = [(egui::Key::ArrowLeft, list.width() - 10.0), (egui::Key::ArrowRight, list.width() + 10.0), (egui::Key::Home, 0.0), (egui::Key::End, f32::MAX)];
        w = keys.into_iter().find(|(k, _)| ui.input(|i| i.key_pressed(*k))).map(|(_, w)| w).or(w);
    }
    if let Some(w) = w {
        cx.view.channel_list_w = Some(w.clamp(CHANNEL_LIST_MIN_W, max_w));
        ui.ctx().request_repaint();
    }
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    if resp.hovered() || resp.dragged() || resp.has_focus() {
        ui.painter().vline(list.right() - 0.5, list.y_range(), ui.visuals().widgets.hovered.fg_stroke);
    }
}

/// Whether `main_layout` shows the member list in a window `width` wide (settings closed).
fn member_list_shown(settings: &Settings, state: &AppState, width: f32) -> bool {
    settings.show_member_list && width >= MEMBER_LIST_MIN_WINDOW_W && members::available(state)
}

/// Width of `main_layout`'s right column (thread panel or member list) in a window `width`
/// wide; 0 when there is none (settings, Friends page, narrow window). Search results size
/// themselves (search.rs).
fn right_column_w(view: &ViewState, settings: &Settings, state: &AppState, width: f32) -> f32 {
    // On the one frame that is drawn for the settings picture (`backdrop_asked` still false) the app shows as usual.
    if (view.chat_hidden() && view.backdrop_asked) || friends::is_open(state) {
        0.0
    } else if call::is_open(state) && !dm_call::in_chat(state) {
        call::chat_w(view, width)
    } else if view.thread_panel.is_some() {
        if width >= MEMBER_LIST_MIN_WINDOW_W { THREAD_PANEL_W } else { 0.0 }
    } else if member_list_shown(settings, state, width) {
        MEMBER_LIST_W
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// Modals, confirmation, settings layers (shared by the social / server / settings modules)
// ---------------------------------------------------------------------------

/// Every modal and dialog, drawn after the page and the profile card (one call site in
/// `RustcordApp::ui`): the open `Modal` through its module, then the confirmation above it.
pub fn show_modals(ctx: &egui::Context, cx: &mut UiCx<'_>) {
    match cx.view.modal {
        Some(Modal::CreateThread { channel_id, from_message }) => threads::create_modal(ctx, cx, channel_id, from_message),
        Some(Modal::NewForumPost { forum_id }) => forum::new_post_modal(ctx, cx, forum_id),
        Some(Modal::Profile { user_id, guild_id }) => profile_modal::show(ctx, cx, user_id, guild_id),
        Some(Modal::GroupDm { channel_id }) => friends::group_dm_modal(ctx, cx, channel_id),
        Some(Modal::CreateChannel { guild_id, parent_id, category }) => server_menu::create_channel_modal(ctx, cx, guild_id, parent_id, category),
        Some(Modal::Invite { guild_id, channel_id }) => server_menu::invite_modal(ctx, cx, guild_id, channel_id),
        Some(Modal::JoinServer) => server_menu::join_server_modal(ctx, cx),
        Some(Modal::Moderate { guild_id, user_id, ban }) => server_menu::moderate_modal(ctx, cx, guild_id, user_id, ban),
        Some(Modal::CustomStatus) => user_settings::custom_status_modal(ctx, cx),
        Some(Modal::PrivacySettings { guild_id }) => server_menu::privacy_modal(ctx, cx, guild_id),
        Some(Modal::ScreenShare) => screen_share::modal(ctx, cx),
        Some(Modal::QuestVideo { quest_id }) => quests::video_modal(ctx, cx, quest_id),
        Some(Modal::QuestClaim { quest_id }) => quests::claim_modal(ctx, cx, quest_id),
        Some(Modal::JoinApplication { guild_id }) => server_menu::join_application_modal(ctx, cx, guild_id),
        Some(Modal::Media) => viewer::lightbox(ctx, cx),
        Some(Modal::Forward { channel_id, message_id }) => forward::modal(ctx, cx, channel_id, message_id),
        Some(Modal::Remind { channel_id, message_id }) => inbox::remind_modal(ctx, cx, channel_id, message_id),
        Some(Modal::CreatePoll { channel_id }) => polls::create_modal(ctx, cx, channel_id),
        Some(Modal::QuickSwitcher) => switcher::modal(ctx, cx),
        // A server just joined from its tag card wants its rules accepted first.
        None => {
            if let Some(g) = cx.state.account.verify_after_join.take() {
                server_menu::open_application(cx, g);
            }
        }
    }
    confirm_dialog(ctx, cx);
}

/// The `ViewState::confirm` question. Cancel, Esc or a click beside it drop it; the button
/// sends its command (once: the dialog closes with it).
fn confirm_dialog(ctx: &egui::Context, cx: &mut UiCx<'_>) {
    let Some(c) = &cx.view.confirm else { return };
    let theme = &*cx.theme;
    let resp = dialog(ctx, theme, "rc_confirm", 440.0, &c.title, |ui| {
        dialog_text(ui, theme, &c.body);
        dialog_buttons(ui, theme, &c.button, c.danger, true)
    });
    match resp.inner {
        Some(true) => {
            if let Some(c) = cx.view.confirm.take() {
                cx.cmd.send(c.then);
            }
        }
        Some(false) => cx.view.confirm = None,
        None if resp.should_close() => cx.view.confirm = None,
        None => {}
    }
}

/// Shared modal look (docs/DESIGN.md §9 modals: E3 on the scrim, radius 2r, 24 px padding,
/// 440 / 520 / 640 wide): the title on the left and a × ("Close") at the top right, then `add`.
/// Esc, the × or a click on the backdrop make `should_close()` true; the caller then clears its
/// `ViewState` field. Narrow windows shrink it; it sits under the title bar, centred in the room
/// left and at most that tall, `add` scrolling under the title and above `dialog_footer` /
/// `dialog_buttons`.
pub fn dialog<R>(ctx: &egui::Context, theme: &Theme, salt: &str, width: f32, title: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> egui::ModalResponse<R> {
    dialog_closable(ctx, theme, salt, width, title, true, add)
}

/// A requested dialog width on the spec's tiers (confirm 440, form 520, wide 640); wider ones
/// (plugin windows) keep theirs.
fn dialog_width(width: f32) -> f32 {
    if width > 700.0 {
        return width;
    }
    [440.0, 520.0, 640.0f32].into_iter().min_by(|a, b| (a - width).abs().total_cmp(&(b - width).abs()).then(b.total_cmp(a))).unwrap_or(width)
}

/// `dialog` without the × while `closable` is false (busy, and Esc would not cancel it either).
pub fn dialog_closable<R>(ctx: &egui::Context, theme: &Theme, salt: &str, width: f32, title: &str, closable: bool, add: impl FnOnce(&mut egui::Ui) -> R) -> egui::ModalResponse<R> {
    let pal = messages::Pal::new(theme);
    let style = ctx.global_style();
    let frame = egui::Frame::popup(&style)
        .fill(pal.tk.raised)
        .shadow(style.visuals.window_shadow)
        .corner_radius(pal.modal_radius())
        .inner_margin(egui::Margin::same(24));
    let width = dialog_width(width).min(ctx.content_rect().width() - 80.0).max(200.0);
    // 12 px under the title bar and above the window bottom (as the profile modal), centred there.
    let max_h = (ctx.content_rect().height() - TITLE_BAR_H - 24.0 - 48.0).max(120.0);
    let id = egui::Id::unique(salt);
    let area = egui::Modal::default_area(id).anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, TITLE_BAR_H / 2.0));
    motion::modal(ctx, area, frame, pal.tk.scrim, |ui| {
        ui.set_width(width);
        // An area's ui is only as tall as last frame's contents; the scroll area takes its height
        // from it, so without this the dialog could never grow.
        ui.set_max_height(max_h);
        ui.spacing_mut().item_spacing.y = 8.0;
        let corner = ui.max_rect().right_top();
        ui.scope(|ui| {
            ui.set_max_width(width - 40.0);
            ui.set_min_height(32.0);
            ui.label(pal.title_text(title));
        });
        let x = egui::Rect::from_min_size(corner + egui::vec2(-28.0, -4.0), egui::Vec2::splat(32.0));
        if closable && messages::vector_button(&mut ui.new_child(egui::UiBuilder::new().max_rect(x)), &pal, icons::Icon::Close, "Close", 32.0, false, pal.text).clicked() {
            ui.close_kind(egui::UiKind::Modal);
        }
        // No fade: it would dim the pinned buttons. Scroll areas inside `add` keep theirs.
        let fade = ui.spacing().scroll.fade;
        ui.spacing_mut().scroll.fade.strength = 0.0;
        let out = egui::ScrollArea::vertical().max_height(ui.available_height()).auto_shrink([false, true]).show(ui, |ui| {
            ui.spacing_mut().scroll.fade = fade;
            add(ui)
        })
        .inner;
        // The frame's outer rect: the content plus its 24 px margin.
        messages::sheen(ui.painter(), ui.min_rect().expand(24.0), f32::from(pal.modal_radius()), &pal.tk);
        out
    })
}

/// Body text of a dialog: muted, wrapped.
pub fn dialog_text(ui: &mut egui::Ui, theme: &Theme, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).color(theme.colors.text_muted.to_color32())).wrap());
}

/// Ghost "Cancel" + the action, right-aligned at the dialog's bottom. Some(true) = the action
/// (primary, or danger when `danger`; disabled unless `enabled`), Some(false) = Cancel.
pub fn dialog_buttons(ui: &mut egui::Ui, theme: &Theme, action: &str, danger: bool, enabled: bool) -> Option<bool> {
    let pal = messages::Pal::new(theme);
    let kind = if danger { messages::Btn::Danger } else { messages::Btn::Primary };
    ui.add_space(8.0);
    dialog_footer(ui, |ui| {
        if messages::button(ui, &pal, kind, action, enabled).clicked() {
            return Some(true);
        }
        messages::button(ui, &pal, messages::Btn::Ghost, "Cancel", true).clicked().then_some(false)
    })
}

/// A dialog's last row, 40 px, right to left, 12 px between its buttons (40 tall). While the
/// body scrolls it stays at the dialog's bottom (sticky), over the body scrolled under it, so
/// its buttons are always in sight.
pub fn dialog_footer<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let (natural, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), egui::Sense::hover());
    let rect = natural.translate(egui::vec2(0.0, (ui.clip_rect().bottom() - natural.bottom()).min(0.0)));
    if rect != natural {
        let cover = rect.with_min_y(rect.top() - 8.0);
        ui.painter().rect_filled(cover, 0.0, ui.stack().bg_color());
        // Discord marks an overflowing body with a line above the footer, so the cut reads as scrollable.
        ui.painter().hline(cover.x_range(), cover.top(), ui.visuals().widgets.noninteractive.bg_stroke);
        // Clicks there must not reach the body under it.
        ui.interact(cover, ui.scope_id().with("rc_dialog_footer"), egui::Sense::click());
    }
    // A child at that spot: the cursor (and the scrolled content's size) stay as allocated above.
    let mut row = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::right_to_left(egui::Align::Center)));
    row.spacing_mut().item_spacing.x = 12.0;
    row.spacing_mut().interact_size.y = 40.0;
    add(&mut row)
}

/// Asks for one picture of the frame being drawn, shrinks it to 1/8 and blurs it (§7.2); the next frame
/// finds it in `slot`. The full-size copies are gone when the callback returns.
fn capture_backdrop(ctx: &egui::Context, slot: Arc<std::sync::Mutex<Option<egui::ColorImage>>>) {
    let ctx2 = ctx.clone();
    ctx.request_screenshot(move |img| {
        let [w, h] = img.size;
        let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
        if let Some(big) = image::RgbaImage::from_raw(w as u32, h as u32, rgba) {
            let small = image::imageops::thumbnail(&big, (w as u32 / 8).max(1), (h as u32 / 8).max(1));
            let small = image::imageops::blur(&small, 1.5);
            let size = [small.width() as usize, small.height() as usize];
            if let Ok(mut s) = slot.lock() {
                *s = Some(egui::ColorImage::from_rgba_unmultiplied(size, small.as_raw()));
            }
        }
        ctx2.request_repaint();
    });
}

/// Settings panel: widest it gets, and its nav column (208 px in windows narrower than `NARROW_W`).
const PANEL_MAX_W: f32 = 1180.0;
const PANEL_NAV_W: f32 = 248.0;
const PANEL_NAV_W_NARROW: f32 = 208.0;
/// Page column of the settings panel.
const LAYER_PAGE_W: f32 = 740.0;
const NARROW_W: f32 = 960.0;

/// Fade at the top and bottom of a scrolled settings column, in the column's own colour (flat,
/// opaque in every theme). `x0` / `x1` keep it off the panel's rounded corners.
fn scroll_fades<R>(ui: &egui::Ui, out: &egui::scroll_area::ScrollAreaOutput<R>, area: egui::Rect, (x0, x1): (f32, f32), color: egui::Color32) {
    use egui::{Rect, pos2};
    let fade = 20.0;
    let top = out.state.clamped_offset().y;
    let below = out.content_size.y - out.inner_rect.height() - top;
    for (t, rect, is_top) in [
        (top, Rect::from_min_max(pos2(x0, area.top()), pos2(x1, area.top() + fade)), true),
        (below, Rect::from_min_max(pos2(x0, area.bottom() - fade), pos2(x1, area.bottom())), false),
    ] {
        if t <= 0.0 {
            continue;
        }
        let mut mesh = egui::Mesh::default();
        mesh.add_colored_rect(rect, color);
        for v in &mut mesh.vertices {
            // Full at the column's edge (once scrolled `fade` px), nothing `fade` px in.
            let edge = (if is_top { rect.bottom() - v.pos.y } else { v.pos.y - rect.top() }) / fade;
            v.color = color.gamma_multiply(((t / fade).min(1.0) * edge).clamp(0.0, 1.0));
        }
        ui.painter().add(mesh);
    }
}

/// Settings layer (user and server settings, FEATURES_NOTES §4.9, DESIGN §7): a floating panel
/// (at most 1180 wide, inset from the window, radius 2.5r, E3) over the frozen picture
/// `main_layout` draws: `nav` in the left column (sidebar colour), `page` in a ≤ 740 px scrolling
/// column, a ghost × at the panel's top right. True when it should close: the ×, or Esc while no
/// popup, profile card or dialog is above it; a click on the dim does nothing. `salt` keeps the
/// two layers' scroll states apart.
pub fn settings_layer(
    ui: &mut egui::Ui,
    cx: &mut UiCx<'_>,
    salt: &str,
    nav: impl FnOnce(&mut egui::Ui, &mut UiCx<'_>),
    page: impl FnOnce(&mut egui::Ui, &mut UiCx<'_>),
) -> bool {
    use egui::{CornerRadius, Margin, Rect, ScrollArea, Stroke, UiBuilder, pos2, vec2};
    let pal = messages::Pal::new(cx.theme);
    let (sidebar, background) = (cx.theme.colors.sidebar.to_color32(), cx.theme.colors.background.to_color32());
    let full = ui.max_rect();
    let narrow = full.width() < NARROW_W;
    let (side, top) = if narrow { (12.0, 12.0) } else { (24.0, 16.0) };
    let avail = Rect::from_min_max(full.min + vec2(side, top), full.max - vec2(side, side));
    let half = avail.width().min(PANEL_MAX_W) / 2.0;
    let panel = Rect::from_min_max(pos2(avail.center().x - half, avail.top()), pos2(avail.center().x + half, avail.bottom()));
    let r = pal.modal_radius();
    let nav_rect = Rect::from_min_max(panel.min, pos2(panel.left() + if narrow { PANEL_NAV_W_NARROW } else { PANEL_NAV_W }, panel.bottom()));
    let page_rect = Rect::from_min_max(pos2(nav_rect.right(), panel.top()), panel.max);
    let p = ui.painter().clone();
    // Softer and shorter than a modal's E3, so the picture behind the panel stays visible.
    let shadow_a = if pal.tk.mode == crate::theme::Mode::Light { 46 } else { 115 };
    p.add(egui::Shadow { offset: [0, 16], blur: 48, spread: 0, color: egui::Color32::from_black_alpha(shadow_a) }.as_shape(panel, r));
    p.rect_filled(nav_rect, CornerRadius { nw: r, sw: r, ne: 0, se: 0 }, sidebar);
    p.rect_filled(page_rect, CornerRadius { ne: r, se: r, nw: 0, sw: 0 }, background);

    let nav_out = ui.scope_builder(UiBuilder::new().max_rect(nav_rect), |ui| {
        ui.spacing_mut().scroll.fade.strength = 0.0;
        ScrollArea::vertical().id_salt((salt, "nav")).auto_shrink(false).show(ui, |ui| {
            egui::Frame::new().inner_margin(Margin { left: 12, right: 12, top: 16, bottom: 24 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 2.0;
                nav(ui, cx);
            });
        })
    });
    let col_w = (page_rect.width() - 80.0).clamp(160.0, LAYER_PAGE_W);
    let pad = ((page_rect.width() - col_w) / 2.0).max(40.0);
    let page_out = ui.scope_builder(UiBuilder::new().max_rect(page_rect), |ui| {
        ui.spacing_mut().scroll.fade.strength = 0.0;
        ScrollArea::vertical().id_salt((salt, "page")).auto_shrink(false).show(ui, |ui| {
            egui::Frame::new().inner_margin(Margin { left: pad as i8, right: 0, top: 40, bottom: 64 }).show(ui, |ui| {
                ui.set_width(col_w);
                page(ui, cx);
            });
        })
    });
    let rf = f32::from(r);
    scroll_fades(ui, &nav_out.inner, nav_rect, (nav_rect.left() + rf, nav_rect.right()), sidebar);
    scroll_fades(ui, &page_out.inner, page_rect, (page_rect.left(), page_rect.right() - rf), background);
    // Edges over the content: the outline (heavier in high contrast), the nav / page line, the lit top edge.
    let outline = if super::theme::is_high_contrast(&cx.theme.colors) { Stroke::new(1.5, pal.muted) } else { Stroke::new(1.0, pal.tk.rule) };
    p.rect_stroke(panel, r, outline, egui::StrokeKind::Inside);
    p.vline(nav_rect.right() - 0.5, panel.y_range(), Stroke::new(1.0, pal.tk.rule));
    messages::sheen(&p, panel, rf, &pal.tk);

    let x = Rect::from_min_size(pos2(panel.right() - 48.0, panel.top() + 16.0), vec2(32.0, 32.0));
    let close = messages::vector_button(&mut ui.new_child(UiBuilder::new().id_salt((salt, "close")).max_rect(x)), &pal, icons::Icon::Close, "Close settings (Esc)", 32.0, false, pal.text);
    let esc = ui.input(|i| i.key_pressed(egui::Key::Escape))
        && !egui::Popup::is_any_open(ui.ctx())
        && cx.view.profile_card.is_none()
        && !cx.view.dialog_open();
    close.clicked() || esc
}

/// Heading over a group of nav rows ("User settings"): the label role, muted, as written; 16 px
/// above (12 for the `first` one), 4 below.
pub fn nav_heading(ui: &mut egui::Ui, theme: &Theme, text: &str, first: bool) {
    let pal = messages::Pal::new(theme);
    ui.add_space(if first { 12.0 } else { 16.0 } - ui.spacing().item_spacing.y);
    ui.add(egui::Label::new(egui::RichText::new(text).font(semibold(pal.label())).color(pal.muted)).selectable(false).truncate());
    ui.add_space(4.0 - ui.spacing().item_spacing.y);
}

/// One 36 px nav row of a settings layer: an 18 px line icon, the label (selected: the `selected`
/// fill and the icon in `accent_ink`; `danger`: red, e.g. Log out). True when clicked.
pub fn nav_item(ui: &mut egui::Ui, theme: &Theme, icon: icons::Icon, label: &str, selected: bool, danger: bool) -> bool {
    let pal = messages::Pal::new(theme);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 36.0), egui::Sense::click());
    let hot = resp.hovered() || resp.has_focus();
    let fill = if selected { pal.tk.selected } else if resp.hovered() { pal.tk.hover } else { egui::Color32::TRANSPARENT };
    ui.painter().rect_filled(rect, pal.radius, fill);
    let color = if danger { pal.danger } else if selected || hot { pal.text } else { pal.muted };
    let icon_color = if danger { pal.danger } else if selected { pal.tk.accent_ink } else { color };
    icons::paint(ui.painter(), icon, egui::Rect::from_center_size(egui::pos2(rect.left() + 19.0, rect.center().y), egui::Vec2::splat(18.0)), icon_color, egui::Color32::TRANSPARENT);
    let font = egui::FontId::new(pal.body(), crate::theme::medium_family());
    let g = one_line(ui, label, font, color, rect.width() - 38.0 - 10.0);
    ui.painter().galley(egui::pos2(rect.left() + 38.0, rect.center().y - g.size().y / 2.0), g, color);
    if resp.has_focus() {
        messages::focus_ring(ui, &pal, rect, f32::from(pal.radius));
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, label));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// Page title at the top of a settings page (the title role).
pub fn page_title(ui: &mut egui::Ui, theme: &Theme, text: &str) {
    let pal = messages::Pal::new(theme);
    ui.label(pal.title_text(text));
    ui.add_space(8.0);
}

/// Section heading inside a settings page: the heading role, 32 px above, no rule.
pub fn section(ui: &mut egui::Ui, theme: &Theme, title: &str) {
    let pal = messages::Pal::new(theme);
    ui.add_space(32.0 - ui.spacing().item_spacing.y);
    ui.label(egui::RichText::new(title).font(semibold(pal.heading())).color(pal.text));
}

/// One muted hint line under a setting (only where the control needs explaining): meta role.
pub fn hint(ui: &mut egui::Ui, theme: &Theme, text: &str) {
    let pal = messages::Pal::new(theme);
    ui.label(egui::RichText::new(text).size(pal.meta()).color(pal.muted));
}

/// Test hook `RUSTCORD_OPEN=modal:<name>`: create-thread / new-post (the open channel), profile
/// (the lowest cached user id other than me and the open guild's owner), group-dm, create-channel / create-category / invite (the open
/// guild / channel), join-server, kick / ban (that user in the open guild), custom-status, privacy
/// (the open guild's Privacy Settings), apply / apply-rules (the join form for the open guild with
/// sample questions / only its rules).
fn demo_modal(state: &AppState, name: &str) -> Option<Modal> {
    let (ch, g) = (state.selected_channel, state.selected_guild);
    let me = state.me.as_ref().map(|u| u.id);
    let owner = state.guilds.iter().find(|x| Some(x.id) == g).map(|x| x.owner_id);
    let user = || state.users.keys().filter(|&&u| Some(u) != me && Some(u) != owner).min().copied();
    Some(match name {
        "create-thread" => Modal::CreateThread { channel_id: ch?, from_message: None },
        "create-poll" => Modal::CreatePoll { channel_id: ch? },
        "new-post" => Modal::NewForumPost { forum_id: ch? },
        "profile" => Modal::Profile { user_id: user()?, guild_id: g },
        "group-dm" => Modal::GroupDm { channel_id: None },
        "create-channel" | "create-category" => Modal::CreateChannel { guild_id: g?, parent_id: None, category: name == "create-category" },
        "invite" => Modal::Invite { guild_id: g?, channel_id: ch? },
        "join-server" => Modal::JoinServer,
        "kick" | "ban" => Modal::Moderate { guild_id: g?, user_id: user()?, ban: name == "ban" },
        "custom-status" => Modal::CustomStatus,
        // The newest message of the open channel.
        "remind" => Modal::Remind { channel_id: ch?, message_id: state.messages.get(&ch?)?.msgs.last()?.id },
        "forward" => Modal::Forward { channel_id: ch?, message_id: state.messages.get(&ch?)?.msgs.last()?.id },
        "privacy" => Modal::PrivacySettings { guild_id: g? },
        // Sample questions instead of a fetch (`server_menu::demo_verification`).
        "apply" | "apply-rules" => Modal::JoinApplication { guild_id: g? },
        _ => return None,
    })
}

fn parse_size(s: &str) -> Option<egui::Vec2> {
    let (w, h) = s.split_once('x')?;
    Some(egui::vec2(w.trim().parse().ok()?, h.trim().parse().ok()?))
}

/// Format an ISO-8601 timestamp for message headers ("Today at 14:03", "12/03/2026 09:15").
pub fn format_timestamp(iso: &str) -> String {
    use chrono::{DateTime, Local};
    let Ok(t) = DateTime::parse_from_rfc3339(iso) else { return String::new() };
    let t = t.with_timezone(&Local);
    let today = Local::now().date_naive();
    let d = t.date_naive();
    if d == today {
        format!("Today at {}", t.format("%H:%M"))
    } else if today.pred_opt() == Some(d) {
        format!("Yesterday at {}", t.format("%H:%M"))
    } else {
        t.format("%d.%m.%Y %H:%M").to_string()
    }
}

/// 0xRRGGBB -> Color32.
pub fn rgb(c: u32) -> egui::Color32 {
    egui::Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Status dot color (presence, connection state): online `positive`, idle `attention`, Do not
/// disturb `danger`, offline muted (docs/DESIGN.md §2.2); streaming is `Tokens::live`.
pub fn presence_color(p: Presence, c: &ThemeColors) -> egui::Color32 {
    match p {
        Presence::Online => crate::theme::Tokens::new(c).positive,
        Presence::Idle => crate::theme::Tokens::new(c).attention,
        Presence::Dnd => c.danger.to_color32(),
        Presence::Offline => c.text_muted.to_color32(),
    }
}

/// Call first in a `horizontal_wrapped` row that mixes labels with buttons or combo boxes: rows
/// start `interact_size.y` tall, shorter than a themed button, which leaves labels a few px high.
pub fn button_row(ui: &mut egui::Ui) {
    ui.set_row_height(ui.text_style_height(&egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.y);
}

/// Bold font of the given size.
pub fn bold(size: f32) -> egui::FontId {
    egui::FontId::new(size, crate::theme::bold_family())
}

/// Semibold (600) font of the given size: titles, headings, labels (docs/DESIGN.md §3).
pub fn semibold(size: f32) -> egui::FontId {
    egui::FontId::new(size, crate::theme::semibold_family())
}

/// Single-line galley, truncated with "…" at `max_width`.
pub fn one_line(ui: &egui::Ui, text: impl Into<String>, font: egui::FontId, color: egui::Color32, max_width: f32) -> Arc<egui::Galley> {
    let text = text.into();
    let mut job = egui::text::LayoutJob { break_on_newline: false, ..Default::default() };
    crate::theme::append_boxed(&mut job, &text, &egui::TextFormat::simple(font, color));
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(0.0));
    ui.painter().layout_job(job)
}

/// Circular avatar (via `images::circle_image`) placed at `rect` without moving the cursor.
pub fn avatar_at(ui: &mut egui::Ui, images: &mut images::ImageCache, url: &str, rect: egui::Rect, animate: bool, opacity: f32) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    child.multiply_opacity(opacity);
    images::circle_image(&mut child, images, url, rect.width(), animate);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialog_widths_snap_to_the_three_tiers() {
        let snapped: Vec<f32> = [442.0, 460.0, 480.0, 520.0, 602.0, 800.0].into_iter().map(dialog_width).collect();
        assert_eq!(snapped, [440.0, 440.0, 520.0, 520.0, 640.0, 800.0]);
    }

    #[test]
    fn same_widgets_ignores_empty_layers_only() {
        let layer = |name| egui::LayerId::new(egui::Order::Foreground, egui::Id::unique(name));
        let w = |id, layer_id, y: f32| {
            let rect = egui::Rect::from_min_size(egui::pos2(0.0, y), egui::vec2(10.0, 10.0));
            egui::WidgetRect { id: egui::Id::unique(id), parent_id: egui::Id::NULL, layer_id, rect, interact_rect: rect, sense: egui::Sense::click(), enabled: true, visible: true }
        };
        let rects = |ws: &[egui::WidgetRect]| {
            let mut r = egui::WidgetRects::default();
            for &x in ws {
                r.insert(x.layer_id, x, egui::InteractOptions::default());
            }
            r
        };
        let (a, b) = (w("a", layer("main"), 0.0), w("b", layer("main"), 10.0));
        let one = rects(&[a, b]);
        assert!(same_widgets(&one, &rects(&[a, b])));
        // A recycled pass keeps an empty list for a layer gone since (a closed tooltip).
        let mut recycled = rects(&[a, b, w("tip", layer("tooltip"), 0.0)]);
        recycled.clear();
        for x in [a, b] {
            recycled.insert(x.layer_id, x, egui::InteractOptions::default());
        }
        assert!(same_widgets(&one, &recycled) && same_widgets(&recycled, &one));
        // Moved, added, removed, reordered, new layer: changed.
        assert!(!same_widgets(&one, &rects(&[a, w("b", layer("main"), 12.0)])));
        assert!(!same_widgets(&one, &rects(&[a, b, w("c", layer("main"), 20.0)])));
        assert!(!same_widgets(&one, &rects(&[a])));
        assert!(!same_widgets(&one, &rects(&[b, a])));
        assert!(!same_widgets(&one, &rects(&[a, b, w("tip", layer("tooltip"), 0.0)])));
        assert!(!same_widgets(&rects(&[a, b, w("tip", layer("tooltip"), 0.0)]), &one));
    }

    #[test]
    fn parse_window_size() {
        assert_eq!(parse_size("720x420"), Some(egui::vec2(720.0, 420.0)));
        assert_eq!(parse_size("720"), None);
        assert_eq!(parse_size("ax1"), None);
    }

    #[test]
    fn channel_list_gives_way_to_the_message_area() {
        assert_eq!(channel_list_width(1180.0, MEMBER_LIST_W, None), (303.0, 360.0));
        assert_eq!(channel_list_width(1000.0, MEMBER_LIST_W, None), (214.0, 214.0));
        assert_eq!(channel_list_width(720.0, 0.0, None), (198.0, 198.0));
        assert_eq!(channel_list_width(600.0, 0.0, None), (192.0, 192.0));
        assert_eq!(channel_list_width(1180.0, 0.0, Some(500.0)), (360.0, 360.0));
        assert_eq!(channel_list_width(1180.0, 0.0, Some(100.0)).0, 192.0);
    }

    #[test]
    fn focus_column_narrows_with_the_lists() {
        assert_eq!(focus_column_w(1200.0, 1.0), 1200.0);
        assert_eq!(focus_column_w(1200.0, 0.0), FOCUS_CHAT_W);
        assert_eq!(focus_column_w(600.0, 0.0), 600.0);
        assert_eq!(focus_column_w(600.0, 0.5), 600.0);
        let steps: Vec<f32> = (0..=10).map(|i| focus_column_w(1400.0, i as f32 / 10.0)).collect();
        assert!(steps.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn right_column_follows_the_page() {
        let (mut view, settings, mut state) = (ViewState::default(), Settings::default(), AppState::default());
        // DM home without a DM open is the Friends page: no right column.
        assert!(friends::is_open(&state));
        view.thread_panel = Some(Id(5));
        assert_eq!(right_column_w(&view, &settings, &state, 1180.0), 0.0);
        state.selected_channel = Some(Id(4));
        assert_eq!(right_column_w(&view, &settings, &state, 1180.0), THREAD_PANEL_W);
        assert_eq!(right_column_w(&view, &settings, &state, 900.0), 0.0);
        view.settings_open = true;
        view.backdrop_asked = true;
        assert_eq!(right_column_w(&view, &settings, &state, 1180.0), 0.0);
    }

    #[test]
    fn demo_modals_need_their_target() {
        let mut state = AppState::default();
        assert_eq!(demo_modal(&state, "join-server"), Some(Modal::JoinServer));
        assert_eq!(demo_modal(&state, "create-thread"), None);
        assert_eq!(demo_modal(&state, "nope"), None);
        state.selected_channel = Some(Id(3));
        assert_eq!(demo_modal(&state, "create-thread"), Some(Modal::CreateThread { channel_id: Id(3), from_message: None }));
    }

    #[test]
    fn open_profile_toggles() {
        let mut v = ViewState::default();
        v.open_profile(Id(1), None, egui::Pos2::ZERO);
        assert_eq!(v.profile_card.map(|p| p.user_id), Some(Id(1)));
        v.open_profile(Id(2), Some(Id(9)), egui::Pos2::ZERO);
        assert_eq!(v.profile_card.map(|p| p.user_id), Some(Id(2)));
        v.open_profile(Id(2), Some(Id(9)), egui::Pos2::ZERO);
        assert!(v.profile_card.is_none());
    }
}
