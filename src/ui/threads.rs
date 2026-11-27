//! Threads UI (FEATURES_NOTES §2.1, layout §4.2 / §4.9):
//! - `sidebar_rows`: threads under their parent in the channel list (from
//!   `AppState::sidebar_threads`, called by sidebar.rs under every channel that has some):
//!   indented rows on a curved spine from the parent's glyph, own unread pill / mention badge,
//!   context menu (`thread_menu`: Mark As Read, Open in Split View, Mute ▸ / Notification
//!   Settings ▸ (disabled until the backend has the thread-member call), Join / Leave, Archive).
//! - `header_button`: the chat header's Threads button and its popout: Active (the known ones from
//!   `active_threads` plus `ThreadList::Active` pages) and Archived (public, or private with
//!   MANAGE_THREADS / the private ones I joined), Join / Leave per row, Create; a row opens the
//!   thread in the panel. The list view is dropped when the popout closes.
//! - `panel`: one thread docked right of its parent's chat (`ViewState::thread_panel`; the shell
//!   loads its history, acks it and closes it with its parent): header (name, Join, the bell with
//!   the notification items, "…" = `thread_menu`, ×), the archived bar, then
//!   `messages::thread_chat` (its own message list and composer).
//! - `create_modal`: Create Thread (`Modal::CreateThread`, from a message's menu or the popout):
//!   the starter message, name (prefilled from it), private toggle (text channels with
//!   CREATE_PRIVATE_THREADS, not from a message), archive duration (the parent's
//!   `default_auto_archive_duration`, else 3 days) -> `CreateThread`.
//! - `track` (per frame, top of the main layout): a thread or forum post created here opens as
//!   the selected channel (`ChannelOpened`); it is docked in the panel beside its parent instead,
//!   like the web client. Also drops the thread list once its channel is closed.
//! - Chat header bits for threads: `breadcrumb` (parent › thread) and `header_glyph`.
//!
//! Hook-up guide: docs/SOCIAL_DESIGN.md. Gates: `can_create_thread`, `can_join_thread`,
//! `can_set_archived`, `can_list_archived`.
//! OWNERSHIP: threads/forum engineer (docs/SOCIAL_UI_DESIGN.md).

use super::icons::Icon;
use super::messages::{MenuIcon, Pal, icon_button, icon_label, menu_item, menu_setup, small_button, vector_button, vector_icon};
use super::{Modal, UiCx, bold, dialog, dialog_buttons, one_line};
use crate::backend::{ArchivedThreads, Command, SocialCommand, ThreadList};
use crate::model::{Channel, Id, channel_type};
use crate::state::AppState;
use crate::state::social::ThreadsView;
use crate::theme::bold_family;
use egui::epaint::QuadraticBezierShape;
use egui::{Align, Color32, CornerRadius, CursorIcon, FontId, Frame, Label, Layout, Margin, Rect, RichText, ScrollArea, Sense, Stroke, TextEdit, pos2, vec2};
use std::time::{SystemTime, UNIX_EPOCH};

/// Sidebar row height (channel rows' 32 px).
const ROW_H: f32 = 32.0;
/// x of the spine from the channel list's left edge: under the parent's 20 px glyph.
const SPINE_X: f32 = 26.0;
/// Rows of the Threads popout.
const LIST_ROW_H: f32 = 52.0;
/// Auto-archive choices (minutes).
pub(super) const ARCHIVE_AFTER: [(u32, &str); 4] = [(60, "1 Hour"), (1440, "24 Hours"), (4320, "3 Days"), (10080, "1 Week")];
/// A created thread / post not opened by then failed (its error was toasted).
const PENDING_MS: u64 = 30_000;

/// Threads UI drafts (in `ViewState::threads_ui`).
#[derive(Debug, Default)]
pub struct ThreadsUi {
    /// Create Thread: the name being typed, private, auto-archive minutes.
    pub name: String,
    pub private: bool,
    pub archive_minutes: u32,
    /// Threads popout: the Archived tab and its list (None = Active).
    archived: Option<ArchivedThreads>,
    /// A thread / forum post just asked for: (parent, starter message, sent at unix ms). Once it
    /// opens, `track` docks it in the panel beside its parent.
    pub(super) pending: Option<(Id, Option<Id>, u64)>,
}

/// Popup id of the Threads popout (`RUSTCORD_OPEN=threads` opens it).
pub fn popup_id() -> egui::Id {
    egui::Id::unique("rc_threads_popout")
}

/// Channel kinds whose header has a Threads button.
pub fn has_threads(kind: u8) -> bool {
    matches!(kind, channel_type::GUILD_TEXT | channel_type::GUILD_ANNOUNCEMENT)
}

/// Chat header of a thread / forum post opened as the channel: "parent ›" before its name; the
/// parent's name opens the parent.
pub(super) fn breadcrumb(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, parent: Id) {
    let name = cx.state.channel_name(parent);
    let text = RichText::new(name).family(bold_family()).size(pal.size + 1.0).color(pal.muted);
    let resp = ui.add(Label::new(text).selectable(false).truncate().sense(Sense::click())).on_hover_cursor(CursorIcon::PointingHand);
    if resp.clicked() {
        cx.state.select_channel(parent);
    }
    vector_icon(ui, Icon::ChevronRight, 16.0, pal.muted, pal.bg);
}

// ---------------------------------------------------------------------------
// Sidebar rows
// ---------------------------------------------------------------------------

/// Thread rows under `parent` (latest activity first): a spine from under the parent's glyph
/// curving into each row, the name (bright and bold when unread, dim when muted), unread pill and
/// mention badge. A click opens the thread as the channel.
pub fn sidebar_rows(ui: &mut egui::Ui, cx: &mut UiCx<'_>, parent: Id, threads: &[Id]) {
    let c = cx.theme.colors.clone();
    let (selected_fill, hover_fill) = super::sidebar::row_fills(ui, &c);
    let spine = Stroke::new(1.5, c.text_muted.to_color32().gamma_multiply(0.35));
    let gap = ui.spacing().item_spacing.y;
    let size = cx.theme.font.size;
    for (i, &t) in threads.iter().enumerate() {
        let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
        let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
        // From under the parent's glyph (first row) or the previous row; on past the curve
        // only while more rows follow.
        let top = row.top() - gap - if i == 0 { 6.0 } else { 0.0 };
        let selected = cx.state.selected_channel == Some(t) || cx.view.thread_panel == Some(t);
        let name = cx.state.channel_name(t);
        // Out of view (the channel list isn't virtualized) the row keeps its widget but skips
        // the paint, like `sidebar::channel_row`; the rect includes the spine above the row.
        if ui.is_rect_visible(Rect::from_min_max(pos2(row.left(), top), row.max).expand(4.0)) || super::sidebar::folding(ui) {
            let x = row.left() + SPINE_X;
            let y = row.center().y;
            let p = ui.painter();
            let last = i + 1 == threads.len();
            p.line_segment([pos2(x, top), pos2(x, if last { y - 8.0 } else { row.bottom() })], spine);
            p.add(QuadraticBezierShape::from_points_stroke([pos2(x, y - 8.0), pos2(x, y), pos2(x + 8.0, y)], false, Color32::TRANSPARENT, spine));
            let body = Rect::from_min_max(pos2(x + 10.0, row.top()), pos2(row.right() - 8.0, row.bottom()));
            let muted = cx.state.is_channel_muted(t);
            let unread = !selected && !muted && cx.state.is_unread(t);
            let mentions = cx.state.mention_count(t);
            if selected || resp.hovered() {
                ui.painter().rect_filled(body, cx.theme.layout.corner_radius, if selected { selected_fill } else { hover_fill });
            }
            if unread {
                let pill = Rect::from_min_size(pos2(row.left(), y - 4.0), vec2(4.0, 8.0));
                ui.painter().rect_filled(pill, CornerRadius { nw: 0, sw: 0, ne: 4, se: 4 }, c.text.to_color32());
            }
            let mut right = body.right() - 8.0;
            if mentions > 0 {
                right -= super::sidebar::badge(ui, &c, pos2(right, y), mentions, None) + 6.0;
            }
            let color = if muted && !selected {
                c.text_muted.to_color32().gamma_multiply(0.55)
            } else if selected || unread || resp.hovered() {
                c.text.to_color32()
            } else {
                c.text_muted.to_color32()
            };
            let font = if unread { bold(size) } else { FontId::proportional(size) };
            let g = one_line(ui, name.as_str(), font, color, right - body.left() - 8.0);
            ui.painter().galley(pos2(body.left() + 8.0, y - g.size().y / 2.0), g, color);
        }
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, &name));
        resp.context_menu(|ui| thread_menu(ui, cx, t));
        if resp.clicked() {
            cx.state.select_channel(t);
            cx.view.settings_open = false;
        }
    }
    let _ = parent;
}

/// A thread's menu (sidebar row, Threads popout row, forum card, the panel's "…"): Mark As Read,
/// Open in Split View, Mute ▸ / Notification Settings ▸ (joined), Join / Leave, Archive /
/// Unarchive (when allowed), Copy Link, Copy Thread ID.
pub(super) fn thread_menu(ui: &mut egui::Ui, cx: &mut UiCx<'_>, t: Id) {
    menu_setup(ui);
    let pal = Pal::new(cx.theme);
    if ui.add_enabled_ui(cx.state.is_unread(t), |ui| menu_item(ui, "Mark as read", MenuIcon::Glyph("✔"), pal.text)).inner {
        super::sidebar::mark_read(cx, [t]);
    }
    if cx.view.thread_panel != Some(t) && menu_item(ui, "Open in split view", MenuIcon::Glyph("⊞"), pal.text) {
        open_in_panel(cx, t);
    }
    let Some(ch) = known(cx.state, t) else { return };
    let (joined, can_join, archived) = (ch.member.is_some(), cx.state.can_join_thread(ch), ch.is_archived());
    let can_archive = cx.state.can_set_archived(ch, !archived);
    let guild = ch.guild_id;
    if joined {
        ui.separator();
        notify_items(ui, cx, t);
    }
    ui.separator();
    if can_join && menu_item(ui, "Join thread", MenuIcon::Glyph("➕"), pal.text) {
        cx.cmd.send(Command::Social(SocialCommand::JoinThread { thread_id: t }));
    }
    if joined && !archived && menu_item(ui, "Leave thread", MenuIcon::Glyph("🚪"), pal.danger) {
        cx.cmd.send(Command::Social(SocialCommand::LeaveThread { thread_id: t }));
    }
    let label = if archived { "Unarchive thread" } else { "Archive thread" };
    if can_archive && menu_item(ui, label, MenuIcon::Glyph("🗄"), pal.text) {
        cx.cmd.send(Command::Social(SocialCommand::SetThreadArchived { thread_id: t, archived: !archived }));
    }
    ui.separator();
    if menu_item(ui, "Copy link", MenuIcon::Icon(Icon::Link), pal.text) {
        let place = guild.map_or_else(|| "@me".to_owned(), |g| g.0.to_string());
        ui.ctx().copy_text(format!("https://discord.com/channels/{place}/{}", t.0));
    }
    if cx.state.account.settings.developer_mode() && menu_item(ui, "Copy thread ID", MenuIcon::Icon(Icon::IdCard), pal.text) {
        ui.ctx().copy_text(t.0.to_string());
    }
}

/// Mute Thread ▸ / Unmute Thread and Notification Settings ▸ of a joined thread (thread menu, the
/// panel's bell), where the channel menu has them.
// shortcut: shown disabled; writing needs a `PATCH /channels/{t}/thread-members/@me/settings`
// command in backend.rs (FEATURES_NOTES §2.1, flags 2 / 4 / 8, muted, mute_config): add it,
// then give these rows the channel menu's durations / levels and send it from here.
fn notify_items(ui: &mut egui::Ui, cx: &UiCx<'_>, t: Id) {
    const WHY: &str = "Not available yet";
    let text = cx.theme.colors.text.to_color32();
    let mute = if cx.state.is_channel_muted(t) { "Unmute thread" } else { "Mute thread" };
    ui.add_enabled_ui(false, |ui| {
        ui.menu_button(RichText::new(mute).color(text), |_| {}).response.on_disabled_hover_text(WHY);
        ui.menu_button(RichText::new("Notifications").color(text), |_| {}).response.on_disabled_hover_text(WHY);
    });
}

/// A thread from the channels (follows the gateway) or, when only listed, the open list view.
pub(super) fn known(state: &AppState, id: Id) -> Option<&Channel> {
    state.channels.get(&id).or_else(|| state.social.threads.as_ref()?.threads.iter().find(|t| t.id == id))
}

/// Opens a thread in the panel beside its parent (selecting the parent). One only known from a
/// list view is added to the channels first.
pub(super) fn open_in_panel(cx: &mut UiCx<'_>, id: Id) {
    if !cx.state.channels.contains_key(&id) {
        let Some(t) = cx.state.social.threads.as_ref().and_then(|v| v.threads.iter().find(|t| t.id == id)).cloned() else { return };
        // shortcut: inserted directly (`open_thread` would select it), so the unjoined-thread cap
        // runs only at the next `open_thread` / thread event; fine while one click adds one.
        cx.state.channels.insert(id, t);
    }
    let Some(parent) = cx.state.channels.get(&id).and_then(|c| c.parent_id) else { return };
    if cx.state.selected_channel != Some(parent) {
        cx.state.select_channel(parent);
    }
    cx.view.settings_open = false;
    // The thread it replaces keeps no history (select_channel's retain keeps at most this one).
    if let Some(old) = cx.view.thread_panel.filter(|o| *o != id && !cx.state.recent_channels.contains(o)) {
        cx.state.messages.remove(&old);
        cx.state.fresh_channels.remove(&old);
    }
    cx.view.thread_panel = Some(id);
}

// ---------------------------------------------------------------------------
// Threads popout
// ---------------------------------------------------------------------------

/// The header's Threads button and its popout under it.
pub(super) fn header_button(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, ch: Id) {
    let ctx = ui.ctx().clone();
    let id = popup_id();
    let open = egui::Popup::is_id_open(&ctx, id);
    let resp = icon_button(ui, pal, "🧵", "Threads", 32.0, open, pal.text);
    if !open && !resp.clicked() && cx.state.social.threads.as_ref().is_some_and(|v| !matches!(v.list, ThreadList::Forum(_))) {
        // Closed: its list goes (memory).
        cx.state.social.threads = None;
    }
    let frame = Frame::popup(ui.style()).fill(pal.surface).corner_radius(pal.card_radius()).inner_margin(Margin::ZERO);
    egui::Popup::new(id, ctx, &resp, ui.layer_id())
        .open_memory(resp.clicked().then_some(egui::SetOpenCommand::Toggle))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .align(egui::RectAlign::BOTTOM_END)
        .gap(8.0)
        .frame(frame)
        .show(|ui| popout(ui, cx, pal, ch));
}

/// The list the popout shows for its tab.
fn wanted_list(tab: Option<ArchivedThreads>) -> ThreadList {
    match tab {
        None => ThreadList::Active { offset: 0 },
        Some(kind) => ThreadList::Archived { kind, before: None },
    }
}

/// Whether `v` lists `want` (any page) for `ch`.
fn lists(v: &ThreadsView, ch: Id, want: &ThreadList) -> bool {
    v.channel_id == ch
        && match (&v.list, want) {
            (ThreadList::Active { .. }, ThreadList::Active { .. }) => true,
            (ThreadList::Archived { kind: a, .. }, ThreadList::Archived { kind: b, .. }) => a == b,
            _ => false,
        }
}

/// Opens `list` as the thread list view and asks for its first page.
pub(super) fn load(cx: &mut UiCx<'_>, ch: Id, list: ThreadList) {
    cx.state.social.threads = Some(ThreadsView::new(ch, list.clone()));
    cx.cmd.send(Command::Social(SocialCommand::FetchThreads { channel_id: ch, list }));
}

fn popout(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, ch: Id) {
    let screen = ui.ctx().content_rect();
    ui.set_width(480.0f32.min(screen.width() - 32.0));
    let tab = cx.view.threads_ui.archived;
    let want = wanted_list(tab);
    let allowed = match tab {
        None => true,
        Some(kind) => cx.state.can_list_archived(ch, kind),
    };
    if allowed && !cx.state.social.threads.as_ref().is_some_and(|v| lists(v, ch, &want)) {
        load(cx, ch, want.clone());
    }
    // Header: title, tabs, Create.
    Frame::new().inner_margin(Margin { left: 16, right: 12, top: 12, bottom: 8 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            vector_icon(ui, Icon::Thread, 20.0, pal.muted, pal.surface);
            ui.add(Label::new(RichText::new("Threads").family(bold_family()).size(pal.size + 1.0).color(pal.text)).selectable(false));
            ui.add_space(12.0);
            let private = if cx.state.can_manage_threads(ch) { ArchivedThreads::Private } else { ArchivedThreads::Joined };
            for (t, label) in [(None, "Active"), (Some(ArchivedThreads::Public), "Archived")] {
                let on = tab.is_some() == t.is_some();
                if tab_chip(ui, pal, label, on).clicked() && !on {
                    cx.view.threads_ui.archived = t;
                }
            }
            if tab.is_some() && cx.state.channels.get(&ch).is_some_and(|c| c.kind == channel_type::GUILD_TEXT) {
                // Public / private archive, one toggle.
                let on = tab != Some(ArchivedThreads::Public);
                let tip = if on { "Showing private threads" } else { "Show private threads" };
                if icon_button(ui, pal, "🔒", tip, 28.0, on, pal.text).clicked() {
                    cx.view.threads_ui.archived = Some(if on { ArchivedThreads::Public } else { private });
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if cx.state.can_create_thread(ch, false) && super::messages::button(ui, pal, super::messages::Btn::Primary, "Create thread", true).clicked() {
                    open_create(cx, ch, None);
                    ui.close();
                }
            });
        });
    });
    ui.painter().hline(ui.min_rect().x_range(), ui.min_rect().bottom(), Stroke::new(1.0, pal.tk.rule));

    // Rows: Active = the known ones first, then those only the search found.
    let mut ids: Vec<Id> = if tab.is_none() { cx.state.active_threads(ch) } else { Vec::new() };
    if let Some(v) = cx.state.social.threads.as_ref().filter(|v| lists(v, ch, &want)) {
        let extra: Vec<Id> = v.threads.iter().map(|t| t.id).filter(|id| !ids.contains(id)).collect();
        ids.extend(extra);
    }
    let (loading, error) = cx.state.social.threads.as_ref().filter(|v| lists(v, ch, &want)).map_or((false, None), |v| (v.loading, v.error.clone()));
    let max_h = (screen.height() * 0.6).clamp(160.0, 480.0);
    Frame::new().inner_margin(Margin::symmetric(8, 8)).show(ui, |ui| {
        if !allowed {
            empty(ui, pal, "You can't see archived threads here.");
            return;
        }
        if ids.is_empty() {
            let failed = error.is_some() && !loading;
            let text = if loading { "Loading…".to_owned() } else { error.unwrap_or_else(|| if tab.is_none() { "No active threads." } else { "No archived threads." }.into()) };
            empty(ui, pal, &text);
            if failed && ui.vertical_centered(|ui| small_button(ui, pal, "Retry", pal.text).clicked()).inner {
                retry(cx.state);
            }
            return;
        }
        let mut open = None;
        let flat = cx.theme.gradient.is_none();
        if flat {
            ui.spacing_mut().scroll.fade.strength = 0.0;
        }
        let out = ScrollArea::vertical().id_salt(("rc_threads_list", ch.0)).max_height(max_h).auto_shrink([false, true]).show_rows(ui, LIST_ROW_H, ids.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let at_end = range.end >= ids.len();
            for &t in &ids[range] {
                if list_row(ui, cx, pal, t) {
                    open = Some(t);
                }
            }
            // Infinite scroll: the next page once the last row shows (not after an error: Retry).
            if at_end && let Some(next) = cx.state.social.threads.as_ref().filter(|v| lists(v, ch, &want) && v.error.is_none()).and_then(ThreadsView::next_page) {
                if let Some(v) = cx.state.social.threads.as_mut() {
                    v.loading = true;
                }
                cx.cmd.send(Command::Social(SocialCommand::FetchThreads { channel_id: ch, list: next }));
            }
        });
        if flat {
            super::scroll_fades(ui, &out, out.inner_rect, (out.inner_rect.left(), out.inner_rect.right()), (pal.surface, pal.surface));
        }
        if let Some(e) = error.filter(|_| !loading) {
            ui.horizontal(|ui| {
                ui.label(RichText::new(e).size(pal.meta()).color(pal.danger));
                if small_button(ui, pal, "Retry", pal.text).clicked() {
                    retry(cx.state);
                }
            });
        }
        if let Some(t) = open {
            open_in_panel(cx, t);
            ui.close();
        }
    });
}

/// Retry after a failed page: the next page again (paging stops while the view holds an error),
/// or the whole list when its first page failed.
pub(super) fn retry(state: &mut AppState) {
    if let Some(v) = state.social.threads.as_mut() {
        if v.has_more {
            v.error = None;
        } else {
            state.social.threads = None;
        }
    }
}

fn empty(ui: &mut egui::Ui, pal: &Pal, text: &str) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| ui.add(Label::new(RichText::new(text).size(pal.meta()).color(pal.muted)).wrap()));
    ui.add_space(24.0);
}

/// Tab-like chip (popout tabs).
fn tab_chip(ui: &mut egui::Ui, pal: &Pal, text: &str, on: bool) -> egui::Response {
    let g = ui.painter().layout_no_wrap(text.to_owned(), FontId::new(pal.size - 1.0, bold_family()), pal.text);
    let (rect, resp) = ui.allocate_exact_size(g.size() + vec2(20.0, 10.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, on, text));
    let hot = resp.hovered() || resp.has_focus();
    if on || hot {
        ui.painter().rect_filled(rect, pal.radius, if on { pal.text.gamma_multiply(0.12) } else { pal.tint() });
    }
    ui.painter().galley(rect.center() - g.size() / 2.0, g, if on || hot { pal.text } else { pal.muted });
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// One popout row: name, "N messages · 3h ago", Join / Leave on the right. True when the row
/// (not its button) was clicked.
fn list_row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, t: Id) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), LIST_ROW_H), Sense::click());
    let Some(ch) = known(cx.state, t) else { return false };
    let (joined, can_join, archived) = (ch.member.is_some(), cx.state.can_join_thread(ch), ch.is_archived());
    let name = ch.name.clone().unwrap_or_default();
    let count = ch.message_count.unwrap_or(0);
    let last = ch.last_message_id.map_or(ch.id, |m| m.max(ch.id));
    let mut meta = format!("{count} message{}, active {}", if count == 1 { "" } else { "s" }, ago(last.timestamp_ms()));
    if archived {
        meta.push_str(", archived");
    }
    let hot = resp.hovered();
    if hot {
        ui.painter().rect_filled(rect.shrink2(vec2(0.0, 2.0)), pal.radius, pal.tint());
    }
    // Join / Leave on the right.
    let mut right = rect.right() - 8.0;
    let mut button_hit = false;
    let action = if can_join { Some(("Join", pal.text)) } else { (joined && !archived && hot).then_some(("Leave", pal.danger)) };
    if let Some((label, color)) = action {
        let w = ui.painter().layout_no_wrap(label.into(), FontId::proportional(pal.meta()), color).size().x + 16.0;
        let r = Rect::from_center_size(pos2(right - w / 2.0, rect.center().y), vec2(w, 24.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
        if small_button(&mut child, pal, label, color).clicked() {
            button_hit = true;
            let cmd = if label == "Join" { SocialCommand::JoinThread { thread_id: t } } else { SocialCommand::LeaveThread { thread_id: t } };
            cx.cmd.send(Command::Social(cmd));
        }
        right -= w + 8.0;
    }
    let x = rect.left() + 12.0;
    let w = right - x;
    let title = one_line(ui, name.as_str(), bold(pal.size), pal.text, w);
    let sub = one_line(ui, meta, FontId::proportional(pal.meta()), pal.muted, w);
    let top = rect.center().y - (title.size().y + sub.size().y + 2.0) / 2.0;
    ui.painter().galley(pos2(x, top + title.size().y + 2.0), sub, pal.muted);
    ui.painter().galley(pos2(x, top), title, pal.text);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
    resp.context_menu(|ui| thread_menu(ui, cx, t));
    resp.on_hover_cursor(CursorIcon::PointingHand).clicked() && !button_hit
}

// ---------------------------------------------------------------------------
// Thread panel
// ---------------------------------------------------------------------------

/// The thread split panel: header (glyph, name; Join, bell, "…" thread menu, ×), an "archived"
/// bar, then the thread's message list and composer.
pub fn panel(ui: &mut egui::Ui, cx: &mut UiCx<'_>, thread: Id) {
    let pal = Pal::new(cx.theme);
    let name = cx.state.channel_name(thread);
    let info = cx.state.channels.get(&thread).map(|t| {
        let archived = t.is_archived();
        (t.member.is_some(), cx.state.can_join_thread(t), archived, cx.state.can_set_archived(t, !archived))
    });
    let (joined, can_join, archived, can_archive) = info.unwrap_or_default();
    let header = egui::Panel::top("rc_thread_panel_header")
        .exact_size(49.0)
        .resizable(false)
        .show_separator_line(false)
        .frame(Frame::new().inner_margin(Margin { left: 16, right: 12, top: 0, bottom: 0 }))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                // Buttons from the right, then glyph and name in the space left of them.
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if icon_button(ui, &pal, "×", "Close", 32.0, false, pal.text).clicked() {
                        cx.view.thread_panel = None;
                    }
                    // "…": the thread menu (Leave, Archive, Copy Link …); the bell: its notifications.
                    let more = super::server_menu::more_button(ui, &pal);
                    egui::Popup::menu(&more).show(|ui| thread_menu(ui, cx, thread));
                    if joined {
                        let bell = if cx.state.is_channel_muted(thread) { Icon::BellOff } else { Icon::Bell };
                        let resp = vector_button(ui, &pal, bell, "Notifications", 32.0, false, pal.text);
                        egui::Popup::menu(&resp).show(|ui| {
                            menu_setup(ui);
                            notify_items(ui, cx, thread);
                        });
                    }
                    if can_join && super::messages::button(ui, &pal, super::messages::Btn::Primary, "Join thread", true).clicked() {
                        cx.cmd.send(Command::Social(SocialCommand::JoinThread { thread_id: thread }));
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        vector_icon(ui, Icon::Thread, 20.0, pal.muted, pal.bg);
                        let title = RichText::new(name.as_str()).family(bold_family()).size(pal.size + 1.0).color(pal.text);
                        ui.add(Label::new(title).selectable(false).truncate());
                    });
                });
            });
        });
    let r = header.response.rect;
    ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, pal.tk.rule));
    if archived {
        let bar = Frame::new().fill(pal.surface).inner_margin(Margin::symmetric(16, 8)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_width(ui.available_width());
                icon_label(ui, Icon::Archive, "This thread is archived", pal.meta(), pal.muted, pal.surface);
                if can_archive {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if small_button(ui, &pal, "Unarchive", pal.text).clicked() {
                            cx.cmd.send(Command::Social(SocialCommand::SetThreadArchived { thread_id: thread, archived: false }));
                        }
                    });
                }
            });
        });
        ui.painter().hline(bar.response.rect.x_range(), bar.response.rect.bottom() - 0.5, Stroke::new(1.0, pal.tk.rule));
    }
    super::messages::thread_chat(ui, cx, thread);
}

// ---------------------------------------------------------------------------
// Create Thread
// ---------------------------------------------------------------------------

/// Opens Create Thread for `channel_id` (from a message's menu: `from_message`, whose text
/// prefills the name).
pub fn open_create(cx: &mut UiCx<'_>, channel_id: Id, from_message: Option<Id>) {
    let starter = from_message.and_then(|m| cx.state.messages.get(&channel_id)?.msgs.iter().find(|x| x.id == m));
    let name: String = starter.and_then(|m| m.content.lines().next()).unwrap_or_default().trim().chars().take(40).collect();
    let archive_minutes = cx.state.channels.get(&channel_id).and_then(|c| c.default_auto_archive_duration).unwrap_or(4320);
    cx.view.threads_ui = ThreadsUi { name, archive_minutes, pending: cx.view.threads_ui.pending, ..ThreadsUi::default() };
    cx.view.modal = Some(Modal::CreateThread { channel_id, from_message });
}

/// `Modal::CreateThread`: starter message, name, private toggle where allowed, auto-archive.
pub fn create_modal(ctx: &egui::Context, cx: &mut UiCx<'_>, channel_id: Id, from_message: Option<Id>) {
    let pal = Pal::new(cx.theme);
    let kind = cx.state.channels.get(&channel_id).map_or(0, |c| c.kind);
    let starter = from_message
        .and_then(|m| cx.state.messages.get(&channel_id)?.msgs.iter().find(|x| x.id == m))
        .map(|m| (m.author.display_name().to_owned(), m.content.clone()));
    // Private threads: text channels only, not from a message.
    let private_ok = from_message.is_none() && kind == channel_type::GUILD_TEXT && cx.state.can_create_thread(channel_id, true);
    let default_archive = cx.state.channels.get(&channel_id).and_then(|c| c.default_auto_archive_duration).unwrap_or(4320);
    let st = &mut cx.view.threads_ui;
    st.private &= private_ok;
    st.archive_minutes = valid_archive(st.archive_minutes, default_archive);
    let allowed = cx.state.can_create_thread(channel_id, st.private);
    let valid = (1..=100).contains(&st.name.trim().chars().count());
    let theme = &*cx.theme;
    let resp = dialog(ctx, theme, "rc_create_thread", 442.0, "Create thread", |ui| {
        if let Some((author, text)) = &starter {
            Frame::new().fill(pal.bg).corner_radius(pal.card_radius()).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(Label::new(RichText::new(author).family(bold_family()).size(pal.meta()).color(pal.text)).selectable(false));
                let preview = text.lines().take(3).collect::<Vec<_>>().join("\n");
                ui.add(Label::new(RichText::new(preview).size(pal.meta()).color(pal.muted)).wrap().selectable(false));
            });
        }
        field_label(ui, &pal, "Thread name");
        let edit = ui.add(TextEdit::singleline(&mut st.name).char_limit(100).hint_text("New thread").desired_width(f32::INFINITY));
        if ui.memory(|m| m.focused().is_none()) {
            edit.request_focus();
        }
        let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if private_ok {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let fill = ui.visuals().window_fill;
                vector_icon(ui, Icon::Lock, 16.0, pal.muted, fill);
                ui.label(RichText::new("Private thread").color(pal.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    super::server_menu::switch(ui, &pal, &mut st.private).on_hover_text("Only people you invite and moderators can see it.");
                });
            });
        }
        field_label(ui, &pal, "Hide after inactivity");
        archive_combo(ui, "rc_thread_archive", &mut st.archive_minutes);
        if !allowed {
            ui.label(RichText::new("You can't create threads here.").size(pal.meta()).color(pal.danger));
        }
        dialog_buttons(ui, theme, "Create", false, allowed && valid).or((enter && allowed && valid).then_some(true))
    });
    match resp.inner {
        Some(true) => {
            let st = &cx.view.threads_ui;
            let cmd = SocialCommand::CreateThread {
                channel_id,
                name: st.name.trim().to_owned(),
                from_message,
                private: st.private,
                parent_kind: kind,
                auto_archive_minutes: st.archive_minutes,
            };
            cx.cmd.send(Command::Social(cmd));
            cx.view.threads_ui.pending = Some((channel_id, from_message, unix_ms()));
            cx.view.modal = None;
        }
        Some(false) => cx.view.modal = None,
        None if resp.should_close() => cx.view.modal = None,
        None => {}
    }
}

/// `minutes` when it is one of `ARCHIVE_AFTER`, else `default` (else 3 days).
pub(super) fn valid_archive(minutes: u32, default: u32) -> u32 {
    let ok = |m: u32| ARCHIVE_AFTER.iter().any(|(a, _)| *a == m);
    if ok(minutes) { minutes } else if ok(default) { default } else { 4320 }
}

/// Small bold label over a form field.
pub(super) fn field_label(ui: &mut egui::Ui, pal: &Pal, text: &str) {
    ui.add_space(4.0);
    ui.add(Label::new(RichText::new(text).family(bold_family()).size(pal.meta()).color(pal.muted)).selectable(false));
}

/// Auto-archive picker (`ARCHIVE_AFTER`).
pub(super) fn archive_combo(ui: &mut egui::Ui, salt: &str, minutes: &mut u32) {
    let current = ARCHIVE_AFTER.iter().find(|(m, _)| m == minutes).map_or("3 Days", |(_, l)| l);
    egui::ComboBox::from_id_salt(salt).selected_text(current).width(160.0).show_ui(ui, |ui| {
        for (m, label) in ARCHIVE_AFTER {
            ui.selectable_value(minutes, m, label);
        }
    });
}

// ---------------------------------------------------------------------------
// Per-frame bookkeeping
// ---------------------------------------------------------------------------

/// Per frame (top of the main layout): docks a thread / post created here in the panel beside its
/// parent once it opens (`ChannelOpened` selects it), and drops the thread list (and a forum's
/// thumbnails) once its channel is no longer the open one.
pub(super) fn track(cx: &mut UiCx<'_>, ppp: f32) {
    if let Some((parent, from_message, sent)) = cx.view.threads_ui.pending {
        let opened = cx.state.selected_channel.and_then(|s| cx.state.channels.get(&s)).filter(|c| c.is_thread() && c.parent_id == Some(parent));
        // From a message the thread id is the message id; otherwise it is newer than the request.
        let new = opened.filter(|c| from_message.map_or(c.id.timestamp_ms() + 15_000 >= sent, |m| c.id == m)).cloned();
        if let Some(t) = new {
            cx.view.threads_ui.pending = None;
            // A new forum post heads the list (the search would only find it on a reload).
            if let Some(v) = cx.state.social.threads.as_mut().filter(|v| v.channel_id == parent && !v.threads.iter().any(|x| x.id == t.id)) {
                v.threads.insert(0, t.clone());
            }
            cx.state.select_channel(parent);
            cx.view.thread_panel = Some(t.id);
        } else if unix_ms() > sent + PENDING_MS {
            cx.view.threads_ui.pending = None;
        }
    }
    // A list outlives neither its channel (Friends page, another channel or DM) nor, for the
    // popout's, the chat (settings layers); a forum keeps its posts under a settings layer.
    let hidden = cx.view.chat_hidden();
    let stale = cx.state.social.threads.as_ref().is_some_and(|v| cx.state.selected_channel != Some(v.channel_id) || (hidden && !matches!(v.list, ThreadList::Forum(_))));
    if stale && let Some(v) = cx.state.social.threads.take() {
        super::forum::forget_thumbs(cx, &v, ppp);
    }
}

pub(super) fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// "now", "5m ago", "3h ago", "2d ago", else the date.
pub(super) fn ago(unix_ms_then: u64) -> String {
    let s = unix_ms().saturating_sub(unix_ms_then) / 1000;
    match s {
        0..60 => "now".into(),
        60..3_600 => format!("{}m ago", s / 60),
        3_600..86_400 => format!("{}h ago", s / 3_600),
        86_400..2_592_000 => format!("{}d ago", s / 86_400),
        _ => chrono::DateTime::from_timestamp_millis(unix_ms_then as i64).map_or_else(String::new, |t| t.with_timezone(&chrono::Local).format("%-d %b %Y").to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_ages() {
        let now = unix_ms();
        assert_eq!(ago(now), "now");
        assert_eq!(ago(now - 5 * 60_000), "5m ago");
        assert_eq!(ago(now - 3 * 3_600_000), "3h ago");
        assert_eq!(ago(now - 2 * 86_400_000), "2d ago");
        assert!(ago(now - 400 * 86_400_000).chars().last().is_some_and(|c| c.is_ascii_digit()), "a date");
    }

    #[test]
    fn archive_choice_falls_back_to_a_valid_one() {
        assert_eq!(valid_archive(60, 4320), 60);
        assert_eq!(valid_archive(0, 10080), 10080, "the parent's default");
        assert_eq!(valid_archive(0, 0), 4320);
    }

    #[test]
    fn retry_resumes_paging_or_reloads() {
        let mut s = AppState::default();
        let failed = |has_more| ThreadsView { has_more, loading: false, error: Some("503".into()), ..ThreadsView::new(Id(1), ThreadList::Active { offset: 0 }) };
        s.social.threads = Some(failed(true));
        assert!(s.social.threads.as_ref().unwrap().next_page().is_some(), "paging is gated by the error, not next_page");
        retry(&mut s);
        assert!(s.social.threads.as_ref().is_some_and(|v| v.error.is_none()), "a later page: cleared, paging resumes");
        s.social.threads = Some(failed(false));
        retry(&mut s);
        assert!(s.social.threads.is_none(), "the first page: the list reloads");
    }

    #[test]
    fn popout_lists_match_by_kind_not_page() {
        let v = ThreadsView::new(Id(1), ThreadList::Archived { kind: ArchivedThreads::Public, before: Some("x".into()) });
        assert!(lists(&v, Id(1), &wanted_list(Some(ArchivedThreads::Public))));
        assert!(!lists(&v, Id(1), &wanted_list(Some(ArchivedThreads::Joined))));
        assert!(!lists(&v, Id(1), &wanted_list(None)));
        assert!(!lists(&v, Id(2), &wanted_list(Some(ArchivedThreads::Public))));
        let a = ThreadsView::new(Id(1), ThreadList::Active { offset: 25 });
        assert!(lists(&a, Id(1), &wanted_list(None)));
    }
}
