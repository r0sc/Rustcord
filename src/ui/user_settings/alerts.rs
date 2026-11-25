//! Notifications > Quiet hours (and, below it, Keyword alerts): Rustcord's own sections for this
//! device (`crate::alerts`, docs/FEATURES_NOTES.md §2.7). Nothing is sent anywhere.

use super::{Pal, combo_icon, section, switch_row};
use crate::alerts::hhmm;
use crate::ui::UiCx;
use egui::{ComboBox, RichText, WidgetInfo};

/// True when `cx.settings` changed (the caller saves).
pub(super) fn show(ui: &mut egui::Ui, cx: &mut UiCx<'_>) -> bool {
    quiet_hours(ui, cx)
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
            crate::ui::server_menu::check_row(ui, &pal, day, &mut on, true);
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
