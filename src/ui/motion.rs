//! Motion (docs/DESIGN.md §7; the ux-motion skill): duration tokens and the helpers every
//! animation goes through. With Reduced motion every duration is 0 (`user_settings::app::frame`
//! sets egui's `animation_time`, `secs` reads it). Everything here asks for frames only while
//! something moves.

use std::sync::{Arc, OnceLock};

use egui::emath::{TSTransform, easing};
use egui::epaint::ClippedShape;
use egui::{AreaState, Color32, Context, Id, LayerId, Order, Pos2, Ui};

/// Chat-speed: menus, context menus, tooltips, popovers and pickers (egui's Area fade =
/// `animation_time` = this, plus the grow in `end_pass`).
pub const FAST: f32 = 0.12;
/// Modals, the lightbox, toasts.
pub const BASE: f32 = 0.20;
/// Settings layers, the title-bar update pill, the login error shake.
pub const LARGE: f32 = 0.30;
/// Exits take this share of the entry (principle 18).
pub const OUT: f32 = 0.5;
/// Floats and modals grow from this scale (principle 11).
pub const GROW: f32 = 0.96;
/// Cards that arrive together start this far apart, at most `STAGGER_MAX` of them.
pub const STAGGER: f32 = 0.04;
pub const STAGGER_MAX: usize = 6;
/// A reaction pill pops to this scale and back.
pub const POP: f32 = 1.12;

/// Test hook `RUSTCORD_MOTION_SCALE=<f>`: every duration x f (10: screenshots mid-animation).
fn scale() -> f32 {
    static S: OnceLock<f32> = OnceLock::new();
    *S.get_or_init(|| std::env::var("RUSTCORD_MOTION_SCALE").ok().and_then(|s| s.parse().ok()).filter(|f: &f32| *f > 0.0).unwrap_or(1.0))
}

/// egui's `animation_time` (Area fade-ins, collapsing headers, scroll bars) for these settings.
pub fn animation_time(reduced: bool) -> f32 {
    if reduced { 0.0 } else { FAST * scale() }
}

/// Windows' "Animation effects" switch is off (Settings > Accessibility > Visual effects): counts
/// as Reduced motion. Read once per start; always false elsewhere.
pub fn os_reduced() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW};
        static S: OnceLock<bool> = OnceLock::new();
        *S.get_or_init(|| {
            let mut on: i32 = 1;
            // SAFETY: the call writes one BOOL through the pointer, which outlives it.
            let ok = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut on).cast(), 0) };
            ok != 0 && on == 0
        })
    }
    #[cfg(not(windows))]
    false
}

/// `s` seconds, or 0 with reduced motion.
pub fn secs(ctx: &Context, s: f32) -> f32 {
    if ctx.global_style().animation_time > 0.0 { s * scale() } else { 0.0 }
}

/// Enter curve (ease-out). The skill's expo curve finishes 2/3 of a 120 ms move in the first
/// 60 Hz frame; cubic-out keeps it visible. Run backwards (exits) it is the matching ease-in.
pub fn ease(t: f32) -> f32 {
    easing::cubic_out(t)
}

/// Scale `s` about `o`.
pub fn about(o: Pos2, s: f32) -> TSTransform {
    TSTransform::new(o.to_vec2() * (1.0 - s), s)
}

/// Eased 0 -> 1 over `s` s since the area on `layer` became visible (egui's own record), for
/// floats drawn only while open. Before or after its `show`.
pub fn opened(ctx: &Context, layer: LayerId, s: f32) -> f32 {
    let dur = secs(ctx, s);
    if dur == 0.0 {
        return 1.0;
    }
    let (now, half) = ctx.input(|i| (i.time, i.predicted_dt * 0.5));
    let shown = ctx.memory(|m| m.areas().visible_last_frame(&layer));
    let at = AreaState::load(ctx, layer.id).and_then(|a| a.last_became_visible_at).filter(|_| shown);
    let lin = (at.map_or(0.0, |at| (now - at) as f32) + half) / dur;
    if lin < 1.0 {
        ctx.request_repaint();
    }
    ease(lin.min(1.0))
}

/// 0 -> 1 while `on`, eased out over `open` s; back over `close` s along the same curve (an
/// ease-in exit); turns around mid-way without a jump. The first call returns the end value:
/// nothing animates on the first frame it is drawn.
pub fn anim(ctx: &Context, id: Id, on: bool, open: f32, close: f32) -> f32 {
    ease(ctx.animate_bool_with_time(id, on, secs(ctx, if on { open } else { close })))
}

/// `anim` for a move between two spots (switch knob, chevron): cubic in-out both ways.
pub fn slide(ctx: &Context, id: Id, on: bool, s: f32) -> f32 {
    easing::cubic_in_out(ctx.animate_bool_with_time(id, on, secs(ctx, s)))
}

/// `rect` drawn at 97 % about its centre (at least 1 px smaller per side) while the pointer holds
/// `resp` down: pressing answers at once (principle 16), instant both ways, no frames.
/// Hit-testing keeps the full rect.
pub fn pressed(resp: &egui::Response, rect: egui::Rect) -> egui::Rect {
    if resp.is_pointer_button_down_on() { held(rect) } else { rect }
}

fn held(rect: egui::Rect) -> egui::Rect {
    rect.shrink2(egui::vec2((rect.width() * 0.015).max(1.0), (rect.height() * 0.015).max(1.0)))
}

/// Starts a pop (1 -> `POP` -> 1 over BASE, see `popped`) for whatever draws under `id`: my own
/// action answered before the server's echo arrives (principle 16).
pub fn pop(ctx: &Context, id: Id) {
    let now = ctx.input(|i| i.time);
    ctx.data_mut(|d| d.insert_temp(id, now));
    ctx.request_repaint();
}

/// The scale of the pop under `id` right now: 1 at rest (and with reduced motion), asking for
/// frames only while it runs.
pub fn popped(ctx: &Context, id: Id) -> f32 {
    let Some(at) = ctx.data(|d| d.get_temp::<f64>(id)) else { return 1.0 };
    let dur = secs(ctx, BASE);
    let lin = (ctx.input(|i| i.time) - at) as f32 / dur;
    if dur == 0.0 || lin >= 1.0 {
        ctx.data_mut(|d| d.remove::<f64>(id));
        return 1.0;
    }
    ctx.request_repaint();
    1.0 + (POP - 1.0) * (lin * std::f32::consts::PI).sin()
}

/// 0 -> 1 for card `k` of a list that replaces a skeleton: `on == false` (the skeleton is
/// showing) arms it, the first `on` call after that runs BASE with `k` x STAGGER of delay (so the
/// cards arrive one after another), and a list that was never a skeleton is at rest (1) at once
/// (principle 3). Frames only while it runs.
pub fn stagger(ctx: &Context, id: Id, on: bool, k: usize) -> f32 {
    let now = ctx.input(|i| i.time);
    if !on {
        ctx.data_mut(|d| d.insert_temp(id, (now, false)));
        return 0.0;
    }
    let Some((at, _)) = ctx.data(|d| d.get_temp::<(f64, bool)>(id)) else { return 1.0 };
    let (dur, gap) = (secs(ctx, BASE), secs(ctx, STAGGER));
    // The clock starts on the first drawn frame after the skeleton.
    let start = if ctx.data(|d| d.get_temp::<(f64, bool)>(id)).is_some_and(|(_, s)| s) { at } else { now };
    ctx.data_mut(|d| d.insert_temp(id, (start, true)));
    let lin = ((now - start) as f32 - gap * k.min(STAGGER_MAX) as f32) / dur;
    if dur == 0.0 || (now - start) as f32 >= dur + gap * STAGGER_MAX as f32 {
        ctx.data_mut(|d| d.remove::<(f64, bool)>(id));
        return 1.0;
    }
    ctx.request_repaint();
    ease(lin.clamp(0.0, 1.0))
}

/// `add` drawn scaled by `s` about its own centre without moving the layout around it. The
/// centre is remembered from the last scaled frame under `id`; the first frame draws plain.
pub fn scaled(ui: &mut Ui, id: Id, s: f32, add: impl FnOnce(&mut Ui) -> egui::Response) -> egui::Response {
    if s == 1.0 {
        return add(ui);
    }
    let key = id.with("centre");
    let resp = match ui.data(|d| d.get_temp::<Pos2>(key)) {
        Some(o) => ui.with_visual_transform(about(o, s), add).inner,
        None => add(ui),
    };
    ui.data_mut(|d| d.insert_temp(key, resp.rect.center()));
    resp
}

#[derive(Clone, Copy)]
struct Tween {
    from: f32,
    to: f32,
    at: f64,
}

/// `target`, reached in `s` s with cubic in-out from wherever the value is (a new target
/// mid-way starts there): rail pill height, segmented selection.
pub fn value(ctx: &Context, id: Id, target: f32, s: f32) -> f32 {
    let (now, dur) = (ctx.input(|i| i.time), f64::from(secs(ctx, s)));
    if dur == 0.0 {
        return target;
    }
    let w = ctx.data_mut(|d| *d.get_temp_mut_or(id, Tween { from: target, to: target, at: now }));
    let cur = egui::lerp(w.from..=w.to, easing::cubic_in_out(((now - w.at) / dur).clamp(0.0, 1.0) as f32));
    if w.to != target {
        ctx.data_mut(|d| d.insert_temp(id, Tween { from: cur, to: target, at: now }));
    }
    if cur != target {
        ctx.request_repaint();
    }
    cur
}

#[derive(Clone, Default)]
struct Own(Vec<LayerId>);
fn own_id() -> Id {
    Id::unique("rc_motion_own")
}

/// `layer` animates itself (modals, toasts, lightbox) or must not move (autocomplete, drag
/// ghost, drop overlay): `end_pass` leaves it alone this pass.
pub fn own(ctx: &Context, layer: LayerId) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Own>(own_id()).0.push(layer));
}

#[derive(Clone)]
struct Ghost {
    layer: LayerId,
    shapes: Arc<[ClippedShape]>,
    pass: u64,
    closed_at: Option<f64>,
}
#[derive(Clone, Default)]
struct Ghosts(Vec<Ghost>);
fn ghosts_id() -> Id {
    Id::unique("rc_motion_ghosts")
}

/// Keeps what `layer` painted this pass: once its owner stops drawing it (closed), its last
/// frame fades out over BASE x OUT with an ease-in instead of vanishing. Call right after the
/// area's `show`. Modals and the lightbox only.
pub fn leave(ctx: &Context, layer: LayerId) {
    if secs(ctx, BASE * OUT) == 0.0 {
        return;
    }
    let shapes: Arc<[ClippedShape]> = ctx.graphics(|g| g.get(layer).map(|l| l.all_entries().cloned().collect())).unwrap_or_default();
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        let all = &mut d.get_temp_mut_or_default::<Ghosts>(ghosts_id()).0;
        all.retain(|g| g.layer != layer);
        all.push(Ghost { layer, shapes, pass, closed_at: None });
    });
}

fn ghosts(ctx: &Context) {
    let (now, pass, dur) = (ctx.input(|i| i.time), ctx.cumulative_pass_nr(), secs(ctx, BASE * OUT));
    let mut all = ctx.data_mut(|d| std::mem::take(d.get_temp_mut_or_default::<Ghosts>(ghosts_id())));
    all.0.retain_mut(|g| {
        if g.pass == pass {
            return true; // still drawn
        }
        let at = *g.closed_at.get_or_insert(now);
        let lin = if dur > 0.0 { (now - at) as f32 / dur } else { 1.0 };
        if lin >= 1.0 {
            return false;
        }
        // Under every float (Middle): a modal opening as this one leaves stays on top.
        let mut p = ctx.layer_painter(LayerId::new(Order::Middle, g.layer.id.with("rc_ghost")));
        p.set_opacity(1.0 - easing::cubic_in(lin));
        for c in g.shapes.iter() {
            p.with_clip_rect(c.clip_rect).add(c.shape.clone());
        }
        ctx.request_repaint();
        true
    });
    ctx.data_mut(|d| d.insert_temp(ghosts_id(), all));
}

/// egui's Modal with Rustcord's motion (principles 5, 11, 18): the scrim fades in; the panel
/// fades in and grows GROW -> 1 over BASE from the click that opened it (else the window
/// centre); closed, its last frame fades out (`leave`). `area` / `frame` / `scrim` as before.
pub fn modal<R>(ctx: &Context, area: egui::Area, frame: egui::Frame, scrim: Color32, add: impl FnOnce(&mut Ui) -> R) -> egui::ModalResponse<R> {
    let layer = area.layer();
    let t = opened(ctx, layer, BASE);
    let from_id = layer.id.with("rc_from");
    let from = if ctx.memory(|m| m.areas().visible_last_frame(&layer)) {
        ctx.data(|d| d.get_temp::<Pos2>(from_id))
    } else {
        let click = ctx.input(|i| i.pointer.any_click().then(|| i.pointer.interact_pos()).flatten());
        ctx.data_mut(|d| match click {
            Some(p) => drop(d.insert_temp(from_id, p)),
            None => d.remove::<Pos2>(from_id),
        });
        click
    }
    .unwrap_or_else(|| ctx.content_rect().center());
    own(ctx, layer);
    let resp = egui::Modal::new(layer.id).area(area.fade_in(false)).frame(egui::Frame::NONE).backdrop_color(scrim.gamma_multiply(t)).show(ctx, |ui| {
        ui.with_visual_transform(about(from, egui::lerp(GROW..=1.0, t)), |ui| {
            ui.multiply_opacity(t);
            frame.show(ui, add).inner
        })
        .inner
    });
    leave(ctx, layer);
    resp
}

/// Last call of every pass (`RustcordApp::ui`). Menus, context menus, submenus, tooltips and
/// popovers (each Foreground / Tooltip area not `own`ed) grow GROW -> 1 about their pivot (the
/// point egui attached them to the trigger; the click for context menus) over FAST while egui
/// fades them in over the same time; they close at once (principle 20). Then the `leave` ghosts.
pub fn end_pass(ctx: &Context) {
    let own = ctx.data_mut(|d| std::mem::take(d.get_temp_mut_or_default::<Own>(own_id())));
    if secs(ctx, FAST) > 0.0 {
        for layer in ctx.memory(|m| m.areas().visible_layer_ids()) {
            if !matches!(layer.order, Order::Foreground | Order::Tooltip) || own.0.contains(&layer) {
                continue;
            }
            let t = opened(ctx, layer, FAST);
            if t < 1.0
                && let Some(o) = AreaState::load(ctx, layer.id).and_then(|a| a.pivot_pos)
            {
                ctx.transform_layer_shapes_after_rounding(layer, about(o, egui::lerp(GROW..=1.0, t)));
            }
        }
    }
    ghosts(ctx);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs passes 1/60 s apart from `from` for `secs`: `f`'s last value and whether the last
    /// pass asked for another frame at once.
    fn run(ctx: &Context, from: f64, secs: f64, mut f: impl FnMut(&Context) -> f32) -> (f32, bool) {
        let (mut v, mut busy, mut t) = (0.0, false, from);
        while t <= from + secs + 1e-9 {
            let mut out = ctx.run_ui(egui::RawInput { time: Some(t), ..Default::default() }, |ui| v = f(ui.ctx()));
            out.textures_delta.clear(); // egui asserts that a dropped delta was applied
            busy = out.viewport_output.get(&egui::ViewportId::ROOT).is_some_and(|o| o.repaint_delay.is_zero());
            t += 1.0 / 60.0;
        }
        (v, busy)
    }

    /// A filled square: no text, so no fonts are needed.
    fn block(ui: &mut Ui) {
        let (r, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
        ui.painter().rect_filled(r, 0.0, Color32::WHITE);
    }

    #[test]
    fn anim_starts_at_rest_runs_and_stops() {
        let ctx = Context::default();
        let id = Id::unique("anim");
        let (first, _) = run(&ctx, 0.0, 0.0, |c| anim(c, id, true, LARGE, 0.0));
        assert_eq!(first, 1.0, "the first call is the end value");
        run(&ctx, 0.1, 0.0, |c| anim(c, id, false, LARGE, 0.0));
        let (mid, busy) = run(&ctx, 0.2, 0.1, |c| anim(c, id, true, LARGE, 0.0));
        assert!(mid > 0.0 && mid < 1.0 && busy);
        let (end, busy) = run(&ctx, 0.4, 0.5, |c| anim(c, id, true, LARGE, 0.0));
        assert!(end == 1.0 && !busy);
        let (shut, _) = run(&ctx, 1.0, 0.0, |c| anim(c, id, false, LARGE, 0.0));
        assert_eq!(shut, 0.0, "a zero close time is instant");
    }

    #[test]
    fn value_settles_and_stops_asking_for_frames() {
        let ctx = Context::default();
        let v = Id::unique("value");
        run(&ctx, 0.0, 0.0, |c| value(c, v, 0.0, FAST));
        let (mid, busy) = run(&ctx, 0.1, 0.05, |c| value(c, v, 40.0, FAST));
        assert!(mid > 0.0 && mid < 40.0 && busy);
        let (end, busy) = run(&ctx, 0.2, 0.3, |c| value(c, v, 40.0, FAST));
        assert!(end == 40.0 && !busy);
        // A new target mid-way starts from where the value is, not from the old target.
        let (back, _) = run(&ctx, 1.0, 0.03, |c| value(c, v, 0.0, FAST));
        assert!(back > 0.0 && back < 40.0);
    }

    #[test]
    fn about_scales_around_the_point() {
        assert_eq!(about(egui::pos2(10.0, 10.0), 0.5) * egui::pos2(20.0, 30.0), egui::pos2(15.0, 20.0));
    }

    #[test]
    fn easing_is_pinned_at_both_ends() {
        assert_eq!((ease(0.0), ease(1.0)), (0.0, 1.0));
        assert!(ease(0.5) > 0.5, "ease-out front-loads the move");
    }

    #[test]
    fn reduced_motion_zeroes_every_duration() {
        assert_eq!(animation_time(true), 0.0);
        assert_eq!(animation_time(false), FAST * scale());
        let ctx = Context::default();
        ctx.all_styles_mut(|s| s.animation_time = animation_time(false));
        assert_eq!(secs(&ctx, BASE), BASE * scale());
        ctx.all_styles_mut(|s| s.animation_time = 0.0);
        assert_eq!((secs(&ctx, BASE), opened(&ctx, LayerId::background(), BASE)), (0.0, 1.0));
    }

    #[test]
    fn a_float_grows_then_stops_asking_for_frames() {
        let ctx = Context::default();
        let layer = LayerId::new(Order::Foreground, Id::unique("float"));
        let show = |c: &Context| {
            egui::Area::new(layer.id).order(Order::Foreground).show(c, block);
            opened(c, layer, FAST)
        };
        let (mid, busy) = run(&ctx, 0.0, 0.06, show);
        assert!(mid > 0.0 && mid < 1.0 && busy);
        let (end, busy) = run(&ctx, 0.1, 0.5, show);
        assert!(end == 1.0 && !busy);
    }

    #[test]
    fn a_ghost_fades_for_half_a_base_then_is_dropped() {
        let ctx = Context::default();
        let layer = LayerId::new(Order::Foreground, Id::unique("modal"));
        run(&ctx, 0.0, 0.3, |c| {
            egui::Area::new(layer.id).order(Order::Foreground).show(c, block);
            leave(c, layer);
            end_pass(c);
            0.0
        });
        // Closed: nothing draws the layer any more.
        let (_, busy) = run(&ctx, 0.4, 0.05, |c| {
            end_pass(c);
            0.0
        });
        assert!(busy, "fading out");
        let (_, busy) = run(&ctx, 0.5, 0.3, |c| {
            end_pass(c);
            0.0
        });
        assert!(!busy, "dropped after BASE x OUT");
    }

    #[test]
    fn a_held_button_draws_at_97_percent() {
        use egui::{Event, PointerButton, RawInput};
        let ctx = Context::default();
        let at = egui::pos2(10.0, 10.0);
        let mut sizes = Vec::new();
        for (t, events) in [
            (0.0, vec![Event::PointerMoved(at)]),
            (0.1, vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Default::default() }]),
            (0.2, vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Default::default() }]),
        ] {
            // The widget needs a frame to be known before a press can hit it: run each twice.
            for _ in 0..2 {
                let mut out = ctx.run_ui(RawInput { time: Some(t), events: events.clone(), ..Default::default() }, |ui| {
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::click());
                    sizes.push(pressed(&resp, rect).width());
                });
                out.textures_delta.clear();
            }
        }
        assert_eq!((sizes[1], sizes[5]), (20.0, 20.0), "idle and released: full size");
        assert!(sizes.contains(&18.0), "held: a 20 px rect is 1 px smaller per side");
        assert_eq!(held(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::splat(100.0))).width(), 97.0, "a 100 px rect is 97 px");
    }

    #[test]
    fn a_pop_swells_then_rests_and_stops_asking_for_frames() {
        let ctx = Context::default();
        let id = Id::unique("pop");
        let (rest, _) = run(&ctx, 0.0, 0.0, |c| popped(c, id));
        assert_eq!(rest, 1.0);
        run(&ctx, 0.1, 0.0, |c| {
            pop(c, id);
            1.0
        });
        let (mid, busy) = run(&ctx, 0.15, 0.03, |c| popped(c, id));
        assert!(mid > 1.05 && mid <= POP && busy);
        let (end, busy) = run(&ctx, 0.4, 0.2, |c| popped(c, id));
        assert!(end == 1.0 && !busy);
        // Reduced motion: no pop at all.
        ctx.all_styles_mut(|s| s.animation_time = 0.0);
        run(&ctx, 1.0, 0.0, |c| {
            pop(c, id);
            1.0
        });
        assert_eq!(run(&ctx, 1.01, 0.0, |c| popped(c, id)).0, 1.0);
    }

    #[test]
    fn a_scaled_widget_keeps_its_layout_and_centre() {
        let ctx = Context::default();
        ctx.all_styles_mut(|s| s.animation_time = animation_time(false));
        let id = Id::unique("scaled");
        let mut rects = Vec::new();
        for _ in 0..3 {
            let mut out = ctx.run_ui(egui::RawInput { time: Some(0.0), ..Default::default() }, |ui| {
                let r = scaled(ui, id, 1.5, |ui| ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover()).1);
                rects.push(r.rect);
                assert_eq!(ui.min_rect().size(), egui::vec2(20.0, 20.0), "layout is untouched");
            });
            out.textures_delta.clear();
        }
        assert_eq!(rects[0].center(), rects[2].center());
    }

    #[test]
    fn the_os_setting_reads_without_panicking() {
        let _ = os_reduced();
    }

    #[test]
    fn cards_stagger_in_only_after_a_skeleton() {
        let ctx = Context::default();
        let id = Id::unique("stagger");
        // Never a skeleton: at rest at once.
        assert_eq!(run(&ctx, 0.0, 0.0, |c| stagger(c, id, true, 0)).0, 1.0);
        run(&ctx, 0.1, 0.0, |c| stagger(c, id, false, 0));
        let first = run(&ctx, 0.2, 0.0, |c| stagger(c, id, true, 0)).0;
        let (later, busy) = run(&ctx, 0.2, 0.02, |c| stagger(c, id, true, 3));
        assert!(first < 1.0 && later == 0.0 && busy, "card 3 waits for its turn");
        let (end, busy) = run(&ctx, 0.3, 0.8, |c| stagger(c, id, true, 3));
        assert!(end == 1.0 && !busy);
    }
}
