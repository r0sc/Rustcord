//! The crash report card (src/crash.rs): shown once on the main screen after a panic, with the
//! report text, Copy report, Report on GitHub, Turn them off (when the panic came from a plugin
//! file) and Dismiss. Nothing is sent: the user copies the text and pastes it into an issue.

use super::messages::{Btn, Callout, Pal, button, callout, sheen};
use super::{UiCx, bold, motion};
use egui::{Align2, Area, Frame, Id, Label, LayerId, Margin, Order, RichText, ScrollArea, Stroke, vec2};

const NEW_ISSUE: &str = "https://github.com/r0sc/Rustcord/issues/new";

pub fn show(ctx: &egui::Context, cx: &mut UiCx<'_>) {
    if cx.view.crash.is_none() {
        cx.view.crash = crate::crash::take_new();
    }
    let Some(report) = cx.view.crash.clone() else { return };
    let pal = Pal::new(cx.theme);
    let well = cx.theme.colors.code_background.to_color32();
    let suspects = report.suspects();
    let (id, wide) = (Id::unique("rc_crash_card"), (ctx.content_rect().width() - 32.0).clamp(280.0, 520.0));
    // Fades in and rises 8 px over BASE; closing is instant.
    let layer = LayerId::new(Order::Foreground, id);
    let a = motion::opened(ctx, layer, motion::BASE);
    let mut done = false;
    let frame = Frame::popup(&ctx.global_style()).fill(pal.surface).corner_radius(pal.float_radius()).inner_margin(Margin::same(16));
    Area::new(id).anchor(Align2::CENTER_TOP, vec2(0.0, 44.0 + 8.0 * (1.0 - a))).order(Order::Foreground).fade_in(false).show(ctx, |ui| {
        motion::own(ctx, layer);
        ui.multiply_opacity(a);
        let out = frame.show(ui, |ui| {
            ui.set_width(wide - 32.0);
            ui.spacing_mut().item_spacing.y = 8.0;
            let title = if report.this_run { "Rustcord hit an error." } else { "Rustcord hit an error last time." };
            ui.add(Label::new(RichText::new(title).font(bold(pal.heading())).color(pal.text)).selectable(false));
            ui.add(Label::new(RichText::new("Nothing was sent. This is the report; copy it into an issue if you want it fixed.").size(pal.meta() + 1.0).color(pal.muted)));
            Frame::new().fill(well).corner_radius(pal.radius).inner_margin(Margin::same(8)).show(ui, |ui| {
                ScrollArea::vertical().max_height(110.0).auto_shrink([false, true]).show(ui, |ui| {
                    ui.add(Label::new(RichText::new(&report.text).monospace().size(pal.meta()).color(pal.text)).wrap());
                });
            });
            if !suspects.is_empty() {
                let names: Vec<&str> = suspects.iter().map(|p| crate::plugins::meta_of(*p).name).collect();
                callout(ui, &pal, Callout::Warn, &format!("It came from a plugin: {}.", names.join(", ")));
            }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if button(ui, &pal, Btn::Primary, "Copy report", true).clicked() {
                    ctx.copy_text(report.text.clone());
                    cx.view.toast("Report copied.", false);
                }
                if button(ui, &pal, Btn::Secondary, "Report on GitHub", true).clicked() {
                    ctx.open_url(egui::OpenUrl::new_tab(NEW_ISSUE));
                    cx.view.toast("Paste the report into the new issue.", false);
                }
                if !suspects.is_empty() && button(ui, &pal, Btn::Secondary, "Turn them off", true).clicked() {
                    let failed = suspects.iter().any(|p| crate::plugins::set(*p, "enabled", toml::Value::Boolean(false)).is_err());
                    cx.view.toast(if failed { "Could not save the plugin settings." } else { "Those plugins are off." }, failed);
                }
                if button(ui, &pal, Btn::Ghost, "Dismiss", true).clicked() {
                    done = true;
                }
            });
        });
        ui.painter().rect_stroke(out.response.rect, pal.float_radius(), Stroke::new(1.0, pal.tk.rule), egui::StrokeKind::Inside);
        sheen(ui.painter(), out.response.rect, f32::from(pal.float_radius()), &pal.tk);
    });
    if done {
        report.dismiss();
        cx.view.crash = None;
    }
}
