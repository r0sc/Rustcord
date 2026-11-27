//! Tabs and split view (docs/DESIGN.md §10.10). A tab bar above the chat while two or more
//! channels are open as tabs, and a split pane that shows a second chat beside the open one.
//! Session only; every tab and split comes from a click (middle-click or the row menu), never
//! from automation. Only the focused pane counts as read and sends typing.
//!
//! The pure parts (`sync`, `open_at`, `close_at`, `move_tab`, `focused_channel`, `split_w`) are
//! tested; the rest draws them.

use super::icons::{self, Icon};
use super::messages::{MenuIcon, Pal, menu_item, menu_setup, vector_button};
use super::{UiCx, ViewState, motion, one_line, sidebar};
use crate::model::Id;
use crate::state::AppState;
use egui::{Align, Frame, Layout, Margin, Rect, Sense, Stroke, UiBuilder, Vec2, pos2, vec2};

pub const MAX_TABS: usize = 8;
/// Windows narrower than this show the split pane in place of the chat (as the thread panel).
pub const SPLIT_MIN_WINDOW_W: f32 = 1000.0;
const BAR_H: f32 = 40.0;
const TAB_H: f32 = 32.0;
const GAP: f32 = 4.0;
const PAD: f32 = 8.0;
const MIN_W: f32 = 48.0;
const MAX_W: f32 = 200.0;

/// Tabs and split state (in `ViewState::tabs_ui`).
#[derive(Debug, Default)]
pub struct TabsUi {
    /// Left to right; empty = no tabs (a single tab is never kept).
    list: Vec<Id>,
    active: usize,
    /// The split pane holds the focus (else the open chat does).
    pub split_focus: bool,
    /// The tab held by the pointer and where inside it.
    drag: Option<(Id, f32)>,
    /// The widget that had the keyboard focus last pass (to see it move between the panes).
    focused: Option<egui::Id>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Opened {
    /// A tab was added (right of the active one).
    Added,
    /// Already a tab: the index.
    Already(usize),
    /// At `MAX_TABS`.
    Full,
    /// No tabs yet and nothing to pair with: just open the channel.
    Plain,
}

// ---------------------------------------------------------------------------
// Pure parts
// ---------------------------------------------------------------------------

/// Brings the tabs in line with the open channel: tabs of missing channels drop out, the open
/// channel activates its tab, or else replaces the active tab's channel (navigation inside a
/// tab); fewer than two tabs clear the list.
pub fn sync(list: &mut Vec<Id>, active: &mut usize, selected: Option<Id>, exists: impl Fn(Id) -> bool) {
    let current = list.get(*active).copied();
    list.retain(|&c| exists(c));
    *active = current.and_then(|c| list.iter().position(|&x| x == c)).unwrap_or((*active).min(list.len().saturating_sub(1)));
    if let Some(sel) = selected {
        match list.iter().position(|&c| c == sel) {
            Some(i) => *active = i,
            None if !list.is_empty() => list[*active] = sel,
            None => {}
        }
    }
    if list.len() < 2 {
        list.clear();
        *active = 0;
    }
}

/// Opens `id` as a background tab right of the active one. With no tabs the open channel becomes
/// the first one.
pub fn open_at(list: &mut Vec<Id>, active: &mut usize, selected: Option<Id>, id: Id) -> Opened {
    if list.is_empty() {
        let Some(sel) = selected.filter(|&s| s != id) else { return Opened::Plain };
        (*list, *active) = (vec![sel, id], 0);
        return Opened::Added;
    }
    if let Some(i) = list.iter().position(|&c| c == id) {
        return Opened::Already(i);
    }
    if list.len() >= MAX_TABS {
        return Opened::Full;
    }
    list.insert(*active + 1, id);
    Opened::Added
}

/// Closes tab `i`. Some(channel) = the active tab went: open that one (its right neighbour, else
/// the left one).
pub fn close_at(list: &mut Vec<Id>, active: &mut usize, i: usize) -> Option<Id> {
    if i >= list.len() {
        return None;
    }
    list.remove(i);
    let was_active = i == *active;
    if i < *active {
        *active -= 1;
    }
    let next = was_active.then(|| list.get(i).or_else(|| list.last()).copied()).flatten();
    if was_active {
        *active = i.min(list.len().saturating_sub(1));
    }
    if list.len() < 2 {
        list.clear();
        *active = 0;
    }
    next
}

/// Moves tab `from` to `to`; the active tab stays on its channel.
pub fn move_tab(list: &mut [Id], active: &mut usize, from: usize, to: usize) {
    if from >= list.len() || to >= list.len() || from == to {
        return;
    }
    let current = list[*active];
    if from < to {
        list[from..=to].rotate_left(1);
    } else {
        list[to..=from].rotate_right(1);
    }
    *active = list.iter().position(|&c| c == current).unwrap_or(0);
}

/// The channel that counts as read and notifies nothing: the split's while it holds the focus,
/// else the open one.
pub fn focused_channel(selected: Option<Id>, split: Option<Id>, split_focus: bool) -> Option<Id> {
    split.filter(|_| split_focus).or(selected)
}

/// The split pane's width: half the room right of the lists (`left_w`: rail and list as drawn).
pub fn split_w(width: f32, left_w: f32) -> f32 {
    ((width - left_w) / 2.0).floor().max(0.0)
}

/// One tab's width in a bar `bar_w` wide with `n` tabs.
fn tab_w(bar_w: f32, n: usize) -> f32 {
    ((bar_w - 2.0 * PAD - GAP * n.saturating_sub(1) as f32) / n.max(1) as f32).clamp(MIN_W, MAX_W)
}

// ---------------------------------------------------------------------------
// Opening and closing
// ---------------------------------------------------------------------------

fn id_of(what: &str, ch: Id) -> egui::Id {
    egui::Id::unique((what, ch.0))
}

fn split_anim_id() -> egui::Id {
    egui::Id::unique("rc_split_open")
}

/// Middle-click on a channel or DM row, or "Open in new tab".
pub(super) fn open(ctx: &egui::Context, cx: &mut UiCx<'_>, id: Id) {
    let t = &mut cx.view.tabs_ui;
    match open_at(&mut t.list, &mut t.active, cx.state.selected_channel, id) {
        Opened::Added => _ = motion::anim(ctx, id_of("rc_tab_grow", id), false, motion::FAST, 0.0),
        Opened::Already(_) => motion::pop(ctx, id_of("rc_tab_pop", id)),
        Opened::Full => cx.view.toast(format!("You can open up to {MAX_TABS} tabs. Close one first."), false),
        Opened::Plain => sidebar::open_channel(cx, id),
    }
}

/// Shows `id` beside the open chat (closes the thread panel, takes the focus).
pub(super) fn open_split(ctx: &egui::Context, cx: &mut UiCx<'_>, id: Id) {
    if cx.state.selected_channel == Some(id) || !cx.state.channels.get(&id).is_some_and(|c| c.is_text_chat() && !c.is_thread()) {
        return;
    }
    cx.view.thread_panel = None;
    cx.state.split_channel = Some(id);
    cx.view.tabs_ui.split_focus = true;
    // The pane slides in from the right (armed here, played by `split_panel`).
    motion::anim(ctx, split_anim_id(), false, motion::BASE, 0.0);
}

pub(super) fn close_split(cx: &mut UiCx<'_>) {
    cx.state.split_channel = None;
    cx.view.tabs_ui.split_focus = false;
}

/// "Open in new tab" and "Open in split view" of a channel or DM row's menu (not for the open
/// channel; split not for threads, forums or voice).
pub(super) fn menu_items(ui: &mut egui::Ui, cx: &mut UiCx<'_>, id: Id) {
    let Some(ch) = cx.state.channels.get(&id) else { return };
    if cx.state.selected_channel == Some(id) || !(ch.is_text_chat() || ch.kind == crate::model::channel_type::GUILD_FORUM) {
        return;
    }
    let (split, text) = (ch.is_text_chat() && !ch.is_thread(), Pal::new(cx.theme).text);
    let ctx = ui.ctx().clone();
    if menu_item(ui, "Open in new tab", MenuIcon::Icon(Icon::Plus), text) {
        open(&ctx, cx, id);
    }
    if split && menu_item(ui, "Open in split view", MenuIcon::Icon(Icon::SplitView), text) {
        open_split(&ctx, cx, id);
    }
}

/// Test hook (`RUSTCORD_OPEN=tab:<id>`): the tab, without a pointer.
pub(super) fn demo_tab(view: &mut ViewState, state: &AppState, id: Id) {
    let t = &mut view.tabs_ui;
    open_at(&mut t.list, &mut t.active, state.selected_channel, id);
}

/// Every pass in the chat layout: the tabs follow the open channel.
pub(super) fn sync_state(view: &mut ViewState, state: &AppState) {
    let t = &mut view.tabs_ui;
    sync(&mut t.list, &mut t.active, state.selected_channel, |c| state.channels.contains_key(&c));
}

// ---------------------------------------------------------------------------
// The bar
// ---------------------------------------------------------------------------

/// The tab bar, only while two or more tabs are open: 40 tall, growing over FAST when the second
/// tab opens, gone at once when it closes. Spans the page and the member list, not the split pane.
pub(super) fn bar(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    let ctx = ui.ctx().clone();
    let shown = cx.view.tabs_ui.list.len() >= 2;
    let grow = motion::anim(&ctx, egui::Id::unique("rc_tabs_bar"), shown, motion::FAST, 0.0);
    if !shown || grow <= 0.0 {
        return;
    }
    let pal = Pal::new(cx.theme);
    egui::Panel::top("rc_tabs").exact_size(BAR_H * grow).resizable(false).show_separator_line(false).frame(Frame::new().fill(pal.bg)).show(ui, |ui| {
        // Laid out at full height against the bottom edge, so the tabs ride the growing bar down.
        let clip = ui.max_rect();
        let rect = Rect::from_min_max(pos2(clip.left(), clip.bottom() - BAR_H), clip.right_bottom());
        ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
            ui.set_clip_rect(clip.intersect(ui.clip_rect()));
            tabs(ui, cx, &pal, rect);
        });
    });
}

fn tabs(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, bar: Rect) {
    let ctx = ui.ctx().clone();
    let list = cx.view.tabs_ui.list.clone();
    let (active, drag) = (cx.view.tabs_ui.active, cx.view.tabs_ui.drag);
    let n = list.len();
    let w = tab_w(bar.width(), n);
    let pitch = w + GAP;
    let pointer = ctx.pointer_latest_pos();
    let y = bar.center().y - TAB_H / 2.0;
    let (mut select, mut close, mut others, mut split, mut reorder) = (None, None, None, None, None);
    let mut drag_next = drag;
    for (i, &ch) in list.iter().enumerate() {
        let held = drag.filter(|(c, _)| *c == ch);
        let slot_id = id_of("rc_tab_slot", ch);
        // Every tab sits in its slot, gliding there after a close or a move; the held one follows
        // the pointer 1:1 (clamped to the bar) and settles when it is let go.
        let mut slot = motion::value(&ctx, slot_id, i as f32, motion::FAST);
        if let (Some((_, off)), Some(p)) = (held, pointer) {
            let x = (p.x - off).clamp(bar.left() + PAD, bar.left() + PAD + pitch * (n - 1) as f32);
            slot = (x - bar.left() - PAD) / pitch;
            motion::set_value(&ctx, slot_id, slot);
            let to = (slot + 0.5).floor().clamp(0.0, (n - 1) as f32) as usize;
            if to != i {
                reorder = Some((i, to));
            }
        }
        let rect = Rect::from_min_size(pos2(bar.left() + PAD + slot * pitch, y), vec2(w, TAB_H));
        let resp = ui.interact(rect, ui.make_persistent_id(("rc_tab", ch.0)), Sense::click_and_drag());
        let selected = i == active;
        let hot = ui.rect_contains_pointer(rect) && drag.is_none() || held.is_some();
        let x_rect = Rect::from_center_size(pos2(rect.right() - 6.0 - 10.0, rect.center().y), Vec2::splat(20.0));
        let show_x = hot || selected;
        let x_resp = show_x.then(|| ui.interact(x_rect, ui.make_persistent_id(("rc_tab_x", ch.0)), Sense::click()));
        let x_hot = x_resp.as_ref().is_some_and(|r| r.hovered());
        if resp.drag_started()
            && let Some(p) = pointer
        {
            drag_next = Some((ch, p.x - rect.left()));
        }
        if resp.drag_stopped() {
            drag_next = None;
        }
        if x_resp.as_ref().is_some_and(|r| r.clicked()) || resp.middle_clicked() {
            close = Some(i);
        } else if resp.clicked() {
            select = Some(ch);
        }
        if resp.has_focus() && ctx.input(|i| i.key_pressed(egui::Key::Delete)) {
            close = Some(i);
        }
        // A new tab fades in and grows, an already open one pops once.
        let grown = motion::anim(&ctx, id_of("rc_tab_grow", ch), true, motion::FAST, 0.0);
        let scale = egui::lerp(motion::GROW..=1.0, grown) * motion::popped(&ctx, id_of("rc_tab_pop", ch));
        ui.with_visual_transform(motion::about(rect.center(), scale), |ui| {
            ui.multiply_opacity(grown);
            paint_tab(ui, cx, pal, ch, rect, selected, hot, x_hot, show_x, x_rect);
        });
        if resp.has_focus() {
            super::messages::focus_ring(ui, pal, rect, f32::from(pal.radius));
        }
        let name = cx.state.channel_name(ch);
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, &name));
        let tip = tab_tip(cx.state, ch);
        let resp = resp.on_hover_text(tip);
        let can_split = cx.state.selected_channel != Some(ch) && cx.state.channels.get(&ch).is_some_and(|c| c.is_text_chat() && !c.is_thread());
        resp.context_menu(|ui| {
            menu_setup(ui);
            let text = pal.text;
            if can_split && menu_item(ui, "Open in split view", MenuIcon::Icon(Icon::SplitView), text) {
                split = Some(ch);
            }
            if menu_item(ui, "Close tab", MenuIcon::Icon(Icon::Close), text) {
                close = Some(i);
            }
            if menu_item(ui, "Close other tabs", MenuIcon::Glyph(""), text) {
                others = Some(ch);
            }
        });
    }
    cx.view.tabs_ui.drag = drag_next.filter(|_| ctx.input(|i| i.pointer.any_down()));
    if let Some((from, to)) = reorder {
        let t = &mut cx.view.tabs_ui;
        move_tab(&mut t.list, &mut t.active, from, to);
    }
    if let Some(ch) = select {
        sidebar::open_channel(cx, ch);
    }
    if let Some(i) = close {
        let t = &mut cx.view.tabs_ui;
        if let Some(next) = close_at(&mut t.list, &mut t.active, i) {
            sidebar::open_channel(cx, next);
        }
    }
    if let Some(ch) = others {
        cx.view.tabs_ui.list.clear();
        cx.view.tabs_ui.active = 0;
        sidebar::open_channel(cx, ch);
    }
    if let Some(ch) = split {
        open_split(&ctx, cx, ch);
    }
}

/// "#general in Rust Workshop" / "@Alice".
fn tab_tip(state: &AppState, ch: Id) -> String {
    let name = state.channel_name(ch);
    match state.channels.get(&ch) {
        Some(c) if c.guild_id.is_some() => match c.guild_id.and_then(|g| state.guild(g)) {
            Some(g) => format!("#{name} in {}", g.name),
            None => format!("#{name}"),
        },
        Some(c) if c.kind == crate::model::channel_type::GROUP_DM => name,
        _ => format!("@{name}"),
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_tab(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, ch: Id, rect: Rect, selected: bool, hot: bool, x_hot: bool, show_x: bool, x_rect: Rect) {
    // Active: the selected fill (never purple: a view switch is not "you"); hover: the hover fill.
    let fill = if selected { pal.tk.selected } else if hot { pal.tk.hover } else { egui::Color32::TRANSPARENT };
    ui.painter().rect_filled(rect, pal.radius, fill);
    let icon = Rect::from_center_size(pos2(rect.left() + PAD + 8.0, rect.center().y), Vec2::splat(16.0));
    let (dm, unread) = (cx.state.channels.get(&ch).is_some_and(|c| c.is_dm_like()), cx.state.is_unread(ch));
    if dm {
        sidebar::dm_avatar_at(ui, cx, ch, icon);
    } else if let Some(c) = cx.state.channels.get(&ch) {
        icons::paint_channel(ui.painter(), icons::of(c, cx.state), icon, pal.muted, pal.bg.blend(fill));
    }
    let mentions = cx.state.mention_count(ch);
    let mut right = rect.right() - PAD;
    if show_x {
        right = x_rect.left() - 2.0;
    } else if mentions > 0 {
        right -= sidebar::badge(ui, &cx.theme.colors, pos2(rect.right() - PAD, rect.center().y), mentions, None) + 4.0;
    }
    let left = rect.left() + PAD + 16.0 + 8.0;
    let color = if selected || hot || unread { pal.text } else { pal.muted };
    let family = if unread { crate::theme::semibold_family() } else { crate::theme::medium_family() };
    // Narrow tabs show the icon only.
    if right - left >= 24.0 {
        let g = one_line(ui, cx.state.channel_name(ch), egui::FontId::new(pal.ui(), family), color, right - left);
        ui.painter().galley(pos2(left, rect.center().y - g.size().y / 2.0), g, color);
    }
    if show_x {
        if x_hot {
            ui.painter().circle_filled(x_rect.center(), 10.0, pal.tk.hover);
        }
        icons::paint(ui.painter(), Icon::Close, Rect::from_center_size(x_rect.center(), Vec2::splat(16.0)), if x_hot { pal.text } else { pal.muted }, fill);
    }
}

// ---------------------------------------------------------------------------
// The split pane
// ---------------------------------------------------------------------------

/// The split pane: a header (icon or avatar, name, a close button) over the channel's own list
/// and composer. Fades in and slides 16 px from the right over BASE (its layout is final at once).
pub(super) fn split_panel(ui: &mut egui::Ui, cx: &mut UiCx<'_>, ch: Id) {
    let t = motion::anim(ui.ctx(), split_anim_id(), true, motion::BASE, 0.0);
    let pal = Pal::new(cx.theme);
    let focus = cx.view.tabs_ui.split_focus;
    ui.with_visual_transform(egui::emath::TSTransform::from_translation(vec2(16.0 * (1.0 - t), 0.0)), |ui| {
        ui.multiply_opacity(t);
        let name = cx.state.channel_name(ch);
        let tint = if focus { pal.text } else { pal.muted };
        let header = egui::Panel::top("rc_split_header")
            .exact_size(49.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().inner_margin(Margin { left: 16, right: 12, top: 0, bottom: 0 }))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if vector_button(ui, &pal, Icon::Close, "Close split view", 32.0, false, pal.text).clicked() {
                            close_split(cx);
                        }
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            if cx.state.channels.get(&ch).is_some_and(|c| c.is_dm_like()) {
                                let (slot, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
                                sidebar::dm_avatar_at(ui, cx, ch, slot);
                            } else if let Some(c) = cx.state.channels.get(&ch) {
                                let (slot, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                                icons::paint_channel(ui.painter(), icons::of(c, cx.state), slot, tint, pal.bg);
                            }
                            let title = egui::RichText::new(name).font(egui::FontId::new(pal.heading(), crate::theme::semibold_family())).color(tint);
                            ui.add(egui::Label::new(title).selectable(false).truncate());
                        });
                    });
                });
            });
        let r = header.response.rect;
        ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, pal.tk.rule));
        super::messages::thread_chat(ui, cx, ch);
    });
}

/// Which pane has the focus: the one last pressed in, or where the keyboard focus just moved
/// (Tab). `split`: its rect (None = no split: the main chat). `main`: the open chat's area. A
/// press that leaves the keyboard focus in the other pane's box takes it away from there.
pub(super) fn track_focus(ctx: &egui::Context, view: &mut ViewState, split: Option<Rect>, main: Rect) {
    let Some(split) = split else {
        (view.tabs_ui.split_focus, view.tabs_ui.focused) = (false, None);
        return;
    };
    let t = &mut view.tabs_ui;
    let focus = ctx.memory(|m| m.focused());
    let focus_rect = focus.and_then(|id| ctx.read_response(id)).map(|r| r.rect);
    let pane = |p: egui::Pos2| if split.contains(p) { Some(true) } else { main.contains(p).then_some(false) };
    let pressed = ctx.input(|i| i.pointer.any_pressed().then(|| i.pointer.press_origin()).flatten());
    if let Some(p) = pressed
        && let Some(in_split) = pane(p)
    {
        t.split_focus = in_split;
        if let (Some(id), Some(r)) = (focus, focus_rect)
            && pane(r.center()).is_some_and(|other| other != in_split)
        {
            ctx.memory_mut(|m| m.surrender_focus(id));
        }
    } else if focus != t.focused
        && let Some(in_split) = focus_rect.and_then(|r| pane(r.center()))
    {
        t.split_focus = in_split;
    }
    t.focused = focus;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[u64]) -> Vec<Id> {
        v.iter().map(|&i| Id(i)).collect()
    }

    fn all(_: Id) -> bool {
        true
    }

    #[test]
    fn sync_follows_the_open_channel() {
        let (mut list, mut active) = (ids(&[1, 2, 3]), 0);
        // Navigation inside a tab replaces its channel.
        sync(&mut list, &mut active, Some(Id(9)), all);
        assert_eq!((list.clone(), active), (ids(&[9, 2, 3]), 0));
        // An open channel that already is a tab activates it.
        sync(&mut list, &mut active, Some(Id(3)), all);
        assert_eq!(active, 2);
        // A missing channel drops out, the active tab follows its channel.
        sync(&mut list, &mut active, Some(Id(3)), |c| c != Id(9));
        assert_eq!((list.clone(), active), (ids(&[2, 3]), 1));
        // One tab left: no tabs.
        sync(&mut list, &mut active, Some(Id(3)), |c| c != Id(2));
        assert!(list.is_empty() && active == 0);
        // Nothing open (Friends page): the tabs stay as they are.
        let (mut list, mut active) = (ids(&[1, 2]), 1);
        sync(&mut list, &mut active, None, all);
        assert_eq!((list, active), (ids(&[1, 2]), 1));
    }

    #[test]
    fn open_at_makes_background_tabs() {
        let (mut list, mut active) = (Vec::new(), 0);
        assert_eq!(open_at(&mut list, &mut active, None, Id(5)), Opened::Plain);
        assert_eq!(open_at(&mut list, &mut active, Some(Id(5)), Id(5)), Opened::Plain);
        // The first middle-click makes [open channel, new].
        assert_eq!(open_at(&mut list, &mut active, Some(Id(1)), Id(2)), Opened::Added);
        assert_eq!((list.clone(), active), (ids(&[1, 2]), 0));
        // Right of the active tab, without switching.
        assert_eq!(open_at(&mut list, &mut active, Some(Id(1)), Id(3)), Opened::Added);
        assert_eq!((list.clone(), active), (ids(&[1, 3, 2]), 0));
        assert_eq!(open_at(&mut list, &mut active, Some(Id(1)), Id(2)), Opened::Already(2));
        for i in 10..20 {
            open_at(&mut list, &mut active, Some(Id(1)), Id(i));
        }
        assert_eq!(list.len(), MAX_TABS);
        assert_eq!(open_at(&mut list, &mut active, Some(Id(1)), Id(99)), Opened::Full);
    }

    #[test]
    fn close_at_picks_a_neighbour() {
        // The active tab goes: its right neighbour opens.
        let (mut list, mut active) = (ids(&[1, 2, 3]), 1);
        assert_eq!(close_at(&mut list, &mut active, 1), Some(Id(3)));
        assert_eq!((list.clone(), active), (ids(&[1, 3]), 1));
        // The last one goes: the left one opens, and one tab left means no tabs.
        assert_eq!(close_at(&mut list, &mut active, 1), Some(Id(1)));
        assert!(list.is_empty() && active == 0);
        // Closing left of the active tab shifts it; closing right of it changes nothing for it.
        let (mut list, mut active) = (ids(&[1, 2, 3, 4]), 2);
        assert_eq!(close_at(&mut list, &mut active, 0), None);
        assert_eq!((list.clone(), active), (ids(&[2, 3, 4]), 1));
        assert_eq!(close_at(&mut list, &mut active, 2), None);
        assert_eq!((list, active), (ids(&[2, 3]), 1));
    }

    #[test]
    fn move_tab_keeps_the_active_channel() {
        let (mut list, mut active) = (ids(&[1, 2, 3, 4]), 1);
        move_tab(&mut list, &mut active, 3, 0);
        assert_eq!((list.clone(), active), (ids(&[4, 1, 2, 3]), 2));
        move_tab(&mut list, &mut active, 0, 3);
        assert_eq!((list.clone(), active), (ids(&[1, 2, 3, 4]), 1));
        move_tab(&mut list, &mut active, 1, 2);
        assert_eq!((list, active), (ids(&[1, 3, 2, 4]), 2));
    }

    #[test]
    fn focus_and_widths() {
        let (sel, split) = (Some(Id(1)), Some(Id(2)));
        assert_eq!(focused_channel(sel, split, true), split);
        assert_eq!(focused_channel(sel, split, false), sel);
        assert_eq!(focused_channel(sel, None, true), sel, "no split open: the open chat");
        assert_eq!(split_w(1280.0, 375.0), 452.0);
        assert_eq!(split_w(100.0, 375.0), 0.0);
        assert_eq!(tab_w(1000.0, 2), MAX_W);
        assert_eq!(tab_w(400.0, 8), MIN_W.max((400.0 - 16.0 - 28.0) / 8.0));
        assert_eq!(tab_w(100.0, 8), MIN_W);
    }
}
