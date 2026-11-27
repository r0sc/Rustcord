//! UI-side image cache: URL -> GPU textures (all frames for animations), request-on-miss
//! via `Command::LoadImage`, frame selection by time, LRU eviction under a memory budget,
//! and avatar/image widgets. A picture that arrives late fades in over its placeholder (`fade`).
//! OWNERSHIP: cache implementer (together with cache/images.rs).

use crate::backend::{Command, CommandSender, DecodedImage, ImageFrame};
use crate::model::{Id, Member, User};
use egui::load::SizedTexture;
use egui::{Sense, TextureHandle, Vec2};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// A texture costs ~2.7x its size in process memory (GL driver copies; measured with
/// `RUSTCORD_MEMTEST=textures:<MB>`), so this stays small for the 150 MB RAM target.
const TEXTURE_BUDGET: usize = 12 * 1024 * 1024;
/// Entries drawn within this many seconds are never evicted.
const KEEP_RECENT_SECS: f64 = 2.0;
/// Entries not drawn for this long are dropped even under budget (the disk cache makes a
/// revisit cheap), so idle memory comes back down after browsing image-heavy channels.
const IDLE_EVICT_SECS: f64 = 60.0;
const RETRY_FAILED_SECS: f64 = 60.0;
/// A picture that takes longer than this after it was asked for fades in (DESIGN §7); a disk-cache
/// hit is quicker and shows at once.
const LATE: Duration = Duration::from_millis(100);

/// Test hook `RUSTCORD_IMAGE_LATE=1`: every arrival counts as late (offline the fixture's pictures come from disk).
fn late_hook() -> bool {
    static S: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *S.get_or_init(|| std::env::var_os("RUSTCORD_IMAGE_LATE").is_some())
}
/// After startup / a (re)connect: the snapshot or READY temporaries are gone by then.
const TRIM_DELAY_SECS: f64 = 3.0;

enum State {
    Loading,
    Ready { frames: Vec<(TextureHandle, u32)>, total_ms: u32, bytes: usize, size: Vec2, stream: Option<Stream> },
    Failed { at: f64 },
}

/// A streamed animation (`DecodedImage::stream`): its one texture shows the next frame once the shown one is due;
/// frames are pulled only while drawn with `animate`, so a hidden card's decoder waits.
struct Stream {
    rx: std::sync::mpsc::Receiver<(u32, ImageFrame)>,
    rewind: std::sync::Arc<std::sync::atomic::AtomicU32>,
    /// The decoder pass whose frames show; frames of an older one (from before a rewind) are dropped.
    pass: u32,
    due: Option<f64>,
    delay: f64,
}

impl Stream {
    /// Advances `tex` when due; when to look again (None: the decoder is gone, the frame stays).
    fn pull(&mut self, tex: &mut TextureHandle, now: f64) -> Option<f64> {
        let due = *self.due.get_or_insert(now + self.delay);
        if now < due {
            return Some(due);
        }
        let next = match self.rx.try_recv() {
            Ok((pass, _)) if pass != self.pass => return self.pull(tex, now),
            Ok((_, f)) => {
                tex.set(f.image, egui::TextureOptions::LINEAR);
                self.delay = f64::from(f.delay_ms) / 1000.0;
                // More than a frame late (the card was hidden): the clock restarts now.
                (if now - due > self.delay { now } else { due }) + self.delay
            }
            // Decoder behind: look again soon, not a whole frame later (this frame would show twice as long).
            Err(std::sync::mpsc::TryRecvError::Empty) => now + (self.delay / 4.0).max(0.004),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => return None,
        };
        self.due = Some(next);
        Some(next)
    }

    /// From the first frame again, shown as soon as the decoder has it.
    fn rewind(&mut self) {
        self.pass = self.rewind.fetch_add(1, std::sync::atomic::Ordering::Relaxed).wrapping_add(1);
        self.due = Some(f64::NEG_INFINITY);
    }
}

struct Entry {
    state: State,
    /// `ctx.input(|i| i.time)` of the last frame this entry was drawn.
    last_used: f64,
    /// `max_side` of the last load sent; drawn bigger, a texture that load capped is loaded again.
    side: u32,
    /// When the load was asked for (a miss or a retry).
    asked: Instant,
    /// It arrived later than `LATE` after that and has not finished fading in: `shown_at` is the
    /// ctx time of the first frame that drew it.
    late: bool,
    shown_at: Option<f64>,
}

pub struct ImageCache {
    cmd: CommandSender,
    entries: HashMap<String, Entry>,
    /// Texture bytes of all Ready entries.
    bytes: usize,
    /// Earliest time (ctx time, seconds) an animation drawn this frame needs its next frame.
    next_frame_at: Option<f64>,
    /// Next `mem_hooks` report (ctx time); 0 = hooks not initialized yet.
    mem_next: f64,
    /// No Ready entry turns idle (IDLE_EVICT_SECS) before this ctx time.
    sweep_at: f64,
    /// When `mem_hooks` calls `release_free_memory` next (ctx time; infinite = not scheduled).
    trim_at: f64,
    was_connected: bool,
}

impl ImageCache {
    pub fn new(cmd: CommandSender) -> Self {
        Self { cmd, entries: HashMap::new(), bytes: 0, next_frame_at: None, mem_next: 0.0, sweep_at: f64::INFINITY, trim_at: f64::INFINITY, was_connected: false }
    }

    pub fn on_loaded(&mut self, ctx: &egui::Context, url: String, result: Result<DecodedImage, String>) {
        // Ignore results for entries removed/cleared (e.g. logout) while loading.
        let Some(entry) = self.entries.get_mut(&url) else { return };
        let old = match entry.state {
            State::Loading => 0,
            // A reload (`get`, drawn bigger) replaces the texture only with a sharper one.
            State::Ready { size, bytes, .. } => match &result {
                Ok(img) if img.frames.first().is_some_and(|f| f.image.size.into_iter().max().unwrap_or(0) as f32 > size.max_elem()) => bytes,
                _ => return,
            },
            State::Failed { .. } => return,
        };
        let first = matches!(entry.state, State::Loading);
        entry.state = match result {
            Ok(img) if !img.frames.is_empty() => upload(ctx, &url, img),
            _ => State::Failed { at: now(ctx) },
        };
        // Only the first arrival fades: a sharper reload replaces a picture already shown.
        entry.late = first && matches!(entry.state, State::Ready { .. }) && (entry.asked.elapsed() > LATE || late_hook());
        self.bytes -= old;
        if let State::Ready { bytes, .. } = entry.state {
            self.bytes += bytes;
            self.sweep_at = self.sweep_at.min(entry.last_used + IDLE_EVICT_SECS);
        }
    }

    /// Texture for `url` (current animation frame if `animate`, else first frame).
    /// Requests a load on miss and returns None until it arrives. Failed loads are not
    /// retried for a while.
    pub fn get(&mut self, ctx: &egui::Context, url: &str, max_side: u32, animate: bool) -> Option<SizedTexture> {
        let (now, max_tex) = ctx.input(|i| (i.time, i.max_texture_side as u32));
        let max_side = max_side.clamp(1, max_tex.max(1));
        let request = |cmd: &CommandSender| cmd.send(Command::LoadImage { url: url.to_owned(), max_side });
        let Some(entry) = self.entries.get_mut(url) else {
            request(&self.cmd);
            self.entries.insert(url.to_owned(), Entry { state: State::Loading, last_used: now, side: max_side, asked: Instant::now(), late: false, shown_at: None });
            return None;
        };
        entry.last_used = now;
        match &mut entry.state {
            State::Loading => None,
            State::Failed { at } => {
                if now - *at > RETRY_FAILED_SECS {
                    request(&self.cmd);
                    entry.state = State::Loading;
                    entry.side = max_side;
                    entry.asked = Instant::now();
                }
                None
            }
            State::Ready { frames, total_ms, size, stream, .. } => {
                // Drawn bigger than the load allowed (another surface asked first, or the scale
                // changed): load once more at this size; this texture shows meanwhile.
                if max_side > entry.side && size.max_elem() as u32 >= entry.side {
                    request(&self.cmd);
                    entry.side = max_side;
                }
                if let Some(s) = stream.as_mut().filter(|_| animate) {
                    if let Some(due) = s.pull(&mut frames[0].0, now) {
                        self.next_frame_at = Some(self.next_frame_at.map_or(due, |t| t.min(due)));
                    }
                    return Some(SizedTexture::from_handle(&frames[0].0));
                }
                if !animate || frames.len() < 2 || *total_ms == 0 {
                    return Some(SizedTexture::from_handle(&frames[0].0));
                }
                let (index, remaining_ms) = frame_at(frames.iter().map(|f| f.1), *total_ms, now);
                let due = now + remaining_ms as f64 / 1000.0;
                self.next_frame_at = Some(self.next_frame_at.map_or(due, |t| t.min(due)));
                Some(SizedTexture::from_handle(&frames[index].0))
            }
        }
    }

    /// Opacity 0 -> 1 of the picture at `url` over FAST once it arrived late (`LATE`); 1 for one that
    /// was there at once and after it has faded in. Asks for frames only while it runs.
    pub fn fade(&mut self, ctx: &egui::Context, url: &str) -> f32 {
        let Some(e) = self.entries.get_mut(url).filter(|e| e.late) else { return 1.0 };
        let at = *e.shown_at.get_or_insert(now(ctx));
        let t = super::motion::since(ctx, at, super::motion::FAST);
        e.late = t < 1.0;
        t
    }

    /// Starts the streamed animation at `url` over at its first frame (an effect layer whose window opens).
    pub fn rewind(&mut self, url: &str) {
        if let Some(Entry { state: State::Ready { stream: Some(s), .. }, .. }) = self.entries.get_mut(url) {
            s.rewind();
        }
    }

    /// Whether `url` has an entry (loading, ready or failed).
    pub fn contains(&self, url: &str) -> bool {
        self.entries.contains_key(url)
    }

    pub fn remove(&mut self, key: &str) {
        if let Some(Entry { state: State::Ready { bytes, .. }, .. }) = self.entries.remove(key) {
            self.bytes -= bytes;
        }
    }

    /// Call once per frame: evicts textures not drawn for IDLE_EVICT_SECS, and the least
    /// recently drawn ones while over budget; schedules a repaint for the next animation frame
    /// of animations drawn this frame, and one at `sweep_at` so an idle window drops textures
    /// that turned idle (at most one frame a minute while textures are loaded). While minimized
    /// there is no UI pass: eviction waits for the next one (GL frees only happen on a paint anyway).
    pub fn end_frame(&mut self, ctx: &egui::Context) {
        let now = now(ctx);
        if let Some(at) = self.next_frame_at.take() {
            ctx.request_repaint_after(Duration::from_secs_f64((at - now).max(0.0)));
        }
        if self.bytes <= TEXTURE_BUDGET && now < self.sweep_at {
            if self.sweep_at.is_finite() {
                ctx.request_repaint_after(Duration::from_secs_f64(self.sweep_at - now));
            }
            return;
        }
        let mut idle: Vec<(f64, String)> = self
            .entries
            .iter()
            .filter(|(_, e)| matches!(e.state, State::Ready { .. }) && now - e.last_used > KEEP_RECENT_SECS)
            .map(|(k, e)| (e.last_used, k.clone()))
            .collect();
        idle.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (used, key) in idle {
            if self.bytes <= TEXTURE_BUDGET && now - used <= IDLE_EVICT_SECS {
                break;
            }
            self.remove(&key);
        }
        self.sweep_at = self.entries.values().filter(|e| matches!(e.state, State::Ready { .. })).map(|e| e.last_used + IDLE_EVICT_SECS).fold(f64::INFINITY, f64::min);
        if self.sweep_at.is_finite() {
            ctx.request_repaint_after(Duration::from_secs_f64((self.sweep_at - now).max(0.0)));
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
        self.next_frame_at = None;
    }

    /// Pixel size of a loaded image (for layout when the caller doesn't know it).
    pub fn loaded_size(&self, url: &str) -> Option<Vec2> {
        match self.entries.get(url)?.state {
            State::Ready { size, .. } => Some(size),
            _ => None,
        }
    }
}

fn now(ctx: &egui::Context) -> f64 {
    ctx.input(|i| i.time)
}

/// Called once per frame by `RustcordApp::logic`: hands free heap memory back to the OS a few
/// seconds after startup and after each (re)connect (`release_free_memory`). Measurement hooks,
/// nothing happens unless set:
/// `RUSTCORD_MEMTEST=presences:<n>,members:<n>,textures:<MB>[@<side>]` fills presences / a member
/// list (`AppState::apply_memtest`) / never-drawn `side`x`side` textures (default 256) on the
/// first call; `RUSTCORD_MEMSTATS=<file>` appends process counters at startup steps, and
/// approximate heap sizes of the state structures, texture and font-atlas bytes and process
/// counters 15 s after start, then every minute.
pub fn mem_hooks(ctx: &egui::Context, state: &mut crate::state::AppState, images: &mut ImageCache) {
    let now = now(ctx);
    let connected = matches!(state.conn, crate::backend::ConnState::Connected);
    if images.mem_next == 0.0 || (connected && !images.was_connected) {
        images.trim_at = now + TRIM_DELAY_SECS;
    }
    images.was_connected = connected;
    if now >= images.trim_at {
        images.trim_at = f64::INFINITY;
        mem_trace("pretrim");
        release_free_memory();
        mem_trace("trimmed");
    } else if images.trim_at.is_finite() {
        ctx.request_repaint_after(Duration::from_secs_f64(images.trim_at - now));
    }
    if images.mem_next == 0.0 {
        mem_trace("frame1");
        if let Ok(spec) = std::env::var("RUSTCORD_MEMTEST") {
            state.apply_memtest(&spec);
            let tex = spec.split(',').find_map(|p| p.trim().strip_prefix("textures:"));
            let (mb, side) = tex.map_or(("0", "256"), |t| t.split_once('@').unwrap_or((t, "256")));
            let (mb, side) = (mb.parse::<usize>().unwrap_or(0), side.parse::<usize>().unwrap_or(256).max(1));
            for i in 0..(mb << 20) / (side * side * 4) {
                let image = egui::ColorImage::new([side, side], vec![egui::Color32::from_gray(i as u8); side * side]);
                let img = DecodedImage { frames: vec![crate::backend::ImageFrame { image, delay_ms: 0 }], stream: None };
                let state = upload(ctx, "memtest", img);
                if let State::Ready { bytes, .. } = state {
                    images.bytes += bytes;
                }
                images.entries.insert(format!("memtest:{i}"), Entry { state, last_used: now, side: side as u32, asked: Instant::now(), late: false, shown_at: None });
                images.sweep_at = now + IDLE_EVICT_SECS;
            }
        }
        images.mem_next = if std::env::var_os("RUSTCORD_MEMSTATS").is_some() { 15.0 } else { f64::INFINITY };
    }
    if images.mem_next.is_infinite() {
        return;
    }
    if now < images.mem_next {
        ctx.request_repaint_after(Duration::from_secs_f64(images.mem_next - now));
        return;
    }
    images.mem_next = now + 60.0;
    let mb = |b: usize| b as f64 / (1024.0 * 1024.0);
    let mut out = format!("--- t={now:.0}s  state (capacity-based heap estimate):\n");
    let mut total = 0;
    for (name, n, bytes) in state.mem_estimate() {
        total += bytes;
        out += &format!("  {name:<24}{n:>8}  {:>8.2} MB\n", mb(bytes));
    }
    let [w, h] = ctx.fonts(|f| f.font_image_size());
    out += &format!("  {:<32}  {:>8.2} MB\n", "total", mb(total));
    out += &format!(
        "textures: images {} entries {:.2} MB (budget {:.0} MB), font atlas {w}x{h} {:.2} MB\n",
        images.entries.len(),
        mb(images.bytes),
        mb(TEXTURE_BUDGET),
        mb(w * h * 4)
    );
    out += &process_counters();
    if let Some(path) = std::env::var_os("RUSTCORD_MEMSTATS") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = f.write_all(out.as_bytes());
        }
    }
}

/// Gives the heaps' cached free pages back to the OS; the process heap's own commit doesn't
/// change, other heaps shrink. Measured (`RUSTCORD_MEMSTATS` pretrim → trimmed, release,
/// offline): ~2 MB private on the fixture and on a large account's snapshot as the app writes it
/// (5 × 50 messages), ~7 MB only with 5 × 400 messages loaded; on the fixture no lasting
/// difference 20 s after start.
pub fn release_free_memory() {
    #[cfg(windows)]
    {
        #[repr(C)]
        struct OptimizeResources {
            version: u32,
            flags: u32,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn HeapSetInformation(heap: isize, class: i32, info: *const OptimizeResources, len: usize) -> i32;
        }
        const HEAP_OPTIMIZE_RESOURCES: i32 = 3;
        let info = OptimizeResources { version: 1, flags: 0 };
        // SAFETY: documented call (Windows 8.1+, fails harmlessly before); a null heap = every heap.
        unsafe { HeapSetInformation(0, HEAP_OPTIMIZE_RESOURCES, &info, size_of::<OptimizeResources>()) };
    }
    // SAFETY: glibc's trim of free heap pages; no preconditions.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        libc::malloc_trim(0);
    }
}

/// `RUSTCORD_MEMSTATS`: process counters at a startup step.
pub fn mem_trace(label: &str) {
    if let Some(path) = std::env::var_os("RUSTCORD_MEMSTATS") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = write!(f, "[{label}] {}", process_counters());
        }
    }
}

#[cfg(windows)]
type Pair = Option<(usize, usize)>;

/// (working set, private bytes) and (process heap allocated, committed): Rust + CRT + driver mallocs.
#[cfg(windows)]
fn counters() -> (Pair, Pair) {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_faults: u32,
        peak_ws: usize,
        ws: usize,
        peak_paged_pool: usize,
        paged_pool: usize,
        peak_nonpaged_pool: usize,
        nonpaged_pool: usize,
        pagefile: usize,
        peak_pagefile: usize,
        private: usize,
    }
    #[repr(C)]
    #[derive(Default)]
    struct HeapSummary {
        cb: u32,
        allocated: usize,
        committed: usize,
        reserved: usize,
        max_reserve: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
        fn GetProcessHeap() -> isize;
        fn HeapSummary(heap: isize, flags: u32, summary: *mut HeapSummary) -> i32;
    }
    let mut c = Counters { cb: size_of::<Counters>() as u32, ..Default::default() };
    let mut h = HeapSummary { cb: size_of::<HeapSummary>() as u32, ..Default::default() };
    // SAFETY: plain out-parameter calls with correctly sized, initialized structs.
    let (ok_c, ok_h) = unsafe {
        (K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) != 0, HeapSummary(GetProcessHeap(), 0, &mut h) != 0)
    };
    (ok_c.then_some((c.ws, c.private)), ok_h.then_some((h.allocated, h.committed)))
}

/// Working set and private bytes in MB (Settings > Advanced > Diagnostics).
#[cfg(windows)]
pub fn memory_mb() -> Option<(f64, f64)> {
    counters().0.map(|(ws, private)| (ws as f64 / MB, private as f64 / MB))
}

#[cfg(windows)]
fn process_counters() -> String {
    let (proc, heap) = counters();
    let mut out = String::new();
    if let Some((ws, private)) = proc {
        out += &format!("process: working set {:.1} MB, private {:.1} MB", ws as f64 / MB, private as f64 / MB);
    }
    if let Some((allocated, committed)) = heap {
        out += &format!(", process heap allocated {:.1} MB / committed {:.1} MB", allocated as f64 / MB, committed as f64 / MB);
    }
    out + "\n"
}

/// Linux: resident set (VmRSS) as the working set, anonymous resident memory (RssAnon) as private.
#[cfg(target_os = "linux")]
pub fn memory_mb() -> Option<(f64, f64)> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let mb = |key: &str| status.lines().find_map(|l| l.strip_prefix(key)?.trim().strip_suffix("kB")?.trim().parse::<f64>().ok()).map(|kb| kb / 1024.0);
    Some((mb("VmRSS:")?, mb("RssAnon:")?))
}

#[cfg(target_os = "linux")]
fn process_counters() -> String {
    let (ws, private) = memory_mb().unwrap_or_default();
    format!("process: working set {ws:.1} MB, private {private:.1} MB\n")
}

/// Not measured on macOS.
#[cfg(not(any(windows, target_os = "linux")))]
pub fn memory_mb() -> Option<(f64, f64)> {
    None
}

#[cfg(not(any(windows, target_os = "linux")))]
fn process_counters() -> String {
    String::new()
}

#[cfg(windows)]
const MB: f64 = 1024.0 * 1024.0;

fn upload(ctx: &egui::Context, name: &str, img: DecodedImage) -> State {
    let first = &img.frames[0].image;
    let size = egui::vec2(first.size[0] as f32, first.size[1] as f32);
    let animated = img.frames.len() > 1 || img.stream.is_some();
    let mut bytes = 0;
    let mut total_ms = 0;
    let stream = img.stream;
    let frames = img
        .frames
        .into_iter()
        .enumerate()
        .map(|(i, f)| {
            bytes += f.image.pixels.len() * 4;
            let delay = if animated { f.delay_ms.max(1) } else { 0 };
            total_ms += delay;
            (ctx.load_texture(format!("{name}#{i}"), f.image, egui::TextureOptions::LINEAR), delay)
        })
        .collect();
    let stream = stream.map(|s| Stream { rx: s.rx, rewind: s.rewind, pass: 0, due: None, delay: f64::from(total_ms) / 1000.0 });
    State::Ready { frames, total_ms, bytes, size, stream }
}

/// Frame shown at `time` (seconds) in a looping animation, and ms until the next frame.
fn frame_at(delays: impl Iterator<Item = u32>, total_ms: u32, time: f64) -> (usize, u32) {
    let t = ((time.max(0.0) * 1000.0) as u64 % total_ms as u64) as u32;
    let mut end = 0;
    let mut last = 0;
    for (i, d) in delays.enumerate() {
        end += d;
        last = i;
        if t < end {
            return (i, end - t);
        }
    }
    (last, 1)
}

fn placeholder_color(ui: &egui::Ui) -> egui::Color32 {
    ui.visuals().widgets.inactive.bg_fill
}

/// Circular image (avatar/guild icon); grey circle placeholder while loading.
pub fn circle_image(ui: &mut egui::Ui, images: &mut ImageCache, url: &str, size: f32, animate: bool) -> egui::Response {
    avatar(ui, images, url, size, animate, None)
}

/// Circular user avatar. Until the image is there (loading, offline) it shows a circle in a
/// muted color derived from `who.0` (user id) with the first letter of `who.1` (display name),
/// so missing avatars look intentional. `None` = plain grey circle (icons that aren't users).
pub fn avatar(ui: &mut egui::Ui, images: &mut ImageCache, url: &str, size: f32, animate: bool, who: Option<(Id, &str)>) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), Sense::click());
    // Off-screen widgets don't load anything (long scrolled lists).
    if ui.is_rect_visible(rect) {
        let max_side = (size * ui.ctx().pixels_per_point()).ceil() as u32;
        let tex = images.get(ui.ctx(), url, max_side, animate);
        // A picture that arrived late fades in over the placeholder.
        let a = tex.map_or(0.0, |_| images.fade(ui.ctx(), url));
        if a < 1.0 {
            match who {
                Some((id, name)) => paint_initial(ui.painter(), rect, size / 2.0, id.0, name, ui.visuals().dark_mode),
                None => {
                    ui.painter().circle_filled(rect.center(), size / 2.0, placeholder_color(ui));
                }
            }
        }
        if let Some(tex) = tex {
            egui::Image::new(tex).corner_radius(size / 2.0).tint(egui::Color32::WHITE.gamma_multiply(a)).paint_at(ui, rect);
        }
    }
    response
}

/// Muted fill + letter color for a placeholder, picked from a small hand-tuned hue set by
/// `seed` (stable per user/guild id).
pub fn initial_colors(seed: u64, dark: bool) -> (egui::Color32, egui::Color32) {
    const HUES: [f32; 8] = [0.02, 0.08, 0.13, 0.36, 0.48, 0.58, 0.70, 0.85];
    // splitmix64 finalizer: neighbouring snowflakes land on different hues.
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    let h = HUES[((z ^ (z >> 31)) % HUES.len() as u64) as usize];
    // Gamma space: `Hsva` is linear, where these values come out washed out (~2:1 contrast).
    let hsv = |s, v| egui::Color32::from(egui::ecolor::HsvaGamma { h, s, v, a: 1.0 });
    if dark { (hsv(0.30, 0.42), hsv(0.12, 0.96)) } else { (hsv(0.24, 0.86), hsv(0.60, 0.38)) }
}

/// Rounded shape (`radius` = half the size for a circle) in the `seed` color with the first
/// letter of `name`. Also for callers that place avatars or icons themselves.
pub fn paint_initial(painter: &egui::Painter, rect: egui::Rect, radius: f32, seed: u64, name: &str, dark: bool) {
    let (fill, fg) = initial_colors(seed, dark);
    painter.rect_filled(rect, radius, fill);
    // Tiny avatars (reply previews) are just the colored dot.
    if rect.height() >= 20.0 {
        let font = egui::FontId::new(rect.height() * 0.42, crate::theme::bold_family());
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, initial(name), font, fg);
    }
}

/// First letter or digit of a name, uppercased ("?" when there is none).
pub fn initial(name: &str) -> String {
    let c = name.chars().find(|c| c.is_alphanumeric()).or_else(|| name.trim().chars().next()).unwrap_or('?');
    c.to_uppercase().collect()
}

/// Image scaled to fit `max` (keeps aspect; `natural` = known pixel size for layout before
/// load). Click-sensing response.
pub fn fit_image(ui: &mut egui::Ui, images: &mut ImageCache, url: &str, max: egui::Vec2, natural: Option<egui::Vec2>, animate: bool) -> egui::Response {
    let source = natural.filter(|s| s.x > 0.0 && s.y > 0.0).or_else(|| images.loaded_size(url));
    let size = source.map_or(max, |s| fit_size(s, max));
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let max_side = (size.max_elem() * ui.ctx().pixels_per_point()).ceil() as u32;
        let radius = ui.visuals().widgets.noninteractive.corner_radius;
        match images.get(ui.ctx(), url, max_side, animate) {
            Some(tex) => egui::Image::new(tex).corner_radius(radius).paint_at(ui, rect),
            None => {
                ui.painter().rect_filled(rect, radius, placeholder_color(ui));
            }
        }
    }
    response
}

/// `natural` scaled down (never up) to fit inside `max`, keeping aspect.
fn fit_size(natural: Vec2, max: Vec2) -> Vec2 {
    let scale = (max.x / natural.x).min(max.y / natural.y).min(1.0);
    (natural * scale).max(egui::vec2(1.0, 1.0))
}

/// Avatar URL for a user in context (guild member avatar > user avatar > default avatar).
/// My own avatar is replaced by the local override (`crate::overrides`) when one is set.
pub fn user_avatar_url(guild_id: Option<Id>, user: &User, member: Option<&Member>, size: u32, animated: bool) -> String {
    use crate::api::cdn;
    if let Some(url) = crate::overrides::my_avatar_url(user.id) {
        return url;
    }
    match (guild_id, member.and_then(|m| m.avatar.as_deref())) {
        (Some(guild_id), Some(hash)) => cdn::member_avatar_url(guild_id, user.id, hash, size, animated),
        _ => cdn::avatar_url(user, size, animated),
    }
}

/// Avatar decoration (docs/COSMETICS_NOTES.md §3) over the avatar at `avatar`: 1.2× its size
/// (snapped to physical pixels), centred, painted over it without taking layout space (paint status dots after it). The
/// static PNG at rest, the APNG while `animate`. `deco`: from `overrides::effective_for`, so my
/// local pick replaces mine.
pub fn decoration_at(ui: &egui::Ui, images: &mut ImageCache, avatar: egui::Rect, deco: Option<&crate::model::DecorationData>, animate: bool, opacity: f32) -> Option<String> {
    let d = deco?;
    let ppp = ui.ctx().pixels_per_point();
    // Whole physical pixels on a pixel-aligned spot: the texture (decoded at exactly `side`) maps 1:1.
    let side = (avatar.width() * 1.2 * ppp).round().max(1.0);
    let min = ((avatar.center().to_vec2() * ppp - Vec2::splat(side / 2.0)).round() / ppp).to_pos2();
    let rect = egui::Rect::from_min_size(min, Vec2::splat(side / ppp));
    let side = side as u32;
    let url = crate::api::cdn::decoration_url(&d.asset, side, animate)?;
    if ui.is_rect_visible(rect)
        && let Some(tex) = images.get(ui.ctx(), &url, side, animate)
    {
        egui::Image::new(tex).tint(egui::Color32::WHITE.gamma_multiply(opacity)).paint_at(ui, rect);
    }
    // The URL drawn, for callers that free it themselves (the cosmetics editor's grid).
    Some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_frame_by_time() {
        let delays = [100u32, 50, 200];
        assert_eq!(frame_at(delays.into_iter(), 350, 0.0), (0, 100));
        assert_eq!(frame_at(delays.into_iter(), 350, 0.120), (1, 30));
        assert_eq!(frame_at(delays.into_iter(), 350, 0.150), (2, 200));
        // Loops.
        assert_eq!(frame_at(delays.into_iter(), 350, 0.360), (0, 90));
    }

    #[test]
    fn placeholder_initials_and_colors() {
        assert_eq!(initial("ferris_dev"), "F");
        assert_eq!(initial("  ._ülrich"), "Ü");
        assert_eq!(initial("🦀"), "🦀");
        assert_eq!(initial(""), "?");
        assert_eq!(initial_colors(42, true), initial_colors(42, true));
        let hues: std::collections::HashSet<_> = (1..9u64).map(|n| initial_colors(1_565_443_437_363_200_000 + n, true).0).collect();
        assert!(hues.len() >= 4, "neighbouring ids should spread over the palette");
    }

    #[test]
    fn fits_keeping_aspect() {
        assert_eq!(fit_size(egui::vec2(800.0, 400.0), egui::vec2(400.0, 300.0)), egui::vec2(400.0, 200.0));
        assert_eq!(fit_size(egui::vec2(100.0, 50.0), egui::vec2(400.0, 300.0)), egui::vec2(100.0, 50.0));
        assert_eq!(fit_size(egui::vec2(300.0, 600.0), egui::vec2(400.0, 300.0)), egui::vec2(150.0, 300.0));
    }

    #[test]
    fn textures_not_drawn_for_a_while_go_even_under_budget() {
        let ctx = egui::Context::default();
        let mut cache = ImageCache::new(CommandSender(tokio::sync::mpsc::unbounded_channel().0));
        let at = |t: f64| egui::RawInput { time: Some(t), ..Default::default() };
        // No painter here: drop the texture uploads / frees a pass produces.
        let end = |ctx: &egui::Context| ctx.end_pass().textures_delta.clear();
        ctx.begin_pass(at(0.0));
        for url in ["shown", "scrolled-away"] {
            assert!(cache.get(&ctx, url, 16, false).is_none(), "requested on a miss");
            let image = egui::ColorImage::new([4, 4], vec![egui::Color32::WHITE; 16]);
            let img = DecodedImage { frames: vec![crate::backend::ImageFrame { image, delay_ms: 0 }], stream: None };
            cache.on_loaded(&ctx, url.into(), Ok(img));
        }
        cache.end_frame(&ctx);
        end(&ctx);
        // "scrolled-away" leaves the screen; an idle window still wakes when it turns idle (after
        // a pass that settles egui's start-up repaints).
        ctx.begin_pass(at(5.0));
        end(&ctx);
        ctx.begin_pass(at(10.0));
        assert!(cache.get(&ctx, "shown", 16, false).is_some());
        cache.end_frame(&ctx);
        let wake = ctx.end_pass().viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        assert_eq!(wake, Duration::from_secs_f64(IDLE_EVICT_SECS - 10.0));
        for t in [IDLE_EVICT_SECS - 1.0, IDLE_EVICT_SECS + 1.0] {
            ctx.begin_pass(at(t));
            assert!(cache.get(&ctx, "shown", 16, false).is_some());
            cache.end_frame(&ctx);
            end(&ctx);
            assert_eq!(cache.contains("scrolled-away"), t < IDLE_EVICT_SECS);
        }
        assert_eq!(cache.bytes, 4 * 4 * 4, "only the drawn texture is left");
        drop(cache);
        ctx.begin_pass(at(100.0));
        end(&ctx);
    }

    #[test]
    fn drawn_bigger_loads_sharper() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut cache = ImageCache::new(CommandSender(tx));
        let ctx = egui::Context::default();
        let img = |side: usize| {
            let image = egui::ColorImage::new([side, side], vec![egui::Color32::WHITE; side * side]);
            Ok(DecodedImage { frames: vec![crate::backend::ImageFrame { image, delay_ms: 0 }], stream: None })
        };
        let size = |c: &ImageCache| c.loaded_size("u").unwrap().x as u32;
        let mut t = 0.0;
        let mut pass = |cache: &mut ImageCache, f: &dyn Fn(&mut ImageCache, &egui::Context)| {
            t += 1.0;
            ctx.begin_pass(egui::RawInput { time: Some(t), ..Default::default() });
            f(cache, &ctx);
            cache.end_frame(&ctx);
            ctx.end_pass().textures_delta.clear();
        };
        pass(&mut cache, &|c, ctx| assert!(c.get(ctx, "u", 48, false).is_none()));
        pass(&mut cache, &|c, ctx| c.on_loaded(ctx, "u".into(), img(48)));
        pass(&mut cache, &|c, ctx| assert!(c.get(ctx, "u", 40, false).is_some(), "smaller: no new load"));
        pass(&mut cache, &|c, ctx| assert!(c.get(ctx, "u", 60, false).is_some(), "old texture meanwhile"));
        pass(&mut cache, &|c, ctx| c.on_loaded(ctx, "u".into(), img(60)));
        assert_eq!((size(&cache), cache.bytes), (60, 60 * 60 * 4));
        pass(&mut cache, &|c, ctx| c.on_loaded(ctx, "u".into(), img(48)));
        assert_eq!(size(&cache), 60, "a late smaller result is ignored");
        pass(&mut cache, &|c, ctx| assert!(c.get(ctx, "u", 120, false).is_some()));
        pass(&mut cache, &|c, ctx| c.on_loaded(ctx, "u".into(), img(60)));
        pass(&mut cache, &|c, ctx| assert!(c.get(ctx, "u", 120, false).is_some(), "no second request"));
        assert_eq!(size(&cache), 60);
        let sides: Vec<u32> = std::iter::from_fn(|| rx.try_recv().ok()).filter_map(|c| if let Command::LoadImage { max_side, .. } = c { Some(max_side) } else { None }).collect();
        assert_eq!(sides, [48, 60, 120]);
    }

    #[test]
    fn late_pictures_fade_in_cached_ones_do_not() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut cache = ImageCache::new(CommandSender(tx));
        let ctx = egui::Context::default();
        let img = |side: usize| {
            let image = egui::ColorImage::new([side, side], vec![egui::Color32::WHITE; side * side]);
            Ok(DecodedImage { frames: vec![crate::backend::ImageFrame { image, delay_ms: 0 }], stream: None })
        };
        // One pass at ctx time `t`; `f` runs inside it. Returns whether the pass asked for another frame.
        let pass = |cache: &mut ImageCache, t: f64, f: &dyn Fn(&mut ImageCache, &egui::Context)| {
            ctx.begin_pass(egui::RawInput { time: Some(t), ..Default::default() });
            f(cache, &ctx);
            let mut out = ctx.end_pass();
            out.textures_delta.clear();
            out.viewport_output.get(&egui::ViewportId::ROOT).is_some_and(|o| o.repaint_delay.is_zero())
        };
        for url in ["fast", "slow"] {
            pass(&mut cache, 1.0, &|c, ctx| assert!(c.get(ctx, url, 48, false).is_none()));
        }
        // "slow" was asked for 0.5 s ago, "fast" just now.
        cache.entries.get_mut("slow").unwrap().asked -= Duration::from_millis(500);
        pass(&mut cache, 2.0, &|c, ctx| {
            c.on_loaded(ctx, "fast".into(), img(48));
            c.on_loaded(ctx, "slow".into(), img(48));
        });
        assert!(!pass(&mut cache, 2.0, &|c, ctx| assert_eq!(c.fade(ctx, "fast"), 1.0)), "a quick one shows at once");
        assert!(pass(&mut cache, 2.5, &|c, ctx| assert_eq!(c.fade(ctx, "slow"), 0.0)), "a late one starts from nothing");
        assert!(pass(&mut cache, 2.56, &|c, ctx| assert!(c.fade(ctx, "slow") > 0.0 && c.fade(ctx, "slow") < 1.0)));
        pass(&mut cache, 2.7, &|c, ctx| assert_eq!(c.fade(ctx, "slow"), 1.0));
        assert!(!pass(&mut cache, 2.8, &|c, ctx| assert_eq!(c.fade(ctx, "slow"), 1.0)), "done: no more frames");
        // A sharper reload does not start it again.
        pass(&mut cache, 3.0, &|c, ctx| _ = c.get(ctx, "slow", 96, false));
        pass(&mut cache, 3.1, &|c, ctx| c.on_loaded(ctx, "slow".into(), img(96)));
        assert!(!pass(&mut cache, 3.2, &|c, ctx| assert_eq!(c.fade(ctx, "slow"), 1.0)));
    }

    #[test]
    fn streamed_animation_pulls_when_due() {
        let (tx_cmd, _cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut cache = ImageCache::new(CommandSender(tx_cmd));
        let ctx = egui::Context::default();
        let frame = |ms| ImageFrame { image: egui::ColorImage::new([2, 2], vec![egui::Color32::WHITE; 4]), delay_ms: ms };
        let (tx, rx) = std::sync::mpsc::sync_channel::<(u32, ImageFrame)>(1);
        let rewind = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        // Runs one pass at `t` and returns the repaint delay it asked for.
        let pass = |cache: &mut ImageCache, t: f64, animate: bool| {
            ctx.begin_pass(egui::RawInput { time: Some(t), ..Default::default() });
            cache.get(&ctx, "fx", 2, animate);
            cache.end_frame(&ctx);
            let mut out = ctx.end_pass();
            out.textures_delta.clear();
            out.viewport_output[&egui::ViewportId::ROOT].repaint_delay.as_secs_f64()
        };
        pass(&mut cache, 0.0, true);
        ctx.begin_pass(egui::RawInput { time: Some(0.5), ..Default::default() });
        cache.on_loaded(&ctx, "fx".into(), Ok(DecodedImage { frames: vec![frame(100)], stream: Some(crate::backend::FrameStream { rx, rewind: rewind.clone() }) }));
        ctx.end_pass().textures_delta.clear();
        tx.send((0, frame(50))).unwrap();
        pass(&mut cache, 1.0, true);
        assert!(matches!(tx.try_send((0, frame(50))), Err(std::sync::mpsc::TrySendError::Full(_))), "not due yet: nothing pulled");
        let wake = pass(&mut cache, 1.1, true);
        assert!(tx.try_send((0, frame(50))).is_ok(), "due: the waiting frame was pulled");
        assert!((wake - 0.05).abs() < 1e-6, "next frame in 50 ms, got {wake}");
        pass(&mut cache, 5.0, false);
        assert!(matches!(tx.try_send((0, frame(50))), Err(std::sync::mpsc::TrySendError::Full(_))), "not animating: nothing pulled");
        let wake = pass(&mut cache, 10.0, true);
        assert!(tx.try_send((0, frame(50))).is_ok(), "pulled again");
        assert!((wake - 0.05).abs() < 1e-6, "late: the clock restarts now, got {wake}");
        pass(&mut cache, 10.05, true); // takes the frame sent above
        let wake = pass(&mut cache, 10.11, true);
        assert!((wake - 0.0125).abs() < 1e-6, "decoder behind: again in a quarter frame, got {wake}");
        // A rewind drops what the decoder queued before it and looks for the new pass's first frame at once.
        tx.send((0, frame(50))).unwrap();
        cache.rewind("fx");
        assert_eq!(rewind.load(std::sync::atomic::Ordering::Relaxed), 1);
        let wake = pass(&mut cache, 10.2, true);
        assert!(tx.try_send((1, frame(70))).is_ok(), "the old pass's frame was dropped");
        assert!((wake - 0.0125).abs() < 1e-6, "the new first frame looked for soon, got {wake}");
        let wake = pass(&mut cache, 10.22, true);
        assert!((wake - 0.0625).abs() < 1e-6, "shown, the next due after its 70 ms, got {wake}");
        drop(cache);
        assert!(tx.send((1, frame(50))).is_err(), "dropping the cache stops the decoder");
    }
}
