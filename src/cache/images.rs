//! Image fetching + disk cache + decoding (runs on the backend runtime).
//!
//! - Only Discord CDN/media hosts, the GIF host (static.klipy.com) and Spotify's album art
//!   (i.scdn.co) are fetched; never sends the auth token.
//! - Disk cache in `config::paths().images_dir()`, file name = FNV-1a hash of the cache key.
//!   The cache key strips volatile signature params (`ex`, `is`, `hm`) of attachment URLs.
//! - Decoding happens in `spawn_blocking`, downscaled so the longest side <= max_side.
//!   GIF / APNG / animated WebP decode to multiple frames, capped by a memory budget
//!   (fallback: first frame only); avatar decorations keep every frame and give up resolution
//!   (down to half) instead; profile effects and banners stream instead (`stream_frames`).
//! - The same disk cache, client and host list serve the media player (`media::net`): the
//!   "media cache" of small videos and gifv loops.
//!
//! OWNERSHIP: cache implementer.

use crate::backend::{BackendEvent, DecodedImage, EventSink, FrameStream, ImageFrame};
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use image::codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder};
use image::{AnimationDecoder, Frames, ImageDecoder, ImageFormat, RgbaImage};
use std::collections::HashSet;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};
use tokio::sync::Semaphore;

const MAX_CONCURRENT: usize = 6;
/// Larger responses are not images we want to show inline (and could be huge originals).
const MAX_DOWNLOAD: usize = 32 * 1024 * 1024;
const DISK_CAP: u64 = 300 * 1024 * 1024;
/// Prune below the cap so it doesn't have to run again on the next start.
const DISK_PRUNE_TARGET: u64 = 240 * 1024 * 1024;
/// A third of the UI texture budget (12 MB) so one GIF can't crowd out everything else.
const MAX_ANIM_BYTES: usize = 4 * 1024 * 1024;
/// Bounds the texture count; MAX_ANIM_BYTES is the real cap.
const MAX_ANIM_FRAMES: usize = 480;

#[derive(Clone)]
pub struct ImageLoader {
    inner: Arc<Inner>,
}

struct Inner {
    http: reqwest::Client,
    permits: Semaphore,
    in_flight: Mutex<HashSet<String>>,
    dir: PathBuf,
}

impl ImageLoader {
    pub fn new() -> Self {
        let http = http_client();
        let dir = crate::config::paths().images_dir();
        let prune_dir = dir.clone();
        let _ = std::thread::Builder::new()
            .name("rustcord-img-prune".into())
            .spawn(move || prune(&prune_dir, DISK_CAP, DISK_PRUNE_TARGET));
        Self {
            inner: Arc::new(Inner {
                http,
                permits: Semaphore::new(MAX_CONCURRENT),
                in_flight: Mutex::new(HashSet::new()),
                dir,
            }),
        }
    }

    /// Fetch+decode in the background and send `BackendEvent::ImageLoaded` to `sink`.
    /// Concurrency-limited; duplicate in-flight requests for the same url are coalesced.
    pub fn request(&self, url: String, max_side: u32, sink: EventSink) {
        if !self.inner.in_flight().insert(url.clone()) {
            return;
        }
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let result = inner.load(&url, max_side).await;
            inner.in_flight().remove(&url);
            sink.send(BackendEvent::ImageLoaded { url, result });
        });
    }
}

impl Inner {
    fn in_flight(&self) -> MutexGuard<'_, HashSet<String>> {
        self.in_flight.lock().unwrap_or_else(|e| e.into_inner())
    }

    async fn load(&self, url: &str, max_side: u32) -> Result<DecodedImage, String> {
        // Local profile overrides: only files inside the overrides folder, never the network.
        let keep = keeps_frames(url);
        let stream = streams(url);
        let dec = move |b: &[u8]| match stream.then(|| stream_frames(b, max_side)).flatten() {
            Some(img) => Ok(img),
            None => decode(b, max_side, keep),
        };
        if let Some(path) = crate::overrides::local_path(url) {
            return blocking(move || std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| dec(&b))).await?;
        }
        let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid image URL: {e}"))?;
        if !host_allowed(&parsed) {
            return Err(format!("image host not allowed: {}", parsed.host_str().unwrap_or("?")));
        }
        let _permit = self.permits.acquire().await.map_err(|e| e.to_string())?;
        let path = self.dir.join(format!("{:016x}", fnv1a64(cache_key(url).as_bytes())));

        let p = path.clone();
        // A cached file that no longer decodes is treated as a miss and overwritten.
        let cached = blocking(move || std::fs::read(&p).ok().and_then(|b| dec(&b).ok())).await?;
        if let Some(img) = cached {
            return Ok(img);
        }
        // Offline mode promises no network at all: disk cache only.
        if crate::config::offline_mode() {
            return Err("offline".into());
        }

        let bytes = self.download(parsed).await?;
        blocking(move || {
            let img = dec(&bytes)?;
            // Only cache what decodes, so error pages never end up on disk.
            if let Err(e) = crate::config::write_atomic(&path, &bytes) {
                eprintln!("rustcord: image cache write failed: {e}");
            }
            Ok(img)
        })
        .await?
    }

    async fn download(&self, url: reqwest::Url) -> Result<Vec<u8>, String> {
        let mut resp = self.http.get(url).send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("HTTP {}", status.as_u16()));
        }
        let len = resp.content_length().unwrap_or(0);
        if len > MAX_DOWNLOAD as u64 {
            return Err("image too large".into());
        }
        let mut body = Vec::with_capacity(len as usize);
        while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
            if body.len() + chunk.len() > MAX_DOWNLOAD {
                return Err("image too large".into());
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(f).await.map_err(|e| format!("image task failed: {e}"))
}

/// The image (and media) HTTP client: same UA as the API client; no Authorization header ever;
/// redirects only within the allowed hosts.
pub(crate) fn http_client() -> reqwest::Client {
    let ua = crate::api::props::ClientProps::new(&crate::config::Settings::default()).user_agent().to_owned();
    reqwest::Client::builder()
        .user_agent(ua)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::custom(|a| {
            if a.previous().len() >= 5 || !host_allowed(a.url()) { a.stop() } else { a.follow() }
        }))
        .build()
        .expect("image HTTP client")
}

/// Disk-cache file of `url`.
pub(crate) fn cache_path(url: &str) -> PathBuf {
    crate::config::paths().images_dir().join(format!("{:016x}", fnv1a64(cache_key(url).as_bytes())))
}

/// https on a Discord CDN/media host, the GIF picker host (FEATURES_NOTES §1.8) or Spotify's
/// album-art host only.
pub(crate) fn host_allowed(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.port().is_none()
        && url.host_str().is_some_and(|h| {
            h == "cdn.discordapp.com"
                || h == "media.discordapp.net"
                || h == "static.klipy.com"
                // Album art of `spotify:` activities (`cdn::activity_asset_url`); the official
                // client loads it from here too.
                || h == "i.scdn.co"
                || h.strip_prefix("images-ext-")
                    .and_then(|r| r.strip_suffix(".discordapp.net"))
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
}

/// URL without the expiring signature params (`ex`, `is`, `hm`); other params identify the variant.
pub(crate) fn cache_key(url: &str) -> String {
    let Some((base, query)) = url.split_once('?') else { return url.to_owned() };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|p| !p.is_empty() && !matches!(p.split('=').next(), Some("ex" | "is" | "hm")))
        .collect();
    if kept.is_empty() { base.to_owned() } else { format!("{base}?{}", kept.join("&")) }
}

pub(crate) fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

/// Delete the oldest files until the folder is below `target`, if it is above `cap`.
fn prune(dir: &Path, cap: u64, target: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            m.is_file().then(|| (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= cap {
        return;
    }
    files.sort();
    for (_, len, path) in files {
        if total <= target {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

/// Avatar decorations keep their frames (`fit_side`): small, and the motion is the point.
fn keeps_frames(url: &str) -> bool {
    url.strip_prefix(crate::api::cdn::CDN).is_some_and(|p| p.starts_with("/avatar-decoration-presets/"))
}

/// Profile-effect layers (COSMETICS_NOTES §5) and profile banners: long animations far over
/// MAX_ANIM_BYTES at the drawn size (a 240-frame banner kept 8 frames and played at 2 fps); they stream.
// shortcut: matched by path; an animation served from elsewhere drops frames like any other.
fn streams(url: &str) -> bool {
    url.strip_prefix(crate::api::cdn::CDN).is_some_and(|p| p.starts_with("/assets/profile_effects/") || p.contains("/banners/"))
}

/// Frames a stream decodes ahead of the shown one.
const STREAM_AHEAD: usize = 2;

/// One pass over an animation: each composited frame and its delay (ms) go to `frame` until it says stop.
type Pass = fn(&[u8], &mut dyn FnMut(&Canvas, u32) -> bool) -> Result<(), String>;

/// A multi-frame APNG or a GIF decoded frame by frame on its own thread (`DecodedImage::stream`), each frame
/// box-shrunk by `Canvas::image`, at most STREAM_AHEAD waiting (a card that isn't drawn pulls nothing, so the
/// thread waits); starts over at the end or on a rewind, ends when the receiver is dropped or after a single
/// frame. Returns with the first frame; None for other formats (or a failed first frame: `decode` then tries).
// shortcut: the compressed file stays in RAM (2-8 MB); read it from the disk-cache file if that turns out too much.
fn stream_frames(bytes: &[u8], max_side: u32) -> Option<DecodedImage> {
    let pass: Pass = if apng_frames(bytes).is_some_and(|n| n > 1) {
        apng_pass
    } else if bytes.starts_with(b"GIF8") {
        gif_pass
    } else {
        return None;
    };
    let bytes: Arc<[u8]> = bytes.into();
    let (tx, rx) = std::sync::mpsc::sync_channel(STREAM_AHEAD);
    let rewind = Arc::new(AtomicU32::new(0));
    let asked = rewind.clone();
    let thread = std::thread::Builder::new().name("rustcord-anim".into()).spawn(move || {
        loop {
            let n = asked.load(Relaxed);
            let (mut sent, mut open) = (0, true);
            // A frame that fails ends the pass, like a truncated file; a rewind too (the next starts at frame 0).
            let _ = pass(&bytes, &mut |canvas, delay_ms| {
                if asked.load(Relaxed) != n {
                    return false;
                }
                open = tx.send((n, ImageFrame { image: canvas.image(max_side), delay_ms })).is_ok();
                sent += 1;
                open
            });
            if !open || (sent < 2 && asked.load(Relaxed) == n) {
                return; // card closed, or a still image: its frame stays
            }
        }
    });
    let (_, first) = rx.recv().ok().filter(|_| thread.is_ok())?;
    Some(DecodedImage { frames: vec![first], stream: Some(FrameStream { rx, rewind }) })
}

/// APNG frames through the `png` crate, composited here: `image`'s own compositor blends every pixel in
/// floats and copies the canvas twice, 19 ms a 450x880 effect frame against 1 ms to decode it (measured).
fn apng_pass(bytes: &[u8], frame: &mut dyn FnMut(&Canvas, u32) -> bool) -> Result<(), String> {
    use png::{BlendOp, ColorType, DisposeOp, Transformations};
    let mut dec = png::Decoder::new_with_limits(Cursor::new(bytes), png::Limits { bytes: 256 << 20 });
    dec.set_transformations(Transformations::EXPAND | Transformations::STRIP_16 | Transformations::ALPHA);
    let mut r = dec.read_info().map_err(|e| e.to_string())?;
    if r.info().interlaced {
        return Err("interlaced APNG".into()); // rare: `decode` handles it
    }
    let (w, h) = r.info().size();
    let mut canvas = Canvas::new(w, h)?;
    // A default image without its own fcTL is a still preview, not part of the animation.
    let mut preview = r.info().frame_control.is_none();
    let mut buf = vec![0; r.output_buffer_size().ok_or("APNG too large")?];
    let mut first = true;
    while let Ok(out) = r.next_frame(&mut buf) {
        if std::mem::take(&mut preview) {
            continue;
        }
        let Some(fc) = r.info().frame_control().copied() else { break };
        let ch = match out.color_type {
            ColorType::Rgba => 4,
            ColorType::GrayscaleAlpha => 2,
            _ => return Err("unexpected APNG colour type".into()),
        };
        let dispose = match fc.dispose_op {
            DisposeOp::None => Dispose::Keep,
            DisposeOp::Background => Dispose::Clear,
            // The spec: on the first frame "previous" is the empty canvas.
            DisposeOp::Previous if first => Dispose::Clear,
            DisposeOp::Previous => Dispose::Restore,
        };
        let area = canvas.clip(fc.x_offset, fc.y_offset, fc.width, fc.height);
        canvas.frame(area, dispose, &buf, out.line_size, ch, fc.blend_op == BlendOp::Over);
        first = false;
        let den = if fc.delay_den == 0 { 100 } else { u32::from(fc.delay_den) };
        if !frame(&canvas, delay_ms((u32::from(fc.delay_num) * 1000 + den / 2) / den)) {
            break;
        }
    }
    Ok(())
}

/// GIF frames through the `gif` crate (RGBA, transparent pixels 0), composited here: `image`'s path took
/// 11 ms a 680x240 banner frame.
fn gif_pass(bytes: &[u8], frame: &mut dyn FnMut(&Canvas, u32) -> bool) -> Result<(), String> {
    let mut opts = gif::DecodeOptions::new();
    opts.set_color_output(gif::ColorOutput::RGBA);
    let mut dec = opts.read_info(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut canvas = Canvas::new(dec.width().into(), dec.height().into())?;
    while let Ok(Some(f)) = dec.read_next_frame() {
        let dispose = match f.dispose {
            gif::DisposalMethod::Background => Dispose::Clear,
            gif::DisposalMethod::Previous => Dispose::Restore,
            _ => Dispose::Keep,
        };
        let area = canvas.clip(f.left.into(), f.top.into(), f.width.into(), f.height.into());
        canvas.frame(area, dispose, &f.buffer, usize::from(f.width) * 4, 4, true);
        if !frame(&canvas, delay_ms(u32::from(f.delay) * 10)) {
            break;
        }
    }
    Ok(())
}

/// What becomes of a frame's area before the next frame is drawn.
#[derive(Clone, Copy, PartialEq)]
enum Dispose {
    Keep,
    /// Transparent again.
    Clear,
    /// Back to how it was before the frame.
    Restore,
}

/// `(x, y, w, h)` inside a canvas.
type Area = (usize, usize, usize, usize);

/// A streamed animation's canvas: premultiplied RGBA, and the last frame's disposal.
struct Canvas {
    w: usize,
    h: usize,
    px: Vec<u8>,
    last: Option<(Area, Dispose, Vec<u8>)>,
}

impl Canvas {
    fn new(w: u32, h: u32) -> Result<Self, String> {
        if w == 0 || h == 0 || w > 8192 || h > 8192 {
            return Err(format!("animation canvas {w}x{h}"));
        }
        let (w, h) = (w as usize, h as usize);
        Ok(Self { w, h, px: vec![0; w * h * 4], last: None })
    }

    /// A frame's rectangle, clipped to the canvas (files may say otherwise).
    fn clip(&self, x: u32, y: u32, w: u32, h: u32) -> Area {
        let (x, y) = ((x as usize).min(self.w), (y as usize).min(self.h));
        (x, y, (w as usize).min(self.w - x), (h as usize).min(self.h - y))
    }

    /// Byte range of row `r` of `area`.
    fn row(&self, (x, y, w, _): Area, r: usize) -> std::ops::Range<usize> {
        let at = ((y + r) * self.w + x) * 4;
        at..at + w * 4
    }

    /// Disposes of the last frame, then draws this one: `src` in straight alpha, `stride` bytes a row,
    /// `ch` 4 (RGBA) or 2 (grey + alpha) bytes a pixel, replacing the area or `over` it.
    fn frame(&mut self, area: Area, dispose: Dispose, src: &[u8], stride: usize, ch: usize, over: bool) {
        match self.last.take() {
            Some((a, Dispose::Clear, _)) => {
                for r in 0..a.3 {
                    let range = self.row(a, r);
                    self.px[range].fill(0);
                }
            }
            Some((a, Dispose::Restore, saved)) => {
                for (r, old) in saved.chunks_exact(a.2 * 4).enumerate() {
                    let range = self.row(a, r);
                    self.px[range].copy_from_slice(old);
                }
            }
            _ => {}
        }
        let saved = if dispose == Dispose::Restore { (0..area.3).flat_map(|r| self.px[self.row(area, r)].to_vec()).collect() } else { Vec::new() };
        for r in 0..area.3 {
            let Some(s) = src.get(r * stride..r * stride + area.2 * ch) else { break };
            let range = self.row(area, r);
            for (d, s) in self.px[range].as_chunks_mut::<4>().0.iter_mut().zip(s.chunks_exact(ch)) {
                let (c, a) = if ch == 4 { ([s[0], s[1], s[2]], s[3]) } else { ([s[0]; 3], s[1]) };
                if !over || a == 255 {
                    *d = [mul(c[0], a), mul(c[1], a), mul(c[2], a), a];
                } else if a > 0 {
                    // Premultiplied "over": src * a + dst * (1 - a), alpha alike.
                    let (a, k) = (u32::from(a), 255 - u32::from(a));
                    for i in 0..3 {
                        d[i] = ((u32::from(c[i]) * a + u32::from(d[i]) * k + 127) / 255) as u8;
                    }
                    d[3] = ((a * 255 + u32::from(d[3]) * k + 127) / 255) as u8;
                }
            }
        }
        self.last = Some((area, dispose, saved));
    }

    /// The canvas scaled down by the whole factor that keeps the longest side >= max_side (box filter: cheap
    /// enough for every frame; the GPU scales the rest). 450x880 and 900x1760 layers both end up 450x880 at 768.
    fn image(&self, max_side: u32) -> egui::ColorImage {
        let (w, h) = (self.w as u32, self.h as u32);
        let f = (w.max(h) / max_side.max(1)).clamp(1, w.min(h));
        if f == 1 {
            return egui::ColorImage::from_rgba_premultiplied([self.w, self.h], &self.px);
        }
        let view = image::ImageBuffer::<image::Rgba<u8>, &[u8]>::from_raw(w, h, &self.px[..]).expect("canvas holds w x h pixels");
        let small = image::imageops::thumbnail(&view, w / f, h / f);
        egui::ColorImage::from_rgba_premultiplied([small.width() as usize, small.height() as usize], small.as_raw())
    }
}

fn mul(c: u8, a: u8) -> u8 {
    ((u32::from(c) * u32::from(a) + 127) / 255) as u8
}

/// Decode bytes (also used for local override images) into frames, longest side <= max_side.
/// `keep_frames`: an APNG keeps all its frames, the side drops instead (`fit_side`).
pub fn decode(bytes: &[u8], max_side: u32, keep_frames: bool) -> Result<DecodedImage, String> {
    let max_side = max_side.max(1);
    let reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    let mut count = None;
    let animation = match reader.format() {
        Some(ImageFormat::Gif) => {
            let mut dec = GifDecoder::new(Cursor::new(bytes)).map_err(err)?;
            dec.set_limits(anim_limits()).map_err(err)?;
            Some(dec.into_frames())
        }
        Some(ImageFormat::Png) => {
            let dec = PngDecoder::with_limits(Cursor::new(bytes), anim_limits()).map_err(err)?;
            if dec.is_apng().map_err(err)? {
                count = keep_frames.then(|| apng_frames(bytes)).flatten();
                Some(dec.apng().map_err(err)?.into_frames())
            } else {
                None
            }
        }
        Some(ImageFormat::WebP) => {
            let mut dec = WebPDecoder::new(Cursor::new(bytes)).map_err(err)?;
            dec.set_limits(anim_limits()).map_err(err)?;
            dec.has_animation().then(|| dec.into_frames())
        }
        _ => None,
    };
    let frames = match animation {
        Some(frames) => animation_frames(frames, max_side, count)?,
        None => vec![ImageFrame { image: color_image(reader.decode().map_err(err)?.into_rgba8(), max_side), delay_ms: 0 }],
    };
    Ok(DecodedImage { frames, stream: None })
}

/// The animation decoders default to no limits; a tiny file can declare a huge canvas.
fn anim_limits() -> image::Limits {
    let mut l = image::Limits::default(); // non_exhaustive: start from default
    l.max_image_width = Some(8192);
    l.max_image_height = Some(8192);
    l.max_alloc = Some(256 << 20);
    l
}

fn err(e: image::ImageError) -> String {
    e.to_string()
}

/// Frame count declared by an APNG's `acTL` chunk (it precedes the first `IDAT`). Untrusted: a
/// wrong count only changes the side, the halving in `animation_frames` still bounds memory.
fn apng_frames(bytes: &[u8]) -> Option<u32> {
    let mut at = 8usize; // signature
    loop {
        let head = bytes.get(at..at.checked_add(8)?)?;
        let len = u32::from_be_bytes(head[..4].try_into().ok()?) as usize;
        let data = at.checked_add(8)?;
        match &head[4..] {
            b"acTL" => return Some(u32::from_be_bytes(bytes.get(data..data.checked_add(4)?)?.try_into().ok()?)),
            b"IDAT" => return None,
            _ => at = data.checked_add(len)?.checked_add(4)?, // data + crc
        }
    }
}

/// Decorations keep every frame: the side drops instead until `n` frames of a w×h canvas fit
/// MAX_ANIM_BYTES, at most to half of `max_side` (then frames are halved as for any animation).
fn fit_side(max_side: u32, n: u32, w: u32, h: u32) -> u32 {
    let (long, short) = (w.max(h).max(1), w.min(h).max(1));
    let side = long.min(max_side);
    let fit = (MAX_ANIM_BYTES as f64 * long as f64 / (4.0 * n.max(1) as f64 * short as f64)).sqrt() as u32;
    fit.clamp(max_side.div_ceil(2).min(side), side)
}

/// Collects frames within MAX_ANIM_FRAMES / MAX_ANIM_BYTES: when over budget, every other
/// kept frame is dropped (its delay merged into the previous one) and from then on only every
/// `stride`-th frame is resized, so memory stays bounded even for very long animations.
/// `count` (frames declared by the file): the side is lowered up front to fit them all.
fn animation_frames(frames: Frames<'_>, max_side: u32, count: Option<u32>) -> Result<Vec<ImageFrame>, String> {
    let mut side = None;
    let mut kept: Vec<ImageFrame> = Vec::new();
    let mut bytes = 0usize;
    let mut stride = 1usize;
    for (i, frame) in frames.enumerate() {
        let frame = match frame {
            Ok(f) => f,
            Err(e) if kept.is_empty() => return Err(e.to_string()),
            // Truncated animation: show what decoded, like browsers do.
            Err(_) => break,
        };
        let delay_ms = frame_delay_ms(frame.delay());
        if i % stride != 0 {
            if let Some(last) = kept.last_mut() {
                last.delay_ms += delay_ms;
            }
            continue;
        }
        let (w, h) = frame.buffer().dimensions();
        let side = *side.get_or_insert_with(|| count.map_or(max_side, |n| fit_side(max_side, n, w, h)));
        let image = color_image(frame.into_buffer(), side);
        bytes += image.pixels.len() * 4;
        kept.push(ImageFrame { image, delay_ms });
        while kept.len() > MAX_ANIM_FRAMES || bytes > MAX_ANIM_BYTES {
            if kept.len() == 1 {
                // A single frame is already over budget: show it as a still image.
                return Ok(vec![ImageFrame { delay_ms: 0, ..kept.remove(0) }]);
            }
            kept = halve(kept);
            bytes = kept.iter().map(|f| f.image.pixels.len() * 4).sum();
            stride *= 2;
        }
    }
    if kept.len() == 1 {
        kept[0].delay_ms = 0;
    }
    if kept.is_empty() { Err("image has no frames".into()) } else { Ok(kept) }
}

fn halve(frames: Vec<ImageFrame>) -> Vec<ImageFrame> {
    let mut out = Vec::with_capacity(frames.len().div_ceil(2));
    let mut it = frames.into_iter();
    while let Some(mut f) = it.next() {
        f.delay_ms += it.next().map_or(0, |g| g.delay_ms);
        out.push(f);
    }
    out
}

fn frame_delay_ms(delay: image::Delay) -> u32 {
    let (n, d) = delay.numer_denom_ms();
    delay_ms((n + d / 2).checked_div(d).unwrap_or(0))
}

/// Browsers show delays of 10 ms or less (often 0) as 100 ms; APNG / WebP delays are exact
/// (60 fps = 17 ms) and must stay.
fn delay_ms(ms: u32) -> u32 {
    if ms <= 10 { 100 } else { ms }
}

fn premultiply(img: &mut RgbaImage) {
    for px in img.as_chunks_mut::<4>().0 {
        let a = px[3] as u16;
        if a != 255 {
            for c in &mut px[..3] {
                *c = ((*c as u16 * a + 127) / 255) as u8;
            }
        }
    }
}

/// Premultiply (imageops::resize assumes it; avoids dark fringes), downscale, convert.
fn color_image(mut img: RgbaImage, max_side: u32) -> egui::ColorImage {
    premultiply(&mut img);
    let (w, h) = img.dimensions();
    let longest = w.max(h);
    if longest > max_side {
        let scale = max_side as f64 / longest as f64;
        let nw = ((w as f64 * scale).round() as u32).clamp(1, max_side);
        let nh = ((h as f64 * scale).round() as u32).clamp(1, max_side);
        img = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle);
    }
    let (w, h) = img.dimensions();
    egui::ColorImage::from_rgba_premultiplied([w as usize, h as usize], img.as_raw())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::gif::GifEncoder;
    use image::{Delay, Frame, Rgba};

    fn frames_of(list: Vec<(RgbaImage, u32)>) -> Frames<'static> {
        Frames::new(Box::new(
            list.into_iter().map(|(img, ms)| Ok(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(ms, 1)))),
        ))
    }

    #[test]
    fn cache_key_drops_signature_params() {
        let u = "https://cdn.discordapp.com/attachments/1/2/a.png?ex=65f&is=65e&hm=abc123&";
        assert_eq!(cache_key(u), "https://cdn.discordapp.com/attachments/1/2/a.png");
        let u = "https://media.discordapp.net/attachments/1/2/a.png?ex=1&is=2&hm=3&width=400&height=300";
        assert_eq!(cache_key(u), "https://media.discordapp.net/attachments/1/2/a.png?width=400&height=300");
        let u = "https://cdn.discordapp.com/avatars/1/abc.webp?size=64";
        assert_eq!(cache_key(u), u);
        assert_eq!(cache_key("https://cdn.discordapp.com/x.png"), "https://cdn.discordapp.com/x.png");
        // Same attachment with a refreshed signature hits the same file.
        let a = "https://cdn.discordapp.com/attachments/1/2/a.png?ex=1&is=2&hm=3&";
        let b = "https://cdn.discordapp.com/attachments/1/2/a.png?ex=9&is=8&hm=7&";
        assert_eq!(fnv1a64(cache_key(a).as_bytes()), fnv1a64(cache_key(b).as_bytes()));
    }

    #[test]
    fn fnv_is_stable() {
        assert_eq!(fnv1a64(b""), 0xcbf29ce484222325);
        assert_eq!(fnv1a64(b"a"), 0xaf63dc4c8601ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x85944171f73967e8);
    }

    #[test]
    fn host_allow_list() {
        let ok = |s: &str| host_allowed(&reqwest::Url::parse(s).unwrap());
        assert!(ok("https://cdn.discordapp.com/avatars/1/a.png"));
        assert!(ok("https://media.discordapp.net/stickers/1.gif"));
        assert!(ok("https://images-ext-1.discordapp.net/external/x/y.png"));
        assert!(ok("https://images-ext-12.discordapp.net/external/x/y.png"));
        assert!(ok("https://static.klipy.com/ii/x/y.gif"));
        assert!(ok("https://i.scdn.co/image/ab67616d0000b2734f8ef2c2c1b3a9a1c4d5e6f7"));
        assert!(!ok("http://i.scdn.co/image/x"));
        assert!(!ok("https://i.scdn.co.evil.example/image/x"));
        assert!(!ok("https://static.klipy.com.evil.example/a.gif"));
        assert!(!ok("http://static.klipy.com/a.gif"));
        assert!(!ok("http://cdn.discordapp.com/a.png"));
        assert!(!ok("https://cdn.discordapp.com:8443/a.png"));
        assert!(!ok("https://cdn.discordapp.com.evil.example/a.png"));
        assert!(!ok("https://images-ext-x.discordapp.net/a.png"));
        assert!(!ok("https://discord.com/api/v9/users/@me"));
        assert!(!ok("https://media.tenor.com/a.gif"));
    }

    #[test]
    fn decodes_and_downscales_png() {
        let img = RgbaImage::from_pixel(64, 32, Rgba([200, 100, 50, 255]));
        let mut buf = Vec::new();
        img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Png).unwrap();
        let out = decode(&buf, 16, false).unwrap();
        assert_eq!(out.frames.len(), 1);
        assert_eq!(out.frames[0].image.size, [16, 8]);
        assert_eq!(out.frames[0].delay_ms, 0);
        assert_eq!(out.frames[0].image.pixels[0], egui::Color32::from_rgb(200, 100, 50));
        // Small images are not upscaled.
        assert_eq!(decode(&buf, 128, false).unwrap().frames[0].image.size, [64, 32]);
    }

    #[test]
    fn decodes_animated_gif() {
        let mut buf = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut buf);
            for (i, ms) in [(0u8, 50u32), (1, 0), (2, 70)] {
                let img = RgbaImage::from_pixel(8, 8, Rgba([i * 100, 0, 0, 255]));
                enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(ms, 1))).unwrap();
            }
        }
        let out = decode(&buf, 4, false).unwrap();
        let delays: Vec<u32> = out.frames.iter().map(|f| f.delay_ms).collect();
        assert_eq!(delays, [50, 100, 70]);
        assert!(out.frames.iter().all(|f| f.image.size == [4, 4]));
    }

    #[test]
    fn rejects_gif_with_huge_canvas() {
        // 35-byte GIF: logical screen `side`², 2-color palette, one 1x1 frame.
        let gif = |side: u16| {
            let mut g = b"GIF89a".to_vec();
            g.extend_from_slice(&side.to_le_bytes());
            g.extend_from_slice(&side.to_le_bytes());
            g.extend_from_slice(b"\x80\x00\x00\x00\x00\x00\xff\xff\xff");
            g.extend_from_slice(b"\x2c\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02\x44\x01\x00\x3b");
            g
        };
        assert_eq!(decode(&gif(2), 64, false).unwrap().frames[0].image.size, [2, 2]);
        // 65535² would need ~17 GB per canvas buffer and abort the process.
        assert!(decode(&gif(65535), 64, false).is_err());
    }

    #[test]
    fn caps_frame_count_keeping_duration() {
        let n = MAX_ANIM_FRAMES * 2 + 40;
        let list = (0..n).map(|_| (RgbaImage::new(2, 2), 30)).collect();
        let out = animation_frames(frames_of(list), 64, None).unwrap();
        assert!(out.len() <= MAX_ANIM_FRAMES);
        assert_eq!(out.iter().map(|f| f.delay_ms).sum::<u32>(), n as u32 * 30);
    }

    #[test]
    fn short_delays_play_as_100_ms_but_exact_ones_stay() {
        assert_eq!(frame_delay_ms(Delay::from_numer_denom_ms(50, 3)), 17);
        let d = |ms| frame_delay_ms(Delay::from_numer_denom_ms(ms, 1));
        assert_eq!([d(10), d(0), d(11), d(20)], [100, 100, 11, 20]);
    }

    #[test]
    fn reads_apng_frame_count() {
        let chunk = |t: &[u8; 4], d: &[u8]| [&(d.len() as u32).to_be_bytes()[..], t, d, &[0; 4]].concat();
        let mut apng = b"\x89PNG\r\n\x1a\n".to_vec();
        apng.extend(chunk(b"IHDR", &[0; 13]));
        apng.extend(chunk(b"acTL", &[&120u32.to_be_bytes()[..], &[0; 4]].concat()));
        assert_eq!(apng_frames(&apng), Some(120));
        assert_eq!(apng_frames(&apng[..30]), None, "truncated");
        let mut png = Vec::new();
        RgbaImage::new(2, 2).write_to(&mut Cursor::new(&mut png), ImageFormat::Png).unwrap();
        assert_eq!(apng_frames(&png), None);
    }

    #[test]
    fn fit_side_trades_resolution_for_frames() {
        assert_eq!(fit_side(144, 120, 288, 288), 93);
        assert_eq!(fit_side(216, 120, 288, 288), 108);
        assert_eq!(fit_side(48, 120, 288, 288), 48);
        assert_eq!(fit_side(216, 120, 40, 40), 40);
        assert_eq!(fit_side(96, 60, 288, 288), 96);
        let list = (0..100).map(|_| (RgbaImage::new(288, 288), 30)).collect();
        let out = animation_frames(frames_of(list), 128, Some(100)).unwrap();
        assert_eq!((out.len(), out[0].image.size), (100, [102, 102]));
        assert_eq!(out.iter().map(|f| f.delay_ms).sum::<u32>(), 3000);
    }

    #[test]
    fn caps_frame_bytes() {
        // 6 x 1 MiB = 6 MiB > 4 MiB budget.
        let list = (0..6).map(|_| (RgbaImage::new(512, 512), 40)).collect();
        let out = animation_frames(frames_of(list), 4096, None).unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(out.iter().map(|f| f.delay_ms).sum::<u32>(), 240);
        // One frame alone over budget (1100² x 4 > 4 MiB): still first frame.
        let list = (0..3).map(|_| (RgbaImage::new(1100, 1100), 40)).collect();
        let out = animation_frames(frames_of(list), 4096, None).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].delay_ms, 0);
    }

    #[test]
    fn prune_removes_oldest_first() {
        let dir = std::env::temp_dir().join(format!("rustcord-prune-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let now = SystemTime::now();
        for (name, age) in [("old", 300), ("mid", 200), ("new", 100)] {
            let path = dir.join(name);
            std::fs::write(&path, [0u8; 10]).unwrap();
            let f = std::fs::File::options().write(true).open(&path).unwrap();
            f.set_modified(now - Duration::from_secs(age)).unwrap();
        }
        prune(&dir, 40, 15); // under cap: nothing happens
        assert!(dir.join("old").exists());
        prune(&dir, 25, 15);
        assert!(!dir.join("old").exists() && !dir.join("mid").exists());
        assert!(dir.join("new").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An APNG of `w`×`h` frames, one per colour, `ms` each.
    fn apng(w: u32, h: u32, colors: &[[u8; 4]], ms: u16) -> Vec<u8> {
        use std::io::Write;
        fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
            out.extend((data.len() as u32).to_be_bytes());
            out.extend(kind);
            out.extend(data);
            let mut crc = flate2::Crc::new();
            crc.update(kind);
            crc.update(data);
            out.extend(crc.sum().to_be_bytes());
        }
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut head = [w.to_be_bytes(), h.to_be_bytes()].concat();
        head.extend([8, 6, 0, 0, 0]);
        chunk(&mut out, b"IHDR", &head);
        chunk(&mut out, b"acTL", &[(colors.len() as u32).to_be_bytes(), 0u32.to_be_bytes()].concat());
        let mut seq = 0u32;
        for (i, c) in colors.iter().enumerate() {
            let mut fctl = seq.to_be_bytes().to_vec();
            seq += 1;
            fctl.extend([w.to_be_bytes(), h.to_be_bytes(), [0; 4], [0; 4]].concat());
            fctl.extend(ms.to_be_bytes());
            fctl.extend(1000u16.to_be_bytes());
            fctl.extend([0, 0]);
            chunk(&mut out, b"fcTL", &fctl);
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
            for _ in 0..h {
                z.write_all(&[0]).unwrap();
                z.write_all(&c.repeat(w as usize)).unwrap();
            }
            let z = z.finish().unwrap();
            if i == 0 {
                chunk(&mut out, b"IDAT", &z);
            } else {
                chunk(&mut out, b"fdAT", &[seq.to_be_bytes().to_vec(), z].concat());
                seq += 1;
            }
        }
        chunk(&mut out, b"IEND", &[]);
        out
    }

    #[test]
    fn effect_apng_streams_frame_by_frame() {
        assert!(streams("https://cdn.discordapp.com/assets/profile_effects/x/intro.png"));
        assert!(streams("https://cdn.discordapp.com/banners/3/a_h.gif?size=1024"));
        assert!(streams("https://cdn.discordapp.com/guilds/1/users/3/banners/a_h.gif?size=1024"));
        assert!(!streams("https://cdn.discordapp.com/avatar-decoration-presets/a_x.png"));
        assert!(!streams("https://media.discordapp.net/attachments/1/2/big.gif"));
        let (r, g, b) = ([255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]);
        let img = stream_frames(&apng(8, 4, &[r, g, b], 40), 4).unwrap();
        assert_eq!(img.frames.len(), 1);
        assert_eq!((img.frames[0].image.size, img.frames[0].delay_ms), ([4, 2], 40));
        let stream = img.stream.unwrap();
        let first = |(_, f): (u32, ImageFrame)| f.image.pixels[0];
        let c = |c: [u8; 4]| egui::Color32::from_rgba_premultiplied(c[0], c[1], c[2], c[3]);
        let got: Vec<_> = (0..4).map(|_| first(stream.rx.recv().unwrap())).collect();
        assert_eq!(got, [c(g), c(b), c(r), c(g)], "in order, then looping");
        // A rewind: the next pass starts at the first frame (at most a frame of the old one is still queued).
        stream.rewind.fetch_add(1, Relaxed);
        let next = std::iter::from_fn(|| stream.rx.recv().ok()).find(|(n, _)| *n == 1).unwrap();
        assert_eq!(first(next), c(r));
    }

    #[test]
    fn canvas_box_factor() {
        let mut c = Canvas::new(90, 176).unwrap();
        c.frame(c.clip(0, 0, 90, 176), Dispose::Keep, &[200, 100, 50, 128].repeat(90 * 176), 90 * 4, 4, false);
        let img = c.image(77);
        assert_eq!(img.size, [45, 88]);
        assert_eq!(img.pixels[0], egui::Color32::from_rgba_premultiplied(100, 50, 25, 128));
        assert_eq!(Canvas::new(45, 88).unwrap().image(77).size, [45, 88]);
    }

    #[test]
    fn canvas_blends_premultiplied_and_disposes() {
        let px = |c: &Canvas, x: usize, y: usize| c.px[(y * c.w + x) * 4..][..4].to_vec();
        let mut c = Canvas::new(4, 2).unwrap();
        c.frame(c.clip(0, 0, 4, 2), Dispose::Keep, &[0, 0, 255, 255].repeat(8), 16, 4, false);
        // Half-transparent red over blue: premultiplied 50/50; a transparent pixel keeps the blue.
        c.frame(c.clip(1, 0, 2, 1), Dispose::Restore, &[255, 0, 0, 128, 255, 0, 0, 0], 8, 4, true);
        assert_eq!((px(&c, 1, 0), px(&c, 2, 0)), (vec![128, 0, 127, 255], vec![0, 0, 255, 255]));
        // Restore: back to blue before the next frame; grey + alpha, clipped at the edge, Clear after.
        c.frame(c.clip(3, 1, 5, 5), Dispose::Clear, &[100, 255].repeat(25), 10, 2, false);
        assert_eq!((px(&c, 1, 0), px(&c, 3, 1)), (vec![0, 0, 255, 255], vec![100, 100, 100, 255]));
        c.frame(c.clip(0, 0, 1, 1), Dispose::Keep, &[0, 255, 0, 255], 4, 4, false);
        assert_eq!((px(&c, 3, 1), px(&c, 0, 0)), (vec![0; 4], vec![0, 255, 0, 255]));
    }

    #[test]
    fn gif_banner_streams_every_frame() {
        // 3 frames of 4x2: blue, a red pixel on top (the rest transparent, kept), green after a clear.
        let mut out = Vec::new();
        {
            let mut enc = gif::Encoder::new(&mut out, 4, 2, &[0, 0, 255, 255, 0, 0, 0, 255, 0, 0, 0, 0]).unwrap();
            let frame = |left, width, height, buffer: Vec<u8>, dispose| gif::Frame { left, width, height, buffer: buffer.into(), dispose, transparent: Some(3), delay: 2, ..Default::default() };
            enc.write_frame(&frame(0, 4, 2, vec![0; 8], gif::DisposalMethod::Keep)).unwrap();
            enc.write_frame(&frame(1, 2, 1, vec![1, 3], gif::DisposalMethod::Background)).unwrap();
            enc.write_frame(&frame(0, 1, 1, vec![2], gif::DisposalMethod::Keep)).unwrap();
        }
        let img = stream_frames(&out, 600).unwrap();
        assert_eq!((img.frames[0].image.size, img.frames[0].delay_ms), ([4, 2], 20));
        let c = |r, g, b| egui::Color32::from_rgba_premultiplied(r, g, b, 255);
        assert_eq!(img.frames[0].image.pixels[1], c(0, 0, 255));
        let rx = img.stream.unwrap().rx;
        let second = rx.recv().unwrap().1.image.pixels;
        assert_eq!((second[1], second[2]), (c(255, 0, 0), c(0, 0, 255)), "drawn over the first frame");
        let third = rx.recv().unwrap().1.image.pixels;
        assert_eq!((third[0], third[1]), (c(0, 255, 0), egui::Color32::TRANSPARENT), "the red pixel cleared");
        assert_eq!(rx.recv().unwrap().1.image.pixels[1], c(0, 0, 255), "loops");
        assert!(stream_frames(b"\x89PNG not animated", 600).is_none());
    }
}
