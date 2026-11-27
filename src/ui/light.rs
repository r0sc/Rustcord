//! Light (docs/DESIGN.md §1 rule 4): the three glows (the rail pill's light, the profile card's
//! aura light and edge, the speaking light), a mention's one flash and the user panel's rim. Plain
//! shapes: no texture, no layer, no frames of their own.

use crate::theme::{Mode, Tokens};
use egui::epaint::{CornerRadiusF32, PathShape, PathStroke, RectShape, RoundedRect, tessellator};
use egui::{Color32, CornerRadius, Pos2, Rect, Shape, Stroke, pos2};

// The helpers are read by the lanes that follow (rail, profile card, call stage, user panel).
/// Whether a theme takes light: dark ones, gradient ones too. Light themes get lift (a tinted
/// shadow) instead, High Contrast nothing.
#[allow(dead_code)]
pub fn lit(tk: &Tokens) -> bool {
    tk.mode == Mode::Dark
}

/// Soft light around `rect`: full `color` over it, nothing `blur` px out. epaint feathers a blurred
/// rect across its edge and clamps the feather to the short side, so the rect grows by blur / 2
/// first (a 4 px pill keeps its blur). Pass `color.additive()` for light that only brightens.
#[allow(dead_code)]
pub fn glow(rect: Rect, radius: f32, color: Color32, blur: f32) -> Shape {
    let r = (radius + blur / 2.0).clamp(0.0, 255.0) as u8;
    RectShape::filled(rect.expand(blur / 2.0), r, color).with_blur_width(blur).into()
}

/// A big glow as two stacked ones (one linear feather looks hard next to a CSS shadow): blur / 3 at
/// 60 % plus blur at 30 %.
#[allow(dead_code)]
pub fn soft_glow(rect: Rect, radius: f32, color: Color32, blur: f32) -> Shape {
    Shape::Vec(vec![glow(rect, radius, color.gamma_multiply(0.6), blur / 3.0), glow(rect, radius, color.gamma_multiply(0.3), blur)])
}

/// A float's 1 px outline lit in its owner's colour: `aura` @ 85 % along the top, 25 % at 38 % of
/// the height, `rule` from 70 % down; `cr` its corners (speech corner too).
#[allow(dead_code)]
pub fn edge_light(rect: Rect, cr: CornerRadius, aura: Color32, rule: Color32) -> Shape {
    let inner = CornerRadiusF32 { nw: f32::from(cr.nw) - 0.5, ne: f32::from(cr.ne) - 0.5, sw: f32::from(cr.sw) - 0.5, se: f32::from(cr.se) - 0.5 };
    let mut pts = Vec::new();
    tessellator::path::rounded_rectangle(&mut pts, RoundedRect::new(rect.shrink(0.5), inner));
    let (top, mid) = (aura.gamma_multiply(0.85), aura.gamma_multiply(0.25));
    let stroke = PathStroke::new_uv(1.0, move |bb, p| {
        let t = egui::remap_clamp(p.y, bb.y_range(), 0.0..=1.0);
        if t < 0.38 {
            top.lerp_to_gamma(mid, t / 0.38)
        } else {
            mid.lerp_to_gamma(rule, ((t - 0.38) / 0.32).min(1.0))
        }
    });
    Shape::closed_line(densify(&pts, 12.0, true), stroke)
}

/// The sheen catching the pointer (the user panel, DESIGN §6.2): the line of `messages::sheen`
/// (1.5 px under the top, `r` in from both ends), a point every 8 px, brightening from `tk.sheen`
/// toward white @ 38 % (`from_white_alpha(97)`) by k x (1 - (dx / 150)²).max(0), k = 1 -
/// `rect.distance_to_pos(pointer)` / 96. No pointer within 96 px: the plain sheen (`Shape::hline`);
/// no sheen (light, High Contrast): `Shape::Noop`. Paint only: pointer moves repaint anyway.
#[allow(dead_code)]
pub fn rim(ctx: &egui::Context, rect: Rect, r: f32, tk: &Tokens) -> Shape {
    if tk.sheen.a() == 0 {
        return Shape::Noop;
    }
    let (x0, x1, y) = (rect.left() + r, rect.right() - r, rect.top() + 1.5);
    let near = ctx.pointer_latest_pos().map(|p| (p, 1.0 - rect.distance_to_pos(p) / 96.0)).filter(|(_, k)| *k > 0.0);
    let Some((p, k)) = near else { return Shape::hline(x0..=x1, y, Stroke::new(1.0, tk.sheen)) };
    let (sheen, white) = (tk.sheen, Color32::from_white_alpha(97));
    let stroke = PathStroke::new_uv(1.0, move |_, q| {
        let dx = (q.x - p.x) / 150.0;
        sheen.lerp_to_gamma(white, k * (1.0 - dx * dx).max(0.0))
    });
    Shape::Path(PathShape::line(densify(&[pos2(x0, y), pos2(x1, y)], 8.0, false), stroke))
}

/// `pts` with a point at least every `step` px (closed: the last segment too).
fn densify(pts: &[Pos2], step: f32, closed: bool) -> Vec<Pos2> {
    let mut out = Vec::with_capacity(pts.len());
    let segments = if closed { pts.len() } else { pts.len().saturating_sub(1) };
    for i in 0..segments {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        let n = ((a.distance(b) / step).ceil() as usize).max(1);
        out.extend((0..n).map(|k| a.lerp(b, k as f32 / n as f32)));
    }
    if !closed && let Some(&last) = pts.last() {
        out.push(last);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use egui::vec2;

    #[test]
    fn a_glow_keeps_its_blur_on_a_thin_pill() {
        let pill = Rect::from_min_size(pos2(10.0, 10.0), vec2(4.0, 40.0));
        let Shape::Rect(r) = glow(pill, 4.0, Color32::WHITE, 12.0) else { panic!("a rect") };
        assert!(r.rect.size().min_elem() >= 16.0 && r.blur_width == 12.0, "{r:?}");
    }

    #[test]
    fn the_edge_light_has_a_point_every_12_px() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(300.0, 200.0));
        let Shape::Path(p) = edge_light(rect, CornerRadius::same(8), Color32::RED, Color32::GRAY) else { panic!("a path") };
        assert!(p.closed);
        let longest = p.points.iter().zip(p.points.iter().cycle().skip(1)).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max);
        assert!(longest <= 12.0 + 1e-3, "{longest}");
    }

    #[test]
    fn the_rim_without_a_pointer_is_the_sheen() {
        let ctx = egui::Context::default();
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(300.0, 60.0));
        let dark = Theme::nightshade().tokens();
        match rim(&ctx, rect, 8.0, &dark) {
            Shape::LineSegment { stroke, .. } => assert_eq!(stroke.color, dark.sheen),
            other => panic!("an hline: {other:?}"),
        }
        assert!(matches!(rim(&ctx, rect, 8.0, &Theme::builtins()[1].tokens()), Shape::Noop));
    }
}
