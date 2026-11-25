//! The updater's UI (docs/UPDATER_DESIGN.md §15): the full-window update screen, the title-bar
//! button, the copy of the Updates rows (user_settings/app.rs) and the `RUSTCORD_UPDATE_DEMO`
//! screenshot hook. Everything reads `status()` once per pass; core (`crate::update`) wakes the
//! UI when it changes, so nothing here repaints on its own.
//! OWNERSHIP: ui package (updater). The engine and `Status` are `crate::update` (core).

use super::messages::{self, Btn, Pal};
use super::{UiCx, ViewState, icons, semibold};
use crate::state::AppState;
use crate::update::{Kind, Phase, Status};
use egui::{Align, Color32, FontId, Frame, Key, Layout, Margin, Rect, RichText, Sense, Shape, Stroke, UiBuilder, Vec2, pos2, vec2};
use std::path::PathBuf;
use std::sync::OnceLock;

/// The updater's state for this pass: the `RUSTCORD_UPDATE_DEMO` fake (§15.4), else core's.
pub fn status() -> Status {
    static DEMO: OnceLock<Option<Status>> = OnceLock::new();
    let demo = DEMO.get_or_init(|| std::env::var("RUSTCORD_UPDATE_DEMO").ok().and_then(|v| demo(v.trim())));
    demo.clone().unwrap_or_else(crate::update::status)
}

/// Test hook `RUSTCORD_UPDATE_DEMO=<name>`: a fake state for version 0.1.1, no network (the
/// variable turns core's updater off, so its buttons do nothing).
fn demo(name: &str) -> Option<Status> {
    let version = "0.1.1".to_owned();
    let (phase, kind, screen) = match name {
        "checking" => (Phase::Checking, Kind::Auto, false),
        "uptodate" => (Phase::UpToDate, Kind::Auto, false),
        "available" => (Phase::Available { version }, Kind::Auto, false),
        "ready" => (Phase::Ready { version }, Kind::Auto, false),
        "download" => (Phase::Downloading { version, done: 7_300_000, total: 12_000_000 }, Kind::Auto, true),
        "install" => (Phase::Installing { version }, Kind::Auto, true),
        "restart" => (Phase::Restarting { version }, Kind::Auto, true),
        "error" => (Phase::Failed { message: "Could not download the update. Check your connection; Rustcord tries again later.".into() }, Kind::Auto, true),
        "deb" => (Phase::Ready { version }, Kind::Package, true),
        "manual" => (Phase::Manual { version, path: PathBuf::from("/home/you/.cache/rustcord/update/rustcord_0.1.1_amd64.deb") }, Kind::Package, true),
        _ => return None,
    };
    Some(Status { phase, kind, screen, enabled: true, note: None })
}

/// No restart now (`crate::update::sync_ui`'s `busy`): in or joining a voice channel or call, a
/// call I just started, or a call ringing me; while the window waits in the tray (`hidden`) also
/// an unsent draft, which a restart would lose, or a quest video that plays on. Visible, the user sees the draft and the
/// title-bar button.
pub(super) fn busy(state: &AppState, view: &ViewState, hidden: bool) -> bool {
    state.voice.channel_id.is_some()
        || state.call_starting()
        || state.incoming_call().is_some()
        || (hidden && (view.drafts.values().any(|d| !d.trim().is_empty()) || super::quests::watching(view)))
}

/// Megabytes as the update screen writes them: 1 MB = 1 000 000 bytes, one decimal.
fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1e6)
}

/// "7.3 MB of 12.0 MB" (an unknown total: "7.3 MB").
fn progress(done: u64, total: u64) -> String {
    if total == 0 { mb(done) } else { format!("{} of {}", mb(done), mb(total)) }
}

// ---------------------------------------------------------------------------
// Title-bar button (§15.2)
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
struct TitleButton {
    label: &'static str,
    tip: String,
    enabled: bool,
}

/// The title-bar button for this phase, if it shows one.
fn title_copy(phase: &Phase, kind: Kind, busy: bool) -> Option<TitleButton> {
    let on = |label, tip: String| Some(TitleButton { label, tip, enabled: true });
    match (phase, kind) {
        (Phase::Available { version }, Kind::NoticeOnly) => on("Update available", format!("Open the Rustcord {version} release page")),
        (Phase::Available { version }, _) => on("Update available", format!("Download and install Rustcord {version}")),
        // Installing never restarts during a call (core waits), so the button waits too.
        (Phase::Ready { .. }, Kind::Package) if busy => Some(TitleButton { label: "Update ready", tip: "Install it after your call ends".into(), enabled: false }),
        (Phase::Ready { .. }, _) if busy => Some(TitleButton { label: "Update ready", tip: "Installs after your call ends".into(), enabled: false }),
        (Phase::Ready { version }, Kind::Package) => on("Update ready", format!("Install Rustcord {version}")),
        (Phase::Ready { version }, _) => on("Update ready", format!("Restart Rustcord to install {version}")),
        (Phase::Manual { version, .. }, _) => on("Update available", format!("Show where Rustcord {version} was saved")),
        _ => None,
    }
}

/// The "Update available" / "Update ready" pill in the title bar row `ui` (§15.2): right-aligned,
/// 8 px left of the inbox button when it shows (`inbox`), else 8 px from the row's end. The row
/// is chrome's content rect, so it never reaches the window buttons.
pub(super) fn title_button(ui: &mut egui::Ui, cx: &UiCx<'_>, up: &Status, inbox: bool) {
    let b = title_copy(&up.phase, up.kind, busy(cx.state, cx.view, false));
    // Fades in and slides 8 px in from the right; never on the first frame it is drawn (principle 19).
    let t = super::motion::anim(ui.ctx(), egui::Id::unique("rc_update_pill"), b.is_some(), super::motion::LARGE, 0.0);
    let Some(b) = b else { return };
    let pal = Pal::new(cx.theme);
    let (ink, icon) = if b.enabled { (pal.tk.positive_text, pal.tk.positive) } else { (pal.muted, pal.muted) };
    let mut p = ui.painter().clone();
    p.multiply_opacity(t);
    let g = p.layout_no_wrap(b.label.to_owned(), semibold(pal.label()), ink);
    let row = ui.max_rect();
    // The inbox button: 28 px, 8 px from the end (sidebar::title_bar).
    let right = row.right() - if inbox { 8.0 + 28.0 + 8.0 } else { 8.0 };
    let w = 8.0 + 16.0 + 6.0 + g.size().x + 8.0;
    let rect = Rect::from_min_max(pos2(right - w, row.center().y - 12.0), pos2(right, row.center().y + 12.0));
    let sense = if b.enabled { Sense::click() } else { Sense::hover() };
    let resp = ui.interact(rect, ui.make_persistent_id("rc_update_button"), sense);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, b.enabled, b.label));
    let bg = cx.theme.panel_fills().frame.to_color32();
    let fill = if b.enabled && resp.hovered() { pal.tk.hover } else { Color32::TRANSPARENT };
    let paint = rect.translate(vec2(8.0 * (1.0 - t), 0.0));
    p.rect_filled(paint, pal.radius, fill);
    let icon_rect = Rect::from_min_size(pos2(paint.left() + 8.0, paint.center().y - 8.0), Vec2::splat(16.0));
    icons::paint(&p, icons::Icon::Download, icon_rect, icon, bg.blend(fill));
    p.galley(pos2(icon_rect.right() + 6.0, paint.center().y - g.size().y / 2.0), g, ink);
    if resp.has_focus() {
        messages::focus_ring(ui, &pal, paint, f32::from(pal.radius));
    }
    let resp = resp.on_hover_text(&b.tip);
    if b.enabled && resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        crate::update::button_clicked();
    }
}

// ---------------------------------------------------------------------------
// Settings > Windows / Linux / macOS > Updates (§15.3; the rows are in user_settings/app.rs)
// ---------------------------------------------------------------------------

/// Description of "Install updates automatically".
pub(super) fn auto_meta(up: &Status) -> &'static str {
    match up.kind {
        _ if !up.enabled => "Updates are off for development builds and test runs.",
        Kind::NoticeOnly => "This copy of Rustcord can't update itself. New versions are on the releases page.",
        Kind::Package => "Rustcord downloads new versions in the background. Installing asks for your password.",
        // macOS never installs from the menu bar: the relaunch could ask for keychain access
        // with no window in sight (design review item 6).
        Kind::Auto if cfg!(target_os = "macos") => "Rustcord downloads new versions in the background and installs them when it starts, never during a call.",
        Kind::Auto => "Rustcord downloads new versions in the background and installs them when it starts or while it waits in the tray, never during a call.",
    }
}

/// The "Version 0.1.0" row's description: what the updater is doing (None while it is off).
pub(super) fn settings_meta(up: &Status) -> Option<String> {
    if !up.enabled {
        return None;
    }
    // A background download failed: the title-bar button stays, the reason is here.
    if let (Phase::Available { version }, Some(note)) = (&up.phase, &up.note) {
        return Some(format!("Version {version} is available. {note}"));
    }
    Some(match &up.phase {
        Phase::Idle => "Rustcord checks for updates every 6 hours.".into(),
        Phase::Checking => "Checking for updates".into(),
        Phase::UpToDate => "Rustcord is up to date.".into(),
        Phase::Available { version } if up.kind == Kind::NoticeOnly => format!("Version {version} is on the releases page."),
        Phase::Available { version } => format!("Version {version} is available."),
        Phase::Downloading { version, total: 0, .. } => format!("Downloading version {version}"),
        Phase::Downloading { version, done, total } => format!("Downloading version {version} ({} %)", (*done as f64 * 100.0 / *total as f64).round().min(100.0)),
        // Not "installs when Rustcord restarts": with auto install off, or for a version that
        // failed to install (blocked), it waits for a click on "Update ready".
        Phase::Ready { version } => format!("Version {version} is ready to install."),
        Phase::Installing { version } => format!("Installing version {version}"),
        Phase::Restarting { .. } => "Restarting".into(),
        Phase::Manual { version, path } => format!("Version {version} is downloaded to {}.", path.display()),
        Phase::Failed { message } => message.clone(),
    })
}

/// "Check for updates" works (not while one runs or an update is on its way).
pub(super) fn can_check(up: &Status) -> bool {
    up.enabled && !matches!(up.phase, Phase::Checking | Phase::Downloading { .. } | Phase::Installing { .. } | Phase::Restarting { .. })
}

// ---------------------------------------------------------------------------
// Update screen (§15.1)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Install,
    Dismiss,
}

/// What the update screen shows for a state (the §15.1 table).
#[derive(Debug, Default, PartialEq)]
struct ScreenCopy {
    title: String,
    /// The progress bar, 0..=1; None: no bar.
    bar: Option<f32>,
    /// A failure: `Alert` and the message in `danger`, in the bar's place.
    error: Option<String>,
    meta: Option<String>,
    /// The saved package, in a well under the meta line.
    path: Option<String>,
    primary: Option<(&'static str, Action)>,
    /// "Not now": dismiss.
    ghost: bool,
}

fn screen_copy(up: &Status) -> ScreenCopy {
    let installing = |version: &str| ScreenCopy { title: "Installing update".into(), bar: Some(1.0), meta: Some(format!("Rustcord {version}")), ..Default::default() };
    match &up.phase {
        Phase::Downloading { version, done, total } => ScreenCopy {
            title: format!("Downloading update {version}"),
            bar: Some(if *total == 0 { 0.0 } else { (*done as f32 / *total as f32).clamp(0.0, 1.0) }),
            meta: Some(progress(*done, *total)),
            ghost: true,
            ..Default::default()
        },
        Phase::Installing { version } => installing(version),
        Phase::Restarting { version } => {
            ScreenCopy { title: "Restarting".into(), bar: Some(1.0), meta: Some(format!("Rustcord {version} is starting.")), ..Default::default() }
        }
        Phase::Failed { message } => ScreenCopy {
            title: "Update failed".into(),
            error: Some(message.clone()),
            meta: Some("Rustcord continues in a few seconds.".into()),
            primary: Some(("Continue", Action::Dismiss)),
            ..Default::default()
        },
        Phase::Ready { version } if up.kind == Kind::Package => ScreenCopy {
            title: format!("Update {version} is ready"),
            meta: Some("Installing it asks for your password.".into()),
            primary: Some(("Install", Action::Install)),
            ghost: true,
            ..Default::default()
        },
        Phase::Manual { version, path } => ScreenCopy {
            title: format!("Update {version} is downloaded"),
            meta: Some("Install it with your package manager:".into()),
            path: Some(path.display().to_string()),
            primary: Some(("Continue", Action::Dismiss)),
            ..Default::default()
        },
        // Not reached in the normal flow; drawn sensibly anyway.
        Phase::Ready { version } => installing(version),
        Phase::Available { version } => ScreenCopy { title: format!("Downloading update {version}"), bar: Some(0.0), ghost: true, ..Default::default() },
        Phase::Idle | Phase::Checking | Phase::UpToDate => ScreenCopy { title: "Checking for updates".into(), bar: Some(0.0), ..Default::default() },
    }
}

/// The full-window update screen under the title bar (`status.screen`): a centred column on the
/// flat frame colour, no card. Enter = the primary button, Esc = "Not now" / "Continue"; a
/// click anywhere on a failure continues too.
pub(super) fn screen(ui: &mut egui::Ui, cx: &UiCx<'_>, up: &Status) {
    let copy = screen_copy(up);
    let busy = busy(cx.state, cx.view, false);
    let pal = Pal::new(cx.theme);
    let page = Frame::new().fill(cx.theme.colors.guild_bar.to_color32());
    egui::CentralPanel::default().frame(page).show(ui, |ui| {
        ui.style_mut().interaction.selectable_labels = false;
        let avail = ui.available_rect_before_wrap();
        // Registered first, so the buttons lie on top of it; not focusable (Tab skips it).
        let page_click = copy.error.is_some() && ui.interact(avail, ui.make_persistent_id("rc_update_page"), Sense::CLICK).clicked();
        // Centred on the column's height from the pass before (one discarded pass when it changes).
        let height_id = ui.make_persistent_id("rc_update_column_h");
        let h: f32 = ui.data(|d| d.get_temp(height_id)).unwrap_or(0.0);
        let col_w = 480.0f32.min(avail.width() - 32.0).max(120.0);
        let top = avail.top() + ((avail.height() - h) / 2.0).max(16.0);
        let col = Rect::from_min_max(pos2(avail.center().x - col_w / 2.0, top), pos2(avail.center().x + col_w / 2.0, avail.bottom().max(top)));
        let out = ui.scope_builder(UiBuilder::new().max_rect(col).layout(Layout::top_down(Align::Center)), |ui| column(ui, cx, &pal, &copy, busy));
        let used = out.response.rect.height();
        if (used - h).abs() > 0.5 {
            ui.data_mut(|d| d.insert_temp(height_id, used));
            ui.ctx().request_discard("update screen height");
        }
        let (enter, esc) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
        // A focused button takes Enter itself.
        let enter = enter && ui.memory(|m| m.focused().is_none());
        let mut action = out.inner.or(copy.primary.filter(|(_, a)| enter && !(busy && *a == Action::Install)).map(|(_, a)| a));
        if (esc && (copy.ghost || copy.primary.is_some_and(|(_, a)| a == Action::Dismiss))) || page_click {
            action = action.or(Some(Action::Dismiss));
        }
        match action {
            Some(Action::Install) => crate::update::install(),
            Some(Action::Dismiss) => crate::update::dismiss(),
            None => {}
        }
    });
}

/// Logo, title, bar or error, meta, path, buttons (§15.1 gaps). The clicked button's action.
fn column(ui: &mut egui::Ui, cx: &UiCx<'_>, pal: &Pal, copy: &ScreenCopy, busy: bool) -> Option<Action> {
    ui.spacing_mut().item_spacing = vec2(12.0, 0.0);
    if let Some(tex) = super::login::app_icon(ui.ctx()) {
        ui.add(egui::Image::new(&tex).fit_to_exact_size(Vec2::splat(56.0)));
    }
    ui.add_space(24.0);
    ui.add(egui::Label::new(RichText::new(&copy.title).font(semibold(pal.title())).color(pal.text)).wrap());
    ui.add_space(16.0);
    if let Some(t) = copy.bar {
        let (rect, _) = ui.allocate_exact_size(vec2(320.0f32.min(ui.available_width()), 4.0), Sense::hover());
        ui.painter().rect_filled(rect, 2.0, pal.tk.edge.gamma_multiply(0.5));
        if t > 0.0 {
            ui.painter().rect_filled(Rect::from_min_size(rect.min, vec2(rect.width() * t, 4.0)), 2.0, pal.text);
        }
        ui.add_space(8.0);
    }
    if let Some(message) = &copy.error {
        error_line(ui, pal, message, cx.theme.colors.guild_bar.to_color32());
        ui.add_space(8.0);
    }
    if let Some(meta) = &copy.meta {
        ui.add(egui::Label::new(RichText::new(meta).size(pal.meta()).color(pal.muted)).wrap());
    }
    if let Some(path) = &copy.path {
        ui.add_space(8.0);
        Frame::new().fill(cx.theme.colors.code_background.to_color32()).corner_radius(pal.radius).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(path).font(FontId::monospace(pal.ui())).color(pal.text)).selectable(true).wrap());
        });
    }
    if copy.primary.is_none() && !copy.ghost {
        return None;
    }
    ui.add_space(24.0);
    buttons(ui, pal, copy, busy)
}

/// "Not now" (ghost) left of the primary button, centred, 40 tall.
fn buttons(ui: &mut egui::Ui, pal: &Pal, copy: &ScreenCopy, busy: bool) -> Option<Action> {
    let font = FontId::new(pal.ui(), crate::theme::medium_family());
    let width = |t: &str| (ui.painter().layout_no_wrap(t.to_owned(), font.clone(), pal.text).size().x + 32.0).max(64.0);
    let labels = copy.ghost.then_some("Not now").into_iter().chain(copy.primary.map(|(l, _)| l));
    let row_w: f32 = labels.map(width).sum::<f32>() + if copy.ghost && copy.primary.is_some() { 12.0 } else { 0.0 };
    ui.allocate_ui_with_layout(vec2(row_w + 1.0, 40.0), Layout::left_to_right(Align::Center), |ui| {
        ui.spacing_mut().interact_size.y = 40.0;
        let mut action = None;
        if copy.ghost && messages::button(ui, pal, Btn::Ghost, "Not now", true).clicked() {
            action = Some(Action::Dismiss);
        }
        if let Some((label, a)) = copy.primary {
            // pkexec would only run after the call (core waits): no dead button meanwhile.
            let blocked = busy && a == Action::Install;
            let resp = messages::button(ui, pal, Btn::Primary, label, !blocked);
            if blocked {
                resp.on_disabled_hover_text("Install it after your call ends");
            } else if resp.clicked() {
                action = Some(a);
            }
        }
        action
    })
    .inner
}

/// A failure line: `Alert` 16 and the message (ui role, `danger`), wrapped to the column.
fn error_line(ui: &mut egui::Ui, pal: &Pal, message: &str, bg: Color32) {
    let g = ui.painter().layout(message.to_owned(), FontId::proportional(pal.ui()), pal.danger, (ui.available_width() - 24.0).max(40.0));
    let first = g.rows.first().map_or(16.0, |r| r.rect().height());
    let (rect, _) = ui.allocate_exact_size(vec2(24.0 + g.size().x, g.size().y.max(16.0)), Sense::hover());
    alert(ui.painter(), Rect::from_min_size(pos2(rect.left(), rect.top() + (first - 16.0) / 2.0), Vec2::splat(16.0)), pal.danger, bg);
    ui.painter().galley(pos2(rect.left() + 24.0, rect.top()), g, pal.danger);
}

/// `Alert`: the channel badge's triangle as a full icon (24-unit grid), the "!" cut out in `bg`.
/// shortcut: icons.rs has only the badge form; move this there as `Icon::Alert` when a second
/// view needs it.
fn alert(p: &egui::Painter, rect: Rect, color: Color32, bg: Color32) {
    let at = |x: f32, y: f32| rect.min + vec2(x, y) * (rect.width() / 24.0);
    let k = rect.width() / 24.0;
    p.add(Shape::convex_polygon(vec![at(12.0, 2.5), at(22.5, 21.0), at(1.5, 21.0)], color, Stroke::new(2.0 * k, color)));
    p.line_segment([at(12.0, 8.5), at(12.0, 14.0)], Stroke::new(2.25 * k, bg));
    p.circle_filled(at(12.0, 17.25), 1.4 * k, bg);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(phase: Phase, kind: Kind) -> Status {
        Status { phase, kind, screen: false, enabled: true, note: None }
    }

    fn v() -> String {
        "0.1.1".into()
    }

    #[test]
    fn every_demo_name_has_its_state() {
        let ready = Phase::Ready { version: v() };
        let path = PathBuf::from("/home/you/.cache/rustcord/update/rustcord_0.1.1_amd64.deb");
        let cases = [
            ("checking", Phase::Checking, Kind::Auto, false),
            ("uptodate", Phase::UpToDate, Kind::Auto, false),
            ("available", Phase::Available { version: v() }, Kind::Auto, false),
            ("ready", ready.clone(), Kind::Auto, false),
            ("download", Phase::Downloading { version: v(), done: 7_300_000, total: 12_000_000 }, Kind::Auto, true),
            ("install", Phase::Installing { version: v() }, Kind::Auto, true),
            ("restart", Phase::Restarting { version: v() }, Kind::Auto, true),
            ("error", Phase::Failed { message: "Could not download the update. Check your connection; Rustcord tries again later.".into() }, Kind::Auto, true),
            ("deb", ready, Kind::Package, true),
            ("manual", Phase::Manual { version: v(), path }, Kind::Package, true),
        ];
        for (name, phase, kind, screen) in cases {
            assert_eq!(demo(name), Some(Status { phase, kind, screen, enabled: true, note: None }), "{name}");
        }
        assert_eq!(demo("nope"), None);
        assert_eq!(demo(""), None);
    }

    #[test]
    fn megabytes_have_one_decimal() {
        assert_eq!(mb(7_300_000), "7.3 MB");
        assert_eq!(mb(0), "0.0 MB");
        assert_eq!(mb(12_049_999), "12.0 MB");
        assert_eq!(progress(7_300_000, 12_000_000), "7.3 MB of 12.0 MB");
        assert_eq!(progress(7_300_000, 0), "7.3 MB");
    }

    #[test]
    fn title_button_per_phase_and_kind() {
        let tip = |p: &Phase, k, busy| title_copy(p, k, busy).map(|b| (b.label, b.tip, b.enabled));
        let available = Phase::Available { version: v() };
        let ready = Phase::Ready { version: v() };
        let s = |t: &str| t.to_owned();
        assert_eq!(tip(&available, Kind::Auto, false), Some(("Update available", s("Download and install Rustcord 0.1.1"), true)));
        assert_eq!(tip(&available, Kind::Package, true), Some(("Update available", s("Download and install Rustcord 0.1.1"), true)), "downloads during a call");
        assert_eq!(tip(&available, Kind::NoticeOnly, false), Some(("Update available", s("Open the Rustcord 0.1.1 release page"), true)));
        assert_eq!(tip(&ready, Kind::Auto, false), Some(("Update ready", s("Restart Rustcord to install 0.1.1"), true)));
        assert_eq!(tip(&ready, Kind::Package, false), Some(("Update ready", s("Install Rustcord 0.1.1"), true)));
        assert_eq!(tip(&ready, Kind::Auto, true), Some(("Update ready", s("Installs after your call ends"), false)));
        assert_eq!(tip(&ready, Kind::Package, true), Some(("Update ready", s("Install it after your call ends"), false)));
        let manual = Phase::Manual { version: v(), path: PathBuf::from("/x.deb") };
        assert_eq!(tip(&manual, Kind::Package, true), Some(("Update available", s("Show where Rustcord 0.1.1 was saved"), true)));
        for p in [Phase::Idle, Phase::Checking, Phase::UpToDate, Phase::Installing { version: v() }, Phase::Failed { message: s("x") }] {
            assert_eq!(tip(&p, Kind::Auto, false), None, "{p:?}");
        }
        assert_eq!(tip(&Phase::Downloading { version: v(), done: 1, total: 2 }, Kind::Auto, false), None);
    }

    #[test]
    fn settings_meta_per_phase() {
        let meta = |p: Phase, k| settings_meta(&st(p, k));
        let s = |t: &str| Some(t.to_owned());
        assert_eq!(meta(Phase::Idle, Kind::Auto), s("Rustcord checks for updates every 6 hours."));
        assert_eq!(meta(Phase::Checking, Kind::Auto), s("Checking for updates"));
        assert_eq!(meta(Phase::UpToDate, Kind::Auto), s("Rustcord is up to date."));
        assert_eq!(meta(Phase::Available { version: v() }, Kind::Auto), s("Version 0.1.1 is available."));
        assert_eq!(meta(Phase::Available { version: v() }, Kind::NoticeOnly), s("Version 0.1.1 is on the releases page."));
        assert_eq!(meta(Phase::Downloading { version: v(), done: 7_300_000, total: 12_000_000 }, Kind::Auto), s("Downloading version 0.1.1 (61 %)"));
        assert_eq!(meta(Phase::Downloading { version: v(), done: 5, total: 0 }, Kind::Auto), s("Downloading version 0.1.1"));
        assert_eq!(meta(Phase::Downloading { version: v(), done: 9, total: 3 }, Kind::Auto), s("Downloading version 0.1.1 (100 %)"));
        assert_eq!(meta(Phase::Ready { version: v() }, Kind::Auto), s("Version 0.1.1 is ready to install."));
        assert_eq!(meta(Phase::Ready { version: v() }, Kind::Package), s("Version 0.1.1 is ready to install."));
        assert_eq!(meta(Phase::Installing { version: v() }, Kind::Auto), s("Installing version 0.1.1"));
        assert_eq!(meta(Phase::Restarting { version: v() }, Kind::Auto), s("Restarting"));
        let path = PathBuf::from("/home/you/x.deb");
        assert_eq!(meta(Phase::Manual { version: v(), path }, Kind::Package), s("Version 0.1.1 is downloaded to /home/you/x.deb."));
        assert_eq!(meta(Phase::Failed { message: "Could not check for updates. Check your connection.".into() }, Kind::Auto), s("Could not check for updates. Check your connection."));
        assert_eq!(settings_meta(&Status { enabled: false, ..Default::default() }), None, "off: no line");
        let failed = Status { note: Some("Could not download the update. Check your connection; Rustcord tries again later.".into()), ..st(Phase::Available { version: v() }, Kind::Auto) };
        assert_eq!(settings_meta(&failed), s("Version 0.1.1 is available. Could not download the update. Check your connection; Rustcord tries again later."));
    }

    #[test]
    fn auto_switch_meta_and_check_button() {
        let off = Status::default();
        assert_eq!(auto_meta(&off), "Updates are off for development builds and test runs.");
        assert!(auto_meta(&st(Phase::Idle, Kind::NoticeOnly)).starts_with("This copy of Rustcord can't update itself."));
        assert!(auto_meta(&st(Phase::Idle, Kind::Package)).ends_with("Installing asks for your password."));
        assert!(auto_meta(&st(Phase::Idle, Kind::Auto)).ends_with("never during a call."));
        assert!(!can_check(&off));
        for p in [Phase::Idle, Phase::UpToDate, Phase::Available { version: v() }, Phase::Ready { version: v() }, Phase::Failed { message: "x".into() }] {
            assert!(can_check(&st(p.clone(), Kind::Auto)), "{p:?}");
        }
        for p in [Phase::Checking, Phase::Downloading { version: v(), done: 0, total: 1 }, Phase::Installing { version: v() }, Phase::Restarting { version: v() }] {
            assert!(!can_check(&st(p.clone(), Kind::Auto)), "{p:?}");
        }
    }

    #[test]
    fn screen_rows_follow_the_table() {
        let copy = |p: Phase, k| screen_copy(&st(p, k));
        let s = |t: &str| Some(t.to_owned());
        let dl = ScreenCopy { title: "Downloading update 0.1.1".into(), bar: Some(7_300_000.0 / 12_000_000.0), meta: s("7.3 MB of 12.0 MB"), ghost: true, ..Default::default() };
        assert_eq!(copy(Phase::Downloading { version: v(), done: 7_300_000, total: 12_000_000 }, Kind::Auto), dl);
        assert_eq!(copy(Phase::Downloading { version: v(), done: 5, total: 0 }, Kind::Auto).bar, Some(0.0));
        let inst = ScreenCopy { title: "Installing update".into(), bar: Some(1.0), meta: s("Rustcord 0.1.1"), ..Default::default() };
        assert_eq!(copy(Phase::Installing { version: v() }, Kind::Auto), inst);
        let restart = ScreenCopy { title: "Restarting".into(), bar: Some(1.0), meta: s("Rustcord 0.1.1 is starting."), ..Default::default() };
        assert_eq!(copy(Phase::Restarting { version: v() }, Kind::Auto), restart);
        let continues = s("Rustcord continues in a few seconds.");
        let err = ScreenCopy { title: "Update failed".into(), error: s("Boom."), meta: continues, primary: Some(("Continue", Action::Dismiss)), ..Default::default() };
        assert_eq!(copy(Phase::Failed { message: "Boom.".into() }, Kind::Auto), err);
        let deb = ScreenCopy {
            title: "Update 0.1.1 is ready".into(),
            meta: s("Installing it asks for your password."),
            primary: Some(("Install", Action::Install)),
            ghost: true,
            ..Default::default()
        };
        assert_eq!(copy(Phase::Ready { version: v() }, Kind::Package), deb);
        let manual = ScreenCopy {
            title: "Update 0.1.1 is downloaded".into(),
            meta: s("Install it with your package manager:"),
            path: s("/x/rustcord_0.1.1_amd64.deb"),
            primary: Some(("Continue", Action::Dismiss)),
            ..Default::default()
        };
        assert_eq!(copy(Phase::Manual { version: v(), path: PathBuf::from("/x/rustcord_0.1.1_amd64.deb") }, Kind::Package), manual);
        // Not reached in the normal flow.
        assert_eq!(copy(Phase::Ready { version: v() }, Kind::Auto), inst);
        assert_eq!(copy(Phase::Checking, Kind::Auto), ScreenCopy { title: "Checking for updates".into(), bar: Some(0.0), ..Default::default() });
        assert_eq!(copy(Phase::Available { version: v() }, Kind::Auto).title, "Downloading update 0.1.1");
    }

    #[test]
    fn busy_means_a_call_or_a_draft_in_the_tray() {
        use crate::model::{Id, User};
        let (mut state, mut view) = (AppState::default(), ViewState::default());
        assert!(!busy(&state, &view, false) && !busy(&state, &view, true));
        state.voice.channel_id = Some(Id(7));
        assert!(busy(&state, &view, false), "in (or joining) voice");
        state.voice.channel_id = None;
        state.me = Some(User { id: Id(1), ..Default::default() });
        state.calls.ringing.insert(Id(9), vec![Id(1)]);
        assert!(busy(&state, &view, false), "a call rings me");
        state.calls.ringing.clear();
        state.start_call(Id(5), false, std::time::Instant::now());
        assert!(busy(&state, &view, false), "a call I just started, before the voice task joins");
        state.call_steps(std::time::Instant::now() + std::time::Duration::from_secs(16));
        assert!(!busy(&state, &view, false), "it never joined: dropped");
        view.drafts.insert(Id(3), "  ".into());
        assert!(!busy(&state, &view, true), "blank draft");
        view.drafts.insert(Id(4), "half a sentence".into());
        assert!(busy(&state, &view, true), "a restart would lose it");
        assert!(!busy(&state, &view, false), "visible: the user sees the draft");
    }
}
