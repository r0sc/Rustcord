//! Server settings (FEATURES_NOTES §3.3-§3.8, layout §4.9): a full-screen layer while
//! `ViewState::server_settings` is `Some((guild, page))`, drawn by `main_layout` in the chat's
//! place. Nav (shared chrome: `settings_layer`, `nav_item`): the server name, then Overview
//! (name, icon, default notifications: `UpdateGuild`), Roles (list with drag reorder:
//! `ReorderRoles`; Create Role; `Page::Role` edits name / colour / hoist / mentionable /
//! permissions: `UpdateRole`, `DeleteRole`), Emoji (`UploadEmoji` with an `rfd` picker,
//! `DeleteEmoji`); User Management: Members (op 37 list + `ChatCommand::SearchMembers`; roles,
//! nickname, kick, ban through server_menu.rs), Invites (`InvitesView` + `FetchGuildInvites`,
//! `DeleteInvite`), Bans (`BansView` + `FetchBans` paging, `Unban`). `Page::Channel` is Edit
//! Channel / Edit Category with its own nav: Overview (name, topic, slowmode, age-restricted,
//! category: `UpdateChannel`), Permissions (overwrites: `SetPermissionOverwrite` /
//! `DeletePermissionOverwrite`), Delete Channel.
//!
//! Hook-up guide: docs/ACCOUNT_SERVER_DESIGN.md. Gate every control with `account_check(&cmd)`
//! (Err = disabled control with the text as hover); edits are drafts (`GuildPatch`,
//! `RolePatch`, `ChannelPatch`: None = as saved) with a floating "unsaved changes" bar whose
//! Save goes out with `begin(op)` and shows `account.ops[&op]`; deletes go through
//! `ViewState::confirm`. Only the open page's view is kept: `close` and page changes drop the
//! bans / invites views, ops entries and emoji textures.
//! OWNERSHIP: server engineer (docs/SOCIAL_UI_DESIGN.md).

use super::icons::{self, Icon};
use super::messages::{Btn, MenuIcon, Pal, button, focus_ring, menu_item, menu_setup, paint_radio, vector_button, vector_icon};
use super::server_menu::{self, choice, error_text, field_label, op_state, switch};
use super::{UiCx, hint, nav_heading, nav_item, one_line, page_title, rgb, section, settings_layer};
use crate::api::cdn;
use crate::backend::{AccountCommand, AccountOp, ChannelPatch, ChatCommand, Command, GuildPatch, OpKind, RolePatch};
use crate::model::{Id, Member, MemberListItem, Overwrite, Perms, Role as GuildRole, channel_type as ct, perm};
use crate::state::AppState;
use crate::state::account::{BansView, InvitesView, OpState};
use egui::{Align2, Color32, FontId, Rect, RichText, Sense, Stroke, TextEdit, UiBuilder, Vec2, pos2, vec2};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    Overview,
    Roles,
    Emoji,
    Members,
    Invites,
    Bans,
    /// Edit Channel / Edit Category of this channel.
    Channel(Id),
    /// Roles > Edit Role.
    Role(Id),
}

impl Page {
    /// Nav groups: the server's own pages, then User Management.
    const SERVER: [Page; 3] = [Page::Overview, Page::Roles, Page::Emoji];
    const PEOPLE: [Page; 3] = [Page::Members, Page::Invites, Page::Bans];

    pub fn title(self) -> &'static str {
        match self {
            Page::Overview | Page::Channel(_) => "Overview",
            Page::Roles | Page::Role(_) => "Roles",
            Page::Emoji => "Emoji",
            Page::Members => "Members",
            Page::Invites => "Invites",
            Page::Bans => "Bans",
        }
    }

    /// The nav row's line icon.
    fn icon(self) -> Icon {
        match self {
            Page::Overview | Page::Channel(_) => Icon::Gear,
            Page::Roles | Page::Role(_) => Icon::Shield,
            Page::Emoji => Icon::Smile,
            Page::Members => Icon::Members,
            Page::Invites => Icon::Link,
            Page::Bans => Icon::Ban,
        }
    }

    /// `RUSTCORD_OPEN=server-settings:<name>`: overview, roles, emoji, members, invites, bans,
    /// `role-<id>` (Edit Role), `channel-<id>` (Edit Channel).
    pub fn from_name(name: &str) -> Option<Page> {
        let id = |prefix: &str| name.strip_prefix(prefix)?.parse().ok().map(Id);
        (Page::SERVER.into_iter().chain(Page::PEOPLE))
            .find(|p| p.title().eq_ignore_ascii_case(name))
            .or_else(|| id("role-").map(Page::Role))
            .or_else(|| id("channel-").map(Page::Channel))
    }

    /// Whether I may use this page (nav rows are hidden otherwise).
    fn allowed(self, state: &AppState, guild_id: Id) -> bool {
        let ok = |cmd: AccountCommand| state.account_check(&cmd).is_ok();
        match self {
            Page::Overview => ok(AccountCommand::UpdateGuild { guild_id, patch: GuildPatch::default() }),
            Page::Roles | Page::Role(_) => ok(AccountCommand::CreateRole { guild_id, role: RolePatch::default() }),
            Page::Emoji => ok(AccountCommand::UploadEmoji { guild_id, name: String::new(), path: PathBuf::new() }),
            Page::Members => state.guild_permissions(guild_id) & (perm::KICK_MEMBERS | perm::BAN_MEMBERS | perm::MANAGE_ROLES | perm::MANAGE_NICKNAMES) != 0,
            Page::Invites => ok(AccountCommand::FetchGuildInvites { guild_id }),
            Page::Bans => ok(AccountCommand::FetchBans { guild_id, after: None }),
            Page::Channel(channel_id) => ok(AccountCommand::UpdateChannel { channel_id, patch: ChannelPatch::default() }),
        }
    }
}

/// The first server page I may use: what "Server Settings" opens (None = no such item).
pub fn first_page(state: &AppState, guild_id: Id) -> Option<Page> {
    Page::SERVER.into_iter().chain(Page::PEOPLE).find(|p| p.allowed(state, guild_id))
}

/// Drafts of the server settings forms (in `ViewState::server_settings_ui`); a page change
/// starts them fresh.
#[derive(Debug, Default)]
pub struct ServerSettingsUi {
    /// The page these drafts belong to (None = just opened).
    page: Option<Page>,
    /// Overview / Edit Role / Edit Channel: changed fields only.
    guild: GuildPatch,
    role: RolePatch,
    channel: ChannelPatch,
    /// Edit Role: Permissions tab; Edit Channel: the Permissions page.
    perms_tab: bool,
    /// Edit Channel > Permissions: the role / member edited and its draft (allow, deny).
    target: Option<(Id, u8)>,
    overwrite: Option<(u64, u64)>,
    /// Search field (roles, members, bans).
    filter: String,
    /// Bans: the indices matching (filter, ban count) — filtered once per change, not per frame.
    ban_hits: Option<(String, usize, Vec<usize>)>,
    /// Emoji: the picked file and its name.
    emoji: Option<(PathBuf, String)>,
    /// Roles: the role being dragged.
    drag: Option<Id>,
    /// The write this page waits on (save bar, emoji upload, revoke).
    op: Option<AccountOp>,
}

/// Opens the layer (server menu "Server Settings", channel menu "Edit Channel").
pub fn open(cx: &mut UiCx<'_>, guild_id: Id, page: Page) {
    cx.view.profile_card = None;
    cx.view.settings_open = false;
    cx.view.server_settings_ui = ServerSettingsUi::default();
    cx.view.server_settings = Some((guild_id, page));
}

/// Closes the layer and drops what its pages loaded.
pub fn close(cx: &mut UiCx<'_>) {
    leave_page(cx);
    cx.view.server_settings = None;
    cx.view.server_settings_ui = ServerSettingsUi::default();
}

/// Drops the open page's view, progress entry and emoji textures.
fn leave_page(cx: &mut UiCx<'_>) {
    if let Some(op) = cx.view.server_settings_ui.op.take() {
        cx.state.account.ops.remove(&op);
    }
    cx.state.account.bans = None;
    cx.state.account.invites = None;
    if cx.view.server_settings_ui.page == Some(Page::Emoji)
        && let Some((g, _)) = cx.view.server_settings
        && let Some(guild) = cx.state.guild(g)
    {
        for e in &guild.emojis {
            cx.images.remove(&cdn::emoji_url(e.id, false, EMOJI_PX));
        }
    }
}

/// First frame of a page: fresh drafts, and the one fetch its view needs (opening it is the
/// user action).
fn enter(cx: &mut UiCx<'_>, guild_id: Id, page: Page) {
    if cx.view.server_settings_ui.page == Some(page) {
        return;
    }
    leave_page(cx);
    cx.view.server_settings_ui = ServerSettingsUi { page: Some(page), ..Default::default() };
    let ok = |cx: &UiCx<'_>, cmd: &AccountCommand| cx.state.account_check(cmd).is_ok();
    match page {
        Page::Bans => {
            let cmd = AccountCommand::FetchBans { guild_id, after: None };
            if ok(cx, &cmd) {
                cx.state.account.bans = Some(BansView::new(guild_id));
                cx.cmd.send(Command::Account(cmd));
            }
        }
        Page::Invites => {
            let cmd = AccountCommand::FetchGuildInvites { guild_id };
            if ok(cx, &cmd) {
                cx.state.account.invites = Some(InvitesView::new(guild_id));
                cx.cmd.send(Command::Account(cmd));
            }
        }
        _ => {}
    }
}

/// What a page asks for once drawn (pages draw with shared borrows).
enum Act {
    /// A one-click write (errors become toasts).
    Send(AccountCommand),
    /// The page's save / upload: progress in the page.
    Save(AccountCommand),
    /// Ask first; `watch` keeps its progress (a revoked invite refetches the list).
    Ask { title: String, body: String, button: &'static str, then: AccountCommand, danger: bool, watch: bool },
    Go(Page),
    /// A refused action's reason.
    Toast(&'static str),
    /// Bans: the next page after this user.
    MoreBans(Id),
}

fn apply(cx: &mut UiCx<'_>, guild_id: Id, acts: Vec<Act>) {
    for act in acts {
        match act {
            Act::Send(cmd) => cx.cmd.send(Command::Account(cmd)),
            Act::Save(cmd) => {
                let op = cmd.op();
                if cx.state.account.begin(op) {
                    cx.cmd.send(Command::Account(cmd));
                }
                cx.view.server_settings_ui.op = Some(op);
            }
            Act::Ask { title, body, button, then, danger, watch } => {
                if watch {
                    // Pending until the confirmation sends it; a cancelled one is dropped with the page.
                    let _ = cx.state.account.begin(then.op());
                    cx.view.server_settings_ui.op = Some(then.op());
                }
                cx.view.confirm(title, body, button, Command::Account(then));
                if let Some(c) = cx.view.confirm.as_mut() {
                    c.danger = danger;
                }
            }
            Act::Go(page) => cx.view.server_settings = Some((guild_id, page)),
            Act::Toast(why) => cx.view.toast(why, true),
            Act::MoreBans(after) => {
                if let Some(v) = cx.state.account.bans.as_mut() {
                    v.loading = true;
                    cx.cmd.send(Command::Account(AccountCommand::FetchBans { guild_id, after: Some(after) }));
                }
            }
        }
    }
}

/// The page's save finished: forget it and its drafts (the gateway update shows the saved
/// values). Returns what it created, if anything.
fn settled(cx: &mut UiCx<'_>) -> Option<Option<Id>> {
    let op = cx.view.server_settings_ui.op?;
    let Some(OpState::Done(id)) = cx.state.account.ops.get(&op).cloned() else { return None };
    cx.state.account.ops.remove(&op);
    let s = &mut cx.view.server_settings_ui;
    (s.op, s.guild, s.role, s.channel, s.overwrite, s.emoji) = (None, GuildPatch::default(), RolePatch::default(), ChannelPatch::default(), None, None);
    Some(id)
}

/// The layer.
pub fn show(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id, page: Page) {
    // Left or deleted meanwhile (GUILD_DELETE).
    let Some(name) = cx.state.guild(guild_id).map(|g| g.name.clone()) else {
        close(cx);
        return;
    };
    // A deleted channel or role: back to the server's pages.
    let page = match page {
        Page::Channel(c) if !cx.state.channels.contains_key(&c) => Page::Overview,
        Page::Role(r) if !cx.state.guild(guild_id).is_some_and(|g| g.roles.iter().any(|x| x.id == r)) => Page::Roles,
        p => p,
    };
    cx.view.server_settings = Some((guild_id, page));
    enter(cx, guild_id, page);
    let (mut pick, mut tab, mut delete) = (None, None, false);
    let close_now = settings_layer(
        ui,
        cx,
        "server_settings",
        |ui, cx| match page {
            Page::Channel(c) => {
                let ch = cx.state.channels.get(&c);
                let category = ch.is_some_and(|c| c.kind == ct::GUILD_CATEGORY);
                let parent = ch.and_then(|c| c.parent_id).map(|p| cx.state.channel_name(p));
                let prefix = if category { "" } else { "# " };
                nav_heading(ui, cx.theme, &format!("{prefix}{}", cx.state.channel_name(c)), true);
                if let Some(p) = parent {
                    hint(ui, cx.theme, &p);
                }
                let perms = cx.view.server_settings_ui.perms_tab;
                if nav_item(ui, cx.theme, Icon::Gear, "Overview", !perms, false) {
                    tab = Some(false);
                }
                if nav_item(ui, cx.theme, Icon::Lock, "Permissions", perms, false) {
                    tab = Some(true);
                }
                if cx.state.account_check(&AccountCommand::DeleteChannel { channel_id: c }).is_ok() {
                    delete = nav_item(ui, cx.theme, Icon::Trash, if category { "Delete category" } else { "Delete channel" }, false, true);
                }
            }
            _ => {
                let mut first = true;
                for (heading, pages) in [(name.as_str(), &Page::SERVER), ("User management", &Page::PEOPLE)] {
                    let shown: Vec<Page> = pages.iter().copied().filter(|p| p.allowed(cx.state, guild_id) || *p == page).collect();
                    if shown.is_empty() {
                        continue;
                    }
                    nav_heading(ui, cx.theme, heading, std::mem::take(&mut first));
                    for p in shown {
                        if nav_item(ui, cx.theme, p.icon(), p.title(), page.title() == p.title(), false) {
                            pick = Some(p);
                        }
                    }
                }
            }
        },
        |ui, cx| match page {
            Page::Overview => overview(ui, cx, guild_id),
            Page::Roles => roles(ui, cx, guild_id),
            Page::Role(r) => role_editor(ui, cx, guild_id, r),
            Page::Emoji => emoji(ui, cx, guild_id),
            Page::Members => members(ui, cx, guild_id),
            Page::Invites => invites(ui, cx, guild_id),
            Page::Bans => bans(ui, cx, guild_id),
            Page::Channel(c) if cx.view.server_settings_ui.perms_tab => channel_permissions(ui, cx, guild_id, c),
            Page::Channel(c) => channel_overview(ui, cx, guild_id, c),
        },
    );
    if let Some(p) = pick {
        cx.view.server_settings = Some((guild_id, p));
    }
    if let Some(perms) = tab {
        let s = &mut cx.view.server_settings_ui;
        (s.perms_tab, s.channel, s.target, s.overwrite) = (perms, ChannelPatch::default(), None, None);
    }
    if delete && let Page::Channel(c) = page {
        server_menu::ask_delete_channel(cx.view, cx.state, c);
    }
    if close_now {
        close(cx);
    }
}

// ---------------------------------------------------------------------------
// Shared bits
// ---------------------------------------------------------------------------

/// Text field over a draft: shows `saved` until edited; editing back to `saved` drops the draft.
fn text_field(ui: &mut egui::Ui, draft: &mut Option<String>, saved: &str, limit: usize, rows: usize, enabled: bool) {
    let mut v = draft.clone().unwrap_or_else(|| saved.to_owned());
    let edit = if rows > 1 { TextEdit::multiline(&mut v).desired_rows(rows) } else { TextEdit::singleline(&mut v) };
    if ui.add_enabled(enabled, edit.char_limit(limit).desired_width(f32::INFINITY)).changed() {
        *draft = (v != saved).then_some(v);
    }
}

/// A value draft: None when it equals the saved value.
fn set<T: PartialEq>(draft: &mut Option<T>, saved: T, v: T) {
    *draft = (v != saved).then_some(v);
}

/// A setting row (§12): the label (ui 14/500) and `tip` as its muted description on the left,
/// the switch on the right, a hairline under it. Some(new value) when toggled.
fn switch_row(ui: &mut egui::Ui, pal: &Pal, label: &str, tip: &str, on: bool, enabled: bool) -> Option<bool> {
    let mut v = on;
    let changed = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(0, 12))
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                let w = (ui.available_width() - 56.0).max(80.0);
                ui.allocate_ui_with_layout(vec2(w, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.add_space(2.0);
                    ui.add(egui::Label::new(RichText::new(label).font(FontId::new(pal.ui(), crate::theme::medium_family())).color(pal.text)).wrap());
                    if !tip.is_empty() {
                        ui.add(egui::Label::new(RichText::new(tip).size(pal.meta()).color(pal.muted)).wrap());
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| ui.add_enabled(enabled, |ui: &mut egui::Ui| switch(ui, pal, &mut v)).changed()).inner
            })
            .inner
        })
        .inner;
    hairline(ui, pal);
    changed.then_some(v)
}

/// The `rule` hairline between rows of one list.
fn hairline(ui: &mut egui::Ui, pal: &Pal) {
    let r = ui.available_rect_before_wrap();
    ui.painter().hline(r.x_range(), r.top(), Stroke::new(1.0, pal.tk.rule));
    ui.add_space(1.0);
}

/// Floating "unsaved changes" bar at the bottom of the page column while `dirty` or a save is
/// pending / failed. Some(true) = Save, Some(false) = Reset.
fn save_bar(ui: &mut egui::Ui, cx: &UiCx<'_>, dirty: bool, why_not: Option<&str>) -> Option<bool> {
    let status = op_state(cx.state, cx.view.server_settings_ui.op);
    let pending = matches!(status, Some(OpState::Pending));
    let failed = match status {
        Some(OpState::Failed(e)) => Some(e.as_str()),
        _ => None,
    };
    if !dirty && !pending && failed.is_none() {
        return None;
    }
    let pal = Pal::new(cx.theme);
    let clip = ui.clip_rect();
    let col = ui.max_rect();
    let width = col.width().min(clip.width() - 20.0).max(200.0);
    let pos = pos2(col.left(), clip.bottom() - 20.0 - 52.0);
    // A float (E2): `raised`, a `rule` edge, the popup shadow.
    let shadow = ui.ctx().global_style().visuals.popup_shadow;
    let frame = egui::Frame::new().fill(pal.tk.raised).corner_radius(pal.card_radius()).stroke(Stroke::new(1.0, pal.tk.rule)).inner_margin(egui::Margin::symmetric(12, 10)).shadow(shadow);
    egui::Area::new(ui.make_persistent_id("rc_save_bar"))
        .order(egui::Order::Middle)
        .fixed_pos(pos)
        .show(ui.ctx(), |ui| {
            frame
                .show(ui, |ui| {
                    ui.set_width(width - 24.0);
                    let (text, color) = match failed {
                        Some(e) => (e, pal.danger),
                        None => ("You have unsaved changes.", pal.text),
                    };
                    egui::Sides::new()
                        .height(32.0)
                        .shrink_left()
                        .show(
                            ui,
                            |ui| ui.add(egui::Label::new(RichText::new(text).color(color)).truncate()).on_hover_text(text),
                            |ui| {
                                let label = if pending { "Saving…" } else { "Save changes" };
                                let mut r = button(ui, &pal, Btn::Primary, label, !pending && dirty && why_not.is_none());
                                if let Some(w) = why_not {
                                    r = r.on_disabled_hover_text(w);
                                }
                                let reset = button(ui, &pal, Btn::Ghost, "Reset", !pending).clicked();
                                if r.clicked() { Some(true) } else { reset.then_some(false) }
                            },
                        )
                        .1
                })
                .inner
        })
        .inner
}

/// Search field with a magnifier inside. True when the text changed.
fn search_field(ui: &mut egui::Ui, text: &mut String, hint_text: &str) -> bool {
    // Leaves ~120 px for a button beside it (Create Role) in narrow windows.
    let w = (ui.available_width() - 120.0).clamp(120.0, 360.0);
    let field = TextEdit::singleline(text).hint_text(hint_text).margin(egui::Margin { left: 26, right: 4, top: 2, bottom: 2 }).desired_width(w);
    let resp = ui.add(field);
    let glass = Rect::from_center_size(pos2(resp.rect.left() + 14.0, resp.rect.center().y), Vec2::splat(14.0));
    icons::paint(ui.painter(), Icon::Search, glass, ui.visuals().weak_text_color(), Color32::TRANSPARENT);
    resp.changed()
}

/// A list heading with its count right-aligned (meta, muted), as the member list groups do:
/// "Roles" .... 12. No "Roles — 12".
fn section_count(ui: &mut egui::Ui, theme: &crate::theme::Theme, title: &str, n: usize) {
    // `section`'s spacing and type, plus the count at the row's right edge.
    let pal = Pal::new(theme);
    ui.add_space(32.0 - ui.spacing().item_spacing.y);
    let head = ui.label(RichText::new(title).font(super::semibold(pal.heading())).color(pal.text)).rect;
    ui.painter().text(pos2(ui.max_rect().right(), head.center().y), Align2::RIGHT_CENTER, n.to_string(), FontId::proportional(pal.meta()), pal.muted);
}

/// A muted line in the middle of an empty list.
fn empty(ui: &mut egui::Ui, pal: &Pal, text: &str) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| ui.label(RichText::new(text).color(pal.muted)));
}

/// The page's one primary action (Create role, Upload emoji): `messages::button` `Primary`.
fn accent_button(ui: &mut egui::Ui, theme: &crate::theme::Theme, label: &str, enabled: bool) -> egui::Response {
    button(ui, &Pal::new(theme), Btn::Primary, label, enabled)
}

fn danger_button(ui: &mut egui::Ui, pal: &Pal, label: &str, enabled: bool) -> egui::Response {
    button(ui, pal, Btn::Danger, label, enabled)
}

/// A radio row (§9): the 20 px radio, then the label (ui 14/500). Clicked = picked.
fn radio_row(ui: &mut egui::Ui, pal: &Pal, label: &str, on: bool, enabled: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width().min(360.0), 32.0), if enabled { Sense::click() } else { Sense::hover() });
    let dot = Rect::from_center_size(pos2(rect.left() + 10.0, rect.center().y), Vec2::splat(20.0));
    paint_radio(ui.painter(), pal, dot, on);
    let color = if enabled { pal.text } else { pal.muted };
    ui.painter().text(pos2(rect.left() + 32.0, rect.center().y), Align2::LEFT_CENTER, label, FontId::new(pal.ui(), crate::theme::medium_family()), color);
    if resp.has_focus() {
        focus_ring(ui, pal, rect, f32::from(pal.radius));
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, enabled, on, label));
    if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }
}

/// Roles highest first (rank: position, then the lower id higher); @everyone last.
fn sorted_roles(roles: &[GuildRole]) -> Vec<&GuildRole> {
    let mut v: Vec<&GuildRole> = roles.iter().collect();
    v.sort_by_key(|r| (std::cmp::Reverse(r.position), r.id));
    v
}

fn role_color(r: &GuildRole, pal: &Pal) -> Color32 {
    if r.color == 0 { pal.muted } else { rgb(r.color) }
}

// ---------------------------------------------------------------------------
// Overview
// ---------------------------------------------------------------------------

fn overview(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id) {
    settled(cx);
    let pal = Pal::new(cx.theme);
    let Some(g) = cx.state.guild(guild_id) else { return };
    let check = cx.state.account_check(&AccountCommand::UpdateGuild { guild_id, patch: GuildPatch::default() });
    let can = check.is_ok();
    let (saved_name, saved_notify) = (g.name.clone(), g.default_message_notifications);
    let icon_url = cdn::guild_icon_url(g, 160, false);
    let theme = &*cx.theme;
    page_title(ui, theme, "Server overview");
    if let Err(why) = check {
        hint(ui, theme, why);
    }
    let s = &mut cx.view.server_settings_ui;
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(80.0), Sense::hover());
        match (&s.guild.icon, icon_url) {
            (Some(Some(_)), _) => {
                ui.painter().circle_filled(rect.center(), 40.0, pal.tk.selected);
                icons::paint(ui.painter(), Icon::Check, Rect::from_center_size(rect.center(), Vec2::splat(28.0)), pal.text, Color32::TRANSPARENT);
            }
            (None, Some(url)) => {
                let tex = cx.images.get(ui.ctx(), &url, 160, false);
                match tex {
                    Some(t) => egui::Image::from_texture(t).corner_radius(40).paint_at(ui, rect),
                    None => paint_initials(ui, rect, guild_id, &saved_name, &pal),
                }
            }
            _ => paint_initials(ui, rect, guild_id, &saved_name, &pal),
        }
        ui.add_space(12.0);
        ui.vertical(|ui| {
            hint(ui, theme, "At least 512×512 recommended.");
            ui.horizontal(|ui| {
                let picked = button(ui, &pal, Btn::Secondary, "Upload image", can).clicked();
                if picked && let Some(path) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp"]).pick_file() {
                    s.guild.icon = Some(Some(path));
                }
                let has = !matches!(s.guild.icon, Some(None)) && (s.guild.icon.is_some() || g.icon.is_some());
                if has && button(ui, &pal, Btn::Ghost, "Remove", can).clicked() {
                    s.guild.icon = if g.icon.is_some() { Some(None) } else { None };
                }
            });
            if let Some(Some(p)) = &s.guild.icon {
                hint(ui, theme, &p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            }
        });
    });
    field_label(ui, theme, "Server name");
    text_field(ui, &mut s.guild.name, &saved_name, 100, 1, can);
    section(ui, theme, "Default notifications");
    ui.add_space(8.0);
    let now = s.guild.default_message_notifications.unwrap_or(saved_notify);
    for (v, label) in [(0, "All messages"), (1, "Only @mentions")] {
        if radio_row(ui, &pal, label, now == v, can).clicked() {
            set(&mut s.guild.default_message_notifications, saved_notify, v);
        }
    }
    hint(ui, theme, "For members who haven't chosen their own.");
    let dirty = s.guild != GuildPatch::default();
    let name_ok = s.guild.name.as_ref().is_none_or(|n| (2..=100).contains(&n.trim().chars().count()));
    match save_bar(ui, cx, dirty, (!name_ok).then_some("Server names are 2-100 characters.")) {
        Some(true) => {
            let mut patch = cx.view.server_settings_ui.guild.clone();
            patch.name = patch.name.map(|n| n.trim().to_owned());
            apply(cx, guild_id, vec![Act::Save(AccountCommand::UpdateGuild { guild_id, patch })]);
        }
        Some(false) => reset(cx),
        None => {}
    }
}

/// A server without an icon: the first letters of its words in `text` 600 on `surface` (the
/// server bar's neutral initials tiles, docs/DESIGN.md §10.1).
fn paint_initials(ui: &egui::Ui, rect: Rect, _guild_id: Id, name: &str, pal: &Pal) {
    ui.painter().circle_filled(rect.center(), rect.width() / 2.0, pal.surface);
    let text: String = name.split_whitespace().filter_map(|w| w.chars().next()).take(3).collect();
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, super::semibold(rect.height() * 0.32), pal.text);
}

/// Reset in the save bar: drafts and a failed save go.
fn reset(cx: &mut UiCx<'_>) {
    let s = &mut cx.view.server_settings_ui;
    if let Some(op) = s.op.take() {
        cx.state.account.ops.remove(&op);
    }
    (s.guild, s.role, s.channel, s.overwrite) = (GuildPatch::default(), RolePatch::default(), ChannelPatch::default(), None);
}

// ---------------------------------------------------------------------------
// Roles
// ---------------------------------------------------------------------------

/// New positions after moving the role at `from` to slot `to` (after its removal) of `order`
/// (highest first, @everyone not included): the roles in between take over the positions of
/// the slots they land on, so roles outside the range keep theirs. Only changes are returned.
fn reorder(order: &[(Id, i32)], from: usize, to: usize) -> Vec<(Id, i32)> {
    if from >= order.len() {
        return Vec::new();
    }
    let mut ids: Vec<Id> = order.iter().map(|r| r.0).collect();
    let id = ids.remove(from);
    let to = to.min(ids.len());
    ids.insert(to, id);
    (from.min(to)..=from.max(to)).filter(|&k| ids[k] != order[k].0).map(|k| (ids[k], order[k].1)).collect()
}

fn roles(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id) {
    // Create Role answered: edit the new role once its GUILD_ROLE_CREATE is in.
    if let Some(OpState::Done(Some(id))) = op_state(cx.state, cx.view.server_settings_ui.op)
        && cx.state.guild(guild_id).is_some_and(|g| g.roles.iter().any(|r| r.id == *id))
    {
        let id = *id;
        settled(cx);
        cx.view.server_settings = Some((guild_id, Page::Role(id)));
        return;
    }
    let pal = Pal::new(cx.theme);
    let Some(g) = cx.state.guild(guild_id) else { return };
    let mut acts = Vec::new();
    let theme = &*cx.theme;
    let state = &*cx.state;
    let s = &mut cx.view.server_settings_ui;
    page_title(ui, theme, "Roles");
    // @everyone: the Default Permissions card, over the search (as on the web).
    let (card, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0), Sense::click());
    let hot = resp.hovered() || resp.has_focus();
    ui.painter().rect(card, pal.radius, if resp.hovered() { pal.tk.hover } else { Color32::TRANSPARENT }, Stroke::new(1.0, pal.tk.rule), egui::StrokeKind::Inside);
    if resp.has_focus() {
        focus_ring(ui, &pal, card, f32::from(pal.radius));
    }
    ui.painter().text(pos2(card.left() + 16.0, card.center().y - 9.0), Align2::LEFT_CENTER, "Default permissions", FontId::new(pal.ui(), crate::theme::medium_family()), pal.text);
    ui.painter().text(pos2(card.left() + 16.0, card.center().y + 10.0), Align2::LEFT_CENTER, "@everyone: what every member of the server can do", FontId::proportional(pal.meta()), pal.muted);
    icons::paint(ui.painter(), Icon::ChevronRight, Rect::from_center_size(pos2(card.right() - 18.0, card.center().y), Vec2::splat(20.0)), if hot { pal.text } else { pal.muted }, Color32::TRANSPARENT);
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        acts.push(Act::Go(Page::Role(guild_id)));
    }
    ui.add_space(12.0);
    let create = AccountCommand::CreateRole { guild_id, role: RolePatch { name: Some("new role".into()), ..Default::default() } };
    let can_create = state.account_check(&create);
    let creating = matches!(op_state(state, s.op), Some(OpState::Pending));
    ui.horizontal(|ui| {
        search_field(ui, &mut s.filter, "Search roles");
        let mut b = accent_button(ui, theme, if creating { "Creating…" } else { "Create role" }, can_create.is_ok() && !creating);
        if let Err(w) = can_create {
            b = b.on_disabled_hover_text(w);
        }
        if b.clicked() {
            acts.push(Act::Save(create));
        }
    });
    if let Some(OpState::Failed(e)) = op_state(state, s.op) {
        error_text(ui, theme, e);
    }
    let list: Vec<&GuildRole> = sorted_roles(&g.roles).into_iter().filter(|r| r.id != guild_id).collect();
    section_count(ui, theme, "Roles", list.len());
    let counts = role_counts(state, guild_id);
    let q = s.filter.trim().to_lowercase();
    let order: Vec<(Id, i32)> = list.iter().map(|r| (r.id, r.position)).collect();
    let mut rows: Vec<(Id, Rect)> = Vec::new();
    let mut menu_for = None;
    for r in list.iter().filter(|r| q.is_empty() || r.name.to_lowercase().contains(&q)) {
        let movable = q.is_empty() && state.can_manage_role(guild_id, r.id);
        let sense = if movable { Sense::click_and_drag() } else { Sense::click() };
        let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), sense);
        rows.push((r.id, row));
        let hot = resp.hovered() || resp.has_focus() || s.drag == Some(r.id);
        if hot {
            ui.painter().rect_filled(row, pal.radius, pal.tk.hover);
        }
        if movable && hot {
            icons::paint(ui.painter(), Icon::Grip, Rect::from_center_size(pos2(row.left() + 10.0, row.center().y), Vec2::splat(16.0)), pal.muted, Color32::TRANSPARENT);
        }
        // Shield-ish dot in the role colour.
        ui.painter().circle_filled(pos2(row.left() + 30.0, row.center().y), 6.0, role_color(r, &pal));
        let name = one_line(ui, r.name.clone(), FontId::new(pal.body(), crate::theme::medium_family()), pal.text, row.width() - 160.0);
        ui.painter().galley(pos2(row.left() + 46.0, row.center().y - name.size().y / 2.0), name, pal.text);
        if let Some(n) = counts.as_ref().map(|c| c.get(&r.id).copied().unwrap_or(0)) {
            let count = ui.painter().text(pos2(row.right() - 48.0, row.center().y), Align2::RIGHT_CENTER, n.to_string(), FontId::proportional(pal.meta()), pal.muted);
            icons::paint(ui.painter(), Icon::Person, Rect::from_center_size(pos2(count.left() - 10.0, row.center().y), Vec2::splat(14.0)), pal.muted, Color32::TRANSPARENT);
        }
        // "…": Edit / Delete Role, Copy Role ID (its popup is drawn after the rows).
        let dots = Rect::from_center_size(pos2(row.right() - 22.0, row.center().y), Vec2::splat(28.0));
        let more = ui.scope_builder(UiBuilder::new().max_rect(dots), |ui| server_menu::more_button(ui, &pal)).inner;
        if more.clicked() || egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&more)) {
            menu_for = Some((r.id, r.name.clone(), more));
        }
        hairline(ui, &pal);
        if resp.drag_started() {
            s.drag = Some(r.id);
        }
        if resp.on_hover_cursor(if movable { egui::CursorIcon::Grab } else { egui::CursorIcon::PointingHand }).clicked() {
            acts.push(Act::Go(Page::Role(r.id)));
        }
    }
    // Drag reorder: the gap under the pointer gets a `focus` line; dropping sends the moved range.
    if let Some(dragged) = s.drag {
        let y = ui.ctx().pointer_interact_pos().map_or(f32::MAX, |p| p.y);
        let gap = rows.iter().position(|(_, r)| y < r.center().y).unwrap_or(rows.len());
        let line_y = rows.get(gap).map_or_else(|| rows.last().map_or(0.0, |(_, r)| r.bottom()), |(_, r)| r.top());
        if let Some((_, first)) = rows.first() {
            ui.painter().hline(first.x_range(), line_y, Stroke::new(2.0, pal.tk.focus));
        }
        if !ui.input(|i| i.pointer.any_down()) {
            s.drag = None;
            let from = order.iter().position(|o| o.0 == dragged);
            if let Some(from) = from {
                let to = if gap > from { gap - 1 } else { gap };
                let positions = reorder(&order, from, to);
                if !positions.is_empty() {
                    let cmd = AccountCommand::ReorderRoles { guild_id, positions };
                    match state.account_check(&cmd) {
                        Ok(()) => acts.push(Act::Send(cmd)),
                        Err(why) => acts.push(Act::Toast(why)),
                    }
                }
            }
        }
    }
    if list.is_empty() {
        empty(ui, &pal, "No roles yet. Create one to group members and their permissions.");
    }
    if let Some((role_id, name, resp)) = menu_for {
        egui::Popup::menu(&resp).show(|ui| {
            menu_setup(ui);
            if menu_item(ui, "Edit role", MenuIcon::Icon(Icon::Edit), pal.text) {
                acts.push(Act::Go(Page::Role(role_id)));
            }
            let delete = AccountCommand::DeleteRole { guild_id, role_id };
            if state.account_check(&delete).is_ok() && menu_item(ui, "Delete role", MenuIcon::Icon(Icon::Trash), pal.danger) {
                let body = "Members lose it and its permissions. You can't undo this.".to_owned();
                acts.push(Act::Ask { title: format!("Delete {name}?"), body, button: "Delete role", then: delete, danger: true, watch: false });
            }
            // Settings > Advanced > Developer Mode.
            let dev = cx.state.account.settings.developer_mode();
            if dev {
                ui.separator();
            }
            if dev && menu_item(ui, "Copy role ID", MenuIcon::Icon(Icon::IdCard), pal.text) {
                ui.ctx().copy_text(role_id.0.to_string());
            }
        });
    }
    apply(cx, guild_id, acts);
}

/// Members per role, only while the member list holds every member (small servers): a count
/// from part of the list would mislead.
// shortcut: no counts on larger servers; `GET /guilds/{g}/roles/member-counts` would give them.
fn role_counts(state: &AppState, guild_id: Id) -> Option<HashMap<Id, usize>> {
    let list = state.member_lists.get(&guild_id)?;
    let members: Vec<&Member> = (list.items.iter())
        .filter_map(|it| match it {
            MemberListItem::Member { member, .. } => Some(member),
            MemberListItem::Group { .. } => None,
        })
        .collect();
    if members.is_empty() || members.len() < list.member_count as usize {
        return None;
    }
    let mut counts = HashMap::new();
    for r in members.iter().flat_map(|m| &m.roles) {
        *counts.entry(*r).or_insert(0) += 1;
    }
    Some(counts)
}

/// Role colour swatches offered next to the custom picker (two rows: bright, dark).
const SWATCHES: [u32; 20] = [
    0x1ABC9C, 0x2ECC71, 0x3498DB, 0x9B59B6, 0xE91E63, 0xF1C40F, 0xE67E22, 0xE74C3C, 0x95A5A6, 0x607D8B, //
    0x11806A, 0x1F8B4C, 0x206694, 0x71368A, 0xAD1457, 0xC27C0E, 0xA84300, 0x992D22, 0x979C9F, 0x546E7A,
];
/// Where a permission applies: server-wide (roles), text-like channels, voice-like channels.
const G: u8 = 1;
const T: u8 = 2;
const V: u8 = 4;
/// Permission toggles grouped like the official client: (bit, name, what it allows, where).
type PermRow = (u8, &'static str, &'static str, u8);
const PERMS: &[(&str, &[PermRow])] = &[
    (
        "General server permissions",
        &[
            (10, "View Channels", "See channels (and read them, with Read Message History)", G | T | V),
            (4, "Manage Channels", "Create, edit or delete channels", G | T | V),
            (28, "Manage Roles", "Create roles and edit or delete roles below their highest role", G | T | V),
            (43, "Create Expressions", "Add custom emoji, stickers and sounds", G),
            (30, "Manage Expressions", "Edit or remove custom emoji, stickers and sounds", G),
            (7, "View Audit Log", "See who changed what in the server", G),
            (29, "Manage Webhooks", "Create, edit or delete webhooks", G | T),
            (5, "Manage Server", "Change the server's name, icon and settings", G),
        ],
    ),
    (
        "Membership permissions",
        &[
            (0, "Create Invite", "Invite new people to this server", G | T | V),
            (26, "Change Nickname", "Change their own nickname", G),
            (27, "Manage Nicknames", "Change other members' nicknames", G),
            (1, "Kick Members", "Remove members; they can rejoin with a new invite", G),
            (2, "Ban Members", "Ban members and delete their recent messages", G),
            (40, "Timeout Members", "Stop members from chatting or speaking for a while", G),
        ],
    ),
    (
        "Text channel permissions",
        &[
            (11, "Send Messages", "Send messages in text channels", G | T | V),
            (38, "Send Messages in Threads", "Send messages in threads", G | T),
            (35, "Create Public Threads", "Create threads everyone in the channel can see", G | T),
            (36, "Create Private Threads", "Create invite-only threads", G | T),
            (14, "Embed Links", "Links show a preview", G | T | V),
            (15, "Attach Files", "Upload files and media", G | T | V),
            (6, "Add Reactions", "Add new emoji reactions to messages", G | T | V),
            (18, "Use External Emoji", "Use emoji from other servers", G | T | V),
            (37, "Use External Stickers", "Use stickers from other servers", G | T | V),
            (17, "Mention @everyone, @here and All Roles", "Notify everyone, everyone online, or any role", G | T | V),
            (13, "Manage Messages", "Delete others' messages", G | T | V),
            (51, "Pin Messages", "Pin and unpin messages", G | T | V),
            (34, "Manage Threads", "Rename, archive, lock and delete threads", G | T),
            (16, "Read Message History", "Read earlier messages", G | T | V),
            (12, "Send Text-to-Speech Messages", "Send /tts messages", G | T | V),
            (46, "Send Voice Messages", "Send voice messages", G | T | V),
            (49, "Create Polls", "Create polls", G | T | V),
            (52, "Bypass Slowmode", "Not affected by slowmode", G | T | V),
        ],
    ),
    (
        "Voice channel permissions",
        &[
            (20, "Connect", "Join voice channels", G | V),
            (21, "Speak", "Talk in voice channels", G | V),
            (9, "Video", "Share their camera, screen or a game", G | V),
            (42, "Use Soundboard", "Play sounds from the soundboard", G | V),
            (45, "Use External Sounds", "Use sounds from other servers", G | V),
            (25, "Use Voice Activity", "Talk without push-to-talk", G | V),
            (8, "Priority Speaker", "Be heard more easily", G | V),
            (22, "Mute Members", "Mute others in voice channels", G | V),
            (23, "Deafen Members", "Deafen others in voice channels", G | V),
            (24, "Move Members", "Move or disconnect members between voice channels", G | V),
        ],
    ),
    (
        "App permissions",
        &[
            (31, "Use Application Commands", "Use commands from apps", G | T | V),
            (39, "Use Activities", "Use activities in voice channels", G | V),
            (50, "Use External Apps", "Use apps that are not added to the server", G | T | V),
        ],
    ),
    ("Event permissions", &[(44, "Create Events", "Create server events", G | V), (33, "Manage Events", "Edit and cancel events", G | V)]),
    ("Advanced permissions", &[(3, "Administrator", "Every permission, and it overrides channel permissions. Give it with care.", G)]),
];

fn role_editor(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id, role_id: Id) {
    settled(cx);
    let pal = Pal::new(cx.theme);
    let theme = &*cx.theme;
    let state = &*cx.state;
    let Some(role) = state.guild(guild_id).and_then(|g| g.roles.iter().find(|r| r.id == role_id)) else { return };
    let everyone = role_id == guild_id;
    let check = state.account_check(&AccountCommand::UpdateRole { guild_id, role_id, patch: RolePatch::default() });
    let can = check.is_ok();
    let mine = state.guild_permissions(guild_id);
    let s = &mut cx.view.server_settings_ui;
    let mut acts = Vec::new();
    let back = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        vector_icon(ui, Icon::ChevronLeft, 16.0, pal.muted, Color32::TRANSPARENT);
        ui.label(RichText::new("Back").color(pal.muted));
    });
    let back = ui.interact(back.response.rect, ui.make_persistent_id("rc_role_back"), Sense::click());
    back.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Back"));
    if back.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        acts.push(Act::Go(Page::Roles));
    }
    page_title(ui, theme, &format!("Edit {}", if everyone { "@everyone" } else { &role.name }));
    if let Err(why) = check {
        hint(ui, theme, why);
    }
    if !everyone {
        // Display | Permissions: the shared segmented control (§9), never an accent underline.
        ui.allocate_ui(vec2(ui.available_width().min(320.0), 36.0), |ui| crate::ui::login::segmented(ui, theme, &mut s.perms_tab, &[(false, "Display"), (true, "Permissions")]));
    }
    if everyone || s.perms_tab {
        let bits = s.role.permissions.unwrap_or(role.permissions.0);
        for (title, rows) in PERMS {
            section(ui, theme, title);
            for &(bit, name, tip, _) in rows.iter().filter(|r| r.3 & G != 0) {
                let b = 1u64 << bit;
                // Only bits I have myself may change (`account_check` repeats this on save).
                if let Some(on) = switch_row(ui, &pal, name, tip, bits & b != 0, can && mine & b != 0) {
                    set(&mut s.role.permissions, role.permissions.0, if on { bits | b } else { bits & !b });
                }
            }
        }
    } else {
        field_label(ui, theme, "Role name");
        text_field(ui, &mut s.role.name, &role.name, 100, 1, can);
        field_label(ui, theme, "Role colour");
        let color = s.role.color.unwrap_or(role.color);
        ui.add_enabled_ui(can, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                if swatch(ui, &pal, 0, color == 0).on_hover_text("Default").clicked() {
                    set(&mut s.role.color, role.color, 0);
                }
                let mut c = [(color >> 16) as u8, (color >> 8) as u8, color as u8];
                if ui.color_edit_button_srgb(&mut c).on_hover_text("Custom colour").changed() {
                    set(&mut s.role.color, role.color, u32::from(c[0]) << 16 | u32::from(c[1]) << 8 | u32::from(c[2]));
                }
                for sw in SWATCHES {
                    if swatch(ui, &pal, sw, color == sw).clicked() {
                        set(&mut s.role.color, role.color, sw);
                    }
                }
            });
        });
        ui.add_space(12.0);
        hairline(ui, &pal);
        let hoist = s.role.hoist.unwrap_or(role.hoist);
        if let Some(v) = switch_row(ui, &pal, "Display role members separately from online members", "", hoist, can) {
            set(&mut s.role.hoist, role.hoist, v);
        }
        let mention = s.role.mentionable.unwrap_or(role.mentionable);
        if let Some(v) = switch_row(ui, &pal, "Allow anyone to @mention this role", "Members without the Mention @everyone permission can ping it too.", mention, can) {
            set(&mut s.role.mentionable, role.mentionable, v);
        }
        ui.add_space(16.0);
        let delete = AccountCommand::DeleteRole { guild_id, role_id };
        if state.account_check(&delete).is_ok() && danger_button(ui, &pal, "Delete role", true).clicked() {
            let body = "Members lose it and its permissions. You can't undo this.".to_owned();
            acts.push(Act::Ask { title: format!("Delete {}?", role.name), body, button: "Delete role", then: delete, danger: true, watch: false });
        }
    }
    let dirty = s.role != RolePatch::default();
    let name_ok = s.role.name.as_ref().is_none_or(|n| !n.trim().is_empty());
    let patch = s.role.clone();
    let why_not = if name_ok { state.account_check(&AccountCommand::UpdateRole { guild_id, role_id, patch: patch.clone() }).err() } else { Some("A role needs a name.") };
    match save_bar(ui, cx, dirty, why_not) {
        Some(true) => acts.push(Act::Save(AccountCommand::UpdateRole { guild_id, role_id, patch })),
        Some(false) => reset(cx),
        None => {}
    }
    apply(cx, guild_id, acts);
}

/// 24 px colour square (0 = the default grey), ticked when selected.
fn swatch(ui: &mut egui::Ui, pal: &Pal, color: u32, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::click());
    let fill = rgb(if color == 0 { 0x99AAB5 } else { color });
    ui.painter().rect_filled(rect, 4, fill);
    if resp.hovered() || resp.has_focus() {
        ui.painter().rect_stroke(rect, 4, Stroke::new(2.0, pal.text), egui::StrokeKind::Outside);
    }
    if selected {
        icons::paint(ui.painter(), Icon::Check, Rect::from_center_size(rect.center(), Vec2::splat(16.0)), Color32::WHITE, Color32::TRANSPARENT);
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, ui.is_enabled(), selected, format!("#{color:06X}")));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ---------------------------------------------------------------------------
// Emoji
// ---------------------------------------------------------------------------

/// Texture size of the emoji list (32 px rows at up to 2x).
const EMOJI_PX: u32 = 64;

/// An emoji name from a file name: `[A-Za-z0-9_]`, 2-32 characters.
fn emoji_name(stem: &str) -> String {
    let mut n: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).take(32).collect();
    if n.len() < 2 {
        n = format!("emoji_{n}");
    }
    n
}

fn valid_emoji_name(n: &str) -> bool {
    (2..=32).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn emoji(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id) {
    settled(cx);
    let pal = Pal::new(cx.theme);
    let theme = &*cx.theme;
    let state = &*cx.state;
    let Some(g) = state.guild(guild_id) else { return };
    let upload_check = state.account_check(&AccountCommand::UploadEmoji { guild_id, name: String::new(), path: PathBuf::new() });
    let can_delete = state.account_check(&AccountCommand::DeleteEmoji { guild_id, emoji_id: Id(0) }).is_ok();
    let status = op_state(state, cx.view.server_settings_ui.op);
    let busy = matches!(status, Some(OpState::Pending));
    let images = &mut *cx.images;
    let s = &mut cx.view.server_settings_ui;
    let mut acts = Vec::new();
    page_title(ui, theme, "Emoji");
    hint(ui, theme, "PNG, JPG, GIF or WebP, resized to 128×128. Names have 2-32 letters, numbers or _.");
    ui.add_space(8.0);
    let mut b = accent_button(ui, theme, "Upload emoji", upload_check.is_ok() && s.emoji.is_none());
    if let Err(why) = upload_check {
        b = b.on_disabled_hover_text(why);
    }
    if b.clicked()
        && let Some(path) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp"]).pick_file()
    {
        let name = emoji_name(&path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
        s.emoji = Some((path, name));
    }
    let mut cancel = false;
    if let Some((path, name)) = &mut s.emoji {
        egui::Frame::new().fill(pal.bg).corner_radius(pal.radius).inner_margin(8).show(ui, |ui| {
            ui.horizontal(|ui| {
                let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                ui.add(egui::Label::new(RichText::new(file).color(pal.muted)).truncate());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    cancel = vector_button(ui, &pal, Icon::Close, "Cancel", 24.0, false, pal.text).clicked() && !busy;
                    let ok = valid_emoji_name(name) && !busy;
                    if button(ui, &pal, Btn::Secondary, if busy { "Uploading…" } else { "Upload" }, ok).clicked() {
                        acts.push(Act::Save(AccountCommand::UploadEmoji { guild_id, name: name.clone(), path: path.clone() }));
                    }
                    ui.add_enabled(!busy, TextEdit::singleline(name).char_limit(32).prefix(":").suffix(":").desired_width(180.0));
                });
            });
        });
        if let Some(OpState::Failed(e)) = status {
            error_text(ui, theme, e);
        }
    }
    if cancel {
        s.emoji = None;
    }
    let slots = [50, 100, 150, 250][usize::from(g.premium_tier.min(3))];
    for animated in [false, true] {
        let list: Vec<_> = g.emojis.iter().filter(|e| e.animated == animated).collect();
        section(ui, theme, if animated { "Animated emoji" } else { "Emoji" });
        hint(ui, theme, &format!("{} of {slots} slots free", slots - list.len().min(slots)));
        ui.add_space(4.0);
        for e in list {
            let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
            if !ui.is_rect_visible(row) {
                continue;
            }
            if resp.hovered() {
                ui.painter().rect_filled(row, pal.radius, pal.tk.hover);
            }
            let icon = Rect::from_center_size(pos2(row.left() + 24.0, row.center().y), Vec2::splat(32.0));
            match images.get(ui.ctx(), &cdn::emoji_url(e.id, false, EMOJI_PX), EMOJI_PX, false) {
                Some(t) => egui::Image::from_texture(t).maintain_aspect_ratio(true).fit_to_exact_size(icon.size()).paint_at(ui, icon),
                None => {
                    ui.painter().rect_filled(icon, (pal.radius / 2).max(2), pal.tk.hover);
                }
            }
            let name = one_line(ui, format!(":{}:", e.name), FontId::proportional(pal.size), pal.text, row.width() - 120.0);
            ui.painter().galley(pos2(row.left() + 56.0, row.center().y - name.size().y / 2.0), name, pal.text);
            if can_delete && resp.hovered() {
                let x = Rect::from_center_size(pos2(row.right() - 20.0, row.center().y), Vec2::splat(24.0));
                let clicked = ui.scope_builder(UiBuilder::new().max_rect(x), |ui| vector_button(ui, &pal, Icon::Trash, "Delete emoji", 24.0, false, pal.danger)).inner.clicked();
                if clicked {
                    let then = AccountCommand::DeleteEmoji { guild_id, emoji_id: e.id };
                    let body = "It disappears from every message that uses it. You can't undo this.".to_owned();
                    acts.push(Act::Ask { title: format!("Delete :{}:?", e.name), body, button: "Delete emoji", then, danger: true, watch: false });
                }
            }
            hairline(ui, &pal);
        }
    }
    apply(cx, guild_id, acts);
}

// ---------------------------------------------------------------------------
// Members
// ---------------------------------------------------------------------------

/// A member object: from the member list, else from member search answers.
fn member_of(state: &AppState, guild_id: Id, user_id: Id) -> Option<&Member> {
    state.find_member(guild_id, user_id).or_else(|| {
        (state.chat.member_search.iter().filter(|s| s.guild_id == guild_id).flat_map(|s| &s.members)).find(|m| m.user.as_ref().is_some_and(|u| u.id == user_id))
    })
}

fn members(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id) {
    let pal = Pal::new(cx.theme);
    page_title(ui, cx.theme, "Server members");
    let s = &mut cx.view.server_settings_ui;
    let changed = search_field(ui, &mut s.filter, "Search by name or ID");
    let q = s.filter.trim().to_lowercase();
    // Beyond the member list's first 100: op 8 search (the gateway debounces per guild).
    if changed && cx.state.need_member_search(guild_id, &q, 100) {
        cx.cmd.send(Command::Chat(ChatCommand::SearchMembers { guild_id, query: q.clone(), limit: 100 }));
    }
    let state = &*cx.state;
    let listed = (state.member_lists.get(&guild_id).into_iter().flat_map(|l| &l.items)).filter_map(|it| match it {
        MemberListItem::Member { member, .. } => Some(member),
        MemberListItem::Group { .. } => None,
    });
    let found = (state.chat.member_search.iter()).filter(|s| !q.is_empty() && s.guild_id == guild_id).flat_map(|s| &s.members);
    let mut ids: Vec<Id> = Vec::new();
    for m in listed.chain(found) {
        let Some(u) = &m.user else { continue };
        let hit = q.is_empty()
            || [Some(u.username.as_str()), u.global_name.as_deref(), m.nick.as_deref()].into_iter().flatten().any(|n| n.to_lowercase().contains(&q))
            || u.id.to_string() == q;
        if hit && !ids.contains(&u.id) {
            ids.push(u.id);
        }
    }
    // The server's count while not searching; none until the member list says it.
    let total = state.member_lists.get(&guild_id).map_or(0, |l| l.member_count as usize);
    match if q.is_empty() { total.max(ids.len()) } else { ids.len() } {
        0 if q.is_empty() => section(ui, cx.theme, "Members"),
        n => section_count(ui, cx.theme, "Members", n),
    }
    if ids.is_empty() {
        empty(ui, &pal, if q.is_empty() { "Search to find members." } else { "No members found." });
        return;
    }
    // Once per frame, not per row: roles by rank, and whether I may hand out roles at all.
    let ranked: Vec<&GuildRole> = state.guild(guild_id).map(|g| sorted_roles(&g.roles)).unwrap_or_default();
    let manage_roles = state.guild_permissions(guild_id) & perm::MANAGE_ROLES != 0;
    let me = state.me.as_ref().map(|u| u.id);
    let images = &mut *cx.images;
    let (mut roles_for, mut menu_for) = (None, None);
    for uid in ids {
        let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::hover());
        if !ui.is_rect_visible(row) {
            continue;
        }
        let Some(m) = member_of(state, guild_id, uid) else { continue };
        // My own member object carries no user.
        let Some(u) = m.user.as_ref().or_else(|| state.users.get(&uid)).or(state.me.as_ref().filter(|me| me.id == uid)) else { continue };
        if resp.hovered() {
            ui.painter().rect_filled(row, pal.radius, pal.tk.hover);
        }
        let name = state.member_display_name(Some(guild_id), u, Some(m));
        let avatar = Rect::from_center_size(pos2(row.left() + 24.0, row.center().y), Vec2::splat(32.0));
        let url = super::images::user_avatar_url(Some(guild_id), u, Some(m), 64, false);
        super::sidebar::user_avatar_at(ui, images, &url, avatar, false, 1.0, (uid, &name));
        let color = state.role_color(guild_id, &m.roles).map_or(pal.text, |c| cx.theme.readable_on_pane(rgb(c), pal.bg));
        let name_w = (row.width() * 0.4).max(120.0);
        let n = one_line(ui, name.clone(), FontId::new(pal.body(), crate::theme::medium_family()), color, name_w);
        ui.painter().galley(pos2(row.left() + 52.0, row.center().y - n.size().y), n, color);
        let un = one_line(ui, format!("@{}", u.username), FontId::proportional(pal.meta()), pal.muted, name_w);
        ui.painter().galley(pos2(row.left() + 52.0, row.center().y + 1.0), un, pal.muted);
        // Role pills while they fit, then "+N".
        let mut x = row.left() + 60.0 + name_w;
        let right = row.right() - 76.0;
        let pills: Vec<&&GuildRole> = ranked.iter().filter(|r| m.roles.contains(&r.id)).collect();
        for (i, r) in pills.iter().enumerate() {
            let label = one_line(ui, r.name.clone(), FontId::proportional(pal.meta()), pal.text, 120.0);
            let w = label.size().x + 26.0;
            if x + w > right {
                let more = format!("+{}", pills.len() - i);
                ui.painter().text(pos2(x, row.center().y), Align2::LEFT_CENTER, more, FontId::proportional(pal.meta()), pal.muted);
                break;
            }
            let pill = Rect::from_min_size(pos2(x, row.center().y - 11.0), vec2(w, 22.0));
            // A role chip (§9): chip radius, neutral fill, the role's colour dot.
            ui.painter().rect_filled(pill, (pal.radius / 2).max(2), pal.tk.hover);
            ui.painter().circle_filled(pos2(pill.left() + 11.0, pill.center().y), 4.5, role_color(r, &pal));
            ui.painter().galley(pos2(pill.left() + 20.0, pill.center().y - label.size().y / 2.0), label, pal.text);
            x += w + 4.0;
        }
        let buttons = Rect::from_min_max(pos2(row.right() - 72.0, row.top()), row.right_bottom());
        ui.scope_builder(UiBuilder::new().max_rect(buttons).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
            // The full list (`assignable_roles`) is built only while its popup is open.
            let can_roles = manage_roles && (me == Some(uid) || state.outranks(guild_id, uid, &m.roles));
            let plus = ui.add_enabled_ui(can_roles, |ui| vector_button(ui, &pal, Icon::Plus, "Roles", 28.0, false, pal.text)).inner;
            if plus.clicked() || egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&plus)) {
                roles_for = Some((uid, plus));
            }
            let more = server_menu::more_button(ui, &pal);
            if more.clicked() || egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&more)) {
                menu_for = Some((uid, more));
            }
        });
        hairline(ui, &pal);
    }
    // The open popups, drawn with the whole `cx` (their items send).
    if let Some((uid, resp)) = roles_for {
        egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            menu_setup(ui);
            for (id, name, color, has) in server_menu::assignable_roles(cx.state, guild_id, uid) {
                let c = if color == 0 { pal.text } else { crate::theme::readable_on(rgb(color), pal.tk.raised) };
                if menu_item(ui, &name, MenuIcon::Check(has), c) {
                    server_menu::toggle_role(cx, guild_id, uid, id, has);
                }
            }
        });
    }
    if let Some((uid, resp)) = menu_for {
        egui::Popup::menu(&resp).show(|ui| {
            menu_setup(ui);
            if menu_item(ui, "Profile", MenuIcon::Icon(Icon::Person), pal.text) {
                cx.view.open_profile(uid, Some(guild_id), resp.rect.left_bottom());
            }
            server_menu::member_menu_items(ui, cx, guild_id, uid);
            // Settings > Advanced > Developer Mode.
            let dev = cx.state.account.settings.developer_mode();
            if dev {
                ui.separator();
            }
            if dev && menu_item(ui, "Copy user ID", MenuIcon::Icon(Icon::IdCard), pal.text) {
                ui.ctx().copy_text(uid.0.to_string());
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Invites and bans
// ---------------------------------------------------------------------------

/// Time left on an invite ("6d 23h", "45m"), "∞" for one that never expires.
fn expires_in(inv: &crate::model::Invite, now: i64) -> String {
    let parse = |t: &str| chrono::DateTime::parse_from_rfc3339(t).ok().map(|t| t.timestamp());
    let end = inv.expires_at.as_deref().and_then(parse).or_else(|| {
        let age = inv.max_age.filter(|a| *a > 0)?;
        Some(parse(inv.created_at.as_deref()?)? + i64::from(age))
    });
    let Some(end) = end else { return "∞".into() };
    let left = end - now;
    match left {
        ..=0 => "Expired".into(),
        1..3600 => format!("{}m", (left + 59) / 60),
        3600..86_400 => format!("{}h {}m", left / 3600, left % 3600 / 60),
        _ => format!("{}d {}h", left / 86_400, left % 86_400 / 3600),
    }
}

fn invites(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id) {
    // A confirmed revoke answered: refetch (INVITE_DELETE never reaches users).
    match op_state(cx.state, cx.view.server_settings_ui.op).cloned() {
        Some(OpState::Done(_)) => {
            if let Some(op) = cx.view.server_settings_ui.op.take() {
                cx.state.account.ops.remove(&op);
            }
            if let Some(v) = cx.state.account.invites.as_mut() {
                v.loading = true;
                cx.cmd.send(Command::Account(AccountCommand::FetchGuildInvites { guild_id }));
            }
        }
        Some(OpState::Failed(e)) => {
            if let Some(op) = cx.view.server_settings_ui.op.take() {
                cx.state.account.ops.remove(&op);
            }
            cx.view.toast(e, true);
        }
        _ => {}
    }
    let pal = Pal::new(cx.theme);
    let theme = &*cx.theme;
    // Streamer Mode: codes masked (revoking still works).
    let hide = cx.settings.streamer.hides_invites();
    page_title(ui, theme, "Invites");
    let Some(v) = cx.state.account.invites.as_ref() else {
        let why = cx.state.account_check(&AccountCommand::FetchGuildInvites { guild_id }).err();
        hint(ui, theme, why.unwrap_or(""));
        return;
    };
    let images = &mut *cx.images;
    let mut acts = Vec::new();
    let w = ui.available_width();
    let cols = [0.0, w * 0.42, w * 0.66, w * 0.8];
    let (head, _) = ui.allocate_exact_size(vec2(w, 28.0), Sense::hover());
    for (x, label) in cols.iter().zip(["Inviter", "Invite code", "Uses", "Expires"]) {
        ui.painter().text(pos2(head.left() + x, head.center().y), Align2::LEFT_CENTER, label, super::semibold(pal.label()), pal.muted);
    }
    hairline(ui, &pal);
    if v.loading && v.invites.is_empty() {
        ui.add(egui::Spinner::new().size(20.0).color(pal.muted));
    }
    if let Some(e) = &v.error {
        error_text(ui, theme, e);
    }
    if !v.loading && v.error.is_none() && v.invites.is_empty() {
        empty(ui, &pal, "No invites yet. Invite people from the server menu.");
    }
    let now = chrono::Utc::now().timestamp();
    for inv in &v.invites {
        let (row, resp) = ui.allocate_exact_size(vec2(w, 48.0), Sense::hover());
        if !ui.is_rect_visible(row) {
            continue;
        }
        if resp.hovered() {
            ui.painter().rect_filled(row, pal.radius, pal.tk.hover);
        }
        if let Some(u) = &inv.inviter {
            let r = Rect::from_center_size(pos2(row.left() + 16.0, row.center().y), Vec2::splat(24.0));
            let url = super::images::user_avatar_url(None, u, None, 48, false);
            super::sidebar::user_avatar_at(ui, images, &url, r, false, 1.0, (u.id, u.display_name()));
            let n = one_line(ui, u.display_name(), FontId::proportional(pal.size), pal.text, cols[1] - 44.0);
            ui.painter().galley(pos2(row.left() + 36.0, row.center().y - n.size().y / 2.0), n, pal.text);
        }
        let uses = match inv.max_uses.filter(|m| *m > 0) {
            Some(max) => format!("{}/{max}", inv.uses.unwrap_or(0)),
            None => inv.uses.unwrap_or(0).to_string(),
        };
        let shown = if hide { "•".repeat(inv.code.chars().count().min(10)) } else { inv.code.clone() };
        let code = one_line(ui, shown.clone(), FontId::monospace(pal.size - 1.0), pal.text, cols[2] - cols[1] - 8.0);
        ui.painter().galley(pos2(row.left() + cols[1], row.center().y - code.size().y / 2.0), code, pal.text);
        for (x, text) in [(cols[2], uses), (cols[3], expires_in(inv, now))] {
            ui.painter().text(pos2(row.left() + x, row.center().y), Align2::LEFT_CENTER, text, FontId::proportional(pal.size - 1.0), pal.text);
        }
        if resp.hovered() {
            let x = Rect::from_center_size(pos2(row.right() - 16.0, row.center().y), Vec2::splat(24.0));
            if ui.scope_builder(UiBuilder::new().max_rect(x), |ui| vector_button(ui, &pal, Icon::Close, "Revoke invite", 24.0, false, pal.danger)).inner.clicked() {
                let then = AccountCommand::DeleteInvite { code: inv.code.clone() };
                let body = "Nobody can join with it afterwards.".to_owned();
                acts.push(Act::Ask { title: format!("Revoke {shown}?"), body, button: "Revoke invite", then, danger: true, watch: true });
            }
        }
        hairline(ui, &pal);
    }
    apply(cx, guild_id, acts);
}

fn bans(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id) {
    let pal = Pal::new(cx.theme);
    let theme = &*cx.theme;
    page_title(ui, theme, "Bans");
    let Some(v) = cx.state.account.bans.as_ref() else {
        let why = cx.state.account_check(&AccountCommand::FetchBans { guild_id, after: None }).err();
        hint(ui, theme, why.unwrap_or(""));
        return;
    };
    let can_unban = cx.state.account_check(&AccountCommand::Unban { guild_id, user_id: Id(0) }).is_ok();
    let images = &mut *cx.images;
    let s = &mut cx.view.server_settings_ui;
    let mut acts = Vec::new();
    hint(ui, theme, &format!("{}{} bans", v.bans.len(), if v.has_more { "+" } else { "" }));
    search_field(ui, &mut s.filter, "Search bans by user");
    let q = s.filter.trim().to_lowercase();
    ui.add_space(8.0);
    if let Some(e) = &v.error {
        error_text(ui, theme, e);
    }
    if !s.ban_hits.as_ref().is_some_and(|(k, n, _)| *k == q && *n == v.bans.len()) {
        let hits = (v.bans.iter().enumerate()).filter(|(_, b)| {
            let u = &b.user;
            q.is_empty() || u.username.to_lowercase().contains(&q) || u.display_name().to_lowercase().contains(&q) || u.id.to_string() == q
        });
        s.ban_hits = Some((q.clone(), v.bans.len(), hits.map(|(i, _)| i).collect()));
    }
    let hits: &[usize] = s.ban_hits.as_ref().map_or(&[], |h| &h.2);
    // Only the rows in view are laid out (up to 10k bans): fixed pitch = row + spacing + hairline.
    let pitch = 48.0 + ui.spacing().item_spacing.y + 1.0;
    let (top, clip) = (ui.cursor().top(), ui.clip_rect());
    let first = (((clip.top() - top) / pitch).floor().max(0.0) as usize).min(hits.len());
    let last = (((clip.bottom() - top) / pitch).ceil().max(0.0) as usize).clamp(first, hits.len());
    ui.add_space(first as f32 * pitch);
    for b in hits[first..last].iter().filter_map(|&i| v.bans.get(i)) {
        let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::click());
        let hot = resp.hovered() || resp.has_focus();
        if hot {
            ui.painter().rect_filled(row, pal.radius, pal.tk.hover);
        }
        let u = &b.user;
        let r = Rect::from_center_size(pos2(row.left() + 24.0, row.center().y), Vec2::splat(32.0));
        let url = super::images::user_avatar_url(None, u, None, 64, false);
        super::sidebar::user_avatar_at(ui, images, &url, r, false, 1.0, (u.id, u.display_name()));
        let n = one_line(ui, u.display_name(), FontId::new(pal.body(), crate::theme::medium_family()), pal.text, row.width() - 200.0);
        ui.painter().galley(pos2(row.left() + 52.0, row.center().y - n.size().y), n, pal.text);
        let un = one_line(ui, format!("@{}", u.username), FontId::proportional(pal.meta()), pal.muted, row.width() - 200.0);
        ui.painter().galley(pos2(row.left() + 52.0, row.center().y + 1.0), un, pal.muted);
        if hot && can_unban {
            ui.painter().text(pos2(row.right() - 12.0, row.center().y), Align2::RIGHT_CENTER, "Revoke ban", FontId::new(pal.ui(), crate::theme::medium_family()), pal.danger);
        }
        let resp = resp.on_hover_text(b.reason.as_deref().unwrap_or("No reason given"));
        if can_unban && resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            let reason = b.reason.as_deref().unwrap_or("No reason given.");
            let body = format!("They can rejoin with an invite. Ban reason: {reason}");
            let then = AccountCommand::Unban { guild_id, user_id: u.id };
            acts.push(Act::Ask { title: format!("Revoke the ban of @{}?", u.username), body, button: "Revoke ban", then, danger: false, watch: false });
        }
        hairline(ui, &pal);
    }
    ui.add_space((hits.len() - last) as f32 * pitch);
    if v.loading {
        ui.add(egui::Spinner::new().size(20.0).color(pal.muted));
    } else if hits.is_empty() && v.error.is_none() {
        empty(ui, &pal, if q.is_empty() { "Nobody is banned from this server." } else { "No bans match. Try another name or ID." });
    }
    if let Some(after) = v.next_after() {
        ui.add_space(8.0);
        if button(ui, &pal, Btn::Secondary, "Load more", true).clicked() {
            acts.push(Act::MoreBans(after));
        }
    }
    apply(cx, guild_id, acts);
}

// ---------------------------------------------------------------------------
// Edit Channel
// ---------------------------------------------------------------------------

/// Slowmode presets (seconds).
const SLOWMODE: [(u32, &str); 14] = [
    (0, "Off"),
    (5, "5s"),
    (10, "10s"),
    (15, "15s"),
    (30, "30s"),
    (60, "1m"),
    (120, "2m"),
    (300, "5m"),
    (600, "10m"),
    (900, "15m"),
    (1800, "30m"),
    (3600, "1h"),
    (7200, "2h"),
    (21_600, "6h"),
];

/// Where a channel kind's permissions apply (`T` / `V` of `PERMS`); categories both.
fn scope(kind: u8) -> u8 {
    match kind {
        ct::GUILD_VOICE | ct::GUILD_STAGE_VOICE => V,
        ct::GUILD_CATEGORY => T | V,
        _ => T,
    }
}

fn channel_overview(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id, channel_id: Id) {
    settled(cx);
    let pal = Pal::new(cx.theme);
    let theme = &*cx.theme;
    let state = &*cx.state;
    let Some(ch) = state.channels.get(&channel_id) else { return };
    let check = state.account_check(&AccountCommand::UpdateChannel { channel_id, patch: ChannelPatch::default() });
    let can = check.is_ok();
    let category = ch.kind == ct::GUILD_CATEGORY;
    let text_like = matches!(ch.kind, ct::GUILD_TEXT | ct::GUILD_ANNOUNCEMENT | ct::GUILD_FORUM | ct::GUILD_MEDIA);
    let s = &mut cx.view.server_settings_ui;
    page_title(ui, theme, "Overview");
    if let Err(why) = check {
        hint(ui, theme, why);
    }
    let saved_name = ch.name.clone().unwrap_or_default();
    field_label(ui, theme, if category { "Category name" } else { "Channel name" });
    text_field(ui, &mut s.channel.name, &saved_name, 100, 1, can);
    if text_like && let Some(n) = s.channel.name.take() {
        // Shown as Discord stores them: lowercase, dashes.
        set(&mut s.channel.name, saved_name.clone(), n.to_lowercase().replace(' ', "-"));
    }
    if text_like {
        field_label(ui, theme, "Channel topic");
        let limit = if matches!(ch.kind, ct::GUILD_FORUM | ct::GUILD_MEDIA) { 4096 } else { 1024 };
        text_field(ui, &mut s.channel.topic, ch.topic.as_deref().unwrap_or(""), limit, 3, can);
    }
    if !category && ch.kind != ct::GUILD_ANNOUNCEMENT {
        field_label(ui, theme, "Slowmode");
        let mut v = s.channel.slowmode.unwrap_or(ch.rate_limit_per_user);
        if ui.add_enabled_ui(can, |ui| choice(ui, "rc_slowmode", &mut v, &SLOWMODE)).inner {
            set(&mut s.channel.slowmode, ch.rate_limit_per_user, v);
        }
        hint(ui, theme, "How long members wait between messages.");
    }
    if !category {
        ui.add_space(12.0);
        hairline(ui, &pal);
        let nsfw = s.channel.nsfw.unwrap_or(ch.nsfw);
        if let Some(v) = switch_row(ui, &pal, "Age-restricted channel", "Members confirm they are of age before they can view it.", nsfw, can) {
            set(&mut s.channel.nsfw, ch.nsfw, v);
        }
        field_label(ui, theme, "Category");
        let mut cats: Vec<_> = state.channels.values().filter(|c| c.guild_id == Some(guild_id) && c.kind == ct::GUILD_CATEGORY).collect();
        cats.sort_by_key(|c| (c.position, c.id));
        let parent = s.channel.parent_id.unwrap_or(ch.parent_id);
        let name_of = |p: Option<Id>| p.map_or_else(|| "No category".to_owned(), |p| state.channel_name(p));
        ui.add_enabled_ui(can, |ui| {
            egui::ComboBox::from_id_salt("rc_channel_parent").selected_text(name_of(parent)).width(ui.available_width().min(400.0)).icon(super::user_settings::combo_icon).show_ui(ui, |ui| {
                for p in std::iter::once(None).chain(cats.iter().map(|c| Some(c.id))) {
                    if ui.selectable_label(parent == p, name_of(p)).clicked() {
                        set(&mut s.channel.parent_id, ch.parent_id, p);
                    }
                }
            });
        });
    }
    let dirty = s.channel != ChannelPatch::default();
    let name_ok = s.channel.name.as_ref().is_none_or(|n| !n.trim().is_empty());
    let patch = s.channel.clone();
    let mut acts = Vec::new();
    match save_bar(ui, cx, dirty, (!name_ok).then_some("A channel needs a name.")) {
        Some(true) => acts.push(Act::Save(AccountCommand::UpdateChannel { channel_id, patch })),
        Some(false) => reset(cx),
        None => {}
    }
    apply(cx, guild_id, acts);
}

/// Deny / inherit / allow as three joined 28×24 buttons (a 1 px `edge` around them; the picked
/// one filled: `danger_fill` deny, `selected` inherit, `positive_fill` allow; the tooltip says
/// which). Some(new) when clicked.
fn tristate(ui: &mut egui::Ui, pal: &Pal, value: i8) -> Option<i8> {
    let (rect, resp) = ui.allocate_exact_size(vec2(84.0, 24.0), Sense::hover());
    let r = (pal.radius / 2).max(2);
    ui.painter().rect_stroke(rect, r, Stroke::new(1.0, pal.tk.edge), egui::StrokeKind::Inside);
    let mut out = None;
    for (i, (v, color, tip)) in [(-1i8, pal.danger, "Deny"), (0, pal.muted, "Inherit"), (1, pal.tk.positive_text, "Allow")].into_iter().enumerate() {
        let r = Rect::from_min_size(rect.min + vec2(28.0 * i as f32, 0.0), vec2(28.0, 24.0));
        let b = ui.interact(r, resp.id.with(i), Sense::click());
        let on = value == v;
        let fill = match (on, v) {
            (true, 0) => pal.tk.selected,
            (true, -1) => pal.tk.danger_fill,
            (true, _) => pal.tk.positive_fill,
            (false, _) if b.hovered() && ui.is_enabled() => pal.tk.hover,
            _ => Color32::TRANSPARENT,
        };
        ui.painter().rect_filled(r.shrink(1.0), 3, fill);
        let on_label = if v == -1 { pal.tk.danger_label } else { pal.tk.positive_label };
        let fg = if on && v != 0 { on_label } else if ui.is_enabled() { color } else { color.gamma_multiply(0.4) };
        // Painted: the fonts lack a matching ✕.
        let (c, st) = (r.center(), Stroke::new(1.6, fg));
        match v {
            -1 => {
                ui.painter().line_segment([c + vec2(-4.0, -4.0), c + vec2(4.0, 4.0)], st);
                ui.painter().line_segment([c + vec2(-4.0, 4.0), c + vec2(4.0, -4.0)], st);
            }
            0 => {
                ui.painter().line_segment([c + vec2(-3.0, 5.0), c + vec2(3.0, -5.0)], st);
            }
            _ => {
                ui.painter().add(egui::Shape::line(vec![c + vec2(-5.0, 0.0), c + vec2(-1.5, 3.5), c + vec2(5.0, -4.0)], st));
            }
        }
        b.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, ui.is_enabled(), on, tip));
        if b.on_hover_text(tip).clicked() {
            out = Some(v);
        }
    }
    out
}

/// Edit Channel > Permissions: the overwrite targets (@everyone, roles, members; "+" adds a
/// role) and deny / inherit / allow per permission of the selected one.
fn channel_permissions(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id, channel_id: Id) {
    let removing = cx.view.server_settings_ui.op.is_some_and(|op| op.kind == OpKind::DeletePermissionOverwrite);
    if settled(cx).is_some() && removing {
        // Its overwrite is gone: back to @everyone.
        cx.view.server_settings_ui.target = None;
    }
    let pal = Pal::new(cx.theme);
    let theme = &*cx.theme;
    let state = &*cx.state;
    let (Some(ch), Some(g)) = (state.channels.get(&channel_id), state.guild(guild_id)) else { return };
    let manage = state.account_check(&AccountCommand::DeletePermissionOverwrite { channel_id, target_id: guild_id });
    let s = &mut cx.view.server_settings_ui;
    let mut acts = Vec::new();
    page_title(ui, theme, "Channel permissions");
    if let Err(why) = manage {
        hint(ui, theme, why);
    }
    // Targets: @everyone, roles with an overwrite (highest first, plus a newly added one), members.
    let (target, kind) = s.target.unwrap_or((guild_id, 0));
    let mut targets: Vec<(Id, u8, String, Color32)> = vec![(guild_id, 0, "@everyone".into(), pal.muted)];
    for r in sorted_roles(&g.roles) {
        if r.id != guild_id && (ch.permission_overwrites.iter().any(|o| o.id == r.id) || target == r.id) {
            targets.push((r.id, 0, r.name.clone(), role_color(r, &pal)));
        }
    }
    for o in ch.permission_overwrites.iter().filter(|o| o.kind == 1) {
        let user = state.users.get(&o.id).or(state.me.as_ref().filter(|u| u.id == o.id));
        targets.push((o.id, 1, user.map_or_else(|| o.id.to_string(), |u| u.display_name().to_owned()), pal.text));
    }
    let existing = ch.permission_overwrites.iter().find(|o| o.id == target);
    let saved = existing.map_or((0, 0), |o| (o.allow.0, o.deny.0));
    let (allow, deny) = s.overwrite.unwrap_or(saved);
    // Menu labels: role colours readable on the menu (`raised`).
    let label = |r: &GuildRole| if r.color == 0 { pal.text } else { crate::theme::readable_on(rgb(r.color), pal.tk.raised) };
    let addable: Vec<(Id, String, Color32)> =
        sorted_roles(&g.roles).into_iter().filter(|r| r.id != guild_id && !targets.iter().any(|t| t.0 == r.id)).map(|r| (r.id, r.name.clone(), label(r))).collect();
    let list = |ui: &mut egui::Ui, s: &mut ServerSettingsUi| {
        ui.horizontal(|ui| {
            field_label(ui, theme, "Roles and members");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let plus = ui.add_enabled_ui(manage.is_ok() && !addable.is_empty(), |ui| vector_button(ui, &pal, Icon::Plus, "Add a role", 24.0, false, pal.text)).inner;
                egui::Popup::menu(&plus).show(|ui| {
                    menu_setup(ui);
                    for (id, name, color) in &addable {
                        if menu_item(ui, name, MenuIcon::Glyph(""), *color) {
                            (s.target, s.overwrite) = (Some((*id, 0)), None);
                        }
                    }
                });
            });
        });
        for (id, k, name, color) in &targets {
            let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
            let selected = *id == target;
            let fill = if selected { pal.tk.selected } else if resp.hovered() { pal.tk.hover } else { Color32::TRANSPARENT };
            ui.painter().rect_filled(row, pal.radius, fill);
            ui.painter().circle_filled(pos2(row.left() + 14.0, row.center().y), 5.0, *color);
            let label = one_line(ui, name.clone(), FontId::new(pal.ui(), crate::theme::medium_family()), pal.text, row.width() - 34.0);
            ui.painter().galley(pos2(row.left() + 26.0, row.center().y - label.size().y / 2.0), label, pal.text);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !selected {
                (s.target, s.overwrite) = (Some((*id, *k)), None);
            }
        }
    };
    let mut rows = |ui: &mut egui::Ui, s: &mut ServerSettingsUi| {
        if target != guild_id && existing.is_some() {
            let remove = button(ui, &pal, Btn::Ghost, "Remove overrides", manage.is_ok());
            if remove.on_hover_text("Remove this role's or member's overrides").clicked() {
                acts.push(Act::Save(AccountCommand::DeletePermissionOverwrite { channel_id, target_id: target }));
            }
        }
        for (title, perm_rows) in PERMS {
            let shown: Vec<&PermRow> = perm_rows.iter().filter(|r| r.3 & scope(ch.kind) != 0).collect();
            if shown.is_empty() {
                continue;
            }
            section(ui, theme, &title.replace("server", "channel"));
            for &&(bit, name, tip, _) in &shown {
                let b = 1u64 << bit;
                let label = match bit {
                    10 => "View Channel",
                    4 => "Manage Channel",
                    28 => "Manage Permissions",
                    _ => name,
                };
                let value = if allow & b != 0 { 1 } else if deny & b != 0 { -1 } else { 0 };
                // The save's own gate: may this bit change from the saved overwrite?
                let flip = Overwrite { id: target, kind, allow: Perms(saved.0 ^ b), deny: Perms(saved.1 & !b) };
                let can = state.account_check(&AccountCommand::SetPermissionOverwrite { channel_id, overwrite: flip }).is_ok();
                let (_, picked) = egui::Sides::new().height(36.0).shrink_left().show(
                    ui,
                    |ui| ui.add(egui::Label::new(RichText::new(label).color(pal.text)).wrap()).on_hover_text(tip),
                    |ui| ui.add_enabled_ui(can, |ui| tristate(ui, &pal, value)).inner,
                );
                hairline(ui, &pal);
                if let Some(v) = picked {
                    let (a, d) = match v {
                        1 => (allow | b, deny & !b),
                        -1 => (allow & !b, deny | b),
                        _ => (allow & !b, deny & !b),
                    };
                    s.overwrite = ((a, d) != saved).then_some((a, d));
                }
            }
        }
    };
    if ui.available_width() >= 560.0 {
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(vec2(190.0, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| list(ui, s));
            ui.add_space(16.0);
            ui.vertical(|ui| rows(ui, s));
        });
    } else {
        list(ui, s);
        rows(ui, s);
    }
    let dirty = s.overwrite.is_some();
    let overwrite = Overwrite { id: target, kind, allow: Perms(allow), deny: Perms(deny) };
    let cmd = AccountCommand::SetPermissionOverwrite { channel_id, overwrite };
    let why_not = state.account_check(&cmd).err();
    match save_bar(ui, cx, dirty, why_not) {
        Some(true) => acts.push(Act::Save(cmd)),
        Some(false) => reset(cx),
        None => {}
    }
    apply(cx, guild_id, acts);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Invite;

    #[test]
    fn page_names() {
        assert_eq!(Page::from_name("roles"), Some(Page::Roles));
        assert_eq!(Page::from_name("Bans"), Some(Page::Bans));
        assert_eq!(Page::from_name("role-42"), Some(Page::Role(Id(42))));
        assert_eq!(Page::from_name("channel-7"), Some(Page::Channel(Id(7))));
        assert_eq!(Page::from_name(""), None);
        assert_eq!(Page::from_name("role-x"), None);
    }

    #[test]
    fn reorder_moves_only_the_range() {
        let (a, b, c, d) = (Id(1), Id(2), Id(3), Id(4));
        let order = [(a, 9), (b, 5), (c, 4), (d, 1)];
        // b to the top: a and b swap positions, d keeps its own.
        assert_eq!(reorder(&order, 1, 0), vec![(b, 9), (a, 5)]);
        // a below c (slot 2 once a is out of the list).
        assert_eq!(reorder(&order, 0, 2), vec![(b, 9), (c, 5), (a, 4)]);
        assert_eq!(reorder(&order, 2, 2), vec![], "dropped where it was");
        assert_eq!(reorder(&order, 9, 0), vec![]);
    }

    #[test]
    fn emoji_names_from_files() {
        assert_eq!(emoji_name("party-parrot"), "party_parrot");
        assert_eq!(emoji_name("x"), "emoji_x");
        assert!(valid_emoji_name(&emoji_name(&"a".repeat(40))));
        assert!(!valid_emoji_name("a"));
        assert!(!valid_emoji_name("no spaces"));
    }

    #[test]
    fn invite_expiry() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-12T10:00:00+00:00").unwrap().timestamp();
        let inv = |expires: Option<&str>, max_age, created: Option<&str>| Invite {
            expires_at: expires.map(Into::into),
            max_age,
            created_at: created.map(Into::into),
            ..Default::default()
        };
        assert_eq!(expires_in(&inv(None, Some(0), None), now), "∞");
        assert_eq!(expires_in(&inv(Some("2026-11-19T09:00:00+00:00"), None, None), now), "6d 23h");
        assert_eq!(expires_in(&inv(None, Some(1800), Some("2026-11-12T09:50:00+00:00")), now), "20m");
        assert_eq!(expires_in(&inv(Some("2026-11-12T09:00:00+00:00"), None, None), now), "Expired");
    }
}
