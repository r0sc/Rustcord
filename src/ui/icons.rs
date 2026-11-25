//! Rustcord's own vector icon set (docs/DESIGN.md §8): simple geometry on a 24-unit grid with a
//! 2-unit margin, painted as egui shapes, so no font has to carry an icon and every icon takes
//! the context's colour. Drawn from scratch; never another app's icon files or path data.
//!
//! - Strokes are 2 units (at least 1.5 physical px) with round ends (a dot at every open end:
//!   epaint has no round caps).
//! - Solid objects are unions of convex fills (epaint fills convex polygons only), overlapping
//!   so no seam shows.
//! - `bg` paints cut-outs: the gap beside an "off" slash, badge rings, the gap between two
//!   overlapping shapes. Pass the fill the icon sits on; `TRANSPARENT` skips them.
//! - Paint with an opaque colour where shapes overlap: a translucent one is blended onto an
//!   opaque `bg` first, otherwise overlaps would show darker.

use crate::model::{Channel, channel_type as ct, perm};
use egui::emath::GuiRounding;
use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};
use std::f32::consts::FRAC_1_SQRT_2 as H;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Icon {
    // Channel kinds (§8.4); `Media` is also the image icon, `Directory` the folder.
    Text,
    Voice,
    Announcement,
    Stage,
    Forum,
    Media,
    Thread,
    Rules,
    Directory,
    // Activity types.
    Playing,
    Listening,
    Streaming,
    Watching,
    Competing,
    // Voice, video and calls.
    Mic,
    MicOff,
    Headphones,
    Deafened,
    Camera,
    CameraOff,
    Screen,
    Hangup,
    Phone,
    Gear,
    // People.
    Person,
    PersonAdd,
    PersonRemove,
    Members,
    At,
    IdCard,
    // Actions.
    Close,
    Plus,
    Minus,
    Check,
    Search,
    Edit,
    Trash,
    Reply,
    Forward,
    Copy,
    Link,
    External,
    Leave,
    Block,
    Shield,
    Ban,
    Refresh,
    Reset,
    Sort,
    Grip,
    More,
    // Things.
    Inbox,
    Pin,
    Message,
    Bell,
    BellOff,
    Archive,
    Envelope,
    SplitView,
    File,
    Attach,
    Hourglass,
    Key,
    Desktop,
    Card,
    Video,
    Eye,
    EyeOff,
    Lock,
    Unlock,
    Boost,
    Clock,
    Star,
    Globe,
    Info,
    Poll,
    // Quests (ui/quests.rs).
    Quest,
    Orb,
    Gift,
    // Server marks (`guild_mark`).
    Verified,
    Partnered,
    BoostTier1,
    BoostTier2,
    BoostTier3,
    // Media (ui/viewer.rs).
    Play,
    Pause,
    Download,
    VoiceOff,
    Expand,
    Collapse,
    // Arrows.
    ArrowLeft,
    ArrowRight,
    ChevronUp,
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    // Emoji-picker categories (with `Clock`, `Star`).
    Smile,
    Leaf,
    Food,
    Ball,
    Car,
    Bulb,
    Heart,
    Flag,
    // Settings pages.
    Palette,
    Accessibility,
    Keyboard,
    Puzzle,
}

/// Small mark at the bottom right of a channel icon: private (`Lock`) or age-restricted
/// (`Alert`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Badge {
    Lock,
    Alert,
}

impl Icon {
    /// Every icon, in declaration order (the icon sheet hook, tests).
    pub(crate) const ALL: [Icon; 108] = {
        use Icon::*;
        [
            Text, Voice, Announcement, Stage, Forum, Media, Thread, Rules, Directory, Playing, Listening, Streaming, Watching, Competing, Mic, MicOff, Headphones,
            Deafened, Camera, CameraOff, Screen, Hangup, Phone, Gear, Person, PersonAdd, PersonRemove, Members, At, IdCard, Close, Plus, Minus, Check, Search, Edit,
            Trash, Reply, Forward, Copy, Link, External, Leave, Block, Shield, Ban, Refresh, Reset, Sort, Grip, More, Inbox, Pin, Message, Bell, BellOff, Archive, Envelope,
            SplitView, File, Attach, Hourglass, Key, Desktop, Card, Video, Eye, EyeOff, Lock, Unlock, Boost, Clock, Star, Globe, Info, Poll, Quest, Orb, Gift, Verified, Partnered, BoostTier1, BoostTier2, BoostTier3, Play, Pause, Download, VoiceOff, Expand, Collapse, ArrowLeft, ArrowRight, ChevronUp,
            ChevronDown,
            ChevronLeft, ChevronRight, Smile, Leaf, Food, Ball, Car, Bulb, Heart, Flag,
            Palette, Accessibility, Keyboard, Puzzle,
        ]
    };

    /// The crossed-out "off" form (`self` when it has none).
    pub(crate) fn off(self) -> Icon {
        match self {
            Icon::Mic => Icon::MicOff,
            Icon::Headphones => Icon::Deafened,
            Icon::Camera => Icon::CameraOff,
            Icon::Eye => Icon::EyeOff,
            Icon::Bell => Icon::BellOff,
            Icon::Voice => Icon::VoiceOff,
            i => i,
        }
    }
}

/// Paints `icon` into the square `rect` in `color`; `bg` is the fill it sits on (cut-outs).
pub(crate) fn paint(p: &Painter, icon: Icon, rect: Rect, color: Color32, bg: Color32) {
    p.extend(shapes(icon, None, rect, color, bg, p.pixels_per_point()));
}

/// Paints a channel icon with its badge (`channel` / `of`).
pub(crate) fn paint_channel(p: &Painter, (icon, badge): (Icon, Option<Badge>), rect: Rect, color: Color32, bg: Color32) {
    p.extend(shapes(icon, badge, rect, color, bg, p.pixels_per_point()));
}

/// The icon of a channel kind (§8.4), the single source for every place a channel shows its
/// kind. `locked`: @everyone can't see it; `rules`: the server's rules channel.
pub(crate) fn channel(kind: u8, locked: bool, nsfw: bool, rules: bool) -> (Icon, Option<Badge>) {
    let icon = match kind {
        ct::GUILD_VOICE => Icon::Voice,
        ct::GUILD_STAGE_VOICE => Icon::Stage,
        ct::GUILD_ANNOUNCEMENT => Icon::Announcement,
        ct::GUILD_FORUM => Icon::Forum,
        ct::GUILD_MEDIA => Icon::Media,
        ct::ANNOUNCEMENT_THREAD | ct::PUBLIC_THREAD | ct::PRIVATE_THREAD => Icon::Thread,
        ct::GUILD_DIRECTORY => Icon::Directory,
        ct::GUILD_CATEGORY => Icon::ChevronDown,
        ct::DM => Icon::At,
        ct::GROUP_DM => Icon::Members,
        ct::GUILD_TEXT if rules => Icon::Rules,
        _ => Icon::Text,
    };
    let lockable = !matches!(icon, Icon::Directory | Icon::ChevronDown | Icon::At | Icon::Members);
    let badge = if (locked && lockable) || kind == ct::PRIVATE_THREAD {
        Some(Badge::Lock)
    } else if nsfw && matches!(icon, Icon::Text | Icon::Announcement | Icon::Voice | Icon::Forum | Icon::Media) {
        Some(Badge::Alert)
    } else {
        None
    };
    (icon, badge)
}

/// `channel` for a channel as stored.
pub(crate) fn of(ch: &Channel, s: &crate::state::AppState) -> (Icon, Option<Badge>) {
    let everyone = ch.guild_id.unwrap_or_default();
    let locked = ch.guild_id.is_some() && ch.permission_overwrites.iter().any(|o| o.id == everyone && o.deny.0 & perm::VIEW_CHANNEL != 0);
    let rules = ch.guild_id.and_then(|g| s.guild(g)?.rules_channel_id) == Some(ch.id);
    channel(ch.kind, locked, ch.nsfw, rules)
}

/// The icon of an activity type (0 playing and unknown types, 1 streaming, 2 listening,
/// 3 watching, 5 competing).
pub(crate) fn activity(kind: u8) -> Icon {
    match kind {
        1 => Icon::Streaming,
        2 => Icon::Listening,
        3 => Icon::Watching,
        5 => Icon::Competing,
        _ => Icon::Playing,
    }
}

/// The vector for an emoji or glyph that served as an icon.
// shortcut: glyph keys stay in call sites, replace with Icon variants when a file is touched.
pub(crate) fn from_glyph(glyph: &str) -> Option<Icon> {
    use Icon::*;
    Some(match glyph.trim_end_matches('\u{FE0F}') {
        "×" | "✖" | "✕" => Close,
        "✏" => Edit,
        "✔" | "✓" => Check,
        "+" | "➕" | "🏠" => Plus,
        "−" => Minus,
        "➖" | "👢" => PersonRemove,
        "🗑" => Trash,
        "💬" => Message,
        "⟳" => Refresh,
        "↺" => Reset,
        "⚙" => Gear,
        "☺" | "🙂" => Smile,
        "‹" => ChevronLeft,
        "›" => ChevronRight,
        "🔒" => Lock,
        "🔓" => Unlock,
        "📥" => Inbox,
        "📌" => Pin,
        "👥" | "👪" => Members,
        "👤" => Person,
        "👁" => Eye,
        "⌛" | "⏳" => Hourglass,
        "↩" => Reply,
        "🆔" => IdCard,
        "🚪" => Leave,
        "🚫" => Block,
        "🛡" => Shield,
        "🗄" => Archive,
        "🔨" => Ban,
        "🔔" => Bell,
        "🔕" => BellOff,
        "#" => Text,
        "🔊" => Voice,
        "📢" => Announcement,
        "🎙" => Stage,
        "🖼" => Media,
        "📁" => Directory,
        "🧵" => Thread,
        "📞" => Phone,
        "📎" => Attach,
        "📄" => File,
        "🔍" => Search,
        "📷" => Camera,
        "✉" => Envelope,
        "⊞" => SplitView,
        "@" => At,
        "↗" => External,
        "→" => ArrowRight,
        "←" => ArrowLeft,
        "🚀" => Boost,
        "🔑" => Key,
        "🖥" => Desktop,
        "💳" => Card,
        "🎮" => Playing,
        "🎬" => Video,
        "🔌" | "🔗" => Link,
        "📋" => Copy,
        "⠿" => Grip,
        "↕" => Sort,
        "⋯" | "…" => More,
        "🕒" | "⏱" => Clock,
        "🌐" => Globe,
        "ℹ" => Info,
        "⭐" => Star,
        "🌿" => Leaf,
        "🍔" => Food,
        "⚽" => Ball,
        "🚗" => Car,
        "💡" => Bulb,
        "❤" => Heart,
        "🏁" => Flag,
        _ => return None,
    })
}

/// The shapes of `icon` (and its `badge`) in `rect`, snapped to the pixel grid.
fn shapes(icon: Icon, badge: Option<Badge>, rect: Rect, color: Color32, bg: Color32, ppp: f32) -> Vec<Shape> {
    let color = if color.a() < 255 && bg.a() == 255 { bg.blend(color) } else { color };
    let mut k = Pen::new(rect.round_to_pixels(ppp), color, bg, ppp);
    draw(&mut k, icon, badge == Some(Badge::Lock));
    match badge {
        Some(Badge::Lock) => {
            k.cut(|k| {
                k.rrect(12.5, 15.5, 24.0, 25.0, 3.0);
                k.width(1.75 + 3.0);
                k.arc(18.25, 17.0, 2.6, 180.0, 360.0);
            });
            k.width(1.75);
            k.rrect(14.0, 17.0, 22.5, 23.5, 1.5);
            k.arc(18.25, 17.0, 2.6, 180.0, 360.0);
        }
        Some(Badge::Alert) => {
            let tri = [(17.5, 12.5), (23.0, 22.5), (12.0, 22.5)];
            k.cut(|k| {
                k.width(3.0);
                k.closed(&tri);
                k.fill(&tri);
            });
            k.fill(&tri);
            if !k.small {
                k.cut(|k| {
                    k.width(1.75);
                    k.line(&[(17.5, 15.75), (17.5, 18.5)]);
                    k.disc(17.5, 20.75, 0.9);
                });
            }
        }
        None => {}
    }
    k.out
}

type P = (f32, f32);

/// Drawing state: unit grid origin and scale, current stroke, the ink (colour or `bg` while
/// cutting), and the shapes so far.
struct Pen {
    o: Pos2,
    u: f32,
    min_w: f32,
    w: f32,
    ink: Color32,
    fg: Color32,
    bg: Color32,
    /// Under 18 px: details under 3 units are left out.
    small: bool,
    out: Vec<Shape>,
}

impl Pen {
    fn new(rect: Rect, color: Color32, bg: Color32, ppp: f32) -> Self {
        let u = rect.width() / 24.0;
        let min_w = 1.5 / ppp;
        Self { o: rect.min, u, min_w, w: (2.0 * u).max(min_w), ink: color, fg: color, bg, small: rect.width() < 18.0, out: Vec::new() }
    }

    fn at(&self, (x, y): P) -> Pos2 {
        self.o + vec2(x, y) * self.u
    }

    fn all(&self, pts: &[P]) -> Vec<Pos2> {
        let mut v: Vec<Pos2> = pts.iter().map(|&p| self.at(p)).collect();
        // Nearly equal neighbours (arc joins) give egui's miter a random normal: a long spike.
        v.dedup_by(|b, a| a.distance(*b) < 0.01);
        v
    }

    /// `all` for a closed outline: an end that repeats the start goes too.
    fn loop_pts(&self, pts: &[P]) -> Vec<Pos2> {
        let mut v = self.all(pts);
        if v.len() > 2 && v[0].distance(v[v.len() - 1]) < 0.01 {
            v.pop();
        }
        v
    }

    /// Stroke width in units (at least 1.5 physical px).
    fn width(&mut self, units: f32) {
        self.w = (units * self.u).max(self.min_w);
    }

    /// Runs `f` in the background colour; nothing on a transparent background.
    fn cut(&mut self, f: impl FnOnce(&mut Self)) {
        let w = self.w;
        if self.bg.a() > 0 {
            self.ink = self.bg;
            f(self);
            self.ink = self.fg;
        }
        self.w = w;
    }

    /// Open polyline with round ends.
    fn line(&mut self, pts: &[P]) {
        let v = self.all(pts);
        let r = self.w / 2.0;
        for end in [v[0], v[v.len() - 1]] {
            self.out.push(Shape::circle_filled(end, r, self.ink));
        }
        self.out.push(Shape::line(v, Stroke::new(self.w, self.ink)));
    }

    fn closed(&mut self, pts: &[P]) {
        self.out.push(Shape::closed_line(self.loop_pts(pts), Stroke::new(self.w, self.ink)));
    }

    /// Filled convex polygon.
    fn fill(&mut self, pts: &[P]) {
        self.out.push(Shape::convex_polygon(self.loop_pts(pts), self.ink, Stroke::NONE));
    }

    fn disc(&mut self, x: f32, y: f32, r: f32) {
        self.out.push(Shape::circle_filled(self.at((x, y)), r * self.u, self.ink));
    }

    fn ring(&mut self, x: f32, y: f32, r: f32) {
        self.out.push(Shape::circle_stroke(self.at((x, y)), r * self.u, Stroke::new(self.w, self.ink)));
    }

    fn rrect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) {
        self.fill(&bubble(x0, y0, x1, y1, r, None));
    }

    fn rrect_line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) {
        self.closed(&bubble(x0, y0, x1, y1, r, None));
    }

    fn arc(&mut self, x: f32, y: f32, r: f32, a0: f32, a1: f32) {
        self.line(&arc(x, y, r, a0, a1));
    }

    /// A speech bubble filled, its tail at the bottom left or right.
    fn bubble_fill(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, r: f32, right: bool) {
        self.rrect(x0, y0, x1, y1, r);
        let tail = if right { [(x1, y1 - r), (x1, y1 + 2.5), (x1 - r - 1.0, y1)] } else { [(x0, y1 - r), (x0 + r + 1.0, y1), (x0, y1 + 2.5)] };
        self.fill(&tail);
    }

    /// The crossed-out form: a slash with a gap in `bg` on both sides.
    fn slash(&mut self) {
        let ends = [(20.5, 3.5), (3.5, 20.5)];
        let w = self.w;
        self.cut(|k| {
            k.w = w + 2.5 * k.u;
            k.line(&ends);
        });
        self.line(&ends);
    }
}

/// Points of an ellipse arc (degrees, clockwise on screen: 0 right, 90 down).
fn ellipse(x: f32, y: f32, rx: f32, ry: f32, a0: f32, a1: f32) -> Vec<P> {
    let n = ((a1 - a0).abs() / 15.0).ceil().clamp(4.0, 16.0) as usize;
    (0..=n)
        .map(|i| {
            let a = (a0 + (a1 - a0) * i as f32 / n as f32).to_radians();
            (x + rx * a.cos(), y + ry * a.sin())
        })
        .collect()
}

fn arc(x: f32, y: f32, r: f32, a0: f32, a1: f32) -> Vec<P> {
    ellipse(x, y, r, r, a0, a1)
}

/// Rounded rectangle outline, clockwise; `tail` = Some(right) replaces a bottom corner with a
/// speech-bubble tail.
fn bubble(x0: f32, y0: f32, x1: f32, y1: f32, r: f32, tail: Option<bool>) -> Vec<P> {
    let mut v = arc(x0 + r, y0 + r, r, 180.0, 270.0);
    v.extend(arc(x1 - r, y0 + r, r, 270.0, 360.0));
    match tail {
        Some(true) => v.extend([(x1, y1 - r), (x1, y1 + 2.5), (x1 - r - 1.0, y1)]),
        _ => v.extend(arc(x1 - r, y1 - r, r, 0.0, 90.0)),
    }
    match tail {
        Some(false) => v.extend([(x0 + r + 1.0, y1), (x0, y1 + 2.5), (x0, y1 - r)]),
        _ => v.extend(arc(x0 + r, y1 - r, r, 90.0, 180.0)),
    }
    v
}

/// Quadratic Bézier from `a` over `c` to `b`.
fn quad(a: P, c: P, b: P) -> Vec<P> {
    (0..=12)
        .map(|i| {
            let t = i as f32 / 12.0;
            let s = 1.0 - t;
            (s * s * a.0 + 2.0 * s * t * c.0 + t * t * b.0, s * s * a.1 + 2.0 * s * t * c.1 + t * t * b.1)
        })
        .collect()
}

/// `pts` turned by `deg` about the centre, then moved by `shift`.
fn turn(pts: &[P], deg: f32, shift: P) -> Vec<P> {
    let (s, c) = deg.to_radians().sin_cos();
    pts.iter().map(|&(x, y)| (12.0 + (x - 12.0) * c - (y - 12.0) * s + shift.0, 12.0 + (x - 12.0) * s + (y - 12.0) * c + shift.1)).collect()
}

fn mirror(pts: &[P]) -> Vec<P> {
    pts.iter().map(|&(x, y)| (24.0 - x, y)).collect()
}

/// The hash of a text channel, scaled by `s` about (12, 12) and moved by `d`.
fn hash(k: &mut Pen, s: f32, d: P) {
    let at = |x: f32, y: f32| (12.0 + (x - 12.0) * s + d.0, 12.0 + (y - 12.0) * s + d.1);
    k.line(&[at(10.5, 3.5), at(7.5, 20.5)]);
    k.line(&[at(16.5, 3.5), at(13.5, 20.5)]);
    k.line(&[at(4.5, 8.5), at(20.5, 8.5)]);
    k.line(&[at(3.5, 15.5), at(19.5, 15.5)]);
}

/// Head and shoulders centred on `x`.
fn person(k: &mut Pen, x: f32, head: f32, rx: f32) {
    k.disc(x, 7.5, head);
    k.fill(&ellipse(x, 21.0, rx, 7.5, 180.0, 360.0));
}

/// Circular arrow: clockwise (refresh) or mirrored (reset).
fn circular(k: &mut Pen, ccw: bool) {
    let a = 320f32.to_radians();
    let e = (12.0 + 7.5 * a.cos(), 12.0 + 7.5 * a.sin());
    let (t, n) = ((-a.sin(), a.cos()), (-a.cos(), -a.sin()));
    let head = [(e.0 - 3.0 * (t.0 - n.0), e.1 - 3.0 * (t.1 - n.1)), e, (e.0 - 3.0 * (t.0 + n.0), e.1 - 3.0 * (t.1 + n.1))];
    let body = arc(12.0, 12.0, 7.5, 30.0, 320.0);
    if ccw {
        k.line(&mirror(&body));
        k.line(&mirror(&head));
    } else {
        k.line(&body);
        k.line(&head);
    }
}

/// An almond eye with its pupil.
fn eye(k: &mut Pen) {
    let mut pts = quad((2.5, 12.0), (12.0, 3.5), (21.5, 12.0));
    pts.extend(quad((21.5, 12.0), (12.0, 20.5), (2.5, 12.0)).into_iter().skip(1).take(11));
    k.closed(&pts);
    k.disc(12.0, 12.0, 2.75);
}

/// The small gem inside the boost level 2 / 3 marks.
const SMALL_GEM: [P; 5] = [(9.5, 7.5), (14.5, 7.5), (16.75, 10.25), (12.0, 16.75), (7.25, 10.25)];

/// The scalloped seal of the verified / partnered marks: a disc with 12 bumps (extent 10).
fn seal(k: &mut Pen) {
    k.disc(12.0, 12.0, 7.75);
    for i in 0..12 {
        let (s, c) = (30.0 * i as f32).to_radians().sin_cos();
        k.disc(12.0 + 7.75 * c, 12.0 + 7.75 * s, 2.25);
    }
}

/// The one mark a server shows before its name (rail tooltip, header, tag card): verified, else
/// partnered, else its boost level 1-3, with its label. Monochrome like every icon (§8.2).
pub(crate) fn guild_mark(features: &[String], premium_tier: u8) -> Option<(Icon, &'static str)> {
    let has = |f: &str| features.iter().any(|x| x == f);
    Some(if has("VERIFIED") {
        (Icon::Verified, "Verified server")
    } else if has("PARTNERED") {
        (Icon::Partnered, "Partnered server")
    } else {
        match premium_tier {
            1 => (Icon::BoostTier1, "Boost level 1"),
            2 => (Icon::BoostTier2, "Boost level 2"),
            3 => (Icon::BoostTier3, "Boost level 3"),
            _ => return None,
        }
    })
}

/// Monitor outline and stand (desktop, screen share).
fn monitor(k: &mut Pen) {
    k.rrect_line(2.5, 4.0, 21.5, 16.5, 2.0);
    k.line(&[(12.0, 16.5), (12.0, 20.0)]);
    k.line(&[(8.0, 20.5), (16.0, 20.5)]);
}

/// A handset lying down (hang up), or standing (`turned`, start a call).
fn handset(k: &mut Pen, turned: bool) {
    let t = |pts: &[P]| if turned { turn(pts, -135.0, (0.0, 0.0)) } else { pts.to_vec() };
    let w = k.w;
    k.width(3.5);
    let band = t(&arc(12.0, 17.0, 8.5, 205.0, 335.0));
    k.line(&band);
    k.w = w;
    k.fill(&t(&bubble(2.5, 12.5, 8.0, 17.0, 1.75, None)));
    k.fill(&t(&bubble(16.0, 12.5, 21.5, 17.0, 1.75, None)));
}

fn draw(k: &mut Pen, icon: Icon, locked: bool) {
    use Icon::*;
    match icon {
        Text => hash(k, 1.0, (0.0, 0.0)),
        Voice | VoiceOff => {
            k.rrect(3.0, 9.0, 7.5, 15.0, 1.0);
            k.fill(&[(7.0, 9.0), (12.0, 4.5), (12.0, 19.5), (7.0, 15.0)]);
            k.arc(12.5, 12.0, 4.25, -45.0, 45.0);
            // A lock badge takes the outer wave's place.
            if !locked {
                k.arc(12.5, 12.0, 8.0, -48.0, 48.0);
            }
            if icon == VoiceOff {
                k.slash();
            }
        }
        Announcement => {
            k.fill(&[(4.5, 9.0), (17.0, 4.25), (17.0, 19.75), (4.5, 15.0)]);
            k.rrect(2.5, 9.0, 5.0, 15.0, 1.0);
            k.line(&[(7.0, 15.5), (8.5, 20.0)]);
            k.line(&[(20.5, 10.0), (20.5, 14.0)]);
        }
        Stage => {
            k.disc(12.0, 8.75, 3.25);
            k.fill(&ellipse(12.0, 20.0, 5.5, 6.0, 180.0, 360.0));
            k.arc(12.0, 10.0, 7.5, -35.0, 35.0);
            k.arc(12.0, 10.0, 7.5, 145.0, 215.0);
        }
        Forum => {
            k.closed(&bubble(2.0, 3.0, 15.0, 13.0, 3.5, Some(false)));
            k.cut(|k| {
                k.width(3.0);
                k.closed(&bubble(9.5, 10.0, 22.0, 19.0, 3.0, Some(true)));
                k.bubble_fill(9.5, 10.0, 22.0, 19.0, 3.0, true);
            });
            k.bubble_fill(9.5, 10.0, 22.0, 19.0, 3.0, true);
        }
        Media => {
            k.rrect_line(3.0, 4.0, 21.0, 20.0, 3.0);
            k.fill(&[(6.0, 17.25), (10.5, 11.5), (14.25, 15.54), (14.25, 17.25)]);
            k.fill(&[(13.75, 15.0), (15.75, 12.75), (18.5, 17.25), (13.75, 17.25)]);
            k.disc(16.0, 8.5, 1.75);
        }
        Thread => {
            k.closed(&bubble(2.5, 3.5, 21.5, 17.0, 4.0, Some(false)));
            if !k.small {
                k.width(1.5);
                hash(k, 0.4, (0.0, -1.75));
            }
        }
        Rules => {
            k.rrect_line(5.0, 3.0, 19.0, 21.0, 2.0);
            k.line(&[(8.5, 12.5), (11.0, 15.0), (15.5, 9.5)]);
        }
        Directory => {
            k.rrect(2.5, 7.0, 21.5, 19.5, 2.0);
            k.rrect(2.5, 4.5, 10.0, 8.0, 1.5);
        }
        Playing => {
            k.rrect_line(2.5, 6.5, 21.5, 17.5, 5.5);
            k.line(&[(5.75, 12.0), (10.25, 12.0)]);
            k.line(&[(8.0, 9.75), (8.0, 14.25)]);
            k.disc(15.5, 10.5, 1.4);
            k.disc(17.75, 13.5, 1.4);
        }
        Listening => {
            for (x, y) in [(6.75, 17.5), (16.75, 15.5)] {
                let head: Vec<P> = ellipse(0.0, 0.0, 3.0, 2.3, 0.0, 360.0).into_iter().map(|(a, b)| {
                    let (s, c) = (-20f32).to_radians().sin_cos();
                    (x + a * c - b * s, y + a * s + b * c)
                }).collect();
                k.fill(&head);
            }
            k.line(&[(9.25, 17.0), (9.25, 7.0)]);
            k.line(&[(19.25, 15.0), (19.25, 5.0)]);
            k.fill(&[(8.25, 5.25), (20.25, 3.25), (20.25, 7.0), (8.25, 9.0)]);
        }
        Streaming => {
            k.disc(12.0, 12.0, 2.25);
            for r in [5.5, 9.5] {
                k.arc(12.0, 12.0, r, 140.0, 220.0);
                k.arc(12.0, 12.0, r, -40.0, 40.0);
            }
        }
        Watching | Eye => eye(k),
        EyeOff => {
            eye(k);
            k.slash();
        }
        Competing => {
            k.rrect(6.5, 3.5, 17.5, 7.5, 0.5);
            k.fill(&ellipse(12.0, 7.0, 5.5, 7.0, 0.0, 180.0));
            k.arc(6.5, 8.0, 3.0, 90.0, 270.0);
            k.arc(17.5, 8.0, 3.0, -90.0, 90.0);
            k.line(&[(12.0, 14.0), (12.0, 18.0)]);
            k.rrect(7.5, 18.0, 16.5, 21.0, 1.0);
        }
        Mic | MicOff => {
            k.rrect(8.5, 2.5, 15.5, 14.5, 3.5);
            k.arc(12.0, 11.0, 6.5, 0.0, 180.0);
            k.line(&[(12.0, 17.5), (12.0, 21.0)]);
            k.line(&[(8.5, 21.0), (15.5, 21.0)]);
            if icon == MicOff {
                k.slash();
            }
        }
        Headphones | Deafened => {
            k.arc(12.0, 13.0, 8.5, 180.0, 360.0);
            k.rrect(2.5, 12.5, 7.5, 21.0, 2.0);
            k.rrect(16.5, 12.5, 21.5, 21.0, 2.0);
            if icon == Deafened {
                k.slash();
            }
        }
        Camera | CameraOff => {
            k.rrect(2.0, 6.5, 15.5, 17.5, 2.5);
            k.fill(&[(16.5, 12.0), (22.0, 8.0), (22.0, 16.0)]);
            if icon == CameraOff {
                k.slash();
            }
        }
        Screen => {
            monitor(k);
            k.line(&[(12.0, 13.0), (12.0, 7.5)]);
            k.line(&[(9.0, 10.5), (12.0, 7.5), (15.0, 10.5)]);
        }
        Desktop => monitor(k),
        Hangup => handset(k, false),
        Phone => handset(k, true),
        Gear => {
            for i in 0..8 {
                let (s, c) = (i as f32 * 45.0).to_radians().sin_cos();
                let at = |r: f32, h: f32| (12.0 + c * r - s * h, 12.0 + s * r + c * h);
                k.fill(&[at(6.0, -2.4), at(10.0, -1.6), at(10.0, 1.6), at(6.0, 2.4)]);
            }
            k.width(4.25);
            k.ring(12.0, 12.0, 4.875);
        }
        Person => person(k, 12.0, 4.0, 7.5),
        PersonAdd | PersonRemove => {
            person(k, 9.5, 3.75, 7.0);
            k.line(&[(15.5, 9.0), (22.5, 9.0)]);
            if icon == PersonAdd {
                k.line(&[(19.0, 5.5), (19.0, 12.5)]);
            }
        }
        Members => {
            k.disc(17.0, 7.0, 3.0);
            k.fill(&ellipse(17.0, 19.5, 5.5, 6.0, 180.0, 360.0));
            k.cut(|k| {
                k.disc(9.0, 8.5, 5.0);
                k.fill(&ellipse(9.0, 20.5, 8.0, 8.0, 180.0, 360.0));
            });
            k.disc(9.0, 8.5, 3.5);
            k.fill(&ellipse(9.0, 20.5, 6.5, 6.5, 180.0, 360.0));
        }
        At => {
            k.ring(12.0, 12.0, 3.5);
            let mut pts = vec![(15.5, 8.5)];
            pts.extend(arc(18.0, 13.5, 2.5, 180.0, 0.0));
            pts.extend(arc(12.0, 12.0, 8.5, 0.0, -290.0));
            k.line(&pts);
        }
        IdCard => {
            k.rrect_line(2.5, 5.0, 21.5, 19.0, 2.5);
            k.disc(8.0, 12.0, 2.5);
            k.line(&[(12.5, 10.0), (18.0, 10.0)]);
            k.line(&[(12.5, 14.0), (16.0, 14.0)]);
        }
        Close => {
            k.line(&[(6.0, 6.0), (18.0, 18.0)]);
            k.line(&[(18.0, 6.0), (6.0, 18.0)]);
        }
        Plus => {
            k.line(&[(12.0, 5.0), (12.0, 19.0)]);
            k.line(&[(5.0, 12.0), (19.0, 12.0)]);
        }
        Minus => k.line(&[(5.0, 12.0), (19.0, 12.0)]),
        Check => k.line(&[(5.0, 12.5), (10.0, 17.0), (19.0, 7.0)]),
        Search => {
            k.ring(10.5, 10.5, 6.0);
            k.line(&[(15.0, 15.0), (20.0, 20.0)]);
        }
        Edit => {
            let (d, n) = ((H, -H), (H, H));
            let at = |along: f32, across: f32| (4.0 + d.0 * along + n.0 * across, 20.0 + d.1 * along + n.1 * across);
            k.closed(&[at(5.0, -2.5), at(19.0, -2.5), at(19.0, 2.5), at(5.0, 2.5)]);
            k.fill(&[at(0.0, 0.0), at(4.0, -2.5), at(4.0, 2.5)]);
            k.line(&[at(15.0, -2.5), at(15.0, 2.5)]);
        }
        Palette => {
            // Open ring with a thumb dip at the lower right, three paint dots.
            let mut pts = arc(12.0, 12.0, 9.0, 75.0, 375.0);
            pts.extend([(16.5, 14.5), (14.5, 16.5)]);
            k.closed(&pts);
            k.disc(7.75, 11.5, 1.5);
            k.disc(10.5, 7.25, 1.5);
            k.disc(15.25, 7.75, 1.5);
        }
        Accessibility => {
            k.ring(12.0, 12.0, 9.0);
            k.disc(12.0, 7.25, 1.6);
            k.line(&[(7.5, 10.0), (12.0, 11.0), (16.5, 10.0)]);
            k.line(&[(12.0, 11.0), (12.0, 14.0)]);
            k.line(&[(9.5, 18.0), (12.0, 14.0), (14.5, 18.0)]);
        }
        Keyboard => {
            k.rrect_line(2.5, 6.0, 21.5, 18.0, 2.5);
            for x in [6.5, 10.0, 13.5, 17.0] {
                k.disc(x, 10.0, 1.0);
            }
            k.line(&[(8.5, 14.5), (15.5, 14.5)]);
        }
        Puzzle => {
            // One piece: knobs on the top and right edges.
            let mut pts = vec![(4.5, 8.5), (8.48, 8.5)];
            pts.extend(arc(10.25, 6.75, 2.5, 135.0, 405.0));
            pts.extend([(16.5, 8.5), (16.5, 11.98)]);
            pts.extend(arc(18.25, 13.75, 2.5, 225.0, 495.0));
            pts.extend([(16.5, 19.5), (4.5, 19.5)]);
            k.closed(&pts);
        }
        Trash => {
            k.line(&[(3.5, 6.0), (20.5, 6.0)]);
            k.line(&[(9.0, 6.0), (9.5, 3.0), (14.5, 3.0), (15.0, 6.0)]);
            k.line(&[(5.75, 6.0), (7.0, 20.5), (17.0, 20.5), (18.25, 6.0)]);
            k.line(&[(10.0, 10.0), (10.0, 16.5)]);
            k.line(&[(14.0, 10.0), (14.0, 16.5)]);
        }
        Reply => {
            k.line(&[(9.0, 4.5), (3.5, 10.0), (9.0, 15.5)]);
            let mut pts = vec![(4.0, 10.0)];
            pts.extend(arc(13.0, 17.5, 7.5, -90.0, 0.0));
            k.line(&pts);
        }
        Forward => {
            k.line(&mirror(&[(9.0, 4.5), (3.5, 10.0), (9.0, 15.5)]));
            let mut pts = vec![(4.0, 10.0)];
            pts.extend(arc(13.0, 17.5, 7.5, -90.0, 0.0));
            k.line(&mirror(&pts));
        }
        Copy => {
            let mut back = vec![(6.5, 15.0)];
            back.extend(arc(5.5, 12.5, 2.5, 90.0, 180.0));
            back.extend(arc(5.5, 5.5, 2.5, 180.0, 270.0));
            back.extend(arc(12.5, 5.5, 2.5, 270.0, 360.0));
            back.push((15.0, 6.5));
            k.line(&back);
            k.rrect(9.0, 9.0, 21.0, 21.0, 2.5);
        }
        Link => {
            let (d, a) = ((H, -H), -45.0);
            for side in [-1.0, 1.0] {
                let c = (12.0 + d.0 * 4.0 * side, 12.0 + d.1 * 4.0 * side);
                let mut pts = arc(c.0 + d.0 * 2.5, c.1 + d.1 * 2.5, 3.5, a - 90.0, a + 90.0);
                pts.extend(arc(c.0 - d.0 * 2.5, c.1 - d.1 * 2.5, 3.5, a + 90.0, a + 270.0));
                k.closed(&pts);
            }
        }
        External => {
            k.line(&[(10.5, 4.5), (4.5, 4.5), (4.5, 19.5), (19.5, 19.5), (19.5, 13.5)]);
            k.line(&[(12.0, 12.0), (19.5, 4.5)]);
            k.line(&[(13.5, 4.5), (19.5, 4.5), (19.5, 10.5)]);
        }
        Leave => {
            k.line(&[(11.0, 4.0), (5.0, 4.0), (5.0, 20.0), (11.0, 20.0)]);
            k.line(&[(10.0, 12.0), (20.5, 12.0)]);
            k.line(&[(16.5, 8.0), (20.5, 12.0), (16.5, 16.0)]);
        }
        Block => {
            k.ring(12.0, 12.0, 9.0);
            k.line(&[(5.6, 5.6), (18.4, 18.4)]);
        }
        Shield => {
            let mut pts = vec![(12.0, 2.5), (20.0, 5.5)];
            pts.extend(quad((20.0, 11.0), (19.5, 18.5), (12.0, 21.5)));
            pts.extend(quad((12.0, 21.5), (4.5, 18.5), (4.0, 11.0)).into_iter().skip(1));
            pts.push((4.0, 5.5));
            k.closed(&pts);
        }
        Ban => {
            let (c, u, v) = ((15.0, 9.0), (H, H), (-H, H));
            let at = |a: f32, b: f32| (c.0 + u.0 * a + v.0 * b, c.1 + u.1 * a + v.1 * b);
            k.fill(&[at(6.0, 2.75), at(6.0, -2.75), at(-6.0, -2.75), at(-6.0, 2.75)]);
            k.width(2.5);
            k.line(&[c, (5.0, 19.0)]);
        }
        Refresh => circular(k, false),
        Reset => circular(k, true),
        Sort => {
            k.line(&[(8.0, 20.0), (8.0, 4.0)]);
            k.line(&[(4.0, 8.0), (8.0, 4.0), (12.0, 8.0)]);
            k.line(&[(16.0, 4.0), (16.0, 20.0)]);
            k.line(&[(12.0, 16.0), (16.0, 20.0), (20.0, 16.0)]);
        }
        Grip => {
            for x in [9.0, 15.0] {
                for y in [6.0, 12.0, 18.0] {
                    k.disc(x, y, 1.75);
                }
            }
        }
        More => {
            for x in [5.0, 12.0, 19.0] {
                k.disc(x, 12.0, 2.0);
            }
        }
        Inbox => {
            k.closed(&[(3.0, 13.0), (5.5, 4.5), (18.5, 4.5), (21.0, 13.0), (21.0, 19.5), (3.0, 19.5)]);
            k.line(&[(3.0, 13.0), (8.0, 13.0), (9.5, 15.75), (14.5, 15.75), (16.0, 13.0), (21.0, 13.0)]);
        }
        Pin => {
            let t = |pts: &[P]| turn(pts, 45.0, (-1.25, 1.25));
            k.fill(&t(&bubble(8.0, 2.5, 16.0, 5.5, 1.0, None)));
            k.fill(&t(&[(9.5, 5.0), (14.5, 5.0), (15.5, 13.0), (8.5, 13.0)]));
            k.fill(&t(&bubble(5.5, 12.5, 18.5, 15.0, 1.25, None)));
            k.line(&t(&[(12.0, 15.0), (12.0, 21.5)]));
        }
        Message => k.bubble_fill(2.5, 3.5, 21.5, 17.0, 4.0, false),
        Bell | BellOff => {
            k.disc(12.0, 10.0, 6.0);
            k.fill(&[(6.0, 10.0), (18.0, 10.0), (19.5, 16.5), (4.5, 16.5)]);
            k.rrect(3.0, 16.0, 21.0, 18.5, 1.25);
            k.fill(&ellipse(12.0, 18.25, 3.0, 3.0, 0.0, 180.0));
            k.disc(12.0, 3.75, 1.5);
            if icon == BellOff {
                k.slash();
            }
        }
        Archive => {
            k.rrect_line(3.0, 4.0, 21.0, 9.0, 1.5);
            k.line(&[(4.5, 9.0), (4.5, 20.0), (19.5, 20.0), (19.5, 9.0)]);
            k.line(&[(10.0, 13.0), (14.0, 13.0)]);
        }
        Envelope => {
            k.rrect_line(2.5, 5.0, 21.5, 19.0, 2.0);
            k.line(&[(3.5, 6.5), (12.0, 13.0), (20.5, 6.5)]);
        }
        SplitView => {
            k.rrect_line(3.0, 4.0, 21.0, 20.0, 2.5);
            k.line(&[(12.0, 4.0), (12.0, 20.0)]);
        }
        File => {
            k.closed(&[(5.0, 2.5), (14.0, 2.5), (19.0, 7.5), (19.0, 21.5), (5.0, 21.5)]);
            k.line(&[(14.0, 2.5), (14.0, 7.5), (19.0, 7.5)]);
        }
        Attach => {
            let mut pts = vec![(9.5, 8.0), (9.5, 16.0)];
            pts.extend(arc(12.0, 16.0, 2.5, 180.0, 0.0));
            pts.push((14.5, 5.0));
            pts.extend(arc(11.0, 5.0, 3.5, 0.0, -180.0));
            pts.push((7.5, 17.0));
            pts.extend(arc(12.0, 17.0, 4.5, 180.0, 0.0));
            pts.push((16.5, 9.0));
            k.line(&turn(&pts, 45.0, (0.0, 0.5)));
        }
        Hourglass => {
            k.line(&[(6.0, 3.0), (18.0, 3.0)]);
            k.line(&[(6.0, 21.0), (18.0, 21.0)]);
            let side = [(7.5, 3.0), (7.5, 6.5), (12.0, 12.0), (7.5, 17.5), (7.5, 21.0)];
            k.line(&side);
            k.line(&mirror(&side));
            k.fill(&[(9.25, 19.75), (14.75, 19.75), (12.0, 16.25)]);
        }
        Key => {
            k.ring(7.5, 16.5, 4.0);
            k.line(&[(10.5, 13.5), (20.0, 4.0)]);
            k.line(&[(17.0, 7.0), (19.5, 9.5)]);
            k.line(&[(14.5, 9.5), (16.5, 11.5)]);
        }
        Card => {
            k.rrect_line(2.5, 5.0, 21.5, 19.0, 2.5);
            k.line(&[(2.5, 9.5), (21.5, 9.5)]);
            k.line(&[(6.0, 15.0), (10.0, 15.0)]);
        }
        Video => {
            k.rrect_line(2.5, 4.5, 21.5, 19.5, 2.5);
            k.fill(&[(10.0, 8.5), (16.0, 12.0), (10.0, 15.5)]);
        }
        Lock => {
            k.rrect(5.0, 10.5, 19.0, 21.0, 2.0);
            let mut pts = vec![(7.5, 10.5)];
            pts.extend(arc(12.0, 8.0, 4.5, 180.0, 360.0));
            pts.push((16.5, 10.5));
            k.line(&pts);
            if !k.small {
                k.cut(|k| k.disc(12.0, 15.75, 1.6));
            }
        }
        Unlock => {
            k.rrect(5.0, 10.5, 19.0, 21.0, 2.0);
            // The shackle swung up: its left end hangs free over the body.
            let mut pts = vec![(16.5, 10.5)];
            pts.extend(arc(12.0, 6.0, 4.5, 0.0, -180.0));
            k.line(&pts);
        }
        Boost => {
            k.closed(&[(8.0, 4.0), (16.0, 4.0), (20.5, 9.0), (12.0, 20.5), (3.5, 9.0)]);
            k.line(&[(3.5, 9.0), (20.5, 9.0)]);
        }
        Clock => {
            k.ring(12.0, 12.0, 9.0);
            k.line(&[(12.0, 6.5), (12.0, 12.0), (16.0, 14.0)]);
        }
        Star => {
            let pt = |i: usize| {
                let (s, c) = (-90.0 + 36.0 * i as f32).to_radians().sin_cos();
                let r = if i.is_multiple_of(2) { 10.0 } else { 4.2 };
                (12.0 + r * c, 12.75 + r * s)
            };
            k.fill(&(0..5).map(|i| pt(2 * i + 1)).collect::<Vec<_>>());
            // Each point runs one unit past its base along its own edges: two anti-aliased
            // edges meeting exactly would show as light seams inside the star.
            let past = |inner: P, tip: P| {
                let (dx, dy) = (inner.0 - tip.0, inner.1 - tip.1);
                let len = dx.hypot(dy);
                (inner.0 + dx / len, inner.1 + dy / len)
            };
            for i in 0..5 {
                let tip = pt(2 * i);
                k.fill(&[past(pt((2 * i + 9) % 10), tip), tip, past(pt(2 * i + 1), tip)]);
            }
        }
        Globe => {
            k.ring(12.0, 12.0, 9.0);
            k.closed(&ellipse(12.0, 12.0, 4.0, 9.0, 0.0, 360.0));
            k.line(&[(3.5, 12.0), (20.5, 12.0)]);
        }
        Info => {
            k.ring(12.0, 12.0, 9.0);
            k.disc(12.0, 7.75, 1.4);
            k.line(&[(12.0, 11.0), (12.0, 16.5)]);
        }
        Poll => {
            k.rrect(3.5, 4.0, 20.5, 8.0, 2.0);
            k.rrect(3.5, 10.0, 14.5, 14.0, 2.0);
            k.rrect(3.5, 16.0, 17.5, 20.0, 2.0);
        }
        Verified => {
            seal(k);
            k.cut(|k| k.line(&[(8.0, 12.25), (10.75, 15.0), (16.25, 9.25)]));
        }
        Partnered => {
            seal(k);
            k.cut(|k| {
                if k.small {
                    k.ring(12.0, 12.0, 3.75);
                } else {
                    k.width(1.75);
                    k.ring(10.25, 12.0, 2.75);
                    k.ring(13.75, 12.0, 2.75);
                }
            });
        }
        BoostTier1 => {
            k.fill(&[(7.5, 4.0), (16.5, 4.0), (20.5, 9.0), (12.0, 20.5), (3.5, 9.0)]);
            k.cut(|k| {
                k.width(1.5);
                k.line(&[(3.5, 9.0), (20.5, 9.0)]);
            });
        }
        BoostTier2 => {
            k.ring(12.0, 12.0, 9.0);
            k.fill(&SMALL_GEM);
        }
        BoostTier3 => {
            k.disc(12.0, 12.0, 10.0);
            k.cut(|k| k.fill(&SMALL_GEM));
        }
        Play => k.fill(&[(7.0, 4.5), (19.5, 12.0), (7.0, 19.5)]),
        Pause => {
            k.rrect(6.0, 4.5, 10.0, 19.5, 1.0);
            k.rrect(14.0, 4.5, 18.0, 19.5, 1.0);
        }
        Download => {
            k.line(&[(12.0, 3.5), (12.0, 15.0)]);
            k.line(&[(7.5, 10.5), (12.0, 15.0), (16.5, 10.5)]);
            k.line(&[(4.0, 16.0), (4.0, 20.0), (20.0, 20.0), (20.0, 16.0)]);
        }
        Expand => {
            k.line(&[(3.5, 9.0), (3.5, 3.5), (9.0, 3.5)]);
            k.line(&[(15.0, 3.5), (20.5, 3.5), (20.5, 9.0)]);
            k.line(&[(20.5, 15.0), (20.5, 20.5), (15.0, 20.5)]);
            k.line(&[(9.0, 20.5), (3.5, 20.5), (3.5, 15.0)]);
        }
        Collapse => {
            k.line(&[(9.0, 3.5), (9.0, 9.0), (3.5, 9.0)]);
            k.line(&[(15.0, 3.5), (15.0, 9.0), (20.5, 9.0)]);
            k.line(&[(20.5, 15.0), (15.0, 15.0), (15.0, 20.5)]);
            k.line(&[(3.5, 15.0), (9.0, 15.0), (9.0, 20.5)]);
        }
        ArrowLeft => {
            k.line(&[(4.0, 12.0), (20.0, 12.0)]);
            k.line(&[(10.0, 6.0), (4.0, 12.0), (10.0, 18.0)]);
        }
        ArrowRight => {
            k.line(&[(4.0, 12.0), (20.0, 12.0)]);
            k.line(&[(14.0, 6.0), (20.0, 12.0), (14.0, 18.0)]);
        }
        ChevronUp => k.line(&[(6.0, 15.0), (12.0, 9.0), (18.0, 15.0)]),
        ChevronDown => k.line(&[(6.0, 9.0), (12.0, 15.0), (18.0, 9.0)]),
        ChevronLeft => k.line(&[(15.0, 6.0), (9.0, 12.0), (15.0, 18.0)]),
        ChevronRight => k.line(&[(9.0, 6.0), (15.0, 12.0), (9.0, 18.0)]),
        Smile => {
            k.ring(12.0, 12.0, 9.0);
            k.disc(8.75, 9.5, 1.5);
            k.disc(15.25, 9.5, 1.5);
            k.arc(12.0, 12.0, 5.25, 30.0, 150.0);
        }
        Leaf => {
            let mut pts = quad((5.0, 19.0), (4.0, 4.0), (19.0, 5.0));
            pts.extend(quad((19.0, 5.0), (20.0, 20.0), (5.0, 19.0)).into_iter().skip(1).take(11));
            k.closed(&pts);
            k.line(&[(3.0, 21.0), (14.0, 10.0)]);
        }
        Food => {
            k.disc(9.0, 13.5, 6.0);
            k.disc(15.0, 13.5, 6.0);
            k.line(&[(12.0, 8.5), (13.0, 4.0)]);
            let leaf: Vec<P> = ellipse(0.0, 0.0, 2.25, 1.25, 0.0, 360.0).into_iter().map(|(a, b)| {
                let (s, c) = (-30f32).to_radians().sin_cos();
                (16.0 + a * c - b * s, 4.75 + a * s + b * c)
            }).collect();
            k.fill(&leaf);
        }
        Ball => {
            k.ring(12.0, 12.0, 9.0);
            let at = |i: usize, r: f32| {
                let (s, c) = (-90.0 + 72.0 * i as f32).to_radians().sin_cos();
                (12.0 + r * c, 12.0 + r * s)
            };
            k.fill(&(0..5).map(|i| at(i, 3.75)).collect::<Vec<_>>());
            for i in 0..5 {
                k.line(&[at(i, 3.75), at(i, 8.5)]);
            }
        }
        Car => {
            k.rrect(2.5, 10.0, 21.5, 16.5, 2.5);
            k.line(&[(6.5, 10.5), (8.5, 5.5), (15.5, 5.5), (17.5, 10.5)]);
            k.disc(7.0, 17.5, 2.5);
            k.disc(17.0, 17.5, 2.5);
        }
        Bulb => {
            k.disc(12.0, 9.5, 6.5);
            k.fill(&[(8.0, 13.0), (16.0, 13.0), (14.5, 17.0), (9.5, 17.0)]);
            k.line(&[(9.5, 19.0), (14.5, 19.0)]);
            k.line(&[(10.5, 21.5), (13.5, 21.5)]);
        }
        Heart => {
            let (r, tip, l, rr) = (4.75, (12.0, 20.5), (7.5, 9.0), (16.5, 9.0));
            // The point where a line from the tip touches the lobe at `c` (outer side).
            let touch = |c: P, sign: f32| {
                let (dx, dy) = (tip.0 - c.0, tip.1 - c.1);
                let a = dy.atan2(dx) + sign * (r / dx.hypot(dy)).acos();
                (c.0 + r * a.cos(), c.1 + r * a.sin())
            };
            k.disc(l.0, l.1, r);
            k.disc(rr.0, rr.1, r);
            k.fill(&[touch(l, 1.0), l, rr, touch(rr, -1.0), tip]);
        }
        Flag => {
            k.line(&[(5.0, 3.0), (5.0, 21.0)]);
            k.fill(&[(5.0, 4.0), (19.0, 4.0), (15.73, 10.0), (5.0, 10.0)]);
            k.fill(&[(5.0, 9.0), (15.73, 9.0), (19.0, 15.0), (5.0, 15.0)]);
        }
        // A medal: two crossing ribbons, a 1.5-unit gap, the disc.
        Quest => {
            k.fill(&[(6.5, 2.5), (10.5, 2.5), (13.0, 10.5), (9.5, 11.5)]);
            k.fill(&[(14.5, 11.5), (11.0, 10.5), (13.5, 2.5), (17.5, 2.5)]);
            k.cut(|k| k.disc(12.0, 15.5, 7.0));
            k.disc(12.0, 15.5, 5.5);
        }
        // A coin-like orb with a crescent highlight.
        Orb => {
            k.disc(12.0, 12.0, 8.5);
            k.cut(|k| k.disc(9.0, 9.0, 3.0));
            k.disc(10.25, 10.25, 3.0);
        }
        Gift => {
            k.rrect(3.5, 11.0, 20.5, 21.0, 1.5);
            k.rrect(2.5, 7.5, 21.5, 11.0, 1.0);
            k.cut(|k| k.fill(&[(11.25, 7.5), (12.75, 7.5), (12.75, 21.0), (11.25, 21.0)]));
            k.width(1.75);
            k.ring(9.25, 5.0, 2.75);
            k.ring(14.75, 5.0, 2.75);
        }
    }
}

/// Test hook `RUSTCORD_NAV_DEMO=icons`: every icon at 16, 20, 24 and 48 px and the channel
/// matrix (plain, private, age-restricted) at 20 px, over the whole window.
pub(crate) fn sheet(ctx: &egui::Context, color: Color32, bg: Color32) {
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("rc_icon_sheet")));
    let screen = ctx.content_rect();
    p.rect_filled(screen, 0, bg);
    let mut y = screen.top() + 8.0;
    for side in [16.0, 20.0, 24.0, 48.0] {
        let step = side + 10.0;
        let mut x = screen.left() + 8.0;
        for icon in Icon::ALL {
            if x + side > screen.right() - 8.0 {
                (x, y) = (screen.left() + 8.0, y + step);
            }
            paint(&p, icon, Rect::from_min_size(pos2(x, y), egui::Vec2::splat(side)), color, bg);
            x += step;
        }
        y += step + 6.0;
    }
    let mut x = screen.left() + 8.0;
    for kind in [ct::GUILD_TEXT, ct::GUILD_VOICE, ct::GUILD_ANNOUNCEMENT, ct::GUILD_STAGE_VOICE, ct::GUILD_FORUM, ct::GUILD_MEDIA, ct::PUBLIC_THREAD, ct::PRIVATE_THREAD, ct::GUILD_DIRECTORY] {
        for (locked, nsfw, rules) in [(false, false, false), (true, false, false), (false, true, false), (false, false, true)] {
            paint_channel(&p, channel(kind, locked, nsfw, rules), Rect::from_min_size(pos2(x, y), egui::Vec2::splat(20.0)), color, bg);
            x += 28.0;
        }
        x += 12.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(shapes: &[Shape]) -> Rect {
        shapes.iter().map(Shape::visual_bounding_rect).fold(Rect::NOTHING, |a, b| a.union(b))
    }

    #[test]
    fn every_icon_paints_inside_its_rect() {
        for side in [16.0, 20.0, 24.0, 48.0] {
            for ppp in [1.0, 1.5, 2.0] {
                let rect = Rect::from_min_size(pos2(10.0, 20.0), egui::Vec2::splat(side));
                for icon in Icon::ALL {
                    let s = shapes(icon, None, rect, Color32::WHITE, Color32::BLACK, ppp);
                    assert!(!s.is_empty(), "{icon:?} paints nothing");
                    let b = bounds(&s);
                    assert!(b.area() > side * side * 0.05, "{icon:?} at {side} px is too small: {b:?}");
                    // Strokes and feathering may reach 1 unit past the 24-unit box.
                    assert!(rect.expand(side / 24.0 + 1.0).contains_rect(b), "{icon:?} at {side} px leaves its rect: {b:?}");
                }
            }
        }
    }

    #[test]
    fn tessellated_icons_have_no_spikes() {
        use egui::epaint::{Mesh, TessellationOptions, Tessellator};
        for side in [16.0, 17.0, 18.0, 20.0, 24.0] {
            for ppp in [1.0, 1.25, 1.5] {
                let rect = Rect::from_min_size(pos2(10.3, 20.7), egui::Vec2::splat(side));
                for icon in Icon::ALL {
                    let mut t = Tessellator::new(ppp, TessellationOptions::default(), [1, 1], vec![]);
                    let mut mesh = Mesh::default();
                    for s in shapes(icon, (icon == Icon::Voice).then_some(Badge::Lock), rect, Color32::WHITE, Color32::BLACK, ppp) {
                        t.tessellate_shape(s, &mut mesh);
                    }
                    let b = mesh.calc_bounds();
                    assert!(rect.expand(side / 24.0 + 2.0).contains_rect(b), "{icon:?} at {side} px, {ppp} ppp spikes out: {b:?} vs {rect:?}");
                }
            }
        }
    }

    #[test]
    fn badges_stay_near_the_icon() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), egui::Vec2::splat(20.0));
        for badge in [Badge::Lock, Badge::Alert] {
            let s = shapes(Icon::Text, Some(badge), rect, Color32::WHITE, Color32::BLACK, 1.0);
            assert!(rect.expand(3.0).contains_rect(bounds(&s)), "{badge:?}");
        }
        // Nothing is cut on a transparent background.
        let plain = shapes(Icon::MicOff, None, rect, Color32::WHITE, Color32::TRANSPARENT, 1.0);
        assert!(plain.iter().all(|s| !matches!(s, Shape::Circle(c) if c.fill == Color32::TRANSPARENT)));
        assert!(shapes(Icon::MicOff, None, rect, Color32::WHITE, Color32::BLACK, 1.0).len() > plain.len());
    }

    #[test]
    fn channel_kinds_map_to_icons() {
        use Badge::{Alert, Lock};
        assert_eq!(channel(ct::GUILD_TEXT, false, false, false), (Icon::Text, None));
        assert_eq!(channel(ct::GUILD_TEXT, true, true, false), (Icon::Text, Some(Lock)));
        assert_eq!(channel(ct::GUILD_TEXT, false, true, false), (Icon::Text, Some(Alert)));
        assert_eq!(channel(ct::GUILD_TEXT, false, true, true), (Icon::Rules, None));
        assert_eq!(channel(ct::GUILD_VOICE, true, false, false), (Icon::Voice, Some(Lock)));
        assert_eq!(channel(ct::GUILD_VOICE, false, true, false), (Icon::Voice, Some(Alert)));
        assert_eq!(channel(ct::GUILD_ANNOUNCEMENT, false, false, false).0, Icon::Announcement);
        assert_eq!(channel(ct::GUILD_STAGE_VOICE, false, true, false), (Icon::Stage, None));
        assert_eq!(channel(ct::GUILD_FORUM, false, false, false).0, Icon::Forum);
        assert_eq!(channel(ct::GUILD_MEDIA, false, true, false), (Icon::Media, Some(Alert)));
        assert_eq!(channel(ct::PUBLIC_THREAD, false, false, false), (Icon::Thread, None));
        assert_eq!(channel(ct::PRIVATE_THREAD, false, false, false), (Icon::Thread, Some(Lock)));
        assert_eq!(channel(ct::GUILD_DIRECTORY, true, false, false), (Icon::Directory, None));
        assert_eq!(channel(ct::DM, false, false, false), (Icon::At, None));
        assert_eq!(channel(ct::GROUP_DM, false, false, false), (Icon::Members, None));
        assert_eq!(channel(99, false, false, false), (Icon::Text, None));
    }

    #[test]
    fn private_means_everyone_cannot_view() {
        use crate::model::{Guild, Id, Overwrite, Perms};
        let deny = |id: u64, bits: u64| Overwrite { id: Id(id), kind: 0, allow: Perms(0), deny: Perms(bits) };
        let mut s = crate::state::AppState::default();
        let mut ch = Channel { id: Id(3), guild_id: Some(Id(7)), ..Default::default() };
        assert_eq!(of(&ch, &s), (Icon::Text, None));
        ch.permission_overwrites = vec![deny(8, perm::VIEW_CHANNEL)];
        assert_eq!(of(&ch, &s).1, None, "a role's deny doesn't make it private");
        ch.permission_overwrites.push(deny(7, perm::VIEW_CHANNEL));
        assert_eq!(of(&ch, &s).1, Some(Badge::Lock));
        s.guilds.push(Guild { id: Id(7), rules_channel_id: Some(Id(3)), ..Default::default() });
        assert_eq!(of(&ch, &s), (Icon::Rules, Some(Badge::Lock)));
    }

    #[test]
    fn glyphs_map_to_icons() {
        assert_eq!(from_glyph("⚙"), Some(Icon::Gear));
        assert_eq!(from_glyph("✏️"), Some(Icon::Edit));
        assert_eq!(from_glyph("🔊"), Some(Icon::Voice));
        assert_eq!(from_glyph("Aa"), None);
        assert_eq!(from_glyph(""), None);
        assert_eq!(activity(2), Icon::Listening);
        assert_eq!(activity(4), Icon::Playing);
        assert_eq!(Icon::Mic.off(), Icon::MicOff);
        let f = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(guild_mark(&f(&["PARTNERED", "VERIFIED"]), 3).map(|m| m.0), Some(Icon::Verified), "verified first");
        assert_eq!(guild_mark(&f(&["PARTNERED"]), 3), Some((Icon::Partnered, "Partnered server")));
        assert_eq!(guild_mark(&f(&["COMMUNITY"]), 2), Some((Icon::BoostTier2, "Boost level 2")));
        assert_eq!(guild_mark(&[], 1).map(|m| m.0), Some(Icon::BoostTier1));
        assert_eq!(guild_mark(&[], 0), None);
        assert_eq!(guild_mark(&[], 4), None, "no such level");
        // `ALL` lists every variant in order (`Puzzle` stays last).
        assert_eq!(Icon::ALL.len(), Icon::Puzzle as usize + 1);
        assert!(Icon::ALL.iter().enumerate().all(|(i, x)| *x as usize == i));
    }
}
