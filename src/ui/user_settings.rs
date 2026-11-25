//! User settings (FEATURES_NOTES §3.1, §3.2, layout §4.9; every page of
//! docs/SETTINGS_NOTES.md in Discord's order): a full-screen layer while
//! `ViewState::settings_open`, a searchable category list on the left
//! (`ViewState::settings_page`; the search matches page titles, option labels and keywords)
//! and one page on the right; Log Out at the bottom. Esc or the × closes it.
//! - User Settings: My Account, Profiles, Profile Cosmetics, Content & Social, Data & Privacy,
//!   Family Center, Authorized Apps, Devices, Connections, Friend Requests.
//! - Billing Settings: one page linking to discord.com. App Settings: Appearance, Accessibility,
//!   Voice & Video, Clips, Chat, Notifications, Keybinds, Language, Windows Settings, Streamer
//!   Mode, Advanced. Activity Settings: Activity Privacy, Registered Games, Game Overlay.
//!   Rustcord: Plugins, Local Profile, Theme Editor.
//! - What Rustcord can't do (billing, clip recording, Family Center linking) is a Discord-like empty state (`empty_state`) above the page's synced rows.
//!
//! Where options live: Discord's settings proto for synced ones (synced.rs, one table per page),
//! REST lists (lists.rs: devices, authorized apps, consents), `config::Settings` for this
//! device (app.rs, keybinds.rs, voice.rs). Also the Set Custom Status modal
//! (`Modal::CustomStatus`) and the lower part of the account popout (`account_popout`).
//!
//! Every account write is an `AccountCommand` sent on a click, with its progress read from
//! `AccountState::ops` (`begin`); errors (captcha, 2FA, "too fast") are shown as they arrive.
//! OWNERSHIP: user-settings engineer (docs/SOCIAL_UI_DESIGN.md); the layer chrome
//! (`settings_layer`, `nav_item`, `section`, `hint`, `dialog`) is shared in mod.rs.

mod account;
mod app;
mod connections;
mod cosmetics;
mod games;
mod keybinds;
mod lists;
pub mod local_profile;
#[cfg(windows)]
mod overlay;
mod plugins;
mod status;
mod synced;
pub mod theme_editor;
mod voice;

pub(super) use app::{frame, in_tray, logic};
pub(super) use keybinds::{Bind, shortcut};
pub(super) use status::account_popout;
pub use status::custom_status_modal;
pub(super) use voice::restore as restore_voice;

use super::icons::{self, Icon};
use super::messages::Pal;
use super::{UiCx, ViewState, hint, nav_heading, nav_item, page_title, section, settings_layer};
use crate::backend::{AccountCommand, AccountOp, Command, NotificationPatch, OpKind};
use crate::state::account::OpState;
use crate::theme::Theme;
use egui::{Align, Layout, RichText, Sense, vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    MyAccount,
    Profiles,
    Cosmetics,
    ContentSocial,
    Privacy,
    FamilyCenter,
    AuthorizedApps,
    Devices,
    Connections,
    FriendRequests,
    Billing,
    Appearance,
    Accessibility,
    VoiceVideo,
    /// Only the synced "record my voice" switch: Rustcord does not capture clips.
    Clips,
    Chat,
    Notifications,
    Keybinds,
    Language,
    Windows,
    StreamerMode,
    Advanced,
    ActivityPrivacy,
    RegisteredGames,
    GameOverlay,
    /// Rustcord: native Vencord-style plugins (plugins.rs).
    Plugins,
    LocalProfile,
    /// Rustcord: the live theme editor (theme_editor.rs).
    ThemeEditor,
}

impl Page {
    /// Nav groups, in Discord's order.
    const GROUPS: [(&str, &[Page]); 5] = [
        (
            "User settings",
            &[
                Page::MyAccount,
                Page::Profiles,
                Page::Cosmetics,
                Page::ContentSocial,
                Page::Privacy,
                Page::FamilyCenter,
                Page::AuthorizedApps,
                Page::Devices,
                Page::Connections,
                Page::FriendRequests,
            ],
        ),
        ("Billing settings", &[Page::Billing]),
        (
            "App settings",
            &[
                Page::Appearance,
                Page::Accessibility,
                Page::VoiceVideo,
                Page::Clips,
                Page::Chat,
                Page::Notifications,
                Page::Keybinds,
                Page::Language,
                Page::Windows,
                Page::StreamerMode,
                Page::Advanced,
            ],
        ),
        // The overlay is Windows only (ui/overlay): elsewhere the page is in no group, so nav, search and `from_name` skip it.
        ("Activity settings", if cfg!(windows) { &[Page::ActivityPrivacy, Page::RegisteredGames, Page::GameOverlay] } else { &[Page::ActivityPrivacy, Page::RegisteredGames] }),
        ("Rustcord", &[Page::Plugins, Page::LocalProfile, Page::ThemeEditor]),
    ];

    /// Title, `RUSTCORD_OPEN=settings:<name>` name, extra search words (lowercase).
    fn info(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Page::MyAccount => (
                "My account",
                "my-account",
                "display name username email phone password two-factor 2fa authenticator sms backup codes security keys account standing disable delete",
            ),
            Page::Profiles => ("Profiles", "profiles", "pronouns about me bio avatar banner colour color"),
            Page::Cosmetics => ("Profile cosmetics", "cosmetics", "avatar decoration profile effect nameplate frame"),
            Page::ContentSocial => ("Content & social", "content", "messaging permissions dms spam sensitive explicit filter"),
            Page::Privacy => ("Data & privacy", "privacy", "data consent personalize statistics read receipts"),
            Page::FamilyCenter => ("Family Center", "family", "teen parent"),
            Page::AuthorizedApps => ("Authorized apps", "apps", "oauth applications deauthorize bots"),
            Page::Devices => ("Devices", "devices", "sessions log out logged in"),
            Page::Connections => ("Connections", "connections", "accounts spotify steam github twitch xbox"),
            Page::FriendRequests => ("Friend requests", "friend-requests", "friends"),
            Page::Billing => ("Nitro & billing", "billing", "nitro boost subscriptions gift payment"),
            Page::Appearance => ("Appearance", "appearance", "theme dark light gradient font member list colours colors density"),
            Page::Accessibility => {
                ("Accessibility", "accessibility", "saturation role colours colors font size scaling zoom spacing reduced motion animation gif tts")
            }
            Page::VoiceVideo => ("Voice & video", "voice", "microphone speaker camera push to talk input output"),
            Page::Clips => ("Clips", "clips", "clips recording record voice"),
            Page::Chat => ("Chat", "chat", "text images embeds links media emoji reactions spoilers"),
            Page::Notifications => ("Notifications", "notifications", "desktop taskbar push mute servers"),
            Page::Keybinds => ("Keybinds", "keybinds", "shortcuts hotkeys keyboard"),
            Page::Language => ("Language", "language", "locale"),
            // One id on every OS; title and rows say Windows, Linux or macOS.
            Page::Windows => (app::DESKTOP.title, "windows", "startup login autostart minimize tray menu bar close update updates version upgrade"),
            Page::StreamerMode => ("Streamer mode", "streamer", "streaming obs hide"),
            Page::Advanced => ("Advanced", "advanced", "developer mode copy id hardware acceleration"),
            Page::ActivityPrivacy => ("Activity privacy", "activity", "activity status game join"),
            Page::RegisteredGames => ("Registered games", "games", "games detection"),
            Page::GameOverlay => ("Game overlay", "overlay", "overlay in-game voice widget notifications corner position avatar names"),
            Page::Plugins => ("Plugins", "plugins", "vencord"),
            Page::LocalProfile => ("Local profile", "local-profile", "local banner avatar this device"),
            Page::ThemeEditor => ("Theme editor", "theme-editor", "customize theme colours colors import export"),
        }
    }

    /// The labels of the page's own (this device) rows, for the search.
    /// shortcut: kept by hand next to the pages' code; move the labels into shared consts if they drift.
    fn local_labels(self) -> &'static [&'static str] {
        match self {
            Page::Appearance => &["Theme", "Gradients", "Message display", "Cozy", "Compact", "Use Discord font (gg sans)", "Show member list"],
            Page::Accessibility => &[
                "Saturation",
                "Show role colours in names",
                "Chat font scaling",
                "Zoom level",
                "Space between message groups",
                "Reduced motion",
                "Play animated avatars, emoji and GIFs",
            ],
            Page::Windows => &app::DESKTOP_LABELS,
            Page::StreamerMode => &["Streamer mode", "Hide personal information", "Hide invite links", "Disable notifications", "Hide window from screen capture"],
            Page::Privacy => &["Send read receipts"],
            Page::Notifications => &["Desktop notifications", "Flash the taskbar"],
            Page::VoiceVideo => &["Input volume", "Input mode", "Voice activity", "Push to talk", "Input sensitivity", "Microphone and sound", "Mute", "Deafen", "Video", "Camera"],
            Page::Advanced => &["Hardware acceleration"],
            Page::GameOverlay => &["Enable in-game overlay", "Position", "Avatar size", "Show names", "Show users"],
            Page::ThemeEditor => &[
                "Base", "Dark", "Light", "Gradient", "Background", "Sidebar", "Server bar", "Surface", "Text", "Muted text", "Accent", "Text on accent", "Message hover",
                "Mention highlight", "Links", "Danger", "Code background", "UI font", "Monospace font", "Font size", "Corner radius", "Gradient angle", "Panel opacity",
            ],
            _ => &[],
        }
    }

    pub fn title(self) -> &'static str {
        self.info().0
    }

    /// `RUSTCORD_OPEN=settings:<name>`: my-account, profiles, cosmetics, content, privacy,
    /// family, apps, devices, connections, friend-requests, billing, appearance, accessibility,
    /// voice, clips, chat, notifications, keybinds, language, windows, streamer, advanced, activity,
    /// games, overlay, plugins, local-profile, theme-editor.
    pub fn from_name(name: &str) -> Option<Page> {
        Self::GROUPS.iter().flat_map(|g| g.1.iter().copied()).find(|p| p.info().1 == name)
    }

    /// The sidebar search: `q` (lowercase, trimmed) in the title, the keywords, a local or synced
    /// option's label (Keybinds: the actions).
    fn matches(self, q: &str) -> bool {
        let (title, _, words) = self.info();
        let has = |s: &str| s.to_lowercase().contains(q);
        let binds = self == Page::Keybinds && Bind::ALL.iter().any(|b| has(b.label()));
        q.is_empty()
            || has(title)
            || words.contains(q)
            || binds
            || self.local_labels().iter().any(|l| has(l))
            || synced::groups(self).iter().any(|g| has(g.title) || g.rows.iter().any(|r| has(r.label)))
    }
}

/// Drafts of the user settings forms (in `ViewState::user_settings_ui`). Per-opening parts are
/// reset when the layer was not drawn in the previous pass; the password buffer is wiped then.
#[derive(Default)]
pub struct UserSettingsUi {
    /// Pass the layer was last drawn in.
    pass: u64,
    status: status::StatusForm,
    account: account::AccountForm,
    /// Profiles page; None until it is first shown (that sends the one `FetchProfile`).
    profile: Option<account::ProfileForm>,
    voice: voice::VoiceForm,
    /// Profile Cosmetics tab / search; lives as long as the login (like the catalog in state).
    cosmetics: cosmetics::Form,
    /// The sidebar's search text (per opening).
    search: String,
    /// The page open when the search moved away from it (it matched nothing): shown again once
    /// the search is cleared, unless a result was clicked meanwhile.
    before_search: Option<Page>,
    /// Pages whose one fetch went out in this opening (lists.rs).
    fetched: Vec<Page>,
    /// Profiles > Server Profiles: the server and its nickname draft (None until first shown);
    /// None = the User Profile tab. Cleared when the layer closes.
    server_tab: Option<(crate::model::Id, Option<String>)>,
    /// Log Out was clicked: the "Are you sure?" dialog is open.
    ask_logout: bool,
}

/// Server menu > Edit Per-server Profile: the settings layer on Profiles, that server's tab.
pub fn open_server_profile(view: &mut ViewState, guild_id: crate::model::Id) {
    view.profile_card = None;
    (view.settings_open, view.settings_page) = (true, Page::Profiles);
    view.user_settings_ui.server_tab = Some((guild_id, None));
}

/// The layer (main_layout calls it while `settings_open`).
pub fn show(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    let pass = ui.ctx().cumulative_pass_nr();
    if cx.view.user_settings_ui.pass + 1 != pass {
        opened(cx);
    }
    cx.view.user_settings_ui.pass = pass;
    if let Some(q) = ui.ctx().data_mut(|d| d.remove_temp::<String>(search_demo_id())) {
        cx.view.user_settings_ui.search = q;
    }
    // Reading the themes folder is disk I/O: once per opening of the layer.
    if cx.view.settings_themes.is_none() {
        cx.view.settings_themes = Some(Theme::load_all());
    }
    let (mut changed, mut log_out) = (false, false);
    let before = cx.view.settings_page;
    // Esc while the Log Out question is open closes only the question.
    let asking = cx.view.user_settings_ui.ask_logout;
    // The 2FA re-check of a waiting action, on any page; before the layer so its Esc wins.
    account::security::mfa_dialog(ui, cx);
    let close = settings_layer(
        ui,
        cx,
        "user_settings",
        |ui, cx| log_out = nav(ui, cx),
        |ui, cx| {
            page_title(ui, cx.theme, cx.view.settings_page.title());
            changed = page(ui, cx);
        },
    ) && !asking;
    cx.view.user_settings_ui.ask_logout |= log_out;
    if cx.view.user_settings_ui.ask_logout {
        let theme = &*cx.theme;
        let resp = super::dialog(ui.ctx(), theme, "rc_logout", 440.0, "Log out of Rustcord?", |ui| {
            super::dialog_text(ui, theme, "You'll need to sign in again, and this device forgets your cached messages.");
            super::dialog_buttons(ui, theme, "Log out", true, true)
        });
        match resp.inner {
            Some(true) => {
                logout(cx);
                return;
            }
            Some(false) => cx.view.user_settings_ui.ask_logout = false,
            None if resp.should_close() => cx.view.user_settings_ui.ask_logout = false,
            None => {}
        }
    }
    let left = close || cx.view.settings_page != before;
    if left && before == Page::Cosmetics {
        cosmetics::leave(ui.ctx(), cx);
    }
    if left && before == Page::Keybinds {
        keybinds::stop_recording(ui.ctx());
    }
    if close {
        cx.view.settings_open = false;
        opened(cx);
        cx.view.user_settings_ui.server_tab = None;
    }
    if changed
        && let Err(e) = cx.settings.save()
    {
        cx.view.toast(format!("Could not save settings: {e}"), true);
    }
}

/// The nav column: search box, the groups (only matching pages while searching), Log Out.
/// True when Log Out was clicked (the caller asks before logging out).
fn nav(ui: &mut egui::Ui, cx: &mut UiCx<'_>) -> bool {
    let search = egui::TextEdit::singleline(&mut cx.view.user_settings_ui.search).hint_text("Search").desired_width(f32::INFINITY);
    ui.add(search);
    ui.add_space(6.0);
    let q = cx.view.user_settings_ui.search.trim().to_lowercase();
    cx.view.settings_page = search_page(cx.view.settings_page, &mut cx.view.user_settings_ui.before_search, &q);
    let mut any = false;
    for (heading, pages) in Page::GROUPS {
        let mut shown = pages.iter().copied().filter(|p| p.matches(&q)).peekable();
        if shown.peek().is_none() {
            continue;
        }
        any = true;
        nav_heading(ui, cx.theme, heading);
        for p in shown {
            if nav_item(ui, cx.theme, p.title(), cx.view.settings_page == p, false) {
                cx.view.settings_page = p;
                cx.view.user_settings_ui.before_search = None;
            }
        }
        ui.separator();
    }
    if !any {
        hint(ui, cx.theme, "No settings match. Try another word.");
        ui.separator();
    }
    nav_item(ui, cx.theme, "Log out", false, true)
}

/// The page to show for the search `q` (lowercase, trimmed): the open one while it matches,
/// else the first match (the nav highlights it), remembering the open one in `before`; once
/// `q` is empty again, the remembered page.
fn search_page(open: Page, before: &mut Option<Page>, q: &str) -> Page {
    if q.is_empty() {
        return before.take().unwrap_or(open);
    }
    if open.matches(q) {
        return open;
    }
    let Some(first) = Page::GROUPS.iter().flat_map(|g| g.1.iter().copied()).find(|p| p.matches(q)) else { return open };
    before.get_or_insert(open);
    first
}

/// `RUSTCORD_OPEN=settings-search:<q>`: the sidebar search filled in (applied when the layer
/// opens, after its per-opening reset).
pub fn demo_search(ctx: &egui::Context, q: &str) {
    ctx.data_mut(|d| d.insert_temp(search_demo_id(), q.to_owned()));
}

fn search_demo_id() -> egui::Id {
    egui::Id::unique("rc_settings_search_demo")
}

/// The layer opened or closed: drop per-opening drafts (unsaved edits, the password, finished
/// progress entries) so nothing stale shows next time.
fn opened(cx: &mut UiCx<'_>) {
    let u = &mut cx.view.user_settings_ui;
    u.account = Default::default();
    u.voice.opened();
    u.search.clear();
    u.before_search = None;
    u.fetched.clear();
    if let Some(p) = &mut u.profile {
        // A save answered while the layer was closed is kept, not reset to the old base.
        p.apply_saved(&mut cx.state.account);
        p.discard();
    }
    for kind in [
        OpKind::UpdateGlobalName,
        OpKind::UpdateUsername,
        OpKind::UpdateAvatar,
        OpKind::UpdateBanner,
        OpKind::UpdateBio,
        OpKind::UpdatePronouns,
        OpKind::UpdateAccentColor,
    ] {
        finish_op(cx, kind);
    }
}

/// The selected page: its own controls, then its synced options (synced.rs). True when
/// `cx.settings` changed (the caller saves them).
fn page(ui: &mut egui::Ui, cx: &mut UiCx<'_>) -> bool {
    let mut changed = false;
    let p = cx.view.settings_page;
    match p {
        Page::MyAccount => account::my_account(ui, cx),
        Page::Profiles => account::profiles(ui, cx),
        Page::Cosmetics => cosmetics::show(ui, cx),
        Page::Connections if cx.settings.streamer.hides_personal() => hint(ui, cx.theme, "Hidden by Streamer Mode."),
        Page::Connections => connections::show(ui, cx),
        Page::VoiceVideo => changed |= voice::show(ui, cx),
        Page::Privacy => {
            lists::consents(ui, cx);
            section(ui, cx.theme, "Read state");
            changed |= switch_row(
                ui,
                cx.theme,
                "Send read receipts",
                Some("Off: channels are marked read only in this app; your other devices keep them unread."),
                &mut cx.settings.send_read_acks,
            );
        }
        Page::Devices => lists::devices(ui, cx),
        Page::AuthorizedApps => lists::authorized_apps(ui, cx),
        Page::FamilyCenter => empty_state(ui, cx, "👪", "Set up in the mobile app", "Linking a teen's account happens in the Discord mobile app.", None),
        Page::Billing => empty_state(ui, cx, "💳", "Managed on discord.com", "Nitro, boosts, subscriptions, gifts and payments.", Some("premium")),
        Page::RegisteredGames => games::show(ui, cx),
        #[cfg(windows)]
        Page::GameOverlay => changed |= overlay::show(ui, cx),
        #[cfg(not(windows))]
        Page::GameOverlay => {}
        Page::Clips => empty_state(ui, cx, "🎬", "No clip recording", "Rustcord doesn't record clips. The switch below is about clips others record.", None),
        Page::LocalProfile => local_profile::show(ui, cx),
        Page::Plugins => plugins::show(ui, cx),
        Page::ThemeEditor => changed |= theme_editor::show(ui, cx),
        Page::Appearance => {
            changed |= appearance(ui, cx);
            section(ui, cx.theme, "Message display");
            synced::message_display(ui, cx);
            section(ui, cx.theme, "Font");
            if switch_row(
                ui,
                cx.theme,
                "Use Discord font (gg sans)",
                Some("Downloaded from discord.com into the cache, never bundled. Off or offline: Segoe UI."),
                &mut cx.settings.use_discord_font,
            ) {
                crate::theme::set_discord_font(ui.ctx(), cx.settings.use_discord_font);
                changed = true;
            }
            section(ui, cx.theme, "Layout");
            changed |= switch_row(
                ui,
                cx.theme,
                "Show member list",
                Some("Next to server channels and group DMs; hidden in narrow windows."),
                &mut cx.settings.show_member_list,
            );
        }
        Page::Accessibility => changed |= app::accessibility(ui, cx),
        Page::Notifications => changed |= notification_switches(ui, cx),
        Page::Keybinds => changed |= keybinds::show(ui, cx),
        Page::Windows => changed |= app::windows(ui, cx),
        Page::StreamerMode => changed |= app::streamer(ui, cx),
        Page::Advanced => app::advanced(ui, cx),
        Page::ContentSocial | Page::FriendRequests | Page::Chat | Page::Language | Page::ActivityPrivacy => {}
    }
    // Profiles > Server Profiles is about one server only.
    if !(p == Page::Profiles && cx.view.user_settings_ui.server_tab.is_some()) {
        synced::show(ui, cx, synced::groups(p));
    }
    if p == Page::Notifications {
        guild_notifications(ui, cx);
    }
    changed
}

/// Discord's settings in the browser (a link the user clicks; Rustcord sends nothing).
/// shortcut: `section` follows the web client's `/settings/<section>` route; unverified per page.
fn open_in_browser(ui: &mut egui::Ui, section: &str) {
    let url = format!("https://discord.com/settings/{section}");
    let resp = super::messages::external_button(ui, "Open in browser");
    if resp.on_hover_text(&url).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
}

/// What Rustcord can't offer on a page (§9 empty state): a 48 px muted icon, a heading line,
/// one sentence why, and the browser link if any. Synced rows may follow under it.
fn empty_state(ui: &mut egui::Ui, cx: &UiCx<'_>, icon: &str, title: &str, text: &str, section: Option<&str>) {
    let pal = Pal::new(cx.theme);
    ui.add_space(32.0);
    ui.vertical_centered(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(48.0, 48.0), Sense::hover());
        match icons::from_glyph(icon) {
            Some(i) => icons::paint(ui.painter(), i, rect, pal.muted, pal.bg),
            None => {
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, icon, egui::FontId::new(44.0, crate::theme::icon_family()), pal.muted);
            }
        }
        ui.add_space(16.0);
        ui.label(RichText::new(title).font(super::semibold(pal.heading())).color(pal.text));
        ui.add(egui::Label::new(RichText::new(text).size(pal.meta()).color(pal.muted)).wrap());
        if let Some(section) = section {
            ui.add_space(16.0);
            open_in_browser(ui, section);
        }
    });
    ui.add_space(16.0);
}

/// Theme picker: round swatches (tooltip = name) of the flat themes and of the gradient themes
/// (the Nitro colour themes; free, only on this device). Returns true when the settings changed.
fn appearance(ui: &mut egui::Ui, cx: &mut UiCx<'_>) -> bool {
    // Taken for the frame (put back below): `folder_button` needs `cx`.
    let themes = cx.view.settings_themes.take().unwrap_or_default();
    let mut picked: Option<Theme> = None;
    section(ui, cx.theme, "Theme");
    ui.horizontal_wrapped(|ui| {
        picked = theme_editor::swatches(ui, &themes, cx.theme, false);
        folder_button(ui, cx, &crate::config::paths().themes_dir(), "Themes folder: add your own as .toml files");
        let pal = Pal::new(cx.theme);
        if super::messages::vector_button(ui, &pal, Icon::Edit, "Customize theme", 28.0, false, pal.text).clicked() {
            cx.view.settings_page = Page::ThemeEditor;
        }
    });
    section(ui, cx.theme, "Gradients");
    let gradient = ui.horizontal_wrapped(|ui| theme_editor::swatches(ui, &themes, cx.theme, true)).inner;
    picked = picked.or(gradient);
    hint(ui, cx.theme, "Free, only on this device.");
    cx.view.settings_themes = Some(themes);

    let Some(theme) = picked.filter(|t| *t != *cx.theme) else { return false };
    cx.settings.theme = theme.name.clone();
    *cx.theme = theme;
    cx.view.theme_dirty = true;
    ui.ctx().request_repaint();
    true
}

/// Notifications on this device: desktop notifications and taskbar flashing.
fn notification_switches(ui: &mut egui::Ui, cx: &mut UiCx<'_>) -> bool {
    let s = &mut *cx.settings;
    let desktop = switch_row(
        ui,
        cx.theme,
        "Desktop notifications",
        Some("Direct messages and mentions, following each server's settings. Silenced while you're on Do Not Disturb."),
        &mut s.desktop_notifications,
    );
    desktop | switch_row(ui, cx.theme, "Flash the taskbar", Some("When a notification arrives while Rustcord is in the background."), &mut s.flash_taskbar)
}

/// Each server's level and mute (`UpdateGuildNotifications`, one request per pick; a server's
/// controls wait while its change is on the way).
fn guild_notifications(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    if cx.state.guilds.is_empty() {
        return;
    }
    section(ui, cx.theme, "Servers");
    const LEVELS: [&str; 3] = ["All messages", "Only @mentions", "Nothing"];
    let pal = Pal::new(cx.theme);
    let mut send = None;
    for g in &cx.state.guilds {
        let s = cx.state.guild_settings.get(&Some(g.id));
        // 3 = inherit: the server's default.
        let level = s.map_or(3, |s| s.message_notifications);
        let level = if level == 3 { g.default_message_notifications.min(1) } else { level.min(2) };
        let muted = cx.state.is_guild_muted(g.id);
        let op = AccountOp { kind: OpKind::UpdateGuildNotifications, target: Some(g.id) };
        let busy = cx.state.account.ops.get(&op) == Some(&OpState::Pending);
        ui.horizontal(|ui| {
            super::button_row(ui);
            let (icon, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
            // The server bar's icons (same URL and size: no new requests), only for rows in view.
            const PX: u32 = super::sidebar::ICON_PX;
            if ui.is_rect_visible(icon) {
                match crate::api::cdn::guild_icon_url(g, PX, false).and_then(|url| cx.images.get(ui.ctx(), &url, PX, false)) {
                    Some(tex) => egui::Image::from_texture(tex).corner_radius(10).paint_at(ui, icon),
                    None => super::images::paint_initial(ui.painter(), icon, 10.0, g.id.0, &g.name, pal.dark),
                }
            }
            let w = ui.available_width() - 210.0;
            ui.allocate_ui_with_layout(vec2(w.max(60.0), 32.0), Layout::left_to_right(Align::Center), |ui| {
                ui.add(egui::Label::new(RichText::new(&g.name).color(pal.text)).truncate());
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    if bell_button(ui, &pal, muted).clicked() {
                        send = Some((g.id, NotificationPatch { muted: Some(!muted), ..Default::default() }));
                    }
                    let mut pick = level;
                    egui::ComboBox::from_id_salt(("rc_guild_notify", g.id)).width(150.0).selected_text(LEVELS[level as usize]).icon(combo_icon).show_ui(ui, |ui| {
                        for (i, l) in LEVELS.iter().enumerate() {
                            ui.selectable_value(&mut pick, i as u8, *l);
                        }
                    });
                    if pick != level {
                        send = Some((g.id, NotificationPatch { level: Some(pick), ..Default::default() }));
                    }
                    if busy {
                        ui.add(egui::Spinner::new().size(14.0));
                    }
                });
            });
        });
    }
    if let Some((guild_id, patch)) = send {
        let cmd = AccountCommand::UpdateGuildNotifications { guild_id: Some(guild_id), patch };
        if cx.state.account.begin(cmd.op()) {
            cx.cmd.send(Command::Account(cmd));
        }
    }
    // Answers: the gateway update shows a success; a failure becomes a toast.
    let mut failed = Vec::new();
    cx.state.account.ops.retain(|op, s| match s {
        OpState::Failed(e) if op.kind == OpKind::UpdateGuildNotifications => {
            failed.push(std::mem::take(e));
            false
        }
        _ => op.kind != OpKind::UpdateGuildNotifications || *s == OpState::Pending,
    });
    for e in failed {
        cx.view.toast(e, true);
    }
}

fn logout(cx: &mut UiCx<'_>) {
    cx.cmd.send(Command::Logout);
    cx.state.reset();
    cx.images.clear();
    // Fresh view state: login screen, no drafts or other leftovers of this account.
    *cx.view = ViewState::default();
    std::thread::spawn(crate::cache::clear_all);
}

// ---------------------------------------------------------------------------
// Small shared pieces of the pages
// ---------------------------------------------------------------------------

/// Progress of one of my own account writes (`target: None`).
fn my_op(kind: OpKind) -> AccountOp {
    AccountOp { kind, target: None }
}

fn op_state(cx: &UiCx<'_>, kind: OpKind) -> Option<OpState> {
    cx.state.account.ops.get(&my_op(kind)).cloned()
}

/// Forget a finished (or failed) write; a pending one stays until its answer.
fn finish_op(cx: &mut UiCx<'_>, kind: OpKind) {
    let ops = &mut cx.state.account.ops;
    if ops.get(&my_op(kind)).is_some_and(|s| *s != OpState::Pending) {
        ops.remove(&my_op(kind));
    }
}

/// Spinner while a write is on its way, its error text (selectable: captcha texts carry a link).
fn op_line(ui: &mut egui::Ui, pal: &Pal, state: Option<&OpState>, pending: &str) {
    match state {
        Some(OpState::Pending) => {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0).color(pal.muted));
                ui.label(RichText::new(pending).size(pal.meta()).color(pal.muted));
            });
        }
        Some(OpState::Failed(e)) => {
            ui.add(egui::Label::new(RichText::new(e).size(pal.meta()).color(pal.danger)).wrap().selectable(true));
        }
        _ => {}
    }
}

/// Field label ("Display name", as written): the label role, 8 px above its field.
fn field_label(ui: &mut egui::Ui, pal: &Pal, text: &str) {
    ui.label(RichText::new(text).font(super::semibold(pal.label())).color(pal.muted));
    ui.add_space(8.0 - ui.spacing().item_spacing.y);
}

/// The view's one primary button (Save changes, Edit profile): `messages::button` `Primary`.
fn primary_button(ui: &mut egui::Ui, theme: &Theme, text: &str, enabled: bool) -> egui::Response {
    super::messages::button(ui, &Pal::new(theme), super::messages::Btn::Primary, text, enabled)
}

/// Folder button that opens `dir` in the file manager (created first); `what` and the path are
/// its tooltip, so the page needs no path text.
fn folder_button(ui: &mut egui::Ui, cx: &mut UiCx<'_>, dir: &std::path::Path, what: &str) {
    let pal = Pal::new(cx.theme);
    let resp = super::messages::vector_button(ui, &pal, Icon::Directory, &format!("{what}\n{}", dir.display()), 28.0, false, pal.text);
    if !resp.clicked() {
        return;
    }
    let opener = if cfg!(windows) { "explorer" } else if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    if let Err(e) = std::fs::create_dir_all(dir).and_then(|()| std::process::Command::new(opener).arg(dir).spawn().map(drop)) {
        cx.view.toast(format!("Could not open the folder: {e}"), true);
    }
}

/// Mute toggle of a server row: a bell, crossed out while muted. The tooltip is the action.
fn bell_button(ui: &mut egui::Ui, pal: &Pal, muted: bool) -> egui::Response {
    let tip = if muted { "Unmute server" } else { "Mute server" };
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Switch, ui.is_enabled(), muted, "Mute server"));
    if ui.is_rect_visible(rect) {
        let hot = resp.hovered() || resp.has_focus();
        let bg = if hot { pal.bg.blend(pal.tint()) } else { pal.bg };
        if hot {
            ui.painter().rect_filled(rect, pal.radius, bg);
        }
        let color = if muted { pal.danger } else if hot { pal.text } else { pal.muted };
        let bell = if muted { Icon::BellOff } else { Icon::Bell };
        icons::paint(ui.painter(), bell, egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(20.0)), color, bg);
    }
    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The shared pill switch (`messages::switch`, §9). `changed()` when clicked.
fn switch(ui: &mut egui::Ui, pal: &Pal, on: &mut bool, label: &str) -> egui::Response {
    super::messages::switch(ui, pal, on, label)
}

/// A switch row (`setting_row`). True when toggled.
fn switch_row(ui: &mut egui::Ui, theme: &Theme, label: &str, hint_text: Option<&str>, on: &mut bool) -> bool {
    let pal = Pal::new(theme);
    setting_row(ui, theme, label, hint_text, vec2(40.0, 24.0), |ui| switch(ui, &pal, on, label).changed())
}

/// A setting row (§12, min 56 tall): the title (ui 14/500) and an optional description (meta,
/// muted) on the left, the `ctl`-sized control on the right, level with the title. A row that
/// directly follows another one (same group: no heading or other content between them) gets a
/// `rule` above it, so a group has rules between its rows and none after the last.
fn setting_row<R>(ui: &mut egui::Ui, theme: &Theme, label: &str, hint_text: Option<&str>, ctl: egui::Vec2, control: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let pal = Pal::new(theme);
    // Where the last row ended, in this pass.
    let last_end = egui::Id::unique("rc_settings_row_end");
    let (pass, start) = (ui.ctx().cumulative_pass_nr(), ui.cursor().min);
    let follows = ui.data(|d| d.get_temp::<(u64, egui::Pos2)>(last_end)).is_some_and(|(p, end)| p == pass && end.distance(start) < 0.5);
    if follows {
        let (rule, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
        ui.painter().hline(rule.x_range(), rule.center().y, egui::Stroke::new(1.0, pal.tk.rule));
    }
    let title_font = egui::FontId::new(pal.ui(), crate::theme::medium_family());
    let line = ui.fonts_mut(|f| f.row_height(&title_font));
    let inner = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(0, 12))
        .show(ui, |ui| {
            ui.set_min_height(32.0);
            ui.horizontal_top(|ui| {
                let text_w = (ui.available_width() - ctl.x - 16.0).max(80.0);
                ui.allocate_ui_with_layout(vec2(text_w, 0.0), Layout::top_down(Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    // Centres the title line on the control.
                    ui.add_space(((ctl.y - line) / 2.0).max(0.0));
                    ui.add(egui::Label::new(RichText::new(label).font(title_font).color(pal.text)).wrap());
                    if let Some(h) = hint_text {
                        ui.add(egui::Label::new(RichText::new(h).size(pal.meta()).color(pal.muted)).wrap());
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Min), control).inner
            })
            .inner
        })
        .inner;
    let end = ui.cursor().min;
    ui.data_mut(|d| d.insert_temp(last_end, (pass, end)));
    inner
}

/// The settings dropdowns' arrow (`ComboBox::icon`): the icon set's `ChevronDown` in the
/// widget's text colour, instead of egui's filled triangle.
pub(super) fn combo_icon(ui: &egui::Ui, rect: egui::Rect, visuals: &egui::style::WidgetVisuals, _open: bool) {
    let r = egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(16.0));
    icons::paint(ui.painter(), Icon::ChevronDown, r, visuals.fg_stroke.color, visuals.weak_bg_fill);
}

#[cfg(test)]
mod tests {
    use super::Page;

    #[test]
    fn pages_have_names_and_search_finds_options() {
        let all: Vec<Page> = Page::GROUPS.iter().flat_map(|g| g.1.iter().copied()).collect();
        // The game overlay page is Windows only.
        assert_eq!(all.len(), 27 + usize::from(cfg!(windows)), "every page is in exactly one group");
        assert_eq!(Page::from_name("overlay").is_some(), cfg!(windows));
        for p in &all {
            assert_eq!(Page::from_name(p.info().1), Some(*p));
        }
        let hits = |q: &str| all.iter().copied().filter(|p| p.matches(q)).collect::<Vec<_>>();
        assert_eq!(hits("zoom"), [Page::Accessibility]);
        // Local rows' labels, not only keywords.
        assert_eq!(hits("gg sans"), [Page::Appearance]);
        assert_eq!(hits("discord font"), [Page::Appearance]);
        assert!(hits("compact").contains(&Page::Appearance), "Message Display");
        assert!(hits("screen capture").contains(&Page::StreamerMode));
        assert!(hits("open inbox").contains(&Page::Keybinds), "a keybind action");
        assert!(hits("spoiler").contains(&Page::Chat), "a synced option's label");
        assert!(hits("developer").contains(&Page::Advanced));
        assert_eq!(hits("upgrade"), [Page::Windows]);
        assert_eq!(hits("install updates"), [Page::Windows], "the Updates rows");
        assert_eq!(hits("authenticator"), [Page::MyAccount]);
        assert_eq!(hits("avatar").first(), Some(&Page::Profiles), "the upload rows moved to Profiles");
        assert_eq!(hits("").len(), all.len());
        assert!(hits("no such setting").is_empty());
    }

    #[test]
    fn rules_only_between_rows_of_a_group() {
        let ctx = egui::Context::default();
        let theme = crate::theme::Theme::ember_dark();
        crate::theme::ThemeApplier::default().apply(&ctx, &theme);
        let mut rules = 0;
        // The theme's fonts are in from the second pass.
        for _ in 0..2 {
            let out = ctx.run_ui(Default::default(), |ui| {
                let mut on = false;
                for label in ["One", "Two", "Three"] {
                    super::switch_row(ui, &theme, label, None, &mut on);
                }
                ui.label("Next group");
                ui.add_enabled_ui(false, |ui| super::switch_row(ui, &theme, "Four", Some("Disabled, in a child ui."), &mut on));
                super::switch_row(ui, &theme, "Five", None, &mut on);
            });
            let long_hline = |s: &egui::Shape| matches!(s, egui::Shape::LineSegment { points: [a, b], .. } if a.y == b.y && b.x - a.x > 100.0);
            rules = out.shapes.iter().filter(|c| long_hline(&c.shape)).count();
            out.drop_without_applying_deltas();
        }
        // One-Two, Two-Three, Four-Five; none above One or Four, none under Three or Five.
        assert_eq!(rules, 3);
    }

    #[test]
    fn search_moves_to_a_match_and_back() {
        use super::search_page;
        let mut before = None;
        assert_eq!(search_page(Page::Chat, &mut before, "zoom"), Page::Accessibility);
        // Still a match while typing on: stays; the first page left is kept.
        assert_eq!(search_page(Page::Accessibility, &mut before, "zoo"), Page::Accessibility);
        assert_eq!(search_page(Page::Accessibility, &mut before, "developer"), Page::Advanced);
        assert_eq!(search_page(Page::Advanced, &mut before, "no such setting"), Page::Advanced);
        assert_eq!(search_page(Page::Advanced, &mut before, ""), Page::Chat);
        assert_eq!((search_page(Page::Chat, &mut before, ""), before), (Page::Chat, None));
    }
}
