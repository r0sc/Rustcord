//! Message requests page (FEATURES_NOTES §2.4): the DM home's "Message requests" entry. A 49 px
//! header with the Requests / Spam segments, then a list of 62 px rows (tick, avatar, name, the
//! trigger message as a preview, Accept and Ignore as round buttons). Previews are fetched a
//! page of 25 at a time, only on a click (`fetch`). Accept and a single Ignore send at once; a
//! batch Ignore asks first (`ask_ignore`). Opening a row shows the DM read-only with a bar in
//! place of the composer (`bar`). Test hook: `demo`.
//!
//! OWNERSHIP: navigation-social (docs/SOCIAL_UI_DESIGN.md).

use super::friends::{self, Action};
use super::icons::{self, Icon};
use super::messages::{self, Btn, Pal};
use super::sidebar::{self, empty_state};
use super::{UiCx, ViewState, format_timestamp, one_line};
use crate::backend::{Command, MAX_REJECT_BATCH, SocialCommand};
use crate::model::{Channel, GatewayEvent, Id, Message, User, channel_type};
use crate::state::AppState;
use crate::state::social::RequestPreviews;
use egui::{Align, FontId, Frame, Layout, Margin, Rect, RichText, Sense, Shape, Stroke, Vec2, pos2, vec2};

const ROW_H: f32 = 62.0;

/// The page's own state (`ViewState::requests_ui`).
#[derive(Debug, Default)]
pub struct RequestsUi {
    /// The Message requests entry is selected: the page, or a request's DM.
    pub open: bool,
    /// The Spam segment.
    pub spam: bool,
    /// Rows ticked for a batch Ignore.
    pub ticked: Vec<Id>,
}

/// Once per pass: the entry stays selected on the page and on a request's DM, anything else
/// resets it (and drops the previews).
pub fn sync(state: &mut AppState, view: &mut ViewState) {
    let in_req = state.selected_channel.is_some_and(|c| state.is_message_request(c));
    if in_req {
        view.requests_ui.open = true;
    }
    if !(view.requests_ui.open && (friends::is_open(state) || in_req)) {
        view.requests_ui = Default::default();
        state.social.request_previews = None;
    }
}

/// The DM list's "Message requests" row: the page, and its first previews.
pub fn open(cx: &mut UiCx<'_>) {
    friends::open(cx.state);
    cx.view.quests_ui.open = false;
    cx.view.settings_open = false;
    cx.view.requests_ui.open = true;
    cx.state.social.request_previews.get_or_insert_with(Default::default);
    fetch(cx);
}

/// Asks for the next page of previews. Only from a click: the row, a segment, "Load more
/// previews", "Retry".
fn fetch(cx: &mut UiCx<'_>) {
    if let Some(channel_ids) = cx.state.begin_request_previews(cx.view.requests_ui.spam) {
        cx.cmd.send(Command::Social(SocialCommand::FetchRequestPreviews { channel_ids }));
    }
}

pub(super) fn accept(cx: &mut UiCx<'_>, channel_id: Id) {
    if cx.state.begin_request_action(&[channel_id]) {
        cx.cmd.send(Command::Social(SocialCommand::AcceptMessageRequest { channel_id }));
    }
}

/// One Ignore needs no question (as Discord and the Friends page).
pub(super) fn ignore(cx: &mut UiCx<'_>, channel_id: Id) {
    if cx.state.begin_request_action(&[channel_id]) {
        cx.cmd.send(Command::Social(SocialCommand::RejectMessageRequests { channel_ids: vec![channel_id] }));
    }
}

/// The batch Ignore question; the command goes out only on its button.
fn ask_ignore(view: &mut ViewState, ids: Vec<Id>) {
    let title = if let [_] = ids[..] { "Ignore 1 message request?".to_owned() } else { format!("Ignore {} message requests?", ids.len()) };
    view.confirm(title, "Ignored requests are deleted from your list.", "Ignore", Command::Social(SocialCommand::RejectMessageRequests { channel_ids: ids }));
}

/// The page, in place of the chat.
pub fn show(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    let pal = Pal::new(cx.theme);
    header(ui, cx, &pal);
    let spam = cx.view.requests_ui.spam;
    let ids = cx.state.message_requests(spam);
    cx.view.requests_ui.ticked.retain(|i| ids.contains(i));
    if ids.is_empty() {
        let text = if spam {
            "No spam. Messages filed as spam show up here."
        } else {
            "No message requests. When someone who isn't your friend messages you, it shows up here."
        };
        empty_state(ui, cx, 48.0, text);
        return;
    }
    egui::ScrollArea::vertical().id_salt(("rc_requests", spam)).auto_shrink(false).show(ui, |ui| {
        Frame::new().inner_margin(Margin { left: 20, right: 20, top: 16, bottom: 16 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            // The role label left, the count right-aligned in meta (docs/DESIGN.md §10.4).
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::hover());
            let n = one_line(ui, ids.len().to_string(), FontId::proportional(pal.meta()), pal.muted, row.width());
            let label = if spam { "Spam" } else { "Requests" };
            let g = one_line(ui, label, super::semibold(pal.label()), pal.muted, row.width() - n.size().x - 28.0);
            ui.painter().galley(pos2(row.right() - 10.0 - n.size().x, row.center().y - n.size().y / 2.0), n, pal.muted);
            ui.painter().galley(pos2(row.left() + 10.0, row.center().y - g.size().y / 2.0), g, pal.muted);
            for &id in &ids {
                request_row(ui, cx, &pal, id);
            }
            footer(ui, cx, &pal, spam);
        });
    });
}

fn header(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal) {
    let header = egui::Panel::top("rc_requests_header")
        .exact_size(49.0)
        .resizable(false)
        .show_separator_line(false)
        .frame(Frame::new().fill(pal.bg).inner_margin(Margin { left: 16, right: 12, top: 0, bottom: 0 }))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if ui.available_width() > 560.0 {
                    let (icon, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                    icons::paint(ui.painter(), Icon::Envelope, icon, pal.muted, pal.bg);
                    ui.label(RichText::new("Message requests").font(super::semibold(pal.heading())).color(pal.text));
                    let (r, _) = ui.allocate_exact_size(vec2(1.0, 20.0), Sense::hover());
                    ui.painter().vline(r.center().x, r.y_range(), Stroke::new(1.0, pal.tk.rule));
                }
                // One segmented control on the well colour, as on the Friends page.
                let track_slot = ui.painter().add(Shape::Noop);
                let mut track = Rect::NOTHING;
                let spam = cx.view.requests_ui.spam;
                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for (is_spam, label) in [(false, "Requests"), (true, "Spam")] {
                        let resp = friends::tab_button(ui, cx, pal, label, spam == is_spam, 0, None);
                        track = track.union(resp.rect);
                        if resp.clicked() && spam != is_spam {
                            cx.view.requests_ui.spam = is_spam;
                            cx.view.requests_ui.ticked.clear();
                            fetch(cx);
                        }
                    }
                });
                let well = cx.theme.colors.code_background.to_color32();
                ui.painter().set(track_slot, Shape::rect_filled(track.expand(2.0), pal.radius, well));
                let n = cx.view.requests_ui.ticked.len();
                if n > 0 {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if messages::button(ui, pal, Btn::Secondary, &format!("Ignore {n} selected"), true).clicked() {
                            let ids = cx.view.requests_ui.ticked.clone();
                            ask_ignore(cx.view, ids);
                        }
                    });
                }
            });
        });
    let r = header.response.rect;
    ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, pal.tk.rule));
}

/// Under the list: the failed page's error with Retry, else "Load more previews".
fn footer(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, spam: bool) {
    let error = cx.state.social.request_previews.as_ref().and_then(|v| v.error.clone());
    if let Some(e) = error {
        ui.add_space(12.0);
        ui.label(RichText::new(format!("Could not load the previews: {e}")).font(FontId::proportional(pal.meta())).color(pal.danger));
        ui.add_space(8.0);
        if messages::button(ui, pal, Btn::Secondary, "Retry", true).clicked() {
            fetch(cx);
        }
    } else if cx.state.more_request_previews(spam) {
        ui.add_space(12.0);
        if messages::button(ui, pal, Btn::Secondary, "Load more previews", true).clicked() {
            fetch(cx);
        }
    }
}

/// One request. Registered back to front: the row, then the tick, then the buttons (the later
/// ones win the hit test, and the Tab order follows).
fn request_row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, id: Id) {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    if !ui.is_rect_visible(row) {
        return;
    }
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(row));
    let ui = &mut ui;
    if cx.state.social.request_busy.contains(&id) {
        ui.multiply_opacity(0.5);
    }
    let name = cx.state.channel_name(id);
    let preview = cx.state.request_preview(id).map(|(text, iso)| (text, format_timestamp(iso)));
    let a11y = match &preview {
        Some((text, _)) => format!("{name}: {text}"),
        None => name.clone(),
    };
    let hot = ui.rect_contains_pointer(row);
    let r = f32::from(pal.card_radius());
    if hot {
        ui.painter().rect_filled(row, pal.card_radius(), pal.bg.blend(pal.tint()));
    } else {
        ui.painter().hline(row.x_range().shrink(10.0), row.top() + 0.5, Stroke::new(1.0, pal.tk.rule));
    }

    // 1. The row: open the DM.
    let resp = ui.interact(row, ui.make_persistent_id(("rc_request", id)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &a11y));
    if resp.has_focus() {
        messages::focus_ring(ui, pal, row.shrink(4.0), r - 4.0);
    }
    if resp.clicked() {
        cx.state.select_channel(id);
        cx.view.settings_open = false;
    }

    // 2. The tick.
    let on = cx.view.requests_ui.ticked.contains(&id);
    let enabled = on || cx.view.requests_ui.ticked.len() < MAX_REJECT_BATCH;
    let tick = Rect::from_center_size(pos2(row.left() + 20.0, row.center().y), Vec2::splat(20.0));
    let tick_resp = ui.interact(Rect::from_center_size(tick.center(), Vec2::splat(32.0)), ui.make_persistent_id(("rc_request_tick", id)), Sense::click());
    tick_resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, enabled, on, format!("Select {name}")));
    let chip = f32::from(pal.radius / 2);
    messages::paint_checkbox(ui.painter(), &pal.tk, super::motion::pressed(&tick_resp, tick), on, chip);
    if tick_resp.has_focus() {
        messages::focus_ring(ui, pal, tick, chip + 2.0);
    }
    if tick_resp.clicked() && enabled {
        let t = &mut cx.view.requests_ui.ticked;
        if on {
            t.retain(|&i| i != id);
        } else {
            t.push(id);
        }
    }
    if !enabled {
        tick_resp.on_hover_text("You can ignore up to 50 at once.");
    }

    // 3. Accept left of Ignore (36 px circles, 10 px apart), registered left to right.
    let right = row.right() - 10.0 - 92.0;
    for (action, tip, slot) in [(Action::Accept, "Accept", 1.0), (Action::Remove, "Ignore", 0.0)] {
        let rect = Rect::from_center_size(pos2(row.right() - 10.0 - 18.0 - 46.0 * slot, row.center().y), Vec2::splat(36.0));
        let b = friends::round_button(ui, cx, pal, (id, rect), action, tip, hot);
        b.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{tip} {name}'s message request")));
        if b.clicked() {
            if action == Action::Accept { accept(cx, id) } else { ignore(cx, id) }
        }
    }

    // Avatar and text.
    let avatar = Rect::from_min_size(pos2(tick.right() + 12.0, row.center().y - 16.0), Vec2::splat(32.0));
    sidebar::dm_avatar_at(ui, cx, id, avatar);
    let text_left = avatar.right() + 12.0;
    let max_w = right - 8.0 - text_left;
    let name_g = one_line(ui, name.as_str(), FontId::new(pal.body(), crate::theme::medium_family()), pal.text, max_w);
    let line2 = match &preview {
        Some((text, _)) => text.clone(),
        None if cx.state.social.request_previews.as_ref().is_some_and(|v| v.loading && v.asked.contains(&id)) => "Loading\u{2026}".to_owned(),
        None => "Open to read the message.".to_owned(),
    };
    let sub_g = one_line(ui, line2, FontId::proportional(pal.meta()), pal.muted, max_w);
    let top = row.center().y - (name_g.size().y + sub_g.size().y) / 2.0;
    let (name_w, name_h) = (name_g.size().x, name_g.size().y);
    let sub_y = top + name_h;
    ui.painter().galley(pos2(text_left, top), name_g, pal.text);
    if let Some((_, when)) = preview.filter(|(_, w)| !w.is_empty()) {
        let left = text_left + name_w + 8.0;
        let g = one_line(ui, when, FontId::proportional(pal.meta()), pal.muted, right - 8.0 - left);
        ui.painter().galley(pos2(left, top + (name_h - g.size().y) / 2.0), g, pal.muted);
    }
    ui.painter().galley(pos2(text_left, sub_y), sub_g, pal.muted);
}

/// In place of the composer under a request's DM: who it is from, Ignore and Accept (the
/// view's one primary). Both are disabled while an answer is on its way.
pub(super) fn bar(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, ch: Id) {
    let name = cx.state.channel_name(ch);
    let spam = cx.state.channels.get(&ch).is_some_and(|c| c.is_spam);
    let busy = cx.state.social.request_busy.contains(&ch);
    let frame = Frame::new().fill(pal.surface).corner_radius(pal.card_radius()).inner_margin(Margin { left: 16, right: 12, top: 12, bottom: 12 });
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        let narrow = ui.available_width() < 480.0;
        let hint = if spam { "Filed as spam. Be careful with links and files." } else { "Accept to reply. Ignoring deletes the request." };
        let text = |ui: &mut egui::Ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.add(egui::Label::new(RichText::new(format!("Message request from {name}")).font(FontId::new(pal.body(), crate::theme::medium_family())).color(pal.text)).truncate());
                ui.add(egui::Label::new(RichText::new(hint).font(FontId::proportional(pal.meta())).color(pal.muted)).truncate());
            });
        };
        let buttons = |ui: &mut egui::Ui, cx: &mut UiCx<'_>| {
            ui.spacing_mut().item_spacing.x = 12.0;
            if messages::button(ui, pal, Btn::Primary, "Accept", !busy).clicked() {
                accept(cx, ch);
            }
            if messages::button(ui, pal, Btn::Secondary, "Ignore", !busy).clicked() {
                ignore(cx, ch);
            }
        };
        if narrow {
            text(ui);
            ui.add_space(8.0);
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 32.0), Layout::right_to_left(Align::Center), |ui| buttons(ui, cx));
        } else {
            ui.horizontal(|ui| {
                text(ui);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| buttons(ui, cx));
            });
        }
    });
}

// ---------------------------------------------------------------------------
// Test hook
// ---------------------------------------------------------------------------

/// Test hook `RUSTCORD_OPEN=requests[:spam|ticked|confirm|open|spam-open]`: three invented
/// requests and one spam DM with their previews, on the page; `spam` the Spam segment,
/// `ticked` two rows ticked, `confirm` the batch question, `open` / `spam-open` a request's DM
/// with the bar. Sends nothing. False for other tokens.
pub fn demo(state: &mut AppState, view: &mut ViewState, token: &str) -> bool {
    let Some(rest) = token.strip_prefix("requests") else { return false };
    let what = rest.trim_start_matches(':');
    if !rest.is_empty() && !rest.starts_with(':') {
        return false;
    }
    let now = chrono::Utc::now();
    let now_ms = now.timestamp_millis().unsigned_abs();
    // (name, minutes ago, spam, text)
    let people = [
        ("Wren Hollis", 50, false, "Hi! We met in the voice chat yesterday. Do you still have the link to that allocator talk?"),
        ("Tomasz Krol", 5, false, "Your bug report on the tile renderer helped a lot, thanks. Mind if I ask a follow-up?"),
        ("Mira Santos", 190, false, "Are you still selling the mechanical keyboard from your post?"),
        ("Prize Desk", 70, true, "Congratulations! You won a $50 gift card. Claim it in the next 24 hours: example.invalid/claim"),
    ];
    let mut view_previews = RequestPreviews::default();
    let base = 9_000_000_000_000_000_000_u64;
    for (i, (name, minutes, spam, text)) in people.into_iter().enumerate() {
        let (user_id, channel_id) = (Id(base + 100 + i as u64), Id(base + 1 + i as u64));
        let user = User { id: user_id, username: name.to_lowercase().replace(' ', "_"), global_name: Some(name.into()), ..Default::default() };
        state.users.insert(user_id, user.clone());
        let at = now_ms - minutes * 60_000;
        let message = Message {
            id: Id::from_timestamp_ms(at),
            channel_id,
            author: user,
            content: text.into(),
            timestamp: (now - chrono::Duration::minutes(minutes as i64)).to_rfc3339(),
            ..Default::default()
        };
        let channel = Channel {
            id: channel_id,
            kind: channel_type::DM,
            recipient_ids: vec![user_id],
            last_message_id: Some(message.id),
            is_message_request: true,
            is_spam: spam,
            ..Default::default()
        };
        state.apply_gateway(GatewayEvent::ChannelCreate(Box::new(channel)), None);
        view_previews.asked.insert(channel_id);
        view_previews.messages.insert(channel_id, message);
    }
    state.social.request_previews = Some(view_previews);
    friends::open(state);
    view.quests_ui.open = false;
    view.settings_open = false;
    view.requests_ui = RequestsUi { open: true, spam: what.starts_with("spam"), ticked: Vec::new() };
    let first_two = || state.message_requests(false).into_iter().take(2).collect::<Vec<_>>();
    match what {
        "ticked" => view.requests_ui.ticked = first_two(),
        "confirm" => {
            let ids = first_two();
            view.requests_ui.ticked = ids.clone();
            ask_ignore(view, ids);
        }
        "open" | "spam-open" => {
            let c = state.message_requests(what == "spam-open")[0];
            let m = state.social.request_previews.as_ref().and_then(|v| v.messages.get(&c)).cloned().expect("demo preview");
            let second = Message { id: Id(m.id.0 + 1), content: "Sorry, I know this is out of the blue.".into(), ..m.clone() };
            state.apply_messages_loaded(c, None, Ok(vec![second, m]), 50);
            state.select_channel(c);
        }
        _ => {}
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dm(id: u64, request: bool) -> Channel {
        Channel { id: Id(id), kind: channel_type::DM, recipient_ids: vec![Id(3)], is_message_request: request, ..Default::default() }
    }

    #[test]
    fn sync_keeps_the_page_and_resets_elsewhere() {
        let mut s = AppState::default();
        let mut v = ViewState::default();
        for ch in [dm(201, true), dm(202, false)] {
            s.apply_gateway(GatewayEvent::ChannelCreate(Box::new(ch)), None);
        }
        s.social.request_previews = Some(Default::default());
        v.requests_ui = RequestsUi { open: true, spam: true, ticked: vec![Id(201)] };
        sync(&mut s, &mut v);
        assert!(v.requests_ui.open && s.social.request_previews.is_some(), "the DM home keeps it");
        s.select_channel(Id(201));
        sync(&mut s, &mut v);
        assert!(v.requests_ui.open && v.requests_ui.ticked == [Id(201)], "so does an open request");
        s.select_channel(Id(202));
        sync(&mut s, &mut v);
        assert!(!v.requests_ui.open && v.requests_ui.ticked.is_empty() && !v.requests_ui.spam);
        assert!(s.social.request_previews.is_none());
        s.select_channel(Id(201));
        sync(&mut s, &mut v);
        assert!(v.requests_ui.open, "selecting a request sets it");
    }

    #[test]
    fn demo_fills_the_page() {
        let mut s = AppState::default();
        let mut v = ViewState::default();
        assert!(!demo(&mut s, &mut v, "requestsx") && !demo(&mut s, &mut v, "quests"));
        assert!(demo(&mut s, &mut v, "requests"));
        assert_eq!(s.request_counts(), (3, 1));
        let first = s.message_requests(false)[0];
        assert!(s.request_preview(first).is_some() && v.requests_ui.open && s.social.request_previews.as_ref().unwrap().asked.len() == 4);
    }
}
