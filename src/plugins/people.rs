//! Name decorations and profile sections: PlatformIndicators, MoreUserTags, ShowMeYourName,
//! UserVoiceShow, ShowTimeoutDuration, IrcColors (next to names in messages and the member
//! list) and FriendsSince, MutualGroupDMs, ShowConnections, PermissionsViewer, MemberCount.
//! Everything here reads cached state only; nothing is fetched.

use super::{Plugin, enabled, opt_bool};
use crate::model::{Guild, Id, Member, MemberListItem, Presence, User, channel_type, perm, relationship_type};
use crate::state::AppState;
use crate::ui::UiCx;
use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Sense, Shape, Stroke, Vec2, pos2, vec2};

/// `UserPresence::platforms` bits.
pub mod platform {
    pub const DESKTOP: u8 = 1;
    pub const MOBILE: u8 = 2;
    pub const WEB: u8 = 4;
    pub const EMBEDDED: u8 = 8;
    pub const VR: u8 = 16;
    pub const ALL: [(u8, &str); 5] = [(DESKTOP, "Desktop"), (MOBILE, "Mobile"), (WEB, "Web"), (EMBEDDED, "Console"), (VR, "VR")];

    /// The keys of a presence's `client_status` object (UD: desktop, mobile, web, embedded, vr).
    pub fn from_client_status(v: &serde_json::Value) -> u8 {
        let Some(m) = v.as_object() else { return 0 };
        m.keys().fold(0, |b, k| {
            b | match k.as_str() {
                "desktop" => DESKTOP,
                "mobile" => MOBILE,
                "web" => WEB,
                "embedded" => EMBEDDED,
                "vr" => VR,
                _ => 0,
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Permissions of anyone (MoreUserTags, PermissionsViewer)
// ---------------------------------------------------------------------------

/// Guild-level permissions of a member (owner / ADMINISTRATOR = all bits).
pub fn member_perms(g: &Guild, user_id: Id, roles: &[Id]) -> u64 {
    if g.owner_id == user_id {
        return perm::ALL;
    }
    let p = g.roles.iter().filter(|r| r.id == g.id || roles.contains(&r.id)).fold(0, |p, r| p | r.permissions.0);
    if p & perm::ADMINISTRATOR != 0 { perm::ALL } else { p }
}

/// MoreUserTags: the one tag a member gets (highest wins).
pub fn user_tag(state: &AppState, guild_id: Option<Id>, user_id: Id, member: Option<&Member>) -> Option<(&'static str, Color32)> {
    let gold = Color32::from_rgb(0xF0, 0xB2, 0x32);
    let Some(g) = guild_id.and_then(|g| state.guild(g)) else {
        // Group DM owner.
        let owner = state.selected_channel.and_then(|c| state.channels.get(&c)).filter(|c| c.kind == channel_type::GROUP_DM)?.owner_id;
        return (owner == Some(user_id)).then_some(("Owner", gold));
    };
    if g.owner_id == user_id {
        return Some(("Owner", gold));
    }
    let roles = member.map(|m| m.roles.as_slice()).or_else(|| state.find_member(g.id, user_id).map(|m| m.roles.as_slice()))?;
    let p = member_perms(g, user_id, roles);
    if p == perm::ALL {
        Some(("Admin", Color32::from_rgb(0xED, 0x42, 0x45)))
    } else if p & (perm::MANAGE_MESSAGES | perm::KICK_MEMBERS | perm::BAN_MEMBERS | perm::MODERATE_MEMBERS) != 0 {
        Some(("Mod", Color32::from_rgb(0x3B, 0xA5, 0x5D)))
    } else if p & (perm::MANAGE_GUILD | perm::MANAGE_CHANNELS | perm::MANAGE_ROLES) != 0 {
        Some(("Staff", Color32::from_rgb(0x58, 0x9C, 0xF0)))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Painting
// ---------------------------------------------------------------------------

/// A platform glyph in the square `r` (vector, so no font has to carry it).
pub fn paint_platform(p: &Painter, bit: u8, r: Rect, color: Color32) {
    let s = r.width();
    let stroke = Stroke::new((s * 0.11).max(1.2), color);
    let c = r.center();
    match bit {
        platform::MOBILE => {
            let body = Rect::from_center_size(c, vec2(s * 0.5, s * 0.86));
            p.rect_stroke(body, s * 0.1, stroke, egui::StrokeKind::Inside);
            p.circle_filled(pos2(c.x, body.bottom() - s * 0.16), s * 0.06, color);
        }
        platform::WEB => {
            p.circle_stroke(c, s * 0.42, stroke);
            p.line_segment([c - vec2(s * 0.42, 0.0), c + vec2(s * 0.42, 0.0)], stroke);
            let pts = (0..=16).map(|i| c + vec2(s * 0.18 * (std::f32::consts::PI * i as f32 / 16.0).sin(), s * 0.42 * (std::f32::consts::PI * i as f32 / 16.0).cos())).collect();
            p.add(Shape::line(pts, stroke));
        }
        platform::EMBEDDED | platform::VR => {
            let body = Rect::from_center_size(c, vec2(s * 0.9, s * 0.52));
            p.rect_stroke(body, s * 0.22, stroke, egui::StrokeKind::Inside);
            let (l, rr) = (c - vec2(s * 0.2, 0.0), c + vec2(s * 0.2, 0.0));
            if bit == platform::VR {
                p.circle_filled(l, s * 0.1, color);
                p.circle_filled(rr, s * 0.1, color);
            } else {
                p.line_segment([l - vec2(s * 0.08, 0.0), l + vec2(s * 0.08, 0.0)], stroke);
                p.line_segment([l - vec2(0.0, s * 0.08), l + vec2(0.0, s * 0.08)], stroke);
                p.circle_filled(rr, s * 0.06, color);
            }
        }
        _ => {
            let screen = Rect::from_min_max(r.min + vec2(s * 0.06, s * 0.1), pos2(r.right() - s * 0.06, r.top() + s * 0.68));
            p.rect_stroke(screen, s * 0.08, stroke, egui::StrokeKind::Inside);
            p.line_segment([pos2(c.x, screen.bottom()), pos2(c.x, r.bottom() - s * 0.1)], stroke);
            p.line_segment([pos2(c.x - s * 0.22, r.bottom() - s * 0.1), pos2(c.x + s * 0.22, r.bottom() - s * 0.1)], stroke);
        }
    }
}

/// A small speaker (UserVoiceShow).
pub fn paint_speaker(p: &Painter, r: Rect, color: Color32) {
    let (c, s) = (r.center(), r.width());
    let body = [c + vec2(-s * 0.42, -s * 0.14), c + vec2(-s * 0.2, -s * 0.14), c + vec2(s * 0.06, -s * 0.38), c + vec2(s * 0.06, s * 0.38), c + vec2(-s * 0.2, s * 0.14), c + vec2(-s * 0.42, s * 0.14)];
    p.add(Shape::convex_polygon(body.to_vec(), color, Stroke::NONE));
    let arc = |rad: f32| (0..=8).map(move |i| c + vec2(s * 0.06, 0.0) + rad * Vec2::angled(-0.8 + 1.6 * i as f32 / 8.0)).collect::<Vec<_>>();
    p.add(Shape::line(arc(s * 0.22), Stroke::new(s * 0.09, color)));
    p.add(Shape::line(arc(s * 0.38), Stroke::new(s * 0.09, color)));
}

/// A text chip (tags, timeout) with its left edge at `left_center`, on an opaque `surface` when
/// given (rows with a nameplate behind them); returns its rect.
fn paint_chip(ui: &egui::Ui, left_center: Pos2, text: &str, color: Color32, size: f32, surface: Option<Color32>) -> Rect {
    let tint = color.gamma_multiply(0.18);
    let v = ui.visuals();
    // Light themes: the text darkened to 4.5:1 on its chip (the gold OWNER is ~1.9:1 on cream).
    let bgs = surface.map_or([v.panel_fill, v.window_fill], |s| [s, s]).map(|b| b.blend(tint));
    let color = if v.dark_mode { color } else { crate::ui::cosmetics::reach(color, &bgs, Color32::BLACK, 4.5) };
    // A known glyph before the first space ("⏱ 5m") becomes a painted icon.
    let (icon, text) = text.split_once(' ').and_then(|(g, rest)| Some((crate::ui::icons::from_glyph(g)?, rest))).map_or((None, text), |(i, t)| (Some(i), t));
    let g = ui.painter().layout_no_wrap(text.to_owned(), FontId::new(size, crate::theme::bold_family()), color);
    let icon_w = if icon.is_some() { size + 2.0 } else { 0.0 };
    let r = Rect::from_min_size(pos2(left_center.x, left_center.y - g.size().y / 2.0 - 1.0), g.size() + vec2(8.0 + icon_w, 2.0));
    ui.painter().rect_filled(r, 4, surface.map_or(tint, |s| s.blend(tint)));
    if let Some(i) = icon {
        crate::ui::icons::paint(ui.painter(), i, Rect::from_center_size(pos2(r.left() + 4.0 + size / 2.0, r.center().y), Vec2::splat(size)), color, Color32::TRANSPARENT);
    }
    ui.painter().galley(r.min + vec2(4.0 + icon_w, 1.0), g, color);
    r
}

/// Where a user is in voice: (channel, guild), shared guilds only (cached voice states).
pub fn voice_channel(state: &AppState, user_id: Id) -> Option<Id> {
    state.voice_states.iter().find(|(_, users)| users.contains_key(&user_id)).map(|(ch, _)| *ch)
}

fn timeout_left(member: Option<&Member>) -> Option<(String, String)> {
    let until = member?.communication_disabled_until.as_deref()?;
    let end = chrono::DateTime::parse_from_rfc3339(until).ok()?;
    let left = end.signed_duration_since(chrono::Utc::now()).num_seconds();
    (left > 0).then(|| (short_duration(left as u64), format!("Timed out until {}", end.with_timezone(&chrono::Local).format("%b %-d, %H:%M"))))
}

/// "3d", "5h", "12m", "40s".
pub fn short_duration(s: u64) -> String {
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

/// The decorations after a name (message header or member row), laid out from `left_center`.
/// `list`: member list (its own options; no hover there: the row takes it); `surface`: under its
/// chips (`paint_chip`). Returns the width.
#[allow(clippy::too_many_arguments)]
fn decorations(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Option<Id>, user: &User, member: Option<&Member>, name: &str, left_center: Pos2, list: bool, surface: Option<Color32>) -> f32 {
    let c = cx.theme.colors.clone();
    let (muted, size) = (c.text_muted.to_color32(), cx.theme.font.size);
    let mut x = left_center.x;
    let y = left_center.y;
    let hover = |ui: &egui::Ui, r: Rect, salt: &str, tip: &str| {
        if !list {
            ui.interact(r, ui.make_persistent_id((salt, user.id, r.min.x.to_bits())), Sense::hover()).on_hover_text(tip);
        }
    };
    // ShowMeYourName: the username after a nickname (and a display name, by option).
    if enabled(Plugin::ShowMeYourName) && !list && !user.bot {
        let nick = name != user.global_name.as_deref().unwrap_or(&user.username);
        let show = (nick || opt_bool(Plugin::ShowMeYourName, "displayNames")) && !name.eq_ignore_ascii_case(&user.username);
        if show {
            let g = ui.painter().layout_no_wrap(user.username.clone(), FontId::proportional(size - 1.5), muted);
            ui.painter().galley(pos2(x, y - g.size().y / 2.0), g.clone(), muted);
            x += g.size().x + 6.0;
        }
    }
    // ShowBadgesInChat: grey glyphs for the badges `public_flags` carries (no profile fetch).
    if enabled(Plugin::ShowBadgesInChat) && !list {
        for (icon, title) in chat_badges(user.public_flags) {
            let r = Rect::from_center_size(pos2(x + 8.0, y), Vec2::splat(16.0));
            crate::ui::icons::paint(ui.painter(), icon, r, muted, Color32::TRANSPARENT);
            hover(ui, r, "rc_badge", &title);
            x = r.right() + 4.0;
        }
    }
    if enabled(Plugin::MoreUserTags) && opt_bool(Plugin::MoreUserTags, if list { "list" } else { "messages" })
        && let Some((tag, color)) = user_tag(cx.state, guild_id, user.id, member)
    {
        let r = paint_chip(ui, pos2(x, y), tag, color, size - 4.0, surface);
        x = r.right() + 6.0;
    }
    if enabled(Plugin::PlatformIndicators) && opt_bool(Plugin::PlatformIndicators, if list { "list" } else { "messages" }) && !user.bot {
        let presence = cx.state.presence_of(user.id);
        let bits = presence.map_or(0, |p| if p.platforms == 0 && p.mobile { platform::MOBILE } else { p.platforms });
        let color = crate::ui::presence_color(presence.map_or(Presence::Offline, |p| p.status), &c);
        for (bit, label) in platform::ALL {
            if bits & bit != 0 {
                let r = Rect::from_center_size(pos2(x + 7.0, y), Vec2::splat(14.0));
                paint_platform(ui.painter(), bit, r, color);
                hover(ui, r, "rc_platform", label);
                x = r.right() + 4.0;
            }
        }
    }
    if enabled(Plugin::UserVoiceShow)
        && opt_bool(Plugin::UserVoiceShow, if list { "showInMemberList" } else { "showInMessages" })
        && let Some(ch) = voice_channel(cx.state, user.id)
    {
        let r = Rect::from_center_size(pos2(x + 8.0, y), Vec2::splat(16.0));
        let tip = format!("In Voice: {}", cx.state.channel_name(ch));
        paint_speaker(ui.painter(), r, muted);
        if !list {
            let resp = ui.interact(r, ui.make_persistent_id(("rc_voice_show", user.id)), Sense::click()).on_hover_text(tip);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                cx.state.select_channel(ch);
            }
        }
        x = r.right() + 4.0;
    }
    if enabled(Plugin::ShowTimeoutDuration)
        && let Some((left, tip)) = timeout_left(member)
    {
        let r = paint_chip(ui, pos2(x, y), &format!("⏱ {left}"), c.danger.to_color32(), size - 4.0, surface);
        hover(ui, r, "rc_timeout", &tip);
        x = r.right() + 4.0;
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(30));
    }
    x - left_center.x
}

/// ShowBadgesInChat: one monochrome glyph and tooltip per public-flag badge, in profile order.
fn chat_badges(flags: u64) -> Vec<(crate::ui::icons::Icon, String)> {
    use crate::ui::icons::Icon;
    crate::ui::cosmetics::flag_badges(flags)
        .into_iter()
        .filter_map(|b| {
            let icon = match b.id.as_str() {
                "staff" => Icon::IdCard,
                "partner" => Icon::Star,
                "hypesquad" | "hypesquad_house_1" | "hypesquad_house_2" | "hypesquad_house_3" => Icon::Flag,
                "bug_hunter_level_1" | "bug_hunter_level_2" => Icon::Bug,
                "early_supporter" => Icon::Heart,
                "verified_developer" | "active_developer" => Icon::Code,
                "certified_moderator" => Icon::Shield,
                _ => return None,
            };
            let title = crate::ui::cosmetics::badge_info(&b.id).map_or(b.description, |(t, _)| t);
            Some((icon, title))
        })
        .collect()
}

/// Message header hook (`messages.rs` author_line, inside its row of height `h`).
pub fn after_name(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Option<Id>, user: &User, member: Option<&Member>, h: f32) {
    if !any_name_plugin() {
        return;
    }
    let name = cx.state.member_display_name(guild_id, user, member);
    let start = ui.cursor().min;
    // Painted first, then the space they took is allocated.
    let w = decorations(ui, cx, guild_id, user, member, &name, pos2(start.x, start.y + h / 2.0), false, None);
    if w > 0.0 {
        ui.allocate_exact_size(vec2(w - 4.0, h), Sense::hover());
    }
}

/// Member-list hook: decorations right of the name (`members.rs` member_row), chips on
/// `surface` when given; returns the width.
pub fn member_row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Option<Id>, user: &User, member: Option<&Member>, left_center: Pos2, surface: Option<Color32>) -> f32 {
    if !any_name_plugin() {
        return 0.0;
    }
    decorations(ui, cx, guild_id, user, member, "", left_center, true, surface)
}

/// The width `member_row` will take, nothing painted (the name is cut to leave it room).
pub fn member_row_width(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Option<Id>, user: &User, member: Option<&Member>) -> f32 {
    if !any_name_plugin() {
        return 0.0;
    }
    let mut ghost = ui.new_child(egui::UiBuilder::new().invisible());
    decorations(&mut ghost, cx, guild_id, user, member, "", Pos2::ZERO, true, None)
}

fn any_name_plugin() -> bool {
    [Plugin::ShowMeYourName, Plugin::ShowBadgesInChat, Plugin::MoreUserTags, Plugin::PlatformIndicators, Plugin::UserVoiceShow, Plugin::ShowTimeoutDuration].into_iter().any(enabled)
}

/// ValidUser: the name of a mentioned user the cache lacks, from their profile once a click on
/// the mention loaded it (the card's own fetch; nothing is requested here).
pub fn valid_user_name(state: &AppState, user_id: Id) -> Option<String> {
    if !enabled(Plugin::ValidUser) {
        return None;
    }
    let p = state.social.profiles.iter().find(|e| e.user_id == user_id)?.result.as_ref()?.as_ref().ok()?;
    Some(p.user.display_name().to_owned())
}

/// IrcColors: a stable bright colour per user for names without a role colour.
pub fn irc_color(guild_id: Option<Id>, user_id: Id, dark: bool) -> Option<Color32> {
    if !enabled(Plugin::IrcColors) || (guild_id.is_some() && opt_bool(Plugin::IrcColors, "onlyDms")) {
        return None;
    }
    // FNV-1a: colours need not match Vencord's, only stay put (PLUGINS_NOTES §0 #10).
    let hash = user_id.0.to_le_bytes().iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x100_0000_01b3));
    Some(hsl((hash % 360) as f32, 1.0, if dark { 0.7 } else { 0.38 }))
}

fn hsl(h: f32, s: f32, l: f32) -> Color32 {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let to = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(to(r), to(g), to(b))
}

// ---------------------------------------------------------------------------
// Profile sections (popout card and full profile)
// ---------------------------------------------------------------------------

fn heading(ui: &mut egui::Ui, theme: &crate::theme::Theme, text: &str, separator: bool) {
    if separator {
        ui.separator();
    }
    ui.label(egui::RichText::new(text).font(crate::ui::bold(theme.font.size - 2.0)).color(theme.colors.text_muted.to_color32()));
}

/// Plugin sections of a profile, after "Member Since" (`popout`: the card, which separates its
/// sections with rules; the full profile does not and shows connections itself).
/// `connections` draws a connections grid (the caller's `cosmetics::connections_grid`).
pub fn profile_sections(ui: &mut egui::Ui, cx: &mut UiCx<'_>, user: &User, guild_id: Option<Id>, popout: bool, connections: &dyn Fn(&mut egui::Ui, &[crate::model::ConnectedAccount])) {
    let is_me = cx.state.me.as_ref().is_some_and(|m| m.id == user.id);
    let theme = cx.theme.clone();
    let (text, link) = (theme.colors.text.to_color32(), theme.colors.link.to_color32());
    if enabled(Plugin::FriendsSince)
        && let Some(since) = cx.state.social.relationships.get(&user.id).filter(|r| r.kind == relationship_type::FRIEND).and_then(|r| r.since.clone())
        && let Ok(t) = chrono::DateTime::parse_from_rfc3339(&since)
    {
        heading(ui, &theme, "Friends Since", popout);
        ui.label(egui::RichText::new(t.with_timezone(&chrono::Local).format("%b %-d, %Y").to_string()).color(text));
    }
    if enabled(Plugin::UserVoiceShow)
        && opt_bool(Plugin::UserVoiceShow, "showInUserProfile")
        && let Some(ch) = voice_channel(cx.state, user.id)
    {
        heading(ui, &theme, "In Voice", popout);
        let guild = cx.state.channels.get(&ch).and_then(|c| c.guild_id).and_then(|g| cx.state.guild(g)).map(|g| g.name.clone());
        let place = match guild {
            Some(g) => format!("{} / {g}", cx.state.channel_name(ch)),
            None => cx.state.channel_name(ch),
        };
        let resp = ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                crate::ui::icons::paint(ui.painter(), crate::ui::icons::Icon::Voice, r, link, Color32::TRANSPARENT);
                ui.add(egui::Label::new(egui::RichText::new(place).color(link)).sense(Sense::click()).truncate())
            })
            .inner;
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            cx.state.select_channel(ch);
            cx.view.profile_card = None;
            cx.view.modal = None;
        }
    }
    if enabled(Plugin::MutualGroupDms) && !is_me {
        let groups: Vec<(Id, String)> = cx
            .state
            .dm_order
            .iter()
            .filter_map(|id| cx.state.channels.get(id))
            .filter(|c| c.kind == channel_type::GROUP_DM && c.recipient_ids.contains(&user.id))
            .map(|c| (c.id, cx.state.channel_name(c.id)))
            .collect();
        if !groups.is_empty() {
            heading(ui, &theme, &format!("{} Mutual Group{}", groups.len(), if groups.len() == 1 { "" } else { "s" }), popout);
            for (id, name) in groups.into_iter().take(8) {
                let resp = ui.add(egui::Label::new(egui::RichText::new(name).color(link)).sense(Sense::click()).truncate());
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    cx.state.select_channel(id);
                    cx.view.profile_card = None;
                    cx.view.modal = None;
                }
            }
        }
    }
    if popout && enabled(Plugin::ShowConnections) {
        let list = cx.state.profile(user.id, guild_id).and_then(|e| e.result.as_ref()?.as_ref().ok()).map(|p| p.connected_accounts.clone()).unwrap_or_default();
        if list.iter().any(|a| crate::ui::cosmetics::shown_connection(&a.kind)) {
            heading(ui, &theme, "Connections", true);
            connections(ui, &list);
        }
    }
    if enabled(Plugin::PlatformIndicators) && opt_bool(Plugin::PlatformIndicators, "badges") && !user.bot
        && let Some(p) = cx.state.presence_of(user.id).filter(|p| p.platforms != 0)
    {
        let color = crate::ui::presence_color(p.status, &theme.colors);
        let names: Vec<&str> = platform::ALL.iter().filter(|(b, _)| p.platforms & b != 0).map(|(_, n)| *n).collect();
        heading(ui, &theme, "Online On", popout);
        ui.horizontal(|ui| {
            for (bit, _) in platform::ALL.iter().filter(|(b, _)| p.platforms & b != 0) {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                paint_platform(ui.painter(), *bit, r, color);
            }
            // Truncated: a long list must not widen the fixed-width card.
            ui.add(egui::Label::new(egui::RichText::new(names.join(", ")).color(text)).truncate());
        });
    }
    if enabled(Plugin::PermissionsViewer)
        && let Some(g) = guild_id
    {
        let resp = ui.add(egui::Label::new(egui::RichText::new("View Permissions").size(theme.font.size - 2.0).color(link)).sense(Sense::click()));
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            super::windows::open(ui.ctx(), super::windows::Window::Permissions { guild_id: g, user_id: user.id });
        }
    }
}

/// BetterRoleDot: a role chip's colour dot (centre `c`) copies the colour on a click.
pub fn role_dot(ui: &egui::Ui, c: Pos2, color: u32) {
    if !enabled(Plugin::BetterRoleDot) || color == 0 {
        return;
    }
    let hex = format!("#{color:06x}");
    let r = Rect::from_center_size(c, Vec2::splat(12.0));
    let resp = ui.interact(r, ui.make_persistent_id(("rc_role_dot", color, c.x.to_bits(), c.y.to_bits())), Sense::click()).on_hover_text(format!("Copy {hex}"));
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        ui.ctx().copy_text(hex);
    }
}

/// MemberCount: "● 12 Online  ● 340 Members" above the member list (`members.rs`).
pub fn member_count(ui: &mut egui::Ui, cx: &UiCx<'_>, guild_id: Id) {
    if !enabled(Plugin::MemberCount) {
        return;
    }
    let Some(l) = cx.state.member_lists.get(&guild_id).filter(|l| l.member_count > 0) else { return };
    let c = &cx.theme.colors;
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    let font = FontId::proportional(cx.theme.font.size - 2.0);
    let muted = c.text_muted.to_color32();
    let mut x = row.left() + 16.0;
    // 0 = not sent: the listed members who are not offline.
    let online = match l.online_count {
        0 => l.items.iter().filter(|i| matches!(i, MemberListItem::Member { status, .. } if *status != Presence::Offline)).count() as u32,
        n => n,
    };
    for (n, word, dot) in [(online, "Online", crate::ui::presence_color(Presence::Online, c)), (l.member_count, "Members", muted)] {
        ui.painter().circle_filled(pos2(x + 4.0, row.center().y), 4.0, dot);
        let r = ui.painter().text(pos2(x + 12.0, row.center().y), Align2::LEFT_CENTER, format!("{n} {word}"), font.clone(), muted);
        x = r.right() + 14.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Perms, Role};

    #[test]
    fn chat_badges_follow_the_flags() {
        use crate::ui::icons::Icon;
        let got = chat_badges(1 | 1 << 6 | 1 << 22);
        let want = [(Icon::IdCard, "Discord staff"), (Icon::Flag, "HypeSquad Bravery"), (Icon::Code, "Active developer")];
        assert_eq!(got.iter().map(|(i, t)| (*i, t.as_str())).collect::<Vec<_>>(), want);
        assert!(chat_badges(1 << 20).is_empty());
        assert!(chat_badges(0).is_empty());
    }

    #[test]
    fn platforms_from_client_status() {
        let v: serde_json::Value = serde_json::from_str(r#"{"desktop":"online","mobile":"idle","vr":"dnd","fridge":"online"}"#).unwrap();
        assert_eq!(platform::from_client_status(&v), platform::DESKTOP | platform::MOBILE | platform::VR);
        assert_eq!(platform::from_client_status(&serde_json::Value::Null), 0);
    }

    #[test]
    fn member_permissions_and_tags() {
        let role = |id, p| Role { id: Id(id), permissions: Perms(p), ..Default::default() };
        let g = Guild { id: Id(1), owner_id: Id(9), roles: vec![role(1, perm::VIEW_CHANNEL), role(2, perm::KICK_MEMBERS), role(3, perm::ADMINISTRATOR)], ..Default::default() };
        assert_eq!(member_perms(&g, Id(9), &[]), perm::ALL);
        assert_eq!(member_perms(&g, Id(5), &[]), perm::VIEW_CHANNEL);
        assert_eq!(member_perms(&g, Id(5), &[Id(2)]), perm::VIEW_CHANNEL | perm::KICK_MEMBERS);
        assert_eq!(member_perms(&g, Id(5), &[Id(3)]), perm::ALL);
        assert_eq!(short_duration(59), "59s");
        assert_eq!(short_duration(3 * 3600 + 5), "3h");
        // IrcColors is off by default.
        assert_eq!(irc_color(None, Id(5), true), None);
        assert_ne!(hsl(0.0, 1.0, 0.7), hsl(180.0, 1.0, 0.7));
    }
}
