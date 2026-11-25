//! macOS: the menu bar icon (tray-icon, an NSStatusItem made on the main thread) with Open
//! Rustcord / Mute / Deafen / Quit Rustcord, and notifications through the User Notifications
//! framework (only from the .app bundle: without a bundle id the framework raises).
//! In the tray the app hides itself; a Dock click or a notification click activates it again,
//! which shows the window (winit has no reopen event).
//! UNCONFIRMED: written against objc2's generated bindings, only `cargo check`ed (no Mac here).

use super::{PAUSED, TrayAction, VOICE, push};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::MainThreadMarker;
use objc2_foundation::NSObjectProtocol;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering::SeqCst};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};

const NOT_STARTED: u8 = 0;
const RUNNING: u8 = 2;
const FAILED: u8 = 3;
static STATE: AtomicU8 = AtomicU8::new(NOT_STARTED);
/// The window waits in the menu bar (`to_tray` .. `shown`).
static IN_TRAY: AtomicBool = AtomicBool::new(false);

struct Tray {
    _icon: tray_icon::TrayIcon,
    mute: CheckMenuItem,
    deafen: CheckMenuItem,
    resume: MenuItem,
    _observer: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

thread_local! {
    /// The main thread's menu bar icon.
    static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
}

/// On the main thread (eframe's creator closure).
pub fn start_tray() {
    let tray = build();
    STATE.store(if tray.is_some() { RUNNING } else { FAILED }, SeqCst);
    TRAY.with(|t| *t.borrow_mut() = tray);
}

fn build() -> Option<Tray> {
    let open = MenuItem::with_id("open", "Open Rustcord", true, None);
    let mute = CheckMenuItem::with_id("mute", "Mute", true, false, None);
    let deafen = CheckMenuItem::with_id("deafen", "Deafen", true, false, None);
    let pause_hour = MenuItem::with_id("pause-hour", "Pause notifications for 1 hour", true, None);
    let pause_tomorrow = MenuItem::with_id("pause-tomorrow", "Pause notifications until tomorrow", true, None);
    let resume = MenuItem::with_id("resume", "Resume notifications", PAUSED.load(SeqCst), None);
    let quit = MenuItem::with_id("quit", "Quit Rustcord", true, None);
    let menu = Menu::with_items(&[&open, &mute, &deafen, &PredefinedMenuItem::separator(), &pause_hour, &pause_tomorrow, &resume, &PredefinedMenuItem::separator(), &quit]).ok()?;
    MenuEvent::set_event_handler(Some(|e: MenuEvent| match e.id.0.as_str() {
        "open" => push(TrayAction::Show),
        "mute" => push(TrayAction::ToggleMute),
        "deafen" => push(TrayAction::ToggleDeafen),
        "pause-hour" => push(TrayAction::PauseHour),
        "pause-tomorrow" => push(TrayAction::PauseTomorrow),
        "resume" => push(TrayAction::Resume),
        "quit" => push(TrayAction::Quit),
        _ => {}
    }));
    // The flat mark in black: a template image, so the menu bar tints it.
    let img = image::load_from_memory(include_bytes!("../../assets/icons/rustcord-tray-36.png")).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    let icon = tray_icon::Icon::from_rgba(img.into_raw(), w, h).ok()?;
    let built = tray_icon::TrayIconBuilder::new().with_menu(Box::new(menu)).with_icon_templated(icon).with_tooltip(crate::config::APP_NAME).build();
    let icon = built.map_err(|e| eprintln!("rustcord: no menu bar icon: {e}")).ok()?;
    Some(Tray { _icon: icon, mute, deafen, resume, _observer: observe_activation() })
}

/// The app became active (Dock icon, notification, app switcher) while in the tray: show.
fn observe_activation() -> Retained<ProtocolObject<dyn NSObjectProtocol>> {
    use objc2_foundation::{NSNotification, NSNotificationCenter, NSOperationQueue};
    let block = block2::RcBlock::new(|_: std::ptr::NonNull<NSNotification>| {
        if IN_TRAY.load(SeqCst) {
            push(TrayAction::Show);
        }
    });
    // SAFETY: AppKit's own notification name; the block only touches atomics and the action queue.
    unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(objc2_app_kit::NSApplicationDidBecomeActiveNotification),
            None,
            Some(&NSOperationQueue::mainQueue()),
            &block,
        )
    }
}

pub fn has_tray() -> bool {
    STATE.load(SeqCst) == RUNNING
}

pub fn tray_failed() -> bool {
    STATE.load(SeqCst) == FAILED
}

pub fn menu_changed() {
    let voice = VOICE.load(SeqCst);
    TRAY.with(|t| {
        if let Some(t) = t.borrow().as_ref() {
            t.mute.set_checked(voice & 1 != 0);
            t.deafen.set_checked(voice & 2 != 0);
            t.resume.set_enabled(PAUSED.load(SeqCst));
        }
    });
}

/// Hides the app (its windows and Dock focus); activating it again shows the window.
pub fn to_tray() {
    IN_TRAY.store(true, SeqCst);
    if let Some(mtm) = MainThreadMarker::new() {
        objc2_app_kit::NSApplication::sharedApplication(mtm).hide(None);
    }
}

pub fn shown() {
    IN_TRAY.store(false, SeqCst);
    if let Some(mtm) = MainThreadMarker::new() {
        objc2_app_kit::NSApplication::sharedApplication(mtm).unhide(None);
    }
}

/// One notification (on the "rustcord-notify" thread).
pub fn deliver(title: &str, body: &str) {
    use objc2_foundation::{NSBundle, NSString};
    use objc2_user_notifications::{UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest, UNUserNotificationCenter};
    // `cargo run` builds have no bundle id, and the notification center raises without one.
    if NSBundle::mainBundle().bundleIdentifier().is_none() {
        return;
    }
    static ASKED: std::sync::Once = std::sync::Once::new();
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let center = UNUserNotificationCenter::currentNotificationCenter();
    ASKED.call_once(|| {
        let done = block2::RcBlock::new(|_: objc2::runtime::Bool, _: *mut objc2_foundation::NSError| {});
        center.requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound, &done);
    });
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    let id = NSString::from_str(&format!("rustcord-{}", NEXT.fetch_add(1, SeqCst)));
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&id, &content, None);
    center.addNotificationRequest_withCompletionHandler(&request, None);
}
