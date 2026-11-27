//! Notifications > Sounds (docs/DESIGN.md 7.1 and 12): the master switch, one volume, and a
//! preview button and a switch per sound. Rustcord's own sounds (`src/sounds.rs`); nothing is
//! sent anywhere. A preview ignores the switches and plays at the volume shown. Below it Custom
//! sounds: the user's own WAV / Ogg Opus file for a message, mention or incoming call, everywhere
//! or for one server, channel or DM. The files are read locally and never copied or uploaded.

use super::{Pal, combo_icon, hint, section, setting_row, switch, switch_row};
use crate::config::MAX_SOUND_PLACES;
use crate::model::Id;
use crate::sounds::{self, Sound};
use crate::ui::UiCx;
use crate::ui::icons::Icon;
use crate::ui::messages::{Btn, button, vector_button};
use egui::{Align, ComboBox, Layout, RichText, Slider, vec2};

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
    changed | custom(ui, cx)
}

/// What a row of Custom sounds asked for this pass.
enum Act {
    Play(Sound),
    Choose(Sound),
    Remove(Sound),
}

/// Notifications > Custom sounds. True when `cx.settings` changed.
fn custom(ui: &mut egui::Ui, cx: &mut UiCx<'_>) -> bool {
    // Test hook `RUSTCORD_NAV_DEMO=sound-place:<id>`: acts like the menu item, once.
    static DEMO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if let Some(id) = crate::ui::sidebar::nav_demo_value("sound-place").and_then(|v| v.parse().ok())
        && !DEMO.swap(true, std::sync::atomic::Ordering::Relaxed)
    {
        let u = &mut cx.view.user_settings_ui;
        (u.sound_place, u.sound_scroll) = (Some(Id(id)), true);
    }
    if std::mem::take(&mut cx.view.user_settings_ui.sound_scroll) {
        ui.scroll_to_cursor(Some(Align::TOP));
    }
    section(ui, cx.theme, "Custom sounds");
    hint(ui, cx.theme, "Your own WAV or Ogg Opus files. Only the first 10 seconds play. Where there's none, Rustcord's sound plays.");
    let deaf = sounds::deafened(cx.state, cx.view.self_deaf);
    let mut place = cx.view.user_settings_ui.sound_place;
    // "Where": everywhere, then every place with a file and the one a menu opened, by name.
    let mut ids: Vec<Id> = cx.settings.sounds.places.keys().copied().chain(place).collect();
    ids.sort();
    ids.dedup();
    let mut named: Vec<(Id, String)> = ids.into_iter().map(|id| (id, super::alerts::place_label(cx.state, id))).collect();
    named.sort_by(|a, b| a.1.cmp(&b.1));
    let shown = place.and_then(|p| named.iter().find(|n| n.0 == p)).map_or("Everywhere", |n| n.1.as_str());
    let height = ui.text_style_height(&egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.y;
    let theme = &*cx.theme;
    setting_row(ui, theme, "Where", Some("Right-click a server, channel or DM and pick Custom sounds to add it."), vec2(220.0, height), |ui| {
        ComboBox::from_id_salt("rc_sound_place").width(220.0).truncate().selected_text(shown).icon(combo_icon).show_ui(ui, |ui| {
            if ui.selectable_label(place.is_none(), "Everywhere").clicked() {
                place = None;
            }
            for (id, label) in &named {
                if ui.selectable_label(place == Some(*id), label).clicked() {
                    place = Some(*id);
                }
            }
        }).response.on_hover_text(shown);
    });
    cx.view.user_settings_ui.sound_place = place;
    // Servers and channels don't ring.
    let rings = place.is_none_or(|p| cx.state.channels.get(&p).is_some_and(|c| c.is_dm_like()));
    let full = place.is_some_and(|p| !cx.settings.sounds.places.contains_key(&p)) && cx.settings.sounds.places.len() >= MAX_SOUND_PLACES;
    let pal = Pal::new(cx.theme);
    let mut act = None;
    for sound in Sound::CUSTOM.into_iter().filter(|s| *s != Sound::IncomingRing || rings) {
        let files = match place {
            None => Some(&cx.settings.sounds.files),
            Some(p) => cx.settings.sounds.places.get(&p),
        };
        let own = files.and_then(|f| f.get(sound.key())).cloned();
        let meta = match &own {
            Some(p) => std::path::Path::new(p).file_name().and_then(|n| n.to_str()).unwrap_or(p).to_owned(),
            None if place.is_none() => "Rustcord's sound".to_owned(),
            None => "Same as everywhere".to_owned(),
        };
        setting_row(ui, theme, sound.label(), Some(&meta), vec2(190.0, 32.0), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.add_enabled_ui(!deaf, |ui| {
                if vector_button(ui, &pal, Icon::Play, &format!("Play the {} sound", sound.label().to_lowercase()), 28.0, false, pal.text).clicked() {
                    act = Some(Act::Play(sound));
                }
            });
            if own.is_some() && vector_button(ui, &pal, Icon::Close, "Remove this file", 28.0, false, pal.text).clicked() {
                act = Some(Act::Remove(sound));
            }
            let choose = button(ui, &pal, Btn::Secondary, "Choose file", !full);
            if choose.on_disabled_hover_text("You have custom sounds for 50 places. Remove one first.").clicked() {
                act = Some(Act::Choose(sound));
            }
        });
    }
    let Some(act) = act else { return false };
    let s = &mut cx.settings.sounds;
    match act {
        Act::Play(sound) => {
            let places = place.into_iter().collect::<Vec<_>>();
            let file = sounds::file_for(s, sound, &places).map(str::to_owned);
            sounds::preview_file(sound, file.as_deref(), cx.settings);
            false
        }
        Act::Choose(sound) => {
            let Some(path) = rfd::FileDialog::new().set_title("Choose a sound").add_filter("WAV or Ogg Opus", &["wav", "ogg", "opus"]).pick_file() else { return false };
            if let Err(e) = sounds::check(&path) {
                cx.view.toast(e, true);
                return false;
            }
            let file = path.to_string_lossy().into_owned();
            let map = match place {
                None => &mut s.files,
                Some(p) => s.places.entry(p).or_default(),
            };
            map.insert(sound.key().to_owned(), file.clone());
            sounds::preview_file(sound, Some(&file), cx.settings);
            true
        }
        Act::Remove(sound) => {
            match place {
                None => drop(s.files.remove(sound.key())),
                Some(p) => {
                    let empty = s.places.get_mut(&p).is_some_and(|m| m.remove(sound.key()).is_some() && m.is_empty());
                    if empty {
                        s.places.remove(&p);
                        cx.view.user_settings_ui.sound_place = None;
                    }
                }
            }
            true
        }
    }
}
