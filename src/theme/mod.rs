//! Themes: TOML files in `<config>/themes/*.toml` plus the built-ins: three flat themes and the
//! gradient themes (gradient.rs; the Nitro colour themes, free and only on this device).
//! A theme drives egui `Visuals`/`Spacing`/text sizes/fonts and the client's own colors.
//!
//! OWNERSHIP: theme implementer. Struct fields + pub fn signatures are a shared contract
//! (ui/ reads `Theme::colors` etc. directly).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::{Path, PathBuf};

mod apply;
mod fonts;
mod gradient;
mod tokens;
pub(crate) use apply::{append_boxed, icon_family, is_high_contrast, medium_family, semibold_family};
pub use fonts::{init_fonts, set_discord_font};
pub use gradient::swatch_mesh;
pub use tokens::{Mode, Tokens, readable_on};

/// Settings > Accessibility > Saturation (0–1, f32 bits): applied to every theme colour as it
/// is drawn (`Rgba::to_color32`), so theme files and the editor keep the real colours.
static SATURATION: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3f80_0000); // 1.0

/// Set by `config::Settings::publish`; re-apply the theme afterwards (egui visuals).
pub fn set_saturation(s: f32) {
    let s = if s.is_finite() { s.clamp(0.0, 1.0) } else { 1.0 };
    SATURATION.store(s.to_bits(), std::sync::atomic::Ordering::Relaxed);
}

/// `c` moved towards its own grey by the saturation setting.
fn desaturate(c: [u8; 4]) -> [u8; 4] {
    let s = f32::from_bits(SATURATION.load(std::sync::atomic::Ordering::Relaxed));
    if s >= 1.0 {
        return c;
    }
    let [r, g, b, a] = c.map(f32::from);
    let grey = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let mix = |v: f32| (grey + (v - grey) * s).round().clamp(0.0, 255.0) as u8;
    [mix(r), mix(g), mix(b), a as u8]
}

/// sRGB color, `"#rrggbb"` or `"#rrggbbaa"` in TOML.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba(pub [u8; 4]);

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self([r, g, b, 255])
    }
    pub fn to_color32(self) -> egui::Color32 {
        let [r, g, b, a] = desaturate(self.0);
        egui::Color32::from_rgba_unmultiplied(r, g, b, a)
    }
    pub fn to_hex(self) -> String {
        let [r, g, b, a] = self.0;
        if a == 255 { format!("#{r:02x}{g:02x}{b:02x}") } else { format!("#{r:02x}{g:02x}{b:02x}{a:02x}") }
    }
    pub fn parse(s: &str) -> Option<Self> {
        let h = s.trim().strip_prefix('#')?;
        let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
        match h.len() {
            6 => Some(Self([byte(0)?, byte(2)?, byte(4)?, 255])),
            8 => Some(Self([byte(0)?, byte(2)?, byte(4)?, byte(6)?])),
            _ => None,
        }
    }
}

impl Serialize for Rgba {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Rgba {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Rgba::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("invalid color {s:?}, expected #rrggbb")))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeColors {
    /// Message area background.
    pub background: Rgba,
    /// Channel/DM list background.
    pub sidebar: Rgba,
    /// Server icon column background.
    pub guild_bar: Rgba,
    /// Inputs, embeds, cards, popups.
    pub surface: Rgba,
    pub text: Rgba,
    pub text_muted: Rgba,
    pub accent: Rgba,
    /// Text drawn on top of `accent`.
    pub accent_text: Rgba,
    pub message_hover: Rgba,
    /// Background of messages that mention me.
    pub mention_highlight: Rgba,
    pub link: Rgba,
    pub danger: Rgba,
    pub code_background: Rgba,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeFont {
    /// Path to a .ttf/.otf for UI text; empty = built-in font.
    pub ui_font: String,
    /// Path to a monospace .ttf/.otf for code; empty = built-in.
    pub mono_font: String,
    /// Base body text size in points.
    pub size: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeLayout {
    pub corner_radius: f32,
    /// Compact: no avatars, single-line "time name: text" rows, tighter spacing.
    pub compact: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub name: String,
    pub dark: bool,
    pub colors: ThemeColors,
    pub font: ThemeFont,
    pub layout: ThemeLayout,
    /// App gradient behind translucent panels (`[gradient]` in TOML); None = flat theme.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gradient: Option<ThemeGradient>,
}

/// A CSS-style linear gradient over the whole window (COSMETICS_NOTES §13.4A).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeGradient {
    /// Degrees, as CSS `linear-gradient`: 0 = towards the top, 90 = towards the right.
    pub angle: f32,
    /// 1–5 colour stops.
    pub stops: Vec<GradientStop>,
    /// Multiplier of the panels' overlay opacity (0.5–1; lower shows more gradient).
    pub overlay: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    pub color: Rgba,
    /// Position along the gradient line in % (0–100).
    pub at: f32,
}

impl Default for ThemeGradient {
    fn default() -> Self {
        Self { angle: 180.0, stops: Vec::new(), overlay: 1.0 }
    }
}

/// Fills of the main window's panels (`Theme::panel_fills`): the flat theme's colours, or
/// translucent overlays over the app gradient.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelFills {
    /// Title bar, server bar, behind the channel list card.
    pub frame: Rgba,
    /// Channel list card (drawn over `frame`), settings nav column.
    pub card: Rgba,
    /// Chat, Friends and settings pages, thread panel.
    pub chat: Rgba,
    /// Member list.
    pub side: Rgba,
}

impl Default for ThemeColors {
    fn default() -> Self {
        Theme::nightshade().colors
    }
}
impl Default for ThemeFont {
    fn default() -> Self {
        Self { ui_font: String::new(), mono_font: String::new(), size: 15.0 }
    }
}
impl Default for ThemeLayout {
    fn default() -> Self {
        Self { corner_radius: 8.0, compact: false }
    }
}
impl Default for Theme {
    fn default() -> Self {
        Self::nightshade()
    }
}

impl Theme {
    /// Built-in dark theme (default): near-black violet-tinted panes, one purple for what is
    /// yours and the main action (docs/DESIGN.md §1-2).
    pub fn nightshade() -> Self {
        Self {
            name: "Nightshade".into(),
            dark: true,
            colors: ThemeColors {
                background: Rgba::rgb(0x12, 0x11, 0x18),
                sidebar: Rgba::rgb(0x0e, 0x0d, 0x13),
                guild_bar: Rgba::rgb(0x08, 0x07, 0x0b),
                surface: Rgba::rgb(0x1c, 0x1a, 0x24),
                text: Rgba::rgb(0xf0, 0xee, 0xf6),
                text_muted: Rgba::rgb(0xa8, 0xa5, 0xb6),
                // The primary purple (hue 278), clear of Discord blurple and Twitch.
                accent: Rgba::rgb(0xa4, 0x2b, 0xea),
                accent_text: Rgba::rgb(0xff, 0xff, 0xff),
                message_hover: Rgba::rgb(0x17, 0x16, 0x1e),
                mention_highlight: Rgba::rgb(0x24, 0x14, 0x31),
                link: Rgba::rgb(0x6c, 0xb4, 0xff),
                danger: Rgba::rgb(0xfc, 0x6d, 0x72),
                code_background: Rgba::rgb(0x0a, 0x09, 0x0e),
            },
            font: ThemeFont::default(),
            layout: ThemeLayout::default(),
            gradient: None,
        }
    }

    fn lilac() -> Self {
        Self {
            name: "Lilac".into(),
            dark: false,
            colors: ThemeColors {
                background: Rgba::rgb(0xfb, 0xfa, 0xfd),
                sidebar: Rgba::rgb(0xf0, 0xee, 0xf5),
                guild_bar: Rgba::rgb(0xe7, 0xe5, 0xee),
                surface: Rgba::rgb(0xeb, 0xe8, 0xf2),
                text: Rgba::rgb(0x17, 0x15, 0x1d),
                text_muted: Rgba::rgb(0x5b, 0x58, 0x69),
                accent: Rgba::rgb(0x8e, 0x15, 0xd5),
                accent_text: Rgba::rgb(0xff, 0xff, 0xff),
                message_hover: Rgba::rgb(0xf4, 0xf2, 0xf8),
                mention_highlight: Rgba::rgb(0xf0, 0xe3, 0xf9),
                link: Rgba::rgb(0x0a, 0x63, 0xc6),
                danger: Rgba::rgb(0xc4, 0x28, 0x38),
                code_background: Rgba::rgb(0xf2, 0xf0, 0xf6),
            },
            font: ThemeFont::default(),
            layout: ThemeLayout::default(),
            gradient: None,
        }
    }

    fn high_contrast() -> Self {
        Self {
            name: "High Contrast".into(),
            dark: true,
            colors: ThemeColors {
                background: Rgba::rgb(0x0a, 0x0a, 0x0a),
                sidebar: Rgba::rgb(0x00, 0x00, 0x00),
                guild_bar: Rgba::rgb(0x00, 0x00, 0x00),
                surface: Rgba::rgb(0x16, 0x16, 0x16),
                text: Rgba::rgb(0xff, 0xff, 0xff),
                text_muted: Rgba::rgb(0xd6, 0xd6, 0xd6),
                accent: Rgba::rgb(0xdd, 0xa3, 0xff),
                accent_text: Rgba::rgb(0x00, 0x00, 0x00),
                message_hover: Rgba::rgb(0x22, 0x22, 0x22),
                mention_highlight: Rgba::rgb(0x34, 0x29, 0x3b),
                link: Rgba::rgb(0x66, 0xe0, 0xff),
                danger: Rgba::rgb(0xff, 0x5c, 0x5c),
                code_background: Rgba::rgb(0x1c, 0x1c, 0x1c),
            },
            font: ThemeFont { size: 16.0, ..ThemeFont::default() },
            layout: ThemeLayout { corner_radius: 4.0, compact: false },
            gradient: None,
        }
    }

    /// The built-ins: dark, light, high-contrast, then the gradient themes in picker order.
    pub fn builtins() -> Vec<Theme> {
        let mut all = vec![Self::nightshade(), Self::lilac(), Self::high_contrast()];
        all.extend(gradient::presets());
        all
    }

    /// Built-ins followed by valid user themes from the themes folder (invalid files skipped).
    pub fn load_all() -> Vec<Theme> {
        let mut all = Self::builtins();
        all.extend(load_user_themes(&themes_dir()).into_iter().map(|(_, t)| t));
        all
    }

    /// Theme by name from `load_all`, falling back to the default.
    pub fn find(name: &str) -> Theme {
        if let Some(t) = Self::builtins().into_iter().find(|t| t.name == name) {
            return t;
        }
        // Settings may also hold a file stem (see `config::Settings::theme`).
        let user = load_user_themes(&themes_dir()).into_iter().find(|(stem, t)| t.name == name || stem == name);
        if let Some((_, t)) = user {
            return t;
        }
        // The old built-in names last, so a user theme really called "Ember Dark" still wins.
        let name = renamed(name);
        Self::builtins().into_iter().find(|t| t.name == name).unwrap_or_default()
    }

    /// The derived colour tokens (tokens.rs, docs/DESIGN.md §2.2).
    pub fn tokens(&self) -> Tokens {
        Tokens::new(&self.colors)
    }

    pub fn is_builtin(&self) -> bool {
        Self::builtins().iter().any(|t| t.name == self.name)
    }

    /// Save into the themes folder as `<sanitized name>.toml`.
    pub fn save_user(&self) -> Result<(), String> {
        self.save_into(&themes_dir()).map(|_| ())
    }

    pub fn export_to(&self, path: &Path) -> Result<(), String> {
        crate::config::write_toml(path, self)
    }

    /// Parse + validate a theme file and copy it into the themes folder.
    pub fn import_from(path: &Path) -> Result<Theme, String> {
        import_into(path, &themes_dir())
    }

    fn save_into(&self, dir: &Path) -> Result<PathBuf, String> {
        let mut theme = self.clone();
        validate(&mut theme)?;
        if theme.is_builtin() {
            return Err(format!("\"{}\" is a built-in theme; save it under another name", theme.name));
        }
        // The file that already holds this name (hand-written files may use another stem).
        let stem = load_user_themes(dir)
            .into_iter()
            .find(|(_, t)| t.name == theme.name)
            .map_or_else(|| sanitize_file_name(&theme.name), |(s, _)| s);
        let path = dir.join(format!("{stem}.toml"));
        // Two names can sanitize to the same file; never overwrite a different or unreadable theme.
        match std::fs::read_to_string(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("can't read {}: {e}", path.display())),
            Ok(text) => match parse_theme(&text) {
                Ok(existing) if existing.name == theme.name => {}
                Ok(existing) => return Err(format!("another theme (\"{}\") already uses {}", existing.name, path.display())),
                Err(_) => return Err(format!("{} exists but is not a valid theme; fix or remove it first", path.display())),
            },
        }
        crate::config::write_toml(&path, &theme)?;
        Ok(path)
    }
}

/// Built-in names before the 0.2 redesign, for settings files that still name them.
pub fn renamed(name: &str) -> &str {
    match name {
        "Ember Dark" => "Nightshade",
        "Paper Light" => "Lilac",
        n => n,
    }
}

/// Theme files larger than this are not themes.
const MAX_THEME_FILE: u64 = 256 * 1024;

fn themes_dir() -> PathBuf {
    crate::config::paths().themes_dir()
}

/// Parses and validates theme TOML. A missing `name` is an error (serde would default it).
fn parse_theme(text: &str) -> Result<Theme, String> {
    #[derive(Deserialize)]
    struct Named {
        #[serde(rename = "name")]
        _name: String,
    }
    toml::from_str::<Named>(text).map_err(|e| format!("invalid theme: {e}"))?;
    let mut theme: Theme = toml::from_str(text).map_err(|e| format!("invalid theme: {e}"))?;
    validate(&mut theme)?;
    Ok(theme)
}

fn validate(theme: &mut Theme) -> Result<(), String> {
    theme.name = theme.name.trim().to_owned();
    if theme.name.is_empty() {
        return Err("theme name is empty".into());
    }
    if theme.name.chars().count() > 64 {
        return Err("theme name is longer than 64 characters".into());
    }
    if !(6.0..=48.0).contains(&theme.font.size) {
        return Err(format!("font size {} is outside 6..=48", theme.font.size));
    }
    if !(0.0..=32.0).contains(&theme.layout.corner_radius) {
        return Err(format!("corner radius {} is outside 0..=32", theme.layout.corner_radius));
    }
    if let Some(g) = &theme.gradient {
        if !(1..=5).contains(&g.stops.len()) {
            return Err(format!("a gradient has 1 to 5 stops, not {}", g.stops.len()));
        }
        if !g.angle.is_finite() || g.stops.iter().any(|s| !(0.0..=100.0).contains(&s.at)) {
            return Err("gradient angle must be a number and stop positions 0..=100".into());
        }
        if !(0.5..=1.0).contains(&g.overlay) {
            return Err(format!("gradient overlay {} is outside 0.5..=1", g.overlay));
        }
    }
    Ok(())
}

/// `(file stem, theme)` for every valid `*.toml` in `dir`, sorted by file name. Files whose
/// theme name is already taken (built-in or an earlier file) are skipped.
fn load_user_themes(dir: &Path) -> Vec<(String, Theme)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("toml")))
        .collect();
    paths.sort();

    let mut taken: Vec<String> = Theme::builtins().into_iter().map(|t| t.name).collect();
    let mut out = Vec::new();
    for path in paths {
        match read_theme_file(&path) {
            Ok(theme) if !taken.contains(&theme.name) => {
                taken.push(theme.name.clone());
                let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                out.push((stem, theme));
            }
            Ok(_) => {}
            Err(e) => eprintln!("rustcord: skipping theme {}: {e}", path.display()),
        }
    }
    out
}

fn read_theme_file(path: &Path) -> Result<Theme, String> {
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len > MAX_THEME_FILE {
        return Err("file is too large to be a theme".into());
    }
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    parse_theme(&text)
}

fn import_into(path: &Path, dir: &Path) -> Result<Theme, String> {
    let mut theme = read_theme_file(path).map_err(|e| format!("can't import {}: {e}", path.display()))?;
    // Never replace a different theme of the same name: pick a free "<name> (n)" instead.
    let taken: Vec<Theme> = load_user_themes(dir).into_iter().map(|(_, t)| t).collect();
    let base = if theme.is_builtin() { format!("{} (custom)", theme.name) } else { theme.name.clone() };
    theme.name = base.clone();
    let mut n = 2;
    while taken.iter().any(|t| t.name == theme.name && *t != theme) {
        theme.name = format!("{base} ({n})");
        n += 1;
    }
    theme.save_into(dir)?;
    Ok(theme)
}

/// File name for a theme: no path separators, reserved characters or reserved device names.
pub fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '(' | ')') { c } else { '_' })
        .take(64)
        .collect();
    let mut s = cleaned.trim().to_owned();
    if s.is_empty() {
        s = "theme".into();
    }
    let upper = s.to_ascii_uppercase();
    let numbered_device = (upper.starts_with("COM") || upper.starts_with("LPT"))
        && upper.len() == 4
        && upper.as_bytes()[3].is_ascii_digit();
    if ["CON", "PRN", "AUX", "NUL"].contains(&upper.as_str()) || numbered_device {
        s.insert(0, '_');
    }
    s
}

/// True when `path` is a font file themes can use (the same check as loading it).
pub fn is_usable_font(path: &Path) -> bool {
    apply::read_font(path).is_some()
}

/// The tokens of the theme `ThemeApplier::apply` installed last, for shared helpers that only
/// have a `Ui` (context menus).
pub fn applied_tokens(ctx: &egui::Context) -> Tokens {
    ctx.data(|d| d.get_temp(tokens_id())).unwrap_or_else(|| Theme::default().tokens())
}

fn tokens_id() -> egui::Id {
    egui::Id::unique("rc_theme_tokens")
}

/// Name of the font family used for bold text (`RichText::strong` only changes color in
/// egui 0.37). Always registered by `ThemeApplier::apply` (Windows: Segoe UI Bold;
/// falls back to the regular UI font if no bold face is available).
pub const BOLD_FAMILY: &str = "bold";

pub fn bold_family() -> egui::FontFamily {
    egui::FontFamily::Name(BOLD_FAMILY.into())
}

/// Applies a theme to egui. Keeps track of loaded fonts so font files are only re-read when
/// the font paths change (font loading is the expensive part).
#[derive(Default)]
pub struct ThemeApplier {
    /// `(ui_font, mono_font, gg sans generation)` of the font set currently installed in egui.
    fonts_key: Option<(String, String, u32)>,
    /// Gradient of the applied theme, and its window mesh for one window rect (gradient.rs).
    gradient: Option<ThemeGradient>,
    gradient_mesh: Option<(egui::Rect, std::sync::Arc<egui::Mesh>)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rustcord-theme-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn builtins_roundtrip_through_toml() {
        for theme in Theme::builtins() {
            let text = toml::to_string_pretty(&theme).unwrap();
            assert_eq!(parse_theme(&text).unwrap(), theme, "{text}");
        }
    }

    #[test]
    fn builtin_names_are_unique_and_default_is_nightshade() {
        let names: Vec<String> = Theme::builtins().into_iter().map(|t| t.name).collect();
        assert_eq!(names[..3], ["Nightshade", "Lilac", "High Contrast"]);
        assert_eq!(names.len(), 3 + 23);
        assert!(names.iter().enumerate().all(|(i, n)| !names[..i].contains(n)));
        assert_eq!(Theme::default().name, "Nightshade");
        let c = Theme::default().colors;
        assert!(apply::contrast_ratio(c.accent_text, c.accent) >= 4.5 && apply::contrast_ratio(c.accent, c.background) >= 3.0);
        assert!(Theme::builtins()[3..].iter().all(|t| t.gradient.is_some()));
        assert_eq!(Theme::find("Sunset").gradient.unwrap().stops.len(), 2);
    }

    #[test]
    fn old_theme_names_still_load() {
        assert_eq!(Theme::find("Ember Dark").name, "Nightshade");
        assert_eq!(Theme::find("Paper Light").name, "Lilac");
    }

    #[test]
    fn gradient_themes_roundtrip_and_validate() {
        let mut t = Theme { name: "Mine".into(), ..Theme::find("Chroma Glow") };
        let text = toml::to_string_pretty(&t).unwrap();
        assert!(text.contains("[gradient]") && text.contains("[[gradient.stops]]"), "{text}");
        assert_eq!(parse_theme(&text).unwrap(), t);
        // Flat themes write no gradient table.
        assert!(!toml::to_string_pretty(&Theme::nightshade()).unwrap().contains("gradient"));
        let g = t.gradient.as_mut().unwrap();
        g.stops.push(g.stops[0]); // 6 stops
        assert!(parse_theme(&toml::to_string_pretty(&t).unwrap()).is_err());
        let ok = "name = \"G\"
[gradient]
angle = 90
stops = [{ color = \"#ff0000\", at = 0 }, { color = \"#0000ff\", at = 100 }]";
        assert_eq!(parse_theme(ok).unwrap().gradient.unwrap().overlay, 1.0);
        assert!(parse_theme(&ok.replace("at = 100", "at = 140")).is_err());
        assert!(parse_theme(&format!("{ok}
overlay = 0.1")).is_err());
        assert!(parse_theme("name = \"G\"
[gradient]
angle = 90").is_err()); // no stops
    }

    #[test]
    fn invalid_color_is_rejected() {
        let good = toml::to_string_pretty(&Theme::nightshade()).unwrap();
        for bad in ["\"#12345\"", "\"red\"", "\"#gg0000\"", "\"#1234567\""] {
            let text = good.replacen("background = \"#121118\"", &format!("background = {bad}"), 1);
            assert_ne!(text, good);
            assert!(parse_theme(&text).is_err(), "{bad} accepted");
        }
        assert_eq!(Rgba::parse("#e8743b80"), Some(Rgba([0xe8, 0x74, 0x3b, 0x80])));
    }

    #[test]
    fn missing_or_empty_name_is_rejected() {
        assert!(parse_theme("dark = true").is_err());
        assert!(parse_theme("name = \"   \"").is_err());
        assert!(parse_theme("name = \"x\"\n[font]\nsize = 1000.0").is_err());
        assert_eq!(parse_theme("name = \" Mine \"").unwrap().name, "Mine");
    }

    #[test]
    fn import_renames_builtin_collision_and_copies_into_dir() {
        let dir = temp_dir("import");
        let src = dir.join("incoming.toml");
        let mut theme = Theme::nightshade();
        theme.colors.accent = Rgba::rgb(1, 2, 3);
        theme.export_to(&src).unwrap();

        let themes = dir.join("themes");
        let imported = import_into(&src, &themes).unwrap();
        assert_eq!(imported.name, "Nightshade (custom)");
        assert_eq!(imported.colors.accent, Rgba::rgb(1, 2, 3));

        let loaded = load_user_themes(&themes);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].0, "Nightshade (custom)");
        assert_eq!(loaded[0].1, imported);

        // Invalid files are rejected and never copied.
        let bad = dir.join("bad.toml");
        std::fs::write(&bad, "name = \"\"").unwrap();
        assert!(import_into(&bad, &themes).is_err());
        assert_eq!(load_user_themes(&themes).len(), 1);

        // A different theme with a taken name gets a free name; an identical one is not duplicated.
        let mut x = Theme { name: "X".into(), ..Theme::nightshade() };
        x.export_to(&src).unwrap();
        assert_eq!(import_into(&src, &themes).unwrap().name, "X");
        assert_eq!(import_into(&src, &themes).unwrap().name, "X");
        x.colors.text = Rgba::rgb(9, 9, 9);
        x.export_to(&src).unwrap();
        assert_eq!(import_into(&src, &themes).unwrap().name, "X (2)");
        theme.colors.accent = Rgba::rgb(4, 5, 6);
        theme.export_to(&src).unwrap();
        assert_eq!(import_into(&src, &themes).unwrap().name, "Nightshade (custom) (2)");
        let names: Vec<String> = load_user_themes(&themes).into_iter().map(|(_, t)| t.name).collect();
        assert_eq!(names, ["Nightshade (custom) (2)", "Nightshade (custom)", "X (2)", "X"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_refuses_builtin_names_and_foreign_files() {
        let dir = temp_dir("save");
        assert!(Theme::nightshade().save_into(&dir).is_err());

        let mut a = Theme::nightshade();
        a.name = "a/b".into();
        a.save_into(&dir).unwrap();
        let mut b = a.clone();
        b.name = "a:b".into(); // same sanitized file name as "a/b"
        assert!(b.save_into(&dir).is_err());
        a.colors.text = Rgba::rgb(9, 9, 9);
        a.save_into(&dir).unwrap(); // same name overwrites
        assert_eq!(load_user_themes(&dir)[0].1.colors.text, Rgba::rgb(9, 9, 9));

        // A file that doesn't parse is never overwritten.
        std::fs::write(dir.join("Typo.toml"), "name = \"Typo\"\ndark = maybe").unwrap();
        assert!(Theme { name: "Typo".into(), ..Theme::nightshade() }.save_into(&dir).is_err());
        // A hand-written theme is saved into its own file, whatever its stem.
        let m = Theme { name: "Midnight".into(), ..Theme::nightshade() };
        crate::config::write_toml(&dir.join("Dark.toml"), &m).unwrap();
        assert_eq!(m.save_into(&dir).unwrap(), dir.join("Dark.toml"));
        assert!(!dir.join("Midnight.toml").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sanitize_file_names() {
        assert_eq!(sanitize_file_name("Nightshade (custom)"), "Nightshade (custom)");
        assert_eq!(sanitize_file_name("../..\\evil:name*?"), "______evil_name__");
        assert_eq!(sanitize_file_name("  ...  "), "___");
        assert_eq!(sanitize_file_name("   "), "theme");
        assert_eq!(sanitize_file_name("con"), "_con");
        assert_eq!(sanitize_file_name("LPT1"), "_LPT1");
        assert_eq!(sanitize_file_name("Console"), "Console");
        assert_eq!(sanitize_file_name(&"x".repeat(100)).len(), 64);
        assert_eq!(sanitize_file_name("Тема ночь"), "Тема ночь");
    }
}
