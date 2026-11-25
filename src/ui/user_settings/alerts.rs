//! Notifications > Quiet hours (and, below it, Keyword alerts): Rustcord's own sections for this
//! device (`crate::alerts`, docs/FEATURES_NOTES.md §2.7). Nothing is sent anywhere.

use super::{Pal, combo_icon, hint, section, setting_row, switch_row};
use crate::alerts::{MAX_KEYWORD_CHARS, MAX_PLACES, hhmm};
use crate::model::Id;
use crate::state::{AppState, SidebarItem};
use crate::ui::UiCx;
use crate::ui::messages::{self, Btn, icon_button};
use crate::ui::server_menu::check_row;
use egui::{ComboBox, Key, RichText, TextEdit, WidgetInfo, vec2};

/// True when `cx.settings` changed (the caller saves).
pub(super) fn show(ui: &mut egui::Ui, cx: &mut UiCx<'_>) -> bool {
    quiet_hours(ui, cx) | keyword_alerts(ui, cx)
}

fn quiet_hours(ui: &mut egui::Ui, cx: &mut UiCx<'_>) -> bool {
    let pal = Pal::new(cx.theme);
    section(ui, cx.theme, "Quiet hours");
    let q = &mut cx.settings.quiet_hours;
    let mut changed = switch_row(
        ui,
        cx.theme,
        "Quiet hours",
        Some("Rustcord stays silent at these times: no notifications or sounds. Your status doesn't change."),
        &mut q.enabled,
    );
    if !q.enabled {
        return changed;
    }
    ui.horizontal(|ui| {
        let label = |ui: &mut egui::Ui, text: &str| ui.label(RichText::new(text).size(pal.ui()).color(pal.muted));
        label(ui, "From");
        changed |= time_combo(ui, "rc_quiet_start", "Quiet hours start", &mut q.start);
        label(ui, "to");
        changed |= time_combo(ui, "rc_quiet_end", "Quiet hours end", &mut q.end);
    });
    ui.horizontal_wrapped(|ui| {
        for (i, day) in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].into_iter().enumerate() {
            let mut on = q.days & (1 << i) != 0;
            check_row(ui, &pal, day, &mut on, true);
            let days = if on { q.days | (1 << i) } else { q.days & !(1 << i) };
            changed |= days != q.days;
            q.days = days;
        }
    });
    let error = if q.start == q.end {
        Some("Pick an end time different from the start.")
    } else if q.days & 0x7F == 0 {
        Some("Pick at least one day.")
    } else {
        None
    };
    if let Some(text) = error {
        ui.label(RichText::new(text).size(pal.meta()).color(pal.danger));
    }
    changed
}

/// Drafts of the Keyword alerts section (the word being typed, the place being picked).
#[derive(Default)]
pub(super) struct Form {
    draft: String,
    error: Option<&'static str>,
    server: Option<Id>,
    channel: Option<Id>,
}

fn keyword_alerts(ui: &mut egui::Ui, cx: &mut UiCx<'_>) -> bool {
    let pal = Pal::new(cx.theme);
    section(ui, cx.theme, "Keyword alerts");
    hint(
        ui,
        cx.theme,
        "Get notified when these words come up in your servers, even in muted channels. Very large servers send messages only while you have them open.",
    );
    ui.add_space(8.0);
    let form = &mut cx.view.user_settings_ui.alerts;
    let a = &mut cx.settings.keyword_alerts;
    let mut changed = false;
    ui.horizontal(|ui| {
        let r = ui.add(TextEdit::singleline(&mut form.draft).hint_text("Add a word or phrase").char_limit(MAX_KEYWORD_CHARS).desired_width(320.0));
        if r.changed() {
            form.error = None;
        }
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let click = messages::button(ui, &pal, Btn::Secondary, "Add", !form.draft.trim().is_empty()).clicked();
        if enter || click {
            match a.add(&form.draft) {
                Ok(()) => {
                    form.draft.clear();
                    changed = true;
                }
                Err(e) => form.error = Some(e),
            }
            if enter {
                r.request_focus();
            }
        }
    });
    if let Some(text) = form.error {
        ui.label(RichText::new(text).size(pal.meta()).color(pal.danger));
    }
    let mut remove = None;
    for (i, k) in a.keywords.iter_mut().enumerate() {
        let (text, whole) = (k.text.clone(), &mut k.whole_word);
        let before = *whole;
        setting_row(ui, cx.theme, &text, None, vec2(160.0, 32.0), |ui| {
            // Right to left: the × is outermost.
            if icon_button(ui, &pal, "×", "Remove keyword", 28.0, false, pal.danger).clicked() {
                remove = Some(i);
            }
            check_row(ui, &pal, "Whole word", whole, true);
        });
        changed |= *whole != before;
    }
    if let Some(i) = remove {
        a.keywords.remove(i);
        changed = true;
    }
    if a.keywords.is_empty() {
        return changed;
    }
    changed |= switch_row(ui, cx.theme, "All servers", Some("Off: only the servers and channels you add below."), &mut a.everywhere);
    if a.everywhere {
        return changed;
    }
    let labels: Vec<String> = a.places.iter().map(|&p| place_label(cx.state, p)).collect();
    let mut remove = None;
    for (i, label) in labels.iter().enumerate() {
        setting_row(ui, cx.theme, label, None, vec2(28.0, 32.0), |ui| {
            if icon_button(ui, &pal, "×", "Remove place", 28.0, false, pal.danger).clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        a.places.remove(i);
        changed = true;
    }
    if a.places.is_empty() {
        hint(ui, cx.theme, "Add a server or channel, or turn on All servers.");
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let before = form.server;
        let name = |id: Id| cx.state.guild(id).map_or("Server", |g| g.name.as_str());
        let r = ComboBox::from_id_salt("rc_kw_server")
            .width(200.0)
            .selected_text(form.server.map_or("Choose a server", name))
            .icon(combo_icon)
            .show_ui(ui, |ui| {
                for g in &cx.state.guilds {
                    ui.selectable_value(&mut form.server, Some(g.id), &g.name);
                }
            });
        r.response.widget_info(|| WidgetInfo::labeled(egui::Role::ComboBox, ui.is_enabled(), "Server"));
        if form.server != before {
            form.channel = None;
        }
        ui.add_enabled_ui(form.server.is_some(), |ui| {
            let text = form.channel.map_or_else(|| "Whole server".to_owned(), |c| cx.state.channel_name(c));
            let r = ComboBox::from_id_salt("rc_kw_channel").width(200.0).selected_text(text).icon(combo_icon).show_ui(ui, |ui| {
                ui.selectable_value(&mut form.channel, None, "Whole server");
                for item in form.server.map(|g| cx.state.sidebar_channels(g)).unwrap_or_default() {
                    if let SidebarItem::Channel { id } = item {
                        ui.selectable_value(&mut form.channel, Some(id), cx.state.channel_name(id));
                    }
                }
            });
            r.response.widget_info(|| WidgetInfo::labeled(egui::Role::ComboBox, ui.is_enabled(), "Channel"));
        });
        let target = form.channel.or(form.server);
        let can = target.is_some_and(|t| a.places.len() < MAX_PLACES && !a.places.contains(&t));
        if messages::button(ui, &pal, Btn::Secondary, "Add", can).clicked()
            && let Some(t) = target
        {
            a.places.push(t);
            changed = true;
        }
    });
    changed
}

/// A place's row title: a server's name, or "#channel in Server".
fn place_label(state: &AppState, id: Id) -> String {
    if let Some(g) = state.guild(id) {
        return g.name.clone();
    }
    let Some(ch) = state.channels.get(&id) else { return "Server or channel not found".into() };
    let guild = ch.guild_id.and_then(|g| state.guild(g)).map_or("a server", |g| g.name.as_str());
    format!("{}{} in {guild}", messages::channel_prefix(ch.kind), state.channel_name(id))
}

/// A time of day in 30-minute steps.
fn time_combo(ui: &mut egui::Ui, salt: &str, label: &str, value: &mut u16) -> bool {
    let before = *value;
    let r = ComboBox::from_id_salt(salt).width(96.0).selected_text(hhmm(*value)).icon(combo_icon).show_ui(ui, |ui| {
        for m in (0..48).map(|i| i * 30) {
            ui.selectable_value(value, m, hhmm(m));
        }
    });
    r.response.widget_info(|| WidgetInfo::labeled(egui::Role::ComboBox, ui.is_enabled(), label));
    *value != before
}
