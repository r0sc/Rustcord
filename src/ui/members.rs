//! Member sidebar from the lazy member list (groups + members with status icon and custom
//! status / activity line); for group DMs the recipients. Clicking a member opens the profile
//! card.
//! OWNERSHIP: ui-shell implementer.

use super::sidebar::{name_and_status, row_fills, status_dot, status_tooltip, user_avatar_at, user_dot, user_presence, user_status_text};
use super::{UiCx, cosmetics, one_line, rgb};
use crate::backend::ConnState;
use crate::config::AnimateMode;
use crate::model::{Activity, CustomStatus, Emoji, GatewayEvent, Id, Member, MemberListItem, Presence, RichPresence, Role, User, UserPresence, channel_type};
use crate::state::AppState;
use egui::{Color32, CornerRadius, FontId, Rect, Role as A11yRole, Sense, Shape, Vec2, WidgetInfo, pos2, vec2};
use std::borrow::Cow;

/// Fixed row height (groups and members) so the list can be virtualized.
const ROW_H: f32 = 42.0;
const AVATAR: f32 = 32.0;
const PAD: f32 = 16.0;
/// Activity art right in a row (DM rows too), and its CDN / decode size.
pub(super) const ART: f32 = 20.0;
const ART_PX: u32 = 64;

/// Whether the selected channel has a member sidebar (guild channel or group DM). Offline with
/// no cached guild member list there is nothing to show, so the message area takes the space.
pub fn available(state: &AppState) -> bool {
    let Some(ch) = state.selected_channel else { return false };
    match state.selected_guild {
        Some(g) => {
            let empty = state.member_lists.get(&g).is_none_or(|l| l.items.is_empty());
            !(empty && (crate::config::offline_mode() || state.conn == ConnState::Offline))
        }
        None => state.channels.get(&ch).is_some_and(|c| c.kind == channel_type::GROUP_DM),
    }
}

pub fn show(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    ui.spacing_mut().item_spacing.y = 0.0;
    match cx.state.selected_guild {
        Some(guild_id) => guild_members(ui, cx, guild_id),
        None => group_dm_members(ui, cx),
    }
}

fn guild_members(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id) {
    let total = cx.state.member_lists.get(&guild_id).map_or(0, |l| l.items.len());
    // Offline with no list the panel is hidden (`available`), so empty means loading.
    if total == 0 {
        skeleton(ui, cx);
        return;
    }
    crate::plugins::people::member_count(ui, cx, guild_id);
    egui::ScrollArea::vertical()
        .id_salt(("members", guild_id))
        .auto_shrink(false)
        .show_rows(ui, ROW_H, total, |ui, range| {
            for i in range {
                // Cloned so rendering can borrow `cx` mutably; only visible rows get here.
                let Some(item) = cx.state.member_lists.get(&guild_id).and_then(|l| l.items.get(i)).cloned() else { break };
                match item {
                    MemberListItem::Group { id, count } => {
                        let roles = cx.state.guild(guild_id).map(|g| g.roles.as_slice()).unwrap_or_default();
                        let label = group_label(&id, roles);
                        group_row(ui, cx, &label, count as usize);
                    }
                    MemberListItem::Member { member, status } => match &member.user {
                        Some(user) => member_row(ui, cx, Some(guild_id), user, Some(&member), Some(status)),
                        None => {
                            ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
                        }
                    },
                }
            }
        });
}

/// Group DM: me plus the recipients.
fn group_dm_members(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    let Some(ch) = cx.state.selected_channel.and_then(|id| cx.state.channels.get(&id)) else { return };
    let me = cx.state.me.clone();
    let my_id = me.as_ref().map(|u| u.id);
    let users: Vec<User> = me
        .into_iter()
        .chain(ch.recipient_ids.iter().filter(|&&id| Some(id) != my_id).filter_map(|id| cx.state.users.get(id).cloned()))
        .collect();
    egui::ScrollArea::vertical().id_salt("group_dm_members").auto_shrink(false).show_rows(
        ui,
        ROW_H,
        users.len() + 1,
        |ui, range| {
            for i in range {
                match i.checked_sub(1) {
                    None => group_row(ui, cx, "Members", users.len()),
                    Some(u) => member_row(ui, cx, None, &users[u], None, None),
                }
            }
        },
    );
}

/// Section header text: role name for hoisted-role groups.
fn group_label(id: &str, roles: &[Role]) -> String {
    match id {
        "online" => "Online".into(),
        "offline" => "Offline".into(),
        _ => id
            .parse::<u64>()
            .ok()
            .and_then(|rid| roles.iter().find(|r| r.id.0 == rid))
            .map_or_else(|| "Members".into(), |r| r.name.clone()),
    }
}

/// Loading state: a few faint avatar + name silhouettes instead of a "Loading…" sentence.
fn skeleton(ui: &mut egui::Ui, cx: &UiCx<'_>) {
    let tint = cx.theme.colors.text.to_color32().gamma_multiply(0.07);
    let (area, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H * 7.0), Sense::hover());
    resp.widget_info(|| WidgetInfo::labeled(A11yRole::Label, true, "Loading members"));
    let p = ui.painter();
    p.rect_filled(Rect::from_min_size(pos2(area.left() + PAD, area.top() + ROW_H - 16.0), vec2(72.0, 8.0)), 4, tint);
    for (i, w) in [112.0, 84.0, 128.0, 96.0, 72.0, 104.0].into_iter().enumerate() {
        let cy = area.top() + ROW_H * (i as f32 + 1.5);
        p.circle_filled(pos2(area.left() + PAD + AVATAR / 2.0, cy), AVATAR / 2.0, tint);
        p.rect_filled(Rect::from_min_size(pos2(area.left() + PAD + AVATAR + 12.0, cy - 5.0), vec2(w, 10.0)), 5, tint);
    }
}

/// Group header: the label role on the left, the count in meta right-aligned, so the counts
/// of all groups form one column (docs/DESIGN.md §10.4).
fn group_row(ui: &mut egui::Ui, cx: &UiCx<'_>, label: &str, count: usize) {
    let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    resp.widget_info(|| WidgetInfo::labeled(A11yRole::Label, true, format!("{label}, {count}")));
    let (color, size) = (cx.theme.colors.text_muted.to_color32(), cx.theme.font.size);
    let n = one_line(ui, count.to_string(), FontId::proportional(size - 3.0), color, row.width());
    let g = one_line(ui, label, super::semibold(size - 2.0), color, row.width() - 2.0 * PAD - n.size().x - 8.0);
    let base = row.bottom() - 6.0;
    ui.painter().galley(pos2(row.right() - PAD - n.size().x, base - n.size().y), n, color);
    ui.painter().galley(pos2(row.left() + PAD, base - g.size().y), g, color);
}

/// 32 px avatar with status icon, the name in its role color over the custom status or
/// activity. Offline members are dimmed (avatar faded, muted name that stays readable in High
/// Contrast) until hovered, without icon or line. `listed`: the member list's status for the row.
fn member_row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Option<Id>, user: &User, member: Option<&Member>, listed: Option<Presence>) {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let bg_slot = ui.painter().add(Shape::Noop);
    let hot = ui.rect_contains_pointer(row);
    // Mine before the gateway names my status: the list's (my group says I'm online), not offline.
    let is_me = cx.state.me.as_ref().is_some_and(|m| m.id == user.id);
    let own_listed = listed.filter(|_| is_me && cx.state.chat.my_status.is_none());
    let offline = own_listed.unwrap_or_else(|| user_presence(cx.state, user.id)) == Presence::Offline;
    let name = cx.state.member_display_name(guild_id, user, member);
    // Cosmetics (ui/cosmetics.rs): nameplate under the row, decoration, name style, guild tag.
    let fx = crate::overrides::effective_for(user, member);
    let (dim, fx_anim) = (offline && !hot, cosmetics::animates(cx, hot));
    // Filled once the text's extent is known (light themes fade the art in after it).
    let plate = ui.painter().add(Shape::Noop);

    let avatar = Rect::from_min_size(pos2(row.left() + PAD, row.center().y - AVATAR / 2.0), Vec2::splat(AVATAR));
    let animate = crate::overrides::my_avatar_animates(user.id, hot).unwrap_or(cx.settings.animate_others == AnimateMode::Always);
    let url = super::images::user_avatar_url(guild_id, user, member, 64, animate);
    let opacity = if dim { 0.35 } else { 1.0 };
    user_avatar_at(ui, cx.images, &url, avatar, animate, opacity, (user.id, &name));
    super::images::decoration_at(ui, cx.images, avatar, fx.decoration.as_ref(), fx_anim, opacity);

    let c = &cx.theme.colors;
    let mut ring = c.sidebar.to_color32();
    // Hovered, or its popout is open (like Discord's): the row's fill and the person's aura.
    let open = cx.view.profile_card.is_some_and(|t| t.user_id == user.id);
    if hot || open {
        ring = row_fills(ui, c).1;
        let bg = row.shrink2(vec2(8.0, 1.0));
        let radius = cx.theme.layout.corner_radius;
        let mut shapes = vec![Shape::rect_filled(bg, CornerRadius::from(radius), ring)];
        if let Some(a) = super::aura::of(ui.ctx(), cx, user, guild_id) {
            let alpha = super::aura::alpha(cx.settings.people_colors, cx.theme.tokens().mode) * if open { 1.0 } else { 0.75 };
            shapes.push(super::aura::row_wash(ui.ctx(), bg, radius as u8, a, alpha));
            // Under the status dot: the wash near its strong end.
            ring = ring.blend(a.gamma_multiply(alpha * 0.85));
        }
        ui.painter().set(bg_slot, Shape::Vec(shapes));
    }
    match own_listed {
        _ if offline => {}
        Some(p) => {
            status_dot(ui.painter(), avatar, p, c, ring);
        }
        None => {
            user_dot(ui.painter(), avatar, cx.state, user.id, c, ring);
        }
    }
    // A colour given with Edit name replaces the role colour (this screen only).
    let role_color = crate::names::color(user.id).map(rgb).or_else(|| guild_id.zip(member).and_then(|(g, m)| cx.state.role_color(g, &m.roles)).map(rgb)).or_else(|| crate::plugins::people::irc_color(guild_id, user.id, ui.visuals().dark_mode));
    let color = if offline { c.text_muted.to_color32() } else { role_color.map_or(c.text.to_color32(), |rc| cx.theme.readable_on_pane(rc, ring)) };
    let chip_color = if offline { color } else { c.text.to_color32() };
    let (text_left, mut max_w) = (avatar.right() + 12.0, row.right() - PAD - avatar.right() - 12.0);
    // The activity's picture at the row's right end, always (the line may show the custom status).
    let activity = crate::activity::activity_of(cx.state, user.id).filter(|_| !offline).map(Cow::into_owned);
    let mut art_tip = None;
    if let Some(a) = &activity {
        let r = Rect::from_min_size(pos2(row.right() - PAD - ART, row.center().y - ART / 2.0), Vec2::splat(ART));
        let tip = art_slot(ui, cx, a, r, ring);
        art_tip = ui.rect_contains_pointer(r).then_some(tip);
        max_w -= ART + 8.0;
    }
    let size = cx.theme.font.size;
    let tag = cosmetics::guild_tag(user, member);
    // Edit name's tag: a neutral chip right after the name, its width taken before the name is cut.
    let mine = crate::names::tag(user.id).filter(|_| !is_me);
    let mine_g = mine.map(|t| ui.painter().layout_no_wrap(t, FontId::new(super::messages::Pal::new(cx.theme).badge(), crate::theme::bold_family()), chip_color));
    let mine_w = mine_g.as_ref().map_or(0.0, |g| g.size().x + 12.0);
    let chip_w = mine_w + tag.map_or(0.0, |pg| cosmetics::tag_chip_width(ui, pg, size) + 4.0);
    // Discord's role icon: after the name and tag, not on offline rows.
    let role_icon = (!offline).then(|| super::messages::role_icon_of(ui, cx, guild_id, member)).flatten();
    let icon_w = if role_icon.is_some() { 20.0 } else { 0.0 };
    // Plugin tags after the name get their room first: the name is cut with "…" instead.
    let plug_w = crate::plugins::people::member_row_width(ui, cx, guild_id, user, member);
    let style = fx.name_style.as_ref().filter(|_| !offline);
    // Body 15/500 (docs/DESIGN.md §10.4).
    let g = cosmetics::name_galley(ui, &name, FontId::new(size, crate::theme::medium_family()), style, color, (max_w - chip_w - icon_w - plug_w).max(24.0));
    let status = user_status_text(cx.state, user.id).filter(|_| !offline);
    let tip = art_tip.or_else(|| status.as_ref().and_then(|st| status_tooltip(ui, st)));
    let under = ui.painter().add(Shape::Noop);
    let (name_rect, text_end) = name_and_status(ui, cx, g.clone(), color, status.as_ref(), pos2(text_left, row.center().y), max_w);
    ui.painter().set(under, cosmetics::name_underlay(name_rect.min, &g, style, ui.visuals().dark_mode));
    // Chips over a nameplate get the row's own colour under them (opaque on the art).
    let surface = fx.nameplate.as_ref().map(|_| ring);
    if let Some(g) = mine_g {
        let chip = Rect::from_center_size(pos2(name_rect.right() + 4.0 + (g.size().x + 8.0) / 2.0, name_rect.center().y), vec2(g.size().x + 8.0, g.size().y + 2.0));
        let radius = cx.theme.layout.corner_radius.round() as u8 / 2;
        // Offline rows stay muted, chip included.
        let fill = cx.theme.tokens().selected;
        ui.painter().rect_filled(chip, radius, if offline { fill.gamma_multiply(0.6) } else { fill });
        ui.painter().galley(chip.center() - g.size() / 2.0, g, chip_color);
    }
    if let Some(pg) = tag {
        let at = pos2(name_rect.right() + 4.0 + mine_w, name_rect.center().y);
        match surface {
            Some(s) => cosmetics::tag_chip_on(ui, cx.images, pg, at, size, chip_color, s),
            None => cosmetics::tag_chip(ui, cx.images, pg, at, size, chip_color),
        };
    }
    if let Some(role) = &role_icon {
        let at = pos2(name_rect.right() + 4.0 + chip_w, name_rect.center().y - 8.0);
        super::messages::paint_role_icon(ui, cx.images, role, Rect::from_min_size(at, Vec2::splat(16.0)), chip_color);
    }
    // Plugins: tags, platforms, voice, timeout after the name.
    let plug_w = crate::plugins::people::member_row(ui, cx, guild_id, user, member, pos2(name_rect.right() + 4.0 + chip_w + icon_w, name_rect.center().y), surface);
    if let Some(np) = &fx.nameplate {
        let end = text_end.max(name_rect.right() + 4.0 + chip_w + icon_w + plug_w);
        let (plate_rect, radius) = (row.shrink2(vec2(8.0, 1.0)), cx.theme.layout.corner_radius.round() as u8);
        let shape = cosmetics::nameplate(ui, cx.images, plate_rect, np, fx_anim, dim, radius, end);
        // A scrim keeps the name and status line off the plate: the art's bright spots (dark
        // themes), the palette tint under coloured names (light themes).
        let scrim = (!dim).then(|| cosmetics::plate_scrim(plate_rect, radius, ring, end, crate::theme::is_high_contrast(&cx.theme.colors)));
        ui.painter().set(plate, Shape::Vec(std::iter::once(shape).chain(scrim).collect()));
    }

    // Registered after the avatar widget so the whole row stays clickable.
    let resp = ui.interact(row, ui.make_persistent_id(("member", user.id)), Sense::click());
    resp.widget_info(|| WidgetInfo::labeled(A11yRole::Button, true, &name));
    if resp.has_focus() {
        let pal = super::messages::Pal::new(cx.theme);
        super::messages::focus_ring(ui, &pal, row.shrink2(vec2(10.0, 3.0)), f32::from(pal.radius));
    }
    // The card's top-right corner 8 px left of the row: it opens beside the list and grows from there.
    let at = (pos2(row.left() - 8.0, row.top()), egui::Align2::RIGHT_TOP);
    super::sidebar::user_context_menu(&resp, cx, user.id, guild_id, at);
    // Over a local name or friend nickname: who they really are.
    let real = cx.state.real_display_name(guild_id, user, member);
    let tip = (name != real && ui.rect_contains_pointer(name_rect)).then(|| format!("{real}
@{}", user.username)).or(tip);
    let resp = match tip {
        Some(t) => resp.on_hover_text(t),
        None => resp,
    };
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        cx.view.open_profile_at(user.id, guild_id, at);
    }
}

/// The activity's picture at a row's right end, where Discord shows the rich-presence icon:
/// its art (`activity::art_url`: rich presence, Spotify album art, the app's icon), else its type
/// icon on a tile (streams in the live colour). `bg`: the row's fill. Returns its tooltip.
pub(super) fn art_slot(ui: &egui::Ui, cx: &mut UiCx<'_>, a: &Activity, rect: Rect, bg: Color32) -> String {
    let url = crate::activity::art_url(ui.ctx(), cx.state, a, ART_PX);
    let tex = url.and_then(|u| cx.images.get(ui.ctx(), &u, ART_PX, false));
    // The chip tier: half the theme's radius.
    let radius = (cx.theme.layout.corner_radius / 2.0).round().clamp(0.0, 8.0) as u8;
    match tex {
        Some(tex) => egui::Image::from_texture(tex).corner_radius(radius).paint_at(ui, rect),
        None => {
            let tk = cx.theme.tokens();
            let tile = tk.hover;
            ui.painter().rect_filled(rect, radius, tile);
            let color = if a.kind == 1 { tk.live } else { cx.theme.colors.text_muted.to_color32() };
            let icon = Rect::from_center_size(rect.center(), Vec2::splat((rect.width() * 0.8).round()));
            super::icons::paint(ui.painter(), super::icons::activity(a.kind), icon, color, bg.blend(tile));
        }
    }
    art_tooltip(a, super::threads::unix_ms())
}

/// "Playing Factorio for 47 min", "Listening to Ferris Song by Crabs".
pub(super) fn art_tooltip(a: &Activity, now_ms: u64) -> String {
    let r = a.rich.as_deref();
    match (r.and_then(|r| r.details.as_deref()), r.and_then(|r| r.state.as_deref())) {
        (Some(song), Some(by)) if a.is_spotify() => format!("Listening to {song} by {by}"),
        (Some(song), None) if a.is_spotify() => format!("Listening to {song}"),
        _ => [Some(a.label()), a.elapsed(now_ms)].into_iter().flatten().collect::<Vec<_>>().join(" "),
    }
}

/// Test hooks: a sample activity of type `kind` (0-5) with rich presence: the game (0) with
/// app-asset art and a small image (`DEMO_APP`, `DEMO_ASSET`, `DEMO_SMALL`), Spotify (2) with
/// album art (`DEMO_ALBUM`), a Twitch stream (1), YouTube (3) and a tournament (5). Offline the
/// art comes from the image cache only (seed it for screenshots); else the type icon shows.
pub(super) fn demo_activity(kind: u8) -> Activity {
    let (name, details, state, mins) = match kind {
        1 => ("Twitch", "Speedrunning the borrow checker", "Factorio", 95),
        2 => ("Spotify", "Borrow Checker Blues", "The Lifetimes", 1),
        3 => ("YouTube", "Rust in 100 seconds", "", 12),
        5 => ("Rust Golf Cup", "Round 3", "Top 10", 1500),
        _ => ("Factorio", "Building a megabase", "Nauvis", 47),
    };
    let text = |s: &str| (!s.is_empty()).then(|| s.to_owned());
    let start = super::threads::unix_ms() - mins * 60_000 - 23_000;
    let rich = RichPresence {
        details: text(details),
        state: text(state),
        start: Some(start),
        application_id: (kind == 0).then_some(DEMO_APP),
        large_image: match kind {
            0 => Some(DEMO_ASSET.into()),
            2 => Some(format!("spotify:{DEMO_ALBUM}")),
            _ => None,
        },
        large_text: match kind {
            0 => Some("Factorio 2.0".into()),
            2 => Some("Zero-Cost Abstractions".into()),
            _ => None,
        },
        small_image: (kind == 0).then(|| DEMO_SMALL.into()),
        end: (kind == 2).then_some(start + 3 * 60_000 + 34_000),
        sync_id: (kind == 2).then(|| "0rUSTcoRDdem0TRack000x".into()),
        url: (kind == 1).then(|| "https://www.twitch.tv/rustcord".into()),
        party: (kind == 0).then_some([3, 4]),
        // The game is joinable and has a custom button (its URL is fetched on a click).
        buttons: if kind == 0 { vec!["Factorio Wiki".into()] } else { Vec::new() },
        session_id: Some("0demo0session".into()),
        flags: if kind == 0 { RichPresence::JOIN | RichPresence::PARTY_PRIVACY_FRIENDS } else { 0 },
        ..Default::default()
    };
    Activity { kind, name: name.into(), rich: Some(Box::new(rich)) }
}

/// Test hooks: a detected game as others see it (no rich-presence art): its app icon shows
/// (`DEMO_DETECTED`'s icon from the saved games list), else the controller tile.
pub(super) fn demo_detected() -> Activity {
    let rich = RichPresence { start: Some(super::threads::unix_ms() - 12 * 60_000), application_id: Some(DEMO_DETECTED), ..Default::default() };
    Activity { kind: 0, name: "Celeste".into(), rich: Some(Box::new(rich)) }
}

/// The demo game's application and art assets (seed the image cache with
/// `app-assets/{DEMO_APP}/{DEMO_ASSET}.png?size=64|128` and `{DEMO_SMALL}.png?size=64` for
/// screenshots), Spotify's album (`i.scdn.co/image/{DEMO_ALBUM}`) and the detected game.
const DEMO_APP: Id = Id(1565443437363200900);
const DEMO_ASSET: &str = "1565443437363200901";
const DEMO_SMALL: &str = "1565443437363200902";
const DEMO_ALBUM: &str = "ab67616d0000b273c0ffee00000000000000beef";
const DEMO_DETECTED: Id = Id(1565443437363200903);

/// Test hook (`RUSTCORD_OPEN=members`, ui/mod.rs): the demo list's presences as the gateway
/// reports them next to a member list, the 2nd-4th online member with a custom status over a
/// game with art, a detected game (app icon) and Spotify on a phone.
pub(super) fn demo_presences(state: &mut AppState, items: &[MemberListItem]) {
    let custom = CustomStatus { text: Some("Reviewing PRs".into()), emoji: Some(Emoji { name: Some("👀".into()), ..Default::default() }) };
    let extras = [
        UserPresence::default(),
        UserPresence { custom_status: Some(custom), activity: Some(demo_activity(0)), ..Default::default() },
        UserPresence { activity: Some(demo_detected()), ..Default::default() },
        UserPresence { mobile: true, activity: Some(demo_activity(2)), ..Default::default() },
    ];
    let online = items.iter().filter_map(|it| match it {
        MemberListItem::Member { member, status } if *status != Presence::Offline => Some((member.user.as_ref()?.id, *status)),
        _ => None,
    });
    let list = online.zip(extras.into_iter().chain(std::iter::repeat_with(UserPresence::default))).map(|((id, status), p)| (id, UserPresence { status, ..p })).collect();
    state.apply_gateway(GatewayEvent::PresenceUpdate(list), None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_labels() {
        let roles = vec![Role { id: Id(42), name: "Maintainers".into(), ..Default::default() }];
        assert_eq!(group_label("online", &roles), "Online");
        assert_eq!(group_label("offline", &roles), "Offline");
        assert_eq!(group_label("42", &roles), "Maintainers");
        assert_eq!(group_label("43", &roles), "Members");
        assert_eq!(group_label("weird", &roles), "Members");
    }
}
