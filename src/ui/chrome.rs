//! Window chrome of the frameless window (`main.rs` turns the OS decorations off): the title
//! bar row drags the window (double-click maximizes / restores), the minimize, maximize /
//! restore and close buttons sit at its right end, and invisible edges resize the window.

use crate::theme::{Theme, ThemeColors};
use egui::viewport::ResizeDirection;
use egui::{Color32, CornerRadius, CursorIcon, Pos2, Rect, Role, Sense, Stroke, StrokeKind, UiBuilder, Vec2, ViewportCommand, WidgetInfo, pos2, vec2};

/// Width of one window button (Windows 11 caption buttons).
const BUTTON_W: f32 = 46.0;
/// Thickness of the resize edges and length of the corner zones along each edge.
const EDGE: f32 = 5.0;
const CORNER: f32 = 12.0;
/// Close button fill on hover (Windows' caption red).
const CLOSE_RED: Color32 = Color32::from_rgb(0xc4, 0x2b, 0x1c);

#[derive(Clone, Copy, PartialEq)]
enum Glyph {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// The title bar row: its empty area drags the window, a double-click on it maximizes /
/// restores, the window buttons take the right end. `content` draws into the row minus the
/// buttons' width on both sides, so centred content stays centred on the window.
pub fn title_bar(ui: &mut egui::Ui, c: &ThemeColors, content: impl FnOnce(&mut egui::Ui)) {
    let rect = ui.max_rect();
    let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
    // Registered first, so every widget drawn on the row lies on top of it. Not focusable.
    let drag = ui.interact(rect, ui.make_persistent_id("window_drag"), Sense::CLICK | Sense::DRAG);
    if drag.double_clicked() {
        ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    } else if drag.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
    let side = 3.0 * BUTTON_W;
    ui.scope_builder(UiBuilder::new().max_rect(rect.shrink2(vec2(side, 0.0))), content);
    // macOS: the native traffic lights sit in the left inset (main.rs, `RustcordApp::ui`).
    if cfg!(target_os = "macos") {
        return;
    }

    let (resize, tip) = if maximized { (Glyph::Restore, "Restore") } else { (Glyph::Maximize, "Maximize") };
    let buttons = [
        (Glyph::Minimize, "Minimize", ViewportCommand::Minimized(true)),
        (resize, tip, ViewportCommand::Maximized(!maximized)),
        (Glyph::Close, "Close", ViewportCommand::Close),
    ];
    for (i, (glyph, tip, cmd)) in buttons.into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.right() - (3 - i) as f32 * BUTTON_W, rect.top()), vec2(BUTTON_W, rect.height()));
        let resp = ui.interact(r, ui.make_persistent_id(("window_button", i)), Sense::click());
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, tip));
        let hot = resp.hovered() || resp.has_focus();
        let pressed = resp.is_pointer_button_down_on();
        let close = glyph == Glyph::Close;
        let fill = match (close, pressed, hot) {
            (true, true, _) => CLOSE_RED.gamma_multiply(0.9),
            (true, false, true) => CLOSE_RED,
            (false, true, _) => c.text.to_color32().gamma_multiply(0.06),
            (false, false, true) => c.text.to_color32().gamma_multiply(0.1),
            (_, false, false) => Color32::TRANSPARENT,
        };
        ui.painter().rect_filled(r, 0.0, fill);
        let ink = match (close, hot || pressed) {
            (true, true) => Color32::WHITE,
            (false, true) => c.text.to_color32(),
            (_, false) => c.text_muted.to_color32(),
        };
        paint_glyph(ui.painter(), r.center(), glyph, ink);
        if resp.on_hover_text(tip).clicked() {
            ui.ctx().send_viewport_cmd(cmd);
        }
    }
}

/// Windows 11 caption glyphs (10 px), snapped to the pixel grid so the 1 px lines stay crisp.
fn paint_glyph(p: &egui::Painter, center: Pos2, glyph: Glyph, color: Color32) {
    let ppp = p.pixels_per_point();
    let n = ppp.round().max(1.0); // stroke width in physical pixels
    let off = if n % 2.0 == 1.0 { 0.5 } else { 0.0 };
    let snap = |v: f32| ((v * ppp - off).round() + off) / ppp;
    let whole = |v: f32| (v * ppp).round() / ppp;
    let stroke = Stroke::new(n / ppp, color);
    let c = pos2(snap(center.x), snap(center.y));
    let h = whole(5.0);
    match glyph {
        Glyph::Minimize => {
            p.hline(c.x - h..=c.x + h, c.y, stroke);
        }
        Glyph::Maximize => {
            p.rect_stroke(Rect::from_center_size(c, Vec2::splat(2.0 * h)), CornerRadius::same(1), stroke, StrokeKind::Middle);
        }
        Glyph::Restore => {
            let d = whole(2.0);
            let front = Rect::from_min_max(pos2(c.x - h, c.y - h + d), pos2(c.x + h - d, c.y + h));
            p.rect_stroke(front, CornerRadius::same(1), stroke, StrokeKind::Middle);
            let back = vec![pos2(c.x - h + d, c.y - h + d), pos2(c.x - h + d, c.y - h), pos2(c.x + h, c.y - h), pos2(c.x + h, c.y + h - d), pos2(c.x + h - d, c.y + h - d)];
            p.add(egui::Shape::line(back, stroke));
        }
        Glyph::Close => {
            p.line_segment([c - Vec2::splat(h), c + Vec2::splat(h)], stroke);
            p.line_segment([pos2(c.x - h, c.y + h), pos2(c.x + h, c.y - h)], stroke);
        }
    }
}

/// The login screen's title bar content: Rustcord's logo mark and name, centred.
pub fn app_label(ui: &egui::Ui, theme: &Theme) {
    let rect = ui.max_rect();
    let muted = theme.colors.text_muted.to_color32();
    let g = ui.painter().layout_no_wrap(crate::config::APP_NAME.to_owned(), super::bold(theme.font.size - 0.5), muted);
    let icon = Rect::from_min_size(pos2(rect.center().x - (24.0 + g.size().x) / 2.0, rect.center().y - 8.0), Vec2::splat(16.0));
    super::sidebar::logo_mark(ui.painter(), icon, super::sidebar::SMALL_MARK, theme.colors.accent.to_color32(), theme.colors.accent_text.to_color32());
    ui.painter().galley(pos2(icon.right() + 8.0, rect.center().y - g.size().y / 2.0), g, muted);
}

/// Resize edge (or corner) of `window` under `p`, if any.
fn resize_dir(window: Rect, p: Pos2) -> Option<ResizeDirection> {
    use ResizeDirection::*;
    let (l, r, t, b) = (p.x - window.left(), window.right() - p.x, p.y - window.top(), window.bottom() - p.y);
    if l.min(r).min(t).min(b) < 0.0 {
        return None;
    }
    // Corner zones reach CORNER along each edge, so corners are easier to grab.
    let along = |d: f32, a: f32, b: f32| d < EDGE || (d < CORNER && (a < EDGE || b < EDGE));
    let (n, s, w, e) = (along(t, l, r), along(b, l, r), along(l, t, b), along(r, t, b));
    Some(match (n, s, w, e) {
        (true, _, true, _) => NorthWest,
        (true, _, _, true) => NorthEast,
        (_, true, true, _) => SouthWest,
        (_, true, _, true) => SouthEast,
        (true, ..) => North,
        (_, true, ..) => South,
        (.., true, _) => West,
        (.., true) => East,
        _ => return None,
    })
}

/// Not maximized or full screen; never on macOS (native edges; winit can't start a resize there).
fn can_resize(info: Option<&egui::ViewportInfo>) -> bool {
    !cfg!(target_os = "macos") && info.is_none_or(|v| !v.maximized.unwrap_or(false) && !v.fullscreen.unwrap_or(false))
}

/// `App::raw_input_hook`: a primary press on a window edge starts the OS resize and is taken
/// out of the input, so nothing under the edge also reacts to it. Not while maximized.
pub fn resize_press(ctx: &egui::Context, raw: &mut egui::RawInput) {
    let Some(window) = raw.screen_rect else { return };
    if !can_resize(raw.viewports.get(&raw.viewport_id)) {
        return;
    }
    raw.events.retain(|e| {
        let egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, .. } = e else { return true };
        let Some(dir) = resize_dir(window, *pos) else { return true };
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
        false
    });
}

/// After everything is drawn: the resize cursor on a window edge.
pub fn resize_cursor(ctx: &egui::Context) {
    let dir = ctx.input(|i| {
        let free = can_resize(Some(i.viewport())) && !i.pointer.any_down();
        i.pointer.hover_pos().filter(|_| free).and_then(|p| resize_dir(i.viewport_rect(), p))
    });
    if let Some(dir) = dir {
        ctx.set_cursor_icon(match dir {
            ResizeDirection::North | ResizeDirection::South => CursorIcon::ResizeVertical,
            ResizeDirection::East | ResizeDirection::West => CursorIcon::ResizeHorizontal,
            ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::ResizeNwSe,
            ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::ResizeNeSw,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_zones() {
        let w = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        let at = |x, y| resize_dir(w, pos2(x, y));
        assert_eq!(at(400.0, 300.0), None);
        assert_eq!(at(400.0, 6.0), None);
        assert_eq!(at(-1.0, 300.0), None);
        assert_eq!(at(2.0, 300.0), Some(ResizeDirection::West));
        assert_eq!(at(797.0, 300.0), Some(ResizeDirection::East));
        assert_eq!(at(400.0, 1.0), Some(ResizeDirection::North));
        assert_eq!(at(400.0, 599.0), Some(ResizeDirection::South));
        assert_eq!(at(1.0, 1.0), Some(ResizeDirection::NorthWest));
        // Corner zones reach CORNER along the edges.
        assert_eq!(at(10.0, 2.0), Some(ResizeDirection::NorthWest));
        assert_eq!(at(790.0, 3.0), Some(ResizeDirection::NorthEast));
        assert_eq!(at(2.0, 590.0), Some(ResizeDirection::SouthWest));
        assert_eq!(at(798.0, 595.0), Some(ResizeDirection::SouthEast));
        assert_eq!(at(20.0, 2.0), Some(ResizeDirection::North));
    }

    #[test]
    fn no_resize_when_maximized() {
        let mut info = egui::ViewportInfo::default();
        assert_eq!(can_resize(Some(&info)) && can_resize(None), !cfg!(target_os = "macos"));
        info.maximized = Some(true);
        assert!(!can_resize(Some(&info)));
    }
}
