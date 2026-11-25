//! Folder layout and user settings.
//!
//! Default locations (via `directories`):
//! - Windows: config `%APPDATA%\Rustcord\config`, cache `%LOCALAPPDATA%\Rustcord\cache`
//! - Linux:   `~/.config/rustcord`, `~/.cache/rustcord`
//! - macOS:   `~/Library/Application Support/Rustcord`, `~/Library/Caches/Rustcord`
//!
//! `RUSTCORD_HOME=<dir>` overrides both (portable mode / tests): `<dir>/config`, `<dir>/cache`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const APP_NAME: &str = "Rustcord";

#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl Paths {
    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join("settings.toml")
    }
    /// User theme files (`*.toml`).
    pub fn themes_dir(&self) -> PathBuf {
        self.config_dir.join("themes")
    }
    /// Local profile overrides (banner/avatar copies + overrides.toml).
    pub fn overrides_dir(&self) -> PathBuf {
        self.config_dir.join("overrides")
    }
    /// Discord's device cookies and the logged-out fingerprint (`api::auth`); private to the user.
    pub fn device_file(&self) -> PathBuf {
        self.config_dir.join("device.json")
    }
    /// Snapshot of guilds/channels/DMs for instant startup.
    pub fn snapshot_file(&self) -> PathBuf {
        self.cache_dir.join("snapshot.json")
    }
    /// Downloaded avatars/icons/emoji/attachments.
    pub fn images_dir(&self) -> PathBuf {
        self.cache_dir.join("images")
    }
}

pub fn paths() -> &'static Paths {
    static P: OnceLock<Paths> = OnceLock::new();
    P.get_or_init(|| {
        // Absolute, so `file://` URLs of override images can be built (relative paths can't).
        let absolute = |p: PathBuf| std::path::absolute(&p).unwrap_or(p);
        if let Some(home) = std::env::var_os("RUSTCORD_HOME") {
            let home = absolute(PathBuf::from(home));
            return Paths { config_dir: home.join("config"), cache_dir: home.join("cache") };
        }
        match directories::ProjectDirs::from("", "", APP_NAME) {
            Some(d) => Paths {
                config_dir: d.config_dir().to_path_buf(),
                cache_dir: d.cache_dir().to_path_buf(),
            },
            None => {
                let base = absolute(std::env::current_dir().unwrap_or_default().join("rustcord-data"));
                Paths { config_dir: base.join("config"), cache_dir: base.join("cache") }
            }
        }
    })
}

/// Offline mode: never read the token, never connect; shows the cached UI only.
/// Used for startup/RAM measurements and screenshots (`RUSTCORD_OFFLINE=1`).
pub fn offline_mode() -> bool {
    std::env::var_os("RUSTCORD_OFFLINE").is_some_and(|v| v != "0")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AnimateMode {
    Always,
    #[default]
    Hover,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Theme name (built-in name or file stem in the themes folder).
    pub theme: String,
    pub desktop_notifications: bool,
    /// Mark channels read on Discord's side (like the official client) while viewing them.
    pub send_read_acks: bool,
    pub show_member_list: bool,
    /// Animate GIF avatars/emoji of other users always or only on hover.
    pub animate_others: AnimateMode,
    /// Optional overrides of the identify/super-properties (see README).
    pub client_build_number: Option<u64>,
    pub system_locale: Option<String>,
    /// Voice & Video (this device): cpal device names (None = system default), the voice
    /// activity threshold (dBFS), the input volume (percent, 0-200, 100 = unchanged), the push-to-talk key (Windows VK, 0 = none) and whether
    /// push-to-talk is on, the camera (`CameraInfo::id`, None = the first one) for `StartCamera`.
    pub voice_input: Option<String>,
    pub voice_output: Option<String>,
    pub voice_threshold_db: f32,
    pub voice_input_volume: u16,
    pub push_to_talk: bool,
    pub push_to_talk_key: u16,
    pub camera: Option<String>,
    /// Emoji picker "Frequently used" (local only): unicode emoji or `<:name:id>`, use count;
    /// most used first, at most 27.
    pub frequent_emoji: Vec<(String, u32)>,
    /// GIF picker favourites (local only), newest first, at most `MAX_FAVORITE_GIFS`.
    pub favorite_gifs: Vec<crate::model::GifItem>,
    /// UI text in Discord's font (gg sans, downloaded from discord.com into the cache dir;
    /// off or unavailable: Segoe UI). `theme::set_discord_font`.
    pub use_discord_font: bool,
    // ----- Settings pages, this device only (docs/SETTINGS_NOTES.md §3; ui/user_settings/app.rs)
    /// Appearance > Zoom level (egui zoom factor 0.5–2; Ctrl +/−/0 change it too).
    pub zoom: f32,
    /// Appearance > Chat font scaling: the message list's text size; None = the theme's.
    pub chat_font_size: Option<f32>,
    /// Appearance > Space between message groups (px, cozy display).
    pub message_group_spacing: f32,
    /// Accessibility > Saturation of the theme's colours (0–1).
    pub saturation: f32,
    /// Accessibility > Show role colours in names.
    pub role_colors: bool,
    /// Accessibility > Reduced motion: no UI animations; animated avatars and emoji only on hover.
    pub reduced_motion: bool,
    pub streamer: StreamerMode,
    /// Keybinds the user changed: action name -> shortcut ("Ctrl+Shift+M"; "" = none).
    pub keybinds: std::collections::BTreeMap<String, String>,
    // Open at sign-in / start minimized are not kept here: the Windows / Linux / macOS settings
    // page reads the OS entry itself (`autostart.rs`). Older files' `open_on_startup`,
    // `start_minimized` and `close_minimizes` (× minimized to the taskbar, off by default) are ignored.
    /// The one-time autostart default (`autostart::default_once`) ran, or the user set autostart
    /// on the Windows / Linux / macOS settings page. Older files lack it: false.
    pub autostart_default_done: bool,
    /// Windows Settings: closing the window (×, Alt+F4) hides it to the tray; Quit is in the
    /// tray menu.
    pub minimize_to_tray: bool,
    /// The "still running in the tray" balloon was shown (once).
    pub tray_hint_shown: bool,
    /// Notifications > Taskbar flashing on a notification while the window is in the background.
    pub flash_taskbar: bool,
    /// Windows/Linux/macOS settings > Updates: install updates automatically (docs/UPDATER_DESIGN.md).
    pub auto_update: bool,
    /// Windows, Settings > Game overlay (ui/overlay).
    pub overlay: OverlaySettings,
    /// Notifications > Sounds (`src/sounds.rs`).
    pub sounds: SoundSettings,
}

/// Notifications > Sounds (`src/sounds.rs`): Rustcord's own sounds.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundSettings {
    /// "Play sounds".
    pub enabled: bool,
    /// Percent 0-100 (gain = (v/100)^2).
    pub volume: u8,
    /// `sounds::Sound::key`s switched off. Strings, so a removed or renamed sound never breaks
    /// loading settings.toml; unknown keys are ignored.
    pub off: Vec<String>,
}

impl Default for SoundSettings {
    fn default() -> Self {
        Self { enabled: true, volume: 70, off: Vec::new() }
    }
}

/// Settings > Streamer Mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StreamerMode {
    pub enabled: bool,
    /// Hide personal information: my username, connections, notification sender and text.
    pub hide_personal_info: bool,
    /// Invite links (Invite People, server Invites) show masked; Copy still works.
    pub hide_invite_links: bool,
    pub disable_notifications: bool,
    /// No app sounds (`sounds::volume`).
    pub disable_sounds: bool,
    /// The window is left out of screen capture (Windows `WDA_EXCLUDEFROMCAPTURE`).
    pub hide_from_capture: bool,
}

impl Default for StreamerMode {
    fn default() -> Self {
        Self { enabled: false, hide_personal_info: true, hide_invite_links: true, disable_notifications: true, disable_sounds: true, hide_from_capture: false }
    }
}

impl StreamerMode {
    /// Streamer Mode is on and hides personal information.
    pub fn hides_personal(&self) -> bool {
        self.enabled && self.hide_personal_info
    }

    /// Streamer Mode is on and hides invite links.
    pub fn hides_invites(&self) -> bool {
        self.enabled && self.hide_invite_links
    }
}

/// Settings > Game overlay (Windows, ui/overlay): Rustcord's own windows over the game, never inside it.
/// Data only, so the settings file loads the same everywhere; labels live in the Windows-only page.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlaySettings {
    pub enabled: bool,
    pub voice_corner: Corner,
    pub notification_corner: Corner,
    pub show_names: Show,
    /// `Never` is not offered; it is treated as `Always`.
    pub show_users: Show,
    pub large_avatars: bool,
}

impl Default for OverlaySettings {
    fn default() -> Self {
        Self { enabled: true, voice_corner: Corner::TopLeft, notification_corner: Corner::BottomRight, show_names: Show::Always, show_users: Show::Always, large_avatars: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Show {
    #[default]
    Always,
    Speaking,
    Never,
}

/// `Settings::role_colors` for renderers that get no `Settings` (`Settings::publish`).
static ROLE_COLORS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn role_colors() -> bool {
    ROLE_COLORS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Bound of `Settings::favorite_gifs`.
pub const MAX_FAVORITE_GIFS: usize = 100;

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "Ember Dark".into(),
            desktop_notifications: true,
            send_read_acks: true,
            show_member_list: true,
            animate_others: AnimateMode::Hover,
            client_build_number: None,
            system_locale: None,
            voice_input: None,
            voice_output: None,
            voice_threshold_db: -50.0,
            voice_input_volume: 100,
            push_to_talk: false,
            push_to_talk_key: 0,
            camera: None,
            frequent_emoji: Vec::new(),
            favorite_gifs: Vec::new(),
            use_discord_font: true,
            zoom: 1.0,
            chat_font_size: None,
            message_group_spacing: 16.0,
            saturation: 1.0,
            role_colors: true,
            reduced_motion: false,
            streamer: StreamerMode::default(),
            keybinds: Default::default(),
            autostart_default_done: false,
            minimize_to_tray: true,
            tray_hint_shown: false,
            flash_taskbar: true,
            auto_update: true,
            overlay: OverlaySettings::default(),
            sounds: SoundSettings::default(),
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let s: Self = read_toml(&paths().settings_file()).unwrap_or_default();
        s.publish();
        s
    }

    /// Hands the options read deep in renderers to their globals (role colours, saturation).
    /// Call after loading and after each change.
    pub fn publish(&self) {
        ROLE_COLORS.store(self.role_colors, std::sync::atomic::Ordering::Relaxed);
        crate::theme::set_saturation(self.saturation);
    }

    pub fn save(&self) -> Result<(), String> {
        write_toml(&paths().settings_file(), self)
    }

    /// The desktop notification to show for (`title`, `body`): None when notifications are off
    /// (or Streamer Mode disables them); without sender and text while Streamer Mode hides
    /// personal information.
    pub fn notification<'a>(&self, title: &'a str, body: &'a str) -> Option<(&'a str, &'a str)> {
        let s = &self.streamer;
        if !self.desktop_notifications || (s.enabled && s.disable_notifications) {
            return None;
        }
        Some(if s.hides_personal() { (APP_NAME, "New message") } else { (title, body) })
    }
}

pub fn read_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    match toml::from_str(&text) {
        Ok(v) => Some(v),
        Err(e) => {
            eprintln!("rustcord: ignoring invalid {}: {e}", path.display());
            None
        }
    }
}

pub fn write_toml<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let text = toml::to_string_pretty(value).map_err(|e| e.to_string())?;
    write_atomic(path, text.as_bytes())
}

/// Write via temp file + rename so a crash never leaves a half-written file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// `write_atomic` for a file only its owner may read (mode 0600 on Unix; the per-user profile
/// folder's ACL does that on Windows).
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("tmp");
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| e.to_string())?;
        f.write_all(bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }
    #[cfg(not(unix))]
    write_atomic(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_through_toml() {
        let s = Settings {
            voice_input: Some("Mic (USB)".into()),
            push_to_talk: true,
            push_to_talk_key: 0x05,
            voice_input_volume: 150,
            favorite_gifs: vec![crate::model::GifItem { url: "https://x/1".into(), ..Default::default() }],
            ..Default::default()
        };
        let back: Settings = toml::from_str(&toml::to_string_pretty(&s).unwrap()).unwrap();
        assert_eq!((back.voice_input.as_deref(), back.push_to_talk, back.push_to_talk_key), (Some("Mic (USB)"), true, 5));
        assert_eq!((back.voice_output, back.voice_threshold_db, back.favorite_gifs.len()), (None, -50.0, 1));
        assert_eq!(back.voice_input_volume, 150);
        // Older files lack the settings pages' fields: their defaults apply.
        let old: Settings = toml::from_str("theme = \"Ember Dark\"\n[streamer]\nenabled = true\n").unwrap();
        assert_eq!((old.zoom, old.flash_taskbar, old.streamer.enabled, old.streamer.hide_personal_info), (1.0, true, true, true));
        assert_eq!(old.voice_input_volume, 100);
        assert!(old.streamer.hides_invites() && !old.streamer.hide_from_capture && !old.autostart_default_done);
        // Updates install automatically unless switched off; older files: on.
        assert!(old.auto_update && back.auto_update);
        let off: Settings = toml::from_str(&toml::to_string_pretty(&Settings { auto_update: false, ..Default::default() }).unwrap()).unwrap();
        assert!(!off.auto_update);
    }

    #[test]
    fn sound_settings_default_and_load() {
        let old: Settings = toml::from_str("theme = \"Ember Dark\"
").unwrap();
        assert_eq!((old.sounds.enabled, old.sounds.volume, old.sounds.off.len(), old.streamer.disable_sounds), (true, 70, 0, true));
        let s: Settings = toml::from_str("[sounds]
off = [\"mention\", \"gone\"]
volume = 30
").unwrap();
        assert_eq!((s.sounds.volume, s.sounds.off.as_slice()), (30, ["mention".to_string(), "gone".to_string()].as_slice()));
    }

    #[test]
    fn old_window_settings_migrate_to_the_tray_default() {
        // Every file saved before the tray wrote `close_minimizes = false` (its default), so it
        // says nothing about a choice: the new default (hide to the tray) applies either way.
        for old in ["close_minimizes = false\nopen_on_startup = true\nstart_minimized = false\n", "close_minimizes = true\n", ""] {
            let s: Settings = toml::from_str(old).unwrap();
            assert!(s.minimize_to_tray && !s.tray_hint_shown, "{old:?}");
            let saved = toml::to_string_pretty(&s).unwrap();
            assert!(saved.contains("minimize_to_tray = true") && !saved.contains("close_minimizes"));
        }
        let off: Settings = toml::from_str("minimize_to_tray = false\ntray_hint_shown = true\n").unwrap();
        assert!(!off.minimize_to_tray && off.tray_hint_shown);
    }

    #[test]
    fn overlay_settings_default_and_use_snake_case() {
        let s: Settings = toml::from_str("theme = \"Ember Dark\"\n").unwrap();
        let o = &s.overlay;
        assert!(o.enabled && o.large_avatars);
        assert_eq!((o.voice_corner, o.notification_corner, o.show_names, o.show_users), (Corner::TopLeft, Corner::BottomRight, Show::Always, Show::Always));
        let t: Settings = toml::from_str("[overlay]\nvoice_corner = \"top_right\"\nnotification_corner = \"off\"\nshow_names = \"speaking\"\n").unwrap();
        assert_eq!((t.overlay.voice_corner, t.overlay.notification_corner, t.overlay.show_names), (Corner::TopRight, Corner::Off, Show::Speaking));
        let saved = toml::to_string_pretty(&t).unwrap();
        assert!(saved.contains("voice_corner = \"top_right\"") && saved.contains("show_names = \"speaking\""));
    }

    #[test]
    fn streamer_mode_gates_notifications() {
        let mut s = Settings::default();
        assert_eq!(s.notification("Ann", "hi"), Some(("Ann", "hi")));
        s.streamer.enabled = true;
        assert_eq!(s.notification("Ann", "hi"), None);
        s.streamer.disable_notifications = false;
        assert_eq!(s.notification("Ann", "hi"), Some((APP_NAME, "New message")));
        s.streamer.hide_personal_info = false;
        assert_eq!(s.notification("Ann", "hi"), Some(("Ann", "hi")));
        s.desktop_notifications = false;
        assert_eq!(s.notification("Ann", "hi"), None);
    }
}
