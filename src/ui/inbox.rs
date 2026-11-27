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
use crate::backend::{ChatCommand, Command, ConnState, MentionFilter, SocialCommand};
use crate::model::{Id, channel_type};
use crate::plugins::link_embeds::{self, Look};
use crate::saved;
use chrono::{DateTime, Local, TimeZone};
use crate::state::social::MentionsView;
use std::collections::HashMap;
use egui::{Align, FontId, Frame, Label, Layout, Margin, Rect, RichText, ScrollArea, Sense, Stroke, UiBuilder, Vec2, pos2, vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Mentions,
    Unreads,
    Keywords,
    /// Messages I saved on this device (`crate::saved`; ids only, the text loads when shown).
    Saved,
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
    /// Saved: the (channel, message) being fetched; one request at a time.
    saved_wait: Option<(Id, Id)>,
    /// The Remind modal's (day, minute); None until it is drawn (then the default).
    pub(super) remind: Option<(u8, u16)>,
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
            for (tab, label) in [(Tab::Mentions, "Mentions"), (Tab::Unreads, "Unreads"), (Tab::Keywords, "Keywords"), (Tab::Saved, "Saved"), (Tab::Recent, "Recent")] {
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
    // The header frame only wraps its content; the rule and the lit edge run the popout's full width.
    let r = egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), header.response.rect.y_range());
    ui.painter().hline(r.x_range(), r.bottom(), Stroke::new(1.0, pal.tk.rule));
    sheen(ui.painter(), r, f32::from(pal.float_radius()), &pal.tk);
    let flat = cx.theme.gradient.is_none();
    if flat {
        ui.spacing_mut().scroll.fade.strength = 0.0;
    }
    let out = ScrollArea::vertical()
        .id_salt(("rc_inbox_list", cx.view.inbox_ui.tab as u8))
        .max_height((screen.height() * 0.7 - 56.0).max(160.0))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            Frame::new().inner_margin(Margin::same(12)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                match (cx.view.inbox_ui.tab, unread) {
                    (Tab::Keywords, _) => keywords(ui, cx, pal),
                    (Tab::Saved, _) => saved(ui, cx, pal),
                    (Tab::Recent, _) => recent(ui, cx, pal),
                    (_, Some(ids)) => unreads(ui, cx, pal, &ids),
                    _ => mentions(ui, cx, pal),
                }
            });
        });
    // Faded into the popout at the scrolled edges, off its rounded corners (flat themes).
    if flat {
        // Inset by the list's card margin: the fade covers the whole card, off the rounded edge.
        let r = 12.0;
        super::scroll_fades(ui, &out, out.inner_rect, (out.inner_rect.left() + r, out.inner_rect.right() - r), (pal.surface, pal.surface));
    }
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
            place_line(ui, cx, pal, guild_id, msg.channel_id, None);
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
fn place_line(ui: &mut egui::Ui, cx: &UiCx<'_>, pal: &Pal, guild_id: Option<Id>, channel: Id, right: Option<&str>) {
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
    // Saved: the reminder's clock and time, right-aligned.
    let mut end = head.right();
    if let Some(t) = right {
        let g = ui.painter().layout_no_wrap(t.to_owned(), FontId::proportional(pal.meta()), pal.muted);
        end -= g.size().x + 4.0;
        ui.painter().galley(pos2(end + 4.0, head.center().y - g.size().y / 2.0), g, pal.muted);
        icons::paint(ui.painter(), Icon::Clock, Rect::from_center_size(pos2(end - 7.0, head.center().y), Vec2::splat(14.0)), pal.muted, pal.surface);
        end -= 22.0;
    }
    let g = one_line(ui, place.as_str(), bold(pal.meta()), pal.muted, end - x);
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
            place_line(ui, cx, pal, guild_id, msg.channel_id, None);
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
// Saved (crate::saved) and reminders
// ---------------------------------------------------------------------------

/// Saves the message, or removes it when it is saved (local; settings.toml keeps ids only).
pub(super) fn toggle_saved(cx: &mut UiCx<'_>, ch: Id, mid: Id) {
    let Some(me) = cx.state.me.as_ref().map(|u| u.id) else { return };
    match saved::toggle(&mut cx.settings.saved, me, ch, mid) {
        Ok(true) => cx.view.toast("Saved. Find it in Inbox > Saved.", false),
        Ok(false) => {}
        Err(e) => cx.view.toast(e, true),
    }
    persist(cx);
}

fn persist(cx: &mut UiCx<'_>) {
    if let Err(e) = cx.settings.save() {
        cx.view.toast(format!("Could not save settings: {e}"), true);
    }
}

fn set_reminder(cx: &mut UiCx<'_>, ch: Id, mid: Id, at: Option<DateTime<Local>>) {
    let Some(me) = cx.state.me.as_ref().map(|u| u.id) else { return };
    match saved::remind(&mut cx.settings.saved, me, ch, mid, at.map(|t| t.timestamp())) {
        Ok(()) => {
            if let Some(t) = at {
                cx.view.toast(format!("Reminder set for {}.", crate::alerts::time_label(t, Local::now())), false);
            }
        }
        Err(e) => cx.view.toast(e, true),
    }
    persist(cx);
}

/// The items of "Remind me" (the message menu's submenu and a Saved row's Remind menu).
pub(super) fn remind_items(ui: &mut egui::Ui, cx: &mut UiCx<'_>, ch: Id, mid: Id) {
    use super::messages::{MenuIcon, menu_item};
    let text = Pal::new(cx.theme).text;
    let now = Local::now();
    let me = cx.state.me.as_ref().map(|u| u.id);
    let set = me.and_then(|me| saved::position(&cx.settings.saved, me, mid)).is_some_and(|i| cx.settings.saved[i].remind_at.is_some());
    if menu_item(ui, "In 1 hour", MenuIcon::Glyph(""), text) {
        set_reminder(cx, ch, mid, Some(now + chrono::TimeDelta::hours(1)));
    }
    if let Some(t) = saved::evening(now)
        && menu_item(ui, "This evening", MenuIcon::Glyph(""), text)
    {
        set_reminder(cx, ch, mid, Some(t));
    }
    if menu_item(ui, "Tomorrow", MenuIcon::Glyph(""), text) {
        set_reminder(cx, ch, mid, Some(crate::alerts::tomorrow_morning(now)));
    }
    ui.separator();
    if menu_item(ui, "Custom…", MenuIcon::Glyph(""), text) {
        cx.view.inbox_ui.remind = None;
        cx.view.modal = Some(super::Modal::Remind { channel_id: ch, message_id: mid });
    }
    if set {
        ui.separator();
        if menu_item(ui, "Remove reminder", MenuIcon::Glyph(""), text) {
            set_reminder(cx, ch, mid, None);
        }
    }
}

/// "Today", "Tomorrow", then the weekday of the day `n` days on.
fn day_label(now: DateTime<Local>, n: u8) -> String {
    match n {
        0 => "Today".into(),
        1 => "Tomorrow".into(),
        n => (now.date_naive() + chrono::Days::new(n.into())).format("%A").to_string(),
    }
}

/// The Custom reminder dialog: a day and a time in 30-minute steps, later than now.
pub(super) fn remind_modal(ctx: &egui::Context, cx: &mut UiCx<'_>, ch: Id, mid: Id) {
    let pal = Pal::new(cx.theme);
    let theme = cx.theme.clone();
    let now = Local::now();
    let (mut day, mut minute) = cx.view.inbox_ui.remind.unwrap_or_else(|| saved::default_custom(now));
    let at = saved::custom_at(now, day, minute);
    let resp = super::dialog(ctx, &theme, "rc_remind", 440.0, "Remind me", |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let r = egui::ComboBox::from_id_salt("rc_remind_day").width(200.0).selected_text(day_label(now, day)).icon(super::user_settings::combo_icon).show_ui(ui, |ui| {
                for d in 0..7 {
                    ui.selectable_value(&mut day, d, day_label(now, d));
                }
            });
            r.response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "Day"));
            super::user_settings::time_combo(ui, "rc_remind_time", "Time", &mut minute);
        });
        if at.is_none() {
            ui.add(Label::new(RichText::new("Pick a time later than now.").size(12.0).color(pal.danger)));
        }
        super::dialog_buttons(ui, &theme, "Set reminder", false, at.is_some())
    });
    cx.view.inbox_ui.remind = Some((day, minute));
    let enter = at.is_some() && !egui::Popup::is_any_open(ctx) && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    match resp.inner {
        Some(true) => set_and_close(cx, ch, mid, at),
        _ if enter => set_and_close(cx, ch, mid, at),
        Some(false) => close_remind(cx),
        None if resp.should_close() => close_remind(cx),
        None => {}
    }
}

fn set_and_close(cx: &mut UiCx<'_>, ch: Id, mid: Id, at: Option<DateTime<Local>>) {
    set_reminder(cx, ch, mid, at);
    close_remind(cx);
}

fn close_remind(cx: &mut UiCx<'_>) {
    cx.view.modal = None;
    cx.view.inbox_ui.remind = None;
}

/// The Saved tab: newest first, each row's message loads while the row is on screen, one request
/// at a time and only while connected (`link_embeds`' cache; never Discord's bookmark endpoints).
fn saved(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal) {
    let Some(me) = cx.state.me.as_ref().map(|u| u.id) else { return };
    let rows: Vec<saved::Saved> = cx.settings.saved.iter().filter(|s| s.owner == me).cloned().collect();
    if rows.is_empty() {
        empty_state(ui, cx, 48.0, "Right-click a message and select Save message to keep it here.");
        return;
    }
    if let Some((ch, mid)) = cx.view.inbox_ui.saved_wait
        && (!matches!(link_embeds::look(cx.state, ch, mid), Look::Loading) || cx.state.conn != ConnState::Connected)
    {
        cx.view.inbox_ui.saved_wait = None;
    }
    ui.spacing_mut().item_spacing.y = 8.0;
    let (mut jump, mut removed) = (None, None);
    let ctx = ui.ctx().clone();
    let now = Local::now();
    for s in &rows {
        let (ch, mid) = (s.channel_id, s.message_id);
        let known = cx.state.channels.contains_key(&ch) && cx.state.can_view(ch);
        let h = cx.view.inbox_ui.heights.get(&mid).copied().unwrap_or(72.0) + 16.0 + ui.spacing().item_spacing.y;
        let est = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), h));
        if !ui.is_rect_visible(est) {
            ui.allocate_space(est.size());
            continue;
        }
        let mut look = link_embeds::look(cx.state, ch, mid);
        if matches!(look, Look::Unknown) {
            if !known {
                look = Look::Gone;
            } else {
                if cx.view.inbox_ui.saved_wait.is_none() && cx.state.conn == ConnState::Connected {
                    // The one request, for the first row on screen that waits.
                    fetch(cx, ch, mid);
                }
                look = Look::Loading;
            }
        }
        let guild_id = cx.state.channels.get(&ch).and_then(|c| c.guild_id);
        if known || matches!(look, Look::Message(_)) {
            let when = s.remind_at.and_then(|t| Local.timestamp_opt(t, 0).single()).map(|t| crate::alerts::time_label(t, now));
            place_line(ui, cx, pal, guild_id, ch, when.as_deref());
        }
        let (rect, clicked, retry) = match &look {
            Look::Message(m) => {
                let card = message_card(ui, cx, pal, guild_id, m, &[]);
                (card.rect, card.clicked(), false)
            }
            other => {
                let text = match other {
                    Look::Loading => "Loading message…",
                    Look::Failed => "Couldn't load this message.",
                    _ => "This message is no longer available.",
                };
                let mut retry = false;
                let rect = Frame::new()
                    .fill(pal.bg)
                    .corner_radius(pal.card_radius())
                    .inner_margin(Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.add(Label::new(RichText::new(text).color(pal.muted)).selectable(false));
                            if matches!(other, Look::Failed) {
                                retry = small_button(ui, pal, "Retry", pal.text).clicked();
                            }
                        });
                    })
                    .response
                    .rect;
                (rect, false, retry)
            }
        };
        cx.view.inbox_ui.heights.insert(mid, rect.height());
        if retry {
            // One request, on this click.
            fetch(cx, ch, mid);
        }
        if clicked {
            jump = Some((ch, mid));
        }
        let menu_id = egui::Id::unique(("rc_saved_remind", mid));
        if ui.rect_contains_pointer(rect) || egui::Popup::is_id_open(&ctx, menu_id) {
            let bar = Rect::from_min_max(rect.left_top() + vec2(0.0, 8.0), rect.right_top() + vec2(-8.0, 32.0));
            let mut bar = ui.new_child(UiBuilder::new().max_rect(bar).layout(Layout::right_to_left(Align::Min)));
            bar.spacing_mut().item_spacing.x = 4.0;
            if icon_button(&mut bar, pal, "×", "Remove", 24.0, false, pal.danger).clicked() {
                removed = Some(mid);
            }
            if matches!(look, Look::Message(_)) {
                let btn = small_button(&mut bar, pal, "Remind", pal.text);
                egui::Popup::menu(&btn).id(menu_id).show(|ui| {
                    super::messages::menu_setup(ui);
                    remind_items(ui, cx, ch, mid);
                });
                if small_button(&mut bar, pal, "Jump", pal.text).clicked() {
                    jump = Some((ch, mid));
                }
            }
        }
    }
    if let Some(mid) = removed {
        cx.settings.saved.retain(|s| !(s.owner == me && s.message_id == mid));
        persist(cx);
    }
    if let Some((ch, mid)) = jump {
        cx.view.inbox_open = false;
        jump_to(ui.ctx(), cx, ch, mid);
    }
}

/// One GET for one message (`FetchAround`, limit 1, through the REST limiter).
fn fetch(cx: &mut UiCx<'_>, ch: Id, mid: Id) {
    link_embeds::ask(mid);
    cx.cmd.send(Command::Chat(ChatCommand::FetchAround { channel_id: ch, message_id: mid, limit: 1 }));
    cx.view.inbox_ui.saved_wait = Some((ch, mid));
}

/// The text of a due reminder: "Author: first line" when the message is at hand, else where it was.
fn reminder_text(state: &crate::state::AppState, s: &saved::Saved) -> String {
    if let Look::Message(m) = link_embeds::look(state, s.channel_id, s.message_id) {
        let guild_id = state.channels.get(&s.channel_id).and_then(|c| c.guild_id).or(m.guild_id);
        let text = crate::state::notification_body(&m);
        // A heading's `# ` is markup, not text.
        let line = text.lines().next().unwrap_or_default().trim_start_matches('#').trim();
        return format!("{}: {line}", state.member_display_name(guild_id, &m.author, m.member.as_ref()));
    }
    match state.channels.get(&s.channel_id) {
        Some(c) => match c.guild_id.and_then(|g| state.guild(g)) {
            Some(g) => format!("#{} in {}", state.channel_name(s.channel_id), g.name),
            None => state.channel_name(s.channel_id),
        },
        None => "A saved message".into(),
    }
}

/// Reminders that came due: each toasts ("Reminder: ...") and is returned as (text for the desktop
/// notification, message to jump to). Run every `logic` pass, so an overdue one fires at the next
/// start. Schedules the next check while one is pending, nothing otherwise.
pub(super) fn reminders(ctx: &egui::Context, state: &crate::state::AppState, view: &mut super::ViewState, settings: &mut crate::config::Settings) -> Vec<(String, (Id, Id))> {
    let Some(me) = state.me.as_ref().map(|u| u.id) else { return Vec::new() };
    let now = Local::now().timestamp();
    let due = saved::take_due(&mut settings.saved, me, now);
    if !due.is_empty()
        && let Err(e) = settings.save()
    {
        view.toast(format!("Could not save settings: {e}"), true);
    }
    if let Some(next) = saved::next_due(&settings.saved, me) {
        let secs = (next - now).clamp(1, 60) as u64;
        ctx.request_repaint_after_for(std::time::Duration::from_secs(secs), egui::ViewportId::ROOT);
    }
    due.into_iter()
        .map(|s| {
            let text = reminder_text(state, &s);
            let line = if settings.hides_sender() { "a saved message".to_owned() } else { text.clone() };
            view.toast(format!("Reminder: {line}"), false);
            (text, (s.channel_id, s.message_id))
        })
        .collect()
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
    let (at, _) = sidebar::name_and_status(ui, cx, title, pal.text, pal.surface, sub.as_ref(), pos2(left, row.center().y), right - left);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Channel, Guild, Message, User};
    use crate::state::AppState;

    #[test]
    fn reminder_text_names_the_author_or_the_place() {
        let mut state = AppState::default();
        state.guilds.push(Guild { id: Id(1), name: "Rust Workshop".into(), ..Default::default() });
        state.channels.insert(Id(2), Channel { id: Id(2), guild_id: Some(Id(1)), name: Some("general".into()), ..Default::default() });
        let s = saved::Saved { owner: Id(9), channel_id: Id(2), message_id: Id(3), remind_at: None };
        assert_eq!(reminder_text(&state, &s), "#general in Rust Workshop");
        let author = User { id: Id(4), username: "bob".into(), ..Default::default() };
        let msg = Message { id: Id(3), channel_id: Id(2), content: "hello\nworld".into(), author, ..Default::default() };
        state.messages.entry(Id(2)).or_default().msgs.push(msg);
        assert_eq!(reminder_text(&state, &s), "bob: hello");
        let gone = saved::Saved { channel_id: Id(77), ..s };
        assert_eq!(reminder_text(&state, &gone), "A saved message");
    }
}
