//! Turning a `Theme` into egui fonts, visuals, text styles and spacing.

use super::{BOLD_FAMILY, Mode, Rgba, Theme, ThemeApplier, ThemeColors, ThemeFont};
use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke, TextStyle, vec2};
use std::path::Path;
use std::sync::Arc;

impl ThemeApplier {
    pub fn apply(&mut self, ctx: &egui::Context, theme: &Theme) {
        // Locked across set_fonts: the gg sans download thread re-applies under the same lock.
        let mut gg = super::fonts::gg();
        gg.applied = Some(theme.font.clone());
        let key = (theme.font.ui_font.clone(), theme.font.mono_font.clone(), gg.generation);
        if self.fonts_key.as_ref() != Some(&key) {
            ctx.set_fonts(build_fonts(&theme.font, gg.faces()));
            self.fonts_key = Some(key);
        }
        drop(gg);
        if self.gradient != theme.gradient {
            self.gradient = theme.gradient.clone();
            self.gradient_mesh = None;
        }

        let which = if theme.dark { egui::Theme::Dark } else { egui::Theme::Light };
        ctx.set_theme(which);
        ctx.data_mut(|d| d.insert_temp(super::tokens_id(), theme.tokens()));
        let visuals = visuals(theme);
        let size = if theme.font.size.is_finite() { theme.font.size.clamp(6.0, 48.0) } else { 15.0 };
        let compact = theme.layout.compact;
        ctx.style_mut_of(which, |s| {
            s.visuals = visuals;
            // Type roles (docs/DESIGN.md §3): body B, ui B-1, meta B-3, title B+5 semibold.
            let prop = |d: f32| FontId::proportional((size + d).max(6.0));
            s.text_styles.insert(TextStyle::Body, prop(0.0));
            s.text_styles.insert(TextStyle::Button, prop(-1.0));
            s.text_styles.insert(TextStyle::Small, prop(-3.0));
            s.text_styles.insert(TextStyle::Monospace, FontId::monospace((size - 1.0).max(6.0)));
            s.text_styles.insert(TextStyle::Heading, FontId::new(size + 5.0, super::semibold_family()));
            s.interaction.tooltip_delay = 0.3;

            let sp = &mut s.spacing;
            sp.tooltip_width = 240.0;
            // Overlay bars: hidden until the pointer is over the area, a thin text-coloured
            // thumb, no track.
            sp.scroll = egui::style::ScrollStyle {
                bar_width: 8.0,
                floating_width: 4.0,
                handle_min_length: 24.0,
                bar_inner_margin: 2.0,
                foreground_color: true,
                active_handle_opacity: 0.3,
                interact_handle_opacity: 0.5,
                active_background_opacity: 0.0,
                interact_background_opacity: 0.0,
                ..egui::style::ScrollStyle::floating()
            };
            if compact {
                sp.item_spacing = vec2(6.0, 3.0);
                sp.button_padding = vec2(8.0, 2.0);
                sp.interact_size = vec2(32.0, 18.0);
                sp.window_margin = egui::Margin::same(8);
                sp.menu_margin = egui::Margin::same(4);
                sp.indent = 14.0;
            } else {
                sp.item_spacing = vec2(8.0, 6.0);
                // 12 x 6 around 20 px text: buttons and inputs come out 32 tall (§4).
                sp.button_padding = vec2(12.0, 6.0);
                sp.interact_size = vec2(40.0, 24.0);
                sp.window_margin = egui::Margin::same(12);
                // Menus and tooltips (egui shares their frame): items concentric at 1.5r - 6.
                sp.menu_margin = egui::Margin::same(6);
                sp.indent = 18.0;
            }
        });
    }
}

/// egui's visuals from the theme and its tokens (docs/DESIGN.md §2, §6, §9): secondary buttons
/// on `selected`, inputs as wells with an `edge` border, the focus colour on keyboard focus,
/// floats on `raised` with E2 / E3 shadows (none in high contrast, a strong edge instead).
fn visuals(theme: &Theme) -> egui::Visuals {
    let c = &theme.colors;
    let tk = super::Tokens::new(c);
    let hc = tk.mode == Mode::HighContrast;
    let light = tk.mode == Mode::Light;
    let mut v = if theme.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    let r = theme.layout.corner_radius.clamp(0.0, 80.0);
    let radius = |k: f32| CornerRadius::same((r * k).round() as u8);
    let text = c.text.to_color32();
    let well = c.code_background.to_color32();
    let line_w = if hc { 1.5 } else { 1.0 };
    let edge = Stroke::new(line_w, tk.edge);
    // Secondary buttons: `selected`, +4 % / +8 % text on hover / press. Translucent, so the
    // gradient themes' panels show through.
    let fill = |extra: f32| text.gamma_multiply(tk.mode.selected_alpha() + extra);

    v.override_text_color = None;
    v.weak_text_color = Some(c.text_muted.to_color32());
    v.hyperlink_color = c.link.to_color32();
    v.panel_fill = c.background.to_color32();
    v.window_fill = tk.raised;
    v.window_stroke = if hc { Stroke::new(1.5, c.text_muted.to_color32()) } else { Stroke::new(1.0, tk.rule) };
    // Wells: inputs, search, code, scroll tracks.
    v.extreme_bg_color = well;
    v.text_edit_bg_color = Some(well);
    v.faint_bg_color = mix(c.background, c.text, 0.04);
    v.code_bg_color = well;
    v.error_fg_color = c.danger.to_color32();
    v.warn_fg_color = tk.attention;
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);
    v.window_corner_radius = radius(2.5);
    v.menu_corner_radius = radius(1.5);
    // E3 (modals) and E2 (menus, tooltips, popouts).
    let shadow = |y: i8, blur: u8, dark: f32, lit: f32| {
        let a = if light { lit } else { dark };
        if hc { Shadow::NONE } else { Shadow { offset: [0, y], blur, spread: 0, color: Color32::from_black_alpha((a * 255.0).round() as u8) } }
    };
    v.window_shadow = shadow(24, 64, 0.65, 0.18);
    v.popup_shadow = shadow(12, 32, 0.55, 0.12);
    v.text_cursor.stroke = Stroke::new(2.0, tk.accent_ink);

    // Selected text in a purple tint; the stroke is also the label colour of selected buttons and
    // the border of a focused text field (recoloured to `focus` by ui::text_field_focus_ring).
    v.selection.bg_fill = tk.selection;
    v.selection.stroke = Stroke::new(2.0, text);

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = c.background.to_color32();
    w.noninteractive.weak_bg_fill = c.background.to_color32();
    w.noninteractive.bg_stroke = Stroke::new(line_w, if hc { c.text_muted.to_color32() } else { tk.rule }); // separators
    w.noninteractive.fg_stroke = Stroke::new(1.0, text); // label text

    // `bg_fill`: checkbox boxes, slider rails. `weak_bg_fill`: buttons. `bg_stroke`: input,
    // checkbox and combo box borders (and buttons, which share it in egui).
    w.inactive.bg_fill = well;
    w.inactive.weak_bg_fill = fill(0.0);
    w.inactive.bg_stroke = edge;
    w.inactive.fg_stroke = Stroke::new(1.0, text);

    w.hovered.bg_fill = well;
    w.hovered.weak_bg_fill = fill(0.04);
    w.hovered.bg_stroke = Stroke::new(line_w, c.text_muted.to_color32());
    w.hovered.fg_stroke = Stroke::new(1.5, text);

    // Pressed or focused (egui's one state for both): the focus ring, never accent.
    w.active.bg_fill = well;
    w.active.weak_bg_fill = fill(0.08);
    w.active.bg_stroke = Stroke::new(2.0, tk.focus);
    w.active.fg_stroke = Stroke::new(1.5, text);

    w.open.bg_fill = well;
    w.open.weak_bg_fill = fill(0.04);
    w.open.bg_stroke = Stroke::new(line_w, c.text_muted.to_color32());
    w.open.fg_stroke = Stroke::new(1.0, text);

    for wv in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        wv.corner_radius = radius(1.0);
        wv.expansion = 0.0;
    }
    v
}

/// Themes whose secondary text is nearly as strong as the main text want strong borders too.
pub(crate) fn is_high_contrast(c: &ThemeColors) -> bool {
    contrast_ratio(c.text_muted, c.background) >= 9.0
}

pub(super) fn contrast_ratio(a: Rgba, b: Rgba) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// WCAG relative luminance.
pub(super) fn luminance(c: Rgba) -> f32 {
    let lin = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c.0[0]) + 0.7152 * lin(c.0[1]) + 0.0722 * lin(c.0[2])
}

/// Opaque sRGB blend `a -> b` (alpha of `a`), saturation setting applied.
pub(super) fn mix(a: Rgba, b: Rgba, t: f32) -> Color32 {
    let ch = |i: usize| (f32::from(a.0[i]) + (f32::from(b.0[i]) - f32::from(a.0[i])) * t).round() as u8;
    Rgba([ch(0), ch(1), ch(2), a.0[3]]).to_color32()
}

/// egui's default fonts (keeping the emoji/icon fallbacks) with nicer primary faces:
/// the theme's own font files, else gg sans (`gg`: decoded faces by weight, fonts.rs) in front
/// of Segoe UI / Consolas on Windows, else egui's built-ins. Medium / semibold text has its own
/// families (`medium_family`, `semibold_family`), the regular / bold chains without gg sans.
pub(super) fn build_fonts(font: &ThemeFont, gg: &[(u16, FontData)]) -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    let custom = custom_font(&font.ui_font);
    // A theme's own UI font wins over gg sans.
    let gg = if custom.is_some() { &[][..] } else { gg };
    let (ui, semibold_face, bold) = match custom {
        // shortcut: a custom UI font has no bold face; bold text uses the regular face.
        Some(data) => (Some(data), None, None),
        None => (system_font("segoeui.ttf"), system_font("seguisb.ttf"), system_font("segoeuib.ttf")),
    };
    let mono = custom_font(&font.mono_font).or_else(|| system_font("consola.ttf"));
    let mut gg_face = |weight: u16| {
        let (_, data) = gg.iter().find(|(w, _)| *w == weight)?;
        let name = format!("gg-sans-{weight}");
        defs.font_data.insert(name.clone(), Arc::new(data.clone()));
        Some(name)
    };
    let gg_faces = [400, 500, 600, 700].map(&mut gg_face);

    let mut prop = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    if let Some(data) = ui {
        defs.font_data.insert("rc-ui".into(), Arc::new(data));
        prop.insert(0, "rc-ui".into());
    }
    // Segoe UI stays right behind it: gg sans has ~720 characters.
    prop.splice(0..0, gg_faces[0].clone());
    // Emoji code points drawn as UI icons stay monochrome and take the icon color.
    defs.families.insert(icon_family(), prop.clone());
    if let Some(data) = emoji_font() {
        // After the text faces (digits, ©, ↔ stay text), before egui's monochrome fallbacks.
        defs.font_data.insert(OS_EMOJI.into(), Arc::new(data));
        for chain in [&mut prop, defs.families.entry(FontFamily::Monospace).or_default()] {
            let at = chain.iter().position(|n| n == "egui-icons").unwrap_or(chain.len());
            chain.insert(at, OS_EMOJI.into());
        }
    }
    let mut bold_chain = prop.clone();
    if let Some(data) = bold {
        defs.font_data.insert("rc-ui-bold".into(), Arc::new(data));
        bold_chain.insert(0, "rc-ui-bold".into());
    }
    if let Some(data) = mono {
        defs.font_data.insert("rc-mono".into(), Arc::new(data));
        defs.families.entry(FontFamily::Monospace).or_default().insert(0, "rc-mono".into());
    }
    let [_, medium, semibold, gg_bold] = gg_faces;
    let with = |face: Option<String>, chain: &[String]| face.into_iter().chain(chain.iter().cloned()).collect::<Vec<_>>();
    // 600 without gg sans: Segoe UI Semibold, not Bold.
    let mut semibold_chain = bold_chain.clone();
    if let Some(data) = semibold_face {
        defs.font_data.insert("rc-ui-semibold".into(), Arc::new(data));
        semibold_chain.insert(0, "rc-ui-semibold".into());
    }
    defs.families.insert(medium_family(), with(medium, &prop));
    defs.families.insert(semibold_family(), with(semibold, &semibold_chain));
    defs.families.insert(FontFamily::Name(BOLD_FAMILY.into()), with(gg_bold, &bold_chain));
    // Box drawing without gg sans: its ━ ─ are short dashes; the provider finds Segoe UI Symbol's.
    defs.families.insert(box_family(), prop.iter().filter(|n| !n.starts_with("gg-sans-")).cloned().collect());
    defs.families.insert(FontFamily::Proportional, prop);
    defs
}

/// Medium (500) text: gg sans Medium, else the regular face.
pub fn medium_family() -> FontFamily {
    FontFamily::Name("rc-medium".into())
}

/// Box drawing (U+2500-257F) in decorated names (`append_boxed`): the text faces without gg sans,
/// whose box glyphs are shorter than their advance, so Segoe UI Symbol's corners and bars draw
/// them. (Horizontal rules like `━━━` are drawn in every family by `fonts::box_rule`.)
pub fn box_family() -> FontFamily {
    FontFamily::Name("rc-box".into())
}

fn is_box(c: char) -> bool {
    ('\u{2500}'..='\u{257f}').contains(&c)
}

/// Appends `text` to `job` in `format`, its box-drawing runs in `box_family` (same size), so
/// rules like `━━» TEAM «━━` stay continuous.
pub fn append_boxed(job: &mut egui::text::LayoutJob, text: &str, format: &egui::TextFormat) {
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        let boxed = is_box(c);
        let end = rest.find(|ch: char| is_box(ch) != boxed).unwrap_or(rest.len());
        let mut f = format.clone();
        if boxed {
            f.font_id = egui::FontId::new(format.font_id.size, box_family());
        }
        job.append(&rest[..end], 0.0, f);
        rest = &rest[end..];
    }
}

/// Semibold (600) text, Discord's usual weight for headings and labels: gg sans Semibold, else
/// Segoe UI Semibold, else the bold face.
pub fn semibold_family() -> FontFamily {
    FontFamily::Name("rc-semibold".into())
}

fn custom_font(path: &str) -> Option<FontData> {
    if path.trim().is_empty() {
        return None;
    }
    let font = read_font(Path::new(path.trim()));
    if font.is_none() {
        eprintln!("rustcord: theme font {path:?} is missing or not a TrueType/OpenType font");
    }
    font.map(FontData::from_owned)
}

/// Mapped rather than read: the pages stay shared with the OS file cache instead of costing
/// ~2.4 MB of private heap (Segoe UI, its bold face, Consolas). Once per process: every font
/// rebuild shares the blob, so `set_fonts` compares pointers, not whole files, and one view
/// per file stays mapped.
/// `file`: a Windows font file name (`%WINDIR%\Fonts`); on Linux / macOS the stand-in from
/// `unix_font_paths`, or an absolute path (the fallback chain's).
pub(super) fn system_font(file: &str) -> Option<FontData> {
    static MAPPED: std::sync::Mutex<Vec<(String, Option<FontData>)>> = std::sync::Mutex::new(Vec::new());
    let mut cache = MAPPED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_, font)) = cache.iter().find(|(f, _)| f == file) {
        return font.clone();
    }
    #[cfg(windows)]
    let paths = {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        vec![Path::new(&root).join("Fonts").join(file)]
    };
    #[cfg(not(windows))]
    let paths: Vec<std::path::PathBuf> = if file.starts_with('/') { vec![file.into()] } else { unix_font_paths(file).iter().map(Into::into).collect() };
    let font = paths.iter().find_map(|path| {
        let mapped = open_font(path).and_then(|f| map_file(&f)).filter(|m| looks_like_font(m.as_ref()));
        match mapped {
            Some(m) => Some(FontData::from_blob(Arc::new(m), 0)),
            None => read_font(path).map(FontData::from_owned),
        }
    });
    cache.push((file.to_owned(), font.clone()));
    font
}

/// Linux / macOS stand-ins for the Windows faces (first one found wins). gg sans still leads
/// every chain; these matter with it off or before its first download.
#[cfg(not(windows))]
fn unix_font_paths(file: &str) -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        return match file {
            "segoeui.ttf" => &["/System/Library/Fonts/Supplemental/Arial.ttf"],
            "segoeuib.ttf" => &["/System/Library/Fonts/Supplemental/Arial Bold.ttf"],
            "consola.ttf" => &["/System/Library/Fonts/Menlo.ttc"],
            _ => &[],
        };
    }
    match file {
        "segoeui.ttf" => &[
            "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/google-noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
        ],
        "seguisb.ttf" => &[
            "/usr/share/fonts/truetype/noto/NotoSans-SemiBold.ttf",
            "/usr/share/fonts/noto/NotoSans-SemiBold.ttf",
            "/usr/share/fonts/google-noto/NotoSans-SemiBold.ttf",
        ],
        "segoeuib.ttf" => &[
            "/usr/share/fonts/truetype/noto/NotoSans-Bold.ttf",
            "/usr/share/fonts/noto/NotoSans-Bold.ttf",
            "/usr/share/fonts/google-noto/NotoSans-Bold.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans-Bold.ttf",
        ],
        "consola.ttf" => &[
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
            "/usr/share/fonts/dejavu-sans-mono-fonts/DejaVuSansMono.ttf",
            "/usr/share/fonts/truetype/noto/NotoSansMono-Regular.ttf",
            "/usr/share/fonts/noto/NotoSansMono-Regular.ttf",
        ],
        _ => &[],
    }
}

/// Font data name of the OS color emoji font.
const OS_EMOJI: &str = "os-emoji";

/// Family for emoji glyphs used as UI icons (💬, 🔒, ⚙): the text faces plus egui's monochrome
/// emoji, tinted like text. Everything else draws unicode emoji with the OS color font.
pub(crate) fn icon_family() -> FontFamily {
    FontFamily::Name("rc-icons".into())
}

/// The OS color emoji font (epaint `color_fonts`): Segoe UI Emoji (COLR), mapped like the UI fonts.
/// epaint centres its shorter line box on Segoe UI's, which leaves the emoji ~0.17 em above the
/// text (and above the centre of their own galley, e.g. in reaction chips): lowered by that.
#[cfg(windows)]
fn emoji_font() -> Option<FontData> {
    system_font("seguiemj.ttf").map(|f| f.tweak(egui::FontTweak { y_offset_factor: 0.17, ..Default::default() }))
}

/// Noto Color Emoji (CBDT), mapped like the UI fonts (see `system_font`).
// shortcut: macOS' Apple Color Emoji.ttc (~190 MB, sbix) is over `open_font`'s cap, so macOS
// keeps the monochrome emoji; lift the cap for mapped files when that matters.
#[cfg(not(windows))]
fn emoji_font() -> Option<FontData> {
    let paths = [
        "/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf",
        "/usr/share/fonts/noto/NotoColorEmoji.ttf",
        "/usr/share/fonts/google-noto-color-emoji-fonts/NotoColorEmoji.ttf",
        "/usr/share/fonts/noto-emoji/NotoColorEmoji.ttf",
    ];
    paths.iter().find_map(|p| system_font(p))
}

/// Opens a font file: only plain local files of a sane size.
fn open_font(path: &Path) -> Option<std::fs::File> {
    // Paths come from (imported) theme files: only plain local files, before any fs call.
    // UNC paths would open network connections (leaking NTLM credentials), device paths hang.
    #[cfg(windows)]
    if !matches!(path.components().next(), Some(std::path::Component::Prefix(p))
        if matches!(p.kind(), std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)))
    {
        return None;
    }
    #[cfg(not(windows))]
    if !path.is_absolute() {
        return None;
    }
    // Fonts are a few MB at most; anything bigger is not worth keeping in RAM.
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > 64 * 1024 * 1024 {
        return None;
    }
    std::fs::File::open(path).ok()
}

pub(super) fn read_font(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    open_font(path)?.read_to_end(&mut bytes).ok()?;
    looks_like_font(&bytes).then_some(bytes)
}

/// Read-only view of a whole file (unmapped on drop). Writers can't truncate a mapped file
/// on Windows; on Unix the fonts are system files only root replaces (by rename, which keeps
/// the mapped inode).
struct MappedFile {
    ptr: *const u8,
    len: usize,
}

// SAFETY: the view is read-only and lives until drop.
unsafe impl Send for MappedFile {}
unsafe impl Sync for MappedFile {}

impl AsRef<[u8]> for MappedFile {
    fn as_ref(&self) -> &[u8] {
        // SAFETY: `ptr` is a live view of `len` readable bytes (see `map_file`).
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileMappingW(file: isize, attrs: *const u8, protect: u32, size_high: u32, size_low: u32, name: *const u16) -> isize;
    fn MapViewOfFile(mapping: isize, access: u32, offset_high: u32, offset_low: u32, bytes: usize) -> *const u8;
    fn UnmapViewOfFile(base: *const u8) -> i32;
    fn CloseHandle(handle: isize) -> i32;
}

#[cfg(windows)]
impl Drop for MappedFile {
    fn drop(&mut self) {
        // SAFETY: `ptr` came from MapViewOfFile and is unmapped once.
        unsafe { UnmapViewOfFile(self.ptr) };
    }
}

#[cfg(windows)]
fn map_file(file: &std::fs::File) -> Option<MappedFile> {
    use std::os::windows::io::AsRawHandle;
    const PAGE_READONLY: u32 = 0x02;
    const FILE_MAP_READ: u32 = 0x04;
    let len = usize::try_from(file.metadata().ok()?.len()).ok().filter(|&l| l > 0)?;
    // SAFETY: plain Win32 calls on a valid file handle; the view keeps the section alive after
    // its handle is closed, and covers the whole file (`len` bytes).
    unsafe {
        let mapping = CreateFileMappingW(file.as_raw_handle() as isize, std::ptr::null(), PAGE_READONLY, 0, 0, std::ptr::null());
        if mapping == 0 {
            return None;
        }
        let ptr = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
        CloseHandle(mapping);
        (!ptr.is_null()).then_some(MappedFile { ptr, len })
    }
}

#[cfg(unix)]
impl Drop for MappedFile {
    fn drop(&mut self) {
        // SAFETY: `ptr`/`len` came from mmap and are unmapped once.
        unsafe { libc::munmap(self.ptr as *mut libc::c_void, self.len) };
    }
}

#[cfg(unix)]
fn map_file(file: &std::fs::File) -> Option<MappedFile> {
    use std::os::fd::AsRawFd;
    let len = usize::try_from(file.metadata().ok()?.len()).ok().filter(|&l| l > 0)?;
    // SAFETY: a private read-only mapping of the whole file on a valid descriptor; it stays
    // valid after the descriptor is closed.
    let ptr = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_PRIVATE, file.as_raw_fd(), 0) };
    (ptr != libc::MAP_FAILED).then_some(MappedFile { ptr: ptr as *const u8, len })
}

/// Sanity check of the sfnt table directory. epaint panics on a font it cannot parse, so a
/// bad path in a theme file must never reach it.
pub(super) fn looks_like_font(b: &[u8]) -> bool {
    let u32_at = |i: usize| b.get(i..i + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]));
    let u16_at = |i: usize| b.get(i..i + 2).map(|s| u16::from_be_bytes([s[0], s[1]]));
    let mut start = 0usize;
    if b.starts_with(b"ttcf") {
        // Collection: epaint uses face 0.
        let Some(off) = u32_at(12) else { return false };
        start = off as usize;
    }
    let Some(tag) = u32_at(start) else { return false };
    if !matches!(tag, 0x0001_0000 | 0x4F54_544F /* OTTO */ | 0x7472_7565 /* true */) {
        return false;
    }
    let Some(num_tables) = u16_at(start + 4) else { return false };
    let mut has_cmap = false;
    for i in 0..usize::from(num_tables) {
        let rec = start + 12 + i * 16;
        let (Some(tag), Some(off), Some(len)) = (b.get(rec..rec + 4), u32_at(rec + 8), u32_at(rec + 12)) else {
            return false;
        };
        if (off as usize).checked_add(len as usize).is_none_or(|end| end > b.len()) {
            return false;
        }
        has_cmap |= tag == b"cmap";
    }
    has_cmap
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_drawing_runs_get_their_own_family() {
        let mut job = egui::text::LayoutJob::default();
        append_boxed(&mut job, "\u{2501}\u{2501}\u{bb} TEAM \u{ab}\u{2501}", &egui::TextFormat::simple(egui::FontId::proportional(14.0), egui::Color32::WHITE));
        let runs: Vec<(&str, bool)> = job.sections.iter().map(|s| (&job.text[s.byte_range.start.0..s.byte_range.end.0], s.format.font_id.family == box_family())).collect();
        assert_eq!(runs, [("\u{2501}\u{2501}", true), ("\u{bb} TEAM \u{ab}", false), ("\u{2501}", true)]);
        assert!(job.sections.iter().all(|s| s.format.font_id.size == 14.0));
    }

    #[test]
    fn only_high_contrast_builtin_gets_strong_borders() {
        let strong: Vec<bool> = Theme::builtins().iter().map(|t| is_high_contrast(&t.colors)).collect();
        assert_eq!(strong[..3], [false, false, true]);
        assert!(!strong[3..].contains(&true));
    }

    #[test]
    fn font_sanity_check() {
        // Minimal sfnt: header + one "cmap" record pointing at 4 bytes inside the file.
        let mut f = vec![0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        f.extend_from_slice(b"cmap");
        f.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 28, 0, 0, 0, 4]);
        f.extend_from_slice(&[0, 0, 0, 0]);
        assert!(looks_like_font(&f));

        let mut out_of_bounds = f.clone();
        out_of_bounds[27] = 200; // table length past the end of the file
        assert!(!looks_like_font(&out_of_bounds));
        assert!(!looks_like_font(b"PK\x03\x04 definitely a zip"));
        assert!(!looks_like_font(&f[..20]));
        assert!(!looks_like_font(b""));
    }

    #[test]
    fn font_paths_must_be_plain_local_files() {
        for p in [r"\\127.0.0.1\x\f.ttf", r"\\?\UNC\127.0.0.1\x\f.ttf", r"\\.\pipe\x", "relative.ttf", "/dev/zero"] {
            assert!(read_font(Path::new(p)).is_none(), "{p}");
        }
        assert!(read_font(&std::env::temp_dir()).is_none()); // a folder, not a file
    }

    #[cfg(windows)]
    #[test]
    fn windows_system_fonts_pass_the_check() {
        // Segoe UI ships with every supported Windows version.
        let mapped = system_font("segoeui.ttf").expect("Segoe UI");
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        assert_eq!(mapped.bytes(), read_font(&Path::new(&root).join("Fonts").join("segoeui.ttf")).unwrap());
        // Mapped once: a rebuild shares the blob (`set_fonts` then compares pointers).
        assert!(Arc::ptr_eq(&mapped.font, &system_font("segoeui.ttf").unwrap().font));
    }

    #[cfg(unix)]
    #[test]
    fn mmap_matches_read() {
        // Whatever this machine has (none: nothing to compare).
        let Some(path) = ["segoeui.ttf", "consola.ttf"].iter().flat_map(|f| unix_font_paths(f)).find(|p| Path::new(p).is_file()) else { return };
        let mapped = system_font(path).expect("a stock font passes the check");
        assert_eq!(mapped.bytes(), read_font(Path::new(path)).unwrap());
        assert!(Arc::ptr_eq(&mapped.font, &system_font(path).unwrap().font), "mapped once");
    }

    #[test]
    fn bold_family_always_registered() {
        let defs = build_fonts(&ThemeFont { ui_font: "Z:\\nope\\missing.ttf".into(), ..ThemeFont::default() }, &[]);
        for family in [FontFamily::Name(BOLD_FAMILY.into()), medium_family(), semibold_family()] {
            let chain = &defs.families[&family];
            assert!(!chain.is_empty());
            assert!(chain.iter().all(|name| defs.font_data.contains_key(name)));
            assert!(chain.contains(&"NotoEmoji-Regular".to_string()));
        }
    }

    #[test]
    fn gg_sans_leads_each_weight() {
        // Any face stands in for the downloaded ones (fonts.rs decodes and checks them).
        let face = FontData::from_static(b"stand-in");
        let gg = [(400, face.clone()), (600, face.clone()), (700, face)];
        let defs = build_fonts(&ThemeFont::default(), &gg);
        let first = |f: FontFamily| defs.families[&f][0].clone();
        assert_eq!(first(FontFamily::Proportional), "gg-sans-400");
        assert_eq!(first(semibold_family()), "gg-sans-600");
        assert_eq!(first(FontFamily::Name(BOLD_FAMILY.into())), "gg-sans-700");
        // No medium face: the regular chain, gg sans regular first.
        assert_eq!(defs.families[&medium_family()], defs.families[&FontFamily::Proportional]);
    }

    #[cfg(windows)]
    #[test]
    fn os_emoji_font_sits_between_text_and_monochrome_faces() {
        let defs = build_fonts(&ThemeFont::default(), &[]);
        for family in [FontFamily::Proportional, FontFamily::Monospace, FontFamily::Name(BOLD_FAMILY.into())] {
            let chain = &defs.families[&family];
            let at = |name: &str| chain.iter().position(|n| n == name).unwrap_or_else(|| panic!("{name} in {family:?}"));
            assert!(at(OS_EMOJI) > 0 && at(OS_EMOJI) < at("NotoEmoji-Regular"), "{family:?}: {chain:?}");
        }
        let icons = &defs.families[&icon_family()];
        assert!(icons.contains(&"NotoEmoji-Regular".to_string()) && !icons.contains(&OS_EMOJI.to_string()));
        // Without gg sans, 600 is Segoe UI Semibold (shipped with Windows), not Bold.
        assert_eq!(defs.families[&semibold_family()][..2], ["rc-ui-semibold", "rc-ui-bold"]);
    }

    #[test]
    fn mix_endpoints() {
        let (a, b) = (Rgba::rgb(0, 100, 200), Rgba::rgb(200, 0, 100));
        assert_eq!(mix(a, b, 0.0), a.to_color32());
        assert_eq!(mix(a, b, 1.0), b.to_color32());
    }
}
