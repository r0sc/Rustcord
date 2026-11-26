//! Discord-flavoured markdown: parser (pure, unit-tested) + egui renderer.
//! Supports bold, italic, underline, strike, inline code, code blocks, spoilers, quotes,
//! headers, subtext, lists, masked/auto links, mentions (user/role/channel/@everyone),
//! custom emoji, timestamps, escapes.
//! OWNERSHIP: markdown implementer.

mod parse;

use super::{ProfileTarget, UiCx};
use crate::config::AnimateMode;
use crate::model::{Id, Member, MemberListItem, User};
use crate::state::AppState;
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, CursorIcon, FontFamily, FontId, Label, Margin, Rect, RichText, Sense, Stroke, Ui};
use parse::{Block, Doc, Inline, Style};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub struct MdOpts<'a> {
    pub guild_id: Option<Id>,
    /// `message.mentions` – resolves <@id> for users not in the cache.
    pub mentions: &'a [User],
    /// Stable id for spoiler reveal state (message id).
    pub id_salt: Id,
    /// Text color override (e.g. muted for system messages).
    pub color: Option<egui::Color32>,
    /// Search terms (lowercase) marked on the text-selection colour.
    pub highlight: &'a [String],
    /// The colour under the text when it is not the chat (a mention row's highlight).
    pub bg: Option<egui::Color32>,
}

/// Render message content. Emoji-only messages (<= 30 emoji) render jumbo (48 px emoji).
pub fn render(ui: &mut egui::Ui, cx: &mut UiCx<'_>, text: &str, opts: &MdOpts<'_>) {
    let _t = super::frame_stats::scope("+markdown");
    let doc = parsed(text);
    if doc.blocks.is_empty() {
        return;
    }
    let pal = Pal::new(cx);
    let look = Look {
        size: pal.size,
        color: opts.color.unwrap_or(pal.text),
        bold: false,
        bg: Color32::TRANSPARENT,
        link: None,
    };
    let mut md = Md { cx, opts, pal, jumbo: doc.jumbo, code_n: 0 };

    // Inside a caller's wrapping row (e.g. compact "time name: text") a lone paragraph flows inline.
    let flowing = ui.layout().main_wrap() && ui.layout().main_dir() == egui::Direction::LeftToRight;
    if flowing && let [Block::Paragraph(items)] = doc.blocks.as_slice() {
        let old = ui.spacing().item_spacing.x;
        ui.spacing_mut().item_spacing.x = 0.0;
        md.inlines(ui, items, look);
        ui.spacing_mut().item_spacing.x = old;
        return;
    }
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        md.blocks(ui, &doc.blocks, look);
    });
}

thread_local! {
    /// Parsed documents by content hash, so frames don't re-parse.
    static CACHE: RefCell<HashMap<u64, Rc<Doc>>> = RefCell::new(HashMap::new());
}

/// shortcut: keyed by a 64-bit hash only (collisions ignored), cleared wholesale at 4000 entries.
fn parsed(text: &str) -> Rc<Doc> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    let key = h.finish();
    CACHE.with_borrow_mut(|cache| {
        if let Some(doc) = cache.get(&key) {
            return doc.clone();
        }
        if cache.len() >= 4000 {
            cache.clear();
        }
        let doc = Rc::new(parse::parse(text));
        cache.insert(key, doc.clone());
        doc
    })
}

/// Theme colors used by the renderer (copied once per call).
#[derive(Clone, Copy)]
struct Pal {
    size: f32,
    text: Color32,
    muted: Color32,
    link: Color32,
    /// Pill text (`Tokens::accent_ink`).
    ink: Color32,
    /// Mention and channel chips (`Tokens::chip`), search hits (`Tokens::selection`).
    chip: Color32,
    selection: Color32,
    code_bg: Color32,
    radius: u8,
}

impl Pal {
    fn new(cx: &UiCx<'_>) -> Self {
        let c = &cx.theme.colors;
        let tk = cx.theme.tokens();
        Self {
            size: cx.theme.font.size,
            text: c.text.to_color32(),
            muted: c.text_muted.to_color32(),
            link: c.link.to_color32(),
            ink: tk.accent_ink,
            chip: tk.chip,
            selection: tk.selection,
            code_bg: c.code_background.to_color32(),
            radius: cx.theme.layout.corner_radius.clamp(0.0, 255.0) as u8,
        }
    }
}

/// Text appearance inherited from the enclosing block/container.
#[derive(Clone, Copy)]
struct Look<'d> {
    size: f32,
    color: Color32,
    bold: bool,
    bg: Color32,
    /// (url, masked) while rendering a link label.
    link: Option<(&'d str, bool)>,
}

struct Md<'a, 'c> {
    cx: &'a mut UiCx<'c>,
    opts: &'a MdOpts<'a>,
    pal: Pal,
    jumbo: bool,
    /// Code blocks rendered so far (scroll-area ids).
    code_n: usize,
}

impl Md<'_, '_> {
    fn blocks<'d>(&mut self, ui: &mut Ui, blocks: &'d [Block], look: Look<'d>) {
        for b in blocks {
            self.block(ui, b, look);
        }
    }

    fn block<'d>(&mut self, ui: &mut Ui, b: &'d Block, look: Look<'d>) {
        match b {
            Block::Paragraph(items) => self.para(ui, items, look),
            Block::Heading { level, content } => {
                let scale = match level {
                    1 => 1.5,
                    2 => 1.3,
                    _ => 1.15,
                };
                ui.add_space(4.0);
                self.para(ui, content, Look { size: look.size * scale, bold: true, ..look });
            }
            Block::Subtext(items) => self.para(ui, items, Look { size: look.size * 0.8, color: self.pal.muted, ..look }),
            Block::CodeBlock { lang, code } => self.code_block(ui, lang.as_deref(), code),
            Block::Quote(inner) => {
                let frame = egui::Frame::new().inner_margin(Margin { left: 12, ..Margin::ZERO });
                let r = frame.show(ui, |ui| self.blocks(ui, inner, look)).response.rect;
                let bar = Rect::from_min_max(r.min, egui::pos2(r.left() + 3.0, r.bottom()));
                ui.painter().rect_filled(bar, 2, self.pal.muted.gamma_multiply(0.45));
            }
            Block::ListItem { ordered, depth, content } => {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let marker = match ordered {
                        Some(n) => format!("{n}."),
                        None => bullet(ui, *depth, look.size).to_owned(),
                    };
                    ui.add_space(4.0);
                    ui.label(RichText::new(marker).size(look.size).color(look.color));
                    ui.add_space(6.0);
                    ui.vertical(|ui| self.blocks(ui, content, look));
                });
            }
        }
    }

    fn para<'d>(&mut self, ui: &mut Ui, items: &'d [Inline], look: Look<'d>) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            self.inlines(ui, items, look);
        });
    }

    /// Text-like runs are batched into one LayoutJob label (flows across rows); everything
    /// else is its own widget in the same wrapping row.
    fn inlines<'d>(&mut self, ui: &mut Ui, items: &'d [Inline], look: Look<'d>) {
        let mut job = LayoutJob::default();
        for it in items {
            match it {
                Inline::Text(t, st) => {
                    // An emoji the fonts can't draw (e.g. 🦀) shows as its `:shortcode:` pill, not an empty box.
                    let mut rest = t.as_str();
                    while let Some((range, code)) = super::messages::missing_emoji(ui, rest) {
                        self.append_text(ui, &mut job, &rest[..range.start], self.text_format(look, *st));
                        flush(ui, &mut job, look);
                        pill(ui, &format!(":{code}:"), self.pal.muted, self.pal.muted, look.size.min(self.pal.size));
                        rest = &rest[range.end..];
                    }
                    self.append_text(ui, &mut job, rest, self.text_format(look, *st));
                }
                Inline::InlineCode(t) => job.append(t, 0.0, self.code_format(look)),
                Inline::LineBreak => job.append("\n", 0.0, self.text_format(look, Style::default())),
                other => {
                    flush(ui, &mut job, look);
                    self.widget(ui, other, look);
                }
            }
        }
        flush(ui, &mut job, look);
    }

    /// Appends `text` with the search terms (`MdOpts::highlight`) on the text-selection colour.
    fn append_text(&self, ui: &Ui, job: &mut LayoutJob, text: &str, format: TextFormat) {
        let mut rest = text;
        while let Some(r) = find_term(rest, self.opts.highlight) {
            self.append_run(ui, job, &rest[..r.start], format.clone());
            self.append_run(ui, job, &rest[r.clone()], TextFormat { background: self.pal.selection, ..format.clone() });
            rest = &rest[r.end..];
        }
        self.append_run(ui, job, rest, format);
    }

    /// Appends `text` with its emoji 1.375× the text size (48 px in a jumbo message), like the
    /// web client's inline emoji, in a row no taller than the text's (52 px when jumbo).
    fn append_run(&self, ui: &Ui, job: &mut LayoutJob, text: &str, format: TextFormat) {
        let mut rest = text;
        while let Some(r) = emoji_run(rest) {
            job.append(&rest[..r.start], 0.0, format.clone());
            let (size, line) = if self.jumbo { (48.0, 52.0) } else { (format.font_id.size * 1.375, ui.fonts_mut(|f| f.row_height(&format.font_id))) };
            let font_id = FontId::new(size, format.font_id.family.clone());
            job.append(&rest[r.clone()], 0.0, TextFormat { font_id, line_height: Some(line), ..format.clone() });
            rest = &rest[r.end..];
        }
        job.append(rest, 0.0, format);
    }

    fn text_format(&self, look: Look<'_>, st: Style) -> TextFormat {
        let color = if look.link.is_some() { self.pal.link } else { look.color };
        let family = if look.bold || st.bold { crate::theme::bold_family() } else { FontFamily::Proportional };
        let line = |on: bool| if on { Stroke::new(1.0, color) } else { Stroke::NONE };
        TextFormat {
            font_id: FontId::new(look.size, family),
            color,
            background: look.bg,
            italics: st.italic,
            underline: line(st.underline),
            strikethrough: line(st.strike),
            ..Default::default()
        }
    }

    fn code_format(&self, look: Look<'_>) -> TextFormat {
        TextFormat {
            font_id: FontId::monospace(look.size * 0.9),
            color: look.color,
            background: self.pal.code_bg,
            ..Default::default()
        }
    }

    fn widget<'d>(&mut self, ui: &mut Ui, it: &'d Inline, look: Look<'d>) {
        let pal = self.pal;
        match it {
            Inline::Spoiler { idx, content } => self.spoiler(ui, *idx, content, look),
            Inline::Link { label, url, masked } => self.inlines(ui, label, Look { link: Some((url, *masked)), ..look }),
            Inline::UserMention(id) => {
                let name = self.user_name(*id).or_else(|| crate::plugins::people::valid_user_name(self.cx.state, *id)).map_or_else(|| "@unknown-user".to_owned(), |n| format!("@{n}"));
                let r = pill(ui, &name, pal.ink, pal.chip, look.size).on_hover_cursor(CursorIcon::PointingHand);
                if r.clicked() {
                    let anchor = r.rect.left_bottom();
                    self.cx.view.profile_card = Some(ProfileTarget { user_id: *id, guild_id: self.opts.guild_id, anchor, pivot: egui::Align2::LEFT_TOP });
                }
            }
            Inline::RoleMention(id) => {
                let (name, color) = self.role(*id);
                match color {
                    Some(c) => {
                        // Readable on its tint over the chat (docs/DESIGN.md §2.4).
                        let fill = c.gamma_multiply(0.18);
                        let on = self.opts.bg.unwrap_or_else(|| self.cx.theme.colors.background.to_color32()).blend(fill);
                        pill(ui, &name, self.cx.theme.readable_on_pane(c, on), fill, look.size)
                    }
                    None => pill(ui, &name, pal.ink, pal.chip, look.size),
                };
            }
            Inline::ChannelMention(id) => {
                let state: &AppState = self.cx.state;
                let name = if state.channels.contains_key(id) { format!("#{}", state.channel_name(*id)) } else { "#unknown".to_owned() };
                let openable = state.can_view(*id) && state.channels.get(id).is_some_and(|c| c.is_text_chat());
                let r = pill(ui, &name, pal.ink, pal.chip, look.size);
                let r = if openable { r.on_hover_cursor(CursorIcon::PointingHand) } else { r };
                if openable && r.clicked() {
                    self.cx.state.select_channel(*id);
                    self.cx.view.settings_open = false;
                }
            }
            Inline::Everyone => {
                pill(ui, "@everyone", pal.ink, pal.chip, look.size);
            }
            Inline::Here => {
                pill(ui, "@here", pal.ink, pal.chip, look.size);
            }
            Inline::SlashCommand(name) => {
                pill(ui, &format!("/{name}"), pal.ink, pal.chip, look.size).on_hover_text("Slash command");
            }
            Inline::CustomEmoji { name, id, animated } => self.emoji(ui, name, *id, *animated, look.size),
            Inline::Timestamp { unix, style } => self.timestamp(ui, *unix, *style, look),
            Inline::Text(..) | Inline::InlineCode(_) | Inline::LineBreak => {}
        }
    }

    /// Display name from message mentions, the user cache or member data.
    fn user_name(&self, id: Id) -> Option<String> {
        let state: &AppState = self.cx.state;
        let gid = self.opts.guild_id;
        let member = gid.and_then(|g| guild_member(state, g, id));
        let user = self
            .opts
            .mentions
            .iter()
            .find(|u| u.id == id)
            .or_else(|| state.users.get(&id))
            .or_else(|| member.and_then(|m| m.user.as_ref()))?;
        Some(state.member_display_name(gid, user, member))
    }

    fn role(&self, id: Id) -> (String, Option<Color32>) {
        let state: &AppState = self.cx.state;
        let role = self.opts.guild_id.and_then(|g| state.guild(g)).and_then(|g| g.roles.iter().find(|r| r.id == id));
        match role {
            // The @everyone role is already named "@everyone".
            Some(r) => {
                let name = if r.name.starts_with('@') { r.name.clone() } else { format!("@{}", r.name) };
                (name, (r.color != 0).then(|| super::rgb(r.color)))
            }
            None => ("@deleted-role".to_owned(), None),
        }
    }

    fn emoji(&mut self, ui: &mut Ui, name: &str, id: Id, animated: bool, size: f32) {
        let (side, req) = if self.jumbo { (48.0, 96) } else { (22.0 * size / self.pal.size, 48) };
        let (rect, r) = ui.allocate_exact_size(egui::vec2(side, side), Sense::hover());
        let animate = animated && (self.cx.settings.animate_others == AnimateMode::Always || r.hovered());
        // Only fetch/decode the animated variant while it actually plays; the static frame covers the rest.
        let anim_tex = if animate { self.cx.images.get(ui.ctx(), &crate::api::cdn::emoji_url(id, true, req), req, true) } else { None };
        let tex = anim_tex.or_else(|| self.cx.images.get(ui.ctx(), &crate::api::cdn::emoji_url(id, false, req), req, false));
        match tex {
            Some(tex) => {
                let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                ui.painter().image(tex.id, fit(rect, tex.size), uv, Color32::WHITE);
            }
            None => {
                ui.painter().rect_filled(rect.shrink(side * 0.1), 4, self.pal.code_bg);
            }
        }
        r.on_hover_text(format!(":{name}:"));
    }

    fn timestamp(&self, ui: &mut Ui, unix: i64, style: char, look: Look<'_>) {
        let now = chrono::Utc::now().timestamp();
        let local = unix.checked_mul(1000).and_then(|ms| super::messages::local_at(ui.ctx(), ms));
        let Some((text, full)) = local.map(|t| format_timestamp(t, unix, style, now)) else {
            ui.label(RichText::new(format!("<t:{unix}:{style}>")).size(look.size).color(look.color));
            return;
        };
        let code_bg = self.pal.code_bg;
        rounded_label(ui, &text, look.size, look.color, |_| code_bg).on_hover_text(full);
        if style == 'R' {
            let age = (now - unix).unsigned_abs();
            let secs = if age < 60 { 1 } else if age < 3600 { 20 } else { 300 };
            ui.ctx().request_repaint_after(std::time::Duration::from_secs(secs));
        }
    }

    fn spoiler<'d>(&mut self, ui: &mut Ui, idx: usize, content: &'d [Inline], look: Look<'d>) {
        let key = (self.opts.id_salt, idx);
        if self.cx.view.revealed_spoilers.contains(&key) || self.cx.state.spoilers_open() {
            self.inlines(ui, content, Look { bg: self.pal.muted.gamma_multiply(0.15), ..look });
            return;
        }
        // Covered: same-colored text on the cover keeps roughly the content's size. Opaque,
        // so the text can't show through.
        let mut text = String::new();
        cover_text(content, &mut text);
        let cover = self.pal.code_bg.lerp_to_gamma(self.pal.muted, 0.25);
        let rt = RichText::new(text).size(look.size).color(cover).background_color(cover);
        let r = ui
            .add(Label::new(rt).selectable(false).sense(Sense::click()))
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text("Spoiler – click to reveal");
        if r.clicked() {
            self.cx.view.revealed_spoilers.insert(key);
        }
    }

    fn code_block(&mut self, ui: &mut Ui, lang: Option<&str>, code: &str) {
        self.code_n += 1;
        let pal = self.pal;
        let salt = ("md_code", self.opts.id_salt, self.code_n);
        egui::Frame::new()
            .fill(pal.code_bg)
            .stroke(Stroke::new(1.0, pal.muted.gamma_multiply(0.12)))
            .corner_radius(pal.radius)
            .inner_margin(Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if let Some(lang) = lang {
                    ui.add(Label::new(RichText::new(lang).size(pal.size - 3.0).color(pal.muted)).selectable(false));
                }
                egui::ScrollArea::horizontal().id_salt(salt).auto_shrink([false, true]).show(ui, |ui| {
                    let text = RichText::new(code).family(FontFamily::Monospace).size(pal.size * 0.9).color(pal.text);
                    ui.add(Label::new(text).extend());
                });
            });
    }
}

/// Only web links may be opened: other schemes (file://, custom handlers) in untrusted messages
/// could launch local programs or leak credentials.
pub fn is_web_scheme(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// Emit the batched text run as one label (a link label when inside a link).
fn flush(ui: &mut Ui, job: &mut LayoutJob, look: Look<'_>) {
    if job.text.is_empty() {
        return;
    }
    let job = std::mem::take(job);
    match look.link {
        None => {
            ui.label(job);
        }
        Some((url, masked)) => {
            let r = ui.add(Label::new(job).selectable(false).sense(Sense::click()));
            let web = is_web_scheme(url);
            let tip = if !web {
                format!("Not opened (unsupported link type): {url}")
            } else if masked {
                format!("Leaving to {url}")
            } else {
                url.to_owned()
            };
            let r = r.on_hover_text(tip);
            if web && r.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            }
        }
    }
}

/// Mention chip (docs/DESIGN.md §9): `fill` behind the text, flowing inline with the paragraph;
/// on hover the text colour at 28 %.
fn pill(ui: &mut Ui, text: &str, fg: Color32, fill: Color32, size: f32) -> egui::Response {
    let r = rounded_label(ui, text, size, fg, |_| fill);
    if r.hovered() {
        // Hover keeps the fill and underlines the label: 2 px in from both ends, 1 px above the bottom.
        let y = r.rect.bottom() - 1.5;
        ui.painter().hline(r.rect.left() + 2.0..=r.rect.right() - 2.0, y, egui::Stroke::new(1.0, fg));
    }
    r
}

/// One-row label on a rounded background (`fill(hovered)`); a text background can't be
/// rounded, so the rect is painted behind it once its size is known.
fn rounded_label(ui: &mut Ui, text: &str, size: f32, fg: Color32, fill: impl Fn(bool) -> Color32) -> egui::Response {
    let bg = ui.painter().add(egui::Shape::Noop);
    // Narrow no-break spaces pad the background without allowing a line break there.
    let rt = RichText::new(format!("\u{202F}{text}\u{202F}")).size(size).color(fg);
    let r = ui.add(Label::new(rt).selectable(false).extend().sense(Sense::click()));
    ui.painter().set(bg, egui::Shape::rect_filled(r.rect, 4, fill(r.hovered())));
    r
}

fn bullet(ui: &Ui, depth: u8, size: f32) -> &'static str {
    let hollow = depth % 2 == 1 && ui.fonts_mut(|f| f.has_glyph(&FontId::proportional(size), '◦'));
    if hollow { "◦" } else { "•" }
}

/// Guild member data for a user: my own member object, else the loaded member list.
fn guild_member(state: &AppState, guild: Id, user: Id) -> Option<&Member> {
    if state.me.as_ref().is_some_and(|me| me.id == user)
        && let Some(m) = state.my_members.get(&guild)
    {
        return Some(m);
    }
    state.member_lists.get(&guild)?.items.iter().find_map(|it| match it {
        MemberListItem::Member { member, .. } if member.user.as_ref().is_some_and(|u| u.id == user) => Some(member),
        _ => None,
    })
}

/// Stand-in text for a covered spoiler (only its length matters).
fn cover_text(items: &[Inline], out: &mut String) {
    for it in items {
        match it {
            Inline::Text(t, _) | Inline::InlineCode(t) => out.push_str(t),
            Inline::LineBreak => out.push('\n'),
            Inline::Spoiler { content, .. } => cover_text(content, out),
            Inline::Link { label, .. } => cover_text(label, out),
            Inline::CustomEmoji { .. } => out.push_str("MM"),
            _ => out.push_str("@mention"),
        }
    }
}

/// Largest rect of `size`'s aspect ratio centered in `rect`.
fn fit(rect: Rect, size: egui::Vec2) -> Rect {
    let s = (rect.width() / size.x.max(1.0)).min(rect.height() / size.y.max(1.0));
    Rect::from_center_size(rect.center(), size * s)
}

/// (shown text, full date for the tooltip) of `unix` at `t`, its time in the viewer's timezone.
fn format_timestamp(t: chrono::DateTime<chrono::FixedOffset>, unix: i64, style: char, now: i64) -> (String, String) {
    let f = |fmt: &str| t.format(fmt).to_string();
    let text = match style {
        't' => f("%H:%M"),
        'T' => f("%H:%M:%S"),
        'd' => f("%d/%m/%Y"),
        'D' => f("%-d %B %Y"),
        'F' => f("%A, %-d %B %Y %H:%M"),
        's' => f("%d/%m/%Y, %H:%M"),
        'S' => f("%d/%m/%Y, %H:%M:%S"),
        'R' => relative_time(unix - now),
        _ => f("%-d %B %Y %H:%M"),
    };
    (text, f("%A, %-d %B %Y %H:%M:%S"))
}

/// "in 3 hours" / "2 days ago"; `delta` = target - now in seconds.
fn relative_time(delta: i64) -> String {
    let a = delta.unsigned_abs();
    if a < 45 {
        return if delta >= 0 { "in a few seconds" } else { "a few seconds ago" }.to_owned();
    }
    let (n, unit) = if a < 3600 {
        ((a / 60).max(1), "minute")
    } else if a < 86_400 {
        (a / 3600, "hour")
    } else if a < 30 * 86_400 {
        (a / 86_400, "day")
    } else if a < 365 * 86_400 {
        (a / (30 * 86_400), "month")
    } else {
        (a / (365 * 86_400), "year")
    };
    let amount = if n == 1 { format!("1 {unit}") } else { format!("{n} {unit}s") };
    if delta >= 0 { format!("in {amount}") } else { format!("{amount} ago") }
}

/// The first case-insensitive match of any of `terms` (lowercase) in `text`, as a byte range.
fn find_term(text: &str, terms: &[String]) -> Option<std::ops::Range<usize>> {
    if terms.iter().all(|t| t.is_empty()) {
        return None;
    }
    text.char_indices().find_map(|(i, _)| {
        terms.iter().filter(|t| !t.is_empty()).find_map(|t| {
            let mut chars = text[i..].chars();
            let mut end = i;
            for tc in t.chars() {
                let c = chars.next()?;
                if !c.to_lowercase().eq(tc.to_lowercase()) {
                    return None;
                }
                end += c.len_utf8();
            }
            Some(i..end)
        })
    })
}

/// The first run of emoji in `text` as a byte range: pictographs back to back with their
/// presentation selectors, tones, joiners and tag characters (flags, ZWJ sequences stay whole for
/// the shaper). Symbols that default to text (©, ↔, ♥ without U+FE0F) and keycaps are left out:
/// the text faces draw those.
// shortcut: a symbol with U+FE0F that the UI font has (Segoe UI: ♠♡♣♥♦) is enlarged as text;
// telling which face draws a char needs a per-face glyph query egui doesn't offer.
fn emoji_run(text: &str) -> Option<std::ops::Range<usize>> {
    let mut start = None;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let emoji = is_pictograph(c, chars.peek().map(|&(_, n)| n));
        let part = matches!(c, '\u{200D}' | '\u{FE0F}' | '\u{20E3}' | '\u{E0020}'..='\u{E007F}');
        match start {
            None if emoji => start = Some(i),
            Some(s) if !emoji && !part => return Some(s..i),
            _ => {}
        }
    }
    start.map(|s| s..text.len())
}

/// A pictograph (emoji, tone, regional indicator), or a symbol shown as emoji by default
/// (Unicode Emoji_Presentation) or with U+FE0F `next`.
fn is_pictograph(c: char, next: Option<char>) -> bool {
    let u = c as u32;
    let presentation = matches!(
        u,
        0x231A..=0x231B | 0x23E9..=0x23EC | 0x23F0 | 0x23F3 | 0x25FD..=0x25FE | 0x2614..=0x2615 | 0x2648..=0x2653 | 0x267F | 0x2693 | 0x26A1
            | 0x26AA..=0x26AB | 0x26BD..=0x26BE | 0x26C4..=0x26C5 | 0x26CE | 0x26D4 | 0x26EA | 0x26F2..=0x26F3 | 0x26F5 | 0x26FA | 0x26FD
            | 0x2705 | 0x270A..=0x270B | 0x2728 | 0x274C | 0x274E | 0x2753..=0x2755 | 0x2757 | 0x2795..=0x2797 | 0x27B0 | 0x27BF
            | 0x2B1B..=0x2B1C | 0x2B50 | 0x2B55
    );
    matches!(u, 0x1F000..=0x1FAFF) || (matches!(u, 0x2300..=0x2BFF) && (presentation || next == Some('\u{FE0F}')))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_runs() {
        assert_eq!(emoji_run("start 🎉🎉 x"), Some(6..14), "back to back: one run");
        assert_eq!(emoji_run("de 🇩🇪!"), Some(3..11), "a flag stays whole");
        assert_eq!(emoji_run("👨‍👩‍👧"), Some(0..18), "ZWJ sequence");
        assert_eq!(emoji_run("❤️ ⚡"), Some(0..6));
        assert_eq!(emoji_run("x👍🏽"), Some(1..9), "tone");
        assert_eq!(emoji_run("© ↔️ 1️⃣ ♥ text"), None, "text symbols and keycaps stay text");
    }

    #[test]
    fn search_terms_match_any_case() {
        let terms = ["rust".to_owned(), "über".to_owned()];
        assert_eq!(find_term("I love Rust!", &terms), Some(7..11));
        assert_eq!(find_term("ÜBER alles", &terms), Some(0..5), "non-ASCII case folds");
        assert_eq!(find_term("nothing here", &terms), None);
        assert_eq!(find_term("rust", &[]), None);
    }

    #[test]
    fn relative_times() {
        assert_eq!(relative_time(5), "in a few seconds");
        assert_eq!(relative_time(-10), "a few seconds ago");
        assert_eq!(relative_time(-50), "1 minute ago");
        assert_eq!(relative_time(2 * 3600 + 5), "in 2 hours");
        assert_eq!(relative_time(-3 * 86_400), "3 days ago");
        assert_eq!(relative_time(-4 * 365 * 86_400), "4 years ago");
    }

    #[test]
    fn timestamp_formats() {
        let ctx = egui::Context::default();
        let at = |unix: i64| unix.checked_mul(1000).and_then(|ms| crate::ui::messages::local_at(&ctx, ms));
        assert!(at(i64::MAX).is_none() && at(8_300_000_000_000).is_none(), "out of range");
        let (rel, full) = format_timestamp(at(1_007_200).unwrap(), 1_000_000 + 7200, 'R', 1_000_000);
        assert_eq!(rel, "in 2 hours");
        assert!(full.contains("1970"));
        let (d, full) = format_timestamp(at(1_618_953_630).unwrap(), 1_618_953_630, 'D', 0);
        assert!(d.contains("April 2021"), "{d}");
        // The same text as straight from chrono's `Local`.
        let local = chrono::DateTime::from_timestamp(1_618_953_630, 0).unwrap().with_timezone(&chrono::Local);
        assert_eq!(full, local.format("%A, %-d %B %Y %H:%M:%S").to_string());
    }

    #[test]
    fn cache_reuses_documents() {
        let a = parsed("**cached**");
        let b = parsed("**cached**");
        assert!(Rc::ptr_eq(&a, &b));
    }

    #[test]
    fn fit_keeps_aspect() {
        let r = fit(Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(22.0, 22.0)), egui::vec2(96.0, 48.0));
        assert_eq!(r.size(), egui::vec2(22.0, 11.0));
        assert_eq!(r.center(), egui::pos2(11.0, 11.0));
    }
}
