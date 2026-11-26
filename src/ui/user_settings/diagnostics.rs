//! Advanced > Diagnostics (plugin ClientDiagnostics, `crate::plugins::diagnostics`): frame times,
//! the slowest sections, memory, voice playback and the connection as cards, and Copy report. While it is on
//! screen it times frames (`frame_stats::keep_live`) and redraws once a second; leaving the page
//! stops both, so idle CPU is back to 0.

use super::{Pal, hint, section};
use crate::plugins::{self, Plugin, diagnostics as data};
use crate::ui::messages::{Btn, button, sheen};
use crate::ui::{UiCx, frame_stats, semibold};
use egui::{Align, Frame, Layout, Margin, RichText};
use std::time::Duration;

/// Memory counters and state sizes are read once a second, not every frame.
#[derive(Clone, Default)]
struct Snap {
    at: f64,
    memory: Option<(f64, f64)>,
    parts: Vec<(&'static str, f64)>,
}

pub(super) fn show(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    if !plugins::enabled(Plugin::ClientDiagnostics) {
        return;
    }
    frame_stats::keep_live();
    ui.ctx().request_repaint_after(Duration::from_secs(1));
    let pal = Pal::new(cx.theme);
    let (id, now) = (egui::Id::unique("rc_diag_snap"), ui.input(|i| i.time));
    let mut snap = ui.data(|d| d.get_temp::<Snap>(id)).filter(|s| now - s.at < 1.0).unwrap_or_else(|| Snap {
        at: now,
        memory: crate::ui::images::memory_mb(),
        parts: data::biggest_parts(&cx.state.mem_estimate(), 5),
    });
    snap.at = snap.at.min(now);
    ui.data_mut(|d| d.insert_temp(id, snap.clone()));
    let window = frame_stats::last();
    let connection = format!("{} for {}", data::conn_text(&cx.state.conn), data::duration_text(cx.view.conn_since.elapsed().as_secs()));
    let facts = data::Facts { window: window.clone(), memory: snap.memory, parts: snap.parts.clone(), connection: connection.clone(), uptime_s: crate::crash::uptime_s(), voice: crate::voice::audio::stats() };

    section(ui, cx.theme, "Diagnostics");
    hint(ui, cx.theme, "Rustcord's own numbers. This page redraws once a second to update them; with it closed, an idle Rustcord draws 0 frames.");

    card(ui, &pal, "Frames, last 2 s", |ui| match &window {
        None => row(ui, &pal, "Measuring", RichText::new("2 s").color(pal.muted)),
        Some(w) => {
            row(ui, &pal, "Drawn", plain(&pal, data::frames_text(w)));
            if w.frames > 0 {
                row(ui, &pal, "Frame time", plain(&pal, format!("{:.2} ms, p95 {:.2} ms", w.frame_ms.0, w.frame_ms.1)));
                row(ui, &pal, "Rustcord's own time", plain(&pal, format!("{:.2} ms, p95 {:.2} ms", w.app_ms.0, w.app_ms.1)));
            }
        }
    });
    if let Some(w) = window.as_ref().filter(|w| w.frames > 0) {
        card(ui, &pal, "Slowest sections, ms per frame", |ui| {
            for (name, ms, slow) in data::slowest(w, 5) {
                // Colour and the word, never colour alone.
                let value = if slow { RichText::new(format!("{ms:.2}  slow")).color(pal.tk.attention) } else { RichText::new(format!("{ms:.2}")).color(pal.text) };
                row(ui, &pal, name, value);
            }
        });
    }
    card(ui, &pal, "Memory", |ui| {
        match snap.memory {
            Some((ws, private)) => {
                row(ui, &pal, "Working set", plain(&pal, format!("{ws:.1} MB")));
                row(ui, &pal, "Private", plain(&pal, format!("{private:.1} MB")));
            }
            None => row(ui, &pal, "Process", RichText::new("not measured on this system").color(pal.muted)),
        }
        ui.add_space(4.0);
        ui.label(RichText::new("Biggest parts of the state").size(pal.meta()).color(pal.muted));
        for (name, mb) in &snap.parts {
            row(ui, &pal, name, plain(&pal, format!("{mb:.1} MB")));
        }
    });
    card(ui, &pal, "Voice playback, since start", |ui| {
        let (dry, late, concealed) = facts.voice;
        row(ui, &pal, "Speaker ran dry", plain(&pal, dry.to_string()));
        row(ui, &pal, "Late packets dropped", plain(&pal, late.to_string()));
        row(ui, &pal, "Frames concealed", plain(&pal, concealed.to_string()));
    });
    card(ui, &pal, "Connection", |ui| {
        row(ui, &pal, "State", plain(&pal, connection));
        row(ui, &pal, "Version", plain(&pal, env!("CARGO_PKG_VERSION").to_owned()));
        row(ui, &pal, "System", plain(&pal, format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)));
        row(ui, &pal, "Open for", plain(&pal, data::duration_text(facts.uptime_s)));
    });
    ui.add_space(12.0);
    if button(ui, &pal, Btn::Secondary, "Copy report", true).clicked() {
        ui.ctx().copy_text(data::report(&facts));
        cx.view.toast("Report copied.", false);
    }
}

fn plain(pal: &Pal, text: String) -> RichText {
    RichText::new(text).color(pal.text)
}

fn card(ui: &mut egui::Ui, pal: &Pal, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(8.0);
    let out = Frame::new().fill(pal.surface).corner_radius(pal.card_radius()).inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.label(RichText::new(title).font(semibold(pal.label())).color(pal.muted));
        add(ui);
    });
    sheen(ui.painter(), out.response.rect, f32::from(pal.card_radius()), &pal.tk);
}

fn row(ui: &mut egui::Ui, pal: &Pal, label: &str, value: RichText) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(pal.ui()).color(pal.text));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(value.size(pal.ui())));
    });
}
