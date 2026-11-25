//! Gradient app themes (the Nitro colour themes, free and local only; COSMETICS_NOTES §13):
//! the presets, the token tint, the panel overlays and the window-wide gradient mesh.

use super::{GradientStop, PanelFills, Rgba, Theme, ThemeApplier, ThemeColors, ThemeGradient};
use egui::epaint::Vertex;
use egui::{Mesh, Pos2, Rect, Shape};
use std::sync::Arc;

/// (name, dark base, CSS angle, stops `(rgb, %)`, overlay multiplier), in the web picker's order
/// (§13.2; ids 0–15, 17, 18, 20, 21, 19, 22, then the campaign-only Hexagon). The Easter Egg
/// preset (the old brand colours) is left out; the two "Blurple" presets carry Rustcord names.
type Preset = (&'static str, bool, f32, &'static [(u32, f32)], f32);
const PRESETS: [Preset; 23] = [
    ("Mint Apple", false, 180.0, &[(0x56b69f, 6.15), (0x63bc61, 48.7), (0x9eca67, 93.07)], 1.0),
    ("Citrus Sherbert", false, 180.0, &[(0xf3b336, 31.1), (0xee8558, 67.09)], 1.0),
    ("Retro Raincloud", false, 148.71, &[(0x3a7ca1, 5.64), (0x7f7eb9, 26.38), (0x7f7eb9, 49.92), (0x3a7ca1, 73.12)], 1.0),
    ("Hanami", false, 38.08, &[(0xefaab3, 3.56), (0xefd696, 35.49), (0xa6daa2, 68.78)], 1.0),
    ("Sunrise", false, 154.19, &[(0x9f4175, 8.62), (0xc49064, 48.07), (0xa6953d, 76.04)], 1.0),
    ("Cotton Candy", false, 180.14, &[(0xf4abb8, 8.5), (0xb1c2fc, 94.28)], 1.0),
    ("Lofi Vibes", false, 179.52, &[(0xa4c0f7, 7.08), (0xa9e4e8, 34.94), (0xb0e2b8, 65.12), (0xcfdfa2, 96.23)], 1.0),
    ("Desert Khaki", false, 38.99, &[(0xe7dbd0, 12.92), (0xdfd0b2, 32.92), (0xe0d6a3, 52.11)], 1.0),
    ("Sunset", true, 141.68, &[(0x48288c, 27.57), (0xdb7f4b, 71.25)], 1.0),
    ("Chroma Glow", true, 128.92, &[(0x0eb5bf, 3.94), (0x4c0ce0, 26.1), (0xa308a7, 39.82), (0x9a53ff, 56.89), (0x218be0, 76.45)], 1.0),
    ("Forest", true, 162.27, &[(0x142215, 11.2), (0x2d4d39, 29.93), (0x454c32, 48.64), (0x5a7c58, 67.85), (0xa98e4b, 83.54)], 1.0),
    ("Crimson Moon", true, 64.92, &[(0x950909, 16.17), (0x000000, 72.0)], 1.0),
    ("Midnight Indigo", true, 48.17, &[(0x5348ca, 11.21), (0x140730, 61.92)], 1.0),
    ("Mars", true, 170.82, &[(0x895240, 14.61), (0x8f4343, 74.62)], 1.0),
    ("Dusk", true, 180.0, &[(0x665069, 12.84), (0x91a3d1, 85.99)], 1.0),
    ("Under the Sea", true, 179.14, &[(0x647962, 1.91), (0x588575, 48.99), (0x6a8482, 96.35)], 1.0),
    ("Retro Storm", true, 148.71, &[(0x3a7ca1, 5.64), (0x58579a, 26.38), (0x58579a, 49.92), (0x3a7ca1, 73.12)], 1.0),
    ("Neon Nights", true, 180.0, &[(0x01a89e, 0.0), (0x7d60ba, 50.0), (0xb43898, 100.0)], 1.0),
    ("Strawberry Lemonade", true, 161.03, &[(0xaf1a6c, 18.79), (0xc26b20, 49.76), (0xe7a525, 80.72)], 1.0),
    ("Aurora", true, 239.16, &[(0x062053, 10.39), (0x191fbb, 26.87), (0x13929a, 48.31), (0x218573, 64.98), (0x051a81, 92.5)], 1.0),
    ("Sepia", true, 69.98, &[(0x857664, 14.14), (0x5b4421, 60.35)], 1.0),
    ("Indigo Twilight", true, 47.61, &[(0x2c3fe7, 11.18), (0x261d83, 64.54)], 1.0),
    ("Hexagon", true, 135.0, &[(0x4645ff, 0.0), (0xaf01f3, 33.33), (0xff6698, 66.67), (0xf89a03, 100.0)], 0.8),
];

/// Panel overlay opacity per level (§13.4B preset columns): (dark, light).
const FRAME: (f32, f32) = (0.70, 0.65); // title bar, server bar ("higher")
const SIDE: (f32, f32) = (0.75, 0.70); // channel list card, member list ("high")
const CHAT: (f32, f32) = (0.80, 0.85); // pages ("chat")

/// The built-in gradient themes, in picker order.
pub(super) fn presets() -> impl Iterator<Item = Theme> {
    PRESETS.iter().map(|&(name, dark, angle, stops, overlay)| {
        let stops = stops.iter().map(|&(rgb, at)| GradientStop { color: rgb_hex(rgb), at }).collect();
        Theme::from_gradient(name, dark, ThemeGradient { angle, stops, overlay })
    })
}

fn rgb_hex(rgb: u32) -> Rgba {
    let [_, r, g, b] = rgb.to_be_bytes();
    Rgba::rgb(r, g, b)
}

impl Theme {
    /// A gradient theme on Rustcord's dark or light base, its colours derived from the gradient.
    pub fn from_gradient(name: &str, dark: bool, gradient: ThemeGradient) -> Theme {
        let base = if dark { Theme::nightshade() } else { Theme::lilac() };
        let mut theme = Theme { name: name.into(), gradient: Some(gradient), ..base };
        theme.retint();
        theme
    }

    /// Re-derives the colours of a gradient theme from its gradient and dark/light base (§13.3,
    /// §13.4C): surfaces and text tinted in OKLab, the panel tokens the gradient's average under
    /// the panel overlay, hover / mention (purple) translucent so the gradient shows through.
    /// Accent and danger stay. No-op for flat themes.
    pub fn retint(&mut self) {
        let Some(g) = &self.gradient else { return };
        let base = if self.dark { Theme::nightshade() } else { Theme::lilac() }.colors;
        let (tint, text) = base_and_text(g, self.dark);
        // Base colour at 50 % and text colour at 38 % of `mix`: w = amt / (100 + amt).
        let (w_base, w_text) = (1.0 / 3.0, 38.0 / 138.0);
        let avg = g.average();
        let panel = |level: (f32, f32)| {
            let a = overlay_alpha(g, self.dark, level);
            let o = if self.dark { 0.0 } else { 255.0 };
            Rgba(std::array::from_fn(|i| if i == 3 { 255 } else { (f32::from(avg.0[i]) * (1.0 - a) + o * a).round() as u8 }))
        };
        let mut c = ThemeColors {
            background: panel(CHAT),
            sidebar: panel(SIDE),
            guild_bar: panel(FRAME),
            surface: oklab_mix(base.surface, tint, w_base),
            code_background: oklab_mix(base.code_background, tint, w_base),
            text: oklab_mix(base.text, text, w_text),
            text_muted: oklab_mix(base.text_muted, text, w_text),
            link: oklab_mix(base.link, text, w_text),
            message_hover: with_alpha(oklab_mix(base.text, text, w_text), 0x0f),
            // A 13 % purple, like the flat themes' mention row.
            mention_highlight: with_alpha(base.accent, 0x21),
            ..base
        };
        // Very dark gradients would pass as high contrast (strong borders everywhere).
        if super::is_high_contrast(&c) {
            c.text_muted = lerp(c.text_muted, c.background, 0.15);
        }
        self.colors = c;
    }

    /// Fills of the main window's columns (`ui::main_layout`, the settings layers). Flat themes:
    /// the theme's own colours. Gradient themes: black (dark) or white (light) overlays over the
    /// window gradient, one level per panel (§13.4B).
    pub fn panel_fills(&self) -> PanelFills {
        let c = &self.colors;
        let Some(g) = &self.gradient else {
            return PanelFills { frame: c.guild_bar, card: c.sidebar, chat: c.background, side: c.sidebar };
        };
        let fill = |a: f32| {
            let o = if self.dark { 0 } else { 255 };
            Rgba([o, o, o, (a.clamp(0.0, 1.0) * 255.0).round() as u8])
        };
        let (frame, side) = (overlay_alpha(g, self.dark, FRAME), overlay_alpha(g, self.dark, SIDE));
        PanelFills {
            frame: fill(frame),
            // Drawn over `frame`: together they make the `side` level.
            card: fill(if frame < 1.0 { (side - frame).max(0.0) / (1.0 - frame) } else { 0.0 }),
            chat: fill(overlay_alpha(g, self.dark, CHAT)),
            side: fill(side),
        }
    }

    /// `fg` as text on a main-window pane whose flat colour is `pane` (`readable_on`). Gradient
    /// themes paint panes as overlays over the window gradient, so `fg` also has to read wherever
    /// the gradient shows through the overlay; flat themes: `readable_on(fg, pane)`.
    pub fn readable_on_pane(&self, fg: egui::Color32, pane: egui::Color32) -> egui::Color32 {
        let fg = super::readable_on(fg, pane);
        let Some(g) = &self.gradient else { return fg };
        let (avg, o) = (g.average().to_color32().to_array(), if self.dark { 0.0 } else { 255.0 });
        let pane = pane.to_array();
        let ch = |c: [u8; 4], i: usize| f32::from(c[i]);
        // pane = avg + (overlay - avg) * alpha: the share of the gradient that shows through
        // (1 - alpha), read off the channel furthest from the overlay colour.
        let i = (0..3).max_by(|&i, &j| (ch(avg, i) - o).abs().total_cmp(&(ch(avg, j) - o).abs())).unwrap_or(0);
        let through = if (ch(avg, i) - o).abs() < 1.0 { 0.0 } else { ((ch(pane, i) - o) / (ch(avg, i) - o)).clamp(0.0, 1.0) };
        // Points along the gradient line (between stops the darkest point can lie inside a band).
        // Each pass only moves `fg` further from the pane, so it stays readable on the earlier ones.
        (0..=16).fold(fg, |fg, k| {
            let at = g.color_at(k as f32 / 16.0).to_color32().to_array();
            let [r, g, b] = [0, 1, 2].map(|i| (ch(pane, i) + (ch(at, i) - ch(avg, i)) * through).round().clamp(0.0, 255.0) as u8);
            super::readable_on(fg, egui::Color32::from_rgb(r, g, b))
        })
    }
}

fn overlay_alpha(g: &ThemeGradient, dark: bool, (d, l): (f32, f32)) -> f32 {
    let m = if g.overlay.is_finite() { g.overlay } else { 1.0 };
    ((if dark { d } else { l }) * m).clamp(0.0, 1.0)
}

fn with_alpha(c: Rgba, a: u8) -> Rgba {
    Rgba([c.0[0], c.0[1], c.0[2], a])
}

impl ThemeGradient {
    /// Stop positions as fractions 0..=1, never decreasing (CSS clamps a stop placed before the
    /// previous one to that one's position).
    fn positions(&self) -> impl Iterator<Item = f32> + '_ {
        let mut last = f32::NEG_INFINITY;
        self.stops.iter().map(move |s| {
            last = last.max(if s.at.is_finite() { s.at / 100.0 } else { 0.0 });
            last
        })
    }

    /// Colour at `t` along the gradient line (0 = start, 1 = end): the end stops' colours
    /// outside them, an sRGB blend between two stops (as CSS draws it).
    pub fn color_at(&self, t: f32) -> Rgba {
        let Some(first) = self.stops.first() else { return Rgba::rgb(0, 0, 0) };
        let (mut prev, mut prev_at) = (first.color, f32::NEG_INFINITY);
        for (s, at) in self.stops.iter().zip(self.positions()) {
            if t <= at {
                let k = if at > prev_at && prev_at.is_finite() { (t - prev_at) / (at - prev_at) } else { 1.0 };
                return lerp(prev, s.color, k);
            }
            (prev, prev_at) = (s.color, at);
        }
        prev
    }

    /// Mean colour along the gradient line (sRGB), for the opaque panel tokens.
    fn average(&self) -> Rgba {
        const N: usize = 32;
        let mut sum = [0.0f32; 3];
        for i in 0..N {
            let c = self.color_at((i as f32 + 0.5) / N as f32);
            for (s, v) in sum.iter_mut().zip(c.0) {
                *s += f32::from(v);
            }
        }
        Rgba::rgb((sum[0] / N as f32).round() as u8, (sum[1] / N as f32).round() as u8, (sum[2] / N as f32).round() as u8)
    }

    /// `poly` (convex, inside `bounds`) filled with the gradient laid over `bounds`: cut into one
    /// band per pair of neighbouring stops, so per-vertex colours are exact (the colour is affine
    /// in position inside a band). A few dozen vertices; build it once per size, not per frame.
    pub fn mesh(&self, bounds: Rect, poly: &[Pos2]) -> Mesh {
        let (sin, cos) = self.angle.to_radians().sin_cos();
        let len = (bounds.width() * sin).abs() + (bounds.height() * cos).abs();
        let centre = bounds.center();
        // CSS gradient line: 0° points up, 90° right; screen y grows downwards.
        let t = |p: Pos2| if len > 0.0 { ((p.x - centre.x) * sin - (p.y - centre.y) * cos) / len + 0.5 } else { 0.5 };
        let mut mesh = Mesh::default();
        let mut lo = f32::NEG_INFINITY;
        for hi in self.positions().chain([f32::INFINITY]) {
            let band = clip(&clip(poly, |p| t(p) - lo), |p| hi - t(p));
            if band.len() >= 3 {
                let at = mesh.vertices.len() as u32;
                mesh.vertices.extend(band.iter().map(|&pos| Vertex { pos, uv: egui::epaint::WHITE_UV, color: self.color_at(t(pos)).to_color32() }));
                for i in 1..band.len() as u32 - 1 {
                    mesh.add_triangle(at, at + i, at + i + 1);
                }
            }
            lo = hi;
        }
        mesh
    }
}

/// The part of convex `poly` where `f >= 0` (`f` affine): Sutherland–Hodgman against one line.
fn clip(poly: &[Pos2], f: impl Fn(Pos2) -> f32) -> Vec<Pos2> {
    let mut out = Vec::with_capacity(poly.len() + 1);
    for (i, &a) in poly.iter().enumerate() {
        let b = poly[(i + 1) % poly.len()];
        let (fa, fb) = (f(a), f(b));
        if fa >= 0.0 {
            out.push(a);
        }
        if (fa >= 0.0) != (fb >= 0.0) {
            out.push(a + (b - a) * (fa / (fa - fb)));
        }
    }
    out
}

/// The gradient's base and text colours (§13.3, chroma-js `Zk`): from its lightest and darkest
/// end stop, fully saturated, at fixed lightness.
fn base_and_text(g: &ThemeGradient, dark: bool) -> (Rgba, Rgba) {
    let first = g.stops.first().map_or(Rgba::rgb(0, 0, 0), |s| s.color);
    let last = g.stops.last().map_or(first, |s| s.color);
    let (lc, mut dc) = if super::apply::luminance(first) > super::apply::luminance(last) { (first, last) } else { (last, first) };
    if hue(dc).is_none_or(|h| h == 0.0) {
        dc = lc;
    }
    let base = if dark { saturated(dc, 0.05) } else { saturated(lc, 0.90) };
    let seed = if dark { lc } else { dc };
    let blue = hue(seed).is_some_and(|h| (200.0..=300.0).contains(&h));
    let l = match (blue, dark) {
        (true, true) => 0.80,
        (true, false) => 0.25,
        (false, true) => 0.95,
        (false, false) => 0.15,
    };
    (base, saturated(seed, l))
}

/// HSL hue in degrees; None for greys.
fn hue(c: Rgba) -> Option<f32> {
    let [r, g, b] = [c.0[0], c.0[1], c.0[2]].map(|v| f32::from(v) / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let d = max - min;
    if d <= 0.0 {
        return None;
    }
    let h = if max == r { ((g - b) / d).rem_euclid(6.0) } else if max == g { (b - r) / d + 2.0 } else { (r - g) / d + 4.0 };
    Some(h * 60.0)
}

/// `c`'s hue at HSL saturation 1 and lightness `l` (greys stay grey).
fn saturated(c: Rgba, l: f32) -> Rgba {
    let Some(h) = hue(c) else {
        let v = (l * 255.0).round() as u8;
        return Rgba::rgb(v, v, v);
    };
    let chroma = 1.0 - (2.0 * l - 1.0).abs();
    let x = chroma * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = l - chroma / 2.0;
    let ch = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgba::rgb(ch(r), ch(g), ch(b))
}

/// sRGB blend `a -> b` by `t` (0..=1), alpha of `a`.
fn lerp(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    Rgba(std::array::from_fn(|i| if i == 3 { a.0[3] } else { (f32::from(a.0[i]) + (f32::from(b.0[i]) - f32::from(a.0[i])) * t).round() as u8 }))
}

/// Blend `a -> b` by `w` in OKLab (the web client's `color-mix(in oklab)`), alpha of `a`.
fn oklab_mix(a: Rgba, b: Rgba, w: f32) -> Rgba {
    let (la, lb) = (to_oklab(a), to_oklab(b));
    let w = f64::from(w);
    from_oklab(std::array::from_fn(|i| la[i] + (lb[i] - la[i]) * w), a.0[3])
}

fn to_oklab(c: Rgba) -> [f64; 3] {
    let lin = |v: u8| {
        let v = f64::from(v) / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    let (r, g, b) = (lin(c.0[0]), lin(c.0[1]), lin(c.0[2]));
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    [
        0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s,
        1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s,
        0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s,
    ]
}

fn from_oklab([l, a, b]: [f64; 3], alpha: u8) -> Rgba {
    let l_ = (l + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3);
    let m_ = (l - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3);
    let s_ = (l - 0.089_484_177_5 * a - 1.291_485_548_0 * b).powi(3);
    let enc = |v: f64| {
        let v = v.clamp(0.0, 1.0);
        let s = if v <= 0.003_130_8 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (s * 255.0).round() as u8
    };
    Rgba([
        enc(4.076_741_662_1 * l_ - 3.307_711_591_3 * m_ + 0.230_969_929_2 * s_),
        enc(-1.268_438_004_6 * l_ + 2.609_757_401_1 * m_ - 0.341_319_396_5 * s_),
        enc(-0.004_196_086_3 * l_ - 0.703_418_614_7 * m_ + 1.707_614_701_0 * s_),
        alpha,
    ])
}

impl ThemeApplier {
    /// Paints the theme's gradient over the whole window, under everything (no-op for flat
    /// themes). The mesh is rebuilt only when the window size or the theme changes.
    pub fn paint_background(&mut self, ctx: &egui::Context) {
        let Some(g) = &self.gradient else { return };
        let rect = ctx.viewport_rect();
        if self.gradient_mesh.as_ref().is_none_or(|(r, _)| *r != rect) {
            let corners = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
            self.gradient_mesh = Some((rect, Arc::new(g.mesh(rect, &corners))));
        }
        if let Some((_, mesh)) = &self.gradient_mesh {
            ctx.layer_painter(egui::LayerId::background()).add(Shape::Mesh(mesh.clone()));
        }
    }
}

/// A round gradient swatch (theme picker): `g` filling the circle at `centre`.
pub fn swatch_mesh(g: &ThemeGradient, centre: Pos2, radius: f32) -> Mesh {
    let rim: Vec<Pos2> = (0..40).map(|i| centre + radius * egui::Vec2::angled(i as f32 * std::f32::consts::TAU / 40.0)).collect();
    g.mesh(Rect::from_center_size(centre, egui::Vec2::splat(radius * 2.0)), &rim)
}

#[cfg(test)]
mod tests {
    use super::super::apply::contrast_ratio;
    use super::*;
    use egui::pos2;

    fn close(a: Rgba, b: Rgba, tol: u8) -> bool {
        a.0.iter().zip(b.0).all(|(x, y)| x.abs_diff(y) <= tol)
    }

    #[test]
    fn base_and_text_match_the_captured_values() {
        // §13.2: base / text colours the web client emitted (Hexagon's are computed there too).
        let want = [
            (0xe8ffcc, 0x004d3a), (0xffeecc, 0x4d1700), (0xccedff, 0x005280), (0xd0ffcc, 0x4d000a),
            (0xfff7cc, 0x4d002a), (0xccd7ff, 0x4d000e), (0xf2ffcc, 0x002a80), (0xffe4cc, 0x4d4000),
            (0x08001a, 0xffefe5), (0x000e1a, 0xe5feff), (0x001a02, 0xfff8e5), (0x1a0000, 0xffe5e5),
            (0x08001a, 0xa299ff), (0x1a0600, 0xffece5), (0x16001a, 0x99b6ff), (0x021a00, 0xe5fffd),
            (0x00101a, 0x99daff), (0x1a0014, 0xe5fffd), (0x1a000e, 0xfff6e5), (0x00091a, 0x99abff),
            (0x1a0f00, 0xfff3e5), (0x02001a, 0x99a3ff), (0x00001a, 0xfff5e5),
        ];
        for (t, (base, text)) in presets().zip(want) {
            let (b, x) = base_and_text(t.gradient.as_ref().unwrap(), t.dark);
            assert!(close(b, rgb_hex(base), 3), "{}: base {} vs {}", t.name, b.to_hex(), rgb_hex(base).to_hex());
            assert!(close(x, rgb_hex(text), 3), "{}: text {} vs {}", t.name, x.to_hex(), rgb_hex(text).to_hex());
        }
    }

    #[test]
    fn oklab_mix_matches_the_notes_vectors() {
        // §13.4D (Sunset on dark, Mint Apple on light).
        let mix = |a, b, w| oklab_mix(rgb_hex(a), rgb_hex(b), w);
        let w_text = 38.0 / 138.0;
        assert!(close(mix(0xefeff1, 0xffefe5, w_text), rgb_hex(0xf3efee), 2));
        assert!(close(mix(0x96979e, 0xffefe5, w_text), rgb_hex(0xb2afb1), 2));
        assert!(close(mix(0x242429, 0x08001a, 1.0 / 3.0), rgb_hex(0x1a1725), 2));
        assert!(close(mix(0x2e2e34, 0x004d3a, w_text), rgb_hex(0x293736), 2));
        assert!(close(mix(0xffffff, 0xe8ffcc, 1.0 / 3.0), rgb_hex(0xf7ffee), 2));
        assert_eq!(mix(0x123456, 0xabcdef, 0.0), rgb_hex(0x123456));
    }

    #[test]
    fn color_at_clamps_and_blends_between_stops() {
        let g = ThemeGradient {
            angle: 90.0,
            stops: vec![GradientStop { color: Rgba::rgb(0, 0, 0), at: 20.0 }, GradientStop { color: Rgba::rgb(200, 100, 0), at: 60.0 }],
            overlay: 1.0,
        };
        assert_eq!(g.color_at(0.0), Rgba::rgb(0, 0, 0));
        assert_eq!(g.color_at(0.4), Rgba::rgb(100, 50, 0));
        assert_eq!(g.color_at(1.0), Rgba::rgb(200, 100, 0));
        // A stop before the previous one sits on it: a hard edge, no reversal.
        let mut back = g.clone();
        back.stops.push(GradientStop { color: Rgba::rgb(0, 0, 255), at: 10.0 });
        assert_eq!(back.color_at(0.6), Rgba::rgb(200, 100, 0));
        assert_eq!(back.color_at(0.61), Rgba::rgb(0, 0, 255));
        let one = ThemeGradient { stops: vec![GradientStop { color: Rgba::rgb(9, 9, 9), at: 50.0 }], ..g };
        assert_eq!(one.color_at(0.0), Rgba::rgb(9, 9, 9));
        assert_eq!(one.color_at(1.0), Rgba::rgb(9, 9, 9));
    }

    #[test]
    fn mesh_covers_the_rect_with_the_css_gradient_line() {
        let g = presets().find(|t| t.name == "Chroma Glow").unwrap().gradient.unwrap();
        let rect = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(1280.0, 720.0));
        let corners = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
        let mesh = g.mesh(rect, &corners);
        let area: f32 = mesh
            .indices
            .chunks(3)
            .map(|t| {
                let [a, b, c] = [0, 1, 2].map(|i| mesh.vertices[t[i] as usize].pos);
                ((b - a).x * (c - a).y - (b - a).y * (c - a).x).abs() / 2.0
            })
            .sum();
        assert!((area - rect.area()).abs() < 1.0, "{area}");
        assert!(mesh.vertices.len() < 40);
        // 90°: left edge = first stop, right edge = last stop.
        let flat = ThemeGradient { angle: 90.0, ..g.clone() };
        let m = flat.mesh(rect, &corners);
        let at = |x: f32| m.vertices.iter().find(|v| v.pos.x == x).unwrap().color;
        assert_eq!(at(0.0), g.stops[0].color.to_color32());
        assert_eq!(at(1280.0), g.stops[4].color.to_color32());
        // 180°: top = first stop.
        let down = ThemeGradient { angle: 180.0, ..g.clone() };
        let m = down.mesh(rect, &corners);
        assert!(m.vertices.iter().filter(|v| v.pos.y == 0.0).all(|v| v.color == g.stops[0].color.to_color32()));
    }

    #[test]
    fn presets_are_readable() {
        for t in presets() {
            let g = t.gradient.as_ref().unwrap();
            let fills = t.panel_fills();
            assert!(!super::super::is_high_contrast(&t.colors), "{}", t.name);
            // Text over the darkest / lightest point of every panel level, as drawn.
            for fill in [fills.chat, fills.side, fills.frame] {
                for i in 0..=20 {
                    let under = g.color_at(i as f32 / 20.0).to_color32();
                    let bg = under.blend(fill.to_color32());
                    let bg = Rgba([bg.r(), bg.g(), bg.b(), 255]);
                    let (text, muted) = (contrast_ratio(t.colors.text, bg), contrast_ratio(t.colors.text_muted, bg));
                    assert!(text >= 7.0 && muted >= 3.0, "{}: text {text:.1} muted {muted:.1} on {}", t.name, bg.to_hex());
                }
            }
        }
    }

    #[test]
    fn names_read_on_every_point_of_a_gradient_pane() {
        let names = [0xe8743b, 0x7b6cff, 0x1abc9c, 0x206694, 0xf1c40f].map(|c| rgb_hex(c).to_color32());
        for t in presets() {
            let (g, fills) = (t.gradient.as_ref().unwrap(), t.panel_fills());
            for (pane, fill) in [(t.colors.background, fills.chat), (t.colors.sidebar, fills.side)] {
                for fg in names {
                    let out = t.readable_on_pane(fg, pane.to_color32());
                    // As drawn: the gradient under the pane's overlay (4.45: 8-bit rounding).
                    for i in 0..=40 {
                        let bg = g.color_at(i as f32 / 40.0).to_color32().blend(fill.to_color32());
                        let r = contrast_ratio(Rgba([out.r(), out.g(), out.b(), 255]), Rgba([bg.r(), bg.g(), bg.b(), 255]));
                        assert!(r >= 4.45, "{}: {fg:?} -> {out:?} reads {r:.2} on {bg:?}", t.name);
                    }
                }
            }
        }
        // Flat themes: plain `readable_on`.
        let flat = Theme::lilac();
        assert_eq!(flat.readable_on_pane(names[0], flat.colors.sidebar.to_color32()), super::super::readable_on(names[0], flat.colors.sidebar.to_color32()));
    }

    #[test]
    fn flat_themes_keep_their_fills() {
        let t = Theme::nightshade();
        let f = t.panel_fills();
        assert_eq!((f.frame, f.card, f.chat, f.side), (t.colors.guild_bar, t.colors.sidebar, t.colors.background, t.colors.sidebar));
        let mut copy = t.clone();
        copy.retint();
        assert_eq!(copy, t);
        // Gradient: the card over the frame makes the side level.
        let s = presets().find(|t| t.name == "Sunset").unwrap();
        let f = s.panel_fills();
        let stacked = 1.0 - (1.0 - f32::from(f.frame.0[3]) / 255.0) * (1.0 - f32::from(f.card.0[3]) / 255.0);
        assert!((stacked - f32::from(f.side.0[3]) / 255.0).abs() < 0.01);
        assert_eq!(f.chat, Rgba([0, 0, 0, 204]));
    }
}
