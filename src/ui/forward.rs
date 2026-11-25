//! Forwarded messages (docs/PROTOCOL_NOTES.md, Forward).
//! - A received forward is drawn by `messages::forwarded`; `demo` is its test hook.
//! - The Forward action (hover toolbar, message menu) opens `modal`: the message, a search field
//!   and the channels and DMs I can send in (`AppState::forward_targets`); one pick, one
//!   `ChatCommand::Forward`. `answered` ends the flow with a toast (or the error in the dialog).
//!
//! Left out on purpose: text sent along, several destinations, partial forwards.
//! OWNERSHIP: ui-messages.

use super::icons;
use super::messages::{Pal, focus_ring, paint_radio, preview_text};
use super::{Modal, UiCx, ViewState, bold, composer, dialog, dialog_buttons, one_line};
use crate::backend::{ChatCommand, Command};
use crate::model::{Id, Message, MessageReference, MessageSnapshot, User, channel_type};
use crate::state::AppState;
use egui::{FontId, Frame, Label, Margin, Rect, RichText, ScrollArea, Sense, Shape, TextEdit, Vec2, pos2, vec2};

/// The dialog's form state.
#[derive(Default)]
pub struct ForwardUi {
    query: String,
    dest: Option<Id>,
    busy: bool,
    error: Option<String>,
}

/// Opens the Forward dialog for a message of `channel_id`.
pub(super) fn open(cx: &mut UiCx<'_>, channel_id: Id, message_id: Id) {
    cx.view.forward_ui = ForwardUi::default();
    cx.view.modal = Some(Modal::Forward { channel_id, message_id });
}

fn close(cx: &mut UiCx<'_>) {
    if !cx.view.forward_ui.busy {
        cx.view.forward_ui = ForwardUi::default();
    }
    cx.view.modal = None;
}

/// `Modal::Forward`.
pub fn modal(ctx: &egui::Context, cx: &mut UiCx<'_>, channel_id: Id, message_id: Id) {
    let pal = Pal::new(cx.theme);
    let theme = cx.theme.clone();
    let preview = cx.state.messages.get(&channel_id).and_then(|c| c.msgs.iter().find(|m| m.id == message_id)).map(|m| (m.author.display_name().to_owned(), preview_text(cx.state, m)));
    let targets = cx.state.forward_targets(&cx.view.forward_ui.query, 50);
    let (dest, busy) = (cx.view.forward_ui.dest, cx.view.forward_ui.busy);
    let resp = dialog(ctx, &theme, "rc_forward", 440.0, "Forward to", |ui| {
        if let Some((author, text)) = &preview {
            Frame::new().fill(pal.bg).corner_radius(pal.card_radius()).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(Label::new(RichText::new(author).font(bold(pal.meta())).color(pal.text)).selectable(false));
                ui.add(Label::new(RichText::new(text).size(pal.meta()).color(pal.muted)).wrap().selectable(false));
            });
        }
        let edit = ui.add(TextEdit::singleline(&mut cx.view.forward_ui.query).hint_text("Search channels and DMs").desired_width(f32::INFINITY));
        if ui.memory(|m| m.focused().is_none()) {
            edit.request_focus();
        }
        let list_h = (ctx.content_rect().height() - 280.0).clamp(96.0, 280.0);
        ScrollArea::vertical().max_height(list_h).min_scrolled_height(list_h).auto_shrink([false, true]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if targets.is_empty() {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| ui.label(RichText::new("No channels match.").color(pal.muted)));
                ui.add_space(24.0);
            }
            for &id in &targets {
                if target_row(ui, cx, &pal, id, dest == Some(id)) {
                    cx.view.forward_ui.dest = Some(id);
                }
            }
        });
        if let Some(e) = &cx.view.forward_ui.error {
            ui.label(RichText::new(e).size(pal.meta()).color(pal.danger));
        }
        dialog_buttons(ui, &theme, if busy { "Forwarding…" } else { "Forward" }, false, dest.is_some() && !busy)
    });
    match resp.inner {
        Some(true) => {
            let ui = &mut cx.view.forward_ui;
            if let Some(dest) = ui.dest.filter(|_| !ui.busy) {
                (ui.busy, ui.error) = (true, None);
                let guild_id = cx.state.channels.get(&channel_id).and_then(|c| c.guild_id);
                cx.cmd.send(Command::Chat(ChatCommand::Forward { channel_id, guild_id, message_id, dest, nonce: composer::new_nonce() }));
            }
        }
        Some(false) => close(cx),
        None if resp.should_close() => close(cx),
        None => {}
    }
}

/// One channel or DM: its icon or avatar, name, where it is, and a radio. True when clicked.
fn target_row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, id: Id, picked: bool) -> bool {
    let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
    let name = cx.state.channel_name(id);
    let place = place(cx.state, id);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, picked, format!("{name}, {place}")));
    if !ui.is_rect_visible(row) {
        return resp.clicked();
    }
    let fill = if picked { Some(pal.tk.selected) } else { resp.hovered().then(|| pal.tint()) };
    if let Some(fill) = fill {
        ui.painter().add(Shape::rect_filled(row, pal.radius, fill));
    }
    if resp.has_focus() {
        focus_ring(ui, pal, row.shrink(4.0), f32::from(pal.radius));
    }
    let slot = Rect::from_min_size(pos2(row.left() + 8.0, row.center().y - 16.0), Vec2::splat(32.0));
    let dm = cx.state.channels.get(&id).is_some_and(|c| c.is_dm_like());
    if dm {
        super::sidebar::dm_avatar_at(ui, cx, id, slot);
    } else if let Some(c) = cx.state.channels.get(&id) {
        icons::paint_channel(ui.painter(), icons::of(c, cx.state), Rect::from_center_size(slot.center(), Vec2::splat(20.0)), pal.muted, fill.unwrap_or(ui.visuals().window_fill));
    }
    let left = slot.right() + 12.0;
    let right = row.right() - 40.0;
    let g = one_line(ui, name.as_str(), FontId::proportional(pal.body()), pal.text, right - left);
    let w = g.size().x;
    ui.painter().galley(pos2(left, row.center().y - g.size().y / 2.0), g, pal.text);
    let g = one_line(ui, place, FontId::proportional(pal.meta()), pal.muted, right - left - w - 8.0);
    ui.painter().galley(pos2(left + w + 8.0, row.center().y - g.size().y / 2.0), g, pal.muted);
    paint_radio(ui.painter(), pal, Rect::from_center_size(pos2(row.right() - 20.0, row.center().y), Vec2::splat(20.0)), picked);
    resp.clicked()
}

/// "Rust Workshop", "Direct message" or "Group".
fn place(state: &AppState, id: Id) -> String {
    let Some(c) = state.channels.get(&id) else { return String::new() };
    match c.kind {
        channel_type::DM => "Direct message".into(),
        channel_type::GROUP_DM => "Group".into(),
        _ => c.guild_id.and_then(|g| state.guild(g)).map(|g| g.name.clone()).unwrap_or_default(),
    }
}

/// `ChatEvent::Forwarded`: a toast, and the dialog closes (or shows the error) when it was
/// waiting for this answer.
pub fn answered(view: &mut ViewState, state: &AppState, dest: Id, result: Result<(), String>) {
    let waiting = matches!(view.modal, Some(Modal::Forward { .. })) && view.forward_ui.busy;
    match result {
        Ok(()) => {
            let name = state.channel_name(dest);
            let to = if state.channels.get(&dest).is_some_and(|c| c.is_dm_like()) { name } else { format!("#{name}") };
            view.toast(format!("Forwarded to {to}"), false);
            if waiting {
                view.modal = None;
                view.forward_ui = ForwardUi::default();
            }
        }
        Err(e) => {
            let text = format!("Could not forward the message: {e}");
            if waiting {
                view.forward_ui.busy = false;
                view.forward_ui.error = Some(text);
            } else {
                view.toast(text, true);
            }
        }
    }
}

/// A cached user other than me (the lowest id, so screenshots stay the same between runs).
pub(super) fn demo_author(state: &AppState) -> User {
    let me = state.me.as_ref().map(|u| u.id);
    state.users.values().filter(|u| Some(u.id) != me).min_by_key(|u| u.id).cloned().unwrap_or_default()
}

/// `RUSTCORD_MSG_DEMO=forward`: a forward of the message in the middle of the loaded history.
pub(super) fn demo(cx: &mut UiCx<'_>, ch: Id, demo: impl Fn(&str) -> bool) {
    if !demo("forward") {
        return;
    }
    let author = demo_author(cx.state);
    let guild_id = cx.state.channels.get(&ch).and_then(|c| c.guild_id);
    let Some(list) = cx.state.messages.get_mut(&ch) else { return };
    let (Some(src), Some(newest)) = (list.msgs.get(list.msgs.len() / 2).cloned(), list.msgs.last()) else { return };
    let snap = Message {
        content: src.content.clone(),
        attachments: src.attachments.clone(),
        embeds: src.embeds.clone(),
        mentions: src.mentions.clone(),
        timestamp: src.timestamp.clone(),
        ..Message::default()
    };
    let forward = Message {
        id: Id(newest.id.0 + (1 << 22)),
        channel_id: ch,
        guild_id,
        author,
        timestamp: newest.timestamp.clone(),
        flags: 1 << 14,
        message_reference: Some(MessageReference { message_id: Some(src.id), channel_id: Some(ch), guild_id, kind: 1 }),
        message_snapshots: vec![MessageSnapshot { message: snap }],
        ..Message::default()
    };
    list.msgs.push(forward);
}
