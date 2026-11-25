//! Settings → Keybinds (docs/SETTINGS_NOTES.md §3 Keybinds, this device only): the app's
//! shortcuts with Discord's defaults, each re-recordable (click, press the new combination;
//! Esc cancels, Backspace clears; Shift+Esc can be recorded). `shortcut` is what the call sites read (inbox, search, pins,
//! accept call, the quick switcher's keys in `ui/switcher.rs`); mute / deafen / Streamer Mode are handled in `app::frame`. Push to talk stays
//! on Voice & Video (it needs a key-up, outside egui).
//! OWNERSHIP: user-settings engineer.

use super::super::messages::Pal;
use super::super::{UiCx, hint, section};
use crate::config::Settings;
use egui::{Key, KeyboardShortcut, ModifierNames, Modifiers, RichText, vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bind {
    ToggleMute,
    ToggleDeafen,
    ToggleStreamerMode,
    Inbox,
    Search,
    Pins,
    AcceptCall,
    QuickSwitcher,
    Shortcuts,
    PrevChannel,
    NextChannel,
    PrevUnread,
    NextUnread,
    PrevServer,
    NextServer,
    MarkServerRead,
}

impl Bind {
    pub(super) const ALL: [Bind; 16] = [
        Bind::ToggleMute,
        Bind::ToggleDeafen,
        Bind::ToggleStreamerMode,
        Bind::Inbox,
        Bind::Search,
        Bind::Pins,
        Bind::AcceptCall,
        Bind::QuickSwitcher,
        Bind::Shortcuts,
        Bind::PrevChannel,
        Bind::NextChannel,
        Bind::PrevUnread,
        Bind::NextUnread,
        Bind::PrevServer,
        Bind::NextServer,
        Bind::MarkServerRead,
    ];

    /// Key in `Settings::keybinds`.
    fn name(self) -> &'static str {
        match self {
            Bind::ToggleMute => "toggle_mute",
            Bind::ToggleDeafen => "toggle_deafen",
            Bind::ToggleStreamerMode => "toggle_streamer_mode",
            Bind::Inbox => "inbox",
            Bind::Search => "search",
            Bind::Pins => "pins",
            Bind::AcceptCall => "accept_call",
            Bind::QuickSwitcher => "quick_switcher",
            Bind::Shortcuts => "shortcuts",
            Bind::PrevChannel => "prev_channel",
            Bind::NextChannel => "next_channel",
            Bind::PrevUnread => "prev_unread",
            Bind::NextUnread => "next_unread",
            Bind::PrevServer => "prev_server",
            Bind::NextServer => "next_server",
            Bind::MarkServerRead => "mark_server_read",
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Bind::ToggleMute => "Toggle mute",
            Bind::ToggleDeafen => "Toggle deafen",
            Bind::ToggleStreamerMode => "Toggle streamer mode",
            Bind::Inbox => "Open inbox",
            Bind::Search => "Search the Channel",
            Bind::Pins => "Pinned messages",
            Bind::AcceptCall => "Answer incoming call",
            Bind::QuickSwitcher => "Open quick switcher",
            Bind::Shortcuts => "Show keyboard shortcuts",
            Bind::PrevChannel => "Previous channel",
            Bind::NextChannel => "Next channel",
            Bind::PrevUnread => "Previous unread channel",
            Bind::NextUnread => "Next unread channel",
            Bind::PrevServer => "Previous server",
            Bind::NextServer => "Next server",
            Bind::MarkServerRead => "Mark server as read",
        }
    }

    fn default_text(self) -> &'static str {
        match self {
            Bind::ToggleMute => "Ctrl+Shift+M",
            Bind::ToggleDeafen => "Ctrl+Shift+D",
            Bind::ToggleStreamerMode => "",
            Bind::Inbox => "Ctrl+I",
            Bind::Search => "Ctrl+F",
            Bind::Pins => "Ctrl+P",
            Bind::AcceptCall => "Ctrl+Enter",
            Bind::QuickSwitcher => "Ctrl+K",
            Bind::Shortcuts => "Ctrl+Slash",
            Bind::PrevChannel => "Alt+Up",
            Bind::NextChannel => "Alt+Down",
            Bind::PrevUnread => "Alt+Shift+Up",
            Bind::NextUnread => "Alt+Shift+Down",
            Bind::PrevServer => "Ctrl+Alt+Up",
            Bind::NextServer => "Ctrl+Alt+Down",
            Bind::MarkServerRead => "Shift+Escape",
        }
    }
}

/// The shortcut bound to `b` (the user's, else the default); None = unbound.
pub fn shortcut(s: &Settings, b: Bind) -> Option<KeyboardShortcut> {
    parse(s.keybinds.get(b.name()).map_or(b.default_text(), String::as_str))
}

/// "Ctrl+Shift+M" (egui key names; "Ctrl" is the platform command key).
pub(in crate::ui) fn parse(text: &str) -> Option<KeyboardShortcut> {
    let (mut mods, mut key) = (Modifiers::NONE, None);
    for part in text.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "cmd" => mods |= Modifiers::COMMAND,
            "shift" => mods.shift = true,
            "alt" => mods.alt = true,
            _ => key = Some(Key::from_name(part)?),
        }
    }
    Some(KeyboardShortcut::new(mods, key?))
}

fn format(k: &KeyboardShortcut) -> String {
    k.format(&ModifierNames::NAMES, false)
}

/// Bind being recorded, in egui temp memory.
fn recording_id() -> egui::Id {
    egui::Id::unique("rc_keybind_recording")
}

/// The page. True when `cx.settings` changed.
pub(super) fn show(ui: &mut egui::Ui, cx: &mut UiCx<'_>) -> bool {
    let pal = Pal::new(cx.theme);
    let ctx = ui.ctx().clone();
    let mut recording: Option<usize> = ctx.data(|d| d.get_temp(recording_id()));
    let mut changed = false;
    // The next key press (with its modifiers) becomes the shortcut; Esc cancels, Backspace clears.
    if let Some(i) = recording {
        let pressed = ctx.input_mut(|inp| {
            let hit = inp.events.iter().find_map(|e| match e {
                egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                _ => None,
            });
            if hit.is_some() {
                inp.events.retain(|e| !matches!(e, egui::Event::Key { .. } | egui::Event::Text(_)));
            }
            hit
        });
        if let Some((key, mods)) = pressed {
            let b = Bind::ALL[i];
            let text = match key {
                Key::Escape if mods.is_none() => None,
                Key::Backspace | Key::Delete => Some(String::new()),
                _ => Some(format(&KeyboardShortcut::new(mods, key))),
            };
            if let Some(t) = text {
                if t == b.default_text() {
                    cx.settings.keybinds.remove(b.name());
                } else {
                    cx.settings.keybinds.insert(b.name().into(), t);
                }
                changed = true;
            }
            recording = None;
        }
    }
    hint(ui, cx.theme, "Click a shortcut, then press the new keys. Esc cancels, Backspace removes it.");
    section(ui, cx.theme, "Keybinds");
    for (i, b) in Bind::ALL.into_iter().enumerate() {
        if b == Bind::QuickSwitcher {
            section(ui, cx.theme, "Navigation");
        }
        ui.horizontal(|ui| {
            ui.set_min_height(36.0);
            ui.label(RichText::new(b.label()).color(pal.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let custom = cx.settings.keybinds.contains_key(b.name());
                if custom && super::super::messages::vector_button(ui, &pal, crate::ui::icons::Icon::Reset, "Reset to default", 28.0, false, pal.text).clicked() {
                    cx.settings.keybinds.remove(b.name());
                    changed = true;
                }
                let text = match (recording == Some(i), shortcut(cx.settings, b)) {
                    (true, _) => "Press keys…".to_owned(),
                    (false, Some(k)) => format(&k),
                    (false, None) => "None".to_owned(),
                };
                // Recording: temper blue, the keyboard-focus colour (never the accent).
                let color = if recording == Some(i) { pal.link } else { pal.text };
                let btn = egui::Button::new(RichText::new(text).color(color).monospace()).min_size(vec2(170.0, 30.0));
                if ui.add(btn).on_hover_text("Record a new shortcut").clicked() {
                    recording = if recording == Some(i) { None } else { Some(i) };
                }
            });
        });
    }
    section(ui, cx.theme, "Voice");
    ui.horizontal(|ui| {
        ui.label(RichText::new("Push to Talk").color(pal.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.link("Set in Voice & Video").clicked() {
                cx.view.settings_page = super::Page::VoiceVideo;
            }
        });
    });
    section(ui, cx.theme, "Built in");
    for (what, keys) in [("Close settings, popups and dialogs", "Esc"), ("Mark channel as read", "Esc"), ("Zoom in / out / reset", "Ctrl + / Ctrl − / Ctrl 0"), ("New line in a message", "Shift+Enter")] {
        ui.horizontal(|ui| {
            ui.label(RichText::new(what).color(pal.muted));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(RichText::new(keys).monospace().color(pal.muted)));
        });
    }
    ctx.data_mut(|d| match recording {
        Some(i) => _ = d.insert_temp(recording_id(), i),
        None => d.remove::<usize>(recording_id()),
    });
    changed
}

/// A shortcut is being recorded (the global keybinds wait meanwhile).
pub(super) fn recording(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<usize>(recording_id()).is_some())
}

/// Leaving the page stops a recording.
pub(super) fn stop_recording(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<usize>(recording_id()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_parse_format_and_override() {
        for b in Bind::ALL {
            let text = b.default_text();
            assert_eq!(parse(text).map(|k| format(&k)).unwrap_or_default(), text, "{b:?}");
        }
        let mut s = Settings::default();
        assert_eq!(shortcut(&s, Bind::Inbox), Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::I)));
        s.keybinds.insert("inbox".into(), "Alt+Shift+F5".into());
        assert_eq!(shortcut(&s, Bind::Inbox), Some(KeyboardShortcut::new(Modifiers::ALT | Modifiers::SHIFT, Key::F5)));
        s.keybinds.insert("inbox".into(), String::new());
        assert_eq!(shortcut(&s, Bind::Inbox), None);
        assert_eq!(parse("Ctrl+Nope"), None);
        assert_eq!(parse("Shift+Escape"), Some(KeyboardShortcut::new(Modifiers::SHIFT, Key::Escape)));
        assert_eq!(parse("Ctrl+Slash"), Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::Slash)));
        assert_eq!(parse("Ctrl+Minus").map(|k| k.logical_key), Some(Key::Minus));
    }
}
