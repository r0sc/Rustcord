//! What the computer plays, for the user panel's music controls (ui/sidebar.rs `music_row`).
//! Windows reads the system media session (`smtc`), Linux the MPRIS players on the session bus
//! (`mpris`); both are event driven: one blocking thread wakes on an OS event, reads the title,
//! artist, picture and what the player can do, and repaints only when that changed. Local only: the
//! track is never sent anywhere, never logged, never put into presence. macOS has no public API.

use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
mod mpris;
#[cfg(windows)]
mod smtc;

pub const SUPPORTED: bool = cfg!(any(windows, target_os = "linux"));

/// A paused player stays visible this long, so Play stays reachable after a pause.
pub const PAUSED_STAYS: Duration = Duration::from_secs(60);

#[derive(Clone, PartialEq)]
pub struct Track {
    pub title: String,
    pub artist: String,
    /// 48x48, shown at 24.
    pub art: Option<Arc<egui::ColorImage>>,
    pub can_next: bool,
    pub playing: bool,
}

/// What the listener thread is asked to do (Linux and Windows).
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
enum Msg {
    Changed,
    Toggle,
    Next,
}

#[derive(Default)]
struct Shared {
    track: Option<Track>,
    /// The last playing / paused change.
    since: Option<Instant>,
    /// Bumped when the picture changed, so the UI uploads it once.
    generation: u64,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared { track: None, since: None, generation: 0 });
static TX: OnceLock<mpsc::Sender<Msg>> = OnceLock::new();

/// The current track, its last playing / paused change and the picture's generation. Starts the
/// listener on the first call (not in screenshot runs: the demo hook drives the row there).
pub fn current(ctx: &egui::Context) -> Option<(Track, Instant, u64)> {
    start(ctx);
    let s = SHARED.lock().ok()?;
    Some((s.track.clone()?, s.since?, s.generation))
}

pub fn toggle() {
    send(Msg::Toggle);
}

pub fn next() {
    send(Msg::Next);
}

fn send(m: Msg) {
    if let Some(tx) = TX.get() {
        let _ = tx.send(m);
    }
}

#[cfg(any(windows, target_os = "linux"))]
fn start(ctx: &egui::Context) {
    if std::env::var_os("RUSTCORD_SCREENSHOT").is_some() {
        return;
    }
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        let (ctx, tx2) = (ctx.clone(), tx.clone());
        let spawned = std::thread::Builder::new().name("rustcord-now-playing".into()).spawn(move || {
            #[cfg(windows)]
            smtc::run(&ctx, &tx2, &rx);
            #[cfg(target_os = "linux")]
            mpris::run(&ctx, &tx2, &rx);
        });
        drop(spawned);
        tx
    });
}

#[cfg(not(any(windows, target_os = "linux")))]
fn start(_: &egui::Context) {}

/// Stores a new track (None: nothing plays) and repaints, only when it differs from the stored one.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
fn publish(ctx: &egui::Context, new: Option<Track>) {
    let Ok(mut s) = SHARED.lock() else { return };
    if s.track == new {
        return;
    }
    let art = |t: &Option<Track>| t.as_ref().and_then(|t| t.art.clone());
    if art(&s.track) != art(&new) {
        s.generation += 1;
    }
    if s.track.as_ref().map(|t| t.playing) != new.as_ref().map(|t| t.playing) {
        s.since = Some(Instant::now());
    }
    s.track = new;
    drop(s);
    ctx.request_repaint();
}

/// The row shows while the track plays, or for `PAUSED_STAYS` after it paused. A title is required.
pub fn visible(t: &Track, since: Instant, now: Instant) -> bool {
    !t.title.is_empty() && (t.playing || now.saturating_duration_since(since) < PAUSED_STAYS)
}

/// Time left of a paused track's grace (when to wake the UI once); None while it plays or is over.
pub fn wake_in(t: &Track, since: Instant, now: Instant) -> Option<Duration> {
    if t.playing {
        return None;
    }
    PAUSED_STAYS.checked_sub(now.saturating_duration_since(since)).filter(|d| !d.is_zero())
}

/// A cover picture as the 48x48 thumbnail the row shows; None when it is over 2 MB or not an image.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
fn art(bytes: &[u8]) -> Option<egui::ColorImage> {
    if bytes.len() > 2 * 1024 * 1024 {
        return None;
    }
    let img = image::load_from_memory(bytes).ok()?.resize_to_fill(48, 48, image::imageops::FilterType::Triangle).to_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied([48, 48], img.as_raw()))
}

/// Which player to show: the first one that plays, else the one shown last while it is still there.
#[cfg(any(target_os = "linux", test))]
fn pick<'a>(players: &'a [(String, bool)], last: Option<&str>) -> Option<&'a str> {
    players.iter().find(|p| p.1).or_else(|| players.iter().find(|p| Some(p.0.as_str()) == last)).map(|p| p.0.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(playing: bool) -> Track {
        Track { title: "Rewriting it in Rust".into(), artist: "Ferris".into(), art: None, can_next: true, playing }
    }

    #[test]
    fn visible_while_playing_and_a_minute_after_pause() {
        let t0 = Instant::now();
        let at = |s| t0 + Duration::from_secs(s);
        assert!(visible(&track(true), t0, at(3600)), "playing: always");
        assert!(visible(&track(false), t0, at(59)));
        assert!(!visible(&track(false), t0, at(60)));
        assert!(!visible(&Track { title: String::new(), ..track(true) }, t0, t0), "a title is required");
        assert_eq!(wake_in(&track(true), t0, t0), None);
        assert_eq!(wake_in(&track(false), t0, at(45)), Some(Duration::from_secs(15)));
        assert_eq!(wake_in(&track(false), t0, at(61)), None);
    }

    #[test]
    fn art_is_shrunk_to_48() {
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(100, 60, image::Rgba([200, 30, 30, 255]))).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let a = art(&png).expect("a picture");
        assert_eq!(a.size, [48, 48]);
        assert_eq!(a.pixels.len(), 48 * 48);
        assert!(art(b"not an image").is_none());
        assert!(art(&vec![0u8; 2 * 1024 * 1024 + 1]).is_none());
    }

    #[test]
    fn pick_prefers_a_playing_player_then_the_last_one() {
        let p = |v: &[(&str, bool)]| v.iter().map(|(n, b)| ((*n).to_owned(), *b)).collect::<Vec<_>>();
        let both = p(&[("a", false), ("b", true)]);
        assert_eq!(pick(&both, Some("a")), Some("b"));
        let paused = p(&[("a", false), ("b", false)]);
        assert_eq!(pick(&paused, Some("b")), Some("b"));
        assert_eq!(pick(&paused, Some("gone")), None);
        assert_eq!(pick(&[], None), None);
    }
}
