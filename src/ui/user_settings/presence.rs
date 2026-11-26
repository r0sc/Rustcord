//! Activity privacy > Rustcord presence: the local switch for "Playing Rustcord", a preview of
//! what others see, and the optional Application ID whose icon becomes the picture
//! (`crate::activity::AppActivity`, docs/FEATURES_NOTES.md §1.3). This device only; the synced
//! activity rows follow below it.

use super::{Pal, hint, section, setting_row, switch_row};
use crate::activity::AppActivity;
use crate::model::{Id, UserStatus};
use crate::ui::UiCx;
use crate::ui::messages::{self, Btn, Callout, callout};
use egui::{RichText, TextEdit, vec2};

const PORTAL: &str = "https://discord.com/developers/applications";
/// Discord looks a presence button's link up by application, so without one nobody sees it.
const NO_BUTTON: &str = "Others see the Get Rustcord button once you add an Application ID below.";

/// "" -> no ID; 17 to 20 digits (spaces around are ignored) -> that ID; anything else -> Err.
fn parse_app_id(text: &str) -> Result<Option<Id>, ()> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    if !(17..=20).contains(&t.len()) || !t.bytes().all(|b| b.is_ascii_digit()) {
        return Err(());
    }
    t.parse().map(|n| Some(Id(n))).map_err(|_| ())
}

/// True when `cx.settings` changed (the caller saves).
pub(super) fn show(ui: &mut egui::Ui, cx: &mut UiCx<'_>) -> bool {
    let pal = Pal::new(cx.theme);
    section(ui, cx.theme, "Rustcord presence");
    let p = &mut cx.settings.rustcord_presence;
    let mut changed = switch_row(
        ui,
        cx.theme,
        "Show Rustcord as my activity",
        Some("Friends see \"Playing Rustcord\" on your profile. It also shows Discord that you use a third-party client."),
        &mut p.enabled,
    );
    let (enabled, current) = (p.enabled, p.application_id);

    ui.add_space(8.0);
    hint(ui, cx.theme, "Preview");
    ui.add_space(4.0);
    let app = cx.state.activity.app.clone().filter(|_| enabled).unwrap_or(AppActivity { application_id: current, start_ms: crate::activity::now_ms() });
    ui.scope(|ui| {
        if !enabled {
            ui.set_opacity(0.5);
        }
        egui::Frame::new().fill(pal.surface).corner_radius(pal.card_radius()).inner_margin(12).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let me = cx.state.me.as_ref().map_or(Id(0), |m| m.id);
            super::super::profile::activity_section(ui, cx, &pal, me, &app.to_activity(), 64.0, pal.surface);
            if app.application_id.is_none() {
                ui.add_space(8.0);
                messages::button(ui, &pal, Btn::Secondary, crate::activity::APP_BUTTON.0, false).on_disabled_hover_text(NO_BUTTON);
            }
        });
    });
    if enabled && app.application_id.is_none() {
        ui.add_space(8.0);
        callout(ui, &pal, Callout::Info, NO_BUTTON);
    }
    if cx.state.chat.my_status == Some(UserStatus::Invisible) {
        ui.add_space(8.0);
        callout(ui, &pal, Callout::Info, "Hidden while you are invisible.");
    }
    if cx.state.activity.detected.is_some() && crate::activity::sharing(cx.state) {
        ui.add_space(8.0);
        callout(ui, &pal, Callout::Info, "Your game shows first while you play.");
    }

    // The draft is checked and saved when the field loses focus (Enter included), never per
    // keystroke: every saved change is one op 3.
    let id = ui.make_persistent_id("rc_presence_app_id");
    let (mut draft, mut error) = ui.data(|d| d.get_temp::<(String, bool)>(id)).unwrap_or_else(|| (current.map(|i| i.0.to_string()).unwrap_or_default(), false));
    setting_row(
        ui,
        cx.theme,
        "Application ID (optional)",
        Some("Make an app in the Discord Developer Portal, give it the Rustcord icon and paste its ID here. Its icon becomes the picture and others see the Get Rustcord button. Empty: neither."),
        vec2(260.0, 32.0),
        |ui| {
            let r = ui.add(TextEdit::singleline(&mut draft).hint_text("18-digit ID").char_limit(32).desired_width(260.0));
            if r.changed() {
                error = false;
            }
            if r.lost_focus() {
                match parse_app_id(&draft) {
                    Ok(new) => {
                        draft = new.map(|i| i.0.to_string()).unwrap_or_default();
                        if new != current {
                            cx.settings.rustcord_presence.application_id = new;
                            changed = true;
                        }
                    }
                    Err(()) => error = true,
                }
            }
        },
    );
    if error {
        ui.label(RichText::new("That is not an application ID (17 to 20 digits).").size(pal.meta()).color(pal.danger));
    }
    ui.data_mut(|d| d.insert_temp(id, (draft, error)));
    ui.add_space(8.0);
    if messages::button(ui, &pal, Btn::Secondary, "Open Developer Portal", true).clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(PORTAL));
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_id_check() {
        assert_eq!(parse_app_id(""), Ok(None));
        assert_eq!(parse_app_id("   "), Ok(None));
        assert_eq!(parse_app_id("123"), Err(()));
        assert_eq!(parse_app_id("123456789012345678"), Ok(Some(Id(123456789012345678))));
        assert_eq!(parse_app_id("  123456789012345678 "), Ok(Some(Id(123456789012345678))));
        assert_eq!(parse_app_id("12345678901234567x"), Err(()));
        assert_eq!(parse_app_id("99999999999999999999"), Err(()), "20 digits but past u64");
    }
}
