//! Title bar (current location), server icon column (home/DMs button, 40 px squircle guild
//! icons, unread/selection pill, mention badge; `server_bar()` order with folders, drag and drop
//! to reorder / build folders via `move_guild`, saved debounced), the channel/DM list card with
//! voice users, and the floating user panel (voice connection; me, mute / deafen / settings)
//! over both columns.
//! Presence helpers (status icons, phone glyph, custom status / activity lines) live here too.
//! Social / server surfaces are hooks into their modules (docs/SOCIAL_UI_DESIGN.md): the server
//! header and menus (server_menu.rs), thread rows (threads.rs), the Friends row, group-DM "+"
//! and the user / DM menu items (friends.rs).
//! OWNERSHIP: navigation-social engineer (docs/SOCIAL_UI_DESIGN.md).

use super::images::{ImageCache, initial, initial_colors, paint_initial};
use super::icons::{self, Icon};
use super::messages::{MenuIcon, Pal, icon_button, menu_item, menu_setup};
use super::{UiCx, ViewState, bold, one_line, presence_color, rgb, server_menu, threads};
use crate::api::cdn;
use crate::backend::{AccountCommand, Command, ConnState};
use crate::config::AnimateMode;
use crate::model::{Activity, CustomStatus, GuildFolder, Id, Presence, UserStatus, channel_type as ct};
use crate::state::account::{GuildDrop, ServerBarItem};
use crate::state::{AppState, SidebarItem};
use crate::theme::{Theme, ThemeColors};
use egui::{Align2, Color32, CornerRadius, FontId, Rect, Role, Sense, Shape, Stroke, Vec2, WidgetInfo, pos2, vec2};
use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, PI};
use std::sync::Arc;
use std::time::{Duration, Instant};

const ICON: f32 = 40.0;
/// CDN size and decode size of guild icons (40 pt at up to 2x scaling).
pub(super) const ICON_PX: u32 = 80;
/// Server icons are squircles; hover/selection only changes their colours.
const ICON_R: f32 = 12.0;
/// Channel-list header; the chat header (messages.rs) has the same height. Incl. the 1 px border.
const HEADER_H: f32 = 49.0;
/// Account row of the floating user panel, and the voice section above it while in a call.
const ACCOUNT_H: f32 = 56.0;
const VOICE_PANEL_H: f32 = 56.0;
/// The music section at the top of the user panel (and its hairline).
const MUSIC_H: f32 = 40.0;
/// A user panel text column narrower than this shows my activity alone, without the custom
/// status and its divider, so the activity name gets the line. The default window's column is
/// 151 px and keeps both (docs/DESIGN.md §10.5).
const PANEL_STATUS_MIN_W: f32 = 140.0;
/// Camera / screen-share buttons under it once connected: 32 px + 8 px gap.
const VOICE_BUTTONS_H: f32 = 40.0;
/// Gap between the floating user panel and the window's left / bottom edges.
const PANEL_MARGIN: f32 = 8.0;
const ROW_H: f32 = 32.0;
const DM_ROW_H: f32 = 42.0;
const VOICE_ROW_H: f32 = 32.0;
const ROW_INSET: f32 = 8.0;
/// Panel padding: header title, category labels and the user panel start here.
const PAD: f32 = 16.0;

// ---------------------------------------------------------------------------
// Guild bar
// ---------------------------------------------------------------------------

pub fn guild_bar(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    user_panel_space(ui, cx, "guild_bar_panel_space");
    demo_folders(ui, cx.state);
    demo_boost(cx.state);
    demo_ping(ui.ctx(), cx.state);
    save_folders(ui.ctx(), cx);
    egui::ScrollArea::vertical()
        .id_salt("guild_bar")
        .auto_shrink(false)
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| {
            // 40 px icons + 8 = the 48 px pitch.
            ui.spacing_mut().item_spacing.y = 8.0;
            // The pill's content-space origin, and the slot for its light under the tiles.
            let top = ui.max_rect().top();
            let light = ui.painter().add(Shape::Noop);
            ui.add_space(4.0);
            home_button(ui, cx);
            crate::plugins::sidebar::read_all_button(ui, cx);

            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
            ui.painter().rect_filled(Rect::from_center_size(r.center(), vec2(32.0, 1.0)), 1, line(&cx.theme.colors).color);

            let badges = cx.state.guild_unreads();
            let mut target = None;
            let mut fold = super::motion::Fold::default();
            for (i, item) in cx.state.server_bar().into_iter().enumerate() {
                match item {
                    ServerBarItem::Guild(id) => {
                        fold.row(ui, None, |ui| guild_button(ui, cx, id, &badges, Slot::Top(i), &mut target));
                    }
                    ServerBarItem::Folder(f) => folder(ui, cx, &f, i, &badges, &mut target, &mut fold),
                }
            }
            fold.row(ui, None, |ui| add_server_button(ui, cx));
            travelling_pill(ui, light, top, cx.theme);
            ui.add_space(16.0);
            finish_drag(ui, cx, target);
        });
}

/// The guild being dragged in the server bar (drag-and-drop payload).
struct GuildDrag(Id);

/// Where a server-bar row sits: top level at `server_bar()` index, or in a folder.
#[derive(Clone, Copy)]
enum Slot {
    Top(usize),
    Child(i64, usize),
}

/// What dropping the dragged guild here does, and its marker: a line between rows or a ring
/// around the icon it merges with.
struct Target {
    to: GuildDrop,
    line: Option<f32>,
    ring: Option<Rect>,
}

/// The guild being dragged, if any.
fn dragged(ctx: &egui::Context) -> Option<Id> {
    egui::DragAndDrop::payload::<GuildDrag>(ctx).map(|d| d.0)
}

/// Drop target for the pointer over the row of `icon` in `slot`: the 8 px at its top / bottom
/// (and half the gap) insert before / after it; the middle does `merge` (a guild: a new folder
/// with it; a closed folder: into it). In a folder the halves insert before / after.
fn zone(pointer: Option<egui::Pos2>, icon: Rect, slot: Slot, merge: Option<GuildDrop>) -> Option<Target> {
    let y = pointer?.y;
    let (top, bottom) = (icon.top() - 4.0, icon.bottom() + 4.0);
    if !(top..bottom).contains(&y) {
        return None;
    }
    let line = |to, y| Some(Target { to, line: Some(y), ring: None });
    match slot {
        Slot::Top(i) if y < icon.top() + 8.0 => line(GuildDrop::Top(i), top),
        Slot::Top(i) if y > icon.bottom() - 8.0 => line(GuildDrop::Top(i + 1), bottom),
        Slot::Top(_) => merge.map(|to| Target { to, line: None, ring: Some(icon) }),
        Slot::Child(id, j) if y < icon.center().y => line(GuildDrop::Folder { id, index: j }, top),
        Slot::Child(id, j) => line(GuildDrop::Folder { id, index: j + 1 }, bottom),
    }
}

/// End of the bar: draws the drop marker and the dragged icon at the pointer; on release moves
/// the guild there (`move_guild`, shown at once) and schedules the save.
fn finish_drag(ui: &mut egui::Ui, cx: &mut UiCx<'_>, target: Option<Target>) {
    let ctx = ui.ctx().clone();
    let Some(id) = dragged(&ctx) else { return };
    let mark = cx.theme.colors.text.to_color32();
    if let Some(t) = &target {
        if let Some(y) = t.line {
            ui.painter().hline(ui.max_rect().x_range().shrink(12.0), y, Stroke::new(2.0, mark));
        }
        if let Some(r) = t.ring {
            ui.painter().rect_stroke(r.expand(2.0), ICON_R + 2.0, Stroke::new(2.0, mark), egui::StrokeKind::Outside);
        }
    }
    if let (Some(p), Some(index)) = (ctx.pointer_latest_pos(), cx.state.guilds.iter().position(|g| g.id == id)) {
        let ghost = egui::Area::new(egui::Id::unique("rc_guild_drag")).order(egui::Order::Tooltip).interactable(false).fade_in(false);
        ghost.fixed_pos(p - Vec2::splat(ICON / 2.0)).show(&ctx, |ui| {
            super::motion::own(&ctx, ui.layer_id());
            let (r, _) = ui.allocate_exact_size(Vec2::splat(ICON), Sense::hover());
            ui.multiply_opacity(0.8);
            paint_guild_icon(ui, cx, index, r, tile_corners(1.0), 1.0, false);
        });
    }
    if !ctx.input(|i| i.pointer.any_released()) {
        return;
    }
    egui::DragAndDrop::clear_payload(&ctx);
    let Some(t) = target else { return };
    let before = cx.state.server_bar();
    if let Some(folders) = cx.state.move_guild(id, t.to) {
        cx.state.account.settings.folders = folders;
        if cx.state.server_bar() != before {
            ctx.data_mut(|d| d.insert_temp(save_id(), Instant::now()));
        }
    }
}

fn save_id() -> egui::Id {
    egui::Id::unique("rc_folders_save")
}

/// Sends the dragged server-bar order (`SetGuildFolders`, the whole list) 1.5 s after the last
/// drop, so a few quick drags make one request.
fn save_folders(ctx: &egui::Context, cx: &mut UiCx<'_>) {
    const DEBOUNCE: Duration = Duration::from_millis(1500);
    let Some(at) = ctx.data(|d| d.get_temp::<Instant>(save_id())) else { return };
    let left = DEBOUNCE.saturating_sub(at.elapsed());
    if !left.is_zero() {
        ctx.request_repaint_after(left);
        return;
    }
    ctx.data_mut(|d| d.remove::<Instant>(save_id()));
    let folders = cx.state.account.settings.folders.clone();
    cx.cmd.send(Command::Account(AccountCommand::SetGuildFolders { folders }));
}

/// A server folder: closed, a tile in its colour with up to four mini icons; open, a folder
/// glyph over its servers on a tinted background. A click toggles it (local only).
fn folder(ui: &mut egui::Ui, cx: &mut UiCx<'_>, f: &GuildFolder, index: usize, badges: &HashMap<Id, (bool, u32)>, target: &mut Option<Target>, fold: &mut super::motion::Fold) {
    let c = cx.theme.colors.clone();
    let fid = f.id.unwrap_or_default();
    let open_key = folder_open_id(fid);
    let open = ui.data(|d| d.get_temp::<bool>(open_key)).unwrap_or(false);
    // No colour picked: neutral (purple is the brand's, never a tile's).
    let color = f.color.map(rgb).unwrap_or(c.text_muted.to_color32());
    let name = f.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| {
        let names = f.guild_ids.iter().filter_map(|g| cx.state.guild(*g)).map(|g| g.name.as_str());
        names.collect::<Vec<_>>().join(", ")
    });
    let (unread, mentions) = f.guild_ids.iter().filter_map(|g| badges.get(g)).fold((false, 0), |(u, m), (gu, gm)| (u || *gu, m + gm));
    let selected = cx.state.selected_guild.is_some_and(|g| f.guild_ids.contains(&g));
    let open = crate::plugins::sidebar::force_open(open, selected);
    let k = super::motion::anim(ui.ctx(), open_key.with("fold"), open, super::motion::BASE, super::motion::BASE * super::motion::OUT);
    let bg = ui.painter().add(Shape::Noop);
    let base = fold.shift();
    // The tile folds with everything above it; its servers slide out from under it (`Fold`).
    let icon = fold.row(ui, None, |ui| {
        let (row, icon, resp, morph, h, corner) = icon_slot(ui, ("rc_folder", fid), (selected && !open, unread && !open));
        let corners = tile_corners(corner);
        if open {
            // The folder glyph on a plain tile; the background spans it and its servers.
            let tab = Rect::from_min_size(icon.center() + vec2(-11.0, -9.0), vec2(9.0, 5.0));
            ui.painter().rect_filled(tab, 2, color);
            ui.painter().rect_filled(Rect::from_center_size(icon.center() + vec2(0.0, 2.0), vec2(22.0, 15.0)), 3, color);
        } else {
            ui.painter().rect_filled(icon, corners, color.gamma_multiply(0.25 + 0.15 * morph));
            for (k, g) in f.guild_ids.iter().take(4).enumerate() {
                let at = icon.min + vec2(3.0 + 18.0 * (k % 2) as f32, 3.0 + 18.0 * (k / 2) as f32);
                if let Some(i) = cx.state.guilds.iter().position(|x| x.id == *g) {
                    paint_guild_icon(ui, cx, i, Rect::from_min_size(at, Vec2::splat(16.0)), 5.0, 0.0, false);
                }
            }
        }
        // The folder's total counts the real number (open or shut), so closing it never pops.
        let muted = f.guild_ids.iter().all(|g| cx.state.is_guild_muted(*g));
        let pop = ping(ui.ctx(), resp.id, mentions, muted, &cx.state.conn);
        decorate(ui, &c, (row, icon, &resp), (h, corners), (if open { 0 } else { mentions }, pop));
        resp.widget_info(|| WidgetInfo::selected(Role::Button, true, open, format!("Folder {name}")));
        let dragging = dragged(ui.ctx()).is_some();
        if !dragging {
            side_tooltip(&resp, &name, None);
        }
        if resp.clicked() {
            ui.data_mut(|d| d.insert_temp(open_key, !open));
            crate::plugins::sidebar::folder_clicked(ui.ctx(), cx.state, fid, !open);
        }
        if dragging {
            let into = GuildDrop::Folder { id: fid, index: if open { 0 } else { usize::MAX } };
            let t = zone(ui.ctx().pointer_latest_pos(), icon, Slot::Top(index), Some(into));
            // Open: below the glyph's top edge everything goes in, first.
            let t = t.map(|t| if open && !matches!(t.to, GuildDrop::Top(i) if i == index) { Target { to: into, line: Some(icon.bottom() + 4.0), ring: None } } else { t });
            if t.is_some() {
                *target = t;
            }
        }
        icon
    });
    if !open && k <= 0.0 {
        return;
    }
    let mut last = icon;
    let run = egui::Id::unique(("rc_fold", fid));
    for (j, g) in f.guild_ids.iter().enumerate() {
        last = fold.row(ui, Some((run, k)), |ui| guild_button(ui, cx, *g, badges, Slot::Child(fid, j), target));
    }
    fold.end(ui.ctx());
    // The background slot is outside every shifted range: placed in screen terms.
    let back = Rect::from_min_max(icon.min - Vec2::splat(4.0) - vec2(0.0, base), last.max + Vec2::splat(4.0) - vec2(0.0, fold.shift()));
    ui.painter().set(bg, Shape::rect_filled(back, ICON_R + 4.0, color.gamma_multiply(0.15)));
}

/// Memory key of a folder's open state (local, this session).
fn folder_open_id(folder: i64) -> egui::Id {
    egui::Id::unique(("rc_folder_open", folder))
}

/// RUSTCORD_NAV_DEMO=folders | folder-open: saved folders as READY would bring them (the
/// first server alone, the others in a named folder; `folder-open` opens it).
fn demo_folders(ui: &egui::Ui, state: &mut AppState) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    if ONCE.is_completed() || !(nav_demo("folders") || nav_demo("folder-open")) {
        return;
    }
    ONCE.call_once(|| {
        let ids: Vec<Id> = state.guilds.iter().map(|g| g.id).collect();
        let Some((first, rest)) = ids.split_first() else { return };
        let folder = GuildFolder { id: Some(7), name: Some("Hobbies".into()), color: Some(0x3fb950), guild_ids: rest.to_vec() };
        state.account.settings.folders = vec![GuildFolder { guild_ids: vec![*first], ..Default::default() }, folder];
        state.account.settings.loaded = true;
        if nav_demo("folder-open") {
            ui.data_mut(|d| d.insert_temp(folder_open_id(7), true));
        }
    });
}

/// RUSTCORD_NAV_DEMO=ping: once, 3 s after start, a mention arrives in the first text channel of the
/// first server that is not open (local state only); `ping-again` adds a second one 1.5 s later.
fn demo_ping(ctx: &egui::Context, state: &mut AppState) {
    static DONE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
    let want = if nav_demo("ping-again") { 2 } else if nav_demo("ping") { 1 } else { 0 };
    let done = DONE.load(std::sync::atomic::Ordering::Relaxed);
    if done >= want {
        return;
    }
    let left = 3.0 + 1.5 * f64::from(done) - ctx.input(|i| i.time);
    if left > 0.0 {
        ctx.request_repaint_after(Duration::from_secs_f64(left));
        return;
    }
    let Some(guild) = state.guilds.iter().map(|g| g.id).find(|g| state.selected_guild != Some(*g)) else { return };
    let mut texts: Vec<Id> = state.channels.values().filter(|c| c.guild_id == Some(guild) && c.kind == crate::model::channel_type::GUILD_TEXT).map(|c| c.id).collect();
    texts.sort_unstable();
    if let Some(ch) = texts.first() {
        state.read_states.entry(*ch).or_insert_with(|| crate::model::ReadState { id: *ch, ..Default::default() }).mention_count += 1;
        DONE.store(done + 1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// RUSTCORD_NAV_DEMO=boost: server marks as READY would bring them: the first server boost level
/// 2 with 9 boosts, the second verified, the third partnered (and level 1, 3 boosts).
fn demo_boost(state: &mut AppState) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    if ONCE.is_completed() || !nav_demo("boost") {
        return;
    }
    ONCE.call_once(|| {
        let marks: [(&[&str], u8, u32); 3] = [(&[], 2, 9), (&["VERIFIED"], 0, 0), (&["PARTNERED"], 1, 3)];
        for (g, (features, tier, count)) in state.guilds.iter_mut().zip(marks) {
            (g.features, g.premium_tier, g.premium_subscription_count) = (features.iter().map(|f| (*f).to_owned()).collect(), tier, count);
        }
    });
}

/// Test hook `RUSTCORD_NAV_DEMO=<comma list>`: navigation / social states offline mode can't
/// reach, none sends a request. `folders` / `folder-open` (server-bar folders), `mentions`
/// (inbox mentions from cached messages), `recent` (the NotificationLog plugin's Recent tab rows from cached messages, three silenced), `profile` (loaded profiles: bio, pronouns, mutual
/// servers and friends, a note), `tab:<online|all|pending|blocked|add>` (Friends tab), `icons`
/// (every vector icon over the window, `icons::sheet`), `no-tray` (Linux: the settings page as
/// without a tray host, `user_settings/app.rs`), `press:<shortcut>` (that key combination,
/// e.g. `Alt+Down`, pressed once at frame 14 where the focus is; `user_settings::parse` names),
/// `join:<voice channel id>[,leave]` (the call island and the user panel's voice section: join at 2 s, connected at 3 s, muted at 4 s, leave at 6 s), `unread` (3 s after start the open server's last text channel turns unread), `ping-again` (a second `ping` mention 1.5 s later), `long-list` (40 local channels in the open server's first category), `scroll:<px>` (the channel list opens
/// scrolled by that much), `ping` (3 s after start one mention reaches a server that is not open), `collapsed` (the open server's first category folded at start; a click opens it), `music` / `music:paused` (the user panel's music row with a demo track; the real media session is
/// not read; a click on Play / Pause flips it).
pub(super) fn nav_demo(flag: &str) -> bool {
    nav_flags().split(',').any(|f| f.trim() == flag)
}

/// The value of a `key:value` entry of `RUSTCORD_NAV_DEMO`.
pub(super) fn nav_demo_value(key: &str) -> Option<&'static str> {
    nav_flags().split(',').find_map(|f| f.trim().strip_prefix(key)?.strip_prefix(':'))
}

fn nav_flags() -> &'static str {
    static FLAGS: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FLAGS.get_or_init(|| std::env::var("RUSTCORD_NAV_DEMO").unwrap_or_default())
}

/// Where the selected tile's row is this pass (y centre, pass number): the rail's one pill travels
/// there (`travelling_pill`).
fn rail_sel_id() -> egui::Id {
    egui::Id::unique("rc_rail_sel")
}

/// One 40x40 icon slot centred in a full-width row. Returns (row, icon rect, click response,
/// morph, pill height, corner): morph eases 0 -> 1 (colours only) while the icon is hovered or
/// `selected`; the pill height eases to 8 unread / 20 hover (the selected 40 is the rail's one
/// pill); corner eases 0 -> 1 as the tile's speech corner tightens (`tile_corners`).
fn icon_slot(ui: &mut egui::Ui, salt: impl egui::AsIdSalt, (selected, unread): (bool, bool)) -> (Rect, Rect, egui::Response, f32, f32, f32) {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), ICON), Sense::hover());
    let icon = Rect::from_center_size(row.center(), Vec2::splat(ICON));
    let resp = ui.interact(icon, ui.make_persistent_id(salt), Sense::click());
    let target = if selected || resp.hovered() { 1.0 } else { 0.0 };
    let morph = super::motion::value(ui.ctx(), resp.id.with("morph"), target, super::motion::FAST);
    let pill = if selected {
        0.0
    } else if resp.hovered() {
        20.0
    } else if unread {
        8.0
    } else {
        0.0
    };
    let h = super::motion::value(ui.ctx(), resp.id.with("pill"), pill, super::motion::FAST);
    let corner = super::motion::slide(ui.ctx(), resp.id.with("corner"), selected, super::motion::BASE);
    if selected {
        // The first selected tile wins: a shut folder is drawn before the server inside it.
        let pass = ui.ctx().cumulative_pass_nr();
        ui.data_mut(|d| {
            if d.get_temp::<(f32, u64)>(rail_sel_id()).is_none_or(|(_, p)| p != pass) {
                d.insert_temp(rail_sel_id(), (row.center().y, pass));
            }
        });
    }
    (row, super::motion::pressed(&resp, icon), resp, morph, h, corner)
}

/// Whether a server-bar badge going from `before` to `now` mentions pops (DESIGN §7): more than it
/// had (not the tile's first sight), not muted, the connection state held for 2 s (READY's counts
/// at start and on a reconnect stay still), a second since its last pop.
fn pops(before: Option<u32>, now: u32, muted: bool, settled_for: f64, since_pop: f64) -> bool {
    before.is_some_and(|b| now > b) && !muted && settled_for >= 2.0 && since_pop >= 1.0
}

/// Seconds the connection state (not its attempt counter) has held.
fn conn_settled(ctx: &egui::Context, conn: &crate::backend::ConnState) -> f64 {
    let (now, id) = (ctx.input(|i| i.time), egui::Id::unique("rc_conn_since"));
    let kind = std::mem::discriminant(conn);
    let (_, since) = ctx.data_mut(|d| {
        let e = d.get_temp_mut_or(id, (kind, now));
        if e.0 != kind {
            *e = (kind, now);
        }
        *e
    });
    now - since
}

/// The pop scale of the badge on the tile `id` (1 at rest): starts a pop when its mentions rose
/// (`pops`). A tile not drawn for a second (a shut folder's servers) counts as seen again.
fn ping(ctx: &egui::Context, id: egui::Id, mentions: u32, muted: bool, conn: &crate::backend::ConnState) -> (f32, Option<(u32, f32)>) {
    let now = ctx.input(|i| i.time);
    let key = id.with("ping");
    let (before, last_pop) = match ctx.data(|d| d.get_temp::<(u32, f64, f64)>(key)) {
        Some((count, pop, seen)) if now - seen < 1.0 => (Some(count), pop),
        Some((_, pop, _)) => (None, pop),
        None => (None, f64::NEG_INFINITY),
    };
    let pop = pops(before, mentions, muted, conn_settled(ctx, conn), now - last_pop);
    ctx.data_mut(|d| d.insert_temp(key, (mentions, if pop { now } else { last_pop }, now)));
    if pop {
        super::motion::pop(ctx, id.with("pop"));
    }
    // The digits roll when a shown count becomes another shown count.
    if let Some(b) = before.filter(|_| rolls(before, mentions)) {
        super::motion::pop(ctx, id.with("roll"));
        ctx.data_mut(|d| d.insert_temp(id.with("roll_from"), b));
    }
    let roll = super::motion::shot(ctx, id.with("roll"), super::motion::FAST).and_then(|t| Some((ctx.data(|d| d.get_temp::<u32>(id.with("roll_from")))?, super::motion::ease(t))));
    (super::motion::popped(ctx, id.with("pop")), roll)
}

/// Whether a badge going from `before` to `now` rolls its digits: both counts shown (not the first
/// sight, not to or from zero) and different.
fn rolls(before: Option<u32>, now: u32) -> bool {
    before.is_some_and(|b| b != now && b > 0 && now > 0)
}

/// The left-edge pill of height `h`, the mention badge with a cut-out ring in the bar color, and
/// the focus ring (it follows the tile's `corners`).
fn decorate(ui: &egui::Ui, c: &ThemeColors, (row, icon, resp): (Rect, Rect, &egui::Response), (h, corners): (f32, CornerRadius), (mentions, (pop, roll)): (u32, (f32, Option<(u32, f32)>))) {
    let tk = crate::theme::applied_tokens(ui.ctx());
    if h >= 1.0 {
        let pill = Rect::from_min_size(pos2(row.left(), row.center().y - h / 2.0), vec2(4.0, h));
        rail_pill(ui.painter(), pill, c.text.to_color32(), tk.accent);
    }
    if mentions > 0 {
        let at = pos2(icon.right() + 4.0, icon.bottom() - 4.0);
        // Pops about the badge's centre (about its right edge less half the 16 px minimum width).
        super::motion::painted_scaled(ui, at - vec2(8.0, 0.0), pop, || _ = badge_with(ui, c, at, mentions, Some(c.guild_bar.to_color32()), roll));
    }
    if resp.has_focus() {
        let grow = |r: u8| r.saturating_add(2);
        let ring = CornerRadius { nw: grow(corners.nw), ne: grow(corners.ne), se: grow(corners.se), sw: grow(corners.sw) };
        ui.painter().rect_stroke(icon.expand(2.0), ring, Stroke::new(2.0, tk.focus), egui::StrokeKind::Outside);
    }
}

/// The rail pill (docs/DESIGN.md §10.1): `text` up to 20 px tall, then toward `accent` on the way
/// to the selected 40 px. One flat rect in every theme.
fn rail_pill(p: &egui::Painter, pill: Rect, text: Color32, accent: Color32) {
    p.rect_filled(pill, CornerRadius { ne: 4, se: 4, ..Default::default() }, text.lerp_to_gamma(accent, pill_t(pill.height())));
}

/// The rail's one 40 px selection pill (docs/DESIGN.md §10.1): it travels from the old tile to the
/// picked one over BASE (`value` in the rail's content space, so scrolling never moves it) and is
/// painted after the tiles; its light (dark themes) goes into `light`, under them. A trip that
/// starts or ends out of view grows in place instead (`jumps`). Known lag: while a folder above the
/// selected server folds, the pill follows the layout and catches up within BASE after the fold.
fn travelling_pill(ui: &mut egui::Ui, light: egui::layers::ShapeIdx, top: f32, theme: &Theme) {
    use super::motion::{BASE, FAST, forget, set_value, value};
    let ctx = ui.ctx().clone();
    let pass = ctx.cumulative_pass_nr();
    let sel = ctx.data(|d| d.get_temp::<(f32, u64)>(rail_sel_id())).filter(|(_, p)| *p == pass).map(|(y, _)| y - top);
    let base = egui::Id::unique("rc_rail_pill");
    let (y_id, h_id, to_id) = (base.with("y"), base.with("h"), base.with("to"));
    let last = ctx.data(|d| d.get_temp::<f32>(to_id));
    if let (Some(new), Some(old)) = (sel, last) && new != old {
        // Where the pill is now (no retarget): a trip out of view grows in place.
        let at = value(&ctx, y_id, old, BASE);
        if jumps(ui.clip_rect().y_range(), top + at, top + new) {
            forget(&ctx, y_id);
            set_value(&ctx, h_id, 0.0);
        }
    }
    if let Some(y) = sel {
        ctx.data_mut(|d| d.insert_temp(to_id, y));
    }
    let y = value(&ctx, y_id, sel.or(last).unwrap_or(0.0), BASE);
    let h = value(&ctx, h_id, if sel.is_some() { 40.0 } else { 0.0 }, FAST);
    if h < 1.0 {
        return;
    }
    let tk = theme.tokens();
    let pill = Rect::from_min_size(pos2(ui.max_rect().left(), top + y - h / 2.0), vec2(4.0, h));
    if super::light::lit(&tk) && h >= 21.0 {
        let t = pill_t(h);
        let near = Rect::from_min_max(pill.left_top() - vec2(0.0, 8.0), pos2(pill.left() + 24.0, pill.bottom() + 8.0));
        let glow = |r: Rect, radius: f32, a: f32, blur: f32| super::light::glow(r, radius, tk.accent.gamma_multiply(a * t).additive(), blur);
        ui.painter().set(light, Shape::Vec(vec![glow(pill, 4.0, 0.75, 12.0), glow(near, 8.0, 0.35, 24.0)]));
    }
    rail_pill(ui.painter(), pill, theme.colors.text.to_color32(), tk.accent);
}

/// Whether a pill trip between two content y's has an end outside the visible `view` (it then grows
/// in place: sliding past tiles nobody can see says nothing).
fn jumps(view: egui::Rangef, from: f32, to: f32) -> bool {
    !view.contains(from) || !view.contains(to)
}

fn pill_t(h: f32) -> f32 {
    ((h - 20.0) / 20.0).clamp(0.0, 1.0)
}

/// A rail tile's corners: `ICON_R` everywhere, the bottom-left tightening to 2 as the pill turns
/// selected (the speech corner, §10).
fn tile_corners(t: f32) -> CornerRadius {
    let r = ICON_R as u8;
    CornerRadius { nw: r, ne: r, se: r, sw: egui::lerp(ICON_R..=2.0, t).round() as u8 }
}

/// Server-bar tooltip: the name in bold, right of the icon; a server's `mark` (verified,
/// partnered, boost level: `icons::guild_mark`) 16 px before it.
fn side_tooltip(resp: &egui::Response, text: &str, mark: Option<Icon>) {
    let mut tip = egui::Tooltip::for_enabled(resp);
    tip.popup = tip.popup.align(egui::RectAlign::RIGHT).align_alternatives(&[]).gap(12.0);
    tip.show(|ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if let Some(icon) = mark {
                super::messages::vector_icon(ui, icon, 16.0, ui.visuals().text_color(), ui.visuals().window_fill());
            }
            ui.label(egui::RichText::new(text).family(crate::theme::bold_family()));
        });
    });
}

fn home_button(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    let c = cx.theme.colors.clone();
    let tk = cx.theme.tokens();
    let selected = cx.state.selected_guild.is_none();
    let (row, icon, resp, morph, h, corner) = icon_slot(ui, "home", (selected, false));
    let corners = tile_corners(corner);
    // The logo itself: a neutral tile with the purple R; hover and selection turn the tile purple
    // (the one place hover is purple).
    let tile = c.surface.to_color32().lerp_to_gamma(tk.accent, morph);
    logo_mark(ui.painter(), icon, corners, tile, tk.accent_ink.lerp_to_gamma(tk.on_accent, morph));
    let dms = cx.state.dm_unread_count();
    let pop = ping(ui.ctx(), resp.id, dms, false, &cx.state.conn);
    decorate(ui, &c, (row, icon, &resp), (h, corners), (dms, pop));

    resp.widget_info(|| WidgetInfo::selected(Role::Button, true, selected, "Direct messages"));
    side_tooltip(&resp, "Direct messages", None);
    if resp.clicked() {
        cx.state.select_guild(None);
        cx.view.settings_open = false;
    }
}

/// A server icon in `slot`. Once the saved folders are known it can be dragged: onto a gap to
/// move it, onto a server to make a folder, onto a folder to file it (`target` = the drop
/// spot under the pointer). Returns the icon rect.
fn guild_button(ui: &mut egui::Ui, cx: &mut UiCx<'_>, id: Id, badges: &HashMap<Id, (bool, u32)>, slot: Slot, target: &mut Option<Target>) -> Rect {
    let c = cx.theme.colors.clone();
    let Some(index) = cx.state.guilds.iter().position(|g| g.id == id) else { return Rect::NOTHING };
    let g = &cx.state.guilds[index];
    let unavailable = g.unavailable;
    // Unavailable guilds (outage) may arrive with only their id.
    let name = if g.name.is_empty() { "Unavailable server".to_owned() } else { g.name.clone() };
    let selected = cx.state.selected_guild == Some(id);
    let (unread, mentions) = badges.get(&id).copied().unwrap_or_default();
    let mark = icons::guild_mark(&g.features, g.premium_tier);

    let (row, icon, resp, morph, h, corner) = icon_slot(ui, ("guild", id), (selected, unread));
    let corners = tile_corners(corner);
    let drag = dragged(ui.ctx());
    let resp = if cx.state.account.settings.loaded { resp.interact(Sense::drag()) } else { resp };
    if resp.drag_started() {
        egui::DragAndDrop::set_payload(ui.ctx(), GuildDrag(id));
    }
    // Animated icons play on hover (or always).
    let animate = resp.hovered() || cx.settings.animate_others == AnimateMode::Always;
    paint_guild_icon(ui, cx, index, icon, corners, morph, animate);
    if drag == Some(id) {
        // Its place while it is dragged: faded.
        ui.painter().rect_filled(icon, corners, c.guild_bar.to_color32().gamma_multiply(0.7));
    }
    let pop = ping(ui.ctx(), resp.id, mentions, cx.state.is_guild_muted(id), &cx.state.conn);
    decorate(ui, &c, (row, icon, &resp), (h, corners), (mentions, pop));

    // "Rust Workshop, boost level 2".
    let label = mark.map_or_else(|| name.clone(), |(_, what)| format!("{name}, {}", what.to_lowercase()));
    resp.widget_info(|| WidgetInfo::selected(Role::Button, true, selected, &label));
    if let Some(d) = drag {
        let merge = (d != id && matches!(slot, Slot::Top(_))).then_some(GuildDrop::NewFolder { with: id });
        if let Some(t) = zone(ui.ctx().pointer_latest_pos(), icon, slot, merge) {
            *target = Some(t);
        }
    } else {
        side_tooltip(&resp, &if unavailable { format!("{name} (unavailable)") } else { name.clone() }, mark.map(|m| m.0));
    }
    if resp.clicked() {
        cx.state.select_guild(Some(id));
        cx.view.settings_open = false;
    }
    resp.context_menu(|ui| server_menu::guild_menu(ui, cx, id));
    icon
}

/// "Add a server" (+) under the servers: opens the join / create dialog (server_menu.rs).
fn add_server_button(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    let c = cx.theme.colors.clone();
    let (_, icon, resp, morph, ..) = icon_slot(ui, "add_server", (false, false));
    let fill = tile_fill(ui, &c, morph);
    ui.painter().rect_filled(icon, ICON_R, fill);
    let fg = c.text_muted.to_color32().lerp_to_gamma(crate::theme::applied_tokens(ui.ctx()).positive, morph);
    icons::paint(ui.painter(), Icon::Plus, Rect::from_center_size(icon.center(), Vec2::splat(20.0)), fg, fill);
    if resp.has_focus() {
        ui.painter().rect_stroke(icon.expand(2.0), ICON_R + 2.0, Stroke::new(2.0, crate::theme::applied_tokens(ui.ctx()).focus), egui::StrokeKind::Outside);
    }
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, "Add a server"));
    side_tooltip(&resp, "Add a server", None);
    if resp.clicked() {
        server_menu::open(cx, super::Modal::JoinServer);
    }
}

/// Marks channels read here and queues their acks (the shell sends them, debounced, when read
/// receipts are on). Also ends a "Mark Unread" pause of automatic acks.
pub(super) fn mark_read(cx: &mut UiCx<'_>, channels: impl IntoIterator<Item = Id>) {
    for ch in channels {
        if cx.state.chat.ack_paused == Some(ch) {
            cx.state.chat.ack_paused = None;
        }
        if let Some(mid) = cx.state.mark_read(ch)
            && cx.settings.send_read_acks
        {
            cx.view.pending_acks.insert(ch, mid);
        }
    }
}

/// `user_menu` on right-click. A click on its VolumeBooster slider keeps it open, so it closes
/// on its own rows only (a submenu's rows keep theirs, as before).
pub(super) fn user_context_menu(resp: &egui::Response, cx: &mut UiCx<'_>, user: Id, guild_id: Option<Id>, at: (egui::Pos2, Align2)) {
    egui::Popup::context_menu(resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        user_menu(ui, cx, user, guild_id, at);
        let clicked = ui.ctx().data_mut(|d| d.remove_temp::<Option<egui::LayerId>>(super::messages::menu_click_id())).flatten();
        if clicked == Some(ui.layer_id()) {
            ui.close();
        }
    });
}

/// A user's menu (member list, voice channel): profile, mention in the open channel, their DM
/// if one exists, and the id. `at` (corner point, corner) places the profile card.
fn user_menu(ui: &mut egui::Ui, cx: &mut UiCx<'_>, user: Id, guild_id: Option<Id>, at: (egui::Pos2, Align2)) {
    menu_setup(ui);
    let text = cx.theme.colors.text.to_color32();
    if menu_item(ui, "Profile", MenuIcon::Glyph("👤"), text) {
        cx.view.open_profile_at(user, guild_id, at);
    }
    if let Some(ch) = cx.state.selected_channel.filter(|&ch| cx.state.can_send(ch))
        && menu_item(ui, "Mention", MenuIcon::Glyph("@"), text)
    {
        let draft = cx.view.drafts.entry(ch).or_default();
        if !draft.is_empty() && !draft.ends_with(char::is_whitespace) {
            draft.push(' ');
        }
        draft.push_str(&format!("<@{}> ", user.0));
    }
    // Message, Add Friend (friends.rs); Kick / Ban (server_menu.rs).
    super::friends::user_menu_items(ui, cx, user);
    if let Some(g) = guild_id {
        server_menu::member_menu_items(ui, cx, g, user);
    }
    crate::plugins::menus::user_items(ui, cx, user, guild_id, &mut |ui, label, glyph| menu_item(ui, label, MenuIcon::Glyph(glyph), text));
    // Settings > Advanced > Developer Mode.
    let dev = cx.state.account.settings.developer_mode();
    if dev {
        ui.separator();
    }
    if dev && menu_item(ui, "Copy user ID", MenuIcon::Glyph("🆔"), text) {
        ui.ctx().copy_text(user.0.to_string());
    }
}

/// A neutral rail tile (`surface`) that takes the `selected` fill as `morph` goes to 1 (hover and
/// selection, docs/DESIGN.md §10.1). The tint goes on twice: once over an already raised tile it
/// hardly shows, and on light themes it only sinks the tile to the rail's own colour.
fn tile_fill(ui: &egui::Ui, c: &ThemeColors, morph: f32) -> Color32 {
    let (surface, selected) = (c.surface.to_color32(), crate::theme::applied_tokens(ui.ctx()).selected);
    surface.lerp_to_gamma(surface.blend(selected).blend(selected), morph)
}

/// Server icon `index` in `rect`; without one, its initials in semibold `text` on a neutral tile
/// that lifts as `morph` goes to 1 (unavailable servers muted). An animated icon plays while
/// `animate`, showing the static frame while the animation loads.
pub(super) fn paint_guild_icon(ui: &mut egui::Ui, cx: &mut UiCx<'_>, index: usize, rect: Rect, radius: impl Into<CornerRadius> + Copy, morph: f32, animate: bool) {
    let g = &cx.state.guilds[index];
    let unavailable = g.unavailable;
    let text = initials(&g.name);
    let (static_url, anim_url) = (cdn::guild_icon_url(g, ICON_PX, false), cdn::guild_icon_url(g, ICON_PX, true).filter(|_| animate));
    let ctx = ui.ctx().clone();
    let anim_tex = anim_url.and_then(|u| cx.images.get(&ctx, &u, ICON_PX, true));
    let animated = anim_tex.is_some();
    let tex = anim_tex.or_else(|| static_url.as_ref().and_then(|u| cx.images.get(&ctx, u, ICON_PX, false)));
    // A static icon that arrived late fades in over the initials (an animated one replaces it at once).
    let a = match (&tex, &static_url) {
        (Some(_), Some(u)) if !animated => cx.images.fade(&ctx, u),
        (Some(_), _) => 1.0,
        _ => 0.0,
    };
    let c = &cx.theme.colors;
    if a < 1.0 {
        let (fill, fg) = if unavailable {
            (c.surface.to_color32(), c.text_muted.to_color32())
        } else {
            (tile_fill(ui, c, morph), c.text.to_color32())
        };
        ui.painter().rect_filled(rect, radius, fill);
        let size = rect.height() * if text.chars().count() > 2 { 0.33 } else { 0.4 };
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, super::semibold(size), fg);
    }
    if let Some(tex) = tex {
        let tint = if unavailable { Color32::from_gray(110) } else { Color32::WHITE };
        egui::Image::from_texture(tex).corner_radius(radius).tint(tint.gamma_multiply(a)).paint_at(ui, rect);
    }
}

/// Title bar: the current location (server, DM or home) centred as a 16 px icon and its name;
/// the inbox button on the right (FEATURES_NOTES §4.5), except under a settings layer.
/// The island's connect ring: peak-end, once.
const RING: f32 = 0.48;

/// RUSTCORD_NAV_DEMO=join:<voice channel id>[,leave]: I join that channel 2 s after start, am
/// connected at 3 s and muted at 4 s; with `leave` I leave at 6 s (local state only, nothing sent).
fn demo_call(ctx: &egui::Context, cx: &mut UiCx<'_>) {
    let Some(id) = nav_demo_value("join").and_then(|v| v.parse::<u64>().ok()).map(Id) else { return };
    let (t, leave) = (ctx.input(|i| i.time), nav_demo("leave"));
    if let Some(next) = [2.0, 3.0, 4.0, 6.0].into_iter().find(|s| *s > t) {
        ctx.request_repaint_after(Duration::from_secs_f64(next - t));
    }
    if t >= 2.0 && !(leave && t >= 6.0) {
        cx.state.voice.channel_id = Some(id);
        cx.state.voice.connected = t >= 3.0;
        cx.view.self_mute |= t >= 4.0;
    } else if leave && t >= 6.0 {
        cx.state.voice.channel_id = None;
        cx.state.voice.connected = false;
    }
}

pub fn title_bar(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    demo_call(ui.ctx(), cx);
    if nav_demo("icons") {
        icons::sheet(ui.ctx(), cx.theme.colors.text_muted.to_color32(), cx.theme.colors.background.to_color32());
    }
    let rect = ui.max_rect();
    if !cx.view.chat_hidden() {
        let pal = super::messages::Pal::new(cx.theme);
        let slot = Rect::from_center_size(pos2(rect.right() - 8.0 - 14.0, rect.center().y), Vec2::splat(28.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(slot), |ui| super::inbox::button(ui, cx, &pal));
    }
    // The island takes the place of the label while I am in a call (cross-fade, §8).
    let island = call_island(ui, cx, rect);
    // The label is gone by mid-way (exits faster than entries): the words never overlap.
    if island >= 0.5 {
        return;
    }
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        if island > 0.0 {
            ui.multiply_opacity((1.0 - 2.0 * island).max(0.0));
        }
        location(ui, cx, rect);
    });
}

/// The current location (server, DM or home), centred in `rect`: a 16 px icon and its name.
fn location(ui: &mut egui::Ui, cx: &mut UiCx<'_>, rect: Rect) {
    let c = cx.theme.colors.clone();
    let muted = c.text_muted.to_color32();
    let guild = cx.state.selected_guild.and_then(|g| cx.state.guilds.iter().position(|x| x.id == g));
    let dm = cx.state.selected_channel.filter(|_| guild.is_none());
    let name = match (guild, dm) {
        (Some(i), _) => cx.state.guilds[i].name.clone(),
        (None, Some(ch)) => cx.state.channel_name(ch),
        (None, None) => "Direct messages".to_owned(),
    };
    // The ui role, 600 (docs/DESIGN.md §10.6).
    let g = one_line(ui, name, super::semibold(cx.theme.font.size - 1.0), muted, rect.width() - 64.0);
    let icon = Rect::from_min_size(pos2(rect.center().x - (24.0 + g.size().x) / 2.0, rect.center().y - 8.0), Vec2::splat(16.0));
    match (guild, dm) {
        (Some(i), _) => paint_guild_icon(ui, cx, i, icon, 4.0, 0.0, false),
        (None, Some(ch)) => {
            dm_avatar_at(ui, cx, ch, icon);
        }
        (None, None) => logo_mark(ui.painter(), icon, SMALL_MARK, c.accent.to_color32(), c.accent_text.to_color32()),
    }
    // The island grows out of this label (`call_island`).
    ui.data_mut(|d| d.insert_temp(egui::Id::unique("rc_location_w"), 24.0 + g.size().x));
    ui.painter().galley(pos2(icon.right() + 8.0, rect.center().y - g.size().y / 2.0), g, muted);
}

/// What the island shows, kept while it fades out after the call ended.
#[derive(Clone)]
struct IslandInfo {
    connected: bool,
    muted: bool,
    channel: Id,
    place: String,
    server: Option<String>,
}

/// The island's width, gliding over BASE only when what it shows changed within the last BASE
/// (state word, channel, whether the server name fits); a CallTimer tick or a window resize
/// changes it at once.
fn island_width(ctx: &egui::Context, key: egui::Id, look: (bool, Id, bool), width: f32) -> f32 {
    let now = ctx.input(|i| i.time);
    let since = ctx.data_mut(|d| {
        let e = d.get_temp_mut_or(key.with("look"), (look, now));
        if e.0 != look {
            *e = (look, now);
        }
        e.1
    });
    if now - since > f64::from(super::motion::secs(ctx, super::motion::BASE)) {
        super::motion::forget(ctx, key.with("w"));
    }
    super::motion::value(ctx, key.with("w"), width, super::motion::BASE)
}

/// Call island (docs/DESIGN.md §8): a pill centred in the title bar while I am in a voice or DM
/// call, over the location label: state, channel, server, mute and disconnect. Returns its
/// opacity (0 when it is not shown), which the label fades against.
fn call_island(ui: &mut egui::Ui, cx: &mut UiCx<'_>, row: Rect) -> f32 {
    let ctx = ui.ctx().clone();
    let key = egui::Id::unique("rc_call_island");
    let in_call = cx.state.voice.channel_id;
    let t = super::motion::anim(&ctx, key, in_call.is_some(), super::motion::BASE, super::motion::BASE * super::motion::OUT);
    let info = match in_call {
        Some(ch) => {
            let guild = cx.state.channels.get(&ch).and_then(|x| x.guild_id).and_then(|g| cx.state.guild(g)).map(|g| g.name.clone());
            let info = IslandInfo { connected: cx.state.voice.connected, muted: cx.view.self_mute || cx.view.self_deaf, channel: ch, place: cx.state.channel_name(ch), server: guild };
            ctx.data_mut(|d| d.insert_temp(key, info.clone()));
            info
        }
        None => match ctx.data(|d| d.get_temp::<IslandInfo>(key)).filter(|_| t > 0.0) {
            Some(info) => info,
            None => {
                ctx.data_mut(|d| d.remove::<IslandInfo>(key));
                return 0.0;
            }
        },
    };
    let max_w = 420f32.min(row.width() - 240.0);
    if max_w < 160.0 {
        return 0.0;
    }
    let tk = cx.theme.tokens();
    let c = &cx.theme.colors;
    let (text, muted, danger) = (c.text.to_color32(), c.text_muted.to_color32(), c.danger.to_color32());
    let live = in_call.is_some();
    let (word, word_color) = if info.connected { ("Voice connected", tk.positive_text) } else { ("Connecting…", crate::theme::readable_on(tk.attention, tk.raised)) };
    let word_text = crate::plugins::sidebar::call_timer(&ctx, cx.state, word);
    let word_g = one_line(ui, word_text, super::semibold(13.0), word_color, f32::INFINITY);
    let chan_full = one_line(ui, info.place.clone(), super::semibold(14.0), text, f32::INFINITY);
    let server_full = info.server.as_ref().map(|s| one_line(ui, s.clone(), FontId::proportional(12.0), muted, f32::INFINITY));
    // 12 pad, dot, 8, word, 8, channel [6, server], 10, two 24 px buttons 4 apart, 2 pad.
    let fixed = 12.0 + 8.0 + 8.0 + word_g.size().x + 8.0 + 10.0 + 52.0 + 2.0;
    let room = max_w - fixed;
    let server_w = server_full.as_ref().map_or(0.0, |g| 6.0 + g.size().x);
    let show_server = chan_full.size().x + server_w <= room;
    let chan_w = chan_full.size().x.min(room.max(0.0));
    let width = island_width(&ctx, key, (info.connected, info.channel, show_server), (fixed + chan_w + if show_server { server_w } else { 0.0 }).max(160.0));

    // The island grows out of the location label (§7): `full` is where its parts sit; `pill` is how
    // much of it shows now, from the label's width + 24 to `width`.
    let full = Rect::from_center_size(row.center(), vec2(width, 28.0));
    let label_w = ctx.data(|d| d.get_temp::<f32>(egui::Id::unique("rc_location_w"))).unwrap_or(160.0);
    let pill = Rect::from_center_size(row.center(), vec2(egui::lerp((label_w + 24.0)..=width, t), 28.0));
    let to_gap = full.right() - 2.0 - 52.0;
    let unmute = Rect::from_min_size(pos2(to_gap, full.center().y - 12.0), Vec2::splat(24.0));
    let hang = unmute.translate(vec2(28.0, 0.0));
    let place = Rect::from_min_max(full.min, pos2(unmute.left() - 4.0, full.max.y));
    // Connected: the dot blends to green over FAST and pops once (docs/DESIGN.md §7); the first sight just shows it.
    let dot = super::motion::anim(&ctx, key.with("dot"), info.connected, super::motion::FAST, super::motion::FAST);
    let was = ctx.data(|d| d.get_temp::<bool>(key.with("was")));
    ctx.data_mut(|d| d.insert_temp(key.with("was"), info.connected));
    if was == Some(false) && info.connected {
        super::motion::pop(&ctx, key.with("pop"));
        super::motion::pop(&ctx, key.with("ring"));
    }
    let pop = super::motion::popped(&ctx, key.with("pop"));
    let ring = super::motion::shot(&ctx, key.with("ring"), RING);
    let pal = super::messages::Pal::new(cx.theme);
    let mut action = None;
    // The chrome fades in with the growth, the content from 40 % of it.
    ui.scope(|ui| {
        ui.multiply_opacity(t);
        let p = ui.painter().clone();
        let stroke = if tk.mode == crate::theme::Mode::HighContrast { Stroke::new(1.5, muted) } else { Stroke::new(1.0, tk.rule) };
        p.add(super::messages::contact(pill, CornerRadius::same(14), ctx.global_style().visuals.popup_shadow.color));
        p.rect_filled(pill, 14, tk.raised);
        p.rect_stroke(pill, 14, stroke, egui::StrokeKind::Inside);
        super::messages::sheen(&p, pill, 14.0, &tk);
    });
    ui.scope(|ui| {
        ui.multiply_opacity(((t - 0.4) / 0.6).clamp(0.0, 1.0));
        let p = ui.painter().with_clip_rect(pill.intersect(ui.painter().clip_rect()));

        // The text part is one button: select the call's channel.
        let go = ui.interact(place, ui.make_persistent_id("call_island_place"), Sense::click());
        if go.hovered() && live {
            p.rect_filled(place.intersect(pill), CornerRadius { nw: 14, sw: 14, ne: 4, se: 4 }, tk.hover);
        }
        let mid = full.center().y;
        // The text is laid out for the final width: cut it at the buttons while the pill catches up.
        let q = p.with_clip_rect(place.intersect(p.clip_rect()));
        let dot_at = pos2(full.left() + 12.0 + 4.0, mid);
        if let Some(lin) = ring {
            // One ring from the dot as it turns connected: r 4 -> 12, 70 % -> 0.
            q.circle_stroke(dot_at, egui::lerp(4.0..=12.0, super::motion::ease(lin)), Stroke::new(1.5, tk.positive.gamma_multiply(0.7 * (1.0 - lin))));
        }
        q.circle_filled(dot_at, 4.0 * pop, tk.attention.lerp_to_gamma(tk.positive, dot));
        let mut x = full.left() + 12.0 + 8.0 + 8.0;
        q.galley(pos2(x, mid - word_g.size().y / 2.0), word_g.clone(), word_color);
        x += word_g.size().x + 8.0;
        let chan = if chan_w < chan_full.size().x { one_line(ui, info.place.clone(), super::semibold(14.0), text, chan_w) } else { chan_full.clone() };
        q.galley(pos2(x, mid - chan.size().y / 2.0), chan.clone(), text);
        if let (true, Some(g)) = (show_server, &server_full) {
            q.galley(pos2(x + chan.size().x + 6.0, mid - g.size().y / 2.0), g.clone(), muted);
        }
        if live {
            let name = match &info.server {
                Some(s) => format!("{}, {}, {s}", word, info.place),
                None => format!("{}, {}", word, info.place),
            };
            go.widget_info(|| WidgetInfo::labeled(Role::Button, true, &name));
            if go.has_focus() {
                super::messages::focus_ring(ui, &pal, place, 14.0);
            }
            if go.on_hover_text(format!("Open {}", info.place)).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                action = Some(false);
            }
        }

        // Mute and disconnect: 24 px round buttons, 16 px icons.
        let mic = ui.interact(unmute, ui.make_persistent_id("call_island_mute"), Sense::click());
        let r = super::motion::pressed(&mic, unmute);
        // Muting draws the slash from its top end while the danger tint comes in (§7).
        let slash = super::motion::anim(&ctx, key.with("slash"), info.muted, super::motion::FAST, super::motion::FAST * super::motion::OUT);
        let fill = if slash > 0.0 { danger.gamma_multiply(0.12 * slash) } else if mic.hovered() { tk.hover } else { Color32::TRANSPARENT };
        p.circle_filled(r.center(), r.width() / 2.0, fill);
        icons::paint_slashed(&p, Icon::Mic, Rect::from_center_size(r.center(), Vec2::splat(16.0)), text.lerp_to_gamma(danger, slash), tk.raised.blend(fill), slash);
        let tip = if info.muted { "Unmute" } else { "Mute" };
        mic.widget_info(|| WidgetInfo::labeled(Role::Button, true, tip));
        if mic.has_focus() {
            super::messages::focus_ring(ui, &pal, unmute, 12.0);
        }
        if live && mic.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            action = Some(true);
        }

        let end = ui.interact(hang, ui.make_persistent_id("call_island_end"), Sense::click());
        let r = super::motion::pressed(&end, hang);
        let hot = end.hovered() || end.has_focus();
        let (fill, fg) = if hot { (tk.danger_fill, tk.danger_label) } else { (Color32::TRANSPARENT, danger) };
        p.circle_filled(r.center(), r.width() / 2.0, fill);
        icons::paint(&p, Icon::Hangup, Rect::from_center_size(r.center(), Vec2::splat(16.0)), fg, tk.raised.blend(fill));
        end.widget_info(|| WidgetInfo::labeled(Role::Button, true, "Disconnect"));
        if end.has_focus() {
            super::messages::focus_ring(ui, &pal, hang, 12.0);
        }
        if live && end.on_hover_text("Disconnect").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            cx.cmd.send(Command::LeaveVoice);
        }
    });
    match action {
        Some(false) => {
            cx.state.select_channel(info.channel);
            cx.view.settings_open = false;
        }
        Some(true) => super::call::toggle_mute(cx, true),
        None => {}
    }
    t
}

/// Channel-list card: the sidebar colour with a 12 px top-left corner over the server-bar
/// colour, and a hairline along its top and left edges that follows the corner.
pub fn paint_card(p: &egui::Painter, rect: Rect, c: &ThemeColors) {
    const R: f32 = 12.0;
    p.rect_filled(rect, CornerRadius { nw: R as u8, ..Default::default() }, c.sidebar.to_color32());
    let corner = rect.left_top() + Vec2::splat(R);
    let mut pts = vec![pos2(rect.left() + 0.5, rect.bottom())];
    pts.extend((0..=8).map(|i| corner + (R - 0.5) * Vec2::angled(PI + FRAC_PI_2 * i as f32 / 8.0)));
    pts.push(pos2(rect.right(), rect.top() + 0.5));
    p.add(Shape::line(pts, line(c)));
}

/// The tile corners of the 16 px mark (title bars): 4, and the speech corner 1.
pub(super) const SMALL_MARK: CornerRadius = CornerRadius { nw: 4, ne: 4, se: 4, sw: 1 };

/// Rustcord's mark (assets/logo.svg, §11): `tile` with `corners`, the R in `glyph`, its counter
/// cut in `tile`. Below 32 px the bolder small R (logo-small.svg).
pub(super) fn logo_mark(p: &egui::Painter, rect: Rect, corners: CornerRadius, tile: Color32, glyph: Color32) {
    let k = rect.width() / 512.0;
    let at = |x: f32, y: f32| rect.min + vec2(x, y) * k;
    let area = |(x0, y0, x1, y1): (f32, f32, f32, f32)| Rect::from_min_max(at(x0, y0), at(x1, y1));
    let r = |v: f32| (v * k).round() as u8;
    // Per size: bowl, stem and counter rects, the bowl's and the counter's round-off, the leg.
    let (bowl, stem, counter, (outer, hole), leg) = if rect.width() < 32.0 {
        ((124.0, 104.0, 372.0, 296.0), (124.0, 104.0, 204.0, 408.0), (204.0, 184.0, 292.0, 216.0), (96.0, 16.0), [(187.71, 260.0), (283.71, 260.0), (400.0, 408.0), (304.0, 408.0)])
    } else {
        ((136.0, 112.0, 368.0, 292.0), (136.0, 112.0, 204.0, 400.0), (204.0, 180.0, 300.0, 224.0), (90.0, 22.0), [(200.67, 256.0), (284.67, 256.0), (394.0, 400.0), (310.0, 400.0)])
    };
    p.rect_filled(rect, corners, tile);
    p.rect_filled(area(bowl), CornerRadius { nw: r(48.0), ne: r(outer), se: r(outer), sw: 0 }, glyph);
    p.rect_filled(area(stem), CornerRadius { nw: r(48.0), ..Default::default() }, glyph);
    p.add(Shape::convex_polygon(leg.iter().map(|&(x, y)| at(x, y)).collect(), glyph, Stroke::NONE));
    p.rect_filled(area(counter), CornerRadius { ne: r(hole), se: r(hole), ..Default::default() }, tile);
}

/// Up to three initials for an icon placeholder ("Rust Workshop" -> "RW").
fn initials(name: &str) -> String {
    let s: String = name.split_whitespace().filter_map(|w| w.chars().next()).take(3).collect();
    if s.is_empty() { "?".into() } else { s }
}

/// Count badge (`danger_fill` pill, badge role 11/700) whose right edge is centred vertically on
/// `right_center`. `ring` draws a 4 px outline in the background color so the badge stands out
/// on top of icons. Returns the badge width.
pub(super) fn badge(ui: &egui::Ui, c: &ThemeColors, right_center: egui::Pos2, count: u32, ring: Option<Color32>) -> f32 {
    badge_with(ui, c, right_center, count, ring, None)
}

/// `badge` with its digits rolling from the count `roll.0` at eased `roll.1` (0 -> 1): the old number
/// leaves 6 px up, the new one rises 6 px in, both cut to the pill; the box has the new width at once.
fn badge_with(ui: &egui::Ui, c: &ThemeColors, right_center: egui::Pos2, count: u32, ring: Option<Color32>, roll: Option<(u32, f32)>) -> f32 {
    let tk = crate::theme::Tokens::new(c);
    let text = |n: u32| if n > 99 { "99+".to_owned() } else { n.to_string() };
    let galley = ui.painter().layout_no_wrap(text(count), bold(11.0), tk.danger_label);
    let h = 16.0;
    let w = (galley.size().x + 8.0).max(h);
    let rect = Rect::from_min_max(pos2(right_center.x - w, right_center.y - h / 2.0), pos2(right_center.x, right_center.y + h / 2.0));
    if let Some(ring) = ring {
        ui.painter().rect_filled(rect.expand(4.0), CornerRadius::same(12), ring);
    }
    ui.painter().rect_filled(rect, CornerRadius::same(8), tk.danger_fill);
    match roll {
        Some((from, e)) => {
            let p = ui.painter().with_clip_rect(rect.intersect(ui.painter().clip_rect()));
            let old = ui.painter().layout_no_wrap(text(from), bold(11.0), tk.danger_label);
            p.galley_with_override_text_color(rect.center() - old.size() / 2.0 - vec2(0.0, 6.0 * e), old, tk.danger_label.gamma_multiply(1.0 - e));
            p.galley_with_override_text_color(rect.center() - galley.size() / 2.0 + vec2(0.0, 6.0 * (1.0 - e)), galley, tk.danger_label.gamma_multiply(e));
        }
        None => ui.painter().galley(rect.center() - galley.size() / 2.0, galley, tk.danger_label),
    }
    w
}

/// Hairline between panel areas (the `rule` token; the message view's header border too).
pub(super) fn line(c: &ThemeColors) -> Stroke {
    Stroke::new(1.0, crate::theme::Tokens::new(c).rule)
}

/// One muted meta-size line (empty states), centred near the top of the area.
pub(super) fn empty_state(ui: &mut egui::Ui, cx: &UiCx<'_>, top: f32, text: &str) -> egui::Response {
    ui.add_space(top);
    let size = cx.theme.font.size - 2.0;
    // The muted text of the visuals: `text_muted`, or a themed profile's (cosmetics::theme_visuals).
    ui.vertical_centered(|ui| ui.label(egui::RichText::new(text).size(size).color(ui.visuals().weak_text_color())))
        .inner
}

/// `super::avatar_at` with the colored-initial placeholder for users (`who` = id, name).
pub(super) fn user_avatar_at(ui: &mut egui::Ui, images: &mut ImageCache, url: &str, rect: Rect, animate: bool, opacity: f32, who: (Id, &str)) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    child.multiply_opacity(opacity);
    super::images::avatar(&mut child, images, url, rect.width(), animate, Some(who));
}

/// (radius, ring, inset) of an avatar's status dot: 10 px on 32 px, 8 px on 24 px, cut out by a
/// 3 px ring; 16 px with a 6 px ring, 4 px in, on the 80 px profile avatar.
fn dot_metrics(avatar: Rect) -> (f32, f32, f32) {
    if avatar.width() >= 80.0 { (8.0, 6.0, 4.0) } else { (avatar.width() / 6.4, 3.0, 0.0) }
}

/// Status icon on the bottom-right of an avatar, cut out of it by a ring in the background
/// color `ring`. Returns the icon's rect (status tooltips).
pub(super) fn status_dot(p: &egui::Painter, avatar: Rect, presence: Presence, c: &ThemeColors, ring: Color32) -> Rect {
    let (center, r) = dot_ring(p, avatar, ring);
    paint_status(p, center, r, presence, c, ring);
    Rect::from_center_size(center, Vec2::splat(2.0 * r))
}

/// The ring that cuts an avatar's status icon out of it; returns the icon's centre and radius.
fn dot_ring(p: &egui::Painter, avatar: Rect, ring: Color32) -> (egui::Pos2, f32) {
    let (r, cut, inset) = dot_metrics(avatar);
    let center = avatar.right_bottom() - Vec2::splat(r + inset);
    p.circle_filled(center, r + cut, ring);
    (center, r)
}

/// Status icon of radius `r` with its cut-outs in `bg`: online a dot, idle a crescent, do not
/// disturb a bar, offline (and invisible) a ring.
pub(super) fn paint_status(p: &egui::Painter, center: egui::Pos2, r: f32, presence: Presence, c: &ThemeColors, bg: Color32) {
    p.circle_filled(center, r, presence_color(presence, c));
    match presence {
        Presence::Online => {}
        Presence::Idle => {
            p.circle_filled(center - Vec2::splat(r / 2.0), r * 0.75, bg);
        }
        Presence::Dnd => {
            p.rect_filled(Rect::from_center_size(center, vec2(r * 1.5, r * 0.5)), r * 0.25, bg);
        }
        Presence::Offline => {
            p.circle_filled(center, r / 2.0, bg);
        }
    }
}

/// `status_dot` of a user (mine: my status); while they stream (`is_streaming`) a `live` dot with
/// a play triangle cut out, else a phone while they are online only on mobile. Returns the
/// icon's rect.
pub(super) fn user_dot(p: &egui::Painter, avatar: Rect, state: &AppState, user: Id, c: &ThemeColors, ring: Color32) -> Rect {
    if is_streaming(state, user) {
        let (center, r) = dot_ring(p, avatar, ring);
        p.circle_filled(center, r, crate::theme::Tokens::new(c).live);
        // Centred on its centroid, like a play button.
        let tri = [vec2(-0.4, -0.55), vec2(0.6, 0.0), vec2(-0.4, 0.55)].map(|v| center + v * r);
        p.add(Shape::convex_polygon(tri.to_vec(), ring, Stroke::NONE));
        return Rect::from_center_size(center, Vec2::splat(2.0 * r));
    }
    let presence = user_presence(state, user);
    if presence != Presence::Online || !state.presence_of(user).is_some_and(|x| x.mobile) {
        return status_dot(p, avatar, presence, c, ring);
    }
    let (r, cut, inset) = dot_metrics(avatar);
    let size = vec2(2.0 * r, 3.0 * r);
    let phone = Rect::from_min_size(avatar.right_bottom() - size - Vec2::splat(inset), size);
    p.rect_filled(phone.expand(cut), r * 0.4 + cut, ring);
    p.rect_filled(phone, r * 0.4, presence_color(presence, c));
    // The screen; the strip left below it is the phone's chin.
    p.rect_filled(Rect::from_min_max(phone.min + vec2(r * 0.4, r * 0.4), phone.max - vec2(r * 0.4, r * 0.8)), 0.0, ring);
    phone
}

/// A user's status: mine from my chosen status and the connection, anyone else's from the
/// gateway's presences (offline when unknown).
pub(super) fn user_presence(state: &AppState, user: Id) -> Presence {
    if state.me.as_ref().is_some_and(|m| m.id == user) { my_presence(state) } else { state.presence(user) }
}

/// "Online", "Idle", "Do not disturb", "Offline".
pub(super) fn presence_label(p: Presence) -> &'static str {
    match p {
        Presence::Online => "Online",
        Presence::Idle => "Idle",
        Presence::Dnd => "Do not disturb",
        Presence::Offline => "Offline",
    }
}

/// Whether a user is live on Twitch / YouTube (their shown activity is a stream, type 1) and not
/// offline: their dot turns `live` and their status word is "Streaming", as in Discord.
pub(super) fn is_streaming(state: &AppState, user: Id) -> bool {
    user_presence(state, user) != Presence::Offline && crate::activity::activity_of(state, user).is_some_and(|a| a.kind == 1)
}

/// `presence_label` of a user's status, or "Streaming" while they stream (status tooltips).
pub(super) fn user_presence_label(state: &AppState, user: Id) -> &'static str {
    if is_streaming(state, user) { "Streaming" } else { presence_label(user_presence(state, user)) }
}

/// A status I can pick, as the status menu names it.
pub(super) fn status_label(s: UserStatus) -> &'static str {
    if s == UserStatus::Invisible { "Invisible" } else { presence_label(s.presence()) }
}

/// A status line: what leads it and text (a unicode emoji leads it).
pub(super) type StatusText = (Option<Lead>, String);

/// The image leading a status line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Lead {
    /// A custom emoji, drawn as an image.
    Emoji(Id),
    /// An activity (its type): its icon, and the text after the verb is bold.
    Activity(u8),
}

/// What shows under a name: the custom status, else the activity ("Playing X").
pub(super) fn status_text(custom: Option<&CustomStatus>, activity: Option<&Activity>) -> Option<StatusText> {
    if let Some(cs) = custom {
        let emoji = cs.emoji.as_ref();
        let image = emoji.and_then(|e| e.id);
        let unicode = emoji.filter(|e| e.id.is_none()).and_then(|e| e.name.as_deref());
        let text = [unicode, cs.text.as_deref()].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
        if image.is_some() || !text.is_empty() {
            return Some((image.map(Lead::Emoji), text));
        }
    }
    activity.map(|a| (Some(Lead::Activity(a.kind)), a.label()))
}

/// The whole status line with `:shortcode:`s for the emoji `paint_status_text` leaves out, when
/// it left one out (the tooltip of the line's row).
pub(super) fn status_tooltip(ui: &egui::Ui, (_, text): &StatusText) -> Option<String> {
    super::messages::missing_emoji(ui, text)?;
    let mut text = text.clone();
    while let Some((range, code)) = super::messages::missing_emoji(ui, &text) {
        text.replace_range(range, &format!(":{code}:"));
    }
    Some(text)
}

/// `status_text` of a user; mine uses my own custom status and game. None while offline.
pub(super) fn user_status_text(state: &AppState, user: Id) -> Option<StatusText> {
    let activity = crate::activity::activity_of(state, user);
    if state.me.as_ref().is_some_and(|m| m.id == user) {
        return status_text(state.chat.my_custom_status.as_ref(), activity.as_deref());
    }
    status_text(state.presence_of(user)?.custom_status.as_ref(), activity.as_deref())
}

/// Paints a status line in `format` with its top-left at `pos`, wrapped to `max_rows` (cut with
/// "…") at `max_w`. Its emoji (a custom one as a static image, or the unicode one leading it) is
/// `emoji_side` px (Discord: 16 in rows, 20 in the profile bubble): at text size the monochrome
/// glyphs blur into dots. It is centred on the first text row and overhangs it evenly. A unicode
/// emoji the fonts can't draw (e.g. 🦀) is left out of these short lines (`status_tooltip` names
/// it). Returns the painted rect.
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_status_text(
    ui: &egui::Ui,
    images: &mut ImageCache,
    pos: egui::Pos2,
    (emoji, text): &StatusText,
    format: egui::TextFormat,
    emoji_side: f32,
    max_w: f32,
    max_rows: usize,
) -> Rect {
    let color = format.color;
    let row_h = ui.fonts_mut(|f| f.row_height(&format.font_id));
    // Activity icons at text size: they are line art, not stand-ins for coloured emoji.
    let side = if matches!(emoji, Some(Lead::Activity(_))) { (row_h * 0.85).round() } else { emoji_side };
    let lead = if emoji.is_some() { side + 4.0 } else { 0.0 };
    let mut text = text.clone();
    while let Some((range, _)) = super::messages::missing_emoji(ui, &text) {
        let end = if text[range.end..].starts_with(' ') { range.end + 1 } else { range.end };
        text.replace_range(range.start..end, "");
    }
    let format = egui::TextFormat { valign: egui::Align::Center, ..format };
    let first = text.split(' ').next().unwrap_or_default();
    let uni = if emojis::get(first).is_some() { first.len() } else { 0 };
    let mut job = egui::text::LayoutJob { break_on_newline: false, ..Default::default() };
    if uni > 0 {
        // Body text color, not the muted line color: the emoji stands in for a coloured one.
        let color = ui.visuals().text_color();
        job.append(&text[..uni], lead, egui::TextFormat { font_id: FontId::proportional(emoji_side), color, ..format.clone() });
    }
    let (body, x) = (&text[uni..], if uni == 0 { lead } else { 0.0 });
    match emoji {
        Some(Lead::Activity(kind)) if let Some(name) = body.strip_prefix(Activity::verb(*kind)) => {
            job.append(Activity::verb(*kind), x, format.clone());
            let font_id = FontId::new(format.font_id.size, crate::theme::bold_family());
            job.append(name, 0.0, egui::TextFormat { font_id, ..format });
        }
        _ => job.append(body, x, format),
    }
    job.wrap = egui::text::TextWrapping { max_width: max_w.max(lead + 1.0), max_rows, break_anywhere: max_rows == 1, overflow_character: Some('…') };
    let g = ui.painter().layout_job(job);
    // The first row's centre stays where a plain text row's would be.
    let first_h = g.rows.first().map_or(row_h, |r| r.rect().height());
    let at = pos - vec2(0.0, (first_h - row_h) / 2.0);
    let mut painted = Rect::from_min_size(at, g.size());
    if let Some(lead) = emoji {
        let icon = Rect::from_min_size(at + vec2(0.0, (first_h - side) / 2.0), Vec2::splat(side));
        match lead {
            Lead::Emoji(id) => {
                let px = (side * ui.ctx().pixels_per_point()).ceil() as u32;
                if let Some(tex) = images.get(ui.ctx(), &cdn::emoji_url(*id, false, px), px, false) {
                    egui::Image::from_texture(tex).paint_at(ui, icon);
                }
            }
            Lead::Activity(kind) => icons::paint(ui.painter(), icons::activity(*kind), icon, color, Color32::TRANSPARENT),
        }
        painted = painted.union(icon);
    }
    ui.painter().galley(at, g, color);
    painted
}

// ---------------------------------------------------------------------------
// Channel / DM list
// ---------------------------------------------------------------------------

/// Channel list of the selected guild, or the DM list when no guild is selected, on the card
/// `paint_card` draws. Its bottom is left free for the floating user panel.
pub fn channel_list(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    let edge = line(&cx.theme.colors);
    let guild = cx.state.selected_guild;
    let title = guild.map(|g| cx.state.guild(g).map(|g| g.name.clone()).unwrap_or_default());

    // The server's aura from its icon: a band under its name that fades out over 150 px, in the
    // card's 12 px corner (`paint_card`).
    let card_top = ui.max_rect().top();
    let mut aura = None;
    if let Some(a) = guild.and_then(|g| super::aura::of_guild(ui.ctx(), cx, g)) {
        let band = ui.max_rect().with_max_y(ui.max_rect().top() + 150.0);
        let alpha = super::aura::alpha(cx.settings.people_colors, cx.theme.tokens().mode);
        aura = Some(a.gamma_multiply(alpha));
        ui.painter().add(super::aura::ramp(ui.ctx(), band, CornerRadius { nw: 12, ..Default::default() }, a.gamma_multiply(alpha), false));
    }
    // No fill: the card's rounded corner shows through.
    egui::Panel::top("channel_header")
        .exact_size(HEADER_H)
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::new())
        .show(ui, |ui| {
            let rect = ui.max_rect();
            match guild.zip(title) {
                Some((gid, title)) => server_menu::server_header(ui, cx, gid, title, rect),
                None => dm_search(ui, cx, rect),
            }
            ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, edge);
        });
    user_panel_space(ui, cx, "channel_list_panel_space");

    ui.spacing_mut().item_spacing.y = 2.0;
    let flat = super::members::flat_fades(ui, cx);
    let mut area = egui::ScrollArea::vertical()
        .id_salt(("channels", guild))
        .auto_shrink(false)
        .content_margin(egui::Margin { left: 0, right: 0, top: 12, bottom: 12 });
    if let Some(px) = nav_demo_value("scroll").and_then(|v| v.parse::<f32>().ok()) {
        area = area.vertical_scroll_offset(px);
    }
    let moved = super::arrival(ui.ctx(), cx.state, cx.view).guild;
    let out = super::motion::arriving(ui, moved, |ui| match guild {
        Some(gid) => Some(area.show(ui, |ui| guild_channels(ui, cx, gid))),
        None => dm_list(ui, cx, area.content_margin(egui::Margin { left: 0, right: 0, top: 0, bottom: 12 })),
    });
    // The list fades into the card at its scrolled edges; the top one takes the server's band along.
    if let (true, Some(out)) = (flat, out) {
        let side = cx.theme.colors.sidebar.to_color32();
        let top = band_under(side, aura, out.inner_rect.top() + 10.0 - card_top);
        super::scroll_fades(ui, &out, out.inner_rect, (out.inner_rect.left(), out.inner_rect.right()), (top, side));
    }
}

/// The channel card's colour `from_top` px under its top: `sidebar` with the server's band (its
/// colour at full strength) blended in as it fades out over 150 px (`channel_list`).
fn band_under(sidebar: Color32, band: Option<Color32>, from_top: f32) -> Color32 {
    band.map_or(sidebar, |b| sidebar.blend(b.gamma_multiply((1.0 - from_top / 150.0).clamp(0.0, 1.0))))
}

/// DM home header: a field that filters the DM list by name.
fn dm_search(ui: &mut egui::Ui, cx: &mut UiCx<'_>, rect: Rect) {
    let c = &cx.theme.colors;
    let id = ui.make_persistent_id("dm_search");
    let rest = if c.guild_bar == c.sidebar { line(c) } else { Stroke::NONE };
    // Focused: the 2 px focus ring replaces the border (§9); the margin gives up the extra width so
    // the text stays put.
    let (stroke, give) = if ui.memory(|m| m.has_focus(id)) { (Stroke::new(2.0, cx.theme.tokens().focus), 2 - rest.width as i8) } else { (rest, 0) };
    let frame = egui::Frame::new()
        .fill(c.guild_bar.to_color32())
        .corner_radius(cx.theme.layout.corner_radius)
        .inner_margin(egui::Margin::symmetric(8 - give, 5 - give))
        .stroke(stroke);
    let edit = egui::TextEdit::singleline(&mut cx.view.dm_filter)
        .id(id)
        .hint_text("Find a conversation")
        .font(FontId::proportional(cx.theme.font.size - 1.0))
        .frame(frame)
        .desired_width(f32::INFINITY);
    ui.put(Rect::from_center_size(rect.center(), vec2(rect.width() - 20.0, 28.0)), edit);
}

/// RUSTCORD_NAV_DEMO=long-list: once, 40 local text channels "demo-1" .. "demo-40" under the open
/// server's first category (`state.channels` only; nothing is sent).
fn demo_long_list(state: &mut AppState, guild_id: Id) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    if !nav_demo("long-list") {
        return;
    }
    ONCE.call_once(|| {
        let Some(SidebarItem::Category { id: parent, .. }) = state.sidebar_channels(guild_id).into_iter().find(|i| matches!(i, SidebarItem::Category { .. })) else { return };
        for n in 1..=40u64 {
            let id = Id(9_000_000_000_000_000 + n);
            let channel = crate::model::Channel { id, kind: crate::model::channel_type::GUILD_TEXT, guild_id: Some(guild_id), name: Some(format!("demo-{n}")), parent_id: Some(parent), position: 100 + n as i32, ..Default::default() };
            state.channels.insert(id, channel);
        }
    });
}

/// RUSTCORD_NAV_DEMO=unread: once, 3 s after start, the open server's last text channel (not the open
/// one) turns unread: its last message is one past its read state (local state only).
fn demo_unread(ctx: &egui::Context, state: &mut AppState, guild_id: Id) {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !nav_demo("unread") || DONE.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let left = 3.0 - ctx.input(|i| i.time);
    if left > 0.0 {
        ctx.request_repaint_after(Duration::from_secs_f64(left));
        return;
    }
    let last = state.sidebar_channels(guild_id).into_iter().rev().find_map(|i| match i {
        SidebarItem::Channel { id } if state.channels.get(&id).is_some_and(|c| c.kind == ct::GUILD_TEXT) && state.selected_channel != Some(id) => Some(id),
        _ => None,
    });
    if let Some(ch) = last.filter(|ch| state.channels.contains_key(ch)) {
        let read = Id(1_000_000_000_000_000);
        state.read_states.insert(ch, crate::model::ReadState { id: ch, last_message_id: Some(read), ..Default::default() });
        if let Some(c) = state.channels.get_mut(&ch) {
            c.last_message_id = Some(Id(read.0 + 1));
        }
        DONE.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

fn guild_channels(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id) {
    demo_long_list(cx.state, guild_id);
    demo_unread(ui.ctx(), cx.state, guild_id);
    let mut items = cx.state.sidebar_channels_with(guild_id, crate::plugins::sidebar::show_hidden());
    if cx.view.hide_muted.contains(&guild_id) {
        drop_muted(cx.state, &mut items);
    }
    if items.is_empty() {
        empty_state(ui, cx, 12.0, "No visible channels");
        return;
    }
    if nav_demo("collapsed") {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            if let Some(SidebarItem::Category { id, .. }) = items.iter().find(|i| matches!(i, SidebarItem::Category { .. })) {
                cx.view.collapsed_categories.insert(*id);
            }
        });
    }
    // Parent -> its threads listed under it (threads.rs draws them).
    let threads = cx.state.sidebar_threads(guild_id);
    let mut collapsed = false;
    // Categories fold open and shut (paint only, `motion::Fold`): k is the current one's.
    let (mut fold, mut k, mut moving) = (super::motion::Fold::default(), 1.0, false);
    let mut plate = Plate::new(ui, egui::Id::unique(("rc_plate", guild_id.0)), ROW_H, ui.max_rect().top());
    for (i, item) in items.into_iter().enumerate() {
        match item {
            SidebarItem::Category { id, name } => {
                if i > 0 {
                    ui.add_space(12.0); // + item spacing = 16 between sections
                }
                collapsed = cx.view.collapsed_categories.contains(&id);
                let resp = fold.row(ui, None, |ui| category_row(ui, cx, id, &name, collapsed));
                resp.context_menu(|ui| server_menu::category_menu(ui, cx, guild_id, id));
                if resp.clicked() {
                    collapsed = !collapsed;
                    if collapsed {
                        cx.view.collapsed_categories.insert(id);
                    } else {
                        cx.view.collapsed_categories.remove(&id);
                    }
                }
                k = super::motion::anim(ui.ctx(), egui::Id::unique(("rc_cat_fold", id)), !collapsed, super::motion::BASE, super::motion::BASE * super::motion::OUT);
                moving |= k > 0.0 && k < 1.0;
            }
            SidebarItem::Channel { id } => {
                // Collapsed categories still show the open channel, channels needing attention
                // and occupied voice channels; the others fold away while k runs down.
                let occupied = cx.state.voice_states.contains_key(&id);
                let stays = stays_listed(cx.state, id);
                let shown = !collapsed || stays || k > 0.0;
                if shown {
                    let folds = (!stays).then(|| (egui::Id::unique(("rc_fold", id)), k));
                    fold.row(ui, folds, |ui| {
                        channel_row(ui, cx, id, &mut plate);
                        if occupied {
                            voice_users(ui, cx, guild_id, id, &plate);
                        }
                        if let Some(t) = threads.get(&id) {
                            threads::sidebar_rows(ui, cx, id, t);
                        }
                    });
                    if cx.state.selected_channel == Some(id) {
                        plate.shift = fold.shift();
                    }
                }
            }
        }
    }
    fold.end(ui.ctx());
    ui.data_mut(|d| d.insert_temp(fold_moving_id(), moving));
    plate.finish(ui, cx);
}

fn fold_moving_id() -> egui::Id {
    egui::Id::unique("rc_fold_moving")
}

/// Whether a category folds this pass: rows whose layout spot is out of view may still be painted
/// in view (`in_view`).
pub(super) fn folding(ui: &egui::Ui) -> bool {
    ui.data(|d| d.get_temp::<bool>(fold_moving_id())).unwrap_or(false)
}

/// What a collapsed category still lists: the open channel, an occupied voice channel, one with
/// mentions, or one that is unread and not muted.
fn stays_listed(s: &AppState, id: Id) -> bool {
    s.selected_channel == Some(id)
        || s.voice_states.contains_key(&id)
        || s.mention_count(id) > 0
        || (s.is_unread(id) && !s.is_channel_muted(id))
}

/// Channel and thread ids of a server in sidebar order, as the list shows them (voice channels
/// included; the caller filters).
pub(super) fn listed_channels(s: &AppState, view: &ViewState, guild_id: Id) -> Vec<Id> {
    let mut items = s.sidebar_channels(guild_id);
    if view.hide_muted.contains(&guild_id) {
        drop_muted(s, &mut items);
    }
    let threads = s.sidebar_threads(guild_id);
    let mut out = Vec::new();
    let mut collapsed = false;
    for item in items {
        match item {
            SidebarItem::Category { id, .. } => collapsed = view.collapsed_categories.contains(&id),
            SidebarItem::Channel { id } if !collapsed || stays_listed(s, id) => {
                out.push(id);
                out.extend(threads.get(&id).into_iter().flatten().copied());
            }
            SidebarItem::Channel { .. } => {}
        }
    }
    out
}

fn reveal_id() -> egui::Id {
    egui::Id::unique("rc_reveal_row")
}

/// Scrolls the sidebar to channel `id` once its row is drawn (the navigation keys, the quick
/// switcher). Guild channel rows only: DM rows are virtualised.
pub(super) fn reveal(ctx: &egui::Context, id: Id) {
    ctx.data_mut(|d| d.insert_temp(reveal_id(), id));
}

/// What a click on a channel row does: join a voice or stage channel, else open it.
pub(super) fn open_channel(cx: &mut UiCx<'_>, id: Id) {
    let Some(ch) = cx.state.channels.get(&id) else { return };
    if matches!(ch.kind, ct::GUILD_VOICE | ct::GUILD_STAGE_VOICE) && cx.state.voice.channel_id != Some(id) {
        cx.cmd.send(Command::JoinVoice { guild_id: ch.guild_id, channel_id: id });
    } else {
        cx.state.select_channel(id);
        cx.view.settings_open = false;
    }
}

/// "Hide Muted Channels": muted channels and categories go, unless open or mentioning me.
fn drop_muted(s: &AppState, items: &mut Vec<SidebarItem>) {
    items.retain(|it| {
        let (SidebarItem::Category { id, .. } | SidebarItem::Channel { id }) = it;
        !s.is_channel_muted(*id) || s.selected_channel == Some(*id) || s.mention_count(*id) > 0
    });
}

/// Category header: its name in the label role (13/600) in the case given, after a 12 px
/// chevron that turns from down (open) to right (collapsed) in `FAST`.
fn category_row(ui: &mut egui::Ui, cx: &UiCx<'_>, id: Id, name: &str, collapsed: bool) -> egui::Response {
    let c = &cx.theme.colors;
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
    let resp = ui.interact(row, ui.make_persistent_id(("category", id)), Sense::click());
    let color = if resp.hovered() || resp.has_focus() { c.text } else { c.text_muted }.to_color32();
    let open = super::motion::slide(ui.ctx(), resp.id.with("open"), !collapsed, super::motion::FAST);
    if in_view(ui, row) {
        let turn = egui::emath::Rot2::from_angle((open - 1.0) * FRAC_PI_2);
        let center = pos2(row.left() + PAD - 2.0, row.center().y);
        let pts = [vec2(-4.0, -2.0), vec2(0.0, 2.0), vec2(4.0, -2.0)].map(|v| center + turn * v);
        ui.painter().add(Shape::line(pts.to_vec(), Stroke::new(1.5, color)));
        let left = row.left() + PAD + 8.0;
        let g = one_line(ui, name, super::semibold(cx.theme.font.size - 2.0), color, row.right() - PAD - left);
        ui.painter().galley(pos2(left, row.center().y - g.size().y / 2.0), g, color);
        if resp.has_focus() {
            row_focus(ui, cx, row);
        }
    }
    resp.widget_info(|| WidgetInfo::selected(Role::DisclosureTriangle, true, !collapsed, name));
    resp
}

/// Whether a row of the (not virtualized) channel list is in view. A row out of view keeps its
/// widget (focus order, menu, keyboard clicks, animations) but skips the paint, which the clip
/// would drop anyway: rows paint inside their rect, the margin covers anti-aliasing.
fn in_view(ui: &egui::Ui, row: Rect) -> bool {
    ui.is_rect_visible(row.expand(4.0)) || folding(ui)
}

/// Keyboard focus ring of a full-width list row, inset so it stays inside the card.
fn row_focus(ui: &egui::Ui, cx: &UiCx<'_>, row: Rect) {
    let pal = Pal::new(cx.theme);
    super::messages::focus_ring(ui, &pal, row.shrink2(vec2(ROW_INSET + 2.0, 2.0)), f32::from(pal.radius));
}

/// One gliding selection plate per list (docs/DESIGN.md §7, §9.11): the `selected` fill, with the
/// sheen line along its top on dark themes, travels from the old row to the new one over BASE. It
/// lives in the list's content space (`origin` = the content's top), so scrolling never moves it.
/// A list not drawn last pass, or a selected row not drawn, starts again in place. Rows tell it
/// which one is selected (`select`) and ask what is under their icons (`row_chrome`).
struct Plate {
    slot: egui::layers::ShapeIdx,
    id: egui::Id,
    origin: f32,
    /// A row's height.
    h: f32,
    /// Where the plate's top is this pass (before this pass's retarget).
    now: Option<f32>,
    sel: Option<Rect>,
    /// How much higher a fold above draws the selected row.
    shift: f32,
}

impl Plate {
    fn new(ui: &egui::Ui, id: egui::Id, h: f32, origin: f32) -> Self {
        let ctx = ui.ctx();
        let (pass, pass_id) = (ctx.cumulative_pass_nr(), id.with("pass"));
        if ctx.data(|d| d.get_temp::<u64>(pass_id)).is_none_or(|p| pass > p + 1) {
            super::motion::forget(ctx, id);
            ctx.data_mut(|d| d.remove::<f32>(id.with("to")));
        }
        ctx.data_mut(|d| d.insert_temp(pass_id, pass));
        let now = ctx.data(|d| d.get_temp::<f32>(id.with("to"))).map(|to| origin + super::motion::value(ctx, id, to, super::motion::BASE));
        Self { slot: ui.painter().add(Shape::Noop), id, origin, h, now, sel: None, shift: 0.0 }
    }

    /// Whether the plate lies over `row` right now (its middle is inside it).
    fn covers(&self, row: Rect) -> bool {
        self.now.is_some_and(|y| (y..y + self.h).contains(&row.center().y))
    }

    fn select(&mut self, row: Rect) {
        self.sel = Some(row);
    }

    /// Draws the plate at its tween's place; no selected row: forgets it.
    fn finish(self, ui: &egui::Ui, cx: &UiCx<'_>) {
        let (ctx, to_id) = (ui.ctx(), self.id.with("to"));
        let Some(r) = self.sel else {
            super::motion::forget(ctx, self.id);
            ctx.data_mut(|d| d.remove::<f32>(to_id));
            return;
        };
        let to = r.top() - self.shift - self.origin;
        ctx.data_mut(|d| d.insert_temp(to_id, to));
        if folding(ui) {
            // A category folding above moves the row each frame: the plate stays on it.
            super::motion::set_value(ctx, self.id, to);
        }
        let y = self.origin + super::motion::value(ctx, self.id, to, super::motion::BASE);
        let (fill, tk, cr) = (row_fills(ui, &cx.theme.colors).0, crate::theme::applied_tokens(ctx), CornerRadius::from(cx.theme.layout.corner_radius));
        let rect = Rect::from_min_size(pos2(r.left(), y), r.size()).shrink2(vec2(ROW_INSET, 0.0));
        let mut shapes = vec![Shape::rect_filled(rect, cr, fill)];
        if tk.sheen.a() > 0 {
            let r = f32::from(cr.nw);
            shapes.push(Shape::hline(rect.left() + r..=rect.right() - r, rect.top() + 0.5, Stroke::new(1.0, tk.sheen)));
        }
        ui.painter().set(self.slot, Shape::Vec(shapes));
    }
}

/// Row hover and unread pill shared by channel, DM and voice-user rows (the selected fill is the
/// list's `Plate`). Hover is on at once and fades out over FAST (`trail`, keyed by `id`); the unread
/// pill grows 0 -> 8 px over FAST. Returns the opaque colour under the row's icons now: the plate's
/// where it covers the row, else the sidebar with the fading hover fill.
#[allow(clippy::too_many_arguments)]
fn row_chrome(ui: &egui::Ui, cx: &UiCx<'_>, bg_slot: egui::layers::ShapeIdx, row: Rect, id: egui::Id, hovered: bool, unread: bool, plate: &Plate) -> Color32 {
    let c = &cx.theme.colors;
    let (selected_fill, hover_fill) = row_fills(ui, c);
    // The selected row shows the plate, not a hover fill over it.
    let hover = hover_fill.gamma_multiply(super::motion::trail(ui.ctx(), id.with("trail"), hovered && plate.sel != Some(row)));
    let cr = CornerRadius::from(cx.theme.layout.corner_radius);
    ui.painter().set(bg_slot, Shape::rect_filled(row.shrink2(vec2(ROW_INSET, 0.0)), cr, hover));
    let u = super::motion::value(ui.ctx(), id.with("unread"), if unread { 8.0 } else { 0.0 }, super::motion::FAST);
    if u >= 0.5 {
        let pill = Rect::from_min_size(pos2(row.left(), row.center().y - u / 2.0), vec2(4.0, u));
        ui.painter().rect_filled(pill, CornerRadius { nw: 0, sw: 0, ne: 4, se: 4 }, c.text.to_color32());
    }
    if plate.covers(row) { selected_fill } else { c.sidebar.to_color32().blend(hover) }
}

/// (selected, hovered) fills of rows: the `selected` and `hover` tokens over the sidebar colour,
/// opaque so status-dot rings and icon cut-outs can use them.
pub(super) fn row_fills(ui: &egui::Ui, c: &ThemeColors) -> (Color32, Color32) {
    let (tk, sidebar) = (crate::theme::applied_tokens(ui.ctx()), c.sidebar.to_color32());
    (sidebar.blend(tk.selected), sidebar.blend(tk.hover))
}

/// Name color of a channel/DM row: dim when muted (45 %), bright when it wants attention.
fn row_text_color(c: &ThemeColors, dim: bool, bright: bool) -> Color32 {
    if dim {
        c.text_muted.to_color32().gamma_multiply(0.45)
    } else if bright {
        c.text.to_color32()
    } else {
        c.text_muted.to_color32()
    }
}

/// One channel. A click on a voice/stage channel joins it (the hover chat button opens its
/// text chat); the connected one has a green bar on its left.
fn channel_row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, id: Id, plate: &mut Plate) {
    // ShowHiddenChannels: a locked, unselectable row.
    if crate::plugins::sidebar::hidden_row(ui, cx, id, ROW_H) {
        return;
    }
    let Some(ch) = cx.state.channels.get(&id) else { return };
    let (kind, guild_id) = (ch.kind, ch.guild_id);
    // Forum / media channels open their post list (forum.rs).
    let supported = ch.is_text_chat() || matches!(kind, ct::GUILD_FORUM | ct::GUILD_MEDIA);
    let voice = matches!(kind, ct::GUILD_VOICE | ct::GUILD_STAGE_VOICE);
    let name = ch.name.clone().unwrap_or_else(|| cx.state.channel_name(id));
    let selected = cx.state.selected_channel == Some(id);
    let connected = voice && cx.state.voice.channel_id == Some(id);

    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    if ui.data(|d| d.get_temp::<Id>(reveal_id())) == Some(id) {
        ui.scroll_to_rect(row, None);
        ui.data_mut(|d| d.remove::<Id>(reveal_id()));
    }
    if selected {
        plate.select(row);
    }
    let shown = in_view(ui, row);
    let bg_slot = shown.then(|| ui.painter().add(Shape::Noop));
    let resp = ui.interact(row, ui.make_persistent_id(("channel", id)), if supported { Sense::click() } else { Sense::hover() });
    // Also while the pointer is on the chat button (a widget on top of the row).
    let hovered = supported && ui.rect_contains_pointer(row);
    let mut open_chat = false;
    if let Some(bg_slot) = bg_slot {
        let muted = cx.state.is_channel_muted(id);
        let unread = supported && !muted && !selected && cx.state.is_unread(id);
        let mentions = if supported { cx.state.mention_count(id) } else { 0 };
        let fill = row_chrome(ui, cx, bg_slot, row, resp.id, hovered, unread, plate);

        let c = &cx.theme.colors;
        let size = cx.theme.font.size;
        if connected {
            let bar = Rect::from_min_size(pos2(row.left() + ROW_INSET, row.top() + 6.0), vec2(2.0, ROW_H - 12.0));
            ui.painter().rect_filled(bar, 1, presence_color(Presence::Online, c));
        }
        let color = row_text_color(c, !supported || muted, selected || unread || hovered);
        // 20 px icon after 8 px of row padding, then the name; it follows the name's colour.
        let slot = Rect::from_center_size(pos2(row.left() + ROW_INSET + 18.0, row.center().y), Vec2::splat(20.0));
        icons::paint_channel(ui.painter(), icons::of(&cx.state.channels[&id], cx.state), slot, color, fill);
        let mut right = row.right() - ROW_INSET - 8.0;
        if voice && hovered {
            let pal = Pal::new(cx.theme);
            let r = Rect::from_center_size(pos2(right - 10.0, row.center().y), Vec2::splat(20.0));
            // Not allocated: the row already holds the space.
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
            open_chat = icon_button(&mut child, &pal, "💬", "Open chat", 20.0, false, pal.text).clicked();
            right -= 24.0;
        } else if mentions > 0 {
            right -= badge(ui, c, pos2(right, row.center().y), mentions, None) + 8.0;
        }
        // Out of view no dots either: they would only keep the 80 ms repaint going.
        right -= crate::plugins::sidebar::typing_dots(ui, cx.state, id, pos2(right, row.center().y));
        // Body 500; unread 600 (docs/DESIGN.md §10.2).
        let font = if unread { super::semibold(size) } else { FontId::new(size, crate::theme::medium_family()) };
        let text_left = row.left() + ROW_INSET + 36.0;
        let g = one_line(ui, name.as_str(), font, color, right - text_left);
        ui.painter().galley(pos2(text_left, row.center().y - g.size().y / 2.0), g, color);
        if resp.has_focus() {
            row_focus(ui, cx, row);
        }
    }

    resp.widget_info(|| WidgetInfo::selected(Role::Button, supported, selected || connected, &name));
    resp.context_menu(|ui| channel_menu(ui, cx, id, guild_id, voice.then_some(connected)));
    if !supported {
        resp.on_hover_text("Rustcord can't open this channel type yet.");
    } else if open_chat {
        cx.state.select_channel(id);
        cx.view.settings_open = false;
        // The call view opens instead of the chat: show its chat column.
        cx.view.call_ui.chat |= connected;
    } else if resp.clicked() {
        open_channel(cx, id);
    } else if resp.middle_clicked() && !voice {
        super::tabs::open(ui.ctx(), cx, id);
    }
}

/// Channel menu. `voice` = Some(connected there) for voice channels: join / disconnect and
/// their text chat.
fn channel_menu(ui: &mut egui::Ui, cx: &mut UiCx<'_>, id: Id, guild_id: Option<Id>, voice: Option<bool>) {
    menu_setup(ui);
    let c = &cx.theme.colors;
    let (text, danger) = (c.text.to_color32(), c.danger.to_color32());
    if ui.add_enabled_ui(cx.state.is_unread(id), |ui| menu_item(ui, "Mark as read", MenuIcon::Glyph("✔"), text)).inner {
        mark_read(cx, [id]);
    }
    if let Some(connected) = voice {
        ui.separator();
        if connected {
            if menu_item(ui, "Disconnect", MenuIcon::Icon(Icon::Hangup), danger) {
                cx.cmd.send(Command::LeaveVoice);
            }
        } else if menu_item(ui, "Join voice", MenuIcon::Glyph("🔊"), text) {
            cx.cmd.send(Command::JoinVoice { guild_id, channel_id: id });
        }
        if menu_item(ui, "Open chat", MenuIcon::Glyph("💬"), text) {
            cx.state.select_channel(id);
            cx.view.settings_open = false;
            cx.view.call_ui.chat |= connected;
        }
    }
    super::tabs::menu_items(ui, cx, id);
    server_menu::channel_menu_items(ui, cx, id);
    ui.separator();
    if menu_item(ui, "Copy link", MenuIcon::Icon(Icon::Link), text) {
        let place = guild_id.map_or_else(|| "@me".to_owned(), |g| g.0.to_string());
        ui.ctx().copy_text(format!("https://discord.com/channels/{place}/{}", id.0));
    }
    if cx.state.account.settings.developer_mode() && menu_item(ui, "Copy channel ID", MenuIcon::Glyph("🆔"), text) {
        ui.ctx().copy_text(id.0.to_string());
    }
}

/// Someone in a voice channel, as listed under it.
struct VoiceUser {
    id: Id,
    name: String,
    url: Option<String>,
    muted: bool,
    deaf: bool,
    /// Muted/deafened by a moderator (marks in the danger color).
    by_server: bool,
    /// Go Live / camera on.
    live: bool,
    video: bool,
}

/// The people in a voice channel, sorted by name, as 24 px avatar rows under its row.
fn voice_users(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id, channel_id: Id, plate: &Plate) {
    let Some(states) = cx.state.voice_states.get(&channel_id) else { return };
    let mut people: Vec<VoiceUser> = states
        .values()
        .map(|v| {
            let user = cx.state.users.get(&v.user_id);
            VoiceUser {
                id: v.user_id,
                name: user.map_or_else(|| "Unknown user".into(), |u| cx.state.member_display_name(Some(guild_id), u, v.member.as_ref())),
                url: user.map(|u| super::images::user_avatar_url(Some(guild_id), u, v.member.as_ref(), 48, false)),
                muted: v.self_mute || v.mute || v.self_deaf || v.deaf,
                deaf: v.self_deaf || v.deaf,
                by_server: v.mute || v.deaf,
                live: v.self_stream,
                video: v.self_video,
            }
        })
        .collect();
    people.sort_by_cached_key(|p| (p.name.to_lowercase(), p.id));
    for p in &people {
        voice_user_row(ui, cx, guild_id, p, plate);
    }
}

fn voice_user_row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, guild_id: Id, u: &VoiceUser, plate: &Plate) {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), VOICE_ROW_H), Sense::hover());
    let bg_slot = ui.painter().add(Shape::Noop);
    let avatar = Rect::from_min_size(pos2(row.left() + 36.0, row.center().y - 12.0), Vec2::splat(24.0));
    match &u.url {
        Some(url) => user_avatar_at(ui, cx.images, url, avatar, false, 1.0, (u.id, &u.name)),
        None => paint_initial(ui.painter(), avatar, 12.0, u.id.0, &u.name, ui.visuals().dark_mode),
    }
    let who = cx.state.users.get(&u.id).or(cx.state.me.as_ref().filter(|m| m.id == u.id));
    let deco = who.and_then(|w| crate::overrides::effective_for(w, None).decoration);
    super::images::decoration_at(ui, cx.images, avatar, deco.as_ref(), false, 1.0);
    // Registered after the avatar widget so the whole row stays clickable.
    let resp = ui.interact(row, ui.make_persistent_id(("voice_user", u.id)), Sense::click());
    let hovered = resp.hovered();
    // The hover fill starts 8 px left of the avatar.
    let fill = row_chrome(ui, cx, bg_slot, row.with_min_x(row.left() + 20.0), resp.id, hovered, false, plate);

    let c = &cx.theme.colors;
    let size = cx.theme.font.size;
    let speak = super::motion::anim(ui.ctx(), resp.id.with("rc_speak"), cx.state.voice.speaking.contains(&u.id), 0.0, super::motion::FAST);
    if speak > 0.0 {
        ui.painter().circle_stroke(avatar.center(), 13.5, Stroke::new(2.0, presence_color(Presence::Online, c).gamma_multiply(speak)));
    }
    let bg = fill;
    let (muted, mark) = (c.text_muted.to_color32(), if u.by_server { c.danger } else { c.text_muted }.to_color32());
    let (mut right, cy) = (row.right() - ROW_INSET - 6.0, row.center().y);
    // Right to left, 4 px apart: "Live" tag, camera, deafened, muted.
    if u.live {
        let w = ui.painter().layout_no_wrap("Live".into(), bold(11.0), Color32::WHITE).size().x + 12.0;
        super::call::live_pill(ui.painter(), pos2(right - w, cy), crate::theme::Tokens::new(c).danger_fill);
        right -= w + 4.0;
    }
    for (on, icon, color) in [(u.video, Icon::Camera, muted), (u.deaf, Icon::Deafened, mark), (u.muted, Icon::MicOff, mark)] {
        if on {
            let r = Rect::from_min_max(pos2(right - 16.0, cy - 8.0), pos2(right, cy + 8.0));
            icons::paint(ui.painter(), icon, r, color, bg);
            right -= 20.0;
        }
    }
    let color = if hovered { c.text } else { c.text_muted }.to_color32();
    let text_left = avatar.right() + 8.0;
    let g = one_line(ui, u.name.as_str(), FontId::proportional(size - 1.0), color, right - 4.0 - text_left);
    ui.painter().galley(pos2(text_left, cy - g.size().y / 2.0), g, color);

    let state = match (u.deaf, u.muted) {
        (true, _) => ", deafened",
        (false, true) => ", muted",
        _ => "",
    };
    let (live, video) = (if u.live { ", live" } else { "" }, if u.video { ", camera on" } else { "" });
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, format!("{}{state}{live}{video}", u.name)));
    let at = (pos2(row.right() + 8.0, row.top()), Align2::LEFT_TOP);
    user_context_menu(&resp, cx, u.id, Some(guild_id), at);
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        cx.view.open_profile_at(u.id, Some(guild_id), at);
    }
}

/// DM rows under a "Direct messages" header, filtered by the header field; closed DMs are
/// left out until a new message arrives in them.
fn dm_list(ui: &mut egui::Ui, cx: &mut UiCx<'_>, area: egui::ScrollArea) -> Option<egui::containers::scroll_area::ScrollAreaOutput<()>> {
    // Opening a closed DM (notification, quick restore of the last channel) lists it again.
    if let Some(open) = cx.state.selected_channel {
        cx.view.hidden_dms.remove(&open);
    }
    let query = cx.view.dm_filter.trim().to_lowercase();
    let (ids, pins) = listed_dms(cx.state, cx.view);
    let (requests, spam) = cx.state.request_counts();
    // The "Message requests" row, while there are any or its page is open.
    let req = usize::from(requests + spam > 0 || cx.view.requests_ui.open);
    let plate_id = egui::Id::unique(("rc_plate", 0u64));
    if ids.is_empty() {
        let mut plate = Plate::new(ui, plate_id, DM_ROW_H, ui.max_rect().top());
        friends_row(ui, cx, Home::Friends, &mut plate);
        friends_row(ui, cx, Home::Quests, &mut plate);
        if req == 1 {
            friends_row(ui, cx, Home::Requests, &mut plate);
        }
        plate.finish(ui, cx);
        dm_header(ui, cx, false);
        empty_state(ui, cx, 24.0, if query.is_empty() { "No direct messages yet. Start one from your friends list." } else { "No conversations match that name." });
        return None;
    }
    // Rows are uniform (Friends, Quests, [Message requests], ["Pinned", the pinned DMs],
    // "Direct messages", the rest): only the visible ones are laid out.
    let head = usize::from(pins > 0);
    let top = 2 + req;
    Some(area.show_rows(ui, DM_ROW_H, ids.len() + 3 + req + head, |ui, range| {
        // The closure's ui starts at the first visible row: the content's top is above it.
        let origin = ui.max_rect().top() - range.start as f32 * (DM_ROW_H + ui.spacing().item_spacing.y);
        let mut plate = Plate::new(ui, plate_id, DM_ROW_H, origin);
        for i in range {
            match i {
                0 => friends_row(ui, cx, Home::Friends, &mut plate),
                1 => friends_row(ui, cx, Home::Quests, &mut plate),
                2 if req == 1 => friends_row(ui, cx, Home::Requests, &mut plate),
                i if i == top && head == 1 => dm_header(ui, cx, true),
                i if i == top + head + pins => dm_header(ui, cx, false),
                i if i < top + head + pins => dm_row(ui, cx, ids[i - top - head], &mut plate),
                i => dm_row(ui, cx, ids[i - top - 1 - head], &mut plate),
            }
        }
        plate.finish(ui, cx);
    }))
}

/// The DM ids as the list shows them (not closed, matching the filter, PinDMs' pinned ones on
/// top) and how many are pinned.
pub(super) fn listed_dms(s: &AppState, view: &ViewState) -> (Vec<Id>, usize) {
    let query = view.dm_filter.trim().to_lowercase();
    let mut ids: Vec<Id> = s
        .dm_order
        .iter()
        .copied()
        .filter(|&id| dm_listed(s, &view.hidden_dms, id) && !s.is_message_request(id))
        .filter(|&id| query.is_empty() || s.channel_name(id).to_lowercase().contains(&query))
        .collect();
    let pins = crate::plugins::sidebar::pins_first(&mut ids);
    (ids, pins)
}

/// The DM list's entries above the conversations.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Home {
    Friends,
    Quests,
    Requests,
}

/// "Friends" on top of the DM list: opens the Friends page (friends.rs); pending requests as a
/// badge. "Quests" (quests.rs, rewards to claim as the badge) and "Message requests"
/// (requests.rs, pending requests as the badge) sit under it.
fn friends_row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, home: Home, plate: &mut Plate) {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), DM_ROW_H), Sense::hover());
    let bg_slot = ui.painter().add(Shape::Noop);
    let id = match home {
        Home::Friends => "friends_row",
        Home::Quests => "quests_row",
        Home::Requests => "requests_row",
    };
    let resp = ui.interact(row, ui.make_persistent_id(id), Sense::click());
    let (quests, requests) = (cx.view.quests_ui.open, cx.view.requests_ui.open);
    let selected = match home {
        Home::Friends => super::friends::is_open(cx.state) && !quests && !requests,
        Home::Quests => super::friends::is_open(cx.state) && quests,
        Home::Requests => requests && !quests,
    };
    if selected {
        plate.select(row);
    }
    let fill = row_chrome(ui, cx, bg_slot, row, resp.id, resp.hovered(), false, plate);
    let c = &cx.theme.colors;
    let color = row_text_color(c, false, selected || resp.hovered());
    let size = cx.theme.font.size;
    let icon = Rect::from_center_size(pos2(row.left() + ROW_INSET + 24.0, row.center().y), Vec2::splat(24.0));
    let (glyph, label) = match home {
        Home::Friends => (Icon::Members, "Friends"),
        Home::Quests => (Icon::Quest, "Quests"),
        Home::Requests => (Icon::Envelope, "Message requests"),
    };
    icons::paint(ui.painter(), glyph, icon, color, fill);
    let mut right = row.right() - ROW_INSET - 8.0;
    let pending = match home {
        Home::Friends => cx.state.pending_count(),
        Home::Quests => cx.state.quests.claimable(),
        Home::Requests => cx.state.request_counts().0,
    } as u32;
    if pending > 0 {
        right -= badge(ui, c, pos2(right, row.center().y), pending, None) + 8.0;
    }
    let left = row.left() + ROW_INSET + 52.0;
    let g = one_line(ui, label, FontId::proportional(size), color, right - left);
    ui.painter().galley(pos2(left, row.center().y - g.size().y / 2.0), g, color);
    if resp.has_focus() {
        // The list's first row (the list clips above it): the ring traces the row's fill, inside it.
        let pal = Pal::new(cx.theme);
        super::messages::focus_ring(ui, &pal, row.shrink2(vec2(ROW_INSET + 4.0, 4.0)), (f32::from(pal.radius) - 4.0).max(0.0));
    }
    let name = if home == Home::Requests && pending > 0 { format!("{label}, {pending}") } else { label.to_owned() };
    resp.widget_info(|| WidgetInfo::selected(Role::Button, true, selected, &name));
    if resp.clicked() {
        if home == Home::Requests {
            super::requests::open(cx);
        } else {
            super::friends::open(cx.state);
            cx.view.quests_ui.open = home == Home::Quests;
            cx.view.requests_ui.open = false;
            cx.view.settings_open = false;
        }
    }
}

/// Whether a DM is listed: never closed here, or a message arrived after it was closed.
fn dm_listed(state: &AppState, hidden: &HashMap<Id, Option<Id>>, id: Id) -> bool {
    hidden.get(&id).is_none_or(|&closed_at| state.channels.get(&id).and_then(|c| c.last_message_id) > closed_at)
}

/// "Direct messages" with a "+" (Create DM: the group picker in friends.rs); PinDMs' "Pinned"
/// above the pinned DMs in the same style, without the "+".
fn dm_header(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pinned: bool) {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), DM_ROW_H), Sense::hover());
    let color = cx.theme.colors.text_muted.to_color32();
    let title = if pinned { "Pinned" } else { "Direct messages" };
    let g = one_line(ui, title, super::semibold(cx.theme.font.size - 2.0), color, row.width() - 2.0 * PAD - 24.0);
    let text_y = row.bottom() - g.size().y - 6.0;
    let center_y = text_y + g.size().y / 2.0;
    ui.painter().galley(pos2(row.left() + PAD + 2.0, text_y), g, color);
    if pinned {
        return;
    }
    let pal = Pal::new(cx.theme);
    let r = Rect::from_center_size(pos2(row.right() - PAD - 4.0, center_y), Vec2::splat(24.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
    if icon_button(&mut child, &pal, "+", "Create DM", 24.0, false, pal.text).clicked() {
        super::friends::open_group_dm(cx);
    }
}

/// A DM's picture in `rect`: the other user's avatar, or a group DM's icon (or its initial).
/// Returns the other user's presence (1:1 DMs).
pub(super) fn dm_avatar_at(ui: &mut egui::Ui, cx: &mut UiCx<'_>, id: Id, rect: Rect) -> Option<Presence> {
    let ch = cx.state.channels.get(&id)?;
    let group = ch.kind == ct::GROUP_DM;
    let first = ch.recipient_ids.first().and_then(|u| cx.state.users.get(u));
    let (url, who) = match (group, first) {
        (true, _) => (ch.icon.as_deref().map(|h| cdn::group_dm_icon_url(id, h, 64)), (id, cx.state.channel_name(id))),
        (false, Some(u)) => (Some(super::images::user_avatar_url(None, u, None, 64, false)), (u.id, u.display_name().to_owned())),
        (false, None) => (None, (id, cx.state.channel_name(id))),
    };
    let presence = ch.recipient_ids.first().filter(|_| !group).map(|&u| user_presence(cx.state, u));
    let dark = ui.visuals().dark_mode;
    match &url {
        Some(url) => user_avatar_at(ui, cx.images, url, rect, false, 1.0, (who.0, &who.1)),
        None => paint_initial(ui.painter(), rect, rect.width() / 2.0, who.0.0, &who.1, dark),
    }
    // Placeholders under 20 px are bare dots; the 16 px title-bar icon still shows the letter.
    let px = (rect.width() * ui.ctx().pixels_per_point()).ceil() as u32;
    if rect.height() < 20.0 && url.is_none_or(|u| cx.images.get(ui.ctx(), &u, px, false).is_none()) {
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, initial(&who.1), bold(9.0), initial_colors(who.0.0, dark).1);
    }
    presence
}

/// Paints `name` (in `color`) at x `left_center.x`, centred on its y; with a status line (the
/// meta role, muted, lifted to 4.5:1 on `bg`, the opaque colour under it) under it, the pair is
/// centred instead. `max_w` bounds the status line.
/// Returns the name's rect (guild tag chips go after it) and the right end of the text.
#[allow(clippy::too_many_arguments)]
pub(super) fn name_and_status(ui: &egui::Ui, cx: &mut UiCx<'_>, name: Arc<egui::Galley>, color: Color32, bg: Color32, status: Option<&StatusText>, left_center: egui::Pos2, max_w: f32) -> (Rect, f32) {
    let (x, cy) = (left_center.x, left_center.y);
    let Some(st) = status else {
        let at = Rect::from_min_size(pos2(x, cy - name.size().y / 2.0), name.size());
        ui.painter().galley(at.min, name, color);
        return (at, at.right());
    };
    let font = FontId::proportional(cx.theme.font.size - 3.0);
    let sub_h = ui.ctx().fonts_mut(|f| f.row_height(&font));
    let (name_h, muted) = (name.size().y, crate::theme::readable_on(cx.theme.colors.text_muted.to_color32(), bg));
    let top = cy - (name_h + sub_h) / 2.0;
    let at = Rect::from_min_size(pos2(x, top), name.size());
    ui.painter().galley(at.min, name, color);
    let sub = paint_status_text(ui, cx.images, pos2(x, top + name_h), st, egui::TextFormat::simple(font, muted), 16.0, max_w, 1);
    (at, at.right().max(sub.right()))
}

fn dm_row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, id: Id, list: &mut Plate) {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), DM_ROW_H), Sense::hover());
    let Some(ch) = cx.state.channels.get(&id) else { return };
    let group = ch.kind == ct::GROUP_DM;
    let name = cx.state.channel_name(id);
    let members = ch.recipient_ids.len() + 1;
    let other = ch.recipient_ids.first().copied().filter(|_| !group);
    let selected = cx.state.selected_channel == Some(id);
    let muted = cx.state.is_channel_muted(id);
    let unread = !muted && !selected && cx.state.is_unread(id);
    let mentions = cx.state.mention_count(id);

    let bg_slot = ui.painter().add(Shape::Noop);
    let wash_slot = ui.painter().add(Shape::Noop);
    // Also while the pointer is on the close button (a widget on top of the row).
    let hot = ui.rect_contains_pointer(row);
    // 1:1 DMs: the other user's cosmetics (ui/cosmetics.rs), the nameplate under the row.
    let peer = other.and_then(|u| cx.state.users.get(&u));
    let (fx, tag) = peer.map(|u| (crate::overrides::effective_for(u, None), super::cosmetics::guild_tag(u, None).cloned())).unwrap_or_default();
    let fx_anim = super::cosmetics::animates(cx, hot);
    // Filled once the text's extent is known (light themes fade the art in after it).
    let plate = ui.painter().add(Shape::Noop);
    let avatar = Rect::from_min_size(pos2(row.left() + ROW_INSET + 8.0, row.center().y - 16.0), Vec2::splat(32.0));
    dm_avatar_at(ui, cx, id, avatar);
    super::images::decoration_at(ui, cx.images, avatar, fx.decoration.as_ref(), fx_anim, 1.0);
    // Registered after the avatar widget so the whole row stays clickable.
    let resp = ui.interact(row, ui.make_persistent_id(("dm", id)), Sense::click());
    if selected {
        list.select(row);
    }
    let fill = row_chrome(ui, cx, bg_slot, row, resp.id, hot, unread, list);
    // Selected or hovered: the other person's aura washes in from the left (ui/aura.rs).
    let h = super::motion::trail(ui.ctx(), resp.id.with("trail"), hot && !selected);
    let aura = other.filter(|_| selected || h > 0.0).and_then(|u| cx.state.users.get(&u).cloned()).and_then(|u| super::aura::of(ui.ctx(), cx, &u, None));

    let c = &cx.theme.colors;
    let size = cx.theme.font.size;
    // The opaque colour under the row's text: the fill with the person's wash blended in.
    let mut ring = fill;
    if let Some(a) = aura {
        let alpha = super::aura::alpha(cx.settings.people_colors, cx.theme.tokens().mode) * if selected { 1.0 } else { 0.75 * h };
        let radius = cx.theme.layout.corner_radius as u8;
        ui.painter().set(wash_slot, super::aura::row_wash(ui.ctx(), row.shrink2(vec2(ROW_INSET, 0.0)), radius, a, alpha, 0.75));
        ring = ring.blend(a.gamma_multiply(alpha * 0.85));
    }
    if let Some(u) = other {
        user_dot(ui.painter(), avatar, cx.state, u, c, ring);
    }
    let color = row_text_color(c, muted, selected || unread || hot);
    let chip_under = fill;
    let mut right = row.right() - ROW_INSET - 8.0;
    let mut close = false;
    if hot {
        let x_rect = Rect::from_min_max(pos2(right - 20.0, row.center().y - 10.0), pos2(right, row.center().y + 10.0));
        let x = ui.interact(x_rect, ui.make_persistent_id(("dm_close", id)), Sense::click());
        let x_color = if x.hovered() { c.text.to_color32() } else { c.text_muted.to_color32().gamma_multiply(0.8) };
        icons::paint(ui.painter(), Icon::Close, Rect::from_center_size(x_rect.center(), Vec2::splat(16.0)), x_color, chip_under);
        x.widget_info(|| WidgetInfo::labeled(Role::Button, true, "Close DM"));
        close = x.on_hover_text("Close DM").clicked();
        right -= 24.0;
    } else if mentions > 0 {
        right -= badge(ui, c, pos2(right, row.center().y), mentions, None) + 8.0;
    }
    // The other user's activity picture (`members::art_slot`); the close button takes its place.
    if let Some(a) = other.filter(|_| !hot).and_then(|u| crate::activity::activity_of(cx.state, u)).map(std::borrow::Cow::into_owned) {
        let art = super::members::ART;
        let r = Rect::from_min_size(pos2(right - art, row.center().y - art / 2.0), Vec2::splat(art));
        super::members::art_slot(ui, cx, &a, r, chip_under);
        right -= art + 8.0;
    }
    right -= super::dm_call::row_mark(ui, cx, id, pos2(right, row.center().y));
    right -= crate::plugins::sidebar::typing_dots(ui, cx.state, id, pos2(right, row.center().y));
    let text_left = avatar.right() + 12.0;
    let font = if unread { bold(size) } else { FontId::proportional(size) };
    let chip_w = tag.as_ref().map_or(0.0, |pg| super::cosmetics::tag_chip_width(ui, pg, size) + 4.0);
    let style = fx.name_style.as_ref().filter(|_| !muted);
    let title = super::cosmetics::name_galley(ui, name.as_str(), font, style, color, right - text_left - chip_w);
    // Group: its size; 1:1: the other user's custom status or activity.
    let sub = if group { Some((None, format!("{members} members"))) } else { other.and_then(|u| user_status_text(cx.state, u)) };
    let under = ui.painter().add(Shape::Noop);
    let (name_rect, text_end) = name_and_status(ui, cx, title.clone(), color, ring, sub.as_ref(), pos2(text_left, row.center().y), right - text_left);
    ui.painter().set(under, super::cosmetics::name_underlay(name_rect.min, &title, style, ui.visuals().dark_mode));
    if let Some(np) = &fx.nameplate {
        let (radius, plate_rect, end) = (cx.theme.layout.corner_radius.round() as u8, row.shrink2(vec2(ROW_INSET, 0.0)), text_end.max(name_rect.right() + chip_w));
        let shape = super::cosmetics::nameplate(ui, cx.images, plate_rect, np, fx_anim, muted, radius, end);
        // A scrim keeps the name and status line off the plate: the art's bright spots (dark
        // themes), the palette tint under coloured names (light themes).
        let scrim = (!muted).then(|| super::cosmetics::plate_scrim(plate_rect, radius, chip_under, end, crate::theme::is_high_contrast(&cx.theme.colors)));
        ui.painter().set(plate, Shape::Vec(std::iter::once(shape).chain(scrim).collect()));
    }
    if let Some(pg) = &tag {
        let at = pos2(name_rect.right() + 4.0, name_rect.center().y);
        match fx.nameplate {
            // Opaque over the nameplate art.
            Some(_) => super::cosmetics::tag_chip_on(ui, cx.images, pg, at, size, color, chip_under),
            None => super::cosmetics::tag_chip(ui, cx.images, pg, at, size, color),
        };
    }

    resp.widget_info(|| WidgetInfo::selected(Role::Button, true, selected, &name));
    resp.context_menu(|ui| dm_menu(ui, cx, id, pos2(row.right() + 8.0, row.top())));
    if close {
        close_dm(cx, id);
    } else if resp.clicked() {
        cx.state.select_channel(id);
        cx.view.settings_open = false;
    } else if resp.middle_clicked() {
        super::tabs::open(ui.ctx(), cx, id);
    }
}

/// Closes a DM here only: it comes back with its next message.
fn close_dm(cx: &mut UiCx<'_>, id: Id) {
    let last = cx.state.channels.get(&id).and_then(|c| c.last_message_id);
    cx.view.hidden_dms.insert(id, last);
    if cx.state.selected_channel == Some(id) {
        cx.state.selected_channel = None;
        cx.state.dirty = true;
    }
}

/// DM menu: mark read, the other user's profile, close (1:1 DMs), and their id (a group's
/// channel id).
fn dm_menu(ui: &mut egui::Ui, cx: &mut UiCx<'_>, id: Id, anchor: egui::Pos2) {
    menu_setup(ui);
    let text = cx.theme.colors.text.to_color32();
    let user = cx.state.channels.get(&id).filter(|c| c.kind == ct::DM).and_then(|c| c.recipient_ids.first().copied());
    if ui.add_enabled_ui(cx.state.is_unread(id), |ui| menu_item(ui, "Mark as read", MenuIcon::Glyph("✔"), text)).inner {
        mark_read(cx, [id]);
    }
    ui.separator();
    super::tabs::menu_items(ui, cx, id);
    if let Some(u) = user
        && menu_item(ui, "Profile", MenuIcon::Glyph("👤"), text)
    {
        cx.view.open_profile(u, None, anchor);
    }
    // Groups have Leave Group instead (friends.rs): one way out each.
    if user.is_some() && menu_item(ui, "Close DM", MenuIcon::Glyph("✖"), text) {
        close_dm(cx, id);
    }
    crate::plugins::sidebar::pin_item(ui, cx, id, &mut |ui, label, glyph| menu_item(ui, label, MenuIcon::Glyph(glyph), text));
    server_menu::sounds_item(ui, cx, id);
    super::friends::dm_menu_items(ui, cx, id);
    let dev = cx.state.account.settings.developer_mode();
    if dev {
        ui.separator();
    }
    let (label, copy) = match user {
        Some(u) => ("Copy user ID", u),
        None => ("Copy channel ID", id),
    };
    if dev && menu_item(ui, label, MenuIcon::Glyph("🆔"), text) {
        ui.ctx().copy_text(copy.0.to_string());
    }
}

// ---------------------------------------------------------------------------
// User panel
// ---------------------------------------------------------------------------

/// Background of the user panel: `surface`, one step up from the panes (E1).
fn user_panel_fill(c: &ThemeColors) -> Color32 {
    c.surface.to_color32()
}

/// Corner radius of the floating user panel: the container tier (1.5r).
fn panel_r(theme: &Theme) -> u8 {
    Pal::new(theme).card_radius()
}

/// Height of the floating user panel: the account row (two text lines grow with the font
/// size), plus the voice section and a hairline while in a call, and the quest video section
/// and its hairline while a quest video plays.
fn user_panel_h(ctx: &egui::Context, cx: &UiCx<'_>) -> f32 {
    user_panel_height(cx.state, cx.view, cx.theme) - voice_target(cx.state) + voice_h(ctx, cx.state) + (MUSIC_H + 1.0) * music(ctx, cx).0
}

/// The "Quest video" section with its hairline, or nothing.
fn quest_h(view: &ViewState) -> f32 {
    if super::quests::watching(view) { super::quests::PANEL_H + 1.0 } else { 0.0 }
}

/// The music section: how far it is open (0 -> 1, eased) and what it shows (with the picture's
/// generation). The last track stays for the exit fade. Off, unsupported or nothing playing: closed.
fn music(ctx: &egui::Context, cx: &UiCx<'_>) -> (f32, Option<(crate::now_playing::Track, u64)>) {
    use crate::now_playing as np;
    let now = Instant::now();
    let live = if nav_demo("music") || nav_demo_value("music").is_some() {
        static SINCE: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        let playing = ctx.data_mut(|d| *d.get_temp_mut_or(egui::Id::unique("rc_music_demo_playing"), nav_demo_value("music") != Some("paused")));
        let track = np::Track { title: "Rewriting it in Rust".into(), artist: "Ferris and the Crabs".into(), art: None, can_next: true, playing };
        Some((track, *SINCE.get_or_init(|| now), 0))
    } else if np::SUPPORTED && cx.settings.music_controls {
        np::current(ctx)
    } else {
        None
    };
    let shown = live.filter(|(t, since, _)| np::visible(t, *since, now));
    let last = egui::Id::unique("rc_music_last");
    let t = super::motion::anim(ctx, egui::Id::unique("rc_music_row"), shown.is_some(), super::motion::BASE, super::motion::BASE * super::motion::OUT);
    let track = match shown {
        Some((track, since, generation)) => {
            // Paused: one wake when the minute is over.
            if let Some(d) = np::wake_in(&track, since, now) {
                ctx.request_repaint_after(d);
            }
            ctx.data_mut(|d| d.insert_temp(last, (track.clone(), generation)));
            Some((track, generation))
        }
        None if t > 0.0 => ctx.data(|d| d.get_temp(last)),
        None => None,
    };
    (t, track)
}

/// `user_panel_h` without a `UiCx` (startup hooks).
// shortcut: the startup profile hook ignores the music row; pass ctx if RUSTCORD_OPEN=profile is ever combined with music.
pub(super) fn user_panel_height(state: &AppState, view: &ViewState, theme: &Theme) -> f32 {
    account_h(theme) + voice_target(state) + quest_h(view)
}

/// The voice section's final height with its hairline: 0 outside a call, the connection row alone
/// while connecting, with the camera / share row once connected.
fn voice_target(state: &AppState) -> f32 {
    let buttons = if state.voice.connected { VOICE_BUTTONS_H } else { 0.0 };
    if state.voice.channel_id.is_some() { VOICE_PANEL_H + buttons + 1.0 } else { 0.0 }
}

/// The voice section's height now (docs/DESIGN.md §7): grows over BASE in a call, shrinks over BASE x OUT
/// after it; the camera row joining it once connected glides the same way.
fn voice_h(ctx: &egui::Context, state: &AppState) -> f32 {
    use super::motion::{BASE, OUT};
    super::motion::value(ctx, egui::Id::unique("rc_voice_section"), voice_target(state), if state.voice.channel_id.is_some() { BASE } else { BASE * OUT })
}

fn account_h(theme: &Theme) -> f32 {
    ACCOUNT_H.max(3.0 * theme.font.size + 12.0)
}

/// Empty bottom panel that keeps a column's content above the floating user panel.
fn user_panel_space(ui: &mut egui::Ui, cx: &UiCx<'_>, salt: &str) {
    egui::Panel::bottom(salt)
        .exact_size(user_panel_h(ui.ctx(), cx) + 2.0 * PANEL_MARGIN)
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::new())
        .show(ui, |_| ());
}

/// The floating user panel, 8 px from the window's bottom-left corner over the server bar and
/// the channel list (`width` = both): the voice connection while in a call, then my account.
pub fn user_panel(ctx: &egui::Context, cx: &mut UiCx<'_>, width: f32, shift: f32) {
    // Focus mode slides the panel left with the lists (`shift` px); gone once it is out.
    if shift >= width {
        return;
    }
    let c = cx.theme.colors.clone();
    let (open, track) = music(ctx, cx);
    let music_h = (MUSIC_H + 1.0) * open;
    let voice_now = voice_h(ctx, cx.state);
    let h = user_panel_height(cx.state, cx.view, cx.theme) - voice_target(cx.state) + voice_now + music_h;
    let screen = ctx.content_rect();
    let pos = pos2(screen.left() + PANEL_MARGIN - shift, screen.bottom() - PANEL_MARGIN - h);
    egui::Area::new(egui::Id::unique("user_panel")).order(egui::Order::Middle).fixed_pos(pos).fade_in(false).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(width - 2.0 * PANEL_MARGIN, h), Sense::hover());
        ui.painter().rect_filled(rect, panel_r(cx.theme), user_panel_fill(&c));
        // My aura from the left, fading out at 75 % (ui/aura.rs).
        if let Some(a) = cx.state.me.clone().and_then(|me| super::aura::of(ctx, cx, &me, None)) {
            let alpha = super::aura::alpha(cx.settings.people_colors, cx.theme.tokens().mode);
            ui.painter().add(super::aura::row_wash(ctx, rect, panel_r(cx.theme), a, alpha, 0.75));
        }
        // The music section: laid out at its final size against its bottom edge, clipped to the
        // part that is open, so the panel's rising top edge reveals it.
        let top = rect.top() + music_h;
        if let Some((track, generation)) = track.filter(|_| open > 0.0) {
            ui.scope(|ui| {
                ui.set_clip_rect(ui.clip_rect().intersect(rect.with_max_y(top)));
                ui.multiply_opacity(open);
                music_row(ui, cx, Rect::from_min_size(pos2(rect.left(), top - MUSIC_H - 1.0), vec2(rect.width(), MUSIC_H)), &track, generation);
                ui.painter().hline(rect.x_range(), top - 0.5, line(&c));
            });
        }
        let quest_h = quest_h(cx.view);
        if quest_h > 0.0 {
            super::quests::panel(ui, cx, Rect::from_min_max(pos2(rect.left(), top), pos2(rect.right(), top + quest_h - 1.0)));
            ui.painter().hline(rect.x_range(), top + quest_h - 0.5, line(&c));
        }
        if voice_now > 0.0 {
            let split = top + quest_h + voice_now;
            // Laid out against the account row at its final size and clipped to the part that is
            // open (the rising top edge reveals it); the camera row joining it moves the rest up.
            let target = voice_target(cx.state);
            if target > 0.0 {
                ui.scope(|ui| {
                    ui.set_clip_rect(ui.clip_rect().intersect(Rect::from_min_max(pos2(rect.left(), split - voice_now), pos2(rect.right(), split))));
                    ui.multiply_opacity((voice_now / target).min(1.0));
                    let full = (voice_now - 1.0).max(VOICE_PANEL_H);
                    voice_panel(ui, cx, Rect::from_min_max(pos2(rect.left(), split - 1.0 - full), pos2(rect.right(), split - 1.0)));
                });
            }
            ui.painter().hline(rect.x_range(), split - 0.5, line(&c));
        }
        account_row(ui, cx, rect.with_min_y(top + quest_h + voice_now), rect.left_top());
        // Last: the outline and the lit top edge stay over the nameplate.
        ui.painter().rect_stroke(rect, panel_r(cx.theme), line(&c), egui::StrokeKind::Inside);
        // The top edge catches the pointer (§6.2).
        ui.painter().add(super::light::rim(ctx, rect, f32::from(panel_r(cx.theme)), &cx.theme.tokens()));
    });
}

/// What the computer plays: the picture (or a `Listening` tile), title over artist, Play / Pause
/// and Next (one OS command each). `rect` is the 40 px row; docs/DESIGN.md §10.5.
fn music_row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, rect: Rect, track: &crate::now_playing::Track, generation: u64) {
    let c = cx.theme.colors.clone();
    let (text, muted, base) = (c.text.to_color32(), c.text_muted.to_color32(), user_panel_fill(&c));
    let ctx = ui.ctx().clone();
    let radius = CornerRadius::from(cx.theme.layout.corner_radius);
    // Next, then Play / Pause, 8 px in and 4 px apart.
    let next = Rect::from_center_size(pos2(rect.right() - 8.0 - 16.0, rect.center().y), Vec2::splat(32.0));
    let play = next.translate(vec2(-36.0, 0.0));
    let button = |ui: &mut egui::Ui, at: Rect, id: &str, icon: Icon, tip: &str, enabled: bool| {
        let resp = ui.interact(at, ui.make_persistent_id(id), if enabled { Sense::click() } else { Sense::hover() });
        let hot = enabled && (resp.hovered() || resp.has_focus());
        let drawn = if enabled { super::motion::pressed(&resp, at) } else { at };
        let bg = if hot { base.blend(text.gamma_multiply(0.08)) } else { base };
        if hot {
            ui.painter().rect_filled(drawn, radius, bg);
        }
        let color = if !enabled { muted.gamma_multiply(0.4) } else if hot { text } else { muted };
        icons::paint(ui.painter(), icon, drawn.shrink(6.0), color, bg);
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, tip));
        resp.on_hover_text(tip).on_hover_cursor(if enabled { egui::CursorIcon::PointingHand } else { egui::CursorIcon::Default }).clicked()
    };
    let demo = nav_demo("music") || nav_demo_value("music").is_some();
    if button(ui, play, "music_play", if track.playing { Icon::Pause } else { Icon::Play }, if track.playing { "Pause" } else { "Play" }, true) {
        if demo {
            ctx.data_mut(|d| d.insert_temp(egui::Id::unique("rc_music_demo_playing"), !track.playing));
        } else {
            crate::now_playing::toggle();
        }
    }
    if button(ui, next, "music_next", Icon::Next, if track.can_next { "Next track" } else { "This player can't skip" }, track.can_next) && !demo {
        crate::now_playing::next();
    }
    // The picture, 24 px at x + 12, rounded like a chip; else a `Listening` tile.
    let art = Rect::from_center_size(pos2(rect.left() + 12.0 + 12.0, rect.center().y), Vec2::splat(24.0));
    let chip = Pal::new(cx.theme).radius / 2;
    let tex = track.art.as_ref().map(|img| {
        let id = egui::Id::unique("rc_music_art");
        match ctx.data(|d| d.get_temp::<(u64, egui::TextureHandle)>(id)) {
            Some((g, tex)) if g == generation => tex,
            _ => {
                let tex = ctx.load_texture("rc_now_playing", (**img).clone(), egui::TextureOptions::LINEAR);
                ctx.data_mut(|d| d.insert_temp(id, (generation, tex.clone())));
                tex
            }
        }
    });
    match tex {
        Some(tex) => egui::Image::from_texture(&tex).corner_radius(chip).paint_at(ui, art),
        None => {
            ui.painter().rect_filled(art, chip, c.code_background.to_color32());
            icons::paint(ui.painter(), Icon::Listening, Rect::from_center_size(art.center(), Vec2::splat(16.0)), muted, c.code_background.to_color32());
        }
    }
    // Title (label role) over artist (meta); the title alone is centred.
    let size = cx.theme.font.size;
    let (left, max_w) = (rect.left() + 44.0, play.left() - 8.0 - (rect.left() + 44.0));
    let title = one_line(ui, track.title.as_str(), super::semibold(size - 2.0), text, max_w);
    let artist = (!track.artist.is_empty()).then(|| one_line(ui, track.artist.as_str(), FontId::proportional(size - 3.0), muted, max_w));
    let block = title.size().y + artist.as_ref().map_or(0.0, |a| a.size().y);
    let top = rect.center().y - block / 2.0;
    let below = top + title.size().y;
    ui.painter().galley(pos2(left, top), title, text);
    if let Some(a) = artist {
        ui.painter().galley(pos2(left, below), a, muted);
    }
    let area = Rect::from_min_max(pos2(left, rect.top()), pos2(left + max_w, rect.bottom()));
    let tip = if track.artist.is_empty() { track.title.clone() } else { format!("{} by {}", track.title, track.artist) };
    let resp = ui.interact(area, ui.make_persistent_id("music_text"), Sense::hover());
    resp.widget_info(|| WidgetInfo::labeled(Role::Label, true, format!("Now playing: {tip}")));
    resp.on_hover_text(tip);
}

/// "Voice connected" (or "Connecting…", "Live" while I share) over "channel / server" (opens the
/// call view), and the disconnect button; once connected, Camera and Screen Share buttons under
/// them (lit while on: a click stops it).
fn voice_panel(ui: &mut egui::Ui, cx: &mut UiCx<'_>, rect: Rect) {
    let Some(ch) = cx.state.voice.channel_id else { return };
    let c = cx.theme.colors.clone();
    let row = Rect::from_min_size(rect.min + Vec2::splat(12.0), vec2(rect.width() - 24.0, 32.0));
    let sharing = cx.state.voice.video.screen_share.is_some();
    // Connected in green text, connecting in amber (both readable on light panels too).
    let tk = cx.theme.tokens();
    let (word, color) = if cx.state.voice.connected { ("Voice connected", tk.positive_text) } else { ("Connecting…", crate::theme::readable_on(tk.attention, user_panel_fill(&c))) };
    let word = &crate::plugins::sidebar::call_timer(ui.ctx(), cx.state, word);
    let guild = cx.state.channels.get(&ch).and_then(|x| x.guild_id).and_then(|g| cx.state.guild(g)).map(|g| g.name.clone());
    let place = match guild {
        Some(g) => format!("{} / {g}", cx.state.channel_name(ch)),
        None => cx.state.channel_name(ch),
    };
    let button = Rect::from_min_size(pos2(row.right() - 32.0, row.center().y - 16.0), Vec2::splat(32.0));
    let max_w = button.left() - 8.0 - row.left();
    let link_rect = Rect::from_min_max(pos2(row.left(), row.center().y), pos2(row.left() + max_w, row.bottom()));
    let link = ui.interact(link_rect, ui.make_persistent_id("voice_place"), Sense::click());
    let link_color = if link.hovered() { c.text } else { c.text_muted }.to_color32();
    // Label role over meta (docs/DESIGN.md §10.5).
    let top_g = one_line(ui, word, super::semibold(cx.theme.font.size - 2.0), color, max_w - if sharing { 44.0 } else { 0.0 });
    let sub_g = one_line(ui, place.as_str(), FontId::proportional(cx.theme.font.size - 3.0), link_color, max_w);
    let top = row.center().y - (top_g.size().y + sub_g.size().y) / 2.0;
    let sub_y = top + top_g.size().y;
    if sharing {
        super::call::live_pill(ui.painter(), pos2(row.left() + top_g.size().x + 6.0, top + top_g.size().y / 2.0), cx.theme.tokens().danger_fill);
    }
    ui.painter().galley(pos2(row.left(), top), top_g, color);
    ui.painter().galley(pos2(row.left(), sub_y), sub_g, link_color);
    link.widget_info(|| WidgetInfo::labeled(Role::Link, true, &place));
    if link.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        cx.state.select_channel(ch);
        cx.view.settings_open = false;
    }

    let resp = ui.interact(button, ui.make_persistent_id("voice_disconnect"), Sense::click());
    let hot = resp.hovered() || resp.has_focus();
    let base = user_panel_fill(&c);
    let bg = if hot { base.blend(c.text.to_color32().gamma_multiply(0.08)) } else { base };
    ui.painter().rect_filled(button, CornerRadius::from(cx.theme.layout.corner_radius), bg);
    icons::paint(ui.painter(), Icon::Hangup, button.shrink(6.0), c.danger.to_color32(), bg);
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, "Disconnect"));
    if resp.on_hover_text("Disconnect").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        cx.cmd.send(Command::LeaveVoice);
    }

    if !cx.state.voice.connected {
        return;
    }
    // Two equal buttons, 8 px apart, on a resting tint so they read as buttons.
    let w = (row.width() - 8.0) / 2.0;
    let colors = (base.blend(c.text.to_color32().gamma_multiply(0.06)), c.text.to_color32(), c.danger.to_color32(), (Color32::TRANSPARENT, Color32::TRANSPARENT));
    let cam = cx.state.voice.video.camera_on;
    // Linux / macOS: no camera or screen capture yet (disabled with the reason).
    use crate::voice::video::capture::{NO_CAMERA, NO_SHARE, SUPPORTED};
    let camera = super::call::MediaButton { icon: Icon::Camera, tip: if !SUPPORTED { NO_CAMERA } else if cam { "Turn off camera" } else { "Turn on camera" }, on: cam, red: false, slashed: false, enabled: SUPPORTED };
    // Lit from the Go Live click on (still connecting): a click cancels it.
    let share = cx.state.voice.video.sharing();
    let screen = super::call::MediaButton { icon: Icon::Screen, tip: if !SUPPORTED { NO_SHARE } else if share { "Stop streaming" } else { "Share your screen" }, on: share, red: false, slashed: false, enabled: SUPPORTED };
    let radius = cx.theme.layout.corner_radius;
    if camera.show(ui, Rect::from_min_size(pos2(row.left(), row.bottom() + 8.0), vec2(w, 32.0)), radius, "voice_camera", colors).clicked() {
        super::call::toggle_camera(cx);
    }
    if screen.show(ui, Rect::from_min_size(pos2(row.right() - w, row.bottom() + 8.0), vec2(w, 32.0)), radius, "voice_screen", colors).clicked() {
        super::call::toggle_share(cx);
    }
}

/// Microphone / headphones: the 32 px button and the ∨ of its device menu.
const GROUP_W: f32 = 50.0;

/// Me, like Discord's panel: my nameplate behind the whole row, avatar (decoration, status) over
/// name and a second line (an activity's icon • my custom status, else my status or "Playing X";
/// the connection while not connected; my username while hovered). A click opens the account
/// popout (my card with the status picker, profile.rs) above the panel, whose top-left is
/// `panel`. Then microphone and headphones, each with its device menu, and settings.
fn account_row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, rect: Rect, panel: egui::Pos2) {
    let c = cx.theme.colors.clone();
    let size = cx.theme.font.size;
    let me = cx.state.me.clone();
    let cy = rect.center().y;
    // From the right, 8 px in and 4 px apart: settings, headphones, microphone.
    let gear = Rect::from_min_size(pos2(rect.right() - 8.0 - 32.0, cy - 16.0), Vec2::splat(32.0));
    let phones = Rect::from_min_size(pos2(gear.left() - 4.0 - GROUP_W, cy - 16.0), vec2(GROUP_W, 32.0));
    let mic = phones.translate(vec2(-GROUP_W - 4.0, 0.0));
    // Plugins: GameActivityToggle's button left of the microphone; AccountPanelServerProfile's
    // server (my server name, avatar and profile there).
    let plug_w = crate::plugins::panel::buttons_width(mic.left() - 4.0 - (rect.left() + 52.0));
    let guild = crate::plugins::panel::profile_guild(cx.state);
    let avatar = Rect::from_min_size(pos2(rect.left() + 12.0, cy - 16.0), Vec2::splat(32.0));
    // Avatar and name: one button with a hover tint.
    let area = Rect::from_min_max(avatar.min - Vec2::splat(4.0), pos2(mic.left() - 4.0 - plug_w, avatar.bottom() + 4.0));
    let hot = me.is_some() && ui.rect_contains_pointer(area);
    let base = user_panel_fill(&c);
    let tint = c.text.to_color32().gamma_multiply(0.08);
    // My cosmetics (real or my local picks; ui/cosmetics.rs): nameplate, decoration, name style.
    let fx = me.as_ref().map(|u| crate::overrides::effective_for(u, None)).unwrap_or_default();
    let fx_anim = super::cosmetics::animates(cx, ui.rect_contains_pointer(rect));
    // My nameplate: filled once the text's extent is known (light themes fade the art in after it).
    let cover = ui.painter().add(Shape::Noop);
    if hot {
        ui.painter().rect_filled(area, CornerRadius::from(cx.theme.layout.corner_radius), tint);
    }
    let fill = if hot { base.blend(tint) } else { base };
    match &me {
        Some(me) => {
            let url = super::images::user_avatar_url(guild, me, guild.and_then(|g| cx.state.find_member(g, me.id)), 64, false);
            let animate = crate::overrides::my_avatar_animates(me.id, ui.rect_contains_pointer(avatar)).unwrap_or(false);
            user_avatar_at(ui, cx.images, &url, avatar, animate, 1.0, (me.id, me.display_name()));
            super::images::decoration_at(ui, cx.images, avatar, fx.decoration.as_ref(), fx_anim, 1.0);
        }
        None => {
            ui.painter().circle_filled(avatar.center(), 16.0, c.surface.to_color32());
        }
    }
    let (line, detail) = my_status_line(ui, cx);
    status_dot(ui.painter(), avatar, my_presence(cx.state), &c, fill);

    let (mute, deaf) = (cx.view.self_mute, cx.view.self_deaf);
    let buttons_left = mic.left() - plug_w;
    crate::plugins::panel::game_button(ui, cx, mic.translate(vec2(-plug_w, 0.0)).with_max_x(mic.left() - 4.0), base);
    let mic = voice_button(ui, cx, mic, false, if mute || deaf { "Unmute" } else { "Mute" }, mute || deaf);
    let phones = voice_button(ui, cx, phones, true, if deaf { "Undeafen" } else { "Deafen" }, deaf);
    let open = cx.view.settings_open;
    if panel_button(ui, cx.theme, gear, Icon::Gear, "User settings", false, open).clicked() {
        cx.view.settings_open = !open;
    }
    if mic || phones {
        let (m, d) = toggle_voice(mute, deaf, mic);
        (cx.view.self_mute, cx.view.self_deaf) = (m, d);
        if d != deaf {
            cx.cmd.send(Command::SetSelfDeaf(d));
        }
        if m != mute {
            cx.cmd.send(Command::SetSelfMute(m));
        }
    }

    let text_left = avatar.right() + 8.0;
    let max_w = area.right() - 4.0 - text_left;
    let name = me.as_ref().map_or_else(|| "Rustcord".to_owned(), |u| cx.state.member_display_name(guild, u, None));
    let label = format!("{name}, {}", line.1);
    let status = line.clone();
    // My activity (my presence's, or the game I share) once my status is known: its icon in
    // green and its name, then my custom status after a divider (cut first). Hovered: my username.
    // Rustcord's own presence says nothing here (it is always on): the status line stays.
    let activity = me.as_ref().and_then(|u| crate::activity::activity_of(cx.state, u.id)).map(std::borrow::Cow::into_owned).filter(|a| !crate::activity::is_rustcord(a));
    let connecting = matches!(cx.state.conn, ConnState::Connecting | ConnState::Reconnecting { .. }) && !crate::config::offline_mode();
    let shown = activity.as_ref().filter(|_| cx.state.chat.my_status.is_some() && !connecting);
    let username = me.as_ref().filter(|_| hot && !cx.settings.streamer.hides_personal()).map(|u| (None, u.username.clone()));
    let (lead, line) = match (username, shown) {
        (Some(u), _) => (None, Some(u)),
        (None, Some(a)) => (Some(a), status_text(cx.state.chat.my_custom_status.as_ref(), None).filter(|_| max_w >= PANEL_STATUS_MIN_W)),
        (None, None) => (None, Some(line)),
    };
    // Name 14/600 over the meta line (docs/DESIGN.md §10.5).
    let name_g = super::cosmetics::name_galley(ui, &name, super::semibold(size - 1.0), fx.name_style.as_ref(), c.text.to_color32(), max_w);
    let sub_font = FontId::proportional(size - 3.0);
    let sub_h = ui.fonts_mut(|f| f.row_height(&sub_font));
    let top = cy - (name_g.size().y + sub_h) / 2.0;
    let name_at = pos2(text_left, top);
    ui.painter().add(super::cosmetics::name_underlay(name_at, &name_g, fx.name_style.as_ref(), ui.visuals().dark_mode));
    let sub_y = top + name_g.size().y;
    let name_end = name_at.x + name_g.size().x;
    ui.painter().galley(name_at, name_g, c.text.to_color32());
    let muted = c.text_muted.to_color32();
    let (mut x, end) = (text_left, text_left + max_w);
    let mut sub_right = x;
    if let Some(a) = lead {
        let side = 14.0;
        let full = one_line(ui, a.name.as_str(), sub_font.clone(), muted, f32::INFINITY).size().x;
        // A narrow panel (720 px window) drops the icon only when that shows the whole name (the
        // tooltip still says "Playing …"); otherwise the icon stays and the name is cut after it.
        if !(max_w < PANEL_STATUS_MIN_W && full + side + 4.0 > end - x && full <= end - x) {
            let icon = Rect::from_min_size(pos2(x, sub_y + (sub_h - side) / 2.0), Vec2::splat(side));
            icons::paint(ui.painter(), icons::activity(a.kind), icon, presence_color(Presence::Online, &c), base);
            x = icon.right() + 4.0;
        }
        let g = one_line(ui, a.name.as_str(), sub_font.clone(), muted, end - x);
        let w = g.size().x;
        ui.painter().galley(pos2(x, sub_y), g, muted);
        x += w;
        sub_right = x;
        if line.is_some() && end - x > 24.0 {
            let mid = sub_y + sub_h / 2.0;
            ui.painter().vline(x + 6.0, mid - 5.0..=mid + 5.0, Stroke::new(1.0, c.text.to_color32().gamma_multiply(0.1)));
            x += 13.0;
        } else {
            x = end;
        }
    }
    if let Some(line) = line.as_ref().filter(|_| end - x > 8.0) {
        sub_right = paint_status_text(ui, cx.images, pos2(x, sub_y), line, egui::TextFormat::simple(sub_font, muted), 16.0, end - x, 1).right();
    }
    if let Some(np) = &fx.nameplate {
        // Under the voice section only the lower corners are the panel's.
        let r = panel_r(cx.theme);
        let corners = if rect.top() > panel.y + 0.5 { CornerRadius { sw: r, se: r, ..CornerRadius::ZERO } } else { CornerRadius::same(r) };
        let end = name_end.max(sub_right);
        let shape = super::cosmetics::nameplate_cover(ui, cx.images, rect, np, fx_anim, corners, base, end);
        // High Contrast: the panel colour stays opaque behind the text (and the buttons).
        let (hc, art) = (crate::theme::is_high_contrast(&c), !matches!(shape, Shape::Noop));
        let strong = (hc && art).then(|| super::cosmetics::plate_scrim(rect, corners, base, end, true));
        // The buttons' glyphs on bright art.
        let buttons = art.then(|| super::cosmetics::buttons_scrim(rect, corners, base, buttons_left, hc));
        ui.painter().set(cover, Shape::Vec(std::iter::once(shape).chain(strong).chain(buttons).collect()));
    }

    let Some(me) = &me else { return };
    // Registered after the avatar widget so the whole area stays clickable.
    let resp = ui.interact(area, ui.make_persistent_id("my_account"), Sense::click());
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, &label));
    let playing = activity.map(|a| super::members::art_tooltip(&a, threads::unix_ms()));
    let tip = [playing, status_tooltip(ui, &status), detail].into_iter().flatten().collect::<Vec<_>>().join("\n");
    let resp = if tip.is_empty() { resp } else { resp.on_hover_text(tip) };
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        cx.view.open_profile_above(me.id, guild, panel - vec2(0.0, 8.0));
    }
}

/// (mute, deaf) after a click on the microphone (`mic`) or on the headphones, like Discord:
/// unmuting while deafened also undeafens; deafening implies mute (shown, and the voice module
/// sends both), and undeafening restores the mute switch as it was.
pub(super) fn toggle_voice(mute: bool, deaf: bool, mic: bool) -> (bool, bool) {
    match (mic, deaf) {
        (true, true) => (false, false),
        (true, false) => (!mute, false),
        (false, _) => (mute, !deaf),
    }
}

/// Hover pill of the user panel's buttons: on the plain panel a 6 % text tint; mostly opaque
/// panel colour, so on a bright nameplate it still shows and keeps the glyph readable.
pub(crate) fn pill_fill(c: &ThemeColors) -> Color32 {
    user_panel_fill(c).lerp_to_gamma(c.text.to_color32(), 0.08).gamma_multiply(0.75)
}

/// Resting glyph colour of the user panel's buttons: near the text colour like Discord's, so the
/// icons stay readable on a bright nameplate.
fn panel_icon_rest(c: &ThemeColors) -> Color32 {
    c.text.to_color32().gamma_multiply(0.82)
}

/// 32 px icon button of the user panel: soft glyph on the panel (its nameplate shows through),
/// tinted on hover, pressed while `on`; `slashed` is the danger-colored "off" variant. The
/// tooltip is also the accessible name.
fn panel_button(ui: &mut egui::Ui, theme: &Theme, rect: Rect, icon: Icon, tip: &str, slashed: bool, on: bool) -> egui::Response {
    let c = &theme.colors;
    let resp = ui.interact(rect, ui.make_persistent_id(("panel_button", icon as u8)), Sense::click());
    let rect = super::motion::pressed(&resp, rect);
    let hot = on || resp.hovered() || resp.has_focus();
    if hot {
        // As bright as a hovered half of the microphone / headphones pill.
        let r = CornerRadius::from(theme.layout.corner_radius.clamp(0.0, 16.0));
        ui.painter().rect_filled(rect, r, pill_fill(c));
        ui.painter().rect_filled(rect, r, c.text.to_color32().gamma_multiply(0.06));
    }
    let color = if slashed {
        c.danger.to_color32()
    } else if hot {
        c.text.to_color32()
    } else {
        panel_icon_rest(c)
    };
    icons::paint(ui.painter(), if slashed { icon.off() } else { icon }, rect.shrink(7.0), color, user_panel_fill(c));
    resp.widget_info(|| WidgetInfo::selected(Role::Button, true, on || slashed, tip));
    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Microphone or headphones (`output`) in `group`: the 32 px button, then the ∨ of its device
/// menu, one pill on hover of either, the hovered half (the ∨ also while its menu is open)
/// brighter; `slashed` (muted / deafened): the red "off" glyph on a red tint. True when the
/// button was clicked.
fn voice_button(ui: &mut egui::Ui, cx: &mut UiCx<'_>, group: Rect, output: bool, tip: &str, slashed: bool) -> bool {
    let c = cx.theme.colors.clone();
    let icon = if output { Icon::Headphones } else { Icon::Mic };
    let (button, chevron) = (group.with_max_x(group.left() + 32.0), group.with_min_x(group.left() + 32.0));
    let resp = ui.interact(button, ui.make_persistent_id(("panel_button", icon as u8)), Sense::click());
    let more = ui.interact(chevron, ui.make_persistent_id(("panel_devices", icon as u8)), Sense::click());
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&more));
    let slash = super::motion::anim(ui.ctx(), ui.make_persistent_id(("rc_slash", output)), slashed, super::motion::FAST, super::motion::FAST * super::motion::OUT);
    let (hot, more_hot) = (resp.hovered() || resp.has_focus(), open || more.hovered() || more.has_focus());
    let r = cx.theme.layout.corner_radius.clamp(0.0, 16.0).round() as u8;
    let p = ui.painter();
    // What the icon sits on (the slash's gap).
    let mut under = user_panel_fill(&c);
    if slash > 0.0 {
        let tint = c.danger.to_color32().gamma_multiply(0.12 * slash);
        p.rect_filled(group, r, tint);
        under = under.blend(tint);
    }
    // Discord: one pill around icon and ∨ while either is hovered, the hovered half brighter.
    if hot || more_hot {
        p.rect_filled(group, r, pill_fill(&c));
        let (half, corners) = if hot { (button, CornerRadius { nw: r, sw: r, ..CornerRadius::ZERO }) } else { (chevron, CornerRadius { ne: r, se: r, ..CornerRadius::ZERO }) };
        p.rect_filled(half, corners, c.text.to_color32().gamma_multiply(0.06));
        under = under.blend(pill_fill(&c));
        if hot {
            under = under.blend(c.text.to_color32().gamma_multiply(0.06));
        }
    }
    let color = if hot { c.text.to_color32() } else { panel_icon_rest(&c) }.lerp_to_gamma(c.danger.to_color32(), slash);
    icons::paint_slashed(p, icon, button.shrink(7.0), color, under, slash);
    let k = chevron.center() - vec2(2.0, 0.0);
    let v = if more_hot { c.text.to_color32() } else { panel_icon_rest(&c) };
    p.add(Shape::line(vec![k + vec2(-3.5, -1.75), k + vec2(0.0, 1.75), k + vec2(3.5, -1.75)], Stroke::new(1.5, v)));

    let label = if output { "Output options" } else { "Input options" };
    more.widget_info(|| WidgetInfo::labeled(Role::Button, true, label));
    if more.clicked() && !open {
        // Local: the voice task lists the audio devices (no request).
        cx.cmd.send(Command::ListVoiceDevices);
    }
    let more = if open { more } else { more.on_hover_text(label) };
    device_menu(cx, &more.on_hover_cursor(egui::CursorIcon::PointingHand), output);
    resp.widget_info(|| WidgetInfo::selected(Role::Button, true, slashed, tip));
    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// The ∨ menu of the microphone / headphones (`output`): the devices of the last
/// `ListVoiceDevices`, Default first and the pick ticked (a pick goes to the voice task and is
/// saved, like on the Voice & Video page), then a link to that page.
fn device_menu(cx: &mut UiCx<'_>, chevron: &egui::Response, output: bool) {
    let (text, muted) = (cx.theme.colors.text.to_color32(), cx.theme.colors.text_muted.to_color32());
    let (list, current) = if output { (&cx.state.voice.outputs, &cx.settings.voice_output) } else { (&cx.state.voice.inputs, &cx.settings.voice_input) };
    let (mut pick, mut page) = (None, false);
    egui::Popup::menu(chevron).align(egui::RectAlign::TOP_START).gap(8.0).show(|ui| {
        menu_setup(ui);
        ui.set_max_width(280.0);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        ui.label(egui::RichText::new(if output { "Output device" } else { "Input device" }).size(12.0).color(muted));
        for name in std::iter::once(None).chain(list.iter().map(Some)) {
            let glyph = if current.as_ref() == name { "✔" } else { "" };
            if menu_item(ui, name.map_or("Default", String::as_str), MenuIcon::Glyph(glyph), text) {
                pick = Some(name.cloned());
            }
        }
        ui.separator();
        page = menu_item(ui, "Voice settings", MenuIcon::Glyph("⚙"), text);
    });
    if let Some(name) = pick {
        *(if output { &mut cx.settings.voice_output } else { &mut cx.settings.voice_input }) = name;
        cx.cmd.send(Command::SetVoiceDevices { input: cx.settings.voice_input.clone(), output: cx.settings.voice_output.clone() });
        if let Err(e) = cx.settings.save() {
            cx.view.toast(format!("Could not save settings: {e}"), true);
        }
    }
    if page {
        cx.view.settings_page = super::user_settings::Page::VoiceVideo;
        cx.view.settings_open = true;
    }
}

/// Test hook `RUSTCORD_OPEN=status:<online|idle|dnd|invisible>`: my status as the gateway would
/// report it, with a sample custom status, and a game as my member-list presence carries it
/// (not with `RUSTCORD_FAKE_GAME`: then my detected game shows).
pub(super) fn demo_status(state: &mut AppState, status: &str) {
    let Some(status) = UserStatus::parse(status) else { return };
    let emoji = crate::model::Emoji { name: Some("🦀".into()), ..Default::default() };
    let custom_status = Some(CustomStatus { text: Some("Rewriting it in Rust".into()), emoji: Some(emoji) });
    state.apply_gateway(crate::model::GatewayEvent::MyStatus { status, custom_status }, None);
    if let Some(me) = state.me.as_ref().map(|u| u.id) {
        let game = std::env::var_os("RUSTCORD_FAKE_GAME").is_none().then(|| super::members::demo_activity(0));
        let p = crate::model::UserPresence { status: status.presence(), activity: game, ..Default::default() };
        state.apply_gateway(crate::model::GatewayEvent::PresenceUpdate(vec![(me, p)]), None);
    }
}

/// (status line, connection details for the tooltip unless connected) of the user panel.
fn my_status_line(ui: &egui::Ui, cx: &UiCx<'_>) -> (StatusText, Option<String>) {
    let (conn, offline_mode) = (&cx.state.conn, crate::config::offline_mode());
    let elapsed = cx.view.conn_since.elapsed().as_secs();
    if matches!(conn, ConnState::Reconnecting { .. }) && !offline_mode {
        // Keep the countdown (tooltip) ticking.
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
    }
    let detail = (*conn != ConnState::Connected || offline_mode).then(|| connection_label(conn, elapsed, offline_mode));
    (my_line(cx.state, offline_mode), detail)
}

/// The text of `my_status_line`: while connecting the connection, else my custom status or
/// my status once the gateway reported it.
fn my_line(state: &AppState, offline_mode: bool) -> StatusText {
    let connecting = matches!(state.conn, ConnState::Connecting | ConnState::Reconnecting { .. });
    match state.chat.my_status {
        Some(s) if !connecting => status_text(state.chat.my_custom_status.as_ref(), None).unwrap_or_else(|| (None, status_label(s).into())),
        _ => (None, connection_word(&state.conn, offline_mode).into()),
    }
}

/// My status dot: my chosen status once known, else the connection state (idle while
/// connecting).
pub(super) fn my_presence(state: &AppState) -> Presence {
    match (state.chat.my_status, &state.conn, crate::config::offline_mode()) {
        (Some(s), ..) => s.presence(),
        (None, _, true) | (None, ConnState::Offline, _) => Presence::Offline,
        (None, ConnState::Connected, _) => Presence::Online,
        _ => Presence::Idle,
    }
}

/// The one word under my name.
fn connection_word(conn: &ConnState, offline_mode: bool) -> &'static str {
    match (conn, offline_mode) {
        (_, true) | (ConnState::Offline, _) => "Offline",
        (ConnState::Connected, _) => "Online",
        (ConnState::Connecting, _) => "Connecting…",
        (ConnState::Reconnecting { .. }, _) => "Reconnecting…",
    }
}

/// Details of the connection state (tooltip of the user panel's status line).
fn connection_label(conn: &ConnState, elapsed_secs: u64, offline_mode: bool) -> String {
    if offline_mode {
        return "Offline mode".into();
    }
    match conn {
        ConnState::Connected => "Connected".into(),
        ConnState::Connecting => "Connecting…".into(),
        ConnState::Reconnecting { in_secs, .. } => {
            let left = u64::from(*in_secs).saturating_sub(elapsed_secs);
            if left == 0 { "Reconnecting…".into() } else { format!("Reconnecting in {left}s") }
        }
        ConnState::Offline => "Offline".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn island_width_glides_only_for_a_new_look() {
        let ctx = egui::Context::default();
        let key = egui::Id::unique("island_test");
        let at = |t: f64, look: (bool, Id, bool), w: f32| {
            let mut v = 0.0;
            let mut out = ctx.run_ui(egui::RawInput { time: Some(t), ..Default::default() }, |ui| v = island_width(ui.ctx(), key, look, w));
            out.textures_delta.clear();
            v
        };
        let (a, b) = ((false, Id(1), true), (true, Id(1), true));
        assert_eq!(at(0.0, a, 200.0), 200.0);
        assert_eq!(at(1.0, a, 207.0), 207.0, "a timer tick: at once");
        assert_eq!(at(2.0, b, 237.0), 207.0, "a new look starts from where it is");
        let mid = at(2.1, b, 237.0);
        assert!(mid > 207.0 && mid < 237.0);
        assert_eq!(at(2.5, b, 237.0), 237.0);
        assert_eq!(at(3.0, b, 244.0), 244.0, "later changes are at once again");
    }

    #[test]
    fn badges_pop_only_for_new_unmuted_mentions() {
        assert!(pops(Some(1), 2, false, 5.0, 5.0));
        assert!(!pops(None, 2, false, 5.0, 5.0), "first sight");
        assert!(!pops(Some(2), 2, false, 5.0, 5.0), "same count");
        assert!(!pops(Some(3), 2, false, 5.0, 5.0), "going down");
        assert!(!pops(Some(1), 2, true, 5.0, 5.0), "muted");
        assert!(!pops(Some(1), 2, false, 1.0, 5.0), "the connection just changed");
        assert!(!pops(Some(1), 2, false, 5.0, 0.5), "popped a moment ago");
    }

    #[test]
    fn a_trip_out_of_view_grows_in_place() {
        let view = egui::Rangef::new(0.0, 100.0);
        assert!(!jumps(view, 10.0, 90.0));
        assert!(jumps(view, 10.0, 190.0));
        assert!(jumps(view, -20.0, 50.0));
    }

    #[test]
    fn badge_digits_roll_only_between_shown_counts() {
        assert!(rolls(Some(1), 2));
        assert!(!rolls(None, 2), "first sight");
        assert!(!rolls(Some(0), 1), "appearing");
        assert!(!rolls(Some(2), 0), "going away");
        assert!(!rolls(Some(3), 3), "same count");
    }

    #[test]
    fn the_top_fade_matches_the_band() {
        let s = Color32::from_rgb(20, 20, 30);
        let b = Color32::from_rgba_unmultiplied(200, 40, 40, 140);
        assert_eq!(band_under(s, None, 40.0), s);
        assert_eq!(band_under(s, Some(b), 0.0), s.blend(b));
        assert_eq!(band_under(s, Some(b), 150.0), s);
        assert_eq!(band_under(s, Some(b), 400.0), s);
        let mid = band_under(s, Some(b), 75.0);
        assert!(mid != s && mid != s.blend(b), "between: a weaker band");
    }

    #[test]
    fn listed_channels_follow_the_sidebar() {
        let snapshot = serde_json::from_str(include_str!("../../tests/fixtures/home/cache/snapshot.json")).unwrap();
        let s = AppState::from_snapshot(snapshot);
        let mut view = ViewState::default();
        let rust = Id(1565443437363200006);
        let ids = |ns: &[u64]| ns.iter().map(|n| Id(1565443437363200000 + n)).collect::<Vec<_>>();
        assert_eq!(listed_channels(&s, &view, rust), ids(&[10, 11, 12, 14]));
        view.collapsed_categories.insert(Id(1565443437363200013));
        assert_eq!(listed_channels(&s, &view, rust), ids(&[10, 11, 12]));
    }

    #[test]
    fn initials_of_names() {
        assert_eq!(initials("Rust Workshop"), "RW");
        assert_eq!(initials("  egui  fans club  extra "), "efc");
        assert_eq!(initials("Ünïcode"), "Ü");
        assert_eq!(initials("   "), "?");
    }

    #[test]
    fn connection_labels() {
        assert_eq!(connection_label(&ConnState::Connected, 0, false), "Connected");
        assert_eq!(connection_label(&ConnState::Connected, 0, true), "Offline mode");
        assert_eq!(connection_label(&ConnState::Reconnecting { attempt: 2, in_secs: 5 }, 2, false), "Reconnecting in 3s");
        assert_eq!(connection_label(&ConnState::Reconnecting { attempt: 2, in_secs: 5 }, 9, false), "Reconnecting…");
        assert_eq!(connection_label(&ConnState::Offline, 0, false), "Offline");
    }

    #[test]
    fn connection_words() {
        assert_eq!(connection_word(&ConnState::Connected, false), "Online");
        assert_eq!(connection_word(&ConnState::Connected, true), "Offline");
        assert_eq!(connection_word(&ConnState::Reconnecting { attempt: 1, in_secs: 3 }, false), "Reconnecting…");
        assert_eq!(connection_word(&ConnState::Offline, false), "Offline");
    }

    #[test]
    fn mute_and_deafen_toggles() {
        // Mic: plain toggle; while deafened it undeafens and unmutes.
        assert_eq!(toggle_voice(false, false, true), (true, false));
        assert_eq!(toggle_voice(true, false, true), (false, false));
        assert_eq!(toggle_voice(true, true, true), (false, false));
        assert_eq!(toggle_voice(false, true, true), (false, false));
        // Headphones: toggle deafen, keep the mute switch for when it ends.
        assert_eq!(toggle_voice(false, false, false), (false, true));
        assert_eq!(toggle_voice(false, true, false), (false, false));
        assert_eq!(toggle_voice(true, false, false), (true, true));
        assert_eq!(toggle_voice(true, true, false), (true, false));
    }

    #[test]
    fn hide_muted_keeps_the_open_channel() {
        let snap = serde_json::from_str(include_str!("../../tests/fixtures/home/cache/snapshot.json")).unwrap();
        let mut s = AppState::from_snapshot(snap);
        let g = s.guilds[0].id;
        let ids = |s: &AppState| -> Vec<Id> {
            let mut items = s.sidebar_channels(g);
            drop_muted(s, &mut items);
            items.iter().map(|(SidebarItem::Category { id, .. } | SidebarItem::Channel { id })| *id).collect()
        };
        let all = ids(&s);
        // Mute the "Voice" category (its channel follows) and the open channel.
        let (voice_cat, open) = (Id(1565443437363200013), s.selected_channel.unwrap());
        let overrides = [voice_cat, open].map(|channel_id| crate::model::ChannelOverride { channel_id, muted: true, ..Default::default() });
        s.guild_settings.insert(Some(g), crate::model::GuildSettings { guild_id: Some(g), channel_overrides: overrides.to_vec(), ..Default::default() });
        let kept = ids(&s);
        assert!(kept.contains(&open));
        assert!(!kept.contains(&voice_cat) && !kept.contains(&Id(1565443437363200014)));
        assert_eq!(kept.len(), all.len() - 2);
    }

    #[test]
    fn drop_zones_around_an_icon() {
        let icon = Rect::from_min_size(pos2(16.0, 100.0), Vec2::splat(ICON));
        let at = |y: f32, slot, merge| zone(Some(pos2(30.0, y)), icon, slot, merge).map(|t| (t.to, t.line, t.ring.is_some()));
        let merge = Some(GuildDrop::NewFolder { with: Id(9) });
        // Top-level: edges insert before / after (the line in the gap), the middle merges.
        assert_eq!(at(97.0, Slot::Top(2), merge), Some((GuildDrop::Top(2), Some(96.0), false)));
        assert_eq!(at(138.0, Slot::Top(2), merge), Some((GuildDrop::Top(3), Some(144.0), false)));
        assert_eq!(at(120.0, Slot::Top(2), merge), Some((GuildDrop::NewFolder { with: Id(9) }, None, true)));
        assert_eq!(at(120.0, Slot::Top(2), None), None);
        assert_eq!(at(150.0, Slot::Top(2), merge), None);
        // In a folder: halves.
        assert_eq!(at(110.0, Slot::Child(7, 1), None), Some((GuildDrop::Folder { id: 7, index: 1 }, Some(96.0), false)));
        assert_eq!(at(130.0, Slot::Child(7, 1), None), Some((GuildDrop::Folder { id: 7, index: 2 }, Some(144.0), false)));
        assert_eq!(zone(None, icon, Slot::Top(0), merge).map(|t| t.to), None);
    }

    #[test]
    fn streams_read_as_streaming() {
        use crate::model::{GatewayEvent, UserPresence};
        let mut s = AppState::default();
        let with = |status, kind| UserPresence { status, activity: Some(Activity { kind, name: "Twitch".into(), rich: None }), ..Default::default() };
        let list = vec![(Id(1), with(Presence::Online, 1)), (Id(2), with(Presence::Idle, 1)), (Id(3), with(Presence::Online, 0)), (Id(4), with(Presence::Offline, 1))];
        s.apply_gateway(GatewayEvent::PresenceUpdate(list), None);
        assert_eq!(user_presence_label(&s, Id(1)), "Streaming");
        assert_eq!(user_presence_label(&s, Id(2)), "Streaming");
        assert_eq!(user_presence_label(&s, Id(3)), "Online");
        assert_eq!(user_presence_label(&s, Id(4)), "Offline");
        assert_eq!(user_presence_label(&s, Id(5)), "Offline");
    }

    #[test]
    fn closed_dms_return_with_new_messages() {
        let mut s = AppState::default();
        let dm = crate::model::Channel { id: Id(5), kind: ct::DM, last_message_id: Some(Id(10)), ..Default::default() };
        s.channels.insert(dm.id, dm);
        let mut hidden = HashMap::new();
        assert!(dm_listed(&s, &hidden, Id(5)));
        hidden.insert(Id(5), Some(Id(10)));
        assert!(!dm_listed(&s, &hidden, Id(5)));
        s.channels.get_mut(&Id(5)).unwrap().last_message_id = Some(Id(11));
        assert!(dm_listed(&s, &hidden, Id(5)));
    }
}
