//! Aura (docs/DESIGN.md §17): each person's own colour on everything about them: their profile
//! popout and full profile (as a profile theme, `cosmetics::theme_*`), their member and DM row on
//! hover or selection, my user panel, and a band under a server's name from its icon. Nitro
//! profile themes keep Discord's colours; Settings > Appearance > People's colours: Bold, Subtle
//! or Off; High Contrast has none.
//!
//! The colour: the profile theme's primary, else the profile's accent (banner) colour, else the
//! average of the avatar (or icon) already in the image disk cache, decoded once on a worker
//! thread. No new traffic.

use crate::config::PeopleColors;
use crate::model::{Id, User};
use crate::theme::Mode;
use egui::ecolor::{Hsva, HsvaGamma};
use egui::epaint::RectShape;
use egui::{Color32, CornerRadius, Rect, Shape, pos2};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use super::UiCx;

/// Past this many decoded pictures the colour cache starts over.
const MAX_COLORS: usize = 2000;
/// The accent's hue (#A42BEA, 278°): a person's colour this close to it is calmed, so purple
/// keeps meaning "you".
const ACCENT_HUE: f32 = 278.0 / 360.0;

/// Average colours per image URL; None: being decoded, or not a picture.
fn colors() -> &'static Mutex<HashMap<String, Option<[u8; 3]>>> {
    static C: OnceLock<Mutex<HashMap<String, Option<[u8; 3]>>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The colour of the picture at `url` once it is in the disk cache (decoded on a worker thread,
/// which wakes the UI); None until then, or when it has none.
pub fn image_color(ctx: &egui::Context, url: &str) -> Option<Color32> {
    let mut map = colors().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(c) = map.get(url) {
        return c.map(|[r, g, b]| Color32::from_rgb(r, g, b));
    }
    let path = crate::cache::images::cache_path(url);
    if !path.exists() {
        return None;
    }
    if map.len() >= MAX_COLORS {
        map.clear();
    }
    map.insert(url.to_owned(), None);
    let (url, ctx) = (url.to_owned(), ctx.clone());
    // shortcut: one short thread per picture (a 64 px avatar decodes in well under 1 ms); a pool if hovering ever spawns many.
    let spawned = std::thread::Builder::new().name("rc-aura".into()).spawn(move || {
        let c = std::fs::read(&path).ok().and_then(|b| average(&b));
        colors().lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(url, c);
        if c.is_some() {
            ctx.request_repaint();
        }
    });
    if spawned.is_err() {
        eprintln!("rustcord: aura: no thread");
    }
    None
}

/// A picture's dominant colour: its opaque pixels in 12 hue bins, each weighted by its chroma
/// squared (a grey or white background barely counts), and the heaviest bin's average. A mean
/// of all pixels would turn a colourful picture into mud. None: not decodable, or empty.
fn average(bytes: &[u8]) -> Option<[u8; 3]> {
    let img = image::load_from_memory(bytes).ok()?.thumbnail(24, 24).to_rgba8();
    let mut bins = [[0.0f32; 4]; 12];
    for p in img.pixels().filter(|p| p[3] >= 128) {
        let [r, g, b] = [p[0], p[1], p[2]].map(|v| f32::from(v) / 255.0);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let c = max - min;
        let hue = if c == 0.0 {
            0.0
        } else if max == r {
            ((g - b) / c).rem_euclid(6.0)
        } else if max == g {
            (b - r) / c + 2.0
        } else {
            (r - g) / c + 4.0
        };
        let w = c * c + 0.002;
        let bin = &mut bins[((hue * 2.0) as usize).min(11)];
        *bin = [bin[0] + r * w, bin[1] + g * w, bin[2] + b * w, bin[3] + w];
    }
    let top = bins.iter().max_by(|a, b| a[3].total_cmp(&b[3]))?;
    (top[3] > 0.0).then(|| [0, 1, 2].map(|i| (top[i] / top[3] * 255.0).round() as u8))
}

/// `c` as an aura: saturation 45-72 %, brightness 38-72 %, a hue near the accent calmed to 18 %
/// saturation. None for a colour without hue (greys add nothing).
fn normalize(c: Color32) -> Option<Color32> {
    let mut h = HsvaGamma::from(Hsva::from(c));
    if h.s < 0.12 {
        return None;
    }
    h.s = h.s.clamp(0.45, 0.72);
    let d = (h.h - ACCENT_HUE).abs();
    if d.min(1.0 - d) < 20.0 / 360.0 {
        h.s = 0.18;
    }
    h.v = h.v.clamp(0.38, 0.72);
    Some(Color32::from(Hsva::from(h)))
}

/// `user`'s aura colour, or None (setting off, High Contrast, no colour known yet).
pub fn of(ctx: &egui::Context, cx: &UiCx<'_>, user: &User, guild_id: Option<Id>) -> Option<Color32> {
    if cx.settings.people_colors == PeopleColors::Off || cx.theme.tokens().mode == Mode::HighContrast {
        return None;
    }
    let profile = super::profile_modal::loaded(cx, user.id, guild_id);
    let given = super::cosmetics::theme_colors(user.id, profile).map(|(primary, _)| primary).or(profile.and_then(|p| p.user_profile.accent_color)).or(user.accent_color);
    let raw = match given {
        Some(c) => super::rgb(c),
        // A default avatar is Discord's colour, not theirs.
        None if user.avatar.as_deref().is_some_and(|h| !h.is_empty()) => image_color(ctx, &crate::api::cdn::avatar_url(user, 64, false))?,
        None => return None,
    };
    normalize(raw)
}

/// A server's aura from its icon (None: no icon, setting off, High Contrast).
pub fn of_guild(ctx: &egui::Context, cx: &UiCx<'_>, guild_id: Id) -> Option<Color32> {
    if cx.settings.people_colors == PeopleColors::Off || cx.theme.tokens().mode == Mode::HighContrast {
        return None;
    }
    let url = crate::api::cdn::guild_icon_url(cx.state.guild(guild_id)?, super::sidebar::ICON_PX, false)?;
    normalize(image_color(ctx, &url)?)
}

/// An aura as a profile theme `(primary, accent)` for `cosmetics::theme_*`: the colour over a
/// deeper one (dark app themes), or both near white (light ones); Subtle halves the colour.
pub fn theme_pair(c: Color32, strength: PeopleColors, mode: Mode) -> (u32, u32) {
    let grey = Color32::from_gray(((u16::from(c.r()) + u16::from(c.g()) + u16::from(c.b())) / 3) as u8);
    let c = if strength == PeopleColors::Subtle { c.lerp_to_gamma(grey, 0.5) } else { c };
    let (top, bottom) = match mode {
        Mode::Light => (c.lerp_to_gamma(Color32::WHITE, 0.72), c.lerp_to_gamma(Color32::WHITE, 0.55)),
        _ => {
            // A dark card (`cosmetics::theme_dark`: luma under 0.5) in a dark app: a bright colour
            // is darkened to luma 0.42 (scaling the channels scales the luma).
            let luma = (0.299 * f32::from(c.r()) + 0.587 * f32::from(c.g()) + 0.114 * f32::from(c.b())) / 255.0;
            let c = if luma > 0.42 { c.lerp_to_gamma(Color32::BLACK, 1.0 - 0.42 / luma) } else { c };
            (c, c.lerp_to_gamma(Color32::BLACK, 0.55))
        }
    };
    let hex = |c: Color32| u32::from_be_bytes([0, c.r(), c.g(), c.b()]);
    (hex(top), hex(bottom))
}

/// The wash's opacity at its strong end: Bold 55 % on dark themes (35 % on light), Subtle half.
pub fn alpha(strength: PeopleColors, mode: Mode) -> f32 {
    let bold = if mode == Mode::Light { 0.35 } else { 0.55 };
    if strength == PeopleColors::Subtle { bold / 2.0 } else { bold }
}

/// `color` over `rect` fading out to the right (`horizontal`) or downwards: one shared two-texel
/// ramp texture per direction, tinted by the shape's fill, so no texture per colour.
pub fn ramp(ctx: &egui::Context, rect: Rect, radius: CornerRadius, color: Color32, horizontal: bool) -> Shape {
    let id = egui::Id::unique(("rc_aura_ramp", horizontal));
    let tex = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)).unwrap_or_else(|| {
        let size = if horizontal { [2, 1] } else { [1, 2] };
        let img = egui::ColorImage::new(size, vec![Color32::WHITE, Color32::TRANSPARENT]);
        let t = ctx.load_texture(if horizontal { "rc-aura-ramp-h" } else { "rc-aura-ramp-v" }, img, egui::TextureOptions::LINEAR);
        ctx.data_mut(|d| d.insert_temp(id, t.clone()));
        t
    });
    // Texel centres: an exact linear ramp from one edge to the other.
    let uv = if horizontal { Rect::from_min_max(pos2(0.25, 0.0), pos2(0.75, 1.0)) } else { Rect::from_min_max(pos2(0.0, 0.25), pos2(1.0, 0.75)) };
    Shape::Rect(RectShape::filled(rect, radius, color).with_texture(tex.id(), uv))
}

/// A row's wash: the aura from the row's left edge fading out at 75 % of its width, over its
/// hover or selected fill (`radius`: the row's corners, kept on the left).
pub fn row_wash(ctx: &egui::Context, bg: Rect, radius: u8, color: Color32, alpha: f32) -> Shape {
    let rect = bg.with_max_x(bg.left() + bg.width() * 0.75);
    ramp(ctx, rect, CornerRadius { nw: radius, sw: radius, ne: 0, se: 0 }, color.gamma_multiply(alpha), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_bounded_and_purple_is_calmed() {
        let v = |c: Color32| HsvaGamma::from(Hsva::from(c));
        assert_eq!(normalize(Color32::from_gray(128)), None, "grey has no aura");
        let orange = normalize(Color32::from_rgb(255, 140, 0)).unwrap();
        assert!((0.37..=0.73).contains(&v(orange).v) && v(orange).s <= 0.73, "{orange:?}");
        let dark_blue = normalize(Color32::from_rgb(10, 20, 60)).unwrap();
        assert!(v(dark_blue).v >= 0.37, "lifted: {dark_blue:?}");
        let purple = normalize(Color32::from_rgb(0xA4, 0x2B, 0xEA)).unwrap();
        assert!(v(purple).s <= 0.19, "the accent's hue is calmed: {purple:?}");
        let teal = normalize(Color32::from_rgb(0, 160, 150)).unwrap();
        assert!(v(teal).s > 0.5, "other hues keep their colour");
    }

    #[test]
    fn the_dominant_colour_wins_over_background_and_mixes() {
        // A red disc on white: the colour is red, not pink.
        let mut img = image::RgbaImage::from_pixel(32, 32, image::Rgba([255, 255, 255, 255]));
        for (x, y, p) in img.enumerate_pixels_mut() {
            if (x as i32 - 16).pow(2) + (y as i32 - 16).pow(2) < 100 {
                *p = image::Rgba([200, 20, 20, 255]);
            }
        }
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let [r, g, b] = average(&png).unwrap();
        assert!(r > 180 && g < 60 && b < 60, "{r} {g} {b}");
        // Two colours side by side: the larger one, not their muddy mean.
        let img = image::RgbaImage::from_fn(32, 32, |x, _| if x < 20 { image::Rgba([20, 60, 220, 255]) } else { image::Rgba([230, 140, 20, 255]) });
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let [r, g, b] = average(&png).unwrap();
        assert!(b > 180 && r < 60, "blue wins: {r} {g} {b}");
        assert_eq!(average(b"not a picture"), None);
    }

    #[test]
    fn theme_pair_follows_mode_and_strength() {
        let c = Color32::from_rgb(40, 90, 180);
        let (top, bottom) = theme_pair(c, PeopleColors::Bold, Mode::Dark);
        assert_eq!(top, 0x285AB4);
        assert!(super::super::cosmetics::theme_dark(top) && super::super::cosmetics::theme_dark(bottom));
        let bright = normalize(Color32::from_rgb(120, 230, 240)).unwrap();
        assert!(super::super::cosmetics::theme_dark(theme_pair(bright, PeopleColors::Bold, Mode::Dark).0), "a bright colour still makes a dark card");
        let (light, _) = theme_pair(c, PeopleColors::Bold, Mode::Light);
        assert!(!super::super::cosmetics::theme_dark(light), "a light card in a light theme");
        let (subtle, _) = theme_pair(c, PeopleColors::Subtle, Mode::Dark);
        assert_ne!(subtle, top);
        assert_eq!(alpha(PeopleColors::Subtle, Mode::Dark), alpha(PeopleColors::Bold, Mode::Dark) / 2.0);
    }
}
