//! Channel header, message list (lazy history on scroll-up, grouping of consecutive messages,
//! replies, attachments, embeds, reactions, hover actions reply/edit/delete/react, inline
//! edit, delete confirmation, reaction picker) and the composer at the bottom.
//! OWNERSHIP: ui-messages implementer (messages.rs, composer.rs, profile.rs).

use super::icons::{self, Icon};
use super::markdown::{self, MdOpts};
use super::{composer, format_timestamp, images, pickers, pins, rgb, search, UiCx, ViewState};
use crate::api::cdn;
use crate::backend::{ChatCommand, Command, ConnState};
use crate::config::AnimateMode;
use crate::model::{channel_type, perm, Attachment, CustomEmoji, Embed, EmbedMedia, Emoji, Id, Member, Message, User};
use crate::state::chat::{AroundView, Upload};
use crate::state::{AppState, PendingMessage};
use crate::theme::{bold_family, Theme};
use egui::load::SizedTexture;
use egui::{
    pos2, vec2, Align, Align2, Button, Color32, CornerRadius, CursorIcon, FontId, Frame, Image, Key, KeyboardShortcut,
    Label, Layout, Margin, Modifiers, Order, Rect, RectAlign, RichText, ScrollArea, Sense, Shape, Stroke, TextEdit,
    UiBuilder, Vec2,
};
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::Duration;

/// History page size for GET /messages.
pub const PAGE_SIZE: u8 = 50;
/// The character counter shows from this length on.
pub const COUNTER_FROM: usize = 1800;

const AVATAR: f32 = 40.0;
const GUTTER: f32 = 56.0;
const COMPACT_GUTTER: f32 = 48.0;
const GROUP_GAP_MS: u64 = 7 * 60 * 1000;
/// Older history is requested when the viewport top is this close to the content top.
const LOAD_MORE_PX: f32 = 300.0;
/// Rows this far outside the viewport are not laid out (their cached height is reserved).
const LAYOUT_MARGIN: f32 = 600.0;
/// Rows closer to the viewport count as on screen (decorations reach a few px outside a row).
const ON_SCREEN_SLACK: f32 = 16.0;
/// Rows off screen within `LAYOUT_MARGIN` are re-measured this often (s), see `keeps_height`.
/// Each time costs a full text layout (egui drops galleys unused for a pass); under the image
/// cache's 2 s keep-recent window, so their emoji and images stay loaded.
const REMEASURE_S: f64 = 1.0;
const MAX_MEDIA: Vec2 = vec2(420.0, 320.0);
const SUPPRESS_EMBEDS: u64 = 1 << 2;
const IS_COMPONENTS_V2: u64 = 1 << 15;
/// Pinning in guilds (MANAGE_MESSAGES alone stopped working 2026-02-23); anyone in DMs.
pub(super) const PIN_MESSAGES: u64 = 1 << 51;
/// A jumped-to message is highlighted this long (seconds).
const FLASH_S: f64 = 2.0;

/// Hover-toolbar quick reactions until the user has reacted with something this session.
const DEFAULT_QUICK: [&str; 3] = ["👍", "❤️", "🔥"];

const JOIN_TEMPLATES: [&str; 13] = [
    "{} joined the party.",
    "{} is here.",
    "Welcome, {}. We hope you brought pizza.",
    "A wild {} appeared.",
    "{} just landed.",
    "{} just slid into the server.",
    "{} just showed up!",
    "Welcome {}. Say hi!",
    "{} hopped into the server.",
    "Everyone welcome {}!",
    "Glad you're here, {}.",
    "Good to see you, {}.",
    "Yay you made it, {}!",
];

// ---------------------------------------------------------------------------
// Shared helpers (also used by composer.rs and profile.rs)
// ---------------------------------------------------------------------------

/// Theme colors as egui colors, plus the derived tokens (`tk`, docs/DESIGN.md §2.2) and the
/// type, radius and focus helpers every view shares.
#[derive(Clone, Copy)]
pub(super) struct Pal {
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub surface: Color32,
    /// Message-row hover (`ThemeColors::message_hover`); other rows and buttons use `tk.hover`.
    pub hover: Color32,
    pub mention: Color32,
    pub link: Color32,
    pub danger: Color32,
    pub bg: Color32,
    /// Control radius r (buttons, inputs, rows); `card_radius` 1.5r, `modal_radius` 2r.
    pub radius: u8,
    /// Body text size B; the type roles (`meta`, `label`, `title` …) derive from it.
    pub size: f32,
    pub dark: bool,
    /// Label on `accent` (primary buttons).
    pub accent_text: Color32,
    /// raised, hover, selected, rule, edge, focus, positive …, danger_fill, chip, heat, scrim.
    pub tk: crate::theme::Tokens,
}

impl Pal {
    pub fn new(t: &Theme) -> Self {
        let c = &t.colors;
        Self {
            text: c.text.to_color32(),
            muted: c.text_muted.to_color32(),
            accent: c.accent.to_color32(),
            surface: c.surface.to_color32(),
            hover: c.message_hover.to_color32(),
            mention: c.mention_highlight.to_color32(),
            link: c.link.to_color32(),
            danger: c.danger.to_color32(),
            bg: c.background.to_color32(),
            radius: t.layout.corner_radius.round().clamp(0.0, 127.0) as u8,
            size: if t.font.size.is_finite() { t.font.size.clamp(6.0, 48.0) } else { 15.0 },
            dark: t.dark,
            accent_text: c.accent_text.to_color32(),
            tk: t.tokens(),
        }
    }

    /// Container tier (1.5r): channel card, user panel, composer, menus, popouts, pickers.
    pub fn card_radius(&self) -> u8 {
        self.radius.saturating_add(self.radius / 2)
    }

    /// Modal tier (2r): modals, profile popout, login panel.
    pub fn modal_radius(&self) -> u8 {
        self.radius.saturating_mul(2)
    }

    // Type roles (§3), all from B = `size`. Weights: `bold`/`semibold_family`/`medium_family`.

    /// B-4, 700: counts, "New", "Live".
    #[allow(dead_code, reason = "shared type role; the main-window views adopt it")]
    pub fn badge(&self) -> f32 {
        self.size - 4.0
    }

    /// B-3: timestamps, status and activity lines, hints, descriptions.
    pub fn meta(&self) -> f32 {
        self.size - 3.0
    }

    /// B-2, 600 muted: categories, member groups, nav groups, field and card labels.
    pub fn label(&self) -> f32 {
        self.size - 2.0
    }

    /// B-1: buttons (500), menus, inputs, tooltips.
    pub fn ui(&self) -> f32 {
        self.size - 1.0
    }

    /// B: messages, channel, DM and member names.
    pub fn body(&self) -> f32 {
        self.size
    }

    /// B+1, 600: chat header name, server header, settings section heading, activity name.
    pub fn heading(&self) -> f32 {
        self.size + 1.0
    }

    /// B+5, 600: settings page title, modal title, popout display name.
    pub fn title(&self) -> f32 {
        self.size + 5.0
    }

    /// B+9, 700: login title, full-profile name, DM start name.
    #[allow(dead_code, reason = "shared type role; login and profile adopt it")]
    pub fn display(&self) -> f32 {
        self.size + 9.0
    }

    /// Hairlines (header border, date separators, card outlines).
    pub fn line(&self) -> Color32 {
        self.muted.gamma_multiply(0.2)
    }

    /// Hover tint that also shows on `surface` (toolbar, chips).
    pub fn tint(&self) -> Color32 {
        self.text.gamma_multiply(0.08)
    }
}

/// Quiet icon-only button for compact chrome (§9): muted icon; on hover the `hot` colour on a
/// `hover` fill; while `on` (a switched-on toggle) the icon is bright on the `selected` fill,
/// never accent. The tooltip is also the accessible name. Known glyphs paint as vectors
/// (`icons::from_glyph`); "" paints nothing.
pub(super) fn icon_button(ui: &mut egui::Ui, pal: &Pal, icon: &str, tip: &str, side: f32, on: bool, hot: Color32) -> egui::Response {
    button_with(ui, pal, icons::from_glyph(icon).ok_or(icon), tip, side, on, hot)
}

/// `icon_button` with a vector icon.
pub(super) fn vector_button(ui: &mut egui::Ui, pal: &Pal, icon: Icon, tip: &str, side: f32, on: bool, hot: Color32) -> egui::Response {
    button_with(ui, pal, Ok(icon), tip, side, on, hot)
}

fn button_with(ui: &mut egui::Ui, pal: &Pal, icon: Result<Icon, &str>, tip: &str, side: f32, on: bool, hot: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(side, side), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), on, tip));
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        let fill = if on { pal.tk.selected } else if hovered { pal.tk.hover } else { Color32::TRANSPARENT };
        ui.painter().rect_filled(rect, pal.radius, fill);
        let color = if on { pal.text } else if hovered || resp.has_focus() { hot } else { pal.muted };
        match icon {
            // 20 px in 28-32 px buttons, 16 px in smaller ones.
            Ok(i) => {
                icons::paint(ui.painter(), i, Rect::from_center_size(rect.center(), Vec2::splat((side - 8.0).clamp(16.0, 20.0))), color, pal.bg.blend(fill));
            }
            // Words ("Aa") and real emoji.
            Err(text) => {
                ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, FontId::new((side - 8.0).min(18.0), crate::theme::icon_family()), color);
            }
        }
        if resp.has_focus() {
            focus_ring(ui, pal, rect, f32::from(pal.radius));
        }
    }
    resp.on_hover_text(tip).on_hover_cursor(CursorIcon::PointingHand)
}

/// The keyboard focus ring (§9): 2 px `focus` at a 2 px offset around `rect` (radius + 2).
/// Draw it while `resp.has_focus()`; rows that fill their column pass `rect.shrink(4.0)` so the
/// ring stays inside it.
pub(super) fn focus_ring(ui: &egui::Ui, pal: &Pal, rect: Rect, radius: f32) {
    ui.painter().rect_stroke(rect.expand(2.0), radius + 2.0, Stroke::new(2.0, pal.tk.focus), egui::StrokeKind::Outside);
}

/// Animation length for motion that answers an action (§7): `secs`, or 0 with reduced motion
/// (`user_settings::app::frame` sets `animation_time` to 0 then).
pub(super) fn motion(ui: &egui::Ui, secs: f32) -> f32 {
    if ui.style().animation_time > 0.0 { secs } else { 0.0 }
}

/// Button kinds of §9. One `Primary` per view (ember); `Danger` only for destructive confirms.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(dead_code, reason = "shared §9 kinds; Secondary / Positive are adopted by the area views")]
pub(super) enum Btn {
    Primary,
    Secondary,
    Danger,
    Positive,
    Ghost,
}

/// A §9 button: 32 tall (raise with `ui.spacing_mut().interact_size.y`, e.g. 40 in modal
/// footers), 16 px padding, at least 64 wide, label ui 14/500. Hover and press change the fill
/// only; keyboard focus draws the focus ring. Disabled filled buttons go grey, never dim orange.
pub(super) fn button(ui: &mut egui::Ui, pal: &Pal, kind: Btn, text: &str, enabled: bool) -> egui::Response {
    ui.scope(|ui| {
        let enabled = enabled && ui.is_enabled();
        let tk = &pal.tk;
        let (black, white) = (Color32::BLACK, Color32::WHITE);
        // (rest, hover, press) fills and the label. Disabled: a neutral fill (`surface` would
        // vanish on the dark themes' floats, which are `surface` too) and muted text.
        let filled = |fill: Color32, label: Color32, hover: Color32, press: Color32| {
            if enabled { ([fill, hover, press], label) } else { ([tk.selected; 3], pal.muted) }
        };
        let (fills, label) = match kind {
            Btn::Primary => filled(pal.accent, pal.accent_text, pal.accent.lerp_to_gamma(white, 0.08), pal.accent.lerp_to_gamma(black, 0.1)),
            Btn::Danger => filled(tk.danger_fill, tk.danger_label, tk.danger_fill.lerp_to_gamma(black, 0.1), tk.danger_fill.lerp_to_gamma(black, 0.18)),
            Btn::Positive => filled(tk.positive_fill, tk.positive_label, tk.positive_fill.lerp_to_gamma(black, 0.1), tk.positive_fill.lerp_to_gamma(black, 0.18)),
            Btn::Secondary => {
                let w = &ui.visuals().widgets;
                ([w.inactive.weak_bg_fill, w.hovered.weak_bg_fill, w.active.weak_bg_fill], pal.text)
            }
            Btn::Ghost => ([Color32::TRANSPARENT, tk.hover, tk.selected], pal.text),
        };
        let filled_kind = matches!(kind, Btn::Primary | Btn::Danger | Btn::Positive);
        let v = ui.visuals_mut();
        if filled_kind {
            // Disabled colours are painted above; no extra fade on top.
            v.disabled_alpha = 1.0;
        }
        let w = &mut v.widgets;
        for (state, fill) in [&mut w.inactive, &mut w.hovered, &mut w.active].into_iter().zip(fills) {
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::NONE;
            state.fg_stroke.color = label;
        }
        ui.spacing_mut().button_padding.x = 16.0;
        let label = RichText::new(text).font(FontId::new(pal.ui(), crate::theme::medium_family()));
        let min_h = ui.spacing().interact_size.y.max(32.0);
        let resp = ui.add_enabled(enabled, Button::new(label).min_size(vec2(64.0, min_h)));
        if resp.has_focus() {
            focus_ring(ui, pal, resp.rect, f32::from(pal.radius));
        }
        resp
    })
    .inner
}

/// Pill switch (§9): 40 x 24. Off: `edge` track, a light knob with a ×; on: `positive` track,
/// the knob with a check. Slides in 120 ms. `changed()` when clicked.
pub(super) fn switch(ui: &mut egui::Ui, pal: &Pal, on: &mut bool, label: &str) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(40.0, 24.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Switch, ui.is_enabled(), *on, label));
    if ui.is_rect_visible(rect) {
        let tk = &pal.tk;
        let t = ui.ctx().animate_bool_with_time(resp.id, *on, motion(ui, 0.12));
        let track = tk.edge.lerp_to_gamma(tk.positive, t);
        let off_knob = if tk.mode == crate::theme::Mode::Light { Color32::WHITE } else { pal.text };
        let knob = off_knob.lerp_to_gamma(tk.on_positive, t);
        let c = pos2(egui::lerp(rect.left() + 12.0..=rect.right() - 12.0, t), rect.center().y);
        let p = ui.painter();
        p.rect_filled(rect, 12.0, track);
        p.circle_filled(c, 9.0, knob);
        let glyph = Stroke::new(1.5, track);
        if t < 0.5 {
            for d in [vec2(3.0, 3.0), vec2(3.0, -3.0)] {
                p.line_segment([c - d, c + d], glyph);
            }
        } else {
            p.add(Shape::line(vec![c + vec2(-3.5, 0.0), c + vec2(-1.0, 2.5), c + vec2(3.5, -2.5)], glyph));
        }
        if resp.has_focus() {
            focus_ring(ui, pal, rect, 12.0);
        }
    }
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// A checkbox box (§9) in `rect` (16-20 px): off a 1.5 px `edge` ring, on `positive` with an
/// `on_positive` check. `radius`: the chip tier (`pal.radius / 2`).
pub(super) fn paint_checkbox(p: &egui::Painter, tk: &crate::theme::Tokens, rect: Rect, on: bool, radius: f32) {
    if on {
        p.rect_filled(rect, radius, tk.positive);
        let (c, s) = (rect.center(), rect.width() / 20.0);
        let tick = vec![c + vec2(-5.0, 0.5) * s, c + vec2(-1.5, 4.0) * s, c + vec2(5.0, -3.5) * s];
        p.add(Shape::line(tick, Stroke::new(2.0 * s, tk.on_positive)));
    } else {
        p.rect_stroke(rect, radius, Stroke::new(1.5, tk.edge), egui::StrokeKind::Inside);
    }
}

/// A radio circle (§9) in `rect` (16-20 px): off a 1.5 px `edge` ring, on a 2 px `text` ring
/// with a `text` dot.
#[allow(dead_code, reason = "shared §9 painter for radio rows (Create channel, menus)")]
pub(super) fn paint_radio(p: &egui::Painter, pal: &Pal, rect: Rect, on: bool) {
    let (c, r) = (rect.center(), rect.width() / 2.0);
    if on {
        p.circle_stroke(c, r - 1.0, Stroke::new(2.0, pal.text));
        p.circle_filled(c, r * 0.4, pal.text);
    } else {
        p.circle_stroke(c, r - 0.75, Stroke::new(1.5, pal.tk.edge));
    }
}

/// Icon on the right of a context-menu row: a vector icon, or a glyph (`icons::from_glyph`
/// turns known ones into vectors; others, e.g. plugin emoji, stay text).
#[derive(Clone, Copy)]
pub(super) enum MenuIcon {
    Glyph(&'static str),
    Icon(Icon),
    /// A checkbox (toggle items).
    Check(bool),
}

/// Call first inside a context menu (§9 menus): 188-320 wide, 32 px rows (submenu buttons
/// too), row fills concentric with the menu's corners, `hover` on hover and keyboard focus.
pub(super) fn menu_setup(ui: &mut egui::Ui) {
    ui.set_min_width(188.0);
    ui.set_max_width(320.0);
    let tk = crate::theme::applied_tokens(ui.ctx());
    let s = ui.style_mut();
    s.spacing.button_padding = vec2(8.0, 4.0);
    s.spacing.interact_size.y = 32.0;
    s.spacing.item_spacing.y = 2.0;
    let inset = u8::try_from(s.spacing.menu_margin.left).unwrap_or(0);
    let r = s.visuals.menu_corner_radius.nw.saturating_sub(inset).max(2);
    let w = &mut s.visuals.widgets;
    for state in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        state.corner_radius = egui::CornerRadius::same(r);
    }
    for state in [&mut w.hovered, &mut w.active, &mut w.open] {
        state.weak_bg_fill = tk.hover;
    }
}

/// Context-menu row (§9): the label (ui 14/500) left, a 16 px icon right in `text_muted`.
/// Pass the theme's `danger` as `color` for a destructive item: danger label and icon, and on
/// hover the `danger_fill` with its label colour. True when clicked (the menu closes itself).
pub(super) fn menu_item(ui: &mut egui::Ui, text: &str, icon: MenuIcon, color: Color32) -> bool {
    let icon = match icon {
        MenuIcon::Glyph(g) => super::icons::from_glyph(g).map_or(icon, MenuIcon::Icon),
        i => i,
    };
    let glyph = if let MenuIcon::Glyph(g) = icon { g } else { "" };
    let tk = crate::theme::applied_tokens(ui.ctx());
    let danger = color == ui.visuals().error_fg_color;
    let muted = ui.visuals().weak_text_color();
    let resp = ui
        .scope(|ui| {
            let w = &mut ui.visuals_mut().widgets;
            for state in [&mut w.inactive, &mut w.hovered, &mut w.active] {
                state.fg_stroke.color = color;
            }
            if danger {
                for state in [&mut w.hovered, &mut w.active] {
                    state.weak_bg_fill = tk.danger_fill;
                    state.fg_stroke.color = tk.danger_label;
                }
            }
            let font = FontId::new(ui.style().text_styles[&egui::TextStyle::Button].size, crate::theme::medium_family());
            ui.add(Button::new(RichText::new(text).font(font)).right_text(RichText::new(glyph).family(crate::theme::icon_family()).color(muted)))
        })
        .inner;
    let hot = resp.hovered() || resp.has_focus();
    let icon_color = if danger { if hot { tk.danger_label } else { color } } else if hot { color } else { muted };
    // Opaque (on the menu fill), so overlapping strokes of an icon don't show darker.
    let fill = if hot { if danger { tk.danger_fill } else { tk.hover } } else { Color32::TRANSPARENT };
    let bg = ui.visuals().window_fill.blend(fill);
    let r = Rect::from_center_size(pos2(resp.rect.right() - ui.spacing().button_padding.x - 8.0, resp.rect.center().y), Vec2::splat(16.0));
    let p = ui.painter();
    match icon {
        MenuIcon::Icon(i) => super::icons::paint(p, i, r, icon_color, bg),
        MenuIcon::Check(on) => {
            // The chip tier: half the theme's control radius.
            let chip = f32::from(ui.ctx().global_style().visuals.widgets.inactive.corner_radius.nw / 2);
            paint_checkbox(p, &tk, r.shrink(1.0), on, chip);
        }
        MenuIcon::Glyph(_) => {}
    }
    if let MenuIcon::Check(on) = icon {
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, ui.is_enabled(), on, text));
    }
    // The focus ring (§9), inside the row: the menu's padding leaves no room around it.
    if resp.has_focus() {
        ui.painter().rect_stroke(resp.rect, ui.visuals().widgets.inactive.corner_radius, Stroke::new(2.0, tk.focus), egui::StrokeKind::Inside);
    }
    if resp.clicked() {
        // For menus that only close on their own rows (`sidebar::user_context_menu`).
        ui.ctx().data_mut(|d| d.insert_temp(menu_click_id(), Some(ui.layer_id())));
    }
    resp.clicked()
}

/// Where the last `menu_item` click was (its menu's layer).
pub(super) fn menu_click_id() -> egui::Id {
    egui::Id::unique("rc_menu_item_click")
}

/// Whether the emoji font draws every glyph of `emoji` (joiners, variation selectors and skin
/// tones aside). The bundled monochrome font lacks some newer emoji: show `NO_GLYPH` instead
/// of an empty box.
pub(super) fn has_emoji_glyph(ui: &egui::Ui, emoji: &str) -> bool {
    let font = FontId::proportional(20.0);
    ui.fonts_mut(|f| {
        emoji.chars().filter(|c| !matches!(c, '\u{200D}' | '\u{FE0F}' | '\u{1F3FB}'..='\u{1F3FF}')).all(|c| f.has_glyph(&font, c))
    })
}

/// Stand-in for an emoji the font can't draw (the real one goes into the tooltip).
pub(super) const NO_GLYPH: &str = "☺";

/// The first emoji in `text` the fonts can't draw (e.g. 🦀 in the monochrome font): its byte
/// range and Discord shortcode, shown as a `:shortcode:` pill instead of an empty box.
pub(super) fn missing_emoji(ui: &egui::Ui, text: &str) -> Option<(std::ops::Range<usize>, &'static str)> {
    let font = FontId::proportional(20.0);
    // Symbol and pictograph blocks only (skin tones count as part of the emoji before them).
    let candidate = |c: char| matches!(c as u32, 0x2000..=0x2BFF | 0x3000..=0x33FF | 0x1F000..=0x1F3FA | 0x1F400..=0x1FAFF);
    let starts: Vec<usize> = text.char_indices().filter(|&(_, c)| candidate(c)).map(|(i, _)| i).collect();
    if starts.is_empty() {
        return None;
    }
    let missing: Vec<usize> = ui.fonts_mut(|f| starts.into_iter().filter(|&i| text[i..].chars().next().is_some_and(|c| !f.has_glyph(&font, c))).collect());
    missing.into_iter().find_map(|i| {
        // Longest emoji sequence from here (ZWJ sequences, flags, tones run up to ~10 chars).
        let ends = text[i..].char_indices().skip(1).map(|(j, _)| i + j).chain([text.len()]).take(12);
        let (end, e) = ends.filter_map(|end| Some((end, emojis::get(&text[i..end])?))).last()?;
        Some((i..end, crate::emoji::shortcode(e).unwrap_or(e.name())))
    })
}

/// `:shortcode:` of a unicode emoji (Discord spelling, else its name): reaction tooltips, and the
/// stand-in for one the fonts can't draw.
pub(super) fn shortcode_label(emoji: &str) -> Option<String> {
    emojis::get(emoji).map(|e| format!(":{}:", crate::emoji::shortcode(e).unwrap_or(e.name())))
}

/// Small rounded status chip ("Offline", "Slowmode 4s", counters) with an optional dot.
pub(super) fn status_chip(ui: &mut egui::Ui, pal: &Pal, dot: Option<Color32>, text: &str, color: Color32) -> egui::Response {
    let g = ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(pal.meta()), color);
    let dot_w = if dot.is_some() { 12.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(16.0 + dot_w + g.size().x, g.size().y + 4.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect_filled(rect, rect.height() / 2.0, pal.tk.selected);
        if let Some(c) = dot {
            p.circle_filled(pos2(rect.left() + 11.0, rect.center().y), 3.0, c);
        }
        p.galley(pos2(rect.left() + 8.0 + dot_w, rect.center().y - g.size().y / 2.0), g, color);
    }
    resp
}

/// Small rounded text button (Retry, Dismiss): a tinted chip that darkens on hover. A known
/// glyph before the first space ("📎 Upload a file") becomes a painted 14 px icon.
pub(super) fn small_button(ui: &mut egui::Ui, pal: &Pal, text: &str, color: Color32) -> egui::Response {
    let (icon, text) = text.split_once(' ').and_then(|(g, rest)| Some((icons::from_glyph(g)?, rest))).map_or((None, text), |(i, t)| (Some(i), t));
    let g = ui.painter().layout_no_wrap(text.to_owned(), FontId::new(pal.meta(), crate::theme::icon_family()), color);
    let icon_w = if icon.is_some() { 18.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 16.0 + icon_w, g.size().y + 6.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), text));
    if ui.is_rect_visible(rect) {
        // A small secondary button: `selected`, +4 % text on hover.
        let w = &ui.visuals().widgets;
        let fill = if resp.hovered() { w.hovered.weak_bg_fill } else { w.inactive.weak_bg_fill };
        ui.painter().rect_filled(rect, pal.radius, fill);
        if let Some(i) = icon {
            icons::paint(ui.painter(), i, Rect::from_center_size(pos2(rect.left() + 15.0, rect.center().y), Vec2::splat(14.0)), color, Color32::TRANSPARENT);
        }
        ui.painter().galley(pos2(rect.left() + 8.0 + icon_w, rect.center().y - g.size().y / 2.0), g, color);
        if resp.has_focus() {
            focus_ring(ui, pal, rect, f32::from(pal.radius));
        }
    }
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// A painted `icon` (about the text height) in front of a one-line label: glyphs that used to
/// sit in text ("📎 file.txt").
pub(super) fn icon_label(ui: &mut egui::Ui, icon: Icon, text: &str, size: f32, color: Color32, bg: Color32) -> egui::Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        vector_icon(ui, icon, (size * 1.1).round(), color, bg);
        ui.add(Label::new(RichText::new(text).size(size).color(color)).selectable(false).truncate())
    })
    .inner
}

/// A secondary button with `label` and the External icon after it (opens a page in the browser).
pub(super) fn external_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    // An em space at 16 px reserves the icon's room.
    let resp = ui.add(Button::new(label).right_text(RichText::new("\u{2003}").size(16.0)).min_size(vec2(64.0, 32.0)));
    let color = ui.style().interact(&resp).text_color();
    let r = Rect::from_center_size(pos2(resp.rect.right() - ui.spacing().button_padding.x - 8.0, resp.rect.center().y), Vec2::splat(16.0));
    icons::paint(ui.painter(), Icon::External, r, color, Color32::TRANSPARENT);
    resp
}

/// A `side` px vector icon laid out like a label; `bg` is the fill under it (cut-outs).
pub(super) fn vector_icon(ui: &mut egui::Ui, icon: Icon, side: f32, color: Color32, bg: Color32) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
    icons::paint(ui.painter(), icon, r, color, bg);
    resp
}

/// Message card of the pins popout and the search results: avatar, name, time and content
/// (`highlight`: search terms, lowercase, marked in the text). Clicking anywhere on it
/// (spoilers and mentions included) answers its response: jump there.
pub(super) fn message_card(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, guild_id: Option<Id>, msg: &Message, highlight: &[String]) -> egui::Response {
    let member = message_member(cx.state, guild_id, msg);
    let name = cx.state.member_display_name(guild_id, &msg.author, member.as_ref());
    let color = name_color(cx, guild_id, member.as_ref(), pal.bg).unwrap_or(pal.text);
    let bg = ui.painter().add(Shape::Noop);
    let rect = Frame::new()
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let url = images::user_avatar_url(guild_id, &msg.author, member.as_ref(), 64, false);
                images::avatar(ui, cx.images, &url, 32.0, false, Some((msg.author.id, &name)));
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let time = time_labels(ui.ctx(), &msg.timestamp).header.clone();
                        let time_w = ui.painter().layout_no_wrap(time.clone(), FontId::proportional(pal.meta()), pal.muted).size().x;
                        ui.scope(|ui| {
                            ui.set_max_width((ui.available_width() - time_w - 8.0).max(24.0));
                            ui.add(Label::new(RichText::new(&name).family(bold_family()).color(color)).selectable(false).truncate());
                        });
                        ui.add(Label::new(RichText::new(time).size(pal.meta()).color(pal.muted)).selectable(false));
                    });
                    if !msg.content.is_empty() {
                        markdown::render(ui, cx, &msg.content, &MdOpts { guild_id, mentions: &msg.mentions, id_salt: msg.id, color: None, highlight, bg: None });
                    }
                    for a in &msg.attachments {
                        icon_label(ui, Icon::Attach, &a.filename, pal.meta(), pal.muted, pal.bg);
                    }
                });
            });
        })
        .response
        .rect;
    // Registered after the content, so it wins the click anywhere on the card.
    let resp = ui.interact(rect, ui.make_persistent_id(("rc_card", msg.id.0)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    let fill = if ui.rect_contains_pointer(rect) { pal.bg.blend(pal.tint()) } else { pal.bg };
    ui.painter().set(bg, Shape::rect_filled(rect, pal.card_radius(), fill));
    resp
}

/// Baseline of a single-line galley, from its top.
fn baseline(g: &egui::Galley) -> f32 {
    g.rows.first().and_then(|r| r.glyphs.first().map(|gl| r.pos.y + gl.pos.y)).unwrap_or(g.size().y)
}

pub(super) fn channel_prefix(kind: u8) -> &'static str {
    match kind {
        channel_type::DM => "@",
        channel_type::GROUP_DM => "",
        _ => "#",
    }
}

/// Member data of a message author: attached to the message (gateway) or cached.
pub(super) fn message_member(state: &AppState, guild_id: Option<Id>, msg: &Message) -> Option<Member> {
    msg.member.clone().or_else(|| state.find_member(guild_id?, msg.author.id).cloned())
}

/// The cached user behind a message author (me / `users`), else the author as stored: cosmetics
/// (decoration, name style, guild tag) follow USER_UPDATE and "Apply to account" like the member
/// and DM lists, not the copy saved with the message.
fn current_user<'a>(state: &'a AppState, author: &'a User) -> &'a User {
    state.me.as_ref().filter(|m| m.id == author.id).or_else(|| state.users.get(&author.id)).unwrap_or(author)
}

/// A member's role colour as text on `bg` (the chat, or the row under the name), moved just far
/// enough for 4.5:1.
pub(super) fn name_color(cx: &UiCx<'_>, guild_id: Option<Id>, member: Option<&Member>, bg: Color32) -> Option<Color32> {
    cx.state.role_color(guild_id?, &member?.roles).map(|c| cx.theme.readable_on_pane(rgb(c), bg))
}

/// The colour under a message row's text while it is hovered (the pointer usually rests on the
/// chat): the chat or a row that mentions me, with `message_row`'s hover fill. Hover moves it
/// toward the text colour, so a name readable here is readable at rest too.
fn row_bg(cx: &UiCx<'_>, pal: &Pal, msg: &Message) -> Color32 {
    if cx.state.mentions_me(msg) { pal.bg.blend(pal.mention.blend(pal.tk.hover)) } else { pal.bg.blend(pal.hover) }
}

/// Small filled label ("Bot", "This is you").
pub(super) fn tag(ui: &mut egui::Ui, text: &str, fill: Color32, color: Color32) {
    Frame::new().fill(fill).corner_radius(3).inner_margin(Margin::symmetric(4, 1)).show(ui, |ui| {
        ui.label(RichText::new(text).small().family(bold_family()).color(color));
    });
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

/// Per-frame facts about the open channel.
struct Lcx {
    ch: Id,
    guild_id: Option<Id>,
    me: Option<Id>,
    kind: u8,
    name: String,
    guild_name: String,
    can_react: bool,
    /// Joining a reaction someone already added needs only READ_MESSAGE_HISTORY.
    can_add_existing: bool,
    can_send: bool,
    can_pin: bool,
    /// Showing a jump window (`FetchAround`) instead of the channel history.
    detached: bool,
    /// The NEW divider goes above the first message after this one (`AppState::unread_marker`).
    unread_after: Option<Id>,
    compact: bool,
    animate_always: bool,
    /// Row hovered in the previous pass (drives hover time, toolbar, avatar animation).
    hovered: Option<Id>,
    /// Row shown as hovered by the `RUSTCORD_MSG_DEMO=hover` test hook.
    demo_hover: Option<Id>,
    /// Message whose context menu is open: it alone shows as hovered meanwhile.
    menu_target: Option<Id>,
    /// The thread panel's list (`thread_chat`): scrolls by its own `Memo::to_bottom`.
    panel: bool,
    pal: Pal,
}

impl Lcx {
    fn animate(&self, user: Id, hovered: bool) -> bool {
        crate::overrides::my_avatar_animates(user, hovered).unwrap_or(self.animate_always || hovered || self.me == Some(user))
    }

    /// A pending scroll-to-bottom: the shell's (open channel) or the thread panel's own.
    fn to_bottom(&self, view: &ViewState, memo: &Memo) -> bool {
        if self.panel { memo.to_bottom } else { view.scroll_to_bottom }
    }

    fn gutter(&self) -> f32 {
        if self.compact { COMPACT_GUTTER } else { GUTTER }
    }

    fn in_thread(&self) -> bool {
        matches!(
            self.kind,
            channel_type::ANNOUNCEMENT_THREAD | channel_type::PUBLIC_THREAD | channel_type::PRIVATE_THREAD
        )
    }
}

/// Message-list bookkeeping kept in egui temp memory between passes.
#[derive(Clone, Default)]
struct Memo {
    channel: Option<Id>,
    /// Last measured row height per message (`keeps_height` says when it is reused).
    heights: HashMap<Id, RowHeight>,
    /// Screen y of the list content's top in the last pass.
    origin: f32,
    /// The row the focused widget was drawn in last pass.
    focus_row: Option<Id>,
    /// A message near the viewport top and its content y. When rows above it change height
    /// (older history prepended, edits, deletes) the offset is shifted to keep it in place.
    anchor: Option<(Id, f32)>,
    at_bottom: bool,
    /// Thread panel: scroll to the bottom once its history is there (set when it opens).
    to_bottom: bool,
    hovered: Option<Id>,
    toolbar: Option<(Id, Rect)>,
    edit_focus: Option<Id>,
    /// Reaction picker anchor and the pass it was opened in.
    picker: Option<(Rect, u64)>,
    /// Message highlighted after a jump, until this time.
    flash: Option<(Id, f64)>,
    /// (first unread loaded message, unread count, more unread not loaded) while that message is
    /// scrolled out above: the new-messages bar shows. `bar_closed` after its "Mark as read".
    new_bar: Option<(Id, usize, bool)>,
    bar_closed: bool,
    /// Viewport distance from the content bottom and viewport height (jump-to-present bar).
    from_bottom: f32,
    view_h: f32,
    /// Open message context menu.
    menu: Option<MsgMenu>,
    #[cfg(test)]
    offset: f32,
}

#[derive(Clone, Copy)]
struct RowHeight {
    /// Content `fingerprint`.
    fp: u64,
    h: f32,
    /// `layout_key` and ctx time of the measurement.
    key: u64,
    at: f64,
}

/// Whether a row keeps its measured height `r` instead of being laid out (`top`: content y).
/// Far-off rows keep one measured at another width: re-measuring all of them on every resize
/// frame is too slow, and the scroll anchor absorbs the drift. Rows just off screen keep a recent
/// one at the same `key` unless `pinned`: while frames keep coming (playing media) they are laid
/// out every `REMEASURE_S`, so a height that changed without a new fingerprint (an image's size
/// arriving, a name resolving) is still caught off screen.
fn keeps_height(r: &RowHeight, top: f32, viewport: Rect, key: u64, now: f64, pinned: bool) -> bool {
    let bottom = top + r.h;
    let far = top > viewport.max.y + LAYOUT_MARGIN || bottom < viewport.min.y - LAYOUT_MARGIN;
    let off_screen = top > viewport.max.y + ON_SCREEN_SLACK || bottom < viewport.min.y - ON_SCREEN_SLACK;
    far || (off_screen && !pinned && r.key == key && now - r.at < REMEASURE_S)
}

/// A key press egui may turn into a focus move this pass.
fn moves_focus(e: &egui::Event) -> bool {
    use egui::Key::{ArrowDown, ArrowLeft, ArrowRight, ArrowUp, Tab};
    matches!(e, egui::Event::Key { key: Tab | ArrowUp | ArrowDown | ArrowLeft | ArrowRight, pressed: true, .. })
}

/// What changes rows' heights without changing their fingerprints: width, scale, text styles
/// (the theme's body size under Chat font scaling), fonts (theme, the gg sans load), group spacing
/// and Settings > Chat.
fn layout_key(ui: &egui::Ui, cx: &UiCx<'_>, width: f32) -> u64 {
    let ctx = ui.ctx();
    let mut h = DefaultHasher::new();
    (width.to_bits(), ctx.pixels_per_point().to_bits(), cx.settings.message_group_spacing.to_bits()).hash(&mut h);
    ui.style().text_styles.hash(&mut h);
    let s = &cx.state.account.settings;
    ([9, 10, 12, 13].map(|n| s.chat_shows(n)), cx.state.spoilers_open()).hash(&mut h);
    ctx.fonts(|f| {
        let d = f.definitions();
        for (name, data) in &d.font_data {
            (name, std::sync::Arc::as_ptr(data)).hash(&mut h);
        }
        d.families.hash(&mut h);
    });
    h.finish()
}

#[derive(Clone, Copy)]
struct MsgMenu {
    id: Id,
    /// The "…" button, or the pointer position of a right-click.
    anchor: Rect,
    from_button: bool,
    /// Opened this pass (egui's popup memory still has to be told).
    new: bool,
}

fn memo_id() -> egui::Id {
    egui::Id::unique("rustcord_message_list")
}

struct HoverRow {
    id: Id,
    rect: Rect,
    own: bool,
    system: bool,
    content: String,
}

pub fn show(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    let Some(ch) = cx.state.selected_channel else {
        let text = if cx.state.has_data { "Select a channel to start chatting." } else { "Loading…" };
        centered_hint(ui, Pal::new(cx.theme).muted, text);
        pickers::close_if_open(ui.ctx(), cx);
        return;
    };
    chat_scaled(ui, cx, ch, false);
}

/// The thread panel's message list and composer (threads.rs) for `thread`, beside the open
/// channel's: own list memory, scroll and composer focus; jumps, search and demo hooks stay with
/// the open channel's list.
pub(super) fn thread_chat(ui: &mut egui::Ui, cx: &mut UiCx<'_>, thread: Id) {
    chat_scaled(ui, cx, thread, true);
}

/// Settings > Accessibility > Chat font scaling: the chat (messages and composer) is drawn with
/// that text size in place of the theme's.
fn chat_scaled(ui: &mut egui::Ui, cx: &mut UiCx<'_>, ch: Id, panel: bool) {
    let theme_size = cx.theme.font.size;
    if let Some(size) = cx.settings.chat_font_size {
        cx.theme.font.size = size;
    }
    chat(ui, cx, ch, panel);
    cx.theme.font.size = theme_size;
}

fn chat(ui: &mut egui::Ui, cx: &mut UiCx<'_>, ch: Id, panel: bool) {
    let pal = Pal::new(cx.theme);
    let Some(channel) = cx.state.channels.get(&ch) else {
        centered_hint(ui, pal.muted, "This channel is not available.");
        if !panel {
            pickers::close_if_open(ui.ctx(), cx);
        }
        return;
    };
    let (kind, guild_id, text_chat) = (channel.kind, channel.guild_id, channel.is_text_chat());
    let name = cx.state.channel_name(ch);
    if !text_chat {
        centered_hint(ui, pal.muted, "This kind of channel can't be shown here.");
        if !panel {
            pickers::close_if_open(ui.ctx(), cx);
        }
        return;
    }
    let newest = cx.state.messages.get(&ch).and_then(|c| c.msgs.last()).map(|m| m.id);
    if !panel {
        demo_setup(ui.ctx(), cx, ch, newest);
        // Narrow windows have no room for the results panel next to the chat: it takes its place.
        if search::is_open(cx.state) && search::narrow(ui.ctx()) {
            search::results(ui, cx);
            pickers::close_if_open(ui.ctx(), cx);
            return;
        }
    }

    let ctx = ui.ctx().clone();
    let memo_key = if panel { memo_id().with("thread_panel") } else { memo_id() };
    let mut memo: Memo = ctx.data_mut(|d| d.remove_temp(memo_key)).unwrap_or_default();
    let jumping = !panel && settle_jump(&ctx, cx, ch);
    // Sending, "Jump to present" or reopening the channel return to the live history.
    if !panel && !jumping && (cx.view.scroll_to_bottom || memo.channel != Some(ch)) {
        cx.state.chat.around = cx.state.chat.around.take().filter(|v| v.channel_id == ch && !cx.view.scroll_to_bottom);
    }
    let detached = detached(cx.state, ch).is_some();
    let jump = detached || (memo.channel == Some(ch) && viewing_older(cx.state, ch, &memo));
    // The chat header's bottom: the composer's popups stay below it.
    let chat_top = ui.max_rect().top();
    egui::Panel::bottom(if panel { "rc_thread_composer" } else { "rc_composer" })
        .resizable(false)
        .show_separator_line(false)
        .frame(Frame::new().fill(pal.bg).inner_margin(Margin { left: 8, right: 8, top: 0, bottom: 8 }))
        .show(ui, |ui| {
            if jump {
                ui.spacing_mut().item_spacing.y = 0.0;
                // The panel's list scrolls by its own flag (the shell's belongs to the chat).
                memo.to_bottom |= jump_bar(ui, cx, &pal, ch) && panel;
            }
            super::frame_stats::time("+composer", || composer::show(ui, cx, ch, chat_top, panel))
        });
    // After the composer: a DM's call area sizes itself to the room the composer left.
    if !panel {
        super::dm_call::banner(ui, cx, ch);
    }

    let lc = Lcx {
        ch,
        guild_id,
        me: cx.state.me.as_ref().map(|u| u.id),
        kind,
        name,
        guild_name: guild_id.and_then(|g| cx.state.guild(g)).map(|g| g.name.clone()).unwrap_or_default(),
        can_react: cx.state.can_react(ch),
        can_add_existing: cx.state.channel_permissions(ch) & perm::READ_MESSAGE_HISTORY != 0,
        can_send: cx.state.can_send(ch),
        can_pin: cx.state.channel_permissions(ch) & PIN_MESSAGES != 0,
        detached,
        unread_after: cx.state.unread_marker.filter(|(c, _)| *c == ch).map(|(_, r)| r),
        compact: cx.theme.layout.compact || cx.state.account.settings.compact_messages(),
        animate_always: cx.settings.animate_others == AnimateMode::Always,
        hovered: memo.hovered,
        demo_hover: newest.filter(|_| !panel && (demo("hover") || demo("menu"))),
        menu_target: memo.menu.map(|m| m.id),
        panel,
        pal,
    };
    message_list(ui, cx, &lc, &mut memo);
    message_menu(ui, cx, &lc, &mut memo);
    delete_modal(&ctx, cx, &lc);
    reaction_picker(&ctx, cx, &lc, &mut memo);
    ctx.data_mut(|d| d.insert_temp(memo_key, memo));
}

/// Whether the "You're viewing older messages" bar shows: scrolled up more than a screen, or
/// paged-in older history is on screen.
fn viewing_older(state: &AppState, ch: Id, memo: &Memo) -> bool {
    let paged = state.messages.get(&ch).is_some_and(|c| c.msgs.len() > crate::state::MAX_MESSAGES_PER_CHANNEL);
    !memo.at_bottom && (memo.from_bottom > memo.view_h || paged)
}

/// Bar above the composer while reading older messages; a click jumps to the newest message
/// (fetching the latest page first when the loaded window doesn't reach it). True when clicked.
fn jump_bar(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, ch: Id) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Jump to present"));
    let r = pal.card_radius();
    // Reaches under the composer below, whose rounded top corners then show the bar's color:
    // bar and box read as one shape.
    let back = Rect::from_min_max(rect.min, rect.max + vec2(0.0, f32::from(r)));
    // A neutral step above the composer; the action reads as a link (ember is not for this).
    let fill = pal.surface.blend(if resp.hovered() { pal.tk.selected } else { pal.tk.hover });
    ui.painter().rect_filled(back, CornerRadius { nw: r, ne: r, sw: 0, se: 0 }, fill);
    let font = FontId::proportional(pal.meta());
    let y = rect.center().y;
    ui.painter().text(pos2(rect.left() + 16.0, y), Align2::LEFT_CENTER, "You're viewing older messages", font, pal.text);
    let color = if resp.hovered() { pal.text } else { pal.link };
    ui.painter().text(pos2(rect.right() - 16.0, y), Align2::RIGHT_CENTER, "Jump to present", FontId::new(pal.meta(), crate::theme::semibold_family()), color);
    if resp.clicked() {
        cx.view.scroll_to_bottom = true;
        cx.state.chat.around = None;
        let newest = cx.state.channels.get(&ch).and_then(|c| c.last_message_id);
        if cx.state.conn == ConnState::Connected
            && let Some(cm) = cx.state.messages.get_mut(&ch)
            && !cm.loading
            && cm.msgs.last().map(|m| m.id) < newest
        {
            cm.loading = true;
            cx.cmd.send(Command::FetchMessages { channel_id: ch, before: None, limit: PAGE_SIZE });
        }
        ui.ctx().request_repaint();
    }
    resp.clicked()
}

fn jump_id() -> egui::Id {
    egui::Id::unique("rc_pending_jump")
}

/// The loaded jump window (`FetchAround`), shown instead of the history: "detached" mode.
fn detached(state: &AppState, ch: Id) -> Option<&[Message]> {
    let v = state.chat.around.as_ref().filter(|v| v.channel_id == ch)?;
    v.result.as_ref()?.as_ref().ok().map(Vec::as_slice)
}

/// The messages on screen: the jump window while detached, else the channel history.
fn shown(state: &AppState, ch: Id) -> &[Message] {
    detached(state, ch).unwrap_or_else(|| state.messages.get(&ch).map_or(&[], |c| &c.msgs))
}

fn in_history(state: &AppState, ch: Id, mid: Id) -> bool {
    state.messages.get(&ch).is_some_and(|c| c.msgs.binary_search_by_key(&mid, |m| m.id).is_ok())
}

/// Jump to a message (reply preview, pin, search hit): opens its channel, then scrolls to the
/// message and flashes it; one that isn't loaded is fetched with the page around it first.
pub(super) fn jump_to(ctx: &egui::Context, cx: &mut UiCx<'_>, ch: Id, mid: Id) {
    if !cx.state.channels.get(&ch).is_some_and(|c| c.is_text_chat()) || !cx.state.can_view(ch) {
        cx.view.toast("That channel can't be opened here.", false);
        return;
    }
    if cx.state.selected_channel != Some(ch) {
        cx.state.select_channel(ch);
        cx.view.settings_open = false;
    }
    if !in_history(cx.state, ch, mid) && !shown(cx.state, ch).iter().any(|m| m.id == mid) {
        cx.state.chat.around = Some(AroundView { channel_id: ch, message_id: mid, result: None });
        cx.cmd.send(Command::Chat(ChatCommand::FetchAround { channel_id: ch, message_id: mid, limit: PAGE_SIZE }));
    }
    ctx.data_mut(|d| d.insert_temp(jump_id(), (ch, mid)));
    ctx.request_repaint();
}

/// Finishes a `jump_to` in the open channel once the shell's selection reset (scroll to the
/// bottom) has run and the message is there. True while it still waits.
fn settle_jump(ctx: &egui::Context, cx: &mut UiCx<'_>, ch: Id) -> bool {
    let Some((jch, mid)) = ctx.data(|d| d.get_temp::<(Id, Id)>(jump_id())) else { return false };
    if jch == ch && cx.view.last_selected != Some(ch) {
        return true;
    }
    if jch == ch {
        absorb_window(cx.state, ch);
        let around = cx.state.chat.around.as_ref().filter(|v| v.channel_id == ch);
        if around.is_some_and(|v| v.result.is_none()) && !in_history(cx.state, ch, mid) {
            return true;
        }
        if in_history(cx.state, ch, mid) || detached(cx.state, ch).is_some_and(|w| w.iter().any(|m| m.id == mid)) {
            if in_history(cx.state, ch, mid) {
                cx.state.chat.around = None;
            }
            cx.view.scroll_to_message = Some(mid);
            cx.view.scroll_to_bottom = false;
        } else {
            // Failed or not found (`apply_chat` toasted): stay in the present.
            cx.state.chat.around = None;
        }
    }
    ctx.data_mut(|d| d.remove_temp::<(Id, Id)>(jump_id()));
    false
}

/// Folds a loaded jump window into the history when the two overlap (or the window reaches the
/// newest message), so the list stays one continuous scroll instead of going detached.
fn absorb_window(state: &mut AppState, ch: Id) {
    let Some(v) = state.chat.around.take_if(|v| v.channel_id == ch) else { return };
    let newest = state.channels.get(&ch).and_then(|c| c.last_message_id);
    let cm = state.messages.entry(ch).or_default();
    match (&v.result, cm.msgs.first().map(|m| m.id)) {
        // Not while the latest page loads: it replaces older cached messages.
        (Some(Ok(page)), Some(first)) if !cm.loading && page.last().is_some_and(|m| m.id >= first) => {
            let older: Vec<Message> = page.iter().take_while(|m| m.id < first).cloned().collect();
            cm.msgs.splice(0..0, older);
            return;
        }
        (Some(Ok(page)), None) if !page.is_empty() && page.last().map(|m| m.id) >= newest => {
            (cm.msgs, cm.loaded, cm.has_more_before) = (page.clone(), true, true);
            return;
        }
        _ => {}
    }
    state.chat.around = Some(v);
}

/// "N new messages since 14:03" over the top of the list while the first unread message is
/// scrolled out above it: a click scrolls to that message, "Mark as read" acks the channel and
/// closes the bar (the "New" divider stays for this visit). A calm `surface` bar; only its "New"
/// tag is red-hot, like the divider's, so an unread channel doesn't read as an error.
fn new_messages_bar(ui: &mut egui::Ui, cx: &mut UiCx<'_>, lc: &Lcx, memo: &mut Memo, list: Rect, (first, count, more): (Id, usize, bool)) {
    let pal = lc.pal;
    let rect = Rect::from_min_size(list.min + vec2(16.0, 0.0), vec2(list.width() - 32.0, 32.0));
    let n = if more { format!("{count}+") } else { count.to_string() };
    let today = chrono::Local::now().date_naive();
    let since = local_time(first.timestamp_ms())
        .map(|t| t.format(if t.date_naive() == today { "%H:%M" } else { "%H:%M, %-d %B" }).to_string())
        .unwrap_or_default();
    let text = format!("{n} new message{} since {since}", if n == "1" { "" } else { "s" });
    let resp = ui.interact(rect, ui.make_persistent_id("rc_new_messages_bar"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    // The action reads as a link, as "Jump to present" does.
    let mark_g = ui.painter().layout_no_wrap("Mark as read".into(), FontId::new(pal.meta(), crate::theme::semibold_family()), pal.link);
    let mark_rect = Rect::from_min_max(pos2(rect.right() - 24.0 - mark_g.size().x, rect.top()), rect.max);
    let mark = ui.interact(mark_rect, ui.make_persistent_id("rc_new_messages_read"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &text));
    mark.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Mark as read"));
    let fill = if resp.hovered() || mark.hovered() { pal.surface.blend(pal.tk.hover) } else { pal.surface };
    let p = ui.painter();
    // A `rule` edge over the scrolling messages (High Contrast needs it); its top row sits on
    // the header's own hairline. Opaque: egui fills only inside the stroke band, so a
    // translucent edge would let the message text under it show through.
    let shape = Rect::from_min_max(rect.min - vec2(0.0, 1.0), rect.max);
    p.rect(shape, CornerRadius { nw: 0, ne: 0, sw: 8, se: 8 }, fill, Stroke::new(1.0, fill.blend(pal.tk.rule)), egui::StrokeKind::Inside);
    let tag = new_tag(p, &pal, pos2(rect.left() + 12.0, rect.center().y), Align2::LEFT_CENTER);
    let text_g = super::one_line(ui, text, FontId::proportional(pal.meta()), pal.text, mark_rect.left() - tag.right() - 16.0);
    let p = ui.painter();
    p.galley(pos2(tag.right() + 8.0, rect.center().y - text_g.size().y / 2.0), text_g, pal.text);
    let mark_pos = pos2(rect.right() - 16.0 - mark_g.size().x, rect.center().y - mark_g.size().y / 2.0);
    if mark.hovered() {
        p.hline(mark_pos.x..=mark_pos.x + mark_g.size().x, mark_pos.y + mark_g.size().y, Stroke::new(1.0, pal.link));
    }
    p.galley(mark_pos, mark_g, pal.link);
    if mark.clicked() {
        super::sidebar::mark_read(cx, [lc.ch]);
        memo.bar_closed = true;
    } else if resp.clicked() {
        cx.view.scroll_to_message = Some(first);
    }
}

/// Test hook `RUSTCORD_MSG_DEMO=<comma list>` for screenshots of states offline mode can't
/// reach, on the newest message: `hover` (toolbar), `menu` (its "…" menu), `reply`, `edit`,
/// `picker`, `delete`, plus `failed` (a failed send), `start` (history start row), `new` (NEW
/// divider above the 3 newest messages), `older` (scrolled up: jump-to-present bar), `emoji`
/// (the composer's emoji picker), `pins` (pins popout listing cached messages), `search` /
/// `indexing` (results panel with cached hits / its indexing state) and `detached` (the older
/// half of the history shown as a jump window: "Jump to present" bar, highlighted target) and
/// `online` (the composer without its "Offline" chip; README screenshots).
pub(super) fn demo(flag: &str) -> bool {
    static FLAGS: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FLAGS.get_or_init(|| std::env::var("RUSTCORD_MSG_DEMO").unwrap_or_default()).split(',').any(|f| f.trim() == flag)
}

fn demo_setup(ctx: &egui::Context, cx: &mut UiCx<'_>, ch: Id, newest: Option<Id>) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    if ONCE.is_completed() || std::env::var_os("RUSTCORD_MSG_DEMO").is_none() {
        return;
    }
    ONCE.call_once(|| {
        // Media (viewer.rs): `media`, `viewer`, `viewer-zoom`, `viewer-video`, `player`, `play`.
        super::viewer::demo(ctx, cx, ch, demo);
        if demo("emoji") {
            ctx.data_mut(|d| d.insert_temp(composer::emoji_picker_id(), 0u64));
        }
        if let Some(mid) = newest {
            let target = Some((ch, mid));
            if demo("reply") {
                cx.view.reply_to = Some((ch, mid, true));
            }
            if demo("edit") {
                cx.view.edit_text = cx.state.messages[&ch].msgs.last().map(|m| m.content.clone()).unwrap_or_default();
                cx.view.editing = target;
            }
            cx.view.reaction_picker = target.filter(|_| demo("picker"));
            cx.view.confirm_delete = target.filter(|_| demo("delete"));
        }
        if demo("failed") {
            let content = "This one didn't make it".to_string();
            cx.state.pending.push(PendingMessage { nonce: "1".into(), channel_id: ch, content, failed: Some("Network unreachable".into()), reply: None });
        }
        if demo("start") && let Some(cm) = cx.state.messages.get_mut(&ch) {
            cm.has_more_before = false;
        }
        if demo("older") {
            cx.view.scroll_to_message = cx.state.messages.get(&ch).and_then(|c| c.msgs.get(4)).map(|m| m.id);
        }
        if demo("new") {
            cx.state.unread_marker = demo_boundary(cx.state, ch).map(|b| (ch, b));
        }
        if demo("pins") {
            pins::demo(ctx, cx, ch);
        }
        if demo("search") || demo("indexing") {
            search::demo(ctx, cx, demo("indexing"));
        }
        if demo("detached")
            && let Some(cm) = cx.state.messages.get_mut(&ch)
            && cm.msgs.len() >= 20
        {
            // The older half becomes a jump window that doesn't reach the (newer) history.
            let window: Vec<Message> = cm.msgs.drain(..cm.msgs.len() / 2).collect();
            let target = window[window.len() / 2].id;
            cx.state.chat.around = Some(AroundView { channel_id: ch, message_id: target, result: Some(Ok(window)) });
            ctx.data_mut(|d| d.insert_temp(jump_id(), (ch, target)));
        }
    });
}

fn centered_hint(ui: &mut egui::Ui, color: Color32, text: &str) {
    ui.centered_and_justified(|ui| ui.label(RichText::new(text).color(color)));
}

/// Chat header over the message area and the member list (the shell adds it before both): type
/// icon, name, topic and the toolbar at the window's right edge: threads (threads.rs), pinned
/// messages (Ctrl+P), the member-list toggle and the search box (Ctrl+F); the inbox sits in the
/// title bar. Nothing while the settings page is open or no channel is.
pub fn chat_header(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    let Some(ch) = cx.state.selected_channel.filter(|_| !cx.view.settings_open) else { return };
    let Some(channel) = cx.state.channels.get(&ch) else { return };
    let (kind, topic) = (channel.kind, channel.topic.clone().unwrap_or_default());
    // Threads and forum posts: "parent › thread" (threads.rs).
    let parent = channel.parent_id.filter(|_| channel.is_thread());
    let header_icon = parent.and_then(|p| cx.state.channels.get(&p)).map_or_else(|| icons::of(channel, cx.state), |p| icons::of(p, cx.state));
    let name = cx.state.channel_name(ch);
    let (pal, name, topic) = (&Pal::new(cx.theme), name.as_str(), topic.as_str());
    // Results of another server or DM close with the switch.
    if cx.state.chat.search.as_ref().is_some_and(|v| Some(v.scope) != search::scope(cx.state)) {
        cx.state.chat.search = None;
    }
    let pressed = |b| super::user_settings::shortcut(cx.settings, b).is_some_and(|k| ui.input_mut(|i| i.consume_shortcut(&k)));
    let (find, pins_key) = (pressed(super::user_settings::Bind::Search), pressed(super::user_settings::Bind::Pins));
    // 49 px incl. the bottom hairline, level with the channel list's header.
    let panel = egui::Panel::top("rc_chat_header")
        .exact_size(49.0)
        .resizable(false)
        .show_separator_line(false)
        .frame(Frame::new().fill(pal.bg).inner_margin(Margin { left: 16, right: 12, top: 0, bottom: 0 }))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if matches!(kind, channel_type::DM | channel_type::GROUP_DM) {
                    // The other user's avatar (with their status when known) or the group's icon.
                    let (slot, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
                    if super::sidebar::dm_avatar_at(ui, cx, ch, slot).is_some()
                        && let Some(&u) = cx.state.channels.get(&ch).and_then(|c| c.recipient_ids.first())
                    {
                        super::sidebar::user_dot(ui.painter(), slot, cx.state, u, &cx.theme.colors, pal.bg);
                    }
                } else {
                    // Fixed icon slot so every channel title starts at one x.
                    let (slot, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                    icons::paint_channel(ui.painter(), header_icon, slot, pal.muted, pal.bg);
                }
                if let Some(p) = parent {
                    super::threads::breadcrumb(ui, cx, pal, p);
                }
                // Search box width as on the web: clamp(64px, 100% - 320px, 244px), at least 96 here.
                let search_w = (ui.available_width() - 320.0).clamp(96.0, 244.0);
                // No toggle without a member list or when the window is too narrow for it anyway.
                let toggle = super::members::available(cx.state) && ui.ctx().content_rect().width() >= super::MEMBER_LIST_MIN_WINDOW_W;
                let threads = super::threads::has_threads(kind);
                // Pins always; threads and the member toggle when shown.
                let tools_w = search_w + 48.0 + if toggle { 40.0 } else { 0.0 } + if threads { 40.0 } else { 0.0 } + super::dm_call::header_w(cx.state, ch);
                // Heading 16/600, a 1x20 rule, the topic in the ui role (docs/DESIGN.md §10.3).
                let title = RichText::new(name).font(FontId::new(pal.heading(), crate::theme::semibold_family())).color(pal.text);
                ui.scope(|ui| {
                    ui.set_max_width((ui.available_width() - tools_w).max(40.0));
                    ui.add(Label::new(title).selectable(false).truncate());
                });
                let first = topic.lines().next().unwrap_or_default().trim();
                if !first.is_empty() && ui.available_width() > tools_w + 80.0 {
                    let (r, _) = ui.allocate_exact_size(vec2(1.0, 20.0), Sense::hover());
                    ui.painter().vline(r.center().x, r.y_range(), Stroke::new(1.0, pal.tk.rule));
                    let w = (ui.available_width() - tools_w).max(0.0);
                    ui.scope(|ui| {
                        ui.set_max_width(w);
                        let text = RichText::new(first).size(pal.ui()).color(pal.muted);
                        ui.add(Label::new(text).selectable(false).truncate()).on_hover_text(topic);
                    });
                }
                // Right to left: the search box rightmost.
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    // A scope: the box's text field must not move the row's cursor, so the
                    // buttons keep the 8 px gap to the whole well.
                    ui.scope(|ui| search::search_box(ui, cx, pal, search_w, find));
                    if toggle {
                        let on = cx.settings.show_member_list;
                        let hint = if on { "Hide member list" } else { "Show member list" };
                        // The shell subscribes to the member list once it is switched on.
                        if icon_button(ui, pal, "👥", hint, 32.0, on, pal.text).clicked() {
                            cx.settings.show_member_list = !on;
                            if let Err(e) = cx.settings.save() {
                                cx.view.toast(format!("Could not save settings: {e}"), true);
                            }
                            ui.ctx().request_repaint();
                        }
                    }
                    pins::button(ui, cx, pal, ch, pins_key);
                    super::dm_call::header_buttons(ui, cx, pal, ch);
                    if threads {
                        super::threads::header_button(ui, cx, pal, ch);
                    }
                });
            });
        });
    let r = panel.response.rect;
    ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, pal.tk.rule));
}

// ---------------------------------------------------------------------------
// Message list
// ---------------------------------------------------------------------------

fn message_list(ui: &mut egui::Ui, cx: &mut UiCx<'_>, lc: &Lcx, memo: &mut Memo) {
    if memo.channel != Some(lc.ch) {
        let heights = std::mem::take(&mut memo.heights);
        *memo = Memo { channel: Some(lc.ch), heights, at_bottom: true, to_bottom: lc.panel, ..Memo::default() };
    }
    if memo.heights.len() > 4000
        && let Some(cm) = cx.state.messages.get(&lc.ch)
    {
        let keep: HashSet<Id> = cm.msgs.iter().map(|m| m.id).collect();
        memo.heights.retain(|id, _| keep.contains(id));
    }
    // Back at the bottom after paging into older history: drop it again (state keeps it while read).
    if memo.at_bottom && !lc.detached && cx.state.messages.get(&lc.ch).is_some_and(|c| c.msgs.len() > crate::state::MAX_MESSAGES_PER_CHANNEL) {
        cx.state.trim_history(lc.ch);
    }
    let area = ScrollArea::vertical()
        .id_salt(("rc_messages", lc.ch.0))
        .auto_shrink(false)
        .stick_to_bottom(true)
        // Scroll corrections must land at once, not animate over several frames.
        .animated(false)
        // Squeezed (a DM call area over a short window): never taller than the room, which
        // would paint messages over the composer.
        .min_scrolled_height(0.0)
        .content_margin(Margin::ZERO);
    let out = area.show_viewport(ui, |ui, viewport| list_contents(ui, cx, lc, memo, viewport));
    if let Some(bar) = memo.new_bar.filter(|_| !memo.bar_closed) {
        new_messages_bar(ui, cx, lc, memo, out.inner_rect, bar);
    }
    let max_offset = (out.content_size.y - out.inner_rect.height()).max(0.0);
    memo.at_bottom = out.state.clamped_offset().y >= max_offset - 2.0;
    memo.from_bottom = max_offset - out.state.clamped_offset().y;
    memo.view_h = out.inner_rect.height();
    #[cfg(test)]
    {
        memo.offset = out.state.clamped_offset().y;
    }
}

/// `RUSTCORD_MSG_DEMO=new`: the 3 newest messages count as unread.
fn demo_boundary(state: &AppState, ch: Id) -> Option<Id> {
    let msgs = &state.messages.get(&ch)?.msgs;
    demo("new").then(|| msgs.len().checked_sub(4).map(|i| msgs[i].id)).flatten()
}

fn list_contents(ui: &mut egui::Ui, cx: &mut UiCx<'_>, lc: &Lcx, memo: &mut Memo, viewport: Rect) {
    let ctx = ui.ctx().clone();
    ui.set_width(ui.available_width());
    ui.spacing_mut().item_spacing.y = 0.0;
    // Content coordinates (same space as `viewport`) = screen y - origin.
    let origin = ui.max_rect().top();
    let width = ui.available_width();
    let want_bottom = lc.to_bottom(cx.view, memo);
    let now = ctx.input(|i| i.time);

    top_row(ui, cx, lc);

    let n = shown(cx.state, lc.ch).len();
    let more_before = lc.detached || cx.state.messages.get(&lc.ch).is_some_and(|c| c.has_more_before);
    let toolbar_hit = memo.toolbar.filter(|(_, r)| ui.rect_contains_pointer(*r)).map(|(id, _)| id);
    let flash = memo.flash.filter(|&(_, until)| now < until);
    let editing = cx.view.editing.filter(|(c, _)| *c == lc.ch).map(|(_, m)| m);
    let key = layout_key(ui, cx, width);
    // egui drops the focus of a widget missing from a pass, so its row stays laid out: by id (the
    // last pass's `focus_row`, which older history prepended above doesn't move) and by the
    // widget's last rect, in that pass's content coordinates (focus an arrow key or Shift+Tab
    // moved between passes). A widget already drawn this pass is outside the list.
    let focus_y = ctx.memory(|m| m.focused()).and_then(|f| {
        ctx.viewport(|vp| (!vp.this_pass.widgets.contains(f)).then(|| vp.prev_pass.widgets.get(f).map(|w| w.rect.center().y - memo.origin)).flatten())
    });
    let focus_row = memo.focus_row;
    memo.origin = origin;
    // A screen reader walks the widgets of every laid-out row, Tab and the arrow keys those of
    // this pass (egui `Focus::begin_pass`).
    let all = ctx.has_accesskit_node(egui::accesskit_root_id()) || ctx.input(|i| i.raw.events.iter().any(moves_focus));
    let mut rows: Vec<(Id, f32, f32)> = Vec::with_capacity(n);
    let mut hover_row = None;
    let mut last_author = None;
    for i in 0..n {
        let (id, group_start, seps, fp) = {
            let msgs = shown(cx.state, lc.ch);
            let Some(m) = msgs.get(i) else { break };
            if crate::plugins::chat::hide_message(cx.state, m) {
                continue;
            }
            let prev = i.checked_sub(1).and_then(|j| msgs.get(j));
            let date_sep = with_times(&ctx, |t| prev.is_none_or(|p| t.day(p.id) != t.day(m.id)));
            let group_start = is_group_start(prev, m, date_sep);
            // First unread message; at the list top only if nothing older can be paged in.
            let new_sep = lc.unread_after.is_some_and(|b| m.id > b && prev.map_or(!more_before, |p| p.id <= b));
            let revealed = cx.view.revealed_spoilers.iter().filter(|(mid, _)| *mid == m.id).count();
            let layout = (lc.compact, lc.pal.size.to_bits(), group_start, date_sep, new_sep, revealed, editing == Some(m.id));
            last_author = Some(m.author.id);
            (m.id, group_start, (date_sep, new_sep), fingerprint(m, layout))
        };
        let top = ui.cursor().top() - origin;
        let cached = memo.heights.get(&id).filter(|r| r.fp == fp);
        let pinned = |h: f32| {
            all || [lc.menu_target, lc.demo_hover, toolbar_hit, focus_row].contains(&Some(id)) || focus_y.is_some_and(|y| (top..=top + h).contains(&y))
        };
        if let Some(&RowHeight { h, .. }) = cached.filter(|r| editing != Some(id) && keeps_height(r, top, viewport, key, now, pinned(r.h))) {
            ui.push_id(id.0, |ui| ui.allocate_space(vec2(width, h)));
            rows.push((id, top, h));
            continue;
        }
        let msg = shown(cx.state, lc.ch)[i].clone();
        let flashing = flash.filter(|(f, _)| *f == id).map_or(0.0, |(_, until)| ((until - now) / 0.5).clamp(0.0, 1.0) as f32);
        let hovered = ui
            .push_id(id.0, |ui| message_block(ui, cx, lc, memo, &msg, group_start, seps, toolbar_hit, flashing))
            .inner;
        let h = ui.cursor().top() - origin - top;
        memo.heights.insert(id, RowHeight { fp, h, key, at: now });
        rows.push((id, top, h));
        if hovered.is_some() {
            hover_row = hovered;
        }
    }
    // Drawn after this list's own Ui in its layer: inside the list.
    let list = ui.unique_id();
    memo.focus_row = ctx
        .memory(|m| m.focused())
        .and_then(|f| {
            ctx.viewport(|vp| {
                let w = &vp.this_pass.widgets;
                let ((layer, i), (list_layer, j)) = (w.order(f)?, w.order(list)?);
                (layer == list_layer && i > j).then(|| w.get(f).map(|r| r.rect.center().y - origin)).flatten()
            })
        })
        .and_then(|y| rows.iter().find(|r| (r.1..=r.1 + r.2).contains(&y)).map(|r| r.0));

    if !lc.detached {
        pending_rows(ui, cx, lc, last_author);
    }
    ui.add_space(16.0);

    memo.new_bar = lc.unread_after.filter(|_| !lc.detached).and_then(|b| {
        let msgs = &cx.state.messages.get(&lc.ch)?.msgs;
        let i = msgs.iter().position(|m| m.id > b)?;
        let top = rows.iter().find(|r| r.0 == msgs[i].id)?.1;
        (top < viewport.min.y).then_some((msgs[i].id, msgs.len() - i, i == 0 && more_before))
    });

    memo.hovered = hover_row.as_ref().map(|h| h.id);
    if memo.hovered != lc.hovered {
        ctx.request_repaint();
    }
    memo.toolbar = None;
    if let Some(h) = &hover_row {
        hover_toolbar(ui, cx, lc, memo, h);
    }
    if let Some((_, until)) = flash {
        // The highlight fades out over its last half second.
        let left = until - now;
        ctx.request_repaint_after(Duration::from_secs_f64(if left > 0.5 { left - 0.5 } else { 1.0 / 30.0 }));
    }

    let content_h = ui.cursor().top() - origin;
    let view_top = resolve_scroll(ui, cx, lc, memo, &rows, viewport, content_h, now);
    if !want_bottom && !lc.detached && view_top < LOAD_MORE_PX {
        load_older(cx, lc);
    }
}

/// Applies scroll requests and keeps the reading position stable. Returns the viewport top
/// the list settles at.
#[allow(clippy::too_many_arguments)]
fn resolve_scroll(
    ui: &egui::Ui,
    cx: &mut UiCx<'_>,
    lc: &Lcx,
    memo: &mut Memo,
    rows: &[(Id, f32, f32)],
    viewport: Rect,
    content_h: f32,
    now: f64,
) -> f32 {
    let view_h = viewport.height();
    let max_offset = (content_h - view_h).max(0.0);
    let mut target = None;
    let to_bottom = lc.to_bottom(cx.view, memo);
    if to_bottom {
        target = Some(max_offset);
        // Keep the request until the channel has something to show.
        if cx.state.messages.get(&lc.ch).is_some_and(|c| c.loaded || !c.msgs.is_empty()) {
            if lc.panel {
                memo.to_bottom = false;
            } else {
                cx.view.scroll_to_bottom = false;
            }
        }
    } else if let Some(mid) = cx.view.scroll_to_message.take_if(|_| !lc.panel) {
        match rows.iter().find(|r| r.0 == mid) {
            Some(&(_, top, h)) => {
                target = Some((top - (view_h - h) / 2.0).clamp(0.0, max_offset));
                memo.flash = Some((mid, now + FLASH_S));
            }
            None => cx.view.toast("That message isn't loaded. Scroll up to load older messages.", false),
        }
    } else if !memo.at_bottom
        && let Some((anchor, old_top)) = memo.anchor
        && let Some(&(_, top, _)) = rows.iter().find(|r| r.0 == anchor)
        && (top - old_top).abs() > 0.5
    {
        target = Some((viewport.min.y + top - old_top).clamp(0.0, max_offset));
    }
    let view_top = target.unwrap_or(viewport.min.y);
    memo.anchor = rows.iter().find(|r| r.1 + r.2 > view_top).map(|r| (r.0, r.1));
    if let Some(y) = target
        && (y - viewport.min.y).abs() > 0.5
    {
        // An explicit delta (unlike a forced offset) also wins over stick-to-bottom. Going to
        // the bottom overshoots so egui clamps exactly to the end and keeps sticking there.
        let y = if to_bottom { y + view_h } else { y };
        ui.scroll_with_delta(vec2(0.0, viewport.min.y - y));
        // Redo the pass at the corrected offset instead of showing a one-frame jump.
        ui.ctx().request_discard("message list scroll position");
    }
    view_top
}

fn load_older(cx: &mut UiCx<'_>, lc: &Lcx) {
    if cx.state.conn != ConnState::Connected {
        return;
    }
    let Some(cm) = cx.state.messages.get_mut(&lc.ch) else { return };
    // Errors wait for the Retry button instead of re-requesting every frame.
    if !cm.loaded || !cm.has_more_before || cm.loading || cm.error.is_some() {
        return;
    }
    let Some(oldest) = cm.msgs.first().map(|m| m.id) else { return };
    cm.loading = true;
    cx.cmd.send(Command::FetchMessages { channel_id: lc.ch, before: Some(oldest), limit: PAGE_SIZE });
}

fn top_row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, lc: &Lcx) {
    let pal = lc.pal;
    if lc.detached {
        ui.add_space(48.0);
        return;
    }
    let (loading, error, more, loaded, empty) = cx
        .state
        .messages
        .get(&lc.ch)
        .map_or((false, None, false, false, true), |c| {
            (c.loading, c.error.clone(), c.has_more_before, c.loaded, c.msgs.is_empty())
        });
    if loading {
        ui.vertical_centered(|ui| {
            ui.add_space(14.0);
            ui.spinner();
            ui.add_space(14.0);
        });
    } else if let Some(err) = error {
        Frame::new().inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Couldn't load messages").color(pal.danger)).on_hover_text(err);
                if small_button(ui, &pal, "Retry", pal.text).clicked() {
                    let newest_known = cx.state.channels.get(&lc.ch).and_then(|c| c.last_message_id);
                    let cm = cx.state.messages.entry(lc.ch).or_default();
                    cm.error = None;
                    cm.loading = true;
                    // The failed request may have been the latest page; then older history
                    // would leave the cache stale.
                    let latest_missing = cm.msgs.last().map(|m| m.id) < newest_known;
                    let before = if latest_missing { None } else { cm.msgs.first().map(|m| m.id) };
                    cx.cmd.send(Command::FetchMessages { channel_id: lc.ch, before, limit: PAGE_SIZE });
                    ui.ctx().request_repaint();
                }
            });
        });
    } else if loaded && !more {
        start_block(ui, cx, lc, None);
    } else if empty && !loaded && cx.state.conn != ConnState::Connected {
        let text = if crate::config::offline_mode() || cx.state.conn == ConnState::Offline { "Messages load when you're connected." } else { "Connecting…" };
        if matches!(lc.kind, channel_type::DM | channel_type::GROUP_DM) {
            // A conversation without cached messages still opens on its start (§13).
            start_block(ui, cx, lc, Some(text));
            return;
        }
        // The shared empty state: one muted meta line, centred in the pane (minus the list's
        // 16 px bottom space, so nothing scrolls). Floored: with fractional pane heights (125 %
        // scaling) a sub-pixel overflow made the scroll-to-bottom correction redo every pass.
        let size = vec2(ui.available_width(), (ui.available_height() - 16.0).floor().max(48.0));
        ui.add_sized(size, Label::new(RichText::new(text).size(pal.meta()).color(pal.muted)).selectable(false));
    } else {
        ui.add_space(48.0);
    }
}

/// The start of the history: a DM shows the other person's 80 px avatar (a group its icon) over
/// the name in the display role, a channel "#name"; then one sentence and the optional `note`.
fn start_block(ui: &mut egui::Ui, cx: &mut UiCx<'_>, lc: &Lcx, note: Option<&str>) {
    let pal = lc.pal;
    let dm = matches!(lc.kind, channel_type::DM | channel_type::GROUP_DM);
    let sub = match lc.kind {
        channel_type::DM => format!("This is the start of your conversation with {}.", lc.name),
        channel_type::GROUP_DM => format!("This is the start of the {} group.", lc.name),
        _ => format!("This is the start of the #{} channel.", lc.name),
    };
    Frame::new().inner_margin(Margin { left: 16, right: 16, top: 24, bottom: 8 }).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        if dm {
            let (slot, _) = ui.allocate_exact_size(Vec2::splat(80.0), Sense::hover());
            super::sidebar::dm_avatar_at(ui, cx, lc.ch, slot);
            ui.add_space(8.0);
        }
        let label = if dm { lc.name.clone() } else { format!("{}{}", channel_prefix(lc.kind), lc.name) };
        ui.add(Label::new(RichText::new(label).family(bold_family()).size(pal.display()).color(pal.text)).selectable(false));
        ui.add(Label::new(RichText::new(sub).color(pal.muted)).selectable(false));
        if let Some(note) = note {
            ui.add_space(4.0);
            ui.add(Label::new(RichText::new(note).size(pal.meta()).color(pal.muted)).selectable(false));
        }
    });
}

/// Day label centred on a hairline and/or the unread marker (a `danger` line with the "New" tag
/// at its right end); one line when both fall on the same message.
fn separator(ui: &mut egui::Ui, pal: &Pal, id: Id, date: bool, new: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), if date { 40.0 } else { 16.0 }), Sense::hover());
    let p = ui.painter();
    // On a pixel centre: a 1 px line between two pixel rows blurs into a faint 2 px band.
    let (color, y) = (if new { pal.danger } else { pal.muted }, p.round_to_pixel_center(rect.center().y));
    let stroke = Stroke::new(1.0, if new { pal.danger } else { pal.tk.rule });
    let mut right = rect.right() - 16.0;
    if new {
        right = new_tag(p, pal, pos2(right, y), Align2::RIGHT_CENTER).left();
    }
    if date {
        let galley = p.layout_no_wrap(day_label(ui.ctx(), id), FontId::new(pal.meta(), crate::theme::semibold_family()), color);
        let (mid, half) = (rect.center().x, galley.size().x / 2.0);
        p.hline(rect.left() + 16.0..=mid - half - 12.0, y, stroke);
        p.hline(mid + half + 12.0..=right, y, stroke);
        p.galley(pos2(mid - half, y - galley.size().y / 2.0), galley, color);
    } else {
        p.hline(rect.left() + 16.0..=right, y, stroke);
    }
}

/// The "New" tag (badge role on `danger_fill`, chip radius, sentence case), placed by `align`
/// at `at`. Returns its rect.
fn new_tag(p: &egui::Painter, pal: &Pal, at: egui::Pos2, align: Align2) -> Rect {
    let g = p.layout_no_wrap("New".into(), FontId::new(pal.badge(), bold_family()), pal.tk.danger_label);
    let tag = align.anchor_size(at, vec2(g.size().x + 10.0, 16.0));
    p.rect_filled(tag, pal.radius / 2, pal.tk.danger_fill);
    p.galley(tag.center() - g.size() / 2.0, g, pal.tk.danger_label);
    tag
}

/// One message (with its date separator). Returns the row if it is hovered.
#[allow(clippy::too_many_arguments)]
fn message_block(
    ui: &mut egui::Ui,
    cx: &mut UiCx<'_>,
    lc: &Lcx,
    memo: &mut Memo,
    msg: &Message,
    group_start: bool,
    (date_sep, new_sep): (bool, bool),
    toolbar_hit: Option<Id>,
    flashing: f32,
) -> Option<HoverRow> {
    let pal = lc.pal;
    if date_sep || new_sep {
        separator(ui, &pal, msg.id, date_sep, new_sep);
    }
    // A thread's starter stub shows the message the thread was created from.
    let stub = msg.kind == 21;
    let starter;
    let msg = match &msg.referenced_message {
        Some(r) if stub => {
            starter = Message {
                id: msg.id,
                channel_id: msg.channel_id,
                reactions: msg.reactions.clone(),
                message_reference: None,
                ..(**r).clone()
            };
            &starter
        }
        _ => msg,
    };
    let system = system_line(cx.state, lc, msg);
    // A separator already spaces the group it opens.
    if group_start && !date_sep && !new_sep {
        ui.add_space(if lc.compact { 4.0 } else { cx.settings.message_group_spacing });
    }
    let hovered_before = lc.hovered == Some(msg.id);
    let bg = ui.painter().add(Shape::Noop);
    let rect = Frame::new()
        .inner_margin(Margin { left: 16, right: 16, top: 2, bottom: 2 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
            // Text rows as tall as the text: the default 22 px min row height loosens the layout.
            ui.spacing_mut().interact_size.y = ui.text_style_height(&egui::TextStyle::Body);
            match &system {
                Some((text, author)) => system_row(ui, cx, lc, msg, text, author),
                None if lc.compact => compact_row(ui, cx, lc, memo, msg, hovered_before),
                None => cozy_row(ui, cx, lc, memo, msg, group_start, hovered_before),
            }
        })
        .response
        .rect;
    let hovered = match (lc.menu_target, toolbar_hit) {
        (Some(id), _) | (None, Some(id)) => id == msg.id,
        (None, None) => ui.rect_contains_pointer(rect) || lc.demo_hover == Some(msg.id),
    };
    // Right-click anywhere on the row (not in its own edit box) opens the message menu.
    if cx.view.editing != Some((lc.ch, msg.id))
        && ui.rect_contains_pointer(rect)
        && let Some(pos) = ui.input(|i| i.pointer.secondary_clicked().then(|| i.pointer.interact_pos()).flatten())
    {
        memo.menu = Some(MsgMenu { id: msg.id, anchor: Rect::from_pos(pos), from_button: false, new: true });
        ui.ctx().request_repaint();
    }
    let mut shapes = Vec::new();
    if flashing > 0.0 {
        // A jumped-to message: the navigation colour (`link`), fading out.
        shapes.push(Shape::rect_filled(rect, 0.0, pal.link.gamma_multiply(0.18 * flashing)));
    } else if crate::plugins::message_logger::is_deleted(msg.id) {
        // MessageLogger: a deleted message keeps a faint red tint.
        shapes.push(Shape::rect_filled(rect, 0.0, pal.danger.gamma_multiply(if hovered { 0.16 } else { 0.1 })));
    } else if system.is_none() && cx.state.mentions_me(msg) {
        // Mentions me: the `mention_highlight` row and a 2 px straw (`attention`) bar.
        let tint = if hovered { pal.mention.blend(pal.tk.hover) } else { pal.mention };
        shapes.push(Shape::rect_filled(rect, 0.0, tint));
        shapes.push(Shape::rect_filled(Rect::from_min_size(rect.min, vec2(2.0, rect.height())), 0.0, pal.tk.attention));
    } else if hovered {
        shapes.push(Shape::rect_filled(rect, 0.0, pal.hover));
    }
    ui.painter().set(bg, Shape::Vec(shapes));
    hovered.then(|| HoverRow {
        id: msg.id,
        rect,
        own: lc.me == Some(msg.author.id),
        system: system.is_some() || stub,
        content: msg.content.clone(),
    })
}

/// A system line: a monochrome icon in the gutter (the call handset green, joins green, leaves
/// red), the text muted with the author (`AUTHOR_MARK`) in bold body colour, then the time.
fn system_row(ui: &mut egui::Ui, cx: &UiCx<'_>, lc: &Lcx, msg: &Message, text: &str, author: &str) {
    let pal = lc.pal;
    let green = super::presence_color(crate::model::Presence::Online, &cx.theme.colors);
    let color = match msg.kind {
        1 | 3 | 7 => green,
        2 => pal.danger,
        _ => pal.muted,
    };
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let (r, _) = ui.allocate_exact_size(vec2(lc.gutter(), 18.0), Sense::hover());
        let c = pos2(if lc.compact { r.left() + 8.0 } else { r.left() + AVATAR / 2.0 }, r.center().y);
        match system_icon(msg.kind) {
            Some(icon) => icons::paint(ui.painter(), icon, Rect::from_center_size(c, Vec2::splat(16.0)), color, pal.bg),
            None => {
                ui.painter().circle_filled(c, 2.5, color);
            }
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let mut job = egui::text::LayoutJob::default();
            let muted = egui::TextFormat::simple(FontId::proportional(pal.size), pal.muted);
            match text.split_once(AUTHOR_MARK) {
                Some((before, after)) => {
                    job.append(before, 0.0, muted.clone());
                    job.append(author, 0.0, egui::TextFormat::simple(FontId::new(pal.size, bold_family()), pal.text));
                    job.append(after, 0.0, muted);
                }
                None => job.append(text, 0.0, muted),
            }
            ui.label(job);
            ui.label(RichText::new(time_labels(ui.ctx(), &msg.timestamp).header.as_str()).size(pal.meta()).color(pal.muted));
        });
    });
}

fn cozy_row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, lc: &Lcx, memo: &mut Memo, msg: &Message, group_start: bool, hovered: bool) {
    let member = message_member(cx.state, lc.guild_id, msg);
    reply_line(ui, cx, lc, msg);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        if group_start {
            let animate = lc.animate(msg.author.id, hovered);
            let url = images::user_avatar_url(lc.guild_id, &msg.author, member.as_ref(), 80, animate);
            let name = cx.state.member_display_name(lc.guild_id, &msg.author, member.as_ref());
            let r = images::avatar(ui, cx.images, &url, AVATAR, animate, Some((msg.author.id, &name))).on_hover_cursor(CursorIcon::PointingHand);
            let deco = crate::overrides::effective_for(current_user(cx.state, &msg.author), member.as_ref()).decoration;
            images::decoration_at(ui, cx.images, r.rect, deco.as_ref(), lc.animate_always || hovered, 1.0);
            if r.clicked() {
                cx.view.open_profile(msg.author.id, lc.guild_id, r.rect.right_top() + vec2(8.0, 0.0));
            }
            ui.add_space(GUTTER - AVATAR);
        } else {
            // Time of a grouped message on hover, in the gutter on its first text line, ending
            // at the avatar column's right edge.
            let (r, resp) = ui.allocate_exact_size(vec2(GUTTER, ui.text_style_height(&egui::TextStyle::Body)), Sense::hover());
            if hovered {
                let pos = pos2(r.left() + AVATAR, r.center().y);
                let t = time_labels(ui.ctx(), &msg.timestamp);
                ui.painter().text(pos, Align2::RIGHT_CENTER, &t.short, FontId::proportional(lc.pal.meta() - 1.0), lc.pal.muted);
                resp.on_hover_text(t.full.as_str());
            }
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 2.0);
            if group_start {
                let bg = row_bg(cx, &lc.pal, msg);
                let t = time_labels(ui.ctx(), &msg.timestamp);
                author_line(ui, cx, lc, &msg.author, member.as_ref(), (&t.header, &t.full), bg);
            }
            body(ui, cx, lc, memo, msg, hovered);
        });
    });
}

fn compact_row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, lc: &Lcx, memo: &mut Memo, msg: &Message, hovered: bool) {
    let pal = lc.pal;
    let member = message_member(cx.state, lc.guild_id, msg);
    reply_line(ui, cx, lc, msg);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let row_h = ui.text_style_height(&egui::TextStyle::Body);
        let (r, resp) = ui.allocate_exact_size(vec2(COMPACT_GUTTER, row_h), Sense::hover());
        let t = time_labels(ui.ctx(), &msg.timestamp);
        ui.painter().text(r.left_center(), Align2::LEFT_CENTER, &t.short, FontId::proportional(pal.meta()), pal.muted);
        resp.on_hover_text(t.full.as_str());
        // Compact puts the tag before the name.
        if msg.author.bot {
            // The app tag: `chip` fill, `link` badge text, chip radius.
            let g = ui.painter().layout_no_wrap("Bot".into(), FontId::new(pal.badge(), bold_family()), pal.link);
            let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 8.0, row_h), Sense::hover());
            let chip = Rect::from_center_size(r.center(), vec2(r.width(), g.size().y + 2.0));
            ui.painter().rect_filled(chip, pal.radius / 2, pal.tk.chip);
            ui.painter().galley(chip.center() - g.size() / 2.0, g, pal.link);
            ui.add_space(6.0);
        }
        let name = cx.state.member_display_name(lc.guild_id, &msg.author, member.as_ref());
        let color = name_color(cx, lc.guild_id, member.as_ref(), row_bg(cx, &pal, msg)).unwrap_or(pal.text);
        let style = crate::overrides::effective_for(current_user(cx.state, &msg.author), member.as_ref()).name_style;
        let name_g = super::cosmetics::name_galley(ui, &format!("{name}:"), FontId::new(pal.size, crate::theme::semibold_family()), style.as_ref(), color, f32::INFINITY);
        let r = ui.add(Label::new(name_g).selectable(false).sense(Sense::click())).on_hover_cursor(CursorIcon::PointingHand);
        if r.clicked() {
            cx.view.open_profile(msg.author.id, lc.guild_id, r.rect.right_top() + vec2(8.0, 0.0));
        }
        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
            body(ui, cx, lc, memo, msg, hovered);
        });
    });
}

/// `bg`: the colour under the line (`row_bg`).
fn author_line(ui: &mut egui::Ui, cx: &mut UiCx<'_>, lc: &Lcx, user: &User, member: Option<&Member>, (time, time_tip): (&str, &str), bg: Color32) {
    let pal = lc.pal;
    let name = cx.state.member_display_name(lc.guild_id, user, member);
    let color = name_color(cx, lc.guild_id, member, bg).or_else(|| crate::plugins::people::irc_color(lc.guild_id, user.id, pal.dark).map(|c| cx.theme.readable_on_pane(c, bg))).unwrap_or(pal.text);
    // Painted galleys instead of labels: labels of different sizes are centered, not
    // baseline-aligned.
    let p = ui.painter().clone();
    // Cosmetics (ui/cosmetics.rs): display-name style, guild tag chip after the name.
    let current = current_user(cx.state, user);
    let style = crate::overrides::effective_for(current, member).name_style;
    // Author 15/600 (docs/DESIGN.md §10.3).
    let name_g = super::cosmetics::name_galley(ui, &name, FontId::new(pal.size, crate::theme::semibold_family()), style.as_ref(), color, f32::INFINITY);
    let tag = super::cosmetics::guild_tag(current, member).cloned();
    let time_g = p.layout_no_wrap(time.to_owned(), FontId::proportional(pal.meta()), pal.muted);
    let bot_g = user.bot.then(|| p.layout_no_wrap("Bot".into(), FontId::new(pal.badge(), bold_family()), pal.link));
    let h = name_g.size().y;
    let base = baseline(&name_g);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.set_row_height(h);
        let (r, resp) = ui.allocate_exact_size(name_g.size(), Sense::click());
        let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
        if resp.hovered() {
            p.hline(r.x_range(), r.top() + base + 2.0, Stroke::new(1.0, color));
        }
        super::cosmetics::paint_name(&p, r.min, name_g, style.as_ref(), color, ui.visuals().dark_mode);
        if resp.clicked() {
            cx.view.open_profile(user.id, lc.guild_id, r.right_top() + vec2(8.0, 0.0));
        }
        if let Some(pg) = &tag {
            super::cosmetics::tag_button(ui, cx, pg, pal.size, h, pal.text);
        }
        if let Some(g) = bot_g {
            let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 8.0, h), Sense::hover());
            let top = r.top() + base - baseline(&g);
            p.rect_filled(Rect::from_min_size(pos2(r.left(), top - 1.0), vec2(r.width(), g.size().y + 2.0)), pal.radius / 2, pal.tk.chip);
            p.galley(pos2(r.left() + 4.0, top), g, pal.link);
        }
        let (r, resp) = ui.allocate_exact_size(vec2(time_g.size().x, h), Sense::hover());
        p.galley(pos2(r.left(), r.top() + base - baseline(&time_g)), time_g, pal.muted);
        if !time_tip.is_empty() {
            resp.on_hover_text(time_tip);
        }
        // Plugins: username, tags, platforms, voice, timeout after the time.
        crate::plugins::people::after_name(ui, cx, lc.guild_id, user, member, h);
    });
}

/// "↱ @name preview" line above a reply; click jumps to the original.
fn reply_line(ui: &mut egui::Ui, cx: &mut UiCx<'_>, lc: &Lcx, msg: &Message) {
    if !is_reply(msg) {
        return;
    }
    let pal = lc.pal;
    let line = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.set_row_height(18.0);
        let (r, _) = ui.allocate_exact_size(vec2(lc.gutter() - 6.0, 18.0), Sense::hover());
        if !lc.compact {
            // Spine from above the avatar to the quoted line.
            let (x, y) = (r.left() + AVATAR / 2.0, r.center().y);
            let points = vec![pos2(x, r.bottom() + 4.0), pos2(x, y + 6.0), pos2(x + 6.0, y), pos2(r.right() - 2.0, y)];
            ui.painter().add(Shape::line(points, Stroke::new(2.0, pal.muted.gamma_multiply(0.35))));
        }
        match msg.referenced_message.as_deref() {
            Some(rm) => {
                let member = message_member(cx.state, lc.guild_id, rm);
                let name = cx.state.member_display_name(lc.guild_id, &rm.author, member.as_ref());
                if !lc.compact {
                    let url = images::user_avatar_url(lc.guild_id, &rm.author, member.as_ref(), 32, false);
                    images::avatar(ui, cx.images, &url, 16.0, false, Some((rm.author.id, &name)));
                }
                let color = name_color(cx, lc.guild_id, member.as_ref(), row_bg(cx, &pal, msg)).unwrap_or(pal.text);
                ui.add(Label::new(RichText::new(name).size(pal.meta()).family(bold_family()).color(color)).selectable(false));
                let mut preview = resolve_tokens(&reply_preview(rm), |tok| token_name(cx.state, lc.guild_id, &rm.mentions, tok));
                while let Some((r, code)) = missing_emoji(ui, &preview) {
                    preview.replace_range(r, &format!(":{code}:"));
                }
                crate::plugins::chat::reply_time(ui, rm, pal.meta() - 1.0, pal.muted);
                ui.add(Label::new(RichText::new(preview).size(pal.meta()).color(pal.muted)).selectable(false).truncate());
            }
            None => {
                ui.add(Label::new(RichText::new("Original message was deleted").size(pal.meta()).italics().color(pal.muted)).selectable(false));
            }
        }
    });
    if let Some(rm) = &msg.referenced_message {
        // Registered after the line's children, so it wins the click anywhere on the line.
        let id = ui.make_persistent_id(("rc_reply_jump", msg.id.0));
        let resp = ui.interact(line.response.rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        if resp.on_hover_text("Jump to message").clicked() {
            jump_to(ui.ctx(), cx, lc.ch, rm.id);
        }
    }
}

fn body(ui: &mut egui::Ui, cx: &mut UiCx<'_>, lc: &Lcx, memo: &mut Memo, msg: &Message, hovered: bool) {
    let pal = lc.pal;
    // The author line sits 2 px above; the message's own parts get a little more air.
    ui.spacing_mut().item_spacing.y = 4.0;
    // Settings > Chat: media posted as links (6.10), embeds (6.12), reactions (6.13).
    let view = &cx.state.account.settings;
    let (links, embeds, reacts) = (view.chat_shows(10), view.chat_shows(12), view.chat_shows(13));
    let show_content = !msg.content.is_empty() && !(links && content_is_media_link(msg));
    let note = placeholder(msg);
    // MessageLogger: earlier versions above, a deleted message greyed.
    crate::plugins::message_logger::show_versions(ui, cx.theme, msg.id);
    let deleted = crate::plugins::message_logger::is_deleted(msg.id).then_some(pal.muted.gamma_multiply(0.75));
    if cx.view.editing == Some((lc.ch, msg.id)) {
        edit_box(ui, cx, lc, memo, msg);
    } else if show_content || note.is_some() || msg.edited_timestamp.is_some() {
        // A wrapping row lets a one-paragraph message flow inline with "(edited)" after it.
        ui.horizontal_wrapped(|ui| {
            if show_content {
                let opts = MdOpts { guild_id: lc.guild_id, mentions: &msg.mentions, id_salt: msg.id, color: deleted, highlight: &[], bg: Some(row_bg(cx, &pal, msg)) };
                markdown::render(ui, cx, &msg.content, &opts);
            }
            if let Some(note) = note {
                ui.label(RichText::new(note).italics().color(pal.muted));
            }
            if let Some(edited) = &msg.edited_timestamp {
                // Body size: smaller text would sit off the baseline (rows center their labels).
                ui.label(RichText::new(" (edited)").color(pal.muted)).on_hover_text(time_labels(ui.ctx(), edited).header.as_str());
            }
        });
    }
    attachments(ui, cx, lc, msg);
    stickers(ui, cx, lc, msg);
    if msg.flags & SUPPRESS_EMBEDS == 0 {
        let media = |e: &Embed| matches!(e.kind.as_deref(), Some("image" | "gifv")) || super::viewer::plays_inline(e);
        for (i, e) in msg.embeds.iter().enumerate().filter(|(_, e)| if media(e) { links } else { embeds }) {
            embed(ui, cx, lc, msg, e, i);
        }
    }
    // Plugins (MessageLinkEmbeds): previews of linked messages; a click jumps there.
    if let Some((ch, mid)) = crate::plugins::link_embeds::show(ui, cx, msg) {
        jump_to(ui.ctx(), cx, ch, mid);
    }
    if reacts {
        reactions(ui, cx, lc, memo, msg, hovered);
    }
}

fn edit_box(ui: &mut egui::Ui, cx: &mut UiCx<'_>, lc: &Lcx, memo: &mut Memo, msg: &Message) {
    let pal = lc.pal;
    let id = ui.make_persistent_id("rc_inline_edit");
    let focused = ui.memory(|m| m.has_focus(id));
    let out = Frame::new()
        .fill(pal.surface)
        .stroke(Stroke::new(1.0, pal.tk.edge))
        .corner_radius(pal.radius)
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            TextEdit::multiline(&mut cx.view.edit_text)
                .id(id)
                .desired_rows(1)
                .desired_width(f32::INFINITY)
                .frame(Frame::NONE)
                // Only Shift+Enter inserts a newline; plain Enter saves (handled below).
                .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter))
                .show(ui)
                .response
                .response
        });
    if focused {
        // The 2 px focus ring replaces the border (§9), as on egui-framed fields.
        ui.painter().rect_stroke(out.response.rect, pal.radius, Stroke::new(2.0, pal.tk.focus), egui::StrokeKind::Inside);
    }
    let resp = out.inner;
    if memo.edit_focus != Some(msg.id) {
        resp.request_focus();
        memo.edit_focus = Some(msg.id);
    }
    let len = cx.view.edit_text.chars().count();
    // The composer's limit (4000 with full Nitro).
    let max_len = crate::plugins::compose::max_len(cx.state.me.as_ref().map_or(0, |u| u.premium_type));
    let (enter, esc) = ui.input(|i| (i.key_pressed(Key::Enter) && !i.modifiers.shift, i.key_pressed(Key::Escape)));
    let mut close = esc && !overlay_open(ui.ctx(), cx);
    if enter && resp.has_focus() && len <= max_len {
        let new = cx.view.edit_text.trim();
        if !new.is_empty() && new != msg.content {
            cx.cmd.send(Command::EditMessage { channel_id: lc.ch, message_id: msg.id, content: crate::plugins::chat::outgoing(new.to_string()) });
        }
        close = true;
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new("Escape to cancel, Enter to save").size(pal.meta()).color(pal.muted));
        if len > COUNTER_FROM {
            let color = if len > max_len { pal.danger } else { pal.muted };
            ui.label(RichText::new(format!("{len}/{max_len}")).size(pal.meta()).color(color));
        }
    });
    if close {
        cx.view.editing = None;
        cx.view.edit_text.clear();
        memo.edit_focus = None;
        ui.ctx().request_repaint();
    }
}

/// A layer that takes Esc for itself is open (picker, delete dialog, profile card, menus).
pub(super) fn overlay_open(ctx: &egui::Context, cx: &UiCx<'_>) -> bool {
    cx.view.reaction_picker.is_some()
        || cx.view.confirm_delete.is_some()
        || cx.view.profile_card.is_some()
        || egui::Popup::is_any_open(ctx)
        || composer::emoji_picker_open(ctx)
        || cx.view.modal == Some(super::Modal::Media)
}

/// The lightbox (viewer.rs) on `msg`'s media at `item`, with the author as the header shows them.
fn open_viewer(ctx: &egui::Context, cx: &mut UiCx<'_>, lc: &Lcx, msg: &Message, item: &super::viewer::MediaItem) {
    let member = message_member(cx.state, lc.guild_id, msg);
    let author = super::viewer::Author {
        id: msg.author.id,
        name: cx.state.member_display_name(lc.guild_id, &msg.author, member.as_ref()),
        avatar: images::user_avatar_url(lc.guild_id, &msg.author, member.as_ref(), 64, false),
        time: format_timestamp(&msg.timestamp),
    };
    super::viewer::open(ctx, cx, msg, Some(author), item);
}

fn attachments(ui: &mut egui::Ui, cx: &mut UiCx<'_>, lc: &Lcx, msg: &Message) {
    let pal = lc.pal;
    for (i, a) in msg.attachments.iter().enumerate() {
        // Settings > Chat > "When uploaded" off: every upload as a file card.
        if !(a.is_image() || a.is_video()) || !cx.state.account.settings.chat_shows(9) {
            if !crate::plugins::voice_messages::player(ui, msg, a, cx.settings.voice_output.as_deref()) {
                file_chip(ui, &pal, a);
            }
            continue;
        }
        let natural = natural_size(a.width, a.height);
        let max = media_box(ui, MAX_MEDIA);
        let shown = fit_size(natural, max);
        let key = (msg.id, 1000 + i);
        if a.is_spoiler() && !cx.view.revealed_spoilers.contains(&key) && !cx.state.spoilers_open() {
            let (rect, r) = ui.allocate_exact_size(shown, Sense::click());
            // Surface + outline: the sidebar color equals the background in some themes.
            let fill = if r.hovered() { pal.surface.blend(pal.tint()) } else { pal.surface };
            ui.painter().rect(rect, pal.radius, fill, Stroke::new(1.0, pal.tk.rule), egui::StrokeKind::Inside);
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, "Spoiler", FontId::new(pal.meta(), crate::theme::semibold_family()), pal.muted);
            if r.on_hover_cursor(CursorIcon::PointingHand).on_hover_text("Click to reveal").clicked() {
                cx.view.revealed_spoilers.insert(key);
                ui.ctx().request_repaint();
            }
            continue;
        }
        // Videos play in place (viewer.rs); Full screen opens the lightbox.
        if a.is_video() {
            let item = super::viewer::attachment_item(a);
            if super::viewer::video(ui, cx, &item, max) {
                open_viewer(ui.ctx(), cx, lc, msg, &item);
            }
            continue;
        }
        let px = shown * ui.ctx().pixels_per_point();
        let url = cdn::attachment_preview_url(a, px.x.round() as u32, px.y.round() as u32);
        // GIFs move like gifv: always with "Play animated GIFs" on, else while hovered.
        let animate = is_gif(a) && (lc.animate_always || ui.rect_contains_pointer(Rect::from_min_size(ui.cursor().min, shown)));
        let r = images::fit_image(ui, cx.images, &url, max, natural, animate)
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(&a.filename);
        // ImageZoom: a held click shows the lens instead of opening the image.
        if crate::plugins::media::image_zoom(ui, cx.images, &r, &url) {
            continue;
        }
        if r.clicked() {
            open_viewer(ui.ctx(), cx, lc, msg, &super::viewer::attachment_item(a));
        }
    }
}

fn file_chip(ui: &mut egui::Ui, pal: &Pal, a: &Attachment) {
    Frame::new()
        .fill(pal.surface)
        .stroke(Stroke::new(1.0, pal.tk.rule))
        .corner_radius(pal.card_radius())
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width().min(420.0));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let (r, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
                icons::paint(ui.painter(), Icon::File, r, pal.muted, pal.surface);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let name = RichText::new(&a.filename).color(pal.link);
                    if ui.add(Label::new(name).truncate().sense(Sense::click())).on_hover_cursor(CursorIcon::PointingHand).on_hover_text("Open in browser").clicked()
                        && markdown::is_web_scheme(&a.url)
                    {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(&a.url));
                    }
                    ui.add(Label::new(RichText::new(human_size(a.size)).size(pal.meta()).color(pal.muted)).selectable(false));
                });
            });
        });
}

fn stickers(ui: &mut egui::Ui, cx: &mut UiCx<'_>, lc: &Lcx, msg: &Message) {
    for s in &msg.sticker_items {
        match cdn::sticker_url(s.id, s.format_type, 320) {
            Some(url) => {
                let size = vec2(160.0, 160.0);
                images::fit_image(ui, cx.images, &url, size, Some(size), true).on_hover_text(&s.name);
            }
            // Lottie stickers are vector animations; show the name.
            None => {
                ui.label(RichText::new(format!("[Sticker: {}]", s.name)).italics().color(lc.pal.muted));
            }
        }
    }
}

fn embed(ui: &mut egui::Ui, cx: &mut UiCx<'_>, lc: &Lcx, msg: &Message, e: &Embed, idx: usize) {
    let kind = e.kind.as_deref().unwrap_or("rich");
    // gifv and proxied videos play in place, images open the lightbox (viewer.rs).
    let item = matches!(kind, "image" | "gifv" | "video").then(|| super::viewer::embed_item(e)).flatten();
    match (kind, item) {
        (_, Some(item)) if item.play_url.is_some() => {
            let max = media_box(ui, MAX_MEDIA);
            let open = if item.kind == super::viewer::Kind::Gifv { super::viewer::gifv(ui, cx, &item, max) } else { super::viewer::video(ui, cx, &item, max) };
            if open {
                open_viewer(ui.ctx(), cx, lc, msg, &item);
            }
        }
        ("image" | "gifv", item) => {
            // Media-only embeds keep the picture in `thumbnail`; gifv without a proxied video has its still.
            if let Some(m) = e.thumbnail.as_ref().or(e.image.as_ref()) {
                let (natural, max) = (natural_size(m.width, m.height), media_box(ui, MAX_MEDIA));
                let px = fit_size(natural, max) * ui.ctx().pixels_per_point();
                // Only proxied media is shown; third-party hosts are never contacted directly.
                let Some(url) = cdn::embed_media_url(m, px.x.round() as u32, px.y.round() as u32) else { return };
                let r = images::fit_image(ui, cx.images, &url, max, natural, kind == "gifv").on_hover_cursor(CursorIcon::PointingHand);
                if r.clicked()
                    && let Some(item) = item
                {
                    open_viewer(ui.ctx(), cx, lc, msg, &item);
                }
            }
        }
        _ => embed_card(ui, cx, lc, msg, e, kind, idx),
    }
}

fn embed_card(ui: &mut egui::Ui, cx: &mut UiCx<'_>, lc: &Lcx, msg: &Message, e: &Embed, kind: &str, idx: usize) {
    let pal = lc.pal;
    let small_thumb = e.thumbnail.is_some() && !matches!(kind, "article" | "video");
    let bg = ui.painter().add(Shape::Noop);
    let rect = Frame::new()
        .inner_margin(Margin { left: 16, right: 12, top: 12, bottom: 12 })
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width().min(520.0));
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    if small_thumb {
                        ui.set_max_width((ui.available_width() - 92.0).max(120.0));
                    }
                    embed_text(ui, cx, lc, msg, e, idx, small_thumb);
                });
                if small_thumb && let Some(t) = &e.thumbnail {
                    embed_media(ui, cx, lc, msg, e, t, vec2(80.0, 80.0));
                }
            });
            let big = media_box(ui, vec2(400.0, 300.0));
            if let Some(img) = &e.image {
                embed_media(ui, cx, lc, msg, e, img, big);
            }
            if !small_thumb && let Some(t) = &e.thumbnail {
                embed_media(ui, cx, lc, msg, e, t, big);
            }
            embed_footer(ui, cx, &pal, e);
        })
        .response
        .rect;
    // The card in the embed color, covered by the surface except a 4 px left edge: a
    // border that follows the rounded corners (the control radius, docs/DESIGN.md §10.3).
    let bar = e.color.filter(|&c| c != 0).map(rgb).unwrap_or(pal.muted.gamma_multiply(0.4));
    let r = pal.radius;
    let inner = Rect::from_min_max(pos2(rect.left() + 4.0, rect.top()), rect.max);
    let card = CornerRadius { nw: r.saturating_sub(1), sw: r.saturating_sub(1), ne: r, se: r };
    ui.painter().set(bg, Shape::Vec(vec![Shape::rect_filled(rect, r, pal.bg.blend(bar)), Shape::rect_filled(inner, card, pal.surface)]));
}

#[allow(clippy::too_many_arguments)]
fn embed_text(ui: &mut egui::Ui, cx: &mut UiCx<'_>, lc: &Lcx, msg: &Message, e: &Embed, idx: usize, small_thumb: bool) {
    let pal = lc.pal;
    if let Some(name) = e.provider.as_ref().and_then(|p| p.name.as_deref()).filter(|n| !n.is_empty()) {
        ui.label(RichText::new(name).size(pal.meta()).color(pal.muted));
    }
    if let Some(a) = e.author.as_ref().filter(|a| !a.name.is_empty()) {
        ui.horizontal(|ui| {
            if let Some(icon) = &a.proxy_icon_url {
                images::circle_image(ui, cx.images, icon, 20.0, false);
            }
            link_or_label(ui, RichText::new(&a.name).size(pal.meta()).family(bold_family()).color(pal.text), a.url.as_deref());
        });
    }
    if let Some(title) = e.title.as_ref().filter(|t| !t.is_empty()) {
        let color = if e.url.as_deref().is_some_and(markdown::is_web_scheme) { pal.link } else { pal.text };
        link_or_label(ui, RichText::new(title).family(bold_family()).color(color), e.url.as_deref());
    }
    if let Some(d) = e.description.as_ref().filter(|d| !d.is_empty()) {
        let opts = MdOpts { guild_id: lc.guild_id, mentions: &msg.mentions, id_salt: sub_salt(msg.id, idx * 32), color: None, highlight: &[], bg: None };
        markdown::render(ui, cx, d, &opts);
    }
    // Inline fields share a row (up to 3, or 2 next to a thumbnail).
    let per_row = if small_thumb { 2 } else { 3 };
    let fields = &e.fields;
    let mut i = 0;
    while i < fields.len() {
        let n = if fields[i].inline { fields[i..].iter().take(per_row).take_while(|f| f.inline).count() } else { 1 };
        ui.columns(n, |cols| {
            for (k, col) in cols.iter_mut().enumerate() {
                let f = &fields[i + k];
                col.label(RichText::new(&f.name).size(pal.meta()).family(bold_family()).color(pal.text));
                let opts = MdOpts { guild_id: lc.guild_id, mentions: &msg.mentions, id_salt: sub_salt(msg.id, idx * 32 + i + k + 1), color: None, highlight: &[], bg: None };
                markdown::render(col, cx, &f.value, &opts);
            }
        });
        i += n;
    }
}

fn embed_footer(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, e: &Embed) {
    let text = e.footer.as_ref().map(|f| f.text.clone()).filter(|t| !t.is_empty());
    let time = e.timestamp.as_deref().map(|iso| time_labels(ui.ctx(), iso).header.clone()).filter(|t| !t.is_empty());
    let line = [text, time].into_iter().flatten().collect::<Vec<_>>().join(" • ");
    if line.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        if let Some(icon) = e.footer.as_ref().and_then(|f| f.proxy_icon_url.as_deref()) {
            images::circle_image(ui, cx.images, icon, 16.0, false);
        }
        ui.label(RichText::new(line).size(pal.meta()).color(pal.muted));
    });
}

/// An embed card's image or thumbnail `m`: a click opens it in the lightbox; a video embed's
/// (YouTube, Twitch: no proxied file) opens the video in the browser.
fn embed_media(ui: &mut egui::Ui, cx: &mut UiCx<'_>, lc: &Lcx, msg: &Message, e: &Embed, m: &EmbedMedia, max: Vec2) {
    let natural = natural_size(m.width, m.height);
    let px = fit_size(natural, max) * ui.ctx().pixels_per_point();
    // Only proxied media is shown; third-party hosts are never contacted directly.
    let Some(url) = cdn::embed_media_url(m, px.x.round() as u32, px.y.round() as u32) else { return };
    if !images::fit_image(ui, cx.images, &url, max, natural, false).on_hover_cursor(CursorIcon::PointingHand).clicked() {
        return;
    }
    match super::viewer::embed_image_item(e, m).filter(|_| e.kind.as_deref() != Some("video")) {
        Some(item) => open_viewer(ui.ctx(), cx, lc, msg, &item),
        None => {
            let target = e.url.as_deref().unwrap_or(m.url.as_str());
            if markdown::is_web_scheme(target) {
                ui.ctx().open_url(egui::OpenUrl::new_tab(target));
            }
        }
    }
}

fn link_or_label(ui: &mut egui::Ui, text: RichText, url: Option<&str>) {
    match url {
        Some(url) if markdown::is_web_scheme(url) => {
            ui.add(egui::Hyperlink::from_label_and_url(text, url).open_in_new_tab(true));
        }
        _ => {
            ui.label(text);
        }
    }
}

fn reactions(ui: &mut egui::Ui, cx: &mut UiCx<'_>, lc: &Lcx, memo: &mut Memo, msg: &Message, hovered: bool) {
    if msg.reactions.is_empty() {
        return;
    }
    let pal = lc.pal;
    let pass = ui.ctx().cumulative_pass_nr();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for r in &msg.reactions {
            let name = r.emoji.name.clone().unwrap_or_default();
            let animate = r.emoji.animated && (lc.animate_always || hovered);
            let anim_url = animate.then(|| cdn::reaction_emoji_url(&r.emoji, 48)).flatten();
            let tex = r.emoji.id.and_then(|id| emoji_texture(ui, cx.images, id, anim_url));
            let label = if r.emoji.id.is_some() { format!(":{name}:") } else { name.clone() };
            let tip = shortcode_label(&name).filter(|_| r.emoji.id.is_none()).unwrap_or_else(|| label.clone());
            let resp = reaction_chip(ui, &pal, tex, &label, &r.count.to_string(), r.me).on_hover_text(tip);
            // Removing my own reaction, or joining an existing one, never needs ADD_REACTIONS.
            if resp.clicked() && (r.me || lc.can_react || lc.can_add_existing) {
                react(ui.ctx(), cx, lc.ch, msg.id, r.emoji.clone(), r.me);
            }
            crate::plugins::chat::who_reacted(ui, cx, lc.ch, msg.id, &r.emoji, &resp);
        }
        // Only on hover, but always laid out so the row never re-wraps under the pointer.
        if lc.can_react {
            let plus = ui.scope(|ui| {
                if !hovered {
                    ui.set_invisible();
                }
                reaction_chip(ui, &pal, None, "+", "", false).on_hover_text("Add reaction")
            });
            if plus.inner.clicked() {
                open_picker(cx, memo, lc.ch, msg.id, plus.inner.rect, pass);
            }
        }
    });
}

/// Reaction pill (24 tall, `surface`): emoji (texture or text; `NO_GLYPH` when the font lacks
/// it) and the count in meta 500. Mine: the `chip` fill with a 1 px `link` border.
fn reaction_chip(ui: &mut egui::Ui, pal: &Pal, tex: Option<SizedTexture>, emoji: &str, count: &str, me: bool) -> egui::Response {
    let p = ui.painter().clone();
    let count_g = p.layout_no_wrap(count.to_owned(), FontId::new(pal.meta(), crate::theme::medium_family()), if me { pal.link } else { pal.text });
    // An emoji the fonts can't draw as one (a missing glyph, a keycap, a flag) shows as its muted
    // `:shortcode:`, never as an empty box or loose letters.
    let drawable = tex.is_some() || (has_emoji_glyph(ui, emoji) && (emojis::get(emoji).is_none() || pickers::drawable(ui.ctx(), emoji)));
    let (emoji, size) = if drawable {
        (emoji.to_owned(), pal.size)
    } else {
        (shortcode_label(emoji).unwrap_or_else(|| NO_GLYPH.to_owned()), pal.meta())
    };
    let emoji_g = tex.is_none().then(|| p.layout_no_wrap(emoji.clone(), FontId::proportional(size), pal.text));
    let icon_w = emoji_g.as_ref().map_or(18.0, |g| g.size().x);
    let gap = if count.is_empty() { 0.0 } else { 6.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(16.0 + icon_w + gap + count_g.size().x, 24.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hot = resp.hovered();
        let fill = match (me, hot) {
            (true, false) => pal.tk.chip,
            (true, true) => pal.link.gamma_multiply(0.28),
            (false, true) => pal.surface.blend(pal.tk.hover),
            _ => pal.surface,
        };
        // High Contrast keeps an edge on every pill (its `surface` sits close to the chat).
        let hc = pal.tk.mode == crate::theme::Mode::HighContrast;
        let stroke = if me { Stroke::new(1.0, pal.link) } else if hc { Stroke::new(1.0, pal.tk.rule) } else { Stroke::NONE };
        p.rect(rect, pal.radius, fill, stroke, egui::StrokeKind::Inside);
        let x = rect.left() + 8.0;
        match (tex, emoji_g) {
            (Some(t), _) => {
                let r = Rect::from_min_size(pos2(x, rect.center().y - 9.0), vec2(18.0, 18.0));
                Image::from_texture(t).paint_at(ui, r);
            }
            (None, Some(g)) => {
                let color = if (emoji == "+" && !hot) || !drawable { pal.muted } else { pal.text };
                p.galley_with_override_text_color(pos2(x, rect.center().y - g.size().y / 2.0), g, color);
            }
            (None, None) => {}
        }
        p.galley(pos2(x + icon_w + gap, rect.center().y - count_g.size().y / 2.0), count_g, pal.text);
    }
    resp
}

/// Custom emoji texture. The still image is used unless `animated_url` is given (the emoji
/// should play right now), so idle animated emoji never decode all their frames; the still
/// also covers while the animation loads.
fn emoji_texture(ui: &egui::Ui, images: &mut images::ImageCache, id: Id, animated_url: Option<String>) -> Option<SizedTexture> {
    animated_url
        .and_then(|url| images.get(ui.ctx(), &url, 48, true))
        .or_else(|| images.get(ui.ctx(), &cdn::emoji_url(id, false, 48), 48, false))
}

fn open_picker(cx: &mut UiCx<'_>, memo: &mut Memo, ch: Id, message: Id, at: Rect, pass: u64) {
    cx.view.reaction_picker = Some((ch, message));
    memo.picker = Some((at, pass));
}

fn recent_reactions_id() -> egui::Id {
    egui::Id::unique("rc_recent_reactions")
}

/// Adds my reaction (remembered for the quick-reaction buttons) or removes it.
fn react(ctx: &egui::Context, cx: &mut UiCx<'_>, channel_id: Id, message_id: Id, emoji: Emoji, remove: bool) {
    if remove {
        cx.cmd.send(Command::RemoveReaction { channel_id, message_id, emoji });
        return;
    }
    ctx.data_mut(|d| {
        let recent = d.get_temp_mut_or_default::<Vec<Emoji>>(recent_reactions_id());
        recent.retain(|e| !same_emoji(e, &emoji));
        recent.insert(0, emoji.clone());
        recent.truncate(DEFAULT_QUICK.len());
    });
    cx.cmd.send(Command::AddReaction { channel_id, message_id, emoji });
}

/// The 3 hover-toolbar reactions: this session's most recent ones (custom emoji only where
/// usable, i.e. from this guild), topped up with the defaults.
fn quick_reactions(ctx: &egui::Context, usable: impl Fn(Id) -> bool) -> Vec<Emoji> {
    let recent: Vec<Emoji> = ctx.data(|d| d.get_temp(recent_reactions_id())).unwrap_or_default();
    let defaults = DEFAULT_QUICK.map(|e| Emoji { id: None, name: Some(e.to_owned()), animated: false });
    let mut out: Vec<Emoji> = Vec::with_capacity(DEFAULT_QUICK.len());
    for e in recent.into_iter().filter(|e| e.id.is_none_or(&usable)).chain(defaults) {
        if out.len() < DEFAULT_QUICK.len() && !out.iter().any(|o| same_emoji(o, &e)) {
            out.push(e);
        }
    }
    out
}

fn same_emoji(a: &Emoji, b: &Emoji) -> bool {
    a.id == b.id && (a.id.is_some() || a.name == b.name)
}

/// Locally echoed messages still being sent (muted) or failed (danger, Retry / Dismiss). One
/// with files carries its upload card (name, progress, × cancels); Retry re-sends the files.
fn pending_rows(ui: &mut egui::Ui, cx: &mut UiCx<'_>, lc: &Lcx, last_author: Option<Id>) {
    let pending: Vec<PendingMessage> = cx.state.pending.iter().filter(|p| p.channel_id == lc.ch).cloned().collect();
    if pending.is_empty() {
        return;
    }
    let pal = lc.pal;
    let me = cx.state.me.clone();
    let my_member = lc.guild_id.and_then(|g| cx.state.my_members.get(&g).cloned());
    let header = !lc.compact && last_author != lc.me;
    let (mut retry, mut dismiss, mut cancel) = (None, None, None);
    for (i, p) in pending.iter().enumerate() {
        let upload = cx.state.chat.uploads.get(&p.nonce).cloned();
        let header_user = me.as_ref().filter(|_| i == 0 && header);
        if header_user.is_some() {
            ui.add_space(16.0);
        }
        Frame::new().inner_margin(Margin { left: 16, right: 16, top: 2, bottom: 2 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                match header_user {
                    Some(me) => {
                        let url = images::user_avatar_url(lc.guild_id, me, my_member.as_ref(), 80, false);
                        let name = cx.state.member_display_name(lc.guild_id, me, my_member.as_ref());
                        images::avatar(ui, cx.images, &url, AVATAR, false, Some((me.id, &name)));
                        ui.add_space(GUTTER - AVATAR);
                    }
                    None => {
                        ui.allocate_exact_size(vec2(lc.gutter(), 1.0), Sense::hover());
                    }
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
                    if let Some(me) = header_user {
                        author_line(ui, cx, lc, me, my_member.as_ref(), (if p.failed.is_some() { "" } else { "Sending…" }, ""), pal.bg);
                    }
                    // Muted in both states: the "Not sent" chip alone carries the error.
                    let opts = MdOpts { guild_id: lc.guild_id, mentions: &[], id_salt: Id(p.nonce.parse().unwrap_or_default()), color: Some(pal.muted), highlight: &[], bg: None };
                    markdown::render(ui, cx, &p.content, &opts);
                    if let Some(u) = &upload
                        && upload_card(ui, &pal, u, p.failed.is_some())
                    {
                        cancel = Some(p.nonce.clone());
                    }
                    if let Some(err) = &p.failed {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            status_chip(ui, &pal, Some(pal.danger), "Not sent", pal.danger).on_hover_text(err);
                            if small_button(ui, &pal, "Retry", pal.text).clicked() {
                                retry = Some(p.nonce.clone());
                            }
                            if small_button(ui, &pal, "Dismiss", pal.muted).clicked() {
                                dismiss = Some(p.nonce.clone());
                            }
                        });
                    }
                });
            });
        });
    }
    if let Some(nonce) = retry
        && let Some(p) = cx.state.pending.iter_mut().find(|p| p.nonce == nonce)
    {
        p.failed = None;
        // Same nonce: if the first attempt reached the server after all, it deduplicates.
        let (channel_id, content, nonce, reply) = (p.channel_id, p.content.clone(), p.nonce.clone(), p.reply.clone());
        let cmd = match cx.state.chat.uploads.get(&nonce) {
            Some(u) => Command::Chat(ChatCommand::SendWithFiles { channel_id, content, nonce, reply, files: u.files.clone() }),
            None => Command::SendMessage { channel_id, content, nonce, reply },
        };
        cx.cmd.send(cmd);
        ui.ctx().request_repaint();
    }
    if let Some(nonce) = cancel {
        cx.cmd.send(Command::Chat(ChatCommand::CancelUpload { nonce: nonce.clone() }));
        dismiss = Some(nonce);
    }
    if let Some(nonce) = dismiss {
        cx.state.pending.retain(|p| p.nonce != nonce);
        cx.state.chat.uploads.remove(&nonce);
        ui.ctx().request_repaint();
    }
}

/// Discord's upload card under a pending message: file glyph, "name + N more", sent / total and
/// a progress bar, × to cancel while it sends (a failed one has Retry / Dismiss on its row).
/// True when × was clicked.
fn upload_card(ui: &mut egui::Ui, pal: &Pal, u: &Upload, failed: bool) -> bool {
    let mut cancel = false;
    let w = ui.available_width().min(432.0);
    Frame::new().fill(pal.surface).stroke(Stroke::new(1.0, pal.tk.rule)).corner_radius(pal.card_radius()).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.set_width(w - 26.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let (icon, _) = ui.allocate_exact_size(vec2(28.0, 36.0), Sense::hover());
            icons::paint(ui.painter(), Icon::File, Rect::from_center_size(icon.center(), Vec2::splat(28.0)), pal.muted, pal.surface);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                cancel = !failed && icon_button(ui, pal, "×", "Cancel upload", 24.0, false, pal.danger).clicked();
                ui.with_layout(Layout::top_down(Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    let first = u.files.first().map_or("", |f| f.filename.as_str());
                    let name = match u.files.len() {
                        0 | 1 => first.to_owned(),
                        n => format!("{first} + {} more", n - 1),
                    };
                    ui.add(Label::new(RichText::new(name).size(pal.meta()).family(bold_family()).color(pal.text)).selectable(false).truncate());
                    if failed {
                        return;
                    }
                    if u.total > 0 {
                        let sizes = format!("{} / {}", composer::human_size(u.sent), composer::human_size(u.total));
                        ui.add(Label::new(RichText::new(sizes).size(pal.meta() - 1.0).color(pal.muted)).selectable(false));
                    }
                    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), Sense::hover());
                    ui.painter().rect_filled(bar, 2, pal.tint());
                    let frac = if u.total > 0 { u.sent as f32 / u.total as f32 } else { 0.0 };
                    ui.painter().rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * frac.clamp(0.0, 1.0), bar.height())), 2, pal.text);
                });
            });
        });
    });
    cancel
}

/// Quick reactions | Add reaction, Reply, Edit, More ("…", the context menu) at the hovered
/// message's top-right corner.
fn hover_toolbar(ui: &mut egui::Ui, cx: &mut UiCx<'_>, lc: &Lcx, memo: &mut Memo, h: &HoverRow) {
    let pal = lc.pal;
    let pass = ui.ctx().cumulative_pass_nr();
    const SIDE: f32 = 24.0;
    let reacts = lc.can_react && !h.system;
    let quick = if reacts {
        let custom: HashSet<Id> = lc.guild_id.and_then(|g| cx.state.guild(g)).map(|g| g.emojis.iter().map(|e| e.id).collect()).unwrap_or_default();
        quick_reactions(ui.ctx(), |id| custom.contains(&id))
    } else {
        Vec::new()
    };
    // Mostly above the row (web: top -25), so its header line stays readable.
    let top = (h.rect.top() - 25.0).max(ui.clip_rect().top() + 4.0);
    let right = h.rect.right().min(ui.clip_rect().right()) - 16.0;
    let area = Rect::from_min_max(pos2(h.rect.left(), top), pos2(right, top + SIDE + 4.0));
    // A child that is not allocated in the list, so it neither moves the cursor nor the content size.
    let mut bar = ui.new_child(UiBuilder::new().max_rect(area).layout(Layout::right_to_left(Align::Center)));
    let shadow = egui::Shadow { offset: [0, 2], blur: 8, spread: 0, color: Color32::from_black_alpha(if pal.dark { 60 } else { 18 }) };
    // A float (E2 colours; a small shadow, the bar sits on the message it belongs to).
    let rect = Frame::new()
        .fill(pal.tk.raised)
        .stroke(Stroke::new(1.0, pal.tk.rule))
        .corner_radius(pal.card_radius())
        .shadow(shadow)
        .inner_margin(Margin::same(2))
        .show(&mut bar, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            // Right-to-left: the first button ends up rightmost.
            let open = lc.menu_target == Some(h.id);
            let more = vector_button(ui, &pal, Icon::More, "More", SIDE, open, pal.text);
            // Follow the button (window resized, list scrolled) while its menu is open.
            if open && let Some(m) = memo.menu.as_mut().filter(|m| m.from_button) {
                m.anchor = more.rect;
            }
            let demo_menu = demo("menu") && !DEMO_MENU.swap(true, std::sync::atomic::Ordering::Relaxed);
            if more.clicked() || demo_menu {
                memo.menu = (!open).then_some(MsgMenu { id: h.id, anchor: more.rect, from_button: true, new: true });
                ui.ctx().request_repaint();
            }
            if h.own && !h.system && icon_button(ui, &pal, "✏", "Edit", SIDE, false, pal.text).clicked() {
                start_edit(cx, lc.ch, h.id, &h.content);
                ui.ctx().request_repaint();
            }
            if lc.can_send && !h.system && icon_button(ui, &pal, "↩", "Reply", SIDE, false, pal.text).clicked() {
                cx.view.reply_to = Some((lc.ch, h.id, crate::plugins::chat::reply_ping(cx.state, lc.ch, h.id)));
                ui.ctx().request_repaint();
            }
            if reacts {
                let r = icon_button(ui, &pal, "☺", "Add reaction", SIDE, false, pal.text);
                if r.clicked() {
                    open_picker(cx, memo, lc.ch, h.id, r.rect, pass);
                }
            }
            if !quick.is_empty() {
                let (r, _) = ui.allocate_exact_size(vec2(9.0, SIDE), Sense::hover());
                ui.painter().vline(r.center().x, r.shrink2(vec2(0.0, 6.0)).y_range(), Stroke::new(1.0, pal.tk.rule));
            }
            for e in quick.iter().rev() {
                if quick_button(ui, cx, &pal, e, SIDE).clicked() {
                    let mine = msg_reacted(cx.state, lc.ch, h.id, e);
                    react(ui.ctx(), cx, lc.ch, h.id, e.clone(), mine);
                }
            }
        })
        .response
        .rect;
    memo.toolbar = Some((h.id, rect));
}

/// `RUSTCORD_MSG_DEMO=menu` opens the menu once, as if "…" was clicked.
static DEMO_MENU: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Toolbar button with a reaction emoji (glyph or custom emoji texture).
fn quick_button(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, e: &Emoji, side: f32) -> egui::Response {
    let name = e.name.clone().unwrap_or_default();
    let tip = if e.id.is_some() { format!("React with :{name}:") } else { format!("React with {name}") };
    let (rect, resp) = ui.allocate_exact_size(vec2(side, side), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &tip));
    let hot = resp.hovered() || resp.has_focus();
    if hot {
        ui.painter().rect_filled(rect, pal.radius, pal.tint());
    }
    // A notch larger than the other toolbar glyphs: emoji detail smudges at icon size.
    let icon = Rect::from_center_size(rect.center(), Vec2::splat(side * 0.66));
    match e.id {
        Some(id) => {
            let anim = (e.animated && hot).then(|| cdn::reaction_emoji_url(e, 48)).flatten();
            if let Some(tex) = emoji_texture(ui, cx.images, id, anim) {
                Image::from_texture(tex).paint_at(ui, icon);
            }
        }
        None => {
            let color = if hot { pal.text } else { pal.muted };
            let glyph = if has_emoji_glyph(ui, &name) { name.as_str() } else { NO_GLYPH };
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(icon.height()), color);
        }
    }
    resp.on_hover_text(tip).on_hover_cursor(CursorIcon::PointingHand)
}

fn start_edit(cx: &mut UiCx<'_>, ch: Id, mid: Id, content: &str) {
    cx.view.edit_text = content.to_owned();
    cx.view.editing = Some((ch, mid));
}

/// The message context menu (right-click on a row, or the toolbar's "…"). One popup id per
/// message, so moving the menu to another message starts a fresh popup.
fn message_menu(ui: &egui::Ui, cx: &mut UiCx<'_>, lc: &Lcx, memo: &mut Memo) {
    let Some(menu) = memo.menu.as_mut() else { return };
    let MsgMenu { id, anchor, from_button, new: open } = *menu;
    menu.new = false;
    let Some(msg) = shown(cx.state, lc.ch).iter().find(|m| m.id == id) else {
        memo.menu = None;
        return;
    };
    // For the plugins' entries (Translate, Search Image, View Raw, …).
    let plugin_msg = msg.clone();
    let system = !normal_kind(msg.kind);
    let own = lc.me == Some(msg.author.id);
    let content = msg.content.clone();
    let (mid, pinned, flags) = (msg.id, msg.pinned, msg.flags);
    let embeds = !msg.embeds.is_empty() && flags & SUPPRESS_EMBEDS == 0;
    let ctx = ui.ctx().clone();
    let popup_id = egui::Id::unique(("rc_message_menu", mid.0));
    let custom: Vec<CustomEmoji> = lc
        .guild_id
        .and_then(|g| cx.state.guild(g))
        .map(|g| g.emojis.iter().filter(|e| e.available).cloned().collect())
        .unwrap_or_default();
    let pal = lc.pal;
    // Left of "…" (never over the member list), flipping upward near the window bottom.
    let (align, alternatives): (_, &[RectAlign]) = if from_button {
        (RectAlign::LEFT_START, &[RectAlign::LEFT_END])
    } else {
        (RectAlign::BOTTOM_START, &[RectAlign::TOP_START, RectAlign::BOTTOM_END, RectAlign::TOP_END])
    };
    let (mut marked, mut view_more) = (false, false);
    egui::Popup::new(popup_id, ctx.clone(), anchor, ui.layer_id())
        .kind(egui::PopupKind::Menu)
        .layout(Layout::top_down_justified(Align::Min))
        .style(egui::containers::menu::menu_style)
        .open_memory(open.then_some(egui::SetOpenCommand::Bool(true)))
        .align(align)
        .align_alternatives(alternatives)
        .gap(4.0)
        .show(|ui| {
            menu_setup(ui);
            if lc.can_react && !system {
                // The hover toolbar's quick reactions as a row on top.
                let usable: HashSet<Id> = custom.iter().map(|e| e.id).collect();
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for e in quick_reactions(&ctx, |id| usable.contains(&id)) {
                        if quick_button(ui, cx, &pal, &e, 32.0).clicked() {
                            let mine = msg_reacted(cx.state, lc.ch, mid, &e);
                            react(&ctx, cx, lc.ch, mid, e, mine);
                            ui.close();
                        }
                    }
                });
                ui.separator();
                // Discord: a few frequently used ones, then "View more" (the reaction picker).
                let picked = ui
                    .menu_button("Add reaction", |ui| {
                        menu_setup(ui);
                        let keys = cx.settings.frequent_emoji.iter().map(|(k, _)| k.as_str()).chain(DEFAULT_QUICK);
                        let mut pick = None;
                        for (e, name) in pickers::frequent_reactions(cx.state, keys, lc.ch, 5) {
                            if emoji_menu_item(ui, cx, &pal, &e, &name) {
                                pick = Some(e);
                            }
                        }
                        ui.separator();
                        view_more = menu_item(ui, "View more", MenuIcon::Glyph("☺"), pal.text);
                        pick
                    })
                    .inner
                    .flatten();
                if let Some(e) = picked.filter(|e| !msg_reacted(cx.state, lc.ch, mid, e)) {
                    react(&ctx, cx, lc.ch, mid, e, false);
                }
            }
            if own && !system && menu_item(ui, "Edit message", MenuIcon::Glyph("✏"), pal.text) {
                start_edit(cx, lc.ch, mid, &content);
            }
            if lc.can_send && !system && menu_item(ui, "Reply", MenuIcon::Glyph("↩"), pal.text) {
                cx.view.reply_to = Some((lc.ch, mid, crate::plugins::chat::reply_ping(cx.state, lc.ch, mid)));
            }
            if !system
                && super::threads::has_threads(lc.kind)
                && cx.state.can_create_thread(lc.ch, false)
                && menu_item(ui, "Create thread", MenuIcon::Icon(Icon::Thread), pal.text)
            {
                super::threads::open_create(cx, lc.ch, Some(mid));
            }
            if !content.is_empty() && menu_item(ui, "Copy text", MenuIcon::Icon(Icon::Copy), pal.text) {
                ctx.copy_text(content.clone());
            }
            ui.separator();
            if lc.can_pin && !system && menu_item(ui, if pinned { "Unpin message" } else { "Pin message" }, MenuIcon::Glyph("📌"), pal.text) {
                let (channel_id, message_id) = (lc.ch, mid);
                let cmd = if pinned { ChatCommand::UnpinMessage { channel_id, message_id } } else { ChatCommand::PinMessage { channel_id, message_id } };
                cx.cmd.send(Command::Chat(cmd));
            }
            if menu_item(ui, "Mark unread", MenuIcon::Glyph("✉"), pal.text) {
                mark_unread(cx, lc.ch, mid);
                marked = true;
            }
            if menu_item(ui, "Copy message link", MenuIcon::Icon(Icon::Link), pal.text) {
                ctx.copy_text(message_link(lc.guild_id, lc.ch, mid));
            }
            if own && embeds && menu_item(ui, "Remove embeds", MenuIcon::Glyph("🚫"), pal.text) {
                cx.cmd.send(Command::Chat(ChatCommand::SuppressEmbeds { channel_id: lc.ch, message_id: mid, flags }));
            }
            crate::plugins::menus::message_items(ui, cx, &plugin_msg, &mut |ui, text, glyph| menu_item(ui, text, MenuIcon::Glyph(glyph), pal.text));
            if own {
                ui.separator();
                if menu_item(ui, "Delete message", MenuIcon::Glyph("🗑"), pal.danger) {
                    cx.view.confirm_delete = Some((lc.ch, mid));
                }
            }
            // No developer-mode setting here yet: the ID stays, last like Discord's.
            // Settings > Advanced > Developer Mode.
            let dev = cx.state.account.settings.developer_mode();
            if dev {
                ui.separator();
            }
            if dev && menu_item(ui, "Copy message ID", MenuIcon::Glyph("🆔"), pal.text) {
                ctx.copy_text(mid.0.to_string());
            }
        });
    if marked {
        memo.bar_closed = false;
    }
    if view_more {
        open_picker(cx, memo, lc.ch, mid, anchor, ctx.cumulative_pass_nr());
    }
    if !egui::Popup::is_id_open(&ctx, popup_id) {
        memo.menu = None;
        ctx.request_repaint();
    }
}

fn msg_reacted(state: &AppState, ch: Id, mid: Id, e: &Emoji) -> bool {
    shown(state, ch).iter().find(|m| m.id == mid).is_some_and(|m| m.reactions.iter().any(|r| r.me && same_emoji(&r.emoji, e)))
}

/// "Mark unread": my read position moves to just before `mid` (the NEW divider and the
/// new-messages bar show there) and automatic acks pause until I leave or send.
fn mark_unread(cx: &mut UiCx<'_>, ch: Id, mid: Id) {
    let Some((target, mention_count)) = cx.state.mark_unread(ch, mid) else { return };
    // A queued read ack would undo it.
    cx.view.pending_acks.remove(&ch);
    cx.state.unread_marker = Some((ch, target));
    if cx.settings.send_read_acks {
        cx.cmd.send(Command::Chat(ChatCommand::MarkUnread { channel_id: ch, message_id: target, mention_count }));
    }
}

fn message_link(guild_id: Option<Id>, ch: Id, mid: Id) -> String {
    let place = guild_id.map_or_else(|| "@me".to_owned(), |g| g.0.to_string());
    format!("https://discord.com/channels/{place}/{}/{}", ch.0, mid.0)
}

/// The reaction picker: the expression picker's Emoji tab in reaction mode
/// (`pickers::reaction_contents`), below the button that opened it, flipped to stay on screen.
/// Shift-click reacts and keeps it open.
fn reaction_picker(ctx: &egui::Context, cx: &mut UiCx<'_>, lc: &Lcx, memo: &mut Memo) {
    let Some((ch, mid)) = cx.view.reaction_picker else {
        pickers::reaction_closed(ctx, cx);
        return;
    };
    // The other list's (chat / thread panel); the chat's list drops one left on neither.
    if ch != lc.ch && (lc.panel || on_screen(cx, ch)) {
        return;
    }
    if ch != lc.ch || !lc.can_react {
        cx.view.reaction_picker = None;
        return;
    }
    let screen = ctx.content_rect();
    let (at, opened) = memo.picker.unwrap_or((Rect::from_center_size(screen.center(), Vec2::ZERO), 0));
    let (pivot, pos) = if at.center().x > screen.center().x {
        (Align2::RIGHT_TOP, at.right_bottom())
    } else {
        (Align2::LEFT_TOP, at.left_bottom())
    };
    let mine: Vec<Emoji> = shown(cx.state, ch)
        .iter()
        .find(|m| m.id == mid)
        .map(|m| m.reactions.iter().filter(|r| r.me).map(|r| r.emoji.clone()).collect())
        .unwrap_or_default();
    let pal = lc.pal;
    let out = egui::Area::new(egui::Id::unique("rc_reaction_picker"))
        .order(Order::Foreground)
        .fixed_pos(pos + vec2(0.0, 4.0))
        .pivot(pivot)
        .constrain(true)
        .show(ctx, |ui| {
            Frame::popup(ui.style())
                .fill(pal.surface)
                .corner_radius(pal.card_radius())
                .inner_margin(0)
                .show(ui, |ui| pickers::reaction_contents(ui, cx, &pal, ch))
                .inner
        });
    // The click that opened the picker is not a click outside it.
    let mut close = ctx.input(|i| i.key_pressed(Key::Escape)) || (ctx.cumulative_pass_nr() != opened && out.response.clicked_elsewhere());
    if let Some(emoji) = out.inner {
        if !mine.iter().any(|m| same_emoji(m, &emoji)) {
            react(ctx, cx, ch, mid, emoji, false);
        }
        close |= !ctx.input(|i| i.modifiers.shift);
    }
    if close {
        cx.view.reaction_picker = None;
        memo.picker = None;
    }
}

/// Add Reaction submenu row: `:name:` with the emoji on the right, where `menu_item` puts icons.
fn emoji_menu_item(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, e: &Emoji, name: &str) -> bool {
    let glyph = match (e.id, e.name.as_deref()) {
        (None, Some(g)) if has_emoji_glyph(ui, g) => g,
        (None, _) => NO_GLYPH,
        // An em space keeps room for the image painted below.
        (Some(_), _) => "\u{2003}",
    };
    let resp = ui.add(Button::new(RichText::new(format!(":{name}:")).color(pal.text)).right_text(RichText::new(glyph).size(18.0).color(pal.text)));
    if let Some(id) = e.id {
        let icon = Rect::from_center_size(pos2(resp.rect.right() - ui.spacing().button_padding.x - 9.0, resp.rect.center().y), Vec2::splat(18.0));
        if let Some(tex) = emoji_texture(ui, cx.images, id, None) {
            Image::from_texture(tex).paint_at(ui, icon);
        }
    }
    resp.clicked()
}

/// Whether the chat or the thread panel shows `ch`.
fn on_screen(cx: &UiCx<'_>, ch: Id) -> bool {
    cx.state.selected_channel == Some(ch) || cx.view.thread_panel == Some(ch)
}

fn delete_modal(ctx: &egui::Context, cx: &mut UiCx<'_>, lc: &Lcx) {
    let pal = &lc.pal;
    let Some((ch, mid)) = cx.view.confirm_delete else { return };
    // Asked by the other list (chat / thread panel); the chat's list drops one left on neither.
    if ch != lc.ch && (lc.panel || on_screen(cx, ch)) {
        return;
    }
    let preview = shown(cx.state, ch)
        .iter()
        .find(|m| m.id == mid)
        .map(|m| {
            let guild_id = cx.state.channels.get(&ch).and_then(|c| c.guild_id);
            let text = resolve_tokens(&truncate_chars(&m.content, 300), |tok| token_name(cx.state, guild_id, &m.mentions, tok));
            (m.author.display_name().to_string(), text)
        });
    let Some((author, text)) = preview else {
        cx.view.confirm_delete = None;
        return;
    };
    let frame = Frame::popup(&ctx.global_style()).fill(pal.surface).corner_radius(pal.card_radius()).inner_margin(Margin::same(24));
    let id = egui::Id::unique("rc_confirm_delete");
    let resp = super::motion::modal(ctx, egui::Modal::default_area(id), frame, pal.tk.scrim, |ui| {
        ui.set_width(380.0);
        ui.label(RichText::new("Delete message").family(bold_family()).size(pal.size + 3.0).color(pal.text));
        ui.label(RichText::new("This can't be undone.").color(pal.muted));
        Frame::new().fill(pal.bg).corner_radius(pal.radius).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(author).family(bold_family()).color(pal.text));
            if !text.is_empty() {
                ui.label(RichText::new(text).color(pal.text));
            }
        });
        ui.add_space(6.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add(Button::new(RichText::new("Delete").color(Color32::WHITE)).fill(pal.danger)).clicked() {
                return Some(true);
            }
            if ui.button("Cancel").clicked() {
                return Some(false);
            }
            None
        })
        .inner
    });
    let close = match resp.inner {
        Some(true) => {
            cx.cmd.send(Command::DeleteMessage { channel_id: ch, message_id: mid });
            true
        }
        Some(false) => true,
        None => resp.should_close(),
    };
    if close {
        cx.view.confirm_delete = None;
    }
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// Message types rendered as regular chat messages.
fn normal_kind(kind: u8) -> bool {
    matches!(kind, 0 | 19 | 20 | 23 | 24 | 26 | 46 | 49)
}

fn is_reply(m: &Message) -> bool {
    m.message_reference.as_ref().is_some_and(|r| r.kind == 0) && (m.kind == 19 || m.referenced_message.is_some())
}

/// First message of a visual group: other author, > 7 min gap, reply, system message or `new_day`.
fn is_group_start(prev: Option<&Message>, m: &Message, new_day: bool) -> bool {
    let Some(p) = prev else { return true };
    p.author.id != m.author.id
        || !normal_kind(p.kind)
        || !normal_kind(m.kind)
        || is_reply(m)
        || m.id.timestamp_ms().saturating_sub(p.id.timestamp_ms()) >= GROUP_GAP_MS
        || new_day
}

/// Hash of everything that changes a row's height.
fn fingerprint(m: &Message, layout: impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    layout.hash(&mut h);
    (&m.content, &m.edited_timestamp, m.kind, m.flags).hash(&mut h);
    (m.attachments.len(), m.embeds.len(), m.sticker_items.len(), m.referenced_message.is_some()).hash(&mut h);
    for r in &m.reactions {
        (&r.emoji, r.count, r.me).hash(&mut h);
    }
    h.finish()
}

struct Sys<'a> {
    author: &'a str,
    target: Option<&'a str>,
    target_is_author: bool,
    guild: &'a str,
    in_thread: bool,
    join_index: usize,
}

/// Stands for the author in a system line until `system_row` sets the name in bold.
const AUTHOR_MARK: &str = "\u{E000}";

/// (system line with `AUTHOR_MARK` for the author, author name); None for normal messages.
fn system_line(state: &AppState, lc: &Lcx, msg: &Message) -> Option<(String, String)> {
    if normal_kind(msg.kind) {
        return None;
    }
    let author = state.member_display_name(lc.guild_id, &msg.author, msg.member.as_ref());
    let target = msg.mentions.first();
    let target_name = target.map(|u| state.member_display_name(lc.guild_id, u, None));
    let sys = Sys {
        author: AUTHOR_MARK,
        target: target_name.as_deref(),
        target_is_author: target.is_some_and(|u| u.id == msg.author.id),
        guild: &lc.guild_name,
        in_thread: lc.in_thread(),
        join_index: (msg.id.timestamp_ms() % 13) as usize,
    };
    let text = system_text(msg.kind, &msg.content, &sys).or_else(|| msg.content.is_empty().then(|| format!("Unsupported message type {}", msg.kind)))?;
    Some((text, author))
}

/// One-line text for system message types; None for types rendered as normal messages.
fn system_text(kind: u8, content: &str, s: &Sys<'_>) -> Option<String> {
    let a = s.author;
    let t = s.target.unwrap_or("someone");
    let place = if s.in_thread { "thread" } else { "group" };
    Some(match kind {
        1 => format!("{a} added {t} to the {place}."),
        2 if s.target_is_author => format!("{a} left the {place}."),
        2 => format!("{a} removed {t} from the {place}."),
        3 => format!("{a} started a call."),
        4 if content.is_empty() => format!("{a} removed the custom group name."),
        4 => format!("{a} changed the channel name: {content}"),
        5 => format!("{a} changed the channel icon."),
        6 => format!("{a} pinned a message to this channel."),
        7 => JOIN_TEMPLATES[s.join_index % JOIN_TEMPLATES.len()].replace("{}", a),
        8..=11 => {
            let times = if content.is_empty() { String::new() } else { format!(" {content} times") };
            let level = if kind > 8 { format!(" {} has achieved Level {}!", s.guild, kind - 8) } else { String::new() };
            format!("{a} just boosted the server{times}!{level}")
        }
        12 => format!("{a} has added {content} to this channel. Its most important updates will show up here."),
        14..=17 => "This server's Server Discovery status changed.".to_string(),
        18 => format!("{a} started a thread: {content}. See all threads."),
        21 => "Sorry, we couldn't load the first message in this thread.".to_string(),
        22 => "Wondering who to invite? Start by inviting anyone who can help you build the server!".to_string(),
        25 => format!("{a} joined a server subscription."),
        27 => format!("{a} started {content}"),
        28 => format!("{a} ended {content}"),
        29 => format!("{a} is now a speaker."),
        30 => format!("{a} requested to speak."),
        31 => format!("{a} changed the Stage topic: {content}"),
        32 => format!("{a} upgraded an application to premium for this server!"),
        36 => format!("{a} enabled security actions until {content}."),
        37 => format!("{a} disabled security actions."),
        38 => format!("{a} reported a raid in {}.", s.guild),
        39 => format!("{a} reported a false alarm in {}.", s.guild),
        44 => format!("{a} made a purchase!"),
        65 => format!("{a} started a voice hangout."),
        67 => format!("{a} accepted your friend request."),
        _ => return None,
    })
}

/// Icon of a system message (None: a dot).
fn system_icon(kind: u8) -> Option<Icon> {
    Some(match kind {
        1 => Icon::Plus,
        2 => Icon::ArrowLeft,
        3 => Icon::Phone,
        6 => Icon::Pin,
        7 => Icon::ArrowRight,
        8..=11 => Icon::Boost,
        _ => return None,
    })
}

fn reply_preview(m: &Message) -> String {
    // Masked over the whole text first: a spoiler may span several lines.
    match mask_spoilers(&m.content).lines().map(str::trim).find(|l| !l.is_empty()) {
        Some(line) => truncate_chars(line, 120),
        None if !m.attachments.is_empty() => "Click to see attachment".into(),
        None if !m.embeds.is_empty() => "Click to see embed".into(),
        None if !m.sticker_items.is_empty() => "Sticker".into(),
        None => String::new(),
    }
}

/// Replaces every `||...||` pair with a placeholder; an unpaired `||` stays as typed.
fn mask_spoilers(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("||") {
        let Some(len) = rest[start + 2..].find("||") else { break };
        out.push_str(&rest[..start]);
        out.push_str("[spoiler]");
        rest = &rest[start + 2 + len + 2..];
    }
    out.push_str(rest);
    out
}

/// Replaces `<...>` tokens for which `name_of` returns a readable name; others stay as typed.
fn resolve_tokens(text: &str, mut name_of: impl FnMut(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        match tail.find('>').and_then(|end| name_of(&tail[1..end]).map(|name| (end, name))) {
            Some((end, name)) => {
                out.push_str(&name);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('<');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Readable form of a mention / channel / custom emoji token (without the angle brackets).
fn token_name(state: &AppState, guild_id: Option<Id>, mentions: &[User], tok: &str) -> Option<String> {
    let id = |s: &str| s.parse::<u64>().ok().map(Id);
    if let Some(role) = tok.strip_prefix("@&") {
        let role = id(role)?;
        let name = guild_id.and_then(|g| state.guild(g)).and_then(|g| g.roles.iter().find(|r| r.id == role)).map(|r| r.name.clone());
        return Some(format!("@{}", name.unwrap_or_else(|| "role".into())));
    }
    if let Some(user) = tok.strip_prefix("@!").or_else(|| tok.strip_prefix('@')) {
        let user = id(user)?;
        let name = mentions
            .iter()
            .find(|u| u.id == user)
            .or_else(|| state.users.get(&user))
            .map(|u| state.member_display_name(guild_id, u, None));
        return Some(format!("@{}", name.unwrap_or_else(|| "unknown-user".into())));
    }
    if let Some(channel) = tok.strip_prefix('#') {
        let channel = id(channel)?;
        return Some(if state.channels.contains_key(&channel) { format!("#{}", state.channel_name(channel)) } else { "#unknown".into() });
    }
    let emoji = tok.strip_prefix("a:").or_else(|| tok.strip_prefix(':'))?;
    let (name, emoji_id) = emoji.rsplit_once(':')?;
    id(emoji_id)?;
    Some(format!(":{name}:"))
}

fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// The official client hides the content when it is just the URL of its only media embed.
fn content_is_media_link(m: &Message) -> bool {
    m.flags & SUPPRESS_EMBEDS == 0
        && m.embeds.len() == 1
        && matches!(m.embeds[0].kind.as_deref(), Some("image" | "gifv"))
        && m.embeds[0].url.as_deref() == Some(m.content.trim())
}

fn placeholder(m: &Message) -> Option<&'static str> {
    if !m.content.is_empty() || !m.attachments.is_empty() || !m.embeds.is_empty() || !m.sticker_items.is_empty() {
        return None;
    }
    if m.message_reference.as_ref().is_some_and(|r| r.kind == 1) {
        Some("Forwarded message (not shown by this client)")
    } else if m.flags & IS_COMPONENTS_V2 != 0 {
        Some("This message uses components this client can't show.")
    } else {
        None
    }
}

/// Distinct markdown id salt for embed parts of a message.
fn sub_salt(id: Id, n: usize) -> Id {
    Id(id.0 ^ ((n as u64 + 1) << 52))
}

fn human_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64 / 1024.0;
    for unit in ["KB", "MB", "GB"] {
        if v < 1024.0 {
            return format!("{v:.1} {unit}");
        }
        v /= 1024.0;
    }
    format!("{v:.1} TB")
}

fn natural_size(w: Option<u32>, h: Option<u32>) -> Option<Vec2> {
    match (w, h) {
        (Some(w), Some(h)) if w > 0 && h > 0 => Some(vec2(w as f32, h as f32)),
        _ => None,
    }
}

/// `max` narrowed to the space left in the row, so media never overflows a narrow window.
fn media_box(ui: &egui::Ui, max: Vec2) -> Vec2 {
    vec2(max.x.min(ui.available_width()).max(1.0), max.y)
}

/// Display size like `images::fit_image`: natural size scaled down to fit, else `max`.
fn fit_size(natural: Option<Vec2>, max: Vec2) -> Vec2 {
    match natural {
        Some(n) => n * (max.x / n.x).min(max.y / n.y).min(1.0),
        None => max,
    }
}

fn is_gif(a: &Attachment) -> bool {
    a.content_type.as_deref() == Some("image/gif") || a.filename.to_ascii_lowercase().ends_with(".gif")
}

fn local_time(unix_ms: u64) -> Option<chrono::DateTime<chrono::Local>> {
    chrono::DateTime::from_timestamp_millis(unix_ms as i64).map(|t| t.with_timezone(&chrono::Local))
}

/// Local-time results by input. chrono's `Local` asks the OS for the zone's rules on every
/// conversion (Windows: GetTimeZoneInformationForYear, ~1 µs) and each row needed several per
/// pass. All of it is dropped when today's date or the zone's offsets change (checked once a pass).
#[derive(Default)]
struct Times {
    pass: Option<u64>,
    zone: Option<Zone>,
    local: HashMap<i64, Option<chrono::DateTime<chrono::FixedOffset>>>,
    labels: HashMap<String, std::rc::Rc<TimeLabels>>,
}

/// Today and the UTC offsets now, on 1 January and on 1 July (seconds).
// shortcut: a switch between two zones with the same offsets but other DST dates keeps the old
// times until midnight; reading the zone's rules needs a platform API per OS.
type Zone = (chrono::NaiveDate, i32, i32, i32);

/// A message time as rows show it: `format_timestamp`, `full_time` (tooltip), `short_time`.
struct TimeLabels {
    header: String,
    full: String,
    short: String,
}

thread_local! {
    static TIMES: std::cell::RefCell<Times> = std::cell::RefCell::default();
}

impl Times {
    fn check(&mut self, pass: u64, zone: impl FnOnce() -> Zone) {
        if self.pass == Some(pass) {
            return;
        }
        self.pass = Some(pass);
        let zone = Some(zone());
        if self.zone != zone || self.local.len() + self.labels.len() > 4000 {
            *self = Times { pass: Some(pass), zone, ..Times::default() };
        }
    }

    fn local(&mut self, unix_ms: i64) -> Option<chrono::DateTime<chrono::FixedOffset>> {
        *self.local.entry(unix_ms).or_insert_with(|| chrono::DateTime::from_timestamp_millis(unix_ms).map(|t| t.with_timezone(&chrono::Local).fixed_offset()))
    }

    /// The local day a message was sent on.
    fn day(&mut self, id: Id) -> Option<chrono::NaiveDate> {
        self.local(id.timestamp_ms() as i64).map(|t| t.date_naive())
    }

    fn labels(&mut self, iso: &str) -> std::rc::Rc<TimeLabels> {
        if let Some(l) = self.labels.get(iso) {
            return l.clone();
        }
        let l = std::rc::Rc::new(TimeLabels { header: format_timestamp(iso), full: full_time(iso), short: short_time(iso) });
        self.labels.insert(iso.to_owned(), l.clone());
        l
    }
}

fn zone_now() -> Zone {
    use chrono::{Datelike, TimeZone};
    let now = chrono::Local::now();
    let at = |month| {
        let noon = chrono::NaiveDate::from_ymd_opt(now.year(), month, 1).and_then(|d| d.and_hms_opt(12, 0, 0));
        noon.map_or(0, |t| chrono::Local.offset_from_utc_datetime(&t).local_minus_utc())
    };
    (now.date_naive(), now.offset().local_minus_utc(), at(1), at(7))
}

fn with_times<R>(ctx: &egui::Context, f: impl FnOnce(&mut Times) -> R) -> R {
    TIMES.with_borrow_mut(|t| {
        t.check(ctx.cumulative_pass_nr(), zone_now);
        f(t)
    })
}

/// `unix_ms` in the local zone (markdown timestamps, ReplyTimestamp).
pub(crate) fn local_at(ctx: &egui::Context, unix_ms: i64) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    with_times(ctx, |t| t.local(unix_ms))
}

/// Today's local date, as of this pass.
pub(crate) fn today(ctx: &egui::Context) -> Option<chrono::NaiveDate> {
    with_times(ctx, |t| t.zone.map(|z| z.0))
}

fn time_labels(ctx: &egui::Context, iso: &str) -> std::rc::Rc<TimeLabels> {
    with_times(ctx, |t| t.labels(iso))
}

fn day_label(ctx: &egui::Context, id: Id) -> String {
    local_at(ctx, id.timestamp_ms() as i64).map(|t| t.format("%-d %B %Y").to_string()).unwrap_or_default()
}

fn short_time(iso: &str) -> String {
    local_format(iso, "%H:%M")
}

/// Tooltip date of a message time ("Tuesday, 10 November 2026 14:12").
fn full_time(iso: &str) -> String {
    local_format(iso, "%A, %-d %B %Y %H:%M")
}

fn local_format(iso: &str, fmt: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso).map(|t| t.with_timezone(&chrono::Local).format(fmt).to_string()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MessageReference;

    /// 2026-01-15 12:00:00 UTC.
    const NOON: u64 = 1_768_478_400_000;

    fn msg(unix_ms: u64, author: u64) -> Message {
        Message {
            id: Id((unix_ms - Id::DISCORD_EPOCH_MS) << 22),
            author: User { id: Id(author), ..Default::default() },
            ..Default::default()
        }
    }

    #[test]
    fn groups_consecutive_messages() {
        let a = msg(NOON, 1);
        let b = msg(NOON + 60_000, 1);
        assert!(is_group_start(None, &a, false));
        assert!(!is_group_start(Some(&a), &b, false));
        // A new day.
        assert!(is_group_start(Some(&a), &b, true));
        // Other author.
        assert!(is_group_start(Some(&a), &msg(NOON + 60_000, 2), false));
        // 7 minutes apart.
        assert!(is_group_start(Some(&a), &msg(NOON + GROUP_GAP_MS, 1), false));
        // Reply.
        let mut reply = b.clone();
        reply.kind = 19;
        reply.message_reference = Some(MessageReference { message_id: Some(a.id), ..Default::default() });
        assert!(is_group_start(Some(&a), &reply, false));
        // System messages stand alone.
        let mut join = b.clone();
        join.kind = 7;
        assert!(is_group_start(Some(&a), &join, false));
        assert!(is_group_start(Some(&join), &msg(NOON + 120_000, 1), false));
    }

    #[test]
    fn local_times_match_chrono_and_follow_the_zone() {
        let ctx = egui::Context::default();
        let iso = "2026-01-15T23:59:30.123000+00:00";
        let t = time_labels(&ctx, iso);
        assert_eq!((t.header.as_str(), t.full.as_str(), t.short.as_str()), (format_timestamp(iso).as_str(), full_time(iso).as_str(), short_time(iso).as_str()));
        assert_eq!(time_labels(&ctx, "not a time").header, "");
        let id = msg(NOON + 43_170_000, 1).id;
        assert_eq!(with_times(&ctx, |t| t.day(id)), local_time(id.timestamp_ms()).map(|t| t.date_naive()));
        assert_eq!(day_label(&ctx, id), local_time(id.timestamp_ms()).unwrap().format("%-d %B %Y").to_string());

        // Checked once per pass; another day or zone drops everything.
        let day = chrono::NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let mut times = Times::default();
        times.check(1, || (day, 3600, 3600, 7200));
        times.local(0);
        times.labels(iso);
        times.check(1, || unreachable!("same pass"));
        times.check(2, || (day, 3600, 3600, 7200));
        assert_eq!((times.local.len(), times.labels.len()), (1, 1), "same zone and day: kept");
        times.check(3, || (day, 3600, 3600, 3600));
        assert!(times.local.is_empty() && times.labels.is_empty(), "other DST rules");
        times.local(0);
        times.check(4, || (day.succ_opt().unwrap(), 3600, 3600, 3600));
        assert!(times.local.is_empty(), "past midnight: Today / Yesterday move");
    }

    #[test]
    fn rows_off_screen_keep_recent_heights() {
        let view = Rect::from_min_max(pos2(0.0, 1000.0), pos2(800.0, 1500.0));
        let r = RowHeight { fp: 1, h: 100.0, key: 7, at: 10.0 };
        let keeps = |top: f32, key: u64, now: f64, pinned: bool| keeps_height(&r, top, view, key, now, pinned);
        // Just above and below the viewport: kept while fresh, at the same layout, unpinned.
        for top in [880.0, 1520.0] {
            assert!(keeps(top, 7, 10.1, false));
            assert!(!keeps(top, 7, 10.0 + REMEASURE_S, false), "stale");
            assert!(!keeps(top, 8, 10.1, false), "width, scale or fonts changed");
            assert!(!keeps(top, 7, 10.1, true), "focus, menu or toolbar on it");
        }
        // On screen, or within the slack around it: always laid out.
        for top in [950.0, 1200.0, 1490.0, 890.0, 1510.0] {
            assert!(!keeps(top, 7, 10.1, false), "{top}");
        }
        // Far off: kept whatever its age or layout.
        assert!(keeps(1000.0 - LAYOUT_MARGIN - 101.0, 8, 99.0, true));
        assert!(keeps(1501.0 + LAYOUT_MARGIN, 8, 99.0, true));
    }

    #[test]
    fn fingerprint_tracks_layout_changes() {
        let a = msg(NOON, 1);
        let base = fingerprint(&a, (800u32, false));
        assert_eq!(base, fingerprint(&a.clone(), (800u32, false)));
        assert_ne!(base, fingerprint(&a, (801u32, false)));
        let mut edited = a.clone();
        edited.content = "changed".into();
        assert_ne!(base, fingerprint(&edited, (800u32, false)));
    }

    #[test]
    fn system_texts() {
        let s = Sys { author: "ann", target: Some("bob"), target_is_author: false, guild: "Rust", in_thread: false, join_index: 3 };
        assert_eq!(system_text(0, "hi", &s), None);
        assert_eq!(system_text(19, "hi", &s), None);
        assert_eq!(system_text(7, "", &s).unwrap(), "A wild ann appeared.");
        assert_eq!(system_text(1, "", &s).unwrap(), "ann added bob to the group.");
        assert_eq!(system_text(2, "", &Sys { target_is_author: true, ..s }).unwrap(), "ann left the group.");
        assert_eq!(system_text(8, "3", &s).unwrap(), "ann just boosted the server 3 times!");
        assert_eq!(system_text(10, "", &s).unwrap(), "ann just boosted the server! Rust has achieved Level 2!");
        assert_eq!(system_text(250, "", &s), None);
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(fit_size(Some(vec2(840.0, 320.0)), MAX_MEDIA), vec2(420.0, 160.0));
        assert_eq!(fit_size(Some(vec2(100.0, 50.0)), MAX_MEDIA), vec2(100.0, 50.0));
        assert_eq!(fit_size(None, MAX_MEDIA), MAX_MEDIA);
        assert_eq!(natural_size(Some(0), Some(10)), None);
    }

    #[test]
    fn previews() {
        let mut m = msg(NOON, 1);
        m.content = "\n  first line  \nsecond".into();
        assert_eq!(reply_preview(&m), "first line");
        assert_eq!(truncate_chars("héllo", 2), "hé…");
        assert_eq!(truncate_chars("hi", 5), "hi");
        m.content = "||ending:\nX dies|| ok".into();
        assert_eq!(reply_preview(&m), "[spoiler] ok");
        assert_eq!(mask_spoilers("a ||b|| c ||d|| e || f"), "a [spoiler] c [spoiler] e || f");
        assert_eq!(mask_spoilers("||||"), "[spoiler]");
        m.content.clear();
        m.attachments.push(Attachment::default());
        assert_eq!(reply_preview(&m), "Click to see attachment");
    }

    #[test]
    fn preview_tokens() {
        let mut state = AppState::default();
        let bob = User { id: Id(5), username: "bob".into(), ..Default::default() };
        state.users.insert(bob.id, bob);
        let r = |t: &str| resolve_tokens(t, |tok| token_name(&state, None, &[], tok));
        assert_eq!(r("hi <@5> and <@!5>"), "hi @bob and @bob");
        assert_eq!(r("<@9> <:wave:123> <a:dance:7>"), "@unknown-user :wave: :dance:");
        assert_eq!(r("a < b > c <@x> <"), "a < b > c <@x> <");
    }

    /// Headless frames of the real view, built from the test fixture.
    struct Harness {
        ctx: egui::Context,
        state: AppState,
        view: crate::ui::ViewState,
        images: images::ImageCache,
        theme: Theme,
        settings: crate::config::Settings,
        cmd: crate::backend::CommandSender,
        overrides: crate::overrides::LocalOverrides,
        _rx: tokio::sync::mpsc::UnboundedReceiver<Command>,
    }

    impl Harness {
        fn new() -> Self {
            let snapshot = serde_json::from_str(include_str!("../../tests/fixtures/home/cache/snapshot.json")).unwrap();
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let cmd = crate::backend::CommandSender(tx);
            let ctx = egui::Context::default();
            let theme = Theme::ember_dark();
            crate::theme::ThemeApplier::default().apply(&ctx, &theme);
            Self {
                ctx,
                state: AppState::from_snapshot(snapshot),
                view: Default::default(),
                images: images::ImageCache::new(cmd.clone()),
                theme,
                settings: Default::default(),
                cmd,
                overrides: Default::default(),
                _rx: rx,
            }
        }

        fn frame(&mut self) {
            self.frame_with(Vec::new());
        }

        fn frame_with(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
            let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 600.0));
            let input = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
            let ctx = self.ctx.clone();
            let mut out = ctx.run_ui(input, |ui| {
                let mut cx = UiCx {
                    state: &mut self.state,
                    view: &mut self.view,
                    images: &mut self.images,
                    theme: &mut self.theme,
                    settings: &mut self.settings,
                    cmd: &self.cmd,
                    overrides: &mut self.overrides,
                };
                // Laid out first, as the shell does.
                if let Some(t) = cx.view.thread_panel {
                    egui::Panel::right("thread_panel").exact_size(400.0).show(ui, |ui| thread_chat(ui, &mut cx, t));
                }
                egui::CentralPanel::default().show(ui, |ui| show(ui, &mut cx));
                crate::ui::profile::show_popup(ui.ctx(), &mut cx);
            });
            out.textures_delta.clear();
            out
        }

        /// The text drawn by one frame.
        fn texts(&mut self) -> Vec<String> {
            let out = self.frame_with(Vec::new());
            out.shapes.iter().filter_map(|s| if let Shape::Text(t) = &s.shape { Some(t.galley.text().to_owned()) } else { None }).collect()
        }

        fn memo(&self) -> Memo {
            self.ctx.data(|d| d.get_temp(memo_id())).unwrap_or_default()
        }
    }

    #[test]
    fn thread_panel_beside_the_chat_keeps_its_own_state() {
        let mut h = Harness::new();
        let ch = h.state.selected_channel.unwrap();
        let thread = Id(9_000);
        let parent = h.state.channels[&ch].clone();
        h.state.channels.insert(thread, crate::model::Channel { id: thread, kind: channel_type::PUBLIC_THREAD, parent_id: Some(ch), ..parent });
        let msgs: Vec<Message> = h.state.messages[&ch].msgs.iter().map(|m| Message { channel_id: thread, ..m.clone() }).collect();
        let mid = msgs.last().unwrap().id;
        h.state.messages.insert(thread, crate::state::ChannelMessages { msgs, loaded: true, ..Default::default() });
        h.view.thread_panel = Some(thread);
        h.view.reply_to = Some((ch, mid, true));
        h.view.reaction_picker = Some((thread, mid));
        h.frame();
        assert_eq!(h.view.reply_to.map(|r| r.0), Some(ch), "the panel's box keeps the chat's reply");
        assert_eq!(h.view.reaction_picker, Some((thread, mid)), "the chat's list keeps the panel's picker");
        let panel: Memo = h.ctx.data(|d| d.get_temp(memo_id().with("thread_panel"))).unwrap_or_default();
        assert_eq!((panel.channel, h.memo().channel), (Some(thread), Some(ch)), "one list memory each");
        assert!(!panel.to_bottom, "the panel opened at its newest message");
        h.view.thread_panel = None;
        h.frame();
        assert_eq!(h.view.reaction_picker, None, "dropped with the panel");
    }

    #[test]
    fn prepending_history_keeps_reading_position() {
        let mut h = Harness::new();
        let ch = h.state.selected_channel.unwrap();
        let cm = h.state.messages.get_mut(&ch).unwrap();
        let older: Vec<Message> = cm.msgs.drain(..10).collect();
        (cm.has_more_before, cm.loaded, cm.loading) = (true, true, false);
        h.frame();
        assert!(h.memo().at_bottom, "opens at the bottom");

        // Read somewhere in the middle, then an older page arrives.
        h.view.scroll_to_message = Some(h.state.messages[&ch].msgs[8].id);
        h.frame();
        let before = h.memo();
        let (anchor, top) = before.anchor.unwrap();
        assert!(!before.at_bottom && before.offset > 0.0);
        h.state.messages.get_mut(&ch).unwrap().msgs.splice(0..0, older);
        h.frame();
        let after = h.memo();
        let (anchor_after, top_after) = after.anchor.unwrap();
        assert_eq!(anchor_after, anchor);
        assert!(top_after > top + 100.0, "rows were prepended above");
        let drift = (top_after - after.offset) - (top - before.offset);
        assert!(drift.abs() < 1.0, "anchor moved on screen by {drift}");

        // Back to the bottom: it sticks there when a new message arrives.
        h.view.scroll_to_bottom = true;
        h.frame();
        assert!(h.memo().at_bottom);
        let mut new = h.state.messages[&ch].msgs.last().unwrap().clone();
        new.id = Id(new.id.0 + (60_000 << 22));
        new.content = "a fresh message\nwith two lines".into();
        h.state.messages.get_mut(&ch).unwrap().msgs.push(new);
        h.frame();
        assert!(h.memo().at_bottom, "stays stuck to the bottom");
    }

    #[test]
    fn overlays_edit_and_compact_mode_render() {
        let mut h = Harness::new();
        let ch = h.state.selected_channel.unwrap();
        let m = h.state.messages[&ch].msgs.last().unwrap().clone();
        let guild_id = h.state.channels[&ch].guild_id;
        h.view.profile_card = Some(crate::ui::ProfileTarget { user_id: m.author.id, guild_id, anchor: egui::pos2(300.0, 200.0), pivot: egui::Align2::LEFT_TOP });
        h.view.confirm_delete = Some((ch, m.id));
        h.view.reaction_picker = Some((ch, m.id));
        h.view.editing = Some((ch, m.id));
        h.view.edit_text = m.content.clone();
        h.view.reply_to = Some((ch, m.id, true));
        h.frame();
        h.frame();
        assert!(h.view.profile_card.is_some() && h.view.confirm_delete.is_some());
        assert_eq!(h.view.reaction_picker.is_some(), h.state.can_react(ch));
        assert_eq!(h.memo().edit_focus, Some(m.id));
        h.theme.layout.compact = true;
        h.frame();
        assert_eq!(h.view.editing, Some((ch, m.id)));
    }

    #[test]
    fn rows_just_off_screen_skip_layout_without_changing_the_picture() {
        let mut h = Harness::new();
        // (text, position) of every text shape its clip rect lets through.
        let visible = |out: &egui::FullOutput| -> Vec<(String, egui::Pos2)> {
            let shown = |s: &&egui::epaint::ClippedShape| matches!(&s.shape, Shape::Text(t) if s.clip_rect.intersects(t.visual_bounding_rect()));
            out.shapes.iter().filter(shown).filter_map(|s| if let Shape::Text(t) = &s.shape { Some((t.galley.text().to_owned(), t.pos)) } else { None }).collect()
        };
        let texts = |out: &egui::FullOutput| out.shapes.iter().filter(|s| matches!(s.shape, Shape::Text(_))).count();
        h.frame();
        h.frame();
        // Measured a frame ago: the rows above the viewport keep their heights.
        let skipped = h.frame_with(Vec::new());
        // Tab walks this pass's widgets: every row in the margin is laid out for it.
        let tabbed = h.frame_with(vec![tab(true), tab(false)]);
        // A screen reader on: every row in the margin is laid out again.
        h.ctx.enable_accesskit();
        let full = h.frame_with(Vec::new());
        assert!(!visible(&full).is_empty());
        assert_eq!(visible(&skipped), visible(&full));
        assert!(texts(&skipped) < texts(&full), "rows off screen were laid out anyway");
        assert_eq!(texts(&tabbed), texts(&full), "a Tab press skipped rows");
    }

    fn tab(pressed: bool) -> egui::Event {
        egui::Event::Key { key: egui::Key::Tab, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE }
    }

    #[test]
    fn focused_row_off_screen_stays_laid_out_when_history_is_prepended() {
        let mut h = Harness::new();
        let ch = h.state.selected_channel.unwrap();
        let cm = h.state.messages.get_mut(&ch).unwrap();
        let older: Vec<Message> = cm.msgs.drain(..2).collect();
        (cm.has_more_before, cm.loaded, cm.loading) = (true, true, false);
        h.frame();
        h.frame();
        // Past the composer, Tab lands in the first laid-out row: above the viewport.
        for _ in 0..40 {
            if h.memo().focus_row.is_some() {
                break;
            }
            h.frame_with(vec![tab(true), tab(false)]);
        }
        let row = h.memo().focus_row.expect("Tab reached a row");
        let at = |h: &Harness, id| h.state.messages[&ch].msgs.iter().position(|m| m.id == id);
        assert!(at(&h, row) < at(&h, h.memo().anchor.unwrap().0), "the focused row is off screen");
        h.frame();
        // Older history moves the row down, away from the focused widget's last rect; measured a
        // pass ago and still off screen, it is laid out for the focus.
        h.state.messages.get_mut(&ch).unwrap().msgs.splice(0..0, older);
        h.frame();
        assert_eq!(h.memo().heights[&row].at, h.ctx.input(|i| i.time));
    }

    #[test]
    fn text_style_sizes_are_part_of_the_layout_key() {
        // High Contrast's body size under Chat font scaling changes no fingerprint.
        let mut h = Harness::new();
        h.frame();
        let old = h.memo().heights.values().next().unwrap().key;
        h.ctx.global_style_mut(|s| {
            s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(21.0));
        });
        h.frame();
        assert!(h.memo().heights.values().any(|r| r.key != old));
    }

    #[test]
    fn new_divider_stays_for_the_visit() {
        let mut h = Harness::new();
        let ch = h.state.selected_channel.unwrap();
        let ids: Vec<Id> = h.state.messages[&ch].msgs.iter().map(|m| m.id).collect();
        let read = ids[ids.len() - 4];
        h.state.channels.get_mut(&ch).unwrap().last_message_id = ids.last().copied();
        h.state.read_states.get_mut(&ch).unwrap().last_message_id = Some(read);
        // Reopened with unread messages: the read position is captured before any ack.
        h.state.select_channel(h.state.dm_order[0]);
        h.state.select_channel(ch);
        h.frame();
        assert_eq!(h.state.unread_marker, Some((ch, read)));
        // Acked while viewing: the divider keeps its place until the channel is left.
        h.state.mark_read(ch);
        h.frame();
        assert_eq!(h.state.unread_marker, Some((ch, read)));
    }

    #[test]
    fn empty_dm_offline_opens_on_its_start() {
        let mut h = Harness::new();
        let dm = h.state.dm_order[0];
        h.state.select_channel(dm);
        let start = format!("This is the start of your conversation with {}.", h.state.channel_name(dm));
        let texts = h.texts();
        assert!(texts.contains(&start), "{texts:?}");
        assert!(texts.iter().any(|t| t == "Messages load when you're connected."), "{texts:?}");
    }

    #[test]
    fn context_menu_opens_and_closes_on_escape() {
        let mut h = Harness::new();
        let ch = h.state.selected_channel.unwrap();
        let mid = h.state.messages[&ch].msgs.last().unwrap().id;
        h.frame();
        let mut memo = h.memo();
        memo.menu = Some(MsgMenu { id: mid, anchor: Rect::from_pos(pos2(400.0, 300.0)), from_button: false, new: true });
        h.ctx.data_mut(|d| d.insert_temp(memo_id(), memo));
        h.frame();
        h.frame();
        assert!(egui::Popup::is_any_open(&h.ctx) && h.memo().menu.is_some());
        let esc = egui::Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
        h.frame_with(vec![esc]);
        h.frame();
        assert!(h.memo().menu.is_none() && !egui::Popup::is_any_open(&h.ctx));
    }

    #[test]
    fn jump_bar_shows_when_scrolled_up() {
        let mut h = Harness::new();
        let ch = h.state.selected_channel.unwrap();
        h.frame();
        assert!(!viewing_older(&h.state, ch, &h.memo()));
        h.view.scroll_to_message = Some(h.state.messages[&ch].msgs[1].id);
        h.frame();
        h.frame();
        assert!(viewing_older(&h.state, ch, &h.memo()));
    }

    #[test]
    fn quick_reactions_prefer_recent_ones() {
        let ctx = egui::Context::default();
        let uni = |e: &str| Emoji { id: None, name: Some(e.into()), animated: false };
        let names = |v: Vec<Emoji>| v.into_iter().map(|e| e.name.unwrap_or_default()).collect::<Vec<_>>();
        assert_eq!(names(quick_reactions(&ctx, |_| true)), ["👍", "❤️", "🔥"]);
        let foreign = Emoji { id: Some(Id(7)), name: Some("blob".into()), animated: false };
        ctx.data_mut(|d| d.insert_temp(recent_reactions_id(), vec![uni("🎉"), foreign, uni("👍")]));
        // Custom emoji of another guild are skipped; duplicates of the defaults collapse.
        assert_eq!(names(quick_reactions(&ctx, |_| false)), ["🎉", "👍", "❤️"]);
        assert_eq!(names(quick_reactions(&ctx, |id| id == Id(7))), ["🎉", "blob", "👍"]);
    }

    #[test]
    fn emoji_without_glyph_becomes_shortcode() {
        let h = Harness::new();
        let mut got = Vec::new();
        let mut out = h.ctx.run_ui(Default::default(), |ui| {
            got = ["plain text", "ok 😀"].map(|t| missing_emoji(ui, t)).to_vec();
            // An emoji newer than every installed font (which one depends on the OS version).
            let font = FontId::proportional(20.0);
            let lacking = emojis::iter().filter(|e| e.as_str().chars().count() == 1).find(|e| {
                let c = e.as_str().chars().next().unwrap_or(' ');
                ('\u{1F400}'..='\u{1FAFF}').contains(&c) && !ui.fonts_mut(|f| f.has_glyph(&font, c))
            });
            if let Some(e) = lacking {
                let code = crate::emoji::shortcode(e).unwrap_or(e.name());
                assert_eq!(missing_emoji(ui, &format!("hi {e}!")), Some((3..3 + e.as_str().len(), code)));
            }
            // The OS color font has 🦀; egui's monochrome one does not.
            #[cfg(windows)]
            assert_eq!(missing_emoji(ui, "hi 🦀!"), None);
        });
        out.textures_delta.clear();
        assert_eq!(got, [None, None]);
    }

    #[test]
    fn message_links() {
        assert_eq!(message_link(Some(Id(1)), Id(2), Id(3)), "https://discord.com/channels/1/2/3");
        assert_eq!(message_link(None, Id(2), Id(3)), "https://discord.com/channels/@me/2/3");
    }

    #[test]
    fn hides_content_that_is_only_a_media_link() {
        let mut m = msg(NOON, 1);
        m.content = "https://example.com/a.png".into();
        m.embeds.push(Embed { kind: Some("image".into()), url: Some(m.content.clone()), ..Default::default() });
        assert!(content_is_media_link(&m));
        m.embeds[0].kind = Some("rich".into());
        assert!(!content_is_media_link(&m));
    }
}
