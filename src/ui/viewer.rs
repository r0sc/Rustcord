//! In-app media (docs/MEDIA_DESIGN.md §5–6): inline video players and gifv in messages, and the
//! lightbox (`Modal::Media`) for a message's images, GIFs and videos with zoom, pan, arrows, Save,
//! Copy image, Copy link and Open in browser as an explicit secondary action. Playback engine:
//! `crate::media`. Quests: `video_slot`.
//!
//! OWNERSHIP: media (`src/media/*`, this file).

use super::icons::{self, Icon};
use super::markdown;
use super::messages::{self, Btn, Pal};
use super::{Modal, UiCx};
use crate::api::cdn;
use crate::config::AnimateMode;
use crate::media::{self, Fail, Opts, Player, State, Status, net};
use crate::model::{Attachment, Embed, EmbedMedia, Id, Message};
use crate::voice::video::VideoKey;
use egui::{Align2, Color32, CursorIcon, FontId, Key, Modifiers, Rect, Sense, Stroke, Vec2, pos2, vec2};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::time::Duration;

const TITLE_BAR_H: f32 = 32.0;
/// Lightbox top and bottom bars; room for the arrows left and right of the media.
const BAR_H: f32 = 56.0;
const SIDE: f32 = 72.0;
const CONTROLS_H: f32 = 40.0;
const MAX_GIFV: usize = 3;
const ZOOM_MAX_SIDE: f32 = 2560.0;
/// Players not drawn this long are dropped (the worker frees itself as well).
const DROP_AFTER: Duration = Duration::from_secs(30);
const SUPPRESS_EMBEDS: u64 = 1 << 2;
/// Black 56 %: play and arrow discs, the GIF badge.
const DISC: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 143);
/// Opaque ink on media (≈ white 92 % on black): icons with overlapping parts must be opaque.
const INK: Color32 = Color32::from_gray(235);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Image { gif: bool },
    Video,
    Gifv,
}

#[derive(Clone, Debug)]
pub enum Src {
    Attachment(Attachment),
    Embed(EmbedMedia),
    /// A CDN URL shown as is (ViewIcons: avatars, banners, server icons; no proxy resize).
    Url(String),
}

/// One image or video of a message, as the lightbox steps through them.
#[derive(Clone, Debug)]
pub struct MediaItem {
    pub kind: Kind,
    /// The picture, or a video's poster (None: black until the first frame).
    pub src: Option<Src>,
    pub play_url: Option<String>,
    /// Save and Copy image: Discord's copy of the file.
    pub file_url: String,
    /// Open in browser and Copy link.
    pub open_url: String,
    pub filename: String,
    pub natural: Option<Vec2>,
    /// WebM / Matroska: the can't-play card without any request.
    pub unsupported: bool,
}

impl MediaItem {
    /// The picture at `w`×`h` px through the media proxy.
    pub fn picture_url(&self, w: u32, h: u32) -> Option<String> {
        match self.src.as_ref()? {
            Src::Attachment(a) => Some(cdn::attachment_preview_url(a, w, h)),
            Src::Embed(m) => cdn::embed_media_url(m, w, h),
            Src::Url(u) => Some(u.clone()),
        }
    }

    fn key(&self) -> u64 {
        media::key_of(self.play_url.as_deref().unwrap_or(&self.file_url))
    }
}

pub struct Author {
    pub id: Id,
    pub name: String,
    pub avatar: String,
    pub time: String,
}

enum Done {
    Copied(Result<egui::ColorImage, String>),
    Saved(String, Result<(), String>),
}

struct Slot {
    player: Player,
    gifv: bool,
}

/// `ViewState::media_ui`: the lightbox's items and every player (dropped on logout = stopped).
pub struct MediaUi {
    pub items: Vec<MediaItem>,
    pub index: usize,
    pub author: Option<Author>,
    /// (scale in points per image pixel, pan of the image centre); None = fitted.
    zoom: Option<(f32, Vec2)>,
    /// Where the click that opened the lightbox was: the media grows from there.
    from: Option<egui::Pos2>,
    players: HashMap<u64, Slot>,
    /// gifv players some copy on screen wants playing this frame (`end_frame` pauses the rest).
    gifv_want: HashSet<u64>,
    textures: HashMap<u64, (u64, egui::TextureHandle)>,
    /// Videos without a CDN poster already checked for a disk-cache copy (whose first frame is
    /// the poster, shown once).
    poster_checked: HashSet<u64>,
    done: (mpsc::Sender<Done>, mpsc::Receiver<Done>),
    /// Session volume (not saved).
    volume: f32,
    muted: bool,
}

impl Default for MediaUi {
    fn default() -> Self {
        MediaUi {
            items: Vec::new(),
            index: 0,
            author: None,
            zoom: None,
            from: None,
            players: HashMap::new(),
            gifv_want: HashSet::new(),
            textures: HashMap::new(),
            poster_checked: HashSet::new(),
            done: mpsc::channel(),
            volume: 1.0,
            muted: false,
        }
    }
}

fn natural(w: Option<u32>, h: Option<u32>) -> Option<Vec2> {
    match (w, h) {
        (Some(w), Some(h)) if w > 0 && h > 0 => Some(vec2(w as f32, h as f32)),
        _ => None,
    }
}

pub fn attachment_item(a: &Attachment) -> MediaItem {
    let video = a.is_video();
    let ct = a.content_type.as_deref().unwrap_or("");
    let name = a.filename.to_ascii_lowercase();
    MediaItem {
        kind: if video { Kind::Video } else { Kind::Image { gif: ct == "image/gif" || name.ends_with(".gif") } },
        // A video's poster is the proxy's still; without a proxy URL there is none.
        src: (!video || cdn::is_discord_media_host(&a.proxy_url)).then(|| Src::Attachment(a.clone())),
        play_url: video.then(|| a.url.clone()),
        file_url: a.url.clone(),
        open_url: a.url.clone(),
        filename: a.filename.clone(),
        natural: natural(a.width, a.height),
        unsupported: video && (matches!(ct, "video/webm" | "video/x-matroska") || name.ends_with(".webm") || name.ends_with(".mkv")),
    }
}

/// Media-only embeds: `image`, `gifv`, and `video` with a Discord-proxied file. YouTube and
/// other players without a proxy are None (their card opens the browser).
pub fn embed_item(e: &Embed) -> Option<MediaItem> {
    let proxied = |m: &EmbedMedia| m.proxy_url.clone().filter(|p| cdn::is_discord_media_host(p));
    let link = |m: &EmbedMedia| e.url.clone().filter(|u| markdown::is_web_scheme(u)).unwrap_or_else(|| m.url.clone());
    let kind = e.kind.as_deref()?;
    match (kind, e.video.as_ref().and_then(|v| Some((v, proxied(v)?)))) {
        ("gifv" | "video", Some((v, url))) => {
            let thumb = e.thumbnail.clone();
            let lower = url.split('?').next().unwrap_or_default().to_ascii_lowercase();
            Some(MediaItem {
                kind: if kind == "gifv" { Kind::Gifv } else { Kind::Video },
                natural: natural(v.width, v.height).or_else(|| thumb.as_ref().and_then(|t| natural(t.width, t.height))),
                src: thumb.map(Src::Embed),
                file_url: url.clone(),
                open_url: link(v),
                filename: net::file_name(&url),
                unsupported: v.content_type.as_deref() == Some("video/webm") || lower.ends_with(".webm"),
                play_url: Some(url),
            })
        }
        ("image" | "gifv", _) => embed_image_item(e, e.thumbnail.as_ref().or(e.image.as_ref())?),
        _ => None,
    }
}

/// One of `e`'s pictures (`m`: its image or thumbnail, also of a rich or link embed) as a
/// lightbox item; None when it isn't on Discord's media proxy.
pub fn embed_image_item(e: &Embed, m: &EmbedMedia) -> Option<MediaItem> {
    let file = m.proxy_url.clone().filter(|p| cdn::is_discord_media_host(p))?;
    let gif = e.kind.as_deref() == Some("gifv") || file.split('?').next().unwrap_or_default().to_ascii_lowercase().ends_with(".gif");
    let open_url = e.url.clone().filter(|u| markdown::is_web_scheme(u)).unwrap_or_else(|| m.url.clone());
    Some(MediaItem { kind: Kind::Image { gif }, src: Some(Src::Embed(m.clone())), play_url: None, filename: net::file_name(&m.url), open_url, file_url: file, natural: natural(m.width, m.height), unsupported: false })
}

/// A message's media in lightbox order: uploads (images and videos, uncovered spoilers only)
/// when Settings > Chat shows uploads, then media embeds when it shows media posted as links.
pub fn media_items(msg: &Message, uploads: bool, links: bool, revealed: impl Fn(usize) -> bool) -> Vec<MediaItem> {
    let mut out: Vec<MediaItem> = msg
        .attachments
        .iter()
        .enumerate()
        .filter(|(i, a)| uploads && (a.is_image() || a.is_video()) && (!a.is_spoiler() || revealed(*i)))
        .map(|(_, a)| attachment_item(a))
        .collect();
    if links && msg.flags & SUPPRESS_EMBEDS == 0 {
        out.extend(msg.embeds.iter().filter_map(embed_item));
    }
    out
}

/// Opens the lightbox on `msg`'s media at the item with `clicked`'s file.
pub fn open(ctx: &egui::Context, cx: &mut UiCx<'_>, msg: &Message, author: Option<Author>, clicked: &MediaItem) {
    let shows = &cx.state.account.settings;
    let (uploads, links) = (shows.chat_shows(9), shows.chat_shows(10));
    let all = cx.state.spoilers_open();
    let items = media_items(msg, uploads, links, |i| all || cx.view.revealed_spoilers.contains(&(msg.id, 1000 + i)));
    let index = items.iter().position(|i| i.file_url == clicked.file_url);
    let items = if index.is_some() { items } else { vec![clicked.clone()] };
    open_items(ctx, cx, items, index.unwrap_or(0), author);
}

/// ViewIcons: opens the CDN image `url` (an avatar, banner or server icon; `aspect` = width / height,
/// as these have no stored size) in the lightbox. Animated ones (gif) animate.
pub fn open_icon(ctx: &egui::Context, cx: &mut UiCx<'_>, url: String, aspect: f32) {
    let gif = url.split('?').next().unwrap_or_default().to_ascii_lowercase().ends_with(".gif");
    let item = MediaItem { kind: Kind::Image { gif }, src: Some(Src::Url(url.clone())), play_url: None, filename: net::file_name(&url), open_url: url.clone(), file_url: url, natural: Some(vec2(1024.0 * aspect.max(1.0), 1024.0 / aspect.min(1.0))), unsupported: false };
    open_items(ctx, cx, vec![item], 0, None);
}

fn open_items(ctx: &egui::Context, cx: &mut UiCx<'_>, items: Vec<MediaItem>, index: usize, author: Option<Author>) {
    let mu = &mut cx.view.media_ui;
    (mu.index, mu.items, mu.author, mu.zoom) = (index.min(items.len().saturating_sub(1)), items, author, None);
    mu.from = ctx.input(|i| i.pointer.any_click().then(|| i.pointer.interact_pos()).flatten());
    cx.view.modal = Some(Modal::Media);
    // Enter, arrows and Space belong to the lightbox, not the composer below it.
    ctx.memory_mut(|m| m.stop_text_input());
}

/// Whether `e` plays inline (a media embed with a Discord-proxied video).
pub fn plays_inline(e: &Embed) -> bool {
    embed_item(e).is_some_and(|i| i.kind != Kind::Video || i.play_url.is_some())
}

/// Ends the frame: results of Save / Copy as toasts, players not drawn for 30 s (or whose worker
/// exited) and their textures dropped. One call per frame (ui/mod.rs).
pub fn end_frame(ctx: &egui::Context, cx: &mut UiCx<'_>) {
    let mu = &mut cx.view.media_ui;
    let mut toasts = Vec::new();
    while let Ok(d) = mu.done.1.try_recv() {
        toasts.push(match d {
            Done::Copied(Ok(img)) => {
                ctx.copy_image(img);
                ("Image copied".to_owned(), false)
            }
            Done::Copied(Err(_)) => ("Could not copy the image. Check your connection, then try again.".to_owned(), true),
            Done::Saved(name, Ok(())) => (format!("Saved {name}"), false),
            Done::Saved(name, Err(_)) => (format!("Could not save {name}. Check your connection, then try again."), true),
        });
    }
    // One play/pause decision per gifv per frame: two copies (inline and lightbox) never fight.
    for (k, s) in &mu.players {
        if s.gifv && !mu.gifv_want.contains(k) && s.player.status().state == State::Playing {
            s.player.pause();
        }
    }
    mu.gifv_want.clear();
    mu.players.retain(|_, s| s.player.status().state != State::Freed && s.player.idle() < DROP_AFTER);
    mu.textures.retain(|k, _| mu.players.contains_key(k));
    if cx.view.modal != Some(Modal::Media) && !mu.items.is_empty() {
        mu.items.clear();
    }
    for (text, error) in toasts {
        cx.view.toast(text, error);
    }
}

fn clock(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// `size` scaled to fit `rect` (up or down), centred.
fn fit_rect(rect: Rect, size: Vec2) -> Rect {
    let s = (rect.width() / size.x.max(1.0)).min(rect.height() / size.y.max(1.0));
    Rect::from_center_size(rect.center(), size * s)
}

/// The pan that keeps the image point under `at` (relative to the media centre) in place when
/// the scale goes from `s0` to `s1`.
fn zoom_about(s0: f32, s1: f32, pan: Vec2, at: Vec2) -> Vec2 {
    at - (at - pan) * (s1 / s0)
}

/// Pan limited so an image larger than the room never shows a gap on a side it overflows; a
/// smaller one stays centred.
fn clamp_pan(pan: Vec2, image: Vec2, room: Vec2) -> Vec2 {
    let lim = ((image - room) / 2.0).max(Vec2::ZERO);
    vec2(pan.x.clamp(-lim.x, lim.x), pan.y.clamp(-lim.y, lim.y))
}

/// The colours of controls on media: `vector_button` with on-media ink and hover.
fn media_pal(pal: &Pal) -> Pal {
    let mut p = *pal;
    (p.muted, p.text, p.bg) = (INK, Color32::WHITE, Color32::BLACK);
    (p.tk.hover, p.tk.selected) = (pal.tk.media_hover, pal.tk.media_hover);
    p
}

/// A round black-56 % button with an icon (play, arrows, download on media).
fn disc(ui: &mut egui::Ui, pal: &Pal, rect: Rect, id: egui::Id, icon: Icon, tip: &str) -> egui::Response {
    let r = ui.interact(rect, id, Sense::click());
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tip));
    let fill = if r.hovered() { DISC.blend(pal.tk.media_hover) } else { DISC };
    ui.painter().circle_filled(rect.center(), rect.width() / 2.0, fill);
    let side = (rect.width() / 2.0).clamp(16.0, 24.0);
    icons::paint(ui.painter(), icon, Rect::from_center_size(rect.center(), Vec2::splat(side)), INK, Color32::BLACK);
    if r.has_focus() {
        messages::focus_ring(ui, pal, rect, rect.width() / 2.0);
    }
    r.on_hover_text(tip).on_hover_cursor(CursorIcon::PointingHand)
}

/// Black 0 → 64 % from the top of `rect` to its bottom (under the controls), the bottom corners
/// rounded by `radius` like the player's.
fn shade(p: &egui::Painter, rect: Rect, radius: f32) {
    let r = radius.min(rect.height() / 2.0);
    // Clockwise: the top edge, then the bottom-right and bottom-left arcs; a fan fills it.
    let mut pts = vec![rect.left_top(), rect.right_top()];
    for (c, from) in [(pos2(rect.right() - r, rect.bottom() - r), 0.0f32), (pos2(rect.left() + r, rect.bottom() - r), 90.0)] {
        pts.extend((0..=8).map(|i| {
            let a = (from + 90.0 * i as f32 / 8.0).to_radians();
            c + r * vec2(a.cos(), a.sin())
        }));
    }
    let mut m = egui::Mesh::default();
    for q in &pts {
        let t = ((q.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        m.colored_vertex(*q, Color32::from_black_alpha((163.0 * t).round() as u8));
    }
    for i in 1..pts.len() as u32 - 1 {
        m.add_triangle(0, i, i + 1);
    }
    p.add(m);
}

/// The latest frame of player `key` as a texture (uploaded when it changed).
fn frame_texture(ctx: &egui::Context, mu: &mut MediaUi, key: u64) -> Option<(egui::TextureId, Vec2)> {
    if let Some((generation, image)) = media::frames(ctx).latest(&VideoKey::Media(key)) {
        match mu.textures.get_mut(&key) {
            Some((g, tex)) => {
                if *g != generation {
                    *g = generation;
                    tex.set(image, egui::TextureOptions::LINEAR);
                }
            }
            None => {
                mu.textures.insert(key, (generation, ctx.load_texture("media", image, egui::TextureOptions::LINEAR)));
            }
        }
    }
    mu.textures.get(&key).map(|(_, t)| (t.id(), t.size_vec2()))
}

fn paint_texture(p: &egui::Painter, rect: Rect, tex: egui::TextureId, size: Vec2, radius: u8) {
    let img = fit_rect(rect, size);
    // Rounded only when it covers the rect; letterboxed edges stay square.
    // 2 px slack: an even-sized converted frame fits a fraction of a pixel short.
    let round = if img.width() >= rect.width() - 2.0 && img.height() >= rect.height() - 2.0 { radius } else { 0 };
    p.add(egui::epaint::RectShape::filled(img, round, Color32::WHITE).with_texture(tex, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))));
}

/// The poster (CDN still) or a black fill.
fn paint_poster(ui: &egui::Ui, cx: &mut UiCx<'_>, item: &MediaItem, rect: Rect, radius: u8) {
    let px = rect.size() * ui.ctx().pixels_per_point();
    let tex = item.picture_url(px.x.round() as u32, px.y.round() as u32).and_then(|u| cx.images.get(ui.ctx(), &u, px.max_elem().ceil() as u32, false));
    if let Some(t) = tex {
        paint_texture(ui.painter(), rect, t.id, t.size, radius);
    }
}

/// `item`'s player, created (paused, showing its first frame) when missing. Gifv: at most
/// `MAX_GIFV` exist; the least recently drawn one gives way unless all are on screen.
fn ensure_player<'a>(ctx: &egui::Context, cx: &'a mut UiCx<'_>, item: &MediaItem, opts: Opts) -> Option<&'a Player> {
    let (key, gifv) = (item.key(), item.kind == Kind::Gifv);
    let mu = &mut cx.view.media_ui;
    if mu.players.get(&key).is_some_and(|s| opts.no_seek_ahead && !s.player.opts.no_seek_ahead) {
        mu.players.remove(&key);
    }
    if !mu.players.contains_key(&key) {
        let url = item.play_url.as_deref()?;
        if gifv && mu.players.values().filter(|s| s.gifv).count() >= MAX_GIFV {
            let (k, idle) = mu.players.iter().filter(|(_, s)| s.gifv).map(|(k, s)| (*k, s.player.idle())).max_by_key(|g| g.1)?;
            if idle < Duration::from_millis(200) {
                return None;
            }
            mu.players.remove(&k);
        }
        mu.players.insert(key, Slot { player: Player::open(ctx, url, opts), gifv });
    }
    mu.players.get(&key).map(|s| &s.player)
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Place {
    Inline,
    Lightbox,
    Slot,
}

/// What a click on a player's chrome asked for beyond playback.
#[derive(Default)]
struct Out {
    #[allow(dead_code, reason = "read by video_slot, which Quests' video modal adopts")]
    status: Option<Status>,
    /// Expand (inline) or Collapse (lightbox).
    toggle_full: bool,
}

/// The video player in `rect`: frame or poster, play disc, controls, error card.
fn player_view(ui: &mut egui::Ui, cx: &mut UiCx<'_>, item: &MediaItem, rect: Rect, place: Place, no_seek_ahead: bool) -> Out {
    let _t = super::frame_stats::scope("+player");
    let ctx = ui.ctx().clone();
    let pal = Pal::new(cx.theme);
    let mp = media_pal(&pal);
    let key = item.key();
    let id = egui::Id::unique(("rc_media", key, place));
    let radius = if place == Place::Inline { pal.radius } else { 0 };
    let opts = Opts { no_seek_ahead, output: cx.settings.voice_output.clone(), ..Opts::default() };
    // Without a CDN poster, a file already on disk shows its own first frame (no request), once:
    // after its player freed itself the play disc rests on black instead of reopening it.
    let url = item.play_url.clone().unwrap_or_default();
    if item.src.is_none() && !item.unsupported && !cx.view.media_ui.players.contains_key(&key) && cx.view.media_ui.poster_checked.insert(key) && net::is_cached(&url) {
        ensure_player(&ctx, cx, item, opts.clone());
    }
    let big = cx.view.modal == Some(Modal::Media) && cx.view.media_ui.items.get(cx.view.media_ui.index).is_some_and(|i| i.key() == key);
    let ppp = ctx.pixels_per_point();
    let mu = &mut cx.view.media_ui;
    let status = mu.players.get(&key).map(|s| {
        s.player.drawn();
        // The lightbox sizes a video it shows; the inline copy below it leaves that alone.
        if place != Place::Inline || !big {
            let px = rect.size() * ppp;
            s.player.set_target(px.x.round() as u32, px.y.round() as u32);
        }
        s.player.set_volume(if mu.muted { 0.0 } else { mu.volume });
        s.player.status()
    });
    // A paused video on screen stays alive: the worker frees one not drawn for 30 s, and a still
    // window doesn't repaint on its own.
    if status.is_some_and(|s| s.state == State::Paused && s.position_ms > 0) {
        ctx.request_repaint_after(Duration::from_secs(20));
    }
    let resp = ui.interact(rect, id, Sense::click());
    let p = ui.painter_at(rect);
    let fail = if item.unsupported { Some(Fail::Unsupported) } else if let Some(State::Failed(f)) = status.map(|s| s.state) { Some(f) } else { None };
    if let Some(f) = fail {
        error_card(ui, cx, item, rect, f);
        return Out { status, ..Out::default() };
    }
    // After the card: its light surface never gets a black edge.
    p.rect_filled(rect, radius, Color32::BLACK);
    match frame_texture(&ctx, &mut cx.view.media_ui, key).filter(|_| status.is_some()) {
        Some((tex, size)) => paint_texture(&p, rect, tex, size, radius),
        None => paint_poster(ui, cx, item, rect, radius),
    }
    let state = status.map(|s| s.state);
    let playing = state == Some(State::Playing);
    let mut out = Out { status, toggle_full: false };
    let toggle = |cx: &mut UiCx<'_>| {
        if playing {
            if let Some(s) = cx.view.media_ui.players.get(&key) {
                s.player.pause();
            }
            return;
        }
        // One video plays at a time, and no voice message under it.
        for (k, s) in &cx.view.media_ui.players {
            if *k != key && !s.gifv {
                s.player.pause();
            }
        }
        crate::plugins::voice_messages::stop();
        if let Some(pl) = ensure_player(&ctx, cx, item, opts.clone()) {
            pl.play();
        }
        ctx.request_repaint();
    };

    // Controls: on hover (hidden 2 s after the pointer stops while playing), and while paused
    // after a start.
    let now = ctx.input(|i| i.time);
    let moved_id = id.with("moved");
    if resp.hovered() && ctx.input(|i| i.pointer.delta() != Vec2::ZERO) {
        ctx.data_mut(|d| d.insert_temp(moved_id, now));
    }
    let moved = ctx.data(|d| d.get_temp::<f64>(moved_id)).unwrap_or(f64::NEG_INFINITY);
    let hover = ui.rect_contains_pointer(rect);
    let started = status.is_some_and(|s| s.position_ms > 0 || matches!(s.state, State::Playing | State::Ended));
    let show = (hover && (!playing || now - moved < 2.0)) || (started && !playing) || (place == Place::Lightbox && !playing);
    if playing && hover && now - moved < 2.0 {
        ctx.request_repaint_after(Duration::from_secs_f64(2.0 - (now - moved) + 0.05));
    }
    let bar = Rect::from_min_max(pos2(rect.left(), rect.bottom() - CONTROLS_H), rect.max);
    let mut on_controls = false;
    if show && rect.height() >= CONTROLS_H * 2.0 {
        let (c, full) = controls(ui, cx, &mp, item, bar, id, status, place, no_seek_ahead, &toggle);
        on_controls = c;
        out.toggle_full = full;
    }
    // The big play disc while not playing; a spinner in it while loading.
    if !playing {
        let d = Rect::from_center_size(rect.center(), Vec2::splat(48.0_f32.min(rect.height() - 8.0)));
        let loading = state == Some(State::Loading);
        let (icon, tip) = if state == Some(State::Ended) { (Icon::Refresh, "Replay") } else { (Icon::Play, "Play") };
        if loading {
            ui.painter().circle_filled(d.center(), d.width() / 2.0, DISC);
            egui::Spinner::new().size(24.0).color(INK).paint_at(ui, Rect::from_center_size(d.center(), Vec2::splat(24.0)));
        } else if disc(ui, &pal, d, id.with("play"), icon, tip).clicked() {
            toggle(cx);
        }
    } else if status.is_some_and(|s| s.buffering) {
        egui::Spinner::new().size(24.0).color(INK).paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(24.0)));
    }
    // Download, top right on hover (inline; the lightbox has it in its top bar).
    if place == Place::Inline && hover && rect.width() > 120.0 {
        let d = Rect::from_min_size(pos2(rect.right() - 40.0, rect.top() + 8.0), Vec2::splat(32.0));
        if disc(ui, &pal, d, id.with("dl"), Icon::Download, "Save video").clicked() {
            save(&ctx, cx, item);
        }
    }
    if resp.clicked() && !on_controls {
        toggle(cx);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, &item.filename));
    if place == Place::Inline {
        resp.on_hover_cursor(CursorIcon::PointingHand);
    }
    out
}

/// The 40 px bar: play/pause, time, seek, volume, full screen. Returns (pointer on it, full
/// screen toggled).
#[allow(clippy::too_many_arguments)]
fn controls(ui: &mut egui::Ui, cx: &mut UiCx<'_>, mp: &Pal, item: &MediaItem, bar: Rect, id: egui::Id, status: Option<Status>, place: Place, no_seek_ahead: bool, toggle: &dyn Fn(&mut UiCx<'_>)) -> (bool, bool) {
    let key = item.key();
    shade(ui.painter(), bar, if place == Place::Inline { f32::from(mp.radius) } else { 0.0 });
    let s = status.unwrap_or(Status { state: State::Paused, position_ms: 0, duration_ms: 0, played_ms: 0, max_ms: 0, size: (0, 0), has_audio: true, buffering: false });
    let playing = s.state == State::Playing;
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(8.0, 4.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    ui.spacing_mut().item_spacing.x = 4.0;
    let mut full = false;
    let (icon, tip) = if playing { (Icon::Pause, "Pause") } else { (Icon::Play, "Play") };
    if messages::vector_button(&mut ui, mp, icon, tip, 32.0, false, Color32::WHITE).clicked() {
        toggle(cx);
    }
    // Rounded: a 2.99 s clip reads "0:03", and its end "0:03 / 0:03".
    let time = format!("{} / {}", clock(s.position_ms + 500), clock(s.duration_ms + 500));
    ui.label(egui::RichText::new(time).size(mp.meta()).color(INK));
    ui.add_space(4.0);
    // Right to left: full screen, volume; the seek bar takes the rest.
    let right = 32.0 + 4.0 + 32.0 + 64.0 + if place == Place::Slot { 0.0 } else { 36.0 };
    let seek_w = (ui.available_width() - right - 8.0).max(24.0);
    let (track, _) = ui.allocate_exact_size(vec2(seek_w, 24.0), Sense::hover());
    seek_bar(&mut ui, cx, mp, key, track, id.with("seek"), s, no_seek_ahead);
    ui.add_space(4.0);
    volume(&mut ui, cx, mp, id.with("vol"), s.has_audio);
    if place != Place::Slot {
        let (icon, tip) = if place == Place::Lightbox { (Icon::Collapse, "Exit full screen") } else { (Icon::Expand, "Full screen") };
        full = messages::vector_button(&mut ui, mp, icon, tip, 32.0, false, Color32::WHITE).clicked();
    }
    (ui.rect_contains_pointer(bar), full)
}

/// 4 px track, the played part, a 12 px knob on hover or drag; ←/→ 5 s while focused.
#[allow(clippy::too_many_arguments)]
fn seek_bar(ui: &mut egui::Ui, cx: &mut UiCx<'_>, mp: &Pal, key: u64, rect: Rect, id: egui::Id, s: Status, no_seek_ahead: bool) {
    let r = ui.interact(rect, id, Sense::click_and_drag());
    let dur = s.duration_ms.max(1);
    let limit = if no_seek_ahead { s.max_ms.min(dur) } else { dur };
    let frac = |x: f32| ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
    let at = |x: f32| ((frac(x) as f64 * dur as f64) as u64).min(limit);
    r.widget_info(|| egui::WidgetInfo::slider(true, s.position_ms as f64 / 1000.0, "Seek"));
    let player = cx.view.media_ui.players.get(&key).map(|s| &s.player);
    let mut shown = s.position_ms;
    if let Some(p) = r.interact_pointer_pos() {
        shown = at(p.x);
        if (r.clicked() || r.drag_stopped())
            && let Some(pl) = player
        {
            pl.seek(shown);
        }
    }
    if r.has_focus() {
        let filter = egui::EventFilter { horizontal_arrows: true, ..Default::default() };
        ui.memory_mut(|m| m.set_focus_lock_filter(r.id, filter));
        let step = [(Key::ArrowLeft, -5000i64), (Key::ArrowRight, 5000)].into_iter().find(|(k, _)| ui.input_mut(|i| i.consume_key(Modifiers::NONE, *k)));
        if let (Some((_, d)), Some(pl)) = (step, player) {
            pl.seek((s.position_ms as i64 + d).clamp(0, limit as i64) as u64);
        }
    }
    let p = ui.painter();
    let cy = rect.center().y;
    let x = |ms: u64| rect.left() + rect.width() * (ms as f32 / dur as f32).clamp(0.0, 1.0);
    let line = |a: f32, b: f32, c: Color32| p.rect_filled(Rect::from_min_max(pos2(a, cy - 2.0), pos2(b, cy + 2.0)), 2, c);
    line(rect.left(), rect.right(), mp.tk.on_media.gamma_multiply(0.3));
    if no_seek_ahead {
        // What can't be skipped to yet stays dimmer.
        line(x(limit), rect.right(), Color32::from_black_alpha(90));
    }
    line(rect.left(), x(shown), INK);
    if r.hovered() || r.dragged() || r.has_focus() {
        p.circle_filled(pos2(x(shown), cy), 6.0, Color32::WHITE);
    }
    if r.has_focus() {
        messages::focus_ring(ui, mp, rect.shrink2(vec2(0.0, 6.0)), 4.0);
    }
    if let Some(h) = r.hover_pos() {
        r.on_hover_text_at_pointer(clock(at(h.x))).on_hover_cursor(CursorIcon::PointingHand);
    }
}

/// Mute toggle; hovering it reveals a 64 px volume slider to its left (session only).
fn volume(ui: &mut egui::Ui, cx: &mut UiCx<'_>, mp: &Pal, id: egui::Id, has_audio: bool) {
    let mu = &mut cx.view.media_ui;
    let (slider, _) = ui.allocate_exact_size(vec2(64.0, 24.0), Sense::hover());
    let silent = mu.muted || mu.volume <= 0.0;
    let (icon, tip) = match (has_audio, silent) {
        (false, _) => (Icon::VoiceOff, "This video's sound can't play here"),
        (true, true) => (Icon::VoiceOff, "Unmute"),
        (true, false) => (Icon::Voice, "Mute"),
    };
    let b = ui.add_enabled_ui(has_audio, |ui| messages::vector_button(ui, mp, icon, tip, 32.0, false, Color32::WHITE)).inner.on_disabled_hover_text(tip);
    if b.clicked() {
        mu.muted = !silent;
        if !mu.muted && mu.volume <= 0.0 {
            mu.volume = 1.0;
        }
    }
    let r = ui.interact(slider, id, Sense::click_and_drag());
    let open = has_audio && (ui.rect_contains_pointer(slider.union(b.rect)) || r.dragged() || r.has_focus());
    if !open {
        return;
    }
    r.widget_info(|| egui::WidgetInfo::slider(true, f64::from(if mu.muted { 0.0 } else { mu.volume }), "Volume"));
    if let Some(p) = r.interact_pointer_pos() {
        mu.volume = ((p.x - slider.left()) / slider.width()).clamp(0.0, 1.0);
        mu.muted = mu.volume <= 0.0;
    }
    if r.has_focus() {
        let filter = egui::EventFilter { horizontal_arrows: true, ..Default::default() };
        ui.memory_mut(|m| m.set_focus_lock_filter(r.id, filter));
        for (k, d) in [(Key::ArrowLeft, -0.1), (Key::ArrowRight, 0.1)] {
            if ui.input_mut(|i| i.consume_key(Modifiers::NONE, k)) {
                mu.volume = (mu.volume + d).clamp(0.0, 1.0);
                mu.muted = mu.volume <= 0.0;
            }
        }
    }
    let v = if mu.muted { 0.0 } else { mu.volume };
    let p = ui.painter();
    let cy = slider.center().y;
    p.rect_filled(Rect::from_min_max(pos2(slider.left(), cy - 2.0), pos2(slider.right(), cy + 2.0)), 2, mp.tk.on_media.gamma_multiply(0.3));
    p.rect_filled(Rect::from_min_max(pos2(slider.left(), cy - 2.0), pos2(slider.left() + slider.width() * v, cy + 2.0)), 2, INK);
    p.circle_filled(pos2(slider.left() + slider.width() * v, cy), 6.0, Color32::WHITE);
    if r.has_focus() {
        messages::focus_ring(ui, mp, slider.shrink2(vec2(0.0, 6.0)), 4.0);
    }
}

/// "Rustcord can't play this video format." and its way out, in the player's place.
fn error_card(ui: &mut egui::Ui, cx: &mut UiCx<'_>, item: &MediaItem, rect: Rect, f: Fail) {
    let pal = Pal::new(cx.theme);
    ui.painter().rect(rect, pal.radius, pal.surface, Stroke::new(1.0, pal.tk.rule), egui::StrokeKind::Inside);
    let (text, open, download, retry) = match f {
        Fail::Unsupported => ("Rustcord can't play this video format.", true, true, false),
        Fail::TooLarge => ("This video is too large to play here.", true, true, false),
        Fail::CantStream => ("This video can't stream here.", true, true, false),
        Fail::Network => ("Video didn't load. Check your connection, then select Retry.", false, false, true),
        Fail::Gone => ("This video is no longer available.", true, false, false),
    };
    let inner = Rect::from_center_size(rect.center(), vec2((rect.width() - 32.0).max(80.0), 84.0_f32.min(rect.height())));
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Center)));
    ui.spacing_mut().item_spacing = vec2(8.0, 12.0);
    ui.add(egui::Label::new(egui::RichText::new(text).size(pal.meta()).color(pal.muted)).wrap());
    ui.horizontal(|ui| {
        // Centre the row of buttons under the text.
        let n = usize::from(open) + usize::from(download) + usize::from(retry);
        let w = n as f32 * 140.0;
        ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
        if retry && messages::button(ui, &pal, Btn::Secondary, "Retry", true).clicked() {
            let key = item.key();
            cx.view.media_ui.players.remove(&key);
            let opts = Opts { output: cx.settings.voice_output.clone(), ..Opts::default() };
            if let Some(p) = ensure_player(&ui.ctx().clone(), cx, item, opts) {
                p.play();
            }
        }
        if open && messages::button(ui, &pal, Btn::Secondary, "Open in browser", true).clicked() && markdown::is_web_scheme(&item.open_url) {
            ui.ctx().open_url(egui::OpenUrl::new_tab(&item.open_url));
        }
        if download && messages::button(ui, &pal, Btn::Secondary, "Download", true).clicked() {
            save(&ui.ctx().clone(), cx, item);
        }
    });
}

/// Save dialog, then the download on the runtime; a toast when it ends.
fn save(ctx: &egui::Context, cx: &mut UiCx<'_>, item: &MediaItem) {
    let name = item.filename.clone();
    let Some(path) = rfd::FileDialog::new().set_file_name(&name).save_file() else { return };
    let (tx, url, ctx2) = (cx.view.media_ui.done.0.clone(), item.file_url.clone(), ctx.clone());
    let n = name.clone();
    let started = net::spawn(async move {
        let r = net::save(url, path).await;
        let _ = tx.send(Done::Saved(n, r));
        ctx2.request_repaint_of(egui::ViewportId::ROOT);
    });
    if !started {
        cx.view.toast(format!("Could not save {name}. Check your connection, then try again."), true);
    }
}

fn copy_image(ctx: &egui::Context, cx: &mut UiCx<'_>, item: &MediaItem) {
    let (tx, url, ctx2) = (cx.view.media_ui.done.0.clone(), item.file_url.clone(), ctx.clone());
    net::spawn(async move {
        let r = match net::fetch_all(url, net::MAX_WHOLE).await {
            Ok(bytes) => tokio::task::spawn_blocking(move || crate::cache::images::decode(&bytes, 4096, false).and_then(|d| d.frames.into_iter().next().map(|f| f.image).ok_or_else(|| "empty".to_owned())))
                .await
                .unwrap_or_else(|e| Err(e.to_string())),
            Err(e) => Err(e),
        };
        let _ = tx.send(Done::Copied(r));
        ctx2.request_repaint_of(egui::ViewportId::ROOT);
    });
}

/// An inline video attachment or proxied video embed (fit into `max`). True: open the lightbox
/// on it (Full screen).
pub fn video(ui: &mut egui::Ui, cx: &mut UiCx<'_>, item: &MediaItem, max: Vec2) -> bool {
    let size = item.natural.map_or(max, |n| n * (max.x / n.x).min(max.y / n.y).min(1.0));
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.is_rect_visible(rect) && player_view(ui, cx, item, rect, Place::Inline, false).toggle_full
}

/// A gifv embed: muted and looping, no controls; plays while "Play animated GIFs" is on and the
/// window focused, or while hovered, else rests on its poster with a GIF badge. True: clicked.
pub fn gifv(ui: &mut egui::Ui, cx: &mut UiCx<'_>, item: &MediaItem, max: Vec2) -> bool {
    let size = item.natural.map_or(max, |n| n * (max.x / n.x).min(max.y / n.y).min(1.0));
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.is_rect_visible(rect) && gifv_view(ui, cx, item, rect, false).clicked()
}

/// gifv autoplay: always in the lightbox, else while hovered, or with "Play animated GIFs" on
/// (reduced motion turns it off) while the window has focus.
fn gifv_plays(lightbox: bool, hovered: bool, animate_always: bool, focused: bool) -> bool {
    lightbox || hovered || (animate_always && focused)
}

fn gifv_view(ui: &mut egui::Ui, cx: &mut UiCx<'_>, item: &MediaItem, rect: Rect, always: bool) -> egui::Response {
    let _t = super::frame_stats::scope("+gifv");
    let ctx = ui.ctx().clone();
    let pal = Pal::new(cx.theme);
    let key = item.key();
    let resp = ui.interact(rect, egui::Id::unique(("rc_gifv", key, always)), Sense::click());
    let want = gifv_plays(always, resp.hovered(), cx.settings.animate_others == AnimateMode::Always, cx.view.window_focused);
    let radius = if always { 0 } else { pal.radius };
    // Not wanted here: `end_frame` pauses it unless another copy wants it.
    if want && !item.unsupported {
        cx.view.media_ui.gifv_want.insert(key);
        if let Some(p) = ensure_player(&ctx, cx, item, Opts { looped: true, muted: true, ..Opts::default() })
            && p.status().state == State::Paused
        {
            p.play();
        }
    }
    let ppp = ctx.pixels_per_point();
    let playing = cx.view.media_ui.players.get(&key).is_some_and(|s| {
        s.player.drawn();
        let px = rect.size() * ppp;
        s.player.set_target(px.x.round() as u32, px.y.round() as u32);
        s.player.status().state == State::Playing
    });
    ui.painter().rect_filled(rect, radius, Color32::BLACK);
    match frame_texture(&ctx, &mut cx.view.media_ui, key) {
        Some((tex, size)) => paint_texture(ui.painter(), rect, tex, size, radius),
        None => paint_poster(ui, cx, item, rect, radius),
    }
    if !playing && rect.height() > 24.0 {
        let g = ui.painter().layout_no_wrap("GIF".into(), FontId::new(pal.badge(), crate::theme::bold_family()), INK);
        let chip = Rect::from_min_size(rect.min + vec2(8.0, 8.0), g.size() + vec2(8.0, 2.0));
        ui.painter().rect_filled(chip, pal.radius / 2, DISC);
        ui.painter().galley(chip.min + vec2(4.0, 1.0), g, INK);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, "GIF"));
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// For other areas (Quests): a video in `size` with play, time, seek (never past the furthest
/// point played with `no_seek_ahead`), volume. The player starts paused on its first frame and
/// plays on the user's click. Its `Status::played_ms` is the time it really played on screen.
#[allow(dead_code, reason = "the Quests area's video modal adopts it")]
pub fn video_slot(ui: &mut egui::Ui, cx: &mut UiCx<'_>, url: &str, size: Vec2, no_seek_ahead: bool) -> Option<Status> {
    let item = MediaItem { kind: Kind::Video, src: None, play_url: Some(url.to_owned()), file_url: url.to_owned(), open_url: url.to_owned(), filename: net::file_name(url), natural: None, unsupported: false };
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let opts = Opts { no_seek_ahead, output: cx.settings.voice_output.clone(), ..Opts::default() };
    ensure_player(&ui.ctx().clone(), cx, &item, opts);
    player_view(ui, cx, &item, rect, Place::Slot, no_seek_ahead).status
}

fn close(cx: &mut UiCx<'_>) {
    cx.view.modal = None;
    let mu = &mut cx.view.media_ui;
    (mu.zoom, mu.author) = (None, None);
    mu.items.clear();
}

/// `Modal::Media`: the lightbox under the title bar (which stays usable).
pub fn lightbox(ctx: &egui::Context, cx: &mut UiCx<'_>) {
    let n = cx.view.media_ui.items.len();
    let Some(item) = cx.view.media_ui.items.get(cx.view.media_ui.index).cloned() else {
        close(cx);
        return;
    };
    let pal = Pal::new(cx.theme);
    let mp = media_pal(&pal);
    let screen = ctx.content_rect();
    let area = Rect::from_min_max(pos2(screen.left(), screen.top() + TITLE_BAR_H), screen.max);
    // Opens over BASE (fade, media 0.92 -> 1 from the click) and fades out over half of it on close (§7).
    let id = egui::Id::unique("rc_lightbox");
    let layer = egui::LayerId::new(egui::Order::Foreground, id);
    let t = super::motion::opened(ctx, layer, super::motion::BASE);

    let mut step = 0i32;
    let mut closing = false;
    egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(area.min).constrain(false).fade_in(false).show(ctx, |ui| {
        super::motion::own(ctx, ui.layer_id());
        ui.set_min_size(area.size());
        ui.set_opacity(t);
        // First, so everything drawn later is on top of it for clicks.
        let backdrop = ui.interact(area, egui::Id::unique("rc_lightbox_bg"), Sense::click_and_drag());
        ui.painter().rect_filled(area, 0, pal.tk.media_scrim);
        // The media fitted (never upscaled) into the room between the bars and the arrows.
        let room = Rect::from_min_max(area.min + vec2(SIDE, BAR_H), area.max - vec2(SIDE, BAR_H));
        let media = super::motion::about(cx.view.media_ui.from.unwrap_or(room.center()), egui::lerp(0.92..=1.0, t)) * room;
        // A zoomed image runs under the bars: it goes first, and the bars sit on scrim bands.
        let image_hit = match item.kind {
            Kind::Image { gif } => Some(image_view(ui, cx, &item, media, area, gif)),
            Kind::Video | Kind::Gifv => None,
        };
        if image_hit.is_some() && cx.view.media_ui.zoom.is_some() {
            for y in [area.top(), area.bottom() - BAR_H] {
                ui.painter().rect_filled(Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), BAR_H)), 0, pal.tk.media_scrim);
            }
        }
        // Top bar: who posted it, then Save, Copy image, Copy link, Close.
        let top = Rect::from_min_size(area.min, vec2(area.width(), BAR_H)).shrink2(vec2(16.0, 12.0));
        if let Some(a) = &cx.view.media_ui.author {
            let av = Rect::from_min_size(pos2(top.left(), top.center().y - 16.0), Vec2::splat(32.0));
            let (url, name, time, id) = (a.avatar.clone(), a.name.clone(), a.time.clone(), a.id);
            super::images::avatar(&mut ui.new_child(egui::UiBuilder::new().max_rect(av)), cx.images, &url, 32.0, false, Some((id, &name)));
            let g = ui.painter().layout_no_wrap(name, FontId::new(pal.body(), crate::theme::semibold_family()), pal.tk.on_media);
            let x = av.right() + 12.0;
            ui.painter().galley(pos2(x, top.center().y - g.size().y - 1.0), g, pal.tk.on_media);
            ui.painter().text(pos2(x, top.center().y + 1.0), Align2::LEFT_TOP, time, FontId::proportional(pal.meta()), pal.tk.on_media_muted);
        }
        let mut row = ui.new_child(egui::UiBuilder::new().max_rect(top).layout(egui::Layout::right_to_left(egui::Align::Center)));
        row.spacing_mut().item_spacing.x = 4.0;
        if messages::vector_button(&mut row, &mp, Icon::Close, "Close", 32.0, false, Color32::WHITE).clicked() {
            closing = true;
        }
        row.label(egui::RichText::new("Esc").size(pal.meta()).color(pal.tk.on_media_muted));
        row.add_space(12.0);
        if messages::vector_button(&mut row, &mp, Icon::Link, "Copy link", 32.0, false, Color32::WHITE).clicked() {
            ctx.copy_text(item.open_url.clone());
            cx.view.toast("Link copied", false);
        }
        if matches!(item.kind, Kind::Image { .. }) && messages::vector_button(&mut row, &mp, Icon::Copy, "Copy image", 32.0, false, Color32::WHITE).clicked() {
            copy_image(ctx, cx, &item);
        }
        let save_tip = if matches!(item.kind, Kind::Image { .. }) { "Save image" } else { "Save video" };
        if messages::vector_button(&mut row, &mp, Icon::Download, save_tip, 32.0, false, Color32::WHITE).clicked() {
            save(ctx, cx, &item);
        }
        // Arrows.
        if n > 1 {
            for (dir, icon, tip, x) in [(-1, Icon::ChevronLeft, "Previous", area.left() + 16.0), (1, Icon::ChevronRight, "Next", area.right() - 56.0)] {
                let r = Rect::from_min_size(pos2(x, room.center().y - 20.0), Vec2::splat(40.0));
                if disc(ui, &pal, r, egui::Id::unique(("rc_lightbox_arrow", dir)), icon, tip).clicked() {
                    step = dir;
                }
            }
        }
        // Videos after the top bar and arrows (Tab order); they never overflow.
        let hit = image_hit.unwrap_or_else(|| {
            let n = item.natural.unwrap_or(vec2(16.0, 9.0) * 120.0);
            let s = (media.width() / n.x).min(media.height() / n.y).min(1.0);
            let r = Rect::from_center_size(media.center(), n * s);
            if item.kind == Kind::Gifv {
                gifv_view(ui, cx, &item, r, true);
            } else if player_view(ui, cx, &item, r, Place::Lightbox, false).toggle_full {
                closing = true;
            }
            r
        });
        // Bottom bar: Open in browser (the explicit way out), "2 of 3".
        let bottom = Rect::from_min_max(pos2(area.left() + 16.0, area.bottom() - BAR_H), pos2(area.right() - 16.0, area.bottom()));
        if open_link(ui, &pal, bottom) && markdown::is_web_scheme(&item.open_url) {
            ctx.open_url(egui::OpenUrl::new_tab(&item.open_url));
        }
        if n > 1 {
            ui.painter().text(bottom.center(), Align2::CENTER_CENTER, format!("{} of {n}", cx.view.media_ui.index + 1), FontId::proportional(pal.meta()), pal.tk.on_media_muted);
        }
        if backdrop.clicked() && !backdrop.interact_pointer_pos().is_some_and(|p| hit.contains(p)) {
            closing = true;
        }
    });
    super::motion::leave(ctx, layer);
    // Keys: Esc closes, ←/→ step (a focused seek bar took its arrows already), Space plays.
    let (esc, left, right, space) = ctx.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::Escape),
            i.consume_key(Modifiers::NONE, Key::ArrowLeft),
            i.consume_key(Modifiers::NONE, Key::ArrowRight),
            item.kind == Kind::Video && i.consume_key(Modifiers::NONE, Key::Space),
        )
    });
    if space && let Some(s) = cx.view.media_ui.players.get(&item.key()) {
        if s.player.status().state == State::Playing { s.player.pause() } else { s.player.play() }
    }
    if esc || closing {
        close(cx);
        return;
    }
    let step = if left { -1 } else if right { 1 } else { step };
    if step != 0 && n > 1 {
        let mu = &mut cx.view.media_ui;
        // The video stepped away from stops (its inline copy behind would keep it playing).
        if let Some(s) = mu.players.get(&item.key()) {
            s.player.pause();
        }
        mu.index = (mu.index as i32 + step).rem_euclid(n as i32) as usize;
        mu.zoom = None;
    }
}

/// "Open in browser" with its External icon; underlined on hover. True when clicked.
fn open_link(ui: &mut egui::Ui, pal: &Pal, bar: Rect) -> bool {
    let g = ui.painter().layout_no_wrap("Open in browser".into(), FontId::new(pal.ui(), crate::theme::medium_family()), pal.tk.on_media);
    let rect = Rect::from_min_size(pos2(bar.left(), bar.center().y - 12.0), vec2(g.size().x + 4.0 + 16.0 + 8.0, 24.0));
    let r = ui.interact(rect, egui::Id::unique("rc_lightbox_open"), Sense::click());
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, "Open in browser"));
    let text = pos2(rect.left() + 4.0, rect.center().y - g.size().y / 2.0);
    let w = g.size().x;
    ui.painter().galley(text, g, pal.tk.on_media);
    let icon = Rect::from_min_size(pos2(text.x + w + 4.0, rect.center().y - 8.0), Vec2::splat(16.0));
    icons::paint(ui.painter(), Icon::External, icon, INK, Color32::BLACK);
    if r.hovered() {
        ui.painter().hline(text.x..=text.x + w, rect.center().y + 9.0, Stroke::new(1.0, pal.tk.on_media));
    }
    if r.has_focus() {
        messages::focus_ring(ui, pal, rect, f32::from(pal.radius));
    }
    r.on_hover_cursor(CursorIcon::PointingHand).clicked()
}

/// An image or GIF in the lightbox: fitted; click toggles fit ↔ 1:1 at the click point, the
/// wheel zooms (fit … 2× natural) about the pointer, drag pans. Returns the image's rect.
fn image_view(ui: &mut egui::Ui, cx: &mut UiCx<'_>, item: &MediaItem, media: Rect, area: Rect, gif: bool) -> Rect {
    let ctx = ui.ctx().clone();
    let ppp = ctx.pixels_per_point();
    let n = item.natural.unwrap_or(media.size());
    let fit = (media.width() / n.x).min(media.height() / n.y).min(1.0);
    let (s, pan) = cx.view.media_ui.zoom.unwrap_or((fit, Vec2::ZERO));
    let rect = Rect::from_center_size(media.center() + pan, n * s);
    let shown = rect.intersect(area);
    let r = ui.interact(shown, egui::Id::unique("rc_lightbox_image"), Sense::click_and_drag());
    let zoomable = fit < 1.0;
    // The fitted texture, and a sharper one (≤ 2560 px) once zoomed in; the fitted one is drawn
    // scaled until it arrives. shortcut: one zoom texture, no tiles; tile if 2560 px zooms look soft.
    let fit_px = n * fit * ppp;
    let fitted = item.picture_url(fit_px.x.round().max(1.0) as u32, fit_px.y.round().max(1.0) as u32);
    let zoom_px = n * (ZOOM_MAX_SIDE / n.max_elem()).min(1.0);
    let sharp = (s > fit).then(|| item.picture_url(zoom_px.x.round() as u32, zoom_px.y.round() as u32)).flatten();
    let tex = sharp
        .and_then(|u| cx.images.get(&ctx, &u, ZOOM_MAX_SIDE as u32, gif))
        .or_else(|| fitted.and_then(|u| cx.images.get(&ctx, &u, fit_px.max_elem().ceil() as u32, gif)));
    let p = ui.painter_at(area);
    match tex {
        Some(t) => {
            p.image(t.id, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => {
            p.rect_filled(rect, 0, Color32::from_gray(24));
            egui::Spinner::new().size(24.0).color(INK).paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(24.0)));
        }
    }
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, &item.filename));
    let mut zoom = cx.view.media_ui.zoom;
    if r.clicked() {
        zoom = match zoom {
            Some(_) => None,
            None if zoomable => {
                let at = r.interact_pointer_pos().map_or(Vec2::ZERO, |p| p - media.center());
                Some((1.0, clamp_pan(zoom_about(fit, 1.0, Vec2::ZERO, at), n, media.size())))
            }
            None => None,
        };
    }
    if zoomable && r.hovered() {
        let wheel = ctx.input(|i| i.smooth_scroll_delta.y);
        if wheel != 0.0 {
            let s1 = (s * 1.1f32.powf(wheel / 40.0)).clamp(fit, 2.0);
            let at = r.hover_pos().map_or(Vec2::ZERO, |p| p - media.center());
            zoom = (s1 > fit + 1e-4).then(|| (s1, clamp_pan(zoom_about(s, s1, pan, at), n * s1, media.size())));
        }
    }
    if let Some((s1, p1)) = zoom.filter(|_| r.dragged()) {
        zoom = Some((s1, clamp_pan(p1 + r.drag_delta(), n * s1, media.size())));
    }
    cx.view.media_ui.zoom = zoom;
    let cursor = if zoom.is_some() {
        if r.dragged() { CursorIcon::Grabbing } else { CursorIcon::Grab }
    } else if zoomable {
        CursorIcon::ZoomIn
    } else {
        CursorIcon::Default
    };
    r.on_hover_cursor(cursor);
    shown
}

/// Test hook `RUSTCORD_MSG_DEMO`: `media` adds messages with the fixture media (seeded into the
/// disk cache by tests/fixtures/make_fixture.py, no request is sent): demo.png and demo.gif at
/// the top of the loaded history; a gifv embed with a WebM upload (the can't-play card), then
/// demo.mp4 at the bottom. `viewer` / `viewer-zoom` open the lightbox on
/// demo.png (fitted / at 1:1), `viewer-video` on demo.mp4 paused at 1.0 s, `player` pauses the
/// inline demo.mp4 at 1.0 s, `play` plays it muted.
pub(super) fn demo(ctx: &egui::Context, cx: &mut UiCx<'_>, ch: Id, flag: impl Fn(&str) -> bool) {
    if !flag("media") {
        return;
    }
    let Some(cm) = cx.state.messages.get_mut(&ch) else { return };
    let Some(last) = cm.msgs.last().cloned() else { return };
    let author = cx.state.users.values().min_by_key(|u| u.id).cloned().unwrap_or_else(|| last.author.clone());
    let att = |n: u64, name: &str, ct: &str, w: u32, h: u32| Attachment {
        id: Id(n),
        filename: name.into(),
        url: format!("https://cdn.discordapp.com/attachments/1/{n}/{name}"),
        content_type: Some(ct.into()),
        width: Some(w),
        height: Some(h),
        size: 100_000,
        ..Default::default()
    };
    let gifv = Embed {
        kind: Some("gifv".into()),
        url: Some("https://example.com/demo".into()),
        video: Some(EmbedMedia { url: "https://example.com/demo.mp4".into(), proxy_url: Some("https://images-ext-1.discordapp.net/external/demo/gifv.mp4".into()), width: Some(640), height: Some(360), content_type: Some("video/mp4".into()) }),
        ..Default::default()
    };
    let msg = |id: Id, attachments: Vec<Attachment>, embeds: Vec<Embed>| Message { id, channel_id: ch, author: author.clone(), timestamp: last.timestamp.clone(), attachments, embeds, ..Default::default() };
    // The pictures go to the top of the loaded history: off screen, so the lightbox (not a small
    // inline copy) loads their texture first; these demo URLs have no proxy size variants.
    let first = cm.msgs.first().map_or(last.id, |m| m.id);
    let images = msg(Id(first.0 - 1), vec![att(1, "demo.png", "image/png", 960, 600), att(2, "demo.gif", "image/gif", 160, 100)], vec![]);
    let gifs = msg(Id(last.id.0 + 1), vec![att(4, "demo.webm", "video/webm", 640, 360)], vec![gifv]);
    let videos = msg(Id(last.id.0 + 2), vec![att(3, "demo.mp4", "video/mp4", 640, 360)], vec![]);
    cm.msgs.insert(0, images.clone());
    cm.msgs.push(gifs);
    cm.msgs.push(videos.clone());
    let who = |m: &Message| Some(Author { id: m.author.id, name: m.author.display_name().to_owned(), avatar: cdn::avatar_url(&m.author, 64, false), time: super::format_timestamp(&m.timestamp) });
    let mp4 = attachment_item(&videos.attachments[0]);
    if flag("player") || flag("play") || flag("viewer-video") {
        // A test hook never makes a sound.
        cx.view.media_ui.muted = flag("play");
        // `loop`: keeps playing for measurements (RUSTCORD_FRAME_STATS).
        let opts = Opts { looped: flag("loop"), output: cx.settings.voice_output.clone(), ..Opts::default() };
        if let Some(p) = ensure_player(ctx, cx, &mp4, opts) {
            if flag("play") { p.play() } else { p.seek(1000) }
        }
    }
    if flag("viewer") || flag("viewer-zoom") {
        open(ctx, cx, &images, who(&images), &attachment_item(&images.attachments[0]));
        if flag("viewer-zoom") {
            cx.view.media_ui.zoom = Some((1.0, Vec2::ZERO));
        }
    } else if flag("viewer-video") {
        open(ctx, cx, &videos, who(&videos), &mp4);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(name: &str, ct: &str) -> Attachment {
        Attachment { filename: name.into(), url: format!("https://cdn.discordapp.com/attachments/1/2/{name}"), proxy_url: format!("https://media.discordapp.net/attachments/1/2/{name}"), content_type: Some(ct.into()), width: Some(100), height: Some(50), ..Default::default() }
    }

    fn proxied(url: &str) -> EmbedMedia {
        EmbedMedia { url: url.into(), proxy_url: Some(format!("https://images-ext-1.discordapp.net/external/x/{url}")), width: Some(64), height: Some(64), content_type: None }
    }

    #[test]
    fn items_in_order_with_filters() {
        let mut m = Message { attachments: vec![att("a.png", "image/png"), att("SPOILER_b.gif", "image/gif"), att("c.mp4", "video/mp4"), att("d.txt", "text/plain"), att("e.webm", "video/webm")], ..Default::default() };
        let youtube = Embed { kind: Some("video".into()), url: Some("https://www.youtube.com/watch?v=x".into()), video: Some(EmbedMedia { url: "https://www.youtube.com/embed/x".into(), ..Default::default() }), thumbnail: Some(proxied("t.jpg")), ..Default::default() };
        let gif = Embed { kind: Some("gifv".into()), url: Some("https://example.com/g".into()), video: Some(proxied("g.mp4")), thumbnail: Some(proxied("g.png")), ..Default::default() };
        let img = Embed { kind: Some("image".into()), thumbnail: Some(proxied("i.png")), ..Default::default() };
        let rich = Embed { kind: Some("rich".into()), image: Some(proxied("r.png")), ..Default::default() };
        m.embeds = vec![youtube.clone(), gif, img, rich];
        let names = |v: Vec<MediaItem>| v.into_iter().map(|i| i.filename).collect::<Vec<_>>();
        assert_eq!(names(media_items(&m, true, true, |_| false)), ["a.png", "c.mp4", "e.webm", "g.mp4", "i.png"], "covered spoilers, files and YouTube are left out");
        assert_eq!(names(media_items(&m, true, true, |i| i == 1)), ["a.png", "SPOILER_b.gif", "c.mp4", "e.webm", "g.mp4", "i.png"]);
        assert_eq!(names(media_items(&m, false, true, |_| true)), ["g.mp4", "i.png"], "uploads shown as files");
        assert_eq!(names(media_items(&m, true, false, |_| false)), ["a.png", "c.mp4", "e.webm"]);
        m.flags = SUPPRESS_EMBEDS;
        assert_eq!(media_items(&m, true, true, |_| false).len(), 3);
        let all = media_items(&Message { flags: 0, ..m.clone() }, true, true, |_| false);
        assert_eq!(all.iter().map(|i| i.kind).collect::<Vec<_>>(), [Kind::Image { gif: false }, Kind::Video, Kind::Video, Kind::Gifv, Kind::Image { gif: false }]);
        assert!(all[2].unsupported && !all[1].unsupported, "WebM is refused without a request");
        assert!(all[1].src.is_some(), "a proxied upload has a poster");
        assert!(!plays_inline(&youtube));
        // A rich or link embed's picture opens in the lightbox on its own; an unproxied one can't.
        let card = Embed { url: Some("https://example.com/post".into()), image: Some(proxied("r.png")), ..Default::default() };
        let it = embed_image_item(&card, card.image.as_ref().unwrap()).unwrap();
        assert_eq!((it.kind, it.filename.as_str(), it.open_url.as_str()), (Kind::Image { gif: false }, "r.png", "https://example.com/post"));
        assert!(it.file_url.starts_with("https://images-ext-1.discordapp.net/"));
        assert!(embed_image_item(&card, &EmbedMedia { url: "https://example.com/x.png".into(), ..Default::default() }).is_none());
    }

    #[test]
    fn gifv_autoplay_rules() {
        assert!(gifv_plays(false, false, true, true), "setting on, window focused");
        assert!(!gifv_plays(false, false, true, false), "setting on, window in the background");
        assert!(!gifv_plays(false, false, false, true), "setting off (or reduced motion): rests on the poster");
        assert!(gifv_plays(false, true, false, false), "hovered always plays");
        assert!(gifv_plays(true, false, false, false), "the lightbox always plays");
    }

    #[test]
    fn zoom_keeps_the_point_under_the_pointer() {
        // Image point under `at` before and after.
        let point = |s: f32, pan: Vec2, at: Vec2| (at - pan) / s;
        let (s0, s1, pan, at) = (0.5, 1.0, vec2(10.0, -4.0), vec2(120.0, 60.0));
        let pan1 = zoom_about(s0, s1, pan, at);
        assert!((point(s0, pan, at) - point(s1, pan1, at)).length() < 1e-4);
        assert_eq!(zoom_about(1.0, 1.0, pan, at), pan);
    }

    #[test]
    fn pan_never_leaves_a_gap() {
        let room = vec2(400.0, 300.0);
        assert_eq!(clamp_pan(vec2(500.0, 0.0), vec2(800.0, 200.0), room), vec2(200.0, 0.0), "200 px over each side at most; centred where it fits");
        assert_eq!(clamp_pan(vec2(-500.0, -90.0), vec2(800.0, 500.0), room), vec2(-200.0, -90.0));
        assert_eq!(clamp_pan(vec2(30.0, 30.0), vec2(100.0, 100.0), room), Vec2::ZERO);
        assert_eq!(clock(61_500), "1:01");
        assert_eq!(clock(3_725_000), "1:02:05");
    }

    /// Headless frames with the lightbox open (the messages.rs harness style).
    #[test]
    fn keys_step_wrap_and_close() {
        let snapshot = serde_json::from_str(include_str!("../../tests/fixtures/home/cache/snapshot.json")).unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let cmd = crate::backend::CommandSender(tx);
        let ctx = egui::Context::default();
        let mut theme = crate::theme::Theme::ember_dark();
        crate::theme::ThemeApplier::default().apply(&ctx, &theme);
        let mut state = crate::state::AppState::from_snapshot(snapshot);
        let mut view = crate::ui::ViewState::default();
        let mut images = super::super::images::ImageCache::new(cmd.clone());
        let mut settings = crate::config::Settings::default();
        let mut overrides = crate::overrides::LocalOverrides::default();
        let mut frame = |view: &mut crate::ui::ViewState, keys: &[Key], setup: bool| {
            let events = keys.iter().flat_map(|&key| [true, false].map(|pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE })).collect();
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 600.0))), events, ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                let mut cx = UiCx { state: &mut state, view, images: &mut images, theme: &mut theme, settings: &mut settings, cmd: &cmd, overrides: &mut overrides };
                if setup {
                    let m = Message { attachments: vec![att("a.png", "image/png"), att("b.png", "image/png"), att("c.png", "image/png")], ..Default::default() };
                    let at = attachment_item(&m.attachments[2]);
                    open(ui.ctx(), &mut cx, &m, None, &at);
                }
                if cx.view.modal == Some(Modal::Media) {
                    lightbox(ui.ctx(), &mut cx);
                }
                end_frame(ui.ctx(), &mut cx);
            });
            out.textures_delta.clear();
        };
        frame(&mut view, &[], true);
        assert_eq!((view.modal, view.media_ui.index, view.media_ui.items.len()), (Some(Modal::Media), 2, 3));
        frame(&mut view, &[Key::ArrowRight], false);
        assert_eq!(view.media_ui.index, 0, "wraps past the end");
        frame(&mut view, &[Key::ArrowLeft], false);
        assert_eq!(view.media_ui.index, 2, "and past the start");
        frame(&mut view, &[Key::Escape], false);
        assert_eq!(view.modal, None);
        assert!(view.media_ui.items.is_empty());
    }
}
