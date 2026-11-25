//! Message input like Discord's box: "+" (Upload a File, Create Thread) on the left, GIF and emoji buttons on
//! the right, an attachment tray above the text (file dialog, drag and drop anywhere, pasted
//! images), `@` / `#` / `:` autocomplete (rows from autocomplete.rs in a popup as wide as the box,
//! right above it), the expression picker (pickers.rs) and the typing line under the box (with
//! the slowmode countdown on its right). Uploads in flight show in the message list
//! (`messages::pending_rows`). Enter sends (with files: `SendWithFiles`), Shift+Enter newline,
//! Esc cancels the reply, Up on empty input edits my last message. Chips: offline, length.
//! Disabled without SEND_MESSAGES.
//! Test hooks (`RUSTCORD_MSG_DEMO`, see `messages::demo`): `attach` (tray with sample files and
//! an error), `upload` (an upload in flight), `typing` (two people typing), `gif` (GIF tab with
//! placeholder categories and results), `ac` / `ac-channel` / `ac-emoji` (`@` / `#` / `:`
//! autocomplete open), `cooldown` (send cooldown), `drop` (the drag-and-drop overlay), `online` (no
//! "Offline" chip, for README screenshots).
//! OWNERSHIP: composer engineer.

use super::autocomplete::{self, Row};
use super::messages::{channel_prefix, demo, icon_button, menu_item, menu_setup, message_member, overlay_open, small_button, status_chip, MenuIcon, Pal, COUNTER_FROM};
use super::pickers::{self, Pick, Tab};
use super::{images, one_line, sidebar, UiCx};
use crate::api::{cdn, upload::MAX_FILES};
use crate::backend::{ChatCommand, Command, ConnState, ReplyTo, UploadFile, UploadSource};
use crate::model::{perm, GifItem, Id};
use crate::state::chat::{GifView, Typist, Upload};
use crate::state::PendingMessage;
use crate::theme::bold_family;
use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::{
    pos2, vec2, Align, Align2, Color32, ColorImage, EventFilter, FontId, Frame, Image, Key, KeyboardShortcut, Label, Layout, Margin, Modifiers,
    Order, Rect, RectAlign, RichText, ScrollArea, Sense, Stroke, TextEdit, TextureHandle, TextureOptions, UiBuilder, Vec2,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The input grows up to this many rows, then scrolls.
const MAX_ROWS: f32 = 8.0;
/// Tray card; its preview is the card minus 56 px (name and size). Short windows get `SMALL_CARD_H`.
const CARD: Vec2 = vec2(152.0, 136.0);
const SMALL_CARD_H: f32 = 104.0;
/// Margins of the tray row around its cards.
const TRAY_PAD_H: f32 = 28.0;
/// One height for every autocomplete row (avatars, channels, emoji).
const AC_ROW_H: f32 = 36.0;
const AC_HEADER_H: f32 = 28.0;
/// Thumbnail texture side (px): sharp at 2x on the 136 px preview, ~100 KB each.
const THUMB_PX: u32 = 272;
/// Thumbnails are skipped for images whose decode would allocate more (transient RAM).
const THUMB_MAX_ALLOC: u64 = 48 << 20;
/// Inline errors hide after this many seconds.
const ERROR_S: f64 = 6.0;
/// Height of the typing line under the box (+ the panel's 8 px margin = Discord's 24).
const TYPING_H: f32 = 16.0;

/// One file waiting in the tray.
#[derive(Clone)]
struct PendingFile {
    file: UploadFile,
    size: u64,
    /// Small preview of an image (freed with the entry).
    thumb: Option<TextureHandle>,
}

/// Per-channel tray (temp memory, like the drafts it belongs to).
#[derive(Clone, Default)]
struct Tray {
    files: Vec<PendingFile>,
    /// Files still being read / encoded off the UI thread.
    loading: usize,
    /// Short inline error and the ctx time it hides at.
    error: Option<(String, f64)>,
}

/// Files prepared off the UI thread: (channel, file or error text).
static INBOX: Mutex<Vec<(Id, Result<PendingFile, String>)>> = Mutex::new(Vec::new());

enum Job {
    Path(PathBuf),
    Pasted(Arc<ColorImage>),
}

/// Autocomplete state between passes (per input), as egui's `CompletionPopup` keeps it.
#[derive(Clone, Default)]
struct AcState {
    selected: usize,
    /// The word `selected` belongs to; a new word starts at the first row.
    word: String,
    open: bool,
    /// The word whose popup Esc closed: it stays closed until the word changes.
    dismissed: Option<String>,
}

/// Popup keys of this pass, taken before the input sees them.
#[derive(Default)]
struct AcKeys {
    down: usize,
    up: usize,
    accept: bool,
    dismiss: bool,
}

impl AcState {
    fn take_keys(&self, ui: &egui::Ui) -> AcKeys {
        if !self.open {
            return AcKeys::default();
        }
        // Unmodified presses only: Shift+Enter still makes a newline.
        let take = |i: &mut egui::InputState, key: Key| {
            let n = i.events.len();
            i.events.retain(|e| !matches!(e, egui::Event::Key { key: k, pressed: true, modifiers, .. } if *k == key && modifiers.is_none()));
            n - i.events.len()
        };
        ui.input_mut(|i| AcKeys {
            down: take(i, Key::ArrowDown),
            up: take(i, Key::ArrowUp),
            accept: take(i, Key::Enter) + take(i, Key::Tab) > 0,
            dismiss: take(i, Key::Escape) > 0,
        })
    }

    /// The rows for `word` after `keys`, and the row Enter/Tab accepted.
    fn update(&mut self, word: &str, keys: &AcKeys, rows: impl FnOnce() -> Vec<Row>) -> (Vec<Row>, Option<usize>) {
        if keys.dismiss {
            self.dismissed = Some(word.to_owned());
        }
        let rows = if self.dismissed.as_deref() == Some(word) { Vec::new() } else { rows() };
        if self.word != word {
            self.selected = 0;
            self.word = word.to_owned();
        }
        if let Some(last) = rows.len().checked_sub(1) {
            let n = rows.len();
            self.selected = (self.selected.min(last) + keys.down + n - keys.up % n) % n;
        }
        let accept = (keys.accept && !rows.is_empty()).then_some(self.selected);
        (rows, accept)
    }
}

/// The open autocomplete of this pass: its rows, the word's char range and the popup title.
struct AcView {
    rows: Vec<Row>,
    range: std::ops::Range<usize>,
    title: String,
    accepted: Option<usize>,
    moved: bool,
    focused: bool,
}

/// The box for `ch`. `panel`: the thread panel's (threads.rs), shown beside the open channel's:
/// its own focus; drops, pastes and the expression picker go to whichever box has the focus
/// (the channel's when neither).
pub fn show(ui: &mut egui::Ui, cx: &mut UiCx<'_>, ch: Id, chat_top: f32, panel: bool) {
    let ctx = ui.ctx().clone();
    let pal = Pal::new(cx.theme);
    let r = pal.card_radius();
    // E1: `surface`, the container radius, ~56 px tall at one row; no border (High Contrast
    // keeps one: its `surface` sits close to the chat).
    let edge = if pal.tk.mode == crate::theme::Mode::HighContrast { Stroke::new(1.0, pal.tk.rule) } else { Stroke::NONE };
    let card = Frame::new().fill(pal.surface).stroke(edge).corner_radius(r);
    let input_margin = Margin { left: 16, right: 12, top: 17, bottom: 17 };
    if !cx.state.can_send(ch) {
        if !panel {
            pickers::close_if_open(&ctx, cx);
        }
        card.inner_margin(input_margin).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(Label::new(RichText::new("You can't send messages here").color(pal.muted)).selectable(false))
                .on_hover_text("You don't have permission to send messages in this channel.");
        });
        typing_line(ui, cx, &pal, ch, 0);
        return;
    }
    // A reply ends with its channel leaving the screen (chat or thread panel).
    if cx.view.reply_to.is_some_and(|(c, _, _)| cx.state.selected_channel != Some(c) && cx.view.thread_panel != Some(c)) {
        cx.view.reply_to = None;
    }
    let reply_here = cx.view.reply_to.filter(|r| r.0 == ch);
    if !panel {
        demo_setup(&ctx, cx, ch);
    }
    let (kind, guild_id) = cx.state.channels.get(&ch).map_or((0, None), |c| (c.kind, c.guild_id));
    let hint = format!("Message {}{}", channel_prefix(kind), cx.state.channel_name(ch));
    // While (re)connecting, sends still go out over REST.
    let offline = cx.state.conn == ConnState::Offline && !demo("online");
    let wait_ms = slowmode_wait(cx, ch).max(cooldown_ms(cx, ch, Instant::now()));
    let can_attach = cx.state.channels.get(&ch).is_some_and(|c| c.guild_id.is_none()) || cx.state.channel_permissions(ch) & perm::ATTACH_FILES != 0;
    let now = ctx.input(|i| i.time);

    // Focus the input when the channel changes, a reply starts or an inline edit here ends.
    // An edit open in another channel does not count: its box is not on screen.
    let editing_here = cx.view.editing.is_some_and(|(c, _)| c == ch);
    let marker = (ch, reply_here.map(|r| r.1), editing_here);
    let marker_id = egui::Id::unique("rc_composer_focus").with(panel);
    let want_focus = !editing_here && ctx.data(|d| d.get_temp::<(Id, Option<Id>, bool)>(marker_id)) != Some(marker);
    ctx.data_mut(|d| d.insert_temp(marker_id, marker));

    let row_h = ui.text_style_height(&egui::TextStyle::Body);
    let edit_id = ui.make_persistent_id(("rc_composer", ch.0));
    let pass = ctx.cumulative_pass_nr();
    // The panel's box comes first in a pass (its panel is laid out before the chat).
    let panel_box = if panel {
        ctx.data_mut(|d| d.insert_temp(panel_box_id(), (edit_id, pass)));
        Some(edit_id)
    } else {
        ctx.data(|d| d.get_temp::<(egui::Id, u64)>(panel_box_id())).filter(|(_, p)| *p == pass).map(|(id, _)| id)
    };
    let focused = ctx.memory(|m| m.focused());
    // Two boxes on screen: the focused one takes drops and pastes, else the channel's.
    let takes_files = if panel { focused == Some(edit_id) } else { panel_box.is_none() || focused != panel_box };
    let mut trays: HashMap<Id, Tray> = ctx.data_mut(|d| d.remove_temp(trays_id())).unwrap_or_default();
    collect_files(&ctx, cx, &mut trays, ch, takes_files.then_some(edit_id), can_attach, &pal);
    let mut tray = trays.remove(&ch).unwrap_or_default();
    if tray.error.as_ref().is_some_and(|(_, until)| *until <= now) {
        tray.error = None;
    }
    if let Some((_, until)) = &tray.error {
        ctx.request_repaint_after(Duration::from_secs_f64(until - now));
    }

    let len_before = cx.view.drafts.get(&ch).map_or(0, |d| d.chars().count());
    // Plugins (CharacterCounter): a counter on every non-empty draft. Nitro's limit is 4000.
    let counting = crate::plugins::compose::counting();
    let max_len = crate::plugins::compose::max_len(cx.state.me.as_ref().map_or(0, |u| u.premium_type));
    // The chip counts what the send checks: mentions and custom emoji expanded.
    let shown_len = match cx.view.drafts.get(&ch) {
        Some(d) if len_before > COUNTER_FROM || counting => autocomplete::expand(d, &mentions(&ctx, ch)).chars().count(),
        _ => len_before,
    };
    let picker_tab = pickers::open_tab(&ctx);
    let (mut toggle, mut pick_files, mut new_thread) = (None, false, false);
    let thread_ok = super::threads::has_threads(kind) && cx.state.can_create_thread(ch, false);
    let mut member_query = None;
    let ac_id = edit_id.with("ac");
    let ac_area = ac_id.with("popup");
    let mut ac: AcState = ctx.data(|d| d.get_temp(ac_id)).unwrap_or_default();
    let mut ac_view = None;
    // One calm card: reply bar, the tray, then the input row.
    let shown = card.show(ui, |ui| {
        // The box's outer top (the content starts inside its 1 px border).
        let card_top = ui.max_rect().top() - 1.0;
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        if let Some((_, mid, ping)) = reply_here {
            reply_bar(ui, cx, &pal, ch, guild_id, mid, ping);
        }
        if !tray.files.is_empty() || tray.loading > 0 {
            // Short windows (720x420): smaller cards, so a whole card still fits and the history keeps ~120 px.
            let room = ctx.content_rect().height() - 280.0;
            let card_h = if room >= CARD.y + TRAY_PAD_H { CARD.y } else { SMALL_CARD_H };
            tray_row(ui, &pal, &mut tray, ch, card_h);
        }
        // Outside any scrolling: the only feedback on a refused file.
        if let Some((err, _)) = &tray.error {
            Frame::new().inner_margin(Margin { left: 16, right: 16, top: 8, bottom: 0 }).show(ui, |ui| {
                ui.add(Label::new(RichText::new(err).size(pal.meta()).color(pal.danger)).truncate());
            });
        }
        Frame::new()
            .inner_margin(input_margin)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    // Right-to-left: the first item ends up rightmost.
                    let emoji = slot_button(ui, row_h, |ui| icon_button(ui, &pal, "☺", "Emoji", 24.0, picker_tab == Some(Tab::Emoji), pal.text));
                    let gif = slot_button(ui, row_h, |ui| painted_button(ui, &pal, "GIF", picker_tab == Some(Tab::Gif), paint_gif_badge));
                    crate::plugins::chat::silent_button(ui, row_h, (pal.text, pal.muted, pal.tint()));
                    if emoji.clicked() {
                        toggle = Some(Tab::Emoji);
                    } else if gif.clicked() {
                        toggle = Some(Tab::Gif);
                    }
                    let counter = crate::plugins::compose::counter(shown_len, max_len).filter(|_| counting || shown_len > COUNTER_FROM);
                    if let Some((text, over)) = counter {
                        status_chip(ui, &pal, None, &text, if over { pal.danger } else { pal.muted }).on_hover_text("Message length limit");
                    }
                    if offline {
                        status_chip(ui, &pal, Some(pal.muted), "Offline", pal.muted).on_hover_text("Offline – sending disabled until reconnected");
                    }
                    ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
                        if can_attach {
                            ui.spacing_mut().item_spacing.x = 12.0;
                            let plus = slot_button(ui, row_h, |ui| painted_button(ui, &pal, "Upload a file", false, paint_plus));
                            // Fully above the box with 8 px of air (Discord), whatever sits above the row.
                            egui::Popup::menu(&plus).align(RectAlign::TOP_START).gap(plus.rect.top() - card_top + 8.0).show(|ui| {
                                menu_setup(ui);
                                pick_files = menu_item(ui, "Upload a file", MenuIcon::Glyph("📎"), pal.text);
                                new_thread = thread_ok && menu_item(ui, "Create thread", MenuIcon::Glyph("🧵"), pal.text);
                            });
                        }
                        let state = &*cx.state;
                        let draft = cx.view.drafts.entry(ch).or_default();
                        ScrollArea::vertical()
                            .id_salt(("rc_composer_scroll", ch.0))
                            .max_height(row_h * MAX_ROWS)
                            .show(ui, |ui| {
                                // Keys for the open popup first, so the input never sees them.
                                let keys = ac.take_keys(ui);
                                let edit = TextEdit::multiline(draft)
                                    .id(edit_id)
                                    .hint_text(fit_hint(ui, hint))
                                    .desired_rows(1)
                                    .desired_width(f32::INFINITY)
                                    .frame(Frame::NONE)
                                    // Only Shift+Enter inserts a newline; plain Enter sends (handled below).
                                    .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter));
                                let filter = edit.get_event_filter();
                                let edit = edit.event_filter(EventFilter { tab: filter.tab || ac.open, escape: filter.escape || ac.open, ..filter });
                                // A click on the popup must not take the focus from the input.
                                let layer = egui::LayerId::new(Order::Foreground, ac_area);
                                let on_popup = ac.open && ctx.pointer_interact_pos().and_then(|p| ctx.layer_id_at(p)) == Some(layer);
                                let surrender = ctx.options(|o| o.input_options.surrender_focus_on);
                                if on_popup {
                                    ctx.options_mut(|o| o.input_options.surrender_focus_on = egui::SurrenderFocusOn::Never);
                                }
                                let out = edit.show(ui);
                                ctx.options_mut(|o| o.input_options.surrender_focus_on = surrender);
                                let resp = out.response.response;
                                let cursor = out.cursor_range.map_or_else(|| draft.chars().count(), |r| r.primary.index.0);
                                let start = draft.chars().take(cursor).enumerate().filter(|(_, c)| c.is_whitespace()).last().map_or(0, |(i, _)| i + 1);
                                let word: String = draft.chars().skip(start).take(cursor - start).collect();
                                let glyph = |s: &str| pickers::drawable(&ctx, s);
                                let (rows, accepted) = ac.update(&word, &keys, || {
                                    if let Some(autocomplete::Trigger::Member(m)) = autocomplete::trigger(&word) {
                                        member_query = Some(m.to_owned());
                                    }
                                    crate::plugins::chat::sort_emoji_rows(autocomplete::suggest(state, ch, &word, &glyph), &cx.settings.frequent_emoji)
                                });
                                if !rows.is_empty() {
                                    let title = autocomplete::title(&word);
                                    ac_view = Some(AcView { rows, range: start..cursor, title, accepted, moved: keys.down != keys.up, focused: resp.has_focus() });
                                }
                                resp
                            })
                            .inner
                    })
                    .inner
                })
                .inner
            })
            .inner
    });
    let resp = shown.inner;
    let frame_rect = shown.response.rect;
    // The popup and the row it accepted (Enter / Tab this pass, or a click).
    let mut ac_open = false;
    if let Some(v) = ac_view {
        let mut pick = v.accepted;
        if pick.is_none() && v.focused {
            // Between the chat header and the box, 8 px off both: it never covers the input.
            let max_h = (frame_rect.top() - chat_top - 16.0).max(AC_HEADER_H + AC_ROW_H + 16.0);
            pick = ac_popup(&ctx, cx, &pal, guild_id, ac_area, frame_rect, max_h, &v.title, &v.rows, &mut ac.selected, v.moved);
            ac_open = pick.is_none();
        }
        if let Some(row) = pick.and_then(|i| v.rows.get(i)) {
            accept_row(&ctx, cx, ch, edit_id, v.range, row);
            ac.dismissed = None;
            ac.selected = 0;
        }
    }
    ac.open = ac_open;
    ctx.data_mut(|d| d.insert_temp(ac_id, ac));

    let draft = cx.view.drafts.entry(ch).or_default();
    let len = draft.chars().count();
    let empty = draft.is_empty();
    // "I'm typing" on edits of real text (not commands); the backend throttles to one per 8 s.
    if resp.changed() && !draft.trim().is_empty() && !draft.starts_with('/') && !crate::plugins::chat::silent_typing() {
        cx.cmd.send(Command::Chat(ChatCommand::Typing { channel_id: ch }));
    }
    // The counter chip was drawn before this frame's typing.
    if (counting || len > COUNTER_FROM || len_before > COUNTER_FROM) && len != len_before {
        ctx.request_repaint();
    }
    if empty {
        forget_mentions(&ctx, ch);
    }
    if let (Some(q), Some(g)) = (member_query, guild_id) {
        search_members(&ctx, cx, g, q);
    }

    let (enter, esc, up) = ui.input(|i| (i.key_pressed(Key::Enter) && !i.modifiers.shift, i.key_pressed(Key::Escape), i.key_pressed(Key::ArrowUp)));
    let overlay = editing_here || overlay_open(&ctx, cx);
    if resp.has_focus() && enter {
        let text = autocomplete::expand(cx.view.drafts.get(&ch).map_or("", |d| d), &mentions(&ctx, ch));
        // Plugins (TextReplace): the user's rules, once, on this send.
        let text = crate::plugins::compose::text_replace(text);
        let content = prepare_send(&text, !tray.files.is_empty(), max_len);
        // Files still being prepared belong to this message: wait for them (the draft stays).
        let refused = if tray.loading > 0 {
            Some("Still preparing files…".to_owned())
        } else if content.is_none() && !text.trim().is_empty() {
            Some(format!("Message is over {max_len} characters"))
        } else {
            None
        };
        if let Some(e) = refused {
            tray.error = Some((e, now + ERROR_S));
            ctx.request_repaint();
        } else if let (0, Some(content)) = (wait_ms, content) {
            let files = tray.files.drain(..).map(|f| f.file).collect();
            send(cx, ch, guild_id, content, files);
            tray.error = None;
            if let Some(d) = cx.view.drafts.get_mut(&ch) {
                d.clear();
            }
            forget_mentions(&ctx, ch);
        }
        resp.request_focus();
    } else if esc && !overlay && (reply_here.is_some() || resp.lost_focus()) {
        // egui drops focus on Esc; keep typing possible without a click.
        if reply_here.is_some() {
            cx.view.reply_to = None;
        }
        resp.request_focus();
        ctx.request_repaint();
    } else if up && empty && resp.has_focus() {
        edit_last(cx, ch);
    } else if want_focus && (panel || panel_box.is_none()) {
        // A thread panel opening beside the chat takes the focus.
        resp.request_focus();
    }

    typing_line(ui, cx, &pal, ch, wait_ms);
    // One picker for both boxes: drawn by the box that opened it (the channel's when the panel's
    // is gone).
    if toggle.is_some() {
        ctx.data_mut(|d| d.insert_temp(picker_owner_id(), panel));
    }
    let panel_owns = ctx.data(|d| d.get_temp::<bool>(picker_owner_id())).unwrap_or(false);
    let pick = if panel { panel_owns } else { !panel_owns || panel_box.is_none() };
    // After the Esc handling above, which must still see the picker open.
    let picked = if pick { pickers::show(&ctx, cx, &pal, ch, frame_rect, toggle) } else { None };
    match picked {
        Some(Pick::Insert(text)) => {
            // Custom emoji read `:name:` in the draft, like the autocomplete's.
            let readable = autocomplete::readable_emoji(&text);
            if let Some(r) = &readable {
                remember_mention(&ctx, ch, r, &text);
            }
            let draft = cx.view.drafts.entry(ch).or_default();
            insert_at_cursor(&ctx, edit_id, draft, readable.as_deref().unwrap_or(&text));
        }
        Some(Pick::SendGif(url)) if wait_ms == 0 => send(cx, ch, guild_id, url, Vec::new()),
        Some(Pick::SendGif(_)) => cx.view.toast("Slowmode is on: try again in a moment", true),
        None => {}
    }
    if new_thread {
        super::threads::open_create(cx, ch, None);
    }
    // The one blocking call the UI allows: the user's own file dialog.
    if pick_files && let Some(paths) = rfd::FileDialog::new().set_title("Upload a file").pick_files() {
        let limit = cx.state.upload_limit(ch);
        add_jobs(&ctx, &mut tray, ch, paths.into_iter().map(Job::Path).collect(), limit);
    }
    trays.insert(ch, tray);
    trays.retain(|_, t| !t.files.is_empty() || t.loading > 0 || t.error.is_some());
    ctx.data_mut(|d| d.insert_temp(trays_id(), trays));
}

/// The placeholder cut to one line ("Message #a-long-na…"): a multiline input wraps it.
fn fit_hint(ui: &egui::Ui, hint: String) -> String {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let max = ui.available_width() - 8.0;
    let w = ui.painter().layout_no_wrap(hint.clone(), font, Color32::WHITE).size().x;
    if w <= max {
        return hint;
    }
    let keep = (hint.chars().count() as f32 * max / w) as usize;
    hint.chars().take(keep.saturating_sub(2)).chain(['…']).collect()
}

fn trays_id() -> egui::Id {
    egui::Id::unique("rc_composer_trays")
}

/// (input id, pass) of the thread panel's box in the pass it was drawn.
fn panel_box_id() -> egui::Id {
    egui::Id::unique("rc_panel_box")
}

/// Whether the thread panel's box opened the expression picker (else the channel's).
fn picker_owner_id() -> egui::Id {
    egui::Id::unique("rc_picker_owner")
}

/// Temp-memory key of the expression picker (also set by the `emoji` demo hook).
pub(super) fn emoji_picker_id() -> egui::Id {
    pickers::id()
}

/// Whether the composer's expression picker (GIFs / emoji) is open.
pub(super) fn emoji_picker_open(ctx: &egui::Context) -> bool {
    pickers::is_open(ctx)
}

/// A 24 px button centred on the first text line without making the row taller.
fn slot_button(ui: &mut egui::Ui, row_h: f32, add: impl FnOnce(&mut egui::Ui) -> egui::Response) -> egui::Response {
    let (slot, _) = ui.allocate_exact_size(vec2(24.0, row_h), Sense::hover());
    let mut child = ui.new_child(UiBuilder::new().max_rect(Rect::from_center_size(slot.center(), Vec2::splat(24.0))));
    add(&mut child)
}

/// `icon_button` with a painted icon: muted, bright on hover / while `on`.
fn painted_button(ui: &mut egui::Ui, pal: &Pal, tip: &str, on: bool, paint: fn(&egui::Painter, Rect, Color32, &Pal)) -> egui::Response {
    let resp = icon_button(ui, pal, "", tip, 24.0, on, pal.text);
    let color = if on || resp.hovered() || resp.has_focus() { pal.text } else { pal.muted };
    paint(ui.painter(), resp.rect, color, pal);
    resp
}

/// Discord's attach button: a filled circle with a plus cut out.
fn paint_plus(p: &egui::Painter, r: Rect, color: Color32, pal: &Pal) {
    let c = r.center();
    p.circle_filled(c, 10.0, color);
    let s = Stroke::new(2.0, pal.surface);
    p.line_segment([c - vec2(5.0, 0.0), c + vec2(5.0, 0.0)], s);
    p.line_segment([c - vec2(0.0, 5.0), c + vec2(0.0, 5.0)], s);
}

/// "GIF" in a rounded outline.
fn paint_gif_badge(p: &egui::Painter, r: Rect, color: Color32, _: &Pal) {
    let b = Rect::from_center_size(r.center(), vec2(20.0, 14.0));
    p.rect_stroke(b, 3, Stroke::new(1.6, color), egui::StrokeKind::Inside);
    p.text(b.center() + vec2(0.0, 0.5), Align2::CENTER_CENTER, "GIF", FontId::new(8.5, bold_family()), color);
}

/// Inserts `text` at the input's cursor (end if it never had one) and focuses it after.
fn insert_at_cursor(ctx: &egui::Context, edit_id: egui::Id, draft: &mut String, text: &str) {
    let mut state = egui::text_edit::TextEditState::load(ctx, edit_id).unwrap_or_default();
    let len = draft.chars().count();
    let at = state.cursor.char_range().map_or(len, |r| r.primary.index.0.min(len));
    let byte = draft.char_indices().nth(at).map_or(draft.len(), |(b, _)| b);
    draft.insert_str(byte, text);
    state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(at + text.chars().count()))));
    state.store(ctx, edit_id);
    ctx.memory_mut(|m| m.request_focus(edit_id));
}

// ---------------------------------------------------------------------------
// Autocomplete
// ---------------------------------------------------------------------------

/// Readable mentions of the drafts (channel -> (readable, wire)), expanded on send.
type Mentions = HashMap<Id, Vec<(String, String)>>;

fn mentions_id() -> egui::Id {
    trays_id().with("mentions")
}

fn remember_mention(ctx: &egui::Context, ch: Id, readable: &str, wire: &str) {
    ctx.data_mut(|d| {
        let list = d.get_temp_mut_or_default::<Mentions>(mentions_id()).entry(ch).or_default();
        // The newest wins: two people called "Alice" can't both be meant by "@Alice".
        list.retain(|(r, _)| r != readable);
        if list.len() >= 64 {
            list.remove(0);
        }
        list.push((readable.to_owned(), wire.to_owned()));
    });
}

fn mentions(ctx: &egui::Context, ch: Id) -> Vec<(String, String)> {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Mentions>(mentions_id()).get(&ch).cloned().unwrap_or_default())
}

fn forget_mentions(ctx: &egui::Context, ch: Id) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Mentions>(mentions_id()).remove(&ch));
}

/// An accepted row: its readable text replaces the word (chars `range`), its wire form is kept
/// for the send, and an emoji counts for "Frequently used".
fn accept_row(ctx: &egui::Context, cx: &mut UiCx<'_>, ch: Id, edit_id: egui::Id, range: std::ops::Range<usize>, row: &Row) {
    let draft = cx.view.drafts.entry(ch).or_default();
    let byte = |i: usize| draft.char_indices().nth(i).map_or(draft.len(), |(b, _)| b);
    let bytes = byte(range.start)..byte(range.end);
    draft.replace_range(bytes, &row.insert);
    let mut state = egui::text_edit::TextEditState::load(ctx, edit_id).unwrap_or_default();
    state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(range.start + row.insert.chars().count()))));
    state.store(ctx, edit_id);
    ctx.memory_mut(|m| m.request_focus(edit_id));
    if let Some(wire) = &row.wire {
        remember_mention(ctx, ch, row.insert.trim_end(), wire);
    }
    pickers::record_use(cx, row.wire.as_deref().unwrap_or(&row.insert));
    ctx.request_repaint();
}

/// The autocomplete card: as wide as the composer box (`anchor`), 8 px above it and at most
/// `max_h` tall, a header line over ≤ 10 rows of one height. Hovering selects; returns the
/// clicked row. `reveal` scrolls the selected row into view (arrow keys).
#[allow(clippy::too_many_arguments)]
fn ac_popup(
    ctx: &egui::Context,
    cx: &mut UiCx<'_>,
    pal: &Pal,
    guild_id: Option<Id>,
    id: egui::Id,
    anchor: Rect,
    max_h: f32,
    title: &str,
    rows: &[Row],
    selected: &mut usize,
    reveal: bool,
) -> Option<usize> {
    // E2: the raised float colour with a `rule` edge (the popup frame's shadow).
    let frame = Frame::popup(&ctx.global_style()).fill(pal.tk.raised).stroke(Stroke::new(1.0, pal.tk.rule)).corner_radius(pal.card_radius()).inner_margin(Margin::same(8));
    let margin = frame.total_margin().sum();
    let mut clicked = None;
    egui::Area::new(id).order(Order::Foreground).fixed_pos(anchor.left_top() - vec2(0.0, 8.0)).pivot(Align2::LEFT_BOTTOM).fade_in(false).show(ctx, |ui| {
        super::motion::own(ctx, ui.layer_id());
        frame.show(ui, |ui| {
            let w = anchor.width() - margin.x;
            ui.set_width(w);
            ui.spacing_mut().item_spacing.y = 0.0;
            let (head, _) = ui.allocate_exact_size(vec2(w, AC_HEADER_H), Sense::hover());
            let font = FontId::new(pal.label(), crate::theme::semibold_family());
            ui.painter().text(pos2(head.left() + 8.0, head.center().y - 2.0), Align2::LEFT_CENTER, title, font, pal.muted);
            let moving = ui.input(|i| i.pointer.is_moving());
            // The menus' hover fill, not the accent: role colors stay readable on it.
            let hot = ui.visuals().widgets.hovered.weak_bg_fill;
            // Whole rows only, so the last one never shows as a cut-off strip.
            let rows_h = ((max_h - margin.y - AC_HEADER_H) / AC_ROW_H).floor().max(1.0) * AC_ROW_H;
            ScrollArea::vertical().max_height(rows_h).show(ui, |ui| {
                for (i, row) in rows.iter().enumerate() {
                    let (rect, resp) = ui.allocate_exact_size(vec2(w, AC_ROW_H), Sense::click());
                    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, i == *selected, &row.label));
                    if resp.hovered() && moving {
                        *selected = i;
                    }
                    if resp.clicked() {
                        clicked = Some(i);
                    }
                    let on = i == *selected;
                    if on && reveal {
                        resp.scroll_to_me(None);
                    }
                    if ui.is_rect_visible(rect) {
                        let fill = if on { hot } else { pal.tk.raised };
                        ui.painter().rect_filled(rect, pal.radius, fill);
                        ac_row(ui, cx, pal, guild_id, row, rect, fill);
                    }
                }
            });
        });
    });
    clicked
}

/// One row: 24 px avatar with status (members), glyph (channels, roles) or 20 px emoji, the
/// label (role colored) and the muted detail on the right.
fn ac_row(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, guild_id: Option<Id>, row: &Row, rect: Rect, fill: Color32) {
    let icon = Rect::from_center_size(pos2(rect.left() + 20.0, rect.center().y), Vec2::splat(24.0));
    match &row.icon {
        autocomplete::Icon::User(u) => {
            let (user, member) = &**u;
            let url = images::user_avatar_url(guild_id, user, member.as_ref(), 48, false);
            sidebar::user_avatar_at(ui, cx.images, &url, icon, false, 1.0, (user.id, &row.label));
            sidebar::user_dot(ui.painter(), icon, cx.state, user.id, &cx.theme.colors, fill);
        }
        autocomplete::Icon::Vector(v) => super::icons::paint_channel(ui.painter(), *v, Rect::from_center_size(icon.center(), Vec2::splat(20.0)), pal.muted, fill),
        autocomplete::Icon::Emoji(e) => {
            ui.painter().text(icon.center(), Align2::CENTER_CENTER, e, FontId::proportional(20.0), pal.text);
        }
        autocomplete::Icon::Custom(e) => {
            if let Some(t) = cx.images.get(ui.ctx(), &cdn::emoji_url(e.id, false, 48), 48, false) {
                Image::from_texture(t).paint_at(ui, Rect::from_center_size(icon.center(), Vec2::splat(20.0)));
            }
        }
    }
    let x = icon.right() + 12.0;
    let detail = (!row.detail.is_empty()).then(|| one_line(ui, row.detail.clone(), FontId::proportional(pal.meta()), pal.muted, rect.width() * 0.4));
    let detail_w = detail.as_ref().map_or(0.0, |g| g.size().x + 16.0);
    let color = row.color.map_or(pal.text, |c| crate::theme::readable_on(c, pal.tk.raised.blend(fill)));
    let label = one_line(ui, row.label.clone(), FontId::proportional(pal.size), color, (rect.right() - 12.0 - detail_w - x).max(24.0));
    ui.painter().galley(pos2(x, rect.center().y - label.size().y / 2.0), label, color);
    if let Some(g) = detail {
        ui.painter().galley(pos2(rect.right() - 12.0 - g.size().x, rect.center().y - g.size().y / 2.0), g, pal.muted);
    }
}

/// `@query` beyond the local cache: op 8 once per new query (the gateway debounces).
fn search_members(ctx: &egui::Context, cx: &mut UiCx<'_>, guild_id: Id, query: String) {
    let id = trays_id().with("member_search");
    let key = (guild_id, query);
    if key.1.trim().is_empty() || ctx.data(|d| d.get_temp::<(Id, String)>(id)).as_ref() == Some(&key) {
        return;
    }
    if cx.state.need_member_search(guild_id, &key.1, autocomplete::SEARCH_LIMIT) {
        cx.cmd.send(Command::Chat(ChatCommand::SearchMembers { guild_id, query: key.1.clone(), limit: autocomplete::SEARCH_LIMIT }));
    }
    ctx.data_mut(|d| d.insert_temp(id, key));
}

// ---------------------------------------------------------------------------
// Attachments
// ---------------------------------------------------------------------------

/// New files for the tray: finished background jobs, files dropped on the window (with the
/// "Upload to #channel" overlay while hovering) and images pasted while the input (or nothing)
/// has focus. `edit_id` None: the other box (thread panel / chat) takes drops and pastes now.
fn collect_files(ctx: &egui::Context, cx: &mut UiCx<'_>, trays: &mut HashMap<Id, Tray>, ch: Id, edit_id: Option<egui::Id>, can_attach: bool, pal: &Pal) {
    let now = ctx.input(|i| i.time);
    drain_prepared(trays, now);
    let Some(edit_id) = edit_id else { return };
    let (hovering, dropped, pasted) = ctx.input(|i| {
        let pasted: Vec<_> = i.events.iter().filter_map(|e| if let egui::Event::PasteImage(img) = e { Some(img.clone()) } else { None }).collect();
        (!i.raw.hovered_files.is_empty(), i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect::<Vec<_>>(), pasted)
    });
    let paste_here = ctx.memory(|m| m.focused().is_none_or(|f| f == edit_id));
    let mut jobs: Vec<Job> = dropped.into_iter().map(Job::Path).collect();
    if paste_here {
        jobs.extend(pasted.into_iter().map(Job::Pasted));
    }
    if (hovering || demo("drop")) && can_attach {
        drop_overlay(ctx, pal, &format!("Upload to {}{}", channel_prefix(cx.state.channels.get(&ch).map_or(0, |c| c.kind)), cx.state.channel_name(ch)));
    }
    if jobs.is_empty() {
        return;
    }
    let tray = trays.entry(ch).or_default();
    if !can_attach {
        tray.error = Some(("You can't upload files here".into(), now + ERROR_S));
        return;
    }
    add_jobs(ctx, tray, ch, jobs, cx.state.upload_limit(ch));
}

/// Files for a form outside the box (forum.rs New Post), kept under `key` like a channel's
/// tray: the cards (preview, spoiler, alt text, remove), an inline error and "Upload a File".
pub(super) fn form_files(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, key: Id) {
    let ctx = ui.ctx().clone();
    let now = ctx.input(|i| i.time);
    let mut trays: HashMap<Id, Tray> = ctx.data_mut(|d| d.remove_temp(trays_id())).unwrap_or_default();
    drain_prepared(&mut trays, now);
    let mut tray = trays.remove(&key).unwrap_or_default();
    if tray.error.as_ref().is_some_and(|(_, until)| *until <= now) {
        tray.error = None;
    }
    if let Some((err, until)) = &tray.error {
        ui.label(RichText::new(err).size(pal.meta()).color(pal.danger));
        ctx.request_repaint_after(Duration::from_secs_f64(until - now));
    }
    if !tray.files.is_empty() || tray.loading > 0 {
        tray_row(ui, pal, &mut tray, key, SMALL_CARD_H);
    }
    if small_button(ui, pal, "📎 Upload a File", pal.text).clicked()
        && let Some(paths) = rfd::FileDialog::new().set_title("Upload a file").pick_files()
    {
        let limit = cx.state.upload_limit(key);
        add_jobs(&ctx, &mut tray, key, paths.into_iter().map(Job::Path).collect(), limit);
    }
    trays.insert(key, tray);
    ctx.data_mut(|d| d.insert_temp(trays_id(), trays));
}

/// The files `form_files` holds for `key` (cheap handles) and whether some are still loading.
pub(super) fn form_file_list(ctx: &egui::Context, key: Id) -> (Vec<UploadFile>, bool) {
    ctx.data(|d| d.get_temp::<HashMap<Id, Tray>>(trays_id()))
        .and_then(|mut t| t.remove(&key))
        .map_or((Vec::new(), false), |t| (t.files.into_iter().map(|f| f.file).collect(), t.loading > 0))
}

/// Drops the files (and previews) `form_files` holds for `key`.
pub(super) fn clear_form_files(ctx: &egui::Context, key: Id) {
    ctx.data_mut(|d| {
        if let Some(t) = d.get_temp_mut_or_default::<HashMap<Id, Tray>>(trays_id()).get_mut(&key) {
            (t.files, t.error) = (Vec::new(), None);
        }
    });
}

/// Moves files prepared off the UI thread into their trays.
fn drain_prepared(trays: &mut HashMap<Id, Tray>, now: f64) {
    let done = std::mem::take(&mut *INBOX.lock().unwrap_or_else(|e| e.into_inner()));
    for (c, file) in done {
        let t = trays.entry(c).or_default();
        t.loading = t.loading.saturating_sub(1);
        match file {
            Ok(f) => t.files.push(f),
            Err(e) => t.error = Some((e, now + ERROR_S)),
        }
    }
}

/// Starts preparing up to the free slots of `tray`; the rest is refused with an error.
fn add_jobs(ctx: &egui::Context, tray: &mut Tray, ch: Id, jobs: Vec<Job>, limit: u64) {
    let room = MAX_FILES.saturating_sub(tray.files.len() + tray.loading);
    if jobs.len() > room {
        tray.error = Some((format!("Up to {MAX_FILES} files per message"), ctx.input(|i| i.time) + ERROR_S));
    }
    for job in jobs.into_iter().take(room) {
        tray.loading += 1;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            // One preparation at a time: parallel full decodes would spike RAM far past the budget.
            static PREP: Mutex<()> = Mutex::new(());
            let _one = PREP.lock().unwrap_or_else(|e| e.into_inner());
            let file = match job {
                Job::Path(p) => from_path(&ctx, p, limit),
                Job::Pasted(img) => from_paste(&ctx, &img, limit),
            };
            INBOX.lock().unwrap_or_else(|e| e.into_inner()).push((ch, file));
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        });
    }
}

fn check_size(name: &str, size: u64, limit: u64) -> Result<(), String> {
    if size > limit { Err(format!("{name} is over the {} limit", human_size(limit))) } else { Ok(()) }
}

/// A file on disk: size check and, for images, a thumbnail (off the UI thread).
fn from_path(ctx: &egui::Context, path: PathBuf, limit: u64) -> Result<PendingFile, String> {
    let name = path.file_name().map_or_else(|| "file".to_owned(), |n| n.to_string_lossy().into_owned());
    let meta = std::fs::metadata(&path).map_err(|e| format!("{name}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{name} is not a file"));
    }
    check_size(&name, meta.len(), limit)?;
    let thumb = image::ImageReader::open(&path)
        .ok()
        .and_then(|r| r.with_guessed_format().ok())
        .and_then(|mut r| {
            let mut limits = image::Limits::default();
            limits.max_alloc = Some(THUMB_MAX_ALLOC);
            r.limits(limits);
            r.decode().ok()
        })
        .map(|img| thumbnail(ctx, &name, &img));
    let file = UploadFile { source: UploadSource::Path(path), filename: name, spoiler: false, description: None };
    Ok(PendingFile { file, size: meta.len(), thumb })
}

/// A pasted clipboard image as `image.png` (encoded in memory, off the UI thread).
fn from_paste(ctx: &egui::Context, img: &ColorImage, limit: u64) -> Result<PendingFile, String> {
    let name = "image.png";
    let [w, h] = img.size;
    // Above 8K: refused before the RGBA copy and PNG encode below (transient RAM).
    if w * h > 7680 * 4320 {
        return Err("The pasted image is too large".into());
    }
    let rgba = img.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
    let buf = image::RgbaImage::from_raw(w as u32, h as u32, rgba).ok_or("The pasted image is unreadable")?;
    let mut png = std::io::Cursor::new(Vec::new());
    buf.write_to(&mut png, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    let png = png.into_inner();
    let size = png.len() as u64;
    check_size(name, size, limit)?;
    let thumb = thumbnail(ctx, name, &image::DynamicImage::ImageRgba8(buf));
    let file = UploadFile { source: UploadSource::Bytes(png.into()), filename: name.into(), spoiler: false, description: None };
    Ok(PendingFile { file, size, thumb: Some(thumb) })
}

fn thumbnail(ctx: &egui::Context, name: &str, img: &image::DynamicImage) -> TextureHandle {
    // `thumbnail` also scales up; small images stay as they are.
    let t = if img.width().max(img.height()) > THUMB_PX { img.thumbnail(THUMB_PX, THUMB_PX).to_rgba8() } else { img.to_rgba8() };
    let image = ColorImage::from_rgba_unmultiplied([t.width() as usize, t.height() as usize], t.as_raw());
    ctx.load_texture(format!("rc_tray/{name}"), image, TextureOptions::LINEAR)
}

/// "1.5 MB", "20 MB", "340 KB", "12 B" (binary units, as Discord's limits).
pub(super) fn human_size(bytes: u64) -> String {
    let (mut v, mut unit) = (bytes as f64, "B");
    for u in ["KB", "MB", "GB"] {
        if v < 1024.0 {
            break;
        }
        (v, unit) = (v / 1024.0, u);
    }
    if unit == "B" {
        return format!("{bytes} B");
    }
    let s = if v < 10.0 { format!("{v:.1}") } else { format!("{v:.0}") };
    format!("{} {unit}", s.trim_end_matches(".0"))
}

/// Full-window drop target while files are dragged over it.
fn drop_overlay(ctx: &egui::Context, pal: &Pal, title: &str) {
    let screen = ctx.content_rect();
    egui::Area::new(trays_id().with("drop")).order(Order::Foreground).fixed_pos(screen.min).interactable(false).show(ctx, |ui| {
        super::motion::own(ctx, ui.layer_id());
        let p = ui.painter();
        p.rect_filled(screen, 0, Color32::from_black_alpha(170));
        let card = Rect::from_center_size(screen.center(), vec2(320.0, 168.0));
        // A raised card (E3) with the drop zone outlined inside it.
        let fill = pal.tk.raised;
        p.rect(card, pal.modal_radius(), fill, Stroke::new(1.0, pal.tk.rule), egui::StrokeKind::Inside);
        p.rect_stroke(card.shrink(8.0), pal.card_radius().saturating_sub(4), Stroke::new(2.0, pal.tk.edge), egui::StrokeKind::Inside);
        super::icons::paint(p, super::icons::Icon::File, Rect::from_center_size(card.center() - vec2(0.0, 22.0), Vec2::splat(48.0)), pal.muted, fill);
        p.text(card.center() + vec2(0.0, 30.0), Align2::CENTER_CENTER, title, FontId::new(pal.title(), crate::theme::semibold_family()), pal.text);
    });
}

/// The tray: one card per file (preview or file glyph, name, size; spoiler, alt text and
/// remove on its corner), placeholders for files still being prepared. `card_h`: `CARD.y`, or
/// `SMALL_CARD_H` in short windows.
fn tray_row(ui: &mut egui::Ui, pal: &Pal, tray: &mut Tray, ch: Id, card_h: f32) {
    let size = vec2(CARD.x, card_h);
    let top = if card_h < CARD.y { 12 } else { 16 };
    let rect = Frame::new()
        .inner_margin(Margin { left: 16, right: 16, top, bottom: 12 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ScrollArea::horizontal().id_salt(("rc_tray", ch.0)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 16.0;
                    let mut remove = None;
                    for (i, f) in tray.files.iter_mut().enumerate() {
                        if file_card(ui, pal, f, i, size) {
                            remove = Some(i);
                        }
                    }
                    for _ in 0..tray.loading {
                        let (r, _) = ui.allocate_exact_size(size, Sense::hover());
                        ui.painter().rect_filled(r, pal.radius, pal.bg);
                        ui.painter().text(r.center(), Align2::CENTER_CENTER, "…", FontId::proportional(20.0), pal.muted);
                    }
                    if let Some(i) = remove {
                        tray.files.remove(i);
                        ui.ctx().request_repaint();
                    }
                });
            });
        })
        .response
        .rect;
    ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, pal.tk.rule));
}

/// One tray card of `size`; true when its remove button was clicked.
fn file_card(ui: &mut egui::Ui, pal: &Pal, f: &mut PendingFile, i: usize, size: Vec2) -> bool {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    if !ui.is_rect_visible(rect) {
        return false;
    }
    let p = ui.painter().clone();
    p.rect_filled(rect, pal.radius, pal.bg);
    // Name and size take the card's bottom 56 px.
    let preview = Rect::from_min_size(rect.min + vec2(8.0, 8.0), vec2(rect.width() - 16.0, rect.height() - 56.0));
    match &f.thumb {
        Some(t) => {
            let fit = (preview.size() / t.size_vec2()).min_elem().min(1.0) * t.size_vec2();
            let r = Rect::from_center_size(preview.center(), fit);
            Image::from_texture(t).corner_radius(4).paint_at(ui, r);
            if f.file.spoiler {
                p.rect_filled(r, 4, Color32::from_black_alpha(215));
            }
        }
        None => {
            super::icons::paint(&p, super::icons::Icon::File, Rect::from_center_size(preview.center(), Vec2::splat(preview.height().min(48.0))), pal.muted, Color32::TRANSPARENT);
        }
    }
    if f.file.spoiler {
        let g = p.layout_no_wrap("Spoiler".into(), FontId::new(pal.meta(), crate::theme::semibold_family()), Color32::WHITE);
        let pill = Rect::from_center_size(preview.center(), g.size() + vec2(16.0, 8.0));
        p.rect_filled(pill, pill.height() / 2.0, Color32::from_black_alpha(160));
        p.galley(pill.center() - g.size() / 2.0, g, Color32::WHITE);
    }
    let x = rect.left() + 8.0;
    let w = rect.width() - 16.0;
    let name = one_line(ui, f.file.filename.clone(), FontId::new(pal.meta(), bold_family()), pal.text, w);
    let name_y = preview.bottom() + 6.0;
    p.galley(pos2(x, name_y), name.clone(), pal.text);
    p.text(pos2(x, name_y + name.size().y + 2.0), Align2::LEFT_TOP, human_size(f.size), FontId::proportional(pal.meta() - 1.0), pal.muted);

    // Spoiler | alt text | remove, in a small bar over the preview's top-right corner.
    let bar = Rect::from_min_size(pos2(rect.right() - 4.0 - 3.0 * 26.0, rect.top() + 4.0), vec2(3.0 * 26.0, 26.0));
    p.rect_filled(bar, pal.radius, pal.surface);
    p.rect_stroke(bar, pal.radius, Stroke::new(1.0, pal.tk.rule), egui::StrokeKind::Inside);
    let mut child = ui.new_child(UiBuilder::new().id_salt(("rc_tray_card", i)).max_rect(bar.shrink(1.0)).layout(Layout::left_to_right(Align::Center)));
    child.spacing_mut().item_spacing.x = 0.0;
    if icon_button(&mut child, pal, "👁", "Mark as spoiler", 24.0, f.file.spoiler, pal.text).clicked() {
        f.file.spoiler = !f.file.spoiler;
    }
    let alt = icon_button(&mut child, pal, "✏", "Alt text", 24.0, f.file.description.is_some(), pal.text);
    egui::Popup::from_toggle_button_response(&alt).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.set_width(260.0);
        ui.label(RichText::new("Alt text").size(pal.meta()).family(bold_family()).color(pal.muted));
        let mut text = f.file.description.take().unwrap_or_default();
        let edit = ui.add(TextEdit::multiline(&mut text).char_limit(1024).desired_rows(2).desired_width(f32::INFINITY).hint_text("Describe this file"));
        if alt.clicked() {
            edit.request_focus();
        }
        f.file.description = (!text.trim().is_empty()).then_some(text);
    });
    // Remove is the danger action: red, not only on hover.
    let remove = icon_button(&mut child, pal, "🗑", "Remove", 24.0, false, pal.danger);
    remove.clicked()
}

// ---------------------------------------------------------------------------
// Typing line
// ---------------------------------------------------------------------------

/// "A is typing…" with three pulsing dots under the box, and while slowmode / a send cooldown
/// holds (`wait_ms`) a clock with the m:ss countdown on the right; the space is always
/// reserved, like Discord's. Repaints only while someone types (dot steps, then the prune
/// deadline) or the countdown runs.
fn typing_line(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, ch: Id, wait_ms: u64) {
    let now = Instant::now();
    if !cx.state.chat.typing.is_empty()
        && let Some(next) = cx.state.prune_typing(now)
    {
        ui.ctx().request_repaint_after(next);
    }
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), TYPING_H), Sense::hover());
    let size = pal.meta() - 0.5;
    if wait_ms > 0 {
        let s = wait_ms.div_ceil(1000);
        let g = ui.painter().layout_no_wrap(format!("{}:{:02}", s / 60, s % 60), FontId::proportional(size), pal.muted);
        let text_pos = pos2(rect.right() - 8.0 - g.size().x, rect.center().y - g.size().y / 2.0);
        let clock = pos2(text_pos.x - 10.0, rect.center().y);
        super::icons::paint(ui.painter(), super::icons::Icon::Clock, Rect::from_center_size(clock, Vec2::splat(14.0)), pal.muted, Color32::TRANSPARENT);
        let hit = Rect::from_min_max(pos2(clock.x - 7.0, rect.top()), pos2(rect.right() - 8.0, rect.bottom()));
        ui.painter().galley(text_pos, g, pal.muted);
        ui.interact(hit, ui.make_persistent_id("rc_slowmode"), Sense::hover()).on_hover_text(format!("You can send again in {s} s"));
        ui.ctx().request_repaint_after(Duration::from_millis(wait_ms.min(1000)));
    }
    let Some(text) = cx.state.typing_text(ch, now) else { return };
    let names = cx.state.typing_users(ch, now);
    let p = ui.painter();
    // Reduced motion: the dots stand still at one alpha and nothing repaints for them.
    let still = super::motion::secs(ui.ctx(), 1.0) == 0.0;
    let t = ui.input(|i| i.time) * 3.0;
    let step = t as usize % 3;
    for i in 0..3 {
        let alpha = if still { 0.6 } else if i == step { 1.0 } else { 0.35 };
        p.circle_filled(pos2(rect.left() + 12.0 + i as f32 * 6.0, rect.center().y), 2.2, pal.text.gamma_multiply(alpha));
    }
    if !still {
        ui.ctx().request_repaint_after(Duration::from_secs_f64((1.0 - t.fract()) / 3.0));
    }
    let mut job = typing_job(&text, &names, FontId::proportional(size), FontId::new(size, bold_family()), pal.muted, pal.text);
    // TypingTweaks: avatars before the text, role colours on the names.
    crate::plugins::chat::tint_typing_names(cx.state, cx.theme, ch, &mut job, pal.bg);
    let x = rect.left() + 32.0 + crate::plugins::chat::typing_avatars(ui, cx, ch, pos2(rect.left() + 32.0, rect.center().y));
    let g = ui.painter().layout_job(job);
    ui.painter().galley(pos2(x, rect.center().y - g.size().y / 2.0), g, pal.muted);
}

/// The typing text with the names in bold.
fn typing_job(text: &str, names: &[String], font: FontId, bold: FontId, color: Color32, strong: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let mut rest = text;
    for name in names {
        let Some(i) = rest.find(name.as_str()) else { break };
        job.append(&rest[..i], 0.0, TextFormat::simple(font.clone(), color));
        job.append(name, 0.0, TextFormat::simple(bold.clone(), strong));
        rest = &rest[i + name.len()..];
    }
    job.append(rest, 0.0, TextFormat::simple(font, color));
    job
}

// ---------------------------------------------------------------------------
// Sending
// ---------------------------------------------------------------------------

/// Milliseconds until slowmode lets me send in `ch` again (0 = now). While one of my sends
/// there is still in flight its time is unknown, so that blocks too (1 s re-check).
fn slowmode_wait(cx: &UiCx<'_>, ch: Id) -> u64 {
    let rate = cx.state.channels.get(&ch).map_or(0, |c| c.rate_limit_per_user);
    let exempt = perm::MANAGE_MESSAGES | perm::MANAGE_CHANNELS | perm::BYPASS_SLOWMODE;
    if rate == 0 || cx.state.channel_permissions(ch) & exempt != 0 {
        return 0;
    }
    if cx.state.pending.iter().any(|p| p.channel_id == ch && p.failed.is_none()) {
        return 1000;
    }
    let me = cx.state.me.as_ref().map(|u| u.id);
    let last_mine = cx
        .state
        .messages
        .get(&ch)
        .and_then(|c| c.msgs.iter().rev().find(|m| Some(m.author.id) == me))
        .map(|m| m.id.timestamp_ms());
    slowmode_left_ms(rate, last_mine, unix_ms())
}

/// The server's send cooldown from a typing answer (`ChatEvent::SendCooldown`).
fn cooldown_ms(cx: &UiCx<'_>, ch: Id, now: Instant) -> u64 {
    cx.state.chat.send_cooldowns.get(&ch).map_or(0, |until| until.saturating_duration_since(now).as_millis() as u64)
}

fn slowmode_left_ms(rate_s: u32, last_mine_ms: Option<u64>, now_ms: u64) -> u64 {
    last_mine_ms.map_or(0, |t| (t + u64::from(rate_s) * 1000).saturating_sub(now_ms))
}

#[allow(clippy::too_many_arguments)]
fn reply_bar(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, ch: Id, guild_id: Option<Id>, mid: Id, ping: bool) {
    let state = &*cx.state;
    let name = state
        .messages
        .get(&ch)
        .and_then(|c| c.msgs.iter().find(|m| m.id == mid))
        .map(|m| state.member_display_name(guild_id, &m.author, message_member(state, guild_id, m).as_ref()))
        .unwrap_or_else(|| "a message".to_string());
    let rect = Frame::new()
        .inner_margin(Margin { left: 16, right: 8, top: 4, bottom: 4 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(RichText::new("Replying to").size(pal.meta()).color(pal.muted));
                ui.label(RichText::new(name).size(pal.meta()).family(bold_family()).color(pal.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icon_button(ui, pal, "×", "Cancel reply (Esc)", 24.0, false, pal.text).clicked() {
                        cx.view.reply_to = None;
                        ui.ctx().request_repaint();
                    }
                    let tip = if ping { "Mention on: the author is notified" } else { "Mention off: the author is not notified" };
                    if icon_button(ui, pal, "@", tip, 24.0, ping, pal.text).clicked()
                        && let Some(reply) = &mut cx.view.reply_to
                    {
                        reply.2 = !reply.2;
                        ui.ctx().request_repaint();
                    }
                });
            });
        })
        .response
        .rect;
    ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, egui::Stroke::new(1.0, pal.tk.rule));
}

/// Sends `content` (and `files` through the upload pipeline) as a reply if one is set.
fn send(cx: &mut UiCx<'_>, ch: Id, guild_id: Option<Id>, content: String, files: Vec<UploadFile>) {
    let nonce = next_nonce(unix_ms()).to_string();
    // Plugins: ClearURLs, SilentMessageToggle.
    let content = crate::plugins::chat::outgoing(content);
    crate::plugins::chat::on_send(&nonce);
    let reply = cx
        .view
        .reply_to
        .take_if(|r| r.0 == ch)
        .map(|(_, message_id, mention_author)| ReplyTo { message_id, guild_id, mention_author });
    cx.state.pending.push(PendingMessage { nonce: nonce.clone(), channel_id: ch, content: content.clone(), failed: None, reply: reply.clone() });
    if files.is_empty() {
        cx.cmd.send(Command::SendMessage { channel_id: ch, content, nonce, reply });
    } else {
        cx.state.chat.uploads.insert(nonce.clone(), Upload { files: files.clone(), ..Default::default() });
        cx.cmd.send(Command::Chat(ChatCommand::SendWithFiles { channel_id: ch, content, nonce, reply, files }));
    }
    cx.view.scroll_to_bottom = true;
}

/// Start editing my most recent editable message in the channel.
fn edit_last(cx: &mut UiCx<'_>, ch: Id) {
    let me = cx.state.me.as_ref().map(|u| u.id);
    let last = cx
        .state
        .messages
        .get(&ch)
        .and_then(|c| c.msgs.iter().rev().find(|m| Some(m.author.id) == me && matches!(m.kind, 0 | 19)));
    if let Some(m) = last {
        cx.view.edit_text = m.content.clone();
        cx.view.editing = Some((ch, m.id));
    }
}

/// Trimmed content, or None if over the length limit `max` or empty without files.
fn prepare_send(draft: &str, with_files: bool, max: usize) -> Option<String> {
    let t = draft.trim();
    ((with_files || !t.is_empty()) && t.chars().count() <= max).then(|| t.to_string())
}

fn snowflake_from_unix_ms(ms: u64) -> u64 {
    ms.saturating_sub(Id::DISCORD_EPOCH_MS) << 22
}

/// Current-time snowflake used as nonce, strictly increasing so two sends never share one
/// (Discord drops a repeated nonce).
fn next_nonce(unix_ms: u64) -> u64 {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let n = snowflake_from_unix_ms(unix_ms).max(LAST.load(Ordering::Relaxed) + 1);
    LAST.store(n, Ordering::Relaxed);
    n
}

fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// `RUSTCORD_MSG_DEMO` composer states (module doc), set once.
fn demo_setup(ctx: &egui::Context, cx: &mut UiCx<'_>, ch: Id) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    if ONCE.is_completed() || std::env::var_os("RUSTCORD_MSG_DEMO").is_none() {
        return;
    }
    ONCE.call_once(|| {
        let now = ctx.input(|i| i.time);
        let file = |name: &str, spoiler, description: Option<&str>| UploadFile {
            source: UploadSource::Bytes(Arc::from([])),
            filename: name.into(),
            spoiler,
            description: description.map(Into::into),
        };
        if demo("attach") {
            let gradient = |seed: u8| {
                let px = (0..150u8).flat_map(|y| (0..240u8).map(move |x| Color32::from_rgb(x.wrapping_add(seed), y.wrapping_mul(2), 160u8.wrapping_sub(seed))));
                let img = ColorImage::new([240, 150], px.collect());
                ctx.load_texture("rc_tray/demo", img, TextureOptions::LINEAR)
            };
            let files = vec![
                PendingFile { file: file("sunset.png", false, Some("A sunset")), size: 1_572_864, thumb: Some(gradient(0)) },
                PendingFile { file: file("notes.pdf", false, None), size: 348_160, thumb: None },
                PendingFile { file: file("plot-twist.png", true, None), size: 92_000, thumb: Some(gradient(90)) },
            ];
            let error = Some((format!("trailer.mp4 is over the {} limit", human_size(cx.state.upload_limit(ch))), now + 600.0));
            let tray = Tray { files, loading: 0, error };
            ctx.data_mut(|d| d.get_temp_mut_or_default::<HashMap<Id, Tray>>(trays_id()).insert(ch, tray));
        }
        if demo("upload") {
            let nonce = "demo-upload".to_owned();
            let content = "Photos from the meetup".to_owned();
            cx.state.pending.push(PendingMessage { nonce: nonce.clone(), channel_id: ch, content, failed: None, reply: None });
            let files = vec![file("holiday.jpg", false, None), file("clip.gif", false, None)];
            cx.state.chat.uploads.insert(nonce, Upload { files, sent: 2 << 20, total: 5 << 20 });
        }
        if demo("typing") {
            let me = cx.state.me.as_ref().map(|u| u.id);
            let mut users: Vec<Id> = cx.state.users.keys().copied().filter(|u| Some(*u) != me).collect();
            users.sort();
            let until = Instant::now() + Duration::from_secs(60);
            let typing = cx.state.chat.typing.entry(ch).or_default();
            for u in users.into_iter().take(2) {
                typing.insert(u, Typist { until, nick: None });
            }
        }
        if demo("gif") {
            pickers::open(ctx, Tab::Gif);
            let categories = ["happy", "thumbs up", "dance", "facepalm", "hug", "wow"].map(|c| GifItem { category: Some(c.into()), ..Default::default() });
            let gifs = [(498, 280), (480, 480), (320, 240), (400, 520), (498, 374), (360, 202), (300, 300), (480, 270)]
                .map(|(width, height)| GifItem { width, height, ..Default::default() });
            let items = categories.into_iter().chain(gifs).collect();
            cx.state.chat.gifs = Some(GifView { query: None, result: Some(Ok(items)) });
        }
        for (flag, draft) in [("ac", "@"), ("ac-channel", "see #"), ("ac-emoji", "nice :sm")] {
            if demo(flag) {
                cx.view.drafts.insert(ch, draft.into());
            }
        }
        if demo("cooldown") {
            cx.state.chat.send_cooldowns.insert(ch, Instant::now() + Duration::from_secs(42));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slowmode_counts_down_from_my_last_message() {
        assert_eq!(slowmode_left_ms(10, None, 5_000), 0);
        assert_eq!(slowmode_left_ms(10, Some(1_000), 5_000), 6_000);
        assert_eq!(slowmode_left_ms(10, Some(1_000), 11_000), 0);
        assert_eq!(slowmode_left_ms(10, Some(1_000), 99_000), 0);
    }

    #[test]
    fn send_text_is_trimmed_and_limited() {
        const MAX_LEN: usize = 2000;
        assert_eq!(prepare_send("  hi\n", false, MAX_LEN), Some("hi".to_string()));
        assert_eq!(prepare_send(" \n\t", false, MAX_LEN), None);
        assert_eq!(prepare_send(" \n\t", true, MAX_LEN), Some(String::new()), "files may go without text");
        assert_eq!(prepare_send(&"ä".repeat(MAX_LEN), false, MAX_LEN).map(|s| s.chars().count()), Some(MAX_LEN));
        assert_eq!(prepare_send(&"a".repeat(MAX_LEN + 1), true, MAX_LEN), None);
        // Nitro: 4000.
        assert!(prepare_send(&"a".repeat(MAX_LEN + 1), true, 4000).is_some());
    }

    #[test]
    fn nonce_is_a_time_snowflake() {
        let ms = 1_768_478_400_000;
        let n = snowflake_from_unix_ms(ms);
        assert_eq!(Id(n).timestamp_ms(), ms);
        let a = next_nonce(ms);
        let b = next_nonce(ms);
        assert!(a >= n && b > a);
    }

    #[test]
    fn sizes_read_like_discord() {
        assert_eq!(human_size(12), "12 B");
        assert_eq!(human_size(2048), "2 KB");
        assert_eq!(human_size(348_160), "340 KB");
        assert_eq!(human_size(1_572_864), "1.5 MB");
        assert_eq!(human_size(20 << 20), "20 MB");
        assert_eq!(check_size("a.mp4", 21 << 20, 20 << 20), Err("a.mp4 is over the 20 MB limit".into()));
        assert!(check_size("a.png", 20 << 20, 20 << 20).is_ok());
    }

    #[test]
    fn typing_names_are_bold() {
        let f = FontId::proportional(12.0);
        let job = typing_job("Ann and Bo are typing…", &["Ann".into(), "Bo".into()], f.clone(), f, Color32::GRAY, Color32::WHITE);
        let parts: Vec<_> = job.sections.iter().map(|s| (&job.text[s.byte_range.start.0..s.byte_range.end.0], s.format.color == Color32::WHITE)).collect();
        assert_eq!(parts, [("", false), ("Ann", true), (" and ", false), ("Bo", true), (" are typing…", false)]);
    }

    #[test]
    fn insert_goes_to_the_cursor() {
        let ctx = egui::Context::default();
        let id = egui::Id::unique("t");
        let mut s = "héllo".to_string();
        insert_at_cursor(&ctx, id, &mut s, "👍");
        assert_eq!(s, "héllo👍", "no cursor yet: at the end");
        let mut st = egui::text_edit::TextEditState::load(&ctx, id).unwrap_or_default();
        st.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(2))));
        st.store(&ctx, id);
        insert_at_cursor(&ctx, id, &mut s, "x");
        assert_eq!(s, "héxllo👍");
    }

    #[test]
    fn pasted_and_picked_files_get_size_and_thumbnail() {
        let ctx = egui::Context::default();
        let img = ColorImage::new([40, 20], vec![Color32::RED; 800]);
        let f = from_paste(&ctx, &img, 1 << 20).unwrap();
        let UploadSource::Bytes(png) = &f.file.source else { panic!("pasted images are in memory") };
        assert!(png.starts_with(b"\x89PNG") && f.size == png.len() as u64 && f.file.filename == "image.png");
        assert_eq!(f.thumb.as_ref().map(|t| t.size()), Some([40, 20]), "small images keep their size");
        assert!(from_paste(&ctx, &img, 10).is_err(), "over the limit");

        let dir = std::env::temp_dir().join(format!("rc_composer_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (png_path, txt_path) = (dir.join("red.png"), dir.join("notes.txt"));
        std::fs::write(&png_path, png).unwrap();
        std::fs::write(&txt_path, "hello").unwrap();
        let p = from_path(&ctx, png_path, 1 << 20).unwrap();
        assert!(p.thumb.is_some() && p.size == png.len() as u64 && p.file.filename == "red.png");
        let t = from_path(&ctx, txt_path, 1 << 20).unwrap();
        assert!(t.thumb.is_none() && t.size == 5);
        assert!(from_path(&ctx, dir.clone(), 1 << 20).is_err(), "folders are refused");
        let _ = std::fs::remove_dir_all(dir);
    }
}
