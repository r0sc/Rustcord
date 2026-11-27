//! App lock (Settings > Windows / Linux / macOS settings > App lock): a screen lock, not
//! encryption. While `locked()` the window shows the lock screen (ui/lock_screen.rs) and
//! notifications carry no message text; the cache on disk is not encrypted. The PIN is stored as
//! a salted, iterated SHA-256 hash in the OS keyring (never the PIN, never in settings.toml), and
//! the only way past a forgotten one is signing out. Offline and test runs keep the hash in memory
//! and never touch the keyring. Nothing here touches the network or the login token.

use crate::config::AppLock;
use sha2::{Digest, Sha256};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const SERVICE: &str = "Rustcord";
pub const ACCOUNT: &str = "app-lock-pin";
const ROUNDS: u32 = 100_000;
const TRIES: u8 = 5;
const WAIT: Duration = Duration::from_secs(30);

static LOCKED: AtomicBool = AtomicBool::new(false);
/// Windows Hello was offered since the last lock.
#[cfg(windows)]
static HELLO_ASKED: AtomicBool = AtomicBool::new(false);
/// The stored hash (`v1$rounds$salt$hash`): read once at start, so unlocking never waits on the keyring.
static STORED: Mutex<Option<String>> = Mutex::new(None);
static THROTTLE: Mutex<Throttle> = Mutex::new(Throttle { wrong: 0, until: None });
/// The last key press, click, wheel turn or text in the window.
static LAST_INPUT: Mutex<Option<Instant>> = Mutex::new(None);

pub fn locked() -> bool {
    LOCKED.load(Ordering::Relaxed)
}

/// Locks (only while the lock is on).
pub fn lock(s: &AppLock) {
    if s.enabled {
        #[cfg(windows)]
        HELLO_ASKED.store(false, Ordering::Relaxed);
        LOCKED.store(true, Ordering::Relaxed);
    }
}

pub fn unlock() {
    LOCKED.store(false, Ordering::Relaxed);
    *lock_of(&THROTTLE) = Throttle::default();
    *lock_of(&LAST_INPUT) = Some(Instant::now());
}

fn lock_of<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// At start (from the preload thread): locked when the lock is on, with its hash read once.
pub fn arm(s: &AppLock) {
    LOCKED.store(s.enabled, Ordering::Relaxed);
    if s.enabled {
        reload();
    }
}

fn entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, ACCOUNT).map_err(|e| e.to_string())
}

/// Reads the hash from the keyring again ("Try again"). Offline runs have only the memory copy.
pub fn reload() -> bool {
    if !crate::config::offline_mode() {
        *lock_of(&STORED) = entry().ok().and_then(|e| e.get_password().ok());
    }
    lock_of(&STORED).is_some()
}

/// 4 to 16 digits.
pub fn pin_ok(pin: &str) -> bool {
    (4..=16).contains(&pin.len()) && pin.bytes().all(|b| b.is_ascii_digit())
}

/// Stores the hash of a new PIN (the keyring first: nothing changes when it refuses).
pub fn set_pin(pin: &str) -> Result<(), String> {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|e| e.to_string())?;
    let stored = encode(pin, &salt, ROUNDS);
    if !crate::config::offline_mode() {
        entry()?.set_password(&stored).map_err(|e| e.to_string())?;
    }
    *lock_of(&STORED) = Some(stored);
    Ok(())
}

/// Removes the stored hash (nothing stored is fine).
pub fn clear_pin() -> Result<(), String> {
    *lock_of(&STORED) = None;
    if crate::config::offline_mode() {
        return Ok(());
    }
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Check {
    Unlocked,
    Wrong,
    /// Too many tries: this long until the next one.
    Wait(Duration),
    /// No hash to compare with (the password store failed or was emptied).
    NoPin,
}

/// Checks a PIN; the right one unlocks.
pub fn try_pin(pin: &str, now: Instant) -> Check {
    if let Some(d) = wait(now) {
        return Check::Wait(d);
    }
    let Some(stored) = lock_of(&STORED).clone() else { return Check::NoPin };
    if verify(&stored, pin) {
        unlock();
        return Check::Unlocked;
    }
    let mut t = lock_of(&THROTTLE);
    t.fail(now);
    Check::Wrong
}

/// Time left of the pause after too many wrong tries.
pub fn wait(now: Instant) -> Option<Duration> {
    lock_of(&THROTTLE).wait(now)
}

/// True once per lock: the lock screen opens Windows Hello by itself the first time it is drawn.
#[cfg(windows)]
pub fn hello_due() -> bool {
    !HELLO_ASKED.swap(true, Ordering::Relaxed)
}

/// Locks after `s.idle_minutes` without input in the window; one `request_repaint_after` per
/// pass, never a loop. `input`: a key, click, wheel turn or text this pass (pointer moves don't count).
pub fn idle(ctx: &egui::Context, s: &AppLock, input: bool) {
    if !s.enabled || locked() {
        return;
    }
    let now = Instant::now();
    let mut last = lock_of(&LAST_INPUT);
    if input || last.is_none() {
        *last = Some(now);
    }
    match last.and_then(|l| deadline(l, s.idle_minutes)) {
        Some(d) if now >= d => {
            drop(last);
            lock(s);
            ctx.request_repaint();
        }
        Some(d) => ctx.request_repaint_after(d - now),
        None => {}
    }
}

/// When the idle lock fires; 0 minutes: never.
fn deadline(last: Instant, minutes: u16) -> Option<Instant> {
    (minutes > 0).then(|| last + Duration::from_secs(u64::from(minutes) * 60))
}

/// Test hook `RUSTCORD_NAV_DEMO=lock`: the lock on, PIN "1234" in memory, locked now.
// shortcut: fixed demo PIN; only RUSTCORD_NAV_DEMO=lock sets it.
pub fn demo(s: &mut AppLock, hello: bool) {
    *s = AppLock { enabled: true, hello, ..AppLock::default() };
    *lock_of(&STORED) = Some(encode("1234", &[7; 16], 10));
    lock(s);
}

fn hash(pin: &str, salt: &[u8], rounds: u32) -> [u8; 32] {
    let mut h: [u8; 32] = Sha256::new().chain_update(salt).chain_update(pin.as_bytes()).finalize().into();
    for _ in 0..rounds {
        h = Sha256::new().chain_update(h).chain_update(salt).finalize().into();
    }
    h
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    s.len().is_multiple_of(2).then_some(())?;
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// `v1$<rounds>$<salt hex>$<hash hex>`.
fn encode(pin: &str, salt: &[u8], rounds: u32) -> String {
    format!("v1${rounds}${}${}", hex(salt), hex(&hash(pin, salt, rounds)))
}

/// A malformed `stored` never verifies.
fn verify(stored: &str, pin: &str) -> bool {
    let mut parts = stored.split('$');
    let (Some("v1"), Some(rounds), Some(salt), Some(want), None) = (parts.next(), parts.next(), parts.next(), parts.next(), parts.next()) else { return false };
    let (Ok(rounds), Some(salt), Some(want)) = (rounds.parse::<u32>(), unhex(salt), unhex(want)) else { return false };
    if rounds > 10_000_000 || want.len() != 32 {
        return false;
    }
    // Constant time: the xor of every byte, not an early exit.
    hash(pin, &salt, rounds).iter().zip(&want).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

/// 5 wrong tries pause the lock screen for 30 s.
#[derive(Default)]
struct Throttle {
    wrong: u8,
    until: Option<Instant>,
}

impl Throttle {
    fn fail(&mut self, now: Instant) {
        self.wrong += 1;
        if self.wrong >= TRIES {
            (self.wrong, self.until) = (0, Some(now + WAIT));
        }
    }

    fn wait(&self, now: Instant) -> Option<Duration> {
        self.until.and_then(|u| u.checked_duration_since(now)).filter(|d| !d.is_zero())
    }
}

/// Windows Hello (face, fingerprint or Windows PIN) as a second way in.
#[cfg(windows)]
pub mod hello {
    use std::sync::Mutex;
    use windows::Security::Credentials::UI::{UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
    use windows::Win32::System::WinRT::IUserConsentVerifierInterop;
    use windows::core::{HSTRING, factory};

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Outcome {
        Verified,
        Cancelled,
        Failed,
    }

    static AVAILABLE: Mutex<Option<bool>> = Mutex::new(None);
    static RESULT: Mutex<Option<Outcome>> = Mutex::new(None);
    static ASKED: std::sync::Once = std::sync::Once::new();

    /// Whether Windows Hello is set up on this PC; None until the OS has answered (the first call asks).
    pub fn available(ctx: &egui::Context) -> Option<bool> {
        let ctx = ctx.clone();
        ASKED.call_once(|| {
            let _ = std::thread::Builder::new().name("rustcord-hello".into()).spawn(move || {
                // SAFETY: plain COM start-up on this thread, once.
                let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
                let ok = UserConsentVerifier::CheckAvailabilityAsync().and_then(|op| op.join()).is_ok_and(|a| a == UserConsentVerifierAvailability::Available);
                *super::lock_of(&AVAILABLE) = Some(ok);
                ctx.request_repaint();
            });
        });
        *super::lock_of(&AVAILABLE)
    }

    /// Opens the Windows Hello prompt over this window (the OS draws it); the answer comes back
    /// through `take_result` and a repaint.
    pub fn prompt(ctx: &egui::Context) {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
        // SAFETY: reads the calling (UI) thread's active window.
        let hwnd = unsafe { GetActiveWindow() } as usize;
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new().name("rustcord-hello-prompt".into()).spawn(move || {
            // SAFETY: plain COM start-up on this thread, once.
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            let message = HSTRING::from("Unlock Rustcord");
            let op = if hwnd == 0 {
                UserConsentVerifier::RequestVerificationAsync(&message)
            } else {
                factory::<UserConsentVerifier, IUserConsentVerifierInterop>().and_then(|f| {
                    // SAFETY: `hwnd` is this process's window handle, valid while the window lives.
                    unsafe { for_window(UserConsentVerifier::RequestVerificationAsync, &f, HWND(hwnd as *mut _), &message) }
                })
            };
            let outcome = match op.and_then(|op| op.join()) {
                Ok(UserConsentVerificationResult::Verified) => Outcome::Verified,
                Ok(UserConsentVerificationResult::Canceled) => Outcome::Cancelled,
                _ => Outcome::Failed,
            };
            *super::lock_of(&RESULT) = Some(outcome);
            ctx.request_repaint();
        });
    }

    /// The window-owned prompt. `_same_type` is never called: it names the operation type, which
    /// the windows crate does not re-export.
    /// SAFETY: `hwnd` must be a valid window handle.
    unsafe fn for_window<T: windows::core::Interface>(_same_type: fn(&HSTRING) -> windows::core::Result<T>, f: &IUserConsentVerifierInterop, hwnd: HWND, message: &HSTRING) -> windows::core::Result<T> {
        // SAFETY: the caller's promise.
        unsafe { f.RequestVerificationForWindowAsync::<T>(hwnd, message) }
    }

    pub fn take_result() -> Option<Outcome> {
        super::lock_of(&RESULT).take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pin_verifies_and_a_wrong_one_does_not() {
        let stored = encode("1234", &[1, 2, 3, 4], 10);
        assert!(stored.starts_with("v1$10$01020304$"));
        assert!(verify(&stored, "1234"));
        assert!(!verify(&stored, "1235") && !verify(&stored, "") && !verify(&stored, "12345"));
        for bad in ["", "v1", "v2$10$00$00", "v1$x$00$00", "v1$10$0$00", "v1$10$00$zz", "v1$10$00$00", "v1$10$00$00$extra"] {
            assert!(!verify(bad, "1234"), "{bad}");
        }
        assert_ne!(stored, encode("1234", &[9, 9, 9, 9], 10), "the salt changes the hash");
    }

    #[test]
    fn pins_are_4_to_16_digits() {
        assert!(pin_ok("1234") && pin_ok("1234567890123456"));
        assert!(!pin_ok("123") && !pin_ok("12345678901234567") && !pin_ok("12a4") && !pin_ok("") && !pin_ok("12 4") && !pin_ok("１２３４"));
    }

    #[test]
    fn five_wrong_tries_wait_30_seconds() {
        let t0 = Instant::now();
        let mut t = Throttle::default();
        for i in 0..4 {
            t.fail(t0 + Duration::from_secs(i));
            assert_eq!(t.wait(t0 + Duration::from_secs(i)), None);
        }
        t.fail(t0 + Duration::from_secs(4));
        assert_eq!(t.wait(t0 + Duration::from_secs(10)), Some(Duration::from_secs(24)));
        assert_eq!(t.wait(t0 + Duration::from_secs(34)), None);
        // The count starts over after a pause.
        t.fail(t0 + Duration::from_secs(40));
        assert_eq!(t.wait(t0 + Duration::from_secs(40)), None);
    }

    #[test]
    fn the_idle_deadline() {
        let t0 = Instant::now();
        assert_eq!(deadline(t0, 0), None);
        assert_eq!(deadline(t0, 15), Some(t0 + Duration::from_secs(900)));
    }
}
