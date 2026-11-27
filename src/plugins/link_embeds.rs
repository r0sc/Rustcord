//! MessageLinkEmbeds (PLUGINS_NOTES S17): a compact preview under a message for each message
//! link in it (at most five). From the local cache only: a linked message that is not loaded
//! shows "Load message", which fetches it with one GET on that click (`FetchAround` with limit 1,
//! through the REST limiter) and keeps it (or the failure) for the session. Never fetches on its own; links
//! to channels I can't view show nothing.

use super::{Plugin, enabled};
use crate::backend::{ChatCommand, ChatEvent, Command};
use crate::model::{Id, Message};
use crate::state::AppState;
use crate::ui::UiCx;
use crate::ui::markdown::MdOpts;
use egui::{CornerRadius, Frame, Label, Margin, RichText, Sense, vec2};
use std::sync::Mutex;

const MAX_LINKS: usize = 5;
/// Fetched messages (and failures) kept, the oldest dropped first.
const MAX_CACHED: usize = 200;

enum Entry {
    Loading,
    Done(Box<Message>),
    Failed,
    /// Discord answered without that message (deleted).
    Gone,
}

static CACHE: Mutex<Vec<(Id, Entry)>> = Mutex::new(Vec::new());
/// Messages whose "Load message" answer (limit 1) has not come yet.
static ASKED: Mutex<Vec<Id>> = Mutex::new(Vec::new());

fn put(mid: Id, e: Entry) {
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    c.retain(|(m, _)| *m != mid);
    c.push((mid, e));
    if c.len() > MAX_CACHED {
        c.remove(0);
    }
}

/// (channel, message) of each message link in `text`, in order, without repeats. Links in `<…>`
/// (embeds suppressed by the sender) are skipped.
pub fn links(text: &str) -> Vec<(Id, Id)> {
    let mut out = Vec::new();
    for token in text.split_whitespace().filter(|t| !t.starts_with('<')) {
        let mut rest = token;
        while let Some(i) = rest.find("http") {
            rest = &rest[i..];
            if let Some(l) = parse(rest).filter(|l| !out.contains(l)) {
                out.push(l);
            }
            rest = &rest[4..];
        }
    }
    out.truncate(MAX_LINKS);
    out
}

/// `http(s)://[sub.]discord[app].com/channels/<guild|@me>/<channel>/<message>…`
fn parse(url: &str) -> Option<(Id, Id)> {
    let url = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = url.split_once('/')?;
    let sub_ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    let host_ok = ["discord.com", "discordapp.com"]
        .iter()
        .any(|b| host == *b || host.strip_suffix(b).and_then(|s| s.strip_suffix('.')).is_some_and(sub_ok));
    if !host_ok {
        return None;
    }
    let mut seg = path.strip_prefix("channels/")?.splitn(4, '/');
    let guild = seg.next()?;
    if guild != "@me" {
        snowflake(guild)?;
    }
    let channel = snowflake(seg.next()?)?;
    let m = seg.next()?;
    let message = snowflake(&m[..m.find(|c: char| !c.is_ascii_digit()).unwrap_or(m.len())])?;
    Some((channel, message))
}

fn snowflake(s: &str) -> Option<Id> {
    if !(17..=20).contains(&s.len()) || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok().map(Id)
}

/// What a preview shows: the message from the loaded history or this plugin's cache (also the
/// Inbox's Saved tab, which asks through `ask`).
pub(crate) enum Look {
    Message(Box<Message>),
    Loading,
    Failed,
    Gone,
    Unknown,
}

pub(crate) fn look(state: &AppState, ch: Id, mid: Id) -> Look {
    let find = |msgs: &[Message]| msgs.binary_search_by_key(&mid, |m| m.id).ok().map(|i| msgs[i].clone());
    let jumped = state.chat.around.as_ref().filter(|v| v.channel_id == ch).and_then(|v| v.result.as_ref()?.as_ref().ok());
    if let Some(m) = state.messages.get(&ch).and_then(|c| find(&c.msgs)).or_else(|| jumped.and_then(|p| find(p))) {
        return Look::Message(Box::new(m));
    }
    let c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    match c.iter().find(|(m, _)| *m == mid).map(|(_, e)| e) {
        Some(Entry::Done(m)) => Look::Message(m.clone()),
        Some(Entry::Loading) => Look::Loading,
        Some(Entry::Failed) => Look::Failed,
        Some(Entry::Gone) => Look::Gone,
        None => Look::Unknown,
    }
}

/// Under a message's content (`messages.rs` body): one preview per message link. Returns the
/// linked message whose preview was clicked (the caller jumps to it).
pub fn show(ui: &mut egui::Ui, cx: &mut UiCx<'_>, msg: &Message) -> Option<(Id, Id)> {
    if !enabled(Plugin::MessageLinkEmbeds) || !msg.content.contains("/channels/") {
        return None;
    }
    let mut jump = None;
    for (ch, mid) in links(&msg.content).into_iter().filter(|&(_, m)| m != msg.id) {
        if preview(ui, cx, msg.id, ch, mid) {
            jump = Some((ch, mid));
        }
    }
    jump
}

/// One preview card; true when clicked.
fn preview(ui: &mut egui::Ui, cx: &mut UiCx<'_>, host: Id, ch: Id, mid: Id) -> bool {
    let look = look(cx.state, ch, mid);
    let viewable = cx.state.channels.contains_key(&ch) && cx.state.can_view(ch);
    if !viewable && matches!(look, Look::Unknown) {
        return false;
    }
    let c = &cx.theme.colors;
    let (text, muted, bar, surface, size) = (c.text.to_color32(), c.text_muted.to_color32(), cx.theme.tokens().edge, c.surface.to_color32(), cx.theme.font.size);
    let radius = (cx.theme.layout.corner_radius.round().clamp(0.0, 60.0) as u8).saturating_mul(2);
    let place = cx.state.channels.get(&ch).map(|x| if x.guild_id.is_some() { format!("#{}", cx.state.channel_name(ch)) } else { cx.state.channel_name(ch) });
    let mut load = false;
    let shown = ui.scope(|ui| {
        ui.set_max_width(ui.available_width().min(520.0));
        Frame::new()
            .fill(surface)
            .corner_radius(radius)
            .inner_margin(Margin { left: 14, right: 12, top: 8, bottom: 8 })
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                match &look {
                    Look::Message(m) => {
                        ui.horizontal(|ui| {
                            let url = crate::ui::images::user_avatar_url(m.guild_id, &m.author, m.member.as_ref(), 32, false);
                            let name = cx.state.member_display_name(m.guild_id, &m.author, m.member.as_ref());
                            // The letter avatar of message headers until (or unless) the picture loads
                            // (20 px: smaller ones are a plain dot, `paint_initial`).
                            crate::ui::images::avatar(ui, cx.images, &url, 20.0, false, Some((m.author.id, &name)));
                            ui.add(Label::new(RichText::new(name).font(crate::ui::bold(size - 1.0)).color(text)).truncate());
                            if let Some(p) = &place {
                                ui.add(Label::new(RichText::new(format!("in {p}")).size(size - 2.0).color(muted)).truncate());
                            }
                        });
                        // Formatted like the message itself (first lines only, `summary`).
                        let opts = MdOpts { guild_id: m.guild_id, mentions: &m.mentions, id_salt: m.id, color: None, highlight: &[], bg: None };
                        crate::ui::markdown::render(ui, cx, &summary(m), &opts);
                    }
                    Look::Loading => {
                        ui.label(RichText::new("Loading message…").color(muted));
                    }
                    Look::Failed => {
                        ui.label(RichText::new("This message could not be loaded.").color(muted));
                    }
                    Look::Gone => {
                        ui.label(RichText::new("This message is no longer available.").color(muted));
                    }
                    Look::Unknown => {
                        ui.horizontal(|ui| {
                            // As tall as the button (a row starts at interact_size.y), so the caption centres on it.
                            ui.set_min_height(ui.text_style_height(&egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.y);
                            ui.label(RichText::new(format!("A message in {}", place.as_deref().unwrap_or("a channel"))).color(muted));
                            load = ui.button("Load message").on_hover_text("Loads this one message").clicked();
                        });
                    }
                }
            })
            .response
    });
    let rect = shown.inner.rect;
    ui.painter().rect_filled(egui::Rect::from_min_size(rect.min, vec2(4.0, rect.height())), CornerRadius { nw: radius, sw: radius, ne: 0, se: 0 }, bar);
    if load {
        // The one request, on this click.
        ask(mid);
        cx.cmd.send(Command::Chat(ChatCommand::FetchAround { channel_id: ch, message_id: mid, limit: 1 }));
        return false;
    }
    if !matches!(look, Look::Message(_)) {
        return false;
    }
    let resp = ui.interact(rect, ui.make_persistent_id(("rc_link_embed", host, mid)), Sense::click());
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Jump to message").clicked()
}

/// The text shown for a linked message: its first lines, or what it holds instead.
fn summary(m: &Message) -> String {
    let content = m.content.trim();
    if content.is_empty() {
        let n = m.attachments.len() + m.embeds.len() + m.sticker_items.len();
        return if n == 0 { "[no content]".into() } else { format!("[no content, {n} attachment{}]", if n == 1 { "" } else { "s" }) };
    }
    let short: String = content.lines().take(3).collect::<Vec<_>>().join("\n").chars().take(300).collect();
    if short.len() < content.len() { format!("{short}…") } else { short }
}

pub(crate) fn ask(mid: Id) {
    put(mid, Entry::Loading);
    ASKED.lock().unwrap_or_else(|e| e.into_inner()).push(mid);
}

/// An answer for a message the plugin asked for goes into the cache. A jump to the same message
/// asks too, and its answer can't be told apart from the plugin's: the jump view gets the event
/// only while it still waits, so a 1-message answer never replaces a loaded jump window.
pub fn on_chat(state: &AppState, ev: ChatEvent) -> Option<ChatEvent> {
    let ChatEvent::AroundLoaded { channel_id, message_id, result } = &ev else { return Some(ev) };
    {
        let mut asked = ASKED.lock().unwrap_or_else(|e| e.into_inner());
        let Some(i) = asked.iter().position(|m| m == message_id) else { return Some(ev) };
        // A jump's page is up to 50; the plugin's own answer is at most 1 (or an error).
        if result.as_ref().map_or(true, |p| p.len() <= 1) {
            asked.remove(i);
        }
    }
    match result.as_ref().ok().and_then(|page| page.iter().find(|m| m.id == *message_id)) {
        Some(m) => put(*message_id, Entry::Done(Box::new(m.clone()))),
        // One of the two answers failing doesn't hide what the other one loaded.
        None if !CACHE.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|(m, e)| m == message_id && matches!(e, Entry::Done(_))) => {
            put(*message_id, if result.is_ok() { Entry::Gone } else { Entry::Failed });
        }
        None => {}
    }
    let waiting = state.chat.around.as_ref().is_some_and(|v| (v.channel_id, v.message_id) == (*channel_id, *message_id) && v.result.is_none());
    waiting.then_some(ev)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_link_parsing() {
        let (g, c, m) = ("112233445566778899", "223344556677889900", "334455667788990011");
        let link = format!("https://discord.com/channels/{g}/{c}/{m}");
        let ids = (Id(c.parse().unwrap()), Id(m.parse().unwrap()));
        assert_eq!(links(&link), [ids]);
        // Subdomains, discordapp, @me, http, trailing text, markdown links.
        assert_eq!(links(&format!("see https://ptb.discord.com/channels/{g}/{c}/{m}).")), [ids]);
        assert_eq!(links(&format!("[x](http://discordapp.com/channels/@me/{c}/{m}?a=1)")), [ids]);
        // Duplicates once, suppressed `<…>` skipped, channel links and wrong hosts ignored.
        assert_eq!(links(&format!("{link} {link} <{link}>")), [ids]);
        assert!(links(&format!("https://discord.com/channels/{g}/{c}")).is_empty());
        assert!(links(&format!("https://evil.com/discord.com/channels/{g}/{c}/{m}")).is_empty());
        assert!(links(&format!("https://notdiscord.com/channels/{g}/{c}/{m}")).is_empty());
        assert!(links(&format!("https://discord.com/channels/{g}/12/{m}")).is_empty());
        // At most five.
        let many: Vec<String> = (0..7).map(|i| format!("https://discord.com/channels/@me/{c}/33445566778899001{i}")).collect();
        assert_eq!(links(&many.join(" ")).len(), MAX_LINKS);
    }

    #[test]
    fn fetch_answers_go_to_the_cache() {
        let mid = Id(4242);
        // Not asked by the plugin: the event goes on to the state.
        let ev = || ChatEvent::AroundLoaded { channel_id: Id(1), message_id: mid, result: Ok(vec![Message { id: mid, content: "hi".into(), ..Default::default() }]) };
        assert!(on_chat(&AppState::default(), ev()).is_some());
        ask(mid);
        assert!(on_chat(&AppState::default(), ev()).is_none());
        assert!(matches!(look(&AppState::default(), Id(1), mid), Look::Message(m) if m.content == "hi"));
        let page = || (4240..4245).map(|id| Message { id: Id(id), ..Default::default() }).collect::<Vec<_>>();
        let around = |m, result| ChatEvent::AroundLoaded { channel_id: Id(1), message_id: Id(m), result };
        // A jump's page with it, then the plugin's own answer failing: the message stays.
        ask(Id(4243));
        assert!(on_chat(&AppState::default(), around(4243, Ok(page()))).is_none());
        assert!(on_chat(&AppState::default(), around(4243, Err("x".into()))).is_none());
        assert!(matches!(look(&AppState::default(), Id(1), Id(4243)), Look::Message(_)));
        assert!(on_chat(&AppState::default(), around(4243, Ok(page()))).is_some(), "answered: no longer the plugin's");
        // An answer without the message is Gone, an error Failed.
        ask(Id(4250));
        assert!(on_chat(&AppState::default(), around(4250, Ok(Vec::new()))).is_none());
        assert!(matches!(look(&AppState::default(), Id(1), Id(4250)), Look::Gone));
        ask(Id(4251));
        assert!(on_chat(&AppState::default(), around(4251, Err("x".into()))).is_none());
        assert!(matches!(look(&AppState::default(), Id(1), Id(4251)), Look::Failed));
        // A jump to it still waiting gets the answer; a loaded jump window is never replaced.
        let mut s = AppState::default();
        s.chat.around = Some(crate::state::chat::AroundView { channel_id: Id(1), message_id: mid, result: None });
        ask(mid);
        assert!(on_chat(&s, ev()).is_some());
        s.chat.around.as_mut().unwrap().result = Some(Ok(page()));
        ask(mid);
        assert!(on_chat(&s, ev()).is_none());
        // And a message shown in the jump window needs no fetch.
        assert!(matches!(look(&s, Id(1), Id(4243)), Look::Message(_)));
        assert_eq!(summary(&Message::default()), "[no content]");
        assert_eq!(summary(&Message { content: "a\nb\nc\nd".into(), ..Default::default() }), "a\nb\nc…");
    }
}
