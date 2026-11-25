//! Call view: while I am in a voice channel and it is the open channel (`is_open`), the page is
//! a dark stage instead of the chat (`main_layout`): a header (channel, "you are live", chat
//! toggle), participant tiles and a control bar (camera, screen share, mute, deafen,
//! disconnect). A tile shows avatar + name, a green outline while speaking, mute / deafen marks and
//! LIVE for streamers; a camera, a watched stream or my own preview shows its latest frame from
//! `VideoFrames` (uploaded only when its generation changes, letterboxed; my camera mirrored). A click expands a tile
//! (the others in a strip below), another click returns to the grid. Someone else's stream needs
//! a click on "Watch stream"; its × or leaving the call view stops watching. The text chat opens
//! beside the stage (in its place in narrow windows).
//! Visibility (docs/VIDEO_DESIGN.md "UI hook-up"): videos painted in a pass are visible, all
//! others hidden; `sync` (App::logic, which also runs while minimized) turns that into
//! `SetVideoVisible` and drops hidden textures. At most 4 remote videos at once; more tiles show
//! the avatar. Nothing starts by itself: every command is a click.
//! Test hook `RUSTCORD_CALL_DEMO` (`demo`).
//! OWNERSHIP: video-ui engineer.

use super::dm_call::PULSE_S;
use super::screen_share::{self, ShareForm};
use super::icons::{self, Icon};
use super::sidebar::{toggle_voice, user_avatar_at};
use super::{Modal, UiCx, ViewState, bold, messages, one_line, pickers, presence_color};
use crate::backend::{Command, CommandSender};
use crate::model::{Id, Presence, VoiceState};
use crate::state::AppState;
use crate::voice::video::stream::{CONNECT_TIMEOUT, parse_stream_key, stream_key};
use crate::voice::video::{MonitorInfo, ScreenSource, VideoCommand, VideoFrames, VideoKey, WindowInfo, capture};
use egui::{Color32, CursorIcon, FontId, Rect, Role, Sense, Stroke, StrokeKind, TextureHandle, UiBuilder, Vec2, WidgetInfo, pos2, vec2};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

/// Remote videos decoded at once (the backend refuses more).
const MAX_REMOTE_VIDEOS: usize = 4;
/// Same height as the chat header.
const HEADER_H: f32 = 49.0;
/// Control bar under the tiles: 48 px buttons, 16 px above and below (`stage`: 40 px buttons
/// and 12 px in a short stage).
const BAR_H: f32 = 80.0;
const GAP: f32 = 8.0;
/// Narrower tiles would be unreadable: the grid scrolls instead.
const MIN_TILE_W: f32 = 176.0;
/// The text chat beside the stage.
const CHAT_W: f32 = super::THREAD_PANEL_W;
/// The stage is dark in every theme, like a video player; tiles and buttons a shade lighter.
const STAGE: Color32 = Color32::from_rgb(14, 12, 12);
const TILE: Color32 = Color32::from_rgb(38, 34, 33);
const BUTTON: Color32 = Color32::from_rgb(56, 51, 50);
const FG: Color32 = Color32::from_gray(238);
const FG_MUTED: Color32 = Color32::from_gray(170);

/// Call view state (`ViewState::call_ui`).
#[derive(Default)]
pub struct CallUi {
    /// Text chat open beside the stage.
    pub(super) chat: bool,
    /// Expanded tile.
    focus: Option<TileKey>,
    /// Streams I clicked "Watch stream" on (stream keys) and when; None once connected (in
    /// `remote`). A click that got nowhere within the backend's CONNECT_TIMEOUT is forgotten.
    watching: HashMap<String, Option<Instant>>,
    /// Videos marked visible with the backend.
    visible: HashSet<VideoKey>,
    /// Videos painted in the current pass, and how many of them are someone else's.
    want: HashSet<VideoKey>,
    remote: usize,
    /// Texture and frame generation of each visible video.
    textures: HashMap<VideoKey, (u64, TextureHandle)>,
    /// `BackendHandle::video`, handed over by `sync` (a reset view gets it again).
    frames: Option<Arc<VideoFrames>>,
    /// Screen share picker form (screen_share.rs).
    pub(super) share: ShareForm,
    demo_done: bool,
}

/// The connected (or joining) voice channel is the open channel: `main_layout` shows the call.
pub fn is_open(state: &AppState) -> bool {
    state.voice.channel_id.is_some_and(|ch| state.selected_channel == Some(ch))
}

/// Width of the chat column beside the stage in a window `width` wide; 0 = none (chat closed,
/// or a narrow window, where it takes the stage's place).
pub fn chat_w(view: &ViewState, width: f32) -> f32 {
    if view.call_ui.chat && width >= super::MEMBER_LIST_MIN_WINDOW_W { CHAT_W } else { 0.0 }
}

fn video(cmd: VideoCommand) -> Command {
    Command::Video(cmd)
}

/// Once per frame from `App::logic`, before anything is drawn: the videos painted in the last
/// pass stay visible, every other one is hidden (`SetVideoVisible` on each change, the texture
/// dropped). Nothing is painted while minimized, so everything is hidden then. Watched streams
/// end when I leave the call view; their list is forgotten with the call.
pub fn sync(view: &mut ViewState, state: &mut AppState, cmd: &CommandSender, frames: &Arc<VideoFrames>) {
    let ui = &mut view.call_ui;
    ui.frames.get_or_insert_with(|| frames.clone());
    let mut want = std::mem::take(&mut ui.want);
    ui.remote = 0;
    // Ended videos start over: a camera back on (even within this batch of events) is asked
    // for again once painted, and never shows the frame from before it went off.
    for key in state.voice.video.ended.drain(..) {
        ui.visible.remove(&key);
        want.remove(&key);
        ui.textures.remove(&key);
    }
    for key in ui.visible.difference(&want) {
        cmd.send(video(VideoCommand::SetVideoVisible { key: key.clone(), visible: false }));
        ui.textures.remove(key);
    }
    for key in want.difference(&ui.visible) {
        cmd.send(video(VideoCommand::SetVideoVisible { key: key.clone(), visible: true }));
    }
    ui.visible = want;
    match state.voice.channel_id {
        // The call ended: the backend stopped everything.
        None => {
            ui.watching.clear();
            ui.focus = None;
        }
        Some(_) if !is_open(state) => {
            for (stream_key, _) in ui.watching.drain() {
                cmd.send(video(VideoCommand::StopWatching { stream_key }));
            }
        }
        Some(ch) => {
            let remote = &state.voice.video.remote;
            // Connected counts as watched (also one that connected late), so leaving stops it.
            for key in remote.keys() {
                if let VideoKey::Stream(k) = key {
                    ui.watching.insert(k.clone(), None);
                }
            }
            let pending = |t: &Option<Instant>| t.is_some_and(|t| t.elapsed() < CONNECT_TIMEOUT);
            ui.watching.retain(|k, t| streaming(state, ch, k) && (pending(t) || remote.contains_key(&VideoKey::Stream(k.clone()))));
        }
    }
}

/// The owner of stream `key` still streams in `ch`.
fn streaming(state: &AppState, ch: Id, key: &str) -> bool {
    parse_stream_key(key).is_some_and(|(_, c, owner)| c == ch && state.voice_states.get(&ch).and_then(|m| m.get(&owner)).is_some_and(|v| v.self_stream))
}

/// Paints `key`'s latest frame letterboxed into `rect` and marks it visible for this pass.
/// False (nothing painted) before its first frame, or when 4 remote videos are already shown
/// (`mine` = my camera / share preview, which does not count).
pub(super) fn paint_video(p: &egui::Painter, call: &mut CallUi, key: &VideoKey, rect: Rect, mine: bool) -> bool {
    let Some(frames) = call.frames.clone() else { return false };
    if !call.want.contains(key) {
        if !mine && call.remote >= MAX_REMOTE_VIDEOS {
            return false;
        }
        call.remote += usize::from(!mine);
        call.want.insert(key.clone());
    }
    // `latest` is None while a writer holds the slot: keep the last texture then.
    if let Some((generation, image)) = frames.latest(key) {
        match call.textures.get_mut(key) {
            Some((g, tex)) => {
                if *g != generation {
                    *g = generation;
                    tex.set(image, egui::TextureOptions::LINEAR);
                }
            }
            None => {
                let tex = p.ctx().load_texture("video", image, egui::TextureOptions::LINEAR);
                call.textures.insert(key.clone(), (generation, tex));
            }
        }
    }
    let Some((_, tex)) = call.textures.get(key) else { return false };
    p.rect_filled(rect, 8, Color32::BLACK);
    let img = fit(rect, tex.size_vec2());
    // Rounded only when it covers the tile; letterboxed edges stay square inside the bars.
    let round = if img.width() >= rect.width() - 0.5 && img.height() >= rect.height() - 0.5 { 8 } else { 0 };
    // My own camera is a mirror (moving left moves left on screen); streams stay as they are.
    let uv = if mine && matches!(key, VideoKey::User(_)) { Rect::from_min_max(pos2(1.0, 0.0), pos2(0.0, 1.0)) } else { Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)) };
    p.add(egui::epaint::RectShape::filled(img, round, Color32::WHITE).with_texture(tex.id(), uv));
    true
}

/// `size` scaled to fit `rect`, centred (letterbox / pillarbox).
fn fit(rect: Rect, size: Vec2) -> Rect {
    if size.x <= 0.0 || size.y <= 0.0 {
        return rect;
    }
    let scale = (rect.width() / size.x).min(rect.height() / size.y);
    Rect::from_center_size(rect.center(), size * scale)
}

/// Columns and tile size for `n` 16:9 tiles in `area`, GAP apart: the layout with the largest
/// tiles; when even those are under MIN_TILE_W, as many columns as fit at that width (the grid
/// then scrolls), unless not even two such rows fit the height (then the largest tiles after
/// all). A row cut to a sliver at the bottom reads as a clipping glitch: the tiles then
/// shrink (not under MIN_TILE_W) until a third of that row peeks out (under `grid_view`'s fade).
fn grid(n: usize, area: Vec2) -> (usize, Vec2) {
    let n = n.max(1);
    let width = |cols: usize| {
        let rows = n.div_ceil(cols) as f32;
        let w = (area.x - GAP * (cols as f32 - 1.0)) / cols as f32;
        w.min((area.y - GAP * (rows - 1.0)) / rows * 16.0 / 9.0)
    };
    let (cols, w) = (1..=n).map(|c| (c, width(c))).fold((1, f32::MIN), |best, c| if c.1 > best.1 { c } else { best });
    if w >= MIN_TILE_W {
        return (cols, vec2(w, w * 9.0 / 16.0));
    }
    let fit_all = (cols, vec2(w, w * 9.0 / 16.0));
    let cols = (((area.x + GAP) / (MIN_TILE_W + GAP)).floor() as usize).clamp(1, n);
    let w = ((area.x - GAP * (cols as f32 - 1.0)) / cols as f32).max(1.0);
    let in_view = (area.y + GAP) / (w * 9.0 / 16.0 + GAP);
    // Not even two rows of those fit (a short stage above a DM's chat): everyone smaller in
    // the layout that shows all, rather than rows cut off by the controls.
    if in_view < 2.0 {
        return fit_all;
    }
    let peek_h = (area.y + GAP) / (in_view.floor() + 0.4) - GAP;
    if in_view >= 1.0 && in_view.fract() < 0.3 && n.div_ceil(cols) as f32 > in_view && peek_h * 16.0 / 9.0 >= MIN_TILE_W {
        return (cols, vec2(peek_h * 16.0 / 9.0, peek_h));
    }
    (cols, vec2(w, w * 9.0 / 16.0))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum TileKey {
    User(Id),
    /// That user's Go Live.
    Stream(Id),
}

/// One tile, built per frame from the voice states.
struct Tile {
    key: TileKey,
    user: Id,
    name: String,
    avatar: Option<String>,
    muted: bool,
    deaf: bool,
    /// Muted / deafened by a moderator.
    by_server: bool,
    /// The user streams (LIVE).
    live: bool,
    speaking: bool,
    /// The video this tile shows (camera on, stream connected, my previews).
    video: Option<VideoKey>,
    /// Stream tiles: the stream key.
    stream: Option<String>,
    /// My own tile (my camera / share preview).
    mine: bool,
    /// Background: TILE tinted with the user's accent colour.
    tint: Color32,
    /// Rung for a call I started, not in it yet.
    ringing: bool,
}

/// Stream tiles first, then everyone in the call by name; me even before my voice state echoes.
fn tiles(cx: &UiCx<'_>, ch: Id) -> Vec<Tile> {
    let s = &*cx.state;
    let guild_id = s.channels.get(&ch).and_then(|c| c.guild_id);
    let me = s.me.as_ref().map(|u| u.id);
    let v = &s.voice.video;
    let states = s.voice_states.get(&ch);
    let mut ids: Vec<Id> = states.map(|m| m.keys().copied().collect()).unwrap_or_default();
    ids.extend(me.filter(|me| !ids.contains(me)));
    let (mut users, mut streams) = (Vec::new(), Vec::new());
    for id in ids {
        let vs = states.and_then(|m| m.get(&id));
        let flag = |f: fn(&VoiceState) -> bool| vs.is_some_and(f);
        let member = vs.and_then(|v| v.member.as_ref());
        let user = s.users.get(&id).or(s.me.as_ref().filter(|u| u.id == id));
        let name = user.map_or_else(|| "Unknown user".into(), |u| s.member_display_name(guild_id, u, member));
        let avatar = user.map(|u| super::images::user_avatar_url(guild_id, u, member, 128, false));
        let mine = Some(id) == me;
        let (mut muted, mut deaf) = (flag(|v| v.self_mute || v.mute), flag(|v| v.self_deaf || v.deaf));
        if mine {
            (muted, deaf) = (muted || cx.view.self_mute, deaf || cx.view.self_deaf);
        }
        let stream = if mine { v.screen_share.clone() } else { flag(|v| v.self_stream).then(|| stream_key(guild_id, ch, id)) };
        let cam = VideoKey::User(id);
        let camera = if mine { v.camera_on } else { v.remote.contains_key(&cam) };
        if let Some(k) = &stream {
            let key = VideoKey::Stream(k.clone());
            let video = (mine || v.remote.contains_key(&key)).then_some(key);
            let (name, avatar, stream) = (name.clone(), avatar.clone(), stream.clone());
            streams.push(Tile { key: TileKey::Stream(id), user: id, name, avatar, muted: false, deaf: false, by_server: false, live: true, speaking: false, video, stream, mine, tint: TILE, ringing: false });
        }
        let speaking = s.voice.speaking.contains(&id);
        let by_server = flag(|v| v.mute || v.deaf);
        let tint = user.and_then(|u| u.accent_color).map_or(TILE, |a| TILE.lerp_to_gamma(Color32::from_rgb((a >> 16) as u8, (a >> 8) as u8, a as u8), 0.3));
        users.push(Tile { key: TileKey::User(id), user: id, name, avatar, muted: muted || deaf, deaf, by_server, live: stream.is_some(), speaking, video: camera.then_some(cam), stream: None, mine, tint, ringing: false });
    }
    // A DM call rings people who aren't in yet (a call I started): after the others, dimmed.
    let rung = s.calls.ringing.get(&ch).into_iter().flatten().filter(|u| Some(**u) != me && !states.is_some_and(|m| m.contains_key(*u)));
    for &id in rung {
        let user = s.users.get(&id);
        let name = user.map_or_else(|| "Unknown user".into(), |u| u.display_name().to_owned());
        let avatar = user.map(|u| super::images::user_avatar_url(None, u, None, 128, false));
        let t = Tile { key: TileKey::User(id), user: id, name, avatar, muted: false, deaf: false, by_server: false, live: false, speaking: false, video: None, stream: None, mine: false, tint: TILE, ringing: true };
        users.push(t);
    }
    for list in [&mut streams, &mut users] {
        list.sort_by_cached_key(|t| (t.ringing, t.name.to_lowercase(), t.user));
    }
    streams.extend(users);
    streams
}

/// The call page (`main_layout`, while `is_open`): header, chat column (`chat_w` > 0), stage.
pub fn show(ui: &mut egui::Ui, cx: &mut UiCx<'_>, chat_w: f32) {
    let Some(ch) = cx.state.voice.channel_id else { return };
    egui::Panel::top("call_header")
        .exact_size(HEADER_H)
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::new().fill(STAGE))
        .show(ui, |ui| header(ui, cx, ch));
    let chat = cx.view.call_ui.chat;
    let page = egui::Frame::new().fill(cx.theme.colors.background.to_color32());
    if chat && chat_w > 0.0 {
        let r = egui::Panel::right("call_chat")
            .exact_size(chat_w)
            .resizable(false)
            .show_separator_line(false)
            .frame(page)
            .show(ui, |ui| messages::show(ui, cx))
            .response
            .rect;
        ui.painter().vline(r.left() + 0.5, r.y_range(), super::sidebar::line(&cx.theme.colors));
    } else if !chat {
        pickers::close_if_open(ui.ctx(), cx);
    }
    let narrow_chat = chat && chat_w == 0.0;
    egui::CentralPanel::default().frame(if narrow_chat { page } else { egui::Frame::new().fill(STAGE) }).show(ui, |ui| {
        if narrow_chat { messages::show(ui, cx) } else { stage(ui, cx, ch) }
    });
}

/// Channel icon and name, the LIVE note while I share, and the chat toggle.
fn header(ui: &mut egui::Ui, cx: &mut UiCx<'_>, ch: Id) {
    let rect = ui.max_rect();
    let (cy, size) = (rect.center().y, cx.theme.font.size);
    let icon = cx.state.channels.get(&ch).map_or((Icon::Voice, None), |c| icons::of(c, cx.state));
    icons::paint_channel(ui.painter(), icon, Rect::from_center_size(pos2(rect.left() + 26.0, cy), Vec2::splat(20.0)), FG_MUTED, STAGE);
    let toggle = Rect::from_center_size(pos2(rect.right() - 32.0, cy), Vec2::splat(32.0));
    let left = rect.left() + 44.0;
    let g = one_line(ui, cx.state.channel_name(ch), bold(size + 1.5), FG, (toggle.left() - 64.0 - left).max(80.0));
    let x = left + g.size().x + 12.0;
    ui.painter().galley(pos2(left, cy - g.size().y / 2.0), g, FG);
    if cx.state.voice.video.screen_share.is_some() {
        let w = live_pill(ui.painter(), pos2(x, cy), cx.theme.tokens().danger_fill);
        let pill = Rect::from_min_size(pos2(x, cy - 8.0), vec2(w, 16.0));
        ui.interact(pill, ui.make_persistent_id("call_live"), Sense::hover()).on_hover_text("You are sharing your screen");
    }
    let chat = cx.view.call_ui.chat;
    if stage_button(ui, toggle, "call_chat_toggle", Icon::Message, if chat { "Hide chat" } else { "Show chat" }, chat).clicked() {
        cx.view.call_ui.chat = !chat;
    }
}

/// The "Live" tag (badge role on `red`, the `danger_fill`; chip radius) whose left edge is
/// centred on `left_center`; returns its width.
pub(super) fn live_pill(p: &egui::Painter, left_center: egui::Pos2, red: Color32) -> f32 {
    let text = on_color(red);
    let g = p.layout_no_wrap("Live".into(), bold(11.0), text);
    let pill = Rect::from_min_size(left_center - vec2(0.0, 8.0), vec2(g.size().x + 12.0, 16.0));
    p.rect_filled(pill, 4, red);
    p.galley(pill.center() - g.size() / 2.0, g, text);
    pill.width()
}

/// Black or white, whichever contrasts more with `bg` (WCAG: black above luminance ~0.18).
fn on_color(bg: Color32) -> Color32 {
    let l = egui::Rgba::from(bg);
    if 0.2126 * l.r() + 0.7152 * l.g() + 0.0722 * l.b() > 0.179 { Color32::BLACK } else { Color32::WHITE }
}

/// 32 px icon button on the stage (lit while `on`); the tooltip is the accessible name.
fn stage_button(ui: &mut egui::Ui, rect: Rect, salt: &str, icon: Icon, tip: &str, on: bool) -> egui::Response {
    let resp = ui.interact(rect, ui.make_persistent_id(salt), Sense::click());
    let hot = resp.hovered() || resp.has_focus();
    if hot || on {
        ui.painter().rect_filled(rect, 8, BUTTON);
    }
    let (color, bg) = if hot || on { (FG, BUTTON) } else { (FG_MUTED, STAGE) };
    icons::paint(ui.painter(), icon, Rect::from_center_size(rect.center(), Vec2::splat(20.0)), color, bg);
    resp.widget_info(|| WidgetInfo::selected(Role::Button, true, on, tip));
    resp.on_hover_text(tip).on_hover_cursor(CursorIcon::PointingHand)
}

/// Tiles (grid, or the focused one over a strip of the others) and the control bar; also the
/// call area above a DM's chat (dm_call.rs).
pub(super) fn stage(ui: &mut egui::Ui, cx: &mut UiCx<'_>, ch: Id) {
    let rect = ui.max_rect();
    let tiles = tiles(cx, ch);
    let focus = cx.view.call_ui.focus.clone().and_then(|f| tiles.iter().position(|t| t.key == f));
    if focus.is_none() {
        cx.view.call_ui.focus = None;
    }
    // As much room above the tiles as between them and the buttons; less of both (and smaller
    // buttons) in a short stage, the call area above a DM's chat in a small window.
    let (bar_h, pad) = if rect.height() < 240.0 { (64.0, 12.0) } else { (BAR_H, 16.0) };
    let bar = Rect::from_min_max(pos2(rect.left(), rect.bottom() - bar_h), rect.max);
    let area = Rect::from_min_max(rect.min + vec2(16.0, pad), pos2(rect.right() - 16.0, bar.top()));
    match focus {
        Some(i) => focused(ui, cx, &tiles, i, area),
        None => grid_view(ui, cx, &tiles, area),
    }
    controls(ui, cx, bar);
}

fn grid_view(ui: &mut egui::Ui, cx: &mut UiCx<'_>, tiles: &[Tile], area: Rect) {
    let n = tiles.len();
    let (cols, size) = grid(n, area.size());
    let rows = n.div_ceil(cols);
    let total_h = rows as f32 * (size.y + GAP) - GAP;
    let mut child = ui.new_child(UiBuilder::new().max_rect(area));
    egui::ScrollArea::vertical().id_salt("call_grid").auto_shrink(false).show(&mut child, |ui| {
        let (block, _) = ui.allocate_exact_size(vec2(ui.available_width(), total_h.max(area.height())), Sense::hover());
        let top = block.top() + ((block.height() - total_h) / 2.0).max(0.0);
        for (i, t) in tiles.iter().enumerate() {
            let (r, c) = (i / cols, i % cols);
            // The last row is centred too.
            let in_row = (n - r * cols).min(cols) as f32;
            let left = block.center().x - (in_row * (size.x + GAP) - GAP) / 2.0;
            tile(ui, cx, t, Rect::from_min_size(pos2(left + c as f32 * (size.x + GAP), top + r as f32 * (size.y + GAP)), size));
        }
        // More rows below: the cut-off row fades into the stage instead of ending in a sliver.
        let clip = ui.clip_rect();
        if top + total_h > clip.bottom() + 1.0 {
            fade(ui.painter(), Rect::from_min_max(pos2(clip.left(), clip.bottom() - 32.0), clip.max));
        }
    });
}

/// `r` from transparent (top) to the stage colour (bottom).
fn fade(p: &egui::Painter, r: Rect) {
    let mut mesh = egui::Mesh::default();
    for (pos, color) in [(r.left_top(), Color32::TRANSPARENT), (r.right_top(), Color32::TRANSPARENT), (r.right_bottom(), STAGE), (r.left_bottom(), STAGE)] {
        mesh.colored_vertex(pos, color);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(mesh);
}

/// Tile `i` large, the others in a strip below (scrolls sideways when even square tiles don't fit).
fn focused(ui: &mut egui::Ui, cx: &mut UiCx<'_>, tiles: &[Tile], i: usize, area: Rect) {
    // A short stage has no room for the strip: the focused tile alone.
    let others = if area.height() < 200.0 { 0 } else { tiles.len() - 1 };
    let strip_h = if others == 0 { 0.0 } else { (area.height() * 0.2).clamp(72.0, 132.0) };
    let main = area.with_max_y(area.bottom() - if others == 0 { 0.0 } else { strip_h + GAP });
    // First, so it gets one of the 4 remote videos.
    tile(ui, cx, &tiles[i], main);
    if others == 0 {
        return;
    }
    let strip = Rect::from_min_max(pos2(area.left(), main.bottom() + GAP), area.max);
    // 16:9, narrower (down to square) before the strip has to scroll.
    let fit_w = (strip.width() + GAP) / others as f32 - GAP;
    let size = vec2((strip_h * 16.0 / 9.0).min(fit_w).max(strip_h), strip_h);
    let total_w = others as f32 * (size.x + GAP) - GAP;
    let mut child = ui.new_child(UiBuilder::new().max_rect(strip));
    egui::ScrollArea::horizontal().id_salt("call_strip").auto_shrink(false).show(&mut child, |ui| {
        let (block, _) = ui.allocate_exact_size(vec2(total_w.max(strip.width()), strip_h), Sense::hover());
        let left = block.left() + ((block.width() - total_w) / 2.0).max(0.0);
        for (k, t) in tiles.iter().enumerate().filter(|&(j, _)| j != i).map(|(_, t)| t).enumerate() {
            tile(ui, cx, t, Rect::from_min_size(pos2(left + k as f32 * (size.x + GAP), block.top()), size));
        }
    });
}

fn tile(ui: &mut egui::Ui, cx: &mut UiCx<'_>, t: &Tile, rect: Rect) {
    // Scrolled away: not painted, so `sync` hides its video.
    if !ui.is_rect_visible(rect) {
        return;
    }
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 8, t.tint);
    let c = cx.theme.colors.clone();
    // Red marks and the "Live" tag: the danger fill (>= 3:1 on the dark pills too).
    let (green, red) = (presence_color(Presence::Online, &c), crate::theme::Tokens::new(&c).danger_fill);
    let shown = t.video.as_ref().is_some_and(|k| paint_video(&p, &mut cx.view.call_ui, k, rect, t.mine));
    let watch = t.stream.as_ref().filter(|_| !t.mine && !shown);
    // The name pill needs 64 px; smaller tiles (a short stage) show it while hovered.
    let pill_room = rect.height() >= 64.0;
    // A stream not watched shows the button, with the avatar above it when there is room.
    if !shown && (watch.is_none() || rect.height() >= 150.0) {
        let side = (rect.height() * 0.36).clamp(24.0, 96.0);
        let mut c = rect.center() - vec2(0.0, if watch.is_some() { 22.0 } else { 0.0 });
        if pill_room {
            c.y = c.y.min(rect.bottom() - 40.0 - side / 2.0);
        }
        let a = Rect::from_center_size(c, Vec2::splat(side));
        let opacity = if t.stream.is_some() { 0.5 } else if t.ringing { 0.4 } else { 1.0 };
        match &t.avatar {
            Some(url) => user_avatar_at(ui, cx.images, url, a, false, opacity, (t.user, &t.name)),
            None => super::images::paint_initial(&p, a, side / 2.0, t.user.0, &t.name, true),
        }
        if t.ringing {
            // The incoming-call popup's ring, pulsing outward; frames only while someone rings.
            // Reduced motion: a still ring at mid-phase.
            let still = super::motion::secs(ui.ctx(), 1.0) == 0.0;
            let phase = if still { 0.5 } else { (ui.input(|i| i.time) % PULSE_S / PULSE_S) as f32 };
            p.circle_stroke(c, side / 2.0 + 2.0 + side * 0.18 * phase, Stroke::new(3.0, green.gamma_multiply(1.0 - phase)));
            if !still {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
            }
        }
    }
    // High Contrast: every tile outlined, like the rest of that theme. Speaking: the tile, green.
    if crate::theme::is_high_contrast(&c) {
        p.rect_stroke(rect, 8, Stroke::new(1.0, FG_MUTED), StrokeKind::Inside);
    }
    // On at once, off fading over FAST: the ring does not flicker with the voice (principle 18).
    let speak = super::motion::anim(ui.ctx(), ui.make_persistent_id(("rc_speak", &t.key)), t.speaking, 0.0, super::motion::FAST);
    if speak > 0.0 {
        p.rect_stroke(rect, 8, Stroke::new(2.0, green.gamma_multiply(speak)), StrokeKind::Inside);
    }
    if t.stream.is_some() {
        live_pill(&p, rect.left_top() + vec2(8.0, 16.0), red);
    }
    if pill_room || ui.rect_contains_pointer(rect) {
        name_pill(ui, &p, t, rect, red);
    } else if t.muted || t.deaf {
        // No room for the pill: its most severe mark alone, in the corner.
        let c = rect.right_bottom() - Vec2::splat(14.0);
        p.circle_filled(c, 11.0, Color32::from_black_alpha(170));
        let icon = if t.deaf { Icon::Deafened } else { Icon::MicOff };
        icons::paint(&p, icon, Rect::from_center_size(c, Vec2::splat(14.0)), if t.by_server { red } else { FG }, Color32::from_gray(30));
    }

    // Registered after the avatar so the whole tile takes the click; the buttons below win.
    let resp = ui.interact(rect, ui.make_persistent_id(("call_tile", &t.key)), Sense::click());
    let focused = cx.view.call_ui.focus.as_ref() == Some(&t.key);
    let what = if t.stream.is_some() {
        format!("{}'s stream", t.name)
    } else if t.ringing {
        format!("{}, ringing", t.name)
    } else {
        t.name.clone()
    };
    resp.widget_info(|| WidgetInfo::selected(Role::Button, true, focused, &what));
    // The user menu, as on their voice row (VolumeBooster's slider is one of its entries); the
    // profile card opens over the tile.
    let anchor = pos2(rect.center().x - 150.0, rect.top() + 8.0);
    super::sidebar::user_context_menu(&resp, cx, t.user, cx.state.selected_guild, anchor);
    if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
        cx.view.call_ui.focus = if focused { None } else { Some(t.key.clone()) };
    }
    if let Some(key) = watch {
        let joining = cx.view.call_ui.watching.get(key);
        if let Some(Some(clicked)) = joining {
            // A frame when the click expires (`sync` then forgets it), even in an idle window.
            ui.ctx().request_repaint_after(CONNECT_TIMEOUT.saturating_sub(clicked.elapsed()));
        }
        let joining = joining.is_some();
        let b = Rect::from_center_size(pos2(rect.center().x, if rect.height() >= 150.0 { rect.center().y + 40.0 } else { rect.center().y }), vec2(132.0, 32.0));
        let label = egui::RichText::new(if joining { "Joining…" } else { "Watch stream" }).color(c.accent_text.to_color32());
        let button = egui::Button::new(label).fill(c.accent.to_color32()).corner_radius(8).min_size(b.size());
        let enabled = cx.state.voice.connected;
        let layout = egui::Layout::centered_and_justified(egui::Direction::LeftToRight);
        if ui.new_child(UiBuilder::new().max_rect(b).layout(layout)).add_enabled(enabled, button).clicked() {
            cx.view.call_ui.watching.entry(key.clone()).or_insert(Some(Instant::now()));
            cx.cmd.send(video(VideoCommand::WatchStream { stream_key: key.clone() }));
        }
    }
    // Also while joining or before the first frame (a watch that never decodes must be stoppable).
    let call = &cx.view.call_ui;
    let stoppable = |k: &String| call.watching.contains_key(k) || cx.state.voice.video.remote.contains_key(&VideoKey::Stream(k.clone()));
    if let Some(key) = t.stream.as_ref().filter(|k| !t.mine && ui.rect_contains_pointer(rect) && stoppable(k)) {
        // On its own backdrop (the name pill's), legible over a bright shared screen.
        let x = Rect::from_min_size(pos2(rect.right() - 40.0, rect.top() + 8.0), Vec2::splat(32.0));
        let resp = ui.interact(x, ui.make_persistent_id("call_stop_watching"), Sense::click());
        let alpha = if resp.hovered() || resp.has_focus() { 230 } else { 170 };
        ui.painter().circle_filled(x.center(), 16.0, Color32::from_black_alpha(alpha));
        icons::paint(ui.painter(), Icon::Close, Rect::from_center_size(x.center(), Vec2::splat(16.0)), FG, Color32::TRANSPARENT);
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, "Stop watching"));
        if resp.on_hover_text("Stop watching").on_hover_cursor(CursorIcon::PointingHand).clicked() {
            cx.view.call_ui.watching.remove(key);
            cx.view.call_ui.focus = None;
            cx.cmd.send(video(VideoCommand::StopWatching { stream_key: key.clone() }));
        }
    }
}

/// Bottom-left label: mute / deafen marks (red when by a moderator), the name, LIVE or a muted
/// "Ringing…". The name keeps up to 40 px: in a narrow tile LIVE / Ringing… go first (the
/// stream tile / the pulse say it), then all marks but the most severe one (deafened), before
/// the name is elided.
fn name_pill(ui: &egui::Ui, p: &egui::Painter, t: &Tile, rect: Rect, red: Color32) {
    let mut marks: Vec<Icon> = [(t.muted, Icon::MicOff), (t.deaf, Icon::Deafened)].into_iter().filter(|m| m.0).map(|m| m.1).collect();
    let mut live = t.live && t.stream.is_none();
    let font = FontId::proportional(13.0);
    let want = p.layout_no_wrap(t.name.clone(), font.clone(), FG).size().x.min(40.0);
    let ringing = t.ringing.then(|| p.layout_no_wrap("Ringing…".into(), font.clone(), FG_MUTED));
    let ring_w = ringing.as_ref().map_or(0.0, |g| g.size().x + 6.0);
    let room = |marks: usize, live: bool| rect.width() - 32.0 - marks as f32 * 20.0 - if live { 44.0 } else { 0.0 };
    let ringing = ringing.filter(|_| room(0, false) - ring_w >= want);
    let ring_w = if ringing.is_some() { ring_w } else { 0.0 };
    live &= room(marks.len(), live) >= want;
    if room(marks.len(), live) < want {
        marks.drain(..marks.len().saturating_sub(1));
    }
    let g = one_line(ui, t.name.as_str(), font, FG, room(marks.len(), live) - ring_w);
    let w = 16.0 + marks.len() as f32 * 20.0 + g.size().x + if live { 44.0 } else { 0.0 } + ring_w;
    let pill = Rect::from_min_size(pos2(rect.left() + 8.0, rect.bottom() - 32.0), vec2(w, 24.0));
    let bg = Color32::from_black_alpha(170);
    p.rect_filled(pill, 4, bg);
    let mut x = pill.left() + 8.0;
    for icon in marks {
        let color = if t.by_server { red } else { FG };
        icons::paint(p, icon, Rect::from_min_size(pos2(x, pill.center().y - 8.0), Vec2::splat(16.0)), color, Color32::from_gray(30));
        x += 20.0;
    }
    p.galley(pos2(x, pill.center().y - g.size().y / 2.0), g.clone(), FG);
    if live {
        live_pill(p, pos2(x + g.size().x + 6.0, pill.center().y), red);
    }
    if let Some(r) = ringing {
        p.galley(pos2(x + g.size().x + 6.0, pill.center().y - r.size().y / 2.0), r, FG_MUTED);
    }
}

/// A media button: an icon in `fg` on `base`; lit (`fg` fill, icon in `base`) while `on` (camera,
/// screen share), danger-filled with a white icon while `red` (only Disconnect), the icon
/// slashed in the danger colour while `slashed` (mute, deafen), dimmed and inert unless
/// `enabled`. Used by the call bar and the voice panel. The tooltip is also the accessible name.
pub(super) struct MediaButton<'a> {
    pub icon: Icon,
    pub tip: &'a str,
    pub on: bool,
    pub red: bool,
    pub slashed: bool,
    pub enabled: bool,
}

impl MediaButton<'_> {
    /// `colors` = (base, fg, danger); the icon is half the button's height, at least 20 px.
    pub(super) fn show(&self, ui: &mut egui::Ui, rect: Rect, radius: f32, salt: &str, (base, fg, danger): (Color32, Color32, Color32)) -> egui::Response {
        let resp = ui.interact(rect, ui.make_persistent_id(salt), if self.enabled { Sense::click() } else { Sense::hover() });
        let rect = super::motion::pressed(&resp, rect);
        let hot = self.enabled && (resp.hovered() || resp.has_focus());
        let fill = if self.red { danger } else if self.on { fg } else { base };
        let bg = match (self.red || self.on, hot) {
            (_, false) => fill,
            (true, true) => fill.lerp_to_gamma(base, 0.15),
            (false, true) => base.lerp_to_gamma(fg, 0.12),
        };
        let alpha = if self.enabled { 1.0 } else { 0.4 };
        let p = ui.painter();
        p.rect_filled(rect, radius, bg.gamma_multiply(alpha));
        let icon = if self.red { Color32::WHITE } else if self.on { base } else if self.slashed { danger } else { fg };
        let side = (rect.height() * 0.5).max(20.0);
        let shape = if self.slashed { self.icon.off() } else { self.icon };
        icons::paint(p, shape, Rect::from_center_size(rect.center(), Vec2::splat(side)), icon.gamma_multiply(alpha), bg.gamma_multiply(alpha));
        resp.widget_info(|| WidgetInfo::selected(Role::Button, self.enabled, self.on || self.slashed, self.tip));
        let resp = resp.on_hover_text(self.tip);
        if self.enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp }
    }
}

/// Camera button: on with the camera picked in Voice & Video settings, or off.
pub(super) fn toggle_camera(cx: &mut UiCx<'_>) {
    let cmd = match cx.state.voice.video.camera_on {
        true => VideoCommand::StopCamera,
        false => VideoCommand::StartCamera { device: cx.settings.camera.clone() },
    };
    cx.cmd.send(video(cmd));
}

/// Screen-share button: the picker, or Stop Streaming while I share or Go Live is connecting.
pub(super) fn toggle_share(cx: &mut UiCx<'_>) {
    let v = &mut cx.state.voice.video;
    if v.sharing() {
        v.share_starting = false;
        cx.cmd.send(video(VideoCommand::StopScreenShare));
    } else {
        screen_share::open(cx);
    }
}

/// Microphone (`mic`) or headphones clicked: like the user panel's buttons.
fn toggle_mute(cx: &mut UiCx<'_>, mic: bool) {
    let (mute, deaf) = (cx.view.self_mute, cx.view.self_deaf);
    let (m, d) = toggle_voice(mute, deaf, mic);
    (cx.view.self_mute, cx.view.self_deaf) = (m, d);
    if d != deaf {
        cx.cmd.send(Command::SetSelfDeaf(d));
    }
    if m != mute {
        cx.cmd.send(Command::SetSelfMute(m));
    }
}

/// Camera, screen share | mute, deafen | disconnect, centred in `bar`.
fn controls(ui: &mut egui::Ui, cx: &mut UiCx<'_>, bar: Rect) {
    let v = &cx.state.voice;
    let (connected, cam, share) = (v.connected, v.video.camera_on, v.video.sharing());
    let (mute, deaf) = (cx.view.self_mute || cx.view.self_deaf, cx.view.self_deaf);
    let wait = |tip| if connected { tip } else { "Connecting…" };
    // Linux / macOS: no camera or screen capture yet (disabled with the reason).
    let send = connected && capture::SUPPORTED;
    let unsupported = |tip: &'static str, no: &'static str| if capture::SUPPORTED { tip } else { no };
    let buttons = [
        MediaButton { icon: Icon::Camera, tip: unsupported(wait(if cam { "Turn off camera" } else { "Turn on camera" }), capture::NO_CAMERA), on: cam, red: false, slashed: false, enabled: send },
        MediaButton { icon: Icon::Screen, tip: unsupported(wait(if share { "Stop streaming" } else { "Share your screen" }), capture::NO_SHARE), on: share, red: false, slashed: false, enabled: send },
        MediaButton { icon: Icon::Mic, tip: if mute { "Unmute" } else { "Mute" }, on: false, red: false, slashed: mute, enabled: true },
        MediaButton { icon: Icon::Headphones, tip: if deaf { "Undeafen" } else { "Deafen" }, on: false, red: false, slashed: deaf, enabled: true },
        MediaButton { icon: Icon::Hangup, tip: "Disconnect", on: false, red: true, slashed: false, enabled: true },
    ];
    let outline = crate::theme::is_high_contrast(&cx.theme.colors);
    // 48 px circles, 12 px apart, 24 px between the groups; Disconnect a 64 px pill. All scaled
    // down to 40 px buttons in a short stage's 64 px bar.
    let s = ((bar.height() - 24.0) / 48.0).min(1.0);
    let widths = [48.0, 48.0, 48.0, 48.0, 64.0].map(|w| w * s);
    let gaps = [12.0, 24.0, 12.0, 24.0].map(|g| g * s);
    let total: f32 = widths.iter().sum::<f32>() + gaps.iter().sum::<f32>();
    let mut x = bar.center().x - total / 2.0;
    let colors = (BUTTON, FG, cx.theme.colors.danger.to_color32());
    let mut clicked = None;
    for (i, b) in buttons.iter().enumerate() {
        let rect = Rect::from_min_size(pos2(x, bar.center().y - 24.0 * s), vec2(widths[i], 48.0 * s));
        if b.show(ui, rect, 24.0 * s, &format!("call_control_{i}"), colors).clicked() {
            clicked = Some(i);
        }
        if outline {
            ui.painter().rect_stroke(rect, 24.0 * s, Stroke::new(1.0, FG_MUTED), StrokeKind::Inside);
        }
        x += widths[i] + gaps.get(i).copied().unwrap_or(0.0);
    }
    match clicked {
        Some(0) => toggle_camera(cx),
        Some(1) => toggle_share(cx),
        Some(i @ (2 | 3)) => toggle_mute(cx, i == 2),
        Some(4) => cx.cmd.send(Command::LeaveVoice),
        _ => {}
    }
}

/// Test hook `RUSTCORD_CALL_DEMO=<comma list>`, with `RUSTCORD_OPEN=voice:<id>,channel:<id>`
/// (applied once I am in that call): call states offline mode can't reach. No request is sent
/// (the visible videos go to the local voice task like any shown tile). `people` (the cached
/// users join: one speaking, one muted, one deafened and live, one with the camera on),
/// `video` (their camera as a 4:3 test picture, my camera on, the live stream watched),
/// `share` (I share my screen), `focus` (the first tile expanded), `chat` (text chat open),
/// `connecting` (still joining), `picker` (the screen share picker with sample sources),
/// `stream:<picture>` (with `people`: the third user's stream, watched, showing that local PNG /
/// JPEG instead of `video`'s test desktop; README screenshots).
pub(super) fn demo(cx: &mut UiCx<'_>) {
    static FLAGS: OnceLock<String> = OnceLock::new();
    let flags = FLAGS.get_or_init(|| std::env::var("RUSTCORD_CALL_DEMO").unwrap_or_default());
    if flags.is_empty() || cx.view.call_ui.demo_done {
        return;
    }
    let (Some(ch), Some(me), Some(frames)) = (cx.state.voice.channel_id, cx.state.me.as_ref().map(|u| u.id), cx.view.call_ui.frames.clone()) else { return };
    cx.view.call_ui.demo_done = true;
    let guild_id = cx.state.channels.get(&ch).and_then(|c| c.guild_id);
    let mut others: Vec<Id> = cx.state.users.keys().copied().filter(|&u| u != me).collect();
    others.sort_unstable();
    for flag in flags.split(',').map(str::trim) {
        match flag {
            "people" => {
                let states = cx.state.voice_states.entry(ch).or_default();
                for (i, &user_id) in others.iter().take(4).enumerate() {
                    let mut vs = VoiceState { guild_id, channel_id: Some(ch), user_id, ..Default::default() };
                    match i {
                        0 => _ = cx.state.voice.speaking.insert(user_id),
                        1 => vs.self_mute = true,
                        2 => (vs.self_mute, vs.self_deaf, vs.self_stream) = (true, true, true),
                        _ => vs.self_video = true,
                    }
                    states.insert(user_id, vs);
                }
            }
            "video" => {
                let v = &mut cx.state.voice.video;
                v.camera_on = true;
                demo_frame(&frames, VideoKey::User(me), (1280, 720), false);
                if let Some(&cam) = others.get(3) {
                    v.remote.insert(VideoKey::User(cam), (640, 480));
                    demo_frame(&frames, VideoKey::User(cam), (640, 480), false);
                }
                if let Some(&streamer) = others.get(2) {
                    let key = stream_key(guild_id, ch, streamer);
                    v.remote.insert(VideoKey::Stream(key.clone()), (1280, 720));
                    demo_frame(&frames, VideoKey::Stream(key.clone()), (1280, 720), true);
                    cx.view.call_ui.watching.insert(key, Some(Instant::now()));
                }
            }
            "share" => {
                let key = stream_key(guild_id, ch, me);
                demo_frame(&frames, VideoKey::Stream(key.clone()), (1280, 720), true);
                cx.state.voice.video.screen_share = Some(key);
                if let Some(vs) = cx.state.voice_states.get_mut(&ch).and_then(|m| m.get_mut(&me)) {
                    vs.self_stream = true;
                }
            }
            "focus" => cx.view.call_ui.focus = tiles(cx, ch).first().map(|t| t.key.clone()),
            "chat" => cx.view.call_ui.chat = true,
            "connecting" => cx.state.voice.connected = false,
            "picker" => {
                let v = &mut cx.state.voice.video;
                v.monitors = vec![
                    MonitorInfo { source: ScreenSource::Monitor(1), name: "Screen 1".into(), width: 2560, height: 1440, primary: true },
                    MonitorInfo { source: ScreenSource::Monitor(2), name: "Screen 2".into(), width: 1920, height: 1080, primary: false },
                ];
                let window = |n: u64, title: &str, process: &str| WindowInfo { source: ScreenSource::Window(n), title: title.into(), process: process.into() };
                v.windows = vec![window(3, "main.rs - Rustcord", "code.exe"), window(4, "Terminal", "wt.exe"), window(5, "Untitled - Paint", "mspaint.exe")];
                cx.view.call_ui.share = ShareForm::default();
                screen_share::demo_pick(&mut cx.view.call_ui.share, ScreenSource::Monitor(1));
                cx.view.modal = Some(Modal::ScreenShare);
            }
            s if s.starts_with("stream:") => {
                let path = &s["stream:".len()..];
                let (Some(&streamer), Ok(img)) = (others.get(2), image::open(path)) else {
                    eprintln!("rustcord: RUSTCORD_CALL_DEMO stream picture {path}: needs `people` and a readable PNG / JPEG");
                    continue;
                };
                let img = img.to_rgba8();
                let (w, h) = img.dimensions();
                let key = stream_key(guild_id, ch, streamer);
                cx.state.voice.video.remote.insert(VideoKey::Stream(key.clone()), (w, h));
                if let Some(writer) = frames.writer(VideoKey::Stream(key.clone())) {
                    writer.write(w, h, |px| px.copy_from_slice(img.as_raw()));
                }
                cx.view.call_ui.watching.insert(key, Some(Instant::now()));
            }
            _ => {}
        }
    }
}

/// A test picture for the demo: a person in front of a warm wall, or a desktop with two windows.
fn demo_frame(frames: &VideoFrames, key: VideoKey, (w, h): (u32, u32), desktop: bool) {
    let Some(writer) = frames.writer(key) else { return };
    writer.write(w, h, |px| {
        for (i, p) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (x, y) = ((i as u32 % w) as f32 / w as f32, (i as u32 / w) as f32 / h as f32);
            let rgb = if desktop {
                let window = |l: f32, t: f32, r: f32, b: f32| x > l && x < r && y > t && y < b;
                if y > 0.94 {
                    [24, 24, 30]
                } else if window(0.08, 0.1, 0.55, 0.7) {
                    if y < 0.15 { [60, 64, 80] } else { [235, 235, 240] }
                } else if window(0.45, 0.3, 0.9, 0.85) {
                    if y < 0.35 { [50, 50, 56] } else { [30, 30, 36] }
                } else {
                    [(30.0 + 60.0 * x) as u8, (60.0 + 50.0 * y) as u8, 140]
                }
            } else {
                // Distances in picture heights: head and shoulders, centred.
                let dx = (x - 0.5) * w as f32 / h as f32;
                let head = dx * dx + (y - 0.42) * (y - 0.42) < 0.03;
                let body = dx * dx / 0.12 + (y - 1.05) * (y - 1.05) / 0.2 < 1.0;
                if head || body { [70, 60, 58] } else { [(180.0 - 60.0 * y) as u8, (120.0 - 30.0 * y) as u8, (90.0 + 40.0 * x) as u8] }
            };
            *p = [rgb[0], rgb[1], rgb[2], 255];
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::video::VideoEvent;

    #[test]
    fn grid_picks_the_largest_tiles() {
        assert_eq!(grid(1, vec2(1600.0, 900.0)), (1, vec2(1600.0, 900.0)));
        // 4 in a 16:9 area: 2×2.
        assert_eq!(grid(4, vec2(1608.0, 908.0)).0, 2);
        // 3 in a wide area: one row; in a tall one: one column.
        assert_eq!(grid(3, vec2(3000.0, 500.0)).0, 3);
        assert_eq!(grid(3, vec2(500.0, 3000.0)).0, 1);
        let (cols, size) = grid(3, vec2(1200.0, 600.0));
        assert_eq!(cols, 2);
        assert!((size.x / size.y - 16.0 / 9.0).abs() < 1e-3);
        // Too many for the area: MIN_TILE_W wide columns, scrolling.
        let (cols, size) = grid(40, vec2(800.0, 400.0));
        assert_eq!(cols, 4);
        assert!(size.x >= MIN_TILE_W && 4.0 * size.x + 3.0 * GAP <= 800.5);
        assert_eq!(grid(0, vec2(10.0, 10.0)).0, 1, "never zero columns");
        // 6 in a short stage (720 × 420 window): the third row would show 5 px; the tiles shrink
        // until about a third of it peeks out, never under MIN_TILE_W.
        let (cols, size) = grid(6, vec2(418.0, 251.0));
        assert_eq!(cols, 2);
        let peek = (251.0 - 2.0 * (size.y + GAP)) / size.y;
        assert!(size.x >= MIN_TILE_W && (0.3..0.4).contains(&peek), "{size:?} {peek}");
        // The short stage above a DM's chat: no room for two rows, so everyone in one row,
        // smaller, never taller than the area.
        let (cols, size) = grid(4, vec2(418.0, 67.0));
        assert!(cols == 4 && size.y <= 67.0 && 4.0 * size.x + 3.0 * GAP <= 418.5, "{size:?}");
        let (cols, size) = grid(2, vec2(418.0, 67.0));
        assert!(cols == 2 && (size.y - 67.0).abs() < 1e-3, "{size:?}");
    }

    #[test]
    fn fit_letterboxes() {
        let tile = Rect::from_min_size(pos2(0.0, 0.0), vec2(160.0, 90.0));
        assert_eq!(fit(tile, vec2(640.0, 480.0)), Rect::from_min_max(pos2(20.0, 0.0), pos2(140.0, 90.0)));
        assert_eq!(fit(tile, vec2(1920.0, 1080.0)), tile);
        assert_eq!(fit(tile, vec2(1000.0, 100.0)), Rect::from_min_max(pos2(0.0, 37.0), pos2(160.0, 53.0)));
        assert_eq!(fit(tile, Vec2::ZERO), tile);
    }

    #[test]
    fn at_most_four_remote_videos_and_sync_sends_changes() {
        let ctx = egui::Context::default();
        let frames = Arc::new(VideoFrames::new(ctx.clone()));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cmd = CommandSender(tx);
        let mut state = AppState::default();
        let mut view = ViewState::default();
        let p = egui::Painter::new(ctx, egui::LayerId::background(), Rect::EVERYTHING);
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(16.0, 9.0));
        assert!(!paint_video(&p, &mut view.call_ui, &VideoKey::User(Id(1)), r, false), "no registry before the first sync");
        sync(&mut view, &mut state, &cmd, &frames);

        let writer = frames.writer(VideoKey::User(Id(1))).unwrap();
        writer.write(4, 2, |px| px.fill(255));
        assert!(paint_video(&p, &mut view.call_ui, &VideoKey::User(Id(1)), r, false), "a frame is painted");
        let generation = |view: &ViewState| view.call_ui.textures[&VideoKey::User(Id(1))].0;
        assert_eq!(generation(&view), 1);
        for u in 2..=6 {
            assert!(!paint_video(&p, &mut view.call_ui, &VideoKey::User(Id(u)), r, false), "no frame yet");
        }
        paint_video(&p, &mut view.call_ui, &VideoKey::User(Id(9)), r, true);
        assert_eq!(view.call_ui.want.len(), 5, "4 remote + my preview");
        assert!(!view.call_ui.want.contains(&VideoKey::User(Id(5))));

        sync(&mut view, &mut state, &cmd, &frames);
        let shown = |rx: &mut tokio::sync::mpsc::UnboundedReceiver<Command>, on: bool| {
            std::iter::from_fn(|| rx.try_recv().ok()).filter(|c| matches!(c, Command::Video(VideoCommand::SetVideoVisible { visible, .. }) if *visible == on)).count()
        };
        assert_eq!(shown(&mut rx, true), 5);
        // Painted again with a new frame (texture updated); the others are hidden; then nothing
        // painted (minimized, call view closed).
        writer.write(4, 2, |px| px.fill(128));
        paint_video(&p, &mut view.call_ui, &VideoKey::User(Id(1)), r, false);
        assert_eq!(generation(&view), 2);
        sync(&mut view, &mut state, &cmd, &frames);
        assert_eq!(shown(&mut rx, false), 4);
        sync(&mut view, &mut state, &cmd, &frames);
        assert_eq!(shown(&mut rx, false), 1);
        assert!(view.call_ui.visible.is_empty() && view.call_ui.textures.is_empty());
    }

    #[test]
    fn an_ended_camera_is_asked_for_again_and_a_failed_watch_is_forgotten() {
        let ctx = egui::Context::default();
        let frames = Arc::new(VideoFrames::new(ctx.clone()));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cmd = CommandSender(tx);
        let (mut state, mut view) = (AppState::default(), ViewState::default());
        let p = egui::Painter::new(ctx, egui::LayerId::background(), Rect::EVERYTHING);
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(16.0, 9.0));
        let cam = VideoKey::User(Id(5));
        let asked = |rx: &mut tokio::sync::mpsc::UnboundedReceiver<Command>| {
            std::iter::from_fn(|| rx.try_recv().ok()).filter(|c| matches!(c, Command::Video(VideoCommand::SetVideoVisible { visible: true, .. }))).count()
        };
        sync(&mut view, &mut state, &cmd, &frames);
        paint_video(&p, &mut view.call_ui, &cam, r, false);
        sync(&mut view, &mut state, &cmd, &frames);
        assert_eq!(asked(&mut rx), 1);
        // Off and on again between two frames: still painted, so asked for once more.
        paint_video(&p, &mut view.call_ui, &cam, r, false);
        for ev in [VideoEvent::RemoteVideoEnded { key: cam.clone() }, VideoEvent::RemoteVideo { key: cam.clone(), width: 0, height: 0 }] {
            state.voice.video.apply(ev);
        }
        sync(&mut view, &mut state, &cmd, &frames);
        paint_video(&p, &mut view.call_ui, &cam, r, false);
        sync(&mut view, &mut state, &cmd, &frames);
        assert_eq!(asked(&mut rx), 1);
        assert!(state.voice.video.ended.is_empty());

        // Watch clicks: one that got nowhere within CONNECT_TIMEOUT is forgotten, a connected
        // one (in `remote`) and a fresh one stay.
        let ch = Id(2);
        (state.voice.channel_id, state.selected_channel) = (Some(ch), Some(ch));
        let keys: Vec<String> = (3..6).map(|owner| stream_key(Some(Id(1)), ch, Id(owner))).collect();
        for owner in 3..6 {
            let vs = VoiceState { channel_id: Some(ch), user_id: Id(owner), self_stream: true, ..Default::default() };
            state.voice_states.entry(ch).or_default().insert(Id(owner), vs);
        }
        let old = Instant::now().checked_sub(CONNECT_TIMEOUT + std::time::Duration::from_secs(1));
        view.call_ui.watching = HashMap::from([(keys[0].clone(), old), (keys[1].clone(), old), (keys[2].clone(), Some(Instant::now()))]);
        state.voice.video.remote.insert(VideoKey::Stream(keys[1].clone()), (0, 0));
        sync(&mut view, &mut state, &cmd, &frames);
        assert_eq!(view.call_ui.watching.len(), 2);
        assert_eq!(view.call_ui.watching.get(&keys[1]), Some(&None), "connected");
        assert!(view.call_ui.watching.contains_key(&keys[2]));
    }

    #[test]
    fn leaving_the_call_view_stops_watching() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cmd = CommandSender(tx);
        let mut view = ViewState::default();
        let mut state = AppState::default();
        let (ch, owner) = (Id(2), Id(3));
        let key = stream_key(Some(Id(1)), ch, owner);
        state.voice.channel_id = Some(ch);
        state.selected_channel = Some(ch);
        let vs = VoiceState { channel_id: Some(ch), user_id: owner, self_stream: true, ..Default::default() };
        state.voice_states.entry(ch).or_default().insert(owner, vs);
        view.call_ui.watching.insert(key.clone(), Some(Instant::now()));
        let frames = Arc::new(VideoFrames::new(egui::Context::default()));
        sync(&mut view, &mut state, &cmd, &frames);
        assert!(view.call_ui.watching.contains_key(&key), "still streaming, still open");
        state.selected_channel = Some(Id(7));
        sync(&mut view, &mut state, &cmd, &frames);
        assert!(view.call_ui.watching.is_empty());
        assert!(std::iter::from_fn(|| rx.try_recv().ok()).any(|c| matches!(c, Command::Video(VideoCommand::StopWatching { stream_key }) if stream_key == key)));
    }
}
