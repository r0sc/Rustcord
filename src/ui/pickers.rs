//! Expression picker above the composer's right edge (FEATURES_NOTES §1.6, §1.8, §4.9), tabs
//! GIFs | Emoji. Emoji: search (focused on open), skin tone, rail (frequently used, saved in the
//! settings; servers with custom emoji; the 8 unicode categories), 40 px grid, preview footer
//! (the hovered emoji, else the first of the section in view); custom emoji I can't use here are
//! greyed. Shift-click keeps it open. GIFs: category tiles in two columns (Favorites, Trending,
//! then trending's categories, a click searches the word), debounced search, two columns of
//! still previews that animate on hover with a ☆ to keep a local favourite; a click sends the
//! GIF's page URL. GIF textures (and hover animations) are freed when the picker closes.
//! Reactions use the Emoji tab alone (`reaction_contents`, shown by messages.rs).
//! OWNERSHIP: composer engineer.

use super::icons::{self, Icon as Vector};
use super::messages::Pal;
use super::{images, UiCx};
use crate::api::cdn;
use crate::backend::{ChatCommand, Command};
use crate::config::MAX_FAVORITE_GIFS;
use crate::emoji;
use crate::model::{perm, CustomEmoji, Emoji, GifItem, Id};
use crate::state::chat::GifView;
use crate::state::AppState;
use crate::theme::bold_family;
use egui::{
    pos2, vec2, Align2, Color32, CursorIcon, FontId, Frame, Image, Key, Margin, Order, Rect, ScrollArea, Sense, Stroke,
    TextEdit, Vec2,
};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

const CELL: f32 = 40.0;
const RAIL_W: f32 = 48.0;
const HEADER_H: f32 = 28.0;
const FOOTER_H: f32 = 48.0;
/// Discord's picker size; smaller windows get what fits.
const MAX_SIZE: Vec2 = vec2(498.0, 486.0);
/// GIF search waits this long after the last keystroke (≥ 500 ms, FEATURES_NOTES §1.8).
const GIF_DEBOUNCE_S: f64 = 0.5;
/// "Frequently used" keeps this many.
const MAX_FREQUENT: usize = 27;
/// Skin tone swatches: default, then light .. dark (`emoji::with_skin_tone` 0..=5).
const TONES: [Color32; 6] = [
    Color32::from_rgb(0xff, 0xc8, 0x3d),
    Color32::from_rgb(0xf7, 0xde, 0xce),
    Color32::from_rgb(0xf3, 0xd2, 0xa2),
    Color32::from_rgb(0xd5, 0xab, 0x88),
    Color32::from_rgb(0xaf, 0x7e, 0x57),
    Color32::from_rgb(0x7c, 0x53, 0x3e),
];
/// Rail icons of `emoji::CATEGORIES`.
const CATEGORY_ICONS: [Vector; 8] = [Vector::Smile, Vector::Leaf, Vector::Food, Vector::Ball, Vector::Car, Vector::Bulb, Vector::Heart, Vector::Flag];

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Tab {
    Gif,
    #[default]
    Emoji,
}

/// What a click in the picker asks the composer to do.
pub enum Pick {
    /// Text for the draft at the cursor (unicode or `<:name:id>`).
    Insert(String),
    /// Send this GIF page URL as a message.
    SendGif(String),
}

#[derive(Clone)]
enum Item {
    Uni(&'static emoji::Emoji),
    Custom { e: CustomEmoji, guild: Id, usable: bool },
}

#[derive(Clone, Copy, PartialEq)]
enum Icon {
    Vector(Vector),
    Guild(Id),
}

struct Section {
    icon: Icon,
    title: String,
    items: Vec<Item>,
}

/// A GIF list opened from its tile while the search is empty (None = the category tiles).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GifSection {
    Favorites,
    Trending,
}

/// Picker state between passes (kept across opens for the session: tone, tab).
#[derive(Clone, Default)]
struct Mem {
    gif_section: Option<GifSection>,
    tab: Tab,
    search: String,
    tone: u8,
    tone_open: bool,
    /// Hovered emoji for the footer.
    preview: Option<Item>,
    /// Section tops in the grid content and its scroll offset, from the last pass.
    section_tops: Vec<f32>,
    scroll_y: f32,
    scroll_to: Option<f32>,
    focus_search: bool,
    gif_query: String,
    /// When the GIF query was last edited (ctx time), until it is sent.
    gif_edit: Option<f64>,
    /// Image-cache URLs this picker loaded that nothing else shows (freed on close).
    loaded: Vec<String>,
    /// The one animation playing under the pointer (only one the picker loaded itself).
    anim: Option<String>,
    /// The emoji sections and the hash of what they were built from (rebuilt when it changes).
    sections: Option<(u64, Arc<Vec<Section>>)>,
}

/// Temp-memory key of the open marker: `u64` = the pass it opened in (also set by the
/// `RUSTCORD_MSG_DEMO=emoji` hook in messages.rs).
pub(super) fn id() -> egui::Id {
    egui::Id::unique("rc_emoji_picker")
}

fn mem_id() -> egui::Id {
    id().with("mem")
}

pub(super) fn is_open(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<u64>(id())).is_some()
}

/// The open picker's tab (None = closed), for the composer's button states.
pub(super) fn open_tab(ctx: &egui::Context) -> Option<Tab> {
    is_open(ctx).then(|| ctx.data(|d| d.get_temp::<Mem>(mem_id())).unwrap_or_default().tab)
}

/// Test hook: open on `tab`.
pub(super) fn open(ctx: &egui::Context, tab: Tab) {
    let mut mem: Mem = ctx.data_mut(|d| d.remove_temp(mem_id())).unwrap_or_default();
    mem.tab = tab;
    ctx.data_mut(|d| {
        d.insert_temp(mem_id(), mem);
        d.insert_temp(id(), 0u64);
    });
}

/// The picker anchored above `anchor` (the composer card). `toggle` = its GIF/emoji button was
/// clicked this pass. Closes on Esc, a click outside, a pick (Shift keeps emoji open).
pub(super) fn show(ctx: &egui::Context, cx: &mut UiCx<'_>, pal: &Pal, ch: Id, anchor: Rect, toggle: Option<Tab>) -> Option<Pick> {
    let pass = ctx.cumulative_pass_nr();
    let mut mem: Mem = ctx.data_mut(|d| d.remove_temp(mem_id())).unwrap_or_default();
    let mut opened = ctx.data(|d| d.get_temp::<u64>(id()));
    if let Some(tab) = toggle {
        if opened.is_some() && mem.tab == tab {
            opened = None;
        } else {
            opened = opened.or(Some(pass));
            mem.tab = tab;
            mem.focus_search = true;
        }
    }
    let Some(opened) = opened else {
        close(ctx, cx, &mut mem);
        return None;
    };
    if opened == pass || opened == 0 {
        mem.focus_search = true;
    }
    let screen = ctx.content_rect();
    let size = vec2(MAX_SIZE.x.min(screen.width() - 16.0), MAX_SIZE.y.min(anchor.top() - screen.top() - 48.0).max(180.0));
    let out = egui::Area::new(id().with("area"))
        .order(Order::Foreground)
        .fixed_pos(pos2(anchor.right(), anchor.top() - 8.0))
        .pivot(Align2::RIGHT_BOTTOM)
        .constrain(true)
        .show(ctx, |ui| {
            Frame::popup(ui.style()).fill(pal.surface).corner_radius(pal.card_radius()).inner_margin(0).show(ui, |ui| {
                ui.set_min_size(size);
                ui.set_max_size(size);
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                tabs(ui, pal, &mut mem);
                match mem.tab {
                    Tab::Emoji => emoji_tab(ui, cx, pal, &mut mem, ch),
                    Tab::Gif => gif_tab(ui, cx, pal, &mut mem),
                }
            })
        });
    let pick = out.inner.inner;
    let keep = matches!((&pick, mem.tab), (Some(Pick::Insert(_)), Tab::Emoji)) && ctx.input(|i| i.modifiers.shift);
    let outside = toggle.is_none() && pass != opened && out.response.clicked_elsewhere();
    if (pick.is_some() && !keep) || outside || ctx.input(|i| i.key_pressed(Key::Escape)) {
        close(ctx, cx, &mut mem);
    } else {
        ctx.data_mut(|d| {
            d.insert_temp(id(), opened);
            d.insert_temp(mem_id(), mem);
        });
    }
    pick
}

/// Discord's reaction picker height: the same picker in emoji-only mode.
const REACTION_H: f32 = 396.0;

fn reaction_mem_id() -> egui::Id {
    id().with("reaction")
}

/// Reaction mode (hover toolbar, message menu "View More"): the Emoji tab without the tab row,
/// `REACTION_H` tall, with its own search and scroll. The pick is the emoji to react with (tone
/// applied, counted in "Frequently used"); nothing goes into the draft.
pub(super) fn reaction_contents(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, ch: Id) -> Option<Emoji> {
    let ctx = ui.ctx().clone();
    let mut mem: Mem = ctx.data_mut(|d| d.remove_temp(reaction_mem_id())).unwrap_or_else(|| Mem {
        tone: ctx.data(|d| d.get_temp(reaction_mem_id().with("tone"))).unwrap_or(0),
        focus_search: true,
        scroll_to: Some(0.0),
        ..Default::default()
    });
    let screen = ctx.content_rect();
    let size = vec2(MAX_SIZE.x.min(screen.width() - 16.0), REACTION_H.min(screen.height() - 16.0));
    ui.set_min_size(size);
    ui.set_max_size(size);
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    ui.add_space(12.0);
    let pick = emoji_tab(ui, cx, pal, &mut mem, ch);
    ctx.data_mut(|d| d.insert_temp(reaction_mem_id(), mem));
    match pick? {
        Pick::Insert(t) if t.starts_with('<') => parse_emoji(&t),
        Pick::Insert(t) => Some(Emoji { id: None, name: Some(t), animated: false }),
        Pick::SendGif(_) => None,
    }
}

/// The reaction picker is closed: free what it loaded; the next one opens fresh (tone kept).
pub(super) fn reaction_closed(ctx: &egui::Context, cx: &mut UiCx<'_>) {
    let Some(mut mem) = ctx.data_mut(|d| d.remove_temp::<Mem>(reaction_mem_id())) else { return };
    free_loaded(cx, &mut mem);
    ctx.data_mut(|d| d.insert_temp(reaction_mem_id().with("tone"), mem.tone));
}

/// The first `n` distinct emoji of `keys` ("Frequently used" keys, then defaults) I can react with
/// here, with their `:name:` (the message menu's Add Reaction submenu).
pub(super) fn frequent_reactions<'a>(state: &AppState, keys: impl Iterator<Item = &'a str>, ch: Id, n: usize) -> Vec<(Emoji, String)> {
    let custom = custom_emoji(state, ch);
    let mut out: Vec<(Emoji, String)> = Vec::with_capacity(n);
    for key in keys {
        let Some(e) = parse_emoji(key).filter(|e| !out.iter().any(|(o, _)| o == e)) else { continue };
        let name = match e.id {
            Some(id) => custom.iter().flat_map(|(_, _, l)| l).find(|(c, ok)| c.id == id && *ok).map(|(c, _)| c.name.clone()),
            None => e.name.as_deref().and_then(emojis::get).map(|u| emoji::shortcode(u).unwrap_or(u.name()).to_owned()),
        };
        if let Some(name) = name {
            out.push((e, name));
        }
        if out.len() == n {
            break;
        }
    }
    out
}

/// Forget the open marker, the search and the GIF results; free what the picker loaded.
fn close(ctx: &egui::Context, cx: &mut UiCx<'_>, mem: &mut Mem) {
    free_loaded(cx, mem);
    if cx.state.chat.gifs.is_some() {
        cx.state.chat.gifs = None;
    }
    mem.search.clear();
    mem.gif_query.clear();
    mem.gif_edit = None;
    mem.gif_section = None;
    mem.preview = None;
    mem.tone_open = false;
    mem.scroll_to = Some(0.0);
    mem.sections = None;
    let mem = std::mem::take(mem);
    ctx.data_mut(|d| {
        d.remove::<u64>(id());
        d.insert_temp(mem_id(), mem);
    });
}

fn free_loaded(cx: &mut UiCx<'_>, mem: &mut Mem) {
    for url in mem.loaded.drain(..) {
        cx.images.remove(&url);
    }
    mem.anim = None;
}

/// Remember `url` for freeing on close, unless something else (chat, reactions) loaded it first.
/// Call before the picker's first `get` of it.
fn track(cx: &UiCx<'_>, mem: &mut Mem, url: &str) {
    if !cx.images.contains(url) && !mem.loaded.iter().any(|u| u == url) {
        mem.loaded.push(url.to_owned());
    }
}

/// `url` becomes the one hover animation; the previous one is freed at once. One the chat
/// already shows (same CDN URL) is left alone.
fn animate(cx: &mut UiCx<'_>, mem: &mut Mem, url: &str) {
    if mem.anim.as_deref() == Some(url) {
        return;
    }
    if let Some(old) = mem.anim.take() {
        cx.images.remove(&old);
        mem.loaded.retain(|u| *u != old);
    }
    if !cx.images.contains(url) {
        track(cx, mem, url);
        mem.anim = Some(url.to_owned());
    }
}

/// Closes the picker where the composer isn't drawn (settings, no or read-only channel, narrow
/// search), so its GIF results and textures don't stay loaded behind it.
pub(super) fn close_if_open(ctx: &egui::Context, cx: &mut UiCx<'_>) {
    if is_open(ctx) {
        let mut mem: Mem = ctx.data_mut(|d| d.remove_temp(mem_id())).unwrap_or_default();
        close(ctx, cx, &mut mem);
    }
}

fn tabs(ui: &mut egui::Ui, pal: &Pal, mem: &mut Mem) {
    Frame::new().inner_margin(Margin { left: 12, right: 12, top: 12, bottom: 8 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (tab, text) in [(Tab::Gif, "GIFs"), (Tab::Emoji, "Emoji")] {
                let on = mem.tab == tab;
                let font = FontId::new(pal.size - 0.5, bold_family());
                let g = ui.painter().layout_no_wrap(text.to_owned(), font, pal.text);
                let (rect, resp) = ui.allocate_exact_size(g.size() + vec2(24.0, 12.0), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, on, text));
                let hot = resp.hovered() || resp.has_focus();
                if on || hot {
                    ui.painter().rect_filled(rect, 8, if on { pal.text.gamma_multiply(0.12) } else { pal.tint() });
                }
                ui.painter().galley(rect.center() - g.size() / 2.0, g, if on || hot { pal.text } else { pal.muted });
                if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() && !on {
                    mem.tab = tab;
                    mem.focus_search = true;
                }
            }
        });
    });
}

/// Search field in the picker's style; `width` excludes anything right of it.
fn search_field(ui: &mut egui::Ui, pal: &Pal, text: &mut String, salt: &str, hint: &str, width: f32, focus: bool) -> egui::Response {
    let resp = ui.add(
        TextEdit::singleline(text)
            .hint_text(hint)
            .desired_width(width)
            .margin(Margin::symmetric(8, 6))
            .background_color(pal.bg)
            .id_salt(salt),
    );
    if focus {
        resp.request_focus();
    }
    resp
}

// ---------------------------------------------------------------------------
// Emoji
// ---------------------------------------------------------------------------

/// Whether the fonts draw `s` as one emoji: a single glyph (variation selectors and tones
/// aside), or color glyphs one emoji wide (the OS color font joins ZWJ sequences; the
/// monochrome fallback shows their parts, and keycaps and flags come out as letters).
pub(super) fn drawable(ctx: &egui::Context, s: &str) -> bool {
    let mut chars = s.chars().filter(|c| !matches!(c, '\u{FE0F}' | '\u{1F3FB}'..='\u{1F3FF}'));
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return ctx.fonts_mut(|f| f.has_glyph(&FontId::proportional(20.0), c));
    }
    // Laid out (and so rasterized) small: each sequence costs a few atlas pixels.
    let font = FontId::proportional(6.0);
    ctx.fonts_mut(|f| {
        let one = f.layout_no_wrap("😀".into(), font.clone(), Color32::WHITE).size().x;
        let g = f.layout_no_wrap(s.into(), font, Color32::WHITE);
        let mut drawn = g.rows.iter().flat_map(|r| &r.row.glyphs).filter(|g| !g.uv_rect.is_nothing()).peekable();
        drawn.peek().is_some() && drawn.all(|g| g.is_color) && g.size().x <= one * 1.2
    })
}

/// The unicode categories, without emoji the fonts can't draw (computed once).
fn unicode_categories(ctx: &egui::Context) -> &'static [Vec<&'static emoji::Emoji>] {
    static CATS: OnceLock<Vec<Vec<&'static emoji::Emoji>>> = OnceLock::new();
    CATS.get_or_init(|| emoji::CATEGORIES.iter().map(|(_, g)| emoji::category_emojis(g).filter(|e| drawable(ctx, e.as_str())).collect()).collect())
}

/// Custom emoji by server (the current one first) with whether I may use each here
/// (FEATURES_NOTES §1.6): available; animated needs Nitro; another server's needs Nitro and,
/// in a server channel, USE_EXTERNAL_EMOJIS. Emoji role limits aren't in the model (allowed).
/// (guild, guild name, its emoji with whether each is usable here), borrowed: callers clone
/// only what they keep.
pub(super) type GuildEmoji<'a> = (Id, &'a str, Vec<(&'a CustomEmoji, bool)>);

pub(super) fn custom_emoji(state: &AppState, ch: Id) -> Vec<GuildEmoji<'_>> {
    let (here, nitro, external) = emoji_rights(state, ch);
    let mut guilds: Vec<_> = state.guilds.iter().filter(|g| !g.emojis.is_empty()).collect();
    guilds.sort_by_key(|g| Some(g.id) != here);
    guilds
        .into_iter()
        .map(|g| {
            let own = Some(g.id) == here;
            let list = g.emojis.iter().map(|e| (e, usable(e, own, nitro, external))).collect();
            (g.id, g.name.as_str(), list)
        })
        .collect()
}

/// This channel's server, whether I have Nitro, whether external emoji are allowed here.
fn emoji_rights(state: &AppState, ch: Id) -> (Option<Id>, bool, bool) {
    let here = state.channels.get(&ch).and_then(|c| c.guild_id);
    let nitro = state.me.as_ref().is_some_and(|u| u.premium_type != 0);
    let external = here.is_none() || state.channel_permissions(ch) & perm::USE_EXTERNAL_EMOJIS != 0;
    (here, nitro, external)
}

fn usable(e: &CustomEmoji, own_guild: bool, nitro: bool, external: bool) -> bool {
    e.available && (!e.animated || nitro) && (own_guild || (nitro && external))
}

/// Wire syntax of a custom emoji.
pub(super) fn custom_text(e: &CustomEmoji) -> String {
    format!("<{}:{}:{}>", if e.animated { "a" } else { "" }, e.name, e.id.0)
}

/// Counts a use for "Frequently used" (saved in the settings): unicode text (tone stripped) or
/// `<:name:id>`; anything else is ignored.
pub(super) fn record_use(cx: &mut UiCx<'_>, text: &str) {
    let Some(e) = parse_emoji(text.trim()) else { return };
    let key = if e.id.is_some() { text.trim().to_owned() } else { e.name.unwrap_or_default() };
    bump(&mut cx.settings.frequent_emoji, key);
    if let Err(err) = cx.settings.save() {
        cx.view.toast(format!("Could not save settings: {err}"), true);
    }
}

/// +1 for `key`; a new one replaces the least used when the list is full.
fn bump(list: &mut Vec<(String, u32)>, key: String) {
    match list.iter().position(|(x, _)| *x == key) {
        Some(i) => list[i].1 += 1,
        None => {
            list.truncate(MAX_FREQUENT - 1);
            list.push((key, 1));
        }
    }
    // Stable: ties keep the earlier use first.
    list.sort_by_key(|x| std::cmp::Reverse(x.1));
}

fn parse_emoji(text: &str) -> Option<Emoji> {
    if let Some(inner) = text.strip_prefix('<').and_then(|t| t.strip_suffix('>')) {
        let mut parts = inner.split(':');
        let animated = parts.next()? == "a";
        let name = parts.next()?.to_owned();
        let id = Id(parts.next()?.parse().ok()?);
        return Some(Emoji { id: Some(id), name: Some(name), animated });
    }
    let base: String = text.chars().filter(|c| !('\u{1F3FB}'..='\u{1F3FF}').contains(c)).collect();
    emojis::get(&base).is_some().then_some(Emoji { id: None, name: Some(base), animated: false })
}

/// `sections` from `mem`'s cache, rebuilt only when the channel, search, "Frequently used",
/// the servers' emoji counts or my rights changed (a per-frame rebuild clones every custom emoji).
/// shortcut: a renamed emoji (same count) shows its old name until the picker reopens.
fn cached_sections(ctx: &egui::Context, state: &AppState, mem: &mut Mem, ch: Id, frequent: &[(String, u32)]) -> Arc<Vec<Section>> {
    let mut h = std::hash::DefaultHasher::new();
    let count: usize = state.guilds.iter().map(|g| g.emojis.len()).sum();
    (ch, mem.search.trim(), frequent, count, emoji_rights(state, ch)).hash(&mut h);
    let key = h.finish();
    if let Some((k, all)) = &mem.sections
        && *k == key
    {
        return all.clone();
    }
    let all = Arc::new(sections(ctx, state, ch, &mem.search, frequent));
    mem.sections = Some((key, all.clone()));
    all
}

fn sections(ctx: &egui::Context, state: &AppState, ch: Id, search: &str, frequent: &[(String, u32)]) -> Vec<Section> {
    let custom = custom_emoji(state, ch);
    let custom_item = |&(e, usable): &(&CustomEmoji, bool), guild: Id| Item::Custom { e: e.clone(), guild, usable };
    let q = search.trim().to_lowercase();
    if !q.is_empty() {
        let mut items: Vec<Item> = custom
            .iter()
            .flat_map(|(gid, _, list)| list.iter().filter(|(e, _)| e.name.to_lowercase().contains(&q)).map(move |x| custom_item(x, *gid)))
            .collect();
        items.extend(emoji::search(&q, 400).into_iter().filter(|(_, e)| drawable(ctx, e.as_str())).map(|(_, e)| Item::Uni(e)));
        return vec![Section { icon: Icon::Vector(Vector::Clock), title: String::new(), items }];
    }
    let mut out = Vec::new();
    let frequent: Vec<Item> = frequent
        .iter()
        .filter_map(|(key, _)| parse_emoji(key))
        .filter_map(|e| match e.id {
            None => emojis::get(e.name.as_deref()?).map(Item::Uni),
            Some(id) => custom.iter().find_map(|(gid, _, list)| list.iter().find(|(c, _)| c.id == id).map(|x| custom_item(x, *gid))),
        })
        .collect();
    if !frequent.is_empty() {
        out.push(Section { icon: Icon::Vector(Vector::Clock), title: "Frequently used".into(), items: frequent });
    }
    for (gid, name, list) in &custom {
        out.push(Section { icon: Icon::Guild(*gid), title: (*name).to_owned(), items: list.iter().map(|x| custom_item(x, *gid)).collect() });
    }
    for (i, ((title, _), list)) in emoji::CATEGORIES.iter().zip(unicode_categories(ctx)).enumerate() {
        out.push(Section { icon: Icon::Vector(CATEGORY_ICONS[i]), title: (*title).to_owned(), items: list.iter().map(|e| Item::Uni(e)).collect() });
    }
    out
}

fn emoji_tab(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, mem: &mut Mem, ch: Id) -> Option<Pick> {
    let ctx = ui.ctx().clone();
    let (mut pick, mut enter) = (None, false);
    // Search row with the skin tone swatch at its end.
    Frame::new().inner_margin(Margin { left: 12, right: 12, top: 0, bottom: 8 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let w = ui.available_width() - 32.0;
            let resp = search_field(ui, pal, &mut mem.search, "rc_emoji_search", "Search emoji", w, std::mem::take(&mut mem.focus_search));
            if resp.changed() {
                mem.scroll_to = Some(0.0);
            }
            enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !mem.search.trim().is_empty();
            tone_picker(ui, pal, mem, resp.rect);
        });
    });
    let all = cached_sections(&ctx, cx.state, mem, ch, &cx.settings.frequent_emoji);
    // Enter picks the first match, as in the web client.
    if enter {
        let first = all.iter().flat_map(|s| &s.items).find(|it| !matches!(it, Item::Custom { usable: false, .. }));
        pick = first.map(|it| item_text(it, mem.tone));
    }
    let body_h = (ui.available_height() - FOOTER_H).max(CELL);
    ui.horizontal_top(|ui| {
        if mem.search.trim().is_empty() {
            rail(ui, cx, pal, mem, &all, body_h);
        }
        let grid_pick = grid(ui, cx, pal, mem, &all, body_h);
        pick = pick.take().or(grid_pick);
    });
    // Nothing hovered: the first emoji of the section in view, as Discord does.
    let shown = mem.preview.clone().or_else(|| all.get(current_section(&mem.section_tops, mem.scroll_y)).and_then(|s| s.items.first().cloned()));
    footer(ui, cx, pal, mem, shown);
    if let Some(Pick::Insert(t)) = &pick {
        record_use(cx, t);
    }
    pick
}

fn item_text(item: &Item, tone: u8) -> Pick {
    Pick::Insert(match item {
        Item::Uni(e) => emoji::with_skin_tone(e, tone).as_str().to_owned(),
        Item::Custom { e, .. } => custom_text(e),
    })
}

/// The swatch button; while open, the six tones cover the search field's end.
fn tone_picker(ui: &mut egui::Ui, pal: &Pal, mem: &mut Mem, search: Rect) {
    let (rect, resp) = ui.allocate_exact_size(vec2(24.0, search.height()), Sense::click());
    let resp = resp.on_hover_text("Skin tone").on_hover_cursor(CursorIcon::PointingHand);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Skin tone"));
    if resp.hovered() {
        ui.painter().rect_filled(rect, pal.radius, pal.tint());
    }
    ui.painter().circle(rect.center(), 8.0, TONES[mem.tone as usize % 6], Stroke::new(1.0, pal.line()));
    if resp.clicked() {
        mem.tone_open = !mem.tone_open;
    }
    if !mem.tone_open {
        return;
    }
    let strip = Rect::from_min_max(pos2(rect.right() - 6.0 * 28.0, search.top()), pos2(rect.right(), search.bottom()));
    ui.painter().rect_filled(strip, pal.radius, pal.surface);
    for (i, color) in TONES.iter().enumerate() {
        let r = Rect::from_center_size(pos2(strip.left() + 14.0 + 28.0 * i as f32, strip.center().y), vec2(26.0, strip.height()));
        let resp = ui.interact(r, ui.scope_id().with(("rc_tone", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        if resp.hovered() || i == mem.tone as usize {
            ui.painter().rect_filled(r, pal.radius, pal.tint());
        }
        ui.painter().circle(r.center(), 8.0, *color, Stroke::new(1.0, pal.line()));
        if resp.clicked() {
            mem.tone = i as u8;
            mem.tone_open = false;
        }
    }
}

/// Index of the section shown at the grid's top.
fn current_section(tops: &[f32], scroll_y: f32) -> usize {
    tops.iter().rposition(|t| *t <= scroll_y + 1.0).unwrap_or(0)
}

fn rail(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, mem: &mut Mem, all: &[Section], h: f32) {
    let current = current_section(&mem.section_tops, mem.scroll_y);
    let (rect, _) = ui.allocate_exact_size(vec2(RAIL_W, h), Sense::hover());
    ui.painter().vline(rect.right() - 0.5, rect.y_range(), Stroke::new(1.0, pal.line()));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(4.0, 0.0))).layout(egui::Layout::top_down(egui::Align::Min)));
    ScrollArea::vertical().id_salt("rc_emoji_rail").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(&mut child, |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        for (i, s) in all.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(RAIL_W - 8.0, 40.0), Sense::click());
            let resp = resp.on_hover_text(&s.title).on_hover_cursor(CursorIcon::PointingHand);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &s.title));
            if !ui.is_rect_visible(r) {
                continue;
            }
            let on = i == current;
            if on || resp.hovered() {
                ui.painter().rect_filled(r.shrink(2.0), pal.radius, pal.tint());
            }
            let color = if on || resp.hovered() { pal.text } else { pal.muted };
            match s.icon {
                Icon::Vector(v) => icons::paint(ui.painter(), v, Rect::from_center_size(r.center(), Vec2::splat(20.0)), color, Color32::TRANSPARENT),
                Icon::Guild(gid) => guild_icon(ui, cx, gid, &s.title, Rect::from_center_size(r.center(), Vec2::splat(28.0))),
            }
            if resp.clicked() {
                mem.scroll_to = mem.section_tops.get(i).copied();
            }
        }
    });
}

fn guild_icon(ui: &mut egui::Ui, cx: &mut UiCx<'_>, gid: Id, name: &str, rect: Rect) {
    let url = cx.state.guild(gid).and_then(|g| cdn::guild_icon_url(g, 64, false));
    let side = (rect.width() * ui.ctx().pixels_per_point()).ceil() as u32;
    match url.and_then(|u| cx.images.get(ui.ctx(), &u, side, false)) {
        Some(tex) => Image::from_texture(tex).corner_radius(8).paint_at(ui, rect),
        None => {
            let letters: String = name.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect();
            images::paint_initial(ui.painter(), rect, 8.0, gid.0, &letters, ui.visuals().dark_mode);
        }
    }
}

fn grid(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, mem: &mut Mem, all: &[Section], h: f32) -> Option<Pick> {
    let mut pick = None;
    let mut hovered = None;
    let mut scroll = ScrollArea::vertical().id_salt("rc_emoji_grid").auto_shrink([false, false]).max_height(h).min_scrolled_height(h);
    if let Some(y) = mem.scroll_to.take() {
        scroll = scroll.vertical_scroll_offset(y);
    }
    // The grid sits in a horizontal row (rail | grid): lay its content out top-down.
    let out = scroll.show(ui, |ui| ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        // Equal free space left of the grid and right of it, before the scroll bar that floats
        // over the right edge.
        let avail = ui.available_width();
        let bar = ui.spacing().scroll.bar_width;
        let cols = (((avail - bar - 16.0) / CELL).floor() as usize).max(1);
        let left = ((avail - bar - cols as f32 * CELL) / 2.0).max(0.0);
        let w = avail - left;
        let top = ui.cursor().top();
        let mut tops = Vec::with_capacity(all.len());
        if all.iter().all(|s| s.items.is_empty()) {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| ui.label(egui::RichText::new("No emoji found").color(pal.muted)));
        }
        for (si, s) in all.iter().enumerate() {
            let (hr, _) = ui.allocate_exact_size(vec2(w + left, if s.title.is_empty() { 4.0 } else { HEADER_H }), Sense::hover());
            tops.push(hr.top() - top);
            if !s.title.is_empty() && ui.is_rect_visible(hr) {
                // The label role, in the case given (no capitals, docs/DESIGN.md §3).
                let font = FontId::new(pal.label(), crate::theme::semibold_family());
                ui.painter().text(pos2(hr.left() + left + 4.0, hr.center().y + 2.0), Align2::LEFT_CENTER, s.title.as_str(), font, pal.muted);
            }
            for (ri, row) in s.items.chunks(cols).enumerate() {
                let (rr, _) = ui.allocate_exact_size(vec2(w + left, CELL), Sense::hover());
                if !ui.is_rect_visible(rr) {
                    continue;
                }
                for (ci, item) in row.iter().enumerate() {
                    let cell = Rect::from_min_size(pos2(rr.left() + left + ci as f32 * CELL, rr.top()), Vec2::splat(CELL));
                    let resp = ui.interact(cell, ui.scope_id().with(("rc_emoji_cell", si, ri * cols + ci)), Sense::click());
                    let usable = !matches!(item, Item::Custom { usable: false, .. });
                    if resp.hovered() {
                        hovered = Some(item.clone());
                        if usable {
                            ui.painter().rect_filled(cell.shrink(2.0), pal.radius, pal.tint());
                            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                        }
                    }
                    paint_item(ui, cx, mem, item, cell.shrink(6.0), resp.hovered());
                    if resp.clicked() && usable {
                        pick = Some(item_text(item, mem.tone));
                    }
                }
            }
        }
        ui.add_space(8.0);
        tops
    }).inner);
    mem.section_tops = out.inner;
    mem.scroll_y = out.state.clamped_offset().y;
    mem.preview = hovered;
    pick
}

fn paint_item(ui: &mut egui::Ui, cx: &mut UiCx<'_>, mem: &mut Mem, item: &Item, rect: Rect, hovered: bool) {
    match item {
        Item::Uni(e) => {
            let color = ui.visuals().text_color();
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, e.as_str(), FontId::proportional(rect.height() * 0.9), color);
        }
        Item::Custom { e, usable, .. } => {
            let side = (rect.width() * ui.ctx().pixels_per_point()).ceil().max(48.0) as u32;
            let mut tex = None;
            if hovered && e.animated && *usable {
                let url = cdn::emoji_url(e.id, true, 48);
                animate(cx, mem, &url);
                tex = cx.images.get(ui.ctx(), &url, side, true);
            }
            let tex = tex.or_else(|| cx.images.get(ui.ctx(), &cdn::emoji_url(e.id, false, 48), side, false));
            let alpha = if *usable { 1.0 } else { 0.3 };
            match tex {
                Some(t) => Image::from_texture(t).tint(Color32::WHITE.gamma_multiply(alpha)).paint_at(ui, rect),
                None => {
                    ui.painter().rect_filled(rect, 4, ui.visuals().widgets.inactive.bg_fill.gamma_multiply(alpha));
                }
            }
        }
    }
}

/// `item`: large preview, `:name:` and, for custom emoji, its server (🔒 if unusable).
fn footer(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, mem: &mut Mem, item: Option<Item>) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), FOOTER_H), Sense::hover());
    let r = pal.card_radius();
    ui.painter().rect_filled(rect, egui::CornerRadius { nw: 0, ne: 0, sw: r, se: r }, pal.bg.gamma_multiply(0.6));
    ui.painter().hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, pal.line()));
    let Some(item) = item else { return };
    let icon = Rect::from_center_size(pos2(rect.left() + 32.0, rect.center().y), Vec2::splat(28.0));
    paint_item(ui, cx, mem, &item, icon, false);
    let (locked, name, sub) = match &item {
        Item::Uni(e) => (false, format!(":{}:", emoji::shortcode(e).unwrap_or(e.name())), String::new()),
        Item::Custom { e, guild, usable } => (!*usable, format!(":{}:", e.name), cx.state.guild(*guild).map(|g| g.name.clone()).unwrap_or_default()),
    };
    let x = icon.right() + 12.0;
    let name_x = if locked { x + 20.0 } else { x };
    let g = super::one_line(ui, name.as_str(), super::bold(pal.size), pal.text, rect.right() - name_x - 12.0);
    let y = if sub.is_empty() { rect.center().y - g.size().y / 2.0 } else { rect.center().y - g.size().y };
    if locked {
        icons::paint(ui.painter(), Vector::Lock, Rect::from_min_size(pos2(x, y + g.size().y / 2.0 - 8.0), Vec2::splat(16.0)), pal.text, Color32::TRANSPARENT);
    }
    ui.painter().galley(pos2(name_x, y), g, pal.text);
    if !sub.is_empty() {
        ui.painter().text(pos2(x, rect.center().y + 1.0), Align2::LEFT_TOP, sub, FontId::proportional(pal.meta()), pal.muted);
    }
}

// ---------------------------------------------------------------------------
// GIFs
// ---------------------------------------------------------------------------

fn gif_tab(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, mem: &mut Mem) -> Option<Pick> {
    let now = ui.input(|i| i.time);
    let mut send_now = false;
    Frame::new().inner_margin(Margin { left: 12, right: 12, top: 0, bottom: 8 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            // In a list or a search: back to the tiles.
            let inside = mem.gif_section.is_some() || !mem.gif_query.trim().is_empty();
            if inside && super::messages::icon_button(ui, pal, "‹", "Back", 28.0, false, pal.text).clicked() {
                mem.gif_section = None;
                mem.gif_query.clear();
                mem.gif_edit = Some(f64::NEG_INFINITY);
            }
            let w = ui.available_width();
            // Inside a list the field names it (a category's word is the query itself).
            let hint = match mem.gif_section {
                Some(GifSection::Trending) => "Trending GIFs",
                Some(GifSection::Favorites) => "Favorites",
                None => "Search GIFs",
            };
            let resp = search_field(ui, pal, &mut mem.gif_query, "rc_gif_search", hint, w, std::mem::take(&mut mem.focus_search));
            if resp.changed() {
                mem.gif_edit = Some(now);
            }
            send_now = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        });
    });
    // Trending (with the categories) when the tab opens; a search once typing pauses (or on Enter).
    let due = match mem.gif_edit {
        Some(t) if !send_now && now - t < GIF_DEBOUNCE_S => {
            ui.ctx().request_repaint_after(Duration::from_secs_f64(t + GIF_DEBOUNCE_S - now));
            false
        }
        Some(_) => true,
        None => send_now,
    };
    let q = mem.gif_query.trim();
    let query = (!q.is_empty()).then(|| q.to_owned());
    if (due && cx.state.chat.gifs.as_ref().is_none_or(|v| v.query != query)) || cx.state.chat.gifs.is_none() {
        mem.gif_edit = None;
        free_loaded(cx, mem);
        cx.state.chat.gifs = Some(GifView { query: query.clone(), result: None });
        cx.cmd.send(Command::Chat(ChatCommand::Gifs { query: query.clone() }));
    } else if due {
        mem.gif_edit = None;
    }
    let result = cx.state.chat.gifs.as_ref().and_then(|v| v.result.as_ref());
    let items: Vec<GifItem> = result.and_then(|r| r.as_ref().ok()).cloned().unwrap_or_default();
    let favorites = query.is_none() && mem.gif_section == Some(GifSection::Favorites);
    if query.is_none() && mem.gif_section.is_none() {
        tiles(ui, cx, pal, mem, &items);
        return None;
    }
    let list: Vec<GifItem> = if favorites { cx.settings.favorite_gifs.clone() } else { items.into_iter().filter(|g| g.category.is_none()).collect() };
    let note = match result {
        _ if favorites => list.is_empty().then(|| "No favorites yet".to_owned()),
        None => Some("Loading…".to_owned()),
        Some(Err(e)) => Some(e.clone()),
        Some(Ok(_)) if list.is_empty() => Some("No GIFs found".to_owned()),
        Some(Ok(_)) => None,
    };
    if let Some(note) = note {
        ui.add_space(32.0);
        ui.vertical_centered(|ui| ui.label(egui::RichText::new(note).color(pal.muted)));
        return None;
    }
    gif_grid(ui, cx, pal, mem, &list)
}

/// What a GIF tile opens.
enum TileKind {
    Section(GifSection),
    Search(String),
}

/// The tab's start: Favorites, Trending and trending's categories as two columns of tiles (a
/// darkened picture under the word); a click opens the list or searches the word.
/// shortcut: a category has no still (the API gives only its animated `src`, up to 4 MB
/// decoded), so its tile is a tint until hovered; upgrade if the loader learns first-frame loads.
fn tiles(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, mem: &mut Mem, items: &[GifItem]) {
    let still = |g: Option<&GifItem>| g.map(|g| g.preview.clone()).filter(|p| !p.is_empty());
    let mut tiles = vec![
        ("Favorites".to_owned(), still(cx.settings.favorite_gifs.first()), None, TileKind::Section(GifSection::Favorites)),
        ("Trending".to_owned(), still(items.iter().find(|g| g.category.is_none())), None, TileKind::Section(GifSection::Trending)),
    ];
    for g in items {
        if let Some(name) = &g.category {
            tiles.push((name.clone(), None, Some(g.src.clone()).filter(|s| !s.is_empty()), TileKind::Search(name.clone())));
        }
    }
    let h = ui.available_height();
    let mut open = None;
    Frame::new().inner_margin(Margin { left: 12, right: 4, top: 0, bottom: 0 }).show(ui, |ui| {
        ScrollArea::vertical().id_salt("rc_gif_tiles").auto_shrink([false, false]).max_height(h).show(ui, |ui| {
            let gap = 8.0;
            let col_w = ((ui.available_width() - 8.0 - gap) / 2.0).floor().max(40.0);
            let tile_h = (col_w * 0.42).round();
            let rows = tiles.len().div_ceil(2) as f32;
            let (area, _) = ui.allocate_exact_size(vec2(2.0 * col_w + gap, rows * (tile_h + gap) + 4.0), Sense::hover());
            let side = (col_w * ui.ctx().pixels_per_point()).ceil() as u32;
            for (i, (label, still, anim, kind)) in tiles.into_iter().enumerate() {
                let r = Rect::from_min_size(area.min + vec2((i % 2) as f32 * (col_w + gap), (i / 2) as f32 * (tile_h + gap)), vec2(col_w, tile_h));
                if !ui.is_rect_visible(r) {
                    continue;
                }
                let resp = ui.interact(r, ui.scope_id().with(("rc_gif_tile", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
                let mut tex = None;
                if resp.hovered() && let Some(url) = &anim {
                    animate(cx, mem, url);
                    tex = cx.images.get(ui.ctx(), url, side, true);
                }
                if tex.is_none() && let Some(url) = &still {
                    track(cx, mem, url);
                    tex = cx.images.get(ui.ctx(), url, side, false);
                }
                let p = ui.painter();
                match tex {
                    Some(t) => Image::from_texture(t).uv(super::profile::cover_uv(t.size, r.size())).corner_radius(6).paint_at(ui, r),
                    None => {
                        let seed = label.bytes().fold(7u64, |h, b| h.wrapping_mul(31).wrapping_add(u64::from(b)));
                        p.rect_filled(r, 6, images::initial_colors(seed, pal.dark).0);
                    }
                }
                p.rect_filled(r, 6, Color32::from_black_alpha(if resp.hovered() { 80 } else { 130 }));
                let font = FontId::new(pal.size + 1.0, bold_family());
                let g = p.layout_no_wrap(label.clone(), font, Color32::WHITE);
                let star_w = if matches!(kind, TileKind::Section(GifSection::Favorites)) { 22.0 } else { 0.0 };
                let x = r.center().x - (g.size().x + star_w) / 2.0;
                if star_w > 0.0 {
                    icons::paint(p, Vector::Star, Rect::from_center_size(pos2(x + 8.0, r.center().y), Vec2::splat(16.0)), Color32::WHITE, Color32::TRANSPARENT);
                }
                p.galley(pos2(x + star_w, r.center().y - g.size().y / 2.0), g, Color32::WHITE);
                if resp.clicked() {
                    open = Some(kind);
                }
            }
        });
    });
    match open {
        Some(TileKind::Section(s)) => mem.gif_section = Some(s),
        Some(TileKind::Search(word)) => {
            mem.gif_query = word;
            mem.gif_edit = Some(f64::NEG_INFINITY);
            ui.ctx().request_repaint();
        }
        None => {}
    }
}

/// Two masonry columns of still previews; the hovered one animates and shows a star that adds it
/// to (or removes it from) the local favourites. A click elsewhere on a GIF sends it.
fn gif_grid(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, mem: &mut Mem, items: &[GifItem]) -> Option<Pick> {
    let mut pick = None;
    let mut toggle = None;
    let h = ui.available_height();
    Frame::new().inner_margin(Margin { left: 12, right: 4, top: 0, bottom: 0 }).show(ui, |ui| {
        ScrollArea::vertical().id_salt("rc_gif_grid").auto_shrink([false, false]).max_height(h).show(ui, |ui| {
            let gap = 8.0;
            let col_w = ((ui.available_width() - 8.0 - gap) / 2.0).floor().max(40.0);
            let rects = masonry(items.iter().map(|g| (g.width, g.height)), col_w, gap);
            let total = rects.iter().map(|r| r.bottom()).fold(0.0, f32::max);
            let (area, _) = ui.allocate_exact_size(vec2(2.0 * col_w + gap, total + 12.0), Sense::hover());
            let side = (col_w * ui.ctx().pixels_per_point()).ceil() as u32;
            for (i, (g, r)) in items.iter().zip(rects).enumerate() {
                let r = r.translate(area.min.to_vec2());
                if !ui.is_rect_visible(r) {
                    continue;
                }
                let resp = ui.interact(r, ui.scope_id().with(("rc_gif", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
                let mut tex = None;
                if resp.hovered() && !g.src.is_empty() {
                    animate(cx, mem, &g.src);
                    tex = cx.images.get(ui.ctx(), &g.src, side, true);
                }
                if tex.is_none() && !g.preview.is_empty() {
                    track(cx, mem, &g.preview);
                    tex = cx.images.get(ui.ctx(), &g.preview, side, false);
                }
                match tex {
                    Some(t) => Image::from_texture(t).corner_radius(6).paint_at(ui, r),
                    None => {
                        ui.painter().rect_filled(r, 6, pal.tint());
                    }
                }
                if !resp.hovered() {
                    continue;
                }
                ui.painter().rect_stroke(r, 6, Stroke::new(2.0, pal.text), egui::StrokeKind::Inside);
                let star = Rect::from_center_size(r.right_top() + vec2(-20.0, 20.0), Vec2::splat(28.0));
                let fav = cx.settings.favorite_gifs.iter().any(|f| f.url == g.url);
                let on_star = ui.ctx().pointer_hover_pos().is_some_and(|p| star.contains(p));
                ui.painter().circle_filled(star.center(), 14.0, Color32::from_black_alpha(if on_star { 200 } else { 150 }));
                icons::paint(ui.painter(), Vector::Star, Rect::from_center_size(star.center(), Vec2::splat(18.0)), if fav { TONES[0] } else { Color32::WHITE }, Color32::TRANSPARENT);
                let resp = if on_star { resp.on_hover_text(if fav { "Remove from favorites" } else { "Add to favorites" }) } else { resp };
                if resp.clicked() && on_star {
                    toggle = Some(g.clone());
                } else if resp.clicked() && !g.url.is_empty() {
                    pick = Some(Pick::SendGif(g.url.clone()));
                }
            }
        });
    });
    if let Some(g) = toggle {
        toggle_favorite(&mut cx.settings.favorite_gifs, g);
        if let Err(e) = cx.settings.save() {
            cx.view.toast(format!("Could not save settings: {e}"), true);
        }
    }
    pick
}

/// Adds `g` first (bounded) or removes it when it is already there.
fn toggle_favorite(list: &mut Vec<GifItem>, g: GifItem) {
    match list.iter().position(|f| f.url == g.url) {
        Some(i) => {
            list.remove(i);
        }
        None => {
            list.insert(0, GifItem { category: None, ..g });
            list.truncate(MAX_FAVORITE_GIFS);
        }
    }
}

/// Two-column masonry: each tile keeps its aspect (height clamped) and goes to the shorter column.
fn masonry(sizes: impl Iterator<Item = (u32, u32)>, col_w: f32, gap: f32) -> Vec<Rect> {
    let mut bottoms = [0.0f32; 2];
    sizes
        .map(|(w, h)| {
            let height = if w > 0 && h > 0 { (col_w * h as f32 / w as f32).clamp(col_w * 0.4, col_w * 1.6) } else { col_w * 0.75 };
            let c = usize::from(bottoms[1] < bottoms[0]);
            let r = Rect::from_min_size(pos2(c as f32 * (col_w + gap), bottoms[c]), vec2(col_w, height));
            bottoms[c] += height + gap;
            r
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opens the picker on `tab` over a 900×600 screen for a few passes; its area rect.
    fn picker_rect(tab: Tab) -> Rect {
        let snapshot = serde_json::from_str(include_str!("../../tests/fixtures/home/cache/snapshot.json")).unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let cmd = crate::backend::CommandSender(tx);
        let mut state = AppState::from_snapshot(snapshot);
        let ch = state.selected_channel.unwrap();
        let (mut view, mut images) = (crate::ui::ViewState::default(), images::ImageCache::new(cmd.clone()));
        let mut theme = crate::theme::Theme::nightshade();
        let (mut settings, mut overrides) = (Default::default(), Default::default());
        let ctx = egui::Context::default();
        open(&ctx, tab);
        for _ in 0..3 {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 600.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                let pal = Pal::new(&theme);
                let mut cx = UiCx { state: &mut state, view: &mut view, images: &mut images, theme: &mut theme, settings: &mut settings, cmd: &cmd, overrides: &mut overrides };
                show(ui.ctx(), &mut cx, &pal, ch, Rect::from_min_size(pos2(300.0, 520.0), vec2(580.0, 56.0)), None);
            });
            out.textures_delta.clear();
        }
        ctx.memory(|m| m.area_rect(id().with("area"))).unwrap()
    }

    #[test]
    fn picker_keeps_discords_size() {
        for tab in [Tab::Emoji, Tab::Gif] {
            let r = picker_rect(tab);
            // + the 1 px popup stroke on both sides.
            assert!(r.width() <= MAX_SIZE.x + 2.0 && r.height() <= MAX_SIZE.y + 2.0, "{tab:?}: {r:?}");
            assert!((r.right() - 880.0).abs() < 1.0 && (r.bottom() - 512.0).abs() < 1.0, "{tab:?} anchored above the box: {r:?}");
        }
    }

    #[test]
    fn masonry_fills_the_shorter_column() {
        let r = masonry([(100, 100), (100, 50), (100, 50), (0, 0)].into_iter(), 100.0, 10.0);
        assert_eq!(r[0], Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 100.0)));
        assert_eq!(r[1], Rect::from_min_size(pos2(110.0, 0.0), vec2(100.0, 50.0)));
        assert_eq!(r[2].min, pos2(110.0, 60.0), "right column is shorter");
        assert_eq!(r[3].size(), vec2(100.0, 75.0), "unknown size: 4:3");
    }

    #[test]
    fn custom_emoji_rules() {
        let e = |animated, available| CustomEmoji { id: Id(1), name: "x".into(), animated, available };
        assert!(usable(&e(false, true), true, false, false));
        assert!(!usable(&e(false, false), true, true, true), "unavailable");
        assert!(!usable(&e(true, true), true, false, true), "animated needs Nitro");
        assert!(!usable(&e(false, true), false, false, true), "other server needs Nitro");
        assert!(!usable(&e(false, true), false, true, false), "and external emoji permission");
        assert!(usable(&e(true, true), false, true, true));
        assert_eq!(custom_text(&e(true, true)), "<a:x:1>");
    }

    #[test]
    fn parses_used_emoji() {
        assert_eq!(parse_emoji("<:blob:42>"), Some(Emoji { id: Some(Id(42)), name: Some("blob".into()), animated: false }));
        assert_eq!(parse_emoji("<a:dance:7>").map(|e| e.animated), Some(true));
        assert_eq!(parse_emoji("👍🏽").and_then(|e| e.name), Some("👍".into()), "tone stripped");
        assert_eq!(parse_emoji("<@12>"), None);
        assert_eq!(parse_emoji("hello"), None);
    }

    #[test]
    fn frequent_reactions_skip_unknown_and_repeats() {
        let snapshot = serde_json::from_str(include_str!("../../tests/fixtures/home/cache/snapshot.json")).unwrap();
        let state = AppState::from_snapshot(snapshot);
        let ch = state.selected_channel.unwrap();
        let keys = ["👍", "<:gone:1>", "hello", "👍", "❤️", "🔥"];
        let got: Vec<_> = frequent_reactions(&state, keys.into_iter(), ch, 2).into_iter().map(|(e, n)| (e.name.unwrap_or_default(), n)).collect();
        assert_eq!(got, [("👍".to_owned(), "thumbsup".to_owned()), ("❤️".to_owned(), "heart".to_owned())]);
    }

    #[test]
    fn sections_are_rebuilt_only_on_change() {
        let snapshot = serde_json::from_str(include_str!("../../tests/fixtures/home/cache/snapshot.json")).unwrap();
        let state = AppState::from_snapshot(snapshot);
        let ch = state.selected_channel.unwrap();
        let (ctx, mut mem) = (egui::Context::default(), Mem::default());
        // Fonts exist after a pass (`drawable`).
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        let a =cached_sections(&ctx, &state, &mut mem, ch, &[]);
        assert!(Arc::ptr_eq(&a, &cached_sections(&ctx, &state, &mut mem, ch, &[])), "same inputs: cached");
        mem.search = "smile".into();
        let b = cached_sections(&ctx, &state, &mut mem, ch, &[]);
        assert!(!Arc::ptr_eq(&a, &b) && b.len() == 1, "search: one result section");
        let c = cached_sections(&ctx, &state, &mut mem, ch, &[("👍".into(), 1)]);
        assert!(!Arc::ptr_eq(&b, &c), "frequently used changed");
    }

    #[test]
    fn sequences_drawable_only_with_a_color_font() {
        let ctx = egui::Context::default();
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        // egui's monochrome emoji: no ligatures.
        assert!(drawable(&ctx, "👍🏽") && drawable(&ctx, "❤️"));
        assert!(!drawable(&ctx, "👨‍👩‍👧") && !drawable(&ctx, "1️⃣") && !drawable(&ctx, "🇩🇪"));
        #[cfg(windows)]
        {
            // Segoe UI Emoji joins ZWJ sequences but has no flags; keycaps start with a text digit.
            crate::theme::ThemeApplier::default().apply(&ctx, &crate::theme::Theme::nightshade());
            ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
            assert!(["👍🏽", "👨‍👩‍👧", "🦀", "🥺", "🏳️‍🌈", "👩🏽‍💻"].iter().all(|s| drawable(&ctx, s)));
            assert!(!drawable(&ctx, "1️⃣") && !drawable(&ctx, "🇩🇪"));
        }
    }

    #[test]
    fn section_at_scroll_offset() {
        let tops = [0.0, 100.0, 300.0];
        assert_eq!(current_section(&tops, 0.0), 0);
        assert_eq!(current_section(&tops, 150.0), 1);
        assert_eq!(current_section(&tops, 299.5), 2);
        assert_eq!(current_section(&[], 10.0), 0);
    }
}
