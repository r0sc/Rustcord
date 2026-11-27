//! The Quests page (DM home, the row under Friends), its cards, and the quest video and claim
//! modals (docs/QUESTS_NOTES.md §4). Accept, claim and "Show code" go out on a click only; video
//! progress only while the quest video the user started really plays in Rustcord's player
//! (`crate::quests::VideoProgress`). That session outlives its modal: it plays on in the
//! background (a "Quest video" section in the user panel, `logic` ticks it while minimized too)
//! until it ends, the quest is done or the user stops it. No polling: the page fetches when it
//! opens (at most every 10 min); gateway events keep it current.
//!
//! OWNERSHIP: quests engineer.

use super::icons::{self, Icon};
use super::messages::{Btn, Pal, button, focus_ring, paint_radio};
use super::{Modal, UiCx, ViewState, semibold};
use crate::backend::{Command, CommandSender, ConnState};
use crate::media::{Fail, State};
use crate::model::Id;
use crate::quests::{Load, Phase, Quest, QuestCommand, TaskKind, VIDEO_OFFLINE, VideoProgress};
use chrono::{DateTime, Datelike, Utc};
use egui::{Align, Color32, CornerRadius, FontId, Layout, Margin, Rect, RichText, Sense, Shape, Vec2, pos2, vec2};
use std::time::{Duration, Instant};

/// Module UI state (`ViewState::quests_ui`).
#[derive(Default)]
pub struct QuestsUi {
    /// The DM home shows the Quests page instead of Friends.
    pub open: bool,
    /// The one quest video session (its modal may be closed: it plays on in the background).
    watch: Option<Watch>,
    /// The claim modal's pick.
    claim_pick: Option<u8>,
    /// The last desktop notification shown was a quest's: a click on it opens the Quests page.
    pub notified_last: bool,
}

/// One quest video session, started by the user; dropped by `stop_watch`, which stops the player.
struct Watch {
    quest_id: Id,
    progress: VideoProgress,
    /// The server's value when it opened (s): the player's `played_ms` counts on from here.
    start: u32,
    /// The in-app player (`crate::media`); None in the demo, or after it freed itself.
    player: Option<crate::media::Player>,
    /// Its frame: (generation, texture).
    texture: Option<(u64, egui::TextureHandle)>,
    muted: bool,
    /// Test hook: a still frame at the server's value, no player, nothing sent.
    demo: bool,
    /// Test hook `watch-play`: plays the fixture clip from the disk cache, muted (still `demo`).
    demo_play: bool,
    /// The progress position (s): the picture's, never past what really played (`logic`).
    pos: f64,
    /// Playing and not buffering.
    playing: bool,
    /// The user paused it in the modal.
    paused: bool,
}

impl Watch {
    fn new(q: &Quest, player: Option<crate::media::Player>) -> Watch {
        let (start, target) = (q.video_value(), q.task().map_or(0, |(_, t)| t.target));
        Watch { quest_id: q.id, progress: VideoProgress::new(start, target), start, demo: player.is_none(), player, texture: None, muted: false, demo_play: false, pos: f64::from(start), playing: false, paused: false }
    }
}

/// A click on a card.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Click {
    Accept,
    Watch,
    Claim,
    ShowCode,
    Copy,
}

// ----- page ---------------------------------------------------------------------------------------

/// The page in the chat's place: header with the Orbs balance, then the sections.
pub fn show(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    let pal = Pal::new(cx.theme);
    let online = cx.state.conn == ConnState::Connected && !crate::config::offline_mode();
    if online && cx.state.quests.should_fetch(Instant::now()) {
        cx.state.quests.load = Load::Loading;
        cx.cmd.send(Command::Quests(QuestCommand::Fetch));
    }
    header(ui, cx, &pal);
    egui::ScrollArea::vertical().id_salt("rc_quests").auto_shrink(false).show(ui, |ui| {
        egui::Frame::new().inner_margin(Margin { left: 16, right: 16, top: 24, bottom: 24 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            body(ui, cx, &pal, online);
        });
    });
}

fn header(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal) {
    let orbs = cx.state.quests.orbs;
    let header = egui::Panel::top("rc_quests_header")
        .exact_size(49.0)
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::new().fill(pal.bg).inner_margin(Margin { left: 16, right: 16, top: 0, bottom: 0 }))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                super::messages::vector_icon(ui, Icon::Quest, 20.0, pal.muted, pal.bg);
                ui.label(RichText::new("Quests").font(semibold(pal.heading())).color(pal.text));
                if let Some(n) = orbs {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let font = FontId::new(pal.ui(), crate::theme::medium_family());
                        let text = ui.label(RichText::new(format!("{n} Orbs")).font(font).color(pal.text));
                        let icon = super::messages::vector_icon(ui, Icon::Orb, 16.0, pal.text, pal.bg);
                        text.union(icon).on_hover_text("Your Orbs balance");
                    });
                }
            });
        });
    let r = header.response.rect;
    ui.painter().hline(r.x_range(), r.bottom() - 0.5, egui::Stroke::new(1.0, pal.tk.rule));
}

fn body(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, online: bool) {
    match &cx.state.quests.load {
        Load::Loaded(_) => {}
        // Skeletons for the first fetch only; a refetch keeps the cards.
        Load::Loading if !cx.state.quests.quests.is_empty() => {}
        Load::Loading => {
            super::motion::stagger(ui.ctx(), stagger_id(), false, 0);
            return skeletons(ui, pal);
        }
        Load::Failed(e) => {
            let e = e.clone();
            if empty_state(ui, pal, "Couldn't load quests", &e, Some("Retry")) {
                // The next pass fetches (when connected).
                cx.state.quests.load = Load::NotAsked;
                ui.ctx().request_repaint();
            }
            return;
        }
        Load::NotAsked => {
            if !online {
                empty_state(ui, pal, "Quests load when you're connected.", "Rustcord fetches them when you open this page.", None);
            }
            return;
        }
    }
    let now = Utc::now();
    if let Some(until) = cx.state.quests.blocked_until.as_deref().and_then(crate::quests::parse_time).filter(|t| *t > now) {
        banner(ui, pal, &format!("You can accept new quests again on {}.", day(until, now)));
        ui.add_space(24.0);
    }
    let sections: Vec<(Phase, Vec<Id>)> = cx.state.quests.sections(now).into_iter().map(|(p, qs)| (p, qs.iter().map(|q| q.id).collect())).collect();
    if sections.is_empty() {
        empty_state(ui, pal, "No quests right now", "New quests show up here when Discord offers them.", None);
        return;
    }
    let (n, cw) = columns(ui.available_width());
    let mut k = 0;
    for (i, (phase, ids)) in sections.iter().enumerate() {
        if i > 0 {
            ui.add_space(8.0);
        }
        section_header(ui, pal, section_title(*phase), ids.len());
        for (row_i, row) in ids.chunks(n).enumerate() {
            // Cards of a row share the tallest one's height (last pass; settles in one frame).
            let key = ui.make_persistent_id(("rc_quest_row", *phase as u8, row_i));
            let min_h = ui.ctx().data(|d| d.get_temp::<f32>(key)).unwrap_or(0.0);
            let mut tallest = 0.0f32;
            let mut clicks = Vec::new();
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                for &id in row {
                    let Some(q) = cx.state.quests.quest(id).cloned() else { continue };
                    // After the skeletons the cards fade in and rise 8 px, one after another.
                    let t = super::motion::stagger(ui.ctx(), stagger_id(), true, k);
                    k += 1;
                    let (h, click) = ui
                        .scope(|ui| {
                            ui.multiply_opacity(t);
                            ui.with_visual_transform(egui::emath::TSTransform::from_translation(egui::vec2(0.0, 8.0 * (1.0 - t))), |ui| card(ui, cx, pal, &q, *phase, cw, min_h, now)).inner
                        })
                        .inner;
                    tallest = tallest.max(h);
                    clicks.extend(click.map(|c| (q, c)));
                }
            });
            if (tallest - min_h).abs() > 0.5 {
                ui.ctx().data_mut(|d| d.insert_temp(key, tallest));
                ui.ctx().request_repaint();
            }
            ui.add_space(16.0);
            for (q, c) in clicks {
                on_click(ui.ctx(), cx, &q, c);
            }
        }
    }
    ui.add_space(8.0);
    ui.label(RichText::new("Rustcord counts quest progress only while you watch a quest video here.").size(pal.meta()).color(pal.muted));
    ui.add_space(4.0);
    let link = ui.add(egui::Label::new(RichText::new("Quest privacy settings").size(pal.meta()).color(pal.link)).sense(Sense::click()));
    if link.has_focus() {
        focus_ring(ui, pal, link.rect, 2.0);
    }
    link.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, "Quest privacy settings"));
    if link.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        // Settings > Data & privacy holds "Use third-party data for Quests".
        (cx.view.settings_open, cx.view.settings_page) = (true, super::user_settings::Page::Privacy);
    }
}

/// (columns, column width) for a content area `w` wide: the most columns (up to 3) that are at
/// least 300 wide, each at most 420.
fn columns(w: f32) -> (usize, f32) {
    (1..=3usize)
        .rev()
        .map(|n| (n, (w - 16.0 * (n as f32 - 1.0)) / n as f32))
        .find(|&(_, cw)| cw >= 300.0)
        .map_or((1, w.max(1.0)), |(n, cw)| (n, cw.min(420.0)))
}

fn section_title(p: Phase) -> &'static str {
    match p {
        Phase::Claimable => "Ready to claim",
        Phase::InProgress => "In progress",
        Phase::Available => "Available",
        Phase::Claimed | Phase::Hidden => "Claimed",
    }
}

fn section_header(ui: &mut egui::Ui, pal: &Pal, title: &str, count: usize) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
    let p = ui.painter();
    p.text(rect.left_center(), egui::Align2::LEFT_CENTER, title, semibold(pal.label()), pal.muted);
    p.text(rect.right_center(), egui::Align2::RIGHT_CENTER, count.to_string(), FontId::proportional(pal.meta()), pal.muted);
    ui.add_space(12.0);
}

/// The enrolment block: `surface`, a Clock in `attention`, the date in words.
fn banner(ui: &mut egui::Ui, pal: &Pal, text: &str) {
    egui::Frame::new().fill(pal.surface).corner_radius(pal.radius).inner_margin(Margin::same(12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        icon_text(ui, Icon::Clock, pal.tk.attention, text, FontId::proportional(pal.ui()), pal.text, 2, pal.surface);
    });
}

/// §9 empty state at 30 % of the page: tile 64 with rings, heading, one meta line, at most one button.
/// True when the button was clicked.
fn empty_state(ui: &mut egui::Ui, pal: &Pal, title: &str, meta: &str, action: Option<&str>) -> bool {
    ui.add_space((ui.available_height() * 0.3 - 80.0).max(24.0));
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        let rings = ui.painter().add(egui::Shape::Noop);
        let tile = super::messages::hero_tile(ui, pal, (Icon::Quest, None));
        ui.painter().set(rings, super::messages::hero_rings(tile.center(), pal.tk.rule));
        ui.add_space(8.0);
        ui.label(RichText::new(title).font(semibold(pal.heading())).color(pal.text));
        ui.label(RichText::new(meta).size(pal.meta()).color(pal.muted));
        action.is_some_and(|a| {
            ui.add_space(8.0);
            button(ui, pal, Btn::Secondary, a, true).clicked()
        })
    })
    .inner
}

/// First fetch: three card outlines with the hero area in `hover`, no text, no spinner.
fn stagger_id() -> egui::Id {
    egui::Id::unique("rc_quest_stagger")
}

fn skeletons(ui: &mut egui::Ui, pal: &Pal) {
    let (n, cw) = columns(ui.available_width());
    let hero_h = (cw * 9.0 / 16.0).round();
    let r = pal.card_radius();
    ui.add_space(36.0);
    for row in [0, 1, 2].chunks(n) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            for _ in row {
                let (rect, _) = ui.allocate_exact_size(vec2(cw, hero_h + 180.0), Sense::hover());
                let p = ui.painter();
                p.rect_filled(rect, r, pal.surface);
                p.rect_filled(Rect::from_min_size(rect.min, vec2(cw, hero_h)), CornerRadius { nw: r, ne: r, sw: 0, se: 0 }, pal.tk.hover);
                for (y, w) in [(28.0, 0.6), (56.0, 0.4), (96.0, 0.5)] {
                    p.rect_filled(Rect::from_min_size(pos2(rect.left() + 16.0, rect.top() + hero_h + y), vec2((cw - 32.0) * w, 12.0)), 6.0, pal.tk.hover);
                }
            }
        });
        ui.add_space(16.0);
    }
}

// ----- card -----------------------------------------------------------------------------------------

/// What the card's task line says and whether Rustcord can do it (QUESTS_NOTES §4.3).
fn task_line(q: &Quest, playable: bool) -> (Icon, String, Option<&'static str>) {
    let game = if q.config.messages.game_title.is_empty() { q.config.application.name.as_str() } else { q.config.messages.game_title.as_str() };
    match q.task() {
        Some((TaskKind::Video, t)) if playable => (Icon::Video, format!("Watch the video for {}.", duration(t.target)), None),
        Some((TaskKind::Video, _)) => (Icon::Video, "Watch this video in the Discord app.".into(), None),
        Some((TaskKind::Desktop, t)) => (
            Icon::Desktop,
            format!("Play {game} for {} in the Discord desktop app.", duration(t.target)),
            Some("Rustcord can't track play time for quests. Progress from the desktop app shows here."),
        ),
        Some((TaskKind::Stream, t)) => (Icon::Screen, format!("Stream {game} to a friend for {} in the Discord desktop app.", duration(t.target)), None),
        Some((TaskKind::Activity, t)) => (Icon::Playing, format!("Play the activity for {} in the Discord app.", duration(t.target)), None),
        Some((TaskKind::Mobile, _)) => (Icon::Phone, "Watch the video in the Discord mobile app.".into(), None),
        Some((TaskKind::Console, _)) => (Icon::Playing, format!("Play {game} on Xbox or PlayStation. Start it in the Discord app."), None),
        Some((TaskKind::Achievement, t)) => {
            let text = t.messages.task_description.clone().filter(|d| !d.trim().is_empty()).unwrap_or_else(|| format!("Reach the goal in {game}."));
            (Icon::Star, text, None)
        }
        Some((TaskKind::Other, _)) | None => (Icon::Info, "Open this quest in the Discord app.".into(), None),
    }
}

/// Whether this quest's video can play here (an MP4 the player has not failed on).
fn playable(cx: &UiCx<'_>, q: &Quest) -> bool {
    !cx.state.quests.no_video.contains(&q.id) && q.video_url().is_some()
}

/// One quest card `w` wide; `min_h` = its row's tallest card in the last pass (the button sits
/// at the bottom of that height). Returns its natural height and the click, if any.
#[allow(clippy::too_many_arguments, reason = "one card's inputs; a struct would only rename them")]
fn card(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, q: &Quest, phase: Phase, w: f32, min_h: f32, now: DateTime<Utc>) -> (f32, Option<Click>) {
    let bg = ui.painter().add(Shape::Noop);
    let radius = pal.card_radius();
    let busy = cx.state.quests.busy.contains(&q.id);
    let can_play = playable(cx, q);
    // A finished video quest just says what it was.
    let (icon, line, extra) = task_line(q, can_play || q.completed());
    let kind = q.task().map(|(k, _)| k);
    let blocked = cx.state.quests.blocked_until.as_deref().and_then(crate::quests::parse_time).filter(|t| *t > now);
    let mut click = None;
    let mut natural = 0.0;
    let out = ui.allocate_ui_with_layout(vec2(w, 0.0), Layout::top_down(Align::Min), |ui| {
        ui.set_width(w);
        ui.spacing_mut().item_spacing.y = 0.0;
        let hero_h = (w * 9.0 / 16.0).round();
        let (hero, _) = ui.allocate_exact_size(vec2(w, hero_h), Sense::hover());
        if ui.is_rect_visible(hero) {
            paint_hero(ui, cx, pal, q, hero, radius);
        }
        let inner = egui::Frame::new().inner_margin(Margin { left: 16, right: 16, top: 28, bottom: 16 }).show(ui, |ui| {
            ui.set_width(w - 32.0);
            let m = &q.config.messages;
            ui.add(egui::Label::new(RichText::new(&m.quest_name).font(semibold(pal.heading())).color(pal.text)).truncate().selectable(false));
            let by = match (m.game_title.as_str(), m.game_publisher.as_str()) {
                ("", _) => String::new(),
                (g, "") => g.to_owned(),
                (g, p) => format!("{g} by {p}"),
            };
            if !by.is_empty() {
                ui.add_space(2.0);
                ui.add(egui::Label::new(RichText::new(by).size(pal.meta()).color(pal.muted)).truncate().selectable(false));
            }
            ui.add_space(8.0);
            let reward = q.config.rewards_config.rewards.first();
            let (r_icon, r_text) = match reward {
                Some(r) if r.kind == 4 => (Icon::Orb, format!("{} Orbs", r.orb_quantity.unwrap_or_default())),
                Some(r) => (Icon::Gift, r.messages.name.clone()),
                None => (Icon::Gift, "A reward".into()),
            };
            icon_text(ui, r_icon, pal.text, &r_text, FontId::proportional(pal.ui()), pal.text, 1, pal.surface);
            let rewards_end = q.config.rewards_config.rewards_expire_at.as_deref().or(Some(q.config.expires_at.as_str()));
            let date = match phase {
                Phase::Claimable => rewards_end.and_then(crate::quests::parse_time).map(|t| format!("Claim by {}", day(t, now))),
                Phase::Claimed => None,
                _ => crate::quests::parse_time(&q.config.expires_at).map(|t| format!("Ends {}", day(t, now))),
            };
            if let Some(d) = date {
                ui.add_space(4.0);
                icon_text(ui, Icon::Clock, pal.muted, &d, FontId::proportional(pal.meta()), pal.muted, 1, pal.surface);
            }
            ui.add_space(12.0);
            let (rule, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
            ui.painter().hline(rule.x_range(), rule.center().y, egui::Stroke::new(1.0, pal.tk.rule));
            ui.add_space(12.0);
            icon_text(ui, icon, pal.muted, &line, FontId::proportional(pal.ui()), pal.text, 2, pal.surface);
            if let Some(extra) = extra.filter(|_| phase != Phase::Claimed) {
                ui.add_space(4.0);
                text_block(ui, extra, FontId::proportional(pal.meta()), pal.muted, 2);
            }
            match phase {
                Phase::InProgress | Phase::Claimable => {
                    ui.add_space(12.0);
                    let live = cx.view.quests_ui.watch.as_ref().filter(|w| w.quest_id == q.id).map(|w| w.pos as u32);
                    progress_row(ui, pal, q, phase == Phase::Claimable || q.completed(), live);
                }
                Phase::Claimed => {
                    ui.add_space(12.0);
                    let at = q.user_status.as_ref().and_then(|s| s.claimed_at.as_deref()).and_then(crate::quests::parse_time);
                    let text = at.map_or_else(|| "Claimed".to_owned(), |t| format!("Claimed {}", day(t, now)));
                    icon_text(ui, Icon::Check, pal.tk.positive, &text, FontId::proportional(pal.ui()), pal.text, 1, pal.surface);
                    if let Some(code) = cx.state.quests.codes.get(&q.id) {
                        ui.add_space(12.0);
                        if code_well(ui, cx, pal, code.expose()) {
                            click = Some(Click::Copy);
                        }
                    }
                }
                _ => {}
            }
            if let Some(n) = cx.state.quests.notices.get(&q.id) {
                ui.add_space(12.0);
                icon_text(ui, Icon::Info, pal.danger, n, FontId::proportional(pal.meta()), pal.danger, 3, pal.surface);
            }
            let used = ui.min_rect().height();
            let action = card_button(q, phase, kind, can_play, cx.state.quests.codes.contains_key(&q.id), watching_quest(cx.view, q.id));
            let tail = if action.is_some() { 12.0 + 32.0 } else { 0.0 };
            natural = hero_h + 28.0 + used + tail + 16.0;
            ui.add_space((min_h - natural).max(0.0));
            if let Some((label, c)) = action {
                ui.add_space(12.0);
                let enabled = !busy && !(c == Click::Accept && blocked.is_some());
                let resp = ui.with_layout(Layout::top_down_justified(Align::Min), |ui| button(ui, pal, Btn::Secondary, label, enabled)).inner;
                let resp = match blocked.filter(|_| c == Click::Accept) {
                    Some(t) => resp.on_disabled_hover_text(format!("You can accept quests again on {}.", day(t, now))),
                    None => resp,
                };
                if resp.clicked() {
                    click = Some(c);
                }
            }
        });
        // Tile over the hero's bottom edge: 40, a 2 px ring in the card colour.
        let tile = Rect::from_min_size(pos2(hero.left() + 16.0, hero.bottom() - 24.0), Vec2::splat(40.0));
        if ui.is_rect_visible(tile) {
            paint_tile(ui, cx, pal, q, tile);
        }
        inner.response.rect
    });
    let rect = out.response.rect;
    ui.painter().set(bg, Shape::rect_filled(rect, radius, pal.surface));
    (natural, click)
}

/// The card's one button for its phase (QUESTS_NOTES §4.3 table).
fn card_button(q: &Quest, phase: Phase, kind: Option<TaskKind>, can_play: bool, has_code: bool, watching: bool) -> Option<(&'static str, Click)> {
    match phase {
        Phase::Available if kind.is_some_and(|k| k != TaskKind::Other) => Some(("Accept quest", Click::Accept)),
        Phase::InProgress if kind == Some(TaskKind::Video) && can_play && !q.completed() => {
            Some((if watching { "Open video" } else if q.video_value() == 0 { "Watch video" } else { "Continue watching" }, Click::Watch))
        }
        Phase::Claimable => Some(("Claim reward", Click::Claim)),
        Phase::Claimed if !has_code && q.config.rewards_config.rewards.first().is_some_and(|r| r.kind == 1) => Some(("Show code", Click::ShowCode)),
        _ => None,
    }
}

/// Progress bar (4 tall, `edge` @ 50 % track, `text` fill) and "1 of 3 min"; done: `positive`
/// with a Check and "Completed". `live`: the playing session's seconds (shown if past the server's).
fn progress_row(ui: &mut egui::Ui, pal: &Pal, q: &Quest, done: bool, live: Option<u32>) {
    let (v, t) = q.progress();
    let v = v.max(live.unwrap_or(0).min(t));
    let (v, t) = if done { (t.max(1), t.max(1)) } else { (v, t) };
    if done {
        icon_text(ui, Icon::Check, pal.tk.positive, "Completed", FontId::proportional(pal.ui()), pal.text, 1, pal.surface);
        ui.add_space(8.0);
    }
    let label = if done { String::new() } else { amount(v, t) };
    let font = FontId::proportional(pal.meta());
    let g = ui.painter().layout_no_wrap(label, font, pal.muted);
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), g.size().y.max(16.0)), Sense::hover());
    let bar_w = if g.size().x > 0.0 { row.width() - g.size().x - 12.0 } else { row.width() };
    let bar = Rect::from_center_size(pos2(row.left() + bar_w / 2.0, row.center().y), vec2(bar_w, 4.0));
    paint_bar(ui.painter(), pal, bar, if t == 0 { 0.0 } else { v as f32 / t as f32 }, done);
    ui.painter().galley(pos2(row.right() - g.size().x, row.center().y - g.size().y / 2.0), g, pal.muted);
}

fn paint_bar(p: &egui::Painter, pal: &Pal, bar: Rect, frac: f32, done: bool) {
    p.rect_filled(bar, 2.0, pal.tk.edge.gamma_multiply(0.5));
    let fill = Rect::from_min_size(bar.min, vec2(bar.width() * frac.clamp(0.0, 1.0), bar.height()));
    if fill.width() > 0.0 {
        p.rect_filled(fill, 2.0, if done { pal.tk.positive } else { pal.text });
    }
}

/// The reward code in a well (mono, `code_background`) with a Copy button. True when copied.
fn code_well(ui: &mut egui::Ui, cx: &UiCx<'_>, pal: &Pal, code: &str) -> bool {
    let well = cx.theme.colors.code_background.to_color32();
    egui::Frame::new()
        .fill(well)
        .corner_radius(pal.radius)
        .inner_margin(Margin { left: 12, right: 4, top: 4, bottom: 4 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(code).font(FontId::monospace(pal.ui())).color(pal.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| button(ui, pal, Btn::Secondary, "Copy", true).clicked()).inner
            })
            .inner
        })
        .inner
}

/// Hero: the image (cover-fit) when loaded, else the quest's two colours as a vertical gradient;
/// top corners round. Images load only for cards on screen.
fn paint_hero(ui: &egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, q: &Quest, rect: Rect, r: u8) {
    let colors = &q.config.colors;
    let parse = |s: &str| Color32::from_hex(s).ok();
    let (top, bottom) = (parse(&colors.primary).unwrap_or(pal.tk.selected), parse(&colors.secondary).unwrap_or(pal.surface));
    gradient(ui.painter(), rect, f32::from(r), top, bottom);
    let url = q.config.assets.hero.as_deref().and_then(|a| crate::api::cdn::quest_asset_url(q.id, a));
    if let Some(tex) = url.and_then(|u| cx.images.get(ui.ctx(), &u, 480, false)) {
        let uv = cover_uv(tex.size, rect.size());
        egui::Image::new(tex).uv(uv).corner_radius(CornerRadius { nw: r, ne: r, sw: 0, se: 0 }).paint_at(ui, rect);
    }
}

fn paint_tile(ui: &egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, q: &Quest, tile: Rect) {
    let r = f32::from(pal.radius);
    ui.painter().rect_filled(tile.expand(2.0), r + 2.0, pal.surface);
    let url = q.config.assets.game_tile.as_deref().and_then(|a| crate::api::cdn::quest_tile_url(q.id, a, pal.dark));
    match url.and_then(|u| cx.images.get(ui.ctx(), &u, 96, false)) {
        Some(tex) => egui::Image::new(tex).corner_radius(pal.radius).paint_at(ui, tile),
        None => {
            let name = if q.config.messages.game_title.is_empty() { &q.config.application.name } else { &q.config.messages.game_title };
            super::images::paint_initial(ui.painter(), tile, r, q.config.application.id.0, name, pal.dark);
        }
    }
}

/// A vertical two-colour gradient over `rect` with round top corners (a convex fan mesh).
fn gradient(p: &egui::Painter, rect: Rect, r: f32, top: Color32, bottom: Color32) {
    let r = r.min(rect.width() / 2.0).min(rect.height());
    let mut pts = Vec::new();
    for (cx, a0) in [(rect.left() + r, 180.0f32), (rect.right() - r, 270.0)] {
        for i in 0..=6 {
            let a = (a0 + 90.0 * i as f32 / 6.0).to_radians();
            pts.push(pos2(cx + r * a.cos(), rect.top() + r + r * a.sin()));
        }
    }
    pts.extend([rect.right_bottom(), rect.left_bottom()]);
    let mut mesh = egui::Mesh::default();
    for v in &pts {
        mesh.colored_vertex(*v, top.lerp_to_gamma(bottom, ((v.y - rect.top()) / rect.height()).clamp(0.0, 1.0)));
    }
    for i in 1..pts.len() as u32 - 1 {
        mesh.add_triangle(0, i, i + 1);
    }
    p.add(mesh);
}

/// UV of a cover fit: the texture's centre part with the target's aspect.
fn cover_uv(tex: Vec2, target: Vec2) -> Rect {
    let full = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    if tex.x <= 0.0 || tex.y <= 0.0 || target.x <= 0.0 || target.y <= 0.0 {
        return full;
    }
    let (ta, ra) = (tex.x / tex.y, target.x / target.y);
    if ta > ra {
        let w = ra / ta;
        Rect::from_min_max(pos2((1.0 - w) / 2.0, 0.0), pos2((1.0 + w) / 2.0, 1.0))
    } else {
        let h = ta / ra;
        Rect::from_min_max(pos2(0.0, (1.0 - h) / 2.0), pos2(1.0, (1.0 + h) / 2.0))
    }
}

/// A 16 px icon on the first line of `text` (wrapped to `rows` lines).
#[allow(clippy::too_many_arguments, reason = "icon, its colour, the text and its look")]
fn icon_text(ui: &mut egui::Ui, icon: Icon, icon_color: Color32, text: &str, font: FontId, color: Color32, rows: usize, bg: Color32) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let row_h = ui.fonts_mut(|f| f.row_height(&font));
        let (r, _) = ui.allocate_exact_size(vec2(16.0, row_h), Sense::hover());
        icons::paint(ui.painter(), icon, Rect::from_center_size(r.center(), Vec2::splat(16.0)), icon_color, bg);
        text_block(ui, text, font, color, rows);
    });
}

/// `text` wrapped to the available width, at most `rows` lines (then "…").
fn text_block(ui: &mut egui::Ui, text: &str, font: FontId, color: Color32, rows: usize) {
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, color, ui.available_width());
    job.wrap.max_rows = rows;
    let g = ui.painter().layout_job(job);
    let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
    ui.painter().galley(r.min, g, color);
}

/// Height of the user panel's "Quest video" section, its hairline not included.
pub const PANEL_H: f32 = 56.0;

/// A quest video session exists (its modal may be closed): the user panel shows its section.
pub fn watching(view: &ViewState) -> bool {
    view.quests_ui.watch.is_some()
}

/// Quest complete: the notification bodies to show now (`RustcordApp::logic`). A quest whose
/// reward just became claimable, once ever (`NOTIFIED_FILE`); the first look after an update or
/// a login only records what already waits. Nothing while the user is looking at the Quests
/// page or the video; those quests count as told. File I/O: never call it from tests.
pub fn completed(state: &mut crate::state::AppState, view: &ViewState) -> Vec<String> {
    if !matches!(state.quests.load, Load::Loaded(_)) || !std::mem::take(&mut state.quests.changed) {
        return Vec::new();
    }
    let looking = view.window_focused
        && !view.chat_hidden()
        && ((view.quests_ui.open && super::friends::is_open(state)) || matches!(view.modal, Some(Modal::QuestVideo { .. })));
    let path = crate::config::paths().cache_dir.join(crate::quests::NOTIFIED_FILE);
    let qs = &mut state.quests;
    let (mut set, seed) = match qs.notified.take() {
        Some(s) => (s, false),
        None => match std::fs::read_to_string(&path) {
            Ok(text) => (text.lines().filter_map(|l| l.trim().parse::<u64>().ok()).map(Id).collect(), false),
            Err(_) => (Default::default(), true),
        },
    };
    let before = set.clone();
    let new = crate::quests::newly_claimable(&qs.quests, &mut set, Utc::now());
    if seed || set != before {
        let text: String = set.iter().map(|id| format!("{}\n", id.0)).collect();
        let _ = crate::config::write_atomic(&path, text.as_bytes());
    }
    qs.notified = Some(set);
    if seed || looking {
        return Vec::new();
    }
    new.into_iter().filter_map(|id| qs.quest(id)).map(|q| format!("{}: claim your reward.", q.config.messages.quest_name)).collect()
}

/// A notification was clicked: if the last one was a quest's, show the Quests page.
pub fn notification_clicked(state: &mut crate::state::AppState, view: &mut ViewState) {
    if std::mem::take(&mut view.quests_ui.notified_last) {
        super::friends::open(state);
        view.quests_ui.open = true;
        view.settings_open = false;
        view.server_settings = None;
    }
}

fn watching_quest(view: &ViewState, id: Id) -> bool {
    view.quests_ui.watch.as_ref().is_some_and(|w| w.quest_id == id)
}

/// Quest players keep playing while not drawn (the user started them; `logic` reports them).
fn video_opts(s: &crate::config::Settings) -> crate::media::Opts {
    crate::media::Opts { output: s.voice_output.clone(), background: true, ..Default::default() }
}

/// Starts `q`'s session and shows its modal; one quest video at a time, so a running one stops
/// first (its last seconds are sent).
fn start_watch(view: &mut ViewState, cmd: &CommandSender, q: &Quest, player: Option<crate::media::Player>) {
    stop_watch(view, cmd);
    view.quests_ui.watch = Some(Watch::new(q, player));
    view.modal = Some(Modal::QuestVideo { quest_id: q.id });
}

/// Ends the session: the last whole seconds go out (not for the demo), its modal closes.
fn stop_watch(view: &mut ViewState, cmd: &CommandSender) {
    if let Some(mut w) = view.quests_ui.watch.take()
        && !w.demo
        && let Some(ts) = w.progress.close()
    {
        cmd.send(Command::Quests(QuestCommand::VideoProgress { quest_id: w.quest_id, timestamp: ts }));
    }
    if matches!(view.modal, Some(Modal::QuestVideo { .. })) {
        view.modal = None;
    }
}

/// Whether a session goes on: playing, or loading, or paused but not by the user (between
/// open and the queued Play). It ends when done, when a send failed, when the user paused and
/// left, or when the player is over (an answer still on its way counts first).
fn keeps_playing(state: Option<State>, user_paused: bool, done: bool, stopped: bool, in_flight: bool) -> bool {
    match state {
        _ if done || stopped || user_paused => false,
        Some(State::Loading | State::Paused | State::Playing) => true,
        Some(State::Ended | State::Freed) => in_flight,
        _ => false,
    }
}

/// Drives the session on every app pass, drawn or not (minimized, in the tray): the player's
/// state, the progress position, the ticks that send progress, and the end. Foreground and
/// background are the same code and the same sends (docs/QUESTS_NOTES.md §3).
pub fn logic(ctx: &egui::Context, state: &mut crate::state::AppState, view: &mut ViewState, cmd: &CommandSender) {
    let modal = matches!(view.modal, Some(Modal::QuestVideo { .. }));
    let Some(id) = view.quests_ui.watch.as_ref().map(|w| w.quest_id) else { return };
    let Some(q) = state.quests.quest(id) else { return stop_watch(view, cmd) };
    let (completed, enrolled, target) = (q.completed(), q.enrolled(), q.task().map_or(0, |(_, t)| t.target));
    let online = state.conn == ConnState::Connected && !crate::config::offline_mode();
    let Some(w) = view.quests_ui.watch.as_mut() else { return };
    if w.demo_play && w.player.is_none() {
        // The clip `make_fixture.py media` seeds; `demo` keeps sending off.
        let opts = crate::media::Opts { muted: true, looped: true, background: true, ..Default::default() };
        let p = crate::media::Player::open(ctx, "https://cdn.discordapp.com/attachments/1/3/demo.mp4", opts);
        p.play();
        w.player = Some(p);
    }
    let status = w.player.as_ref().map(|p| p.status());
    match status.map(|s| s.state) {
        Some(State::Failed(Fail::Network)) => _ = state.quests.notices.insert(id, VIDEO_OFFLINE.into()),
        Some(State::Failed(_)) => _ = state.quests.no_video.insert(id),
        // Paused and hidden for a long time: the worker ended; Play opens it again.
        Some(State::Freed) if modal => (w.player, w.texture) = (None, None),
        _ => {}
    }
    // The picture's position, never past what really played since the session's start.
    (w.pos, w.playing) = match status {
        Some(st) => ((st.position_ms as f64 / 1000.0).min(f64::from(w.start) + st.played_ms as f64 / 1000.0), st.state == State::Playing && !st.buffering),
        None => (f64::from(w.progress.saved()), false),
    };
    if !modal {
        w.texture = None;
    }
    if !w.demo
        && online
        && enrolled
        && !completed
        && let Some(st) = status
        && let Some(ts) = w.progress.tick(w.pos, st.played_ms, Instant::now())
    {
        cmd.send(Command::Quests(QuestCommand::VideoProgress { quest_id: id, timestamp: ts }));
    }
    // Only what the server confirmed counts as done.
    let done = completed || (target > 0 && w.progress.saved() >= target);
    let st = status.map(|s| s.state);
    if !modal && !keeps_playing(st, w.paused, done, w.progress.stopped(), w.progress.in_flight()) {
        return stop_watch(view, cmd);
    }
    if matches!(st, Some(State::Loading | State::Playing)) || (st == Some(State::Paused) && !w.paused) {
        ctx.request_repaint_after_for(Duration::from_secs(1), egui::ViewportId::ROOT);
    }
}

fn on_click(ctx: &egui::Context, cx: &mut UiCx<'_>, q: &Quest, c: Click) {
    let qs = &mut cx.state.quests;
    qs.notices.remove(&q.id);
    let sealed = q.traffic_metadata_sealed.clone();
    let send = |qs: &mut crate::quests::QuestState, cmd: QuestCommand| {
        qs.busy.insert(q.id);
        cx.cmd.send(Command::Quests(cmd));
    };
    match c {
        Click::Accept => send(qs, QuestCommand::Enroll { quest_id: q.id, sealed }),
        Click::ShowCode => send(qs, QuestCommand::RewardCode { quest_id: q.id }),
        Click::Claim => match q.claim_platform() {
            Some(platform) => send(qs, QuestCommand::Claim { quest_id: q.id, platform, sealed }),
            None => {
                cx.view.quests_ui.claim_pick = None;
                cx.view.modal = Some(Modal::QuestClaim { quest_id: q.id });
            }
        },
        Click::Watch => {
            // Its video already plays: just show it.
            if watching_quest(cx.view, q.id) {
                cx.view.modal = Some(Modal::QuestVideo { quest_id: q.id });
                return;
            }
            let Some(url) = q.video_url() else { return };
            // Opens playing, resumed at the server's value; no seeking control after that.
            let player = crate::media::Player::open(ctx, &url, video_opts(cx.settings));
            player.seek(u64::from(q.video_value()) * 1000);
            player.play();
            start_watch(cx.view, cx.cmd, q, Some(player));
        }
        Click::Copy => {
            if let Some(code) = qs.codes.get(&q.id) {
                ctx.copy_text(code.expose().to_owned());
                cx.view.toast("Code copied", false);
            }
        }
    }
}

// ----- modals ---------------------------------------------------------------------------------------

/// `Modal::QuestVideo`: the quest video (wide 640), a display-only progress bar, "Stop video",
/// Close, and "Claim reward" once done. Closing (button, ×, Esc, scrim) only hides it: the
/// session plays on (`logic`) unless the user paused it.
pub fn video_modal(ctx: &egui::Context, cx: &mut UiCx<'_>, quest_id: Id) {
    let Some(q) = cx.state.quests.quest(quest_id).cloned() else { return stop_watch(cx.view, cx.cmd) };
    if !watching_quest(cx.view, quest_id) {
        cx.view.modal = None;
        return;
    }
    let task = q.task().map(|(_, t)| t.clone()).unwrap_or_default();
    let title = task.messages.video_title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| q.config.messages.quest_name.clone());
    let notice = cx.state.quests.notices.get(&quest_id).cloned();
    let theme = &*cx.theme;
    let watch = cx.view.quests_ui.watch.as_mut().expect("checked above");
    let pal = Pal::new(theme);
    // Drawn: conversion and publishing of its pictures run while this modal paints.
    let status = watch.player.as_ref().map(|p| {
        p.drawn();
        p.status()
    });
    let state = status.map(|s| s.state);
    let failed = matches!(state, Some(State::Failed(f)) if f != Fail::Network);
    let offline = state == Some(State::Failed(Fail::Network));
    let (pos, playing) = (watch.pos, watch.playing);
    let duration = match status {
        Some(st) if st.duration_ms > 0 => st.duration_ms as f64 / 1000.0,
        _ => f64::from(task.target),
    };
    let done = q.completed() || (watch.progress.target() > 0 && watch.progress.saved() >= watch.progress.target());
    let mut reopen = false;
    let resp = super::dialog(ctx, theme, "rc_quest_video", 640.0, &title, |ui| {
        let w = ui.available_width();
        let (frame, _) = ui.allocate_exact_size(vec2(w, (w * 9.0 / 16.0).round()), Sense::hover());
        ui.painter().rect_filled(frame, pal.radius, Color32::BLACK);
        if failed {
            let text = "This video can't play in Rustcord. Watch it in the Discord app.";
            let g = ui.painter().layout(text.into(), FontId::proportional(pal.ui()), Color32::WHITE, frame.width() - 48.0);
            ui.painter().galley(frame.center() - g.size() / 2.0, g, Color32::WHITE);
        } else if !paint_frame(ui, watch, frame) {
            icons::paint(ui.painter(), Icon::Video, Rect::from_center_size(frame.center(), Vec2::splat(48.0)), Color32::from_gray(160), Color32::BLACK);
        }
        ui.add_space(8.0);
        let shown = pos.min(f64::from(task.target));
        let font = FontId::proportional(pal.meta());
        // Play / pause, the time, sound on / off. No seeking.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let enabled = !watch.demo && !failed;
            let (icon, tip) = if playing { (Icon::Pause, "Pause") } else { (Icon::Play, "Play") };
            if ui.add_enabled_ui(enabled, |ui| super::messages::vector_button(ui, &pal, icon, tip, 32.0, false, pal.text)).inner.clicked() {
                match &watch.player {
                    Some(p) if playing => {
                        p.pause();
                        watch.paused = true;
                    }
                    // Lost its connection: Play opens it again.
                    Some(_) if offline => reopen = true,
                    Some(p) => {
                        p.play();
                        watch.paused = false;
                    }
                    None => reopen = true,
                }
            }
            ui.label(RichText::new(format!("{} / {}", clock(pos), clock(duration))).font(font.clone()).color(pal.muted));
            if status.is_some_and(|s| s.has_audio) {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (icon, tip) = if watch.muted { (Icon::VoiceOff, "Unmute") } else { (Icon::Voice, "Mute") };
                    if super::messages::vector_button(ui, &pal, icon, tip, 32.0, false, pal.text).clicked() {
                        set_muted(watch, !watch.muted);
                    }
                });
            }
        });
        let left = (f64::from(task.target) - shown).max(0.0) as u32;
        let label = if done { "Done".to_owned() } else { format!("{} left", time_left(left)) };
        let g = ui.painter().layout_no_wrap(label, font, pal.muted);
        let (row, _) = ui.allocate_exact_size(vec2(w, g.size().y.max(16.0)), Sense::hover());
        let bar = Rect::from_min_max(pos2(row.left(), row.center().y - 2.0), pos2(row.right() - g.size().x - 12.0, row.center().y + 2.0));
        paint_bar(ui.painter(), &pal, bar, if task.target == 0 { 0.0 } else { (shown / f64::from(task.target)) as f32 }, done);
        ui.painter().galley(pos2(row.right() - g.size().x, row.center().y - g.size().y / 2.0), g, pal.muted);
        let line = match &notice {
            Some(n) => n.as_str(),
            None if done => "Done. Claim your reward.",
            None => "Progress counts while the video plays, also after you close this.",
        };
        ui.label(RichText::new(line).size(pal.meta()).color(if notice.is_some() { pal.danger } else { pal.muted }));
        ui.add_space(8.0);
        let stoppable = !done && watch.player.is_some();
        super::dialog_footer(ui, |ui| {
            if done && button(ui, &pal, Btn::Primary, "Claim reward", !cx.state.quests.busy.contains(&quest_id)).clicked() {
                return Some(Act::Claim);
            }
            if button(ui, &pal, Btn::Ghost, "Close", true).clicked() {
                return Some(Act::Close);
            }
            (stoppable && button(ui, &pal, Btn::Ghost, "Stop video", true).clicked()).then_some(Act::Stop)
        })
    });
    if reopen && let Some(url) = q.video_url() {
        let opts = video_opts(cx.settings);
        cx.state.quests.notices.remove(&quest_id);
        let w = cx.view.quests_ui.watch.as_mut().expect("checked above");
        let p = crate::media::Player::open(ctx, &url, opts);
        // The new player's `played_ms` counts on from the seconds already saved.
        w.start = w.progress.saved();
        p.seek(u64::from(w.start) * 1000);
        p.set_volume(if w.muted { 0.0 } else { 1.0 });
        p.play();
        (w.player, w.paused) = (Some(p), false);
    }
    match resp.inner {
        Some(Act::Claim) => {
            stop_watch(cx.view, cx.cmd);
            on_click(ctx, cx, &q, Click::Claim);
        }
        Some(Act::Stop) => stop_watch(cx.view, cx.cmd),
        // Hidden only: the next `logic` pass ends a session that is not playing.
        Some(Act::Close) => {
            cx.view.modal = None;
            ctx.request_repaint();
        }
        None if resp.should_close() => {
            cx.view.modal = None;
            ctx.request_repaint();
        }
        None => {}
    }
}

/// What the video modal's footer asked for.
enum Act {
    Claim,
    Close,
    Stop,
}

/// Sound on or off for the session (the panel and the modal share it): volume 0, the stream runs on.
fn set_muted(w: &mut Watch, muted: bool) {
    w.muted = muted;
    if let Some(p) = &w.player {
        p.set_volume(if muted { 0.0 } else { 1.0 });
    }
}

/// The "Quest video" section of the floating user panel (`sidebar::user_panel`), in `rect`
/// (`PANEL_H` tall): the label, the quest's name (opens the video), Mute and Stop, and a live
/// progress bar.
pub fn panel(ui: &mut egui::Ui, cx: &mut UiCx<'_>, rect: Rect) {
    let Some(w) = cx.view.quests_ui.watch.as_ref() else { return };
    let Some(q) = cx.state.quests.quest(w.quest_id) else { return };
    let (name, target) = (q.config.messages.quest_name.clone(), q.task().map_or(0, |(_, t)| t.target));
    let (pos, muted, has_audio) = (w.pos, w.muted, w.player.as_ref().is_some_and(|p| p.status().has_audio));
    let done = q.completed() || (target > 0 && w.progress.saved() >= target);
    let (c, tk) = (cx.theme.colors.clone(), cx.theme.tokens());
    let size = cx.theme.font.size;
    let row = Rect::from_min_size(rect.min + Vec2::splat(12.0), vec2(rect.width() - 24.0, 32.0));
    let base = c.surface.to_color32();
    let radius = cx.theme.layout.corner_radius;
    let button = |ui: &mut egui::Ui, at: Rect, id: &str, icon: Icon, tip: &str| {
        let resp = ui.interact(at, ui.make_persistent_id(id), Sense::click());
        let bg = if resp.hovered() || resp.has_focus() { base.blend(c.text.to_color32().gamma_multiply(0.08)) } else { base };
        ui.painter().rect_filled(at, CornerRadius::from(radius), bg);
        icons::paint(ui.painter(), icon, at.shrink(6.0), c.text_muted.to_color32(), bg);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tip));
        resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
    };
    let stop = Rect::from_min_size(pos2(row.right() - 32.0, row.top()), Vec2::splat(32.0));
    let mut left = stop.left();
    let mut toggle_mute = false;
    if has_audio {
        let at = stop.translate(vec2(-36.0, 0.0));
        left = at.left();
        let (icon, tip) = if muted { (Icon::VoiceOff, "Unmute") } else { (Icon::Voice, "Mute") };
        toggle_mute = button(ui, at, "quest_video_mute", icon, tip);
    }
    let stop_clicked = button(ui, stop, "quest_video_stop", Icon::Close, "Stop video");
    let max_w = left - 8.0 - row.left();
    let link_rect = Rect::from_min_max(pos2(row.left(), row.center().y), pos2(row.left() + max_w, row.bottom()));
    let link = ui.interact(link_rect, ui.make_persistent_id("quest_video_open"), Sense::click());
    let link_color = if link.hovered() { c.text } else { c.text_muted }.to_color32();
    let top_g = super::one_line(ui, "Quest video", semibold(size - 2.0), tk.positive_text, max_w);
    let sub_g = super::one_line(ui, name.as_str(), FontId::proportional(size - 3.0), link_color, max_w);
    let top = row.center().y - (top_g.size().y + sub_g.size().y) / 2.0;
    let sub_y = top + top_g.size().y;
    ui.painter().galley(pos2(row.left(), top), top_g, tk.positive_text);
    ui.painter().galley(pos2(row.left(), sub_y), sub_g, link_color);
    link.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, &name));
    let open = link.on_hover_text("Open the video").on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
    let pal = Pal::new(cx.theme);
    let bar = Rect::from_min_size(pos2(row.left(), row.bottom() + 4.0), vec2(row.width(), 3.0));
    paint_bar(ui.painter(), &pal, bar, if target == 0 { 0.0 } else { (pos.min(f64::from(target)) / f64::from(target)) as f32 }, done);
    if toggle_mute && let Some(w) = cx.view.quests_ui.watch.as_mut() {
        set_muted(w, !muted);
    }
    if stop_clicked {
        stop_watch(cx.view, cx.cmd);
    } else if open && let Some(id) = cx.view.quests_ui.watch.as_ref().map(|w| w.quest_id) {
        cx.view.modal = Some(Modal::QuestVideo { quest_id: id });
    }
}

/// The player's latest frame, letterboxed into `rect`; false before the first one.
fn paint_frame(ui: &egui::Ui, watch: &mut Watch, rect: Rect) -> bool {
    let Some(p) = &watch.player else { return false };
    let ppp = ui.ctx().pixels_per_point();
    p.set_target((rect.width() * ppp) as u32 & !1, (rect.height() * ppp) as u32 & !1);
    // `latest` is None while the worker writes: keep the last texture then.
    if let Some((generation, image)) = crate::media::frames(ui.ctx()).latest(&p.key()) {
        if let Some((g, tex)) = &mut watch.texture {
            if *g != generation {
                *g = generation;
                tex.set(image, egui::TextureOptions::LINEAR);
            }
        } else {
            watch.texture = Some((generation, ui.ctx().load_texture("rc_quest_video", image, egui::TextureOptions::LINEAR)));
        }
    }
    let Some((_, tex)) = &watch.texture else { return false };
    let size = tex.size_vec2();
    let scale = (rect.width() / size.x).min(rect.height() / size.y);
    let img = Rect::from_center_size(rect.center(), size * scale);
    ui.painter().image(tex.id(), img, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    true
}

/// A video progress answer (ui/mod.rs drain): the session may send its next one, or stops.
pub fn on_video_answer(view: &mut ViewState, quest_id: Id, ok: bool) {
    if let Some(w) = view.quests_ui.watch.as_mut().filter(|w| w.quest_id == quest_id) {
        w.progress.answered(ok);
    }
}

/// `Modal::QuestClaim`: the platform the reward's code is made for (several listed).
pub fn claim_modal(ctx: &egui::Context, cx: &mut UiCx<'_>, quest_id: Id) {
    let Some(q) = cx.state.quests.quest(quest_id).cloned() else {
        cx.view.modal = None;
        return;
    };
    let theme = &*cx.theme;
    let pal = Pal::new(theme);
    let pick = &mut cx.view.quests_ui.claim_pick;
    let busy = cx.state.quests.busy.contains(&quest_id);
    let resp = super::dialog(ctx, theme, "rc_quest_claim", 440.0, "Where will you use this reward?", |ui| {
        super::dialog_text(ui, theme, "Discord makes the code for the platform you pick.");
        ui.add_space(8.0);
        egui::Frame::new().fill(pal.surface).corner_radius(pal.radius).inner_margin(Margin::same(4)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            for (i, &p) in q.config.rewards_config.platforms.iter().enumerate() {
                if i > 0 {
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
                    ui.painter().hline(r.x_range().shrink(8.0), r.center().y, egui::Stroke::new(1.0, pal.tk.rule));
                }
                if platform_row(ui, &pal, p, *pick == Some(p)).clicked() {
                    *pick = Some(p);
                }
            }
        });
        super::dialog_buttons(ui, theme, "Claim reward", false, pick.is_some() && !busy)
    });
    match resp.inner {
        Some(true) => {
            if let Some(platform) = cx.view.quests_ui.claim_pick.take() {
                cx.state.quests.notices.remove(&quest_id);
                cx.state.quests.busy.insert(quest_id);
                cx.cmd.send(Command::Quests(QuestCommand::Claim { quest_id, platform, sealed: q.traffic_metadata_sealed.clone() }));
            }
            cx.view.modal = None;
        }
        Some(false) => cx.view.modal = None,
        None if resp.should_close() => cx.view.modal = None,
        None => {}
    }
}

/// UD Quest Platform Type -> its name and icon.
fn platform(p: u8) -> (&'static str, Icon) {
    match p {
        1 => ("Xbox", Icon::Playing),
        2 => ("PlayStation", Icon::Playing),
        3 => ("Nintendo Switch", Icon::Playing),
        4 => ("PC", Icon::Desktop),
        _ => ("All platforms", Icon::Globe),
    }
}

/// A §9 option row (40): icon 20, name 15/500, the radio on the right.
fn platform_row(ui: &mut egui::Ui, pal: &Pal, p: u8, selected: bool) -> egui::Response {
    let (label, icon) = platform(p);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
    let r = pal.radius.saturating_sub(4).max(2);
    if resp.hovered() {
        ui.painter().rect_filled(rect, r, pal.tk.hover);
    }
    let hot = selected || resp.hovered() || resp.has_focus();
    icons::paint(ui.painter(), icon, Rect::from_center_size(pos2(rect.left() + 22.0, rect.center().y), Vec2::splat(20.0)), if hot { pal.text } else { pal.muted }, pal.surface);
    let font = FontId::new(pal.body(), crate::theme::medium_family());
    ui.painter().text(pos2(rect.left() + 44.0, rect.center().y), egui::Align2::LEFT_CENTER, label, font, pal.text);
    paint_radio(ui.painter(), pal, Rect::from_center_size(pos2(rect.right() - 22.0, rect.center().y), Vec2::splat(20.0)), selected);
    if resp.has_focus() {
        focus_ring(ui, pal, rect.shrink(2.0), f32::from(r));
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, selected, label));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ----- words -----------------------------------------------------------------------------------------

/// "45 seconds" under two minutes, else whole minutes rounded up.
fn duration(secs: u32) -> String {
    if secs < 120 { format!("{secs} seconds") } else { format!("{} minutes", secs.div_ceil(60)) }
}

/// "1 of 3 min" (seconds under two minutes).
fn amount(v: u32, t: u32) -> String {
    if t < 120 { format!("{v} of {t} sec") } else { format!("{} of {} min", v / 60, t.div_ceil(60)) }
}

/// "1 min 48 s".
fn time_left(secs: u32) -> String {
    match (secs / 60, secs % 60) {
        (0, s) => format!("{s} s"),
        (m, 0) => format!("{m} min"),
        (m, s) => format!("{m} min {s} s"),
    }
}

/// "1:12".
fn clock(secs: f64) -> String {
    let s = secs.max(0.0) as u32;
    format!("{}:{:02}", s / 60, s % 60)
}

/// "Nov 28", with the year when it is not this year's.
fn day(t: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let local = t.with_timezone(&chrono::Local);
    if local.year() == now.with_timezone(&chrono::Local).year() { local.format("%b %-d").to_string() } else { local.format("%b %-d, %Y").to_string() }
}

// ----- test hooks ------------------------------------------------------------------------------------

/// `RUSTCORD_OPEN=quests[:blocked|empty|error|loading|watch|watch-play|bg|complete|claim]` (ui/mod.rs): the page with
/// `DEMO_JSON` (Orbs 1240, a code shown, a captcha notice), the blocked banner, the empty / failed
/// / loading states, the video modal (a still frame at 1:12 of 3:00, nothing sent), `watch-play`
/// (the fixture clip plays in it), `bg` (the same without the modal: the user panel's section
/// and the card's "Open video"), `complete` (the Friends page; quest 101 completes, so the next
/// pass shows the "Quest complete" notification and writes `NOTIFIED_FILE` in the home) or the
/// claim modal. Sends nothing. False for other tokens.
pub fn demo(state: &mut crate::state::AppState, view: &mut ViewState, token: &str) -> bool {
    let Some(rest) = token.strip_prefix("quests") else { return false };
    let what = rest.trim_start_matches(':');
    if !rest.is_empty() && !rest.starts_with(':') {
        return false;
    }
    super::friends::open(state);
    view.quests_ui.open = true;
    let q = &mut state.quests;
    // Dates relative to today, so the demo never expires.
    let shift = |d: i64| (Utc::now() + chrono::Duration::days(d)).to_rfc3339();
    let json = DEMO_JSON.replace("2026-11-28T00:00:00+00:00", &shift(14)).replace("2026-12-28T00:00:00+00:00", &shift(44)).replace("2026-11-12T12:00:00+00:00", &shift(-2));
    let page = serde_json::from_str(&json).expect("DEMO_JSON parses");
    q.apply(crate::quests::QuestEvent::Loaded(Ok(page)));
    q.orbs = Some(1240);
    q.codes.insert(Id(106), std::sync::Arc::new(crate::auth::Secret::new("RCDEMO-1234-ABCD".into())));
    q.notices.insert(Id(104), "Discord wants a captcha to claim this. Claim it in the official app.".into());
    match what {
        "blocked" => q.blocked_until = Some(shift(6)),
        "empty" => q.quests.clear(),
        // Only a first fetch shows these (a refetch keeps the cards).
        "error" => (q.quests, q.load) = (Vec::new(), Load::Failed("Network problem: could not connect to Discord. Check your internet connection.".into())),
        "loading" => (q.quests, q.load) = (Vec::new(), Load::Loading),
        "watch" => {
            view.quests_ui.watch = Some(Watch::new(q.quest(Id(101)).expect("demo quest 101"), None));
            view.modal = Some(Modal::QuestVideo { quest_id: Id(101) });
        }
        "watch-play" | "bg" => {
            let mut w = Watch::new(q.quest(Id(101)).expect("demo quest 101"), None);
            (w.start, w.muted, w.demo_play) = (0, true, true);
            view.quests_ui.watch = Some(w);
            // `bg`: the page with the panel section, the video plays on without its modal.
            view.modal = (what == "watch-play").then_some(Modal::QuestVideo { quest_id: Id(101) });
        }
        "claim" => view.modal = Some(Modal::QuestClaim { quest_id: Id(106) }),
        // The Friends page; the next `completed` pass notifies for 101 (104 counts as told).
        "complete" => {
            view.quests_ui.open = false;
            q.notified = Some([Id(104)].into());
            if let Some(s) = q.quests.iter_mut().find(|x| x.id == Id(101)).and_then(|x| x.user_status.as_mut()) {
                s.completed_at = Some(shift(0));
            }
        }
        _ => {}
    }
    true
}

/// Made-up quests for the demo hook and the parser tests (own names; no Discord copy or assets).
pub const DEMO_JSON: &str = r##"{"quests":[
 {"id":"101","preview":false,"config":{"id":"101","config_version":2,"starts_at":"2026-11-01T00:00:00+00:00","expires_at":"2026-11-28T00:00:00+00:00","features":[15],
  "application":{"id":"9001","name":"Ferris Studio"},"assets":{"hero":"hero.png","game_tile":"tile.png","logotype":"logo.png"},
  "colors":{"primary":"#C2410C","secondary":"#1E293B"},
  "messages":{"quest_name":"Finish three sketches","game_title":"Ferris Studio","game_publisher":"Crab Works"},
  "task_config_v2":{"join_operator":"or","tasks":{"WATCH_VIDEO":{"type":"WATCH_VIDEO","target":180,
    "assets":{"video":{"url":"https://cdn.discordapp.com/quests/101/trailer.mp4"}},"messages":{"video_title":"Finish three sketches: the trailer"}}}},
  "rewards_config":{"assignment_method":1,"platforms":[0],"rewards_expire_at":"2026-12-28T00:00:00+00:00",
    "rewards":[{"type":4,"sku_id":"7001","messages":{"name":"700 Orbs","name_with_article":"700 Orbs"},"orb_quantity":700}]}},
  "user_status":{"user_id":"1","quest_id":"101","enrolled_at":"2026-11-10T10:00:00+00:00","completed_at":null,"claimed_at":null,
    "progress":{"WATCH_VIDEO":{"event_name":"WATCH_VIDEO","value":72,"updated_at":"2026-11-10T10:02:00+00:00","completed_at":null}}}},
 {"id":"102","preview":false,"config":{"id":"102","config_version":2,"expires_at":"2026-11-28T00:00:00+00:00","features":[],
  "application":{"id":"9002","name":"Rust Belt Racer"},"assets":{"hero":"hero.png","game_tile":"tile.png"},
  "colors":{"primary":"#0F766E","secondary":"#172554"},
  "messages":{"quest_name":"Lap the borrow checker","game_title":"Rust Belt Racer","game_publisher":"Ironclad Games"},
  "task_config_v2":{"join_operator":"or","tasks":{"PLAY_ON_DESKTOP":{"type":"PLAY_ON_DESKTOP","target":900,"applications":[{"id":"9002"}]}}},
  "rewards_config":{"assignment_method":1,"platforms":[0],"rewards_expire_at":"2026-12-28T00:00:00+00:00",
    "rewards":[{"type":3,"sku_id":"7002","messages":{"name":"Racer avatar decoration","name_with_article":"a Racer avatar decoration"}}]}},
  "user_status":{"user_id":"1","quest_id":"102","enrolled_at":"2026-11-09T18:00:00+00:00","completed_at":null,"claimed_at":null,
    "progress":{"PLAY_ON_DESKTOP":{"event_name":"PLAY_ON_DESKTOP","value":540,"updated_at":"2026-11-12T20:00:00+00:00","completed_at":null}}}},
 {"id":"103","preview":false,"config":{"id":"103","config_version":2,"expires_at":"2026-11-28T00:00:00+00:00","features":[],
  "application":{"id":"9003","name":"Cargo Quest"},"assets":{"hero":"hero.png","game_tile":"tile.png"},
  "colors":{"primary":"#7C3AED","secondary":"#1F2937"},
  "messages":{"quest_name":"Ship your first crate","game_title":"Cargo Quest","game_publisher":"Borrow Checker Studio"},
  "task_config_v2":{"join_operator":"or","tasks":{"WATCH_VIDEO":{"type":"WATCH_VIDEO","target":120,
    "assets":{"video_low_res":{"url":"trailer_low.mp4"},"video":{"url":"trailer.mp4"}}}}},
  "rewards_config":{"assignment_method":1,"platforms":[],"rewards_expire_at":"2026-12-28T00:00:00+00:00",
    "rewards":[{"type":4,"sku_id":"7003","messages":{"name":"500 Orbs","name_with_article":"500 Orbs"},"orb_quantity":500}]}},
  "user_status":null},
 {"id":"104","preview":false,"config":{"id":"104","config_version":2,"expires_at":"2026-11-28T00:00:00+00:00","features":[],
  "application":{"id":"9001","name":"Ferris Studio"},"assets":{"hero":"hero2.png","game_tile":"tile.png"},
  "colors":{"primary":"#B45309","secondary":"#292524"},
  "messages":{"quest_name":"Watch the reveal","game_title":"Ferris Studio","game_publisher":"Crab Works"},
  "task_config_v2":{"join_operator":"or","tasks":{"WATCH_VIDEO":{"type":"WATCH_VIDEO","target":90}}},
  "rewards_config":{"assignment_method":1,"platforms":[0],"rewards_expire_at":"2026-12-28T00:00:00+00:00",
    "rewards":[{"type":4,"sku_id":"7004","messages":{"name":"1000 Orbs","name_with_article":"1000 Orbs"},"orb_quantity":1000}]}},
  "user_status":{"user_id":"1","quest_id":"104","enrolled_at":"2026-11-08T09:00:00+00:00","completed_at":"2026-11-08T09:02:00+00:00","claimed_at":null,
    "progress":{"WATCH_VIDEO":{"event_name":"WATCH_VIDEO","value":90,"updated_at":"2026-11-08T09:02:00+00:00","completed_at":"2026-11-08T09:02:00+00:00"}}}},
 {"id":"105","preview":false,"config":{"id":"105","config_version":2,"expires_at":"2026-11-28T00:00:00+00:00","features":[],
  "application":{"id":"9003","name":"Cargo Quest"},"assets":{"hero":"hero.webm","game_tile":"tile.png"},
  "colors":{"primary":"#2563EB","secondary":"#0F172A"},
  "messages":{"quest_name":"Unbox on the go","game_title":"Cargo Quest","game_publisher":"Borrow Checker Studio"},
  "task_config_v2":{"join_operator":"or","tasks":{"WATCH_VIDEO_ON_MOBILE":{"type":"WATCH_VIDEO_ON_MOBILE","target":60}}},
  "rewards_config":{"assignment_method":1,"platforms":[0],"rewards_expire_at":"2026-12-28T00:00:00+00:00",
    "rewards":[{"type":3,"sku_id":"7005","messages":{"name":"Crate profile frame","name_with_article":"a Crate profile frame"}}]}},
  "user_status":null},
 {"id":"106","preview":false,"config":{"id":"106","config_version":2,"expires_at":"2026-11-28T00:00:00+00:00","features":[],
  "application":{"id":"9002","name":"Rust Belt Racer"},"assets":{"hero":"hero.png","game_tile":"tile.png"},
  "colors":{"primary":"#BE123C","secondary":"#1C1917"},
  "messages":{"quest_name":"Start your engines","game_title":"Rust Belt Racer","game_publisher":"Ironclad Games"},
  "task_config_v2":{"join_operator":"or","tasks":{"WATCH_VIDEO":{"type":"WATCH_VIDEO","target":60}}},
  "rewards_config":{"assignment_method":1,"platforms":[4,1,2],"rewards_expire_at":"2026-12-28T00:00:00+00:00",
    "rewards":[{"type":1,"sku_id":"7006","messages":{"name":"Rust Belt Racer starter pack","name_with_article":"the starter pack"}}]}},
  "user_status":{"user_id":"1","quest_id":"106","enrolled_at":"2026-11-05T09:00:00+00:00","completed_at":"2026-11-05T09:01:00+00:00","claimed_at":"2026-11-12T12:00:00+00:00",
    "progress":{"WATCH_VIDEO":{"event_name":"WATCH_VIDEO","value":60,"updated_at":"2026-11-05T09:01:00+00:00","completed_at":"2026-11-05T09:01:00+00:00"}}}}
 ],"excluded_quests":[],"quest_enrollment_blocked_until":null}"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_and_words() {
        assert_eq!(columns(773.0), (2, 378.5));
        assert_eq!(columns(1400.0), (3, 420.0));
        assert_eq!(columns(320.0), (1, 320.0));
        assert_eq!(columns(250.0), (1, 250.0));
        assert_eq!((duration(45), duration(180), duration(901)), ("45 seconds".into(), "3 minutes".into(), "16 minutes".into()));
        assert_eq!((amount(72, 180), amount(30, 90)), ("1 of 3 min".into(), "30 of 90 sec".into()));
        assert_eq!((time_left(108), time_left(45), time_left(120)), ("1 min 48 s".into(), "45 s".into(), "2 min".into()));
        assert_eq!(clock(72.9), "1:12");
        let uv = cover_uv(vec2(200.0, 100.0), vec2(160.0, 90.0));
        assert!((uv.width() - 0.888_889).abs() < 1e-3 && uv.height() == 1.0);
    }

    #[test]
    fn keeps_playing_rules() {
        use State::*;
        // (state, user paused, done, stopped, in flight) -> goes on
        let rows = [
            (Some(Playing), false, false, false, false, true),
            (Some(Loading), false, false, false, false, true),
            (Some(Paused), false, false, false, false, true),
            (Some(Paused), true, false, false, false, false),
            (Some(Ended), false, false, false, false, false),
            (Some(Ended), false, false, false, true, true),
            (Some(Freed), false, false, false, false, false),
            (Some(Failed(Fail::Network)), false, false, false, false, false),
            (None, false, false, false, false, false),
            (Some(Playing), false, true, false, false, false),
            (Some(Playing), false, false, true, false, false),
        ];
        for (state, paused, done, stopped, in_flight, want) in rows {
            assert_eq!(keeps_playing(state, paused, done, stopped, in_flight), want, "{state:?} paused={paused} done={done} stopped={stopped} in_flight={in_flight}");
        }
    }

    /// One quest video at a time: starting another stops the running one and sends its last
    /// seconds; "Watch" on the one that plays only reopens its modal.
    #[test]
    fn one_quest_video_at_a_time() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cmd = crate::backend::CommandSender(tx);
        let mut state = crate::state::AppState::default();
        let mut view = ViewState::default();
        demo(&mut state, &mut view, "quests");
        let (q101, q103) = (state.quests.quest(Id(101)).unwrap().clone(), state.quests.quest(Id(103)).unwrap().clone());
        start_watch(&mut view, &cmd, &q101, None);
        let t0 = Instant::now();
        let w = view.quests_ui.watch.as_mut().unwrap();
        w.demo = false;
        assert_eq!(w.progress.tick(72.0, 0, t0), None);
        assert_eq!(w.progress.tick(73.9, 1900, t0 + Duration::from_millis(1900)), None);
        assert!(rx.try_recv().is_err(), "nothing sent yet");
        start_watch(&mut view, &cmd, &q103, None);
        assert!(matches!(rx.try_recv(), Ok(Command::Quests(QuestCommand::VideoProgress { quest_id: Id(101), timestamp: 73 }))));
        assert!(rx.try_recv().is_err());
        assert_eq!(view.quests_ui.watch.as_ref().map(|w| w.quest_id), Some(Id(103)));
        assert!(matches!(view.modal, Some(Modal::QuestVideo { quest_id: Id(103) })));
        // Watch on the playing quest: the modal only.
        view.modal = None;
        let (mut images, mut theme) = (super::super::images::ImageCache::new(cmd.clone()), crate::theme::Theme::nightshade());
        let (mut settings, mut overrides) = (Default::default(), Default::default());
        let ctx = egui::Context::default();
        let mut cx = UiCx { state: &mut state, view: &mut view, images: &mut images, theme: &mut theme, settings: &mut settings, cmd: &cmd, overrides: &mut overrides };
        on_click(&ctx, &mut cx, &q103, Click::Watch);
        assert!(matches!(view.modal, Some(Modal::QuestVideo { quest_id: Id(103) })));
        assert!(rx.try_recv().is_err() && view.quests_ui.watch.as_ref().is_some_and(|w| w.player.is_none()));
        drop((images, view));
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
    }

    /// Runs the page and modals for every demo token over a few passes: nothing reaches the
    /// backend but image loads.
    #[test]
    fn demo_hooks_send_nothing() {
        for token in ["quests", "quests:blocked", "quests:empty", "quests:error", "quests:loading", "quests:watch", "quests:watch-play", "quests:bg", "quests:complete", "quests:claim"] {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let cmd = crate::backend::CommandSender(tx);
            let mut state = crate::state::AppState::default();
            state.conn = ConnState::Connected;
            let (mut view, mut images) = (ViewState::default(), super::super::images::ImageCache::new(cmd.clone()));
            assert!(demo(&mut state, &mut view, token), "{token}");
            let mut theme = crate::theme::Theme::nightshade();
            let (mut settings, mut overrides) = (Default::default(), Default::default());
            let ctx = egui::Context::default();
            for _ in 0..3 {
                let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1180.0, 760.0))), ..Default::default() };
                ctx.run_ui(input, |ui| {
                    let mut cx = UiCx { state: &mut state, view: &mut view, images: &mut images, theme: &mut theme, settings: &mut settings, cmd: &cmd, overrides: &mut overrides };
                    egui::CentralPanel::default().show(ui, |ui| show(ui, &mut cx));
                    super::super::show_modals(ui.ctx(), &mut cx);
                })
                .textures_delta
                .clear();
            }
            while let Ok(c) = rx.try_recv() {
                assert!(matches!(c, Command::LoadImage { .. }), "{token} sent {c:?}");
            }
            assert!(view.quests_ui.open || token == "quests:complete");
            // Textures decoded in the background free on drop: flush that delta while ctx lives.
            drop((images, view));
            ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        }
        assert!(!demo(&mut crate::state::AppState::default(), &mut ViewState::default(), "questsx"));
    }
}
