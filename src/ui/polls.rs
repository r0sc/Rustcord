//! Polls (docs/PROTOCOL_NOTES.md, Polls; docs/DESIGN.md §10.3 Polls).
//! - `card`: a poll in a message. The vote view (rows with a radio or checkbox, "Vote") shows
//!   until I voted, the poll closed, or I peeked at the results; the results view has a bar per
//!   answer and the share. Voting counts at once (`AppState::set_my_poll_votes`) and sends one
//!   `ChatCommand::PollVote`; a failure puts my votes back. Other people's votes arrive as
//!   gateway events and change the bars without a tween. Only my own Vote or Show results grows
//!   the bars (`motion::BASE`).
//! - Pure helpers (`time_left`, `shares`, `closed`, `votes`) are tested below. The one timer is
//!   the "N hours left" text, which wakes the view when it changes.
//!
//! Left out on purpose: ending a poll early, answer emoji in the create dialog, polls inside
//! pins, search and the inbox.
//! OWNERSHIP: ui-messages.

use super::icons::{self, Icon};
use super::messages::{Btn, Pal, button, emoji_texture, focus_ring, paint_checkbox, paint_radio};
use super::{UiCx, motion, one_line, semibold};
use crate::backend::{ChatCommand, Command};
use crate::model::{Id, Message, Poll, PollAnswer, PollAnswerCount, PollMedia, PollResults};
use crate::state::polls::my_votes;
use egui::{Align, FontId, Frame, Image, Label, Layout, Margin, Rect, RichText, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};
use std::time::Duration;

/// Widest a poll card gets.
const MAX_W: f32 = 440.0;

/// The polls' form and view state.
#[derive(Default)]
pub struct PollsUi {
    /// The answers ticked in the poll I am voting in (one poll at a time).
    picked: Option<(Id, Vec<u32>)>,
    /// The poll whose results I look at without having voted.
    peek: Option<Id>,
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// Unix ms of an ISO-8601 time.
fn parse_ms(iso: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(iso).ok().map(|t| t.timestamp_millis())
}

/// Finalized, or its time is up.
fn closed(p: &Poll, now_ms: i64) -> bool {
    p.results.as_ref().is_some_and(|r| r.is_finalized) || p.expiry.as_deref().and_then(parse_ms).is_some_and(|end| end <= now_ms)
}

/// Rounded percent of the sum per count; no votes gives zeros.
fn shares(counts: &[u32]) -> Vec<u32> {
    let total: u64 = counts.iter().map(|&c| u64::from(c)).sum();
    counts.iter().map(|&c| (u64::from(c) * 100 + total / 2).checked_div(total).unwrap_or(0) as u32).collect()
}

fn votes(n: u32) -> String {
    if n == 1 { "1 vote".into() } else { format!("{n} votes") }
}

/// "3 days left", "5 hours left", "1 minute left", "Less than a minute left" and when that text
/// changes; None once the time is up.
fn time_left(now_ms: i64, end_ms: i64) -> Option<(String, Duration)> {
    const MIN: i64 = 60_000;
    const HOUR: i64 = 60 * MIN;
    const DAY: i64 = 24 * HOUR;
    let left = end_ms - now_ms;
    if left <= 0 {
        return None;
    }
    let (unit, word) = match left {
        l if l >= DAY => (DAY, "day"),
        l if l >= HOUR => (HOUR, "hour"),
        l if l >= MIN => (MIN, "minute"),
        _ => return Some(("Less than a minute left".into(), Duration::from_millis(left as u64 + 1))),
    };
    let n = left / unit;
    let text = format!("{n} {word}{} left", if n == 1 { "" } else { "s" });
    Some((text, Duration::from_millis((left % unit) as u64 + 1000)))
}

fn count_of(p: &Poll, answer_id: u32) -> u32 {
    p.results.iter().flat_map(|r| &r.answer_counts).find(|c| c.id == answer_id).map_or(0, |c| c.count)
}

// ---------------------------------------------------------------------------
// The card
// ---------------------------------------------------------------------------

/// What the card shows this pass.
struct View {
    /// Bars and shares instead of radios.
    results: bool,
    /// The counts are known (an absent `results` is unknown, not zero).
    known: bool,
    multi: bool,
    closed: bool,
    /// 0 -> 1 growth of the bars after my Vote or Show results.
    grow: f32,
}

/// One answer's numbers.
struct Row {
    count: u32,
    share: u32,
    mine: bool,
    picked: bool,
    winner: bool,
}

pub(super) fn card(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, ch: Id, mid: Id, poll: &Poll) {
    let ctx = ui.ctx().clone();
    let now = chrono::Utc::now().timestamp_millis();
    let mine = my_votes(poll);
    let closed = closed(poll, now);
    let peek = cx.view.polls_ui.peek == Some(mid);
    let known = poll.results.is_some();
    let results = closed || !mine.is_empty() || peek;
    let grow = if closed { 1.0 } else { motion::anim(&ctx, egui::Id::unique(("rc_poll_grow", mid.0)), !mine.is_empty() || peek, motion::BASE, 0.0) };
    let view = View { results, known, multi: poll.allow_multiselect, closed, grow };
    let picked: Vec<u32> = cx.view.polls_ui.picked.as_ref().filter(|(m, _)| *m == mid).map(|(_, v)| v.clone()).unwrap_or_default();
    let counts: Vec<u32> = poll.answers.iter().map(|a| count_of(poll, a.answer_id)).collect();
    let (shares, total, top) = (shares(&counts), counts.iter().sum::<u32>(), counts.iter().copied().max().unwrap_or(0));
    let hc = pal.tk.mode == crate::theme::Mode::HighContrast;
    let stroke = if hc { Stroke::new(1.0, pal.tk.rule) } else { Stroke::NONE };
    let width = ui.available_width().min(MAX_W);
    // The frame's own width: 16 px padding on both sides.
    Frame::new().fill(pal.surface).stroke(stroke).corner_radius(pal.card_radius()).inner_margin(Margin::same(16)).show(ui, |ui| {
        ui.set_width(width - 32.0);
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.add(Label::new(RichText::new(&poll.question.text).font(semibold(pal.heading())).color(pal.text)).wrap());
        if !view.results {
            let hint = if view.multi { "Select one or more answers" } else { "Select one answer" };
            ui.label(RichText::new(hint).size(pal.meta()).color(pal.muted));
        }
        let mut clicked = None;
        for (i, a) in poll.answers.iter().enumerate() {
            let row = Row {
                count: counts[i],
                share: shares[i],
                mine: mine.contains(&a.answer_id),
                picked: picked.contains(&a.answer_id),
                winner: view.closed && counts[i] > 0 && counts[i] == top,
            };
            if answer_row(ui, cx, pal, &view, a, &row) {
                clicked = Some(a.answer_id);
            }
        }
        if let Some(id) = clicked {
            let ui_state = &mut cx.view.polls_ui;
            let mut ids = picked.clone();
            if view.multi {
                match ids.iter().position(|x| *x == id) {
                    Some(at) => {
                        ids.remove(at);
                    }
                    None => ids.push(id),
                }
            } else {
                ids = vec![id];
            }
            ui_state.picked = Some((mid, ids));
        }
        ui.add_space(4.0);
        footer(ui, cx, pal, &view, (ch, mid), (total, &mine, &picked), poll, now);
    });
    // The text "N hours left" changes by itself: wake when it does.
    if !closed && let Some((_, wake)) = poll.expiry.as_deref().and_then(parse_ms).and_then(|end| time_left(now, end)) {
        ctx.request_repaint_after(wake);
    }
}

/// One answer: a row to pick (vote view) or a bar with its share (results view). True when a
/// vote row was clicked.
fn answer_row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, v: &View, a: &PollAnswer, r: &Row) -> bool {
    let text = a.poll_media.text.as_str();
    let sense = if v.results { Sense::hover() } else { Sense::click() };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), sense);
    if v.results {
        let note = format!("{text}: {}, {}%{}{}", votes(r.count), r.share, if r.mine { ", your vote" } else { "" }, if r.winner { ", most votes" } else { "" });
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, note.clone()));
    } else {
        let role = if v.multi { egui::Role::CheckBox } else { egui::Role::RadioButton };
        resp.widget_info(|| egui::WidgetInfo::selected(role, true, r.picked, text));
    }
    let resp = if v.results && v.known && r.count > 0 { resp.on_hover_text(votes(r.count)) } else { resp };
    if !ui.is_rect_visible(rect) {
        return resp.clicked();
    }
    let (tk, radius) = (&pal.tk, pal.radius);
    let mut right = rect.right() - 12.0;
    let family = if v.results && r.winner { crate::theme::semibold_family() } else { crate::theme::medium_family() };
    if v.results {
        ui.painter().rect_filled(rect, radius, tk.hover);
        if v.known {
            let w = rect.width() * r.share as f32 / 100.0 * v.grow;
            if w >= 1.0 {
                ui.painter().rect_filled(Rect::from_min_size(rect.min, vec2(w, rect.height())), radius, if r.mine { tk.chip } else { tk.selected });
            }
            if r.mine {
                ui.painter().rect_stroke(rect, radius, Stroke::new(1.0, pal.link), StrokeKind::Inside);
            }
            let g = ui.painter().layout_no_wrap(format!("{}%", r.share), FontId::new(pal.ui(), crate::theme::semibold_family()), pal.text);
            right -= g.size().x;
            ui.painter().galley(pos2(right, rect.center().y - g.size().y / 2.0), g, pal.text);
            if r.mine {
                right -= 20.0;
                icons::paint(ui.painter(), Icon::Check, Rect::from_center_size(pos2(right + 8.0, rect.center().y), Vec2::splat(16.0)), pal.link, tk.chip);
            }
        }
    } else {
        let draw = motion::pressed(&resp, rect);
        ui.painter().rect_filled(draw, radius, if r.picked || resp.hovered() { tk.selected } else { tk.hover });
        if resp.has_focus() {
            focus_ring(ui, pal, draw, f32::from(radius));
        }
        let control = Rect::from_center_size(pos2(rect.right() - 20.0, rect.center().y), Vec2::splat(20.0));
        if v.multi {
            paint_checkbox(ui.painter(), tk, control, r.picked, f32::from(radius / 2));
        } else {
            paint_radio(ui.painter(), pal, control, r.picked);
        }
        right = control.left();
    }
    paint_text(ui, cx, pal, a, rect, right - 8.0, family);
    resp.clicked()
}

/// The answer's emoji and text, left aligned in `rect`, ending before `right`.
fn paint_text(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, a: &PollAnswer, rect: Rect, right: f32, family: egui::FontFamily) {
    let mut x = rect.left() + 12.0;
    if let Some(e) = &a.poll_media.emoji {
        let slot = Rect::from_center_size(pos2(x + 10.0, rect.center().y), Vec2::splat(20.0));
        match (e.id, e.name.as_deref()) {
            (Some(id), _) => {
                if let Some(tex) = emoji_texture(ui, cx.images, id, None) {
                    Image::from_texture(tex).paint_at(ui, slot);
                }
            }
            (None, Some(name)) => {
                let g = ui.painter().layout_no_wrap(name.to_owned(), FontId::proportional(pal.body()), pal.text);
                ui.painter().galley(pos2(slot.center().x - g.size().x / 2.0, rect.center().y - g.size().y / 2.0), g, pal.text);
            }
            _ => {}
        }
        x += 28.0;
    }
    let g = one_line(ui, a.poll_media.text.as_str(), FontId::new(pal.body(), family), pal.text, right - x);
    ui.painter().galley(pos2(x, rect.center().y - g.size().y / 2.0), g, pal.text);
}

/// Votes, time left, and the buttons.
#[allow(clippy::too_many_arguments)]
fn footer(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, v: &View, (ch, mid): (Id, Id), (total, mine, picked): (u32, &[u32], &[u32]), poll: &Poll, now: i64) {
    ui.horizontal(|ui| {
        // The buttons are 32 tall: the row is too, so the labels centre on them.
        ui.set_min_height(32.0);
        ui.spacing_mut().item_spacing.x = 12.0;
        if v.known {
            ui.label(RichText::new(votes(total)).size(pal.meta()).color(pal.muted));
        }
        let left = if v.closed { Some("Poll closed".to_owned()) } else { poll.expiry.as_deref().and_then(parse_ms).and_then(|end| time_left(now, end)).map(|(t, _)| t) };
        if let Some(t) = left {
            ui.label(RichText::new(t).size(pal.meta()).color(pal.muted));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if v.closed {
                return;
            }
            let peek = cx.view.polls_ui.peek == Some(mid);
            if !mine.is_empty() {
                if button(ui, pal, Btn::Ghost, "Remove vote", true).clicked() {
                    send_vote(cx, ch, mid, Vec::new(), mine);
                }
            } else if peek {
                if button(ui, pal, Btn::Ghost, "Back to voting", true).clicked() {
                    cx.view.polls_ui.peek = None;
                }
            } else {
                // Secondary, not Primary: one ember button per view.
                if button(ui, pal, Btn::Secondary, "Vote", !picked.is_empty()).clicked() {
                    send_vote(cx, ch, mid, picked.to_vec(), mine);
                }
                if v.known && button(ui, pal, Btn::Ghost, "Show results", true).clicked() {
                    cx.view.polls_ui.peek = Some(mid);
                }
            }
        });
    });
}

/// Counts my votes at once and asks for them; a failure restores `mine`.
fn send_vote(cx: &mut UiCx<'_>, ch: Id, mid: Id, answers: Vec<u32>, mine: &[u32]) {
    let restore = cx.state.set_my_poll_votes(ch, mid, &answers).unwrap_or_else(|| mine.to_vec());
    cx.cmd.send(Command::Chat(ChatCommand::PollVote { channel_id: ch, message_id: mid, answer_ids: answers, restore }));
    cx.view.polls_ui.picked = None;
    cx.view.polls_ui.peek = None;
}

// ---------------------------------------------------------------------------
// Test hook
// ---------------------------------------------------------------------------

fn iso_in(secs: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(secs)).to_rfc3339()
}

fn answer(id: u32, text: &str) -> PollAnswer {
    PollAnswer { answer_id: id, poll_media: PollMedia { text: text.into(), emoji: None } }
}

fn counts(list: &[(u32, u32, bool)]) -> Option<PollResults> {
    Some(PollResults { is_finalized: false, answer_counts: list.iter().map(|&(id, count, me_voted)| PollAnswerCount { id, count, me_voted }).collect() })
}

/// `RUSTCORD_MSG_DEMO=poll`: three polls at the end of the open channel (a vote view with 23 h
/// left, a multiple choice I voted in with 3 days left, a closed one).
pub(super) fn demo(cx: &mut UiCx<'_>, ch: Id, demo: impl Fn(&str) -> bool) {
    if !demo("poll") {
        return;
    }
    let author = super::forward::demo_author(cx.state);
    let guild_id = cx.state.channels.get(&ch).and_then(|c| c.guild_id);
    let Some(list) = cx.state.messages.get_mut(&ch) else { return };
    let Some(newest) = list.msgs.last().map(|m| (m.id, m.timestamp.clone())) else { return };
    let single = Poll {
        question: PollMedia { text: "Where should the next meetup be?".into(), emoji: None },
        answers: vec![answer(1, "Berlin"), answer(2, "Hamburg"), answer(3, "Remote")],
        expiry: Some(iso_in(23 * 3600 + 600)),
        allow_multiselect: false,
        results: counts(&[(1, 5, false), (2, 3, false)]),
    };
    let multi = Poll {
        question: PollMedia { text: "Which feature next?".into(), emoji: None },
        answers: vec![answer(1, "Polls"), answer(2, "Forwards"), answer(3, "Threads in the sidebar"), answer(4, "Voice effects")],
        expiry: Some(iso_in(3 * 24 * 3600 + 600)),
        allow_multiselect: true,
        results: counts(&[(1, 8, true), (2, 4, false), (3, 6, true)]),
    };
    let done = Poll {
        question: PollMedia { text: "Best editor?".into(), emoji: None },
        answers: vec![answer(1, "Helix"), answer(2, "Neovim"), answer(3, "VS Code")],
        expiry: Some(iso_in(-3600)),
        allow_multiselect: false,
        results: Some(PollResults { is_finalized: true, answer_counts: vec![PollAnswerCount { id: 1, count: 4, me_voted: true }, PollAnswerCount { id: 2, count: 9, me_voted: false }, PollAnswerCount { id: 3, count: 7, me_voted: false }] }),
    };
    for (i, poll) in [single, multi, done].into_iter().enumerate() {
        list.msgs.push(Message {
            id: Id(newest.0.0 + ((i as u64 + 1) << 22)),
            channel_id: ch,
            guild_id,
            author: author.clone(),
            timestamp: newest.1.clone(),
            poll: Some(Box::new(poll)),
            ..Message::default()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_left_words_plurals_and_wake_times() {
        let end = 10 * 86_400_000;
        let left = |ms: i64| time_left(end - ms, end);
        assert_eq!(left(3 * 86_400_000 + 5000).unwrap(), ("3 days left".into(), Duration::from_millis(5000 + 1000)));
        assert_eq!(left(86_400_000).unwrap().0, "1 day left");
        assert_eq!(left(86_400_000 - 1).unwrap().0, "23 hours left");
        assert_eq!(left(3_600_000).unwrap().0, "1 hour left");
        assert_eq!(left(5 * 3_600_000 + 60_000).unwrap(), ("5 hours left".into(), Duration::from_millis(60_000 + 1000)));
        assert_eq!(left(60_000).unwrap().0, "1 minute left");
        assert_eq!(left(125_000).unwrap(), ("2 minutes left".into(), Duration::from_millis(5000 + 1000)));
        // Under a minute: the text stays until the time is up.
        assert_eq!(left(59_000).unwrap(), ("Less than a minute left".into(), Duration::from_millis(59_001)));
        assert_eq!(time_left(end, end), None);
        assert_eq!(time_left(end + 1, end), None);
    }

    #[test]
    fn shares_round_and_survive_no_votes() {
        assert_eq!(shares(&[]), Vec::<u32>::new());
        assert_eq!(shares(&[0, 0]), [0, 0]);
        assert_eq!(shares(&[1, 2]), [33, 67]);
        assert_eq!(shares(&[1, 1, 1]), [33, 33, 33]);
        assert_eq!(shares(&[5, 3, 0]), [63, 38, 0]);
        assert_eq!(shares(&[4, 9, 7]), [20, 45, 35]);
        assert_eq!(votes(1), "1 vote");
        assert_eq!(votes(0), "0 votes");
        assert_eq!(votes(12), "12 votes");
    }

    #[test]
    fn closed_when_finalized_or_expired() {
        let mut p = Poll { expiry: Some("2026-01-02T00:00:00+00:00".into()), ..Default::default() };
        let end = parse_ms("2026-01-02T00:00:00+00:00").unwrap();
        assert!(!closed(&p, end - 1) && closed(&p, end));
        p.expiry = None;
        assert!(!closed(&p, i64::MAX), "no expiry stays open until finalized");
        p.results = Some(PollResults { is_finalized: true, answer_counts: Vec::new() });
        assert!(closed(&p, 0));
        assert_eq!(parse_ms("nope"), None);
    }
}
