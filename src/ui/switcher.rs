//! The quick switcher (Ctrl+K): jump to a server, channel, thread, group DM or person from one
//! field (`@` people, `#` text channels, `!` voice channels, `*` servers). Local state only, every
//! pick goes through the path a click already uses; no request of its own (docs/FEATURES_NOTES.md
//! §4.9). It opens and closes at once, without motion (ux-motion principle 20: used all day).
//! OWNERSHIP: navigation (docs/ARCHITECTURE.md).

use super::icons::{self, Icon};
use super::messages::{self, Pal};
use super::user_settings::{Bind, shortcut};
use super::{Modal, TITLE_BAR_H, UiCx, ViewState, sidebar};
use crate::model::{Id, channel_type as ct, relationship_type};
use crate::state::AppState;
use egui::{Key, Modifiers, Rect, Sense, Stroke, vec2};

/// What a result row opens.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Target {
    Server(Id),
    /// A guild channel, thread or group DM.
    Channel(Id),
    /// A friend or the recipient of a 1:1 DM.
    Person(Id),
}

/// The switcher's draft: the typed text and its results.
#[derive(Default)]
pub struct SwitcherUi {
    query: String,
    sel: usize,
    hits: Vec<Target>,
    for_query: Option<String>,
    focus: bool,
}

/// Opens the switcher with `query` typed in (Ctrl+K: ""; test hook RUSTCORD_OPEN=switcher[:<query>]).
pub(super) fn open(view: &mut ViewState, query: &str) {
    view.switcher_ui = SwitcherUi { query: query.into(), focus: true, ..Default::default() };
    view.modal = Some(Modal::QuickSwitcher);
}

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

/// Consumes every press of `k` this pass, repeats too (a focused text field never sees them);
/// true when one was a new press: one press, one step, however long the key is held.
fn fresh(ctx: &egui::Context, k: &egui::KeyboardShortcut) -> bool {
    ctx.input_mut(|i| {
        let mut new = false;
        i.events.retain(|e| match e {
            egui::Event::Key { key, pressed: true, repeat, modifiers, .. } if *key == k.logical_key && modifiers.matches_logically(k.modifiers) => {
                new |= !repeat;
                false
            }
            _ => true,
        });
        new
    })
}

const KEYS: [Bind; 1] = [Bind::QuickSwitcher];

/// The switcher's shortcuts (called from `user_settings::app::global_keys`, main screen only).
pub(super) fn keys(ctx: &egui::Context, cx: &mut UiCx<'_>) {
    let open = cx.view.modal == Some(Modal::QuickSwitcher);
    if cx.view.chat_hidden() || cx.view.editing.is_some() || messages::overlay_open(ctx, cx) || (cx.view.dialog_open() && !open) {
        return;
    }
    // Most modifiers first: egui ignores extra Shift/Alt, so Alt+Shift+Up also matches Alt+Up.
    let mut binds: Vec<(Bind, egui::KeyboardShortcut)> = KEYS.iter().filter_map(|&b| Some((b, shortcut(cx.settings, b)?))).collect();
    binds.sort_by_key(|(_, k)| std::cmp::Reverse(u8::from(k.modifiers.command || k.modifiers.ctrl) + u8::from(k.modifiers.alt) + u8::from(k.modifiers.shift)));
    for (b, k) in binds {
        // While the switcher is open only its own key (close) counts.
        if (open && b != Bind::QuickSwitcher) || !fresh(ctx, &k) {
            continue;
        }
        act(cx, b, open);
        return;
    }
}

fn act(cx: &mut UiCx<'_>, b: Bind, open: bool) {
    match b {
        Bind::QuickSwitcher if open => cx.view.modal = None,
        Bind::QuickSwitcher => self::open(cx.view, ""),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Filter {
    Any,
    People,
    Text,
    Voice,
    Servers,
}

/// The prefix filter and the rest of the query, trimmed.
fn split(q: &str) -> (Filter, &str) {
    let q = q.trim_start();
    let (f, rest) = match q.chars().next() {
        Some('@') => (Filter::People, &q[1..]),
        Some('#') => (Filter::Text, &q[1..]),
        Some('!') => (Filter::Voice, &q[1..]),
        Some('*') => (Filter::Servers, &q[1..]),
        _ => (Filter::Any, q),
    };
    (f, rest.trim())
}

/// How well `name` matches `q` (already lowercase): exact, prefix, word start, substring,
/// subsequence (the tighter the better); None for no match.
fn score(name: &str, q: &str) -> Option<u32> {
    let name = name.to_lowercase();
    if name == q {
        return Some(4000);
    }
    if name.starts_with(q) {
        return Some(3000);
    }
    let mut prev = None::<char>;
    let starts = name.char_indices().any(|(i, c)| {
        let word = !prev.is_some_and(char::is_alphanumeric);
        prev = Some(c);
        word && name[i..].starts_with(q)
    });
    if starts {
        return Some(2000);
    }
    if name.contains(q) {
        return Some(1000);
    }
    let mut want = q.chars().peekable();
    let (mut first, mut last) = (None, 0);
    for (i, c) in name.chars().enumerate() {
        if want.peek() == Some(&c) {
            want.next();
            first.get_or_insert(i);
            last = i;
            if want.peek().is_none() {
                break;
            }
        }
    }
    let first = first.filter(|_| want.peek().is_none())?;
    let slack = (last - first + 1).saturating_sub(q.chars().count());
    Some(500 - slack.min(499) as u32)
}

/// A 1:1 DM is shown as its person.
fn target_of(s: &AppState, id: Id) -> Target {
    match s.channels.get(&id) {
        Some(c) if c.kind == ct::DM && !c.recipient_ids.is_empty() => Target::Person(c.recipient_ids[0]),
        _ => Target::Channel(id),
    }
}

/// Position in the recent channels (smaller is newer); `usize::MAX` for none.
fn recency(s: &AppState, t: Target) -> usize {
    let at = |id: Id| s.recent_channels.iter().position(|&c| c == id);
    match t {
        Target::Channel(c) => at(c),
        Target::Person(u) => s.dm_with(u).and_then(at),
        Target::Server(g) => s.recent_channels.iter().position(|c| s.channels.get(c).is_some_and(|c| c.guild_id == Some(g))),
    }
    .unwrap_or(usize::MAX)
}

fn search(s: &AppState, query: &str) -> Vec<Target> {
    let (filter, rest) = split(query);
    if rest.is_empty() && filter == Filter::Any {
        let mut out: Vec<Target> = Vec::new();
        for id in s.recent_channels.iter().copied().chain(s.unread_channels()) {
            if Some(id) == s.selected_channel || !s.channels.contains_key(&id) || !s.can_view(id) {
                continue;
            }
            let t = target_of(s, id);
            if !out.contains(&t) {
                out.push(t);
            }
        }
        out.truncate(20);
        return out;
    }
    let q = rest.to_lowercase();
    // Without text after a prefix every candidate of that kind is listed (score 0).
    let rate = |name: &str| if q.is_empty() { Some(0) } else { score(name, &q) };
    let mut found: Vec<(u32, Target, String)> = Vec::new();
    if matches!(filter, Filter::Any | Filter::Servers) {
        for g in s.guilds.iter().filter(|g| !g.unavailable && !g.name.is_empty()) {
            if let Some(sc) = rate(&g.name) {
                found.push((sc, Target::Server(g.id), g.name.clone()));
            }
        }
    }
    for c in s.channels.values().filter(|c| c.guild_id.is_some()) {
        let text = matches!(c.kind, ct::GUILD_TEXT | ct::GUILD_ANNOUNCEMENT | ct::GUILD_FORUM | ct::GUILD_MEDIA)
            || (c.is_thread() && c.member.is_some() && !c.is_archived());
        let voice = matches!(c.kind, ct::GUILD_VOICE | ct::GUILD_STAGE_VOICE);
        if !(text && matches!(filter, Filter::Any | Filter::Text) || voice && matches!(filter, Filter::Any | Filter::Voice)) {
            continue;
        }
        let name = c.name.clone().unwrap_or_default();
        // The permission check is the expensive part: only for channels that matched.
        if let Some(sc) = rate(&name).filter(|_| s.can_view(c.id)) {
            found.push((sc, Target::Channel(c.id), name));
        }
    }
    let dms = || s.dm_order.iter().filter_map(|id| s.channels.get(id)).filter(|c| !s.is_message_request(c.id));
    if filter == Filter::Any {
        for c in dms().filter(|c| c.kind == ct::GROUP_DM) {
            let name = s.channel_name(c.id);
            if let Some(sc) = rate(&name) {
                found.push((sc, Target::Channel(c.id), name));
            }
        }
    }
    if matches!(filter, Filter::Any | Filter::People) {
        let friends = s.social.relationships.values().filter(|r| r.kind == relationship_type::FRIEND).map(|r| r.id);
        let dm_people = dms().filter(|c| c.kind == ct::DM).filter_map(|c| c.recipient_ids.first().copied());
        let mut people: Vec<Id> = friends.chain(dm_people).collect();
        people.sort_by_key(|u| u.0);
        people.dedup();
        for u in people.into_iter().filter(|u| s.me.as_ref().is_none_or(|m| m.id != *u)) {
            let Some(user) = s.users.get(&u) else { continue };
            let names = [s.friend_name(u), user.username.clone(), user.global_name.clone().unwrap_or_default()];
            if let Some(sc) = names.iter().filter(|n| !n.is_empty()).filter_map(|n| rate(n)).max() {
                found.push((sc, Target::Person(u), s.friend_name(u)));
            }
        }
    }
    found.sort_by_cached_key(|(sc, t, name)| (std::cmp::Reverse(*sc), recency(s, *t), name.chars().count(), name.to_lowercase()));
    found.into_iter().take(50).map(|(_, t, _)| t).collect()
}

// ---------------------------------------------------------------------------
// The modal
// ---------------------------------------------------------------------------

/// What a row shows: name, kind word, context (server / username), unread and mentions.
struct Info {
    name: String,
    kind: &'static str,
    context: Option<String>,
    unread: bool,
    mentions: u32,
}

fn describe(s: &AppState, t: Target) -> Info {
    let unread_of = |id: Id| (s.is_unread(id), s.mention_count(id));
    match t {
        Target::Server(g) => {
            let name = s.guild(g).map(|g| g.name.clone()).unwrap_or_default();
            Info { name, kind: "server", context: None, unread: false, mentions: 0 }
        }
        Target::Channel(id) => {
            let ch = s.channels.get(&id);
            let kind = match ch.map_or(0, |c| c.kind) {
                _ if ch.is_some_and(|c| c.is_thread()) => "thread",
                ct::GUILD_VOICE | ct::GUILD_STAGE_VOICE => "voice channel",
                ct::GUILD_FORUM | ct::GUILD_MEDIA => "forum",
                ct::GROUP_DM => "group conversation",
                _ => "text channel",
            };
            let context = ch.and_then(|c| c.guild_id).and_then(|g| s.guild(g)).map(|g| g.name.clone());
            let (unread, mentions) = unread_of(id);
            Info { name: s.channel_name(id), kind, context, unread, mentions }
        }
        Target::Person(u) => {
            let name = s.friend_name(u);
            let context = s.users.get(&u).map(|x| x.username.clone()).filter(|n| *n != name);
            let (unread, mentions) = s.dm_with(u).map_or((false, 0), unread_of);
            Info { name, kind: "person", context, unread, mentions }
        }
    }
}

/// The switcher. Takes its draft out of the view meanwhile (no borrow fights with the images).
pub(super) fn modal(ctx: &egui::Context, cx: &mut UiCx<'_>) {
    let mut st = std::mem::take(&mut cx.view.switcher_ui);
    let pal = Pal::new(cx.theme);
    let style = ctx.global_style();
    let id = egui::Id::unique("rc_quick_switcher");
    let area = egui::Modal::default_area(id).anchor(egui::Align2::CENTER_CENTER, vec2(0.0, TITLE_BAR_H / 2.0)).fade_in(false);
    // Opens and closes at once: no grow from `end_pass`, no ghost (ux-motion 20).
    super::motion::own(ctx, area.layer());
    let frame = egui::Frame::popup(&style).fill(pal.tk.raised).shadow(style.visuals.window_shadow).corner_radius(pal.modal_radius()).inner_margin(egui::Margin::same(16));
    let width = 520.0f32.min(ctx.content_rect().width() - 80.0).max(200.0);
    let rows = ((ctx.content_rect().height() - TITLE_BAR_H - 72.0 - 32.0 - 48.0 - 8.0 - 41.0) / 34.0).floor().clamp(3.0, 8.0);
    let mut pick = None;
    let resp = egui::Modal::new(id).area(area).frame(frame).backdrop_color(pal.tk.scrim).show(ctx, |ui| {
        ui.set_width(width - 32.0);
        let input_id = id.with("input");
        let mut moved = false;
        // Up / Down / Enter go to the list while the field has the focus (before it draws).
        if ui.memory(|m| m.has_focus(input_id)) && !st.hits.is_empty() {
            let n = st.hits.len();
            let (down, up, enter) = ui.input_mut(|i| (i.count_and_consume_key(Modifiers::NONE, Key::ArrowDown), i.count_and_consume_key(Modifiers::NONE, Key::ArrowUp), i.consume_key(Modifiers::NONE, Key::Enter)));
            if down + up > 0 {
                st.sel = (st.sel + down % n + n - up % n) % n;
                moved = true;
            }
            if enter {
                pick = st.hits.get(st.sel).copied();
            }
        }
        // Input: 48 tall, the focus ring in place of the border, a search icon in the left margin.
        let focused = ui.memory(|m| m.has_focus(input_id));
        let frame = egui::Frame::new()
            .fill(cx.theme.colors.code_background.to_color32())
            .corner_radius(pal.radius)
            .inner_margin(egui::Margin { left: if focused { 38 } else { 40 }, right: if focused { 10 } else { 12 }, top: 0, bottom: 0 })
            .stroke(if focused { Stroke::new(2.0, pal.tk.focus) } else { Stroke::NONE });
        let edit = egui::TextEdit::singleline(&mut st.query)
            .id(input_id)
            .hint_text("Jump to a channel, server or person")
            .font(egui::FontId::proportional(pal.title()))
            .frame(frame)
            .align(egui::Align2::LEFT_CENTER)
            .desired_width(f32::INFINITY);
        let input = ui.add_sized([ui.available_width(), 48.0], edit);
        let icon = Rect::from_center_size(egui::pos2(input.rect.left() + 22.0, input.rect.center().y), egui::Vec2::splat(20.0));
        icons::paint(ui.painter(), Icon::Search, icon, pal.muted, pal.tk.raised);
        if st.focus || ui.memory(|m| m.focused().is_none()) {
            input.request_focus();
            st.focus = false;
        }
        // The list matches the typed text in the same pass.
        if st.for_query.as_deref() != Some(&st.query) {
            st.hits = search(cx.state, &st.query);
            st.for_query = Some(st.query.clone());
            st.sel = 0;
        }
        st.sel = st.sel.min(st.hits.len().saturating_sub(1));

        ui.add_space(8.0);
        let area_h = rows * 32.0 + (rows - 1.0) * 2.0;
        ui.spacing_mut().item_spacing.y = 2.0;
        egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(area_h).min_scrolled_height(area_h).show(ui, |ui| {
            if st.hits.is_empty() {
                let text = if split(&st.query).1.is_empty() { "Type a name to jump there." } else { "No matches. Try fewer letters." };
                ui.centered_and_justified(|ui| ui.label(egui::RichText::new(text).color(pal.muted)));
                return;
            }
            let moved_pointer = ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO);
            for (i, &t) in st.hits.clone().iter().enumerate() {
                let resp = row(ui, cx, &pal, t, i == st.sel);
                if moved && i == st.sel {
                    resp.scroll_to_me(None);
                }
                if (resp.hovered() && moved_pointer) || resp.has_focus() {
                    st.sel = i;
                }
                if resp.clicked() {
                    pick = Some(t);
                }
            }
        });

        ui.add_space(8.0);
        let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
        ui.painter().rect_filled(line.expand2(vec2(16.0, 0.0)), 0.0, pal.tk.rule);
        ui.add_space(8.0);
        ui.add(egui::Label::new(egui::RichText::new("Start with @ for people, # for text channels, ! for voice channels or * for servers.").size(pal.meta()).color(pal.muted)).wrap());
    });
    cx.view.switcher_ui = st;
    if let Some(t) = pick {
        go(cx, t);
    } else if resp.should_close() {
        cx.view.modal = None;
    }
}

/// Opens a result like its click does.
fn go(cx: &mut UiCx<'_>, t: Target) {
    cx.view.modal = None;
    match t {
        Target::Server(g) => cx.state.select_guild(Some(g)),
        Target::Channel(c) => sidebar::open_channel(cx, c),
        Target::Person(u) => super::friends::message_user(cx, u),
    }
}

/// One result row: 32 tall, icon, name, then context and the mention badge on the right.
fn row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, t: Target, selected: bool) -> egui::Response {
    let resp = ui.allocate_response(vec2(ui.available_width(), 32.0), Sense::click());
    let rect = resp.rect;
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let info = describe(cx.state, t);
    let fill = if selected {
        pal.tk.selected
    } else if resp.hovered() {
        pal.tk.hover
    } else {
        egui::Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, pal.radius, fill);
    let icon = Rect::from_center_size(egui::pos2(rect.left() + 18.0, rect.center().y), egui::Vec2::splat(20.0));
    let icon_color = if selected { pal.text } else { pal.muted };
    match t {
        Target::Server(g) => {
            if let Some(i) = cx.state.guilds.iter().position(|x| x.id == g) {
                sidebar::paint_guild_icon(ui, cx, i, icon, f32::from(pal.radius) / 2.0, 0.0, false);
            }
        }
        Target::Channel(c) => match cx.state.channels.get(&c) {
            Some(ch) if ch.guild_id.is_some() => icons::paint_channel(ui.painter(), icons::of(ch, cx.state), icon, icon_color, pal.tk.raised.blend(fill)),
            _ => _ = sidebar::dm_avatar_at(ui, cx, c, icon),
        },
        Target::Person(u) => {
            if let Some(dm) = cx.state.dm_with(u) {
                sidebar::dm_avatar_at(ui, cx, dm, icon);
            } else if let Some(user) = cx.state.users.get(&u) {
                let url = super::images::user_avatar_url(None, user, None, 64, false);
                sidebar::user_avatar_at(ui, cx.images, &url, icon, false, 1.0, (u, &info.name));
            }
        }
    }
    let mut right = rect.right() - 8.0;
    if info.mentions > 0 {
        right -= sidebar::badge(ui, &cx.theme.colors, egui::pos2(right, rect.center().y), info.mentions, None) + 8.0;
    }
    if let Some(ctx_text) = info.context.as_deref() {
        let max = rect.width() * 0.4;
        let g = super::one_line(ui, ctx_text, egui::FontId::proportional(pal.meta()), pal.muted, max);
        let w = g.size().x;
        ui.painter().galley(egui::pos2(right - w, rect.center().y - g.size().y / 2.0), g, pal.muted);
        right -= w + 8.0;
    }
    let font = if info.unread { super::semibold(pal.size) } else { egui::FontId::new(pal.size, crate::theme::medium_family()) };
    let left = rect.left() + 36.0;
    let g = super::one_line(ui, info.name.as_str(), font, pal.text, right - left);
    ui.painter().galley(egui::pos2(left, rect.center().y - g.size().y / 2.0), g, pal.text);
    if resp.has_focus() {
        messages::focus_ring(ui, pal, rect, f32::from(pal.radius));
    }
    let mut label = format!("{}, {}", info.name, info.kind);
    if let Some(c) = info.context.as_deref().filter(|_| matches!(t, Target::Channel(_))) {
        label += &format!(", {c}");
    }
    if info.unread {
        label += ", unread";
    }
    if info.mentions > 0 {
        label += &format!(", {} mention{}", info.mentions, if info.mentions == 1 { "" } else { "s" });
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, &label));
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn id(n: u64) -> Id {
        Id(1565443437363200000 + n)
    }

    fn state() -> AppState {
        AppState::from_snapshot(serde_json::from_str(include_str!("../../tests/fixtures/home/cache/snapshot.json")).unwrap())
    }

    #[test]
    fn prefixes_split_off() {
        assert_eq!(split("@ana"), (Filter::People, "ana"));
        assert_eq!(split("# gen"), (Filter::Text, "gen"));
        assert_eq!(split("!"), (Filter::Voice, ""));
        assert_eq!(split("*rust"), (Filter::Servers, "rust"));
        assert_eq!(split(" rust "), (Filter::Any, "rust"));
    }

    #[test]
    fn scores_rank_exact_over_prefix_over_loose() {
        let s = |n| score(n, "gen");
        assert!(score("general", "general") > s("general"));
        assert!(s("general") > s("off-topic-gen"));
        assert!(s("off-topic-gen") > s("regenerate"));
        assert!(s("regenerate") > s("green"));
        assert!(s("green").is_some());
        assert_eq!(s("help"), None);
        assert_eq!(s("General"), s("general"));
    }

    #[test]
    fn search_finds_by_kind() {
        let s = state();
        assert_eq!(search(&s, "gen").first(), Some(&Target::Channel(id(10))));
        let text = search(&s, "#");
        assert!(!text.contains(&Target::Channel(id(14))), "no voice channel under #");
        assert_eq!(text.first(), Some(&Target::Channel(id(10))), "the recent one first");
        assert_eq!(search(&s, "!"), [Target::Channel(id(14))]);
        assert_eq!(search(&s, "*pix"), [Target::Server(id(15))]);
        let mut people = search(&s, "@");
        people.sort_by_key(|t| format!("{t:?}"));
        assert_eq!(people, [Target::Person(id(2)), Target::Person(id(3)), Target::Person(id(4))]);
        assert!(search(&s, "").iter().all(|t| *t != Target::Channel(id(10))), "never the open channel");
        assert!(search(&s, "zzzz").is_empty());
    }

    /// Headless passes of `keys` over the fixture.
    struct Harness {
        ctx: egui::Context,
        state: AppState,
        view: ViewState,
        images: super::super::images::ImageCache,
        theme: crate::theme::Theme,
        settings: crate::config::Settings,
        cmd: crate::backend::CommandSender,
        overrides: crate::overrides::LocalOverrides,
        rx: tokio::sync::mpsc::UnboundedReceiver<crate::backend::Command>,
    }

    impl Harness {
        fn new() -> Self {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let cmd = crate::backend::CommandSender(tx);
            let mut state = state();
            state.select_guild(Some(id(6)));
            state.select_channel(id(10));
            Self {
                ctx: egui::Context::default(),
                state,
                view: Default::default(),
                images: super::super::images::ImageCache::new(cmd.clone()),
                theme: crate::theme::Theme::ember_dark(),
                settings: Default::default(),
                cmd,
                overrides: Default::default(),
                rx,
            }
        }

        /// One pass with the key going down; a key still down from the pass before is a repeat
        /// (egui sets the flag itself).
        fn press(&mut self, mods: Modifiers, key: Key) {
            self.pass(vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: mods }]);
        }

        fn release(&mut self, mods: Modifiers, key: Key) {
            self.pass(vec![egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: mods }]);
        }

        fn pass(&mut self, events: Vec<egui::Event>) {
            let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 600.0))), events, ..Default::default() };
            let ctx = self.ctx.clone();
            let mut out = ctx.run_ui(raw, |ui| {
                let mut cx = UiCx {
                    state: &mut self.state,
                    view: &mut self.view,
                    images: &mut self.images,
                    theme: &mut self.theme,
                    settings: &mut self.settings,
                    cmd: &self.cmd,
                    overrides: &mut self.overrides,
                };
                keys(ui.ctx(), &mut cx);
                if cx.view.modal == Some(Modal::QuickSwitcher) {
                    modal(ui.ctx(), &mut cx);
                }
            });
            out.textures_delta.clear();
        }
    }

    #[test]
    fn ctrl_k_opens_and_closes_the_switcher() {
        let mut h = Harness::new();
        h.press(Modifiers::COMMAND, Key::K);
        assert_eq!(h.view.modal, Some(Modal::QuickSwitcher));
        h.press(Modifiers::COMMAND, Key::K);
        assert_eq!(h.view.modal, Some(Modal::QuickSwitcher), "a held key is no second press");
        h.release(Modifiers::COMMAND, Key::K);
        h.press(Modifiers::COMMAND, Key::K);
        assert_eq!(h.view.modal, None);
        h.release(Modifiers::COMMAND, Key::K);
        h.view.modal = Some(Modal::JoinServer);
        h.press(Modifiers::COMMAND, Key::K);
        assert_eq!(h.view.modal, Some(Modal::JoinServer), "not over another dialog");
        let _ = &h.rx;
    }
}
