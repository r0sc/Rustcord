//! Pinned messages popout (FEATURES_NOTES §1.4, layout §4.9): the chat header's pin button
//! (Ctrl+P) opens a 420 px card under it listing `FetchPins` pages, newest pin first, with Jump
//! and, given PIN_MESSAGES (anyone in DMs), Unpin. The list is dropped when it closes.
//! OWNERSHIP: ui-messages implementer.

use super::messages::{icon_button, jump_to, message_card, small_button, Pal, PIN_MESSAGES};
use super::UiCx;
use crate::backend::{ChatCommand, Command};
use crate::model::{Id, Message, PinnedMessage};
use crate::state::chat::PinsView;
use egui::{vec2, Align, Frame, Label, Layout, Margin, Rect, RichText, ScrollArea, Sense, Stroke, UiBuilder};

pub(super) fn popup_id() -> egui::Id {
    egui::Id::unique("rc_pins_popout")
}

/// The header's pin button and its popout; `toggle_key` = Ctrl+P was pressed.
pub(super) fn button(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, ch: Id, toggle_key: bool) {
    let ctx = ui.ctx().clone();
    let id = popup_id();
    // Another channel's list closes with the switch.
    if cx.state.chat.pins.as_ref().is_some_and(|v| v.channel_id != ch) {
        cx.state.chat.pins = None;
        egui::Popup::close_id(&ctx, id);
    }
    let open = egui::Popup::is_id_open(&ctx, id);
    // Selected while its popout is open: bright glyph on the hover tint.
    let fill = ui.painter().add(egui::Shape::Noop);
    let resp = icon_button(ui, pal, "📌", "Pinned messages", 32.0, open, pal.text);
    if open {
        ui.painter().set(fill, egui::Shape::rect_filled(resp.rect, pal.radius, pal.tint()));
    }
    let toggle = resp.clicked() || toggle_key;
    let frame = Frame::popup(ui.style()).fill(pal.surface).corner_radius(pal.float_radius()).inner_margin(Margin::ZERO);
    egui::Popup::new(id, ctx.clone(), &resp, ui.layer_id())
        .open_memory(toggle.then_some(egui::SetOpenCommand::Toggle))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .align(egui::RectAlign::BOTTOM_END)
        .gap(8.0)
        .frame(frame)
        .show(|ui| contents(ui, cx, pal, ch));
    if !egui::Popup::is_id_open(&ctx, id) {
        cx.state.chat.pins = None;
    }
}

fn contents(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, ch: Id) {
    // Just opened, or CHANNEL_PINS_UPDATE marked the list stale: (re)load the first page.
    match cx.state.chat.pins.as_mut() {
        None => {
            cx.state.chat.pins = Some(PinsView { channel_id: ch, loading: true, ..Default::default() });
            cx.cmd.send(Command::Chat(ChatCommand::FetchPins { channel_id: ch, before: None }));
        }
        // Once failed, only Retry refetches (no loop on 4xx / offline).
        Some(v) if v.stale && !v.loading && v.error.is_none() => {
            v.loading = true;
            cx.cmd.send(Command::Chat(ChatCommand::FetchPins { channel_id: ch, before: None }));
        }
        Some(_) => {}
    }
    let screen = ui.ctx().content_rect();
    ui.set_width(420.0f32.min(screen.width() - 32.0));
    Frame::new().inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add(Label::new(RichText::new("Pinned messages").font(egui::FontId::new(pal.heading(), crate::theme::semibold_family())).color(pal.text)).selectable(false));
    });
    let r = ui.min_rect();
    ui.painter().hline(r.x_range(), r.bottom(), Stroke::new(1.0, pal.tk.rule));
    let guild_id = cx.state.channels.get(&ch).and_then(|c| c.guild_id);
    let can_unpin = cx.state.channel_permissions(ch) & PIN_MESSAGES != 0;
    ScrollArea::vertical()
        .id_salt("rc_pins_list")
        .max_height((screen.height() * 0.8 - 56.0).max(140.0))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            Frame::new().inner_margin(Margin::same(12)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 8.0;
                list(ui, cx, pal, ch, guild_id, can_unpin);
            });
        });
}

fn list(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, ch: Id, guild_id: Option<Id>, can_unpin: bool) {
    let Some(v) = cx.state.chat.pins.as_ref() else { return };
    let (n, has_more, loading, error) = (v.items.len(), v.has_more, v.loading, v.error.clone());
    if n == 0 {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 96.0), Sense::hover());
        let mut ui = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::centered_and_justified(egui::Direction::TopDown)));
        if loading {
            ui.spinner();
        } else if error.is_none() {
            ui.add(Label::new(RichText::new("No pinned messages yet").color(pal.muted)).selectable(false));
        }
    }
    let mut jump = None;
    for i in 0..n {
        let Some(msg) = cx.state.chat.pins.as_ref().and_then(|v| v.items.get(i)).map(|p| p.message.clone()) else { break };
        let card = message_card(ui, cx, pal, guild_id, &msg, &[]);
        if card.clicked() {
            jump = Some(msg.id);
        }
        if ui.rect_contains_pointer(card.rect) {
            let bar = Rect::from_min_max(card.rect.left_top() + vec2(0.0, 8.0), card.rect.right_top() + vec2(-8.0, 32.0));
            let mut bar = ui.new_child(UiBuilder::new().max_rect(bar).layout(Layout::right_to_left(Align::Min)));
            bar.spacing_mut().item_spacing.x = 4.0;
            if can_unpin && icon_button(&mut bar, pal, "×", "Unpin", 24.0, false, pal.danger).clicked() {
                cx.cmd.send(Command::Chat(ChatCommand::UnpinMessage { channel_id: ch, message_id: msg.id }));
                // CHANNEL_PINS_UPDATE refetches; drop it from the list right away.
                if let Some(v) = cx.state.chat.pins.as_mut() {
                    v.items.retain(|p| p.message.id != msg.id);
                }
            }
            if small_button(&mut bar, pal, "Jump", pal.text).clicked() {
                jump = Some(msg.id);
            }
        }
    }
    if let Some(e) = error {
        ui.horizontal(|ui| {
            ui.label(RichText::new(e).color(pal.danger));
            if small_button(ui, pal, "Retry", pal.text).clicked() {
                // A stale list reloads from the start.
                let before = cx.state.chat.pins.as_ref().filter(|v| !v.stale).and_then(|v| v.items.last()).map(|p| p.pinned_at.clone());
                fetch(cx, ch, before);
            }
        });
    } else if has_more && n > 0 {
        // Scrolled to the end: the next page.
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
        if loading {
            ui.put(rect, egui::Spinner::new());
        } else if ui.is_rect_visible(rect) {
            let before = cx.state.chat.pins.as_ref().and_then(|v| v.items.last()).map(|p| p.pinned_at.clone());
            fetch(cx, ch, before);
        }
    }
    if let Some(mid) = jump {
        egui::Popup::close_id(ui.ctx(), popup_id());
        jump_to(ui.ctx(), cx, ch, mid);
    }
}

fn fetch(cx: &mut UiCx<'_>, ch: Id, before: Option<String>) {
    if let Some(v) = cx.state.chat.pins.as_mut() {
        v.loading = true;
        v.error = None;
    }
    cx.cmd.send(Command::Chat(ChatCommand::FetchPins { channel_id: ch, before }));
}

/// `RUSTCORD_MSG_DEMO=pins`: the popout open with a few cached messages as pins (no request).
pub(super) fn demo(ctx: &egui::Context, cx: &mut UiCx<'_>, ch: Id) {
    let pin = |m: &Message| PinnedMessage { pinned_at: m.timestamp.clone(), message: m.clone() };
    let items = cx.state.messages.get(&ch).map(|c| c.msgs.iter().rev().step_by(5).take(4).map(pin).collect()).unwrap_or_default();
    cx.state.chat.pins = Some(PinsView { channel_id: ch, items, ..Default::default() });
    egui::Popup::open_id(ctx, popup_id());
}
