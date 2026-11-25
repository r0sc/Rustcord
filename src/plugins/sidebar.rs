//! Channel list and server bar: ShowHiddenChannels (locked rows, display only), TypingIndicator
//! (dots on channel / DM rows), ReadAllNotificationsButton, BetterFolders (folder behaviour)
//! and CallTimer (voice panel).

use super::{Plugin, enabled, opt_bool, opt_choice};
use crate::backend::{Command, SocialCommand};
use crate::model::{Id, channel_type as ct, perm};
use crate::state::AppState;
use crate::ui::UiCx;
use egui::{Align2, FontId, Pos2, Rect, Sense, pos2, vec2};
use std::sync::Mutex;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// ShowHiddenChannels
// ---------------------------------------------------------------------------

/// `sidebar_channels` lists channels I can't view too (state.rs hook).
pub fn show_hidden() -> bool {
    enabled(Plugin::ShowHiddenChannels)
}

/// A guild channel I can't view (or, voice, can't connect to).
pub fn is_hidden(state: &AppState, id: Id) -> bool {
    let Some(c) = state.channels.get(&id).filter(|c| c.guild_id.is_some()) else { return false };
    let p = state.channel_permissions(id);
    p & perm::VIEW_CHANNEL == 0 || (matches!(c.kind, ct::GUILD_VOICE | ct::GUILD_STAGE_VOICE) && p & (1 << 20) == 0)
}

/// Draws a hidden channel's row (`sidebar.rs` channel_row, `row_h` tall): lock, dimmed name, its
/// details on hover. Never selectable: no history, no member list, no voice for these. True when
/// the row was a hidden channel.
pub fn hidden_row(ui: &mut egui::Ui, cx: &UiCx<'_>, id: Id, row_h: f32) -> bool {
    if !show_hidden() || !is_hidden(cx.state, id) {
        return false;
    }
    let Some(ch) = cx.state.channels.get(&id) else { return false };
    let c = &cx.theme.colors;
    let size = cx.theme.font.size;
    let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
    let color = c.text_muted.to_color32().gamma_multiply(0.55);
    // Its kind's icon with a lock (docs/DESIGN.md §8.4).
    let icon = (crate::ui::icons::of(ch, cx.state).0, Some(crate::ui::icons::Badge::Lock));
    crate::ui::icons::paint_channel(ui.painter(), icon, Rect::from_center_size(pos2(row.left() + 26.0, row.center().y), egui::Vec2::splat(20.0)), color, c.sidebar.to_color32());
    let name = ch.name.clone().unwrap_or_default();
    let g = crate::ui::one_line(ui, name.as_str(), FontId::proportional(size), color, row.right() - row.left() - 52.0);
    ui.painter().galley(pos2(row.left() + 42.0, row.center().y - g.size().y / 2.0), g, color);
    resp.on_hover_ui(|ui| hidden_details(ui, cx.state, id));
    true
}

/// What the lock screen of Vencord shows, from cached data only.
fn hidden_details(ui: &mut egui::Ui, s: &AppState, id: Id) {
    let Some(ch) = s.channels.get(&id) else { return };
    ui.set_max_width(320.0);
    ui.label(egui::RichText::new(format!("{} is hidden from you", ch.name.as_deref().unwrap_or("This channel"))).family(crate::theme::bold_family()));
    if ch.nsfw {
        ui.label("Age-restricted");
    }
    if let Some(t) = ch.topic.as_deref().filter(|t| !t.is_empty()) {
        ui.label(t);
    }
    if let Some(m) = ch.last_message_id {
        ui.label(format!("Last message: {}", super::windows::snowflake_date(m)));
    }
    if ch.rate_limit_per_user > 0 {
        ui.label(format!("Slowmode: {}", super::people::short_duration(u64::from(ch.rate_limit_per_user))));
    }
    // Who may see it: roles and members allowed by an overwrite.
    let g = ch.guild_id.and_then(|g| s.guild(g));
    let allowed: Vec<String> = ch
        .permission_overwrites
        .iter()
        .filter(|o| o.allow.0 & perm::VIEW_CHANNEL != 0)
        .filter_map(|o| match o.kind {
            0 => g.and_then(|g| g.roles.iter().find(|r| r.id == o.id)).map(|r| format!("@{}", r.name.trim_start_matches('@'))),
            _ => s.users.get(&o.id).map(|u| u.display_name().to_owned()),
        })
        .take(12)
        .collect();
    if !allowed.is_empty() {
        ui.label(format!("Visible to: {}", allowed.join(", ")));
    }
}

// ---------------------------------------------------------------------------
// TypingIndicator
// ---------------------------------------------------------------------------

/// Typing dots right-aligned at `right_center` of a channel / DM row; returns the width taken.
pub fn typing_dots(ui: &egui::Ui, state: &AppState, id: Id, right_center: Pos2) -> f32 {
    if !enabled(Plugin::TypingIndicator)
        || (!opt_bool(Plugin::TypingIndicator, "includeCurrentChannel") && state.selected_channel == Some(id))
        || (!opt_bool(Plugin::TypingIndicator, "includeMutedChannels") && state.is_channel_muted(id))
    {
        return 0.0;
    }
    let Some(tip) = state.typing_text(id, Instant::now()) else { return 0.0 };
    let r = Rect::from_min_max(right_center - vec2(24.0, 8.0), right_center + vec2(0.0, 8.0));
    let t = ui.input(|i| i.time);
    let color = ui.visuals().text_color();
    for k in 0..3 {
        let phase = ((t * 3.0 - k as f64 * 0.4).sin() * 0.5 + 0.5) as f32;
        ui.painter().circle_filled(pos2(r.left() + 4.0 + 8.0 * k as f32, r.center().y), 2.5, color.gamma_multiply(0.4 + 0.6 * phase));
    }
    ui.interact(r, ui.make_persistent_id(("rc_typing_dots", id)), Sense::hover()).on_hover_text(tip);
    ui.ctx().request_repaint_after(Duration::from_millis(80));
    28.0
}

// ---------------------------------------------------------------------------
// ReadAllNotificationsButton
// ---------------------------------------------------------------------------

/// "Read All" under the home button: every server channel and joined thread read (no DMs).
pub fn read_all_button(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    if !enabled(Plugin::ReadAllNotificationsButton) {
        return;
    }
    let c = &cx.theme.colors;
    let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
    let pill = Rect::from_center_size(row.center(), vec2(48.0, 22.0));
    let hot = resp.hovered();
    // Hover is never purple: the row's `selected` fill and the full text colour.
    let (fill, text) = if hot { (cx.theme.tokens().selected, c.text.to_color32()) } else { (c.surface.to_color32(), c.text_muted.to_color32()) };
    ui.painter().rect_filled(pill, 11, fill);
    ui.painter().text(pill.center(), Align2::CENTER_CENTER, "Read All", FontId::new(10.5, crate::theme::bold_family()), text);
    let resp = resp.on_hover_text("Mark all servers read").on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.clicked() {
        let guilds: Vec<Id> = cx.state.guilds.iter().map(|g| g.id).collect();
        let acks: Vec<(Id, Id)> = guilds.into_iter().flat_map(|g| cx.state.mark_guild_read(g)).collect();
        for (ch, _) in &acks {
            cx.view.pending_acks.remove(ch);
        }
        if cx.settings.send_read_acks && !acks.is_empty() {
            cx.cmd.send(Command::Social(SocialCommand::MarkAllRead { acks }));
        }
    }
}

// ---------------------------------------------------------------------------
// BetterFolders
// ---------------------------------------------------------------------------

/// Same key as `sidebar.rs` folder_open_id (the folder's open state this session).
fn folder_key(folder: i64) -> egui::Id {
    egui::Id::unique(("rc_folder_open", folder))
}

/// A folder was clicked open: with "closeOthers" the other folders close.
pub fn folder_clicked(ctx: &egui::Context, state: &AppState, folder: i64, opened: bool) {
    if !enabled(Plugin::BetterFolders) || !opened || !opt_bool(Plugin::BetterFolders, "closeOthers") {
        return;
    }
    ctx.data_mut(|d| {
        for f in state.account.settings.folders.iter().filter_map(|f| f.id).filter(|&f| f != folder) {
            d.insert_temp(folder_key(f), false);
        }
    });
}

/// "forceOpen": the folder of the open server stays open.
pub fn force_open(open: bool, holds_selected: bool) -> bool {
    open || (holds_selected && enabled(Plugin::BetterFolders) && opt_bool(Plugin::BetterFolders, "forceOpen"))
}

// ---------------------------------------------------------------------------
// PinDMs
// ---------------------------------------------------------------------------

/// Pinned DM channels (plugins.toml `[PinDMs] pinned = ["<channel id>", …]`), newest pin last.
fn pinned() -> Vec<Id> {
    let Some(toml::Value::Array(list)) = super::opt_raw(Plugin::PinDms, "pinned") else { return Vec::new() };
    list.iter().filter_map(|v| v.as_str()?.parse().ok().map(Id)).collect()
}

/// The DM list (`sidebar.rs` dm_list): pinned DMs on top, both groups in their own order.
/// Returns how many are pinned (they get their own "Pinned" header).
pub fn pins_first(ids: &mut [Id]) -> usize {
    if !enabled(Plugin::PinDms) {
        return 0;
    }
    let pins = pinned();
    order(ids, &pins);
    ids.iter().take_while(|id| pins.contains(id)).count()
}

fn order(ids: &mut [Id], pins: &[Id]) {
    ids.sort_by_key(|id| !pins.contains(id));
}

/// "Pin DM" / "Unpin DM" in the DM menu (`item` draws the caller's menu row).
pub fn pin_item(ui: &mut egui::Ui, cx: &mut UiCx<'_>, id: Id, item: super::menus::Item<'_>) {
    if !enabled(Plugin::PinDms) {
        return;
    }
    let mut pins = pinned();
    let pinned = pins.contains(&id);
    if !item(ui, if pinned { "Unpin DM" } else { "Pin DM" }, "📌") {
        return;
    }
    if pinned {
        pins.retain(|p| *p != id);
    } else {
        pins.push(id);
    }
    let list = pins.iter().map(|p| toml::Value::String(p.0.to_string())).collect();
    if let Err(e) = super::set(Plugin::PinDms, "pinned", toml::Value::Array(list)) {
        cx.view.toast(format!("Could not save plugin settings: {e}"), true);
    }
}

// ---------------------------------------------------------------------------
// CallTimer
// ---------------------------------------------------------------------------

static CALL: Mutex<Option<(Id, Instant)>> = Mutex::new(None);

/// The voice panel's "Voice Connected" with the time in this channel after it.
pub fn call_timer(ctx: &egui::Context, state: &AppState, word: &str) -> String {
    let mut call = CALL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(ch) = state.voice.channel_id.filter(|_| state.voice.connected) else {
        *call = None;
        return word.to_owned();
    };
    if call.is_none_or(|(c, _)| c != ch) {
        *call = Some((ch, Instant::now()));
    }
    if !enabled(Plugin::CallTimer) {
        return word.to_owned();
    }
    let secs = call.map_or(0, |(_, t)| t.elapsed().as_secs());
    ctx.request_repaint_after(Duration::from_millis(1000 - (call.map_or(0, |(_, t)| t.elapsed().subsec_millis()) as u64)));
    format!("{word}  {}", format_call(secs, opt_choice(Plugin::CallTimer, "format") == 1))
}

/// Stopwatch "1d 02:03:04" / "03:04", or human "1d 2h 3m 4s".
pub fn format_call(s: u64, human: bool) -> String {
    let (d, h, m, sec) = (s / 86400, s / 3600 % 24, s / 60 % 60, s % 60);
    if human {
        let parts = [(d, "d"), (h, "h"), (m, "m")];
        let mut out: Vec<String> = parts.iter().skip_while(|(v, _)| *v == 0).map(|(v, u)| format!("{v}{u}")).collect();
        out.push(format!("{sec}s"));
        return out.join(" ");
    }
    let days = if d > 0 { format!("{d}d ") } else { String::new() };
    if d > 0 || h > 0 { format!("{days}{h:02}:{m:02}:{sec:02}") } else { format!("{m:02}:{sec:02}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_times_and_typing_tips() {
        assert_eq!(format_call(42, false), "00:42");
        assert_eq!(format_call(3 * 3600 + 5 * 60 + 9, false), "03:05:09");
        assert_eq!(format_call(86400 + 61, false), "1d 00:01:01");
        assert_eq!(format_call(86400 + 61, true), "1d 0h 1m 1s");
        assert_eq!(format_call(61, true), "1m 1s");
        // Off by default: nothing changes.
        assert!(!show_hidden());
        assert!(!force_open(false, true));
    }

    #[test]
    fn pinned_dms_on_top() {
        let mut ids = [Id(1), Id(2), Id(3), Id(4), Id(5)];
        order(&mut ids, &[Id(4), Id(2), Id(9)]);
        // Pinned first, each group keeps the list's (most recent) order.
        assert_eq!(ids, [Id(2), Id(4), Id(1), Id(3), Id(5)]);
        // Off by default: untouched.
        let mut ids = [Id(3), Id(1)];
        assert_eq!(pins_first(&mut ids), 0);
        assert_eq!(ids, [Id(3), Id(1)]);
    }
}
