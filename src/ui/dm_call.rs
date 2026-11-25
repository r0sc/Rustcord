//! DM / group DM calls in the chat (VOICE_NOTES §1.4, laid out like the web client's DM call):
//! the header's Start Voice Call / Start Video Call buttons (`header_buttons`), the call area
//! above the messages (`banner`: who is in the call with Join buttons while I'm not, the call
//! stage of call.rs while I am), the DM list's in-call mark (`row_mark`), the incoming-call
//! popup (`incoming`; Ctrl+Enter accepts) and `logic`, which sends the ring and the camera a
//! call I started is due for (`AppState::call_steps`). Every command comes from a click: a new
//! call rings once, nothing accepts, declines or rings again on its own. The desktop
//! notification comes from `AppState::apply_gateway` (CALL_* starting to ring me), so Do Not
//! Disturb silences it like a message; the popup shows anyway.
//! Test hook `RUSTCORD_OPEN=call:<dm id>` / `ring:<dm id>` / `outgoing:<dm id>` (`demo`).
//! OWNERSHIP: dm-calls engineer.

use super::messages::Pal;
use super::icons::{self, Icon};
use super::sidebar::user_avatar_at;
use super::{UiCx, bold, call, one_line, presence_color};
use crate::backend::{Command, CommandSender};
use crate::model::{Channel, Id, Presence, VoiceState};
use crate::state::AppState;
use crate::voice::video::{VideoCommand, capture};
use egui::{Align2, Color32, CursorIcon, Rect, Role, Sense, Stroke, Vec2, WidgetInfo, pos2, vec2};
use std::time::{Duration, Instant};

/// The dark stage of the call view (call.rs), in every theme.
const STAGE: Color32 = Color32::from_rgb(14, 12, 12);
const BUTTON: Color32 = Color32::from_rgb(56, 51, 50);
const FG: Color32 = Color32::from_gray(238);
const FG_MUTED: Color32 = Color32::from_gray(170);
/// The call area while I'm not in the call: a row of avatars over the Join buttons.
const BANNER_H: f32 = 176.0;
const AVATAR: f32 = 64.0;
/// One pulse of the ring around the caller (incoming) or the people being rung (call.rs).
pub(super) const PULSE_S: f64 = 1.4;

/// The open channel is a DM / group DM: my call there stays above its chat (`banner`) instead of
/// the full call page (`main_layout`).
pub fn in_chat(state: &AppState) -> bool {
    state.selected_channel.and_then(|c| state.channels.get(&c)).is_some_and(Channel::is_dm_like)
}

/// App::logic, once per frame: the ring / camera a call I started is due for.
pub fn logic(state: &mut AppState, cmd: &CommandSender, camera: &Option<String>) {
    let steps = state.call_steps(Instant::now());
    if let Some((channel_id, check_ringable)) = steps.ring {
        cmd.send(Command::RingCall { channel_id, recipients: None, check_ringable });
    }
    if steps.camera {
        cmd.send(Command::Video(VideoCommand::StartCamera { device: camera.clone() }));
    }
}

/// A click on call / video call / Join / Accept in `ch`.
fn join(cx: &mut UiCx<'_>, ch: Id, video: bool) {
    cx.state.start_call(ch, video, Instant::now());
    cx.cmd.send(Command::JoinVoice { guild_id: None, channel_id: ch });
}

/// Accept, or Start Voice / Video Call in a user's menu (friends.rs): opens the DM over
/// whatever page is up, then `join` (one click, one ring).
pub(super) fn open_and_join(cx: &mut UiCx<'_>, ch: Id, video: bool) {
    join(cx, ch, video);
    cx.state.select_channel(ch);
    (cx.view.settings_open, cx.view.server_settings, cx.view.profile_card, cx.view.modal) = (false, None, None, None);
}

fn header_shown(state: &AppState, ch: Id) -> bool {
    state.channels.get(&ch).is_some_and(Channel::is_dm_like) && state.voice.channel_id != Some(ch)
}

/// Width of `header_buttons` in the chat header of `ch`.
pub(super) fn header_w(state: &AppState, ch: Id) -> f32 {
    if header_shown(state, ch) { 80.0 } else { 0.0 }
}

/// DM chat header, in its right-to-left tool row after Pins: Start Video Call, then Start Voice
/// Call ("Join …" while a call is going on there). Hidden while I'm in it: the stage has the
/// controls.
pub(super) fn header_buttons(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, ch: Id) {
    if !header_shown(cx.state, ch) {
        return;
    }
    let verb = if cx.state.call_in(ch) { "Join" } else { "Start" };
    for (video, icon) in [(true, Icon::Camera), (false, Icon::Phone)] {
        // Linux / macOS: no camera yet, so no video call (disabled with the reason).
        let enabled = !video || capture::SUPPORTED;
        let tip = if enabled { format!("{verb} {} Call", if video { "Video" } else { "Voice" }) } else { capture::NO_VIDEO_CALL.to_owned() };
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(32.0), if enabled { Sense::click() } else { Sense::hover() });
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, &tip));
        let hot = enabled && (resp.hovered() || resp.has_focus());
        if hot {
            ui.painter().rect_filled(rect, pal.radius, pal.tint());
        }
        let (color, bg) = if hot { (pal.text, pal.bg.blend(pal.tint())) } else { (pal.muted.gamma_multiply(if enabled { 1.0 } else { 0.4 }), pal.bg) };
        icons::paint(ui.painter(), icon, Rect::from_center_size(rect.center(), Vec2::splat(20.0)), color, bg);
        if resp.on_hover_text(tip.as_str()).on_hover_cursor(if enabled { CursorIcon::PointingHand } else { CursorIcon::Default }).clicked() && enabled {
            join(cx, ch, video);
        }
    }
}

/// Above a DM's messages (`messages::chat`): while I'm in its call, the call stage (tiles and
/// controls with Disconnect, call.rs) over half the page; while others are, who is in it (and
/// who is being rung, dimmed) with Join Call / Join with Video.
pub(super) fn banner(ui: &mut egui::Ui, cx: &mut UiCx<'_>, ch: Id) {
    if !cx.state.channels.get(&ch).is_some_and(Channel::is_dm_like) {
        return;
    }
    let mine = cx.state.voice.channel_id == Some(ch);
    if !mine && !cx.state.call_in(ch) {
        return;
    }
    // Half the chat, but the composer (placed first) keeps about two message rows above it.
    let h = if mine { (ui.max_rect().height() * 0.5).min(ui.available_height() - 96.0).clamp(120.0, 480.0) } else { BANNER_H };
    // 12 px of page under the stage: the first date divider doesn't touch it.
    let frame = egui::Frame::new().fill(STAGE).outer_margin(egui::Margin { bottom: 12, ..egui::Margin::ZERO });
    egui::Panel::top("rc_dm_call")
        .exact_size(h)
        .resizable(false)
        .show_separator_line(false)
        .frame(frame)
        .show(ui, |ui| if mine { call::stage(ui, cx, ch) } else { waiting(ui, cx, ch) });
}

/// One avatar of the call area.
struct Person {
    id: Id,
    name: String,
    avatar: Option<String>,
    /// False: being rung.
    in_call: bool,
    muted: bool,
    deaf: bool,
    by_server: bool,
    speaking: bool,
}

fn people(s: &AppState, ch: Id) -> Vec<Person> {
    let states = s.voice_states.get(&ch);
    let person = |id: Id, v: Option<&VoiceState>| {
        let user = s.users.get(&id).or(s.me.as_ref().filter(|u| u.id == id));
        let flag = |f: fn(&VoiceState) -> bool| v.is_some_and(f);
        Person {
            id,
            name: user.map_or_else(|| "Unknown user".into(), |u| u.display_name().to_owned()),
            avatar: user.map(|u| super::images::user_avatar_url(None, u, None, 128, false)),
            in_call: v.is_some(),
            muted: flag(|v| v.self_mute || v.mute),
            deaf: flag(|v| v.self_deaf || v.deaf),
            by_server: flag(|v| v.mute || v.deaf),
            speaking: s.voice.speaking.contains(&id),
        }
    };
    let mut out: Vec<Person> = states.into_iter().flatten().map(|(u, v)| person(*u, Some(v))).collect();
    let rung = s.calls.ringing.get(&ch).into_iter().flatten().filter(|u| !states.is_some_and(|m| m.contains_key(u)));
    out.extend(rung.map(|u| person(*u, None)));
    out.sort_by_cached_key(|p| (!p.in_call, p.name.to_lowercase(), p.id));
    out
}

/// The call area while I'm not in the call.
fn waiting(ui: &mut egui::Ui, cx: &mut UiCx<'_>, ch: Id) {
    let rect = ui.max_rect();
    let people = people(cx.state, ch);
    let (green, red) = (presence_color(Presence::Online, &cx.theme.colors), cx.theme.colors.danger.to_color32());
    let step = AVATAR + 16.0;
    let fit = (((rect.width() - 32.0 + 16.0) / step).floor() as usize).max(1);
    // Too many for the row: the last slot says how many more.
    let shown = if people.len() > fit { fit - 1 } else { people.len() };
    let slots = shown + usize::from(people.len() > shown);
    let cy = rect.top() + 24.0 + AVATAR / 2.0;
    let mut x = rect.center().x - (slots as f32 * step - 16.0) / 2.0 + AVATAR / 2.0;
    for p in &people[..shown] {
        let a = Rect::from_center_size(pos2(x, cy), Vec2::splat(AVATAR));
        x += step;
        let opacity = if p.in_call { 1.0 } else { 0.4 };
        match &p.avatar {
            Some(url) => user_avatar_at(ui, cx.images, url, a, false, opacity, (p.id, &p.name)),
            None => super::images::paint_initial(ui.painter(), a, AVATAR / 2.0, p.id.0, &p.name, true),
        }
        let painter = ui.painter();
        let speak = super::motion::anim(ui.ctx(), egui::Id::unique(("rc_speak_dm", p.id)), p.speaking, 0.0, super::motion::FAST);
        if speak > 0.0 {
            painter.circle_stroke(a.center(), AVATAR / 2.0 + 3.0, Stroke::new(2.5, green.gamma_multiply(speak)));
        }
        if p.muted || p.deaf {
            // 24 px badge, cut out of the avatar by a 2 px ring of the stage.
            let c = a.right_bottom() - Vec2::splat(10.0);
            painter.circle_filled(c, 14.0, STAGE);
            painter.circle_filled(c, 12.0, BUTTON);
            let icon = if p.deaf { Icon::Deafened } else { Icon::MicOff };
            icons::paint(painter, icon, Rect::from_center_size(c, Vec2::splat(16.0)), if p.by_server { red } else { FG }, BUTTON);
        }
        let state = match (p.in_call, p.deaf, p.muted) {
            (false, ..) => " (ringing)",
            (_, true, _) => " (deafened)",
            (_, _, true) => " (muted)",
            _ => "",
        };
        let what = format!("{}{state}", p.name);
        let resp = ui.interact(a, ui.make_persistent_id(("dm_call_person", p.id)), Sense::click());
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, &what));
        if resp.on_hover_text(what.as_str()).on_hover_cursor(CursorIcon::PointingHand).clicked() {
            cx.view.open_profile(p.id, None, pos2(a.right() + 8.0, a.top()));
        }
    }
    if slots > shown {
        let a = Rect::from_center_size(pos2(x, cy), Vec2::splat(AVATAR));
        ui.painter().circle_filled(a.center(), AVATAR / 2.0, BUTTON);
        let more = format!("+{}", people.len() - shown);
        ui.painter().text(a.center(), Align2::CENTER_CENTER, more, bold(18.0), FG);
    }
    // Join call (`positive_fill`, its label: white on the presence green fails 4.5:1) and Join
    // with video, centred under the avatars.
    let tk = cx.theme.tokens();
    let (w1, w2, gap) = (112.0, 136.0, 8.0);
    let left = rect.center().x - (w1 + w2 + gap) / 2.0;
    let top = cy + AVATAR / 2.0 + 20.0;
    let buttons = [(false, "Join call", w1, tk.positive_fill, tk.positive_label), (true, "Join with video", w2, BUTTON, FG)];
    let mut x = left;
    for (video, label, w, fill, fg) in buttons {
        let r = Rect::from_min_size(pos2(x, top), vec2(w, 36.0));
        x += w + gap;
        let text = egui::RichText::new(label).color(fg).font(egui::FontId::new(cx.theme.font.size - 1.0, crate::theme::medium_family()));
        let button = egui::Button::new(text).fill(fill).corner_radius(cx.theme.layout.corner_radius);
        // Linux / macOS: no camera yet (disabled with the reason).
        let resp = ui.add_enabled_ui(!video || capture::SUPPORTED, |ui| ui.put(r, button)).inner.on_disabled_hover_text(capture::NO_VIDEO_CALL);
        if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
            join(cx, ch, video);
        }
    }
}

/// The DM list's mark for a call going on in `ch`: a green handset right-aligned at `right`
/// (vertically centred); returns the width it takes (0 without a call).
pub(super) fn row_mark(ui: &mut egui::Ui, cx: &UiCx<'_>, ch: Id, right: egui::Pos2) -> f32 {
    if !cx.state.call_in(ch) {
        return 0.0;
    }
    let r = Rect::from_min_size(pos2(right.x - 16.0, right.y - 8.0), Vec2::splat(16.0));
    icons::paint(ui.painter(), Icon::Phone, r, presence_color(Presence::Online, &cx.theme.colors), Color32::TRANSPARENT);
    let n = cx.state.voice_states.get(&ch).map_or(0, |m| m.len());
    let tip = if n == 1 { "1 person in a call".to_owned() } else { format!("{n} people in a call") };
    ui.interact(r, ui.make_persistent_id(("dm_call_mark", ch)), Sense::hover()).on_hover_text(tip);
    20.0
}

/// The incoming-call popup (above everything, centred): the caller's avatar in a pulsing
/// ring, the DM's name, Decline (danger fill, stops ringing me), Accept with video and Accept (positive fill,
/// Ctrl+Enter). Accepting joins the call and opens the DM.
/// `accept`: the Answer keybind was pressed (taken by `ui()` before the page could see it).
pub(super) fn incoming(ctx: &egui::Context, cx: &mut UiCx<'_>, accept: bool) {
    let Some(ch) = cx.state.incoming_call() else { return };
    let s = &*cx.state;
    let name = s.channel_name(ch);
    let caller = s.caller(ch).and_then(|u| s.users.get(&u)).map(|u| (u.id, u.display_name().to_owned(), super::images::user_avatar_url(None, u, None, 128, false)));
    let green = presence_color(Presence::Online, &cx.theme.colors);
    // The buttons: `danger_fill` / `positive_fill` under the white icons (§2.2).
    let tk = cx.theme.tokens();
    // Some(Some(video)) = accept, Some(None) = decline.
    let mut action = accept.then_some(Some(false));
    let card = egui::Frame::popup(&ctx.global_style()).fill(STAGE).corner_radius(12).inner_margin(24);
    // Reduced motion: the ring stands still and nothing repaints.
    let still = super::motion::secs(ctx, 1.0) == 0.0;
    egui::Area::new(egui::Id::unique("rc_incoming_call"))
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, -40.0))
        .show(ctx, |ui| {
            card.show(ui, |ui| {
                ui.set_width(200.0);
                ui.vertical_centered(|ui| {
                    let (slot, _) = ui.allocate_exact_size(Vec2::splat(112.0), Sense::hover());
                    let phase = if still { 0.5 } else { (ui.input(|i| i.time) % PULSE_S / PULSE_S) as f32 };
                    let ring = 40.0 + 14.0 * phase;
                    ui.painter().circle_stroke(slot.center(), ring, Stroke::new(3.0, green.gamma_multiply(1.0 - phase)));
                    let a = Rect::from_center_size(slot.center(), Vec2::splat(80.0));
                    match &caller {
                        Some((id, who, url)) => user_avatar_at(ui, cx.images, url, a, false, 1.0, (*id, who)),
                        None => super::images::paint_initial(ui.painter(), a, 40.0, ch.0, &name, true),
                    }
                    ui.add_space(8.0);
                    let g = one_line(ui, name.as_str(), bold(cx.theme.font.size + 3.0), FG, 200.0);
                    ui.add(egui::Label::new(g).selectable(false));
                    ui.label(egui::RichText::new("Incoming call…").color(FG_MUTED).size(cx.theme.font.size - 1.0));
                    ui.add_space(20.0);
                    let (row, _) = ui.allocate_exact_size(vec2(176.0, 48.0), Sense::hover());
                    let buttons = [(None, tk.danger_fill, "Decline", Icon::Hangup), (Some(true), tk.positive_fill, "Accept with video", Icon::Camera), (Some(false), tk.positive_fill, "Accept", Icon::Phone)];
                    // Linux / macOS: no camera yet, so no Accept with video; the rest centred.
                    let buttons: Vec<_> = buttons.into_iter().filter(|b| capture::SUPPORTED || b.0 != Some(true)).collect();
                    let left = row.left() + (row.width() - (buttons.len() as f32 * 64.0 - 16.0)) / 2.0;
                    for (i, (what, fill, tip, icon)) in buttons.into_iter().enumerate() {
                        let r = Rect::from_min_size(pos2(left + i as f32 * 64.0, row.top()), Vec2::splat(48.0));
                        if round_button(ui, r, fill, tip, icon).clicked() {
                            action = Some(what);
                        }
                    }
                });
            });
        });
    // The pulse: a few frames a second while the popup is up (no repaint otherwise).
    if !still {
        ctx.request_repaint_after(Duration::from_millis(33));
    }
    match action {
        Some(Some(video)) => open_and_join(cx, ch, video),
        Some(None) => {
            cx.state.decline_call(ch);
            cx.cmd.send(Command::StopRinging { channel_id: ch, recipients: None });
        }
        None => {}
    }
}

/// A round button filled with `fill`, the icon in white; the tooltip is the accessible name.
fn round_button(ui: &mut egui::Ui, rect: Rect, fill: Color32, tip: &str, icon: Icon) -> egui::Response {
    let resp = ui.interact(rect, ui.make_persistent_id(("dm_call_button", tip)), Sense::click());
    let rect = super::motion::pressed(&resp, rect);
    let hot = resp.hovered() || resp.has_focus();
    let fill = if hot { fill.lerp_to_gamma(Color32::WHITE, 0.15) } else { fill };
    ui.painter().circle_filled(rect.center(), rect.width() / 2.0, fill);
    icons::paint(ui.painter(), icon, Rect::from_center_size(rect.center(), Vec2::splat(rect.width() * 0.5)), Color32::WHITE, fill);
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, tip));
    resp.on_hover_text(tip).on_hover_cursor(CursorIcon::PointingHand)
}

/// Test hook `RUSTCORD_OPEN=call:<dm id>`: a call going on in that DM / group DM with its other
/// members in it (the second muted, the third deafened); `ring:<dm id>`: only its first member
/// is in it and it rings me; `outgoing:<dm id>`: the call I just started there (I'm connected,
/// the DM is open), ringing all its other members. True when `flag` was one of them. No request.
pub fn demo(state: &mut AppState, flag: &str) -> bool {
    let (kind, id) = match flag.split_once(':') {
        Some((kind @ ("call" | "ring" | "outgoing"), id)) => (kind, id),
        _ => return false,
    };
    let Some(ch) = id.parse().ok().map(Id) else { return true };
    let Some(mut members) = state.channels.get(&ch).map(|c| c.recipient_ids.clone()) else { return true };
    if kind == "outgoing" {
        let me = state.me.as_ref().map(|u| u.id);
        (state.voice.channel_id, state.voice.connected) = (Some(ch), true);
        let vs = me.map(|user_id| VoiceState { channel_id: Some(ch), user_id, ..Default::default() });
        state.voice_states.entry(ch).or_default().extend(vs.map(|v| (v.user_id, v)));
        state.calls.ringing.insert(ch, members);
        state.select_channel(ch);
        return true;
    }
    let ring = kind == "ring";
    members.truncate(if ring { 1 } else { members.len() });
    for (i, user_id) in members.into_iter().enumerate() {
        let vs = VoiceState { channel_id: Some(ch), user_id, self_mute: i == 1, self_deaf: i == 2, ..Default::default() };
        state.voice_states.entry(ch).or_default().insert(user_id, vs);
    }
    let me = state.me.as_ref().map(|u| u.id).filter(|_| ring);
    state.calls.ringing.insert(ch, me.into_iter().collect());
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{GatewayEvent, User, channel_type};

    #[test]
    fn demo_hook_rings_me_from_the_first_member() {
        let mut s = AppState::default();
        s.me = Some(User { id: Id(1), ..Default::default() });
        let group = Channel { id: Id(10), kind: channel_type::GROUP_DM, recipient_ids: vec![Id(2), Id(3)], ..Default::default() };
        s.apply_gateway(GatewayEvent::ChannelCreate(Box::new(group)), None);
        assert!(!demo(&mut s, "voice:10"));
        assert!(demo(&mut s, "ring:10"));
        assert_eq!((s.incoming_call(), s.caller(Id(10))), (Some(Id(10)), Some(Id(2))));
        assert!(demo(&mut s, "call:10"));
        assert_eq!(s.incoming_call(), None);
        assert_eq!(s.voice_states[&Id(10)].len(), 2);
        assert!(s.voice_states[&Id(10)][&Id(3)].self_mute);
        // Outgoing: only me in it, both others rung, nothing ringing me.
        s.voice_states.clear();
        assert!(demo(&mut s, "outgoing:10"));
        assert_eq!((s.voice.channel_id, s.selected_channel, s.incoming_call()), (Some(Id(10)), Some(Id(10)), None));
        assert_eq!(s.voice_states[&Id(10)].keys().collect::<Vec<_>>(), [&Id(1)]);
        assert_eq!(s.calls.ringing[&Id(10)], [Id(2), Id(3)]);
    }
}
