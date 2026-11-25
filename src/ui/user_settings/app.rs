//! App settings kept on this device (`config::Settings`, docs/SETTINGS_NOTES.md §3 "L"):
//! Accessibility, Windows Settings, Streamer Mode; and `frame`, the per-frame
//! hook that applies them: zoom (Ctrl +/−/0 included, kept in `Settings::zoom`), reduced motion,
//! the global keybinds (mute, deafen, Streamer Mode); `logic`, the window's tray life (close to
//! the tray, tray menu, `--minimized`), which also runs while the window is hidden.
//! OWNERSHIP: user-settings engineer; Windows Settings and `logic`: desktop engineer; the Updates
//! rows: updater ui (copy in ui/update.rs).

use super::super::messages::Pal;
use super::super::{UiCx, ViewState, hint, section};
use super::keybinds::{self, Bind};
use super::switch_row;
use crate::backend::{Command, CommandSender};
use crate::config::{AnimateMode, Settings};
use crate::notify::TrayAction;
use egui::{RichText, Slider, ViewportCommand};

/// Accessibility (this device): saturation, role colours, chat text size, zoom, message
/// spacing, motion. True when the settings changed.
pub(super) fn accessibility(ui: &mut egui::Ui, cx: &mut UiCx<'_>) -> bool {
    let pal = Pal::new(cx.theme);
    let theme_size = pal.size;
    let s = &mut *cx.settings;
    let mut changed = false;
    ui.spacing_mut().slider_width = 320.0;

    section(ui, cx.theme, "Saturation");
    let mut pct = (s.saturation * 100.0).round();
    let r = ui.add(Slider::new(&mut pct, 0.0..=100.0).step_by(5.0).fixed_decimals(0).suffix(" %"));
    if r.changed() {
        // Live while dragging; every colour goes through it, so the visuals are re-applied too.
        s.saturation = pct / 100.0;
        s.publish();
        cx.view.theme_dirty = true;
    }
    changed |= released(&r);
    section(ui, cx.theme, "Role colours");
    changed |= switch_row(ui, cx.theme, "Show role colours in names", None, &mut s.role_colors);

    section(ui, cx.theme, "Chat font scaling");
    let mut size = s.chat_font_size.unwrap_or(theme_size).round();
    ui.horizontal(|ui| {
        let r = ui.add(Slider::new(&mut size, 12.0..=24.0).step_by(1.0).fixed_decimals(0).suffix(" px"));
        if r.changed() {
            s.chat_font_size = Some(size);
        }
        changed |= released(&r);
        if s.chat_font_size.is_some() && ui.link("Theme size").on_hover_text("Use the theme's text size").clicked() {
            s.chat_font_size = None;
            changed = true;
        }
    });

    section(ui, cx.theme, "Zoom level");
    // Applied when let go (zooming while dragging would move the slider under the pointer):
    // the dragged value waits in temp memory meanwhile.
    let drag_id = egui::Id::unique("rc_zoom_slider");
    let mut zoom = ui.data(|d| d.get_temp(drag_id)).unwrap_or((s.zoom * 100.0).round());
    let r = ui.add(Slider::new(&mut zoom, 50.0..=200.0).step_by(10.0).fixed_decimals(0).suffix(" %"));
    if r.dragged() {
        ui.data_mut(|d| d.insert_temp(drag_id, zoom));
    } else {
        ui.data_mut(|d| d.remove::<f32>(drag_id));
        if released(&r) {
            s.zoom = zoom / 100.0;
            changed = true;
        }
    }
    hint(ui, cx.theme, "Ctrl + / Ctrl − zoom, Ctrl 0 resets.");

    section(ui, cx.theme, "Space between message groups");
    ui.horizontal(|ui| {
        for px in [0.0, 4.0, 8.0, 16.0, 24.0] {
            changed |= ui.selectable_value(&mut s.message_group_spacing, px, RichText::new(format!("{px} px")).color(pal.text)).changed();
        }
    });

    section(ui, cx.theme, "Motion");
    if switch_row(ui, cx.theme, "Reduced motion", Some(if cfg!(windows) { "No UI animations; animated avatars, emoji and effects only while hovered. Also on while Windows' Animation effects are off." } else { "No UI animations; animated avatars, emoji and effects only while hovered." }), &mut s.reduced_motion) {
        changed = true;
        if s.reduced_motion {
            s.animate_others = AnimateMode::Hover;
        }
    }
    let mut always = s.animate_others == AnimateMode::Always;
    let enabled = !s.reduced_motion;
    let theme = &*cx.theme;
    if ui.add_enabled_ui(enabled, |ui| switch_row(ui, theme, "Play animated avatars, emoji and GIFs", Some("Off: they play while hovered."), &mut always)).inner {
        s.animate_others = if always { AnimateMode::Always } else { AnimateMode::Hover };
        changed = true;
    }
    if changed {
        s.publish();
    }
    changed
}

/// A slider was let go, or changed without a drag (keys): the moment to save.
pub(super) fn released(r: &egui::Response) -> bool {
    r.drag_stopped() || (r.changed() && !r.dragged())
}

/// The copy of the Windows / Linux / macOS settings page (one page id `windows` on every OS).
pub(super) struct DesktopCopy {
    pub title: &'static str,
    pub startup: &'static str,
    pub minimized_meta: &'static str,
    pub close: &'static str,
    pub close_meta: &'static str,
    pub first_close: &'static str,
}

pub(super) const DESKTOP: DesktopCopy = if cfg!(target_os = "macos") {
    DesktopCopy {
        title: "macOS settings",
        startup: "Open Rustcord when I log in",
        minimized_meta: "Rustcord starts in the menu bar, not as a window.",
        close: "Keep Rustcord in the menu bar when I close the window",
        close_meta: "Rustcord keeps running in the menu bar. Quit it from the menu bar icon or with Cmd+Q.",
        first_close: "Rustcord is still running in the menu bar. Quit it from the menu bar icon.",
    }
} else if cfg!(target_os = "linux") {
    DesktopCopy {
        title: "Linux settings",
        startup: "Open Rustcord when I log in",
        minimized_meta: "Rustcord starts in the system tray, not as a window.",
        close: "Minimize to tray when I close the window",
        close_meta: "Rustcord keeps running in the system tray. Quit it from the tray icon.",
        first_close: "Rustcord is still running in the tray. Quit it from the tray icon.",
    }
} else {
    DesktopCopy {
        title: "Windows settings",
        startup: "Open Rustcord when I sign in to Windows",
        minimized_meta: "Rustcord starts in the notification area, not as a window.",
        close: "Minimize to tray when I close the window",
        close_meta: "Rustcord keeps running in the notification area. Quit it from the tray icon.",
        first_close: "Rustcord is still running in the tray. Quit it from the tray icon.",
    }
};

/// The page's own rows, for the settings search.
pub(super) const DESKTOP_LABELS: [&str; 5] = [DESKTOP.startup, "Start minimized", DESKTOP.close, AUTO_UPDATE, CHECK_UPDATES];
const AUTO_UPDATE: &str = "Install updates automatically";
const CHECK_UPDATES: &str = "Check for updates";

/// Windows / Linux / macOS settings: open at sign-in and start minimized (the OS entry itself,
/// read each time the page draws: the installer or another copy may have changed it), minimize
/// to tray. True when the settings changed.
pub(super) fn windows(ui: &mut egui::Ui, cx: &mut UiCx<'_>) -> bool {
    section(ui, cx.theme, "System startup");
    let entry = crate::autostart::get();
    let mut open = entry.is_some();
    let mut minimized = entry.as_deref().is_none_or(|e| e.contains("--minimized"));
    let mut toggled = switch_row(ui, cx.theme, DESKTOP.startup, None, &mut open);
    let theme = &*cx.theme;
    toggled |= ui.add_enabled_ui(open, |ui| switch_row(ui, theme, "Start minimized", Some(DESKTOP.minimized_meta), &mut minimized)).inner;
    let mut changed = false;
    if toggled {
        match crate::autostart::set(open, minimized) {
            // An explicit choice: the one-time default never overrides it.
            Ok(()) => changed = !std::mem::replace(&mut cx.settings.autostart_default_done, true),
            Err(e) => cx.view.toast(format!("Could not change the startup entry: {e}"), true),
        }
    }
    section(ui, cx.theme, "Window");
    // Linux without a tray host: closing quits, whatever the switch says.
    let no_tray = cfg!(target_os = "linux") && (crate::notify::tray_failed() || super::super::sidebar::nav_demo("no-tray"));
    let meta = if no_tray { "No system tray found, so closing the window quits Rustcord." } else { DESKTOP.close_meta };
    let theme = &*cx.theme;
    changed |= ui.add_enabled_ui(!no_tray, |ui| switch_row(ui, theme, DESKTOP.close, Some(meta), &mut cx.settings.minimize_to_tray)).inner;
    changed | updates(ui, cx)
}

/// Updates (docs/UPDATER_DESIGN.md §15.3): install automatically (off while this copy can't
/// update itself or the updater is off), the running version with what the updater is doing,
/// and "Check for updates". True when the settings changed.
fn updates(ui: &mut egui::Ui, cx: &mut UiCx<'_>) -> bool {
    use crate::ui::update;
    let up = update::status();
    section(ui, cx.theme, "Updates");
    let theme = &*cx.theme;
    let can = up.enabled && up.kind != crate::update::Kind::NoticeOnly;
    let auto = &mut cx.settings.auto_update;
    let toggled = ui.add_enabled_ui(can, |ui| switch_row(ui, theme, AUTO_UPDATE, Some(update::auto_meta(&up)), auto)).inner;
    if toggled {
        crate::update::set_auto(*auto);
    }
    let pal = Pal::new(theme);
    let font = egui::FontId::new(pal.ui(), crate::theme::medium_family());
    let button_w = (ui.painter().layout_no_wrap(CHECK_UPDATES.to_owned(), font, pal.text).size().x + 32.0).max(64.0);
    let version = concat!("Version ", env!("CARGO_PKG_VERSION"));
    super::setting_row(ui, theme, version, update::settings_meta(&up).as_deref(), egui::vec2(button_w, 32.0), |ui| {
        if super::super::messages::button(ui, &pal, super::super::messages::Btn::Secondary, CHECK_UPDATES, update::can_check(&up)).clicked() {
            crate::update::check_now();
        }
    });
    toggled
}

/// Streamer Mode (no OBS auto-enable: no process scan).
pub(super) fn streamer(ui: &mut egui::Ui, cx: &mut UiCx<'_>) -> bool {
    let s = &mut cx.settings.streamer;
    let mut changed = switch_row(ui, cx.theme, "Streamer mode", Some("Hides what you pick below while you stream. Also on a keybind (Keybinds)."), &mut s.enabled);
    section(ui, cx.theme, "Options");
    let theme = &*cx.theme;
    ui.add_enabled_ui(s.enabled, |ui| {
        changed |= switch_row(ui, theme, "Hide personal information", Some("Your username, connections and who messaged you."), &mut s.hide_personal_info);
        changed |= switch_row(ui, theme, "Hide invite links", Some("Copy still works."), &mut s.hide_invite_links);
        changed |= switch_row(ui, theme, "Disable notifications", None, &mut s.disable_notifications);
        changed |= switch_row(ui, theme, "Disable sounds", None, &mut s.disable_sounds);
        if cfg!(windows) {
            changed |= switch_row(ui, theme, "Hide window from screen capture", Some("Others see a blank area where Rustcord is."), &mut s.hide_from_capture);
        }
    });
    changed
}

/// Advanced (this device).
pub(super) fn advanced(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    section(ui, cx.theme, "Hardware acceleration");
    hint(ui, cx.theme, "Always on: Rustcord draws with the GPU (OpenGL).");
}

// ---------------------------------------------------------------------------
// Per frame (RustcordApp::ui, every screen)
// ---------------------------------------------------------------------------

/// Applies the settings that act every frame. Saves when a key changed one.
pub(in crate::ui) fn frame(ctx: &egui::Context, cx: &mut UiCx<'_>) {
    let mut save = sync_zoom(ctx, cx);
    let want = crate::ui::motion::animation_time(cx.settings.reduced_motion || crate::ui::motion::os_reduced());
    if ctx.global_style().animation_time != want {
        // egui's programmatic scroll animation (100-300 ms) would still move under Reduced motion.
        ctx.all_styles_mut(|s| {
            s.animation_time = want;
            s.scroll_animation = if want == 0.0 { egui::style::ScrollAnimation::none() } else { Default::default() };
        });
    }
    caret_timeout(ctx);
    // Signed in only; recording a shortcut on the Keybinds page takes every key.
    if cx.view.screen == super::super::Screen::Main && !keybinds::recording(ctx) {
        save |= global_keys(ctx, cx);
    }
    exclude_from_capture(ctx, cx.settings.streamer.enabled && cx.settings.streamer.hide_from_capture);
    if save
        && let Err(e) = cx.settings.save()
    {
        cx.view.toast(format!("Could not save settings: {e}"), true);
    }
}

/// Like Windows' caret timeout: the text caret blinks for 5 s after the last input, then stays
/// on. A blinking caret repaints the whole window twice a second, so a focused composer in a
/// window left in front would never go idle.
fn caret_timeout(ctx: &egui::Context) {
    const BLINK_FOR: f64 = 5.0;
    let id = egui::Id::unique("rc_last_input");
    let (now, input) = ctx.input(|i| (i.time, !i.events.is_empty()));
    let last = ctx.data_mut(|d| {
        if input {
            d.insert_temp(id, now);
        }
        *d.get_temp_mut_or_insert_with(id, || now)
    });
    let blink = now - last < BLINK_FOR;
    if ctx.global_style().visuals.text_cursor.blink != blink {
        ctx.all_styles_mut(|s| s.visuals.text_cursor.blink = blink);
    }
}

/// The window's tray life, every pass (`RustcordApp::logic`), also while it is hidden: tray
/// menu clicks, closing to the tray (× and Alt+F4) unless quitting, `--minimized` at startup;
/// and the mute state for the tray menu's check marks. True when one of Rustcord's notifications
/// was clicked (the window is shown then too).
pub(in crate::ui) fn logic(ctx: &egui::Context, view: &mut ViewState, cmd: &CommandSender, settings: &mut Settings) -> bool {
    let mut clicked = false;
    let quitting = egui::Id::unique("rc_quitting");
    let started = egui::Id::unique("rc_started");
    if !ctx.data(|d| d.get_temp::<bool>(started).unwrap_or(false)) {
        ctx.data_mut(|d| d.insert_temp(started, true));
        // Started at sign-in (`--minimized` in the Run value): straight to the tray.
        if crate::notify::has_tray() && std::env::args().any(|a| a == "--minimized") {
            to_tray(ctx);
        }
    }
    for action in crate::notify::take_actions() {
        match action {
            TrayAction::Show => show_window(ctx),
            TrayAction::Notification => {
                show_window(ctx);
                clicked = true;
            }
            TrayAction::ToggleMute | TrayAction::ToggleDeafen => toggle_voice(view, cmd, action == TrayAction::ToggleMute),
            TrayAction::PauseHour | TrayAction::PauseTomorrow | TrayAction::Resume => {
                let now = chrono::Local::now();
                let until = match action {
                    TrayAction::PauseHour => Some(now + chrono::TimeDelta::hours(1)),
                    TrayAction::PauseTomorrow => Some(crate::alerts::tomorrow_morning(now)),
                    _ => None,
                };
                view.toast(crate::alerts::pause(&mut settings.quiet_hours, until, now), false);
                if let Err(e) = settings.save() {
                    view.toast(format!("Could not save settings: {e}"), true);
                }
            }
            TrayAction::Quit => {
                ctx.data_mut(|d| d.insert_temp(quitting, true));
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
        }
    }
    // The tray went away while the window waited there (Linux: no tray host after all).
    if in_tray(ctx) && !crate::notify::has_tray() {
        show_window(ctx);
    }
    let quit = ctx.data(|d| d.get_temp::<bool>(quitting).unwrap_or(false));
    // Without a tray icon (screenshot runs, a failed icon) closing quits as before.
    if !quit && settings.minimize_to_tray && crate::notify::has_tray() && ctx.input(|i| i.viewport().close_requested()) {
        ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        to_tray(ctx);
        if !settings.tray_hint_shown {
            settings.tray_hint_shown = true;
            if let Err(e) = settings.save() {
                view.toast(format!("Could not save settings: {e}"), true);
            }
            crate::notify::show(crate::config::APP_NAME, DESKTOP.first_close);
        }
    }
    crate::notify::set_voice_state(view.self_mute, view.self_deaf);
    crate::notify::set_paused(settings.quiet_hours.paused(chrono::Utc::now().timestamp()));
    // Linux, macOS: push to talk from this window's keys (no global key state there).
    #[cfg(not(windows))]
    crate::voice::audio::set_ptt_held(view.window_focused && settings.push_to_talk && ctx.input(|i| super::voice::key_held(i, settings.push_to_talk_key)));
    clicked
}

/// The window waits in the tray (`to_tray` .. `show_window`).
fn in_tray_id() -> egui::Id {
    egui::Id::unique("rc_in_tray")
}

/// The window waits in the tray (the updater's `hidden`, `RustcordApp::logic`).
pub(in crate::ui) fn in_tray(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(in_tray_id()).unwrap_or(false))
}

/// Hides the window, taskbar button included. Minimized as well, so eframe runs only `logic`
/// (no ui pass, no painting) while it waits in the tray. macOS: hidden only (miniaturizing an
/// ordered-out window puts it back in the Dock), and the app hides itself.
fn to_tray(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(in_tray_id(), true));
    ctx.send_viewport_cmd(ViewportCommand::Visible(false));
    if !cfg!(target_os = "macos") {
        ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
    }
    crate::notify::to_tray_os();
}

/// Back from the tray (tray click, a second start, a notification click).
fn show_window(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(in_tray_id(), false));
    crate::notify::shown_os();
    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(ViewportCommand::Focus);
}

/// Streamer Mode's "Hide window from screen capture": the window's display affinity follows
/// `on`, set when it changes (retried each frame until the window is active: the UI thread's
/// active window is ours then, no window handle needed).
#[cfg(windows)]
fn exclude_from_capture(ctx: &egui::Context, on: bool) {
    let id = egui::Id::unique("rc_capture_hidden");
    if ctx.data(|d| d.get_temp::<bool>(id).unwrap_or(false)) == on {
        return;
    }
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE};
    // SAFETY: plain Win32 calls on this thread's own active window (null: not active yet).
    let done = unsafe {
        let hwnd = GetActiveWindow();
        !hwnd.is_null() && SetWindowDisplayAffinity(hwnd, if on { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE }) != 0
    };
    if done {
        ctx.data_mut(|d| d.insert_temp(id, on));
    }
}

/// Linux, macOS: not offered (the switch is Windows only).
#[cfg(not(windows))]
fn exclude_from_capture(_: &egui::Context, _: bool) {}

/// `Settings::zoom` -> egui, and egui's own Ctrl +/−/0 -> `Settings::zoom` (true: save).
fn sync_zoom(ctx: &egui::Context, cx: &mut UiCx<'_>) -> bool {
    let id = egui::Id::unique("rc_zoom_applied");
    let applied: Option<f32> = ctx.data(|d| d.get_temp(id));
    let want = cx.settings.zoom.clamp(0.5, 2.0);
    let (set, adopt) = match applied {
        // Startup, or the slider moved: apply (egui takes it at the next pass).
        None => (true, false),
        Some(a) if a != want => (true, false),
        // A zoom key changed egui's factor since.
        Some(a) => (false, (ctx.zoom_factor() - a).abs() > 1e-3),
    };
    if set {
        ctx.set_zoom_factor(want);
        ctx.data_mut(|d| d.insert_temp(id, want));
    } else if adopt {
        let z = (ctx.zoom_factor() * 100.0).round() / 100.0;
        cx.settings.zoom = z;
        ctx.data_mut(|d| d.insert_temp(id, z));
        if z != ctx.zoom_factor() {
            ctx.set_zoom_factor(z);
        }
    }
    adopt
}

/// Toggle Mute / Deafen (like the user panel's buttons) and Toggle Streamer Mode, then the
/// switcher's keys. True when a setting changed (save).
fn global_keys(ctx: &egui::Context, cx: &mut UiCx<'_>) -> bool {
    let pressed = |b| keybinds::shortcut(cx.settings, b).is_some_and(|k| ctx.input_mut(|i| i.consume_shortcut(&k)));
    let (mic, phones, streamer) = (pressed(Bind::ToggleMute), pressed(Bind::ToggleDeafen), pressed(Bind::ToggleStreamerMode));
    if mic || phones {
        toggle_voice(cx.view, cx.cmd, mic);
    }
    if streamer {
        let s = &mut cx.settings.streamer;
        s.enabled = !s.enabled;
        cx.view.toast(if s.enabled { "Streamer mode is on" } else { "Streamer mode is off" }.to_owned(), false);
    }
    // The quick switcher's keys (ui/switcher.rs); before the chat, so a focused field never sees them.
    super::super::switcher::keys(ctx, cx);
    streamer
}

/// Mute (`mic`) or Deafen toggled like the user panel's buttons (keybind, tray menu).
fn toggle_voice(view: &mut ViewState, cmd: &CommandSender, mic: bool) {
    let (mute, deaf) = (view.self_mute, view.self_deaf);
    let (m, d) = super::super::sidebar::toggle_voice(mute, deaf, mic);
    (view.self_mute, view.self_deaf) = (m, d);
    if d != deaf {
        cmd.send(Command::SetSelfDeaf(d));
    }
    if m != mute {
        cmd.send(Command::SetSelfMute(m));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn caret_stops_blinking_5_s_after_input() {
        let ctx = egui::Context::default();
        let frame = |time: f64, key: bool| {
            let events = if key { vec![egui::Event::Text("a".into())] } else { vec![] };
            let input = egui::RawInput { time: Some(time), events, ..Default::default() };
            ctx.run_pass(input, super::caret_timeout).drop_without_applying_deltas();
            ctx.global_style().visuals.text_cursor.blink
        };
        assert!(frame(0.0, false), "blinks after the start");
        assert!(!frame(5.5, false), "no input for 5 s: steady");
        assert!(frame(6.0, true), "typing wakes it");
        assert!(frame(10.5, false));
        assert!(!frame(11.5, false));
    }
}
