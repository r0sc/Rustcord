//! Game overlay (Windows only; docs/OVERLAY_DESIGN.md §8): Rustcord's own windows over the game
//! show who is in your voice call and new messages. All options live on this device
//! (`config::OverlaySettings`).
//! OWNERSHIP: overlay.

use super::{combo_icon, setting_row, switch_row};
use crate::config::{Corner, Show};
use crate::theme::Theme;
use crate::ui::{UiCx, hint, section};

const CORNERS: [(Corner, &str); 5] = [(Corner::TopLeft, "Top left"), (Corner::TopRight, "Top right"), (Corner::BottomLeft, "Bottom left"), (Corner::BottomRight, "Bottom right"), (Corner::Off, "Off")];

/// The page. True when a setting changed (the caller saves).
pub(super) fn show(ui: &mut egui::Ui, cx: &mut UiCx<'_>) -> bool {
    let theme = &*cx.theme;
    let o = &mut cx.settings.overlay;
    let mut changed = switch_row(
        ui,
        theme,
        "Enable in-game overlay",
        Some("Shows your voice call and new messages on top of games in windowed and borderless fullscreen. Exclusive fullscreen isn't supported."),
        &mut o.enabled,
    );
    ui.add_enabled_ui(o.enabled, |ui| {
        section(ui, theme, "Voice");
        changed |= choice_row(ui, theme, "voice_corner", "Position", Some("Where the people in your call show."), &mut o.voice_corner, &CORNERS);
        let mut small = !o.large_avatars;
        changed |= choice_row(ui, theme, "avatar_size", "Avatar size", None, &mut small, &[(true, "Small"), (false, "Large")]);
        o.large_avatars = !small;
        changed |= choice_row(ui, theme, "show_names", "Show names", None, &mut o.show_names, &[(Show::Always, "Always"), (Show::Speaking, "While speaking"), (Show::Never, "Never")]);
        changed |= choice_row(ui, theme, "show_users", "Show users", None, &mut o.show_users, &[(Show::Always, "Always"), (Show::Speaking, "While speaking")]);
        section(ui, theme, "Notifications");
        let text = "New direct messages and mentions, as your notification settings allow. Each one hides after 5 seconds.";
        changed |= choice_row(ui, theme, "notification_corner", "Position", Some(text), &mut o.notification_corner, &CORNERS);
    });
    ui.add_space(16.0);
    hint(ui, theme, "The overlay shows while a game from Registered games is in front. It's a separate Rustcord window: nothing is added to the game.");
    changed
}

/// A setting row with a dropdown, like `synced::pick_row`. `id` tells apart rows with the same label.
/// A value the list lacks (a settings file edited by hand) shows as the first option.
pub(super) fn choice_row<T: Copy + PartialEq>(ui: &mut egui::Ui, theme: &Theme, id: &str, label: &str, hint_text: Option<&str>, value: &mut T, options: &[(T, &str)]) -> bool {
    const WIDTH: f32 = 220.0;
    // egui's combo box: one line of button text and the button padding.
    let height = ui.text_style_height(&egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.y;
    let shown = options.iter().find(|(v, _)| v == value).map_or(options[0].1, |(_, l)| *l);
    setting_row(ui, theme, label, hint_text, egui::vec2(WIDTH, height), |ui| {
        let mut changed = false;
        egui::ComboBox::from_id_salt(("rc_overlay_pick", id)).width(WIDTH).selected_text(shown).icon(combo_icon).show_ui(ui, |ui| {
            for (v, l) in options {
                if ui.selectable_label(value == v, *l).clicked() && value != v {
                    *value = *v;
                    changed = true;
                }
            }
        });
        changed
    })
}
