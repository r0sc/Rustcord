//! Set Custom Status (`Modal::CustomStatus`: emoji, text, "Clear after") and the lower part of
//! the account popout (status picker, custom status row, Edit Profile). Both save through the
//! settings proto: `ChatCommand::SetStatus` (op 3), then `SetPersistentStatus` (the PATCH), and `SetCustomStatus`.
//! OWNERSHIP: user-settings engineer (docs/SOCIAL_UI_DESIGN.md).

use super::{Page, finish_op, op_line, op_state};
use crate::api::cdn;
use crate::backend::{AccountCommand, ChatCommand, Command, OpKind};
use crate::model::{CustomStatus, Emoji, UserStatus};
use crate::state::AppState;
use crate::state::account::OpState;
use crate::ui::icons::{self, Icon};
use crate::ui::messages::{Pal, icon_button};
use crate::ui::sidebar::{paint_status, presence_label, status_label, user_presence};
use crate::ui::{Modal, UiCx, dialog, dialog_buttons, pickers};
use chrono::{DateTime, Duration, TimeZone};
use egui::{Align2, FontId, Frame, Margin, Rect, Sense, TextEdit, Vec2, pos2, vec2};

/// Set Custom Status drafts.
#[derive(Default)]
pub(super) struct StatusForm {
    text: String,
    /// Unicode emoji (a saved custom emoji is kept as it was).
    emoji: Option<Emoji>,
    clear: Expiry,
    /// Emoji search in the little picker.
    query: String,
    /// Save was clicked: the modal closes once that write is done.
    saving: bool,
}

/// "Clear after" presets (Discord's order).
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum Expiry {
    #[default]
    Today,
    Hours4,
    Hour1,
    Min30,
    Never,
}

impl Expiry {
    const ALL: [Expiry; 5] = [Expiry::Today, Expiry::Hours4, Expiry::Hour1, Expiry::Min30, Expiry::Never];

    fn label(self) -> &'static str {
        match self {
            Expiry::Today => "Today",
            Expiry::Hours4 => "4 hours",
            Expiry::Hour1 => "1 hour",
            Expiry::Min30 => "30 minutes",
            Expiry::Never => "Don't clear",
        }
    }

    /// Unix ms when a status set at `now` ends ("Today": the next local midnight); None = never.
    fn ends<Tz: TimeZone>(self, now: DateTime<Tz>) -> Option<u64> {
        let after = |d: Duration| Some((now.clone() + d).timestamp_millis().max(0) as u64);
        match self {
            Expiry::Today => {
                let midnight = now.date_naive().succ_opt()?.and_hms_opt(0, 0, 0)?;
                // A skipped midnight (DST): a day from now.
                match now.timezone().from_local_datetime(&midnight).earliest() {
                    Some(t) => Some(t.timestamp_millis().max(0) as u64),
                    None => after(Duration::hours(24)),
                }
            }
            Expiry::Hours4 => after(Duration::hours(4)),
            Expiry::Hour1 => after(Duration::hours(1)),
            Expiry::Min30 => after(Duration::minutes(30)),
            Expiry::Never => None,
        }
    }
}

fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

/// My custom status as saved, hidden once expired; before READY's settings, the effective one.
/// With its expiry.
fn current(state: &AppState, now_ms: u64) -> (Option<&CustomStatus>, Option<u64>) {
    let s = &state.account.settings;
    if !s.loaded {
        return (state.chat.my_custom_status.as_ref(), None);
    }
    let ends = s.status.custom_status_expires_at_ms;
    if ends.is_some_and(|e| e <= now_ms) {
        return (None, None);
    }
    (s.status.custom_status.as_ref(), ends)
}

/// A status emoji centred at `c`: a custom one's image, a unicode one the font has, else ☺.
fn paint_emoji(ui: &mut egui::Ui, images: &mut crate::ui::images::ImageCache, e: Option<&Emoji>, c: egui::Pos2, fallback: egui::Color32) {
    let font = FontId::proportional(18.0);
    match e {
        Some(Emoji { id: Some(id), animated, .. }) => {
            if let Some(tex) = images.get(ui.ctx(), &cdn::emoji_url(*id, *animated, 48), 48, false) {
                egui::Image::from_texture(tex).paint_at(ui, Rect::from_center_size(c, Vec2::splat(20.0)));
            }
        }
        Some(Emoji { name: Some(n), .. }) if pickers::drawable(ui.ctx(), n) => {
            ui.painter().text(c, Align2::CENTER_CENTER, n, font, ui.visuals().text_color());
        }
        _ => {
            icons::paint(ui.painter(), Icon::Smile, Rect::from_center_size(c, Vec2::splat(20.0)), fallback, egui::Color32::TRANSPARENT);
        }
    }
}

/// Opens Set Custom Status (account popout), prefilled with the current one.
pub(super) fn open_custom_status(cx: &mut UiCx<'_>) {
    let (cur, ends) = current(cx.state, now_ms());
    let f = &mut cx.view.user_settings_ui.status;
    f.text = cur.and_then(|c| c.text.clone()).unwrap_or_default();
    f.emoji = cur.and_then(|c| c.emoji.clone());
    f.clear = if cur.is_some() && ends.is_none() { Expiry::Never } else { Expiry::Today };
    f.query.clear();
    f.saving = false;
    cx.view.profile_card = None;
    cx.view.modal = Some(Modal::CustomStatus);
}

/// `Modal::CustomStatus`: emoji button + text (128) + clear ×, "Clear after", Save. Saving an
/// empty status clears it. Closes once the write is done; its error stays in the dialog.
pub fn custom_status_modal(ctx: &egui::Context, cx: &mut UiCx<'_>) {
    let state = op_state(cx, OpKind::SetCustomStatus).filter(|_| cx.view.user_settings_ui.status.saving);
    if matches!(state, Some(OpState::Done(_))) {
        close(cx);
        return;
    }
    let pending = state == Some(OpState::Pending);
    let theme = &*cx.theme;
    let pal = Pal::new(theme);
    let (f, frequent) = (&mut cx.view.user_settings_ui.status, &cx.settings.frequent_emoji);
    let images = &mut *cx.images;
    let resp = dialog(ctx, theme, "rc_custom_status", 442.0, "Set a custom status", |ui| {
        ui.add_enabled_ui(!pending, |ui| {
            // An input (§9): the well fill and a 1 px `edge` border; focused, the 2 px focus ring
            // replaces the border, as on egui-framed fields.
            let well = theme.colors.code_background.to_color32();
            let edit_id = ui.make_persistent_id("rc_custom_status_text");
            let focused = ui.memory(|m| m.has_focus(edit_id));
            let field = Frame::new().fill(well).stroke(egui::Stroke::new(1.0, pal.tk.edge)).corner_radius(pal.radius).inner_margin(Margin::symmetric(6, 4)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let button = icon_button(ui, &pal, "", "Choose an emoji", 28.0, f.emoji.is_some(), pal.text);
                    paint_emoji(ui, images, f.emoji.as_ref(), button.rect.center(), if button.hovered() { pal.text } else { pal.muted });
                    egui::Popup::menu(&button).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                        if let Some(e) = emoji_grid(ui, &pal, &mut f.query, frequent) {
                            f.emoji = Some(Emoji { id: None, name: Some(e), animated: false });
                            ui.close();
                        }
                    });
                    let set = !f.text.is_empty() || f.emoji.is_some();
                    let w = ui.available_width() - if set { 34.0 } else { 0.0 };
                    let edit = TextEdit::singleline(&mut f.text).id(edit_id).char_limit(128).hint_text("What's on your mind?").frame(Frame::NONE).desired_width(w);
                    ui.add(edit);
                    if set && super::super::messages::vector_button(ui, &pal, Icon::Close, "Clear status", 28.0, false, pal.text).clicked() {
                        (f.text, f.emoji) = (String::new(), None);
                    }
                });
            });
            if focused {
                ui.painter().rect_stroke(field.response.rect, pal.radius, egui::Stroke::new(2.0, pal.tk.focus), egui::StrokeKind::Inside);
            }
            ui.add_space(4.0);
            super::field_label(ui, &pal, "Clear after");
            egui::ComboBox::from_id_salt("rc_status_clear").width(ui.available_width()).selected_text(f.clear.label()).icon(super::combo_icon).show_ui(ui, |ui| {
                for e in Expiry::ALL {
                    ui.selectable_value(&mut f.clear, e, e.label());
                }
            });
        });
        op_line(ui, &pal, state.as_ref(), "Saving…");
        dialog_buttons(ui, theme, if pending { "Saving…" } else { "Save" }, false, !pending)
    });
    match resp.inner {
        Some(true) => {
            let f = &cx.view.user_settings_ui.status;
            let text = f.text.trim();
            let text = (!text.is_empty()).then(|| text.to_owned());
            let set = text.is_some() || f.emoji.is_some();
            let expires_at_ms = if set { f.clear.ends(chrono::Local::now()) } else { None };
            let cmd = AccountCommand::SetCustomStatus { text, emoji: f.emoji.clone(), expires_at_ms };
            if cx.state.account.begin(cmd.op()) {
                cx.cmd.send(Command::Account(cmd));
                cx.view.user_settings_ui.status.saving = true;
            }
        }
        Some(false) => close(cx),
        None if resp.should_close() => close(cx),
        None => {}
    }
}

fn close(cx: &mut UiCx<'_>) {
    cx.view.modal = None;
    cx.view.user_settings_ui.status.saving = false;
    finish_op(cx, OpKind::SetCustomStatus);
}

/// A small emoji chooser: search, else my frequently used plus the first smileys. Returns the
/// clicked emoji.
fn emoji_grid(ui: &mut egui::Ui, pal: &Pal, query: &mut String, frequent: &[(String, u32)]) -> Option<String> {
    const MAX: usize = 48;
    ui.set_width(8.0 * 36.0);
    ui.add(TextEdit::singleline(query).hint_text("Find an emoji").desired_width(f32::INFINITY)).request_focus();
    let ctx = ui.ctx().clone();
    let mut list: Vec<&str> = Vec::with_capacity(MAX);
    let mut push = |s: &'static str| {
        if list.len() < MAX && !list.contains(&s) && pickers::drawable(&ctx, s) {
            list.push(s);
        }
    };
    if query.trim().is_empty() {
        // `frequent_emoji` holds owned strings: map them to the static table entries.
        for (s, _) in frequent.iter().filter(|(s, _)| !s.starts_with('<')) {
            if let Some(e) = emojis::get(s) {
                push(e.as_str());
            }
        }
        for e in crate::emoji::category_emojis(crate::emoji::CATEGORIES[0].1) {
            push(e.as_str());
        }
    } else {
        for (_, e) in crate::emoji::search(query, MAX) {
            push(e.as_str());
        }
    }
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for s in list {
            if icon_button(ui, pal, s, s, 32.0, true, pal.text).clicked() {
                picked = Some(s.to_owned());
            }
        }
    });
    picked
}

/// A submenu flyout beside `row`: opens on hover or click, closes once the pointer leaves the
/// row and the flyout (the account popout's status and pause rows).
fn flyout(ctx: &egui::Context, id: egui::Id, row: Rect, resp: &egui::Response, layer: egui::LayerId, add: impl FnOnce(&mut egui::Ui)) {
    let open = egui::Popup::is_id_open(ctx, id);
    let near = ctx.memory(|m| m.area_rect(id)).map_or(row, |r| row.union(r)).expand(6.0);
    let away = open && ctx.pointer_hover_pos().is_some_and(|p| !near.contains(p));
    let cmd = if resp.hovered() || resp.clicked() { Some(egui::SetOpenCommand::Bool(true)) } else { away.then_some(egui::SetOpenCommand::Bool(false)) };
    egui::Popup::new(id, ctx.clone(), row, layer)
        .kind(egui::PopupKind::Menu)
        .style(egui::containers::menu::menu_style)
        .open_memory(cmd)
        .align(egui::RectAlign::RIGHT_START)
        .align_alternatives(&[egui::RectAlign::LEFT_START])
        .gap(16.0)
        .show(|ui| {
            ui.set_width(188.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui);
        });
}

/// Lower part of the account popout (my card above the user panel): one row with my status
/// (icon, name, ▸) whose flyout on the right lists the statuses I can pick (mine checked; it
/// opens on hover or click; a pick is saved with `SetPersistentStatus` and closes the popout),
/// then my custom status (click: edit; ×: clear) or Set Custom Status, and Edit Profile; under
/// it a second group with Copy User ID (clipboard only).
pub(in crate::ui) fn account_popout(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal) {
    let current_status = cx.state.chat.my_status;
    let c = cx.theme.colors.clone();
    let me = cx.state.me.as_ref().map(|u| u.id).unwrap_or_default();
    let (presence, label) = match current_status {
        Some(s) => (s.presence(), status_label(s)),
        None => (user_presence(cx.state, me), presence_label(user_presence(cx.state, me))),
    };
    let (custom, custom_emoji) = match current(cx.state, now_ms()).0 {
        Some(s) => (Some(s.text.clone().unwrap_or_else(|| "Edit custom status".into())), s.emoji.clone()),
        None => (None, None),
    };
    let ctx = ui.ctx().clone();
    let flyout_id = egui::Id::unique("rc_status_flyout");
    let mut picked = None;
    // The pause flyout's pick: Some(Some(t)) pause until t, Some(None) resume.
    let mut pause_pick: Option<Option<DateTime<chrono::Local>>> = None;
    let now = chrono::Local::now();
    let (mut edit_status, mut clear_status, mut edit, mut copy_id) = (false, false, false, false);
    // A 32 px menu row (§9 menus, ui 14/500): hover fill concentric with its group, the label at
    // x 32; `open` keeps the fill (its flyout is open).
    let item_r = pal.radius.saturating_sub(4).max(2);
    let row = |ui: &mut egui::Ui, label: &str, open: bool, right: f32| {
        let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
        let fill = if open || resp.hovered() || resp.has_focus() { pal.bg.blend(pal.tk.hover) } else { pal.bg };
        ui.painter().rect_filled(row, item_r, fill);
        let g = crate::ui::one_line(ui, label, FontId::new(pal.ui(), crate::theme::medium_family()), pal.text, row.width() - 32.0 - right);
        ui.painter().galley(pos2(row.left() + 32.0, row.center().y - g.size().y / 2.0), g, pal.text);
        (row, fill, resp)
    };
    Frame::new().inner_margin(Margin { left: 12, right: 12, top: 0, bottom: 12 }).show(ui, |ui| {
        Frame::new().fill(pal.bg).corner_radius(pal.radius).inner_margin(4).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let open = egui::Popup::is_id_open(&ctx, flyout_id);
            let (row_rect, fill, resp) = row(ui, label, open, 24.0);
            paint_status(ui.painter(), pos2(row_rect.left() + 16.0, row_rect.center().y), 5.0, presence, &c, fill);
            let arrow = Rect::from_center_size(pos2(row_rect.right() - 14.0, row_rect.center().y), Vec2::splat(14.0));
            ui.painter().add(egui::containers::menu::SubMenuButton::arrow_shape(arrow, pal.muted));
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Status: {label}")));
            flyout(&ctx, flyout_id, row_rect, &resp, ui.layer_id(), |ui| {
                for s in [UserStatus::Online, UserStatus::Idle, UserStatus::Dnd, UserStatus::Invisible] {
                    let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
                    let bg = ui.visuals().window_fill;
                    let fill = if resp.hovered() || resp.has_focus() { ui.visuals().widgets.hovered.weak_bg_fill } else { bg };
                    ui.painter().rect_filled(row, item_r, fill);
                    paint_status(ui.painter(), pos2(row.left() + 14.0, row.center().y), 5.0, s.presence(), &c, fill);
                    let label = status_label(s);
                    // The app's menu colours: `pal` may be a themed card's (cosmetics::theme_pal).
                    let text = ui.visuals().text_color();
                    ui.painter().text(pos2(row.left() + 30.0, row.center().y), Align2::LEFT_CENTER, label, FontId::new(pal.ui(), crate::theme::medium_family()), text);
                    if current_status == Some(s) {
                        icons::paint(ui.painter(), Icon::Check, Rect::from_center_size(pos2(row.right() - 18.0, row.center().y), Vec2::splat(16.0)), text, fill);
                    }
                    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, current_status == Some(s), label));
                    let resp = match s {
                        UserStatus::Invisible => resp.on_hover_text("You will appear offline"),
                        UserStatus::Dnd => resp.on_hover_text("You will not receive desktop notifications"),
                        _ => resp,
                    };
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        picked = Some(s);
                    }
                }
            });

            // Pause notifications: a flyout with the same choices as the tray.
            let (pause_label, quiet, paused) = {
                let q = &cx.settings.quiet_hours;
                match q.until(now) {
                    Some(t) if q.paused(now.timestamp()) => (format!("Paused until {}", crate::alerts::time_label(t, now)), true, true),
                    Some(t) => (format!("Quiet hours until {}", crate::alerts::time_label(t, now)), true, false),
                    None => ("Pause notifications".to_owned(), false, false),
                }
            };
            let pause_id = egui::Id::unique("rc_pause_flyout");
            let open = egui::Popup::is_id_open(&ctx, pause_id);
            let (r, fill, resp) = row(ui, &pause_label, open, 24.0);
            let bell = if quiet { Icon::BellOff } else { Icon::Bell };
            icons::paint(ui.painter(), bell, Rect::from_center_size(pos2(r.left() + 16.0, r.center().y), Vec2::splat(18.0)), pal.muted, fill);
            let arrow = Rect::from_center_size(pos2(r.right() - 14.0, r.center().y), Vec2::splat(14.0));
            ui.painter().add(egui::containers::menu::SubMenuButton::arrow_shape(arrow, pal.muted));
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, pause_label.clone()));
            flyout(&ctx, pause_id, r, &resp, ui.layer_id(), |ui| {
                let choices = [("For 1 hour", Some(now + chrono::TimeDelta::hours(1))), ("Until tomorrow", Some(crate::alerts::tomorrow_morning(now))), ("Resume notifications", None)];
                for (label, until) in choices.into_iter().filter(|c| paused || c.1.is_some()) {
                    let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
                    let fill = if resp.hovered() || resp.has_focus() { ui.visuals().widgets.hovered.weak_bg_fill } else { ui.visuals().window_fill };
                    ui.painter().rect_filled(row, item_r, fill);
                    ui.painter().text(pos2(row.left() + 12.0, row.center().y), Align2::LEFT_CENTER, label, FontId::new(pal.ui(), crate::theme::medium_family()), ui.visuals().text_color());
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::MenuItem, true, label));
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        pause_pick = Some(until);
                    }
                }
            });

            // My custom status (click edits it, × clears it), or Set custom status.
            let label = custom.as_deref().unwrap_or("Set custom status");
            let (r, fill, resp) = row(ui, label, false, if custom.is_some() { 24.0 } else { 0.0 });
            paint_emoji(ui, cx.images, custom_emoji.as_ref(), pos2(r.left() + 16.0, r.center().y), pal.muted);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, if custom.is_some() { "Edit custom status" } else { label }));
            edit_status = resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
            if custom.is_some() {
                // Registered after the row, so it takes the click.
                let x = Rect::from_center_size(pos2(r.right() - 14.0, r.center().y), Vec2::splat(20.0));
                let resp = ui.interact(x, ui.make_persistent_id("rc_clear_custom_status"), Sense::click());
                let color = if resp.hovered() { pal.text } else { pal.muted };
                if resp.hovered() {
                    ui.painter().circle_filled(x.center(), 10.0, fill.blend(pal.tk.hover));
                }
                icons::paint(ui.painter(), Icon::Close, Rect::from_center_size(x.center(), Vec2::splat(16.0)), color, egui::Color32::TRANSPARENT);
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Clear custom status"));
                clear_status = resp.on_hover_text("Clear custom status").on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
            }

            let (r, _, resp) = row(ui, "Edit profile", false, 0.0);
            icons::paint(ui.painter(), Icon::Edit, Rect::from_center_size(pos2(r.left() + 16.0, r.center().y), Vec2::splat(18.0)), pal.muted, egui::Color32::TRANSPARENT);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Edit profile"));
            edit = resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
        });
        // Discord's second group. shortcut: no Switch Accounts row until Rustcord has multi-account.
        ui.add_space(8.0);
        Frame::new().fill(pal.bg).corner_radius(pal.radius).inner_margin(4).show(ui, |ui| {
            let (r, fill, resp) = row(ui, "Copy user ID", false, 0.0);
            icons::paint(ui.painter(), Icon::IdCard, Rect::from_center_size(pos2(r.left() + 16.0, r.center().y), Vec2::splat(18.0)), pal.muted, fill);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Copy user ID"));
            copy_id = resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
        });
    });
    if copy_id {
        ui.ctx().copy_text(me.0.to_string());
        cx.view.toast("Copied your user ID".to_owned(), false);
    }
    if let Some(status) = picked {
        // Op 3 now, then the settings write (errors: toast); the proto codec is unit-tested
        // against the FEATURES_NOTES §3.1 vectors (api/settings_proto.rs `vectors_*`).
        cx.cmd.send(Command::Chat(ChatCommand::SetStatus { status, afk: false }));
        cx.cmd.send(Command::Account(AccountCommand::SetPersistentStatus { status }));
        cx.view.profile_card = None;
    }
    if let Some(until) = pause_pick {
        let toast = crate::alerts::pause(&mut cx.settings.quiet_hours, until, now);
        cx.view.toast(toast, false);
        if let Err(e) = cx.settings.save() {
            cx.view.toast(format!("Could not save settings: {e}"), true);
        }
        cx.view.profile_card = None;
    }
    if clear_status {
        cx.cmd.send(Command::Account(AccountCommand::SetCustomStatus { text: None, emoji: None, expires_at_ms: None }));
    } else if edit_status {
        open_custom_status(cx);
    }
    if edit {
        cx.view.profile_card = None;
        (cx.view.settings_open, cx.view.settings_page) = (true, Page::Profiles);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn expiry_presets() {
        let now = Utc.with_ymd_and_hms(2026, 11, 12, 21, 30, 0).unwrap();
        let ms = |t: DateTime<Utc>| t.timestamp_millis() as u64;
        assert_eq!(Expiry::Today.ends(now), Some(ms(Utc.with_ymd_and_hms(2026, 11, 13, 0, 0, 0).unwrap())));
        assert_eq!(Expiry::Hours4.ends(now), Some(ms(now) + 4 * 3_600_000));
        assert_eq!(Expiry::Hour1.ends(now), Some(ms(now) + 3_600_000));
        assert_eq!(Expiry::Min30.ends(now), Some(ms(now) + 1_800_000));
        assert_eq!(Expiry::Never.ends(now), None);
    }

    #[test]
    fn expired_custom_status_is_hidden() {
        let mut s = AppState::default();
        s.chat.my_custom_status = Some(CustomStatus { text: Some("live".into()), emoji: None });
        assert_eq!(current(&s, 10).0.and_then(|c| c.text.as_deref()), Some("live"), "before READY: the effective one");
        s.account.settings.loaded = true;
        s.account.settings.status.custom_status = Some(CustomStatus { text: Some("saved".into()), emoji: None });
        s.account.settings.status.custom_status_expires_at_ms = Some(100);
        assert_eq!(current(&s, 99), (s.account.settings.status.custom_status.as_ref(), Some(100)));
        assert_eq!(current(&s, 100), (None, None));
    }
}
