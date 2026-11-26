//! Settings stored in Discord's settings proto (docs/SETTINGS_NOTES.md, field numbers and types
//! checked against PreloadedUserSettings): one table row per option (label, field path,
//! control). A change shows at once (`SettingsView::set_local`) and is saved with one
//! `SetSetting` holding only the changed paths of that top-level field (the backend merges them
//! into the current proto, so siblings keep their values). A field's rows wait while its write
//! is on the way; a failed write puts the old value back and shows a toast.
//! Of field 11 (status) only 11.3 "Share detected activity"; not here: options the notes mark
//! mobile-only, Nitro-gated or UNCONFIRMED in meaning.
//! OWNERSHIP: user-settings engineer.

use super::Page;
use super::super::messages::Pal;
use super::super::{UiCx, section};
use crate::api::settings_proto::SettingValue;
use crate::backend::{AccountCommand, AccountOp, Command, OpKind};
use crate::model::Id;
use crate::state::account::OpState;

/// Where an option lives: top-level field, subfield path, wrapped (`BoolValue` / `UInt32Value`
/// / `StringValue`) or a plain proto3 scalar (omitted when 0).
#[derive(Clone, Copy, Debug)]
pub(super) struct At {
    top: u32,
    path: &'static [u32],
    wrapped: bool,
}

const fn w(top: u32, path: &'static [u32]) -> At {
    At { top, path, wrapped: true }
}

const fn p(top: u32, path: &'static [u32]) -> At {
    At { top, path, wrapped: false }
}

#[derive(Debug)]
enum Ctl {
    /// On/off, read from the first field, written to all (same top-level field). `inv`: the
    /// stored flag means "off" (e.g. `hide_legacy_username`). `def`: shown state while unset.
    Switch { at: &'static [At], def: bool, inv: bool },
    /// One of `opts` (value, label); `def` while unset (a plain 0 is unset).
    Pick { at: At, def: u64, opts: &'static [(u64, &'static str)] },
    /// A `StringValue` choice.
    Text { at: At, def: &'static str, opts: &'static [(&'static str, &'static str)] },
    /// One bit of a `UInt32Value` flags field (`def` while unset).
    Flag { at: At, bit: u64, def: u64 },
}

#[derive(Debug)]
pub(super) struct Row {
    pub label: &'static str,
    hint: &'static str,
    ctl: Ctl,
}

impl Row {
    const fn on(label: &'static str, hint: &'static str, at: &'static [At], def: bool) -> Row {
        Row { label, hint, ctl: Ctl::Switch { at, def, inv: false } }
    }
    /// A switch whose stored flag means "off" (opt-outs, restrictions).
    const fn inv(label: &'static str, hint: &'static str, at: &'static [At], def: bool) -> Row {
        Row { label, hint, ctl: Ctl::Switch { at, def, inv: true } }
    }
    const fn pick(label: &'static str, hint: &'static str, at: At, def: u64, opts: &'static [(u64, &'static str)]) -> Row {
        Row { label, hint, ctl: Ctl::Pick { at, def, opts } }
    }
    const fn flag(label: &'static str, hint: &'static str, at: At, bit: u64, def: u64) -> Row {
        Row { label, hint, ctl: Ctl::Flag { at, bit, def } }
    }
}

pub(super) struct Group {
    pub title: &'static str,
    pub rows: &'static [Row],
}

// ----- the tables, per page (docs/SETTINGS_NOTES.md §1, §3, §4)

const REDACT: &[(u64, &str)] = &[(1, "Show"), (2, "Blur"), (3, "Block")];
const REDACT_SERVERS: &[(u64, &str)] = &[(1, "Show"), (2, "Blur")];

const PROFILES: &[Group] = &[Group {
    title: "Profile",
    rows: &[Row::inv("Show legacy username badge", "Your old name#0000 tag on your profile.", &[w(8, &[22])], true)],
}];

const CONTENT_SOCIAL: &[Group] = &[
    Group {
        title: "Sensitive media",
        rows: &[
            Row::pick("Direct messages from friends", "", p(6, &[29, 2]), 1, REDACT),
            Row::pick("Direct messages from others", "", p(6, &[29, 3]), 3, REDACT),
            Row::pick("Server channels", "", p(6, &[29, 1]), 1, REDACT_SERVERS),
        ],
    },
    Group {
        title: "Graphic content",
        rows: &[
            Row::pick("Direct messages from friends", "", p(6, &[32, 2]), 1, REDACT),
            Row::pick("Direct messages from others", "", p(6, &[32, 3]), 3, REDACT),
            Row::pick("Server channels", "", p(6, &[32, 1]), 1, REDACT_SERVERS),
        ],
    },
    Group {
        title: "Keyword filters",
        rows: &[
            Row::on("Profanity", "", &[w(6, &[30, 1])], false),
            Row::on("Sexual content", "", &[w(6, &[30, 2])], false),
            Row::on("Hateful slurs", "", &[w(6, &[30, 3])], false),
        ],
    },
    Group {
        title: "Direct messages",
        rows: &[
            Row::pick(
                "Filter direct message spam",
                "",
                p(6, &[27]),
                2,
                &[(1, "Don't filter"), (2, "From non-friends"), (3, "From everyone")],
            ),
            Row::inv("Allow DMs from members of new servers", "", &[p(8, &[4]), w(8, &[27])], true),
            Row::inv("Allow message requests from server members", "", &[w(8, &[17])], true),
            Row::on("Warn me about risky conversations", "Safety alerts in DMs with people you may not know.", &[w(8, &[23])], true),
        ],
    },
    Group {
        title: "Age-restricted content",
        rows: &[
            Row::on("Allow age-restricted servers on iOS", "", &[w(6, &[20])], false),
            Row::on("Allow age-restricted app commands in DMs", "", &[w(6, &[23])], false),
        ],
    },
    Group {
        title: "Games",
        rows: &[
            Row::pick("In-game direct messages", "", p(8, &[28]), 1, &[(1, "From everyone"), (2, "Players of my games"), (3, "Nobody")]),
            Row::on("Show DMs from game friends", "", &[w(8, &[26])], true),
            Row::pick("Leaderboards in new servers", "", p(8, &[25]), 0, &[(0, "Show me"), (1, "Hide me")]),
        ],
    },
];

/// 8.11 friend source flags: 2 friends of friends, 4 server members, 8 everyone (needs 2 | 4).
const FRIEND_SOURCES: At = w(8, &[11]);

const FRIEND_REQUESTS: &[Group] = &[
    Group {
        title: "Who can send you a friend request",
        rows: &[
            Row::flag("Everyone", "", FRIEND_SOURCES, 8, 14),
            Row::flag("Friends of friends", "", FRIEND_SOURCES, 2, 14),
            Row::flag("Server members", "", FRIEND_SOURCES, 4, 14),
        ],
    },
    Group { title: "Requests", rows: &[Row::on("Hide notes on friend requests", "", &[w(8, &[33])], false)] },
];

const PRIVACY: &[Group] = &[
    Group {
        title: "Profile",
        rows: &[Row::pick(
            "Who can see your profile",
            "",
            p(8, &[32]),
            3,
            &[(1, "Friends"), (2, "Friends and small servers"), (3, "Friends and server members")],
        )],
    },
    Group {
        title: "Your data",
        rows: &[
            Row::on("Help improve safety systems", "Lets Discord use reports on your messages to train its spam filters.", &[w(8, &[19])], false),
            Row::inv("Use third-party data for Quests", "", &[w(8, &[30])], true),
            Row::on("Allow screen reader detection", "", &[p(8, &[7])], false),
        ],
    },
    Group {
        title: "Discoverability",
        rows: &[
            Row::flag("Find me by phone number", "", w(8, &[12]), 2, 0),
            Row::flag("Find me by email", "", w(8, &[12]), 4, 0),
        ],
    },
];

const CLIPS: &[Group] = &[Group { title: "Voice", rows: &[Row::on("Let others record my voice in Clips", "", &[w(19, &[1])], true)] }];

const FAMILY: &[Group] = &[Group { title: "Family Center", rows: &[Row::on("Show Family Center", "", &[w(8, &[21])], false)] }];

const ACTIVITY: &[Group] = &[
    Group {
        title: "Activity status",
        rows: &[
            // 8.29 is a restriction: OFF shares everywhere.
            Row::pick("Share my activity in servers", "", p(8, &[29]), 1, &[(1, "In every server"), (2, "Not in large servers"), (3, "Nowhere")]),
            Row::on("Show recent games on my profile", "", &[w(8, &[24])], true),
            Row::on("Share detected activity", "Show the game I'm playing as my status.", &[w(11, &[3])], true),
        ],
    },
    Group {
        title: "Joining",
        rows: &[
            Row::on("Friends can join my activities", "", &[w(8, &[1])], true),
            Row::on("Voice channel members can join my activities", "", &[w(8, &[2])], true),
        ],
    },
];

const CHAT: &[Group] = &[
    Group {
        title: "Display images, videos and GIFs",
        rows: &[
            Row::on("When posted as links", "", &[w(6, &[10])], true),
            Row::on("When uploaded", "Off: uploads show as file cards.", &[w(6, &[9])], true),
            Row::on("With image descriptions", "", &[w(6, &[7])], false),
        ],
    },
    Group { title: "Embeds and link previews", rows: &[Row::on("Show embeds and website previews", "", &[w(6, &[12])], true)] },
    Group {
        title: "Emoji",
        rows: &[
            Row::on("Show emoji reactions", "", &[w(6, &[13])], true),
            Row::on("Convert emoticons to emoji", "Like :) to 🙂 (official apps).", &[w(6, &[21])], true),
            Row::on("Show expression suggestions", "", &[w(6, &[22])], true),
        ],
    },
    Group {
        title: "Spoilers",
        rows: &[Row {
            label: "Show spoiler content",
            hint: "",
            ctl: Ctl::Text { at: w(6, &[4]), def: "ON_CLICK", opts: &[("ON_CLICK", "On click"), ("IF_MODERATOR", "On servers I moderate"), ("ALWAYS", "Always")] },
        }],
    },
    Group {
        title: "Other",
        rows: &[
            Row::on("Open threads in split view", "", &[w(6, &[3])], true),
            Row::pick("Search the web with", "", p(6, &[37]), 1, &[(1, "Google"), (2, "Bing"), (3, "DuckDuckGo")]),
        ],
    },
];

const ACCESSIBILITY: &[Group] = &[Group {
    title: "Text-to-speech",
    rows: &[Row::on("Allow the /tts command", "", &[w(6, &[16])], true)],
}];

const NOTIFICATIONS: &[Group] = &[
    Group {
        title: "Reactions",
        rows: &[Row::pick("Notify me about reactions", "", p(7, &[7]), 0, &[(0, "On all my messages"), (1, "Only in DMs"), (2, "Never")])],
    },
    Group {
        title: "Friends",
        rows: &[
            Row::on("Friends going live", "", &[w(5, &[7])], true),
            Row::on("Tell friends when I go live", "", &[w(7, &[2])], false),
            Row::on("Tell friends when I come online", "", &[w(7, &[25])], true),
            Row::on("Tell friends when I update my profile", "", &[w(7, &[24])], true),
        ],
    },
    // "Push notifications when inactive for" left out: its field (P5.6?) is UNCONFIRMED.
];

const VOICE: &[Group] = &[Group {
    title: "Video and streams",
    rows: &[
        Row::on("Always preview video", "", &[w(5, &[5])], false),
        Row::inv("Show stream previews", "", &[w(5, &[10])], true),
    ],
}];

const ADVANCED: &[Group] = &[Group {
    title: "Developer",
    rows: &[Row::on("Developer Mode", "Adds “Copy ID” to the menus of users, servers, channels and messages.", &[p(13, &[2])], false)],
}];

/// Discord's languages (`locale` codes) with their own names.
const LOCALES: &[(&str, &str)] = &[
    ("da", "Dansk"),
    ("de", "Deutsch"),
    ("en-GB", "English, UK"),
    ("en-US", "English, US"),
    ("es-ES", "Español"),
    ("es-419", "Español, LATAM"),
    ("fr", "Français"),
    ("hr", "Hrvatski"),
    ("it", "Italiano"),
    ("lt", "Lietuviškai"),
    ("hu", "Magyar"),
    ("nl", "Nederlands"),
    ("no", "Norsk"),
    ("pl", "Polski"),
    ("pt-BR", "Português do Brasil"),
    ("ro", "Română"),
    ("fi", "Suomi"),
    ("sv-SE", "Svenska"),
    ("vi", "Tiếng Việt"),
    ("tr", "Türkçe"),
    ("cs", "Čeština"),
    ("el", "Ελληνικά"),
    ("bg", "български"),
    ("ru", "Русский"),
    ("uk", "Українська"),
    ("hi", "हिन्दी"),
    ("th", "ไทย"),
    ("zh-CN", "中文"),
    ("ja", "日本語"),
    ("zh-TW", "繁體中文"),
    ("ko", "한국어"),
];

const LANGUAGE: &[Group] = &[Group {
    title: "Language",
    rows: &[Row {
        label: "Language",
        hint: "Saved to your account (emails, server-side text). Rustcord's own text stays English.",
        ctl: Ctl::Text { at: w(12, &[1]), def: "en-US", opts: LOCALES },
    }],
}];

/// Chat 6.17 `message_display_compact`: Appearance's Message Display cards, like Discord's.
const COMPACT: At = w(6, &[17]);

/// The synced options of a page (empty for pages without any).
pub(super) fn groups(page: Page) -> &'static [Group] {
    match page {
        Page::Profiles => PROFILES,
        Page::ContentSocial => CONTENT_SOCIAL,
        Page::FriendRequests => FRIEND_REQUESTS,
        Page::Privacy => PRIVACY,
        Page::Clips => CLIPS,
        Page::FamilyCenter => FAMILY,
        Page::ActivityPrivacy => ACTIVITY,
        Page::Chat => CHAT,
        Page::Accessibility => ACCESSIBILITY,
        Page::Notifications => NOTIFICATIONS,
        Page::VoiceVideo => VOICE,
        Page::Advanced => ADVANCED,
        Page::Language => LANGUAGE,
        _ => &[],
    }
}

// ----- drawing and saving

fn busy(cx: &UiCx<'_>, top: u32) -> bool {
    cx.state.account.ops.get(&AccountOp { kind: OpKind::SetSetting, target: Some(Id(u64::from(top))) }) == Some(&OpState::Pending)
}

fn read(cx: &UiCx<'_>, at: At) -> Option<u64> {
    cx.state.account.settings.proto_varint(at.top, at.path, at.wrapped)
}

fn value(at: At, v: u64) -> (Vec<u32>, SettingValue) {
    (at.path.to_vec(), if at.wrapped { SettingValue::Wrapped(v) } else { SettingValue::Plain(v) })
}

/// Friend sources after (un)ticking `bit`: Everyone needs both others; dropping one of them
/// drops Everyone.
fn flags_after(at: At, flags: u64, bit: u64, on: bool) -> u64 {
    let f = if on { flags | bit } else { flags & !bit };
    if at.top != FRIEND_SOURCES.top || at.path != FRIEND_SOURCES.path {
        return f;
    }
    match (bit, on) {
        (8, true) => f | 2 | 4,
        (2 | 4, false) => f & !8,
        _ => f,
    }
}

/// Shows a change at once and sends its one write (nothing while that field's last write is on
/// its way: the rows are disabled then).
fn save(cx: &mut UiCx<'_>, top: u32, values: Vec<(Vec<u32>, SettingValue)>) {
    let cmd = AccountCommand::SetSetting { top, values: values.clone() };
    if cx.state.account.begin(cmd.op()) {
        cx.state.account.settings.set_local(top, &values);
        cx.cmd.send(Command::Account(cmd));
    }
}

/// The groups' sections and rows; at most one write per pass (one click). True when a local
/// setting changed (the caller saves it).
pub(super) fn show(ui: &mut egui::Ui, cx: &mut UiCx<'_>, groups: &'static [Group]) -> bool {
    let (mut write, mut changed) = (None, false);
    for g in groups {
        section(ui, cx.theme, g.title);
        for row in g.rows {
            if let Some(w) = row_ui(ui, cx, row) {
                write = Some(w);
            }
        }
        // This device's switch for the game connection sits under "Share detected activity", the last row of this group.
        if g.title == "Activity status" {
            changed |= game_connection(ui, cx);
        }
    }
    if let Some((top, values)) = write {
        save(cx, top, values);
    }
    changed
}

/// Let games show their activity (`Settings::game_connection`): not in Discord's proto.
fn game_connection(ui: &mut egui::Ui, cx: &mut UiCx<'_>) -> bool {
    let busy = cx.settings.game_connection && cx.state.rpc.none_free;
    let hint = if busy { "Another app is using the game connection." } else { "Games on this computer can show what you're doing in them and let friends join you. Rustcord answers them in place of the Discord app." };
    super::switch_row(ui, cx.theme, "Let games show their activity", Some(hint), &mut cx.settings.game_connection)
}

/// Appearance > Message Display: Cozy / Compact cards with a sketch of each (`COMPACT`, saved
/// like the rows). A theme's own compact flag counts too; picking Cozy clears it (an unsaved
/// theme edit, like the theme editor's).
pub(super) fn message_display(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    let synced = read(cx, COMPACT).is_some_and(|v| v != 0);
    let compact = synced || cx.theme.layout.compact;
    let pal = Pal::new(cx.theme);
    let mut pick = compact;
    ui.add_enabled_ui(!busy(cx, COMPACT.top), |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            for (dense, label) in [(false, "Cozy"), (true, "Compact")] {
                if display_card(ui, &pal, label, dense, compact == dense).clicked() {
                    pick = dense;
                }
            }
        });
    });
    if pick == compact {
        return;
    }
    if !pick && cx.theme.layout.compact {
        cx.theme.layout.compact = false;
        cx.view.theme_dirty = true;
    }
    if pick != synced {
        save(cx, COMPACT.top, vec![value(COMPACT, u64::from(pick))]);
    }
}

/// One Message Display choice: a sketch of three messages (cozy: avatar, name line over the
/// text; compact: one line each), the radio and label under it, a 2 px `text` ring when `on`.
fn display_card(ui: &mut egui::Ui, pal: &Pal, label: &str, dense: bool, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(176.0, 112.0), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, ui.is_enabled(), on, label));
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let hot = resp.hovered() || resp.has_focus();
        let ring = if on { pal.text } else if hot { pal.muted } else { pal.tk.rule };
        p.rect_filled(rect, pal.radius, pal.bg);
        p.rect_stroke(rect, pal.radius, egui::Stroke::new(if on { 2.0 } else { 1.0 }, ring), egui::StrokeKind::Inside);
        let sketch = egui::Rect::from_min_max(rect.min + egui::vec2(14.0, 14.0), egui::pos2(rect.right() - 14.0, rect.bottom() - 34.0));
        let ink = pal.muted.gamma_multiply(0.5);
        let bar = |x: f32, y: f32, w: f32, c: egui::Color32| p.rect_filled(egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, 6.0)), 3.0, c);
        for (i, w) in [0.8, 0.55, 0.7].into_iter().enumerate() {
            let (x, y) = (sketch.left(), sketch.top() + i as f32 * if dense { 14.0 } else { 22.0 });
            if dense {
                bar(x, y, 22.0, pal.muted);
                bar(x + 28.0, y, (sketch.width() - 28.0) * w, ink);
            } else if i < 2 {
                p.circle_filled(egui::pos2(x + 8.0, y + 8.0), 8.0, ink);
                bar(x + 22.0, y + 1.0, 36.0, pal.muted);
                bar(x + 22.0, y + 10.0, (sketch.width() - 22.0) * w, ink);
            }
        }
        let dot = egui::pos2(rect.left() + 20.0, rect.bottom() - 18.0);
        super::super::messages::paint_radio(p, pal, egui::Rect::from_center_size(dot, egui::Vec2::splat(16.0)), on);
        p.text(dot + egui::vec2(14.0, 0.0), egui::Align2::LEFT_CENTER, label, egui::FontId::new(pal.ui(), crate::theme::medium_family()), pal.text);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// One write: a top-level field and the (path, value) pairs to change in it.
type Write = (u32, Vec<(Vec<u32>, SettingValue)>);

fn row_ui(ui: &mut egui::Ui, cx: &mut UiCx<'_>, row: &Row) -> Option<Write> {
    let (label, hint_text) = (row.label, (!row.hint.is_empty()).then_some(row.hint));
    match row.ctl {
        Ctl::Switch { at, def, inv } => {
            let first = at[0];
            let mut on = read(cx, first).map_or(def, |v| (v != 0) != inv);
            let enabled = !busy(cx, first.top);
            let theme = &*cx.theme;
            let changed = ui.add_enabled_ui(enabled, |ui| super::switch_row(ui, theme, label, hint_text, &mut on)).inner;
            let stored = u64::from(on != inv);
            changed.then(|| (first.top, at.iter().map(|&a| value(a, stored)).collect()))
        }
        Ctl::Flag { at, bit, def } => {
            let flags = read(cx, at).unwrap_or(def);
            let mut on = flags & bit != 0;
            let enabled = !busy(cx, at.top);
            let theme = &*cx.theme;
            let changed = ui.add_enabled_ui(enabled, |ui| super::switch_row(ui, theme, label, hint_text, &mut on)).inner;
            changed.then(|| (at.top, vec![value(at, flags_after(at, flags, bit, on))]))
        }
        Ctl::Pick { at, def, opts } => {
            let cur = read(cx, at).filter(|&v| v != 0 || at.wrapped).unwrap_or(def);
            let shown = opts.iter().find(|o| o.0 == cur).map_or("", |o| o.1);
            let mut pick = cur;
            pick_row(ui, cx, label, hint_text, at, shown, |ui| {
                for &(v, text) in opts {
                    ui.selectable_value(&mut pick, v, text);
                }
            });
            (pick != cur).then(|| (at.top, vec![value(at, pick)]))
        }
        Ctl::Text { at, def, opts } => {
            let cur = cx.state.account.settings.proto_str(at.top, at.path).filter(|s| !s.is_empty()).unwrap_or_else(|| def.to_owned());
            let shown = opts.iter().find(|o| o.0 == cur).map_or(cur.as_str(), |o| o.1).to_owned();
            let mut pick = cur.clone();
            pick_row(ui, cx, label, hint_text, at, &shown, |ui| {
                for &(v, text) in opts {
                    if ui.selectable_label(pick == v, text).clicked() {
                        v.clone_into(&mut pick);
                    }
                }
            });
            (pick != cur).then(|| (at.top, vec![(at.path.to_vec(), SettingValue::Str(pick))]))
        }
    }
}

/// A setting row (`super::setting_row`) with a dropdown: the shared settings combo box.
fn pick_row(ui: &mut egui::Ui, cx: &UiCx<'_>, label: &str, hint_text: Option<&str>, at: At, shown: &str, items: impl FnOnce(&mut egui::Ui)) {
    const WIDTH: f32 = 220.0;
    // egui's combo box: one line of button text and the button padding.
    let height = ui.text_style_height(&egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.y;
    let enabled = !busy(cx, at.top);
    super::setting_row(ui, cx.theme, label, hint_text, egui::vec2(WIDTH, height), |ui| {
        ui.add_enabled_ui(enabled, |ui| {
            egui::ComboBox::from_id_salt(("rc_synced_pick", at.top, at.path)).width(WIDTH).selected_text(shown).icon(super::combo_icon).show_ui(ui, items);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::settings_proto::{decode, read_varint, with_values};

    const ALL: &[&[Group]] = &[PROFILES, CONTENT_SOCIAL, FRIEND_REQUESTS, PRIVACY, CLIPS, FAMILY, ACTIVITY, CHAT, ACCESSIBILITY, NOTIFICATIONS, VOICE, ADVANCED, LANGUAGE];

    #[test]
    fn rows_are_sane() {
        // No option is on two rows (flags: no bit twice).
        let mut seen = std::collections::HashSet::new();
        for row in ALL.iter().flat_map(|g| g.iter()).flat_map(|g| g.rows) {
            let (at, bit) = match row.ctl {
                Ctl::Switch { at, .. } => (at[0], 0),
                Ctl::Pick { at, .. } | Ctl::Text { at, .. } => (at, 0),
                Ctl::Flag { at, bit, .. } => (at, bit),
            };
            assert!(seen.insert((at.top, at.path, bit)), "{} is shown twice", row.label);
            match row.ctl {
                // Every path of a switch is in one top-level field (one SetSetting), never 14 and
                // of 11 only 11.3 (`api::account::save_settings` refuses the rest).
                Ctl::Switch { at, .. } => assert!(at.iter().all(|a| a.top == at[0].top && a.top != 14 && (a.top != 11 || a.path == [3]))),
                Ctl::Pick { at, def, opts } => assert!(opts.iter().any(|o| o.0 == def) && !matches!(at.top, 11 | 14)),
                Ctl::Text { def, opts, .. } => assert!(opts.iter().any(|o| o.0 == def)),
                Ctl::Flag { bit, .. } => assert!(bit.is_power_of_two()),
            }
        }
    }

    #[test]
    fn writes_match_the_notes_vectors() {
        // docs/SETTINGS_NOTES.md §0: compact mode on / off, developer mode on.
        let enc = |v| with_values("", COMPACT.top, &[value(COMPACT, v)]).unwrap();
        assert_eq!((enc(1).as_str(), enc(0).as_str()), ("MgWKAQIIAQ==", "MgWKAQIIAA=="));
        let Ctl::Switch { at, .. } = ADVANCED[0].rows[0].ctl else { panic!() };
        assert_eq!(with_values("", 13, &[value(at[0], 1)]).unwrap(), "agIQAQ==");
        // "Allow DMs from new servers" off = both restriction flags set, in one write.
        let dms = CONTENT_SOCIAL.iter().flat_map(|g| g.rows).find(|r| r.label.starts_with("Allow DMs")).unwrap();
        let Ctl::Switch { at, inv: true, .. } = dms.ctl else { panic!() };
        let out = with_values("", 8, &at.iter().map(|&a| value(a, 1)).collect::<Vec<_>>()).unwrap();
        let f = &decode(&out).unwrap().raw[&8];
        assert_eq!((read_varint(f, &[4], false), read_varint(f, &[27], true)), (Some(1), Some(1)));
    }

    #[test]
    fn friend_request_flags() {
        let a = FRIEND_SOURCES;
        assert_eq!(flags_after(a, 0, 8, true), 14);
        assert_eq!(flags_after(a, 14, 2, false), 4);
        assert_eq!(flags_after(a, 6, 4, false), 2);
        assert_eq!(flags_after(a, 2, 4, true), 6);
        // Other flag fields have no implied bits.
        assert_eq!(flags_after(w(8, &[12]), 6, 2, false), 4);
    }
}
