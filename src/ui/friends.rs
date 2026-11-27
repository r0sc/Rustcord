//! Friends page (DM home with no DM open; FEATURES_NOTES §2.3, layout §4.3): a 49 px header
//! with the tabs Online / All / Pending / Blocked and the green Add Friend tab, a search field
//! and 62 px rows (avatar with status, name, status line; Message / More, Accept / Ignore /
//! Cancel, Unblock as round icon buttons), Pending with a Spam section, and the 360 px Active Now
//! column (friends with a game / music / stream) in windows ≥ 1100 px. The 80013 question
//! (`ask_stranger`): when `state.social.confirm_stranger` is set, ask with `ViewState::confirm`
//! (`danger: false`, `AcceptFriend { confirm_stranger: true }`) and clear it. Also the social
//! items of the user and DM menus (`user_menu_items`, `dm_menu_items`) and `Modal::GroupDm`:
//! the New Group DM / Add Friends picker, or Rename Group (`FriendsUi::rename`).
//!
//! Hook-up guide: docs/SOCIAL_DESIGN.md (Friends tabs, Add Friend, Accept, Remove, Block, DMs).
//! Remove Friend, Block, Leave Group go through `ViewState::confirm`.
//! OWNERSHIP: friends/DMs/inbox/profile engineer (docs/SOCIAL_UI_DESIGN.md).

use super::icons::{self, Icon};
use super::messages::{MenuIcon, Pal, menu_item, menu_setup};
use super::sidebar::{self, empty_state, name_and_status, user_avatar_at, user_dot, user_status_text};
use super::{Modal, UiCx, bold, dialog, dialog_buttons, dialog_text, one_line, presence_color};
use crate::backend::{Command, ContextLocation, SocialCommand};
use crate::model::{CustomStatus, Emoji, GatewayEvent, Id, Presence, Relationship, UserPresence, channel_type, relationship_type};
use crate::state::AppState;
use crate::state::social::{AddFriendView, FriendsTab};
use egui::{Color32, CursorIcon, FontId, Frame, Margin, Rect, RichText, Sense, Shape, Stroke, Vec2, pos2, vec2};

/// Friends page tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    List(FriendsTab),
    AddFriend,
}

impl Default for Tab {
    fn default() -> Self {
        Tab::List(FriendsTab::Online)
    }
}

/// Friends page and group-DM picker state (in `ViewState::friends_ui`).
#[derive(Debug, Default)]
pub struct FriendsUi {
    pub tab: Tab,
    /// Search field of the list tabs.
    pub query: String,
    /// Add Friend tab: the username typed.
    pub add_name: String,
    /// Group DM picker: friends ticked, and its filter text.
    pub picked: Vec<Id>,
    pub pick_query: String,
    /// `Modal::GroupDm` of a group in rename mode: the name typed.
    pub rename: Option<String>,
    /// The open Edit name form of a person's menu.
    pub name_edit: Option<NameDraft>,
}

/// Group DMs hold 10 people, me included (§2.4).
const GROUP_MAX: usize = 10;
const ROW_H: f32 = 62.0;
/// Active Now column width and the narrowest window that shows it.
const ACTIVE_NOW_W: f32 = 360.0;
const ACTIVE_NOW_MIN_WINDOW_W: f32 = 1100.0;

/// The Friends page is the DM home with no DM open (`main_layout` shows it then).
pub fn is_open(state: &AppState) -> bool {
    state.selected_guild.is_none() && state.selected_channel.is_none()
}

/// The DM list's Friends row: shows the Friends page.
pub fn open(state: &mut AppState) {
    state.selected_guild = None;
    state.selected_channel = None;
    state.dirty = true;
}

/// The page (in place of the chat): header with the tabs, then the tab; Active Now on the right
/// in windows at least `ACTIVE_NOW_MIN_WINDOW_W` wide.
pub fn show(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    demo_tab(cx);
    let pal = Pal::new(cx.theme);
    header(ui, cx, &pal);
    if ui.ctx().content_rect().width() >= ACTIVE_NOW_MIN_WINDOW_W {
        active_now(ui, cx, &pal);
    }
    match cx.view.friends_ui.tab {
        Tab::AddFriend => add_friend_page(ui, cx, &pal),
        Tab::List(tab) => list_page(ui, cx, &pal, tab),
    }
}

fn header(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal) {
    let pending = cx.state.pending_count();
    let header = egui::Panel::top("rc_friends_header")
        .exact_size(49.0)
        .resizable(false)
        .show_separator_line(false)
        .frame(Frame::new().fill(pal.bg).inner_margin(Margin { left: 16, right: 12, top: 0, bottom: 0 }))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                // The title goes first when the tabs need the room; a 1x20 rule after it, as in the
                // chat header.
                if ui.available_width() > 560.0 {
                    let (icon, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                    icons::paint(ui.painter(), Icon::Members, icon, pal.muted, pal.bg);
                    ui.label(RichText::new("Friends").font(super::semibold(pal.heading())).color(pal.text));
                    let (r, _) = ui.allocate_exact_size(vec2(1.0, 20.0), Sense::hover());
                    ui.painter().vline(r.center().x, r.y_range(), Stroke::new(1.0, pal.tk.rule));
                }
                // The list tabs: one segmented control (docs/DESIGN.md §9) on the well colour.
                let track_slot = ui.painter().add(Shape::Noop);
                let mut track = Rect::NOTHING;
                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let tabs = [(FriendsTab::Online, "Online"), (FriendsTab::All, "All"), (FriendsTab::Pending, "Pending"), (FriendsTab::Blocked, "Blocked")];
                    for (tab, label) in tabs {
                        let badge = if tab == FriendsTab::Pending { pending } else { 0 };
                        let resp = tab_button(ui, cx, pal, label, cx.view.friends_ui.tab == Tab::List(tab), badge, None);
                        track = track.union(resp.rect);
                        if resp.clicked() {
                            cx.view.friends_ui.tab = Tab::List(tab);
                        }
                    }
                });
                let well = cx.theme.colors.code_background.to_color32();
                ui.painter().set(track_slot, Shape::rect_filled(track.expand(2.0), pal.radius, well));
                ui.add_space(4.0);
                // The view's one primary action.
                if tab_button(ui, cx, pal, "Add friend", cx.view.friends_ui.tab == Tab::AddFriend, 0, Some(pal.accent)).clicked() {
                    cx.view.friends_ui.tab = Tab::AddFriend;
                }
            });
        });
    let r = header.response.rect;
    ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, pal.tk.rule));
}

/// A header tab: a segment of the list tabs (32 tall, ui 14/500; selected on the `selected`
/// fill, `raised` on light themes); `primary` (the accent) makes it the filled Add friend button,
/// a plain selected segment while its page is open. `badge` = a count pill after the label.
pub(super) fn tab_button(ui: &mut egui::Ui, cx: &UiCx<'_>, pal: &Pal, text: &str, selected: bool, badge: usize, primary: Option<Color32>) -> egui::Response {
    let font = FontId::new(pal.ui(), crate::theme::medium_family());
    let g = ui.painter().layout_no_wrap(text.to_owned(), font.clone(), pal.text);
    let badge_w = if badge > 0 { 24.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 24.0 + badge_w, 32.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, text));
    let hot = resp.hovered() || resp.has_focus();
    let on = if pal.tk.mode == crate::theme::Mode::Light { pal.tk.raised } else { pal.tk.selected };
    let (fill, color) = match primary {
        Some(c) if !selected => (if hot { c.lerp_to_gamma(Color32::WHITE, 0.08) } else { c }, pal.accent_text),
        _ if selected => (on, pal.text),
        _ if hot => (pal.tk.hover, pal.text),
        _ => (Color32::TRANSPARENT, pal.muted),
    };
    let radius = if primary.is_some() { pal.radius } else { pal.radius.saturating_sub(2) };
    ui.painter().rect_filled(rect, radius, fill);
    ui.painter().text(pos2(rect.left() + 12.0, rect.center().y), egui::Align2::LEFT_CENTER, text, font, color);
    if badge > 0 {
        sidebar::badge(ui, &cx.theme.colors, pos2(rect.right() - 8.0, rect.center().y), badge as u32, None);
    }
    if resp.has_focus() {
        super::messages::focus_ring(ui, pal, rect, f32::from(radius));
    }
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

// ---------------------------------------------------------------------------
// List tabs
// ---------------------------------------------------------------------------

fn list_page(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, tab: FriendsTab) {
    let query = cx.view.friends_ui.query.trim().to_lowercase();
    let matches = |s: &AppState, id: &Id| {
        query.is_empty()
            || s.friend_name(*id).to_lowercase().contains(&query)
            || s.users.get(id).is_some_and(|u| u.username.to_lowercase().contains(&query))
    };
    let mut sections = vec![(section_title(tab), cx.state.friends_tab(tab))];
    if tab == FriendsTab::Pending {
        sections.push(("Spam", cx.state.friends_tab(FriendsTab::Spam)));
    }
    let total: usize = sections.iter().map(|(_, ids)| ids.len()).sum();
    for (_, ids) in &mut sections {
        ids.retain(|id| matches(cx.state, id));
    }
    Frame::new().inner_margin(Margin { left: 20, right: 20, top: 16, bottom: 8 }).show(ui, |ui| {
        let edit = egui::TextEdit::singleline(&mut cx.view.friends_ui.query)
            .hint_text("Search")
            .font(FontId::proportional(pal.size))
            .margin(Margin::symmetric(10, 7))
            .desired_width(f32::INFINITY);
        ui.add_enabled(total > 0, edit);
    });
    if total == 0 {
        let text = match tab {
            FriendsTab::Online => "No one's around to play with.",
            FriendsTab::All => "No friends yet. Select Add friend to send a request.",
            FriendsTab::Pending | FriendsTab::Spam => "No pending friend requests.",
            FriendsTab::Blocked => "You haven't blocked anyone.",
        };
        empty_state(ui, cx, 48.0, text);
        return;
    }
    egui::ScrollArea::vertical().id_salt(("rc_friends_list", tab as u8)).auto_shrink(false).show(ui, |ui| {
        Frame::new().inner_margin(Margin { left: 20, right: 20, top: 0, bottom: 16 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut shown = 0;
            for (title, ids) in sections.iter().filter(|(_, ids)| !ids.is_empty()) {
                // The label role left, the count right-aligned in meta (docs/DESIGN.md §10.4).
                let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::hover());
                let n = one_line(ui, ids.len().to_string(), FontId::proportional(pal.meta()), pal.muted, row.width());
                let g = one_line(ui, *title, super::semibold(pal.label()), pal.muted, row.width() - n.size().x - 28.0);
                ui.painter().galley(pos2(row.right() - 10.0 - n.size().x, row.center().y - n.size().y / 2.0), n, pal.muted);
                ui.painter().galley(pos2(row.left() + 10.0, row.center().y - g.size().y / 2.0), g, pal.muted);
                for &id in ids {
                    friend_row(ui, cx, pal, id);
                }
                shown += ids.len();
            }
            if shown == 0 {
                empty_state(ui, cx, 24.0, "No matches.");
            }
        });
    });
}

fn section_title(tab: FriendsTab) -> &'static str {
    match tab {
        FriendsTab::Online => "Online",
        FriendsTab::All => "All friends",
        FriendsTab::Pending => "Pending",
        FriendsTab::Spam => "Spam",
        FriendsTab::Blocked => "Blocked",
    }
}

/// One user: avatar with status, name (username on hover), status line, and the actions of
/// their relationship as round buttons. Off-screen rows only take their space.
fn friend_row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, id: Id) {
    use relationship_type::*;
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    if !ui.is_rect_visible(row) {
        return;
    }
    let kind = cx.state.social.relationships.get(&id).map_or(0, |r| r.kind);
    let bg_slot = ui.painter().add(Shape::Noop);
    let hot = ui.rect_contains_pointer(row);
    let ring = if hot { pal.bg.blend(pal.tint()) } else { pal.bg };
    if hot {
        ui.painter().set(bg_slot, Shape::rect_filled(row, pal.card_radius(), ring));
    } else {
        ui.painter().hline(row.x_range().shrink(10.0), row.top() + 0.5, Stroke::new(1.0, pal.tk.rule));
    }
    let name = cx.state.friend_name(id);
    let avatar = Rect::from_min_size(pos2(row.left() + 10.0, row.center().y - 16.0), Vec2::splat(32.0));
    friend_avatar(ui, cx, pal, id, &name, avatar);
    if kind == FRIEND {
        user_dot(ui.painter(), avatar, cx.state, id, &cx.theme.colors, ring);
    }

    // Right to left: the actions, 36 px circles 10 px apart.
    let actions: &[(Action, &str)] = match kind {
        FRIEND => &[(Action::More, "More"), (Action::Message, "Message")],
        INCOMING => &[(Action::Remove, "Ignore"), (Action::Accept, "Accept")],
        OUTGOING => &[(Action::Remove, "Cancel")],
        BLOCKED => &[(Action::Unblock, "Unblock")],
        _ => &[],
    };
    let mut right = row.right() - 10.0;
    let mut clicked_action = false;
    for &(action, tip) in actions {
        let r = Rect::from_center_size(pos2(right - 18.0, row.center().y), Vec2::splat(36.0));
        right -= 46.0;
        let resp = round_button(ui, cx, pal, (id, r), action, tip, hot);
        clicked_action |= resp.clicked();
        if action == Action::More {
            egui::Popup::menu(&resp).show(|ui| {
                menu_setup(ui);
                call_items(ui, cx, id);
                relationship_items(ui, cx, id, ContextLocation::Friends);
            });
        } else if resp.clicked() {
            run(cx, action, id);
        }
    }
    // A friend's activity picture left of the actions (`members::art_slot`, docs/DESIGN.md §10.4).
    if kind == FRIEND && let Some(a) = crate::activity::activity_of(cx.state, id).map(std::borrow::Cow::into_owned) {
        let r = Rect::from_min_size(pos2(right - super::members::ART, row.center().y - super::members::ART / 2.0), Vec2::splat(super::members::ART));
        let tip = super::members::art_slot(ui, cx, &a, r, ring);
        ui.interact(r, ui.make_persistent_id(("rc_friend_art", id)), Sense::hover()).on_hover_text(tip);
        right -= super::members::ART + 10.0;
    }

    let text_left = avatar.right() + 12.0;
    let max_w = right - 8.0 - text_left;
    let name_g = one_line(ui, name.as_str(), FontId::new(pal.size, crate::theme::medium_family()), pal.text, max_w);
    let name_w = name_g.size().x;
    let status = match kind {
        FRIEND => user_status_text(cx.state, id).or_else(|| Some((None, sidebar::presence_label(sidebar::user_presence(cx.state, id)).to_owned()))),
        INCOMING => Some((None, "Incoming friend request".to_owned())),
        OUTGOING => Some((None, "Outgoing friend request".to_owned())),
        BLOCKED => Some((None, "Blocked".to_owned())),
        _ => None,
    };
    name_and_status(ui, cx, name_g, pal.text, status.as_ref(), pos2(text_left, row.center().y), max_w);
    // The username next to the name on hover.
    if hot && let Some(u) = cx.state.users.get(&id).filter(|u| u.username != name) {
        let left = text_left + name_w + 6.0;
        let g = one_line(ui, u.username.as_str(), FontId::proportional(pal.meta()), pal.muted, max_w - name_w - 6.0);
        let y = if status.is_some() { row.center().y - g.size().y + 1.0 } else { row.center().y - g.size().y / 2.0 };
        ui.painter().galley(pos2(left, y), g, pal.muted);
    }

    // Registered last so the row (outside its buttons) takes the click: their DM, else their card.
    let resp = ui.interact(row.with_max_x(right + 4.0), ui.make_persistent_id(("rc_friend_row", id)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
    let at = (pos2(row.center().x, row.top()), egui::Align2::CENTER_TOP);
    sidebar::user_context_menu(&resp, cx, id, None, at);
    if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() && !clicked_action {
        if kind == FRIEND {
            message_user(cx, id);
        } else {
            cx.view.open_profile_at(id, None, at);
        }
    }
}

/// A user's 32 px avatar (their initial while uncached).
fn friend_avatar(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, id: Id, name: &str, rect: Rect) {
    match cx.state.users.get(&id) {
        Some(u) => {
            let url = super::images::user_avatar_url(None, u, None, 64, false);
            user_avatar_at(ui, cx.images, &url, rect, false, 1.0, (id, name));
        }
        None => super::images::paint_initial(ui.painter(), rect, 16.0, id.0, name, pal.dark),
    }
}

/// Active Now (layout §4.3): online friends with an activity (game, music, stream), one card
/// each; a card opens their profile card.
// shortcut: friends in voice channels are not listed (no cross-server voice lookup yet).
fn active_now(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal) {
    let r = egui::Panel::right("rc_active_now")
        .exact_size(ACTIVE_NOW_W)
        .resizable(false)
        .show_separator_line(false)
        .frame(Frame::new().inner_margin(Margin::same(16)))
        .show(ui, |ui| {
            ui.label(RichText::new("Active now").font(bold(pal.size + 4.0)).color(pal.text));
            ui.add_space(8.0);
            let s = &*cx.state;
            let active: Vec<Id> = s.friends_tab(FriendsTab::Online).into_iter().filter(|id| s.presence_of(*id).is_some_and(|p| p.activity.is_some())).collect();
            if active.is_empty() {
                ui.add_space(8.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("It's quiet for now…").font(bold(pal.size)).color(pal.text));
                    ui.add(egui::Label::new(RichText::new("Friends' games and music show up here.").size(pal.meta()).color(pal.muted)).wrap());
                });
                return;
            }
            egui::ScrollArea::vertical().id_salt("rc_active_now").auto_shrink(false).show(ui, |ui| {
                for id in active {
                    active_card(ui, cx, pal, id);
                }
            });
        })
        .response
        .rect;
    ui.painter().vline(r.left() + 0.5, r.y_range(), Stroke::new(1.0, pal.line()));
}

/// One Active Now card: avatar with status, name and status line over the activity
/// (`profile::activity_section`).
fn active_card(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, id: Id) {
    let Some(activity) = cx.state.presence_of(id).and_then(|p| p.activity.clone()) else { return };
    let name = cx.state.friend_name(id);
    let fill_slot = ui.painter().add(Shape::Noop);
    let inner = Frame::new().inner_margin(Margin::same(12)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 10.0;
        let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
        let avatar = Rect::from_min_size(head.left_top(), Vec2::splat(32.0));
        friend_avatar(ui, cx, pal, id, &name, avatar);
        user_dot(ui.painter(), avatar, cx.state, id, &cx.theme.colors, pal.bg);
        let (x, w) = (avatar.right() + 12.0, head.right() - avatar.right() - 12.0);
        let n = one_line(ui, name.as_str(), bold(pal.size), pal.text, w);
        name_and_status(ui, cx, n, pal.text, user_status_text(cx.state, id).as_ref(), pos2(x, head.center().y), w);
        super::profile::activity_section(ui, cx, pal, id, &activity, 48.0, pal.bg);
    });
    // Registered after the avatar widget so the whole card stays clickable.
    let rect = inner.response.rect;
    let resp = ui.interact(rect, ui.make_persistent_id(("rc_active_card", id)), Sense::click());
    let fill = if resp.hovered() { pal.tint() } else { Color32::TRANSPARENT };
    ui.painter().set(fill_slot, egui::epaint::RectShape::new(rect, pal.card_radius(), fill, Stroke::new(1.0, pal.line()), egui::StrokeKind::Inside));
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
    if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
        cx.view.open_profile_at(id, None, (pos2(rect.left() - 8.0, rect.top()), egui::Align2::RIGHT_TOP));
    }
    ui.add_space(8.0);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    Message,
    More,
    Accept,
    /// Ignore an incoming request / cancel an outgoing one.
    Remove,
    Unblock,
}

fn run(cx: &mut UiCx<'_>, action: Action, user: Id) {
    let location = ContextLocation::Friends;
    match action {
        Action::Message => message_user(cx, user),
        Action::Accept => add_friend(cx, user, location),
        Action::Remove | Action::Unblock => remove(cx, user, location),
        Action::More => {}
    }
}

/// 36 px round action button on the row (`row_hot`: the row is hovered, so the circle shows
/// the hover colour). Accept turns green, Ignore / Cancel red on hover.
pub(super) fn round_button(ui: &mut egui::Ui, cx: &UiCx<'_>, pal: &Pal, (user, rect): (Id, Rect), action: Action, tip: &str, row_hot: bool) -> egui::Response {
    let resp = ui.interact(rect, ui.make_persistent_id(("rc_friend_action", user, action as u8)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tip));
    let hot = resp.hovered() || resp.has_focus();
    let p = ui.painter();
    let fill = if row_hot { pal.surface.lerp_to_gamma(pal.bg, 0.3) } else { pal.surface };
    p.circle_filled(rect.center(), 18.0, fill);
    let hot_color = match action {
        Action::Accept => presence_color(Presence::Online, &cx.theme.colors),
        Action::Remove => pal.danger,
        _ => pal.text,
    };
    let color = if hot { hot_color } else { pal.muted };
    let icon = match action {
        Action::More => Icon::More,
        Action::Accept => Icon::Check,
        Action::Remove => Icon::Close,
        Action::Message => Icon::Message,
        Action::Unblock => Icon::Block,
    };
    icons::paint(p, icon, Rect::from_center_size(rect.center(), Vec2::splat(20.0)), color, fill);
    resp.on_hover_text(tip).on_hover_cursor(CursorIcon::PointingHand)
}

// ---------------------------------------------------------------------------
// Add Friend tab
// ---------------------------------------------------------------------------

fn add_friend_page(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal) {
    let green = presence_color(Presence::Online, &cx.theme.colors);
    Frame::new().inner_margin(Margin::symmetric(30, 20)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.label(RichText::new("Add friend").font(super::semibold(pal.title())).color(pal.text));
        ui.label(RichText::new("You can add friends with their username.").color(pal.muted));
        let name = cx.view.friends_ui.add_name.trim().to_owned();
        // The answer to what is in the field now (a new name hides the old answer).
        let answer = cx.state.social.add_friend.as_ref().filter(|v| v.username == name).map(|v| v.result.clone());
        let sending = matches!(answer, Some(None));
        let edit_id = ui.make_persistent_id("rc_add_friend_name");
        let focused = ui.memory(|m| m.has_focus(edit_id));
        let border = match &answer {
            Some(Some(Ok(()))) => green,
            Some(Some(Err(_))) => pal.danger,
            _ => pal.tk.edge,
        };
        let mut send = false;
        let field = Frame::new().fill(pal.surface).stroke(Stroke::new(1.0, border)).corner_radius(pal.card_radius()).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
            // Fixed row height: `with_layout` would centre in (and take) all the height left.
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 32.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let button_w = 170.0;
                let edit = egui::TextEdit::singleline(&mut cx.view.friends_ui.add_name)
                    .id(edit_id)
                    .hint_text("Enter a username")
                    .char_limit(37)
                    .frame(Frame::NONE)
                    .font(FontId::proportional(pal.size + 1.0))
                    .desired_width(ui.available_width() - button_w - 8.0);
                let resp = ui.add(edit);
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let ready = !name.is_empty() && !sending;
                send = (super::messages::button(ui, pal, super::messages::Btn::Primary, "Send friend request", ready).clicked() || enter) && ready;
            });
        });
        if focused {
            // The 2 px focus ring replaces the border (§9), as on egui-framed fields.
            ui.painter().rect_stroke(field.response.rect, pal.card_radius(), Stroke::new(2.0, pal.tk.focus), egui::StrokeKind::Inside);
        }
        match answer {
            Some(Some(Ok(()))) => {
                ui.label(RichText::new(format!("Success! Your friend request to {name} was sent.")).color(green));
            }
            Some(Some(Err(e))) => {
                ui.add(egui::Label::new(RichText::new(e).color(pal.danger)).wrap().selectable(true));
            }
            Some(None) => {
                ui.spinner();
            }
            None => {}
        }
        if send {
            cx.state.social.add_friend = Some(AddFriendView { username: name.clone(), result: None });
            cx.cmd.send(Command::Social(SocialCommand::SendFriendRequest { username: name }));
        }
    });
}

// ---------------------------------------------------------------------------
// Shared actions (profile, menus, rows)
// ---------------------------------------------------------------------------

/// "Message": opens the cached DM, else asks for one (`OpenDm`, answered by `ChannelOpened`).
pub fn message_user(cx: &mut UiCx<'_>, user: Id) {
    match cx.state.dm_with(user) {
        Some(dm) => cx.state.select_channel(dm),
        None => cx.cmd.send(Command::Social(SocialCommand::OpenDm { user_id: user })),
    }
    cx.view.profile_card = None;
    cx.view.modal = None;
}

/// Whether "Add Friend" makes sense for `user`: not me, not a bot, not already a friend,
/// requested or blocked (an incoming request: the same call accepts it).
pub fn can_add_friend(state: &AppState, user: Id) -> bool {
    use relationship_type::*;
    let me = state.me.as_ref().is_some_and(|m| m.id == user);
    let bot = state.users.get(&user).is_some_and(|u| u.bot);
    let kind = state.social.relationships.get(&user).map(|r| r.kind);
    !me && !bot && !matches!(kind, Some(FRIEND | OUTGOING | BLOCKED))
}

/// "Add Friend" on a user (profile, menus) or Accept: `AcceptFriend` by id. Only on a click.
pub fn add_friend(cx: &mut UiCx<'_>, user: Id, location: ContextLocation) {
    cx.cmd.send(Command::Social(SocialCommand::AcceptFriend { user_id: user, confirm_stranger: false, location }));
}

/// Ignore / cancel a request, unblock: no question (nothing is lost).
fn remove(cx: &mut UiCx<'_>, user: Id, location: ContextLocation) {
    cx.cmd.send(Command::Social(SocialCommand::RemoveRelationship { user_id: user, location }));
}

/// Remove Friend, after asking.
pub(super) fn ask_remove(cx: &mut UiCx<'_>, user: Id, location: ContextLocation) {
    let name = cx.state.friend_name(user);
    let then = Command::Social(SocialCommand::RemoveRelationship { user_id: user, location });
    cx.view.confirm(format!("Remove {name} as a friend?"), "To be friends again, one of you sends a new friend request.", "Remove friend", then);
}

/// Block, after asking (it also ends a friendship).
pub(super) fn ask_block(cx: &mut UiCx<'_>, user: Id, location: ContextLocation) {
    let name = cx.state.friend_name(user);
    let then = Command::Social(SocialCommand::BlockUser { user_id: user, location });
    cx.view.confirm(format!("Block {name}?"), "They won't be able to message you, and they are removed from your friends.", "Block", then);
}

/// The 80013 answer: "accept a request from someone you share no server with?" (accent
/// button). Clears `confirm_stranger` either way. Call every frame (profile.rs does).
pub(super) fn ask_stranger(cx: &mut UiCx<'_>) {
    if cx.view.confirm.is_some() {
        return;
    }
    let Some(user) = cx.state.social.confirm_stranger.take() else { return };
    let name = cx.state.friend_name(user);
    let then = Command::Social(SocialCommand::AcceptFriend { user_id: user, confirm_stranger: true, location: ContextLocation::Friends });
    cx.view.confirm("Accept friend request?", format!("You don't share a server with {name}. Accept their request anyway?"), "Accept", then);
    if let Some(c) = &mut cx.view.confirm {
        c.danger = false;
    }
}

/// Social items of a user's menu (member list, voice users, friend rows): Message, then the
/// relationship items.
pub fn user_menu_items(ui: &mut egui::Ui, cx: &mut UiCx<'_>, user: Id) {
    if cx.state.me.as_ref().is_some_and(|m| m.id == user) {
        return;
    }
    if menu_item(ui, "Message", MenuIcon::Glyph("💬"), cx.theme.colors.text.to_color32()) {
        message_user(cx, user);
    }
    call_items(ui, cx, user);
    name_submenu(ui, cx, user);
    relationship_items(ui, cx, user, ContextLocation::ContextMenu);
}

/// Start (or Join) Voice / Video Call: opens our DM with `user` and calls there, like its header
/// buttons (one click, one ring). Only with that DM cached (a new DM would need a second
/// step after `ChannelOpened`) and while I'm not in its call.
fn call_items(ui: &mut egui::Ui, cx: &mut UiCx<'_>, user: Id) {
    let Some(dm) = cx.state.dm_with(user).filter(|dm| cx.state.voice.channel_id != Some(*dm)) else { return };
    let verb = if cx.state.call_in(dm) { "Join" } else { "Start" };
    let text = cx.theme.colors.text.to_color32();
    for (video, what, icon) in [(false, "Voice", Icon::Phone), (true, "Video", Icon::Camera)] {
        // Linux / macOS: no camera yet, so no video call (disabled with the reason).
        let item = ui.add_enabled_ui(!video || crate::voice::video::capture::SUPPORTED, |ui| menu_item(ui, &format!("{verb} {what} Call"), MenuIcon::Icon(icon), text));
        let _ = item.response.on_disabled_hover_text(crate::voice::video::capture::NO_VIDEO_CALL);
        if item.inner {
            super::dm_call::open_and_join(cx, dm, video);
        }
    }
}

/// Add Friend / Accept / Cancel Request / Remove Friend, then Block / Unblock.
fn relationship_items(ui: &mut egui::Ui, cx: &mut UiCx<'_>, user: Id, location: ContextLocation) {
    use relationship_type::*;
    if cx.state.me.as_ref().is_some_and(|m| m.id == user) {
        return;
    }
    let (text, danger) = (cx.theme.colors.text.to_color32(), cx.theme.colors.danger.to_color32());
    let kind = cx.state.social.relationships.get(&user).map(|r| r.kind);
    match kind {
        Some(FRIEND) => {
            if menu_item(ui, "Remove friend", MenuIcon::Glyph("➖"), danger) {
                ask_remove(cx, user, location);
            }
        }
        Some(INCOMING) => {
            if menu_item(ui, "Accept friend request", MenuIcon::Glyph("✔"), text) {
                add_friend(cx, user, location);
            }
        }
        Some(OUTGOING) => {
            if menu_item(ui, "Cancel friend request", MenuIcon::Glyph("✖"), text) {
                remove(cx, user, location);
            }
        }
        _ if can_add_friend(cx.state, user) && menu_item(ui, "Add friend", MenuIcon::Glyph("➕"), text) => add_friend(cx, user, location),
        _ => {}
    }
    if kind == Some(BLOCKED) {
        if menu_item(ui, "Unblock", MenuIcon::Glyph("🔓"), text) {
            remove(cx, user, location);
        }
    } else if menu_item(ui, "Block", MenuIcon::Glyph("🚫"), danger) {
        ask_block(cx, user, location);
    }
}

/// Social items of a DM row's menu: a group's Add Friends, Rename and Leave (asks); a 1:1 DM's
/// relationship items for the other user.
pub fn dm_menu_items(ui: &mut egui::Ui, cx: &mut UiCx<'_>, channel_id: Id) {
    let Some(ch) = cx.state.channels.get(&channel_id) else { return };
    let (group, other) = (ch.kind == channel_type::GROUP_DM, ch.recipient_ids.first().copied());
    let (text, danger) = (cx.theme.colors.text.to_color32(), cx.theme.colors.danger.to_color32());
    if !group {
        if let Some(u) = other {
            name_submenu(ui, cx, u);
            relationship_items(ui, cx, u, ContextLocation::ContextMenu);
        }
        return;
    }
    if menu_item(ui, "Add friends", MenuIcon::Glyph("➕"), text) {
        open_picker(cx, Some(channel_id));
    }
    if menu_item(ui, "Rename group", MenuIcon::Glyph("✏"), text) {
        cx.view.friends_ui.rename = Some(cx.state.channels.get(&channel_id).and_then(|c| c.name.clone()).unwrap_or_default());
        cx.view.modal = Some(Modal::GroupDm { channel_id: Some(channel_id) });
    }
    if menu_item(ui, "Leave group", MenuIcon::Glyph("🚪"), danger) {
        let name = cx.state.channel_name(channel_id);
        let then = Command::Social(SocialCommand::CloseDm { channel_id, silent: false });
        cx.view.confirm(format!("Leave {name}?"), "You can come back only if someone in the group adds you again.", "Leave group", then);
    }
}

// ---------------------------------------------------------------------------
// Edit name (crate::names)
// ---------------------------------------------------------------------------

/// The open "Edit name" form (in `FriendsUi::name_edit`).
#[derive(Debug, Clone, PartialEq)]
pub struct NameDraft {
    user: Id,
    name: String,
    tag: String,
    color: Option<u32>,
}

/// Edit name ▸: a small form in the submenu (no dialog). For a friend the name is Discord's friend
/// nickname (one PATCH on Save, only when it changed); for anyone else a name only this device
/// shows. The tag and colour always stay on this device.
fn name_submenu(ui: &mut egui::Ui, cx: &mut UiCx<'_>, user: Id) {
    use super::messages::{Btn, button};
    use crate::names::{NAME_MAX, PALETTE, TAG_MAX};
    let pal = Pal::new(cx.theme);
    let keep_open = egui::containers::menu::MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    let friend = cx.state.social.relationships.get(&user).is_some_and(|r| r.kind == relationship_type::FRIEND);
    egui::containers::menu::SubMenuButton::new(RichText::new("Edit name").color(pal.text)).config(keep_open).ui(ui, |ui| {
        menu_setup(ui);
        ui.set_width(240.0);
        if cx.view.friends_ui.name_edit.as_ref().is_none_or(|d| d.user != user) {
            let local = cx.settings.custom_names.iter().find(|n| n.id == user).cloned().unwrap_or_default();
            // A local name shows in every list, so it is what the field holds; a friend's Discord nickname otherwise.
            let name = if friend { local.name.clone().or_else(|| cx.state.friend_nickname(user).map(str::to_owned)) } else { local.name.clone() };
            cx.view.friends_ui.name_edit = Some(NameDraft { user, name: name.unwrap_or_default(), tag: local.tag.unwrap_or_default(), color: local.color });
        }
        let hint = cx.state.users.get(&user).map(|u| u.display_name().to_owned()).unwrap_or_default();
        let Some(d) = cx.view.friends_ui.name_edit.as_mut() else { return };
        let meta = |ui: &mut egui::Ui, text: &str| ui.add(egui::Label::new(RichText::new(text).size(pal.meta()).color(pal.muted)).selectable(false));
        meta(ui, "Name");
        let name = ui.add(egui::TextEdit::singleline(&mut d.name).char_limit(NAME_MAX).hint_text(hint).desired_width(f32::INFINITY));
        meta(ui, "Tag");
        let tag = ui.add(egui::TextEdit::singleline(&mut d.tag).char_limit(TAG_MAX).hint_text("Add a tag").desired_width(f32::INFINITY));
        meta(ui, "Colour");
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for pick in std::iter::once(None).chain(PALETTE.iter().map(|&(c, w)| Some((c, w)))) {
                let word = pick.map_or("None", |(_, w)| w);
                let value = pick.map(|(c, _)| c);
                let (r, resp) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
                let on = d.color == value;
                resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, on, word));
                match pick {
                    Some((c, _)) => _ = ui.painter().circle_filled(r.center(), 10.0, super::rgb(c)),
                    None => {
                        let ring = Stroke::new(1.5, pal.tk.edge);
                        ui.painter().circle_stroke(r.center(), 9.25, ring);
                        let o = 9.25 * std::f32::consts::FRAC_1_SQRT_2;
                        ui.painter().line_segment([r.center() + vec2(-o, o), r.center() + vec2(o, -o)], ring);
                    }
                }
                if on {
                    ui.painter().circle_stroke(r.center(), 13.0, Stroke::new(2.0, pal.tk.focus));
                }
                if resp.on_hover_text(word).on_hover_cursor(CursorIcon::PointingHand).clicked() {
                    d.color = value;
                }
            }
        });
        meta(ui, if friend { "Discord keeps the nickname. Tag and colour stay on this device." } else { "Only you see this, on this device." });
        let enter = (name.lost_focus() || tag.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let (reset, save) = ui
            .horizontal(|ui| {
                let reset = button(ui, &pal, Btn::Ghost, "Reset", true).clicked();
                let save = button(ui, &pal, Btn::Primary, "Save", true).clicked();
                (reset, save)
            })
            .inner;
        if reset || save || enter {
            let draft = if reset { NameDraft { user, name: String::new(), tag: String::new(), color: None } } else { d.clone() };
            cx.view.friends_ui.name_edit = None;
            apply_name(cx, draft, friend);
            ui.close();
        }
    });
}

/// Saves the form: the tag and colour (and a stranger's name) on this device, a friend's name as
/// Discord's friend nickname (one request, only when it differs from what Discord has).
fn apply_name(cx: &mut UiCx<'_>, d: NameDraft, friend: bool) {
    let nickname = Some(d.name.trim().to_owned()).filter(|n| !n.is_empty());
    // The field still holds the local name: it stays local and Discord's nickname is not touched.
    let keep_local = friend && nickname.is_some() && nickname == crate::names::name(d.user);
    let entry = crate::names::CustomName { id: d.user, name: if !friend || keep_local { nickname.clone() } else { None }, tag: Some(d.tag), color: d.color };
    if let Err(e) = crate::names::set(&mut cx.settings.custom_names, entry) {
        cx.view.toast(e, true);
    }
    cx.settings.publish();
    if let Err(e) = cx.settings.save() {
        cx.view.toast(format!("Could not save settings: {e}"), true);
    }
    if friend && !keep_local && nickname.as_deref() != cx.state.friend_nickname(d.user) {
        cx.cmd.send(Command::Social(SocialCommand::SetFriendNickname { user_id: d.user, nickname }));
    }
}

// ---------------------------------------------------------------------------
// Group DM picker / rename (Modal::GroupDm)
// ---------------------------------------------------------------------------

/// Opens the New Group DM picker (DM list header "+").
pub fn open_group_dm(cx: &mut UiCx<'_>) {
    open_picker(cx, None);
}

fn open_picker(cx: &mut UiCx<'_>, channel_id: Option<Id>) {
    let f = &mut cx.view.friends_ui;
    (f.picked, f.pick_query, f.rename) = (Vec::new(), String::new(), None);
    cx.view.modal = Some(Modal::GroupDm { channel_id });
}

fn close_modal(cx: &mut UiCx<'_>) {
    let f = &mut cx.view.friends_ui;
    (f.picked, f.pick_query, f.rename) = (Vec::new(), String::new(), None);
    cx.view.modal = None;
}

/// `Modal::GroupDm`: Rename Group (`FriendsUi::rename` set), else pick friends for a new group
/// (one friend: their DM) or to add to `channel_id` (a 1:1 DM becomes a new group with them).
pub fn group_dm_modal(ctx: &egui::Context, cx: &mut UiCx<'_>, channel_id: Option<Id>) {
    if let (Some(c), Some(_)) = (channel_id, &cx.view.friends_ui.rename) {
        return rename_modal(ctx, cx, c);
    }
    let ch = channel_id.and_then(|c| cx.state.channels.get(&c));
    let group = ch.is_some_and(|c| c.kind == channel_type::GROUP_DM);
    let present: Vec<Id> = ch.map(|c| c.recipient_ids.clone()).unwrap_or_default();
    let room = (GROUP_MAX - 1).saturating_sub(present.len());
    let query = cx.view.friends_ui.pick_query.trim().to_lowercase();
    let friends: Vec<Id> = (cx.state.friends_tab(FriendsTab::All).into_iter())
        .filter(|id| !present.contains(id))
        .filter(|id| query.is_empty() || cx.state.friend_name(*id).to_lowercase().contains(&query))
        .collect();
    let title = if channel_id.is_some() { "Add friends to DM" } else { "Select friends" };
    let action = if group { "Add" } else { "Create DM" };
    let picked = cx.view.friends_ui.picked.len();
    let pal = Pal::new(cx.theme);
    let theme = cx.theme.clone();
    let resp = dialog(ctx, &theme, "rc_group_dm", 440.0, title, |ui| {
        let left = room.saturating_sub(picked);
        let note = if left > 0 { format!("You can add {left} more friend{}.", if left == 1 { "" } else { "s" }) } else { "This group is full.".to_owned() };
        dialog_text(ui, &theme, &note);
        let edit = egui::TextEdit::singleline(&mut cx.view.friends_ui.pick_query).hint_text("Type the username of a friend").desired_width(f32::INFINITY);
        ui.add(edit);
        let list_h = (ctx.content_rect().height() - 280.0).clamp(96.0, 280.0);
        egui::ScrollArea::vertical().max_height(list_h).min_scrolled_height(list_h).auto_shrink([false, true]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if friends.is_empty() {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| ui.label(RichText::new("No friends to add.").color(pal.muted)));
                ui.add_space(24.0);
            }
            for &id in &friends {
                pick_row(ui, cx, &pal, id, left > 0);
            }
        });
        dialog_buttons(ui, &theme, action, false, picked > 0)
    });
    match resp.inner {
        Some(true) => {
            let picked = std::mem::take(&mut cx.view.friends_ui.picked);
            match (channel_id, group) {
                // One request per added friend (Discord's client does the same).
                (Some(c), true) => {
                    for user_id in picked {
                        cx.cmd.send(Command::Social(SocialCommand::AddRecipient { channel_id: c, user_id }));
                    }
                }
                (None, _) if picked.len() == 1 => message_user(cx, picked[0]),
                // A 1:1 DM: a new group with the other user and the picked friends.
                _ => {
                    let recipients = present.into_iter().chain(picked).collect();
                    cx.cmd.send(Command::Social(SocialCommand::CreateGroupDm { recipients }));
                }
            }
            close_modal(cx);
        }
        Some(false) => close_modal(cx),
        None if resp.should_close() => close_modal(cx),
        None => {}
    }
}

/// A friend in the picker: avatar, name, username and a check box; `room` = more may be ticked.
fn pick_row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, id: Id, room: bool) {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
    if !ui.is_rect_visible(row) {
        return;
    }
    let on = cx.view.friends_ui.picked.contains(&id);
    let bg = ui.painter().add(Shape::Noop);
    let name = cx.state.friend_name(id);
    let avatar = Rect::from_min_size(pos2(row.left() + 8.0, row.center().y - 16.0), Vec2::splat(32.0));
    if let Some(u) = cx.state.users.get(&id) {
        let url = super::images::user_avatar_url(None, u, None, 64, false);
        user_avatar_at(ui, cx.images, &url, avatar, false, 1.0, (id, &name));
    }
    let resp = ui.interact(row, ui.make_persistent_id(("rc_pick", id)), Sense::click());
    let enabled = on || room;
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, enabled, on, &name));
    if resp.hovered() && enabled {
        ui.painter().set(bg, Shape::rect_filled(row, pal.radius, pal.tint()));
    }
    let left = avatar.right() + 12.0;
    let g = one_line(ui, name.as_str(), FontId::proportional(pal.size), pal.text, row.right() - 40.0 - left);
    let w = g.size().x;
    ui.painter().galley(pos2(left, row.center().y - g.size().y / 2.0), g, pal.text);
    if let Some(u) = cx.state.users.get(&id) {
        let g = one_line(ui, u.username.as_str(), FontId::proportional(pal.meta()), pal.muted, row.right() - 40.0 - left - w - 8.0);
        ui.painter().galley(pos2(left + w + 8.0, row.center().y - g.size().y / 2.0), g, pal.muted);
    }
    let check = Rect::from_center_size(pos2(row.right() - 20.0, row.center().y), Vec2::splat(20.0));
    // The shared checkbox (green when on, never accent); faded once the group is full.
    ui.scope(|ui| {
        ui.multiply_opacity(if on || room { 1.0 } else { 0.4 });
        super::messages::paint_checkbox(ui.painter(), &pal.tk, check, on, f32::from(pal.radius / 2));
    });
    if resp.clicked() && enabled {
        let picked = &mut cx.view.friends_ui.picked;
        match picked.iter().position(|p| *p == id) {
            Some(i) => {
                picked.remove(i);
            }
            None => picked.push(id),
        }
    }
}

/// Rename Group: the name field (empty = no name: the members' names show).
fn rename_modal(ctx: &egui::Context, cx: &mut UiCx<'_>, channel_id: Id) {
    let current = cx.state.channels.get(&channel_id).and_then(|c| c.name.clone()).unwrap_or_default();
    let theme = cx.theme.clone();
    let mut enter = false;
    let resp = dialog(ctx, &theme, "rc_rename_group", 440.0, "Rename group", |ui| {
        let name = cx.view.friends_ui.rename.as_mut()?;
        let r = ui.add(egui::TextEdit::singleline(name).hint_text(cx.state.channel_name(channel_id)).char_limit(100).desired_width(f32::INFINITY));
        if ui.memory(|m| m.focused().is_none()) {
            r.request_focus();
        }
        enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let changed = name.trim() != current;
        dialog_buttons(ui, &theme, "Save", false, changed).or((enter && changed).then_some(true))
    });
    match resp.inner {
        Some(true) => {
            let name = cx.view.friends_ui.rename.take().unwrap_or_default().trim().to_owned();
            cx.cmd.send(Command::Social(SocialCommand::RenameGroup { channel_id, name }));
            close_modal(cx);
        }
        Some(false) => close_modal(cx),
        None if resp.should_close() => close_modal(cx),
        None => {}
    }
}

// ---------------------------------------------------------------------------
// Test hooks
// ---------------------------------------------------------------------------

/// Test hook `RUSTCORD_OPEN=friends`: cached users as friends (two online with a custom status
/// and a game, one busy, one offline), requests (one filed as spam) and blocks, then the
/// Friends page.
pub fn demo(state: &mut AppState) {
    use relationship_type::*;
    let mut ids: Vec<Id> = state.users.keys().copied().filter(|&u| state.me.as_ref().is_none_or(|m| m.id != u)).collect();
    ids.sort();
    let mut friends = Vec::new();
    let mut incoming = 0;
    for (i, id) in ids.into_iter().enumerate() {
        let kind = [FRIEND, FRIEND, INCOMING, OUTGOING, BLOCKED][i % 5];
        incoming += usize::from(kind == INCOMING);
        let is_spam_request = kind == INCOMING && incoming == 2;
        if kind == FRIEND {
            friends.push(id);
        }
        state.social.relationships.insert(id, Relationship { id, kind, is_spam_request, ..Default::default() });
    }
    let custom = CustomStatus { text: Some("Reviewing PRs".into()), emoji: Some(Emoji { name: Some("👀".into()), ..Default::default() }) };
    let mut presences = vec![
        UserPresence { status: Presence::Online, custom_status: Some(custom), ..Default::default() },
        UserPresence { status: Presence::Idle, activity: Some(super::members::demo_activity(0)), ..Default::default() },
        UserPresence { status: Presence::Dnd, ..Default::default() },
    ];
    // More friends (a larger snapshot): one per other activity type.
    presences.extend([2, 3, 1, 5].map(|k| UserPresence { status: Presence::Online, activity: Some(super::members::demo_activity(k)), ..Default::default() }));
    let list = friends.into_iter().zip(presences).collect();
    state.apply_gateway(GatewayEvent::PresenceUpdate(list), None);
    open(state);
}

/// `RUSTCORD_NAV_DEMO=tab:<online|all|pending|blocked|add>` (sidebar::nav_demo): that tab.
fn demo_tab(cx: &mut UiCx<'_>) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let tab = match sidebar::nav_demo_value("tab") {
            Some("all") => Tab::List(FriendsTab::All),
            Some("pending") => Tab::List(FriendsTab::Pending),
            Some("blocked") => Tab::List(FriendsTab::Blocked),
            Some("add") => Tab::AddFriend,
            _ => return,
        };
        cx.view.friends_ui.tab = tab;
    });
}
