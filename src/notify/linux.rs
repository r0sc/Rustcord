//! Linux: the tray icon as a StatusNotifierItem (ksni, its own D-Bus thread; KDE, Ubuntu's
//! GNOME, XFCE …) and notifications over `org.freedesktop.Notifications` (zbus). Without a tray
//! host (GNOME without the AppIndicator extension) there is no icon: closing the window quits.

use super::{TrayAction, VOICE, push};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU8, Ordering::SeqCst};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const NOT_STARTED: u8 = 0;
const STARTING: u8 = 1;
const RUNNING: u8 = 2;
const FAILED: u8 = 3;
static STATE: AtomicU8 = AtomicU8::new(NOT_STARTED);
static HANDLE: Mutex<Option<ksni::blocking::Handle<RustcordTray>>> = Mutex::new(None);

pub fn start_tray() {
    if STATE.compare_exchange(NOT_STARTED, STARTING, SeqCst, SeqCst).is_err() {
        return;
    }
    // Pure Wayland (no X11 to force, main.rs): a hidden window could never come back.
    if std::env::var_os("DISPLAY").is_none() || std::thread::Builder::new().name("rustcord-tray".into()).spawn(run).is_err() {
        STATE.store(FAILED, SeqCst);
    }
}

pub fn has_tray() -> bool {
    matches!(STATE.load(SeqCst), STARTING | RUNNING)
}

pub fn tray_failed() -> bool {
    STATE.load(SeqCst) == FAILED
}

/// The menu re-reads the check marks from `VOICE`.
pub fn menu_changed() {
    if let Some(h) = HANDLE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        h.update(|_| ());
    }
}

fn run() {
    use ksni::blocking::TrayMethods;
    let pngs: [&[u8]; 2] = [include_bytes!("../../assets/icons/rustcord-32.png"), include_bytes!("../../assets/icons/rustcord-64.png")];
    let icons: Vec<ksni::Icon> = pngs.into_iter().filter_map(argb).collect();
    // Started at login, the panel may not be up yet: try for 10 s.
    for _ in 0..10 {
        match (RustcordTray { icons: icons.clone() }).spawn() {
            Ok(h) => {
                *HANDLE.lock().unwrap_or_else(|e| e.into_inner()) = Some(h);
                STATE.store(RUNNING, SeqCst);
                return;
            }
            Err(ksni::Error::Dbus(e)) => {
                eprintln!("rustcord: no tray icon: {e}");
                break;
            }
            Err(_) => std::thread::sleep(Duration::from_secs(1)),
        }
    }
    STATE.store(FAILED, SeqCst);
    // A window waiting in the tray comes back (`user_settings::logic`).
    super::wake();
}

/// ARGB32 in network byte order, as StatusNotifierItem wants it.
fn argb(png: &[u8]) -> Option<ksni::Icon> {
    let img = image::load_from_memory(png).ok()?.into_rgba8();
    let (width, height) = (img.width() as i32, img.height() as i32);
    let mut data = img.into_raw();
    for px in data.as_chunks_mut::<4>().0 {
        px.rotate_right(1);
    }
    Some(ksni::Icon { width, height, data })
}

struct RustcordTray {
    icons: Vec<ksni::Icon>,
}

impl ksni::Tray for RustcordTray {
    fn id(&self) -> String {
        "rustcord".into()
    }

    fn title(&self) -> String {
        crate::config::APP_NAME.into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icons.clone()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: crate::config::APP_NAME.into(), ..Default::default() }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        push(TrayAction::Show);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{CheckmarkItem, StandardItem};
        let voice = VOICE.load(SeqCst);
        let item = |label: &str, action: TrayAction| -> ksni::MenuItem<Self> {
            StandardItem { label: label.into(), activate: Box::new(move |_: &mut Self| push(action)), ..Default::default() }.into()
        };
        let check = |label: &str, on: bool, action: TrayAction| -> ksni::MenuItem<Self> {
            CheckmarkItem { label: label.into(), checked: on, activate: Box::new(move |_: &mut Self| push(action)), ..Default::default() }.into()
        };
        vec![
            item("Open Rustcord", TrayAction::Show),
            check("Mute", voice & 1 != 0, TrayAction::ToggleMute),
            check("Deafen", voice & 2 != 0, TrayAction::ToggleDeafen),
            ksni::MenuItem::Separator,
            item("Pause notifications for 1 hour", TrayAction::PauseHour),
            item("Pause notifications until tomorrow", TrayAction::PauseTomorrow),
            StandardItem {
                label: "Resume notifications".into(),
                enabled: super::PAUSED.load(SeqCst),
                activate: Box::new(|_: &mut Self| push(TrayAction::Resume)),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            item("Quit Rustcord", TrayAction::Quit),
        ]
    }
}

/// Ids of the last notifications sent: a click on one of them opens the window.
static SENT: Mutex<VecDeque<u32>> = Mutex::new(VecDeque::new());

/// The session bus, connected on the first notification (None: no bus, notifications off).
fn bus() -> Option<&'static zbus::blocking::Connection> {
    static BUS: OnceLock<Option<zbus::blocking::Connection>> = OnceLock::new();
    BUS.get_or_init(|| {
        let conn = zbus::blocking::Connection::session().ok()?;
        let listen = conn.clone();
        let _ = std::thread::Builder::new().name("rustcord-notify-click".into()).spawn(move || clicks(&listen));
        Some(conn)
    })
    .as_ref()
}

/// One notification (on the "rustcord-notify" thread). Its "default" action opens the window.
pub fn deliver(title: &str, body: &str) {
    let Some(conn) = bus() else { return };
    let hints = std::collections::HashMap::from([("desktop-entry", zbus::zvariant::Value::from("rustcord"))]);
    let args = (crate::config::APP_NAME, 0u32, "rustcord", title, escape_markup(body), vec!["default", "Open"], hints, -1i32);
    let reply = conn.call_method(Some("org.freedesktop.Notifications"), "/org/freedesktop/Notifications", Some("org.freedesktop.Notifications"), "Notify", &args);
    if let Ok(id) = reply.and_then(|m| m.body().deserialize::<u32>()) {
        let mut sent = SENT.lock().unwrap_or_else(|e| e.into_inner());
        if sent.len() == 20 {
            sent.pop_front();
        }
        sent.push_back(id);
    }
}

fn clicks(conn: &zbus::blocking::Connection) {
    let proxy = zbus::blocking::Proxy::new(conn, "org.freedesktop.Notifications", "/org/freedesktop/Notifications", "org.freedesktop.Notifications");
    let Ok(signals) = proxy.and_then(|p| p.receive_signal("ActionInvoked")) else { return };
    for msg in signals {
        if let Ok((id, _action)) = msg.body().deserialize::<(u32, String)>()
            && SENT.lock().unwrap_or_else(|e| e.into_inner()).contains(&id)
        {
            push(TrayAction::Notification);
        }
    }
}

/// Servers with `body-markup` would render message text as markup.
fn escape_markup(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_body_is_escaped() {
        assert_eq!(escape_markup("<b>&"), "&lt;b&gt;&amp;");
        assert_eq!(escape_markup("plain"), "plain");
    }

    #[test]
    fn tray_icons_are_argb() {
        let icon = argb(include_bytes!("../../assets/icons/rustcord-32.png")).unwrap();
        assert_eq!((icon.width, icon.height, icon.data.len()), (32, 32, 32 * 32 * 4));
        let rgba = image::load_from_memory(include_bytes!("../../assets/icons/rustcord-32.png")).unwrap().into_rgba8().into_raw();
        let i = rgba.as_chunks::<4>().0.iter().position(|p| p[3] == 255).unwrap() * 4;
        assert_eq!(icon.data[i..i + 4], [rgba[i + 3], rgba[i], rgba[i + 1], rgba[i + 2]]);
    }
}
