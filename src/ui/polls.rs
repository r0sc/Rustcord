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
use super::messages::{Btn, Pal, button, emoji_texture, focus_ring, paint_checkbox, paint_radio, small_button, vector_button};
use super::threads::field_label;
use super::{Modal, UiCx, ViewState, composer, dialog, dialog_buttons, motion, one_line, semibold};
use crate::backend::{ChatCommand, Command, PollDraft};
use crate::model::{Embed, EmbedField, Id, Message, MessageReference, Poll, PollAnswer, PollAnswerCount, PollMedia, PollResults, perm};
use crate::state::chat::PollVotersView;
use crate::state::polls::my_votes;
use egui::{Align, FontId, Frame, Image, Label, Layout, Margin, Rect, RichText, Sense, Stroke, StrokeKind, TextEdit, Vec2, pos2, vec2};
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
    /// The open voters popover: poll, answer, and "opened this pass" (it opens its popup once).
    voters: Option<(Id, u32, bool)>,
    // The create dialog's draft.
    question: String,
    answers: Vec<String>,
    hours: u32,
    multi: bool,
    busy: bool,
    error: Option<String>,
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

pub(super) fn card(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, ch: Id, guild_id: Option<Id>, mid: Id, poll: &Poll) {
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
        let open = cx.view.polls_ui.voters.filter(|(m, _, _)| *m == mid).map(|(_, a, _)| a);
        let mut anchor = None;
        for (i, a) in poll.answers.iter().enumerate() {
            let row = Row {
                count: counts[i],
                share: shares[i],
                mine: mine.contains(&a.answer_id),
                picked: picked.contains(&a.answer_id),
                winner: view.closed && counts[i] > 0 && counts[i] == top,
            };
            let (hit, rect) = answer_row(ui, cx, pal, &view, a, &row);
            if hit {
                clicked = Some((a.answer_id, rect));
            }
            if open == Some(a.answer_id) {
                anchor = Some((i, rect));
            }
        }
        if let Some((id, _)) = clicked.filter(|_| view.results) {
            toggle_voters(&ctx, cx, ch, mid, id);
        } else if let Some((id, _)) = clicked {
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
        // Opened by a click on that row (or the demo): while its rows are not drawn, nothing is.
        if let Some((i, rect)) = anchor {
            voters_popover(ui, cx, pal, (ch, guild_id, mid), &poll.answers[i], counts[i], rect);
        }
        ui.add_space(4.0);
        footer(ui, cx, pal, &view, (ch, mid), (total, &mine, &picked), poll, now);
    });
    // The text "N hours left" changes by itself: wake when it does.
    if !closed && let Some((_, wake)) = poll.expiry.as_deref().and_then(parse_ms).and_then(|end| time_left(now, end)) {
        ctx.request_repaint_after(wake);
    }
}

/// One answer: a row to pick (vote view) or a bar with its share (results view). True when it
/// was clicked, and its rect.
fn answer_row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, v: &View, a: &PollAnswer, r: &Row) -> (bool, Rect) {
    let text = a.poll_media.text.as_str();
    // A result with votes opens the voters; one without (or without counts) is just a label.
    let voters = v.results && v.known && r.count > 0;
    let sense = if v.results && !voters { Sense::hover() } else { Sense::click() };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), sense);
    if v.results {
        let note = format!("{text}: {}, {}%{}{}", votes(r.count), r.share, if r.mine { ", your vote" } else { "" }, if r.winner { ", most votes" } else { "" });
        let role = if voters { egui::Role::Button } else { egui::Role::Label };
        resp.widget_info(|| egui::WidgetInfo::labeled(role, true, note.clone()));
    } else {
        let role = if v.multi { egui::Role::CheckBox } else { egui::Role::RadioButton };
        resp.widget_info(|| egui::WidgetInfo::selected(role, true, r.picked, text));
    }
    let resp = if voters { resp.on_hover_text(votes(r.count)).on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp };
    if !ui.is_rect_visible(rect) {
        return (resp.clicked(), rect);
    }
    let (tk, radius) = (&pal.tk, pal.radius);
    let mut right = rect.right() - 12.0;
    let family = if v.results && r.winner { crate::theme::semibold_family() } else { crate::theme::medium_family() };
    if v.results {
        ui.painter().rect_filled(rect, radius, if voters && resp.hovered() { tk.hover.blend(pal.tint()) } else { tk.hover });
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
        if voters && resp.has_focus() {
            focus_ring(ui, pal, rect, f32::from(radius));
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
    (resp.clicked(), rect)
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
// Create a poll
// ---------------------------------------------------------------------------

/// Question and answer limits (Discord's).
const QUESTION_MAX: usize = 300;
const ANSWER_MAX: usize = 55;
const ANSWERS_MAX: usize = 10;
const HOURS: [(u32, &str); 7] = [(1, "1 hour"), (4, "4 hours"), (8, "8 hours"), (24, "24 hours"), (72, "3 days"), (168, "1 week"), (336, "2 weeks")];

impl PollsUi {
    fn reset_draft(&mut self) {
        (self.question, self.answers, self.hours, self.multi, self.busy, self.error) = Default::default();
    }
}

/// What the dialog sends: everything trimmed, empty answers dropped, at most ten; None without
/// a question or an answer.
fn draft(question: &str, answers: &[String], hours: u32, multi: bool) -> Option<PollDraft> {
    let answers: Vec<String> = answers.iter().map(|a| a.trim()).filter(|a| !a.is_empty()).take(ANSWERS_MAX).map(str::to_owned).collect();
    let question = question.trim().to_owned();
    (!question.is_empty() && !answers.is_empty()).then_some(PollDraft { question, answers, hours, multi })
}

/// Create poll from the composer's "+" menu.
pub(super) fn open_create(cx: &mut UiCx<'_>, channel_id: Id) {
    cx.view.modal = Some(Modal::CreatePoll { channel_id });
}

fn close_create(cx: &mut UiCx<'_>) {
    if !cx.view.polls_ui.busy {
        cx.view.polls_ui.reset_draft();
    }
    cx.view.modal = None;
}

/// `Modal::CreatePoll`: question, 2 to 10 answers, duration, multiple answers.
pub fn create_modal(ctx: &egui::Context, cx: &mut UiCx<'_>, ch: Id) {
    let pal = Pal::new(cx.theme);
    let theme = cx.theme.clone();
    let can_poll = cx.state.channel_permissions(ch) & perm::SEND_POLLS != 0;
    let st = &mut cx.view.polls_ui;
    if st.hours == 0 {
        st.hours = 24;
    }
    while st.answers.len() < 2 {
        st.answers.push(String::new());
    }
    let ready = can_poll && !st.busy && draft(&st.question, &st.answers, st.hours, st.multi).is_some();
    let resp = dialog(ctx, &theme, "rc_create_poll", 520.0, "Create a poll", |ui| {
        field_label(ui, &pal, "Question");
        let q = ui.add(TextEdit::singleline(&mut st.question).char_limit(QUESTION_MAX).hint_text("What do you want to ask?").desired_width(f32::INFINITY));
        if ui.memory(|m| m.focused().is_none()) {
            q.request_focus();
        }
        field_label(ui, &pal, "Answers");
        let removable = st.answers.len() > 2;
        let mut remove = None;
        for (i, a) in st.answers.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                let w = ui.available_width() - if removable { 32.0 + ui.spacing().item_spacing.x } else { 0.0 };
                ui.add(TextEdit::singleline(a).char_limit(ANSWER_MAX).hint_text("Type an answer").desired_width(w));
                if removable && vector_button(ui, &pal, Icon::Close, "Remove answer", 32.0, false, pal.text).clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            st.answers.remove(i);
        }
        if st.answers.len() < ANSWERS_MAX && button(ui, &pal, Btn::Ghost, "Add answer", true).clicked() {
            st.answers.push(String::new());
        }
        field_label(ui, &pal, "Duration");
        super::server_menu::choice(ui, "rc_poll_hours", &mut st.hours, &HOURS);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Allow multiple answers").font(FontId::new(pal.ui(), crate::theme::medium_family())).color(pal.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                super::messages::switch(ui, &pal, &mut st.multi, "Allow multiple answers");
            });
        });
        if !can_poll {
            ui.label(RichText::new("You can't create polls in this channel.").size(pal.meta()).color(pal.danger));
        }
        if let Some(e) = &st.error {
            ui.label(RichText::new(e).size(pal.meta()).color(pal.danger));
        }
        dialog_buttons(ui, &theme, if st.busy { "Posting…" } else { "Post" }, false, ready)
    });
    match resp.inner {
        Some(true) => {
            let st = &mut cx.view.polls_ui;
            if let Some(poll) = draft(&st.question, &st.answers, st.hours, st.multi).filter(|_| ready) {
                (st.busy, st.error) = (true, None);
                cx.cmd.send(Command::Chat(ChatCommand::SendPoll { channel_id: ch, nonce: composer::new_nonce(), poll }));
            }
        }
        Some(false) => close_create(cx),
        None if resp.should_close() => close_create(cx),
        None => {}
    }
}

/// `ChatEvent::PollPosted`: the dialog closes, or shows what went wrong (a toast when it was
/// closed meanwhile; the draft stays).
pub fn posted(view: &mut ViewState, channel_id: Id, result: Result<(), String>) {
    let open = view.modal == Some(Modal::CreatePoll { channel_id });
    match result {
        Ok(()) => {
            view.polls_ui.reset_draft();
            if open {
                view.modal = None;
            }
        }
        Err(e) => {
            let text = format!("Could not post the poll: {e}");
            view.polls_ui.busy = false;
            if open {
                view.polls_ui.error = Some(text);
            } else {
                view.toast(text, true);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The result message (type 46)
// ---------------------------------------------------------------------------

/// What a `poll_result` embed says.
#[derive(Debug, PartialEq)]
struct Summary {
    question: String,
    /// Set when there is a single winner.
    winner: Option<String>,
    votes: u32,
    total: u32,
}

fn field<'a>(e: &'a Embed, name: &str) -> Option<&'a str> {
    e.fields.iter().find(|f| f.name == name).map(|f| f.value.as_str())
}

fn result_summary(e: &Embed) -> Summary {
    let number = |name| field(e, name).and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    let winner = field(e, "victor_answer_id")
        .filter(|id| !id.is_empty())
        .and_then(|_| field(e, "victor_answer_text").or_else(|| field(e, "victor_answer_emoji_name")))
        .filter(|t| !t.is_empty())
        .map(str::to_owned);
    Summary { question: field(e, "poll_question_text").unwrap_or_default().to_owned(), winner, votes: number("victor_answer_votes"), total: number("total_votes") }
}

/// "Poll closed" with the question and the winner. When the message points at the poll, the whole
/// card is a button that jumps there.
pub(super) fn result_card(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, ch: Id, msg: &Message, e: &Embed) {
    let s = result_summary(e);
    let target = msg.message_reference.as_ref().and_then(|r| Some((r.channel_id.unwrap_or(ch), r.message_id?)));
    let width = ui.available_width().min(MAX_W);
    let bg = ui.painter().add(egui::Shape::Noop);
    let fill = pal.surface;
    let rect = Frame::new().corner_radius(pal.radius).inner_margin(Margin::same(12)).show(ui, |ui| {
        ui.set_width(width - 24.0);
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            super::messages::vector_icon(ui, Icon::Poll, 16.0, pal.muted, fill);
            ui.label(RichText::new("Poll closed").size(pal.meta()).color(pal.muted));
        });
        ui.add(Label::new(RichText::new(&s.question).font(semibold(pal.body())).color(pal.text)).wrap());
        match (&s.winner, s.total) {
            (_, 0) => {
                ui.label(RichText::new("No one voted.").size(pal.meta()).color(pal.muted));
            }
            (Some(w), total) => {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    super::messages::vector_icon(ui, Icon::Check, 16.0, pal.text, fill);
                    ui.label(RichText::new(w).font(FontId::new(pal.ui(), crate::theme::medium_family())).color(pal.text));
                    ui.add_space(4.0);
                    let pct = shares(&[s.votes, total.saturating_sub(s.votes)])[0];
                    ui.label(RichText::new(format!("{}, {pct}%", votes(s.votes))).size(pal.meta()).color(pal.muted));
                });
            }
            (None, _) => {
                ui.label(RichText::new("The poll ended in a tie.").size(pal.meta()).color(pal.muted));
            }
        }
    })
    .response
    .rect;
    let tk = &pal.tk;
    let Some((tch, mid)) = target else {
        ui.painter().set(bg, egui::Shape::rect_filled(rect, pal.radius, fill));
        return;
    };
    let resp = ui.interact(rect, ui.make_persistent_id(("rc_poll_result", msg.id.0)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("View poll: {}", s.question)));
    let shown = if resp.hovered() { fill.blend(tk.hover) } else { fill };
    ui.painter().set(bg, egui::Shape::rect_filled(rect, pal.radius, shown));
    if resp.has_focus() {
        focus_ring(ui, pal, rect, f32::from(pal.radius));
    }
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("View poll").clicked() {
        super::messages::jump_to(ui.ctx(), cx, tch, mid);
    }
}

// ---------------------------------------------------------------------------
// Who voted
// ---------------------------------------------------------------------------

fn voters_id(mid: Id, answer: u32) -> egui::Id {
    egui::Id::unique(("rc_poll_voters", mid.0, answer))
}

/// A click on a result row: its voters open (one request), a second click closes them.
fn toggle_voters(ctx: &egui::Context, cx: &mut UiCx<'_>, ch: Id, mid: Id, answer: u32) {
    let open = cx.view.polls_ui.voters;
    cx.view.polls_ui.voters = None;
    cx.state.chat.poll_voters = None;
    if let Some((m, a, _)) = open {
        egui::Popup::close_id(ctx, voters_id(m, a));
    }
    if open.is_some_and(|(m, a, _)| (m, a) == (mid, answer)) {
        return;
    }
    cx.view.polls_ui.voters = Some((mid, answer, true));
    cx.state.chat.poll_voters = Some(PollVotersView { channel_id: ch, message_id: mid, answer_id: answer, loading: true, ..Default::default() });
    cx.cmd.send(Command::Chat(ChatCommand::FetchPollVoters { channel_id: ch, message_id: mid, answer_id: answer, after: None }));
}

/// The list under the open answer's row: avatar and name, a click opens the profile.
fn voters_popover(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, (ch, guild_id, mid): (Id, Option<Id>, Id), answer: &PollAnswer, count: u32, row: Rect) {
    let ctx = ui.ctx().clone();
    let id = voters_id(mid, answer.answer_id);
    let opened_now = cx.view.polls_ui.voters.as_mut().is_some_and(|v| std::mem::take(&mut v.2));
    let frame = Frame::popup(ui.style()).fill(pal.tk.raised).stroke(Stroke::new(1.0, pal.tk.rule)).corner_radius(pal.card_radius()).inner_margin(Margin::same(8));
    egui::Popup::new(id, ctx.clone(), row, ui.layer_id())
        .kind(egui::PopupKind::Popup)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .open_memory(opened_now.then_some(egui::SetOpenCommand::Bool(true)))
        .align(egui::RectAlign::BOTTOM_START)
        .align_alternatives(&[egui::RectAlign::TOP_START])
        .gap(4.0)
        .width(280.0)
        .frame(frame)
        .show(|ui| voters_list(ui, cx, pal, (ch, guild_id, mid), answer, count));
    if !egui::Popup::is_id_open(&ctx, id) {
        cx.view.polls_ui.voters = None;
        cx.state.chat.poll_voters = None;
    }
}

fn voters_list(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, (ch, guild_id, mid): (Id, Option<Id>, Id), answer: &PollAnswer, count: u32) {
    ui.set_width(ui.available_width());
    ui.spacing_mut().item_spacing.y = 4.0;
    let Some(v) = cx.state.chat.poll_voters.as_ref().filter(|v| (v.message_id, v.answer_id) == (mid, answer.answer_id)) else { return };
    let (users, loading, error, more) = (v.users.clone(), v.loading, v.error.clone(), v.has_more);
    ui.horizontal(|ui| {
        ui.add(Label::new(RichText::new(&answer.poll_media.text).font(semibold(pal.label())).color(pal.muted)).truncate());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(RichText::new(votes(count)).size(pal.meta()).color(pal.muted)));
    });
    let fetch = |cx: &mut UiCx<'_>, after: Option<Id>| {
        if let Some(v) = cx.state.chat.poll_voters.as_mut() {
            (v.loading, v.error) = (true, None);
        }
        cx.cmd.send(Command::Chat(ChatCommand::FetchPollVoters { channel_id: ch, message_id: mid, answer_id: answer.answer_id, after }));
    };
    // Up to ten rows fit as they are; more scroll (a scroll area clips the last row of a short list).
    let rows = |ui: &mut egui::Ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for u in &users {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
            let name = cx.state.member_display_name(guild_id, u, None);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name.clone()));
            if !ui.is_rect_visible(rect) {
                continue;
            }
            if resp.hovered() {
                ui.painter().rect_filled(rect, pal.radius, pal.tk.hover);
            }
            if resp.has_focus() {
                focus_ring(ui, pal, rect, f32::from(pal.radius));
            }
            let avatar = Rect::from_min_size(pos2(rect.left() + 4.0, rect.center().y - 12.0), Vec2::splat(24.0));
            let url = super::images::user_avatar_url(guild_id, u, None, 48, false);
            super::sidebar::user_avatar_at(ui, cx.images, &url, avatar, false, 1.0, (u.id, &name));
            let g = one_line(ui, name.as_str(), FontId::new(pal.body(), crate::theme::medium_family()), pal.text, rect.right() - avatar.right() - 16.0);
            ui.painter().galley(pos2(avatar.right() + 8.0, rect.center().y - g.size().y / 2.0), g, pal.text);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                cx.view.open_profile(u.id, guild_id, rect.right_top());
                ui.close();
            }
        }
    };
    if users.len() > 10 {
        egui::ScrollArea::vertical().id_salt("rc_poll_voters_list").max_height(320.0).auto_shrink([false, true]).show(ui, rows);
    } else {
        ui.scope(rows);
    }
    if loading && users.is_empty() {
        ui.label(RichText::new("Loading…").size(pal.meta()).color(pal.muted));
    }
    if let Some(e) = error {
        ui.label(RichText::new(format!("Could not load the voters: {e}")).size(pal.meta()).color(pal.danger));
        if small_button(ui, pal, "Retry", pal.text).clicked() {
            fetch(cx, users.last().map(|u| u.id));
        }
    } else if more && !loading && button(ui, pal, Btn::Ghost, "Show more", true).clicked() {
        fetch(cx, users.last().map(|u| u.id));
    }
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
    let multi_id = Id(newest.0.0 + (2 << 22));
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
    // The message Discord posts when the third poll ends (type 46), pointing at it.
    let fields = [("poll_question_text", "Best editor?"), ("victor_answer_votes", "9"), ("total_votes", "20"), ("victor_answer_id", "2"), ("victor_answer_text", "Neovim")];
    let embed = Embed {
        kind: Some("poll_result".into()),
        fields: fields.iter().map(|(n, v)| EmbedField { name: (*n).into(), value: (*v).into(), inline: false }).collect(),
        ..Embed::default()
    };
    list.msgs.push(Message {
        id: Id(newest.0.0 + (4 << 22)),
        channel_id: ch,
        guild_id,
        author,
        kind: 46,
        timestamp: newest.1.clone(),
        embeds: vec![embed],
        message_reference: Some(MessageReference { message_id: Some(Id(newest.0.0 + (3 << 22))), channel_id: Some(ch), guild_id, kind: 0 }),
        ..Message::default()
    });
    // `poll-voters`: the voters of "Polls" in the second poll, from the cached users.
    if demo("poll-voters") {
        let mut users: Vec<_> = cx.state.users.values().cloned().collect();
        users.sort_by_key(|u| u.id);
        users.truncate(6);
        cx.view.polls_ui.voters = Some((multi_id, 1, true));
        cx.state.chat.poll_voters = Some(PollVotersView { channel_id: ch, message_id: multi_id, answer_id: 1, users, ..Default::default() });
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

    fn embed(fields: &[(&str, &str)]) -> Embed {
        Embed { fields: fields.iter().map(|(n, v)| EmbedField { name: (*n).into(), value: (*v).into(), inline: false }).collect(), ..Embed::default() }
    }

    #[test]
    fn result_summary_winner_tie_and_no_votes() {
        let win = embed(&[("poll_question_text", "Best?"), ("victor_answer_votes", "9"), ("total_votes", "20"), ("victor_answer_id", "2"), ("victor_answer_text", "Neovim")]);
        assert_eq!(result_summary(&win), Summary { question: "Best?".into(), winner: Some("Neovim".into()), votes: 9, total: 20 });
        // An emoji-only answer shows its emoji name.
        let emoji = embed(&[("victor_answer_id", "1"), ("victor_answer_emoji_name", "🎉"), ("victor_answer_votes", "3"), ("total_votes", "3")]);
        assert_eq!(result_summary(&emoji).winner.as_deref(), Some("🎉"));
        // No winner id: a tie.
        let tie = embed(&[("poll_question_text", "Best?"), ("victor_answer_votes", "5"), ("total_votes", "10")]);
        assert_eq!(result_summary(&tie), Summary { question: "Best?".into(), winner: None, votes: 5, total: 10 });
        // Nothing voted, or fields that are not numbers.
        let none = embed(&[("poll_question_text", "Best?"), ("total_votes", "0")]);
        assert_eq!((result_summary(&none).total, result_summary(&none).winner), (0, None));
        assert_eq!(result_summary(&embed(&[("total_votes", "many")])).total, 0);
    }

    #[test]
    fn draft_trims_drops_empty_answers_and_caps_at_ten() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(draft("  ", &a(&["x", "y"]), 24, false).is_none(), "a blank question");
        assert!(draft("Q?", &a(&["", "  "]), 24, false).is_none(), "no answers");
        let d = draft("  Q?  ", &a(&[" x ", "", "y", "  "]), 72, true).unwrap();
        assert_eq!((d.question.as_str(), d.answers.as_slice(), d.hours, d.multi), ("Q?", &a(&["x", "y"])[..], 72, true));
        let many: Vec<String> = (0..14).map(|i| format!("a{i}")).collect();
        let d = draft("Q?", &many, 24, false).unwrap();
        assert_eq!((d.answers.len(), d.answers[9].as_str()), (10, "a9"));
    }

    #[test]
    fn posting_ends_in_a_closed_dialog_or_an_error() {
        let mut v = ViewState::default();
        v.polls_ui.question = "Q?".into();
        v.polls_ui.busy = true;
        v.modal = Some(Modal::CreatePoll { channel_id: Id(5) });
        // Failed with the dialog open: the error shows in it, the draft stays.
        posted(&mut v, Id(5), Err("Forbidden".into()));
        assert_eq!((v.polls_ui.busy, v.polls_ui.question.as_str()), (false, "Q?"));
        assert_eq!(v.polls_ui.error.as_deref(), Some("Could not post the poll: Forbidden"));
        assert!(v.modal.is_some() && v.toasts.is_empty());
        // Failed after it closed: a toast.
        v.modal = None;
        posted(&mut v, Id(5), Err("Forbidden".into()));
        assert_eq!(v.toasts.len(), 1);
        // Posted: the draft is reset and the dialog closes.
        v.modal = Some(Modal::CreatePoll { channel_id: Id(5) });
        posted(&mut v, Id(5), Ok(()));
        assert!(v.modal.is_none() && v.polls_ui.question.is_empty() && v.polls_ui.error.is_none());
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
