//! The setup window (docs/DESIGN.md §12 "Installer"): a 32 px title bar of its own with the
//! mark, one screen with the options and the main button, then progress and the result. The
//! window takes each screen's height (`Setup::fit`), so no screen is half empty.
//! Colours, sizes and controls follow DESIGN.md §2 (Nightshade), §3, §9; the setup reads no
//! theme files, so the values are repeated here.

use crate::payload;
use crate::setup::{self, APP_EXE, Mode, Options, VERSION};
use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align, Color32, CornerRadius, FontFamily, FontId, Layout, Pos2, Rect, Response, Role, Sense, Stroke, StrokeKind, UiBuilder, Vec2, ViewportCommand, WidgetInfo,
    pos2, vec2,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const FRAME: Color32 = Color32::from_rgb(0x08, 0x07, 0x0b);
const BACKGROUND: Color32 = Color32::from_rgb(0x12, 0x11, 0x18);
const SURFACE: Color32 = Color32::from_rgb(0x1c, 0x1a, 0x24);
const TEXT: Color32 = Color32::from_rgb(0xf0, 0xee, 0xf6);
const MUTED: Color32 = Color32::from_rgb(0xa8, 0xa5, 0xb6);
const ACCENT: Color32 = Color32::from_rgb(0xa4, 0x2b, 0xea);
const ACCENT_TEXT: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
const EDGE: Color32 = Color32::from_rgb(0x83, 0x80, 0x8f);
const DANGER_FILL: Color32 = Color32::from_rgb(0xcc, 0x3d, 0x3f);
const FOCUS: Color32 = Color32::from_rgb(0xbe, 0x67, 0xf0);
/// Windows' caption red (ui/chrome.rs).
const CLOSE_RED: Color32 = Color32::from_rgb(0xc4, 0x2b, 0x1c);
const LOGO: &[u8] = include_bytes!("../../../assets/icons/rustcord-128.png");
const SEMIBOLD: &str = "semibold";
const WIDTH: f32 = 520.0;
/// The first screen's height, so the window opens at its final size: install (all fixed sizes),
/// remove (its paragraph wraps to two lines in Segoe UI; `Setup::fit` corrects any other font).
const INSTALL_HEIGHT: f32 = 32.0 + 32.0 + 48.0 + 24.0 + 2.0 * 56.0 + 12.0 + 16.0 + 24.0 + 40.0 + 32.0;
const REMOVE_HEIGHT: f32 = 341.0;

/// `text` at `a` opacity: hover 6 %, selected and rules 10 % (DESIGN.md §2.2).
fn tint(a: f32) -> Color32 {
    TEXT.gamma_multiply(a)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Options,
    Working,
    Done,
    Failed,
}

/// Between the window and the worker thread.
#[derive(Default)]
struct Shared {
    fraction: f32,
    result: Option<Result<(), String>>,
}

struct Setup {
    mode: Mode,
    opts: Options,
    screen: Screen,
    error: String,
    shared: Arc<Mutex<Shared>>,
    logo: egui::TextureHandle,
    /// Uninstalled: the uninstaller deletes itself once the window has closed.
    removed: Arc<AtomicBool>,
    /// `RUSTCORD_SCREENSHOT`: the PNG to save, and the frame count.
    shot: Option<PathBuf>,
    frame_no: u32,
    /// The window's inner height asked for last (`fit`).
    height: f32,
}

pub fn run(mode: Mode, opts: Options) -> eframe::Result {
    let removed = Arc::new(AtomicBool::new(false));
    let dir = opts.dir.clone();
    let icon = image::load_from_memory(LOGO).map(|i| i.resize(64, 64, image::imageops::FilterType::Lanczos3).to_rgba8());
    let icon = icon.map(|i| egui::IconData { width: i.width(), height: i.height(), rgba: i.into_raw() }).unwrap_or_default();
    let height = if mode == Mode::Install { INSTALL_HEIGHT } else { REMOVE_HEIGHT };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Rustcord setup")
            .with_app_id("rustcord-setup")
            .with_inner_size([WIDTH, height])
            .with_resizable(false)
            .with_decorations(false)
            .with_icon(icon),
        renderer: eframe::Renderer::Glow,
        centered: true,
        ..Default::default()
    };
    let flag = removed.clone();
    eframe::run_native("Rustcord setup", options, Box::new(move |cc| Ok(Box::new(Setup::new(cc, mode, opts, flag, height)))))?;
    if removed.load(Ordering::SeqCst) {
        setup::remove_self(&dir);
    }
    Ok(())
}

impl Setup {
    fn new(cc: &eframe::CreationContext<'_>, mode: Mode, opts: Options, removed: Arc<AtomicBool>, height: f32) -> Self {
        let ctx = &cc.egui_ctx;
        ctx.set_theme(egui::Theme::Dark);
        fonts(ctx);
        let logo = image::load_from_memory(LOGO).map(|i| i.to_rgba8()).unwrap_or_default();
        let logo = egui::ColorImage::from_rgba_unmultiplied([logo.width() as usize, logo.height() as usize], logo.as_raw());
        let logo = ctx.load_texture("rustcord-logo", logo, egui::TextureOptions { mipmap_mode: Some(egui::TextureFilter::Linear), ..egui::TextureOptions::LINEAR });
        let shot = std::env::var_os("RUSTCORD_SCREENSHOT").map(PathBuf::from);
        let mut setup = Setup { mode, opts, screen: Screen::Options, error: String::new(), shared: Default::default(), logo, removed, shot, frame_no: 0, height };
        if setup.shot.is_some() {
            setup.demo(&std::env::var("RUSTCORD_SETUP_SCREEN").unwrap_or_default());
        }
        setup
    }

    /// `RUSTCORD_SETUP_SCREEN` for screenshots: shows a screen without doing anything.
    fn demo(&mut self, screen: &str) {
        match screen {
            "progress" => {
                self.screen = Screen::Working;
                self.shared.lock().unwrap_or_else(|e| e.into_inner()).fraction = 0.62;
            }
            "done" | "removed" => self.screen = Screen::Done,
            "error" => {
                self.screen = Screen::Failed;
                self.error = "Rustcord is still running. Quit it from the tray icon, then try again.".into();
            }
            _ => {}
        }
        if matches!(screen, "uninstall" | "removed") {
            self.mode = Mode::Uninstall;
        }
    }

    /// Runs the install (or removal) on a worker thread; again after a failure ("Try again").
    fn start(&mut self, ctx: &egui::Context) {
        self.screen = Screen::Working;
        self.error.clear();
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).fraction = 0.0;
        let (shared, opts, mode, ctx) = (self.shared.clone(), self.opts.clone(), self.mode, ctx.clone());
        std::thread::spawn(move || {
            let mut report = |f: f32| {
                shared.lock().unwrap_or_else(|e| e.into_inner()).fraction = f;
                ctx.request_repaint();
            };
            let result = match mode {
                Mode::Install => match payload::split(setup::own_exe()) {
                    (stub, Some(p)) => setup::install(&opts, stub, &p, &mut report),
                    (_, None) => Err(setup::NO_PAYLOAD.to_owned()),
                },
                Mode::Uninstall => setup::uninstall(&opts),
            };
            shared.lock().unwrap_or_else(|e| e.into_inner()).result = Some(result);
            ctx.request_repaint();
        });
    }

    /// Takes the worker's result.
    fn poll(&mut self) {
        let Some(result) = self.shared.lock().unwrap_or_else(|e| e.into_inner()).result.take() else { return };
        match result {
            Ok(()) => {
                self.screen = Screen::Done;
                self.removed.store(self.mode == Mode::Uninstall, Ordering::SeqCst);
            }
            Err(e) => {
                self.error = e;
                self.screen = Screen::Failed;
            }
        }
    }

    fn heading(&self) -> &'static str {
        match (self.mode, self.screen) {
            (Mode::Install, Screen::Options) => "Install Rustcord",
            (Mode::Install, Screen::Working) => "Installing Rustcord",
            (Mode::Install, Screen::Done) => "Rustcord is installed",
            (Mode::Install, Screen::Failed) => "Rustcord couldn't be installed",
            (Mode::Uninstall, Screen::Options) => "Remove Rustcord",
            (Mode::Uninstall, Screen::Working) => "Removing Rustcord",
            (Mode::Uninstall, Screen::Done) => "Rustcord was removed",
            (Mode::Uninstall, Screen::Failed) => "Rustcord couldn't be removed",
        }
    }

    fn title_bar(&self, ui: &mut egui::Ui, bar: Rect) {
        let ctx = ui.ctx().clone();
        ui.painter().rect_filled(bar, 0.0, FRAME);
        // First, so the buttons lie on top of it.
        let drag = ui.interact(bar, ui.make_persistent_id("window_drag"), Sense::click_and_drag());
        if drag.drag_started_by(egui::PointerButton::Primary) {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }
        let mark = Rect::from_min_size(pos2(bar.left() + 12.0, bar.center().y - 8.0), Vec2::splat(16.0));
        ui.painter().image(self.logo.id(), mark, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
        let g = ui.painter().layout_no_wrap("Rustcord setup".into(), FontId::new(14.0, semibold()), MUTED);
        ui.painter().galley(pos2(mark.right() + 8.0, bar.center().y - g.size().y / 2.0), g, MUTED);

        let busy = self.screen == Screen::Working;
        for (i, (close, tip)) in [(false, "Minimize"), (true, "Close")].into_iter().enumerate() {
            let r = Rect::from_min_size(pos2(bar.right() - (2 - i) as f32 * 46.0, bar.top()), vec2(46.0, bar.height()));
            let resp = ui.interact(r, ui.make_persistent_id(("window_button", i)), Sense::click());
            resp.widget_info(|| WidgetInfo::labeled(Role::Button, !(close && busy), tip));
            let hot = (resp.hovered() || resp.has_focus()) && !(close && busy);
            let fill = match (close, hot) {
                (true, true) => CLOSE_RED,
                (false, true) => tint(0.1),
                _ => Color32::TRANSPARENT,
            };
            ui.painter().rect_filled(r, 0.0, fill);
            let ink = if close && hot { Color32::WHITE } else if hot { TEXT } else { MUTED };
            caption_glyph(ui.painter(), r.center(), close, ink);
            if resp.on_hover_text(tip).clicked() && !(close && busy) {
                ctx.send_viewport_cmd(if close { ViewportCommand::Close } else { ViewportCommand::Minimized(true) });
            }
        }
    }

    /// The screen, top down; the window takes its height (`fit`).
    fn body(&mut self, ui: &mut egui::Ui) {
        let area = ui.max_rect();
        // Every gap is written out below.
        ui.spacing_mut().item_spacing.y = 0.0;
        // Header: the mark, the heading (title role) and the version (meta).
        let (head, _) = ui.allocate_exact_size(vec2(area.width(), 48.0), Sense::hover());
        let logo = Rect::from_min_size(head.min, Vec2::splat(48.0));
        ui.painter().image(self.logo.id(), logo, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
        let title = ui.painter().layout_no_wrap(self.heading().into(), FontId::new(20.0, semibold()), TEXT);
        ui.painter().galley(pos2(logo.right() + 16.0, head.top() + 1.0), title, TEXT);
        let meta = ui.painter().layout_no_wrap(format!("Version {VERSION}"), FontId::proportional(12.0), MUTED);
        ui.painter().galley(pos2(logo.right() + 16.0, head.top() + 29.0), meta, MUTED);
        ui.add_space(24.0);

        match (self.mode, self.screen) {
            (Mode::Install, Screen::Options) => {
                let o = &mut self.opts;
                let rows = group(ui, 2);
                // The same words as the app's Windows settings row (docs/DESIGN.md §12).
                row_text(ui, rows[0], "Open Rustcord when I sign in to Windows", Some("Rustcord starts in the notification area."));
                switch(ui, rows[0], &mut o.autostart, "Open Rustcord when I sign in to Windows");
                row_text(ui, rows[1], "Create a desktop shortcut", None);
                switch(ui, rows[1], &mut o.desktop, "Create a desktop shortcut");
                // Where it goes, as a note: the folder is fixed per user (--dir changes it).
                ui.add_space(12.0);
                meta_line(ui, &format!("Installs to {}", o.dir.display()));
            }
            (Mode::Uninstall, Screen::Options) => {
                paragraph(ui, "This removes the app, its shortcuts and its startup entry. Your Discord account stays as it is.", MUTED);
                ui.add_space(16.0);
                let rows = group(ui, 1);
                row_text(ui, rows[0], "Also delete my local data and sign out", Some("Settings, cache and the saved login on this PC."));
                checkbox(ui, rows[0], &mut self.opts.purge, "Also delete my local data and sign out");
            }
            (_, Screen::Working) => {
                let fraction = self.shared.lock().unwrap_or_else(|e| e.into_inner()).fraction.clamp(0.0, 1.0);
                let (bar, _) = ui.allocate_exact_size(vec2(area.width(), 4.0), Sense::hover());
                ui.painter().rect_filled(bar, 2.0, EDGE.gamma_multiply(0.5));
                ui.painter().rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * fraction, 4.0)), 2.0, TEXT);
                ui.add_space(8.0);
                paragraph(ui, &format!("{}%", (fraction * 100.0).round()), MUTED);
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
            (Mode::Install, Screen::Done) => paragraph(ui, "Find it in the Start menu, or open it now.", MUTED),
            (Mode::Uninstall, Screen::Done) => paragraph(ui, "Your Discord account stays as it is.", MUTED),
            (_, Screen::Failed) => paragraph(ui, &self.error.clone(), TEXT),
        }

        if self.screen == Screen::Working {
            return;
        }
        // Footer: 24 under the content, right-aligned, 12 apart, the main action rightmost.
        ui.add_space(24.0);
        let ctx = ui.ctx().clone();
        ui.allocate_ui_with_layout(vec2(area.width(), 40.0), Layout::right_to_left(Align::Center), |ui| {
            ui.set_min_height(40.0);
            ui.spacing_mut().item_spacing.x = 12.0;
            match (self.mode, self.screen) {
                (mode, Screen::Options) => {
                    let (label, kind) = if mode == Mode::Install { ("Install", Kind::Primary) } else { ("Remove", Kind::Danger) };
                    if button(ui, label, kind).clicked() {
                        self.start(&ctx);
                    }
                    if button(ui, "Cancel", Kind::Ghost).clicked() {
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                }
                (Mode::Install, Screen::Done) => {
                    if button(ui, "Open Rustcord", Kind::Primary).clicked() {
                        let _ = std::process::Command::new(self.opts.dir.join(APP_EXE)).current_dir(&self.opts.dir).spawn();
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                    if button(ui, "Close", Kind::Ghost).clicked() {
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                }
                (_, Screen::Done) => {
                    if button(ui, "Close", Kind::Secondary).clicked() {
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                }
                (_, Screen::Failed) => {
                    if button(ui, "Try again", Kind::Primary).clicked() {
                        self.start(&ctx);
                    }
                    if button(ui, "Close", Kind::Ghost).clicked() {
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                }
                (_, Screen::Working) => {}
            }
        });
    }

    /// Sizes the window to the screen's `height` (points), keeping its centre where it is.
    fn fit(&mut self, ctx: &egui::Context, height: f32) {
        let height = height.round();
        if (height - self.height).abs() < 1.0 {
            return;
        }
        self.height = height;
        // The rects of the size the window has now (an earlier request may still be on its way).
        let (inner, outer) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().outer_rect));
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(WIDTH, height)));
        if let (Some(inner), Some(outer)) = (inner, outer) {
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(outer.min + vec2(0.0, ((inner.height() - height) / 2.0).round())));
        }
    }

    /// `RUSTCORD_SCREENSHOT`: saves the 10th frame and closes.
    fn screenshot(&mut self, ctx: &egui::Context) {
        let Some(path) = self.shot.clone() else { return };
        self.frame_no += 1;
        if self.frame_no == 10 {
            let ctx2 = ctx.clone();
            ctx.request_screenshot(move |img| {
                let [w, h] = img.size;
                let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, rgba) {
                    let _ = buf.save(&path);
                }
                ctx2.send_viewport_cmd(ViewportCommand::Close);
            });
        }
        if self.frame_no < 20 {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
}

impl eframe::App for Setup {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.screen == Screen::Working {
            self.poll();
        }
        // Closing mid-way would leave half an install (a screenshot run has no worker).
        if self.screen == Screen::Working && self.shot.is_none() && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        }
        let full = ui.max_rect();
        ui.painter().rect_filled(full, 0.0, BACKGROUND);
        let bar = Rect::from_min_size(full.min, vec2(full.width(), 32.0));
        self.title_bar(ui, bar);
        let body = Rect::from_min_max(pos2(full.left() + 32.0, bar.bottom() + 32.0), full.max - vec2(32.0, 32.0));
        let used = ui.scope_builder(UiBuilder::new().max_rect(body), |ui| self.body(ui)).response.rect;
        self.fit(&ctx, used.bottom() + 32.0 - full.top());
        self.screenshot(&ctx);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::from(BACKGROUND).to_array()
    }
}

/// Segoe UI and Segoe UI Semibold from the system (nothing bundled), egui's font as fallback.
fn fonts(ctx: &egui::Context) {
    let mut defs = egui::FontDefinitions::default();
    let dir = PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| r"C:\Windows".into())).join("Fonts");
    let mut chain = |file: &str, base: &[String]| {
        let mut family = base.to_vec();
        if let Ok(bytes) = std::fs::read(dir.join(file)) {
            defs.font_data.insert(file.into(), Arc::new(egui::FontData::from_owned(bytes)));
            family.insert(0, file.to_owned());
        }
        family
    };
    let base = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let regular = chain("segoeui.ttf", &base);
    let semibold = chain("seguisb.ttf", &regular);
    defs.families.insert(FontFamily::Proportional, regular);
    defs.families.insert(FontFamily::Name(SEMIBOLD.into()), semibold);
    ctx.set_fonts(defs);
}

fn semibold() -> FontFamily {
    FontFamily::Name(SEMIBOLD.into())
}

/// Wrapped body text (ui role, 14).
fn paragraph(ui: &mut egui::Ui, text: &str, color: Color32) {
    let g = ui.painter().layout(text.to_owned(), FontId::proportional(14.0), color, ui.available_width());
    let (rect, _) = ui.allocate_exact_size(g.size(), Sense::hover());
    ui.painter().galley(rect.min, g, color);
}

/// One line of meta text (12, `text_muted`, 16 tall) in line with the rows' titles; a long
/// one ends in "…" and shows in full on hover.
fn meta_line(ui: &mut egui::Ui, text: &str) {
    let mut job = LayoutJob::simple_singleline(text.into(), FontId::proportional(12.0), MUTED);
    job.wrap = TextWrapping { max_width: ui.available_width() - 32.0, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let g = ui.painter().layout_job(job);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
    let elided = g.elided;
    ui.painter().galley(pos2(rect.left() + 16.0, rect.center().y - g.size().y / 2.0), g, MUTED);
    if elided {
        resp.on_hover_text(text);
    }
}

/// One `surface` container (radius 12) of `n` setting rows, 56 tall, with `rule` lines between
/// them; returns the rows.
fn group(ui: &mut egui::Ui, n: usize) -> Vec<Rect> {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0 * n as f32), Sense::hover());
    ui.painter().rect_filled(rect, 12.0, SURFACE);
    let rows: Vec<Rect> = (0..n).map(|i| Rect::from_min_size(rect.min + vec2(0.0, 56.0 * i as f32), vec2(rect.width(), 56.0))).collect();
    for r in &rows[1..] {
        ui.painter().hline(r.left() + 16.0..=r.right() - 16.0, r.top(), Stroke::new(1.0, tint(0.1)));
    }
    rows
}

/// A row's title (ui 14) and one-line description (meta 12, `text_muted`), left.
fn row_text(ui: &egui::Ui, row: Rect, title: &str, description: Option<&str>) {
    let width = row.width() - 32.0 - 64.0;
    let t = ui.painter().layout_no_wrap(title.into(), FontId::proportional(14.0), TEXT);
    let top = if description.is_some() { row.center().y - 19.0 } else { row.center().y - t.size().y / 2.0 };
    ui.painter().galley(pos2(row.left() + 16.0, top), t, TEXT);
    if let Some(d) = description {
        let mut job = LayoutJob::simple_singleline(d.into(), FontId::proportional(12.0), MUTED);
        job.wrap = TextWrapping { max_width: width, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
        let g = ui.painter().layout_job(job);
        ui.painter().galley(pos2(row.left() + 16.0, row.center().y + 2.0), g, MUTED);
    }
}

/// DESIGN.md §9 switch at the row's right end: 40×24 pill, 18 knob; off: `edge` track, knob with
/// a ×; on: `positive` track, knob with a check. Slides in 120 ms.
fn switch(ui: &mut egui::Ui, row: Rect, on: &mut bool, label: &str) {
    let rect = Rect::from_min_size(pos2(row.right() - 16.0 - 40.0, row.center().y - 12.0), vec2(40.0, 24.0));
    let id = ui.make_persistent_id(label);
    let mut resp = ui.interact(rect, id, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| WidgetInfo::selected(Role::Switch, true, *on, label));
    let t = ui.ctx().animate_bool_with_time(id, *on, 0.12);
    let p = ui.painter();
    let track = EDGE.lerp_to_gamma(ACCENT, t);
    p.rect_filled(rect, 12.0, track);
    let knob = pos2(egui::lerp(rect.left() + 12.0..=rect.right() - 12.0, t), rect.center().y);
    p.circle_filled(knob, 9.0, TEXT.lerp_to_gamma(ACCENT_TEXT, t));
    let ink = Stroke::new(1.5, track);
    if t > 0.5 {
        p.line_segment([knob + vec2(-3.5, 0.0), knob + vec2(-1.0, 2.5)], ink);
        p.line_segment([knob + vec2(-1.0, 2.5), knob + vec2(3.5, -2.5)], ink);
    } else {
        p.line_segment([knob - Vec2::splat(3.0), knob + Vec2::splat(3.0)], ink);
        p.line_segment([knob + vec2(-3.0, 3.0), knob + vec2(3.0, -3.0)], ink);
    }
    focus_ring(ui, &resp, rect, 12);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand);
}

/// DESIGN.md §9 checkbox at the row's right end: 20, radius 4; off a 1.5 px `edge` border, on
/// `positive` with an `on_positive` check.
fn checkbox(ui: &mut egui::Ui, row: Rect, on: &mut bool, label: &str) {
    let rect = Rect::from_min_size(pos2(row.right() - 16.0 - 20.0, row.center().y - 10.0), Vec2::splat(20.0));
    let mut resp = ui.interact(rect, ui.make_persistent_id(label), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| WidgetInfo::selected(Role::CheckBox, true, *on, label));
    let p = ui.painter();
    if *on {
        p.rect_filled(rect, 4.0, ACCENT);
        let c = rect.center();
        let ink = Stroke::new(2.0, ACCENT_TEXT);
        p.line_segment([c + vec2(-4.5, 0.0), c + vec2(-1.5, 3.0)], ink);
        p.line_segment([c + vec2(-1.5, 3.0), c + vec2(4.5, -3.5)], ink);
    } else {
        p.rect_stroke(rect, 4.0, Stroke::new(1.5, EDGE), StrokeKind::Inside);
    }
    focus_ring(ui, &resp, rect, 4);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand);
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Primary,
    Secondary,
    Danger,
    Ghost,
}

/// DESIGN.md §9 button, 40 tall (dialog footer), label ui 14, padding 16, radius 8.
fn button(ui: &mut egui::Ui, label: &str, kind: Kind) -> Response {
    let (rest, ink) = match kind {
        Kind::Primary => (ACCENT, ACCENT_TEXT),
        Kind::Secondary => (tint(0.1), TEXT),
        Kind::Danger => (DANGER_FILL, Color32::WHITE),
        Kind::Ghost => (Color32::TRANSPARENT, TEXT),
    };
    let g = ui.painter().layout_no_wrap(label.into(), FontId::proportional(14.0), ink);
    let (rect, resp) = ui.allocate_exact_size(vec2((g.size().x + 32.0).max(64.0), 40.0), Sense::click());
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, label));
    let (hover, down) = (resp.hovered(), resp.is_pointer_button_down_on());
    let fill = match kind {
        Kind::Primary if down => rest.lerp_to_gamma(Color32::BLACK, 0.1),
        Kind::Primary if hover => rest.lerp_to_gamma(Color32::WHITE, 0.08),
        Kind::Danger if down => rest.lerp_to_gamma(Color32::BLACK, 0.18),
        Kind::Danger if hover => rest.lerp_to_gamma(Color32::BLACK, 0.1),
        Kind::Secondary if down => tint(0.18),
        Kind::Secondary if hover => tint(0.14),
        Kind::Ghost if down => tint(0.1),
        Kind::Ghost if hover => tint(0.06),
        _ => rest,
    };
    ui.painter().rect_filled(rect, 8.0, fill);
    ui.painter().galley(rect.center() - g.size() / 2.0, g, ink);
    focus_ring(ui, &resp, rect, 8);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Keyboard focus: 2 px `focus` at a 2 px offset, radius + 2.
fn focus_ring(ui: &egui::Ui, resp: &Response, rect: Rect, radius: u8) {
    if resp.has_focus() {
        ui.painter().rect_stroke(rect.expand(2.0), CornerRadius::same(radius + 2), Stroke::new(2.0, FOCUS), StrokeKind::Outside);
    }
}

/// Windows 11 caption glyphs (10 px): minimize, close.
fn caption_glyph(p: &egui::Painter, center: Pos2, close: bool, color: Color32) {
    let ppp = p.pixels_per_point();
    let snap = |v: f32| ((v * ppp - 0.5).round() + 0.5) / ppp;
    let c = pos2(snap(center.x), snap(center.y));
    let stroke = Stroke::new(1.0 / ppp * ppp.round().max(1.0), color);
    if close {
        p.line_segment([c - Vec2::splat(5.0), c + Vec2::splat(5.0)], stroke);
        p.line_segment([pos2(c.x - 5.0, c.y + 5.0), pos2(c.x + 5.0, c.y - 5.0)], stroke);
    } else {
        p.hline(c.x - 5.0..=c.x + 5.0, c.y, stroke);
    }
}
