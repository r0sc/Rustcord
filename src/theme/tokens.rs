//! Colour tokens derived from a theme's colours (docs/DESIGN.md §2.2). `ThemeColors` stays the
//! TOML contract; everything else is computed here, so custom themes get every token, and the
//! state colours (green online, amber idle, pink live) stay fixed per mode, as in Discord.

use super::apply::{is_high_contrast, luminance, mix};
use super::{Rgba, ThemeColors};
use egui::Color32;

/// Which set of fixed state colours a theme gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Dark,
    Light,
    HighContrast,
}

impl Mode {
    pub fn of(c: &ThemeColors) -> Self {
        if is_high_contrast(c) {
            Self::HighContrast
        } else if luminance(c.background) > 0.4 {
            Self::Light
        } else {
            Self::Dark
        }
    }

    /// Opacity of `text` in the `selected` fill (secondary buttons add 4 / 8 % on hover / press).
    pub fn selected_alpha(self) -> f32 {
        match self {
            Self::Dark => 0.10,
            Self::Light => 0.08,
            Self::HighContrast => 0.18,
        }
    }
}

/// Every hue has one job (§1): purple is `ThemeColors::accent`, the rest live here.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    pub mode: Mode,
    /// Floating layers: menus, popouts, modals, tooltips, pickers, autocomplete.
    pub raised: Color32,
    /// Row and button hover. Translucent, so it works on gradients.
    pub hover: Color32,
    /// Selected row, selected segment, toggled icon button, secondary button.
    pub selected: Color32,
    /// Hairlines: list rows, menu separators, float edges.
    pub rule: Color32,
    /// Component boundaries (>= 3:1): input border, switch-off track, checkbox and radio rings.
    pub edge: Color32,
    /// The purple primary (`ThemeColors::accent`) for helpers that only have `Tokens`: on-state fills.
    pub accent: Color32,
    /// Knob and check drawn on `accent`.
    pub on_accent: Color32,
    /// Purple as text, glyph, ring or bar: readable (4.5:1) on a chip inside a mention row and on
    /// the floats, hue kept.
    pub accent_ink: Color32,
    /// Keyboard focus ring (`accent_ink`).
    pub focus: Color32,
    /// The lit top edge of raised layers (dark themes only).
    pub sheen: Color32,
    /// Online, voice connected, speaking, my activity icon.
    pub positive: Color32,
    /// Green text ("Voice connected").
    pub positive_text: Color32,
    /// Check and knob glyph drawn on `positive`.
    pub on_positive: Color32,
    /// Join / Accept call buttons, with `positive_label` on them.
    pub positive_fill: Color32,
    pub positive_label: Color32,
    /// Idle, "Connecting…", warnings (amber).
    pub attention: Color32,
    /// Streaming status (pink).
    pub live: Color32,
    /// Destructive buttons, count badges, "New" and "Live" tags, danger menu hover, with
    /// `danger_label` on them.
    pub danger_fill: Color32,
    pub danger_label: Color32,
    /// Mention and channel chips, app tags.
    pub chip: Color32,
    /// Selected text.
    pub selection: Color32,
    /// Rail pill colours: a flat accent until the rail commit removes it.
    pub heat: [Color32; 3],
    /// Behind modals.
    pub scrim: Color32,
    /// Media surrounds, the same in every theme (§10.3 media): the lightbox backdrop, text and
    /// icons on media (and on its black 56 % discs), and their hover fill.
    pub media_scrim: Color32,
    pub on_media: Color32,
    pub on_media_muted: Color32,
    pub media_hover: Color32,
}

/// Amber (`attention`) of the dark and light modes, for gradient themes' mention tint.
pub(super) const ATTENTION: (Rgba, Rgba) = (Rgba::rgb(0xeb, 0xc1, 0x66), Rgba::rgb(0xa8, 0x6b, 0x00));

impl Tokens {
    pub fn new(c: &ThemeColors) -> Self {
        let mode = Mode::of(c);
        let k = |d: f32, l: f32, h: f32| match mode {
            Mode::Dark => d,
            Mode::Light => l,
            Mode::HighContrast => h,
        };
        let pick = |d: u32, l: u32, h: u32| hex(match mode {
            Mode::Dark => d,
            Mode::Light => l,
            Mode::HighContrast => h,
        });
        let at = |col: Rgba, a: f32| col.to_color32().gamma_multiply(a);
        let hc = mode == Mode::HighContrast;
        let accent = c.accent.to_color32();
        let raised = if mode == Mode::Light { hex(0xffffff) } else { c.surface.to_color32() };
        // Purple as text: readable on a chip inside a mention row, the most saturated place it
        // sits, and on the floats. readable_on keeps the hue.
        let chip_a = k(0.16, 0.12, 0.22);
        let chip = |under: Color32| under.blend(accent.gamma_multiply(chip_a));
        let row = c.background.to_color32().blend(c.mention_highlight.to_color32()); // opaque for gradient themes too
        let accent_ink = readable_on(readable_on(accent, chip(row)), raised);
        Self {
            mode,
            raised,
            hover: at(c.text, k(0.06, 0.06, 0.12)),
            selected: at(c.text, mode.selected_alpha()),
            rule: if hc { at(c.text_muted, 0.45) } else { at(c.text, k(0.10, 0.12, 0.0)) },
            edge: mix(c.background, c.text_muted, if hc { 0.8 } else { 0.75 }),
            accent,
            on_accent: c.accent_text.to_color32(),
            accent_ink,
            focus: accent_ink,
            sheen: if mode == Mode::Dark { Color32::from_white_alpha(15) } else { Color32::TRANSPARENT },
            positive: pick(0x50c76a, 0x1d8b3d, 0x4fe07a),
            positive_text: pick(0x50c76a, 0x00792b, 0x4fe07a),
            on_positive: pick(0x08070b, 0xffffff, 0x000000),
            positive_fill: pick(0x25803f, 0x25803f, 0x4fe07a),
            positive_label: pick(0xffffff, 0xffffff, 0x000000),
            attention: match mode {
                Mode::Dark => ATTENTION.0.to_color32(),
                Mode::Light => ATTENTION.1.to_color32(),
                Mode::HighContrast => hex(0xffd400),
            },
            live: pick(0xf062c0, 0xb0237f, 0xff8ad8),
            danger_fill: pick(0xcc3d3f, 0xcc2827, 0xff5c5c),
            danger_label: pick(0xffffff, 0xffffff, 0x000000),
            chip: at(c.accent, chip_a),
            selection: at(c.accent, k(0.40, 0.24, 0.40)),
            heat: [accent; 3],
            scrim: Color32::from_black_alpha((255.0 * k(0.64, 0.4, 0.8)).round() as u8),
            media_scrim: Color32::from_black_alpha(if hc { 242 } else { 224 }),
            on_media: Color32::from_white_alpha(235),
            on_media_muted: Color32::from_white_alpha(163),
            media_hover: Color32::from_white_alpha(36),
        }
    }
}

/// `0xrrggbb` through `Rgba::to_color32`, so the Saturation setting applies.
fn hex(rgb: u32) -> Color32 {
    let [_, r, g, b] = rgb.to_be_bytes();
    Rgba::rgb(r, g, b).to_color32()
}

/// `fg` (a role or plugin name colour) as text on `bg` (the opaque pane it sits on): unchanged
/// at 4.5:1 or more, else moved toward black (light panes) or white (dark panes) just far enough
/// for 4.5:1, so the hue stays.
pub fn readable_on(fg: Color32, bg: Color32) -> Color32 {
    let [r, g, b, a] = fg.to_srgba_unmultiplied();
    let lum = |c: [u8; 3]| luminance(Rgba([c[0], c[1], c[2], 255]));
    let lb = lum([bg.r(), bg.g(), bg.b()]);
    let ratio = |l: f32| (l.max(lb) + 0.05) / (l.min(lb) + 0.05);
    if ratio(lum([r, g, b])) >= 4.5 {
        return fg;
    }
    // Black reaches more contrast than white on panes above ~0.18 luminance.
    let to_black = (lb + 0.05) / 0.05 >= 1.05 / (lb + 0.05);
    let target = if to_black { 0.0 } else { 255.0 };
    let at = |t: f32| [r, g, b].map(|v| (f32::from(v) + (target - f32::from(v)) * t).round() as u8);
    // Past the pane's luminance on the target's side and at 4.5:1: monotonic in t.
    let ok = |t: f32| {
        let l = lum(at(t));
        (if to_black { l <= lb } else { l >= lb }) && ratio(l) >= 4.5
    };
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..12 {
        let mid = (lo + hi) / 2.0;
        if ok(mid) { hi = mid } else { lo = mid }
    }
    let [r, g, b] = at(hi);
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    fn ratio(a: Color32, b: Color32) -> f32 {
        let rgba = |c: Color32| Rgba([c.r(), c.g(), c.b(), 255]);
        super::super::apply::contrast_ratio(rgba(a), rgba(b))
    }

    #[test]
    fn builtins_get_their_mode_and_readable_states() {
        let modes: Vec<Mode> = Theme::builtins()[..3].iter().map(|t| Mode::of(&t.colors)).collect();
        assert_eq!(modes, [Mode::Dark, Mode::Light, Mode::HighContrast]);
        for t in &Theme::builtins()[..3] {
            let (c, tk) = (&t.colors, Tokens::new(&t.colors));
            let bg = c.background.to_color32();
            // Component boundaries and state glyphs (the switch check): at least 3:1 (WCAG 1.4.11).
            assert!(ratio(tk.edge, bg) >= 3.0 && ratio(tk.edge, c.surface.to_color32()) >= 3.0, "{} edge", t.name);
            assert!(ratio(tk.positive, bg) >= 3.0 && ratio(tk.on_positive, tk.positive) >= 3.0, "{} positive", t.name);
            assert!(ratio(tk.danger_label, tk.danger_fill) >= 4.5 && ratio(tk.positive_label, tk.positive_fill) >= 4.5, "{} fills", t.name);
            assert!(ratio(c.danger.to_color32(), tk.raised) >= 4.5, "{} danger on menus", t.name);
            assert!(ratio(tk.focus, c.sidebar.to_color32()) >= 3.0, "{} focus", t.name);
            // The purple primary: its label (also hovered), the fill on the pane, purple as text.
            let acc = c.accent.to_color32();
            assert!(ratio(tk.on_accent, acc) >= 4.5, "{} accent label", t.name);
            assert!(ratio(c.accent_text.to_color32(), acc.lerp_to_gamma(Color32::WHITE, 0.06)) >= 4.5, "{} accent label hovered", t.name);
            assert!(ratio(acc, bg) >= 3.0, "{} accent on the pane", t.name);
            let row = bg.blend(c.mention_highlight.to_color32());
            assert!(ratio(tk.accent_ink, row.blend(tk.chip)) >= 4.5, "{} accent_ink on a chip in a mention row", t.name);
            assert!(ratio(tk.accent_ink, tk.raised) >= 4.5, "{} accent_ink on floats", t.name);
        }
        // The spec's value for Nightshade's edge (mix of background and muted text).
        assert_eq!(Tokens::new(&Theme::nightshade().colors).edge, Color32::from_rgb(0x83, 0x80, 0x8f));
    }

    #[test]
    fn readable_on_reaches_4_5_keeping_hue() {
        let (orange, white, black) = (Color32::from_rgb(0xe8, 0x74, 0x3b), Color32::WHITE, Color32::BLACK);
        // Readable colours pass through untouched, alpha included.
        assert_eq!(readable_on(orange, black), orange);
        let faded = Color32::from_rgba_unmultiplied(0x10, 0x20, 0x30, 128);
        assert_eq!(readable_on(faded, white), faded);
        for bg in [white, Color32::from_rgb(0xf2, 0xf3, 0xf5), Color32::from_rgb(0x31, 0x33, 0x38), Color32::from_rgb(0x80, 0x80, 0x80)] {
            for fg in [orange, Color32::from_rgb(0x1a, 0xbc, 0x9c), Color32::from_rgb(0x20, 0x20, 0x60), Color32::from_rgb(0x99, 0xaa, 0xb5)] {
                let out = readable_on(fg, bg);
                assert!(ratio(out, bg) >= 4.5, "{fg:?} on {bg:?} -> {out:?}");
                // Only as far as needed: a step back toward fg drops below 4.5.
                if out != fg {
                    let back = Color32::from_rgb(out.r().midpoint(fg.r()), out.g().midpoint(fg.g()), out.b().midpoint(fg.b()));
                    assert!(ratio(back, bg) < 4.5, "{fg:?} on {bg:?} went too far: {out:?}");
                }
            }
        }
        // Darkened toward black on a light pane: same channel ratios (hue kept).
        let dark = readable_on(orange, white);
        assert!(dark.r() < orange.r() && (f32::from(dark.g()) / f32::from(dark.r()) - 0x74 as f32 / 0xe8 as f32).abs() < 0.02);
    }
}
