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

/// People's colours (ui/aura.rs): how strongly each person's own colour shows on their
/// profile, rows and cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PeopleColors {
    #[default]
    Bold,
    Subtle,
    Off,
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
    /// Appearance > Layout: the user panel shows what the computer plays (crate::now_playing). Windows and Linux.
    pub music_controls: bool,
    /// Animate GIF avatars/emoji of other users always or only on hover.
    pub animate_others: AnimateMode,
    /// Settings > Appearance > People's colours.
    pub people_colors: PeopleColors,
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
    /// Account popout > status flyout: custom statuses saved on this device (newest first), at most
    /// `MAX_STATUS_PRESETS`. Never synced to Discord.
    pub status_presets: Vec<StatusPreset>,
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
    pub rustcord_presence: RustcordPresence,
    /// Activity privacy > Let games show their activity: Rustcord answers on the local game pipe (`crate::rpc`).
    pub game_connection: bool,
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
    /// Windows / Linux / macOS settings > App lock (crate::lock). The PIN's hash lives in the OS
    /// keyring, never here.
    pub app_lock: AppLock,
    /// Notifications > Keyword alerts (crate::alerts).
    pub keyword_alerts: crate::alerts::KeywordAlerts,
    /// Content & social > Hidden words (crate::alerts): matching messages fold into one line.
    pub hidden_words: Vec<crate::alerts::Keyword>,
    /// Notifications > Quiet hours and the pause from the tray / account menu (crate::alerts).
    pub quiet_hours: crate::alerts::QuietHours,
    /// Inbox > Saved (crate::saved): ids only, the text loads when shown.
    pub saved: Vec<crate::saved::Saved>,
    /// Edit name (crate::names): names, tags and colours only this device shows, for all accounts.
    pub custom_names: Vec<crate::names::CustomName>,
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
    /// Custom sounds everywhere: `sounds::Sound::key` -> absolute file path. Only the keys of
    /// `Sound::CUSTOM` are read. The user's own file, never copied or uploaded.
    pub files: std::collections::BTreeMap<String, String>,
    /// Custom sounds for one server, channel or DM (its id): key -> path. Beats `files`.
    pub places: std::collections::BTreeMap<crate::model::Id, std::collections::BTreeMap<String, String>>,
}

/// Windows / Linux / macOS settings > App lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppLock {
    /// A PIN is set: Rustcord asks for it at start.
    pub enabled: bool,
    /// Windows Hello also unlocks.
    pub hello: bool,
    /// "Lock after" in minutes; 0 = never.
    pub idle_minutes: u16,
    /// Lock when the window hides in the tray / menu bar.
    pub on_tray: bool,
}

impl Default for AppLock {
    fn default() -> Self {
        Self { enabled: false, hello: false, idle_minutes: 15, on_tray: true }
    }
}

/// Bound of `SoundSettings::places`.
pub const MAX_SOUND_PLACES: usize = 50;

impl Default for SoundSettings {
    fn default() -> Self {
        Self { enabled: true, volume: 70, off: Vec::new(), files: Default::default(), places: Default::default() }
    }
}

/// Settings > Activity privacy > Rustcord presence (this device): "Playing Rustcord" next to my game.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RustcordPresence {
    pub enabled: bool,
    /// Optional: a Discord application whose icon becomes the picture.
    pub application_id: Option<crate::model::Id>,
    /// The one-time "Rustcord now shows as your activity" toast was shown (or not needed).
    pub notice_seen: bool,
}

impl Default for RustcordPresence {
    fn default() -> Self {
        Self { enabled: true, application_id: None, notice_seen: false }
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

/// Bound of `Settings::status_presets`.
pub const MAX_STATUS_PRESETS: usize = 8;

/// A saved custom status. `clear` is a `user_settings::status::Expiry` key: "today", "4h",
/// "1h", "30m" or "never" (unknown keys read as "today"), so a renamed choice never breaks loading.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StatusPreset {
    pub text: Option<String>,
    pub emoji: Option<crate::model::Emoji>,
    pub clear: String,
}

/// Bound of `Settings::favorite_gifs`.
pub const MAX_FAVORITE_GIFS: usize = 100;

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "Nightshade".into(),
            desktop_notifications: true,
            send_read_acks: true,
            show_member_list: true,
            music_controls: true,
            animate_others: AnimateMode::Hover,
            people_colors: PeopleColors::default(),
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
            status_presets: Vec::new(),
            favorite_gifs: Vec::new(),
            use_discord_font: true,
            zoom: 1.0,
            chat_font_size: None,
            message_group_spacing: 16.0,
            saturation: 1.0,
            role_colors: true,
            reduced_motion: false,
            streamer: StreamerMode::default(),
            rustcord_presence: RustcordPresence::default(),
            game_connection: true,
            keybinds: Default::default(),
            autostart_default_done: false,
            minimize_to_tray: true,
            tray_hint_shown: false,
            flash_taskbar: true,
            auto_update: true,
            overlay: OverlaySettings::default(),
            sounds: SoundSettings::default(),
            app_lock: AppLock::default(),
            keyword_alerts: Default::default(),
            hidden_words: Vec::new(),
            quiet_hours: Default::default(),
            saved: Vec::new(),
            custom_names: Vec::new(),
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let file = paths().settings_file();
        let existed = file.exists();
        let mut s: Self = read_toml(&file).unwrap_or_default();
        // A fresh install needs no "this changed" notice; a 0.1.4 file (no such key) gets it once.
        s.rustcord_presence.notice_seen |= !existed;
        s.keyword_alerts.normalize();
        crate::alerts::normalize_words(&mut s.hidden_words);
        s.publish();
        s
    }

    /// Hands the options read deep in renderers to their globals (role colours, saturation).
    /// Call after loading and after each change.
    pub fn publish(&self) {
        ROLE_COLORS.store(self.role_colors, std::sync::atomic::Ordering::Relaxed);
        crate::theme::set_saturation(self.saturation);
        crate::names::publish(&self.custom_names);
    }

    pub fn save(&self) -> Result<(), String> {
        write_toml(&paths().settings_file(), self)
    }

    /// The desktop notification to show for (`title`, `body`): None when notifications are off
    /// (or Streamer Mode disables them); without sender and text while Streamer Mode hides
    /// personal information.
    pub fn notification<'a>(&self, title: &'a str, body: &'a str) -> Option<(&'a str, &'a str)> {
        self.notification_when(crate::lock::locked(), title, body)
    }

    /// `notification`, also without sender and text while the app is locked (`locked`).
    fn notification_when<'a>(&self, locked: bool, title: &'a str, body: &'a str) -> Option<(&'a str, &'a str)> {
        let s = &self.streamer;
        if !self.desktop_notifications || (s.enabled && s.disable_notifications) {
            return None;
        }
        Some(if s.hides_personal() || locked { (APP_NAME, "New message") } else { (title, body) })
    }

    /// No sender (avatar, name) on a notification: Streamer Mode's privacy form, or the app lock.
    pub fn hides_sender(&self) -> bool {
        self.streamer.hides_personal() || crate::lock::locked()
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
        // Status presets: none in an old file; a unicode emoji survives the round trip.
        let preset = StatusPreset { text: Some("Rewriting it in Rust".into()), emoji: Some(crate::model::Emoji { name: Some("🦀".into()), ..Default::default() }), clear: "1h".into() };
        let with: Settings = toml::from_str(&toml::to_string_pretty(&Settings { status_presets: vec![preset.clone()], ..Default::default() }).unwrap()).unwrap();
        assert_eq!(with.status_presets, vec![preset]);
        // Older files lack the settings pages' fields: their defaults apply.
        let old: Settings = toml::from_str("theme = \"Ember Dark\"\n[streamer]\nenabled = true\n").unwrap();
        assert_eq!((old.zoom, old.flash_taskbar, old.streamer.enabled, old.streamer.hide_personal_info), (1.0, true, true, true));
        assert_eq!(old.voice_input_volume, 100);
        assert!(old.status_presets.is_empty() && old.music_controls, "music controls: on for old files");
        assert_eq!(crate::theme::Theme::find(&old.theme).name, "Nightshade");
        assert!(old.streamer.hides_invites() && !old.streamer.hide_from_capture && !old.autostart_default_done);
        // Updates install automatically unless switched off; older files: on.
        assert!(old.auto_update && back.auto_update);
        let off: Settings = toml::from_str(&toml::to_string_pretty(&Settings { auto_update: false, ..Default::default() }).unwrap()).unwrap();
        assert!(!off.auto_update);
        // Quiet hours: off in older files; a round trip keeps the schedule and the pause.
        assert!(!old.quiet_hours.enabled && old.quiet_hours.paused_until.is_none());
        let q = crate::alerts::QuietHours { enabled: true, days: 0b0011111, start: 23 * 60 + 30, end: 6 * 60, paused_until: Some(1_800_000_000) };
        let back: Settings = toml::from_str(&toml::to_string_pretty(&Settings { quiet_hours: q.clone(), ..Default::default() }).unwrap()).unwrap();
        assert_eq!(back.quiet_hours, q);
        // Saved messages: none in an old file; a round trip keeps them, with and without a reminder.
        assert!(old.saved.is_empty());
        let saved = vec![
            crate::saved::Saved { owner: crate::model::Id(1), channel_id: crate::model::Id(2), message_id: crate::model::Id(3), remind_at: Some(1_800_000_000) },
            crate::saved::Saved { owner: crate::model::Id(1), channel_id: crate::model::Id(2), message_id: crate::model::Id(4), remind_at: None },
        ];
        let text = toml::to_string_pretty(&Settings { saved: saved.clone(), ..Default::default() }).unwrap();
        assert!(text.contains("[[saved]]") && text.contains("message_id = \"3\""), "{text}");
        assert_eq!(toml::from_str::<Settings>(&text).unwrap().saved, saved);
        // Custom names: none in an old file; a round trip keeps them (`[[custom_names]]` tables).
        assert!(old.custom_names.is_empty());
        let names = vec![crate::names::CustomName { id: crate::model::Id(7), name: Some("Ali".into()), tag: None, color: Some(0x4A90E2) }];
        let text = toml::to_string_pretty(&Settings { custom_names: names.clone(), ..Default::default() }).unwrap();
        assert!(text.contains("[[custom_names]]") && text.contains("id = \"7\""), "{text}");
        assert_eq!(toml::from_str::<Settings>(&text).unwrap().custom_names, names);
        // Keyword alerts: an old file has none and watches every server; a round trip keeps them.
        assert!(old.keyword_alerts.keywords.is_empty() && old.keyword_alerts.everywhere);
        let k = crate::alerts::KeywordAlerts {
            keywords: vec![crate::alerts::Keyword { text: "release".into(), whole_word: false }],
            everywhere: false,
            places: vec![crate::model::Id(42)],
        };
        let text = toml::to_string_pretty(&Settings { keyword_alerts: k.clone(), ..Default::default() }).unwrap();
        assert!(text.contains("places = [\"42\"]"), "{text}");
        assert_eq!(toml::from_str::<Settings>(&text).unwrap().keyword_alerts, k);
        // Hidden words: none in an old file; a round trip keeps them (`[[hidden_words]]` tables).
        assert!(old.hidden_words.is_empty());
        let hidden = vec![crate::alerts::Keyword { text: "async".into(), whole_word: false }];
        let text = toml::to_string_pretty(&Settings { hidden_words: hidden.clone(), ..Default::default() }).unwrap();
        assert!(text.contains("[[hidden_words]]"), "{text}");
        assert_eq!(toml::from_str::<Settings>(&text).unwrap().hidden_words, hidden);
        let mut messy = vec![crate::alerts::Keyword { text: "  Rust ".into(), whole_word: true }, crate::alerts::Keyword { text: "rust".into(), whole_word: false }, Default::default()];
        crate::alerts::normalize_words(&mut messy);
        assert_eq!(messy.len(), 1);
        assert_eq!(messy[0].text, "rust");
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
        // Custom sounds: none in an old file; a round trip keeps them (an `Id` as the table key).
        assert!(old.sounds.files.is_empty() && old.sounds.places.is_empty());
        let mut custom = Settings::default();
        custom.sounds.files.insert("message".into(), "C:/Users/Public/ping.wav".into());
        custom.sounds.places.insert(crate::model::Id(1565443437363200010), [("mention".to_owned(), "ding.ogg".to_owned())].into());
        let text = toml::to_string_pretty(&custom).unwrap();
        assert!(text.contains("[sounds.places.1565443437363200010]"), "{text}");
        let back: Settings = toml::from_str(&text).unwrap();
        assert_eq!((back.sounds.files, back.sounds.places), (custom.sounds.files, custom.sounds.places));
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
    fn rustcord_presence_defaults_on_and_round_trips() {
        let old: Settings = toml::from_str("theme = \"Ember Dark\"
").unwrap();
        assert!(old.rustcord_presence.enabled && old.rustcord_presence.application_id.is_none() && !old.rustcord_presence.notice_seen);
        let rustcord_presence = RustcordPresence { enabled: false, application_id: Some(crate::model::Id(123456789012345678)), notice_seen: true };
        let s = Settings { rustcord_presence, ..Default::default() };
        let back: Settings = toml::from_str(&toml::to_string_pretty(&s).unwrap()).unwrap();
        assert_eq!(back.rustcord_presence, s.rustcord_presence);
        // Files from before the game connection load with it on.
        assert!(old.game_connection);
        let off: Settings = toml::from_str("game_connection = false").unwrap();
        assert!(!off.game_connection);
    }

    #[test]
    fn a_locked_app_shows_notifications_without_text() {
        let mut s = Settings::default();
        assert_eq!(s.notification_when(true, "Ann", "hi"), Some((APP_NAME, "New message")));
        assert_eq!(s.notification_when(false, "Ann", "hi"), Some(("Ann", "hi")));
        s.desktop_notifications = false;
        assert_eq!(s.notification_when(true, "Ann", "hi"), None, "off stays off");
        // The lock is off in a fresh and in an old file.
        let old: Settings = toml::from_str("theme = \"Ember Dark\"
").unwrap();
        assert_eq!(old.app_lock, AppLock { enabled: false, hello: false, idle_minutes: 15, on_tray: true });
        let on: Settings = toml::from_str("[app_lock]
enabled = true
idle_minutes = 5
").unwrap();
        assert!(on.app_lock.enabled && on.app_lock.on_tray && on.app_lock.idle_minutes == 5);
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
