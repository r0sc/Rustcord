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

/// Seconds since the area on `layer` became visible (egui's own record), in plain durations (the
/// test scale divided out); INFINITY with reduced motion; asks for frames while under `until`.
pub fn open_for(ctx: &Context, layer: LayerId, until: f32) -> f32 {
    if secs(ctx, until) == 0.0 {
        return f32::INFINITY;
    }
    let (now, half) = ctx.input(|i| (i.time, i.predicted_dt * 0.5));
    let shown = ctx.memory(|m| m.areas().visible_last_frame(&layer));
    let at = AreaState::load(ctx, layer.id).and_then(|a| a.last_became_visible_at).filter(|_| shown);
    let plain = (at.map_or(0.0, |at| (now - at) as f32) + half) / scale();
    if plain < until {
        ctx.request_repaint();
    }
    plain
}

/// Eased 0 -> 1 over `s` s since the area on `layer` became visible (egui's own record), for
/// floats drawn only while open. Before or after its `show`.
pub fn opened(ctx: &Context, layer: LayerId, s: f32) -> f32 {
    ease((open_for(ctx, layer, s) / s).min(1.0))
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

/// Linear 0 -> 1 over `s` since `pop(ctx, id)` stamped `id`; None at rest, once done (the stamp is
/// dropped) and with reduced motion. One-shots: the pop, the island's connect ring, a digit roll.
pub fn shot(ctx: &Context, id: Id, s: f32) -> Option<f32> {
    let at = ctx.data(|d| d.get_temp::<f64>(id))?;
    let dur = secs(ctx, s);
    let lin = (ctx.input(|i| i.time) - at) as f32 / dur;
    if dur == 0.0 || lin >= 1.0 {
        ctx.data_mut(|d| d.remove::<f64>(id));
        return None;
    }
    ctx.request_repaint();
    Some(lin)
}

/// The scale of the pop under `id` right now: 1 at rest (and with reduced motion), asking for
/// frames only while it runs.
pub fn popped(ctx: &Context, id: Id) -> f32 {
    shot(ctx, id, BASE).map_or(1.0, |t| 1.0 + (POP - 1.0) * (t * std::f32::consts::PI).sin())
}

/// Row hover (DESIGN §7): on at once, off over FAST along the ease-in exit, so a pointer moving
/// down a list leaves a short trail. Frames only while it fades.
#[allow(dead_code)] // the lanes use these
pub fn trail(ctx: &Context, id: Id, hot: bool) -> f32 {
    anim(ctx, id, hot, 0.0, FAST)
}

/// Eased 0 -> 1 over `s` since `key` last changed under `id`: content that replaced other content
/// (a switched list, the hover toolbar on another message). The first key seen is at rest (1); 1
/// with reduced motion; frames only while it runs.
pub fn arrive(ctx: &Context, id: Id, key: u64, s: f32) -> f32 {
    let now = ctx.input(|i| i.time);
    let at = ctx.data_mut(|d| {
        let e = d.get_temp_mut_or(id, (key, f64::NEG_INFINITY));
        if e.0 != key {
            *e = (key, now);
        }
        e.1
    });
    since(ctx, at, s)
}

/// `add` as content that arrives: at opacity `t`, painted `dy` lower (paint only: layout, ids and
/// clicks are where it settles). Always one scope, so ids do not change when an arrival starts.
#[allow(dead_code)]
pub fn arriving<R>(ui: &mut Ui, (t, dy): (f32, f32), add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope(|ui| {
        if t < 1.0 {
            ui.multiply_opacity(t);
        }
        shifted(ui, dy, None, add)
    })
    .inner
}

/// When the house spring has settled (`springy`).
#[allow(dead_code)]
pub const SPRING_T: f32 = 0.25;
/// The house spring's step response (damping 0.72, period 0.18 s): 0 -> 1 with a 3.8 % overshoot
/// at 130 ms, pinned to 1 from SPRING_T on (it is 0.9986 there). The profile card springs on it.
#[allow(dead_code)]
pub fn springy(t: f32) -> f32 {
    if t >= SPRING_T {
        return 1.0;
    }
    let (w, z) = (std::f32::consts::TAU / 0.18, 0.72f32);
    let wd = w * (1.0 - z * z).sqrt();
    1.0 - (-z * w * t).exp() * ((wd * t).cos() + z * w / wd * (wd * t).sin())
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

/// Eased 0 -> 1 over `s` s since `at` (ctx time); 1 with reduced motion and once done; asks for
/// frames until then.
pub fn since(ctx: &Context, at: f64, s: f32) -> f32 {
    let dur = secs(ctx, s);
    let lin = if dur > 0.0 { (ctx.input(|i| i.time) - at) as f32 / dur } else { 1.0 };
    if lin >= 1.0 {
        return 1.0;
    }
    ctx.request_repaint();
    ease(lin.max(0.0))
}

/// Drops `value`'s tween under `id`: its next call starts at its target, no glide from a stale
/// spot (a card opened afresh, an island width that only the call timer's tick changed).
pub fn forget(ctx: &Context, id: Id) {
    ctx.data_mut(|d| d.remove::<Tween>(id));
}

/// `add` painted `dy` lower without a child Ui (layout, widget ids and input unchanged). Its shapes
/// stay clipped to `ui`'s clip (egui's `with_visual_transform` moves the clip with them, which
/// would paint list rows over a header) and, with `clip_top`, to below that screen y.
pub fn shifted<R>(ui: &mut Ui, dy: f32, clip_top: Option<f32>, add: impl FnOnce(&mut Ui) -> R) -> R {
    if dy == 0.0 && clip_top.is_none() {
        return add(ui);
    }
    let (layer, mut clip) = (ui.layer_id(), ui.clip_rect());
    if let Some(y) = clip_top {
        clip.min.y = clip.min.y.max(y); // past max.y: nothing shows
    }
    let start = ui.ctx().graphics(|g| g.get(layer).map_or(0, |l| l.next_idx().0));
    let out = add(ui);
    let t = TSTransform::from_translation(egui::vec2(0.0, dy));
    ui.ctx().graphics_mut(|g| {
        let list = g.entry(layer);
        for i in start..list.next_idx().0 {
            list.mutate_shape(egui::layers::ShapeIdx(i), |c| {
                let keep = c.clip_rect; // transform() moves the clip with the shape
                c.transform(t);
                c.clip_rect = keep.intersect(clip);
            });
        }
    });
    out
}

/// What `add` paints, scaled by `s` about `o` (no child Ui: layout and widget ids are untouched).
pub fn painted_scaled(ui: &Ui, o: Pos2, s: f32, add: impl FnOnce()) {
    let layer = ui.layer_id();
    let start = ui.ctx().graphics(|g| g.get(layer).map_or(0, |l| l.next_idx().0));
    add();
    ui.ctx().graphics_mut(|g| {
        let list = g.entry(layer);
        let end = list.next_idx();
        list.transform_range(egui::layers::ShapeIdx(start), end, about(o, s));
    });
}

/// Paint-only folding for a list whose sections open and shut (server folders, channel
/// categories; DESIGN §7). The caller keeps a closing section laid out until its k reaches 0, so
/// layout, hit-testing and drop targets never slide. Consecutive rows of one folding section form
/// a run that slides out from under the row above it (clipped there) by (1 - k) of its height;
/// every later row is drawn that much higher. A run's height is last pass's (a run never drawn
/// before is hidden for its first pass, when k is still ~0).
#[derive(Default)]
pub struct Fold {
    shift: f32,
    run: Option<Run>,
}

struct Run {
    key: Id,
    clip: f32,
    k: f32,
    h: f32,
}

impl Fold {
    /// One row; `folds`: (the row's own key, its section's k) while that section is laid out
    /// (also at k == 1, so its height is known when it starts closing), else None.
    pub fn row<R>(&mut self, ui: &mut Ui, folds: Option<(Id, f32)>, add: impl FnOnce(&mut Ui) -> R) -> R {
        let Some((key, k)) = folds else {
            self.end(ui.ctx());
            return shifted(ui, -self.shift, None, add);
        };
        let top = ui.cursor().top();
        if self.run.as_ref().is_none_or(|r| r.k != k) {
            self.end(ui.ctx());
            self.run = Some(Run { key, clip: top - self.shift, k, h: 0.0 });
        }
        let (run_key, clip) = self.run.as_ref().map_or((key, top), |r| (r.key, r.clip));
        let known = ui.ctx().data(|d| d.get_temp::<f32>(run_key));
        let out = match known {
            _ if k >= 1.0 => shifted(ui, -self.shift, None, add),
            Some(h) => shifted(ui, -(self.shift + (1.0 - k) * h), Some(clip), add),
            None => shifted(ui, -self.shift, Some(ui.clip_rect().max.y), add),
        };
        let bottom = ui.cursor().top();
        if let Some(r) = &mut self.run {
            r.h += bottom - top; // the row and its item spacing
        }
        out
    }

    /// Ends the current run: later rows move up by what it hides; its height is kept for next pass.
    pub fn end(&mut self, ctx: &Context) {
        if let Some(r) = self.run.take() {
            self.shift += (1.0 - r.k) * r.h;
            ctx.data_mut(|d| d.insert_temp(r.key, r.h));
        }
    }

    /// How much higher the next row is drawn.
    pub fn shift(&self) -> f32 {
        self.shift
    }
}

/// Puts `value`'s tween under `id` at `v` at once; the next `value` call eases from there to its
/// target (a dragged tab follows the pointer, then settles into its slot).
pub fn set_value(ctx: &Context, id: Id, v: f32) {
    let now = ctx.input(|i| i.time);
    ctx.data_mut(|d| d.insert_temp(id, Tween { from: v, to: v, at: now }));
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

fn origin_id() -> Id {
    Id::unique("rc_modal_origin")
}

/// The next modal grows from `pos` (its trigger), also when a key opened it.
pub fn modal_origin(ctx: &Context, pos: Pos2) {
    ctx.data_mut(|d| d.insert_temp(origin_id(), pos));
}

/// egui's Modal with Rustcord's motion (principles 5, 11, 18): the scrim fades in; the panel
/// fades in and grows GROW -> 1 over BASE from its trigger (`modal_origin`), else the click that
/// opened it, else the window centre; closed, its last frame fades out (`leave`). `area` /
/// `frame` / `scrim` as before.
pub fn modal<R>(ctx: &Context, area: egui::Area, frame: egui::Frame, scrim: Color32, add: impl FnOnce(&mut Ui) -> R) -> egui::ModalResponse<R> {
    let layer = area.layer();
    let t = opened(ctx, layer, BASE);
    let from_id = layer.id.with("rc_from");
    let from = if ctx.memory(|m| m.areas().visible_last_frame(&layer)) {
        ctx.data(|d| d.get_temp::<Pos2>(from_id))
    } else {
        let origin = ctx.data_mut(|d| d.remove_temp::<Pos2>(origin_id()));
        let click = origin.or_else(|| ctx.input(|i| i.pointer.any_click().then(|| i.pointer.interact_pos()).flatten()));
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

/// Last call of every pass (`RustcordApp::ui`). Every float (each Foreground / Tooltip area) gets
/// its contact shadow (`messages::contact_shadows`). Menus, context menus, submenus, tooltips and
/// popovers not `own`ed grow GROW -> 1 about their pivot (the point egui attached them to the
/// trigger; the click for context menus) over FAST while egui fades them in over the same time;
/// they close at once (principle 20). Then the `leave` ghosts.
pub fn end_pass(ctx: &Context) {
    let own = ctx.data_mut(|d| std::mem::take(d.get_temp_mut_or_default::<Own>(own_id())));
    let grow = secs(ctx, FAST) > 0.0;
    for layer in ctx.memory(|m| m.areas().visible_layer_ids()) {
        if !matches!(layer.order, Order::Foreground | Order::Tooltip) {
            continue;
        }
        super::messages::contact_shadows(ctx, layer);
        if !grow || own.0.contains(&layer) {
            continue;
        }
        let t = opened(ctx, layer, FAST);
        if t < 1.0
            && let Some(o) = AreaState::load(ctx, layer.id).and_then(|a| a.pivot_pos)
        {
            ctx.transform_layer_shapes_after_rounding(layer, about(o, egui::lerp(GROW..=1.0, t)));
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
    fn forget_makes_the_next_value_land_at_once() {
        let ctx = Context::default();
        let v = Id::unique("forget");
        run(&ctx, 0.0, 0.0, |c| value(c, v, 0.0, FAST));
        let (mid, busy) = run(&ctx, 0.1, 0.05, |c| value(c, v, 40.0, FAST));
        assert!(mid > 0.0 && mid < 40.0 && busy);
        forget(&ctx, v);
        let (now, busy) = run(&ctx, 0.3, 0.05, |c| value(c, v, 10.0, FAST));
        assert!(now == 10.0 && !busy);
    }

    /// The rect of the shape filled `c` and its clip rect, from a pass's output.
    fn painted(out: &egui::FullOutput, c: Color32) -> (egui::Rect, egui::Rect) {
        out.shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Rect(r) if r.fill == c => Some((r.rect, s.clip_rect)),
                _ => None,
            })
            .expect("the block was painted")
    }

    fn tile(ui: &mut Ui, c: Color32) {
        let (r, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
        ui.painter().rect_filled(r, 0.0, c);
    }

    #[test]
    fn shifted_moves_paint_not_layout() {
        let ctx = Context::default();
        let (mut top, mut used) = (0.0, 0.0);
        let mut out = ctx.run_ui(egui::RawInput { time: Some(0.0), ..Default::default() }, |ui| {
            top = ui.cursor().top();
            shifted(ui, -10.0, Some(top + 4.0), |ui| tile(ui, Color32::RED));
            used = ui.cursor().top() - top;
        });
        out.textures_delta.clear();
        let (rect, clip) = painted(&out, Color32::RED);
        assert_eq!(rect.top(), top - 10.0, "painted 10 px higher");
        assert_eq!(clip.top(), top + 4.0, "clipped to below the row above");
        assert!(used >= 20.0, "layout took the row's own room");
    }

    #[test]
    fn shifted_keeps_rows_moved_past_the_clip() {
        let ctx = Context::default();
        let mut top = 0.0;
        let mut out = ctx.run_ui(egui::RawInput { time: Some(0.0), ..Default::default() }, |ui| {
            top = ui.cursor().top();
            shifted(ui, -500.0, Some(top - 1000.0), |ui| tile(ui, Color32::RED));
        });
        out.textures_delta.clear();
        let (_, clip) = painted(&out, Color32::RED);
        assert!(clip.height() > 0.0, "the clip stays inside the ui's, not moved off with the shape");
    }

    #[test]
    fn a_folding_run_slides_and_later_rows_follow() {
        let ctx = Context::default();
        let key = Id::unique("fold_run");
        let pass = |t: f64, k: f32| {
            let (mut top, mut shift) = (0.0, 0.0);
            let mut out = ctx.run_ui(egui::RawInput { time: Some(t), ..Default::default() }, |ui| {
                top = ui.cursor().top();
                let mut fold = Fold::default();
                fold.row(ui, None, |ui| tile(ui, Color32::RED));
                fold.row(ui, Some((key, k)), |ui| tile(ui, Color32::GREEN));
                fold.row(ui, Some((key, k)), |ui| tile(ui, Color32::BLUE));
                fold.row(ui, None, |ui| tile(ui, Color32::YELLOW));
                fold.end(ui.ctx());
                shift = fold.shift();
            });
            out.textures_delta.clear();
            (out, top, shift)
        };
        let (_, top, shift) = pass(0.0, 0.5);
        let h = ctx.data(|d| d.get_temp::<f32>(key)).expect("the run's height is kept");
        assert_eq!(shift, 0.5 * h);
        let (out, _, _) = pass(0.1, 0.5);
        let row = h / 2.0; // two rows of one pitch
        let (yellow, _) = painted(&out, Color32::YELLOW);
        assert_eq!(yellow.top(), top + 3.0 * row - 0.5 * h, "a later row is drawn the hidden part higher");
        let (green, clip) = painted(&out, Color32::GREEN);
        assert_eq!(clip.top(), top + row, "the run slides out from under the row above");
        assert_eq!(green.top(), top + row - 0.5 * h);
        let (out, _, shift) = pass(0.2, 1.0);
        assert_eq!(shift, 0.0);
        assert_eq!(painted(&out, Color32::YELLOW).0.top(), top + 3.0 * row, "open: nothing moved");
        assert_eq!(painted(&out, Color32::GREEN).0.top(), top + row);
    }

    #[test]
    fn since_runs_once_and_rests() {
        let ctx = Context::default();
        let (start, busy) = run(&ctx, 1.0, 0.0, |c| since(c, 1.0, FAST));
        assert!(start == 0.0 && busy);
        let (mid, busy) = run(&ctx, 1.05, 0.0, |c| since(c, 1.0, FAST));
        assert!(mid > 0.0 && mid < 1.0 && busy);
        let (end, busy) = run(&ctx, 1.3, 0.1, |c| since(c, 1.0, FAST));
        assert!(end == 1.0 && !busy);
        ctx.all_styles_mut(|s| s.animation_time = 0.0);
        assert_eq!(run(&ctx, 2.0, 0.0, |c| since(c, 2.0, FAST)).0, 1.0, "reduced motion: at once");
    }

    #[test]
    fn set_value_moves_at_once_and_value_eases_on_from_there() {
        let ctx = Context::default();
        let v = Id::unique("set_value");
        run(&ctx, 0.0, 0.0, |c| value(c, v, 0.0, FAST));
        run(&ctx, 0.1, 0.0, |c| {
            set_value(c, v, 30.0);
            0.0
        });
        let (mid, busy) = run(&ctx, 0.2, 0.05, |c| value(c, v, 50.0, FAST));
        assert!((30.0..50.0).contains(&mid) && busy);
        let (end, busy) = run(&ctx, 0.4, 0.3, |c| value(c, v, 50.0, FAST));
        assert!(end == 50.0 && !busy);
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
        assert_eq!(open_for(&ctx, LayerId::background(), BASE), f32::INFINITY);
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
    fn a_modal_grows_from_its_trigger_once() {
        let ctx = Context::default();
        let area = || egui::Area::new(Id::unique("rc_test_modal"));
        let from = |c: &Context| c.data(|d| d.get_temp::<Pos2>(area().layer().id.with("rc_from")));
        let p = Pos2::new(40.0, 60.0);
        run(&ctx, 0.0, 0.0, |c| {
            modal_origin(c, p);
            modal(c, area(), egui::Frame::NONE, Color32::BLACK, block);
            0.0
        });
        assert_eq!(from(&ctx), Some(p), "opened by a key: grows from the trigger, not the centre");
        assert_eq!(ctx.data(|d| d.get_temp::<Pos2>(origin_id())), None, "used up");
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
    fn trail_is_instant_on_and_fades_out() {
        let ctx = Context::default();
        let id = Id::unique("trail");
        run(&ctx, 0.0, 0.0, |c| trail(c, id, false));
        let (on, _) = run(&ctx, 0.1, 0.0, |c| trail(c, id, true));
        assert_eq!(on, 1.0);
        let (mid, busy) = run(&ctx, 0.1 + 1.0 / 60.0, 0.03, |c| trail(c, id, false));
        assert!(mid > 0.0 && mid < 1.0 && busy);
        let (end, busy) = run(&ctx, 0.3, 0.3, |c| trail(c, id, false));
        assert!(end == 0.0 && !busy);
    }

    #[test]
    fn arrive_rests_first_runs_on_a_new_key_and_stops() {
        let ctx = Context::default();
        let id = Id::unique("arrive");
        assert_eq!(run(&ctx, 0.0, 0.0, |c| arrive(c, id, 1, BASE)).0, 1.0);
        let (mid, busy) = run(&ctx, 0.1, 0.05, |c| arrive(c, id, 2, BASE));
        assert!(mid < 1.0 && busy);
        let (end, busy) = run(&ctx, 0.2, 0.5, |c| arrive(c, id, 2, BASE));
        assert!(end == 1.0 && !busy);
        ctx.all_styles_mut(|s| s.animation_time = 0.0);
        assert_eq!(run(&ctx, 1.0, 0.0, |c| arrive(c, id, 3, BASE)).0, 1.0);
    }

    #[test]
    fn a_shot_runs_once_then_is_none() {
        let ctx = Context::default();
        let id = Id::unique("shot");
        let some = |c: &Context| shot(c, id, BASE).unwrap_or(-1.0);
        assert_eq!(run(&ctx, 0.0, 0.0, some).0, -1.0);
        run(&ctx, 0.1, 0.0, |c| {
            pop(c, id);
            1.0
        });
        let (mid, busy) = run(&ctx, 0.15, 0.05, some);
        assert!(mid > 0.0 && mid < 1.0 && busy);
        let (end, busy) = run(&ctx, 0.4, 0.2, some);
        assert!(end == -1.0 && !busy);
        ctx.all_styles_mut(|s| s.animation_time = 0.0);
        run(&ctx, 1.0, 0.0, |c| {
            pop(c, id);
            1.0
        });
        assert_eq!(run(&ctx, 1.01, 0.0, some).0, -1.0);
    }

    #[test]
    fn springy_overshoots_a_little_and_is_pinned() {
        assert_eq!(springy(0.0), 0.0);
        let peak = (0..250).map(|i| springy(i as f32 * 0.001)).fold(0.0, f32::max);
        assert!(peak > 1.0 && peak <= 1.05, "{peak}");
        assert_eq!(springy(SPRING_T), 1.0);
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
