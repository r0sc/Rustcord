//! Profile cosmetics display (docs/COSMETICS_NOTES.md), shared by the chat author line, member
//! list, DM list, user panel and the profile card / full profile: guild tag chip, nameplate row
//! background, display-name styles, profile theme gradient, profile effect and frame overlays,
//! badges, connections and the game collection (the avatar decoration is
//! `images::decoration_at`).
//!
//! Others' cosmetics are Discord data; without Nitro mine may be my local picks of Discord SKUs
//! (`overrides::effective_for`, `overrides::my_cosmetics`: client-only, never sent). Images come only from the Discord CDN
//! (`api::cdn` builders, the loader's host allow-list); animated ones follow
//! `Settings::animate_others` (always / while hovered), effect and frame textures are freed with
//! their profile (`release`). Effect / frame configs and games are fetched once per SKU / game
//! when a profile shows them (`AppState::request_product`, `request_games`).
//! OWNERSHIP: cosmetics-display engineer.

use super::images::ImageCache;
use super::messages::Pal;
use super::{UiCx, bold, one_line, rgb};
use crate::api::cdn;
use crate::backend::{Command, SocialCommand};
use crate::config::AnimateMode;
use crate::model::*;
use egui::epaint::{RectShape, TextShape};
use egui::load::SizedTexture;
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Color32, CornerRadius, FontFamily, FontId, Galley, Pos2, Rect, Sense, Shape, Vec2, pos2, vec2};
use std::sync::Arc;
use std::time::Duration;

/// Whether others' animated cosmetics play now (`hovered`: the thing they sit on).
pub fn animates(cx: &UiCx<'_>, hovered: bool) -> bool {
    cx.settings.animate_others == AnimateMode::Always || hovered
}

/// A texture filling `rect` (rounded `radius`, tinted) as a shape, for clipped painters / slots.
fn tex_shape(tex: SizedTexture, rect: Rect, radius: impl Into<CornerRadius>, tint: Color32) -> Shape {
    Shape::Rect(RectShape::filled(rect, radius, tint).with_texture(tex.id, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))))
}

fn px(ctx: &egui::Context, side: f32) -> u32 {
    (side * ctx.pixels_per_point()).ceil().max(1.0) as u32
}

// ---------------------------------------------------------------------------
// Guild tag (§2)
// ---------------------------------------------------------------------------

/// Automod-quarantine member flags (name, bio, guild tag) hide the tag in that guild.
const QUARANTINED: u64 = (1 << 7) | (1 << 8) | (1 << 10);
const CHIP_PAD: f32 = 4.0;

/// The guild tag next to `user`'s name (`member`: theirs in the guild shown, if any).
pub fn guild_tag<'a>(user: &'a User, member: Option<&Member>) -> Option<&'a PrimaryGuild> {
    let pg = user.primary_guild.as_deref().or(user.clan.as_deref())?;
    let shown = pg.identity_enabled == Some(true) && pg.identity_guild_id.is_some() && pg.tag.as_deref().is_some_and(|t| !t.trim().is_empty());
    (shown && member.is_none_or(|m| m.flags & QUARANTINED == 0)).then_some(pg)
}

/// The chip's tooltip and accessible name: "View {server}" when I am in it, else "View server".
fn tag_tooltip(state: &crate::state::AppState, pg: &PrimaryGuild) -> String {
    match pg.identity_guild_id.and_then(|g| state.guild(g)) {
        Some(g) => format!("View {}", g.name),
        None => "View server".into(),
    }
}

/// The guild tag chip as a button (author line, profile card, full profile): `tag_chip` in a
/// `row_h` tall slot with its tooltip, pointing hand and focus ring. A click (Enter, Space)
/// toggles the server tag card (`guild_card`), asking for the server's profile as it opens.
pub fn tag_button(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pg: &PrimaryGuild, size: f32, row_h: f32, text: Color32) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(tag_chip_width(ui, pg, size), row_h), Sense::click());
    let chip = tag_chip(ui, cx.images, pg, r.left_center(), size, text);
    if resp.has_focus() {
        super::messages::focus_ring(ui, &Pal::new(cx.theme), chip, 4.0);
    }
    let tip = tag_tooltip(cx.state, pg);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &tip));
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    let resp = if open { resp } else { resp.on_hover_text(&tip) }.on_hover_cursor(egui::CursorIcon::PointingHand);
    // The demo hook opens the chip on a profile surface (card or modal: Foreground), not the chat's.
    let toggle = resp.clicked() || (ui.layer_id().order == egui::Order::Foreground && super::guild_card::demo_open());
    if let Some(g) = pg.identity_guild_id.filter(|_| toggle && !open) {
        super::guild_card::fetch(cx, g);
    }
    egui::Popup::from_response(&resp)
        .open_memory(toggle.then_some(egui::SetOpenCommand::Toggle))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .width(super::guild_card::WIDTH)
        // Beside the tag where there is room (Discord's spot; the profile popout holding the tag stays
        // readable above and below it), else the other side, else under / over it.
        .align(egui::RectAlign::RIGHT_START)
        .align_alternatives(&[egui::RectAlign::LEFT_START, egui::RectAlign::RIGHT_END, egui::RectAlign::LEFT_END, egui::RectAlign::BOTTOM_START, egui::RectAlign::TOP_START])
        .gap(8.0)
        .frame(egui::Frame::popup(ui.style()).inner_margin(0))
        .show(|ui| super::guild_card::show(ui, cx, pg));
    resp
}

fn chip_parts(ui: &egui::Ui, pg: &PrimaryGuild, size: f32, color: Color32) -> (Arc<Galley>, Option<String>, f32) {
    let text = (size - 2.0).max(9.0);
    let g = ui.painter().layout_no_wrap(pg.tag.clone().unwrap_or_default(), bold(text), color);
    let side = text.round();
    let badge = pg.identity_guild_id.zip(pg.badge.as_deref()).and_then(|(id, b)| cdn::guild_tag_badge_url(id, b, px(ui.ctx(), side)));
    (g, badge, side)
}

/// Width of `tag_chip` next to a name of text size `size`.
pub fn tag_chip_width(ui: &egui::Ui, pg: &PrimaryGuild, size: f32) -> f32 {
    let (g, badge, side) = chip_parts(ui, pg, size, Color32::WHITE);
    2.0 * CHIP_PAD + g.size().x + badge.map_or(0.0, |_| side + 3.0)
}

/// The guild tag chip (badge + tag on a faint pill) with its left edge at `left_center`.
pub fn tag_chip(ui: &egui::Ui, images: &mut ImageCache, pg: &PrimaryGuild, left_center: Pos2, size: f32, text: Color32) -> Rect {
    let w = tag_chip_width(ui, pg, size);
    let (g, badge, side) = chip_parts(ui, pg, size, text);
    let rect = Rect::from_min_size(pos2(left_center.x, left_center.y - (g.size().y + 2.0) / 2.0), vec2(w, g.size().y + 2.0));
    let p = ui.painter();
    p.rect_filled(rect, 4, text.gamma_multiply(0.12));
    let mut x = rect.left() + CHIP_PAD;
    if let Some(url) = badge {
        let r = Rect::from_center_size(pos2(x + side / 2.0, rect.center().y), Vec2::splat(side));
        if let Some(tex) = images.get(ui.ctx(), &url, px(ui.ctx(), side), false) {
            p.add(tex_shape(tex, r, 0, Color32::WHITE));
        }
        x += side + 3.0;
    }
    p.galley(pos2(x, rect.center().y - g.size().y / 2.0), g, text);
    rect
}

/// `tag_chip` on an opaque `surface` pill: rows with a nameplate behind them.
pub fn tag_chip_on(ui: &egui::Ui, images: &mut ImageCache, pg: &PrimaryGuild, left_center: Pos2, size: f32, text: Color32, surface: Color32) -> Rect {
    let slot = ui.painter().add(Shape::Noop);
    let rect = tag_chip(ui, images, pg, left_center, size, text);
    ui.painter().set(slot, Shape::rect_filled(rect, 4, surface));
    rect
}

// ---------------------------------------------------------------------------
// Nameplate (§4)
// ---------------------------------------------------------------------------

/// Narrowest nameplate row (pt, ≈ 200 px at 125 %) that still shows the art.
const PLATE_ART_MIN: f32 = 160.0;

/// Nameplate palette colour (dark / light theme); None = no gradient (`none`, unknown keys).
fn palette(key: &str, dark: bool) -> Option<Color32> {
    let (d, l) = match key {
        "crimson" => (0x900007, 0xE7040F),
        "berry" => (0x893A99, 0xB11FCF),
        "sky" => (0x0080B7, 0x56CCFF),
        "teal" => (0x086460, 0x7DEED7),
        "forest" => (0x2D5401, 0x6AA624),
        "bubble_gum" => (0xDC3E97, 0xF957B3),
        "violet" => (0x730BC8, 0x972FED),
        "cobalt" => (0x0131C2, 0x4278FF),
        "clover" => (0x047B20, 0x63CD5A),
        "lemon" => (0xF6CD12, 0xFED400),
        "white" => (0xFFFFFF, 0xFFFFFF),
        "black" => (0x000000, 0x000000),
        _ => return None,
    };
    Some(rgb(if dark { d } else { l }))
}

/// A nameplate behind a list row (`row`: its hover rect, `radius` its corners), as a shape for
/// a slot under the row's content: the palette gradient (clear → 30 % at the right) and the 16:3
/// image right-aligned at row height, animated while `animate`. `dim` (offline / muted rows): the
/// image at 10 %, no gradient. Rows narrower than `PLATE_ART_MIN` get only the gradient. Light
/// themes: the art fades in only after the row's text (`text_end`), dark art under dark text
/// being unreadable (dark themes keep the whole art: their text is light).
#[allow(clippy::too_many_arguments, reason = "the row's geometry and state")]
pub fn nameplate(ui: &egui::Ui, images: &mut ImageCache, row: Rect, np: &Nameplate, animate: bool, dim: bool, radius: u8, text_end: f32) -> Shape {
    let dark = ui.visuals().dark_mode;
    let mut out = Vec::new();
    if let Some(c) = np.palette.as_deref().and_then(|k| palette(k, dark)).filter(|_| !dim) {
        let strong = Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 0x4d);
        out.push(h_gradient(row, CornerRadius::same(radius), Color32::TRANSPARENT, strong).into());
    }
    // A narrow row (the user panel in a small window): the art would run over the name / status.
    let url = cdn::nameplate_url(&np.asset, animate).filter(|_| row.width() >= PLATE_ART_MIN);
    let r = Rect::from_min_max(pos2(row.right() - row.height() * 16.0 / 3.0, row.top()), row.max);
    if let Some(tex) = url.and_then(|url| images.get(ui.ctx(), &url, px(ui.ctx(), r.width()), animate)) {
        // Cut to the row by UV (the shape goes into the caller's slot, under its clip rect).
        let vis = r.intersect(row);
        let uv = Rect::from_min_max(pos2((vis.left() - r.left()) / r.width(), 0.0), pos2(1.0, 1.0));
        let corners = if r.left() <= row.left() { CornerRadius::same(radius) } else { CornerRadius { ne: radius, se: radius, nw: 0, sw: 0 } };
        out.push(match (dark, dim) {
            (false, false) => faded_tex(tex, vis, uv, corners, text_end, 1.0),
            _ => Shape::Rect(RectShape::filled(vis, corners, Color32::WHITE.gamma_multiply(if dim { 0.1 } else { 1.0 })).with_texture(tex.id, uv)),
        });
    }
    Shape::Vec(out)
}

/// `tex` (its `uv` part) over `r`, fading in after `text_end`: clear up to 8 px past it, `peak`
/// opacity 40 px later (light themes: the name and status stay on the plain row / panel colour,
/// at their normal contrast). One strip mesh (a second shape would leave a seam where they
/// meet), its columns following the right `corners`; the left ones are clear anyway.
fn faded_tex(tex: SizedTexture, r: Rect, uv: Rect, corners: CornerRadius, text_end: f32, peak: f32) -> Shape {
    let (ne, se) = (f32::from(corners.ne).min(r.height() / 2.0), f32::from(corners.se).min(r.height() / 2.0));
    let x1 = (text_end + 48.0).clamp(r.left(), r.right());
    let x0 = (text_end + 8.0).clamp(r.left(), x1);
    let u = |x: f32| uv.left() + uv.width() * (x - r.left()) / r.width().max(1.0);
    let alpha = |x: f32| if x1 > x0 { peak * egui::remap_clamp(x, x0..=x1, 0.0..=1.0) } else { peak };
    // How far a corner of radius `rad` cuts into the edge at `x`.
    let inset = |x: f32, rad: f32| {
        let d = x - (r.right() - rad);
        if d > 0.0 { rad - (rad * rad - d * d).max(0.0).sqrt() } else { 0.0 }
    };
    let arc = |rad: f32| (0..=8).map(move |i| r.right() - rad + rad * (i as f32 * std::f32::consts::FRAC_PI_2 / 8.0).sin());
    let mut xs: Vec<f32> = [r.left(), x0, x1, r.right()].into_iter().chain(arc(ne)).chain(arc(se)).filter(|x| r.x_range().contains(*x)).collect();
    xs.sort_by(f32::total_cmp);
    xs.dedup();
    let mut mesh = egui::Mesh::with_texture(tex.id);
    for (i, &x) in xs.iter().enumerate() {
        let color = Color32::WHITE.gamma_multiply(alpha(x));
        for y in [r.top() + inset(x, ne), r.bottom() - inset(x, se)] {
            let v = uv.top() + uv.height() * (y - r.top()) / r.height().max(1.0);
            mesh.vertices.push(egui::epaint::Vertex { pos: pos2(x, y), uv: pos2(u(x), v), color });
        }
        if i > 0 {
            let k = 2 * i as u32;
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k + 1, k);
        }
    }
    Shape::mesh(mesh)
}

/// Discord's scrim between a row's nameplate and its text: `bg` (the row's colour) at
/// 70 % (`strong`, High Contrast: opaque) under the text, up to 8 px past its end (`text_end`),
/// clear 40 px later (at least over the left 60 %), so lit spots of the art never sit under the
/// status line. One strip mesh over `row` with `corners` (one radius, or per corner: the user
/// panel under the voice section).
pub fn plate_scrim(row: Rect, corners: impl Into<CornerRadius>, bg: Color32, text_end: f32, strong: bool) -> Shape {
    let x0 = text_end + 8.0;
    let x1 = (x0 + 40.0).max(row.left() + row.width() * 0.6);
    let peak = if strong { 1.0 } else { 0.7 };
    scrim(row, corners.into(), bg, [x0, x1], |x| peak * (1.0 - egui::remap_clamp(x, x0..=x1.max(x0 + 1.0), 0.0..=1.0)))
}

/// The scrim behind the user panel's buttons on my nameplate (`row`: the panel, `buttons_left`:
/// the leftmost button's left edge): `bg` (the panel colour) at 50 % (`strong`, High Contrast:
/// opaque) from 8 px before the buttons to the right edge, faded in over the 24 px before, so
/// their glyphs keep their contrast on bright art.
pub fn buttons_scrim(row: Rect, corners: impl Into<CornerRadius>, bg: Color32, buttons_left: f32, strong: bool) -> Shape {
    let (x0, x1) = (buttons_left - 32.0, buttons_left - 8.0);
    let peak = if strong { 1.0 } else { 0.5 };
    scrim(row, corners.into(), bg, [x0, x1], |x| peak * egui::remap_clamp(x, x0..=x1, 0.0..=1.0))
}

/// `bg` at `alpha(x)` over `row` with corners `c`: one strip mesh, columns at the `stops` and
/// along the corners.
fn scrim(row: Rect, c: CornerRadius, bg: Color32, stops: [f32; 2], alpha: impl Fn(f32) -> f32) -> Shape {
    let rad = |v: u8| f32::from(v).min(row.height() / 2.0);
    // How far a corner of radius `r` cuts into the edge `d` px inside its quarter.
    let cut = |r: f32, d: f32| if d > 0.0 { r - (r * r - d * d).max(0.0).sqrt() } else { 0.0 };
    let inset = |x: f32, left: u8, right: u8| cut(rad(left), row.left() + rad(left) - x).max(cut(rad(right), x - (row.right() - rad(right))));
    let arc = |v: u8| (0..=8).map(move |i| rad(v) * (1.0 - (i as f32 * std::f32::consts::FRAC_PI_2 / 8.0).cos()));
    let lefts = arc(c.nw).chain(arc(c.sw)).map(|d| row.left() + d);
    let rights = arc(c.ne).chain(arc(c.se)).map(|d| row.right() - d);
    let mut xs: Vec<f32> = lefts.chain(rights).chain(stops).filter(|x| row.x_range().contains(*x)).collect();
    xs.sort_by(f32::total_cmp);
    xs.dedup();
    let mut mesh = egui::Mesh::default();
    for (i, &x) in xs.iter().enumerate() {
        let color = bg.gamma_multiply(alpha(x));
        mesh.colored_vertex(pos2(x, row.top() + inset(x, c.nw, c.ne)), color);
        mesh.colored_vertex(pos2(x, row.bottom() - inset(x, c.sw, c.se)), color);
        if i > 0 {
            let k = 2 * i as u32;
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k + 1, k);
        }
    }
    Shape::mesh(mesh)
}

/// `left` → `right` across `rect` with `radius` corners: a fan over the rounded outline (convex),
/// so both ends follow the corners.
fn h_gradient(rect: Rect, radius: CornerRadius, left: Color32, right: Color32) -> egui::Mesh {
    let mut pts = Vec::new();
    egui::epaint::tessellator::path::rounded_rectangle(&mut pts, egui::epaint::RoundedRect::new(rect, radius));
    let at = |q: Pos2| left.lerp_to_gamma(right, egui::remap_clamp(q.x, rect.x_range(), 0.0..=1.0));
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.center(), at(rect.center()));
    for (i, &q) in pts.iter().enumerate() {
        mesh.colored_vertex(q, at(q));
        mesh.add_triangle(0, 1 + i as u32, 1 + ((i + 1) % pts.len()) as u32);
    }
    mesh
}

/// My nameplate as the background of the whole account row (`rect`, `radius` corners), like
/// Discord's, as a shape for a slot over the panel's fill: the 16:3 art cover-cropped edge to
/// edge, animated while `animate`, under a light veil of the panel colour `veil` (20 %) and two
/// scrims that only protect the text: behind the name and status (80 %, clear a little after
/// `text_end`) and behind the buttons (`buttons_scrim`, drawn by the caller). The rest of the art
/// stays as it is, in dark and light themes alike. Noop while the art is not loaded (plain fill).
#[allow(clippy::too_many_arguments, reason = "the panel's geometry and state")]
pub fn nameplate_cover(ui: &egui::Ui, images: &mut ImageCache, rect: Rect, np: &Nameplate, animate: bool, radius: CornerRadius, veil: Color32, text_end: f32) -> Shape {
    let Some(url) = cdn::nameplate_url(&np.asset, animate) else { return Shape::Noop };
    let Some(tex) = images.get(ui.ctx(), &url, px(ui.ctx(), rect.width()), animate) else { return Shape::Noop };
    let uv = super::profile::cover_uv(tex.size, rect.size());
    let art = RectShape::filled(rect, radius, Color32::WHITE).with_texture(tex.id, uv);
    let light = RectShape::filled(rect, radius, veil.gamma_multiply(0.2));
    // Unlike a list row's `plate_scrim` (60 % of its width) only the text's own extent is covered.
    let (x0, x1) = (text_end + 8.0, text_end + 48.0);
    let text = scrim(rect, radius, veil, [x0, x1], |x| 0.8 * (1.0 - egui::remap_clamp(x, x0..=x1, 0.0..=1.0)));
    Shape::Vec(vec![art.into(), light.into(), text])
}

// ---------------------------------------------------------------------------
// Display-name styles (§7)
// ---------------------------------------------------------------------------

/// shortcut: no display-name fonts are bundled; the pixel / futuristic ones (8Bit, Mainframe)
/// use the monospace face, the rest the given one. Bundle OFL fonts to show them properly.
fn name_family(style: Option<&NameStyle>, default: FontFamily) -> FontFamily {
    match style.and_then(|s| s.font_id) {
        Some(8 | 14) => FontFamily::Monospace,
        _ => default,
    }
}

/// The family a display-name font draws in (the picker's labels), None = not bundled: it draws
/// in the UI font, so picking it changes nothing here.
pub fn font_face(font_id: u32) -> Option<FontFamily> {
    match font_id {
        11 => Some(FontFamily::Proportional),
        8 | 14 => Some(FontFamily::Monospace),
        _ => None,
    }
}

fn luminance(c: Color32) -> f32 {
    let f = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
}

fn contrast(a: Color32, b: Color32) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// A style colour lightened (dark mode) / darkened until it reads on the background (≥ 3:1).
fn readable(c: Color32, dark: bool) -> Color32 {
    let (bg, to) = if dark { (rgb(0x121214), Color32::WHITE) } else { (rgb(0xFBFBFB), Color32::BLACK) };
    reach(c, &[bg], to, 3.0)
}

/// `c` moved towards `to` (10 % steps, at most 10) until it has `min` contrast on every `bgs`.
pub(crate) fn reach(mut c: Color32, bgs: &[Color32], to: Color32, min: f32) -> Color32 {
    for _ in 0..10 {
        if bgs.iter().all(|&bg| contrast(c, bg) >= min) {
            break;
        }
        c = c.lerp_to_gamma(to, 0.1);
    }
    c
}

/// `t` (0..=1) along evenly spaced colour stops.
fn sample(colors: &[Color32], t: f32) -> Color32 {
    let n = colors.len();
    if n < 2 {
        return colors.first().copied().unwrap_or(Color32::WHITE);
    }
    let x = t.clamp(0.0, 1.0) * (n - 1) as f32;
    let i = (x.floor() as usize).min(n - 2);
    colors[i].lerp_to_gamma(colors[i + 1], x - i as f32)
}

/// The style's colours, readable on the app background.
fn style_colors(style: Option<&NameStyle>, dark: bool) -> Vec<Color32> {
    style.map(|s| s.colors.iter().take(5).map(|&c| readable(rgb(c), dark)).collect()).unwrap_or_default()
}

/// A display name in its style, truncated at `max_w`: one colour (solid, and the core of neon /
/// toon / pop), a gradient across the letters (gradient, glow, prism), or the colours cycling
/// per letter (gummy). Without a style (or colours) the name is `fallback` (role colour).
pub fn name_galley(ui: &egui::Ui, name: &str, font: FontId, style: Option<&NameStyle>, fallback: Color32, max_w: f32) -> Arc<Galley> {
    let font = FontId::new(font.size, name_family(style, font.family));
    let dark = ui.visuals().dark_mode;
    let colors = style_colors(style, dark);
    let mut job = LayoutJob { wrap: TextWrapping::truncate_at_width(max_w.max(0.0)), break_on_newline: false, ..Default::default() };
    let n = name.chars().count();
    let effect = style.and_then(|s| s.effect_id).unwrap_or(1);
    let per_char = |job: &mut LayoutJob, color: &dyn Fn(usize) -> Color32| {
        for (i, (at, ch)) in name.char_indices().enumerate() {
            crate::theme::append_boxed(job, &name[at..at + ch.len_utf8()], &TextFormat::simple(font.clone(), color(i)));
        }
    };
    match (effect, colors.len()) {
        (_, 0) => crate::theme::append_boxed(&mut job, name, &TextFormat::simple(font, fallback)),
        (2 | 6 | 7, 2..) => per_char(&mut job, &|i| sample(&colors, i as f32 / (n.max(2) - 1) as f32)),
        (8, 2..) => per_char(&mut job, &|i| colors[i % colors.len()]),
        // Neon and pop: a light core over the glow / darker shadow (light themes: neon's core
        // deeper, a light one would vanish on the light background; pop's the colour itself).
        (3 | 5, _) => {
            let core = match (dark, effect) {
                (true, _) => colors[0].lerp_to_gamma(Color32::WHITE, 0.55),
                (false, 3) => colors[0].lerp_to_gamma(Color32::BLACK, 0.35),
                (false, _) => colors[0],
            };
            crate::theme::append_boxed(&mut job, name, &TextFormat::simple(font, core));
        }
        _ => crate::theme::append_boxed(&mut job, name, &TextFormat::simple(font, colors[0])),
    }
    ui.painter().layout_job(job)
}

/// What neon (soft glow), toon (dark outline) and pop (drop shadow) draw under the name: paint
/// it (or `set` a slot) before the name galley at `pos`; `dark`: the mode of what it sits on.
/// `Noop` for the other styles.
pub fn name_underlay(pos: Pos2, galley: &Arc<Galley>, style: Option<&NameStyle>, dark: bool) -> Shape {
    let Some(&c) = style.and_then(|s| s.colors.first()) else { return Shape::Noop };
    let c = rgb(c);
    let (offsets, color): (Vec<Vec2>, Color32) = match style.and_then(|s| s.effect_id) {
        // Neon: a soft glow, faint copies on two rings (1.5 and 3 px) that fade outwards. Light
        // backgrounds: the inner ring at half strength (Discord's lighter shadow there); the full
        // glow reads as a highlighter box.
        Some(3) => {
            let ring = |n: usize, r: f32| (0..n).map(move |i| Vec2::angled(i as f32 * std::f32::consts::TAU / n as f32) * r);
            if dark { (ring(8, 1.5).chain(ring(12, 3.0)).collect(), c.gamma_multiply(0.08)) } else { (ring(8, 1.5).collect(), c.gamma_multiply(0.04)) }
        }
        // Toon: a dark outline; light backgrounds: a light one (a dark one muddies the name).
        Some(4) => {
            let outline = if dark { Color32::from_black_alpha(200) } else { Color32::from_white_alpha(170) };
            ([(-1.0, -1.0), (0.0, -1.0), (1.0, -1.0), (-1.0, 0.0), (1.0, 0.0), (-1.0, 1.0), (0.0, 1.0), (1.0, 1.0)].map(Vec2::from).to_vec(), outline)
        }
        // Pop: one crisp shadow, the colour 40 % darker; light backgrounds: the colour at 30 %.
        Some(5) => (vec![vec2(1.5, 1.5)], if dark { c.lerp_to_gamma(Color32::BLACK, 0.4) } else { c.gamma_multiply(0.3) }),
        _ => return Shape::Noop,
    };
    let copy = |&d: &Vec2| Shape::Text(TextShape::new(pos + d, galley.clone(), color).with_override_text_color(color));
    Shape::Vec(offsets.iter().map(copy).collect())
}

/// `name_galley` + `name_underlay` painted at `pos` (layouts that paint the name themselves).
pub fn paint_name(painter: &egui::Painter, pos: Pos2, galley: Arc<Galley>, style: Option<&NameStyle>, fallback: Color32, dark: bool) {
    painter.add(name_underlay(pos, &galley, style, dark));
    painter.galley(pos, galley, fallback);
}

// ---------------------------------------------------------------------------
// Profile theme (§8)
// ---------------------------------------------------------------------------

/// Profile theme `(primary, accent)`: my local pick for me, else the loaded profile's.
pub fn theme_colors(user_id: Id, profile: Option<&UserProfile>) -> Option<(u32, u32)> {
    let local = crate::overrides::my_cosmetics(user_id).and_then(|c| c.theme_colors);
    local.map(|[a, b]| (a, b)).or_else(|| profile.and_then(|p| p.theme_colors()))
}

fn veil(dark: bool) -> Color32 {
    if dark { Color32::BLACK } else { Color32::WHITE }
}

/// A themed card's mode (§8), whatever the app's: dark when darkness(primary) > 0.5.
pub fn theme_dark(primary: u32) -> bool {
    let c = rgb(primary);
    (0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32) / 255.0 < 0.5
}

/// The colours inside a themed card, in its own mode (`theme_dark`): text tinted 20 % towards
/// the primary (§8 textMixAmount 25), pushed until it reaches 7:1 (muted 4.5:1) on both ends of
/// the veiled gradient; `surface` the body's top colour, `bg` (role pills, boxes) the veil at 45 %.
pub(super) fn theme_pal(pal: &Pal, (primary, accent): (u32, u32)) -> Pal {
    let dark = theme_dark(primary);
    let body = [primary, accent].map(|c| rgb(c).lerp_to_gamma(veil(dark), 0.6));
    let (text, muted, to) = if dark { (0xF2F3F5, 0xB5BAC1, Color32::WHITE) } else { (0x060607, 0x4E5058, Color32::BLACK) };
    let tinted = |c: u32| rgb(c).lerp_to_gamma(rgb(primary), 0.2);
    Pal {
        text: reach(tinted(text), &body, to, 7.0),
        muted: reach(tinted(muted), &body, to, 4.5),
        surface: body[0],
        bg: veil(dark).gamma_multiply(0.45),
        dark,
        ..*pal
    }
}

/// A themed card's widgets (labels, separators, text fields, buttons) in its colours.
pub(super) fn theme_visuals(ui: &mut egui::Ui, pal: &Pal) {
    let v = ui.visuals_mut();
    v.dark_mode = pal.dark;
    v.weak_text_color = Some(pal.muted);
    let w = &mut v.widgets;
    w.noninteractive.fg_stroke.color = pal.text;
    w.noninteractive.bg_stroke.color = pal.line();
    for (s, a) in [(&mut w.inactive, 0.08), (&mut w.hovered, 0.13), (&mut w.open, 0.13)] {
        s.fg_stroke.color = pal.text;
        (s.bg_fill, s.weak_bg_fill) = (pal.text.gamma_multiply(a), pal.text.gamma_multiply(a));
    }
}

/// The overlay `theme_shapes` lays over a themed card's body (an uploaded banner fades into it).
pub fn theme_veil(dark: bool) -> Color32 {
    veil(dark).gamma_multiply(0.6)
}

/// The card colour under a themed profile's avatar (its ring, the status-dot cut-out).
pub fn theme_ring(primary: u32, dark: bool) -> Color32 {
    rgb(primary).lerp_to_gamma(veil(dark), 0.6)
}

/// The theme gradient (primary at the top → accent at the bottom) over `card` with `radius`
/// corners, and the body overlay below `body_top` (black 60 % in a dark card, white in a light
/// one: `theme_pal`'s mode). `salt`: one gradient texture per surface.
pub fn theme_shapes(ctx: &egui::Context, salt: &str, card: Rect, radius: u8, (primary, accent): (u32, u32), body_top: f32, dark: bool) -> Shape {
    let id = egui::Id::unique(("rc_theme_tex", salt));
    let cached = ctx.data(|d| d.get_temp::<((u32, u32), egui::TextureHandle)>(id)).filter(|(k, _)| *k == (primary, accent));
    let tex = cached.map(|(_, t)| t).unwrap_or_else(|| {
        let img = egui::ColorImage::new([1, 2], vec![rgb(primary), rgb(accent)]);
        let t = ctx.load_texture(format!("rc-theme-{salt}"), img, egui::TextureOptions::LINEAR);
        ctx.data_mut(|d| d.insert_temp(id, ((primary, accent), t.clone())));
        t
    });
    // Texel centres of the 1×2 texture: an exact linear gradient top to bottom.
    let uv = Rect::from_min_max(pos2(0.0, 0.25), pos2(1.0, 0.75));
    let gradient = Shape::Rect(RectShape::filled(card, radius, Color32::WHITE).with_texture(tex.id(), uv));
    // On the pixel grid like the banner fade above it (`profile::paint_banner`).
    let body_top = egui::emath::GuiRounding::round_to_pixels(body_top, ctx.pixels_per_point());
    let body = Rect::from_min_max(pos2(card.left(), body_top.clamp(card.top(), card.bottom())), card.max);
    let overlay = Shape::rect_filled(body, CornerRadius { nw: 0, ne: 0, sw: radius, se: radius }, theme_veil(dark));
    Shape::Vec(vec![gradient, overlay])
}

/// A themed card's border like Discord's: primary at the top → accent at the bottom, 3 px inside
/// `card`'s `radius` corners. Painted over the card's content (the banner too).
pub fn theme_outline(card: Rect, radius: u8, (primary, accent): (u32, u32)) -> Shape {
    const W: f32 = 3.0;
    let mut pts = Vec::new();
    let path = egui::epaint::RoundedRect::new(card.shrink(W / 2.0), (radius as f32 - W / 2.0).max(0.0));
    egui::epaint::tessellator::path::rounded_rectangle(&mut pts, path);
    let (a, b) = (rgb(primary), rgb(accent));
    let stroke = egui::epaint::PathStroke::new_uv(W, move |bb, p| a.lerp_to_gamma(b, egui::remap_clamp(p.y, bb.y_range(), 0.0..=1.0)));
    Shape::closed_line(pts, stroke)
}

// ---------------------------------------------------------------------------
// Profile effect (§5) and frame (§6)
// ---------------------------------------------------------------------------

/// SKU of the effect (`kind` 1) or frame (3) a profile shows: my local pick for me (a "none"
/// pick hides mine), else the profile's (guild profile first).
pub fn item_sku(user_id: Id, profile: Option<&UserProfile>, kind: u16) -> Option<Id> {
    let local = crate::overrides::my_cosmetics(user_id).and_then(|c| if kind == 1 { c.effect } else { c.frame });
    match local {
        Some(i) => i.sku_id.filter(|_| !i.is_none()),
        None => profile?.collectible_sku(kind, chrono::Utc::now().timestamp()),
    }
}

/// The product item of `sku` (of `kind`), asking for the product once when it is unknown.
pub fn product_item(cx: &mut UiCx<'_>, sku: Id, kind: u16) -> Option<CollectibleItem> {
    match cx.state.social.products.get(&sku) {
        Some(p) => p.as_ref()?.items.iter().find(|i| i.kind == kind).cloned(),
        None => {
            if cx.state.request_product(sku) {
                cx.cmd.send(Command::Social(SocialCommand::FetchProduct { sku_id: sku }));
            }
            None
        }
    }
}

/// What a shown profile draws around its card: theme colours, profile effect and frame.
pub struct ProfileLook {
    pub theme: Option<(u32, u32)>,
    effect: Option<CollectibleItem>,
    frame: Option<CollectibleItem>,
}

/// The look of `user_id`'s profile (`with_profile`: the loaded profile counts; the account
/// popout shows only my local picks).
pub fn profile_look(cx: &mut UiCx<'_>, user_id: Id, guild_id: Option<Id>, with_profile: bool) -> ProfileLook {
    let profile = super::profile_modal::loaded(cx, user_id, guild_id).filter(|_| with_profile);
    let theme = theme_colors(user_id, profile);
    let (effect, frame) = (item_sku(user_id, profile, 1), item_sku(user_id, profile, 3));
    ProfileLook { theme, effect: effect.and_then(|s| product_item(cx, s, 1)), frame: frame.and_then(|s| product_item(cx, s, 3)) }
}

impl ProfileLook {
    /// How far the frame's top staples stand above a card `card_w` wide (§6), so the caller
    /// keeps that room under the title bar.
    pub fn overflow_top(&self, card_w: f32) -> f32 {
        let Some(f) = &self.frame else { return 0.0 };
        f.overflow_top.unwrap_or(0.0) * card_w / f.inner_width.filter(|w| *w > 0.0).unwrap_or(card_w)
    }

    /// After the card (`card`: its rect, `radius` its corners) is drawn: the back frame layers
    /// into `behind` (a slot of the layer painter added before the card), the front ones and the
    /// effect (`t`: seconds shown) over it.
    #[allow(clippy::too_many_arguments, reason = "the card's geometry and clock")]
    pub fn finish(&self, ui: &egui::Ui, images: &mut ImageCache, card: Rect, radius: u8, behind: egui::layers::ShapeIdx, t: f64, animate: bool) {
        let mut lp = ui.ctx().layer_painter(ui.layer_id());
        lp.set_opacity(ui.opacity()); // a fresh painter would ignore the card's (and modal's) fade
        if let Some(f) = &self.frame {
            lp.set(behind, Shape::Vec(frame_shapes(ui, images, card, f, true)));
            for s in frame_shapes(ui, images, card, f, false) {
                lp.add(s);
            }
        }
        if let Some(e) = &self.effect {
            paint_effect(ui, images, card, radius, e, t, animate);
        }
    }
}

/// Seconds a surface (`salt`, e.g. the card and its user) has been shown without a break.
pub fn shown_for(ctx: &egui::Context, salt: impl std::hash::Hash + std::fmt::Debug) -> f64 {
    let id = egui::Id::unique(("rc_fx_clock", salt));
    let (now, pass) = (ctx.input(|i| i.time), ctx.cumulative_pass_nr());
    let start = match ctx.data(|d| d.get_temp::<(f64, u64)>(id)) {
        Some((start, last)) if last + 1 >= pass => start,
        _ => now,
    };
    ctx.data_mut(|d| d.insert_temp(id, (start, pass)));
    now - start
}

fn urls_id() -> egui::Id {
    egui::Id::unique("rc_cosmetic_profile_urls")
}

/// A large effect / frame image for `rect` at its drawn size (≤ 768 px; effect layers stream at their own rate), remembered for `release`.
fn big_image(ui: &egui::Ui, images: &mut ImageCache, url: &str, rect: Rect, animate: bool) -> Option<SizedTexture> {
    ui.ctx().data_mut(|d| {
        let list = d.get_temp_mut_or_default::<Vec<String>>(urls_id());
        if !list.iter().any(|u| u == url) {
            list.push(url.to_owned());
        }
    });
    let side = (rect.width().max(rect.height()) * ui.ctx().pixels_per_point()).min(768.0);
    images.get(ui.ctx(), url, side.max(16.0) as u32, animate)
}

/// Frees the effect / frame textures of a closed profile (card or full profile).
pub fn release(ctx: &egui::Context, images: &mut ImageCache) {
    for url in ctx.data_mut(|d| d.remove_temp::<Vec<String>>(urls_id())).unwrap_or_default() {
        images.remove(&url);
    }
}

/// (shown at `t` ms, ms until that changes) for an effect layer (§5 visibility rule).
fn layer_window(l: &EffectLayer, t: f32) -> (bool, Option<f32>) {
    if t < l.start {
        return (false, Some(l.start - t));
    }
    let d = l.duration.max(1.0);
    if !l.looping {
        let end = l.start + d;
        return if t <= end { (true, Some(end - t)) } else { (false, None) };
    }
    if l.loop_delay <= 0.0 {
        return (true, None);
    }
    let phase = (t - l.start) % (d + l.loop_delay);
    if phase <= d { (true, Some(d - phase)) } else { (false, Some(d + l.loop_delay - phase)) }
}

/// `tex` stretched over `r`, cut to `card` with `radius` corners: a strip mesh whose rows end on
/// the corner arcs (a clip rect would leave square corners over the rounded card).
fn rounded_tex(tex: SizedTexture, r: Rect, card: Rect, radius: f32) -> Shape {
    let clip = r.intersect(card);
    if !clip.is_positive() || !r.is_positive() {
        return Shape::Noop;
    }
    let radius = radius.min(card.width() / 2.0).min(card.height() / 2.0);
    // Rows on the arcs (8 steps a corner) and at the clip's own top and bottom.
    let arc = (0..=8).map(|i| radius * (1.0 - (i as f32 * std::f32::consts::FRAC_PI_2 / 8.0).cos()));
    let mut ys: Vec<f32> = arc.flat_map(|d| [card.top() + d, card.bottom() - d]).chain([clip.top(), clip.bottom()]).filter(|y| clip.y_range().contains(*y)).collect();
    ys.sort_by(f32::total_cmp);
    ys.dedup();
    let mut mesh = egui::Mesh::with_texture(tex.id);
    for (i, &y) in ys.iter().enumerate() {
        let d = radius - (y - card.top()).min(card.bottom() - y);
        let inset = if d > 0.0 { radius - (radius * radius - d * d).max(0.0).sqrt() } else { 0.0 };
        let left = (card.left() + inset).max(clip.left());
        for x in [left, (card.right() - inset).min(clip.right()).max(left)] {
            let uv = pos2((x - r.left()) / r.width(), (y - r.top()) / r.height());
            mesh.vertices.push(egui::epaint::Vertex { pos: pos2(x, y), uv, color: Color32::WHITE });
        }
        if i > 0 {
            let k = 2 * i as u32;
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k + 1, k);
        }
    }
    Shape::mesh(mesh)
}

/// The profile effect over `card` (banner included), cut to its `radius` corners: its layers
/// by z-index, each in its time window `t` s after the card appeared (the clock starts at
/// −0.5 s), scaled from 450 px. Without `animate` only the reduced-motion still.
pub fn paint_effect(ui: &egui::Ui, images: &mut ImageCache, card: Rect, radius: u8, item: &CollectibleItem, t: f64, animate: bool) {
    let p = ui.ctx().layer_painter(ui.layer_id()).with_clip_rect(card.intersect(ui.clip_rect()));
    let radius = f32::from(radius);
    let k = card.width() / 450.0;
    let rect_of = |x: f32, y: f32, w: Option<f32>, h: Option<f32>| {
        let (w, h) = (w.unwrap_or(450.0).max(1.0), h.unwrap_or(880.0).max(1.0));
        Rect::from_min_size(card.min + vec2(x * k, y * k), vec2(w * k, h * k))
    };
    if !animate {
        let still = item.reduced_motion_src.as_deref().or(item.static_frame_src.as_deref()).and_then(cdn::collectible_asset_url);
        let first = item.effects.first();
        let r = rect_of(0.0, 0.0, first.and_then(|l| l.width), first.and_then(|l| l.height));
        if let Some(tex) = still.and_then(|u| big_image(ui, images, &u, r, false)) {
            p.add(rounded_tex(tex, r, card, radius));
        }
        return;
    }
    let t = (t * 1000.0) as f32 - 500.0;
    let mut layers: Vec<&EffectLayer> = item.effects.iter().collect();
    layers.sort_by_key(|l| l.z_index);
    let mut wake: Option<f32> = None;
    for l in layers {
        let (shown, next) = layer_window(l, t);
        if let Some(n) = next {
            wake = Some(wake.map_or(n, |w| w.min(n)));
        }
        let r = rect_of(l.position.x, l.position.y, l.width, l.height);
        if let Some(tex) = cdn::collectible_asset_url(&l.src).filter(|_| shown).and_then(|u| big_image(ui, images, &u, r, true)) {
            p.add(rounded_tex(tex, r, card, radius));
        }
    }
    if let Some(ms) = wake {
        ui.ctx().request_repaint_after(Duration::from_millis(ms.ceil().max(1.0) as u64));
    }
}

/// Profile-frame layers around `card` (§6): the `back` ones (under the card's content) or the
/// front ones, as shapes for the caller's (unclipped) layer painter or slot. Layers span the
/// card plus the horizontal overflow; staples sit on the top / bottom edge, rails run down the
/// card, borders tile it vertically.
pub fn frame_shapes(ui: &egui::Ui, images: &mut ImageCache, card: Rect, item: &CollectibleItem, back: bool) -> Vec<Shape> {
    let k = card.width() / item.inner_width.filter(|w| *w > 0.0).unwrap_or(card.width());
    let (ot, ob, oh) = (item.overflow_top.unwrap_or(0.0) * k, item.overflow_bottom.unwrap_or(0.0) * k, item.overflow_horizontal.unwrap_or(0.0) * k);
    let (left, w) = (card.left() - oh, card.width() + 2.0 * oh);
    let mut out = Vec::new();
    for l in item.layers.iter().filter(|l| (l.order == 1) == back) {
        let Some(url) = cdn::frame_layer_url(item.sku_id, &l.id) else { continue };
        // A layer taller than wide needs its height in pixels, known once loaded (`get` then reloads sharper).
        let tall = images.loaded_size(&url).map_or(w, |s| w * s.y / s.x.max(1.0));
        let Some(tex) = big_image(ui, images, &url, Rect::from_min_size(card.min, vec2(w, tall)), false) else { continue };
        let h = w * tex.size.y / tex.size.x.max(1.0);
        let at = |top: f32, height: f32| Rect::from_min_size(pos2(left, top), vec2(w, height));
        match l.kind {
            // Rail: down the whole card, unless the card is too wide for it (responsive).
            1 if !(l.responsive && card.width() / card.height().max(1.0) >= w / h.max(1.0)) => out.push(tex_shape(tex, at(card.top(), card.height()), 0, Color32::WHITE)),
            1 => {}
            2 => {
                let n = (card.height() / h.max(1.0)).ceil() as usize;
                out.extend((0..n.min(64)).map(|i| tex_shape(tex, at(card.top() + i as f32 * h, h), 0, Color32::WHITE)));
            }
            _ => {
                let top = match l.anchor {
                    1 => card.bottom() + ob - h,
                    2 => card.center().y - h / 2.0,
                    _ => card.top() - ot,
                };
                out.push(tex_shape(tex, at(top, h), 0, Color32::WHITE));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Badges (§9)
// ---------------------------------------------------------------------------

/// `public_flags` bit → (badge id, icon hash, description): the fallback without a profile.
const FLAG_BADGES: [(u32, &str, &str, &str); 12] = [
    (0, "staff", "5e74e9b61934fc1f67c65515d1f7e60d", "Staff"),
    (1, "partner", "3f9748e53446a137a052f3454e2de41e", "Partnered Server Owner"),
    (2, "hypesquad", "bf01d1073931f921909045f3a39fd264", "HypeSquad Events"),
    (3, "bug_hunter_level_1", "2717692c7dca7289b35297368a940dd0", "Bug Hunter"),
    (6, "hypesquad_house_1", "8a88d63823d8a71cd5e390baa45efa02", "HypeSquad Bravery"),
    (7, "hypesquad_house_2", "011940fd013da3f7fb926e4a1cd2e618", "HypeSquad Brilliance"),
    (8, "hypesquad_house_3", "3aa41de486fa12454c3761e8e223442e", "HypeSquad Balance"),
    (9, "early_supporter", "7060786766c9c840eb3019e725d2b358", "Early Supporter"),
    (14, "bug_hunter_level_2", "848f79194d4be5ff5f81505cbd0ce1e6", "Bug Hunter Level 2"),
    (17, "verified_developer", "6df5892e0f35b051f8b61eace34f4967", "Early Verified Bot Developer"),
    (18, "certified_moderator", "fee1624003e2fee35cb398e125dc479b", "Moderator Programs Alumni"),
    (22, "active_developer", "6bdc42827a38498929a4920da12695d9", "Active Developer"),
];

/// Badges `public_flags` implies.
pub fn flag_badges(flags: u64) -> Vec<ProfileBadge> {
    FLAG_BADGES
        .iter()
        .filter(|(bit, ..)| flags & (1 << bit) != 0)
        .map(|&(_, id, icon, description)| ProfileBadge { id: id.into(), description: description.into(), icon: icon.into(), link: None })
        .collect()
}

fn since(iso: Option<&str>) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(iso?).ok().map(|t| t.format("%b %-d, %Y").to_string())
}

/// A profile's badges (+ the guild's in a guild context, the Nitro / boost ones with their
/// dates), or the `public_flags` ones while no profile is loaded.
pub fn badges(user: &User, profile: Option<&UserProfile>) -> Vec<ProfileBadge> {
    let Some(p) = profile else { return flag_badges(user.public_flags) };
    let mut out: Vec<ProfileBadge> = p.badges.iter().chain(&p.guild_badges).cloned().collect();
    for b in &mut out {
        let date = if b.id == "premium" {
            since(p.premium_since.as_deref()).map(|d| format!("Subscriber since {d}"))
        } else if b.id.starts_with("guild_booster_lvl") {
            since(p.premium_guild_since.as_deref()).map(|d| format!("Server boosting since {d}"))
        } else {
            None
        };
        if let Some(d) = date {
            b.description = d;
        }
    }
    out
}

/// What a badge is, in Rustcord's own words: (title, body); None = an id this table doesn't
/// know (its API description is then the whole popover). UNCONFIRMED facts (COSMETICS_NOTES §9):
/// the `premium_tenure_*` ids, what earns `orb_profile_badge`, the booster month steps.
pub fn badge_info(id: &str) -> Option<(String, String)> {
    let fixed = |t: &str, b: &str| Some((t.to_owned(), b.to_owned()));
    let house = "One of the three HypeSquad houses. People joined a house through a short quiz in Discord's settings.";
    if let Some(n) = id.strip_prefix("premium_tenure_").and_then(|r| r.split('_').next()).and_then(|n| n.parse::<u32>().ok()) {
        let months = if n == 1 { "1 month".to_owned() } else { format!("{n} months") };
        return Some((format!("Nitro, {months}"), format!("Has had Nitro for at least {months}. The gem changes as the time grows.")));
    }
    if id.starts_with("guild_booster_lvl") {
        return fixed("Server booster", "Boosts a server. The gem changes as the boost goes on, after 1, 2, 3, 6, 9, 12, 15, 18 and 24 months.");
    }
    match id {
        "staff" => fixed("Discord staff", "Works at Discord."),
        "partner" => fixed("Partnered server owner", "Owns a server in the Discord Partner Program."),
        "certified_moderator" => fixed("Moderator programs alumni", "Took part in Discord's programs for community moderators."),
        "hypesquad" => fixed("HypeSquad Events", "Helped out at Discord community events as part of HypeSquad Events."),
        "hypesquad_house_1" => fixed("HypeSquad Bravery", house),
        "hypesquad_house_2" => fixed("HypeSquad Brilliance", house),
        "hypesquad_house_3" => fixed("HypeSquad Balance", house),
        "bug_hunter_level_1" => fixed("Bug hunter", "Reported bugs to Discord that were confirmed and fixed."),
        "bug_hunter_level_2" => fixed("Gold bug hunter", "One of the most active bug hunters, with many confirmed reports."),
        "active_developer" => fixed("Active developer", "Owns an app whose commands were used in the last 30 days. The badge goes away when the app stops being used."),
        "verified_developer" => fixed("Early verified bot developer", "Had a bot verified by Discord before August 2020, when this badge stopped being given out."),
        "early_supporter" => fixed("Early supporter", "Bought Nitro before October 2018."),
        "premium" => fixed("Nitro", "Subscribes to Nitro. The date shows when the subscription started."),
        "legacy_username" => fixed("Legacy username", "This account is older than Discord's 2023 switch to unique usernames. The old name with its #number is shown above."),
        "quest_completed" => fixed("Quest completed", "Finished a Quest: a task from a game, like playing it for a while, that earns a reward."),
        "orb_profile_badge" => fixed("Orbs", "Bought with Orbs, the reward points from Quests."),
        "bot_commands" => fixed("Supports commands", "This app has commands. Type / in a chat to use them."),
        "automod" => fixed("Uses AutoMod", "This app makes AutoMod rules that filter messages in servers."),
        _ => None,
    }
}

/// A badge popover's (title, subtitle, body): the subtitle is the API description ("Subscriber
/// since Mar 4, 2021") when it says more than the title.
fn badge_text(b: &ProfileBadge) -> (String, Option<&str>, Option<String>) {
    match badge_info(&b.id) {
        Some((title, body)) => {
            let sub = Some(b.description.as_str()).filter(|d| !d.trim().is_empty() && !d.eq_ignore_ascii_case(&title));
            (title, sub, Some(body))
        }
        None => (b.description.clone(), None, None),
    }
}

/// 22 px badge icons on a faint pill: the description as tooltip; a click (or Enter / Space)
/// opens its info popover (`badge_popover`), never a link. Only loaded icons show (no grey
/// placeholder circles, docs/DESIGN.md §12); none loaded yet, no pill.
pub(super) fn badge_row(ui: &mut egui::Ui, images: &mut ImageCache, pal: &Pal, badges: &[ProfileBadge]) {
    let loaded: Vec<(SizedTexture, &ProfileBadge)> = badges.iter().filter_map(|b| Some((cdn::badge_icon_url(&b.icon).and_then(|u| badge_icon(ui, images, &u))?, b))).collect();
    if loaded.is_empty() {
        return;
    }
    egui::Frame::new().fill(pal.tk.hover).corner_radius(pal.radius).inner_margin(egui::Margin::symmetric(4, 3)).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            for (i, (tex, b)) in loaded.into_iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
                ui.painter().add(tex_shape(tex, r, 0, Color32::WHITE));
                if resp.has_focus() {
                    super::messages::focus_ring(ui, pal, r, 4.0);
                }
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &b.description));
                let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
                let resp = if open { resp } else { resp.on_hover_text(&b.description) }.on_hover_cursor(egui::CursorIcon::PointingHand);
                let toggle = resp.clicked() || demo_badge(i);
                egui::Popup::from_response(&resp)
                    .open_memory(toggle.then_some(egui::SetOpenCommand::Toggle))
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .width(280.0)
                    .gap(8.0)
                    .frame(egui::Frame::popup(ui.style()).inner_margin(12))
                    .show(|ui| badge_popover(ui, pal, tex, b));
            }
        });
    });
}

/// The badge's info (280 wide, E2): its icon at 40, the title (heading 16/600), the API
/// description under it (meta, muted), then what it means (ui 14).
fn badge_popover(ui: &mut egui::Ui, pal: &Pal, tex: SizedTexture, b: &ProfileBadge) {
    ui.set_width(256.0);
    let (title, sub, body) = badge_text(b);
    // The app's colours (the popup's), not a themed card's: it floats outside the card.
    let pal = &Pal { text: ui.visuals().text_color(), muted: ui.visuals().weak_text_color(), ..*pal };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        let (r, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
        ui.painter().add(tex_shape(tex, r, 0, Color32::WHITE));
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add(egui::Label::new(egui::RichText::new(&title).font(FontId::new(pal.heading(), crate::theme::semibold_family())).color(pal.text)).wrap());
            if let Some(s) = sub {
                ui.add(egui::Label::new(egui::RichText::new(s).size(pal.meta()).color(pal.muted)).wrap());
            }
        });
    });
    if let Some(body) = body {
        ui.add_space(4.0);
        ui.add(egui::Label::new(egui::RichText::new(body).size(pal.ui()).color(pal.text)).wrap());
    }
}

/// Test hook `RUSTCORD_NAV_DEMO=profile,badge:<i>`: the i-th shown badge's popover opens once.
fn demo_badge(i: usize) -> bool {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let want = super::sidebar::nav_demo_value("badge").and_then(|v| v.parse::<usize>().ok());
    want == Some(i) && !DONE.swap(true, std::sync::atomic::Ordering::Relaxed)
}

/// A badge icon still missing this long after it was first drawn is asked for once more: a
/// failed load would otherwise wait a minute, and for a repaint.
const BADGE_RETRY_SECS: f64 = 6.0;

/// A badge icon at its CDN size (72, so the 40 px popover icon is sharp), retried once
/// (`BADGE_RETRY_SECS`); a load still running is not sent twice (the loader joins it).
fn badge_icon(ui: &egui::Ui, images: &mut ImageCache, url: &str) -> Option<SizedTexture> {
    let side = 72;
    if let Some(tex) = images.get(ui.ctx(), url, side, false) {
        return Some(tex);
    }
    let id = egui::Id::unique(("rc_badge_retry", url));
    let now = ui.input(|i| i.time);
    let (since, retried) = ui.ctx().data_mut(|d| *d.get_temp_mut_or_insert_with(id, || (now, false)));
    if retried {
        return None;
    }
    if now - since < BADGE_RETRY_SECS {
        ui.ctx().request_repaint_after(Duration::from_secs_f64(BADGE_RETRY_SECS - (now - since)));
        return None;
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, (since, true)));
    images.remove(url);
    images.get(ui.ctx(), url, side, false)
}

// ---------------------------------------------------------------------------
// Connections (§10)
// ---------------------------------------------------------------------------

/// `encodeURIComponent`'s set: everything but `A-Z a-z 0-9 - _ . ! ~ * ' ( )`.
const URI_COMPONENT: &percent_encoding::AsciiSet =
    &percent_encoding::NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'!').remove(b'~').remove(b'*').remove(b'\'').remove(b'(').remove(b')');

/// Profile page of a connection; None = the platform has none (a click copies the name).
pub fn connection_url(kind: &str, id: &str, name: &str) -> Option<String> {
    let e = |s: &str| percent_encoding::utf8_percent_encode(s, URI_COMPONENT).to_string();
    let url = match kind {
        "twitch" => format!("https://www.twitch.tv/{}", e(name)),
        "youtube" => format!("https://www.youtube.com/channel/{}", e(id)),
        "bluesky" => format!("https://bsky.app/profile/{}", e(id).replace("%3A", ":")),
        "steam" => format!("https://steamcommunity.com/profiles/{}", e(id)),
        "reddit" => format!("https://www.reddit.com/u/{}", e(name)),
        "twitter" => format!("https://x.com/{}", e(name)),
        "twitter_legacy" => format!("https://twitter.com/{}", e(name)),
        "spotify" => format!("https://open.spotify.com/user/{}", e(id)),
        "github" => format!("https://github.com/{}", e(name)),
        "roblox" => format!("https://roblox.com/users/{}/profile", e(id)),
        "ebay" => format!("https://www.ebay.com/usr/{}", e(name)),
        "tiktok" => format!("https://www.tiktok.com/@{}", e(name)),
        "instagram" => format!("https://www.instagram.com/{}", e(name)),
        "letterboxd" => format!("https://letterboxd.com/{}/", e(name)),
        "mastodon" => id.to_owned(),
        "domain" => format!("https://{}/", e(id)),
        _ => return None,
    };
    // Only real https pages (a mastodon id is a URL already).
    reqwest::Url::parse(&url).ok().filter(|u| u.scheme() == "https" && u.host_str().is_some()).map(|_| url)
}

/// Types without a platform entry (dropped from lists).
pub fn shown_connection(kind: &str) -> bool {
    !matches!(kind, "soundcloud" | "playstation-stg" | "contacts")
}

/// Platform name and colour (painted icon; no third-party image hosts).
pub fn platform(kind: &str) -> (String, u32) {
    let (name, color) = match kind {
        "github" => ("GitHub", 0x24292F),
        "twitch" => ("Twitch", 0x9146FF),
        "youtube" => ("YouTube", 0xFF0000),
        "steam" => ("Steam", 0x1B2838),
        "spotify" => ("Spotify", 0x1DB954),
        "xbox" => ("Xbox", 0x107C10),
        "reddit" => ("Reddit", 0xFF4500),
        "twitter" => ("X", 0x14171A),
        "twitter_legacy" => ("Twitter", 0x1DA1F2),
        "playstation" => ("PlayStation", 0x003791),
        "battlenet" => ("Battle.net", 0x148EFF),
        "epicgames" => ("Epic Games", 0x313131),
        "tiktok" => ("TikTok", 0x111111),
        "instagram" => ("Instagram", 0xE4405F),
        "bluesky" => ("Bluesky", 0x1185FE),
        "roblox" => ("Roblox", 0x232527),
        "ebay" => ("eBay", 0xE53238),
        "facebook" => ("Facebook", 0x1877F2),
        "leagueoflegends" => ("League of Legends", 0xC89B3C),
        "riotgames" => ("Riot Games", 0xD32936),
        "crunchyroll" => ("Crunchyroll", 0xF47521),
        "paypal" => ("PayPal", 0x003087),
        "letterboxd" => ("Letterboxd", 0x202830),
        "mastodon" => ("Mastodon", 0x6364FF),
        "domain" => ("Domain", 0x5D6B7A),
        "skype" => ("Skype", 0x00AFF0),
        "bungie" => ("Bungie.net", 0x0E7AB7),
        "amazon-music" => ("Amazon Music", 0x25D1DA),
        "samsung" => ("Samsung Galaxy", 0x1428A0),
        "meta_quest_or_horizon" => ("Meta Quest", 0x0467DF),
        other => return (other.to_owned(), 0x5D6B7A),
    };
    (name.to_owned(), color)
}

/// The platform's icon: its initial on a tile in its colour.
pub fn paint_platform_icon(painter: &egui::Painter, rect: Rect, kind: &str) {
    let (name, color) = platform(kind);
    painter.rect_filled(rect, rect.width() * 0.25, rgb(color));
    let letter = super::images::initial(&name);
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, letter, bold(rect.height() * 0.55), Color32::WHITE);
}

/// Connected accounts as a two-column grid of tiles: platform icon, name, Check when verified,
/// External when it opens a profile page (else a click copies the name).
pub(super) fn connections_grid(ui: &mut egui::Ui, pal: &Pal, accounts: &[ConnectedAccount]) {
    let list: Vec<&ConnectedAccount> = accounts.iter().filter(|a| shown_connection(&a.kind)).collect();
    let w = ((ui.available_width() - 8.0) / 2.0).floor();
    for pair in list.chunks(2) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            for a in pair {
                connection_tile(ui, pal, a, w);
            }
        });
    }
}

fn connection_tile(ui: &mut egui::Ui, pal: &Pal, a: &ConnectedAccount, w: f32) {
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 44.0), Sense::click());
    let url = connection_url(&a.kind, &a.id, &a.name);
    let fill = if resp.hovered() { pal.tk.hover } else { Color32::TRANSPARENT };
    ui.painter().rect(rect, pal.radius, fill, egui::Stroke::new(1.0, pal.tk.rule), egui::StrokeKind::Inside);
    let icon = Rect::from_min_size(pos2(rect.left() + 10.0, rect.center().y - 12.0), Vec2::splat(24.0));
    paint_platform_icon(ui.painter(), icon, &a.kind);
    let right = rect.right() - 10.0 - if url.is_some() { 18.0 } else { 0.0 };
    let check = if a.verified { 18.0 } else { 0.0 };
    let g = one_line(ui, a.name.as_str(), FontId::new(pal.ui(), crate::theme::medium_family()), pal.text, right - icon.right() - 10.0 - check);
    let gx = icon.right() + 10.0;
    let gw = g.size().x;
    ui.painter().galley(pos2(gx, rect.center().y - g.size().y / 2.0), g, pal.text);
    let mark = |x: f32, icon| super::icons::paint(ui.painter(), icon, Rect::from_min_size(pos2(x, rect.center().y - 7.0), Vec2::splat(14.0)), pal.muted, Color32::TRANSPARENT);
    if a.verified {
        mark(gx + gw + 4.0, super::icons::Icon::Check);
    }
    if url.is_some() {
        mark(rect.right() - 24.0, super::icons::Icon::External);
    }
    let (label, _) = platform(&a.kind);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, format!("{label}: {}", a.name)));
    let tip = match &url {
        Some(_) => format!("{label}{}", if a.verified { ", verified" } else { "" }),
        None => format!("{label}: select to copy the name"),
    };
    let resp = resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.clicked() {
        match url {
            Some(u) => ui.ctx().open_url(egui::OpenUrl::new_tab(u)),
            None => ui.ctx().copy_text(a.name.clone()),
        }
    }
}

// ---------------------------------------------------------------------------
// Game collection (§11)
// ---------------------------------------------------------------------------

fn widget_title(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "favorite_games" => "Favorite game",
        "current_games" => "Games in rotation",
        "played_games" => "Games I like",
        "want_to_play_games" => "Want to play",
        _ => return None,
    })
}

/// "open_to_teach" → "Open to teach".
fn tag_label(tag: &str) -> String {
    let t = tag.replace('_', " ");
    let mut c = t.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// The profile's game widgets (favourite with its comment and tags, then rows of 3:4 cover
/// tiles: cover from `/games`, else the app icon, else the name on a tile). Asks for unknown
/// games once (`FetchGames`).
pub(super) fn game_collection(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, widgets: &[ProfileWidget]) {
    let shown: Vec<&ProfileWidget> = widgets.iter().filter(|w| widget_title(&w.data.kind).is_some() && !w.data.games.is_empty()).collect();
    let ids = cx.state.request_games(shown.iter().flat_map(|w| w.data.games.iter().map(|g| g.game_id)));
    if !ids.is_empty() {
        cx.cmd.send(Command::Social(SocialCommand::FetchGames { ids }));
    }
    for w in shown {
        // A section of the profile: the label role, 16 px under the previous one.
        ui.add_space(16.0 - ui.spacing().item_spacing.y);
        ui.label(egui::RichText::new(widget_title(&w.data.kind).unwrap_or_default()).font(super::semibold(pal.label())).color(pal.muted));
        let detailed = w.data.kind == "favorite_games";
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(12.0, 12.0);
            for g in &w.data.games {
                game_tile(ui, cx, pal, g, detailed);
            }
        });
    }
}

fn game_tile(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, wg: &WidgetGame, detailed: bool) {
    let game = cx.state.social.games.get(&wg.game_id).and_then(Option::as_ref).cloned();
    let name = game.as_ref().map_or_else(String::new, |g| g.name.clone());
    let cover = Vec2::new(72.0, 96.0);
    let w = if detailed { ui.available_width() } else { cover.x };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, cover.y + if detailed { 0.0 } else { 20.0 }), Sense::hover());
    let art = Rect::from_min_size(rect.min, cover);
    let painter = ui.painter();
    painter.rect_filled(art, pal.radius, pal.tk.hover);
    let cover_url = game.as_ref().and_then(|g| g.supplemental_game_data.as_ref()?.cover_image_url.as_deref()).and_then(cdn::game_cover_url);
    let icon_url = game.as_ref().and_then(|g| cdn::app_icon_url(g.id, g.icon_hash.as_deref()?, 64));
    if let Some(tex) = cover_url.and_then(|u| cx.images.get(ui.ctx(), &u, px(ui.ctx(), cover.y), false)) {
        let uv = super::profile::cover_uv(tex.size, cover);
        painter.add(Shape::Rect(RectShape::filled(art, pal.radius, Color32::WHITE).with_texture(tex.id, uv)));
    } else if let Some(tex) = icon_url.and_then(|u| cx.images.get(ui.ctx(), &u, px(ui.ctx(), 40.0), false)) {
        painter.add(tex_shape(tex, Rect::from_center_size(art.center(), Vec2::splat(40.0)), 8, Color32::WHITE));
    } else if !name.is_empty() {
        painter.text(art.center(), egui::Align2::CENTER_CENTER, super::images::initial(&name), bold(28.0), pal.muted);
    }
    if detailed {
        let x = art.right() + 12.0;
        let mut y = art.top() + 4.0;
        let max_w = rect.right() - x;
        let mut line = |text: String, font: FontId, color: Color32| {
            let g = one_line(ui, text, font, color, max_w);
            let h = g.size().y;
            ui.painter().galley(pos2(x, y), g, color);
            y += h + 4.0;
        };
        line(if name.is_empty() { "…".into() } else { name.clone() }, super::semibold(pal.heading()), pal.text);
        if let Some(c) = wg.comment.as_deref().filter(|c| !c.is_empty()) {
            line(c.to_owned(), FontId::proportional(pal.meta()), pal.text);
        }
        if !wg.tags.is_empty() {
            line(wg.tags.iter().map(|t| tag_label(t)).collect::<Vec<_>>().join(", "), FontId::proportional(pal.meta()), pal.muted);
        }
    } else {
        let g = one_line(ui, name.as_str(), FontId::proportional(pal.meta()), pal.muted, cover.x);
        ui.painter().galley(pos2(art.left(), art.bottom() + 3.0), g, pal.muted);
        if !name.is_empty() {
            resp.on_hover_text(name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gold_tag_reaches_4_5_on_cream() {
        // MoreUserTags' OWNER chip on Paper Light's darkest surface (`plugins/people.rs` paint_chip).
        let gold = Color32::from_rgb(0xF0, 0xB2, 0x32);
        let bg = Color32::from_rgb(0xED, 0xE6, 0xDC).blend(gold.gamma_multiply(0.18));
        assert!(contrast(reach(gold, &[bg], Color32::BLACK, 4.5), bg) >= 4.5);
    }

    #[test]
    fn light_nameplate_art_is_clear_under_the_text() {
        let tex = SizedTexture::new(egui::TextureId::default(), vec2(160.0, 30.0));
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(240.0, 44.0));
        let full = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        let Shape::Mesh(m) = faded_tex(tex, r, full, CornerRadius::same(8), 100.0, 1.0) else { panic!() };
        for v in &m.vertices {
            assert!(r.expand(0.01).contains(v.pos), "inside the row: {:?}", v.pos);
            let a = v.color.a();
            if v.pos.x <= 108.0 {
                assert_eq!(a, 0, "under the text and its gap");
            } else if v.pos.x >= 148.0 {
                assert_eq!(a, 255, "after the fade");
            }
            // The right corners are rounded: every vertex there is on or inside the 8 px arcs.
            if v.pos.x > 232.0 {
                assert!(v.pos.distance(pos2(232.0, v.pos.y.clamp(8.0, 36.0))) <= 8.01, "{:?}", v.pos);
            }
        }
        // No text (a shop tile): the whole art.
        let Shape::Mesh(m) = faded_tex(tex, r, full, CornerRadius::ZERO, f32::NEG_INFINITY, 1.0) else { panic!() };
        assert!(m.vertices.iter().all(|v| v.color.a() == 255));
    }

    #[test]
    fn scrim_follows_each_corner_and_clears_after_the_text() {
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(240.0, 44.0));
        // The user panel under the voice section: square top, rounded bottom corners.
        let Shape::Mesh(m) = plate_scrim(r, CornerRadius { sw: 8, se: 8, ..CornerRadius::ZERO }, Color32::BLACK, 100.0, false) else { panic!() };
        let left: Vec<f32> = m.vertices.iter().filter(|v| v.pos.x == 0.0).map(|v| v.pos.y).collect();
        assert_eq!(left, [0.0, 36.0]);
        for v in &m.vertices {
            assert!(r.expand(0.01).contains(v.pos), "{:?}", v.pos);
            if v.pos.x <= 108.0 {
                assert_eq!(v.color.a(), Color32::BLACK.gamma_multiply(0.7).a(), "under the text");
            } else if v.pos.x >= 148.0 {
                assert_eq!(v.color.a(), 0, "clear past the fade");
            }
        }
    }

    #[test]
    fn badge_copy_is_our_own_and_keeps_the_dates() {
        assert_eq!(badge_info("premium").map(|b| b.0), Some("Nitro".into()));
        assert_eq!(badge_info("guild_booster_lvl3").map(|b| b.0), Some("Server booster".into()));
        assert_eq!(badge_info("premium_tenure_12_month_v2").map(|b| b.0), Some("Nitro, 12 months".into()));
        let (title, body) = badge_info("premium_tenure_1_month").unwrap();
        assert_eq!(title, "Nitro, 1 month");
        assert!(body.contains("at least 1 month."));
        assert_eq!(badge_info("something_new"), None);
        let b = |id: &str, d: &str| ProfileBadge { id: id.into(), description: d.into(), icon: String::new(), link: Some("https://x".into()) };
        assert_eq!(badge_text(&b("premium", "Subscriber since Mar 4, 2021")).1, Some("Subscriber since Mar 4, 2021"));
        assert_eq!(badge_text(&b("staff", "Discord Staff")).1, None, "same words as the title: no subtitle");
        assert_eq!(badge_text(&b("something_new", "A new badge")), ("A new badge".into(), None, None));
    }

    #[test]
    fn missing_badge_icon_is_asked_for_once_more() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut images = ImageCache::new(crate::backend::CommandSender(tx));
        let ctx = egui::Context::default();
        let url = "https://cdn.discordapp.com/badge-icons/abc.png";
        // Never answered: the first request, one retry after BADGE_RETRY_SECS, then nothing.
        for t in [0.0, 1.0, BADGE_RETRY_SECS + 1.0, 30.0, 120.0] {
            let input = egui::RawInput { time: Some(t), ..Default::default() };
            ctx.run_ui(input, |ui| assert!(badge_icon(ui, &mut images, url).is_none())).textures_delta.clear();
        }
        let sent = std::iter::from_fn(|| rx.try_recv().ok()).filter(|c| matches!(c, Command::LoadImage { .. })).count();
        assert_eq!(sent, 2);
    }

    fn user_with_tag(enabled: Option<bool>, tag: Option<&str>) -> User {
        let pg = PrimaryGuild { identity_guild_id: Some(Id(5)), identity_enabled: enabled, tag: tag.map(Into::into), badge: Some("b".into()) };
        User { primary_guild: Some(Box::new(pg)), ..Default::default() }
    }

    #[test]
    fn guild_tag_rules() {
        let u = user_with_tag(Some(true), Some("MEOW"));
        assert_eq!(guild_tag(&u, None).and_then(|p| p.tag.as_deref()), Some("MEOW"));
        let quarantined = Member { flags: 1 << 10, ..Default::default() };
        assert!(guild_tag(&u, Some(&quarantined)).is_none());
        assert!(guild_tag(&u, Some(&Member { flags: 1 << 3, ..Default::default() })).is_some(), "other flags keep it");
        assert!(guild_tag(&user_with_tag(Some(false), Some("MEOW")), None).is_none());
        assert!(guild_tag(&user_with_tag(Some(true), None), None).is_none(), "not re-affirmed");
        // Old name.
        let old = User { clan: u.primary_guild.clone(), ..Default::default() };
        assert!(guild_tag(&old, None).is_some());
    }

    #[test]
    fn effect_layer_windows() {
        let l = |start: f32, duration: f32, looping: bool, loop_delay: f32| EffectLayer { start, duration, looping, loop_delay, ..Default::default() };
        assert_eq!(layer_window(&l(100.0, 1000.0, false, 0.0), 50.0), (false, Some(50.0)));
        assert_eq!(layer_window(&l(100.0, 1000.0, false, 0.0), 600.0), (true, Some(500.0)));
        assert_eq!(layer_window(&l(100.0, 1000.0, false, 0.0), 1200.0), (false, None));
        assert_eq!(layer_window(&l(0.0, 1000.0, true, 0.0), 5000.0), (true, None));
        // Loop of 1 s + 2 s pause: 3.5 s in = in the pause, 2.5 s until the next round.
        assert_eq!(layer_window(&l(0.0, 1000.0, true, 2000.0), 1500.0), (false, Some(1500.0)));
        assert_eq!(layer_window(&l(0.0, 1000.0, true, 2000.0), 3200.0), (true, Some(800.0)));
    }

    #[test]
    fn connection_urls() {
        assert_eq!(connection_url("github", "1", "ferris").as_deref(), Some("https://github.com/ferris"));
        assert_eq!(connection_url("twitch", "1", "a b/c").as_deref(), Some("https://www.twitch.tv/a%20b%2Fc"));
        assert_eq!(connection_url("bluesky", "did:plc:abc", "x").as_deref(), Some("https://bsky.app/profile/did:plc:abc"));
        assert_eq!(connection_url("steam", "7656", "x").as_deref(), Some("https://steamcommunity.com/profiles/7656"));
        assert_eq!(connection_url("domain", "example.com", "x").as_deref(), Some("https://example.com/"));
        assert_eq!(connection_url("mastodon", "https://mastodon.social/@x", "x").as_deref(), Some("https://mastodon.social/@x"));
        assert_eq!(connection_url("mastodon", "javascript:alert(1)", "x"), None);
        assert_eq!(connection_url("xbox", "1", "x"), None, "no page: copy the name");
        assert_eq!(connection_url("skype", "1", "x"), None);
        assert!(!shown_connection("contacts") && shown_connection("github"));
        assert_eq!(platform("github").0, "GitHub");
        assert_eq!(platform("newthing"), ("newthing".into(), 0x5D6B7A));
    }

    #[test]
    fn badges_from_flags_and_profile() {
        let ids: Vec<String> = flag_badges((1 << 9) | (1 << 22) | (1 << 4)).into_iter().map(|b| b.id).collect();
        assert_eq!(ids, ["early_supporter", "active_developer"]);
        let user = User { public_flags: 1, ..Default::default() };
        assert_eq!(badges(&user, None)[0].id, "staff");
        let p = UserProfile {
            badges: vec![ProfileBadge { id: "premium".into(), description: "Nitro".into(), icon: "x".into(), link: None }],
            guild_badges: vec![ProfileBadge { id: "guild_booster_lvl2".into(), description: "Boost".into(), icon: "y".into(), link: None }],
            premium_since: Some("2021-03-04T00:00:00+00:00".into()),
            ..Default::default()
        };
        let b = badges(&user, Some(&p));
        assert_eq!((b[0].description.as_str(), b[1].description.as_str()), ("Subscriber since Mar 4, 2021", "Boost"));
    }

    #[test]
    fn name_colours_read_and_blend() {
        let dark_bg = rgb(0x121214);
        let c = readable(rgb(0x101010), true);
        assert!(contrast(c, dark_bg) >= 3.0, "{c:?}");
        assert_eq!(readable(rgb(0xFFC490), true), rgb(0xFFC490), "already readable");
        let stops = [Color32::BLACK, Color32::WHITE];
        assert_eq!((sample(&stops, 0.0), sample(&stops, 1.0)), (Color32::BLACK, Color32::WHITE));
        assert_eq!(sample(&[Color32::RED], 0.5), Color32::RED);
        assert_eq!(name_family(Some(&NameStyle { font_id: Some(8), ..Default::default() }), FontFamily::Proportional), FontFamily::Monospace);
        assert_eq!(palette("none", true), None);
        assert_eq!(palette("crimson", false), Some(rgb(0xE7040F)));
        assert_eq!(tag_label("open_to_teach"), "Open to teach");
    }

    #[test]
    fn themed_cards_pick_their_mode_and_read() {
        let app = Pal::new(&crate::theme::Theme::default());
        // Midnight Blurple, Cotton Candy, Sunset (light accent under a dark primary).
        for (colors, dark) in [((0x5348CA, 0x140730), true), ((0xF4ABB8, 0xB1C2FC), false), ((0x48288C, 0xDB7F4B), true)] {
            let p = theme_pal(&app, colors);
            assert_eq!(p.dark, dark, "{colors:x?}");
            for c in [colors.0, colors.1] {
                let body = rgb(c).lerp_to_gamma(veil(dark), 0.6);
                assert!(contrast(p.muted, body) >= 4.5 && contrast(p.text, body) >= 7.0, "{colors:x?} on {body:?}");
            }
        }
    }

    #[test]
    fn effect_mesh_follows_the_rounded_card() {
        let card = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 200.0));
        let tex = SizedTexture::new(egui::TextureId::User(1), vec2(10.0, 10.0));
        let Shape::Mesh(m) = rounded_tex(tex, card, card, 10.0) else { panic!("a mesh") };
        // The top row runs between the corner arcs, the arcs end on the sides; uv = position.
        let top: Vec<f32> = m.vertices.iter().filter(|v| v.pos.y == 0.0).map(|v| v.pos.x).collect();
        assert_eq!(top, [10.0, 90.0]);
        assert!(m.vertices.iter().any(|v| v.pos == pos2(0.0, 10.0)) && m.vertices.iter().any(|v| v.pos == pos2(100.0, 190.0)));
        assert!(m.vertices.iter().all(|v| (v.uv.x - v.pos.x / 100.0).abs() < 1e-5 && (v.uv.y - v.pos.y / 200.0).abs() < 1e-5));
        // A layer below the card's corners: only its part inside the card.
        let low = Rect::from_min_size(pos2(0.0, 150.0), vec2(100.0, 100.0));
        let Shape::Mesh(m) = rounded_tex(tex, low, card, 10.0) else { panic!("a mesh") };
        assert!(m.vertices.iter().all(|v| v.pos.y >= 150.0 && v.pos.y <= 200.0 && (v.uv.y - (v.pos.y - 150.0) / 100.0).abs() < 1e-5));
    }

    #[test]
    fn products_and_games_requested_once() {
        let mut s = crate::state::AppState::default();
        assert!(s.request_product(Id(7)));
        assert!(!s.request_product(Id(7)), "asked once");
        let ids = s.request_games([Id(1), Id(1), Id(0), Id(2)]);
        assert_eq!(ids, [Id(1), Id(2)]);
        assert!(s.request_games([Id(2)]).is_empty());
        s.apply_social(crate::backend::SocialEvent::GamesLoaded { ids, result: Ok(vec![Game { id: Id(1), name: "Factorio".into(), ..Default::default() }]) });
        assert_eq!(s.social.games[&Id(1)].as_ref().map(|g| g.name.as_str()), Some("Factorio"));
        assert!(s.social.games[&Id(2)].is_none(), "missing from the answer: not asked again");
        let product = CollectibleProduct { sku_id: Id(7), items: vec![CollectibleItem { kind: 1, ..Default::default() }], ..Default::default() };
        s.apply_social(crate::backend::SocialEvent::ProductLoaded { sku_id: Id(7), result: Ok(product) });
        assert!(s.social.products[&Id(7)].is_some());
    }
}
