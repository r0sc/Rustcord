//! Settings > Profile Cosmetics: avatar decoration, nameplate, profile effect and profile frame
//! from Discord's catalog and my owned items (real CDN thumbnails), display-name style and
//! profile theme, with a live preview of my card (COSMETICS_NOTES §0.4, §12).
//!
//! What a pick does (`use_of`): an item I own, and with Nitro the name style / theme, goes on my
//! account only through "Apply to account" (`AccountCommand::UpdateCosmetics`; "Remove"
//! unequips). Without Nitro every other pick applies at once and only in this client
//! (`crate::overrides::LocalCosmetics`, "Only visible to you"; nothing is sent). With Nitro no
//! local pick renders and unowned items are preview-only (Discord rejects them).
//! The catalog loads when the page first opens (24 h disk copy), my owned items each time it opens.
//! OWNERSHIP: cosmetics-source-gating engineer.

use super::super::messages::Pal;
use super::super::{UiCx, cosmetics as display, hint, images, one_line, profile_modal, rgb, sidebar::user_avatar_at};
use super::{op_line, op_state, primary_button};
use crate::api::collectibles::{self, CatalogItem, CosmeticsPatch, kind};
use crate::backend::{AccountCommand, Command, OpKind, SocialCommand};
use crate::state::account::OpState;
use crate::model::{DecorationData, Id, NameStyle, Nameplate, ProfileCollectible, User, UserProfile};
use crate::overrides::{self, LocalCosmetics, LocalItem, LocalNameStyle};
use egui::{CornerRadius, FontId, Rect, RichText, ScrollArea, Sense, Shape, Stroke, TextEdit, Vec2, pos2, vec2};

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum Tab {
    #[default]
    Decoration,
    Nameplate,
    Effect,
    Frame,
    NameStyle,
    Theme,
}

impl Tab {
    const ALL: [Tab; 6] = [Tab::Decoration, Tab::Nameplate, Tab::Effect, Tab::Frame, Tab::NameStyle, Tab::Theme];

    fn label(self) -> &'static str {
        match self {
            Tab::Decoration => "Avatar decoration",
            Tab::Nameplate => "Nameplate",
            Tab::Effect => "Profile effect",
            Tab::Frame => "Profile frame",
            Tab::NameStyle => "Name style",
            Tab::Theme => "Profile theme",
        }
    }

    /// The collectible kind of a catalog tab.
    fn kind(self) -> Option<u8> {
        match self {
            Tab::Decoration => Some(kind::DECORATION),
            Tab::Nameplate => Some(kind::NAMEPLATE),
            Tab::Effect => Some(kind::EFFECT),
            Tab::Frame => Some(kind::FRAME),
            Tab::NameStyle | Tab::Theme => None,
        }
    }
}

/// Editor state (`UserSettingsUi::cosmetics`: reset with the view at logout, like the catalog).
#[derive(Default)]
pub(super) struct Form {
    tab: Tab,
    search: String,
    /// Category filter (None = all).
    category: Option<String>,
    /// Pass this page was last drawn in (None or a gap = opened: owned items are asked again).
    pass: Option<u64>,
    /// The tile last clicked in this tab (`Some(None)`: "None"); the preview shows it.
    draft: Option<Option<CatalogItem>>,
    /// Nitro: the name style / theme being edited, until "Apply to account".
    style: Option<LocalNameStyle>,
    theme: Option<[u32; 2]>,
    /// Tab of the last account write (its progress / error shows there).
    applying: Option<Tab>,
    /// The catalog shown is the saved copy, not Discord's answer this session.
    cached: bool,
    /// A local colour changed while dragging: saved on release.
    dirty: bool,
    /// My banner in the preview (freed by `leave`).
    banner: Option<String>,
}

/// What picking something does for me (COSMETICS_NOTES §0.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Use {
    /// "Apply to account" equips it for real: owned items, and the Nitro perks with Nitro.
    Account,
    /// Without Nitro: only in this client, at once; nothing is sent.
    Local,
    /// Nitro but not owned: Discord rejects it and local picks are off with Nitro.
    Preview,
}

fn use_of(nitro: bool, owned: bool) -> Use {
    match (owned, nitro) {
        (true, _) => Use::Account,
        (false, false) => Use::Local,
        (false, true) => Use::Preview,
    }
}

/// Picker order [MOB] (11 = the UI font). Fonts that are not bundled draw with the UI font.
const FONTS: [(u32, &str); 12] = [
    (11, "Default"),
    (12, "Tempo"),
    (3, "Sakura"),
    (4, "Jellybean"),
    (6, "Modern"),
    (7, "Medieval"),
    (8, "8Bit"),
    (10, "Vampyre"),
    (13, "Monkey Bars"),
    (14, "Mainframe"),
    (15, "Headbang"),
    (16, "Journal"),
];
/// (id, name, colours it takes).
const EFFECTS: [(u32, &str, usize); 8] =
    [(1, "Solid", 1), (2, "Gradient", 2), (3, "Neon", 1), (4, "Toon", 1), (5, "Pop", 1), (6, "Glow", 2), (7, "Prism", 5), (8, "Gummy", 4)];
const SOLID: [u32; 6] = [0x24E36D, 0x1C98EB, 0xC32AFF, 0xDA0149, 0xF93722, 0xC6A615];
const GRADIENT: [[u32; 2]; 5] = [[0x2AAEA6, 0xFFC490], [0xE45EFF, 0x267DE0], [0x7E4FCB, 0x67CB98], [0xEFB4AA, 0xE46CE2], [0xF5983C, 0xE55AE7]];
const GUMMY: [[u32; 4]; 7] = [
    [0xE9A9D5, 0xA9DDF0, 0xB9A7EE, 0xB9EACB],
    [0xFF6FC2, 0xF54FA8, 0xFF9BD6, 0xD96AB7],
    [0xFFB57A, 0xFF936F, 0xFF7C78, 0xF06B8A],
    [0xC4ED65, 0x98DE60, 0x6CCD63, 0x45BC66],
    [0x7CB8FF, 0x8EE0D0, 0x9D94F5, 0x66CFF0],
    [0xD77BFF, 0xB56BFF, 0xE45CE5, 0x8B6CFF],
    [0xD83A5F, 0xC94BCB, 0x8B5CFF, 0xB06CFF],
];
const PRISM: [[u32; 5]; 3] =
    [[0xFE9242, 0x35ED7E, 0xFFE047, 0xFF4CD2, 0x5865F2], [0x7EE7D8, 0x7CCBFF, 0x7F8CFF, 0xB56DFF, 0xF38AD8], [0x7A0906, 0xC81E12, 0xF45A18, 0xFF9500, 0xFFD447]];
/// Profile theme presets: first / last stop of the app gradient themes (COSMETICS_NOTES §13.2).
const THEMES: [(&str, [u32; 2]); 8] = [
    ("Mint Apple", [0x56B69F, 0x9ECA67]),
    ("Cotton Candy", [0xF4ABB8, 0xB1C2FC]),
    ("Sunset", [0x48288C, 0xDB7F4B]),
    ("Chroma Glow", [0x0EB5BF, 0x218BE0]),
    ("Crimson Moon", [0x950909, 0x000000]),
    ("Midnight Blurple", [0x5348CA, 0x140730]),
    ("Aurora", [0x062053, 0x051A81]),
    ("Strawberry Lemonade", [0xAF1A6C, 0xE7A525]),
];
/// Discord's API takes at most this many display-name colours (§7).
const API_COLORS: usize = 2;
const PREVIEW_W: f32 = 280.0;

/// My account's real profile cosmetics (my loaded profile; the user ones are on `state.me`).
#[derive(Default)]
struct Mine {
    /// `user_profile.collectibles` once my profile loaded (a profile write sends the full list).
    items: Option<Vec<ProfileCollectible>>,
    effect: Option<Id>,
    frame: Option<Id>,
    theme: Option<[u32; 2]>,
}

fn mine(cx: &UiCx<'_>, me: Id) -> Mine {
    profile_modal::loaded(cx, me, None).map_or_else(Mine::default, |p| mine_of(p, chrono::Utc::now().timestamp()))
}

/// `items` is what a profile write keeps: the live entries, plus an effect my profile reports
/// only in the legacy shape (else equipping a frame would unequip it).
fn mine_of(p: &UserProfile, now: i64) -> Mine {
    let effect = p.collectible_sku(1, now);
    let mut items: Vec<ProfileCollectible> = p.user_profile.collectibles.iter().filter(|c| c.expires_at.is_none_or(|t| t > now)).cloned().collect();
    if let Some(e) = effect.filter(|e| !items.iter().any(|c| c.sku_id == *e)) {
        items.push(ProfileCollectible { sku_id: e, kind: 1, expires_at: None });
    }
    Mine { items: Some(items), effect, frame: p.collectible_sku(3, now), theme: p.theme_colors().map(|(a, b)| [a, b]) }
}

pub fn show(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    let Some(me) = cx.state.me.clone() else { return };
    let mut f = std::mem::take(&mut cx.view.user_settings_ui.cosmetics);
    let pass = ui.ctx().cumulative_pass_nr();
    if f.pass.is_none_or(|p| p + 1 != pass)
        && let Some(saved) = request(cx, true, false)
    {
        f.cached = saved;
    }
    f.pass = Some(pass);
    // My global profile: the real effect / frame / theme and the list a profile write keeps.
    // Every pass: an Apply drops it (`request_profile` sends only when nothing is cached or loading).
    profile_modal::fetch(cx, me.id, None);
    let nitro = overrides::full_nitro(&me);
    let pal = Pal::new(cx.theme);
    let mine = mine(cx, me.id);
    hint(ui, cx.theme, if nitro { "From Discord's shop: apply what you own to your account." } else { "From Discord's shop. Without Nitro your picks are only visible to you." });
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        for t in Tab::ALL {
            if ui.selectable_label(f.tab == t, t.label()).clicked() && f.tab != t {
                // Only a dropped effect / frame draft changes what the preview's overlay shows.
                let drop_fx = f.draft.is_some() && matches!(f.tab, Tab::Effect | Tab::Frame);
                (f.tab, f.category, f.draft, f.style, f.theme) = (t, None, None, None, None);
                if drop_fx {
                    display::release(ui.ctx(), cx.images);
                }
                drop_thumbs(ui.ctx(), cx.images);
            }
        }
    });
    ui.add_space(10.0);
    if ui.available_width() >= 640.0 {
        ui.horizontal_top(|ui| {
            let w = ui.available_width() - PREVIEW_W - 24.0;
            ui.vertical(|ui| {
                ui.set_width(w);
                content(ui, cx, &pal, &mut f, &me, &mine);
            });
            ui.add_space(24.0);
            ui.vertical(|ui| {
                ui.set_width(PREVIEW_W);
                side(ui, cx, &pal, &mut f, &me, &mine);
            });
        });
    } else {
        side(ui, cx, &pal, &mut f, &me, &mine);
        ui.add_space(12.0);
        content(ui, cx, &pal, &mut f, &me, &mine);
    }
    // A colour drag saved once, on release.
    if f.dirty && !ui.input(|i| i.pointer.any_down()) {
        f.dirty = false;
        commit(cx);
    }
    cx.view.user_settings_ui.cosmetics = f;
}

/// The page closed (settings closed or another page): frees the preview's effect / frame /
/// banner and the grid's thumbnails instead of waiting for the idle sweep.
pub(super) fn leave(ctx: &egui::Context, cx: &mut UiCx<'_>) {
    display::release(ctx, cx.images);
    drop_thumbs(ctx, cx.images);
    if let Some(url) = cx.view.user_settings_ui.cosmetics.banner.take() {
        cx.images.remove(&url);
    }
}

/// Catalog thumbnails drawn since the page opened or the tab changed (not my avatar: shared).
fn thumbs_id() -> egui::Id {
    egui::Id::unique("rc_cosmetics_thumbs")
}

fn remember_thumb(ctx: &egui::Context, url: &str) {
    ctx.data_mut(|d| {
        let set = d.get_temp_mut_or_default::<std::collections::HashSet<String>>(thumbs_id());
        if !set.contains(url) {
            set.insert(url.to_owned());
        }
    });
}

fn drop_thumbs(ctx: &egui::Context, images: &mut images::ImageCache) {
    for url in ctx.data_mut(|d| d.remove_temp::<std::collections::HashSet<String>>(thumbs_id())).unwrap_or_default() {
        images.remove(&url);
    }
}

/// The catalog until it loaded (its 24 h saved copy, else one request; `fresh`: ask Discord
/// anyway), my owned items each time the page `opened` (else only after a failure: Retry).
/// Returns where a (re)loaded catalog comes from: Some(true) the saved copy, Some(false) Discord.
fn request(cx: &mut UiCx<'_>, opened: bool, fresh: bool) -> Option<bool> {
    let s = &mut cx.state.social;
    let mut source = None;
    // One catalog request at a time (reopening the page while it loads asks nothing again).
    if fresh || (!s.catalog_requested && !matches!(s.collectibles, Some(Ok(_)))) {
        let saved = if fresh { None } else { collectibles::read_cache() };
        source = Some(saved.is_some());
        match saved {
            Some(items) => s.collectibles = Some(Ok(items)),
            None => {
                s.collectibles = None;
                s.catalog_requested = true;
                cx.cmd.send(Command::Social(SocialCommand::FetchCollectibles));
            }
        }
    }
    if opened || !matches!(s.owned_collectibles, Some(Ok(_))) {
        s.owned_collectibles = None;
        cx.cmd.send(Command::Social(SocialCommand::FetchOwnedCollectibles));
    }
    source
}

fn content(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, f: &mut Form, me: &User, mine: &Mine) {
    let nitro = overrides::full_nitro(me);
    match f.tab.kind() {
        Some(k) => catalog_tab(ui, cx, pal, f, me, mine, k),
        None if f.tab == Tab::NameStyle => name_style(ui, cx, pal, f, nitro),
        None => theme(ui, cx, pal, f, nitro, mine),
    }
}

fn slot(c: &mut LocalCosmetics, k: u8) -> &mut Option<LocalItem> {
    match k {
        kind::DECORATION => &mut c.decoration,
        kind::NAMEPLATE => &mut c.nameplate,
        kind::EFFECT => &mut c.effect,
        _ => &mut c.frame,
    }
}

fn to_local(i: &CatalogItem) -> LocalItem {
    LocalItem { sku_id: Some(i.sku_id), asset: i.asset.clone(), palette: i.palette.clone(), label: i.name.clone() }
}

fn is_owned(cx: &UiCx<'_>, sku: Id) -> bool {
    matches!(&cx.state.social.owned_collectibles, Some(Ok(items)) if items.iter().any(|i| i.sku_id == sku))
}

/// The SKU my account shows in slot `k` (live items only).
fn real_sku(me: &User, mine: &Mine, k: u8) -> Option<Id> {
    let live = |e: Option<i64>| e.is_none_or(|t| t > chrono::Utc::now().timestamp());
    match k {
        kind::DECORATION => me.avatar_decoration_data.as_ref().filter(|d| live(d.expires_at)).and_then(|d| d.sku_id),
        kind::NAMEPLATE => me.collectibles.as_ref()?.nameplate.as_ref().filter(|n| live(n.expires_at)).and_then(|n| n.sku_id),
        kind::EFFECT => mine.effect,
        _ => mine.frame,
    }
}

/// The SKU the preview shows in slot `k`: the clicked tile, else my local pick, else my real one.
fn shown_sku(f: &Form, me: &User, mine: &Mine, k: u8) -> Option<Id> {
    if let Some(d) = f.draft.as_ref().filter(|_| f.tab.kind() == Some(k)) {
        return d.as_ref().map(|i| i.sku_id);
    }
    match overrides::my_cosmetics(me.id).and_then(|mut c| slot(&mut c, k).take()) {
        Some(local) => local.sku_id,
        None => real_sku(me, mine, k),
    }
}

/// "You're offline …" for the backend's logged-out answer, else the error.
fn load_error(what: &str, e: &str) -> String {
    if e == "Not connected" { format!("You're offline: {what} loads once you're connected.") } else { format!("Couldn't load {what} ({e}).") }
}

/// Status line, search, category filter, then the grid: my owned items first (also ones the shop
/// no longer lists), then the shop's, "None" in front.
fn catalog_tab(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, f: &mut Form, me: &User, mine: &Mine, k: u8) {
    let nitro = overrides::full_nitro(me);
    let shown = shown_sku(f, me, mine, k);
    // Taken out for the frame so the grid can borrow them while `cx` is used mutably.
    let catalog = cx.state.social.collectibles.take();
    let owned_list = cx.state.social.owned_collectibles.take();
    let owned: &[CatalogItem] = match &owned_list {
        Some(Ok(items)) => items,
        _ => &[],
    };
    let mut all: Vec<&CatalogItem> = owned.iter().filter(|i| i.kind == k).collect();
    if let Some(Ok(items)) = &catalog {
        all.extend(items.iter().filter(|i| i.kind == k && !owned.iter().any(|o| o.sku_id == i.sku_id)));
    }
    let (mut retry, mut refresh) = (false, false);
    let status = match (&catalog, &owned_list) {
        (None, _) => None,
        (Some(Err(e)), _) => Some(load_error("Discord's catalog", e)),
        (_, Some(Err(e))) => Some(load_error("your collection", e)),
        _ => Some(String::new()),
    };
    match &status {
        None => {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0).color(pal.muted));
                ui.label(RichText::new("Loading Discord's catalog…").size(pal.meta()).color(pal.muted));
            });
        }
        Some(text) if !text.is_empty() => {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(text).size(pal.meta()).color(pal.muted));
                retry = ui.small_button("Retry").clicked();
            });
        }
        _ => {}
    }
    let mut categories: Vec<&str> = Vec::new();
    for i in &all {
        if !categories.contains(&i.category.as_str()) {
            categories.push(&i.category);
        }
    }
    let (loaded, searchable) = (matches!(&catalog, Some(Ok(_))), !all.is_empty());
    if searchable {
        ui.horizontal(|ui| {
            ui.add(TextEdit::singleline(&mut f.search).hint_text("Search").desired_width(180.0));
            if categories.len() > 1 {
                let label = f.category.clone().unwrap_or_else(|| "All categories".into());
                egui::ComboBox::from_id_salt("rc_cosmetics_category").selected_text(label).width(180.0).icon(super::combo_icon).show_ui(ui, |ui| {
                    ui.selectable_value(&mut f.category, None, "All categories");
                    for c in &categories {
                        ui.selectable_value(&mut f.category, Some((*c).to_owned()), *c);
                    }
                });
            }
        });
        ui.add_space(6.0);
    }
    let needle = f.search.trim().to_lowercase();
    let list: Vec<&CatalogItem> = all
        .into_iter()
        .filter(|i| f.category.as_deref().is_none_or(|c| i.category == c))
        .filter(|i| needle.is_empty() || i.name.to_lowercase().contains(&needle) || i.category.to_lowercase().contains(&needle))
        .collect();
    if list.is_empty() && loaded {
        let what = kind_name(k);
        // Only Discord's answer this session (and my collection with it) says the shop has none.
        let live = !f.cached && matches!(&owned_list, Some(Ok(_)));
        ui.horizontal_wrapped(|ui| {
            let text = if !needle.is_empty() && searchable {
                format!("No {what} match \"{}\"", f.search.trim())
            } else if live {
                format!("No {what} in Discord's shop right now")
            } else {
                format!("No {what} loaded yet")
            };
            ui.label(RichText::new(text).size(pal.meta()).color(pal.muted));
            // The status line above has its own Retry.
            if !live && needle.is_empty() && status.as_deref() == Some("") {
                refresh = ui.small_button("Retry").clicked();
            }
        });
        ui.add_space(6.0);
    }
    // "None" always leads: it hides my item even when the shop lists none.
    let pick = grid(ui, cx, pal, k, &list, shown, owned);
    if let Some(p) = pick {
        f.draft = Some(p.cloned());
        let owned_pick = p.is_some_and(|i| owned.iter().any(|o| o.sku_id == i.sku_id));
        // Until my collection answered an owned item would read as not owned: preview only.
        if use_of(nitro, owned_pick) == Use::Local && owned_list.is_some() {
            *slot(&mut cx.overrides.cosmetics, k) = Some(p.map(to_local).unwrap_or_default());
            commit(cx);
        }
        // The previous preview's effect / frame textures (other tabs leave the overlay as is).
        if matches!(k, kind::EFFECT | kind::FRAME) {
            display::release(ui.ctx(), cx.images);
        }
    }
    cx.state.social.collectibles = catalog;
    cx.state.social.owned_collectibles = owned_list;
    if (retry || refresh)
        && let Some(saved) = request(cx, false, refresh)
    {
        f.cached = saved;
    }
}

/// Tiles in rows (only visible rows are laid out; only visible tiles load images). Returns the
/// clicked tile (`Some(None)`: "None").
fn grid<'a>(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, k: u8, items: &[&'a CatalogItem], shown: Option<Id>, owned: &[CatalogItem]) -> Option<Option<&'a CatalogItem>> {
    let wide = k == kind::NAMEPLATE;
    let tile = if wide { vec2(232.0, 66.0) } else { vec2(96.0, 104.0) };
    let gap = 8.0;
    let cols = (((ui.available_width() + gap) / (tile.x + gap)).floor() as usize).max(1);
    let rows = (items.len() + 1).div_ceil(cols);
    let mut picked = None;
    ScrollArea::vertical().id_salt(("rc_cosmetics_grid", k)).max_height(420.0).auto_shrink([false, true]).show_rows(ui, tile.y, rows, |ui, range| {
        ui.spacing_mut().item_spacing = vec2(gap, gap);
        for row in range {
            ui.horizontal(|ui| {
                for n in row * cols..((row + 1) * cols).min(items.len() + 1) {
                    let item = n.checked_sub(1).map(|i| items[i]);
                    let selected = item.map(|i| i.sku_id) == shown;
                    let mine = item.is_some_and(|i| owned.iter().any(|o| o.sku_id == i.sku_id));
                    if tile_ui(ui, cx, pal, k, tile, item, selected, mine).clicked() {
                        picked = Some(item);
                    }
                }
            });
        }
    });
    picked
}

#[allow(clippy::too_many_arguments, reason = "one call site: a tile's whole state")]
fn tile_ui(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, k: u8, size: Vec2, item: Option<&CatalogItem>, selected: bool, owned: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let name = item.map_or("None", |i| i.name.as_str());
    let resp = resp.on_hover_text(match item {
        Some(_) if owned => format!("{name}\nIn your collection"),
        Some(i) if !i.category.is_empty() => format!("{name}\n{}", i.category),
        _ => name.to_owned(),
    });
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let hot = resp.hovered();
    let p = ui.painter().clone();
    p.rect_filled(rect, pal.radius, if hot { pal.hover } else { pal.tint() });
    let art = Rect::from_min_max(rect.min + vec2(6.0, 6.0), pos2(rect.max.x - 6.0, rect.max.y - 22.0));
    match item {
        None => {
            let r = art.height().min(art.width()) * 0.28;
            let c = art.center();
            p.circle_stroke(c, r, Stroke::new(2.0, pal.muted));
            p.line_segment([c + vec2(-r, r) * 0.7, c + vec2(r, -r) * 0.7], Stroke::new(2.0, pal.muted));
        }
        Some(i) => art_ui(ui, cx, pal, k, art, i, hot),
    }
    let g = one_line(ui, name, FontId::proportional(pal.meta()), if selected { pal.text } else { pal.muted }, rect.width() - 8.0);
    p.galley(pos2(rect.center().x - g.size().x / 2.0, rect.max.y - 19.0), g, pal.text);
    if owned {
        let c = pos2(rect.right() - 10.0, rect.top() + 10.0);
        // Owned: a check on the primary (a state, with its shape).
        p.circle_filled(c, 7.0, pal.tk.accent);
        super::super::icons::paint(&p, super::super::icons::Icon::Check, Rect::from_center_size(c, Vec2::splat(10.0)), pal.tk.on_accent, pal.tk.accent);
    }
    if selected {
        p.rect_stroke(rect, pal.radius, Stroke::new(2.0, pal.text), egui::StrokeKind::Inside);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The tile picture, drawn by the shared renderers so it looks as it will: decorations around
/// my avatar, nameplates as a row background, effects / frames as their shop preview. Static
/// until hovered.
fn art_ui(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, k: u8, art: Rect, i: &CatalogItem, hot: bool) {
    match k {
        kind::NAMEPLATE => {
            let np = Nameplate { asset: i.asset.clone(), palette: i.palette.clone(), ..Default::default() };
            // No text on the tile: the whole art, light themes too.
            ui.painter().add(display::nameplate(ui, cx.images, art, &np, hot, false, pal.radius, f32::NEG_INFINITY));
            if let Some(url) = crate::api::cdn::nameplate_url(&i.asset, hot) {
                remember_thumb(ui.ctx(), &url);
            }
        }
        kind::DECORATION => {
            let avatar = Rect::from_center_size(art.center(), Vec2::splat(art.height() * 0.75));
            // Painted, not an avatar widget: that would take the tile's clicks.
            if let Some(me) = cx.state.me.as_ref() {
                let url = overrides::my_avatar_url(me.id).unwrap_or_else(|| images::user_avatar_url(None, me, None, 64, false));
                let side = (avatar.width() * ui.ctx().pixels_per_point()).ceil() as u32;
                match cx.images.get(ui.ctx(), &url, side, false) {
                    Some(tex) => egui::Image::new(tex).corner_radius(avatar.width() / 2.0).paint_at(ui, avatar),
                    None => images::paint_initial(ui.painter(), avatar, avatar.width() / 2.0, me.id.0, me.display_name(), pal.dark),
                }
            }
            let d = DecorationData { asset: i.asset.clone(), ..Default::default() };
            if let Some(url) = images::decoration_at(ui, cx.images, avatar, Some(&d), hot, 1.0) {
                remember_thumb(ui.ctx(), &url);
            }
        }
        _ => {
            let px = (art.width().max(art.height()) * ui.ctx().pixels_per_point()).ceil() as u32;
            let Some(still) = i.thumb_url(px, false) else { return };
            // Hovered: the animated variant once it is loaded, the still one until then.
            let moving = i.thumb_url(px, true).filter(|u| hot && *u != still);
            for u in std::iter::once(&still).chain(&moving) {
                remember_thumb(ui.ctx(), u);
            }
            let tex = moving.and_then(|u| cx.images.get(ui.ctx(), &u, px, true));
            let Some(tex) = tex.or_else(|| cx.images.get(ui.ctx(), &still, px, hot)) else { return };
            let scale = (art.width() / tex.size.x.max(1.0)).min(art.height() / tex.size.y.max(1.0));
            egui::Image::from_texture(tex).paint_at(ui, Rect::from_center_size(art.center(), tex.size * scale));
        }
    }
}

fn from_style(s: &NameStyle) -> LocalNameStyle {
    LocalNameStyle { font_id: s.font_id.unwrap_or(11), effect_id: s.effect_id.unwrap_or(1), colors: s.colors.clone() }
}

/// No colours = plain name.
fn to_style(s: &LocalNameStyle) -> Option<NameStyle> {
    (!s.colors.is_empty()).then(|| NameStyle { font_id: Some(s.font_id), effect_id: Some(s.effect_id), colors: s.colors.clone() })
}

/// Font, effect and colours of my display name (Nitro: a draft for "Apply to account";
/// otherwise my local style: shown at once, saved when the mouse is released).
fn name_style(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, f: &mut Form, nitro: bool) {
    let real = cx.state.me.as_ref().and_then(|m| m.display_name_styles.as_deref()).map(from_style);
    let current = if nitro { f.style.clone().or(real) } else { cx.overrides.cosmetics.name_style.clone() };
    let mut s = current.unwrap_or_default();
    let effect_colors = |e: u32| EFFECTS.iter().find(|x| x.0 == e).map_or(1, |x| x.2);
    let mut changed = false;
    label(ui, pal, "Font");
    ui.horizontal_wrapped(|ui| {
        // shortcut: Discord serves these fonts only as WOFF2 from discord.com/assets (hashed
        // names; no WOFF2 decoder in the tree), so the ones Rustcord lacks are dimmed. Load them
        // once a decoder (brotli) is approved.
        let button = egui::TextStyle::Button.resolve(ui.style());
        for (id, name) in FONTS {
            // Each in its own face where Rustcord has it; the others draw in the UI font.
            let resp = match display::font_face(id) {
                Some(face) => {
                    let job = on_ui_baseline(ui, name, FontId::new(button.size, face), &button);
                    ui.selectable_value(&mut s.font_id, id, job)
                }
                None => {
                    let text = if s.font_id == id { RichText::new(name) } else { RichText::new(name).color(pal.muted.gamma_multiply(0.6)) };
                    ui.selectable_value(&mut s.font_id, id, text).on_hover_text("Font not available in Rustcord")
                }
            };
            changed |= resp.changed();
        }
    });
    label(ui, pal, "Effect");
    ui.horizontal_wrapped(|ui| {
        for (id, name, n) in EFFECTS {
            if ui.selectable_label(s.effect_id == id, name).clicked() && s.effect_id != id {
                s.effect_id = id;
                // Keep my colours when the count fits, else start from this effect's first preset.
                if s.colors.len() != n {
                    s.colors = presets_for(id).into_iter().next().unwrap_or_default();
                }
                changed = true;
            }
        }
    });
    label(ui, pal, "Colours");
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        for p in presets_for(s.effect_id) {
            if swatch(ui, pal, &p, s.colors == p).clicked() {
                s.colors = p;
                changed = true;
            }
        }
    });
    ui.horizontal(|ui| {
        let n = effect_colors(s.effect_id);
        s.colors.resize(n, *s.colors.last().unwrap_or(&SOLID[0]));
        for c in s.colors.iter_mut() {
            let mut v = [(*c >> 16) as u8, (*c >> 8) as u8, *c as u8];
            if ui.color_edit_button_srgb(&mut v).on_hover_text("Custom colour").changed() {
                *c = u32::from(v[0]) << 16 | u32::from(v[1]) << 8 | u32::from(v[2]);
                changed = true;
            }
        }
    });
    if !changed {
        return;
    }
    if nitro {
        f.style = Some(s);
    } else {
        cx.overrides.cosmetics.name_style = Some(s);
        overrides::publish(cx.state.me.as_ref(), cx.overrides);
        f.dirty = true;
    }
}

/// `text` in `font`, as tall as it is in `ui_font` and with its baseline where `ui_font`'s
/// would be (chips in a row are centred by their line boxes: a face whose box has more room
/// under the baseline, like the monospace stand-ins, would sit higher). Measured from the laid
/// out text: the section's line height `l`, bottom-aligned in the row and centred in itself,
/// puts the baseline at `b - h/2 + H - l/2`; that equals `B` for `l = 2(b + H - B) - h`.
fn on_ui_baseline(ui: &egui::Ui, text: &str, font: FontId, ui_font: &FontId) -> egui::text::LayoutJob {
    let measure = |f: &FontId| {
        let g = ui.painter().layout_no_wrap(text.to_owned(), f.clone(), egui::Color32::PLACEHOLDER);
        let base = g.rows.first().and_then(|r| r.glyphs.first().map(|gl| r.pos.y + gl.pos.y)).unwrap_or(g.size().y);
        (base, g.size().y)
    };
    let ((b, h), (big_b, big_h)) = (measure(&font), measure(ui_font));
    let line_height = Some((2.0 * (b + big_h - big_b) - h).max(1.0));
    let format = egui::TextFormat { font_id: font, line_height, color: egui::Color32::PLACEHOLDER, ..Default::default() };
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), format);
    job.first_row_min_height = big_h;
    job
}

fn presets_for(effect: u32) -> Vec<Vec<u32>> {
    match effect {
        2 | 6 => GRADIENT.iter().map(|p| p.to_vec()).collect(),
        7 => PRISM.iter().map(|p| p.to_vec()).collect(),
        8 => GUMMY.iter().map(|p| p.to_vec()).collect(),
        _ => SOLID.iter().map(|c| vec![*c]).collect(),
    }
}

/// Profile card colours: presets and two pickers (Nitro: a draft for "Apply to account";
/// otherwise my local theme: shown at once, saved when the mouse is released).
fn theme(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, f: &mut Form, nitro: bool, mine: &Mine) {
    let current = if nitro { f.theme.or(mine.theme) } else { cx.overrides.cosmetics.theme_colors };
    let mut c = current.unwrap_or([0x2B2D31, 0x1E1F22]);
    let mut changed = false;
    label(ui, pal, "Presets");
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        for (name, p) in THEMES {
            if swatch(ui, pal, &p, current == Some(p)).on_hover_text(name).clicked() {
                c = p;
                changed = true;
            }
        }
    });
    ui.horizontal(|ui| {
        for (i, what) in ["Primary", "Accent"].into_iter().enumerate() {
            let mut v = [(c[i] >> 16) as u8, (c[i] >> 8) as u8, c[i] as u8];
            if ui.color_edit_button_srgb(&mut v).on_hover_text(what).changed() {
                c[i] = u32::from(v[0]) << 16 | u32::from(v[1]) << 8 | u32::from(v[2]);
                changed = true;
            }
            ui.label(RichText::new(what).size(pal.meta()).color(pal.muted));
        }
    });
    if !changed {
        return;
    }
    if nitro {
        f.theme = Some(c);
    } else {
        cx.overrides.cosmetics.theme_colors = Some(c);
        overrides::publish(cx.state.me.as_ref(), cx.overrides);
        f.dirty = true;
    }
}

/// Small rounded swatch of 1–5 colours side by side.
fn swatch(ui: &mut egui::Ui, pal: &Pal, colors: &[u32], selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(36.0, 24.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let n = colors.len().max(1) as f32;
        for (i, c) in colors.iter().enumerate() {
            let w = rect.width() / n;
            let r = Rect::from_min_size(pos2(rect.left() + w * i as f32, rect.top()), vec2(w, rect.height()));
            ui.painter().rect_filled(r, 0.0, rgb(*c));
        }
        let stroke = if selected { Stroke::new(2.0, pal.text) } else { Stroke::new(1.0, pal.tk.rule) };
        ui.painter().rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A field label: the label role (13/600, muted, sentence case as written).
fn label(ui: &mut egui::Ui, pal: &Pal, text: &str) {
    ui.add_space(8.0 - ui.spacing().item_spacing.y);
    ui.label(RichText::new(text).font(super::super::semibold(pal.label())).color(pal.muted));
}

// ---------------------------------------------------------------------------
// Preview and account actions
// ---------------------------------------------------------------------------

/// What the preview card draws.
struct Look {
    deco: Option<DecorationData>,
    plate: Option<Nameplate>,
    style: Option<NameStyle>,
    theme: Option<(u32, u32)>,
    effect: Option<Id>,
    frame: Option<Id>,
}

/// My card as others will see it (or, without Nitro, as I will): my effective cosmetics with
/// this tab's draft on top.
fn look(f: &Form, me: &User, mine: &Mine) -> Look {
    let fx = overrides::effective_for(me, None);
    let local = overrides::my_cosmetics(me.id).unwrap_or_default();
    let item = |l: Option<LocalItem>, real: Option<Id>| match l {
        Some(i) => i.sku_id,
        None => real,
    };
    let mut l = Look {
        deco: fx.decoration,
        plate: fx.nameplate,
        style: fx.name_style,
        theme: local.theme_colors.or(mine.theme).map(|[a, b]| (a, b)),
        effect: item(local.effect, mine.effect),
        frame: item(local.frame, mine.frame),
    };
    if let Some(d) = &f.draft {
        match f.tab {
            Tab::Decoration => l.deco = d.as_ref().map(|i| DecorationData { asset: i.asset.clone(), sku_id: Some(i.sku_id), expires_at: None }),
            Tab::Nameplate => {
                l.plate = d.as_ref().map(|i| Nameplate { asset: i.asset.clone(), sku_id: Some(i.sku_id), palette: i.palette.clone(), label: Some(i.name.clone()), expires_at: None })
            }
            Tab::Effect => l.effect = d.as_ref().map(|i| i.sku_id),
            Tab::Frame => l.frame = d.as_ref().map(|i| i.sku_id),
            Tab::NameStyle | Tab::Theme => {}
        }
    }
    if let Some(s) = &f.style {
        l.style = to_style(s);
    }
    if let Some([a, b]) = f.theme {
        l.theme = Some((a, b));
    }
    l
}

fn side(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, f: &mut Form, me: &User, mine: &Mine) {
    f.banner = preview_card(ui, cx, pal, me, &look(f, me, mine));
    ui.add_space(12.0);
    actions(ui, cx, pal, f, me, mine);
}

/// My profile card in small: banner, avatar with its decoration, styled name, a member-list row
/// with the nameplate, the theme behind it, the effect over it and the frame around it. Returns
/// the banner's URL.
fn preview_card(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, me: &User, l: &Look) -> Option<String> {
    // A themed card in its own colours and mode, like my real card (cosmetics::theme_pal).
    let pal = &l.theme.map_or(*pal, |t| display::theme_pal(pal, t));
    let effect = l.effect.and_then(|s| display::product_item(cx, s, 1));
    let frame = l.frame.and_then(|s| display::product_item(cx, s, 3));
    let full = ui.available_width().min(PREVIEW_W);
    // The frame's overflow (§6) stays inside the column.
    let k = |fr: &crate::model::CollectibleItem| full / fr.inner_width.filter(|w| *w > 0.0).unwrap_or(full);
    let oh = frame.as_ref().map_or(0.0, |fr| fr.overflow_horizontal.unwrap_or(0.0) * k(fr));
    let ot = frame.as_ref().map_or(0.0, |fr| fr.overflow_top.unwrap_or(0.0) * k(fr));
    ui.add_space(ot);
    let w = full - 2.0 * oh;
    let (banner_h, avatar) = (64.0, 72.0);
    let h = banner_h + avatar / 2.0 + 8.0 + 26.0 + 20.0 + 10.0 + 44.0 + 14.0;
    let behind = ui.painter().add(Shape::Noop);
    let (outer, resp) = ui.allocate_exact_size(vec2(full, h), Sense::hover());
    let card = Rect::from_min_size(pos2(outer.left() + oh, outer.top()), vec2(w, h));
    let radius = pal.card_radius();
    let banner = Rect::from_min_size(card.min, vec2(w, banner_h));
    match l.theme {
        Some(c) => {
            ui.painter().add(display::theme_shapes(ui.ctx(), "cosmetics-preview", card, radius, c, banner.bottom(), pal.dark));
        }
        None => {
            ui.painter().rect(card, radius, pal.surface, Stroke::new(1.0, pal.tk.rule), egui::StrokeKind::Inside);
        }
    }
    // No banner colour: the card itself (profile::banner), never a flat grey block.
    let fill = l.theme.map(|(a, _)| rgb(a)).or(me.accent_color.map(rgb)).unwrap_or(pal.surface);
    let top = CornerRadius { nw: radius, ne: radius, sw: 0, se: 0 };
    // My uploaded (or local) banner fades into the card, as on my profile card.
    let (banner_url, _) = crate::ui::profile::banner(cx, me, None, true);
    match banner_url.as_ref().and_then(|url| cx.images.get(ui.ctx(), url, 600, false)) {
        Some(tex) => crate::ui::profile::paint_banner(ui.painter(), banner, top, tex, l.theme.map(|_| display::theme_veil(pal.dark))),
        None => {
            ui.painter().rect_filled(banner, top, fill);
        }
    }
    let hovered = resp.hovered();
    let anim = display::animates(cx, hovered);
    let ring = l.theme.map_or(pal.surface, |(a, _)| display::theme_ring(a, pal.dark));
    let center = pos2(card.left() + 16.0 + avatar / 2.0, banner.bottom());
    ui.painter().circle_filled(center, avatar / 2.0 + 5.0, ring);
    let av = Rect::from_center_size(center, Vec2::splat(avatar));
    let url = overrides::my_avatar_url(me.id).map_or_else(|| images::user_avatar_url(None, me, None, 160, true), |u| overrides::large(&u));
    let av_anim = overrides::my_avatar_animates(me.id, hovered).unwrap_or(anim);
    user_avatar_at(ui, cx.images, &url, av, av_anim, 1.0, (me.id, me.display_name()));
    images::decoration_at(ui, cx.images, av, l.deco.as_ref(), anim, 1.0);
    let left = card.left() + 16.0;
    let mut y = av.bottom() + 8.0;
    let g = display::name_galley(ui, me.display_name(), FontId::new(20.0, crate::theme::bold_family()), l.style.as_ref(), pal.text, w - 32.0);
    let gh = g.size().y;
    display::paint_name(ui.painter(), pos2(left, y), g, l.style.as_ref(), pal.text, pal.dark);
    y += gh + 2.0;
    let handle = one_line(ui, me.username.as_str(), FontId::proportional(pal.meta()), pal.muted, w - 32.0);
    ui.painter().galley(pos2(left, y), handle, pal.muted);
    y += 28.0;
    // How the member list / DM list shows me.
    let row = Rect::from_min_size(pos2(card.left() + 8.0, y), vec2(w - 16.0, 44.0));
    ui.painter().rect_filled(row, pal.radius, pal.bg.gamma_multiply(0.7));
    let small = Rect::from_min_size(pos2(row.left() + 8.0, row.center().y - 16.0), Vec2::splat(32.0));
    let rg = display::name_galley(ui, me.display_name(), FontId::proportional(pal.size), l.style.as_ref(), pal.text, row.right() - small.right() - 20.0);
    if let Some(np) = &l.plate {
        ui.painter().add(display::nameplate(ui, cx.images, row, np, anim, false, pal.radius, small.right() + 10.0 + rg.size().x));
    }
    let small_url = overrides::my_avatar_url(me.id).unwrap_or_else(|| images::user_avatar_url(None, me, None, 64, false));
    user_avatar_at(ui, cx.images, &small_url, small, false, 1.0, (me.id, me.display_name()));
    images::decoration_at(ui, cx.images, small, l.deco.as_ref(), false, 1.0);
    let ry = row.center().y - rg.size().y / 2.0;
    display::paint_name(ui.painter(), pos2(small.right() + 10.0, ry), rg, l.style.as_ref(), pal.text, pal.dark);
    if let Some(c) = l.theme {
        ui.painter().add(display::theme_outline(card, radius, c));
    }
    if let Some(e) = &effect {
        display::paint_effect(ui, cx.images, card, radius, e, display::shown_for(ui.ctx(), ("cosmetics-preview", e.sku_id)), anim);
    }
    if let Some(fr) = &frame {
        ui.painter().set(behind, Shape::Vec(display::frame_shapes(ui, cx.images, card, fr, true)));
        for s in display::frame_shapes(ui, cx.images, card, fr, false) {
            ui.painter().add(s);
        }
    }
    banner_url
}

/// The buttons of the current tab: "Apply to account" (owned items; with Nitro also the name
/// style / theme), without Nitro "Reset" (drop my local pick) with the "Only visible to you"
/// note, and under a divider "Remove from account" (unequip what my account has; hidden while
/// an item I can't equip is previewed).
fn actions(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, f: &mut Form, me: &User, mine: &Mine) {
    let nitro = overrides::full_nitro(me);
    let (apply, remove, note) = account_actions(cx, f, me, mine);
    let pending = op_state(cx, OpKind::UpdateCosmetics) == Some(OpState::Pending);
    // What renders (Nitro: nothing; SKU-less picks of older builds are ignored).
    let local_set = overrides::my_cosmetics(me.id).is_some_and(|c| local_set(&c, f.tab));
    let mut send = None;
    let mut reset = false;
    ui.horizontal_wrapped(|ui| {
        if let Some(a) = &apply {
            let resp = primary_button(ui, cx.theme, "Apply to account", a.is_ok() && !pending);
            let resp = match a {
                Err(why) => resp.on_disabled_hover_text(*why),
                Ok(_) => resp.on_hover_text("Equip it on your Discord account"),
            };
            if resp.clicked() {
                send = a.clone().ok().map(|p| (p, true));
            }
        }
        if !nitro {
            reset = ui.add_enabled(local_set, egui::Button::new("Reset").min_size(vec2(64.0, 32.0))).on_hover_text("Show your account's own again").clicked();
        }
    });
    if local_set {
        hint(ui, cx.theme, "Only visible to you");
    }
    if let Some(n) = note {
        hint(ui, cx.theme, n);
    }
    // An account write, apart from the picks above so it never reads as "undo my pick".
    if let Some(r) = &remove {
        ui.separator();
        let tip = format!("Unequip {} from your Discord account", remove_what(cx, f.tab, me, mine));
        if ui.add_enabled(!pending, egui::Button::new("Remove from account").min_size(vec2(64.0, 32.0))).on_hover_text(tip).clicked() {
            send = Some((r.clone(), false));
        }
    }
    if f.applying == Some(f.tab) {
        op_line(ui, pal, op_state(cx, OpKind::UpdateCosmetics).as_ref(), "Applying…");
    }
    if reset {
        clear_local(&mut cx.overrides.cosmetics, f.tab);
        f.draft = None;
        commit(cx);
    }
    if let Some((patch, apply)) = send {
        let cmd = AccountCommand::UpdateCosmetics { patch };
        if cx.state.account.begin(cmd.op()) {
            cx.cmd.send(Command::Account(cmd));
            f.applying = Some(f.tab);
            // Equipped for real: my local pick would hide it.
            if apply && local_set {
                clear_local(&mut cx.overrides.cosmetics, f.tab);
                commit(cx);
            }
        }
    }
}

type Apply = Result<CosmeticsPatch, &'static str>;

/// What "Remove from account" unequips (its tooltip): the preview may show another item.
fn remove_what(cx: &UiCx<'_>, tab: Tab, me: &User, mine: &Mine) -> String {
    let named = |sku: Id| {
        let find = |l: &Option<Result<Vec<CatalogItem>, String>>| match l {
            Some(Ok(items)) => items.iter().find(|i| i.sku_id == sku).map(|i| i.name.clone()),
            _ => None,
        };
        find(&cx.state.social.owned_collectibles).or_else(|| find(&cx.state.social.collectibles))
    };
    match tab.kind() {
        Some(k) => real_sku(me, mine, k).and_then(named).unwrap_or_else(|| format!("your current {}", tab.label().to_lowercase())),
        None => format!("your current {}", tab.label().to_lowercase()),
    }
}

/// Plural of a catalog kind ("No profile frames …").
fn kind_name(k: u8) -> &'static str {
    match k {
        kind::DECORATION => "avatar decorations",
        kind::NAMEPLATE => "nameplates",
        kind::EFFECT => "profile effects",
        _ => "profile frames",
    }
}

/// (Apply to account, Remove, a note) for the current tab and draft.
fn account_actions(cx: &UiCx<'_>, f: &Form, me: &User, mine: &Mine) -> (Option<Apply>, Option<CosmeticsPatch>, Option<&'static str>) {
    let nitro = overrides::full_nitro(me);
    match f.tab.kind() {
        Some(k) => {
            let real = real_sku(me, mine, k);
            let remove = real.and_then(|_| equip(k, None, mine).ok());
            let Some(Some(item)) = &f.draft else { return (None, remove, None) };
            match use_of(nitro, is_owned(cx, item.sku_id)) {
                Use::Account => {
                    let apply = equip(k, Some(item.sku_id), mine).and_then(|p| if real == Some(item.sku_id) { Err("Already on your account") } else { Ok(p) });
                    (Some(apply), remove, None)
                }
                // Previewing something I can't equip: no account write next to it.
                Use::Preview => (None, None, Some("Not in your collection: get it in Discord's Shop to use it.")),
                Use::Local => (None, remove, None),
            }
        }
        None if !nitro => (None, None, None),
        None if f.tab == Tab::NameStyle => {
            let real = me.display_name_styles.as_deref().filter(|s| !s.colors.is_empty());
            let remove = real.map(|_| CosmeticsPatch::NameStyle(None));
            let apply = f.style.as_ref().map(|s| match to_style(s) {
                _ if s.colors.len() > API_COLORS => Err("Discord takes at most 2 colours: pick Solid, Gradient, Neon, Toon, Pop or Glow"),
                Some(st) if real.map(from_style).as_ref() != Some(s) => Ok(CosmeticsPatch::NameStyle(Some(st))),
                _ => Err("Already on your account"),
            });
            (apply, remove, None)
        }
        None => {
            let remove = mine.theme.map(|_| CosmeticsPatch::ThemeColors(None));
            let apply = f.theme.map(|t| if mine.theme == Some(t) { Err("Already on your account") } else { Ok(CosmeticsPatch::ThemeColors(Some(t))) });
            (apply, remove, None)
        }
    }
}

/// The write that sets my slot `k` to `sku` (None = unequip). Profile items need my loaded
/// profile: the full list is sent, and a missing one would unequip the other item.
fn equip(k: u8, sku: Option<Id>, mine: &Mine) -> Apply {
    Ok(match k {
        kind::DECORATION => CosmeticsPatch::Decoration(sku),
        kind::NAMEPLATE => CosmeticsPatch::Nameplate(sku),
        _ => CosmeticsPatch::ProfileItems(collectibles::profile_sku_ids(mine.items.as_deref().ok_or("Loading your profile…")?, k as u16, sku)),
    })
}

fn local_set(c: &LocalCosmetics, tab: Tab) -> bool {
    match tab.kind() {
        Some(k) => slot(&mut c.clone(), k).is_some(),
        None if tab == Tab::NameStyle => c.name_style.is_some(),
        None => c.theme_colors.is_some(),
    }
}

fn clear_local(c: &mut LocalCosmetics, tab: Tab) {
    match tab.kind() {
        Some(k) => *slot(c, k) = None,
        None if tab == Tab::NameStyle => c.name_style = None,
        None => c.theme_colors = None,
    }
}

/// Publish to the renderers and save (toast if saving failed).
fn commit(cx: &mut UiCx<'_>) {
    overrides::publish(cx.state.me.as_ref(), cx.overrides);
    if let Err(e) = cx.overrides.save() {
        cx.view.toast(format!("Could not save local cosmetics: {e}"), true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gating() {
        assert_eq!(use_of(false, false), Use::Local, "no Nitro: only in this client");
        assert_eq!(use_of(false, true), Use::Account, "owned: equip for real without Nitro too");
        assert_eq!(use_of(true, true), Use::Account);
        assert_eq!(use_of(true, false), Use::Preview, "Nitro, not owned: never local, never sent");
        // Every effect's presets have the colour count the effect takes.
        for (id, _, n) in EFFECTS {
            assert!(presets_for(id).iter().all(|p| p.len() == n), "effect {id}");
        }
    }

    #[test]
    fn profile_writes_need_my_profile() {
        let item = |sku, kind| ProfileCollectible { sku_id: Id(sku), kind, expires_at: None };
        let mut mine = Mine::default();
        // Not loaded: never a list that would unequip the other item.
        assert!(equip(kind::EFFECT, Some(Id(9)), &mine).is_err());
        assert_eq!(equip(kind::DECORATION, Some(Id(9)), &mine), Ok(CosmeticsPatch::Decoration(Some(Id(9)))));
        mine.items = Some(vec![item(1, 1), item(3, 3)]);
        assert_eq!(equip(kind::EFFECT, Some(Id(9)), &mine), Ok(CosmeticsPatch::ProfileItems(vec![Id(3), Id(9)])));
        assert_eq!(equip(kind::FRAME, None, &mine), Ok(CosmeticsPatch::ProfileItems(vec![Id(1)])));
        // An expired effect entry is not resent; the legacy-only effect is kept.
        let fields = crate::model::ProfileFields {
            profile_effect: Some(crate::model::ProfileEffectRef { id: Id(4), expires_at: None }),
            collectibles: vec![item(3, 3), ProfileCollectible { sku_id: Id(6), kind: 1, expires_at: Some(50) }],
            ..Default::default()
        };
        let mine = mine_of(&UserProfile { user_profile: fields, ..Default::default() }, 100);
        assert_eq!((mine.effect, mine.frame), (Some(Id(4)), Some(Id(3))));
        assert_eq!(equip(kind::FRAME, Some(Id(9)), &mine), Ok(CosmeticsPatch::ProfileItems(vec![Id(4), Id(9)])));
    }

    #[test]
    fn shown_and_real_slots() {
        let me = User {
            id: Id(1),
            avatar_decoration_data: Some(Box::new(DecorationData { asset: "a".into(), sku_id: Some(Id(5)), expires_at: Some(1) })),
            ..Default::default()
        };
        let mine = Mine { frame: Some(Id(8)), ..Default::default() };
        assert_eq!(real_sku(&me, &mine, kind::DECORATION), None, "expired");
        assert_eq!(real_sku(&me, &mine, kind::FRAME), Some(Id(8)));
        let mut f = Form { tab: Tab::Frame, draft: Some(None), ..Default::default() };
        assert_eq!(shown_sku(&f, &me, &mine, kind::FRAME), None, "the None tile");
        f.tab = Tab::Effect;
        assert_eq!(shown_sku(&f, &me, &mine, kind::FRAME), Some(Id(8)), "a draft of another tab does not count");
        let mut c = LocalCosmetics::default();
        assert!(!local_set(&c, Tab::Theme));
        c.theme_colors = Some([1, 2]);
        c.frame = Some(LocalItem::default());
        assert!(local_set(&c, Tab::Theme) && local_set(&c, Tab::Frame) && !local_set(&c, Tab::Effect));
        clear_local(&mut c, Tab::Frame);
        assert_eq!(c.frame, None);
    }

    #[test]
    fn stand_in_face_sits_on_the_ui_baseline() {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(Default::default(), |ui| {
            let base = |g: &egui::Galley| g.rows[0].pos.y + g.rows[0].glyphs[0].pos.y;
            let ui_font = FontId::proportional(16.0);
            let plain = ui.painter().layout_no_wrap("8Bit".into(), ui_font.clone(), egui::Color32::WHITE);
            let mono = ui.painter().layout_job(on_ui_baseline(ui, "8Bit", FontId::monospace(16.0), &ui_font));
            assert_eq!(mono.size().y, plain.size().y, "as tall as the other chips");
            assert!((base(&mono) - base(&plain)).abs() <= 1.0, "baseline {} vs {}", base(&mono), base(&plain));
        });
        out.textures_delta.clear();
    }
}
