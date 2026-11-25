//! Notifications > Sounds (docs/DESIGN.md 7.1 and 12): the master switch, one volume, and a
//! preview button and a switch per sound. Rustcord's own sounds (`src/sounds.rs`); nothing is
//! sent anywhere. A preview ignores the switches and plays at the volume shown.

use super::{Pal, hint, section, setting_row, switch, switch_row};
use crate::sounds::{self, Sound};
use crate::ui::UiCx;
use crate::ui::icons::Icon;
use crate::ui::messages::vector_button;
use egui::{Align, Layout, RichText, Slider, vec2};

/// True when `cx.settings` changed (the caller saves).
pub(super) fn show(ui: &mut egui::Ui, cx: &mut UiCx<'_>) -> bool {
    let pal = Pal::new(cx.theme);
    let deaf = sounds::deafened(cx.state, cx.view.self_deaf);
    section(ui, cx.theme, "Sounds");
    let s = &mut *cx.settings;
    let mut changed = switch_row(
        ui,
        cx.theme,
        "Play sounds",
        Some("Rustcord's own sounds. None play while you're deafened; on Do Not Disturb, messages and incoming calls stay silent."),
        &mut s.sounds.enabled,
    );
    let mut preview = None;
    let enabled = s.sounds.enabled;
    ui.add_enabled_ui(enabled, |ui| {
        let label = ui.label(RichText::new("Volume").font(crate::ui::semibold(pal.label())).color(pal.muted));
        ui.add_space(8.0 - ui.spacing().item_spacing.y);
        ui.spacing_mut().slider_width = 320.0;
        let r = ui.add(Slider::new(&mut s.sounds.volume, 0..=100).step_by(5.0).suffix(" %")).labelled_by(label.id);
        if super::app::released(&r) {
            changed = true;
            preview = Some(Sound::Message);
        }
        if deaf {
            hint(ui, cx.theme, "You're deafened, so no sounds play.");
        }
        for sound in Sound::ALL {
            let key = sound.key();
            let mut on = !s.sounds.off.iter().any(|k| k == key);
            let was = on;
            setting_row(ui, cx.theme, sound.label(), sound.meta(), vec2(76.0, 28.0), |ui| {
                ui.allocate_ui_with_layout(vec2(76.0, 28.0), Layout::left_to_right(Align::Center), |ui| {
                    // 28 + 8 + 40 = the control width, so rows do not creep to the right.
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let tip = format!("Play the {} sound", sound.label().to_lowercase());
                    ui.add_enabled_ui(!deaf, |ui| {
                        if vector_button(ui, &pal, Icon::Play, &tip, 28.0, false, pal.text).clicked() {
                            preview = Some(sound);
                        }
                    });
                    switch(ui, &pal, &mut on, sound.label());
                });
            });
            if on != was {
                changed = true;
                if on {
                    s.sounds.off.retain(|k| k != key);
                } else {
                    s.sounds.off.push(key.into());
                }
            }
        }
    });
    if let Some(sound) = preview {
        sounds::preview(sound, cx.settings);
    }
    changed
}
