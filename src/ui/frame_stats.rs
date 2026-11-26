//! Test hook `RUSTCORD_FRAME_STATS=1` (docs/ARCHITECTURE.md): every 2 s one stderr line with the
//! frames drawn, eframe's time per frame (`cpu_usage`: our passes + egui + tessellation + GL
//! paint, without the vsync wait), our own time (`logic` + `ui`), one sampled tessellation of the
//! current shapes and the sections timed with `scope` / `time` in ms per frame. Sections named
//! `+name` run inside another one (markdown inside chat …) and are left out of `unscoped`.
//! `RUSTCORD_FRAME_STATS=continuous` also repaints every frame (cost of a full repaint).
//! Off, every call is one cached check.
//! Settings > Advanced > Diagnostics (plugin ClientDiagnostics) uses the same numbers live:
//! `keep_live` while that section is drawn makes `end` keep each 2 s window for `last`.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const EVERY: Duration = Duration::from_secs(2);

/// 0 off, 1 on, 2 continuous.
fn mode() -> u8 {
    static MODE: OnceLock<u8> = OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("RUSTCORD_FRAME_STATS").as_deref() {
        Ok("continuous") => 2,
        Ok("" | "0") | Err(_) => 0,
        Ok(_) => 1,
    })
}

/// Passes left in which the live view (the Diagnostics section) counts as shown; each draw of
/// the section sets it to 2, each pass takes one off, so leaving the page ends the timing.
static LIVE: AtomicU8 = AtomicU8::new(0);
static LAST: Mutex<Option<Window>> = Mutex::new(None);

fn active() -> bool {
    mode() != 0 || LIVE.load(Ordering::Relaxed) > 0
}

/// The Diagnostics section calls this every time it is drawn.
pub fn keep_live() {
    LIVE.store(2, Ordering::Relaxed);
}

/// A finished 2 s window: what the live view shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Window {
    pub frames: usize,
    pub secs: f32,
    /// (mean, p95) in ms: eframe's time per frame and ours.
    pub frame_ms: (f32, f32),
    pub app_ms: (f32, f32),
    /// (section, ms per frame), in first-seen order.
    pub sections: Vec<(&'static str, f32)>,
}

/// The newest finished window, while the live view is shown.
pub fn last() -> Option<Window> {
    LAST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn window(s: &mut Stats) -> Window {
    let n = s.app_ms.len().max(1) as f32;
    Window {
        frames: s.app_ms.len(),
        secs: s.since.map_or(0.0, |t| t.elapsed().as_secs_f32()),
        frame_ms: mean_p95(&mut s.frame_ms),
        app_ms: mean_p95(&mut s.app_ms),
        sections: s.sections.iter().map(|(name, total, _)| (*name, total.as_secs_f32() * 1000.0 / n)).collect(),
    }
}

#[derive(Default)]
struct Stats {
    since: Option<Instant>,
    pass: Option<Instant>,
    frame_nr: u64,
    /// eframe's time per frame and ours (ms).
    frame_ms: Vec<f32>,
    app_ms: Vec<f32>,
    /// (name, total, calls), in first-seen order.
    sections: Vec<(&'static str, Duration, u32)>,
}

thread_local! {
    static STATS: RefCell<Stats> = RefCell::default();
}

pub struct Scope(&'static str, Instant);

impl Drop for Scope {
    fn drop(&mut self) {
        let d = self.1.elapsed();
        STATS.with_borrow_mut(|s| match s.sections.iter_mut().find(|e| e.0 == self.0) {
            Some(e) => (e.1, e.2) = (e.1 + d, e.2 + 1),
            None => s.sections.push((self.0, d, 1)),
        });
    }
}

/// Times the rest of the enclosing block as section `name` (nothing with the hook off).
pub fn scope(name: &'static str) -> Option<Scope> {
    active().then(|| Scope(name, Instant::now()))
}

/// `f` timed as section `name`.
pub fn time<R>(name: &'static str, f: impl FnOnce() -> R) -> R {
    let _t = scope(name);
    f()
}

/// Start of a pass (`App::logic`). `last`: eframe's time of the previous frame.
pub fn begin(ctx: &egui::Context, last: Option<f32>) {
    if !active() {
        return;
    }
    STATS.with_borrow_mut(|s| {
        if let Some(t) = last.filter(|_| ctx.cumulative_frame_nr() != s.frame_nr) {
            s.frame_ms.push(t * 1000.0);
        }
        s.pass = Some(Instant::now());
    });
}

/// End of a pass (`App::ui`): our time, the report every 2 s, the continuous repaint.
pub fn end(ctx: &egui::Context) {
    if !active() {
        return;
    }
    let live = LIVE.load(Ordering::Relaxed) > 0;
    let nr = ctx.cumulative_frame_nr();
    let done = STATS.with_borrow_mut(|s| {
        let app = s.pass.take().map_or(0.0, |t| t.elapsed().as_secs_f32() * 1000.0);
        match s.app_ms.last_mut().filter(|_| nr == s.frame_nr) {
            // A second pass of the same frame (`request_discard`).
            Some(a) => *a += app,
            None => s.app_ms.push(app),
        }
        s.frame_nr = nr;
        let since = *s.since.get_or_insert_with(Instant::now);
        (since.elapsed() >= EVERY).then(|| std::mem::replace(s, Stats { frame_nr: nr, ..Stats::default() }))
    });
    if let Some(mut s) = done {
        if live {
            *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(window(&mut s));
        }
        if mode() != 0 {
            eprintln!("{}", report(ctx, s));
        }
    }
    // The live view was not drawn in the last two passes: forget it, so nothing is timed or kept.
    if live && LIVE.fetch_sub(1, Ordering::Relaxed) == 1 {
        *LAST.lock().unwrap_or_else(|e| e.into_inner()) = None;
        if mode() == 0 {
            STATS.with_borrow_mut(|s| *s = Stats::default());
        }
    }
    if mode() == 2 {
        ctx.request_repaint();
    }
}

fn mean_p95(v: &mut [f32]) -> (f32, f32) {
    if v.is_empty() {
        return (0.0, 0.0);
    }
    v.sort_by(f32::total_cmp);
    (v.iter().sum::<f32>() / v.len() as f32, v[(v.len() * 95 / 100).min(v.len() - 1)])
}

fn report(ctx: &egui::Context, mut s: Stats) -> String {
    let secs = s.since.map_or(0.0, |t| t.elapsed().as_secs_f32());
    let n = s.app_ms.len().max(1) as f32;
    let ((fm, fp), (am, ap)) = (mean_p95(&mut s.frame_ms), mean_p95(&mut s.app_ms));
    let (tess, shapes, prims, verts) = tessellate(ctx);
    let mut out = format!(
        "frame-stats: {} frames in {secs:.1} s | frame {fm:.2} ms (p95 {fp:.2}) | app {am:.2} (p95 {ap:.2}) | tess {tess:.2} ({shapes} shapes, {prims} prims, {verts} vtx) | rest {:.2} |",
        s.app_ms.len(),
        fm - am - tess
    );
    let mut scoped = 0.0;
    for (name, total, calls) in &s.sections {
        let ms = total.as_secs_f32() * 1000.0 / n;
        if !name.starts_with('+') {
            scoped += ms;
        }
        let _ = write!(out, " {name} {ms:.2}");
        if *calls as f32 >= 1.5 * n {
            let _ = write!(out, " x{:.0}", *calls as f32 / n);
        }
        out.push(',');
    }
    let _ = write!(out, " unscoped {:.2}", am - scoped);
    out
}

/// One tessellation of the shapes painted so far this pass: (ms, shapes, clip primitives, vertices).
fn tessellate(ctx: &egui::Context) -> (f32, usize, usize, usize) {
    let mut layers: Vec<egui::LayerId> = ctx.memory(|m| m.layer_ids().collect());
    if !layers.contains(&egui::LayerId::background()) {
        layers.push(egui::LayerId::background());
    }
    let shapes: Vec<egui::epaint::ClippedShape> = ctx.graphics(|g| layers.iter().filter_map(|l| g.get(*l)).flat_map(|l| l.all_entries().cloned()).collect());
    let count = shapes.len();
    let t = Instant::now();
    let prims = ctx.tessellate(shapes, ctx.pixels_per_point());
    let ms = t.elapsed().as_secs_f32() * 1000.0;
    let verts = prims.iter().map(|p| if let egui::epaint::Primitive::Mesh(m) = &p.primitive { m.vertices.len() } else { 0 }).sum();
    (ms, count, prims.len(), verts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_has_the_means_and_ms_per_frame() {
        let mut s = Stats {
            since: Some(Instant::now()),
            frame_ms: vec![2.0, 4.0],
            app_ms: vec![1.0, 3.0],
            sections: vec![("chat", Duration::from_millis(20), 2), ("+markdown", Duration::from_millis(4), 2)],
            ..Stats::default()
        };
        let w = window(&mut s);
        assert_eq!((w.frames, w.frame_ms, w.app_ms), (2, (3.0, 4.0), (2.0, 3.0)));
        assert_eq!(w.sections, [("chat", 10.0), ("+markdown", 2.0)]);
    }

    #[test]
    fn live_off_keeps_nothing() {
        let ctx = egui::Context::default();
        assert_eq!(LIVE.load(Ordering::Relaxed), 0);
        begin(&ctx, Some(0.01));
        end(&ctx);
        assert_eq!(last(), None);
        assert!(STATS.with_borrow(|s| s.app_ms.is_empty() && s.since.is_none()));
    }

    #[test]
    fn mean_and_p95() {
        let mut v: Vec<f32> = (1..=20).rev().map(|i| i as f32).collect();
        assert_eq!(super::mean_p95(&mut v), (10.5, 20.0));
        assert_eq!(super::mean_p95(&mut []), (0.0, 0.0));
    }
}
