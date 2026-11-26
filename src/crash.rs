//! Crash reports: a panic writes a small text file to `<config>/crash/`; the main screen shows
//! it once as a card (ui/crash_card.rs) with Copy, Report on GitHub and Dismiss. Always on and
//! local: nothing leaves the PC, and the report holds no token and no message text, only
//! the version, the system, the panic's own message and place, and the plugins that were on.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Reports kept on disk; older ones are deleted when a new one is written.
const KEEP: usize = 5;
const MESSAGE_MAX: usize = 500;

/// Set by the hook; the UI takes it once and loads the new report.
static NEW: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub path: PathBuf,
    pub text: String,
    /// False for a report from an earlier run.
    pub this_run: bool,
}

impl Report {
    /// `file:line:col` of the panic (the "Location:" line of the text).
    pub fn location(&self) -> Option<&str> {
        self.text.lines().find_map(|l| l.strip_prefix("Location: "))
    }

    /// Plugins whose code the panic came from and that are on now.
    pub fn suspects(&self) -> Vec<crate::plugins::Plugin> {
        self.location().map(crate::plugins::in_file).unwrap_or_default()
    }

    pub fn dismiss(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn dir() -> PathBuf {
    crate::config::paths().config_dir.join("crash")
}

/// The report text from its parts. The message is cut to 500 characters.
pub fn report_text(thread: &str, message: &str, location: &str, uptime_s: u64, plugins: &[&str]) -> String {
    let message: String = message.chars().take(MESSAGE_MAX).collect();
    format!(
        "Rustcord {} crash report\nSystem: {} {}\nThread: {thread}\nUptime: {uptime_s} s\nLocation: {location}\nMessage: {message}\nPlugins on: {}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        if plugins.is_empty() { "none".to_owned() } else { plugins.join(", ") },
    )
}

fn files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("crash-") && n.ends_with(".txt"))).collect();
    v.sort();
    v
}

/// Deletes all but the newest `KEEP` reports of `dir`.
fn prune(dir: &Path) {
    let files = files(dir);
    for p in &files[..files.len().saturating_sub(KEEP)] {
        let _ = std::fs::remove_file(p);
    }
}

/// Writes one report into `dir` (created if needed). Never panics.
fn write(dir: &Path, text: &str) -> Option<PathBuf> {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    std::fs::create_dir_all(dir).ok()?;
    // Names sort by time: seconds, then a counter for several panics in one second.
    let mut n = 0;
    let mut path = dir.join(format!("crash-{secs}-{n}.txt"));
    while path.exists() {
        n += 1;
        path = dir.join(format!("crash-{secs}-{n}.txt"));
    }
    std::fs::write(&path, text).ok()?;
    prune(dir);
    Some(path)
}

/// Keeps the default hook (stderr) and adds the report file. `ctx` wakes the UI when a
/// background thread panicked and the app lives on.
pub fn install(ctx: &egui::Context) {
    let (ctx, started) = (ctx.clone(), Instant::now());
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        let payload = info.payload();
        let message = payload.downcast_ref::<&str>().map(|s| (*s).to_owned()).or_else(|| payload.downcast_ref::<String>().cloned()).unwrap_or_else(|| "(no message)".into());
        let location = info.location().map_or_else(|| "unknown".to_owned(), |l| format!("{}:{}:{}", l.file().replace('\\', "/"), l.line(), l.column()));
        let thread = std::thread::current().name().unwrap_or("unnamed").to_owned();
        let text = report_text(&thread, &message, &location, started.elapsed().as_secs(), &crate::plugins::enabled_names());
        if write(&dir(), &text).is_some() {
            NEW.store(true, Ordering::Relaxed);
            ctx.request_repaint();
        }
    }));
}

/// The newest report in `dir`.
fn newest(dir: &Path, this_run: bool) -> Option<Report> {
    let path = files(dir).pop()?;
    let text = std::fs::read_to_string(&path).ok()?;
    Some(Report { path, text, this_run })
}

/// A report left by an earlier run (called once at start).
pub fn pending() -> Option<Report> {
    newest(&dir(), false)
}

/// A report the hook wrote in this run, once.
pub fn take_new() -> Option<Report> {
    if NEW.swap(false, Ordering::Relaxed) { newest(&dir(), true) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rustcord-crash-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn report_has_only_the_listed_parts_and_cuts_the_message() {
        let t = report_text("main", &"x".repeat(900), "src/plugins/people.rs:10:5", 42, &["MemberCount", "FriendsSince"]);
        let lines: Vec<_> = t.lines().collect();
        assert_eq!(lines.len(), 7);
        assert_eq!(lines[2], "Thread: main");
        assert_eq!(lines[3], "Uptime: 42 s");
        assert_eq!(lines[4], "Location: src/plugins/people.rs:10:5");
        assert_eq!(lines[5].len(), "Message: ".len() + 500);
        assert_eq!(lines[6], "Plugins on: MemberCount, FriendsSince");
        assert!(report_text("t", "m", "l", 0, &[]).contains("Plugins on: none"));
    }

    #[test]
    fn keeps_the_newest_five() {
        let d = temp("prune");
        std::fs::create_dir_all(&d).unwrap();
        for i in 0..8 {
            std::fs::write(d.join(format!("crash-{}-0.txt", 1000 + i)), "x").unwrap();
        }
        std::fs::write(d.join("other.txt"), "x").unwrap();
        prune(&d);
        let left: Vec<_> = files(&d).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(left, ["crash-1003-0.txt", "crash-1004-0.txt", "crash-1005-0.txt", "crash-1006-0.txt", "crash-1007-0.txt"]);
        assert!(d.join("other.txt").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn write_read_dismiss_and_a_bad_dir_does_not_panic() {
        let d = temp("write");
        let text = report_text("main", "boom", "src/ui/mod.rs:1:1", 1, &[]);
        let first = write(&d, &text).unwrap();
        let second = write(&d, &text).unwrap();
        assert_ne!(first, second);
        let r = newest(&d, true).unwrap();
        assert_eq!((r.path.as_path(), r.text.as_str()), (second.as_path(), text.as_str()));
        assert_eq!(r.location(), Some("src/ui/mod.rs:1:1"));
        r.dismiss();
        assert_eq!(newest(&d, false).unwrap().path, first);
        // A path under a file cannot be created: None, no panic.
        let file = d.join("f");
        std::fs::write(&file, "x").unwrap();
        assert!(write(&file.join("sub"), "x").is_none());
        let _ = std::fs::remove_dir_all(&d);
    }
}
