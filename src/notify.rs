//! Desktop integration: the notification-area (tray) icon, balloon / toast notifications and
//! the single-instance check.
//!
//! Windows (windows-sys only, no extra crates), on the thread "rustcord-tray", which owns a
//! hidden window:
//! - The icon (the flat mark, icon resource 2) shows while Rustcord runs. A left click, or a
//!   click on a balloon, opens the window; the right-click menu has Open Rustcord, Mute, Deafen,
//!   the notification pause (1 hour, until tomorrow, Resume)
//!   and Quit Rustcord. Clicks reach the UI thread as `TrayAction`s plus a repaint
//!   (`take_actions`, read in `App::logic`, which also runs while the window is hidden).
//! - Balloons are rate limited: one at a time, the rest summed up (`Coalescer`).
//! - Single instance per data dir: a named mutex (`claim_instance`); a second start posts
//!   "show" (or "quit", `--quit`) to the running one's tray window (`signal_running`).
//!
//! Linux and macOS (`notify/unix.rs`): single instance over a Unix socket, notifications on the
//! thread "rustcord-notify" with the same rate limit. The tray: Linux a StatusNotifierItem
//! (`notify/linux.rs`, ksni; none without a tray host, closing then quits), macOS the menu bar
//! icon (`notify/macos.rs`, tray-icon, on the main thread).
//!
//! OWNERSHIP: desktop engineer.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as os;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as os;
#[cfg(unix)]
mod unix;

/// A tray click (or a message from a second start) for the UI thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    /// A click on one of Rustcord's notifications (Windows, Linux; macOS reports `Show`).
    #[cfg_attr(target_os = "macos", allow(dead_code, reason = "macOS reports a notification click as Show"))]
    Notification,
    ToggleMute,
    ToggleDeafen,
    PauseHour,
    PauseTomorrow,
    Resume,
    Quit,
}

static ACTIONS: Mutex<Vec<TrayAction>> = Mutex::new(Vec::new());
static WAKE: OnceLock<egui::Context> = OnceLock::new();
/// The panel's voice toggles, for the menu's check marks: bit 0 muted, bit 1 deafened.
static VOICE: AtomicU8 = AtomicU8::new(0);
/// Notifications are paused by hand: the tray menu's Resume is enabled.
static PAUSED: AtomicBool = AtomicBool::new(false);

/// Puts the icon in the notification area / menu bar (once, at startup, on the main thread).
/// Tray clicks wake `ctx`.
pub fn start(ctx: &egui::Context) {
    let _ = WAKE.set(ctx.clone());
    #[cfg(windows)]
    imp::start();
    #[cfg(unix)]
    unix::start();
}

/// The tray icon is up (or coming up), so a closed window can wait there.
pub fn has_tray() -> bool {
    #[cfg(windows)]
    {
        imp::has_tray()
    }
    #[cfg(unix)]
    {
        os::has_tray()
    }
}

/// The tray was tried and is not there (Linux without a tray host): closing quits.
pub fn tray_failed() -> bool {
    #[cfg(windows)]
    {
        imp::failed()
    }
    #[cfg(unix)]
    {
        os::tray_failed()
    }
}

/// The window just went to the tray (macOS: the app hides, so a Dock click brings it back).
pub fn to_tray_os() {
    #[cfg(target_os = "macos")]
    macos::to_tray();
}

/// The window is about to show again (macOS: the app unhides).
pub fn shown_os() {
    #[cfg(target_os = "macos")]
    macos::shown();
}

/// Show a notification (non-blocking; only once `start` ran). Title/body are truncated to the
/// platform limits.
pub fn show(title: &str, body: &str) {
    #[cfg(windows)]
    imp::show(title, body);
    #[cfg(unix)]
    unix::show(title, body);
}

/// Remove the tray icon (call on exit).
pub fn shutdown() {
    #[cfg(windows)]
    imp::shutdown();
    #[cfg(unix)]
    unix::shutdown();
}

/// Tray clicks since the last call, oldest first.
pub fn take_actions() -> Vec<TrayAction> {
    std::mem::take(&mut *ACTIONS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// The panel's mute / deafen state, shown as check marks in the tray menu.
pub fn set_voice_state(muted: bool, deafened: bool) {
    let bits = u8::from(muted) | (u8::from(deafened) << 1);
    let changed = VOICE.swap(bits, Ordering::Relaxed) != bits;
    #[cfg(unix)]
    if changed {
        os::menu_changed();
    }
    #[cfg(windows)]
    let _ = changed;
}

/// Whether notifications are paused by hand, for the tray menu's Resume item.
pub fn set_paused(on: bool) {
    let changed = PAUSED.swap(on, Ordering::Relaxed) != on;
    #[cfg(unix)]
    if changed {
        os::menu_changed();
    }
    #[cfg(windows)]
    let _ = changed;
}

fn push(action: TrayAction) {
    ACTIONS.lock().unwrap_or_else(|e| e.into_inner()).push(action);
    wake();
}

/// A repaint, so `App::logic` runs (also while the window is hidden).
fn wake() {
    if let Some(ctx) = WAKE.get() {
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
}

/// At most one notification per this interval; messages in between are summarized.
const MIN_GAP: Duration = Duration::from_secs(3);

/// Rate limiting of notifications: the first message shows right away, later ones within
/// `MIN_GAP` are counted and shown as one summary once the gap has passed.
#[derive(Default)]
struct Coalescer {
    last_shown: Option<Instant>,
    pending: u32,
    latest: Option<(String, String)>,
}

impl Coalescer {
    fn ready(&self, now: Instant) -> bool {
        self.last_shown.is_none_or(|t| now.duration_since(t) >= MIN_GAP)
    }

    fn push(&mut self, now: Instant, title: String, body: String) -> Option<(String, String)> {
        if self.pending == 0 && self.ready(now) {
            self.last_shown = Some(now);
            return Some((title, body));
        }
        self.pending += 1;
        self.latest = Some((title, body));
        None
    }

    fn flush(&mut self, now: Instant) -> Option<(String, String)> {
        if self.pending == 0 || !self.ready(now) {
            return None;
        }
        let count = std::mem::take(&mut self.pending);
        let latest = self.latest.take();
        self.last_shown = Some(now);
        if count == 1 {
            latest
        } else {
            Some((crate::config::APP_NAME.to_owned(), format!("{count} new messages")))
        }
    }

    /// Time until a pending summary may be shown, `None` if nothing is pending.
    fn next_flush(&self, now: Instant) -> Option<Duration> {
        (self.pending > 0).then(|| self.last_shown.map_or(Duration::ZERO, |t| MIN_GAP.saturating_sub(now.duration_since(t))))
    }
}

/// Name of the single-instance mutex and the tray window class for a data dir: runs with
/// another data dir (`RUSTCORD_HOME`, tests, screenshots) never see each other.
pub fn instance_name(data_dir: &Path) -> String {
    use sha2::Digest;
    let hash = sha2::Sha256::digest(data_dir.to_string_lossy().to_lowercase().as_bytes());
    format!("Rustcord-{}", hash[..8].iter().map(|b| format!("{b:02x}")).collect::<String>())
}

fn my_instance_name() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| instance_name(&crate::config::paths().config_dir))
}

/// True when no other Rustcord runs with this data dir; this process then holds it until it
/// exits.
pub fn claim_instance() -> bool {
    #[cfg(windows)]
    {
        imp::claim_instance()
    }
    #[cfg(unix)]
    {
        unix::claim_instance()
    }
}

/// `claim_instance`, retried every 100 ms until it succeeds or `wait` has passed (ZERO: one
/// try). `--after-update`: the previous version is still on its way out.
pub fn claim_instance_within(wait: Duration) -> bool {
    retry_for(wait, claim_instance)
}

fn retry_for(wait: Duration, mut attempt: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + wait;
    loop {
        if attempt() {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Quit like the tray menu's Quit (also while the window is hidden): the updater, once the new
/// version is started.
pub fn request_quit() {
    push(TrayAction::Quit);
}

/// Asks the Rustcord that holds this data dir to come to the front, or to quit (`quit`; then
/// waits until it has exited, at most 20 s). False when none answered within 5 s.
pub fn signal_running(quit: bool) -> bool {
    #[cfg(windows)]
    {
        imp::signal_running(quit)
    }
    #[cfg(unix)]
    {
        unix::signal_running(quit)
    }
}

#[cfg(windows)]
mod imp {
    use super::{Coalescer, TrayAction, push};
    use std::cell::RefCell;
    use std::ptr::{null, null_mut};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU8, AtomicU32, AtomicUsize, Ordering::SeqCst};
    use std::sync::mpsc::{self, Sender};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Threading::{CreateMutexW, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::{
        NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_RESPECT_QUIET_TIME, NIIF_USER, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIN_SELECT,
        NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow, DispatchMessageW, FindWindowW,
        GetMessageW, GetSystemMetrics, GetWindowThreadProcessId, HICON, IDI_APPLICATION, IMAGE_ICON, KillTimer, LR_SHARED, LoadIconW, LoadImageW, MF_CHECKED, MF_GRAYED,
        MF_SEPARATOR, MF_STRING, MSG, PostMessageW, PostQuitMessage, RegisterClassExW, RegisterWindowMessageW, SM_CXSMICON, SM_CYSMICON, SetForegroundWindow,
        SetMenuDefaultItem, SetTimer, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, TranslateMessage, WM_APP, WM_CONTEXTMENU, WM_NULL, WM_TIMER, WM_USER,
        WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_POPUP,
    };

    /// Icon callbacks.
    const WM_TRAY: u32 = WM_APP + 1;
    /// `show` queued balloons.
    const WM_BALLOON: u32 = WM_APP + 2;
    /// From another process (`signal_running`), so the values must stay fixed.
    const WM_SHOW_APP: u32 = WM_APP + 3;
    const WM_QUIT_APP: u32 = WM_APP + 4;
    /// `shutdown`.
    const WM_STOP: u32 = WM_APP + 5;
    const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
    const NIN_BALLOONUSERCLICK: u32 = WM_USER + 5;
    const FLUSH_TIMER: usize = 1;
    const ID_OPEN: usize = 1;
    const ID_MUTE: usize = 2;
    const ID_DEAFEN: usize = 3;
    const ID_PAUSE_HOUR: usize = 5;
    const ID_PAUSE_TOMORROW: usize = 6;
    const ID_RESUME: usize = 7;
    const ID_QUIT: usize = 4;

    const NOT_STARTED: u8 = 0;
    const STARTING: u8 = 1;
    const RUNNING: u8 = 2;
    const FAILED: u8 = 3;
    static STATE: AtomicU8 = AtomicU8::new(NOT_STARTED);
    /// The tray window (`HWND` as usize); 0 until it exists and after `shutdown`.
    static WINDOW: AtomicUsize = AtomicUsize::new(0);
    static PENDING: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
    static STOPPED: Mutex<Option<Sender<()>>> = Mutex::new(None);
    static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

    thread_local! {
        /// The tray thread's icon state, for the window procedure.
        static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
    }

    pub fn start() {
        if STATE.compare_exchange(NOT_STARTED, STARTING, SeqCst, SeqCst).is_ok()
            && std::thread::Builder::new().name("rustcord-tray".into()).spawn(run).is_err()
        {
            STATE.store(FAILED, SeqCst);
        }
    }

    pub fn has_tray() -> bool {
        matches!(STATE.load(SeqCst), STARTING | RUNNING)
    }

    pub fn failed() -> bool {
        STATE.load(SeqCst) == FAILED
    }

    pub fn show(title: &str, body: &str) {
        if !has_tray() {
            return;
        }
        PENDING.lock().unwrap_or_else(|e| e.into_inner()).push((title.to_owned(), body.to_owned()));
        // Not there yet: `run` picks the queue up once the window exists.
        post(WINDOW.load(SeqCst), WM_BALLOON);
    }

    pub fn shutdown() {
        let hwnd = WINDOW.swap(0, SeqCst);
        if hwnd == 0 {
            return;
        }
        let (tx, rx) = mpsc::channel();
        *STOPPED.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
        // Wait briefly so the icon is really gone before the process exits (no ghost icon).
        if post(hwnd, WM_STOP) {
            let _ = rx.recv_timeout(Duration::from_secs(1));
        }
    }

    fn post(hwnd: usize, msg: u32) -> bool {
        // SAFETY: posting to a window handle is safe even if it has gone meanwhile.
        hwnd != 0 && unsafe { PostMessageW(hwnd as HWND, msg, 0, 0) } != 0
    }

    pub fn claim_instance() -> bool {
        let name = wide(&format!(r"Local\{}", super::my_instance_name()));
        // SAFETY: a NUL-terminated name; the handle stays open until the process exits.
        unsafe {
            let mutex = CreateMutexW(null(), 0, name.as_ptr());
            if !mutex.is_null() && GetLastError() == ERROR_ALREADY_EXISTS {
                CloseHandle(mutex);
                return false;
            }
        }
        // Also when the mutex could not be made at all: better two windows than none.
        true
    }

    pub fn signal_running(quit: bool) -> bool {
        let class = wide(super::my_instance_name());
        // The running one may still be creating its tray window.
        for _ in 0..50 {
            // SAFETY: plain Win32 calls with valid arguments; the process handle is closed.
            unsafe {
                let hwnd = FindWindowW(class.as_ptr(), null());
                if !hwnd.is_null() {
                    let mut pid = 0;
                    GetWindowThreadProcessId(hwnd, &mut pid);
                    // The user just started this process, so it may hand the foreground on.
                    AllowSetForegroundWindow(pid);
                    let process = if quit { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) } else { null_mut() };
                    PostMessageW(hwnd, if quit { WM_QUIT_APP } else { WM_SHOW_APP }, 0, 0);
                    if !process.is_null() {
                        WaitForSingleObject(process, 20_000);
                        CloseHandle(process);
                    }
                    return true;
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    fn run() {
        let Some(tray) = Tray::create() else {
            STATE.store(FAILED, SeqCst);
            return;
        };
        let hwnd = tray.hwnd as usize;
        TRAY.with(|t| *t.borrow_mut() = Some(tray));
        WINDOW.store(hwnd, SeqCst);
        STATE.store(RUNNING, SeqCst);
        // Balloons asked for while the window was being created.
        post(hwnd, WM_BALLOON);
        let mut msg = MSG::default();
        // SAFETY: the standard message loop of the thread that owns the window; it blocks, so an
        // idle tray costs no wakeups.
        unsafe {
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if let Some(ack) = STOPPED.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = ack.send(());
        }
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            // Version 4: the event in the low word of lParam, the click point in wParam.
            WM_TRAY => match (lparam & 0xffff) as u32 {
                NIN_SELECT | NIN_KEYSELECT => push(TrayAction::Show),
                NIN_BALLOONUSERCLICK => push(TrayAction::Notification),
                WM_CONTEXTMENU => menu(hwnd, (wparam & 0xffff) as i16 as i32, ((wparam >> 16) & 0xffff) as i16 as i32),
                _ => {}
            },
            WM_SHOW_APP => push(TrayAction::Show),
            WM_QUIT_APP => push(TrayAction::Quit),
            WM_BALLOON | WM_TIMER => with_tray(|t| t.balloons(msg == WM_TIMER)),
            WM_STOP => {
                // Dropping the tray removes the icon.
                TRAY.with(|t| t.borrow_mut().take());
                // SAFETY: our own window, destroyed once, on its thread.
                unsafe {
                    DestroyWindow(hwnd);
                    PostQuitMessage(0);
                }
            }
            // Explorer restarted: the icon is gone, add it again.
            m if m != 0 && m == TASKBAR_CREATED.load(SeqCst) => with_tray(|t| {
                t.add();
            }),
            // SAFETY: the default handling for everything else.
            _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
        0
    }

    /// Short borrows only: the menu's modal loop runs this window procedure again.
    fn with_tray(f: impl FnOnce(&mut Tray)) {
        TRAY.with(|t| {
            if let Some(tray) = t.borrow_mut().as_mut() {
                f(tray);
            }
        });
    }

    /// The right-click menu at the click point; the choice goes to the UI thread.
    fn menu(hwnd: HWND, x: i32, y: i32) {
        let voice = super::VOICE.load(SeqCst);
        let check = |on: bool| if on { MF_CHECKED } else { 0 };
        let paused = super::PAUSED.load(SeqCst);
        let items = [
            (ID_OPEN, "Open Rustcord", MF_STRING),
            (ID_MUTE, "Mute", MF_STRING | check(voice & 1 != 0)),
            (ID_DEAFEN, "Deafen", MF_STRING | check(voice & 2 != 0)),
            (0, "", MF_SEPARATOR),
            (ID_PAUSE_HOUR, "Pause notifications for 1 hour", MF_STRING),
            (ID_PAUSE_TOMORROW, "Pause notifications until tomorrow", MF_STRING),
            (ID_RESUME, "Resume notifications", MF_STRING | if paused { 0 } else { MF_GRAYED }),
            (0, "", MF_SEPARATOR),
            (ID_QUIT, "Quit Rustcord", MF_STRING),
        ];
        // SAFETY: a menu created, shown and destroyed here, with NUL-terminated labels that
        // outlive the calls.
        let picked = unsafe {
            let menu = CreatePopupMenu();
            if menu.is_null() {
                return;
            }
            let labels: Vec<Vec<u16>> = items.iter().map(|i| wide(i.1)).collect();
            for (item, label) in items.iter().zip(&labels) {
                AppendMenuW(menu, item.2, item.0, if item.2 == MF_SEPARATOR { null() } else { label.as_ptr() });
            }
            SetMenuDefaultItem(menu, ID_OPEN as u32, 0);
            // In front first, or the menu stays open when clicking elsewhere.
            SetForegroundWindow(hwnd);
            let picked = TrackPopupMenuEx(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, x, y, hwnd, null());
            PostMessageW(hwnd, WM_NULL, 0, 0);
            DestroyMenu(menu);
            picked as usize
        };
        match picked {
            ID_OPEN => push(TrayAction::Show),
            ID_MUTE => push(TrayAction::ToggleMute),
            ID_DEAFEN => push(TrayAction::ToggleDeafen),
            ID_PAUSE_HOUR => push(TrayAction::PauseHour),
            ID_PAUSE_TOMORROW => push(TrayAction::PauseTomorrow),
            ID_RESUME => push(TrayAction::Resume),
            ID_QUIT => push(TrayAction::Quit),
            _ => {}
        }
    }

    /// The hidden window's icon, owned by the tray thread.
    struct Tray {
        hwnd: HWND,
        /// The flat mark in the notification area.
        icon: HICON,
        /// The app icon on balloons / toasts.
        app_icon: HICON,
        coalescer: Coalescer,
    }

    impl Tray {
        fn create() -> Option<Self> {
            let class = wide(super::my_instance_name());
            let title = wide(crate::config::APP_NAME);
            // SAFETY: plain Win32 calls with valid, NUL-terminated arguments; the handles are
            // only used on this thread.
            unsafe {
                let module = GetModuleHandleW(null());
                let wc = WNDCLASSEXW {
                    cbSize: size_of::<WNDCLASSEXW>() as u32,
                    lpfnWndProc: Some(wndproc),
                    hInstance: module,
                    lpszClassName: class.as_ptr(),
                    ..Default::default()
                };
                if RegisterClassExW(&wc) == 0 {
                    return None;
                }
                TASKBAR_CREATED.store(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()), SeqCst);
                // Hidden and top-level (not message-only): it gets the TaskbarCreated broadcast,
                // and a second start finds it by its class name.
                let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW, class.as_ptr(), title.as_ptr(), WS_POPUP, 0, 0, 0, 0, null_mut(), null_mut(), module, null());
                if hwnd.is_null() {
                    return None;
                }
                // Resource ids (assets/rustcord.rc): 1 the app icon, 2 the flat mark (logo-mono.svg),
                // loaded at the small-icon size so the shell does not scale a 32 px frame down (shared:
                // the system frees it).
                let mut app_icon = LoadIconW(module, std::ptr::without_provenance(1));
                if app_icon.is_null() {
                    app_icon = LoadIconW(null_mut(), IDI_APPLICATION);
                }
                let mark = LoadImageW(module, std::ptr::without_provenance(2), IMAGE_ICON, GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON), LR_SHARED);
                let icon = if mark.is_null() { app_icon } else { mark };
                let tray = Tray { hwnd, icon, app_icon, coalescer: Coalescer::default() };
                // Fails while the taskbar is not up yet (early at sign-in): TaskbarCreated adds it then.
                tray.add();
                Some(tray)
            }
        }

        fn data(&self) -> NOTIFYICONDATAW {
            NOTIFYICONDATAW {
                cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: 1,
                ..Default::default()
            }
        }

        fn add(&self) -> bool {
            let mut data = self.data();
            data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
            data.uCallbackMessage = WM_TRAY;
            data.hIcon = self.icon;
            fill_wide(&mut data.szTip, crate::config::APP_NAME);
            // SAFETY: `data` is a fully initialized NOTIFYICONDATAW with the right cbSize.
            unsafe {
                if Shell_NotifyIconW(NIM_ADD, &data) == 0 {
                    return false;
                }
                // Version 4: NIN_SELECT for clicks, the menu point in wParam.
                data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                Shell_NotifyIconW(NIM_SETVERSION, &data) != 0
            }
        }

        /// Shows what `show` queued and what the rate limit held back; arms the timer for the rest.
        fn balloons(&mut self, timer: bool) {
            let now = Instant::now();
            // SAFETY: our window's timer, on its thread.
            if timer {
                unsafe { KillTimer(self.hwnd, FLUSH_TIMER) };
            }
            let queued = std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()));
            for (title, body) in queued {
                if let Some((t, b)) = self.coalescer.push(now, title, body) {
                    self.balloon(&t, &b);
                }
            }
            if let Some((t, b)) = self.coalescer.flush(now) {
                self.balloon(&t, &b);
            }
            if let Some(wait) = self.coalescer.next_flush(now) {
                // SAFETY: as above.
                unsafe { SetTimer(self.hwnd, FLUSH_TIMER, wait.as_millis().clamp(1, u32::MAX as u128) as u32, None) };
            }
        }

        fn balloon(&self, title: &str, body: &str) {
            let mut data = self.data();
            data.uFlags = NIF_INFO;
            data.dwInfoFlags = NIIF_USER | NIIF_RESPECT_QUIET_TIME;
            data.hBalloonIcon = self.app_icon;
            fill_wide(&mut data.szInfoTitle, title);
            // An empty szInfo would remove the balloon instead of showing it.
            fill_wide(&mut data.szInfo, if body.trim().is_empty() { "New message" } else { body });
            // SAFETY: as in `add`.
            unsafe {
                // Fails when Explorer restarted and dropped our icon: add it again and retry.
                if Shell_NotifyIconW(NIM_MODIFY, &data) == 0 && self.add() {
                    Shell_NotifyIconW(NIM_MODIFY, &data);
                }
            }
        }
    }

    impl Drop for Tray {
        fn drop(&mut self) {
            let data = self.data();
            // SAFETY: removes our own icon (the window is destroyed by WM_STOP).
            unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// Copies `s` into a fixed UTF-16 buffer with a terminating NUL.
    fn fill_wide<const N: usize>(dst: &mut [u16; N], s: &str) {
        let units = truncate_utf16(s, N - 1);
        dst[..units.len()].copy_from_slice(&units);
        dst[units.len()] = 0;
    }

    /// UTF-16 of `s` (NULs removed) in at most `max` units; cut on a char boundary and marked
    /// with "…" when it doesn't fit.
    fn truncate_utf16(s: &str, max: usize) -> Vec<u16> {
        let clean: String = s.chars().filter(|&c| c != '\0').collect();
        let units: Vec<u16> = clean.encode_utf16().collect();
        if units.len() <= max {
            return units;
        }
        let mut out = Vec::with_capacity(max);
        let mut buf = [0u16; 2];
        for c in clean.chars() {
            let enc = c.encode_utf16(&mut buf);
            if out.len() + enc.len() + 1 > max {
                break;
            }
            out.extend_from_slice(enc);
        }
        if max > 0 {
            out.push('…' as u16);
        }
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn truncation_fits_and_respects_char_boundaries() {
            assert_eq!(truncate_utf16("hi", 63), "hi".encode_utf16().collect::<Vec<_>>());
            assert_eq!(truncate_utf16("a\0b", 63), "ab".encode_utf16().collect::<Vec<_>>());

            let exact = "x".repeat(63);
            assert_eq!(truncate_utf16(&exact, 63).len(), 63);

            let long = "y".repeat(300);
            let cut = truncate_utf16(&long, 255);
            assert_eq!(cut.len(), 255);
            assert_eq!(*cut.last().unwrap(), '…' as u16);

            // 😀 is a surrogate pair; never keep half of one.
            let emoji = "😀".repeat(40);
            let cut = truncate_utf16(&emoji, 63);
            assert!(cut.len() <= 63);
            assert!(String::from_utf16(&cut).is_ok());
            assert!(String::from_utf16(&cut).unwrap().ends_with("😀…"));

            let mut buf = [1u16; 64];
            fill_wide(&mut buf, &long);
            assert_eq!(buf[63], 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalescer_rate_limits_and_summarizes() {
        let t0 = Instant::now();
        let s = |n: u64| t0 + Duration::from_millis(n);
        let mut c = Coalescer::default();
        let msg = |i: u32| (format!("t{i}"), format!("b{i}"));

        assert_eq!(c.push(s(0), "t0".into(), "b0".into()), Some(msg(0)));
        assert_eq!(c.push(s(100), "t1".into(), "b1".into()), None);
        assert_eq!(c.next_flush(s(1000)), Some(Duration::from_millis(2000)));
        assert_eq!(c.flush(s(2999)), None);
        // A single throttled message is shown as itself.
        assert_eq!(c.flush(s(3000)), Some(msg(1)));
        assert_eq!(c.next_flush(s(3000)), None);

        for i in 2..6 {
            assert_eq!(c.push(s(3500), format!("t{i}"), format!("b{i}")), None);
        }
        let (_, body) = c.flush(s(6000)).unwrap();
        assert_eq!(body, "4 new messages");
        assert_eq!(c.flush(s(9000)), None);

        // Quiet for long enough: shown immediately again.
        assert_eq!(c.push(s(9001), "t9".into(), "b9".into()), Some(msg(9)));
    }

    #[test]
    fn instance_name_follows_the_data_dir() {
        let user = instance_name(Path::new(r"C:\Users\ann\AppData\Roaming\Rustcord\config"));
        assert!(user.starts_with("Rustcord-") && user.len() == "Rustcord-".len() + 16, "{user}");
        // Windows paths are case-insensitive: the same folder is the same instance.
        assert_eq!(user, instance_name(Path::new(r"C:\USERS\Ann\AppData\Roaming\Rustcord\config")));
        // A test run's RUSTCORD_HOME never meets the user's Rustcord.
        assert_ne!(user, instance_name(Path::new(r"C:\Temp\fx4\shot\config")));
        assert_ne!(instance_name(Path::new(r"C:\a\config")), instance_name(Path::new(r"C:\b\config")));
    }

    #[test]
    fn retry_for_tries_until_the_deadline() {
        let mut tries = 0;
        assert!(!retry_for(Duration::ZERO, || {
            tries += 1;
            false
        }));
        assert_eq!(tries, 1, "ZERO: one try");
        let mut tries = 0;
        assert!(retry_for(Duration::from_secs(5), || {
            tries += 1;
            tries == 3
        }));
        let started = Instant::now();
        assert!(!retry_for(Duration::from_millis(250), || false));
        assert!(started.elapsed() >= Duration::from_millis(250) && started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn tray_actions_queue_in_order() {
        push(TrayAction::ToggleMute);
        push(TrayAction::Show);
        let got = take_actions();
        assert!(got.ends_with(&[TrayAction::ToggleMute, TrayAction::Show]), "{got:?}");
        assert!(take_actions().is_empty());
    }
}
