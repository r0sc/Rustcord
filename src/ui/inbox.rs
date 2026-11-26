//! Inbox popout (FEATURES_NOTES §2.5, layout §4.9; Ctrl+I): Mentions (REST pages of
//! `MentionsView`, newest first; Jump and × per card, the next page when the list end shows),
//! Unreads (local `unread_channels`; a click opens the channel, ✔ marks one read, the
//! header's ✔ marks all read via `mark_all_read` + `MarkAllRead`) and Keywords (this session's
//! keyword-alert matches, `crate::alerts`; a keyword notification click jumps to its message). Opened by the inbox button
//! (`button`, on the right of the title bar) under it; `ViewState::inbox_open` is its open
//! state. The mentions list is dropped when it closes.
//! OWNERSHIP: friends/DMs/inbox/profile engineer (docs/SOCIAL_UI_DESIGN.md).

use super::messages::{Pal, icon_button, jump_to, message_card, sheen, small_button};
use super::sidebar::{self, badge, dm_avatar_at, empty_state};
use super::icons::{self, Icon};
use super::{UiCx, bold, one_line};
use crate::backend::{Command, MentionFilter, SocialCommand};
use crate::model::{Id, channel_type};
use crate::state::social::MentionsView;
use std::collections::HashMap;
use egui::{Align, FontId, Frame, Label, Layout, Margin, Rect, RichText, ScrollArea, Sense, Stroke, UiBuilder, Vec2, pos2, vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Mentions,
    Unreads,
    Keywords,
    /// NotificationLog plugin: this session's notifications, also the silenced ones.
    Recent,
}

/// Inbox state (in `ViewState::inbox_ui`).
#[derive(Debug, Default)]
pub struct InboxUi {
    pub tab: Tab,
    /// Mentions: each card's last drawn height (off-screen cards only take their space).
    heights: HashMap<Id, f32>,
    /// This session's keyword matches, newest first (`crate::alerts`; memory only).
    pub(super) keyword_hits: std::collections::VecDeque<crate::alerts::Hit>,
    /// The Recent tab's rows, newest first (`plugins::notifier::log`; memory only, gone on logout).
    pub(super) recent: std::collections::VecDeque<crate::plugins::notifier::Entry>,
    /// (channel, message) of the keyword notification shown last: a click on it jumps there.
    pub(super) notified: Option<(Id, Id)>,
    /// A keyword notification was clicked: jump there once the title bar is drawn (`button`).
    jump: Option<(Id, Id)>,
}

/// Mentions per page (the API allows 1-100).
const PAGE: u8 = 25;

fn popup_id() -> egui::Id {
    egui::Id::unique("rc_inbox_popout")
}

/// The title bar's 28 px inbox button and its popout below it (right-aligned). Ctrl+I toggles it.
pub(super) fn button(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal) {
    if let Some((ch, mid)) = cx.view.inbox_ui.jump.take() {
        cx.view.inbox_open = false;
        jump_to(ui.ctx(), cx, ch, mid);
    }
    let bind = super::user_settings::shortcut(cx.settings, super::user_settings::Bind::Inbox);
    let key = bind.is_some_and(|k| ui.input_mut(|i| i.consume_shortcut(&k)));
    // The tooltip names the bound shortcut (Settings > Keybinds), not the default.
    let tip = bind.map_or_else(|| "Inbox".to_owned(), |k| format!("Inbox ({})", ui.ctx().format_shortcut(&k)));
    let fill = ui.painter().add(egui::Shape::Noop);
    let resp = icon_button(ui, pal, "📥", &tip, 28.0, cx.view.inbox_open, pal.text);
    if cx.view.inbox_open {
        ui.painter().set(fill, egui::Shape::rect_filled(resp.rect, pal.radius, pal.tint()));
    }
    if resp.clicked() || key {
        cx.view.inbox_open = !cx.view.inbox_open;
    }
    let mut open = cx.view.inbox_open;
    let frame = Frame::popup(ui.style()).fill(pal.surface).corner_radius(pal.float_radius()).inner_margin(Margin::ZERO);
    egui::Popup::new(popup_id(), ui.ctx().clone(), &resp, ui.layer_id())
        .open_bool(&mut open)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .align(egui::RectAlign::BOTTOM_END)
        .gap(8.0)
        .frame(frame)
        .show(|ui| contents(ui, cx, pal));
    // A jump inside closes it too.
    cx.view.inbox_open &= open;
    if !cx.view.inbox_open {
        cx.state.social.mentions = None;
        cx.view.inbox_ui.heights = HashMap::new();
    }
}

fn contents(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal) {
    let screen = ui.ctx().content_rect();
    ui.set_width(480.0f32.min(screen.width() - 32.0));
    ui.set_min_height(320.0f32.min(screen.height() - 96.0));
    let log = crate::plugins::enabled(crate::plugins::Plugin::NotificationLog);
    // The plugin was switched off with its tab open.
    if cx.view.inbox_ui.tab == Tab::Recent && !log {
        cx.view.inbox_ui.tab = Tab::Mentions;
    }
    let unread = (cx.view.inbox_ui.tab == Tab::Unreads).then(|| cx.state.unread_channels());
    let header = Frame::new().inner_margin(Margin { left: 16, right: 12, top: 10, bottom: 10 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.add(Label::new(RichText::new("Inbox").font(bold(pal.size + 2.0)).color(pal.text)).selectable(false));
            ui.add_space(8.0);
            for (tab, label) in [(Tab::Mentions, "Mentions"), (Tab::Unreads, "Unreads"), (Tab::Keywords, "Keywords"), (Tab::Recent, "Recent")] {
                if (tab != Tab::Recent || log) && tab_pill(ui, pal, label, cx.view.inbox_ui.tab == tab).clicked() {
                    cx.view.inbox_ui.tab = tab;
                }
            }
            if cx.view.inbox_ui.tab == Tab::Recent && !cx.view.inbox_ui.recent.is_empty() {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if small_button(ui, pal, "Clear", pal.text).clicked() {
                        cx.view.inbox_ui.recent.clear();
                    }
                });
            }
            if let Some(ids) = unread.as_ref().filter(|ids| !ids.is_empty()) {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icon_button(ui, pal, "✔", "Mark all as read", 28.0, false, pal.text).clicked() {
                        mark_all(cx, ids);
                    }
                });
            }
        });
    });
    // Under the header itself: `ui.min_rect()` would include the popup's minimum height.
    let r = header.response.rect;
    ui.painter().hline(r.x_range(), r.bottom(), Stroke::new(1.0, pal.tk.rule));
    sheen(ui.painter(), r, f32::from(pal.float_radius()), &pal.tk);
    ScrollArea::vertical()
        .id_salt(("rc_inbox_list", cx.view.inbox_ui.tab as u8))
        .max_height((screen.height() * 0.7 - 56.0).max(160.0))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            Frame::new().inner_margin(Margin::same(12)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                match (cx.view.inbox_ui.tab, unread) {
                    (Tab::Keywords, _) => keywords(ui, cx, pal),
                    (Tab::Recent, _) => recent(ui, cx, pal),
                    (_, Some(ids)) => unreads(ui, cx, pal, &ids),
                    _ => mentions(ui, cx, pal),
                }
            });
        });
}

/// A header tab: tinted while selected or hovered.
fn tab_pill(ui: &mut egui::Ui, pal: &Pal, text: &str, selected: bool) -> egui::Response {
    let g = ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(pal.size - 1.0), pal.text);
    let (rect, resp) = ui.allocate_exact_size(g.size() + vec2(16.0, 10.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, text));
    let hot = resp.hovered() || resp.has_focus();
    let fill = if selected { pal.text.gamma_multiply(0.14) } else if hot { pal.tint() } else { egui::Color32::TRANSPARENT };
    ui.painter().rect_filled(rect, pal.radius, fill);
    ui.painter().galley(rect.center() - g.size() / 2.0, g, if selected || hot { pal.text } else { pal.muted });
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ---------------------------------------------------------------------------
// Mentions
// ---------------------------------------------------------------------------

fn mentions(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal) {
    // Opening the tab loads the first page (once: Retry after an error).
    if cx.state.social.mentions.is_none() {
        start(cx);
    }
    let Some(v) = cx.state.social.mentions.as_ref() else { return };
    let (n, has_more, loading, error) = (v.messages.len(), v.has_more, v.loading, v.error.clone());
    ui.spacing_mut().item_spacing.y = 8.0;
    if n == 0 {
        if loading {
            ui.add_space(48.0);
            ui.vertical_centered(|ui| ui.spinner());
        } else if error.is_none() {
            empty_state(ui, cx, 48.0, "No mentions in the last 7 days.");
        }
    }
    let (mut jump, mut prev, mut removed) = (None, None, None);
    // Drawn from a borrowed-out list (no per-frame clones), put back below.
    let mut msgs = cx.state.social.mentions.as_mut().map(|v| std::mem::take(&mut v.messages)).unwrap_or_default();
    for msg in &msgs {
        let guild_id = cx.state.channels.get(&msg.channel_id).and_then(|c| c.guild_id).or(msg.guild_id);
        let head = prev != Some(msg.channel_id);
        prev = Some(msg.channel_id);
        // Off-screen: only the space of its last height (+ the channel line), nothing drawn.
        let h = cx.view.inbox_ui.heights.get(&msg.id).copied().unwrap_or(96.0) + if head { 16.0 + ui.spacing().item_spacing.y } else { 0.0 };
        let est = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), h));
        if !ui.is_rect_visible(est) {
            ui.allocate_space(est.size());
            continue;
        }
        // Where it was said, once per run of cards from one channel: "#channel in Server".
        if head {
            place_line(ui, cx, pal, guild_id, msg.channel_id);
        }
        let card = message_card(ui, cx, pal, guild_id, msg, &[]);
        cx.view.inbox_ui.heights.insert(msg.id, card.rect.height());
        if card.clicked() {
            jump = Some((msg.channel_id, msg.id));
        }
        if ui.rect_contains_pointer(card.rect) {
            let bar = Rect::from_min_max(card.rect.left_top() + vec2(0.0, 8.0), card.rect.right_top() + vec2(-8.0, 32.0));
            let mut bar = ui.new_child(UiBuilder::new().max_rect(bar).layout(Layout::right_to_left(Align::Min)));
            bar.spacing_mut().item_spacing.x = 4.0;
            if icon_button(&mut bar, pal, "×", "Remove", 24.0, false, pal.danger).clicked() {
                cx.cmd.send(Command::Social(SocialCommand::DeleteMention { message_id: msg.id }));
                removed = Some(msg.id);
            }
            if small_button(&mut bar, pal, "Jump", pal.text).clicked() {
                jump = Some((msg.channel_id, msg.id));
            }
        }
    }
    // RECENT_MENTION_DELETE confirms; gone from the list right away.
    if let Some(id) = removed {
        msgs.retain(|m| m.id != id);
    }
    if let Some(v) = cx.state.social.mentions.as_mut() {
        v.messages = msgs;
    }
    if let Some(e) = error {
        ui.horizontal_wrapped(|ui| {
            ui.add(Label::new(RichText::new(e).color(pal.danger)).wrap());
            if small_button(ui, pal, "Retry", pal.text).clicked() {
                cx.state.social.mentions = None;
            }
        });
    } else if has_more && n > 0 {
        // Scrolled to the end: the next page.
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
        if loading {
            ui.put(rect, egui::Spinner::new());
        } else if ui.is_rect_visible(rect)
            && let Some(v) = cx.state.social.mentions.as_mut()
            && let Some(before) = v.next_before()
        {
            v.loading = true;
            let filter = v.filter;
            cx.cmd.send(Command::Social(SocialCommand::FetchMentions { filter, before: Some(before), limit: PAGE }));
        }
    }
    if let Some((ch, mid)) = jump {
        cx.view.inbox_open = false;
        jump_to(ui.ctx(), cx, ch, mid);
    }
}

/// The 16 px line above a run of cards: "#channel in Server" (a thread's icon in front).
fn place_line(ui: &mut egui::Ui, cx: &UiCx<'_>, pal: &Pal, guild_id: Option<Id>, channel: Id) {
    let (icon, place) = match guild_id.and_then(|g| cx.state.guild(g)) {
        Some(g) => {
            let (icon, name) = place_name(cx, channel);
            (icon, format!("{name} in {}", g.name))
        }
        None => (None, cx.state.channel_name(channel)),
    };
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
    let mut x = head.left() + 4.0;
    if let Some(icon) = icon {
        icons::paint(ui.painter(), icon, Rect::from_min_size(pos2(x, head.center().y - 7.0), Vec2::splat(14.0)), pal.muted, pal.surface);
        x += 18.0;
    }
    let g = one_line(ui, place.as_str(), bold(pal.meta()), pal.muted, head.right() - x);
    ui.painter().galley(pos2(x, head.center().y - g.size().y / 2.0), g, pal.muted);
}

fn channel_kind(cx: &UiCx<'_>, ch: Id) -> u8 {
    cx.state.channels.get(&ch).map_or(channel_type::GUILD_TEXT, |c| c.kind)
}

/// "#general", "@user", or a thread / forum post with the thread icon to paint in front.
fn place_name(cx: &UiCx<'_>, ch: Id) -> (Option<Icon>, String) {
    let kind = channel_kind(cx, ch);
    let name = cx.state.channel_name(ch);
    if cx.state.channels.get(&ch).is_some_and(|c| c.is_thread()) {
        (Some(Icon::Thread), name)
    } else {
        (None, format!("{}{name}", super::messages::channel_prefix(kind)))
    }
}

/// The first page (`RUSTCORD_NAV_DEMO=mentions`: cached messages instead, no request).
fn start(cx: &mut UiCx<'_>) {
    let filter = MentionFilter::default();
    if sidebar::nav_demo("mentions") {
        let msgs = cx.state.messages.values().flat_map(|c| c.msgs.iter().rev().take(3)).take(5).cloned().collect();
        cx.state.social.mentions = Some(MentionsView { filter, messages: msgs, ..Default::default() });
        return;
    }
    cx.state.social.mentions = Some(MentionsView::new(filter));
    cx.cmd.send(Command::Social(SocialCommand::FetchMentions { filter, before: None, limit: PAGE }));
}

// ---------------------------------------------------------------------------
// Keywords
// ---------------------------------------------------------------------------

/// This session's keyword matches (`crate::alerts`): the same cards as Mentions, the matched
/// word marked. A click or Jump opens the message; × drops the card (local only).
fn keywords(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal) {
    if cx.settings.keyword_alerts.keywords.is_empty() {
        empty_state(ui, cx, 48.0, "Get notified when words you care about come up.");
        ui.add_space(12.0);
        ui.vertical_centered(|ui| {
            if super::messages::button(ui, pal, super::messages::Btn::Secondary, "Set up keyword alerts", true).clicked() {
                cx.view.inbox_open = false;
                (cx.view.settings_open, cx.view.settings_page) = (true, super::user_settings::Page::Notifications);
            }
        });
        return;
    }
    if cx.view.inbox_ui.keyword_hits.is_empty() {
        empty_state(ui, cx, 48.0, "No keyword alerts yet. They stay here until you quit Rustcord.");
        return;
    }
    ui.spacing_mut().item_spacing.y = 8.0;
    let (mut jump, mut prev, mut removed) = (None, None, None);
    let hits = std::mem::take(&mut cx.view.inbox_ui.keyword_hits);
    for hit in &hits {
        let msg = &hit.msg;
        let guild_id = cx.state.channels.get(&msg.channel_id).and_then(|c| c.guild_id).or(msg.guild_id);
        let head = prev != Some(msg.channel_id);
        prev = Some(msg.channel_id);
        let h = cx.view.inbox_ui.heights.get(&msg.id).copied().unwrap_or(96.0) + if head { 16.0 + ui.spacing().item_spacing.y } else { 0.0 };
        let est = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), h));
        if !ui.is_rect_visible(est) {
            ui.allocate_space(est.size());
            continue;
        }
        if head {
            place_line(ui, cx, pal, guild_id, msg.channel_id);
        }
        let card = message_card(ui, cx, pal, guild_id, msg, std::slice::from_ref(&hit.keyword));
        cx.view.inbox_ui.heights.insert(msg.id, card.rect.height());
        if card.clicked() {
            jump = Some((msg.channel_id, msg.id));
        }
        if ui.rect_contains_pointer(card.rect) {
            let bar = Rect::from_min_max(card.rect.left_top() + vec2(0.0, 8.0), card.rect.right_top() + vec2(-8.0, 32.0));
            let mut bar = ui.new_child(UiBuilder::new().max_rect(bar).layout(Layout::right_to_left(Align::Min)));
            bar.spacing_mut().item_spacing.x = 4.0;
            if icon_button(&mut bar, pal, "×", "Remove", 24.0, false, pal.danger).clicked() {
                removed = Some(msg.id);
            }
            if small_button(&mut bar, pal, "Jump", pal.text).clicked() {
                jump = Some((msg.channel_id, msg.id));
            }
        }
    }
    cx.view.inbox_ui.keyword_hits = hits;
    if let Some(id) = removed {
        cx.view.inbox_ui.keyword_hits.retain(|h| h.msg.id != id);
    }
    if let Some((ch, mid)) = jump {
        cx.view.inbox_open = false;
        jump_to(ui.ctx(), cx, ch, mid);
    }
}

// ---------------------------------------------------------------------------
// Recent (NotificationLog)
// ---------------------------------------------------------------------------

fn recent(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal) {
    if cx.view.inbox_ui.recent.is_empty() {
        empty_state(ui, cx, 48.0, "No notifications yet. They stay here until you quit Rustcord, the silenced ones too.");
        return;
    }
    ui.spacing_mut().item_spacing.y = 4.0;
    let mut jump = None;
    let rows = std::mem::take(&mut cx.view.inbox_ui.recent);
    for (i, e) in rows.iter().enumerate() {
        if recent_row(ui, cx, pal, e, i).clicked() {
            jump = Some((e.channel_id, e.message_id));
        }
    }
    cx.view.inbox_ui.recent = rows;
    if let Some((ch, mid)) = jump {
        cx.view.inbox_open = false;
        jump_to(ui.ctx(), cx, ch, mid);
    }
}

/// A notification: the sender's picture, title and time, one line of text and, when it was
/// silenced, the reason as a tag (the word, never colour alone). A click jumps to the message.
fn recent_row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, e: &crate::plugins::notifier::Entry, i: usize) -> egui::Response {
    let guild_id = cx.state.channels.get(&e.channel_id).and_then(|c| c.guild_id);
    let author = e.author_id.and_then(|id| cx.state.users.get(&id).cloned().or_else(|| cx.state.me.clone().filter(|m| m.id == id)));
    let bg = ui.painter().add(egui::Shape::Noop);
    let rect = Frame::new()
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                match &author {
                    Some(u) => {
                        let name = cx.state.member_display_name(guild_id, u, None);
                        super::images::avatar(ui, cx.images, &super::images::user_avatar_url(guild_id, u, None, 64, false), 32.0, false, Some((u.id, &name)));
                    }
                    None => {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 16.0, pal.tk.selected);
                    }
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let time = super::messages::local_at(ui.ctx(), e.at_ms as i64).map(|t| t.format("%H:%M").to_string()).unwrap_or_default();
                        let time_w = ui.painter().layout_no_wrap(time.clone(), FontId::proportional(pal.meta()), pal.muted).size().x;
                        ui.scope(|ui| {
                            ui.set_max_width((ui.available_width() - time_w - 8.0).max(24.0));
                            ui.add(Label::new(RichText::new(&e.title).font(bold(pal.size)).color(pal.text)).selectable(false).truncate());
                        });
                        ui.add(Label::new(RichText::new(time).size(pal.meta()).color(pal.muted)).selectable(false));
                    });
                    ui.add(Label::new(RichText::new(&e.body).color(pal.muted)).selectable(false).truncate());
                    if let Some(s) = e.silenced {
                        super::messages::tag(ui, s.label(), pal.tk.selected, pal.text);
                    }
                });
            });
        })
        .response
        .rect;
    // Registered after the content, so it wins the click anywhere on the row.
    let resp = ui.interact(rect, ui.make_persistent_id(("rc_recent", i)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let fill = if ui.rect_contains_pointer(rect) { pal.surface.blend(pal.tint()) } else { egui::Color32::TRANSPARENT };
    ui.painter().set(bg, egui::Shape::rect_filled(rect, pal.card_radius(), fill));
    resp
}

/// `RUSTCORD_NAV_DEMO=recent`: this session's rows from the cached messages, three of them
/// silenced for the three reasons. In memory only.
pub(super) fn demo_recent(state: &crate::state::AppState, view: &mut super::ViewState) {
    use crate::plugins::notifier::Silenced;
    let me = state.me.as_ref().map(|u| u.id);
    let reasons = [None, Some(Silenced::QuietHours), None, Some(Silenced::DoNotDisturb), Some(Silenced::OnePing)];
    let msgs = state.recent_channels.iter().filter_map(|ch| state.messages.get(ch)).flat_map(|cm| cm.msgs.iter().rev()).filter(|m| Some(m.author.id) != me);
    for (i, (msg, silenced)) in msgs.zip(reasons).enumerate() {
        let guild_id = state.channels.get(&msg.channel_id).and_then(|c| c.guild_id);
        let author = state.member_display_name(guild_id, &msg.author, None);
        let title = match guild_id {
            Some(g) => format!("{author} (#{}, {})", state.channel_name(msg.channel_id), state.guild(g).map_or("", |g| g.name.as_str())),
            None => author.clone(),
        };
        let body = crate::state::notification_body(msg);
        view.inbox_ui.recent.push_back(crate::plugins::notifier::Entry {
            channel_id: msg.channel_id,
            message_id: msg.id,
            author_id: Some(msg.author.id),
            title,
            body,
            at_ms: crate::activity::now_ms().saturating_sub(i as u64 * 7 * 60_000),
            silenced,
        });
    }
    view.inbox_ui.tab = Tab::Recent;
}

/// A click on a keyword notification: jump to its message once the title bar is drawn again.
pub(super) fn notification_clicked(view: &mut super::ViewState) {
    if let Some(j) = view.inbox_ui.notified.take() {
        view.inbox_ui.jump = Some(j);
        view.settings_open = false;
        view.server_settings = None;
    }
}

/// `RUSTCORD_NAV_DEMO=keywords`: keywords and matches from the cached messages, quiet hours on
/// with a pause. In memory only (never saved), sends nothing.
pub(super) fn demo(state: &crate::state::AppState, view: &mut super::ViewState, settings: &mut crate::config::Settings) {
    let me = state.me.as_ref().map(|u| u.id);
    let a = &mut settings.keyword_alerts;
    let hits = &mut view.inbox_ui.keyword_hits;
    for ch in state.recent_channels.iter().filter(|c| state.channels.get(c).is_some_and(|c| c.guild_id.is_some())) {
        let Some(cm) = state.messages.get(ch) else { continue };
        for msg in cm.msgs.iter().rev().filter(|m| Some(m.author.id) != me).take(3) {
            let word = msg.content.split(|c: char| !c.is_alphabetic()).find(|w| w.chars().count() >= 4);
            if let Some(w) = word.map(str::to_lowercase)
                && hits.len() < 5
            {
                let _ = a.add(&w);
                // Markdown marks (`__word__`) can keep a whole-word match from happening.
                if let Some(k) = a.find(&msg.content) {
                    hits.push_back(crate::alerts::Hit { msg: msg.clone(), keyword: k.text.clone() });
                }
            }
        }
    }
    if a.keywords.is_empty() {
        for w in ["rust", "release"] {
            let _ = a.add(w);
        }
    }
    let now = chrono::Utc::now().timestamp();
    settings.quiet_hours = crate::alerts::QuietHours { enabled: true, days: 0b0011111, start: 22 * 60, end: 7 * 60, paused_until: Some(now + 3600) };
    view.inbox_ui.tab = Tab::Keywords;
}

// ---------------------------------------------------------------------------
// Unreads
// ---------------------------------------------------------------------------

fn unreads(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, ids: &[Id]) {
    if ids.is_empty() {
        empty_state(ui, cx, 48.0, "You're all caught up.");
        return;
    }
    ui.spacing_mut().item_spacing.y = 2.0;
    for &id in ids {
        unread_row(ui, cx, pal, id);
    }
}

/// An unread channel: its picture / glyph, name and server, mention count; ✔ on hover.
fn unread_row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, id: Id) {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
    if !ui.is_rect_visible(row) {
        return;
    }
    let bg = ui.painter().add(egui::Shape::Noop);
    let kind = channel_kind(cx, id);
    let dm = matches!(kind, channel_type::DM | channel_type::GROUP_DM);
    let icon = Rect::from_min_size(pos2(row.left() + 8.0, row.center().y - 14.0), Vec2::splat(28.0));
    if dm {
        dm_avatar_at(ui, cx, id, icon);
    } else {
        ui.painter().rect_filled(icon, pal.radius, pal.tint());
        let kind_icon = cx.state.channels.get(&id).map_or((Icon::Text, None), |c| icons::of(c, cx.state));
        icons::paint_channel(ui.painter(), kind_icon, Rect::from_center_size(icon.center(), Vec2::splat(20.0)), pal.muted, pal.surface.blend(pal.tint()));
    }
    let resp = ui.interact(row, ui.make_persistent_id(("rc_inbox_unread", id)), Sense::click());
    let hot = ui.rect_contains_pointer(row);
    if hot {
        ui.painter().set(bg, egui::Shape::rect_filled(row, pal.radius, pal.tint()));
    }
    let mut right = row.right() - 8.0;
    let mut mark = false;
    if hot {
        let r = Rect::from_center_size(pos2(right - 14.0, row.center().y), Vec2::splat(28.0));
        let mut child = ui.new_child(UiBuilder::new().max_rect(r));
        mark = icon_button(&mut child, pal, "✔", "Mark as read", 28.0, false, pal.text).clicked();
        right -= 32.0;
    } else if cx.state.mention_count(id) > 0 {
        right -= badge(ui, &cx.theme.colors, pos2(right, row.center().y), cx.state.mention_count(id), None) + 8.0;
    }
    let (thread, name) = place_name(cx, id);
    // A thread's icon goes in front of its name (and the server line under it).
    let left = icon.right() + if thread.is_some() { 30.0 } else { 10.0 };
    let guild = cx.state.channels.get(&id).and_then(|c| c.guild_id).and_then(|g| cx.state.guild(g)).map(|g| g.name.clone());
    let title = one_line(ui, name.as_str(), bold(pal.size), pal.text, right - left);
    let sub = guild.map(|g| (None, g));
    let (at, _) = sidebar::name_and_status(ui, cx, title, pal.text, sub.as_ref(), pos2(left, row.center().y), right - left);
    if let Some(i) = thread {
        icons::paint(ui.painter(), i, Rect::from_center_size(pos2(at.left() - 12.0, at.center().y), Vec2::splat(16.0)), pal.text, pal.surface);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
    if mark {
        sidebar::mark_read(cx, [id]);
    } else if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        cx.view.inbox_open = false;
        cx.state.select_channel(id);
    }
}

/// "Mark all as read": local at once; the bulk ack only with read receipts on.
fn mark_all(cx: &mut UiCx<'_>, ids: &[Id]) {
    let acks = cx.state.mark_all_read(ids);
    if cx.settings.send_read_acks && !acks.is_empty() {
        cx.cmd.send(Command::Social(SocialCommand::MarkAllRead { acks }));
    }
}
