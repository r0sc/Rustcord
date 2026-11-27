//! Profile card popup (avatar with status, banner, custom status bubble, names, activity, roles
//! in guild). Opened via `ViewState::profile_card`. My card shows my local banner/avatar
//! overrides (`crate::overrides`); opened above the user panel it is the account popout, whose
//! lower part (status picker, custom status, Edit Profile) is `user_settings::account_popout`.
//! Opening a card loads the profile (`profile_modal::fetch`): pronouns, About Me, mutual counts
//! (open the full profile on that tab), Member Since and my Note. Others' cards end with the
//! Message / friend state (Add, Accept, sent) / More (View full profile) actions. Also asks the 80013
//! question each frame (`friends::ask_stranger`).
//! OWNERSHIP: friends/DMs/inbox/profile engineer (docs/SOCIAL_UI_DESIGN.md).

use super::icons::Icon;
use super::messages::{menu_item, menu_setup, sheen, tag, vector_button, Btn, MenuIcon, Pal};
use super::profile_modal::{self, Tab};
use super::sidebar::{paint_status_text, status_label, status_tooltip, user_dot, user_presence, user_presence_label, user_status_text};
use super::{cosmetics, images, rgb, Modal, UiCx};
use crate::api::cdn;
use crate::backend::ContextLocation;
use crate::model::relationship_type;
use crate::config::AnimateMode;
use crate::model::{Activity, Id, Member, MemberListItem, Presence, User};
use crate::overrides::{self, Kind};
use crate::theme::bold_family;
use egui::emath::GuiRounding;
use egui::epaint::{RectShape, Vertex};
use egui::load::SizedTexture;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, Frame, Image, Key, Margin, Order, Rect, RichText, Sense, Stroke, Vec2};

const WIDTH: f32 = 340.0;
const AVATAR: f32 = 80.0;

/// (user, guild, anchor x/y bits) of the shown card and the pass it was opened in.
type Seen = ((Id, Option<Id>, u32, u32), u64);

/// Temp: the banner URL the card shows (freed when it closes or shows another one).
fn banner_id() -> egui::Id {
    egui::Id::unique("rc_profile_card_banner")
}

pub fn show_popup(ctx: &egui::Context, cx: &mut UiCx<'_>) {
    super::friends::ask_stranger(cx);
    let seen_id = egui::Id::unique("rc_profile_card_seen");
    let Some(t) = cx.view.profile_card else {
        // Just closed: a note still being typed is saved like on blur; the banner texture goes
        // too, unless the full profile of that user now shows it.
        if let Some(((user, ..), _)) = ctx.data_mut(|d| d.remove_temp::<Option<Seen>>(seen_id)).flatten() {
            profile_modal::save_note(cx);
            if cx.view.modal.is_none() {
                cosmetics::release(ctx, cx.images);
            }
            let url = ctx.data_mut(|d| d.remove_temp::<String>(banner_id())).unwrap_or_default();
            let reused = matches!(cx.view.modal, Some(Modal::Profile { user_id, .. }) if user_id == user);
            if !url.is_empty() && !reused {
                cx.images.remove(&url);
            }
        }
        return;
    };
    // The click that opened the card must not count as a click outside it.
    let pass = ctx.cumulative_pass_nr();
    let key = (t.user_id, t.guild_id, t.anchor.x.to_bits(), t.anchor.y.to_bits());
    let opened = match ctx.data(|d| d.get_temp::<Option<Seen>>(seen_id)).flatten() {
        Some((k, p)) if k == key => p,
        _ => {
            ctx.data_mut(|d| d.insert_temp::<Option<Seen>>(seen_id, Some((key, pass))));
            pass
        }
    };
    let Some(user) = find_user(cx, t.user_id) else {
        cx.view.profile_card = None;
        return;
    };
    let is_me = cx.state.me.as_ref().is_some_and(|m| m.id == user.id);
    // My card growing up from the user panel is the account popout: global profile + statuses.
    let account = is_me && t.pivot == Align2::LEFT_BOTTOM;
    let guild_id = t.guild_id.filter(|_| !account);
    if pass == opened {
        // Opened by a click: the profile (bio, mutual servers, note; for the account popout
        // my badges: Nitro, boost, quests), cached for minutes.
        profile_modal::save_note(cx);
        if is_me {
            // My profile after a failed load (a network error): asked again now, not in minutes.
            cx.state.social.profiles.retain(|e| e.user_id != user.id || e.loading || !matches!(e.result, Some(Err(_))));
        }
        profile_modal::fetch(cx, t.user_id, guild_id);
    }
    let member = card_member(cx, guild_id, t.user_id);
    // Theme, effect and frame (ui/cosmetics.rs; the account popout: only my local picks).
    let look = cosmetics::profile_look(ctx, cx, user.id, guild_id, !account);
    // A themed card has its own colours and light / dark mode (§8), whatever the app's.
    let app = Pal::new(cx.theme);
    // A float (E2, docs/DESIGN.md §6): `raised`, which the avatar ring and art tiles follow.
    let pal = look.theme.map_or(Pal { surface: app.tk.raised, ..app }, |t| cosmetics::theme_pal(&app, t));
    let r = pal.float_radius();
    let shadow = ctx.global_style().visuals.popup_shadow;
    // The footer's More menu, a badge's popover and the server tag card are layers of their own:
    // a click or Escape while one is open closes only that (read before it closes itself).
    let menu_open = egui::Popup::is_any_open(ctx);
    let mut bounds = ctx.content_rect().shrink(16.0);
    // A frame's top staples stand above the card: their room too, not the title bar.
    bounds.min.y = ctx.content_rect().top() + super::TITLE_BAR_H + 8.0 + look.overflow_top(WIDTH);
    // Another person while the card is open: it glides there (BASE, in-out, from where it is); a card
    // opened afresh or from another corner starts at its spot (and grows, `motion::end_pass`).
    let glide = egui::Id::unique("rc_profile_glide");
    let layer = egui::LayerId::new(Order::Foreground, egui::Id::unique("rc_profile_card"));
    let same = ctx.memory(|m| m.areas().visible_last_frame(&layer)) && ctx.data(|d| d.get_temp::<Align2>(glide)) == Some(t.pivot);
    ctx.data_mut(|d| d.insert_temp(glide, t.pivot));
    if !same {
        super::motion::forget(ctx, glide.with("x"));
        super::motion::forget(ctx, glide.with("y"));
    }
    let at = pos2(super::motion::value(ctx, glide.with("x"), t.anchor.x, super::motion::BASE), super::motion::value(ctx, glide.with("y"), t.anchor.y, super::motion::BASE));
    // Opening afresh: the card springs out of its row and its parts follow (DESIGN §7). Time since
    // it became visible, so a card that glides to the next person does not start over.
    super::motion::own(ctx, layer);
    let secs = super::motion::open_for(ctx, layer, super::motion::SPRING_T);
    // The corner nearest the row is the speech corner (§5, §9.6); the last pass's rect says whether the window pushed the card off its anchor.
    // Measured against the anchor the last rect was laid out at, so a glide does not read as a push.
    let prev_at = ctx.data(|d| d.get_temp::<egui::Pos2>(glide.with("at")));
    ctx.data_mut(|d| d.insert_temp(glide.with("at"), at));
    let cr = speech(t.pivot, r, egui::AreaState::load(ctx, layer.id).map(|s| s.rect()), prev_at.unwrap_or(at));
    let aura = look.aura.then(|| super::aura::of(ctx, cx, &user, guild_id)).flatten();
    let resp = egui::Area::new(egui::Id::unique("rc_profile_card"))
        .order(Order::Foreground)
        .pivot(t.pivot)
        .fixed_pos(at)
        // Keep 16 px off the window edges, 8 px under the title bar (the user-panel anchor sits
        // near the bottom).
        .constrain_to(bounds)
        .show(ctx, |ui| {
            let behind = ui.ctx().layer_painter(ui.layer_id()).add(egui::Shape::Noop);
            let glow_slot = ui.ctx().layer_painter(ui.layer_id()).add(egui::Shape::Noop);
            // A themed card gets the theme's gradient outline instead (`theme_outline`); an aura the edge.
            let stroke = match look.theme.filter(|_| !look.aura) {
                Some(_) => Stroke::NONE,
                // An aura card's edge is the lit one, drawn over the content.
                None if aura.is_some() => Stroke::NONE,
                // High contrast floats have no shadow, a 1.5 px muted edge instead.
                None if pal.tk.mode == crate::theme::Mode::HighContrast => Stroke::new(1.5, pal.muted),
                None => Stroke::new(1.0, pal.tk.rule),
            };
            let framed = Frame::new()
                .fill(pal.surface)
                .corner_radius(cr)
                .stroke(stroke)
                .shadow(shadow)
                .show(ui, |ui| {
                    if look.theme.is_some() {
                        cosmetics::theme_visuals(ui, &pal);
                    }
                    let theme_slot = ui.painter().add(egui::Shape::Noop);
                    let mut banner_bottom = 0.0;
                    // Taller than the window (short windows, long About Me): it scrolls.
                    let max_h = bounds.height() - 2.0;
                    // Fixed width like Discord's: content never widens the card.
                    egui::ScrollArea::vertical().max_width(WIDTH).max_height(max_h).min_scrolled_height(max_h).show(ui, |ui| {
                        banner_bottom = card(ui, cx, &pal, &user, member.as_ref(), guild_id, (is_me, account), look.theme, cr, secs);
                        part(ui, secs, 3, |ui| {
                            if account {
                                // Status picker, custom status, Edit Profile (user_settings.rs).
                                super::user_settings::account_popout(ui, cx, &pal);
                            } else {
                                actions(ui, cx, &pal, user.id, guild_id, is_me);
                            }
                        });
                    });
                    if let Some(colors) = look.theme {
                        ui.painter().set(theme_slot, cosmetics::theme_shapes(ui.ctx(), "card", ui.min_rect(), cr, colors, banner_bottom, pal.dark));
                    }
                });
            let rect = framed.response.rect;
            if let Some(colors) = look.theme.filter(|_| !look.aura) {
                ui.painter().add(cosmetics::theme_outline(rect, cr, colors));
            } else {
                sheen(ui.painter(), rect, f32::from(r), &pal.tk);
            }
            // Their light under the card (a tinted shadow in Lilac) and the edge lit in their colour.
            if let Some(a) = aura {
                let k = match cx.theme.tokens().mode {
                    crate::theme::Mode::Dark => 0.48,
                    crate::theme::Mode::Light => 0.14,
                    crate::theme::Mode::HighContrast => 0.0,
                };
                if k > 0.0 {
                    ui.ctx().layer_painter(ui.layer_id()).set(glow_slot, egui::Shape::Vec(vec![super::light::soft_glow(rect.translate(vec2(0.0, 10.0)).shrink(6.0), f32::from(r) - 6.0, a.gamma_multiply(k), 56.0)]));
                    // (wrapped: messages::add_contact would take the bare [blur, blur] pair for a Frame's [shadow, body])
                }
                ui.painter().add(super::light::edge_light(rect, cr, a, pal.tk.rule));
            }
            let animate = cosmetics::animates(cx, ui.rect_contains_pointer(rect));
            look.finish(ui, cx.images, rect, r, behind, cosmetics::shown_for(ui.ctx(), ("card", user.id)), animate);
        })
        .response;
    if secs < super::motion::SPRING_T {
        let s = super::motion::springy(secs);
        // From the side the row is on: right of the list (RIGHT_TOP) it comes from the right, above the user panel up from below.
        let from = if t.pivot.y() == egui::Align::Max { vec2(0.0, 12.0) } else { vec2(t.pivot.x().to_sign() * 12.0, 0.0) };
        ctx.transform_layer_shapes_after_rounding(layer, super::motion::about(at, egui::lerp(0.94..=1.0, s)) * egui::emath::TSTransform::from_translation(from * (1.0 - s)));
    }
    if !menu_open && (ctx.input(|i| i.key_pressed(Key::Escape)) || (pass != opened && resp.clicked_elsewhere())) {
        cx.view.profile_card = None;
    }
}

/// The card's corners: `r` each, but the corner of `pivot` is the 2 px speech corner when the card
/// (`last` pass's rect) still sits on its anchor `at` (the window edge did not push it more than
/// 12 px). Centred pivots have none.
fn speech(pivot: Align2, r: u8, last: Option<Rect>, at: egui::Pos2) -> CornerRadius {
    let (top, bottom) = (pivot.y() == egui::Align::Min, pivot.y() == egui::Align::Max);
    let edge = if top { last.map(|l| l.top()) } else { last.map(|l| l.bottom()) };
    if !(top || bottom) || pivot.x() == egui::Align::Center || edge.is_some_and(|e| (e - at.y).abs() > 12.0) {
        return CornerRadius::same(r);
    }
    let mut cr = CornerRadius::same(r);
    let corner = match (top, pivot.x() == egui::Align::Max) {
        (true, true) => &mut cr.ne,
        (true, false) => &mut cr.nw,
        (false, true) => &mut cr.se,
        (false, false) => &mut cr.sw,
    };
    *corner = 2;
    cr
}

/// A part of the card arriving: its `k`-th stagger after the card opened, rising 4 px and fading in over
/// FAST (`secs`: time since the card opened; done by 240 ms at k = 3).
fn part<R>(ui: &mut egui::Ui, secs: f32, k: usize, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let e = super::motion::ease(((secs - k as f32 * super::motion::STAGGER) / super::motion::FAST).clamp(0.0, 1.0));
    super::motion::arriving(ui, (e, 4.0 * (1.0 - e)), add)
}

/// The card's banner, avatar and names; others' cards and my own (not the account popout) also
/// show the loaded profile: pronouns, About Me, Member Since, mutual counts and my Note. With
/// cosmetics (ui/cosmetics.rs): decoration, name style, guild tag, badges; `theme`: the
/// profile theme the caller paints under it. Returns the banner's bottom edge.
#[allow(clippy::too_many_arguments, reason = "one call site; the card's whole context")]
fn card(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, user: &User, member: Option<&Member>, guild_id: Option<Id>, (is_me, account): (bool, bool), theme: Option<(u32, u32)>, cr: CornerRadius, secs: f32) -> f32 {
    ui.set_width(WIDTH);
    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
    let top = CornerRadius { sw: 0, se: 0, ..cr };

    let (banner_url, fill) = banner(cx, user, guild_id, is_me);
    let banner_h = if banner_url.is_some() { 106.0 } else { 60.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(WIDTH, banner_h), Sense::hover());
    let animated = banner_animates(cx, banner_url.as_deref(), is_me, ui.rect_contains_pointer(rect));
    let shown = banner_url.as_deref().unwrap_or_default();
    let old = ui.ctx().data_mut(|d| {
        let s = d.get_temp_mut_or_default::<String>(banner_id());
        (s.as_str() != shown).then(|| std::mem::replace(s, shown.to_owned()))
    });
    if let Some(old) = old.filter(|o| !o.is_empty()) {
        cx.images.remove(&old);
    }
    match banner_url.and_then(|url| cx.images.get(ui.ctx(), &url, 600, animated)) {
        Some(tex) => paint_banner(ui.painter(), rect, top, tex, theme.map(|_| cosmetics::theme_veil(pal.dark))),
        None => {
            ui.painter().rect_filled(rect, top, band_fill(fill, pal, theme.is_some()));
        }
    }

    // Avatar overlapping the banner edge, ringed in the card color.
    let ring = theme.map_or(pal.surface, |(primary, _)| cosmetics::theme_ring(primary, pal.dark));
    let center = pos2(rect.left() + 16.0 + AVATAR / 2.0, rect.bottom());
    ui.painter().circle_filled(center, AVATAR / 2.0 + 5.0, ring);
    let avatar_rect = Rect::from_center_size(center, Vec2::splat(AVATAR));
    let avatar_url = overrides::my_avatar_url(user.id)
        .map_or_else(|| images::user_avatar_url(guild_id, user, member, 160, true), |u| overrides::large(&u));
    let animate = overrides::my_avatar_animates(user.id, ui.rect_contains_pointer(avatar_rect)).unwrap_or(true);
    let name = cx.state.member_display_name(guild_id, user, member);
    super::sidebar::user_avatar_at(ui, cx.images, &avatar_url, avatar_rect, animate, 1.0, (user.id, &name));
    // ViewIcons (below), when on, opens the picture instead.
    if !crate::plugins::enabled(crate::plugins::Plugin::ViewIcons) {
        avatar_link(ui, cx, pal, avatar_rect, user.id, guild_id);
    }
    // ViewIcons: the avatar and banner open in full size.
    crate::plugins::media::icon_link(ui, cx, rect, "View Banner", 2.5, || crate::plugins::media::banner_full(user));
    crate::plugins::media::icon_link(ui, cx, avatar_rect, "View Avatar", 1.0, || Some(crate::plugins::media::avatar_full(user)));
    let fx = overrides::effective_for(user, member);
    let fx_anim = cosmetics::animates(cx, ui.ui_contains_pointer());
    images::decoration_at(ui, cx.images, avatar_rect, fx.decoration.as_ref(), fx_anim, 1.0);
    let dot = user_dot(ui.painter(), avatar_rect, cx.state, user.id, &cx.theme.colors, ring);
    let presence = user_presence(cx.state, user.id);
    let label = match cx.state.chat.my_status.filter(|_| is_me) {
        Some(s) => status_label(s),
        None => user_presence_label(cx.state, user.id),
    };
    ui.interact(dot, ui.make_persistent_id("rc_card_status"), Sense::hover()).on_hover_text(label);
    // Custom status bubble right of the avatar, at most two lines (mine also while invisible,
    // like the user panel).
    let custom = if is_me { cx.state.chat.my_custom_status.is_some() } else { cx.state.presence_of(user.id).is_some_and(|p| p.custom_status.is_some()) };
    if let Some(st) = user_status_text(cx.state, user.id).filter(|_| custom && (is_me || presence != Presence::Offline)) {
        let left = avatar_rect.right() + 16.0;
        let pos = pos2(left + 12.0, rect.bottom() + 8.0);
        let bubble = ui.painter().add(egui::Shape::Noop);
        let format = egui::TextFormat::simple(FontId::proportional(pal.meta()), pal.text);
        let r = paint_status_text(ui, cx.images, pos, &st, format, 20.0, rect.right() - 16.0 - 12.0 - pos.x, 2).expand2(vec2(12.0, 6.0));
        ui.painter().set(bubble, egui::Shape::rect_filled(r, pal.card_radius(), pal.bg));
        if let Some(tip) = status_tooltip(ui, &st) {
            ui.interact(r, ui.make_persistent_id("rc_card_bubble"), Sense::hover()).on_hover_text(tip);
        }
    }
    ui.add_space(AVATAR / 2.0 + 6.0);

    Frame::new().inner_margin(Margin { left: 16, right: 16, top: 0, bottom: 16 }).show(ui, |ui| {
        ui.set_width(WIDTH - 32.0);
        let style = fx.name_style.as_ref();
        let name_g = if style.is_some() {
            cosmetics::name_galley(ui, &name, FontId::new(pal.title(), bold_family()), style, pal.text, WIDTH - 32.0)
        } else {
            // The title role's tight tracking (a name style keeps its own letters).
            let mut job = egui::text::LayoutJob { wrap: egui::text::TextWrapping::truncate_at_width(WIDTH - 32.0), break_on_newline: false, ..Default::default() };
            let format = egui::TextFormat { extra_letter_spacing: -0.3, ..egui::TextFormat::simple(FontId::new(pal.title(), bold_family()), pal.text) };
            crate::theme::append_boxed(&mut job, &name, &format);
            ui.painter().layout_job(job)
        };
        let handle = match user.discriminator.as_deref() {
            // Streamer Mode: my own username is personal information.
            _ if is_me && cx.settings.streamer.hides_personal() => String::new(),
            Some(d) if !d.is_empty() && d != "0" => format!("{}#{d}", user.username),
            _ => user.username.clone(),
        };
        let profile = profile_modal::loaded(cx, user.id, guild_id).filter(|_| !account);
        let pronouns = profile.and_then(|p| p.pronouns()).map(str::to_owned);
        let bio = profile.and_then(|p| p.bio()).map(str::to_owned);
        let mutual = profile.filter(|_| !is_me).map(|p| (p.mutual_guilds.len(), p.mutual_friends_count.map_or(p.mutual_friends.len(), |n| n as usize)));
        let note = profile.filter(|_| !is_me).map(|p| p.note.clone().unwrap_or_default());
        // The account popout too: the profile's badges, not only the `public_flags` ones.
        let badges = cosmetics::badges(user, profile_modal::loaded(cx, user.id, guild_id));
        part(ui, secs, 1, |ui| {
            let (r, _) = ui.allocate_exact_size(name_g.size(), Sense::hover());
            cosmetics::paint_name(ui.painter(), r.min, name_g, style, pal.text, pal.dark);
            ui.horizontal_wrapped(|ui| {
                // "alice", 8 px, "they/them": no bullet between them.
                ui.spacing_mut().item_spacing.x = 8.0;
                if !handle.is_empty() {
                    ui.label(RichText::new(handle).color(pal.muted));
                }
                if let Some(pg) = cosmetics::guild_tag(user, member) {
                    let h = ui.text_style_height(&egui::TextStyle::Body);
                    cosmetics::tag_button(ui, cx, pg, pal.size, h, pal.text);
                }
                if let Some(p) = pronouns {
                    ui.label(RichText::new(p).color(pal.muted));
                }
                if user.bot {
                    tag(ui, "Bot", pal.tk.chip, pal.link);
                }
            });
            cosmetics::badge_row(ui, cx.images, pal, &badges);
            if let Some((servers, friends)) = mutual.filter(|m| *m != (0, 0)) {
                mutual_line(ui, cx, pal, user.id, guild_id, servers, friends);
            }
        });
        // Sections 16 apart, each under a label-role heading; no rules between them (§12).
        let activity = crate::activity::activity_of(cx.state, user.id).map(std::borrow::Cow::into_owned);
        if bio.is_some() || activity.is_some() {
            part(ui, secs, 2, |ui| {
                if let Some(bio) = bio {
                    section(ui, pal, "About me");
                    let opts = super::markdown::MdOpts { guild_id, mentions: &[], id_salt: user.id, color: Some(pal.text), highlight: &[], bg: None };
                    super::markdown::render(ui, cx, &bio, &opts);
                }
                // Its own section like Roles: "Playing", "Listening to Spotify", … as the label. Mine
                // includes the game I share.
                if let Some(activity) = activity {
                    section(ui, pal, &activity.heading());
                    activity_section(ui, cx, pal, user.id, &activity, 64.0, pal.surface);
                }
            });
        }
        let roles = guild_roles(cx, guild_id, member);
        if !account || !roles.is_empty() || note.is_some() {
            part(ui, secs, 3, |ui| {
                if !account {
                    section(ui, pal, "Member since");
                    ui.label(RichText::new(member_since(user.id)).size(pal.ui()).color(pal.text));
                }
                if !roles.is_empty() {
                    section(ui, pal, "Roles");
                    role_pills(ui, cx.images, pal, &roles, 3);
                }
                if !account {
                    // Plugins: Friends Since, In Voice, Mutual Groups, Connections, Online On, permissions.
                    crate::plugins::people::profile_sections(ui, cx, user, guild_id, true, &|ui, list| cosmetics::connections_grid(ui, pal, list));
                }
                if let Some(note) = note {
                    section(ui, pal, "Note");
                    profile_modal::note_field(ui, cx, user.id, &note);
                }
            });
        }
    });
    rect.bottom()
}

/// An activity card (profile card, full profile, Active Now), `side` px art: its picture
/// (`activity::art_url`) or its type icon on a tile, the small image on the corner (`bg`: its
/// ring), then per kind (docs/DESIGN.md §11.2):
/// - game, watching, competing: name, details, state with the party size, "12:34 elapsed";
/// - Spotify: song, "by" artist, "on" album, a progress bar with both times, "Play on Spotify";
/// - stream: "Live" tag and title, the game, "Watch".
///
/// Then its buttons: "Join" / "Ask to Join" (`RpcState::join_button`), the custom ones, Spotify's or the stream's.
/// The links open in the browser on a click only. Clocks repaint once a second while shown.
/// `user`: whose activity (custom button links are fetched per user).
pub(super) fn activity_section(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, user: Id, a: &Activity, side: f32, bg: egui::Color32) {
    let rich = a.rich.as_deref();
    let now = super::threads::unix_ms();
    let (spotify, stream) = (a.is_spotify(), a.kind == 1);
    let start = rich.and_then(|r| r.start).filter(|&s| s <= now);
    let progress = a.progress(now).filter(|_| spotify);
    let timer = start.filter(|_| !spotify && !stream);
    // Wake once, when the shown time next changes: "for 47 min" each minute, a song each second.
    let since = now - start.unwrap_or(now);
    if timer.is_some() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(60_000 - since % 60_000));
    } else if progress.is_some_and(|(p, t)| p < t) {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(1000 - since % 1000));
    }
    let heading = super::semibold(pal.heading());
    let (body, meta) = (FontId::proportional(pal.ui()), FontId::proportional(pal.meta()));
    let field = |f: fn(&crate::model::RichPresence) -> &Option<String>| rich.and_then(|r| f(r).clone());
    let (details, state) = (field(|r| &r.details), field(|r| &r.state));
    let title = |d: Option<String>| d.unwrap_or_else(|| a.name.clone());
    let lines = if spotify {
        [
            Some((title(details), heading, pal.text)),
            state.map(|s| (format!("by {s}"), body.clone(), pal.text)),
            field(|r| &r.large_text).map(|album| (format!("on {album}"), body, pal.muted)),
            None,
        ]
    } else if stream {
        [Some((title(details), heading, pal.text)), state.map(|game| (game, body, pal.muted)), None, None]
    } else {
        let party = rich.and_then(|r| r.party).map(|[n, max]| format!("({n} of {max})"));
        let state = match (state, party) {
            (Some(s), Some(p)) => Some(format!("{s} {p}")),
            (s, p) => s.or(p),
        };
        [
            Some((a.name.clone(), heading, pal.text)),
            details.map(|d| (d, body.clone(), pal.text)),
            state.map(|s| (s, body, pal.muted)),
            timer.and_then(|_| a.elapsed(now)).map(|t| (t, meta.clone(), pal.muted)),
        ]
    };
    let w = ui.available_width();
    let text_w = w - side - 12.0;
    // The stream's "Live" tag leads its title.
    let live = stream.then(|| super::one_line(ui, "Live", FontId::new(pal.badge(), bold_family()), pal.tk.danger_label, 64.0));
    let tag_w = live.as_ref().map_or(0.0, |g| g.size().x + 8.0 + 6.0);
    let galleys: Vec<_> = lines.into_iter().flatten().enumerate().map(|(i, (t, f, c))| super::one_line(ui, t, f, c, text_w - if i == 0 { tag_w } else { 0.0 })).collect();
    let text_h = galleys.iter().map(|g| g.size().y + 2.0).sum::<f32>() - 2.0;
    let (rect, _) = ui.allocate_exact_size(vec2(w, side.max(text_h)), Sense::hover());
    let img = Rect::from_min_size(pos2(rect.left(), rect.center().y - side / 2.0), Vec2::splat(side));
    let art = crate::activity::art_url(ui.ctx(), cx.state, a, 128).and_then(|u| cx.images.get(ui.ctx(), &u, 128, false));
    match art {
        Some(tex) => Image::from_texture(tex).corner_radius(pal.radius).paint_at(ui, img),
        // Rustcord's own activity (shown locally, never sent as art): the R mark.
        None if crate::activity::is_rustcord(a) => super::sidebar::logo_mark(ui.painter(), img, egui::CornerRadius::same((side * 0.15).round() as u8), pal.tk.raised, pal.tk.accent_ink),
        None => {
            let tile = pal.tk.hover;
            ui.painter().rect_filled(img, pal.radius, tile);
            let color = if stream { pal.tk.live } else { pal.muted };
            let icon = Rect::from_center_size(img.center(), Vec2::splat((side / 2.0).round()));
            super::icons::paint(ui.painter(), super::icons::activity(a.kind), icon, color, bg.blend(tile));
        }
    }
    let small = cdn::activity_image_url(a, true, 64).filter(|_| !spotify).and_then(|u| cx.images.get(ui.ctx(), &u, 64, false));
    if let Some(tex) = small {
        let small = Rect::from_center_size(img.right_bottom() - vec2(3.0, 3.0), Vec2::splat((side * 0.375).round()));
        ui.painter().circle_filled(small.center(), small.width() / 2.0 + 2.0, bg);
        Image::from_texture(tex).corner_radius(small.width() / 2.0).paint_at(ui, small);
    }
    let tip = [field(|r| &r.large_text).filter(|_| !spotify), field(|r| &r.small_text)].into_iter().flatten().collect::<Vec<_>>().join("\n");
    if !tip.is_empty() {
        ui.interact(img, ui.make_persistent_id(("rc_activity_art", rect.min.x.to_bits(), rect.min.y.to_bits())), Sense::hover()).on_hover_text(tip);
    }
    let mut y = rect.center().y - text_h / 2.0;
    for (i, g) in galleys.into_iter().enumerate() {
        let (h, mut x) = (g.size().y, img.right() + 12.0);
        if let (0, Some(tag)) = (i, &live) {
            let r = Rect::from_min_size(pos2(x, y + (h - tag.size().y - 2.0) / 2.0), tag.size() + vec2(8.0, 2.0));
            ui.painter().rect_filled(r, (pal.radius / 2).max(2), pal.tk.danger_fill);
            ui.painter().galley(r.min + vec2(4.0, 1.0), tag.clone(), pal.tk.danger_label);
            x = r.right() + 6.0;
        }
        ui.painter().galley(pos2(x, y), g, pal.text);
        y += h + 2.0;
    }
    if let Some((played, total)) = progress {
        ui.add_space(8.0);
        let (bar, _) = ui.allocate_exact_size(vec2(w, 4.0), Sense::hover());
        ui.painter().rect_filled(bar, 2.0, pal.muted.gamma_multiply(0.35));
        let done = Rect::from_min_size(bar.min, vec2(bar.width() * played as f32 / total as f32, bar.height()));
        ui.painter().rect_filled(done, 2.0, pal.text);
        let (l, r) = [played, total].map(|ms| super::one_line(ui, crate::model::clock(ms), meta.clone(), pal.muted, w / 2.0)).into();
        let (row, _) = ui.allocate_exact_size(vec2(w, l.size().y + 2.0), Sense::hover());
        ui.painter().galley(row.left_top() + vec2(0.0, 2.0), l, pal.muted);
        ui.painter().galley(pos2(row.right() - r.size().x, row.top() + 2.0), r, pal.muted);
    }
    let link = match (spotify, stream) {
        (true, _) => a.spotify_track_url().map(|u| ("Play on Spotify", u)),
        (_, true) => a.stream_url().map(|u| ("Watch", u.to_owned())),
        _ => None,
    };
    let me = cx.state.me.as_ref().map(|m| m.id);
    let friend = cx.state.social.relationships.get(&user).is_some_and(|r| r.kind == relationship_type::FRIEND);
    let same_voice = me.is_some_and(|me| cx.state.voice_states.values().any(|v| v.contains_key(&me) && v.contains_key(&user)));
    let join = a.join_label(friend, same_voice).filter(|_| me != Some(user));
    let custom = rich.map_or(&[][..], |r| &r.buttons[..]);
    if join.is_none() && custom.is_empty() && link.is_none() {
        return;
    }
    ui.add_space(8.0);
    // Rich-presence buttons (§11.2): secondary, 32, in a row; each opens its link on the click only.
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        if let Some(label) = join {
            // Only a click sends: "Join" asks Discord for the secret and hands it to my game over the local game connection; "Ask to Join" sends one message.
            let online = crate::activity::online(cx.state);
            let b = cx.state.rpc.join_button(label, online, user, a);
            let mut r = super::messages::button(ui, pal, Btn::Secondary, b.label, b.enabled);
            if let Some(h) = &b.hover {
                r = r.on_disabled_hover_text(h);
            }
            if r.clicked() {
                cx.state.rpc.join_click(cx.cmd, label, online, user, a, super::composer::new_nonce);
            }
        }
        for (i, label) in custom.iter().enumerate() {
            let known = rich.and_then(|r| r.button_urls.get(i));
            let mut r = super::messages::button(ui, pal, Btn::Secondary, label, true).on_hover_cursor(egui::CursorIcon::PointingHand);
            if let Some(u) = known {
                r = r.on_hover_text(u);
            }
            if r.clicked() && let Some(url) = crate::activity::button_url(cx.state, cx.cmd, user, a, i).filter(|u| super::markdown::is_web_scheme(u)) {
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            }
        }
        if let Some((label, url)) = link
            && super::messages::button(ui, pal, Btn::Secondary, label, true).on_hover_text(&url).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
        {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
    });
}

/// Section heading in the label role (13/600, muted, sentence case), 16 px under the previous
/// section, like the full profile's (`profile_modal::label`).
fn section(ui: &mut egui::Ui, pal: &Pal, text: &str) {
    ui.add_space(16.0 - ui.spacing().item_spacing.y);
    ui.label(RichText::new(text).font(super::semibold(pal.label())).color(pal.muted));
}

/// Role chip height and the gap between chips / rows.
const CHIP_H: f32 = 22.0;
const CHIP_GAP: f32 = 4.0;
/// Chip padding left of the colour dot (8 px) + gap to the name, and right of the name.
const CHIP_LEAD: f32 = 6.0 + 8.0 + 4.0;
const CHIP_PAD: f32 = 6.0;
/// A role's icon in its pill: 14 px, 4 px after the dot.
const CHIP_ICON: f32 = 14.0;
const CHIP_ICON_W: f32 = CHIP_ICON + 4.0;

/// A pill's width: the dot, the role icon if it has one, the name.
fn chip_width(name_w: f32, icon: bool) -> f32 {
    CHIP_LEAD + if icon { CHIP_ICON_W } else { 0.0 } + name_w + CHIP_PAD
}

/// A role in the profile's list: name, colour, and its icon when the server shows role icons.
pub(super) type RolePill = (String, u32, Option<crate::model::Role>);

/// (row, x) of chips of `widths` flowed into rows of `max_w`.
fn flow(widths: &[f32], max_w: f32) -> Vec<(usize, f32)> {
    let (mut row, mut x) = (0, 0.0);
    widths
        .iter()
        .map(|&w| {
            if x > 0.0 && x + w > max_w {
                (row, x) = (row + 1, 0.0);
            }
            let at = (row, x);
            x += w + CHIP_GAP;
            at
        })
        .collect()
}

/// How many of the chips fit in `max_rows` rows, leaving room for a "+N" chip `more_w` wide
/// after them when not all do.
fn chips_shown(widths: &[f32], max_w: f32, max_rows: usize, more_w: f32) -> usize {
    let fits = |ws: &[f32]| flow(ws, max_w).last().is_none_or(|&(r, _)| r < max_rows);
    if fits(widths) {
        return widths.len();
    }
    (0..widths.len()).rev().find(|&k| fits(&[&widths[..k], &[more_w][..]].concat())).unwrap_or(0)
}

/// Role chips (colour dot + name) flowed into rows of the available width: a name longer than
/// a row is cut with "…" (tooltip: the whole name); beyond `max_rows` rows a "+N" chip that
/// shows them all (the card scrolls), until the roles change.
pub(super) fn role_pills(ui: &mut egui::Ui, images: &mut images::ImageCache, pal: &Pal, roles: &[RolePill], max_rows: usize) {
    let open_id = ui.make_persistent_id(("rc_roles_open", roles.iter().map(|r| &r.0).collect::<Vec<_>>()));
    let max_rows = if ui.data(|d| d.get_temp::<bool>(open_id).unwrap_or(false)) { usize::MAX } else { max_rows };
    let font = FontId::new(pal.meta(), crate::theme::medium_family());
    let max_w = ui.available_width();
    // An icon that can't be drawn (an emoji the fonts lack) leaves no room.
    let icons: Vec<Option<&crate::model::Role>> = roles
        .iter()
        .map(|(_, _, r)| r.as_ref().filter(|r| r.icon.as_deref().is_some_and(|h| !h.is_empty()) || r.unicode_emoji.as_deref().is_some_and(|e| super::messages::has_emoji_glyph(ui, e))))
        .collect();
    let galleys: Vec<_> = roles
        .iter()
        .zip(&icons)
        .map(|((name, ..), icon)| super::one_line(ui, name.as_str(), font.clone(), pal.text, max_w - chip_width(0.0, icon.is_some())))
        .collect();
    let mut widths: Vec<f32> = galleys.iter().zip(&icons).map(|(g, icon)| chip_width(g.size().x, icon.is_some())).collect();
    let more = |n: usize| super::one_line(ui, format!("+{n}"), font.clone(), pal.muted, max_w);
    let shown = chips_shown(&widths, max_w, max_rows, more(roles.len()).size().x + 2.0 * CHIP_PAD);
    let rest = roles.len() - shown;
    widths.truncate(shown);
    let more_g = (rest > 0).then(|| more(rest));
    if let Some(g) = &more_g {
        widths.push(g.size().x + 2.0 * CHIP_PAD);
    }
    let at = flow(&widths, max_w);
    let rows = at.last().map_or(0, |&(r, _)| r + 1) as f32;
    let (rect, _) = ui.allocate_exact_size(vec2(max_w, rows * CHIP_H + (rows - 1.0).max(0.0) * CHIP_GAP), Sense::hover());
    let chip = |i: usize| {
        let (row, x) = at[i];
        Rect::from_min_size(rect.min + vec2(x, row as f32 * (CHIP_H + CHIP_GAP)), vec2(widths[i], CHIP_H))
    };
    // Chip tier radius; a neutral tint that shows on the raised card in every mode.
    let radius = (pal.radius / 2).max(2);
    for (i, (((name, color, _), g), icon)) in roles.iter().zip(galleys).zip(&icons).take(shown).enumerate() {
        let r = chip(i);
        ui.painter().rect_filled(r, radius, pal.tk.hover);
        ui.painter().circle_filled(pos2(r.left() + 10.0, r.center().y), 4.0, if *color != 0 { rgb(*color) } else { pal.muted });
        crate::plugins::people::role_dot(ui, pos2(r.left() + 10.0, r.center().y), *color);
        let elided = g.elided;
        let name_x = if let Some(role) = icon {
            let at = pos2(r.left() + CHIP_LEAD + CHIP_ICON / 2.0, r.center().y);
            super::messages::paint_role_icon(ui, images, role, Rect::from_center_size(at, Vec2::splat(CHIP_ICON)), pal.text);
            CHIP_LEAD + CHIP_ICON_W
        } else {
            CHIP_LEAD
        };
        ui.painter().galley(pos2(r.left() + name_x, r.center().y - g.size().y / 2.0), g, pal.text);
        if elided {
            ui.interact(r, ui.make_persistent_id(("rc_role_chip", i)), Sense::hover()).on_hover_text(name);
        }
    }
    if let Some(g) = more_g {
        let r = chip(shown);
        let resp = ui.interact(r, ui.make_persistent_id("rc_role_more"), Sense::click());
        ui.painter().rect_filled(r, radius, if resp.hovered() { pal.tk.selected } else { pal.tk.hover });
        ui.painter().galley(pos2(r.left() + CHIP_PAD, r.center().y - g.size().y / 2.0), g, if resp.hovered() { pal.text } else { pal.muted });
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Show all roles"));
        if resp.on_hover_text("Show all roles").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            ui.data_mut(|d| d.insert_temp(open_id, true));
        }
    }
}

/// "3 mutual servers", "2 mutual friends": two ghost links, each opens the full profile on that tab.
fn mutual_line(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, user_id: Id, guild_id: Option<Id>, servers: usize, friends: usize) {
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        for (n, text, tab) in [(servers, "server", Tab::Servers), (friends, "friend", Tab::Friends)] {
            if n == 0 {
                continue;
            }
            let label = egui::Label::new(RichText::new(format!("{n} mutual {text}{}", plural(n))).size(pal.meta()).color(pal.muted)).sense(Sense::click());
            let link = ui.add(label);
            if link.hovered() {
                ui.painter().hline(link.rect.x_range(), link.rect.bottom(), Stroke::new(1.0, pal.muted));
            }
            if link.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                profile_modal::open_tab(cx, user_id, guild_id, tab);
            }
        }
    });
}

/// Account creation date from the user id ("Jan 5, 2021").
pub(super) fn member_since(user_id: Id) -> String {
    let at = chrono::DateTime::from_timestamp_millis(user_id.timestamp_ms() as i64);
    at.map_or_else(String::new, |t| t.format("%b %-d, %Y").to_string())
}

/// (image url, colour under or instead of it) of a user's banner: my local override, the
/// loaded profile's (server profile first), the cached user's. The colour: the profile theme,
/// else the user's accent colour; None = `band_fill`'s fallback.
pub(super) fn banner(cx: &UiCx<'_>, user: &User, guild_id: Option<Id>, is_me: bool) -> (Option<String>, Option<egui::Color32>) {
    let profile = profile_modal::loaded(cx, user.id, guild_id);
    let local = is_me.then(|| cx.overrides.url(Kind::Banner)).flatten();
    let url = local.or_else(|| match profile.and_then(|p| p.banner()) {
        // shortcut: cdn.rs has no server-profile banner URL; same asset layout under guilds/{g}/users/{u}.
        Some((hash, true)) => {
            let user_url = cdn::user_banner_url(user.id, hash, 600);
            guild_id.map(|g| user_url.replacen(&format!("banners/{}", user.id), &format!("guilds/{g}/users/{}/banners", user.id), 1))
        }
        Some((hash, false)) => Some(cdn::user_banner_url(user.id, hash, 600)),
        None => user.banner.as_deref().map(|h| cdn::user_banner_url(user.id, h, 600)),
    });
    let color = profile.and_then(|p| p.theme_colors()).map(|(primary, _)| primary).or(user.accent_color);
    (url, color.map(rgb))
}

/// The banner band's fill under or instead of its image: the user's colour, else a step off
/// the card (`selected` over `raised`), so every card keeps the band the avatar overlaps
/// (docs/DESIGN.md §12). A themed card's own gradient is the band there.
pub(super) fn band_fill(fill: Option<Color32>, pal: &Pal, themed: bool) -> Color32 {
    fill.unwrap_or(if themed { Color32::TRANSPARENT } else { pal.tk.selected })
}

/// Whether the banner `url` animates: my local one per its setting (or while `hovered`),
/// others' when animated.
pub(super) fn banner_animates(cx: &UiCx<'_>, url: Option<&str>, is_me: bool, hovered: bool) -> bool {
    if is_me && cx.overrides.url(Kind::Banner).is_some() {
        cx.overrides.avatar_animate == AnimateMode::Always || hovered
    } else {
        url.is_some_and(|u| u.contains("/a_"))
    }
}

/// Lower share of an uploaded banner that fades into the card.
const BANNER_FADE: f32 = 0.45;

/// An uploaded banner cover-cropped into `rect` (rounded `top` corners), its lower part fading
/// out into what is painted under it: the card surface or the profile theme. `veil`: a themed
/// card's body overlay (`cosmetics::theme_veil`), ramped in under the fade so the banner meets
/// the themed body without a seam.
pub(super) fn paint_banner(painter: &egui::Painter, rect: Rect, top: CornerRadius, tex: SizedTexture, veil: Option<Color32>) {
    // The fade ends on the pixel grid, where the themed body overlay (a rect, snapped by the
    // tessellator) starts: from a fractional edge neither covered the row between them (a 1 px
    // line of bare theme gradient).
    let rect = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.max.y.round_to_pixels(painter.pixels_per_point())));
    let uv = cover_uv(tex.size, rect.size());
    let v = |y: f32| egui::remap(y, rect.y_range(), uv.y_range());
    let split = rect.bottom() - rect.height() * BANNER_FADE;
    let (mut fade, mut ramp) = (egui::Mesh::with_texture(tex.id), egui::Mesh::default());
    const ROWS: u32 = 8;
    for i in 0..=ROWS {
        let t = i as f32 / ROWS as f32;
        // Smoothstep: no visible edge where the fade starts or ends.
        let a = t * t * (3.0 - 2.0 * t);
        let y = egui::lerp(split..=rect.bottom(), t);
        for (x, u) in [(rect.left(), uv.left()), (rect.right(), uv.right())] {
            fade.vertices.push(Vertex { pos: pos2(x, y), uv: pos2(u, v(y)), color: Color32::WHITE.gamma_multiply(1.0 - a) });
            ramp.colored_vertex(pos2(x, y), veil.unwrap_or_default().gamma_multiply(a));
        }
        if i > 0 {
            let k = 2 * i;
            for m in [&mut fade, &mut ramp] {
                m.add_triangle(k - 2, k - 1, k);
                m.add_triangle(k - 1, k + 1, k);
            }
        }
    }
    if veil.is_some() {
        painter.add(ramp);
    }
    // The rounded solid part reaches 1 px into the (still opaque) fade: no seam from its feathered edge.
    let solid = Rect::from_x_y_ranges(rect.x_range(), rect.top()..=split + 1.0);
    let solid_uv = Rect::from_x_y_ranges(uv.x_range(), uv.top()..=v(split + 1.0));
    painter.add(RectShape::filled(solid, top, Color32::WHITE).with_texture(tex.id, solid_uv));
    painter.add(fade);
}

/// The friend-state button of a profile: Add Friend, Accept (incoming), a disabled "sent" mark
/// (outgoing); nothing for friends, blocked users, bots and me.
pub(super) fn friend_button(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, user_id: Id) {
    use relationship_type::*;
    match cx.state.social.relationships.get(&user_id).map(|r| r.kind) {
        Some(INCOMING) => {
            if vector_button(ui, pal, Icon::Check, "Accept friend request", 32.0, false, pal.text).clicked() {
                super::friends::add_friend(cx, user_id, ContextLocation::UserProfile);
            }
        }
        Some(OUTGOING) => {
            let sent = ui.add_enabled_ui(false, |ui| vector_button(ui, pal, Icon::Hourglass, "Friend request sent", 32.0, false, pal.text));
            sent.inner.on_disabled_hover_text("Friend request sent");
        }
        _ if super::friends::can_add_friend(cx.state, user_id) && vector_button(ui, pal, Icon::PersonAdd, "Add friend", 32.0, false, pal.text).clicked() => {
            super::friends::add_friend(cx, user_id, ContextLocation::UserProfile);
        }
        _ => {}
    }
}

/// The card's avatar opens the full profile, like Discord's: pointing hand, Tab focus with a ring,
/// Enter / Space, a veil that fades in over FAST on hover (principle 8), and the modal grows from
/// the avatar (principle 11).
fn avatar_link(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, avatar: Rect, user_id: Id, guild_id: Option<Id>) {
    let resp = ui.interact(avatar, ui.make_persistent_id("rc_card_avatar"), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "View full profile"));
    let hot = resp.hovered() || resp.has_focus();
    let veil = super::motion::anim(ui.ctx(), resp.id, hot, super::motion::FAST, super::motion::FAST * super::motion::OUT);
    if veil > 0.0 {
        ui.painter().circle_filled(avatar.center(), avatar.width() / 2.0, Color32::from_black_alpha((veil * 56.0) as u8));
    }
    if resp.has_focus() {
        super::messages::focus_ring(ui, pal, avatar, avatar.width() / 2.0);
    }
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        super::motion::modal_origin(ui.ctx(), avatar.center());
        profile_modal::open(cx, user_id, guild_id);
    }
}

/// The footer's More menu (`actions`).
fn menu_id() -> egui::Id {
    egui::Id::unique("rc_profile_card_more")
}

/// Popout footer (§12): Message (my own card: Edit profile), Add Friend (friends.rs, not on
/// mine), and More: View full profile (profile_modal.rs), Copy user ID in Developer Mode.
fn actions(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, user_id: Id, guild_id: Option<Id>, is_me: bool) {
    Frame::new().inner_margin(Margin { left: 16, right: 16, top: 0, bottom: 16 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            // The card's one primary: Message, or on my own card (opened in a server or DM) Edit profile.
            let (label, own) = if is_me { ("Edit profile", true) } else { ("Message", false) };
            if super::messages::button(ui, pal, Btn::Primary, label, true).clicked() {
                if own {
                    cx.view.profile_card = None;
                    (cx.view.settings_open, cx.view.settings_page) = (true, super::user_settings::Page::Profiles);
                } else {
                    super::friends::message_user(cx, user_id);
                }
            }
            if !is_me {
                friend_button(ui, cx, pal, user_id);
            }
            let more = vector_button(ui, pal, Icon::More, "More", 32.0, false, pal.text);
            egui::Popup::menu(&more).id(menu_id()).show(|ui| {
                menu_setup(ui);
                // The app's menu colours: `pal` may be a themed card's (cosmetics::theme_pal).
                let text = cx.theme.colors.text.to_color32();
                if menu_item(ui, "View full profile", MenuIcon::Icon(Icon::Person), text) {
                    profile_modal::open(cx, user_id, guild_id);
                }
                // Settings > Advanced > Developer Mode.
                if cx.state.account.settings.developer_mode() {
                    ui.separator();
                    if menu_item(ui, "Copy user ID", MenuIcon::Icon(Icon::IdCard), text) {
                        ui.ctx().copy_text(user_id.0.to_string());
                    }
                }
            });
        });
    });
}

/// (name, color) of the member's roles in the guild, highest first, without @everyone.
pub(super) fn guild_roles(cx: &UiCx<'_>, guild_id: Option<Id>, member: Option<&Member>) -> Vec<RolePill> {
    let (Some(g), Some(m)) = (guild_id, member) else { return Vec::new() };
    let Some(guild) = cx.state.guild(g) else { return Vec::new() };
    let mut roles: Vec<_> = guild.roles.iter().filter(|r| r.id != g && m.roles.contains(&r.id)).collect();
    roles.sort_by_key(|r| std::cmp::Reverse(r.position));
    roles.into_iter().map(|r| (r.name.clone(), r.color, cx.state.role_icon(g, &[r.id]).cloned())).collect()
}

fn find_user(cx: &UiCx<'_>, id: Id) -> Option<User> {
    let s = &*cx.state;
    if let Some(me) = s.me.as_ref().filter(|u| u.id == id) {
        return Some(me.clone());
    }
    if let Some(u) = s.users.get(&id) {
        return Some(u.clone());
    }
    let in_channel = s.selected_channel.and_then(|c| s.messages.get(&c)).and_then(|c| {
        c.msgs.iter().rev().find_map(|m| {
            if m.author.id == id { Some(m.author.clone()) } else { m.mentions.iter().find(|u| u.id == id).cloned() }
        })
    });
    in_channel.or_else(|| {
        s.member_lists.values().flat_map(|l| &l.items).find_map(|it| match it {
            MemberListItem::Member { member, .. } => member.user.as_ref().filter(|u| u.id == id).cloned(),
            _ => None,
        })
    })
}

/// Member data for the card: cached (mine / member list), else from a message in the open channel.
fn card_member(cx: &UiCx<'_>, guild_id: Option<Id>, user_id: Id) -> Option<Member> {
    let g = guild_id?;
    let s = &*cx.state;
    s.find_member(g, user_id).cloned().or_else(|| {
        s.selected_channel
            .and_then(|c| s.messages.get(&c))?
            .msgs
            .iter()
            .rev()
            .find(|m| m.author.id == user_id && m.member.is_some())
            .and_then(|m| m.member.clone())
    })
}

/// UV rect that crops a texture to fill `target` (like CSS `object-fit: cover`).
pub(super) fn cover_uv(tex: Vec2, target: Vec2) -> Rect {
    let full = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    if tex.x <= 0.0 || tex.y <= 0.0 || target.x <= 0.0 || target.y <= 0.0 {
        return full;
    }
    let (tex_aspect, target_aspect) = (tex.x / tex.y, target.x / target.y);
    if tex_aspect > target_aspect {
        let w = target_aspect / tex_aspect;
        Rect::from_min_max(pos2((1.0 - w) / 2.0, 0.0), pos2((1.0 + w) / 2.0, 1.0))
    } else {
        let h = tex_aspect / target_aspect;
        Rect::from_min_max(pos2(0.0, (1.0 - h) / 2.0), pos2(1.0, (1.0 + h) / 2.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_crops_the_longer_side() {
        let full = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        assert_eq!(cover_uv(vec2(600.0, 200.0), vec2(300.0, 100.0)), full);
        // Square texture into a 3:1 banner: keep the middle third vertically.
        let uv = cover_uv(vec2(600.0, 600.0), vec2(300.0, 100.0));
        assert!((uv.min.y - 1.0 / 3.0).abs() < 1e-5 && (uv.max.y - 2.0 / 3.0).abs() < 1e-5);
        assert_eq!((uv.min.x, uv.max.x), (0.0, 1.0));
        // Wide texture into a square: keep the middle horizontally.
        let uv = cover_uv(vec2(400.0, 100.0), vec2(100.0, 100.0));
        assert!((uv.min.x - 0.375).abs() < 1e-5 && (uv.max.x - 0.625).abs() < 1e-5);
        assert_eq!(cover_uv(Vec2::ZERO, vec2(1.0, 1.0)), full);
    }

    #[test]
    fn the_speech_corner_points_at_the_row() {
        let (at, card) = (pos2(500.0, 100.0), |top: f32| Some(Rect::from_min_size(pos2(160.0, top), vec2(340.0, 300.0))));
        assert_eq!(speech(Align2::RIGHT_TOP, 16, card(108.0), at), CornerRadius { ne: 2, ..CornerRadius::same(16) });
        assert_eq!(speech(Align2::LEFT_TOP, 16, card(100.0), at), CornerRadius { nw: 2, ..CornerRadius::same(16) });
        assert_eq!(speech(Align2::RIGHT_TOP, 16, card(140.0), at), CornerRadius::same(16), "pushed 40 px off its anchor");
        assert_eq!(speech(Align2::CENTER_TOP, 16, card(100.0), at), CornerRadius::same(16));
        assert_eq!(speech(Align2::LEFT_BOTTOM, 16, card(-200.0), at), CornerRadius { sw: 2, ..CornerRadius::same(16) }, "bottom edge 100 on the anchor");
    }

    #[test]
    fn a_role_icon_widens_its_pill_by_18() {
        assert_eq!(chip_width(40.0, true) - chip_width(40.0, false), 18.0);
    }

    #[test]
    fn role_chips_wrap_and_collapse_into_more() {
        // Rows of 100: 60 + 4 + 30 fit, the next 60 wraps.
        assert_eq!(flow(&[60.0, 30.0, 60.0, 100.0], 100.0), vec![(0, 0.0), (0, 64.0), (1, 0.0), (2, 0.0)]);
        // A chip wider than the row (never: names are cut to it) still gets a row of its own.
        assert_eq!(flow(&[150.0, 10.0], 100.0), vec![(0, 0.0), (1, 0.0)]);
        // 40 chips of 40 in rows of 100: 2 per row; 3 rows show 5 of them and "+35".
        let widths = vec![40.0; 40];
        assert_eq!(chips_shown(&widths, 100.0, 3, 30.0), 5);
        assert_eq!(chips_shown(&widths[..6], 100.0, 3, 30.0), 6);
        assert_eq!(chips_shown(&widths, 100.0, usize::MAX, 30.0), 40);
        assert_eq!(chips_shown(&[], 100.0, 3, 30.0), 0);
    }

    #[test]
    fn every_card_keeps_its_banner_band() {
        let red = Color32::from_rgb(200, 0, 0);
        for theme in &crate::theme::Theme::builtins()[..3] {
            let pal = Pal::new(theme);
            assert_eq!(band_fill(Some(red), &pal, false), red);
            // No colour: still a band, a visible step off the raised card.
            assert_ne!(pal.tk.raised.blend(band_fill(None, &pal, false)), pal.tk.raised, "{}", theme.name);
            // A themed card's gradient is the band.
            assert_eq!(band_fill(None, &pal, true), Color32::TRANSPARENT);
        }
    }

    #[test]
    fn banner_fades_out_over_its_lower_part() {
        let ctx = egui::Context::default();
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(300.0, 100.0));
        let tex = SizedTexture::new(egui::TextureId::User(7), vec2(600.0, 600.0));
        let veil = Color32::from_black_alpha(153);
        let mut out = ctx.run_ui(Default::default(), |ui| paint_banner(&ui.ctx().layer_painter(egui::LayerId::background()), rect, CornerRadius::same(10), tex, Some(veil)));
        let meshes: Vec<_> = out.shapes.iter().filter_map(|c| if let egui::Shape::Mesh(m) = &c.shape { Some(m.clone()) } else { None }).collect();
        let (ramp, fade) = (&meshes[0], &meshes[1]);
        let (first, last) = (fade.vertices[0], *fade.vertices.last().unwrap());
        // Opaque where the fade starts, gone at the bottom edge, the texture still cover-cropped.
        assert_eq!((first.color.a(), last.color.a()), (255, 0));
        assert!((first.pos.y - 55.0).abs() < 1e-3 && last.pos.y == 100.0);
        assert!((last.uv.y - 2.0 / 3.0).abs() < 1e-5);
        // The themed body overlay is fully there at the bottom edge, absent where the fade starts.
        assert_eq!((ramp.vertices[0].color.a(), ramp.vertices.last().unwrap().color), (0, veil));
        out.textures_delta.clear();

        // 125 %: a bottom edge on a pixel centre (186 pt = 232.5 px) snaps to the grid like the
        // themed body rect under it, no row left between them.
        ctx.set_pixels_per_point(1.25);
        let rect = Rect::from_min_size(pos2(0.0, 80.0), vec2(300.0, 106.0));
        let out = ctx.run_ui(Default::default(), |ui| {
            assert_eq!(ui.ctx().pixels_per_point(), 1.25);
            paint_banner(&ui.ctx().layer_painter(egui::LayerId::background()), rect, CornerRadius::same(10), tex, Some(veil))
        });
        let bottom = out.shapes.iter().find_map(|c| if let egui::Shape::Mesh(m) = &c.shape { m.vertices.last().map(|v| v.pos.y) } else { None }).unwrap();
        assert_eq!(bottom * 1.25, 233.0);
    }
}
