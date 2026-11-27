//! Forum (15) and media (16) channel view (FEATURES_NOTES §2.2, layout §4.9), shown in place of
//! the chat while one is the open channel:
//! - top bar: title search (Enter applies, × clears), New Post; under it Sort & View (Recent
//!   Activity / Date Posted, List / Gallery; defaults from `default_sort_order` /
//!   `default_forum_layout`, media channels a gallery) and the tag filter pills;
//! - post cards from `ThreadsView` with `ThreadList::Forum` (one search per filter change, the
//!   next page when the last card shows; until the first page arrives, or offline, the posts
//!   already known locally): title, unread dot, pin, "author: snippet" of the first message,
//!   reply count, last activity, tags, the first image as thumbnail (list: right; gallery: on
//!   top). A card opens the post in the thread panel; its menu is the thread menu.
//! - New Post (`Modal::NewForumPost`): title, tags from `post_tags` (REQUIRE_TAG / ≤ 5), text,
//!   files (`composer::form_files`), auto-archive; `check_forum_post` before `CreateForumPost`.
//!   The new post opens in the panel (`threads::track`).
//!
//! The list view and its thumbnails are dropped once the forum closes (`threads::track`).
//! OWNERSHIP: threads/forum engineer (docs/SOCIAL_UI_DESIGN.md).

use super::icons::{self, Icon};
use super::messages::{Pal, icon_button, menu_setup, small_button};
use super::server_menu::radio_item;
use super::threads::{self, ago, archive_combo, field_label};
use super::{Modal, UiCx, bold, composer, dialog, dialog_buttons, images, one_line};
use crate::api::cdn;
use crate::backend::{Command, ConnState, ForumPost, SocialCommand, ThreadList, ThreadQuery, ThreadSort};
use crate::model::{Channel, ForumTag, Id, Message, User, channel_type};
use crate::state::AppState;
use crate::theme::bold_family;
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Align, Align2, Color32, CursorIcon, FontId, Frame, Key, Label, Layout, Margin, Rect, RichText, ScrollArea, Sense, Stroke, TextEdit, Vec2, pos2, vec2};

/// List card height (+ `GAP` between cards) and its thumbnail.
const CARD_H: f32 = 112.0;
const GAP: f32 = 8.0;
const THUMB: f32 = 80.0;
/// Gallery tiles: minimum width, height, image height.
const TILE_W: f32 = 220.0;
const TILE_H: f32 = 236.0;
const TILE_IMG_H: f32 = 132.0;
/// Forum flags: REQUIRE_TAG; post flag PINNED.
const REQUIRE_TAG: u64 = 1 << 4;
const PINNED: u64 = 1 << 1;
const MAX_POST_TAGS: usize = 5;

/// Forum view state and New Post drafts (in `ViewState::forum_ui`).
#[derive(Debug, Default)]
pub struct ForumUi {
    /// Search field (post titles); applied on Enter.
    pub search: String,
    /// New Post: title and text.
    pub title: String,
    pub body: String,
    /// The forum the filters and the draft belong to (reset when another one opens).
    forum: Option<Id>,
    /// Applied search, tag filter, sort and layout (None = the forum's default).
    query: String,
    tags: Vec<Id>,
    sort: Option<ThreadSort>,
    gallery: Option<bool>,
    /// New Post: tags, auto-archive minutes and the check's answer.
    post_tags: Vec<Id>,
    archive_minutes: u32,
    error: Option<String>,
}

/// The open channel when it is a forum or media channel (`main_layout` shows `show` then).
pub fn open_forum(state: &AppState) -> Option<Id> {
    let ch = state.selected_channel?;
    state.channels.get(&ch).filter(|c| matches!(c.kind, channel_type::GUILD_FORUM | channel_type::GUILD_MEDIA)).map(|c| c.id)
}

/// The search this forum's view should show.
fn wanted_query(fu: &ForumUi, default_sort: ThreadSort) -> ThreadQuery {
    ThreadQuery {
        name: (!fu.query.is_empty()).then(|| fu.query.clone()),
        tags: fu.tags.clone(),
        sort: fu.sort.unwrap_or(default_sort),
        ..ThreadQuery::default()
    }
}

/// Same search (any page).
fn same_query(a: &ThreadQuery, b: &ThreadQuery) -> bool {
    (&a.name, &a.tags, a.match_all_tags, a.archived, a.sort) == (&b.name, &b.tags, b.match_all_tags, b.archived, b.sort)
}

/// The forum page: top bar, filters, then the post cards.
pub fn show(ui: &mut egui::Ui, cx: &mut UiCx<'_>, forum: Id) {
    let pal = Pal::new(cx.theme);
    let Some(f) = cx.state.channels.get(&forum) else { return };
    let default_sort = if f.default_sort_order == Some(1) { ThreadSort::Created } else { ThreadSort::LastActivity };
    let default_gallery = f.kind == channel_type::GUILD_MEDIA || f.default_forum_layout == Some(2);
    let available: Vec<ForumTag> = f.available_tags.clone();
    if cx.view.forum_ui.forum != Some(forum) {
        cx.view.forum_ui = ForumUi { forum: Some(forum), ..ForumUi::default() };
    }
    let want = wanted_query(&cx.view.forum_ui, default_sort);
    let current = cx.state.social.threads.as_ref().is_some_and(|v| v.channel_id == forum && matches!(&v.list, ThreadList::Forum(q) if same_query(q, &want)));
    if !current {
        threads::load(cx, forum, ThreadList::Forum(want));
    }
    let gallery = cx.view.forum_ui.gallery.unwrap_or(default_gallery);

    let bar = Frame::new().inner_margin(Margin { left: 16, right: 16, top: 12, bottom: 10 }).show(ui, |ui| {
        top_bar(ui, cx, &pal, forum, (default_sort, default_gallery), &available);
    });
    let r = bar.response.rect;
    ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, pal.tk.rule));
    posts(ui, cx, &pal, forum, &available, gallery);
}

/// Search and New Post, then Sort & View and the tag pills. `defaults`: the forum's sort and
/// gallery layout.
fn top_bar(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, forum: Id, defaults: (ThreadSort, bool), available: &[ForumTag]) {
    let (default_sort, default_gallery) = defaults;
    let can_post = cx.state.can_create_post(forum);
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let resp = super::messages::button(ui, pal, super::messages::Btn::Primary, "New post", can_post).on_disabled_hover_text("You can't post in this forum.");
            if resp.clicked() {
                open_new_post(cx, forum);
            }
            let fu = &mut cx.view.forum_ui;
            if !fu.query.is_empty() && icon_button(ui, pal, "×", "Clear search", 32.0, false, pal.text).clicked() {
                (fu.query, fu.search) = (String::new(), String::new());
            }
            let field = TextEdit::singleline(&mut fu.search).char_limit(100).hint_text("Search posts").margin(Margin { left: 30, right: 4, top: 2, bottom: 2 });
            let edit = ui.add(field.desired_width(f32::INFINITY).min_size(vec2(0.0, 32.0)));
            let glass = Rect::from_center_size(pos2(edit.rect.left() + 16.0, edit.rect.center().y), Vec2::splat(16.0));
            icons::paint(ui.painter(), Icon::Search, glass, pal.muted, Color32::TRANSPARENT);
            if edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                fu.query = fu.search.trim().to_owned();
            }
        });
    });
    ui.add_space(8.0);
    // Sort & View, then the tag pills on the same row (`tag_filter`).
    ui.horizontal(|ui| {
        let fu = &mut cx.view.forum_ui;
        let menu = small_button(ui, pal, "↕ Sort and view", pal.text);
        egui::Popup::menu(&menu).show(|ui| {
            menu_setup(ui);
            let sort = fu.sort.unwrap_or(default_sort);
            let gallery = fu.gallery.unwrap_or(default_gallery);
            ui.label(RichText::new("Sort by").size(pal.meta()).family(bold_family()).color(pal.muted));
            for (s, label) in [(ThreadSort::LastActivity, "Recent activity"), (ThreadSort::Created, "Date posted")] {
                if radio_item(ui, label, sort == s, pal.text) {
                    fu.sort = Some(s);
                }
            }
            ui.separator();
            ui.label(RichText::new("View as").size(pal.meta()).family(bold_family()).color(pal.muted));
            for (g, label) in [(false, "List"), (true, "Gallery")] {
                if radio_item(ui, label, gallery == g, pal.text) {
                    fu.gallery = Some(g);
                }
            }
        });
        if !available.is_empty() {
            ui.add_space(4.0);
            tag_filter(ui, cx, pal, available);
        }
    });
}

/// Tag filter pills (any of the picked tags), in one scrolling row.
fn tag_filter(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, available: &[ForumTag]) {
    let fu = &mut cx.view.forum_ui;
    ScrollArea::horizontal().id_salt("rc_forum_tags").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for t in available {
                let on = fu.tags.contains(&t.id);
                if tag_pill(ui, pal, &tag_label(t), on).clicked() {
                    if on {
                        fu.tags.retain(|x| *x != t.id);
                    } else if fu.tags.len() < 20 {
                        fu.tags.push(t.id);
                    }
                }
            }
            if !fu.tags.is_empty() && small_button(ui, pal, "Clear", pal.muted).clicked() {
                fu.tags.clear();
            }
        });
    });
}

/// A tag as a toggle pill: quiet fill; while picked the chip colours (`chip` fill, `link` text
/// and outline), never the accent.
fn tag_pill(ui: &mut egui::Ui, pal: &Pal, text: &str, on: bool) -> egui::Response {
    let color = if on { pal.link } else { pal.text };
    let g = ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(pal.meta()), color);
    let (rect, resp) = ui.allocate_exact_size(g.size() + vec2(20.0, 10.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, on, text));
    let fill = if on { pal.tk.chip } else if resp.hovered() { pal.tk.selected } else { pal.tk.hover };
    let stroke = Stroke::new(1.0, if on { pal.link } else { Color32::TRANSPARENT });
    ui.painter().rect(rect, rect.height() / 2.0, fill, stroke, egui::StrokeKind::Inside);
    ui.painter().galley(rect.center() - g.size() / 2.0, g, color);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

fn tag_label(t: &ForumTag) -> String {
    match t.emoji_name.as_deref().filter(|e| !e.is_empty()) {
        Some(e) => format!("{e} {}", t.name),
        None => t.name.clone(),
    }
}

/// The post ids shown: the search's, or before it answers (and offline) the known active posts
/// when no filter is set.
fn post_ids(state: &AppState, forum: Id, unfiltered: bool, sort: ThreadSort) -> Vec<Id> {
    let view = state.social.threads.as_ref().filter(|v| v.channel_id == forum);
    match view {
        Some(v) if !v.threads.is_empty() => v.threads.iter().map(|t| t.id).collect(),
        _ if unfiltered => {
            let mut ids = state.active_threads(forum);
            if sort == ThreadSort::Created {
                ids.sort_unstable_by(|a, b| b.cmp(a));
            }
            ids
        }
        _ => Vec::new(),
    }
}

/// First message of a post: from the search, else the loaded history (its id is the post's).
fn first_message(state: &AppState, post: Id) -> Option<&Message> {
    let listed = state.social.threads.as_ref().and_then(|v| v.first_message(post));
    listed.or_else(|| state.messages.get(&post)?.msgs.first().filter(|m| m.id == post))
}

fn posts(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, forum: Id, available: &[ForumTag], gallery: bool) {
    let fu = &cx.view.forum_ui;
    let unfiltered = fu.query.is_empty() && fu.tags.is_empty();
    let sort = fu.sort.unwrap_or(ThreadSort::LastActivity);
    let ids = post_ids(cx.state, forum, unfiltered, sort);
    let (loading, error, more) = cx.state.social.threads.as_ref().filter(|v| v.channel_id == forum).map_or((false, None, false), |v| (v.loading, v.error.clone(), v.has_more));
    if ids.is_empty() {
        if loading {
            ui.add_space(48.0);
            ui.vertical_centered(|ui| ui.label(RichText::new("Loading posts…").size(pal.meta()).color(pal.muted)));
            return;
        }
        let offline = crate::config::offline_mode() || cx.state.conn != ConnState::Connected;
        let (heading, text) = match &error {
            Some(_) if offline => ("Couldn't load posts", "Posts load when you're connected."),
            Some(e) => ("Couldn't load posts", e.as_str()),
            None if unfiltered => ("No posts yet", "Start one with New post."),
            None => ("No posts match", "Try another search or fewer tags."),
        };
        let media = cx.state.channels.get(&forum).is_some_and(|c| c.kind == channel_type::GUILD_MEDIA);
        if empty_state(ui, pal, if media { Icon::Media } else { Icon::Forum }, heading, text, error.is_some()) {
            threads::retry(cx.state);
        }
        return;
    }
    let width = ui.available_width() - 32.0;
    let cols = if gallery { ((width + GAP) / (TILE_W + GAP)).floor().max(1.0) as usize } else { 1 };
    let row_h = if gallery { TILE_H } else { CARD_H };
    let rows = ids.len().div_ceil(cols);
    // One extra row for "Loading…" / the error under the cards.
    let footer = loading || error.is_some();
    let mut open = None;
    ScrollArea::vertical().id_salt(("rc_forum_posts", forum.0)).auto_shrink(false).show_rows(ui, row_h + GAP, rows + usize::from(footer), |ui, range| {
        let at_end = range.end >= rows;
        Frame::new().inner_margin(Margin { left: 16, right: 16, top: 0, bottom: 0 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(GAP, GAP);
            for row in range {
                if row >= rows {
                    let text = error.clone().unwrap_or_else(|| "Loading…".into());
                    ui.allocate_ui_with_layout(vec2(width, row_h), Layout::top_down(Align::Center), |ui| {
                        ui.set_min_size(vec2(width, row_h));
                        ui.add_space(row_h / 2.0 - 24.0);
                        ui.add(Label::new(RichText::new(text).size(pal.meta()).color(pal.muted)));
                        if error.is_some() && !loading && small_button(ui, pal, "Retry", pal.text).clicked() {
                            threads::retry(cx.state);
                        }
                    });
                    continue;
                }
                ui.horizontal(|ui| {
                    let tile_w = (width - GAP * (cols as f32 - 1.0)) / cols as f32;
                    for &id in ids.iter().skip(row * cols).take(cols) {
                        let resp = if gallery { tile(ui, cx, pal, id, available, vec2(tile_w, row_h)) } else { card(ui, cx, pal, id, available, vec2(width, row_h)) };
                        if resp.clicked() {
                            open = Some(id);
                        }
                    }
                });
            }
        });
        // Infinite scroll: the next page once the last cards show (not after an error: Retry).
        if at_end && more && error.is_none() && let Some(next) = cx.state.social.threads.as_ref().and_then(crate::state::social::ThreadsView::next_page) {
            if let Some(v) = cx.state.social.threads.as_mut() {
                v.loading = true;
            }
            cx.cmd.send(Command::Social(SocialCommand::FetchThreads { channel_id: forum, list: next }));
        }
    });
    if let Some(id) = open {
        threads::open_in_panel(cx, id);
    }
}

/// Empty state (docs/DESIGN.md §9), its block centred near 30 % of the height: the chat start's
/// tile and rings with the icon, a heading, one meta sentence and, with `retry`, a secondary Retry.
/// True when clicked.
fn empty_state(ui: &mut egui::Ui, pal: &Pal, icon: Icon, heading: &str, text: &str, retry: bool) -> bool {
    ui.add_space((ui.available_height() * 0.3 - 76.0).max(24.0));
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        // Clipped to above the heading, so the rings read as a halo over the tile, not as lines through the text.
        let top = ui.cursor().top();
        let halo = ui.painter().with_clip_rect(ui.clip_rect().intersect(egui::Rect::everything_above(top + 64.0 + 8.0)));
        let rings = halo.add(egui::Shape::Noop);
        let tile = super::messages::hero_tile(ui, pal, (icon, None));
        halo.set(rings, super::messages::hero_rings(tile.center(), pal.tk.rule));
        ui.add_space(12.0);
        ui.label(RichText::new(heading).font(FontId::new(pal.heading(), crate::theme::semibold_family())).color(pal.text));
        ui.add(Label::new(RichText::new(text).size(pal.meta()).color(pal.muted)).wrap());
        ui.add_space(12.0);
        retry && super::messages::button(ui, pal, super::messages::Btn::Secondary, "Retry", true).clicked()
    })
    .inner
}

/// What a card shows of a post.
struct PostInfo {
    title: String,
    pinned: bool,
    unread: bool,
    open: bool,
    replies: u32,
    last: Id,
    tags: Vec<Id>,
    author: Option<User>,
    snippet: String,
    image: Option<String>,
}

fn post_info(cx: &UiCx<'_>, id: Id, thumb: (u32, u32)) -> Option<PostInfo> {
    let s = &*cx.state;
    let c: &Channel = threads::known(s, id)?;
    let first = first_message(s, id);
    let author = first.map(|m| m.author.clone()).or_else(|| c.owner_id.and_then(|o| s.users.get(&o).cloned()));
    let image = first.and_then(|m| m.attachments.iter().find(|a| a.is_image())).map(|a| cdn::attachment_preview_url(a, thumb.0, thumb.1));
    Some(PostInfo {
        title: c.name.clone().unwrap_or_default(),
        pinned: c.flags & PINNED != 0,
        unread: c.member.is_some() && s.is_unread(id),
        open: cx.view.thread_panel == Some(id),
        replies: c.message_count.unwrap_or(0),
        last: c.last_message_id.map_or(c.id, |m| m.max(c.id)),
        tags: c.applied_tags.clone(),
        author,
        snippet: first.map(|m| plain(&m.content)).unwrap_or_default(),
        image,
    })
}

/// Markdown markers and line breaks out, for a one- or two-line snippet.
fn plain(text: &str) -> String {
    let flat: String = text.chars().take(300).map(|c| if c == '\n' { ' ' } else { c }).filter(|c| !matches!(c, '*' | '_' | '`' | '~' | '|' | '>')).collect();
    flat.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Card frame: raised on hover, a `text` outline while open in the panel (the selection).
fn card_frame(ui: &mut egui::Ui, pal: &Pal, size: Vec2, open: bool) -> (Rect, egui::Response) {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hot = resp.hovered();
    let fill = if hot { pal.surface } else { pal.surface.lerp_to_gamma(pal.bg, 0.45) };
    let stroke = if open { pal.text } else if hot { pal.muted.gamma_multiply(0.45) } else { pal.tk.rule };
    ui.painter().rect(rect, pal.card_radius(), fill, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
    (rect, resp.on_hover_cursor(CursorIcon::PointingHand))
}

/// List card: title row, "author: snippet" (2 lines), replies · activity · tags; thumbnail right.
fn card(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, id: Id, available: &[ForumTag], size: Vec2) -> egui::Response {
    let Some(p) = post_info(cx, id, card_px(ui.ctx().pixels_per_point())) else { return ui.allocate_response(size, Sense::hover()) };
    let (rect, resp) = card_frame(ui, pal, size, p.open);
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let inner = rect.shrink2(vec2(16.0, 12.0));
    let mut right = inner.right();
    if let Some(url) = &p.image {
        let r = Rect::from_min_size(pos2(inner.right() - THUMB, rect.center().y - THUMB / 2.0), Vec2::splat(THUMB));
        thumbnail(ui, cx, url, r, pal);
        right = r.left() - 12.0;
    }
    let w = right - inner.left();
    let mut x = inner.left();
    let title_y = inner.top() + 10.0;
    if p.unread {
        ui.painter().circle_filled(pos2(x + 4.0, title_y), 4.0, pal.text);
        x += 14.0;
    }
    if p.pinned {
        icons::paint(ui.painter(), Icon::Pin, Rect::from_min_size(pos2(x, title_y - 8.0), Vec2::splat(16.0)), pal.muted, Color32::TRANSPARENT);
        x += 20.0;
    }
    let title_color = if p.unread || resp.hovered() { pal.text } else { pal.text.gamma_multiply(0.92) };
    let title = one_line(ui, p.title.as_str(), bold(pal.size + 1.0), title_color, right - x);
    ui.painter().galley(pos2(x, title_y - title.size().y / 2.0), title, title_color);
    // "Author: snippet", two lines.
    let snippet_top = inner.top() + 24.0;
    let mut sx = inner.left();
    if let Some(u) = &p.author {
        let url = images::user_avatar_url(None, u, None, 32, false);
        let av = Rect::from_min_size(pos2(sx, snippet_top + 1.0), Vec2::splat(16.0));
        super::sidebar::user_avatar_at(ui, cx.images, &url, av, false, 1.0, (u.id, u.display_name()));
        sx += 22.0;
    }
    let job = snippet_job(p.author.as_ref().map(User::display_name), &p.snippet, pal, right - sx);
    let g = ui.painter().layout_job(job);
    ui.painter().galley(pos2(sx, snippet_top), g, pal.muted);
    footer(ui, pal, &p, available, pos2(inner.left(), inner.bottom() - 9.0), w);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &p.title));
    resp.context_menu(|ui| threads::thread_menu(ui, cx, id));
    resp
}

/// Thumbnail request sizes. The tile's does not follow the tile width (always < 2 tiles + gap,
/// so the cover crop has the pixels) to keep one cache key while the window resizes.
fn card_px(ppp: f32) -> (u32, u32) {
    let px = (THUMB * ppp) as u32 * 2;
    (px, px)
}

fn tile_px(ppp: f32) -> (u32, u32) {
    (((2.0 * TILE_W + GAP) * ppp) as u32, (TILE_IMG_H * ppp) as u32)
}

/// Frees the thumbnails of a dropped post list (from `threads::track`).
pub(super) fn forget_thumbs(cx: &mut UiCx<'_>, v: &crate::state::social::ThreadsView, ppp: f32) {
    if !matches!(v.list, ThreadList::Forum(_)) {
        return;
    }
    for a in v.first_messages.iter().filter_map(|m| m.attachments.iter().find(|a| a.is_image())) {
        for (w, h) in [card_px(ppp), tile_px(ppp)] {
            cx.images.remove(&cdn::attachment_preview_url(a, w, h));
        }
    }
}

/// Gallery tile: image (or the title's initial) on top, title, replies · activity.
fn tile(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, id: Id, available: &[ForumTag], size: Vec2) -> egui::Response {
    let Some(p) = post_info(cx, id, tile_px(ui.ctx().pixels_per_point())) else { return ui.allocate_response(size, Sense::hover()) };
    let (rect, resp) = card_frame(ui, pal, size, p.open);
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let img = Rect::from_min_size(rect.min + vec2(1.0, 1.0), vec2(rect.width() - 2.0, TILE_IMG_H));
    match &p.image {
        Some(url) => thumbnail(ui, cx, url, img, pal),
        None => {
            let (bg, fg) = images::initial_colors(id.0, pal.dark);
            ui.painter().rect_filled(img, egui::CornerRadius { nw: pal.card_radius(), ne: pal.card_radius(), sw: 0, se: 0 }, bg.gamma_multiply(0.5));
            ui.painter().text(img.center(), Align2::CENTER_CENTER, images::initial(&p.title), bold(32.0), fg);
        }
    }
    let inner = Rect::from_min_max(pos2(rect.left() + 12.0, img.bottom() + 10.0), rect.max - vec2(12.0, 10.0));
    let mut job = LayoutJob::single_section(p.title.clone(), TextFormat::simple(bold(pal.size), pal.text));
    job.wrap = TextWrapping { max_width: inner.width(), max_rows: 2, break_anywhere: false, overflow_character: Some('…') };
    let g = ui.painter().layout_job(job);
    ui.painter().galley(inner.min, g, pal.text);
    if p.unread {
        ui.painter().circle_filled(pos2(rect.right() - 12.0, img.bottom() + 12.0), 4.0, pal.text);
    }
    footer(ui, pal, &p, available, pos2(inner.left(), inner.bottom() - 9.0), inner.width());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &p.title));
    resp.context_menu(|ui| threads::thread_menu(ui, cx, id));
    resp
}

/// Thread icon, "12 replies, active 3h ago" and the post's tags, left-centred on `at`.
fn footer(ui: &egui::Ui, pal: &Pal, p: &PostInfo, available: &[ForumTag], at: egui::Pos2, max_w: f32) {
    let font = FontId::proportional(pal.meta());
    let meta = format!("{} repl{}, active {}", p.replies, if p.replies == 1 { "y" } else { "ies" }, ago(p.last.timestamp_ms()));
    icons::paint(ui.painter(), Icon::Thread, Rect::from_center_size(pos2(at.x + 7.0, at.y), Vec2::splat(14.0)), pal.muted, Color32::TRANSPARENT);
    let g = one_line(ui, meta, font.clone(), pal.muted, max_w - 18.0);
    let mut x = at.x + 18.0 + g.size().x + 12.0;
    ui.painter().galley(pos2(at.x + 18.0, at.y - g.size().y / 2.0), g, pal.muted);
    for t in p.tags.iter().filter_map(|id| available.iter().find(|t| t.id == *id)) {
        let g = ui.painter().layout_no_wrap(tag_label(t), FontId::proportional(pal.meta() - 1.0), pal.text);
        let r = Rect::from_min_size(pos2(x, at.y - 9.0), g.size() + vec2(12.0, 0.0)).with_max_y(at.y + 9.0);
        if r.right() > at.x + max_w {
            break;
        }
        ui.painter().rect_filled(r, 9, pal.text.gamma_multiply(0.1));
        ui.painter().galley(pos2(r.left() + 6.0, r.center().y - g.size().y / 2.0), g, pal.text);
        x = r.right() + 4.0;
    }
}

/// "Author: snippet" in two lines at most.
fn snippet_job(author: Option<&str>, snippet: &str, pal: &Pal, max_w: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    if let Some(a) = author {
        let sep = if snippet.is_empty() { "" } else { ": " };
        job.append(&format!("{a}{sep}"), 0.0, TextFormat::simple(bold(pal.size - 1.0), pal.text.gamma_multiply(0.85)));
    }
    job.append(snippet, 0.0, TextFormat::simple(FontId::proportional(pal.size - 1.0), pal.muted));
    job.wrap = TextWrapping { max_width: max_w.max(10.0), max_rows: 2, break_anywhere: false, overflow_character: Some('…') };
    job
}

/// An image cropped to fill `rect` (rounded), a quiet placeholder while it loads.
fn thumbnail(ui: &egui::Ui, cx: &mut UiCx<'_>, url: &str, rect: Rect, pal: &Pal) {
    let side = (rect.size().max_elem() * ui.ctx().pixels_per_point()).ceil() as u32;
    let radius = pal.card_radius();
    match cx.images.get(ui.ctx(), url, side, false) {
        Some(tex) => {
            // Cover: crop the texture to the rect's aspect.
            let (t, r) = (tex.size, rect.size());
            let scale = (r.x / t.x).max(r.y / t.y);
            let half = (r / scale / t) / 2.0;
            let uv = Rect::from_center_size(pos2(0.5, 0.5), half * 2.0);
            egui::Image::new(tex).uv(uv).corner_radius(radius).paint_at(ui, rect);
        }
        None => {
            ui.painter().rect_filled(rect, radius, pal.text.gamma_multiply(0.06));
        }
    }
}

// ---------------------------------------------------------------------------
// New Post
// ---------------------------------------------------------------------------

/// Opens New Post (the draft of this forum stays between openings).
fn open_new_post(cx: &mut UiCx<'_>, forum: Id) {
    let fu = &mut cx.view.forum_ui;
    fu.archive_minutes = cx.state.channels.get(&forum).and_then(|c| c.default_auto_archive_duration).unwrap_or(4320);
    fu.error = None;
    cx.view.modal = Some(Modal::NewForumPost { forum_id: forum });
}

/// `Modal::NewForumPost`: title, tags, text, files, auto-archive; Post checks with
/// `check_forum_post` and shows its answer instead of sending a request that would fail.
pub fn new_post_modal(ctx: &egui::Context, cx: &mut UiCx<'_>, forum: Id) {
    let theme = cx.theme.clone();
    let pal = Pal::new(&theme);
    let offered: Vec<ForumTag> = cx.state.post_tags(forum).into_iter().cloned().collect();
    let required = cx.state.channels.get(&forum).is_some_and(|c| c.flags & REQUIRE_TAG != 0);
    let title = format!("New post in {}", cx.state.channel_name(forum));
    let default_archive = cx.state.channels.get(&forum).and_then(|c| c.default_auto_archive_duration).unwrap_or(4320);
    cx.view.forum_ui.archive_minutes = threads::valid_archive(cx.view.forum_ui.archive_minutes, default_archive);
    // The message box gives way in short windows (720×420) so the form fits.
    let body_h = (ctx.content_rect().height() - 360.0).clamp(60.0, 200.0);
    let resp = dialog(ctx, &theme, "rc_new_post", 602.0, &title, |ui| {
        let fu = &mut cx.view.forum_ui;
        let edit = ui.add(TextEdit::singleline(&mut fu.title).char_limit(100).hint_text("Post title").desired_width(f32::INFINITY));
        if ui.memory(|m| m.focused().is_none()) {
            edit.request_focus();
        }
        if !offered.is_empty() {
            field_label(ui, &pal, if required { "Tags (at least one)" } else { "Tags" });
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for t in &offered {
                    let on = fu.post_tags.contains(&t.id);
                    if tag_pill(ui, &pal, &tag_label(t), on).clicked() {
                        if on {
                            fu.post_tags.retain(|x| *x != t.id);
                        } else if fu.post_tags.len() < MAX_POST_TAGS {
                            fu.post_tags.push(t.id);
                        }
                    }
                }
            });
        }
        ScrollArea::vertical().id_salt("rc_new_post_body").max_height(body_h).show(ui, |ui| {
            let rows = if body_h < 100.0 { 2 } else { 5 };
            ui.add(TextEdit::multiline(&mut fu.body).hint_text("Write something…").desired_rows(rows).desired_width(f32::INFINITY));
        });
        composer::form_files(ui, cx, &pal, forum);
        let fu = &mut cx.view.forum_ui;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Hide after inactivity").family(bold_family()).size(pal.meta()).color(pal.muted));
            archive_combo(ui, "rc_post_archive", &mut fu.archive_minutes);
        });
        if let Some(e) = &fu.error {
            ui.label(RichText::new(e).size(pal.meta()).color(pal.danger));
        }
        dialog_buttons(ui, &theme, "Post", false, !fu.title.trim().is_empty())
    });
    match resp.inner {
        Some(true) => {
            let (files, loading) = composer::form_file_list(ctx, forum);
            let fu = &cx.view.forum_ui;
            let post = ForumPost {
                name: fu.title.trim().to_owned(),
                content: fu.body.trim().to_owned(),
                applied_tags: fu.post_tags.clone(),
                files,
                auto_archive_minutes: fu.archive_minutes,
            };
            let check = if loading { Err("Still preparing files…".to_owned()) } else { cx.state.check_forum_post(forum, &post) };
            match check {
                Err(e) => cx.view.forum_ui.error = Some(e),
                Ok(()) => {
                    cx.cmd.send(Command::Social(SocialCommand::CreateForumPost { channel_id: forum, post }));
                    let fu = &mut cx.view.forum_ui;
                    (fu.title, fu.body, fu.error) = (String::new(), String::new(), None);
                    fu.post_tags.clear();
                    composer::clear_form_files(ctx, forum);
                    cx.view.threads_ui.pending = Some((forum, None, threads::unix_ms()));
                    cx.view.modal = None;
                }
            }
        }
        Some(false) => close_new_post(ctx, cx, forum),
        None if resp.should_close() => close_new_post(ctx, cx, forum),
        None => {}
    }
}

/// Cancel: the text draft stays, the files (and their previews) go.
fn close_new_post(ctx: &egui::Context, cx: &mut UiCx<'_>, forum: Id) {
    composer::clear_form_files(ctx, forum);
    cx.view.modal = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_are_flat_text() {
        assert_eq!(plain("**Bold** idea:\n\n> quoted `code`"), "Bold idea: quoted code");
        assert_eq!(plain(&"a".repeat(500)).len(), 300);
    }

    #[test]
    fn filters_change_the_search() {
        let mut fu = ForumUi::default();
        let a = wanted_query(&fu, ThreadSort::Created);
        assert_eq!((a.name.clone(), a.sort, a.offset), (None, ThreadSort::Created, 0));
        fu.query = "crash".into();
        fu.tags.push(Id(7));
        let b = wanted_query(&fu, ThreadSort::Created);
        assert!(!same_query(&a, &b));
        assert!(same_query(&b, &ThreadQuery { offset: 50, ..b.clone() }), "pages of one search");
        fu.sort = Some(ThreadSort::LastActivity);
        assert_eq!(wanted_query(&fu, ThreadSort::Created).sort, ThreadSort::LastActivity);
    }
}
