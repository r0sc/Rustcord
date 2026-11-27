//! Windows / Linux / macOS settings > App lock (crate::lock): a PIN (and Windows Hello on
//! Windows) before the window shows. It is a screen lock: the copy says so, the cache on disk is
//! not encrypted. The PIN is only ever typed into the dialog's zeroized buffers.

use super::{Pal, choice_row, field_label, section, switch_row};
use crate::config::AppLock;
use crate::ui::login::field;
use crate::ui::messages::{Btn, button};
use crate::ui::{UiCx, dialog_footer};
use egui::{RichText, TextEdit};
use zeroize::{Zeroize, Zeroizing};

/// The "Set a PIN" dialog's state; dropped with the layer (`opened`), which wipes the buffers.
#[derive(Default)]
pub(super) struct Form {
    open: bool,
    pin: Zeroizing<String>,
    again: Zeroizing<String>,
    error: Option<String>,
}

const OFF_META: &str = "Asks for your PIN when Rustcord starts. It only hides the window: messages and images cached on this computer are not encrypted.";

/// True when `cx.settings` changed (the caller saves).
pub(super) fn show(ui: &mut egui::Ui, cx: &mut UiCx<'_>) -> bool {
    // Test hook `RUSTCORD_NAV_DEMO=lock-setup`: the dialog, once.
    static DEMO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if crate::ui::sidebar::nav_demo("lock-setup") && !DEMO.swap(true, std::sync::atomic::Ordering::Relaxed) {
        cx.view.user_settings_ui.app_lock.open = true;
    }
    section(ui, cx.theme, "App lock");
    let mut changed = false;
    let mut on = cx.settings.app_lock.enabled;
    if switch_row(ui, cx.theme, "Lock Rustcord with a PIN", Some(OFF_META), &mut on) {
        if on {
            // The switch stays off until a PIN is saved.
            cx.view.user_settings_ui.app_lock = Form { open: true, ..Default::default() };
        } else {
            // Whoever turns it off already has the app open.
            let _ = crate::lock::clear_pin();
            cx.settings.app_lock.enabled = false;
            cx.settings.app_lock.hello = false;
            cx.view.toast("App lock is off", false);
            changed = true;
        }
    }
    if cx.settings.app_lock.enabled {
        let theme = &*cx.theme;
        let s = &mut cx.settings.app_lock;
        #[cfg(windows)]
        {
            let ok = crate::lock::hello::available(ui.ctx()) == Some(true);
            let meta = if ok { "Your face, fingerprint or Windows PIN. Your Rustcord PIN works too." } else { "Windows Hello isn't set up on this PC." };
            changed |= ui.add_enabled_ui(ok, |ui| switch_row(ui, theme, "Unlock with Windows Hello", Some(meta), &mut s.hello)).inner;
        }
        let times = [(0u16, "Never"), (1, "1 minute"), (5, "5 minutes"), (15, "15 minutes"), (30, "30 minutes"), (60, "1 hour")];
        changed |= choice_row(ui, theme, "rc_lock_after", "Lock after", Some("When you haven't used Rustcord for this long."), &mut s.idle_minutes, &times);
        let tray = if cfg!(target_os = "macos") { "Lock when hidden in the menu bar" } else { "Lock when hidden in the tray" };
        changed |= switch_row(ui, theme, tray, Some("Opening Rustcord again asks for your PIN."), &mut s.on_tray);
    }
    if cx.view.user_settings_ui.app_lock.open {
        changed |= dialog(ui.ctx(), cx);
    }
    changed
}

/// "Set a PIN": two password fields, digits only, Enter in the second one saves. True when the
/// lock was turned on (the settings changed).
fn dialog(ctx: &egui::Context, cx: &mut UiCx<'_>) -> bool {
    let theme = &*cx.theme;
    let pal = Pal::new(theme);
    let f = &mut cx.view.user_settings_ui.app_lock;
    let mut save = false;
    let resp = crate::ui::dialog(ctx, theme, "rc_set_pin", 440.0, "Set a PIN", |ui| {
        crate::ui::dialog_text(ui, theme, "You'll enter it when Rustcord starts and when it locks. If you forget it, you can sign out and sign in again.");
        ui.add_space(16.0);
        for (label, id) in [("PIN", "rc_pin_a"), ("Repeat PIN", "rc_pin_b")] {
            field_label(ui, &pal, label);
            let buf = if id == "rc_pin_a" { &mut f.pin } else { &mut f.again };
            let (r, _) = field(ui, theme, TextEdit::singleline(&mut **buf).password(true).char_limit(16).id(ui.make_persistent_id(id)), None, false);
            if r.changed() {
                buf.retain(|c| c.is_ascii_digit());
            }
            if id == "rc_pin_a" && r.changed() {
                f.error = None;
            }
            if id == "rc_pin_b" && r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                save = true;
            }
            ui.add_space(8.0);
        }
        if let Some(e) = &f.error {
            ui.add(egui::Label::new(RichText::new(e).size(pal.meta()).color(pal.danger)).wrap());
        }
        ui.add_space(8.0);
        dialog_footer(ui, |ui| {
            if button(ui, &pal, Btn::Primary, "Set PIN", true).clicked() {
                save = true;
            }
            button(ui, &pal, Btn::Ghost, "Cancel", true).clicked()
        })
    });
    let cancel = resp.inner || resp.should_close();
    if save {
        let f = &mut cx.view.user_settings_ui.app_lock;
        f.error = if !crate::lock::pin_ok(&f.pin) {
            Some("Use 4 to 16 digits.".into())
        } else if *f.pin != *f.again {
            Some("The two PINs are different.".into())
        } else {
            crate::lock::set_pin(&f.pin).err().map(|e| format!("Rustcord can't save the PIN in your system's password store: {e}"))
        };
        if f.error.is_none() {
            *f = Form::default();
            cx.settings.app_lock = AppLock { enabled: true, ..cx.settings.app_lock.clone() };
            crate::lock::unlock();
            cx.view.toast("App lock is on", false);
            return true;
        }
    } else if cancel {
        let f = &mut cx.view.user_settings_ui.app_lock;
        f.pin.zeroize();
        f.again.zeroize();
        *f = Form::default();
    }
    false
}
