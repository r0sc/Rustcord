//! Full user profile (FEATURES_NOTES §2.6, §4.8: "View Full Profile"): `Modal::Profile`, a 600
//! px modal with a 140 px banner and a 120 px avatar, Message / Add Friend / More, names and
//! pronouns, and the tabs About Me (bio, Member Since, roles, game collection, connections,
//! Note; cosmetics from ui/cosmetics.rs: theme, effect, frame, decoration, name style, guild
//! tag, badges), Mutual
//! Servers and Mutual Friends. Data: `fetch` (`request_profile` + `FetchProfile`) when it
//! opens, then `profile(u, g)`. The banner texture is freed when it closes. Also the Note box
//! shared with the popout (`note_field`, `SetNote` on blur).
//! OWNERSHIP: friends/DMs/inbox/profile engineer (docs/SOCIAL_UI_DESIGN.md).

use super::icons::Icon;
use super::messages::{Btn, MenuIcon, Pal, menu_item, menu_setup, vector_button};
use super::sidebar::{self, user_avatar_at, user_dot};
use super::{Modal, UiCx, bold, cosmetics, one_line};
use crate::backend::{Command, SocialCommand};
use crate::model::{
    CollectibleItem, CollectibleProduct, ConnectedAccount, EffectLayer, FrameLayer, Game, GameExtra, Id, MutualGuild, ProfileBadge, ProfileCollectible, ProfileFields,
    ProfileWidget, User, UserProfile, WidgetData, WidgetGame,
};
use crate::state::social::ProfileEntry;
use egui::{CornerRadius, FontId, Frame, Image, Margin, Rect, RichText, Sense, Stroke, Vec2, pos2, vec2};
use std::time::Instant;

const WIDTH: f32 = 600.0;
/// Banner height; 96 in short windows, 72 in very short ones (the avatar 88 there).
const BANNER: f32 = 140.0;
const AVATAR: f32 = 120.0;
/// Space kept above (under the title bar) and below the modal.
const MARGIN: f32 = 12.0;

/// Full-profile state (in `ViewState::profile_modal_ui`).
#[derive(Debug, Default)]
pub struct ProfileModalUi {
    pub tab: Tab,
    /// The Note box being edited (popout or modal): user and text.
    pub note: Option<(Id, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    About,
    Servers,
    Friends,
}

/// "View Full Profile" (profile popout, menus): closes the popout, opens the modal.
pub fn open(cx: &mut UiCx<'_>, user_id: Id, guild_id: Option<Id>) {
    open_tab(cx, user_id, guild_id, Tab::About);
}

/// `open` on a tab (the popout's mutual servers / friends line).
pub(super) fn open_tab(cx: &mut UiCx<'_>, user_id: Id, guild_id: Option<Id>, tab: Tab) {
    save_note(cx);
    cx.view.profile_card = None;
    cx.view.profile_modal_ui = ProfileModalUi { tab, note: None };
    cx.view.modal = Some(Modal::Profile { user_id, guild_id });
    fetch(cx, user_id, guild_id);
}

/// Loads a profile when a card or this modal opens (cached for minutes, never twice at once).
pub(super) fn fetch(cx: &mut UiCx<'_>, user_id: Id, guild_id: Option<Id>) {
    demo_profile(cx, user_id, guild_id);
    if cx.state.request_profile(user_id, guild_id, Instant::now()) {
        cx.cmd.send(Command::Social(SocialCommand::FetchProfile { user_id, guild_id }));
    }
}

/// The loaded profile of (user, guild), if its last answer was one.
pub(super) fn loaded<'a>(cx: &'a UiCx<'_>, user_id: Id, guild_id: Option<Id>) -> Option<&'a UserProfile> {
    cx.state.profile(user_id, guild_id)?.result.as_ref()?.as_ref().ok().map(|p| &**p)
}

/// `Modal::Profile`.
pub fn show(ctx: &egui::Context, cx: &mut UiCx<'_>, user_id: Id, guild_id: Option<Id>) {
    demo_profile(cx, user_id, guild_id);
    let user = cx.state.users.get(&user_id).or(cx.state.me.as_ref().filter(|m| m.id == user_id)).cloned();
    let Some(user) = user.or_else(|| loaded(cx, user_id, guild_id).map(|p| p.user.clone())) else {
        cx.view.modal = None;
        return;
    };
    // Cosmetics (ui/cosmetics.rs): theme, effect, frame, decoration, name style, tag, badges.
    let look = cosmetics::profile_look(cx, user_id, guild_id, true);
    // A themed profile has its own colours and light / dark mode (§8), whatever the app's.
    let app = Pal::new(cx.theme);
    // A modal (E3): `raised`, which the avatar ring and art tiles follow.
    let pal = look.theme.map_or(Pal { surface: app.tk.raised, ..app }, |t| cosmetics::theme_pal(&app, t));
    let r = pal.modal_radius();
    let is_me = cx.state.me.as_ref().is_some_and(|m| m.id == user_id);
    let (banner_url, fill) = super::profile::banner(cx, &user, guild_id, is_me);
    let (width, screen_h) = (WIDTH.min(ctx.content_rect().width() - 48.0), ctx.content_rect().height());
    // The whole modal fits under the title bar with a margin, and so do a frame's top staples
    // (pushed down by their height): the tab body scrolls in what the rest (measured last frame)
    // leaves, or, when that is under 80 px, the whole card scrolls.
    let ot = look.overflow_top(width);
    let room = screen_h - super::TITLE_BAR_H - 2.0 * MARGIN - ot;
    let side = if room < 480.0 { 88.0 } else { AVATAR };
    let banner_h = if room < 480.0 { 72.0 } else if screen_h < 600.0 { 96.0 } else { BANNER };
    let rest_id = egui::Id::unique(("rc_profile_modal_rest", banner_h.to_bits()));
    let rest = ctx.data(|d| d.get_temp::<f32>(rest_id)).unwrap_or(banner_h + 250.0);
    let short = room - rest < 80.0;
    let body_h = (room - rest).clamp(80.0, 320.0);
    let style = ctx.global_style();
    let edge = match look.theme {
        // The theme's gradient outline instead (`cosmetics::theme_outline`).
        Some(_) => Stroke::NONE,
        // High contrast floats have no shadow, a 1.5 px muted edge instead.
        None if pal.tk.mode == crate::theme::Mode::HighContrast => Stroke::new(1.5, pal.muted),
        None => Stroke::new(1.0, pal.tk.rule),
    };
    let frame = Frame::popup(&style).fill(pal.surface).shadow(style.visuals.window_shadow).stroke(edge).corner_radius(r).inner_margin(Margin::ZERO);
    // Centred under that line (its height from last frame); a modal taller than the room is
    // clipped at the window bottom instead of pushed up over the title bar. Not constrained:
    // the area's clip (= its constrain rect) also clips the backdrop, which must dim the whole
    // window like every other modal's.
    let id = egui::Id::unique("rc_profile_modal");
    let top = super::TITLE_BAR_H + MARGIN + ot;
    let h = ctx.memory(|m| m.area_rect(id)).map_or(0.0, |r| r.height());
    let y = top + ((screen_h - top - MARGIN - h) / 2.0).max(0.0);
    let area = egui::Modal::default_area(id).anchor(egui::Align2::CENTER_TOP, vec2(0.0, y)).constrain(false);
    let member = guild_id.and_then(|g| cx.state.find_member(g, user_id)).cloned();
    let fx = crate::overrides::effective_for(&user, member.as_ref());
    let resp = super::motion::modal(ctx, area, frame, pal.tk.scrim, |ui| {
        if look.theme.is_some() {
            cosmetics::theme_visuals(ui, &pal);
        }
        let theme_slot = ui.painter().add(egui::Shape::Noop);
        let behind = ui.ctx().layer_painter(ui.layer_id()).add(egui::Shape::Noop);
        let fx_anim = cosmetics::animates(cx, ui.ui_contains_pointer());
        ui.set_width(width);
        ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
        // The card's own scroll area only scrolls when `short`; else the tab body fits under it.
        ui.set_max_height(room);
        let card_scroll = egui::ScrollArea::vertical().id_salt(("rc_profile_modal_card", user_id)).max_height(room).auto_shrink([false, true]);
        let banner_bottom = card_scroll
            .show(ui, |ui| {
                let top = CornerRadius { nw: r, ne: r, sw: 0, se: 0 };
                let (rect, _) = ui.allocate_exact_size(vec2(width, banner_h), Sense::hover());
                let animated = super::profile::banner_animates(cx, banner_url.as_deref(), is_me, ui.rect_contains_pointer(rect));
                match banner_url.as_deref().and_then(|u| cx.images.get(ui.ctx(), u, 600, animated)) {
                    Some(tex) => super::profile::paint_banner(ui.painter(), rect, top, tex, look.theme.map(|_| cosmetics::theme_veil(pal.dark))),
                    None => {
                        ui.painter().rect_filled(rect, top, super::profile::band_fill(fill, &pal, look.theme.is_some()));
                    }
                }
                let center = pos2(rect.left() + 24.0 + side / 2.0, rect.bottom());
                let ring = look.theme.map_or(pal.surface, |(primary, _)| cosmetics::theme_ring(primary, pal.dark));
                ui.painter().circle_filled(center, side / 2.0 + 7.0, ring);
                let avatar = Rect::from_center_size(center, Vec2::splat(side));
                let name = cx.state.member_display_name(guild_id, &user, None);
                // My local avatar like on my card (profile.rs).
                let url = crate::overrides::my_avatar_url(user_id).map_or_else(|| super::images::user_avatar_url(None, &user, None, 256, false), |u| crate::overrides::large(&u));
                let hovered = ui.rect_contains_pointer(avatar);
                let animate = crate::overrides::my_avatar_animates(user_id, hovered).unwrap_or(hovered);
                user_avatar_at(ui, cx.images, &url, avatar, animate, 1.0, (user_id, &name));
                super::images::decoration_at(ui, cx.images, avatar, fx.decoration.as_ref(), fx_anim, 1.0);
                user_dot(ui.painter(), avatar, cx.state, user_id, &cx.theme.colors, ring);

                // Actions right of the avatar, under the banner.
                let bar = Rect::from_min_max(pos2(avatar.right() + 16.0, rect.bottom() + 12.0), pos2(rect.right() - 16.0, rect.bottom() + 48.0));
                let mut bar = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::right_to_left(egui::Align::Center)));
                actions(&mut bar, cx, &pal, user_id, is_me);
                ui.add_space(side / 2.0 + 12.0);

                Frame::new().inner_margin(Margin { left: 24, right: 24, top: 0, bottom: 20 }).show(ui, |ui| {
                    ui.set_width(width - 48.0);
                    let style = fx.name_style.as_ref();
                    let name_g = cosmetics::name_galley(ui, &name, bold(pal.display()), style, pal.text, width - 48.0);
                    let (r, _) = ui.allocate_exact_size(name_g.size(), Sense::hover());
                    cosmetics::paint_name(ui.painter(), r.min, name_g, style, pal.text, pal.dark);
                    let profile = loaded(cx, user_id, guild_id);
                    let pronouns = profile.and_then(|p| p.pronouns()).map(str::to_owned);
                    let badges = cosmetics::badges(&user, profile);
                    ui.horizontal_wrapped(|ui| {
                        // "alice", 8 px, "they/them": no bullet between them.
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.label(RichText::new(&user.username).color(pal.muted));
                        if let Some(p) = pronouns {
                            ui.label(RichText::new(p).color(pal.muted));
                        }
                        if let Some(pg) = cosmetics::guild_tag(&user, member.as_ref()) {
                            let h = ui.text_style_height(&egui::TextStyle::Body);
                            cosmetics::tag_button(ui, cx, pg, pal.size, h, pal.text);
                        }
                    });
                    cosmetics::badge_row(ui, cx.images, &pal, &badges);
                    ui.add_space(8.0);
                    tabs(ui, cx, &pal, user_id, guild_id);
                    // The rest: everything but the tab body (its 20 px bottom margin included).
                    ui.ctx().data_mut(|d| d.insert_temp(rest_id, ui.cursor().top() - rect.top() + 20.0));
                    if short {
                        body(ui, cx, &pal, &user, guild_id);
                    } else {
                        egui::ScrollArea::vertical().max_height(body_h).min_scrolled_height(body_h).auto_shrink([false, true]).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            body(ui, cx, &pal, &user, guild_id);
                        });
                    }
                });
                rect.bottom()
            })
            .inner;
        let card = ui.min_rect();
        if let Some(colors) = look.theme {
            ui.painter().set(theme_slot, cosmetics::theme_shapes(ui.ctx(), "modal", card, r, colors, banner_bottom, pal.dark));
            ui.painter().add(cosmetics::theme_outline(card, r, colors));
        }
        look.finish(ui, cx.images, card, r, behind, cosmetics::shown_for(ui.ctx(), ("modal", user_id)), fx_anim);
    });
    if resp.should_close() || cx.view.modal != Some(Modal::Profile { user_id, guild_id }) {
        save_note(cx);
        if cx.view.profile_card.is_none() {
            cosmetics::release(ctx, cx.images);
        }
        if let Some(url) = &banner_url {
            cx.images.remove(url);
        }
        if cx.view.modal == Some(Modal::Profile { user_id, guild_id }) {
            cx.view.modal = None;
        }
    }
}

/// Message (the one primary), the friend state, and More (relationship items, Copy user ID).
fn actions(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, user_id: Id, is_me: bool) {
    ui.spacing_mut().item_spacing.x = 8.0;
    if is_me {
        return;
    }
    let more = vector_button(ui, pal, Icon::More, "More", 32.0, false, pal.text);
    egui::Popup::menu(&more).show(|ui| {
        menu_setup(ui);
        super::friends::user_menu_items(ui, cx, user_id);
        // Settings > Advanced > Developer Mode.
        let dev = cx.state.account.settings.developer_mode();
        if dev {
            ui.separator();
        }
        if dev && menu_item(ui, "Copy user ID", MenuIcon::Icon(Icon::IdCard), pal.text) {
            ui.ctx().copy_text(user_id.0.to_string());
        }
    });
    super::profile::friend_button(ui, cx, pal, user_id);
    if super::messages::button(ui, pal, Btn::Primary, "Message", true).clicked() {
        super::friends::message_user(cx, user_id);
    }
}

fn tabs(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, user_id: Id, guild_id: Option<Id>) {
    let (servers, friends) = loaded(cx, user_id, guild_id).map_or((0, 0), |p| (p.mutual_guilds.len(), mutual_friends(p)));
    let is_me = cx.state.me.as_ref().is_some_and(|m| m.id == user_id);
    let mut list = vec![(Tab::About, "About me".to_owned())];
    if !is_me {
        list.push((Tab::Servers, format!("{servers} mutual server{}", if servers == 1 { "" } else { "s" })));
        list.push((Tab::Friends, format!("{friends} mutual friend{}", if friends == 1 { "" } else { "s" })));
    }
    let rect = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 24.0;
            for (tab, label) in list {
                let selected = cx.view.profile_modal_ui.tab == tab;
                let g = ui.painter().layout_no_wrap(label.clone(), FontId::new(pal.ui(), crate::theme::medium_family()), pal.text);
                let (r, resp) = ui.allocate_exact_size(g.size() + vec2(0.0, 12.0), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, &label));
                let color = if selected || resp.hovered() { pal.text } else { pal.muted };
                ui.painter().galley(r.left_top(), g, color);
                if selected {
                    ui.painter().hline(r.x_range(), r.bottom() - 1.0, Stroke::new(2.0, pal.text));
                }
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    save_note(cx);
                    cx.view.profile_modal_ui.tab = tab;
                }
            }
        })
        .response
        .rect;
    ui.painter().hline(rect.x_range(), rect.bottom() + 0.5, Stroke::new(1.0, pal.tk.rule));
    ui.add_space(6.0);
}

fn mutual_friends(p: &UserProfile) -> usize {
    p.mutual_friends_count.map_or(p.mutual_friends.len(), |n| n as usize)
}

fn body(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, user: &User, guild_id: Option<Id>) {
    let entry = cx.state.profile(user.id, guild_id);
    let (loading, error) = (entry.is_some_and(|e| e.loading && e.result.is_none()), entry.and_then(|e| e.result.as_ref()?.as_ref().err().cloned()));
    if loading {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| ui.spinner());
        return;
    }
    if let Some(e) = error {
        ui.label(RichText::new(e).color(pal.muted));
    }
    match cx.view.profile_modal_ui.tab {
        Tab::About => about(ui, cx, pal, user, guild_id),
        Tab::Servers => servers(ui, cx, pal, user.id, guild_id),
        Tab::Friends => friends(ui, cx, pal, user.id, guild_id),
    }
}

/// A section heading of the About tab: the label role, 16 px under the previous section.
fn label(ui: &mut egui::Ui, pal: &Pal, text: &str) {
    ui.add_space(16.0 - ui.spacing().item_spacing.y);
    ui.label(RichText::new(text).font(super::semibold(pal.label())).color(pal.muted));
}

fn about(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, user: &User, guild_id: Option<Id>) {
    let p = loaded(cx, user.id, guild_id);
    let bio = p.and_then(|p| p.bio()).map(str::to_owned);
    let connections = p.map(|p| p.connected_accounts.clone()).unwrap_or_default();
    let widgets = p.map(|p| p.widgets.clone()).unwrap_or_default();
    let note = p.map(|p| p.note.clone().unwrap_or_default());
    if let Some(bio) = bio {
        let opts = super::markdown::MdOpts { guild_id, mentions: &[], id_salt: user.id, color: Some(pal.text), highlight: &[], bg: None };
        super::markdown::render(ui, cx, &bio, &opts);
    }
    if let Some(a) = crate::activity::activity_of(cx.state, user.id).map(std::borrow::Cow::into_owned) {
        label(ui, pal, &a.heading());
        super::profile::activity_section(ui, cx, pal, user.id, &a, 64.0, pal.surface);
    }
    label(ui, pal, "Member since");
    ui.label(RichText::new(super::profile::member_since(user.id)).size(pal.ui()).color(pal.text));
    let member = guild_id.and_then(|g| cx.state.find_member(g, user.id)).cloned();
    let roles = super::profile::guild_roles(cx, guild_id, member.as_ref());
    if !roles.is_empty() {
        label(ui, pal, "Roles");
        super::profile::role_pills(ui, pal, &roles, usize::MAX);
    }
    // Connections and the game collection (profile widgets): ui/cosmetics.rs.
    if connections.iter().any(|a| cosmetics::shown_connection(&a.kind)) {
        label(ui, pal, "Connections");
        cosmetics::connections_grid(ui, pal, &connections);
    }
    cosmetics::game_collection(ui, cx, pal, &widgets);
    if let Some(note) = note.filter(|_| cx.state.me.as_ref().is_none_or(|m| m.id != user.id)) {
        label(ui, pal, "Note");
        note_field(ui, cx, user.id, &note);
    }
}

fn servers(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, user_id: Id, guild_id: Option<Id>) {
    let list: Vec<MutualGuild> = loaded(cx, user_id, guild_id).map(|p| p.mutual_guilds.clone()).unwrap_or_default();
    if list.is_empty() {
        sidebar::empty_state(ui, cx, 24.0, "No servers in common.");
    }
    for m in list {
        let Some(g) = cx.state.guild(m.id) else { continue };
        let name = g.name.clone();
        let icon = crate::api::cdn::guild_icon_url(g, 64, false);
        if list_row(ui, cx, pal, ("rc_mutual_guild", m.id), &name, m.nick.as_deref(), |ui, cx, r| {
            match &icon {
                Some(url) => {
                    if let Some(tex) = cx.images.get(ui.ctx(), url, 64, false) {
                        Image::from_texture(tex).corner_radius(10).paint_at(ui, r);
                    }
                }
                None => super::images::paint_initial(ui.painter(), r, 10.0, m.id.0, &name, pal.dark),
            }
        }) {
            cx.state.select_guild(Some(m.id));
            cx.view.settings_open = false;
            cx.view.modal = None;
        }
    }
}

fn friends(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, user_id: Id, guild_id: Option<Id>) {
    let list: Vec<User> = loaded(cx, user_id, guild_id).map(|p| p.mutual_friends.clone()).unwrap_or_default();
    if list.is_empty() {
        sidebar::empty_state(ui, cx, 24.0, "No friends in common.");
    }
    for u in list {
        let name = u.display_name().to_owned();
        let url = super::images::user_avatar_url(None, &u, None, 64, false);
        let clicked = list_row(ui, cx, pal, ("rc_mutual_friend", u.id), &name, Some(&u.username), |ui, cx, r| {
            user_avatar_at(ui, cx.images, &url, r, false, 1.0, (u.id, &name));
        });
        if clicked {
            open_tab(cx, u.id, None, Tab::About);
        }
    }
}

/// A 48 px clickable row: a 32 px picture (`paint`), the name and a muted second part.
fn list_row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, salt: impl egui::AsIdSalt, name: &str, sub: Option<&str>, paint: impl FnOnce(&mut egui::Ui, &mut UiCx<'_>, Rect)) -> bool {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::hover());
    let bg = ui.painter().add(egui::Shape::Noop);
    let pic = Rect::from_min_size(pos2(row.left() + 8.0, row.center().y - 16.0), Vec2::splat(32.0));
    paint(ui, cx, pic);
    let resp = ui.interact(row, ui.make_persistent_id(salt), Sense::click());
    if resp.hovered() {
        ui.painter().set(bg, egui::Shape::rect_filled(row, pal.radius, pal.tk.hover));
    }
    let left = pic.right() + 12.0;
    let g = one_line(ui, name, bold(pal.size), pal.text, row.right() - left - 8.0);
    let w = g.size().x;
    ui.painter().galley(pos2(left, row.center().y - g.size().y / 2.0), g, pal.text);
    if let Some(sub) = sub {
        let g = one_line(ui, sub, FontId::proportional(pal.meta()), pal.muted, row.right() - left - w - 16.0);
        ui.painter().galley(pos2(left + w + 8.0, row.center().y - g.size().y / 2.0), g, pal.muted);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// The Note box (≤ 256 chars, only I see it): saved with `SetNote` when it loses focus and
/// changed; `saved` is the loaded note.
pub(super) fn note_field(ui: &mut egui::Ui, cx: &mut UiCx<'_>, user_id: Id, saved: &str) {
    let draft = &mut cx.view.profile_modal_ui.note;
    if draft.as_ref().is_none_or(|(u, _)| *u != user_id) {
        *draft = Some((user_id, saved.to_owned()));
    }
    let Some((_, text)) = draft.as_mut() else { return };
    let edit = egui::TextEdit::multiline(text).hint_text("Click to add a note").char_limit(256).desired_rows(1).desired_width(f32::INFINITY).frame(Frame::NONE);
    let resp = ui.add(edit);
    // Frameless, so egui draws no focused border: the house ring shows the field has focus.
    if resp.has_focus() {
        super::messages::focus_ring(ui, &Pal::new(cx.theme), resp.rect.expand2(vec2(4.0, 1.0)), 2.0);
    }
    if resp.lost_focus() && text.trim() != saved.trim() {
        let text = text.clone();
        send_note(cx, user_id, &text);
    }
}

/// A note still focused when its card / modal closes: saved like on blur.
pub(super) fn save_note(cx: &mut UiCx<'_>) {
    let Some((user_id, text)) = cx.view.profile_modal_ui.note.take() else { return };
    let known = cx.state.social.profiles.iter().filter(|e| e.user_id == user_id).find_map(|e| e.result.as_ref()?.as_ref().ok()?.note.clone()).unwrap_or_default();
    if text.trim() != known.trim() {
        send_note(cx, user_id, &text);
    }
}

/// `SetNote`, and the cached profiles show it at once (USER_NOTE_UPDATE confirms), so a later
/// blur or close does not send it again.
fn send_note(cx: &mut UiCx<'_>, user_id: Id, text: &str) {
    let note = text.trim().to_owned();
    for e in cx.state.social.profiles.iter_mut().filter(|e| e.user_id == user_id) {
        if let Some(Ok(p)) = e.result.as_mut() {
            p.note = Some(note.clone());
        }
    }
    cx.cmd.send(Command::Social(SocialCommand::SetNote { user_id, note }));
}

/// `RUSTCORD_NAV_DEMO=profile`: a loaded profile with a bio, pronouns, the cached servers and
/// users as mutual ones and a note (no request), plus cosmetics: a theme, a profile effect and
/// frame (`DEMO_EFFECT` / `DEMO_FRAME`, products put into the state), badges, connections and a
/// game collection (`DEMO_GAMES`). Their images come from the image cache offline (seed it with
/// the URLs `api::cdn` builds for them).
fn demo_profile(cx: &mut UiCx<'_>, user_id: Id, guild_id: Option<Id>) {
    if !sidebar::nav_demo("profile") {
        return;
    }
    super::guild_card::demo(cx, user_id);
    if cx.state.profile(user_id, guild_id).is_some() {
        return;
    }
    let Some(user) = cx.state.users.get(&user_id).cloned() else { return };
    let collectibles = [(DEMO_EFFECT, 1), (DEMO_FRAME, 3)].map(|(sku_id, kind)| ProfileCollectible { sku_id, kind, expires_at: None }).to_vec();
    let fields = ProfileFields {
        bio: Some("Rustacean, **egui** fan and part-time *borrow checker whisperer*.".into()),
        pronouns: Some("they/them".into()),
        theme_colors: Some(vec![0x5348CA, 0x140730]),
        collectibles,
        ..Default::default()
    };
    let mutual_guilds = cx.state.guilds.iter().map(|g| MutualGuild { id: g.id, nick: None }).collect();
    let mutual_friends = cx.state.users.values().filter(|u| u.id != user_id).take(3).cloned().collect();
    let badge = |id: &str, icon: &str, description: &str| ProfileBadge { id: id.into(), description: description.into(), icon: icon.into(), link: None };
    let badges = vec![
        badge("hypesquad_house_1", "8a88d63823d8a71cd5e390baa45efa02", "HypeSquad Bravery"),
        badge("active_developer", "6bdc42827a38498929a4920da12695d9", "Active Developer"),
        badge("premium", "43aa87a32b12a410943ab55ccf290b34", "Subscriber"),
        badge("quest_completed", "7d9ae358c8c5e118768335dbe68b4fb8", "Completed a Quest"),
    ];
    let account = |kind: &str, name: &str, verified: bool| ConnectedAccount { kind: kind.into(), id: format!("{name}-id"), name: name.into(), verified, ..Default::default() };
    let connected_accounts = vec![account("github", "ferris-dev", true), account("steam", "crabby", false), account("spotify", "Ferris", true), account("xbox", "FerrisCrab", false)];
    let game = |i: usize| WidgetGame { game_id: DEMO_GAMES[i].0, ..Default::default() };
    let favorite = WidgetGame { comment: Some("Megabase number four is going fine.".into()), tags: vec!["expert".into(), "open_to_teach".into()], ..game(0) };
    let widget = |kind: &str, games: Vec<WidgetGame>| ProfileWidget { data: WidgetData { kind: kind.into(), games } };
    let widgets = vec![widget("favorite_games", vec![favorite]), widget("played_games", vec![game(1), game(2), game(3)])];
    let profile = UserProfile {
        user,
        user_profile: fields,
        mutual_guilds,
        mutual_friends,
        note: Some("Met at RustConf".into()),
        badges,
        connected_accounts,
        widgets,
        premium_since: Some("2021-03-04T00:00:00+00:00".into()),
        ..Default::default()
    };
    demo_cosmetics(cx);
    let entry = ProfileEntry { user_id, guild_id, fetched: Instant::now(), loading: false, result: Some(Ok(Box::new(profile))) };
    cx.state.social.profiles.push_back(entry);
}


/// Demo profile effect / frame SKUs and games (`demo_profile`).
const DEMO_EFFECT: Id = Id(1565443437363201001);
const DEMO_FRAME: Id = Id(1565443437363201002);
const DEMO_GAMES: [(Id, &str); 4] = [
    (Id(1565443437363201010), "Factorio"),
    (Id(1565443437363201011), "Celeste"),
    (Id(1565443437363201012), "Outer Wilds"),
    (Id(1565443437363201013), "Hades"),
];

/// The demo products and games, as `FetchProduct` / `FetchGames` would leave them (no request).
fn demo_cosmetics(cx: &mut UiCx<'_>) {
    let s = &mut cx.state.social;
    let effect = CollectibleItem {
        kind: 1,
        sku_id: DEMO_EFFECT,
        reduced_motion_src: Some("assets/profile_effects/rustcord_demo/reduced.png".into()),
        effects: vec![EffectLayer { src: "assets/profile_effects/rustcord_demo/intro.png".into(), looping: true, width: Some(450.0), height: Some(880.0), duration: 3000.0, ..Default::default() }],
        ..Default::default()
    };
    let layer = FrameLayer { id: "rcdemo".into(), ..Default::default() };
    let frame = CollectibleItem { kind: 3, sku_id: DEMO_FRAME, inner_width: Some(1200.0), overflow_top: Some(110.0), overflow_horizontal: Some(30.0), layers: vec![layer], ..Default::default() };
    for item in [effect, frame] {
        s.products.insert(item.sku_id, Some(Box::new(CollectibleProduct { sku_id: item.sku_id, kind: item.kind, items: vec![item], ..Default::default() })));
    }
    for (i, (id, name)) in DEMO_GAMES.into_iter().enumerate() {
        // Covers for the first two, the app icon for the third, the name alone for the last.
        let cover = (i < 2).then(|| format!("{}/app-assets/{id}/demo-cover.png", crate::api::cdn::CDN));
        let extra = cover.map(|c| GameExtra { cover_image_url: Some(c) });
        let icon_hash = (i == 2).then(|| "democon".into());
        s.games.insert(id, Some(Game { id, name: name.into(), icon_hash, supplemental_game_data: extra }));
    }
}
