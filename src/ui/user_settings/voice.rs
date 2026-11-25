//! Voice & Video: input / output device (`ListVoiceDevices` when the page opens, ⟳ to list
//! again), input volume, input mode (voice activity with its threshold over my live level, or push-to-talk
//! with a captured key), mute / deafen, and the camera (`VideoCommand::ListCameras`; the pick
//! is what the call UI starts; its preview while it is on in a call). Commands go to the local voice task, no request is sent. The
//! choices are kept in `Settings` (this device) and re-sent at startup (`restore`).
//! OWNERSHIP: user-settings engineer (docs/SOCIAL_UI_DESIGN.md).

use super::{field_label, switch_row};
use crate::backend::{Command, CommandSender};
use crate::config::Settings;
use crate::ui::messages::Pal;
use crate::ui::{hint, section};
use crate::voice::InputMode;
use crate::voice::video::{VideoCommand, VideoKey, capture};
use egui::{Key, Modifiers, PointerButton, Rect, RichText, Sense, Stroke, vec2};

/// The page's per-opening state (the choices live in `Settings`).
#[derive(Default)]
pub(super) struct VoiceForm {
    /// Device lists were requested in this opening.
    listed: bool,
    /// Waiting for the push-to-talk key; `mods` = modifiers when it started (a newly held one
    /// counts as the key).
    capturing: Option<Modifiers>,
    /// A choice changed and is not saved yet (saved once no button is held, not per drag step).
    dirty: bool,
}

impl VoiceForm {
    pub(super) fn opened(&mut self) {
        self.listed = false;
        self.capturing = None;
    }
}

/// The saved input mode (push-to-talk only with a key).
fn input_mode(s: &Settings) -> InputMode {
    match s.push_to_talk_key {
        vk if s.push_to_talk && vk != 0 => InputMode::PushToTalk { vk },
        _ => InputMode::VoiceActivity { threshold_db: s.voice_threshold_db },
    }
}

/// At startup: hands the saved devices and input mode to the voice task (local, no request).
pub(in crate::ui) fn restore(s: &Settings, cmd: &CommandSender) {
    crate::voice::audio::set_input_volume(s.voice_input_volume);
    if s.voice_input.is_some() || s.voice_output.is_some() {
        cmd.send(Command::SetVoiceDevices { input: s.voice_input.clone(), output: s.voice_output.clone() });
    }
    if input_mode(s) != InputMode::default() {
        cmd.send(Command::SetVoiceInputMode(input_mode(s)));
    }
}

/// The page. True when `Settings` should be saved now.
pub(super) fn show(ui: &mut egui::Ui, cx: &mut crate::ui::UiCx<'_>) -> bool {
    let pal = Pal::new(cx.theme);
    let f = &mut cx.view.user_settings_ui.voice;
    if !f.listed {
        f.listed = true;
        cx.cmd.send(Command::ListVoiceDevices);
        if capture::SUPPORTED {
            cx.cmd.send(Command::Video(VideoCommand::ListCameras));
        }
    }

    // Devices: two columns when there is room.
    let v = &cx.state.voice;
    let s = &mut *cx.settings;
    let mut devices = false;
    let w = ui.available_width();
    let two = w >= 460.0;
    let col = if two { (w - 52.0) / 2.0 } else { w - 40.0 };
    let row = |ui: &mut egui::Ui| {
        ui.spacing_mut().item_spacing = vec2(12.0, 6.0);
        for (label, list, pick) in [("Input device", &v.inputs, &mut s.voice_input), ("Output device", &v.outputs, &mut s.voice_output)] {
            ui.vertical(|ui| {
                ui.set_width(col);
                field_label(ui, &pal, label);
                egui::ComboBox::from_id_salt(("rc_voice_dev", label)).width(col).selected_text(pick.as_deref().unwrap_or("Default")).icon(super::combo_icon).show_ui(ui, |ui| {
                    devices |= ui.selectable_value(pick, None, "Default").changed();
                    for name in list {
                        if ui.selectable_label(pick.as_deref() == Some(name), name).clicked() {
                            *pick = Some(name.clone());
                            devices = true;
                        }
                    }
                });
            });
        }
        if crate::ui::messages::vector_button(ui, &pal, crate::ui::icons::Icon::Refresh, "Refresh devices", 28.0, false, pal.text).clicked() {
            cx.cmd.send(Command::ListVoiceDevices);
            if capture::SUPPORTED {
                cx.cmd.send(Command::Video(VideoCommand::ListCameras));
            }
        }
    };
    // Stacked when narrow: a wrapping row does not wrap a sized `vertical` child.
    if two {
        ui.horizontal_wrapped(row);
    } else {
        ui.vertical(row);
    }
    if devices {
        cx.cmd.send(Command::SetVoiceDevices { input: s.voice_input.clone(), output: s.voice_output.clone() });
        f.dirty = true;
    }

    ui.add_space(16.0);
    field_label(ui, &pal, "Input volume");
    ui.spacing_mut().slider_width = (col - 64.0).max(120.0);
    if ui.add(egui::Slider::new(&mut s.voice_input_volume, 0..=crate::voice::audio::MAX_INPUT_VOLUME).suffix(" %")).changed() {
        // Live, also in a call: the audio thread reads it for every frame.
        crate::voice::audio::set_input_volume(s.voice_input_volume);
        f.dirty = true;
    }

    section(ui, cx.theme, "Input mode");
    let old_mode = input_mode(s);
    let mut mode = old_mode;
    // Two radio rows, 8 px apart; the unselected one ringed so it reads on dark pages.
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.visuals_mut().widgets.inactive.bg_stroke = Stroke::new(1.5, pal.tk.edge);
        let va = InputMode::VoiceActivity { threshold_db: s.voice_threshold_db };
        if ui.radio(matches!(mode, InputMode::VoiceActivity { .. }), "Voice activity").clicked() {
            mode = va;
        }
        if ui.radio(matches!(mode, InputMode::PushToTalk { .. }), "Push to talk").clicked() && !matches!(mode, InputMode::PushToTalk { .. }) {
            if s.push_to_talk_key == 0 {
                // No key yet: ask for one first; the mode switches once it is pressed.
                f.capturing = Some(ui.input(|i| i.modifiers));
            } else {
                mode = InputMode::PushToTalk { vk: s.push_to_talk_key };
            }
        }
    });
    ui.add_space(16.0);
    match &mut mode {
        InputMode::VoiceActivity { threshold_db } => {
            field_label(ui, &pal, "Input sensitivity");
            sensitivity(ui, &pal, threshold_db, v.channel_id.is_some().then_some(v.level));
        }
        InputMode::PushToTalk { .. } => {}
    }
    if matches!(mode, InputMode::PushToTalk { .. }) || f.capturing.is_some() {
        field_label(ui, &pal, "Shortcut");
        let label = match f.capturing {
            Some(_) => "Press a key…".to_owned(),
            None => key_name(s.push_to_talk_key),
        };
        let resp = ui.add(egui::Button::new(RichText::new(label).color(pal.text)).min_size(vec2(160.0, 32.0)).selected(f.capturing.is_some()));
        let resp = resp.on_hover_text(if f.capturing.is_some() { "Esc cancels" } else { "Record a key" });
        if resp.clicked() && f.capturing.is_none() {
            f.capturing = Some(ui.input(|i| i.modifiers));
        } else if let Some(start) = f.capturing {
            match capture(ui, start) {
                Some(Some(vk)) => {
                    (s.push_to_talk_key, f.capturing) = (vk, None);
                    mode = InputMode::PushToTalk { vk };
                }
                Some(None) => f.capturing = None,
                None => {}
            }
        }
        if !cfg!(windows) {
            hint(ui, cx.theme, "Push to talk works while the Rustcord window is in front.");
        }
    }
    if mode != old_mode {
        match mode {
            InputMode::VoiceActivity { threshold_db } => (s.push_to_talk, s.voice_threshold_db) = (false, threshold_db),
            InputMode::PushToTalk { vk } => (s.push_to_talk, s.push_to_talk_key) = (true, vk),
        }
        f.dirty = true;
        cx.cmd.send(Command::SetVoiceInputMode(mode));
    }

    section(ui, cx.theme, "Microphone and sound");
    let (old_mute, old_deaf) = (cx.view.self_mute, cx.view.self_deaf);
    // Shown like the user panel: deafened counts as muted.
    let (mut mute, mut deaf) = (old_mute || old_deaf, old_deaf);
    let m = switch_row(ui, cx.theme, "Mute", None, &mut mute);
    let d = switch_row(ui, cx.theme, "Deafen", None, &mut deaf);
    if m || d {
        let (mute, deaf) = crate::ui::sidebar::toggle_voice(old_mute, old_deaf, m);
        (cx.view.self_mute, cx.view.self_deaf) = (mute, deaf);
        if deaf != old_deaf {
            cx.cmd.send(Command::SetSelfDeaf(deaf));
        }
        if mute != old_mute {
            cx.cmd.send(Command::SetSelfMute(mute));
        }
    }

    section(ui, cx.theme, "Video");
    field_label(ui, &pal, "Camera");
    if !capture::SUPPORTED {
        hint(ui, cx.theme, capture::NO_CAMERA);
        let f = &mut cx.view.user_settings_ui.voice;
        let save = f.dirty && !ui.input(|i| i.pointer.any_down());
        f.dirty &= !save;
        return save;
    }
    let cams = &cx.state.voice.video.cameras;
    let camera = &mut cx.settings.camera;
    let shown = camera.as_deref().and_then(|id| cams.iter().find(|c| c.id == id)).map_or("Default", |c| c.name.as_str());
    let mut picked = false;
    egui::ComboBox::from_id_salt("rc_camera").width(col.min(ui.available_width())).selected_text(shown).icon(super::combo_icon).show_ui(ui, |ui| {
        picked |= ui.selectable_value(camera, None, "Default").changed();
        for c in cams {
            if ui.selectable_label(camera.as_deref() == Some(c.id.as_str()), &c.name).clicked() {
                *camera = Some(c.id.clone());
                picked = true;
            }
        }
    });
    // Preview: my camera's own frames (`User(me)`) while it is on. shortcut: no off-call test,
    // StartCamera needs a call (VIDEO_DESIGN); a local-only preview command would lift that.
    let pw = col.min(ui.available_width()).min(360.0);
    let (rect, _) = ui.allocate_exact_size(vec2(pw, pw * 9.0 / 16.0), Sense::hover());
    let me = cx.state.me.as_ref().map(|u| VideoKey::User(u.id)).filter(|_| cx.state.voice.video.camera_on);
    // Scrolled out of view: not painted, so `call::sync` stops the preview frames.
    let live = me.as_ref().filter(|_| ui.is_rect_visible(rect)).is_some_and(|k| crate::ui::call::paint_video(ui.painter(), &mut cx.view.call_ui, k, rect, true));
    if !live {
        ui.painter().rect_filled(rect, pal.card_radius(), cx.theme.colors.sidebar.to_color32());
        let camera = egui::Rect::from_center_size(rect.center() - vec2(0.0, 10.0), egui::Vec2::splat(32.0));
        crate::ui::icons::paint(ui.painter(), crate::ui::icons::Icon::Camera, camera, pal.muted, cx.theme.colors.sidebar.to_color32());
        let text = if me.is_some() { "Starting camera…" } else { "Turn your camera on in a call" };
        ui.painter().text(rect.center() + vec2(0.0, 24.0), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(pal.meta()), pal.muted);
    }
    let f = &mut cx.view.user_settings_ui.voice;
    f.dirty |= picked;
    let save = f.dirty && !ui.input(|i| i.pointer.any_down());
    f.dirty &= !save;
    save
}

/// Threshold slider (-100..=0 dB) over my live input level (`level` 0..=1 on the same scale;
/// None outside a call). True when changed.
fn sensitivity(ui: &mut egui::Ui, pal: &Pal, db: &mut f32, level: Option<f32>) -> bool {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 24.0), Sense::click_and_drag());
    let track = Rect::from_center_size(rect.center(), vec2(w - 16.0, 8.0));
    let x_of = |db: f32| egui::lerp(track.left()..=track.right(), (db + 100.0) / 100.0);
    let mut changed = false;
    if let Some(p) = resp.interact_pointer_pos().filter(|_| resp.dragged() || resp.clicked()) {
        let new = (((p.x - track.left()) / track.width()).clamp(0.0, 1.0) * 100.0 - 100.0).round();
        changed = new != *db;
        *db = new;
    }
    let p = ui.painter();
    // Below the threshold amber, above it green (transmitting), like Discord's bar.
    p.rect_filled(Rect::from_min_max(track.min, egui::pos2(x_of(*db), track.max.y)), 4.0, egui::Color32::from_rgb(0xe0, 0xa2, 0x1b).gamma_multiply(0.7));
    p.rect_filled(Rect::from_min_max(egui::pos2(x_of(*db), track.min.y), track.max), 4.0, egui::Color32::from_rgb(0x3f, 0xb9, 0x50).gamma_multiply(0.7));
    if let Some(l) = level {
        let lx = egui::lerp(track.left()..=track.right(), l.clamp(0.0, 1.0));
        p.rect_filled(Rect::from_min_max(track.min, egui::pos2(lx, track.max.y)), 4.0, pal.text.gamma_multiply(0.5));
    }
    let knob = egui::pos2(x_of(*db), track.center().y);
    p.circle(knob, 9.0, egui::Color32::WHITE, Stroke::new(1.0, pal.line()));
    let resp = resp.on_hover_text(format!("{:.0} dB", *db));
    resp.widget_info(|| egui::WidgetInfo::slider(true, f64::from(*db), "Input sensitivity"));
    changed
}

/// One frame of key capture: Some(Some(vk)) = a key was pressed, Some(None) = cancelled (Esc,
/// consumed so the layer stays open), None = still waiting.
fn capture(ui: &mut egui::Ui, start: Modifiers) -> Option<Option<u16>> {
    ui.input_mut(|i| {
        if i.consume_key(Modifiers::NONE, Key::Escape) {
            return Some(None);
        }
        for ev in &i.events {
            match ev {
                egui::Event::Key { key, pressed: true, .. } if let Some(vk) = vk_code(*key) => return Some(Some(vk)),
                egui::Event::PointerButton { button: PointerButton::Extra1, pressed: true, .. } => return Some(Some(0x05)),
                egui::Event::PointerButton { button: PointerButton::Extra2, pressed: true, .. } => return Some(Some(0x06)),
                _ => {}
            }
        }
        // Modifiers alone send no key event: a newly held one is the key.
        let m = i.modifiers;
        [(m.shift && !start.shift, 0x10), (m.ctrl && !start.ctrl, 0x11), (m.alt && !start.alt, 0x12)]
            .into_iter()
            .find(|(new, _)| *new)
            .map(|(_, vk)| Some(vk))
    })
}

/// Windows virtual-key code of an egui key (letters, digits, F1-F24, navigation, US punctuation).
fn vk_code(key: Key) -> Option<u16> {
    use Key::*;
    let (k, between) = (key as u16, |a: Key, b: Key| (a as u16..=b as u16).contains(&(key as u16)));
    Some(match key {
        _ if between(A, Z) => 0x41 + k - A as u16,
        _ if between(Num0, Num9) => 0x30 + k - Num0 as u16,
        _ if between(F1, F24) => 0x70 + k - F1 as u16,
        Space => 0x20,
        Tab => 0x09,
        Backspace => 0x08,
        Enter => 0x0D,
        Insert => 0x2D,
        Delete => 0x2E,
        Home => 0x24,
        End => 0x23,
        PageUp => 0x21,
        PageDown => 0x22,
        ArrowLeft => 0x25,
        ArrowUp => 0x26,
        ArrowRight => 0x27,
        ArrowDown => 0x28,
        Backtick => 0xC0,
        Minus => 0xBD,
        Equals => 0xBB,
        OpenBracket => 0xDB,
        CloseBracket => 0xDD,
        Backslash => 0xDC,
        Semicolon => 0xBA,
        Quote => 0xDE,
        Comma => 0xBC,
        Period => 0xBE,
        Slash => 0xBF,
        _ => return None,
    })
}

/// The push-to-talk key `vk` is held in this window's input (Linux, macOS: no global key state).
#[cfg_attr(windows, allow(dead_code))]
pub(super) fn key_held(i: &egui::InputState, vk: u16) -> bool {
    match vk {
        0 => false,
        0x05 => i.pointer.button_down(PointerButton::Extra1),
        0x06 => i.pointer.button_down(PointerButton::Extra2),
        0x10 => i.modifiers.shift,
        0x11 => i.modifiers.ctrl,
        0x12 => i.modifiers.alt,
        _ => Key::ALL.iter().find(|k| vk_code(**k) == Some(vk)).is_some_and(|k| i.key_down(*k)),
    }
}

fn key_name(vk: u16) -> String {
    match vk {
        0 => "Record keybind".into(),
        0x05 => "Mouse 4".into(),
        0x06 => "Mouse 5".into(),
        0x10 => "Shift".into(),
        0x11 => "Ctrl".into(),
        0x12 => "Alt".into(),
        _ => Key::ALL.iter().find(|k| vk_code(**k) == Some(vk)).map_or_else(|| format!("Key {vk:#04x}"), |k| k.symbol_or_name().to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_key_codes() {
        assert_eq!(vk_code(Key::A), Some(0x41));
        assert_eq!(vk_code(Key::Z), Some(0x5A));
        assert_eq!(vk_code(Key::Num0), Some(0x30));
        assert_eq!(vk_code(Key::Num9), Some(0x39));
        assert_eq!(vk_code(Key::F1), Some(0x70));
        assert_eq!(vk_code(Key::F24), Some(0x87));
        assert_eq!(vk_code(Key::Backtick), Some(0xC0));
        assert_eq!(vk_code(Key::Escape), None);
        assert_eq!(key_name(0x56), "V");
        assert_eq!(key_name(0x05), "Mouse 4");
        assert_eq!(key_name(0), "Record keybind");
    }

    #[test]
    fn push_to_talk_key_from_window_input() {
        let ctx = egui::Context::default();
        let held = |events: Vec<egui::Event>, vk: u16| {
            let mut out = false;
            ctx.run_pass(egui::RawInput { events, ..Default::default() }, |ctx| out = ctx.input(|i| key_held(i, vk))).drop_without_applying_deltas();
            out
        };
        let key = |pressed| egui::Event::Key { key: Key::V, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
        assert!(held(vec![key(true)], 0x56), "V down");
        assert!(held(vec![], 0x56), "still held");
        assert!(!held(vec![], 0x41), "another key");
        assert!(!held(vec![key(false)], 0x56), "released");
        assert!(!held(vec![], 0));
    }
}
