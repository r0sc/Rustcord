//! Server menus and dialogs (FEATURES_NOTES §3.3-§3.11, layout §4.2 / §4.11):
//! - `server_header`: the channel list's 49 px header: the server-name pill (its dropdown is
//!   `header_menu`) and the 32 px Invite button right of it.
//! - `guild_menu`: the server icon's right-click menu (§4.11 order); the header dropdown has
//!   the same items but Mark As Read in the §4.2 order. Mark As Read, Invite People, Mute Server ▸ /
//!   Notification Settings ▸ (`UpdateGuildNotifications`), Hide Muted Channels, Server Settings,
//!   Create Channel / Category, Privacy Settings (dialog: one `SetSetting` per switch),
//!   Edit Per-server Profile (Settings > Profiles, its server tab), Leave Server (confirm),
//!   Copy ID.
//! - `channel_menu_items` / `category_menu`: Invite, Mute ▸, Notification Settings ▸
//!   (`UpdateChannelOverride`), Edit (= `server_settings::Page::Channel`), Delete (confirm);
//!   `member_menu_items`: a member's Roles ▸ (stays open for several toggles), Change
//!   Nickname ▸ (a field in the submenu), Kick / Ban (the Moderate dialog).
//! - Dialogs: Create Channel / Category (`CreateChannel`, kinds 0 / 2 / 5 / 15, Private =
//!   deny View Channel to @everyone once the channel exists), Invite (`created_invite` +
//!   `CreateInvite`, copy, expiry / use limit), Add a Server (create: `CreateGuild` with name and
//!   icon; join: `invite_code`, `request_invite` + `ResolveInvite` preview, then `AcceptInvite`),
//!   Moderate (kick / ban with reason and the ban's delete-messages preset).
//!
//! Hook-up guide: docs/ACCOUNT_SERVER_DESIGN.md. Every item is gated with `account_check` and
//! hidden without the permission; leave / delete only after `ViewState::confirm`, kick / ban in
//! the Moderate dialog; captcha / 2FA answers arrive as `Done(Err(text))` and are shown as
//! selectable text. Menu actions send untracked (errors become toasts); dialogs track their
//! write with `begin(op)` and drop the `ops` entry when they close.
//! OWNERSHIP: server engineer (docs/SOCIAL_UI_DESIGN.md).

use super::icons::{self, Icon};
use super::messages::{Btn, MenuIcon, Pal, button, menu_item, menu_setup, paint_radio, vector_button, vector_icon};
use super::server_settings::{self, Page};
use super::sidebar::{mark_read, row_fills};
use super::{Modal, UiCx, ViewState, bold, dialog, dialog_buttons, dialog_text, one_line};
use crate::backend::{AccountCommand, AccountOp, Command, NotificationPatch};
use crate::model::{Id, MuteConfig, Overwrite, Perms, channel_type as ct, perm};
use crate::state::account::{CreatedInvite, OpState};
use crate::state::{AppState, SidebarItem};
use egui::containers::menu::{MenuConfig, SubMenuButton};
use egui::{Align2, Button, Color32, PopupCloseBehavior, Rect, RichText, Role, Sense, Shape, Stroke, TextEdit, UiBuilder, Vec2, WidgetInfo, pos2, vec2};
use std::path::PathBuf;

/// Mute presets (FEATURES_NOTES §4.11); None = until I unmute.
const MUTE_FOR: [(&str, Option<u32>); 6] = [
    ("For 15 minutes", Some(900)),
    ("For 1 hour", Some(3600)),
    ("For 3 hours", Some(10_800)),
    ("For 8 hours", Some(28_800)),
    ("For 24 hours", Some(86_400)),
    ("Until I turn it back on", None),
];
/// `message_notifications` levels.
const LEVELS: [(&str, u8); 3] = [("All messages", 0), ("Only @mentions", 1), ("Nothing", 2)];
/// Invite dialog: expiry (seconds, 0 = never) and use limit (0 = none).
const INVITE_AGES: [(u32, &str); 7] =
    [(1800, "30 minutes"), (3600, "1 hour"), (21_600, "6 hours"), (43_200, "12 hours"), (86_400, "1 day"), (604_800, "7 days"), (0, "Never")];
const INVITE_USES: [(u32, &str); 7] = [(0, "No limit"), (1, "1 use"), (5, "5 uses"), (10, "10 uses"), (25, "25 uses"), (50, "50 uses"), (100, "100 uses")];
/// Ban: delete the member's messages of the last … (FEATURES_NOTES §3.6 presets).
const BAN_DELETE: [(u32, &str); 7] = [
    (0, "Don't delete any"),
    (3600, "Previous hour"),
    (21_600, "Previous 6 hours"),
    (43_200, "Previous 12 hours"),
    (86_400, "Previous 24 hours"),
    (259_200, "Previous 3 days"),
    (604_800, "Previous 7 days"),
];
/// Create channel: the kinds offered and what each is for (its one-line description).
const KINDS: [(u8, &str, &str); 4] = [
    (ct::GUILD_TEXT, "Text", "Messages, images and links in one channel"),
    (ct::GUILD_VOICE, "Voice", "Talk live, with cameras and screen sharing"),
    (ct::GUILD_FORUM, "Forum", "One post per topic, each with its own replies"),
    (ct::GUILD_ANNOUNCEMENT, "Announcement", "Updates that other servers can follow"),
];

/// Drafts of the server dialogs (in `ViewState::server_menu_ui`); reset when a dialog opens.
#[derive(Debug)]
pub struct ServerMenuUi {
    /// Channel name, invite link, server name or reason being typed.
    pub text: String,
    /// Create Channel: the kind picked; Private Channel / Category.
    kind: u8,
    private: bool,
    /// Add a Server: the screen shown.
    step: Step,
    /// Add a Server > Create: the icon file picked.
    icon: Option<PathBuf>,
    /// Add a Server > Join: the code whose preview is shown.
    code: Option<String>,
    /// Invite: settings shown; expiry and use limit of the next link; "Copied" since (ctx time).
    invite_edit: bool,
    max_age: u32,
    max_uses: u32,
    copied: Option<f64>,
    /// Ban: the delete-messages preset.
    delete_secs: u32,
    /// The write this dialog waits on (its `account.ops` entry goes with the dialog).
    op: Option<AccountOp>,
    /// Member menu ▸ Change Nickname: (member, draft).
    nick: Option<(Id, String)>,
    /// Apply to join: one answer per form field (`filled`).
    apply: Vec<Answer>,
}

impl Default for ServerMenuUi {
    fn default() -> Self {
        Self {
            text: String::new(),
            kind: ct::GUILD_TEXT,
            private: false,
            step: Step::Start,
            icon: None,
            code: None,
            invite_edit: false,
            max_age: 604_800,
            max_uses: 0,
            copied: None,
            delete_secs: 0,
            op: None,
            nick: None,
            apply: Vec::new(),
        }
    }
}

/// Screens of the Add a Server dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Start,
    Create,
    Join,
}

/// Opens a server dialog with fresh drafts. Invite People asks for its link right away (the
/// click is the user action; the dialog shows it once it arrives).
pub fn open(cx: &mut UiCx<'_>, modal: Modal) {
    cx.view.profile_card = None;
    cx.view.server_menu_ui = ServerMenuUi::default();
    if let Modal::Invite { channel_id, .. } = modal {
        create_invite(cx, channel_id);
    }
    cx.view.modal = Some(modal);
}

/// Closes the dialog and forgets what it waited on (a late error then becomes a toast).
fn close(cx: &mut UiCx<'_>) {
    if let Some(op) = cx.view.server_menu_ui.op.take() {
        cx.state.account.ops.remove(&op);
    }
    cx.state.account.created_invite = None;
    cx.view.modal = None;
}

/// Sends a write the open dialog shows progress for; a click while the same write is pending
/// sends nothing.
fn send_tracked(cx: &mut UiCx<'_>, cmd: AccountCommand) {
    let op = cmd.op();
    if cx.state.account.begin(op) {
        cx.cmd.send(Command::Account(cmd));
    }
    cx.view.server_menu_ui.op = Some(op);
}

/// State of the write a form waits on.
pub(super) fn op_state(state: &AppState, op: Option<AccountOp>) -> Option<&OpState> {
    state.account.ops.get(&op?)
}

// ---------------------------------------------------------------------------
// Header and menus
// ---------------------------------------------------------------------------

/// Server name in a 32 px pill with a chevron (tinted on hover / while open; a click or a
/// right-click opens the dropdown below it), and a 32 px Invite button right of it when I may
/// invite.
pub fn server_header(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id, title: String, rect: Rect) {
    let c = cx.theme.colors.clone();
    let invite = invite_channel(cx.state, guild_id).filter(|&ch| can_invite(cx.state, ch));
    let pill_w = rect.width() - 16.0 - if invite.is_some() { 36.0 } else { 0.0 };
    let pill = Rect::from_min_size(pos2(rect.left() + 8.0, rect.center().y - 16.0), vec2(pill_w, 32.0));
    let resp = ui.interact(pill, ui.make_persistent_id(("guild_header", guild_id)), Sense::click());
    let menu_open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    if menu_open || resp.hovered() {
        ui.painter().rect_filled(pill, 8, row_fills(ui, &c).1);
    }
    let color = c.text.to_color32();
    // Chevron pointing down, up while the menu is open.
    let tip = pos2(pill.right() - 14.0, pill.center().y + if menu_open { -2.0 } else { 2.0 });
    let dy = if menu_open { 4.0 } else { -4.0 };
    ui.painter().add(Shape::line(vec![tip + vec2(-4.0, dy), tip, tip + vec2(4.0, dy)], Stroke::new(1.5, color)));
    // The server's mark (verified, partnered, boost level) before the name, with its tooltip.
    let guild = cx.state.guild(guild_id);
    let mark = guild.and_then(|g| icons::guild_mark(&g.features, g.premium_tier).map(|m| (m, mark_tip(m, g.premium_subscription_count))));
    let left = if mark.is_some() { 30.0 } else { 8.0 };
    if let Some(((icon, _), tip)) = &mark {
        let r = Rect::from_center_size(pos2(pill.left() + 16.0, pill.center().y), Vec2::splat(16.0));
        let bg = if menu_open || resp.hovered() { row_fills(ui, &c).1 } else { c.sidebar.to_color32() };
        icons::paint(ui.painter(), *icon, r, color, bg);
        ui.interact(r, ui.make_persistent_id(("guild_header_mark", guild_id)), Sense::hover()).on_hover_text(tip);
    }
    let g = one_line(ui, title.as_str(), bold(cx.theme.font.size + 1.0), color, pill.width() - 32.0 - left);
    ui.painter().galley(pos2(pill.left() + left, pill.center().y - g.size().y / 2.0), g, color);
    resp.widget_info(|| WidgetInfo::selected(Role::Button, true, menu_open, &title));
    let set = if resp.secondary_clicked() || demo_menu() { Some(egui::SetOpenCommand::Bool(true)) } else { resp.clicked().then_some(egui::SetOpenCommand::Toggle) };
    egui::Popup::menu(&resp).open_memory(set).gap(4.0).show(|ui| header_menu(ui, cx, guild_id));
    if let Some(ch) = invite {
        let r = Rect::from_center_size(pos2(rect.right() - 24.0, rect.center().y), Vec2::splat(32.0));
        let pal = Pal::new(cx.theme);
        let clicked = ui.scope_builder(UiBuilder::new().max_rect(r), |ui| invite_icon_button(ui, &pal)).inner.clicked();
        if clicked {
            open(cx, Modal::Invite { guild_id, channel_id: ch });
        }
    }
}

/// Tooltip of a server mark: its label, a boost level with the count ("Boost level 2, 9 boosts").
pub(super) fn mark_tip((icon, label): (Icon, &str), boosts: u32) -> String {
    match boosts {
        0 => label.to_owned(),
        _ if matches!(icon, Icon::Verified | Icon::Partnered) => label.to_owned(),
        1 => format!("{label}, 1 boost"),
        n => format!("{label}, {n} boosts"),
    }
}

/// Test hook `RUSTCORD_OPEN=server-menu`: the header dropdown starts open (with
/// `RUSTCORD_POINTER` on a row, its submenu too). True once.
fn demo_menu() -> bool {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    !DONE.load(std::sync::atomic::Ordering::Relaxed)
        && !DONE.swap(true, std::sync::atomic::Ordering::Relaxed)
        && std::env::var("RUSTCORD_OPEN").is_ok_and(|v| v.split(',').any(|s| s.trim() == "server-menu"))
}

/// 32 px "Invite People" button: a person with a plus.
fn invite_icon_button(ui: &mut egui::Ui, pal: &Pal) -> egui::Response {
    vector_button(ui, pal, Icon::PersonAdd, "Invite people", 32.0, false, pal.text)
}

/// 28 px "More" button.
pub(super) fn more_button(ui: &mut egui::Ui, pal: &Pal) -> egui::Response {
    vector_button(ui, pal, Icon::More, "More", 28.0, false, pal.text)
}

/// The header dropdown (§4.2 order): Invite People, Server Settings, Create Channel / Category |
/// Notification Settings ▸ (with Mute Server ▸), Privacy Settings | Edit Per-server Profile,
/// Hide Muted Channels | Leave Server | Copy Server ID. Mark As Read is the server icon's.
fn header_menu(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id) {
    menu_setup(ui);
    // Invite people in `text`: menu items never carry the accent (docs/DESIGN.md §2.3).
    let text = cx.theme.colors.text.to_color32();
    invite_item(ui, cx, guild_id, text);
    settings_item(ui, cx, guild_id, text);
    create_items(ui, cx, guild_id, text);
    ui.separator();
    notification_menu(ui, cx, guild_id, None, true);
    privacy_item(ui, cx, guild_id, text);
    ui.separator();
    server_profile_item(ui, cx, guild_id, text);
    hide_muted_item(ui, cx, guild_id, text);
    crate::plugins::menus::guild_items(ui, cx, guild_id, &mut |ui, label, glyph| menu_item(ui, label, MenuIcon::Glyph(glyph), text));
    leave_item(ui, cx, guild_id);
    // Settings > Advanced > Developer Mode.
    let dev = cx.state.account.settings.developer_mode();
    if dev {
        ui.separator();
    }
    if dev && menu_item(ui, "Copy server ID", MenuIcon::Icon(Icon::IdCard), text) {
        ui.ctx().copy_text(guild_id.0.to_string());
    }
}

/// The server icon's right-click menu (§4.11 order).
pub fn guild_menu(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id) {
    menu_setup(ui);
    let text = cx.theme.colors.text.to_color32();
    mark_read_item(ui, cx, guild_id, text);
    ui.separator();
    if invite_item(ui, cx, guild_id, text) {
        ui.separator();
    }
    mute_items(ui, cx, guild_id, None, "server");
    notification_menu(ui, cx, guild_id, None, false);
    hide_muted_item(ui, cx, guild_id, text);
    privacy_item(ui, cx, guild_id, text);
    server_profile_item(ui, cx, guild_id, text);
    ui.separator();
    settings_item(ui, cx, guild_id, text);
    create_items(ui, cx, guild_id, text);
    crate::plugins::menus::guild_items(ui, cx, guild_id, &mut |ui, label, glyph| menu_item(ui, label, MenuIcon::Glyph(glyph), text));
    leave_item(ui, cx, guild_id);
    // Settings > Advanced > Developer Mode.
    let dev = cx.state.account.settings.developer_mode();
    if dev {
        ui.separator();
    }
    if dev && menu_item(ui, "Copy server ID", MenuIcon::Icon(Icon::IdCard), text) {
        ui.ctx().copy_text(guild_id.0.to_string());
    }
}

fn mark_read_item(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id, text: Color32) {
    let s = &*cx.state;
    let unread = s.channels.values().any(|c| c.guild_id == Some(guild_id) && s.is_unread(c.id));
    if ui.add_enabled_ui(unread, |ui| menu_item(ui, "Mark as read", MenuIcon::Icon(Icon::Check), text)).inner {
        mark_server_read(cx, guild_id);
    }
}

/// Mark as read on a server (its menu, Shift+Esc): every unread channel and joined thread, acked in bulk (§3.11).
pub(super) fn mark_server_read(cx: &mut UiCx<'_>, guild_id: Id) {
    let acks = cx.state.mark_guild_read(guild_id);
    for (ch, _) in &acks {
        cx.view.pending_acks.remove(ch);
    }
    if cx.settings.send_read_acks && !acks.is_empty() {
        cx.cmd.send(Command::Social(crate::backend::SocialCommand::MarkAllRead { acks }));
    }
}

/// Invite people, when I may invite to the open (or first) channel. True when shown.
fn invite_item(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id, color: Color32) -> bool {
    let Some(ch) = invite_channel(cx.state, guild_id).filter(|&ch| can_invite(cx.state, ch)) else { return false };
    if menu_item(ui, "Invite people", MenuIcon::Icon(Icon::PersonAdd), color) {
        open(cx, Modal::Invite { guild_id, channel_id: ch });
    }
    true
}

/// Server Settings, when any of its pages is mine to use.
fn settings_item(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id, text: Color32) {
    if let Some(page) = server_settings::first_page(cx.state, guild_id)
        && menu_item(ui, "Server settings", MenuIcon::Icon(Icon::Gear), text)
    {
        server_settings::open(cx, guild_id, page);
    }
}

fn create_items(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id, text: Color32) {
    let create = AccountCommand::CreateChannel { guild_id, name: String::new(), kind: ct::GUILD_TEXT, parent_id: None };
    if cx.state.account_check(&create).is_err() {
        return;
    }
    if menu_item(ui, "Create channel", MenuIcon::Icon(Icon::Plus), text) {
        open(cx, Modal::CreateChannel { guild_id, parent_id: None, category: false });
    }
    if menu_item(ui, "Create category", MenuIcon::Icon(Icon::Directory), text) {
        open(cx, Modal::CreateChannel { guild_id, parent_id: None, category: true });
    }
}

/// Privacy Settings (the dialog).
fn privacy_item(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id, text: Color32) {
    if menu_item(ui, "Privacy", MenuIcon::Icon(Icon::Shield), text) {
        open(cx, Modal::PrivacySettings { guild_id });
    }
}

/// Edit Per-server Profile (Settings > Profiles on this server).
fn server_profile_item(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id, text: Color32) {
    if menu_item(ui, "Edit server profile", MenuIcon::Icon(Icon::Edit), text) {
        super::user_settings::open_server_profile(cx.view, guild_id);
    }
}

/// Per-server privacy (SETTINGS_NOTES §1 "Per-server privacy menu"): settings proto 8.x lists of
/// guild ids where the option is off. (label, hint, field).
const PRIVACY: [(&str, &str, u32); 4] = [
    ("Direct messages", "Members of this server can send you direct messages.", 3),
    ("Message requests", "Messages from members you may not know go to your message requests.", 16),
    ("Activity status", "Members of this server see what you're playing or listening to.", 13),
    ("Activity joining", "Members of this server can join your activities.", 15),
];

/// The list after switching `guild` on (removed: allowed again) or off (added).
fn privacy_list(mut ids: Vec<u64>, guild: u64, on: bool) -> Vec<u64> {
    ids.retain(|&g| g != guild);
    if !on {
        ids.push(guild);
    }
    ids
}

/// `Modal::PrivacySettings`: a switch per `PRIVACY` row, then Leaderboards (3.1[guild].11
/// `leaderboards_disabled`); a click sends one `SetSetting` (field 8: that field's whole list;
/// field 3: that server's entry); the switches of that field wait while it is on its way.
pub fn privacy_modal(ctx: &egui::Context, cx: &mut UiCx<'_>, guild_id: Id) {
    use crate::api::settings_proto::SettingValue;
    let name = cx.state.guilds.iter().find(|g| g.id == guild_id).map_or_else(String::new, |g| g.name.clone());
    let theme = &*cx.theme;
    let pal = Pal::new(theme);
    let settings = &cx.state.account.settings;
    let busy = |top| cx.state.account.ops.get(&AccountOp { kind: crate::backend::OpKind::SetSetting, target: Some(Id(top)) }) == Some(&OpState::Pending);
    let mut write = None;
    let resp = dialog(ctx, theme, "rc_privacy", 440.0, &format!("Privacy in {name}"), |ui| {
        // One setting row (§12): title and description left, the switch right, a hairline
        // between rows; Some(new state) when clicked.
        let mut first = true;
        let mut row = |ui: &mut egui::Ui, label: &str, hint: &str, mut on: bool, busy: bool| {
            if !std::mem::take(&mut first) {
                // 12 px of air on both sides of the hairline, as the settings rows have.
                ui.add_space(4.0);
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
                ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, pal.tk.rule));
                ui.add_space(4.0);
            }
            ui.horizontal_top(|ui| {
                let w = (ui.available_width() - 56.0).max(80.0);
                ui.allocate_ui_with_layout(vec2(w, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(RichText::new(label).font(egui::FontId::new(pal.ui(), crate::theme::medium_family())).color(pal.text));
                    ui.add(egui::Label::new(RichText::new(hint).size(pal.meta()).color(pal.muted)).wrap());
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| ui.add_enabled_ui(!busy, |ui| switch(ui, &pal, &mut on)).inner.changed()).inner
            })
            .inner
            .then_some(on)
        };
        for (label, hint, field) in PRIVACY {
            let ids = settings.proto_ids(8, &[field]);
            if let Some(on) = row(ui, label, hint, !ids.contains(&guild_id.0), busy(8)) {
                write = Some((8, vec![field], SettingValue::Ids(privacy_list(ids, guild_id.0, on))));
            }
        }
        let shown = settings.proto_entry_varint(3, 1, guild_id.0, &[11]).is_none_or(|v| v == 0);
        if let Some(on) = row(ui, "Leaderboards", "You show up on this server's leaderboards.", shown, busy(3)) {
            let value = Box::new(SettingValue::Plain(u64::from(!on)));
            write = Some((3, vec![1], SettingValue::Entry { key: guild_id.0, path: vec![11], value }));
        }
        // Each switch is saved on its click: Done only closes (no Cancel that could not undo).
        ui.add_space(8.0);
        super::dialog_footer(ui, |ui| button(ui, &pal, Btn::Primary, "Done", true).clicked())
    });
    if let Some((top, path, value)) = write {
        let values = vec![(path, value)];
        let cmd = AccountCommand::SetSetting { top, values: values.clone() };
        if cx.state.account.begin(cmd.op()) {
            cx.state.account.settings.set_local(top, &values);
            cx.cmd.send(Command::Account(cmd));
        }
    }
    if resp.inner || resp.should_close() {
        close(cx);
    }
}

fn hide_muted_item(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id, text: Color32) {
    let hidden = cx.view.hide_muted.contains(&guild_id);
    if menu_item(ui, "Hide muted channels", MenuIcon::Check(hidden), text) && !cx.view.hide_muted.remove(&guild_id) {
        cx.view.hide_muted.insert(guild_id);
    }
}

fn leave_item(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id) {
    if cx.state.account_check(&AccountCommand::LeaveGuild { guild_id }).is_ok() {
        ui.separator();
        if menu_item(ui, "Leave server", MenuIcon::Icon(Icon::Leave), cx.theme.colors.danger.to_color32()) {
            ask_leave(cx.view, cx.state, guild_id);
        }
    }
}

/// Channel an "Invite people" link points to: the open channel of this server, else its first
/// listed channel.
fn invite_channel(state: &AppState, guild_id: Id) -> Option<Id> {
    let open = state.selected_channel.filter(|c| state.channels.get(c).is_some_and(|c| c.guild_id == Some(guild_id) && !c.is_thread()));
    open.or_else(|| state.sidebar_channels(guild_id).into_iter().find_map(|i| if let SidebarItem::Channel { id } = i { Some(id) } else { None }))
}

fn can_invite(state: &AppState, channel_id: Id) -> bool {
    state.account_check(&AccountCommand::CreateInvite { channel_id, max_age: 0, max_uses: 0, temporary: false }).is_ok()
}

/// The Leave server question (not offered to the owner: `account_check` says why).
pub fn ask_leave(view: &mut ViewState, state: &AppState, guild_id: Id) {
    let cmd = AccountCommand::LeaveGuild { guild_id };
    if let Err(why) = state.account_check(&cmd) {
        view.toast(why, true);
        return;
    }
    let name = state.guild(guild_id).map(|g| g.name.clone()).unwrap_or_default();
    view.confirm(format!("Leave {name}?"), "You'll need a new invite to rejoin.", "Leave server", Command::Account(cmd));
}

/// Delete Channel / Delete Category question.
pub(super) fn ask_delete_channel(view: &mut ViewState, state: &AppState, channel_id: Id) {
    let cmd = AccountCommand::DeleteChannel { channel_id };
    if let Err(why) = state.account_check(&cmd) {
        view.toast(why, true);
        return;
    }
    let name = state.channel_name(channel_id);
    let (title, body, button) = if state.channels.get(&channel_id).is_some_and(|c| c.kind == ct::GUILD_CATEGORY) {
        (format!("Delete {name}?"), "Its channels stay, without a category. You can't undo this.", "Delete category")
    } else {
        (format!("Delete #{name}?"), "Its messages are gone for everyone. You can't undo this.", "Delete channel")
    };
    view.confirm(title, body, button, Command::Account(cmd));
}

/// Items of a guild channel's menu (sidebar), before its Copy link / ID: Invite people,
/// Mute ▸, Notification Settings ▸, Edit Channel, Delete Channel (confirm).
pub fn channel_menu_items(ui: &mut egui::Ui, cx: &mut UiCx<'_>, channel_id: Id) {
    let Some((guild_id, kind)) = cx.state.channels.get(&channel_id).and_then(|c| Some((c.guild_id?, c.kind))) else { return };
    let c = &cx.theme.colors;
    let (text, danger) = (c.text.to_color32(), c.danger.to_color32());
    let category = kind == ct::GUILD_CATEGORY;
    let noun = if category { "category" } else { "channel" };
    ui.separator();
    if !category && can_invite(cx.state, channel_id) && menu_item(ui, "Invite people", MenuIcon::Icon(Icon::PersonAdd), text) {
        open(cx, Modal::Invite { guild_id, channel_id });
    }
    mute_items(ui, cx, guild_id, Some(channel_id), noun);
    notification_menu(ui, cx, guild_id, Some(channel_id), false);
    let edit = cx.state.account_check(&AccountCommand::UpdateChannel { channel_id, patch: Default::default() }).is_ok();
    let delete = cx.state.account_check(&AccountCommand::DeleteChannel { channel_id }).is_ok();
    if edit || delete {
        ui.separator();
    }
    if edit && menu_item(ui, &format!("Edit {noun}"), MenuIcon::Icon(Icon::Gear), text) {
        server_settings::open(cx, guild_id, Page::Channel(channel_id));
    }
    if delete && menu_item(ui, &format!("Delete {noun}"), MenuIcon::Icon(Icon::Trash), danger) {
        ask_delete_channel(cx.view, cx.state, channel_id);
    }
}

/// A category's right-click menu: Mark As Read, Collapse, Mute ▸, Notification Settings ▸,
/// Create Channel in it, Edit / Delete Category, Copy ID.
pub fn category_menu(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id, category_id: Id) {
    menu_setup(ui);
    let text = cx.theme.colors.text.to_color32();
    let s = &*cx.state;
    let unread: Vec<Id> = s.channels.values().filter(|c| c.parent_id == Some(category_id) && c.is_text_chat() && s.is_unread(c.id)).map(|c| c.id).collect();
    if ui.add_enabled_ui(!unread.is_empty(), |ui| menu_item(ui, "Mark as read", MenuIcon::Icon(Icon::Check), text)).inner {
        mark_read(cx, unread);
    }
    ui.separator();
    let collapsed = cx.view.collapsed_categories.contains(&category_id);
    if menu_item(ui, "Collapse category", MenuIcon::Check(collapsed), text) && !cx.view.collapsed_categories.remove(&category_id) {
        cx.view.collapsed_categories.insert(category_id);
    }
    if menu_item(ui, "Collapse all categories", MenuIcon::Glyph(""), text) {
        let cats = cx.state.channels.values().filter(|c| c.guild_id == Some(guild_id) && c.kind == ct::GUILD_CATEGORY).map(|c| c.id);
        cx.view.collapsed_categories.extend(cats);
    }
    let create = AccountCommand::CreateChannel { guild_id, name: String::new(), kind: ct::GUILD_TEXT, parent_id: Some(category_id) };
    if cx.state.account_check(&create).is_ok() {
        ui.separator();
        if menu_item(ui, "Create channel", MenuIcon::Icon(Icon::Plus), text) {
            open(cx, Modal::CreateChannel { guild_id, parent_id: Some(category_id), category: false });
        }
    }
    channel_menu_items(ui, cx, category_id);
    // Settings > Advanced > Developer Mode.
    let dev = cx.state.account.settings.developer_mode();
    if dev {
        ui.separator();
    }
    if dev && menu_item(ui, "Copy category ID", MenuIcon::Icon(Icon::IdCard), text) {
        ui.ctx().copy_text(category_id.0.to_string());
    }
}

/// A member's admin items (member list, voice users, server settings > Members): Roles ▸,
/// Change Nickname ▸, Kick, Ban. Each only with the permission and a higher top role.
pub fn member_menu_items(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id, user_id: Id) {
    let me = cx.state.me.as_ref().is_some_and(|u| u.id == user_id);
    let danger = cx.theme.colors.danger.to_color32();
    let roles = assignable_roles(cx.state, guild_id, user_id);
    let nick_cmd = if me { AccountCommand::SetMyNick { guild_id, nick: None } } else { AccountCommand::SetNick { guild_id, user_id, nick: None } };
    let can_nick = cx.state.account_check(&nick_cmd).is_ok();
    let kick = AccountCommand::KickMember { guild_id, user_id, reason: None };
    let ban = AccountCommand::BanMember { guild_id, user_id, reason: None, delete_message_seconds: 0 };
    let (can_kick, can_ban) = (cx.state.account_check(&kick).is_ok(), cx.state.account_check(&ban).is_ok());
    if roles.is_empty() && !can_nick && !can_kick && !can_ban {
        return;
    }
    ui.separator();
    if !roles.is_empty() {
        roles_submenu(ui, cx, guild_id, user_id, &roles);
    }
    if can_nick {
        nick_submenu(ui, cx, guild_id, user_id, me);
    }
    let name = user_name(cx.state, guild_id, user_id);
    if can_kick && menu_item(ui, &format!("Kick {name}"), MenuIcon::Icon(Icon::PersonRemove), danger) {
        open(cx, Modal::Moderate { guild_id, user_id, ban: false });
    }
    if can_ban && menu_item(ui, &format!("Ban {name}"), MenuIcon::Icon(Icon::Ban), danger) {
        open(cx, Modal::Moderate { guild_id, user_id, ban: true });
    }
}

/// Roles I may give this member or take away (highest first; none when the member is not
/// cached, as its roles are unknown): (role, name, colour, has it).
pub(super) fn assignable_roles(state: &AppState, guild_id: Id, user_id: Id) -> Vec<(Id, String, u32, bool)> {
    let (Some(g), Some(member)) = (state.guild(guild_id), state.find_member(guild_id, user_id)) else { return Vec::new() };
    let me = state.me.as_ref().is_some_and(|u| u.id == user_id);
    if !me && !state.outranks(guild_id, user_id, &member.roles) {
        return Vec::new();
    }
    let mut roles: Vec<_> = (g.roles.iter())
        .filter(|r| state.account_check(&AccountCommand::AddMemberRole { guild_id, user_id, role_id: r.id }).is_ok())
        .map(|r| (r.position, r.id, r.name.clone(), r.color, member.roles.contains(&r.id)))
        .collect();
    roles.sort_by_key(|r| (std::cmp::Reverse(r.0), r.1));
    roles.into_iter().map(|(_, id, name, color, has)| (id, name, color, has)).collect()
}

/// Gives or takes a role (one click, one request; the gateway's member update shows it).
pub(super) fn toggle_role(cx: &mut UiCx<'_>, guild_id: Id, user_id: Id, role_id: Id, has: bool) {
    let cmd = if has { AccountCommand::RemoveMemberRole { guild_id, user_id, role_id } } else { AccountCommand::AddMemberRole { guild_id, user_id, role_id } };
    if cx.state.account_check(&cmd).is_ok() {
        cx.cmd.send(Command::Account(cmd));
    }
}

/// Roles ▸ with a check per role; it stays open so several roles can be toggled.
fn roles_submenu(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id, user_id: Id, roles: &[(Id, String, u32, bool)]) {
    let (text, raised) = (cx.theme.colors.text.to_color32(), cx.theme.tokens().raised);
    let keep_open = MenuConfig::new().close_behavior(PopupCloseBehavior::CloseOnClickOutside);
    SubMenuButton::new(RichText::new("Roles").color(text)).config(keep_open).ui(ui, |ui| {
        menu_setup(ui);
        for (id, name, color, has) in roles {
            let color = if *color == 0 { text } else { crate::theme::readable_on(super::rgb(*color), raised) };
            if menu_item(ui, name, MenuIcon::Check(*has), color) {
                toggle_role(cx, guild_id, user_id, *id, *has);
            }
        }
    });
}

/// Change Nickname ▸: a field with Save (Enter) and Reset, inside the submenu (no dialog).
fn nick_submenu(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id, user_id: Id, me: bool) {
    let pal = Pal::new(cx.theme);
    let keep_open = MenuConfig::new().close_behavior(PopupCloseBehavior::CloseOnClickOutside);
    SubMenuButton::new(RichText::new("Change nickname").color(pal.text)).config(keep_open).ui(ui, |ui| {
        menu_setup(ui);
        ui.set_width(240.0);
        let current = cx.state.find_member(guild_id, user_id).and_then(|m| m.nick.clone()).unwrap_or_default();
        let hint = cx.state.users.get(&user_id).or(cx.state.me.as_ref().filter(|_| me)).map(|u| u.display_name().to_owned()).unwrap_or_default();
        let m = &mut cx.view.server_menu_ui;
        let fresh = m.nick.as_ref().is_none_or(|(u, _)| *u != user_id);
        if fresh {
            m.nick = Some((user_id, current));
        }
        let Some((_, draft)) = m.nick.as_mut() else { return };
        let field = ui.add(TextEdit::singleline(draft).char_limit(32).hint_text(hint).desired_width(f32::INFINITY));
        if fresh {
            field.request_focus();
        }
        let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let (reset, save) = ui
            .horizontal(|ui| {
                let reset = button(ui, &pal, Btn::Ghost, "Reset", true).clicked();
                let save = button(ui, &pal, Btn::Primary, "Save", true).clicked();
                (reset, save)
            })
            .inner;
        if reset || save || enter {
            let nick = if reset { None } else { Some(draft.trim().to_owned()).filter(|n| !n.is_empty()) };
            let cmd = if me { AccountCommand::SetMyNick { guild_id, nick } } else { AccountCommand::SetNick { guild_id, user_id, nick } };
            cx.view.server_menu_ui.nick = None;
            cx.cmd.send(Command::Account(cmd));
            ui.close();
        }
    });
}

fn user_name(state: &AppState, guild_id: Id, user_id: Id) -> String {
    let user = state.users.get(&user_id).or(state.me.as_ref().filter(|u| u.id == user_id));
    user.map_or_else(|| user_id.to_string(), |u| state.member_display_name(Some(guild_id), u, None))
}

// ---------------------------------------------------------------------------
// Notification menus (§3.10)
// ---------------------------------------------------------------------------

/// Muted by its own setting (a channel's parent aside) and the level (3 = inherit), plus the
/// guild's suppress switches.
fn notify_state(state: &AppState, guild_id: Id, channel: Option<Id>) -> (bool, u8, bool, bool) {
    let s = state.guild_settings.get(&Some(guild_id));
    match channel {
        None => (state.is_guild_muted(guild_id), s.map_or(3, |s| s.message_notifications), s.is_some_and(|s| s.suppress_everyone), s.is_some_and(|s| s.suppress_roles)),
        Some(c) => {
            let o = s.and_then(|s| s.channel_overrides.iter().find(|o| o.channel_id == c));
            (o.is_some_and(|o| mute_active(o.muted, &o.mute_config)), o.map_or(3, |o| o.message_notifications), false, false)
        }
    }
}

/// Muted and the mute has not ended (state.rs keeps its own copy private).
fn mute_active(muted: bool, cfg: &Option<MuteConfig>) -> bool {
    let end = cfg.as_ref().and_then(|c| c.end_time.as_deref());
    muted && end.is_none_or(|t| chrono::DateTime::parse_from_rfc3339(t).map_or(true, |end| end > chrono::Utc::now()))
}

/// One click, one request (menus close on the click, so there is nothing to coalesce).
fn notify(cx: &mut UiCx<'_>, guild_id: Id, channel: Option<Id>, patch: NotificationPatch) {
    let cmd = match channel {
        None => AccountCommand::UpdateGuildNotifications { guild_id: Some(guild_id), patch },
        Some(channel_id) => AccountCommand::UpdateChannelOverride { guild_id: Some(guild_id), channel_id, patch },
    };
    cx.cmd.send(Command::Account(cmd));
}

/// "Mute X ▸" with the durations, or "Unmute X" while muted.
fn mute_items(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id, channel: Option<Id>, noun: &str) {
    let text = cx.theme.colors.text.to_color32();
    if notify_state(cx.state, guild_id, channel).0 {
        if menu_item(ui, &format!("Unmute {noun}"), MenuIcon::Icon(Icon::Bell), text) {
            notify(cx, guild_id, channel, NotificationPatch { muted: Some(false), ..Default::default() });
        }
        return;
    }
    let picked = ui
        .menu_button(RichText::new(format!("Mute {noun}")).color(text), |ui| {
            menu_setup(ui);
            MUTE_FOR.into_iter().find(|(label, _)| menu_item(ui, label, MenuIcon::Glyph(""), text)).map(|(_, secs)| secs)
        })
        .inner
        .flatten();
    if let Some(mute_secs) = picked {
        notify(cx, guild_id, channel, NotificationPatch { muted: Some(true), mute_secs, ..Default::default() });
    }
}

/// Notification Settings ▸: the level (a channel may follow its category / the server), the
/// server's suppress switches and, in the header dropdown, Mute Server ▸.
fn notification_menu(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id, channel: Option<Id>, with_mute: bool) {
    let text = cx.theme.colors.text.to_color32();
    let (_, level, everyone, roles) = notify_state(cx.state, guild_id, channel);
    let in_category = channel.and_then(|c| cx.state.channels.get(&c)).is_some_and(|c| c.parent_id.is_some());
    let server_default = cx.state.guild(guild_id).map_or(0, |g| g.default_message_notifications);
    let picked = ui
        .menu_button(RichText::new("Notifications").color(text), |ui| {
            menu_setup(ui);
            // Room for the longest label next to its painted check box.
            ui.set_min_width(270.0);
            let mut patch = None;
            let shown = match channel {
                Some(_) => {
                    let label = if in_category { "Use category default" } else { "Use server default" };
                    if radio_item(ui, label, level == 3, text) {
                        patch = Some(NotificationPatch { level: Some(3), ..Default::default() });
                    }
                    ui.separator();
                    level
                }
                None if level == 3 => server_default,
                None => level,
            };
            for (label, l) in LEVELS {
                if radio_item(ui, label, shown == l, text) {
                    patch = Some(NotificationPatch { level: Some(l), ..Default::default() });
                }
            }
            if channel.is_none() {
                ui.separator();
                if menu_item(ui, "Suppress @everyone and @here", MenuIcon::Check(everyone), text) {
                    patch = Some(NotificationPatch { suppress_everyone: Some(!everyone), ..Default::default() });
                }
                if menu_item(ui, "Suppress all role @mentions", MenuIcon::Check(roles), text) {
                    patch = Some(NotificationPatch { suppress_roles: Some(!roles), ..Default::default() });
                }
                if with_mute {
                    ui.separator();
                    mute_items(ui, cx, guild_id, None, "server");
                }
            }
            patch
        })
        .inner
        .flatten();
    if let Some(p) = picked {
        notify(cx, guild_id, channel, p);
    }
}

/// Menu row with a radio on the right (§9: off a 1.5 px `edge` ring, on a `color` ring and dot).
pub(super) fn radio_item(ui: &mut egui::Ui, text: &str, on: bool, color: Color32) -> bool {
    let font = egui::FontId::new(ui.style().text_styles[&egui::TextStyle::Button].size, crate::theme::medium_family());
    let resp = ui.add(Button::new(RichText::new(text).font(font).color(color)).right_text(RichText::new("  ")));
    let c = pos2(resp.rect.right() - ui.spacing().button_padding.x - 8.0, resp.rect.center().y);
    if on {
        ui.painter().circle_stroke(c, 7.0, Stroke::new(2.0, color));
        ui.painter().circle_filled(c, 3.2, color);
    } else {
        ui.painter().circle_stroke(c, 7.25, Stroke::new(1.5, crate::theme::applied_tokens(ui.ctx()).edge));
    }
    resp.widget_info(|| WidgetInfo::selected(Role::RadioButton, ui.is_enabled(), on, text));
    resp.clicked()
}

// ---------------------------------------------------------------------------
// Dialogs
// ---------------------------------------------------------------------------

/// Label over a dialog / settings field ("Channel name"): the label role (13/600, muted,
/// sentence case as written), 8 px above its field.
pub(super) fn field_label(ui: &mut egui::Ui, theme: &crate::theme::Theme, text: &str) {
    ui.add_space(8.0);
    let color = theme.colors.text_muted.to_color32();
    ui.label(RichText::new(text).font(super::semibold(theme.font.size - 2.0)).color(color));
}

/// An error answer (captcha hand-offs carry a link: selectable).
pub(super) fn error_text(ui: &mut egui::Ui, theme: &crate::theme::Theme, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).color(theme.colors.danger.to_color32()).size(theme.font.size - 1.0)).wrap().selectable(true));
}

/// `Modal::CreateChannel`: kind (channels only), name, Private. Private sends the @everyone
/// View Channel deny once the channel exists; the dialog waits for that.
pub fn create_channel_modal(ctx: &egui::Context, cx: &mut UiCx<'_>, guild_id: Id, parent_id: Option<Id>, category: bool) {
    let status = op_state(cx.state, cx.view.server_menu_ui.op).cloned();
    if let Some(OpState::Done(created)) = status {
        let m = &cx.view.server_menu_ui;
        match created.filter(|_| m.private) {
            // Wait for its CHANNEL_CREATE: the overwrite is checked against the cached channel.
            Some(id) if !cx.state.channels.contains_key(&id) => {}
            Some(id) => {
                let overwrite = Overwrite { id: guild_id, kind: 0, allow: Perms(0), deny: Perms(perm::VIEW_CHANNEL) };
                let cmd = AccountCommand::SetPermissionOverwrite { channel_id: id, overwrite };
                match cx.state.account_check(&cmd) {
                    Ok(()) => cx.cmd.send(Command::Account(cmd)),
                    Err(why) => cx.view.toast(why, true),
                }
                return close(cx);
            }
            None => return close(cx),
        }
    }
    let busy = matches!(status, Some(OpState::Pending | OpState::Done(_)));
    let pal = Pal::new(cx.theme);
    let theme = &*cx.theme;
    let state = &*cx.state;
    let m = &mut cx.view.server_menu_ui;
    // Also when opened without `open` (test hook): a category is always kind 4.
    let kind = if category { ct::GUILD_CATEGORY } else { m.kind };
    let text_like = matches!(kind, ct::GUILD_TEXT | ct::GUILD_ANNOUNCEMENT | ct::GUILD_FORUM);
    let cmd = AccountCommand::CreateChannel { guild_id, name: m.text.trim().to_owned(), kind, parent_id };
    let allowed = state.account_check(&cmd);
    // Private needs MANAGE_ROLES; offered to owners and admins only: anyone else would lock
    // themselves out (shortcut: no member / role picker yet, add one to offer it to all).
    let admin = state.guild(guild_id).is_some_and(|g| state.me.as_ref().is_some_and(|me| me.id == g.owner_id)) || state.guild_permissions(guild_id) & perm::ADMINISTRATOR != 0;
    let noun = if category { "category" } else { "channel" };
    let title = format!("Create {noun}");
    let short = ctx.content_rect().height() < 600.0;
    let resp = dialog(ctx, theme, "rc_create_channel", 460.0, &title, |ui| {
        if let Some(p) = parent_id {
            dialog_text(ui, theme, &format!("in {}", state.channel_name(p)));
        }
        if !category {
            if !short {
                field_label(ui, theme, "Channel type");
            }
            // Radio rows in one group with hairlines between them (§9 modals). Short windows
            // (720×420) get two kinds per row and no descriptions, so the dialog fits.
            let per_row = if short { 2 } else { 1 };
            let bg = ui.visuals().window_fill.blend(pal.tk.hover);
            egui::Frame::new().fill(pal.tk.hover).corner_radius(pal.radius).inner_margin(4).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (i, kinds) in KINDS.chunks(per_row).enumerate() {
                    if i > 0 {
                        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
                        ui.painter().hline(r.shrink2(vec2(12.0, 0.0)).x_range(), r.center().y, Stroke::new(1.0, pal.tk.rule));
                    }
                    ui.columns(per_row, |cols| {
                        for (col, &(kind, label, desc)) in cols.iter_mut().zip(kinds) {
                            let resp = kind_row(col, &pal, kind, label, (!short).then_some(desc), m.kind == kind, bg);
                            if resp.on_hover_text(desc).clicked() {
                                m.kind = kind;
                            }
                        }
                    });
                }
            });
        }
        field_label(ui, theme, &format!("{} name", if category { "Category" } else { "Channel" }));
        let hint = if category { "New category" } else { "new-channel" };
        // The kind's icon sits inside the field, before the name.
        let left = if category { 4 } else { 30 };
        let edit = TextEdit::singleline(&mut m.text).char_limit(100).hint_text(hint).margin(egui::Margin { left, right: 4, top: 2, bottom: 2 }).desired_width(f32::INFINITY);
        let field = ui.add_enabled(!busy, edit);
        if !category {
            let r = Rect::from_center_size(pos2(field.rect.left() + 16.0, field.rect.center().y), Vec2::splat(16.0));
            icons::paint_channel(ui.painter(), icons::channel(m.kind, m.private, false, false), r, pal.muted, ui.visuals().text_edit_bg_color());
        }
        if text_like && field.changed() {
            // Text-like names are lowercase with dashes, as Discord shows them while typing.
            m.text = m.text.to_lowercase().replace(' ', "-");
        }
        if admin {
            ui.add_space(8.0);
            ui.horizontal_top(|ui| {
                let fill = ui.visuals().window_fill;
                vector_icon(ui, Icon::Lock, 20.0, pal.muted, fill);
                let w = (ui.available_width() - 56.0).max(80.0);
                ui.allocate_ui_with_layout(vec2(w, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(RichText::new(format!("Private {noun}")).font(egui::FontId::new(pal.ui(), crate::theme::medium_family())).color(pal.text));
                    ui.label(RichText::new("Only members and roles you pick can see it.").size(pal.meta()).color(pal.muted));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    ui.add_enabled(!busy, |ui: &mut egui::Ui| switch(ui, &pal, &mut m.private));
                });
            });
        }
        if let Err(why) = allowed {
            error_text(ui, theme, why);
        }
        if let Some(OpState::Failed(e)) = &status {
            error_text(ui, theme, e);
        }
        let label = if busy { "Creating…".to_owned() } else { title.clone() };
        dialog_buttons(ui, theme, &label, false, allowed.is_ok() && !busy && !m.text.trim().is_empty())
    });
    // Cancel, ×, Esc, backdrop; a pending Private follow-up keeps it open (it would be lost and
    // the channel stay public).
    let keep = busy && cx.view.server_menu_ui.private;
    match resp.inner {
        Some(true) => send_tracked(cx, cmd),
        Some(false) if !keep => close(cx),
        None if resp.should_close() && !keep => close(cx),
        _ => {}
    }
}

/// A kind choice (§9 option rows): icon 20, name 15/500 and its one-line description (`None`:
/// name only, short windows), the radio on the right. Hover tints the row; the radio shows the
/// choice. `bg`: the group fill under it (icon cut-outs).
fn kind_row(ui: &mut egui::Ui, pal: &Pal, kind: u8, label: &str, desc: Option<&str>, selected: bool, bg: Color32) -> egui::Response {
    let height = if desc.is_some() { 56.0 } else { 40.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    let hot = resp.hovered() || resp.has_focus();
    // Concentric with the group (radius r, 4 px inset).
    let r = pal.radius.saturating_sub(4).max(2);
    if resp.hovered() {
        ui.painter().rect_filled(rect, r, pal.tk.hover);
    }
    let icon = Rect::from_center_size(pos2(rect.left() + 22.0, rect.center().y), Vec2::splat(20.0));
    let color = if selected || hot { pal.text } else { pal.muted };
    icons::paint_channel(ui.painter(), icons::channel(kind, false, false, false), icon, color, bg);
    let (x, name) = (rect.left() + 44.0, egui::FontId::new(pal.body(), crate::theme::medium_family()));
    match desc {
        Some(d) => {
            ui.painter().text(pos2(x, rect.center().y + 1.0), Align2::LEFT_BOTTOM, label, name, pal.text);
            let g = one_line(ui, d, egui::FontId::proportional(pal.meta()), pal.muted, rect.right() - 44.0 - x);
            ui.painter().galley(pos2(x, rect.center().y + 2.0), g, pal.muted);
        }
        None => {
            ui.painter().text(pos2(x, rect.center().y), Align2::LEFT_CENTER, label, name, pal.text);
        }
    }
    paint_radio(ui.painter(), pal, Rect::from_center_size(pos2(rect.right() - 22.0, rect.center().y), Vec2::splat(20.0)), selected);
    if resp.has_focus() {
        super::messages::focus_ring(ui, pal, rect.shrink(2.0), f32::from(r));
    }
    resp.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, selected, label));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The shared pill switch (`messages::switch`, §9): `positive` when on, never accent.
pub(super) fn switch(ui: &mut egui::Ui, pal: &Pal, on: &mut bool) -> egui::Response {
    super::messages::switch(ui, pal, on, "")
}

/// Asks for an invite link with the dialog's expiry / use limit (opening Invite people, or
/// Generate a new link).
fn create_invite(cx: &mut UiCx<'_>, channel_id: Id) {
    let m = &cx.view.server_menu_ui;
    let cmd = AccountCommand::CreateInvite { channel_id, max_age: m.max_age, max_uses: m.max_uses, temporary: false };
    if cx.state.account_check(&cmd).is_ok() {
        cx.state.account.created_invite = Some(CreatedInvite { channel_id, result: None });
        cx.cmd.send(Command::Account(cmd));
    }
}

/// `Modal::Invite`: the link with Copy, its expiry, and (gear) expiry / use limit with
/// Generate a New Link.
pub fn invite_modal(ctx: &egui::Context, cx: &mut UiCx<'_>, guild_id: Id, channel_id: Id) {
    let name = cx.state.guild(guild_id).map(|g| g.name.clone()).unwrap_or_default();
    let ch = cx.state.channel_name(channel_id);
    let allowed = cx.state.account_check(&AccountCommand::CreateInvite { channel_id, max_age: 0, max_uses: 0, temporary: false });
    let pal = Pal::new(cx.theme);
    let theme = &*cx.theme;
    let made = cx.state.account.created_invite.as_ref().filter(|v| v.channel_id == channel_id);
    let loading = made.is_some_and(|v| v.result.is_none());
    let invite = made.and_then(|v| v.result.as_ref());
    let link = invite.and_then(|r| r.as_ref().ok()).and_then(|i| i.as_ref()).map(|i| (format!("https://discord.gg/{}", i.code), i.max_age.unwrap_or(0)));
    // Streamer Mode: the link masked on screen; Copy still copies it.
    let hide = cx.settings.streamer.hides_invites();
    let now = ctx.input(|i| i.time);
    let m = &mut cx.view.server_menu_ui;
    let copied = m.copied.is_some_and(|t| now - t < 1.5);
    let mut generate = false;
    let resp = dialog(ctx, theme, "rc_invite", 442.0, &format!("Invite friends to {name}"), |ui| {
        super::messages::icon_label(ui, Icon::Text, &ch, pal.ui(), pal.muted, ui.visuals().window_fill);
        field_label(ui, theme, "Send a server invite link to a friend");
        egui::Frame::new().fill(pal.bg).corner_radius(pal.radius).inner_margin(egui::Margin { left: 10, right: 4, top: 4, bottom: 4 }).show(ui, |ui| {
            egui::Sides::new().height(32.0).shrink_left().show(
                ui,
                |ui| {
                    let shown = match (&link, loading) {
                        (Some(_), _) if hide => RichText::new("discord.gg/••••••••").color(pal.text),
                        (Some((l, _)), _) => RichText::new(l).color(pal.text),
                        (None, true) => RichText::new("Creating a link…").color(pal.muted),
                        (None, false) => RichText::new("No link yet").color(pal.muted),
                    };
                    ui.add(egui::Label::new(shown).truncate().selectable(true));
                },
                |ui| {
                    // The view's one primary; "Copied" confirms in green with its word.
                    let (kind, label) = if copied { (Btn::Positive, "Copied") } else { (Btn::Primary, "Copy") };
                    if button(ui, &pal, kind, label, link.is_some()).clicked()
                        && let Some((l, _)) = &link
                    {
                        ui.ctx().copy_text(l.clone());
                        m.copied = Some(now);
                        ui.ctx().request_repaint_after(std::time::Duration::from_millis(1600));
                    }
                },
            );
        });
        match invite {
            Some(Err(e)) => error_text(ui, theme, e),
            Some(Ok(None)) => error_text(ui, theme, "Invites are paused for this server."),
            _ => {}
        }
        if let Err(why) = allowed {
            error_text(ui, theme, why);
        }
        ui.horizontal(|ui| {
            let age = link.as_ref().map_or(m.max_age, |l| l.1);
            let expiry = match INVITE_AGES.iter().find(|a| a.0 == age) {
                Some((0, _)) => "Your invite link never expires.".to_owned(),
                Some((_, label)) => format!("Your invite link expires in {label}."),
                None => format!("Your invite link expires in {} hours.", age / 3600),
            };
            ui.label(RichText::new(expiry).size(pal.meta()).color(pal.muted));
            if vector_button(ui, &pal, Icon::Gear, "Edit invite link", 24.0, m.invite_edit, pal.text).clicked() {
                m.invite_edit = !m.invite_edit;
            }
        });
        if m.invite_edit {
            field_label(ui, theme, "Expire after");
            choice(ui, "rc_invite_age", &mut m.max_age, &INVITE_AGES);
            field_label(ui, theme, "Max number of uses");
            choice(ui, "rc_invite_uses", &mut m.max_uses, &INVITE_USES);
            ui.add_space(8.0);
            generate = button(ui, &pal, Btn::Secondary, "Generate a new link", allowed.is_ok() && !loading).clicked();
        }
    });
    if generate {
        create_invite(cx, channel_id);
    }
    if resp.should_close() {
        close(cx);
    }
}

/// ComboBox over (value, label) presets, with the settings dropdowns' chevron.
pub(super) fn choice(ui: &mut egui::Ui, salt: &str, value: &mut u32, options: &[(u32, &str)]) -> bool {
    let shown = options.iter().find(|o| o.0 == *value).map_or("", |o| o.1);
    let mut changed = false;
    let combo = egui::ComboBox::from_id_salt(salt).selected_text(shown).width(ui.available_width().min(400.0));
    combo.icon(super::user_settings::combo_icon).show_ui(ui, |ui| {
        for (v, label) in options {
            changed |= ui.selectable_value(value, *v, *label).changed();
        }
    });
    changed
}

/// The code of an invite link or a bare code ("hTKzmak").
fn parse_code(text: &str) -> Option<String> {
    let t = text.trim();
    let bare = (2..=64).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    crate::api::guild_admin::invite_code(t).or(bare.then_some(t)).map(str::to_owned)
}

/// `Modal::JoinServer` (Add a server): Create my own (name, icon) or Join a server (link ->
/// Continue resolves its preview -> Join server accepts it).
pub fn join_server_modal(ctx: &egui::Context, cx: &mut UiCx<'_>) {
    let status = op_state(cx.state, cx.view.server_menu_ui.op).cloned();
    // Created / joined: the state opens the server once its GUILD_CREATE arrives.
    if let Some(OpState::Done(_)) = status {
        return close(cx);
    }
    match cx.view.server_menu_ui.step {
        Step::Start => start_screen(ctx, cx),
        Step::Create => create_screen(ctx, cx, status),
        Step::Join => join_screen(ctx, cx, status),
    }
}

fn start_screen(ctx: &egui::Context, cx: &mut UiCx<'_>) {
    let pal = Pal::new(cx.theme);
    let theme = &*cx.theme;
    let mut next = None;
    let resp = dialog(ctx, theme, "rc_add_server", 442.0, "Create a server", |ui| {
        dialog_text(ui, theme, "Your server is where you and your friends hang out.");
        ui.add_space(8.0);
        if big_choice(ui, &pal, Icon::Plus, "Create my own").clicked() {
            next = Some(Step::Create);
        }
        ui.add_space(16.0);
        ui.vertical_centered(|ui| ui.label(RichText::new("Have an invite already?").font(super::semibold(pal.heading())).color(pal.text)));
        if ui.vertical_centered(|ui| button(ui, &pal, Btn::Secondary, "Join a server", true)).inner.clicked() {
            next = Some(Step::Join);
        }
    });
    if let Some(step) = next {
        let m = &mut cx.view.server_menu_ui;
        m.step = step;
        if step == Step::Create {
            m.text = cx.state.me.as_ref().map(|u| format!("{}'s server", u.display_name())).unwrap_or_default();
        }
    }
    if resp.should_close() {
        close(cx);
    }
}

/// 56 px full-width choice row: icon, label (heading 16/600), chevron; a `rule` outline
/// (it is a button among text), the `hover` fill on hover.
fn big_choice(ui: &mut egui::Ui, pal: &Pal, icon: Icon, label: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0), Sense::click());
    let hot = resp.hovered() || resp.has_focus();
    ui.painter().rect(rect, pal.radius, if resp.hovered() { pal.tk.hover } else { Color32::TRANSPARENT }, Stroke::new(1.0, pal.tk.rule), egui::StrokeKind::Inside);
    if resp.has_focus() {
        super::messages::focus_ring(ui, pal, rect, f32::from(pal.radius));
    }
    let bg = ui.visuals().window_fill;
    icons::paint(ui.painter(), icon, Rect::from_center_size(pos2(rect.left() + 28.0, rect.center().y), Vec2::splat(24.0)), if hot { pal.text } else { pal.muted }, bg);
    ui.painter().text(pos2(rect.left() + 56.0, rect.center().y), Align2::LEFT_CENTER, label, super::semibold(pal.heading()), pal.text);
    icons::paint(ui.painter(), Icon::ChevronRight, Rect::from_center_size(pos2(rect.right() - 18.0, rect.center().y), Vec2::splat(20.0)), pal.muted, bg);
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, label));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Back (left, ghost) and the action (right, the primary). Some(true) = action, Some(false) = Back.
fn back_and(ui: &mut egui::Ui, pal: &Pal, action: &str, enabled: bool) -> Option<bool> {
    ui.add_space(8.0);
    // The dialog's pinned last row; Back first (left), so it comes first in the Tab order.
    super::dialog_footer(ui, |ui| {
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            let back = button(ui, pal, Btn::Ghost, "Back", true).clicked();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if button(ui, pal, Btn::Primary, action, enabled).clicked() { Some(true) } else { back.then_some(false) }
            })
            .inner
        })
        .inner
    })
}

fn create_screen(ctx: &egui::Context, cx: &mut UiCx<'_>, status: Option<OpState>) {
    let pal = Pal::new(cx.theme);
    let theme = &*cx.theme;
    let m = &mut cx.view.server_menu_ui;
    let busy = matches!(status, Some(OpState::Pending));
    let resp = dialog(ctx, theme, "rc_create_server", 442.0, "Customize your server", |ui| {
        dialog_text(ui, theme, "Give it a name and an icon. You can change both later.");
        ui.add_space(8.0);
        ui.vertical_centered(|ui| {
            let (rect, resp) = ui.allocate_exact_size(Vec2::splat(80.0), Sense::click());
            let hot = resp.hovered() || resp.has_focus();
            let stroke = Stroke::new(2.0, if hot { pal.text } else { pal.muted });
            match &m.icon {
                Some(_) => {
                    ui.painter().circle_filled(rect.center(), 40.0, pal.tk.selected);
                    icons::paint(ui.painter(), Icon::Check, Rect::from_center_size(rect.center(), Vec2::splat(28.0)), pal.text, Color32::TRANSPARENT);
                }
                None => {
                    // Dashed ring with a camera and "Upload", like the official dialog.
                    for i in 0..24 {
                        let a = std::f32::consts::TAU * i as f32 / 24.0;
                        ui.painter().line_segment([rect.center() + 39.0 * Vec2::angled(a), rect.center() + 39.0 * Vec2::angled(a + 0.15)], stroke);
                    }
                    icons::paint(ui.painter(), Icon::Camera, Rect::from_center_size(rect.center() - vec2(0.0, 8.0), Vec2::splat(20.0)), stroke.color, Color32::TRANSPARENT);
                    ui.painter().text(rect.center() + vec2(0.0, 14.0), Align2::CENTER_CENTER, "Upload", super::semibold(pal.badge()), stroke.color);
                }
            }
            resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, "Upload an icon"));
            let tip = m.icon.as_ref().and_then(|p| p.file_name()).map_or_else(|| "Upload an icon".to_owned(), |n| n.to_string_lossy().into_owned());
            if resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
                && let Some(path) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp"]).pick_file()
            {
                m.icon = Some(path);
            }
        });
        field_label(ui, theme, "Server name");
        ui.add_enabled(!busy, TextEdit::singleline(&mut m.text).char_limit(100).desired_width(f32::INFINITY));
        if let Some(OpState::Failed(e)) = &status {
            error_text(ui, theme, e);
        }
        let len = m.text.trim().chars().count();
        back_and(ui, &pal, if busy { "Creating…" } else { "Create" }, !busy && (2..=100).contains(&len))
    });
    match resp.inner {
        Some(true) => {
            let m = &cx.view.server_menu_ui;
            let cmd = AccountCommand::CreateGuild { name: m.text.trim().to_owned(), icon: m.icon.clone() };
            send_tracked(cx, cmd);
        }
        Some(false) => back_to_start(cx),
        None if resp.should_close() => close(cx),
        None => {}
    }
}

fn back_to_start(cx: &mut UiCx<'_>) {
    if let Some(op) = cx.view.server_menu_ui.op.take() {
        cx.state.account.ops.remove(&op);
    }
    cx.view.server_menu_ui = ServerMenuUi::default();
}

fn join_screen(ctx: &egui::Context, cx: &mut UiCx<'_>, status: Option<OpState>) {
    let pal = Pal::new(cx.theme);
    let busy = matches!(status, Some(OpState::Pending));
    let code = parse_code(&cx.view.server_menu_ui.text);
    // The preview shown: the one asked for with Continue, while the field still names it.
    let shown = code.as_ref().filter(|c| cx.view.server_menu_ui.code.as_ref() == Some(*c));
    let preview = shown.and_then(|c| cx.state.invite_preview(c));
    let resolved = preview.and_then(|p| p.result.as_ref()?.as_ref().ok());
    let known = resolved.and_then(|i| i.guild.as_ref()).map(|g| g.id).filter(|g| cx.state.guild(*g).is_some());
    let online = super::presence_color(crate::model::Presence::Online, &cx.theme.colors);
    let theme = &*cx.theme;
    let images = &mut *cx.images;
    let m = &mut cx.view.server_menu_ui;
    let mut edited = false;
    let resp = dialog(ctx, theme, "rc_join_server", 442.0, "Join a server", |ui| {
        dialog_text(ui, theme, "Paste an invite to join a server that already exists.");
        field_label(ui, theme, "Invite link");
        let field = ui.add_enabled(!busy, TextEdit::singleline(&mut m.text).hint_text("https://discord.gg/hTKzmak").desired_width(f32::INFINITY));
        edited = field.changed();
        super::hint(ui, theme, "Invites look like hTKzmak or https://discord.gg/cool-people");
        if let Some(p) = preview {
            match &p.result {
                Some(Ok(invite)) => invite_card(ui, &pal, images, invite, online),
                Some(Err(e)) => error_text(ui, theme, e),
                None => {
                    ui.add(egui::Spinner::new().size(16.0).color(pal.muted));
                }
            }
        }
        if let Some(OpState::Failed(e)) = &status {
            error_text(ui, theme, e);
        }
        let action = match (busy, known, resolved) {
            (true, ..) => "Joining…",
            (_, Some(_), _) => "Open server",
            (_, _, Some(_)) => "Join server",
            _ => "Continue",
        };
        let loading = preview.is_some_and(|p| p.loading);
        back_and(ui, &pal, action, !busy && code.is_some() && !loading)
    });
    match resp.inner {
        Some(true) => match (known, resolved, code) {
            (Some(g), ..) => {
                cx.state.select_guild(Some(g));
                close(cx);
            }
            (_, Some(invite), Some(code)) => {
                let cmd = AccountCommand::AcceptInvite {
                    code,
                    guild_id: invite.guild.as_ref().map(|g| g.id),
                    channel_id: invite.channel.as_ref().map(|c| c.id),
                    channel_type: invite.channel.as_ref().map(|c| c.kind),
                };
                send_tracked(cx, cmd);
            }
            (_, _, Some(code)) => {
                if cx.state.request_invite(&code, std::time::Instant::now()) {
                    cx.cmd.send(Command::Account(AccountCommand::ResolveInvite { code: code.clone() }));
                }
                let m = &mut cx.view.server_menu_ui;
                if let Some(op) = m.op.take() {
                    cx.state.account.ops.remove(&op);
                }
                m.code = Some(code);
            }
            _ => {}
        },
        Some(false) => back_to_start(cx),
        None if resp.should_close() => close(cx),
        // Another link: a failed join's error no longer applies.
        None if edited && !busy => {
            if let Some(op) = cx.view.server_menu_ui.op.take() {
                cx.state.account.ops.remove(&op);
            }
        }
        None => {}
    }
}

/// Resolved invite: server icon, name, online / member counts (or a group DM's name).
fn invite_card(ui: &mut egui::Ui, pal: &Pal, images: &mut super::images::ImageCache, invite: &crate::model::Invite, online: Color32) {
    egui::Frame::new().fill(pal.bg).corner_radius(pal.card_radius()).inner_margin(12).show(ui, |ui| {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(48.0), Sense::hover());
            let guild = invite.guild.as_ref();
            let name = guild.map(|g| g.name.clone()).or_else(|| invite.channel.as_ref().and_then(|c| c.name.clone())).unwrap_or_default();
            let url = guild.and_then(|g| crate::api::cdn::guild_icon_url(g, 96, false));
            let tex = url.and_then(|u| images.get(ui.ctx(), &u, 96, false));
            match tex {
                Some(t) => egui::Image::from_texture(t).corner_radius(14).paint_at(ui, rect),
                None => super::images::paint_initial(ui.painter(), rect, 14.0, guild.map_or(0, |g| g.id.0), &name, pal.dark),
            }
            ui.vertical(|ui| {
                ui.label(RichText::new(name).font(super::semibold(pal.heading())).color(pal.text));
                ui.horizontal(|ui| {
                    if let Some(n) = invite.approximate_presence_count {
                        stat(ui, pal, online, &format!("{n} online"));
                    }
                    if let Some(n) = invite.approximate_member_count {
                        stat(ui, pal, pal.muted, &format!("{n} members"));
                    }
                });
            });
        });
    });
}

fn stat(ui: &mut egui::Ui, pal: &Pal, dot: Color32, text: &str) {
    let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
    ui.painter().circle_filled(r.center(), 4.0, dot);
    ui.label(RichText::new(text).size(pal.meta()).color(pal.muted));
}

// ---------------------------------------------------------------------------
// Apply to join / accept the rules (docs/GUILD_CARD_NOTES.md)
// ---------------------------------------------------------------------------

/// One answer of the join form, by field (`ServerMenuUi::apply`).
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    Terms(bool),
    Text(String),
    Choice(Option<usize>),
}

/// Character limits of the two text field kinds.
fn text_limit(kind: &str) -> usize {
    if kind == "PARAGRAPH" { 1000 } else { 150 }
}

fn field_kind(f: &serde_json::Value) -> &str {
    f.get("field_type").and_then(serde_json::Value::as_str).unwrap_or_default()
}

/// The form fields to send: each field object as received plus its `response` (TERMS `true`,
/// text trimmed and cut to its limit, MULTIPLE_CHOICE the choice's index). None while a
/// required field is unanswered or a field is of a kind Rustcord can't show.
// UNCONFIRMED: an unanswered optional field is sent with `"response": null`.
pub(super) fn filled(fields: &[serde_json::Value], answers: &[Answer]) -> Option<Vec<serde_json::Value>> {
    use serde_json::{Value, json};
    fields
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let required = f.get("required").and_then(Value::as_bool).unwrap_or(false);
            let choices = f.get("choices").and_then(Value::as_array).map_or(0, Vec::len);
            let kind = field_kind(f);
            let response = match (kind, answers.get(i)) {
                ("TERMS", Some(Answer::Terms(true))) => json!(true),
                ("TEXT_INPUT" | "PARAGRAPH", Some(Answer::Text(t))) if !t.trim().is_empty() => json!(t.trim().chars().take(text_limit(kind)).collect::<String>()),
                ("MULTIPLE_CHOICE", Some(Answer::Choice(Some(c)))) if *c < choices => json!(c),
                ("TERMS" | "TEXT_INPUT" | "PARAGRAPH" | "MULTIPLE_CHOICE", _) if !required => Value::Null,
                _ => return None,
            };
            let mut f = f.clone();
            f.as_object_mut()?.insert("response".into(), response);
            Some(f)
        })
        .collect()
}

/// "Apply to join" on a tag card, or the rules after a join: the form, asked for as it opens.
pub(super) fn open_application(cx: &mut UiCx<'_>, guild_id: Id) {
    cx.state.account.verification = None;
    open(cx, Modal::JoinApplication { guild_id });
}

/// `Modal::JoinApplication`: the server's rules (a checkbox) and questions, then Submit
/// application (`SubmitJoinRequest`); a rules-only form is "Accept the rules of …". The
/// questions are fetched once as it opens (`FetchMemberVerification`); closing forgets them.
pub fn join_application_modal(ctx: &egui::Context, cx: &mut UiCx<'_>, guild_id: Id) {
    let name = (cx.state.guild_profile(guild_id).map(|p| p.name.clone()).filter(|n| !n.is_empty()))
        .or_else(|| cx.state.guild(guild_id).map(|g| g.name.clone()))
        .unwrap_or_else(|| "this server".into());
    if cx.state.account.verification.as_ref().is_none_or(|(g, _)| *g != guild_id) {
        let demo = demo_verification();
        cx.state.account.verification = Some((guild_id, demo.clone().map(Ok)));
        if demo.is_none() {
            cx.cmd.send(Command::Account(AccountCommand::FetchMemberVerification { guild_id }));
        }
    }
    let status = op_state(cx.state, cx.view.server_menu_ui.op).cloned();
    let rules_only = |v: &crate::model::MemberVerification| !v.form_fields.is_empty() && v.form_fields.iter().all(|f| field_kind(f) == "TERMS");
    if let Some(OpState::Done(joined)) = status {
        let rules = matches!(&cx.state.account.verification, Some((_, Some(Ok(v)))) if rules_only(v));
        let text = match joined {
            Some(_) if rules => format!("You can now talk in {name}."),
            Some(_) => format!("You joined {name}."),
            None => format!("Application sent to {name}."),
        };
        cx.view.toast(text, false);
        // The tag card keeps showing "Application sent" (its `ops` entry stays).
        cx.view.server_menu_ui.op = None;
        cx.state.account.verification = None;
        return close(cx);
    }
    let busy = matches!(status, Some(OpState::Pending));
    let pal = Pal::new(cx.theme);
    let theme = &*cx.theme;
    let loaded = match &cx.state.account.verification {
        Some((_, Some(r))) => Some(r.clone()),
        _ => None,
    };
    let m = &mut cx.view.server_menu_ui;
    if let Some(Ok(v)) = &loaded
        && m.apply.len() != v.form_fields.len()
    {
        m.apply = v.form_fields.iter().map(|f| match field_kind(f) {
            "TERMS" => Answer::Terms(false),
            "MULTIPLE_CHOICE" => Answer::Choice(None),
            _ => Answer::Text(String::new()),
        }).collect();
    }
    let rules = matches!(&loaded, Some(Ok(v)) if rules_only(v));
    let title = if rules { format!("Accept the rules of {name}") } else { format!("Apply to join {name}") };
    let mut retry = false;
    let resp = dialog(ctx, theme, "rc_join_application", 520.0, &title, |ui| {
        match &loaded {
            None => {
                ui.label(RichText::new("Loading the questions…").size(pal.meta()).color(pal.muted));
                None
            }
            Some(Err(_)) => {
                error_text(ui, theme, "Couldn't load the questions. Check your connection, then try again.");
                retry = button(ui, &pal, Btn::Secondary, "Try again", true).clicked();
                dialog_buttons(ui, theme, "Submit application", false, false)
            }
            Some(Ok(v)) => {
                let about = v.description.as_deref().map(str::trim).filter(|d| !d.is_empty());
                match about {
                    Some(d) => dialog_text(ui, theme, d),
                    None if !rules => dialog_text(ui, theme, "The server's moderators read every application."),
                    None => {}
                }
                let mut unknown = false;
                for (i, f) in v.form_fields.iter().enumerate() {
                    unknown |= !form_field(ui, &pal, theme, f, &mut m.apply[i], !busy, i);
                }
                if unknown {
                    error_text(ui, theme, "This server asks something Rustcord can't show yet. Apply in the official app.");
                }
                if let Some(OpState::Failed(e)) = &status {
                    error_text(ui, theme, e);
                }
                let ready = filled(&v.form_fields, &m.apply).is_some();
                let action = if busy { "Sending…" } else if rules { "Accept rules" } else { "Submit application" };
                dialog_buttons(ui, theme, action, false, ready && !busy)
            }
        }
    });
    if retry {
        cx.state.account.verification = None;
    }
    match resp.inner {
        Some(true) => {
            if let Some(Ok(v)) = loaded
                && let Some(form_fields) = filled(&v.form_fields, &cx.view.server_menu_ui.apply)
            {
                send_tracked(cx, AccountCommand::SubmitJoinRequest { guild_id, version: v.version, form_fields });
            }
        }
        Some(false) if !busy => {
            cx.state.account.verification = None;
            close(cx);
        }
        None if resp.should_close() && !busy => {
            cx.state.account.verification = None;
            close(cx);
        }
        _ => {}
    }
}

/// One field of the join form; false = a kind Rustcord can't show.
fn form_field(ui: &mut egui::Ui, pal: &Pal, theme: &crate::theme::Theme, f: &serde_json::Value, answer: &mut Answer, enabled: bool, i: usize) -> bool {
    use serde_json::Value;
    let text = |key: &str| f.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
    let required = f.get("required").and_then(Value::as_bool).unwrap_or(false);
    let kind = field_kind(f);
    let label = text("label");
    let heading = |ui: &mut egui::Ui, label: &str| {
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(RichText::new(label).font(super::semibold(pal.label())).color(pal.text));
            if !required {
                ui.label(RichText::new("Optional").size(pal.meta()).color(pal.muted));
            }
        });
    };
    match (kind, answer) {
        ("TERMS", Answer::Terms(on)) => {
            heading(ui, "Server rules");
            let rules: Vec<&str> = f.get("values").and_then(Value::as_array).map(|v| v.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            for (n, rule) in rules.iter().enumerate() {
                ui.add(egui::Label::new(RichText::new(format!("{}. {rule}", n + 1)).size(pal.ui()).color(pal.text)).wrap());
            }
            check_row(ui, pal, "I've read and agree to the rules", on, enabled);
        }
        ("TEXT_INPUT" | "PARAGRAPH", Answer::Text(t)) => {
            heading(ui, &label);
            let limit = text_limit(kind);
            let edit = if kind == "PARAGRAPH" { TextEdit::multiline(t).desired_rows(4) } else { TextEdit::singleline(t) };
            let hint = text("placeholder");
            ui.add_enabled(enabled, edit.char_limit(limit).hint_text(hint).desired_width(f32::INFINITY));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                ui.label(RichText::new(format!("{}/{limit}", t.chars().count())).size(pal.meta()).color(pal.muted));
            });
        }
        ("MULTIPLE_CHOICE", Answer::Choice(pick)) => {
            heading(ui, &label);
            let choices: Vec<&str> = f.get("choices").and_then(Value::as_array).map(|v| v.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            for (c, choice) in choices.iter().enumerate() {
                if radio_row(ui, pal, choice, *pick == Some(c), enabled, (i, c)) {
                    *pick = Some(c);
                }
            }
        }
        _ => {
            if !label.is_empty() {
                field_label(ui, theme, &label);
            }
            return false;
        }
    }
    true
}

/// A §9 checkbox row (20 px box, label ui 14).
pub(super) fn check_row(ui: &mut egui::Ui, pal: &Pal, label: &str, on: &mut bool, enabled: bool) {
    let font = egui::FontId::proportional(pal.ui());
    let w = ui.painter().layout_no_wrap(label.to_owned(), font.clone(), pal.text).size().x;
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0 + w, 32.0), if enabled { Sense::click() } else { Sense::hover() });
    if enabled && resp.clicked() {
        *on = !*on;
    }
    let radius = f32::from((pal.radius / 2).max(2));
    let check = Rect::from_center_size(pos2(rect.left() + 10.0, rect.center().y), Vec2::splat(20.0));
    super::messages::paint_checkbox(ui.painter(), &pal.tk, check, *on, radius);
    ui.painter().text(pos2(check.right() + 8.0, rect.center().y), Align2::LEFT_CENTER, label, font, if enabled { pal.text } else { pal.muted });
    if resp.has_focus() {
        super::messages::focus_ring(ui, pal, rect, radius);
    }
    resp.widget_info(|| WidgetInfo::selected(Role::CheckBox, enabled, *on, label));
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
}

/// A 32 px radio row (§9: the radio left of its label). True when picked.
fn radio_row(ui: &mut egui::Ui, pal: &Pal, label: &str, on: bool, enabled: bool, salt: (usize, usize)) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    let resp = ui.interact(rect, ui.make_persistent_id(("rc_apply_choice", salt)), if enabled { Sense::click() } else { Sense::hover() });
    if resp.hovered() && enabled {
        ui.painter().rect_filled(rect, pal.radius, pal.tk.hover);
    }
    paint_radio(ui.painter(), pal, Rect::from_center_size(pos2(rect.left() + 18.0, rect.center().y), Vec2::splat(20.0)), on);
    let g = one_line(ui, label, egui::FontId::proportional(pal.ui()), pal.text, rect.width() - 44.0);
    ui.painter().galley(pos2(rect.left() + 36.0, rect.center().y - g.size().y / 2.0), g, pal.text);
    if resp.has_focus() {
        super::messages::focus_ring(ui, pal, rect.shrink(2.0), f32::from(pal.radius));
    }
    resp.widget_info(|| WidgetInfo::selected(Role::RadioButton, enabled, on, label));
    enabled && resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// Test hook `RUSTCORD_OPEN=modal:apply` / `modal:apply-rules`: sample questions (rules, a
/// line, a paragraph, a choice) / only the rules, instead of a fetch.
fn demo_verification() -> Option<crate::model::MemberVerification> {
    let open = std::env::var("RUSTCORD_OPEN").unwrap_or_default();
    let has = |n: &str| open.split(',').any(|s| s.trim() == n);
    let rules_only = has("modal:apply-rules");
    if !rules_only && !has("modal:apply") {
        return None;
    }
    let terms = serde_json::json!({"field_type":"TERMS","label":"Read and agree to the server rules","values":["Be kind to each other.","No spam or self-promotion.","Keep it about building things."],"required":true,"description":null,"automations":null});
    let mut form_fields = vec![terms];
    if !rules_only {
        form_fields.extend([
            serde_json::json!({"field_type":"TEXT_INPUT","label":"What do you build with Rust?","placeholder":"A game, a CLI, a chat client…","required":true,"description":null,"automations":null}),
            serde_json::json!({"field_type":"PARAGRAPH","label":"Anything else we should know?","required":false,"description":null,"automations":null}),
            serde_json::json!({"field_type":"MULTIPLE_CHOICE","label":"How did you find us?","choices":["A friend","Server discovery","Somewhere else"],"required":true,"description":null,"automations":null}),
        ]);
    }
    let description = (!rules_only).then(|| "Tell us a bit about you. A moderator reads every application.".to_owned());
    Some(crate::model::MemberVerification { version: Some("2026-11-01T12:00:00+00:00".into()), description, form_fields })
}

/// `Modal::Moderate`: kick or ban with a reason (ban: delete-messages preset). The dialog is
/// the confirmation; it closes once the server answered.
pub fn moderate_modal(ctx: &egui::Context, cx: &mut UiCx<'_>, guild_id: Id, user_id: Id, ban: bool) {
    let status = op_state(cx.state, cx.view.server_menu_ui.op).cloned();
    if let Some(OpState::Done(_)) = status {
        return close(cx);
    }
    let who = user_name(cx.state, guild_id, user_id);
    let busy = matches!(status, Some(OpState::Pending));
    let theme = &*cx.theme;
    let m = &mut cx.view.server_menu_ui;
    let reason = Some(m.text.trim().to_owned()).filter(|r| !r.is_empty());
    let cmd = if ban {
        AccountCommand::BanMember { guild_id, user_id, reason, delete_message_seconds: m.delete_secs }
    } else {
        AccountCommand::KickMember { guild_id, user_id, reason }
    };
    let allowed = cx.state.account_check(&cmd);
    let (action, busy_label, body) = if ban {
        ("Ban", "Banning…", "They can't rejoin until you revoke the ban.")
    } else {
        ("Kick", "Kicking…", "They can rejoin with a new invite.")
    };
    let resp = dialog(ctx, theme, "rc_moderate", 442.0, &format!("{action} {who}?"), |ui| {
        dialog_text(ui, theme, body);
        if ban {
            field_label(ui, theme, "Delete message history");
            choice(ui, "rc_ban_delete", &mut m.delete_secs, &BAN_DELETE);
        }
        field_label(ui, theme, "Reason");
        ui.add_enabled(!busy, TextEdit::multiline(&mut m.text).char_limit(512).desired_rows(3).desired_width(f32::INFINITY));
        if let Err(why) = allowed {
            error_text(ui, theme, why);
        }
        if let Some(OpState::Failed(e)) = &status {
            error_text(ui, theme, e);
        }
        dialog_buttons(ui, theme, if busy { busy_label } else { action }, true, allowed.is_ok() && !busy)
    });
    match resp.inner {
        Some(true) => send_tracked(cx, cmd),
        Some(false) => close(cx),
        None if resp.should_close() => close(cx),
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Guild, User};

    #[test]
    fn privacy_lists_add_and_remove_the_guild() {
        assert_eq!(privacy_list(vec![1, 5], 5, true), [1]);
        assert_eq!(privacy_list(vec![1], 5, false), [1, 5]);
        // Off twice never lists it twice.
        assert_eq!(privacy_list(vec![5], 5, false), [5]);
    }

    #[test]
    fn leave_asks_first_and_never_for_the_owner() {
        let (mut view, mut state) = (ViewState::default(), AppState::default());
        state.me = Some(User { id: Id(1), ..Default::default() });
        state.guilds.push(Guild { id: Id(5), name: "Den".into(), owner_id: Id(2), ..Default::default() });
        ask_leave(&mut view, &state, Id(5));
        let c = view.confirm.take().expect("a question, nothing sent yet");
        assert!(c.danger && c.button == "Leave server" && c.title == "Leave Den?");
        assert!(matches!(c.then, Command::Account(AccountCommand::LeaveGuild { guild_id: Id(5) })));
        state.guilds[0].owner_id = Id(1);
        ask_leave(&mut view, &state, Id(5));
        assert!(view.confirm.is_none() && view.toasts.len() == 1);
    }

    #[test]
    fn invite_codes_from_links_and_bare_codes() {
        assert_eq!(parse_code(" https://discord.gg/cool-people ").as_deref(), Some("cool-people"));
        assert_eq!(parse_code("discord.com/invite/hTKzmak").as_deref(), Some("hTKzmak"));
        assert_eq!(parse_code("hTKzmak").as_deref(), Some("hTKzmak"));
        assert_eq!(parse_code("not a code"), None);
        assert_eq!(parse_code("x"), None);
    }

    #[test]
    fn join_form_answers_fill_every_field() {
        use serde_json::json;
        let fields = vec![
            json!({"field_type":"TERMS","label":"Rules","values":["Be kind."],"required":true,"automations":null,"extra":7}),
            json!({"field_type":"TEXT_INPUT","label":"Why?","required":true}),
            json!({"field_type":"PARAGRAPH","label":"More","required":false}),
            json!({"field_type":"MULTIPLE_CHOICE","label":"From","choices":["A","B"],"required":true}),
        ];
        let answers = |terms, text: &str, choice| vec![Answer::Terms(terms), Answer::Text(text.into()), Answer::Text(String::new()), Answer::Choice(choice)];
        let out = filled(&fields, &answers(true, "  Tools  ", Some(1))).expect("all required answered");
        assert_eq!(out[0]["response"], json!(true));
        assert_eq!(out[0]["extra"], json!(7), "unknown keys go back unchanged");
        assert_eq!(out[1]["response"], json!("Tools"), "trimmed");
        assert_eq!(out[2]["response"], serde_json::Value::Null, "optional, unanswered");
        assert_eq!(out[3]["response"], json!(1), "the choice's index");
        assert_eq!(filled(&fields, &answers(false, "x", Some(0))), None, "rules not accepted");
        assert_eq!(filled(&fields, &answers(true, "   ", Some(0))), None, "blank text");
        assert_eq!(filled(&fields, &answers(true, "x", None)), None, "no choice");
        assert_eq!(filled(&fields, &answers(true, "x", Some(2))), None, "no such choice");
        let long = filled(&fields, &answers(true, &"a".repeat(400), Some(0))).unwrap();
        assert_eq!(long[1]["response"].as_str().map(str::len), Some(150));
        let odd = vec![json!({"field_type":"VERIFICATION","label":"?","required":false})];
        assert_eq!(filled(&odd, &[Answer::Text(String::new())]), None, "a kind Rustcord can't show");
    }

    #[test]
    fn mark_tooltips_count_boosts() {
        assert_eq!(mark_tip((Icon::BoostTier2, "Boost level 2"), 9), "Boost level 2, 9 boosts");
        assert_eq!(mark_tip((Icon::BoostTier1, "Boost level 1"), 1), "Boost level 1, 1 boost");
        assert_eq!(mark_tip((Icon::BoostTier1, "Boost level 1"), 0), "Boost level 1");
        assert_eq!(mark_tip((Icon::Verified, "Verified server"), 14), "Verified server");
    }

    #[test]
    fn timed_mutes_end() {
        let at = |t: &str| Some(MuteConfig { end_time: Some(t.into()) });
        assert!(mute_active(true, &None), "until unmuted");
        assert!(mute_active(true, &at("2999-01-01T00:00:00+00:00")));
        assert!(!mute_active(true, &at("2001-01-01T00:00:00+00:00")));
        assert!(!mute_active(false, &None));
    }
}
