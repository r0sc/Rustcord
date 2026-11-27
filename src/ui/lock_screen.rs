//! The lock screen (docs/DESIGN.md §12 "Lock screen"): replaces the main window under the title
//! bar while `crate::lock::locked()`. Laid out like the update screen: the flat frame colour, one
//! centred column, no card. Locking and unlocking are instant (a fade would show the private
//! screen for longer); a wrong PIN shakes the column like the login card. It asks for no frames
//! while idle. The PIN lives only in a zeroized buffer here and is checked in `crate::lock`.

use super::login::{app_icon, field, primary_button};
use super::messages::{Btn, Pal, button};
use super::{UiCx, dialog, dialog_buttons, dialog_text, motion};
use egui::{Align, Direction, Frame, Key, Layout, Rect, RichText, Sense, TextEdit, UiBuilder, Vec2, pos2, vec2};
use std::sync::{LazyLock, Mutex};
use std::time::Instant;
use zeroize::{Zeroize, Zeroizing};

static INPUT: LazyLock<Mutex<Zeroizing<String>>> = LazyLock::new(|| Mutex::new(Zeroizing::new(String::new())));

#[derive(Clone, Copy, PartialEq, Eq)]
enum Problem {
    Wrong,
    NoPin,
    #[cfg_attr(not(windows), allow(dead_code))]
    Hello,
}

struct State {
    problem: Option<Problem>,
    /// The "Sign out and remove the lock?" confirm is open.
    forgot: bool,
    /// The demo hooks were applied.
    seeded: bool,
}

static STATE: Mutex<State> = Mutex::new(State { problem: None, forgot: false, seeded: false });

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Demo runs (`RUSTCORD_NAV_DEMO=lock[:wrong|:hello|:forgot]`) never open the OS prompt.
fn demo() -> Option<&'static str> {
    super::sidebar::nav_demo("lock").then_some("").or_else(|| super::sidebar::nav_demo_value("lock"))
}

pub(super) fn screen(ui: &mut egui::Ui, cx: &mut UiCx<'_>) {
    let ctx = ui.ctx().clone();
    let pal = Pal::new(cx.theme);
    {
        let mut st = state();
        if !std::mem::replace(&mut st.seeded, true) {
            match demo() {
                Some("wrong") => st.problem = Some(Problem::Wrong),
                Some("forgot") => st.forgot = true,
                _ => {}
            }
        }
    }
    let hello = hello_available(&ctx, cx);
    #[cfg(windows)]
    {
        use crate::lock::hello::{self, Outcome};
        match hello::take_result() {
            Some(Outcome::Verified) => {
                crate::lock::unlock();
                ctx.request_repaint();
                return;
            }
            Some(Outcome::Failed) => state().problem = Some(Problem::Hello),
            _ => {}
        }
        // Once per lock, the first time the screen is drawn while the window has focus.
        if hello && demo().is_none() && std::env::var_os("RUSTCORD_SCREENSHOT").is_none() && cx.view.window_focused && crate::lock::hello_due() {
            hello::prompt(&ctx);
        }
    }
    let now = Instant::now();
    let wait = crate::lock::wait(now);
    if let Some(d) = wait {
        // One wake when the pause is over, no countdown.
        ctx.request_repaint_after(d);
    }
    let problem = state().problem;
    let message = match (wait, problem) {
        (Some(_), _) => Some("Too many tries. Wait 30 seconds, then try again."),
        (None, Some(Problem::Wrong)) => Some("Wrong PIN. Try again."),
        (None, Some(Problem::NoPin)) => Some("Rustcord can't find your PIN. Sign out to remove the lock."),
        (None, Some(Problem::Hello)) => Some("Windows Hello didn't unlock Rustcord."),
        (None, None) => None,
    };
    let page = Frame::new().fill(cx.theme.colors.guild_bar.to_color32());
    let mut acts = Acts::default();
    egui::CentralPanel::default().frame(page).show(ui, |ui| {
        ui.style_mut().interaction.selectable_labels = false;
        let avail = ui.available_rect_before_wrap();
        // Centred on the column's height from the pass before (one discarded pass when it changes).
        let height_id = ui.make_persistent_id("rc_lock_column_h");
        let h: f32 = ui.data(|d| d.get_temp(height_id)).unwrap_or(0.0);
        let col_w = 320.0f32.min(avail.width() - 32.0).max(120.0);
        let top = avail.top() + ((avail.height() - h) / 2.0).max(16.0);
        let col = Rect::from_min_max(pos2(avail.center().x - col_w / 2.0, top), pos2(avail.center().x + col_w / 2.0, avail.bottom().max(top)));
        // A wrong PIN: 3 swings of 6 px, decaying over LARGE, like the login card (principle 9).
        let t = motion::anim(&ctx, ui.make_persistent_id("rc_lock_shake"), problem == Some(Problem::Wrong) && wait.is_none(), motion::LARGE, 0.0);
        let dx = 6.0 * (t * 6.0 * std::f32::consts::PI).sin() * (1.0 - t);
        let out = ui.scope_builder(UiBuilder::new().max_rect(col).layout(Layout::top_down(Align::Center)), |ui| {
            ui.with_visual_transform(egui::emath::TSTransform::from_translation(vec2(dx, 0.0)), |ui| {
                column(ui, cx, &pal, message, wait.is_some(), hello, &mut acts)
            })
        });
        let used = out.response.rect.height();
        if (used - h).abs() > 0.5 {
            ui.data_mut(|d| d.insert_temp(height_id, used));
            ctx.request_discard("lock screen height");
        }
    });
    if acts.retry {
        let found = crate::lock::reload();
        state().problem = (!found).then_some(Problem::NoPin);
    }
    #[cfg(windows)]
    if acts.hello {
        state().problem = None;
        crate::lock::hello::prompt(&ctx);
    }
    #[cfg(not(windows))]
    let _ = acts.hello;
    if acts.forgot {
        state().forgot = true;
    }
    if acts.submit {
        // The buffer is wiped whatever the answer is.
        let pin = {
            let mut buf = INPUT.lock().unwrap_or_else(|e| e.into_inner());
            let pin = buf.clone();
            buf.zeroize();
            pin
        };
        match crate::lock::try_pin(&pin, now) {
            crate::lock::Check::Unlocked => {
                *state() = State { problem: None, forgot: false, seeded: true };
                ctx.request_repaint();
            }
            crate::lock::Check::Wrong => state().problem = Some(Problem::Wrong),
            crate::lock::Check::Wait(_) => ctx.request_repaint(),
            crate::lock::Check::NoPin => state().problem = Some(Problem::NoPin),
        }
    }
    if state().forgot {
        confirm_forgot(&ctx, cx);
    }
}

/// What the column's buttons asked for.
#[derive(Default)]
struct Acts {
    submit: bool,
    hello: bool,
    forgot: bool,
    retry: bool,
}

/// Windows Hello is on for the lock and the OS has it set up (the demo shows the button as well).
fn hello_available(ctx: &egui::Context, cx: &UiCx<'_>) -> bool {
    #[cfg(windows)]
    {
        cx.settings.app_lock.hello && (demo() == Some("hello") || crate::lock::hello::available(ctx) == Some(true))
    }
    #[cfg(not(windows))]
    {
        let _ = (ctx, cx);
        false
    }
}

/// Logo, title, the PIN field, the error, Unlock, Windows Hello and "Forgot your PIN?" (§12 gaps).
fn column(ui: &mut egui::Ui, cx: &mut UiCx<'_>, pal: &Pal, message: Option<&str>, paused: bool, hello: bool, acts: &mut Acts) {
    ui.spacing_mut().item_spacing = vec2(12.0, 0.0);
    if let Some(tex) = app_icon(ui.ctx()) {
        ui.add(egui::Image::new(&tex).fit_to_exact_size(Vec2::splat(56.0)));
    }
    ui.add_space(24.0);
    ui.add(egui::Label::new(pal.display_text("Rustcord is locked")).wrap());
    ui.add_space(8.0);
    ui.label(RichText::new("Enter your PIN.").size(pal.ui()).color(pal.muted));
    ui.add_space(24.0);
    let no_pin = state().problem == Some(Problem::NoPin);
    let mut buf = INPUT.lock().unwrap_or_else(|e| e.into_inner());
    let edit = TextEdit::singleline(&mut **buf).password(true).char_limit(16).hint_text("PIN").id(ui.make_persistent_id("rc_lock_pin"));
    let (resp, _) = ui.add_enabled_ui(!paused && !no_pin, |ui| ui.allocate_ui(vec2(320.0, 40.0), |ui| field(ui, cx.theme, edit, None, false)).inner).inner;
    if resp.changed() {
        // Digits only, and a new try clears the error.
        buf.retain(|c| c.is_ascii_digit());
        let mut st = state();
        if st.problem != Some(Problem::NoPin) {
            st.problem = None;
        }
    }
    let ready = crate::lock::pin_ok(&buf);
    drop(buf);
    // The field has focus when the screen appears (and again after a wrong PIN).
    if !paused && !no_pin && ui.memory(|m| m.focused().is_none()) {
        resp.request_focus();
    }
    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    if let Some(m) = message {
        ui.add_space(8.0);
        ui.add(egui::Label::new(RichText::new(m).size(pal.meta()).color(pal.danger)).wrap());
    }
    ui.add_space(16.0);
    let size = vec2(320.0f32.min(ui.available_width()), 40.0);
    if no_pin {
        let retry = ui.allocate_ui(size, |ui| ui.with_layout(Layout::centered_and_justified(Direction::TopDown), |ui| button(ui, pal, Btn::Secondary, "Try again", true))).inner.inner;
        acts.retry = retry.clicked();
    } else {
        let unlock = ui.allocate_ui(size, |ui| primary_button(ui, cx, "Unlock", ready && !paused)).inner;
        acts.submit = (unlock.clicked() || enter) && ready && !paused;
    }
    if hello {
        ui.add_space(8.0);
        let click = ui.allocate_ui(size, |ui| ui.with_layout(Layout::centered_and_justified(Direction::TopDown), |ui| button(ui, pal, Btn::Secondary, "Use Windows Hello", true))).inner.inner;
        acts.hello = click.clicked();
    }
    ui.add_space(16.0);
    let link = ui.add(egui::Label::new(RichText::new("Forgot your PIN?").size(pal.meta()).color(pal.link)).sense(Sense::click()));
    acts.forgot = link.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
}

/// "Sign out and remove the lock?": the way out that never locks anyone out.
fn confirm_forgot(ctx: &egui::Context, cx: &mut UiCx<'_>) {
    let theme = &*cx.theme;
    let resp = dialog(ctx, theme, "rc_lock_forgot", 440.0, "Sign out and remove the lock?", |ui| {
        dialog_text(ui, theme, "Rustcord signs you out, deletes the messages and images it saved on this computer, and turns off the app lock. Your Discord account isn't affected.");
        dialog_buttons(ui, theme, "Sign out", true, true)
    });
    match resp.inner {
        Some(true) => forget(cx),
        Some(false) => state().forgot = false,
        None if resp.should_close() => state().forgot = false,
        None => {}
    }
}

/// Removes the PIN, turns the lock off and signs out (the cache is cleared by the logout).
fn forget(cx: &mut UiCx<'_>) {
    // An error is ignored: the lock goes off anyway, so nobody stays locked out.
    let _ = crate::lock::clear_pin();
    cx.settings.app_lock = crate::config::AppLock::default();
    let _ = cx.settings.save();
    crate::lock::unlock();
    *state() = State { problem: None, forgot: false, seeded: true };
    super::user_settings::logout(cx);
}
