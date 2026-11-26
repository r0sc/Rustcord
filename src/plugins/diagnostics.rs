//! ClientDiagnostics: Settings > Advanced shows numbers Rustcord already measures: frame times
//! and the slowest sections (`ui::frame_stats`, timed only while the section is on screen),
//! memory (`ui::images::memory_mb`, `AppState::mem_estimate`), voice playback glitches
//! (`voice::audio::stats`), the connection and the version.
//! This file holds the data and the plain-text report; `ui/user_settings/diagnostics.rs` draws it.
//! The report is copied by a click; nothing leaves the PC and it holds only names and numbers.

use crate::backend::ConnState;
use crate::ui::frame_stats::Window;

/// A section over this many ms per frame is called slow.
pub const SLOW_MS: f32 = 8.0;

/// The `n` slowest sections of a window: (name, ms per frame, slow).
pub fn slowest(w: &Window, n: usize) -> Vec<(&'static str, f32, bool)> {
    let mut v: Vec<_> = w.sections.iter().map(|(name, ms)| (*name, *ms, *ms > SLOW_MS)).collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1));
    v.truncate(n);
    v
}

/// The `n` biggest state parts of `AppState::mem_estimate`, in MB.
pub fn biggest_parts(estimate: &[(&'static str, usize, usize)], n: usize) -> Vec<(&'static str, f64)> {
    let mut v: Vec<_> = estimate.iter().map(|(name, _, bytes)| (*name, *bytes as f64 / (1024.0 * 1024.0))).collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1));
    v.truncate(n);
    v
}

/// "45 s", "12 min", "3 h 5 min".
pub fn duration_text(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs} s"),
        60..3600 => format!("{} min", secs / 60),
        _ => format!("{} h {} min", secs / 3600, secs % 3600 / 60),
    }
}

pub fn conn_text(c: &ConnState) -> String {
    match c {
        ConnState::Offline => "Offline".into(),
        ConnState::Connecting => "Connecting".into(),
        ConnState::Connected => "Connected".into(),
        ConnState::Reconnecting { attempt, in_secs } => format!("Reconnecting (attempt {attempt}, in {in_secs} s)"),
    }
}

/// "0 frames (idle)" or "N frames in 2.0 s".
pub fn frames_text(w: &Window) -> String {
    if w.frames == 0 { "0 frames (idle)".into() } else { format!("{} frames in {:.1} s", w.frames, w.secs) }
}

/// What the report is made of.
pub struct Facts {
    pub window: Option<Window>,
    /// Working set and private MB; None where it is not measured.
    pub memory: Option<(f64, f64)>,
    pub parts: Vec<(&'static str, f64)>,
    pub connection: String,
    pub uptime_s: u64,
    /// `voice::audio::stats`: speaker dry-outs, late packets, concealed frames since start.
    pub voice: (u32, u32, u32),
}

/// "2 speaker dry-outs, 0 late packets, 14 concealed frames".
pub fn voice_text((dry, late, concealed): (u32, u32, u32)) -> String {
    format!("{dry} speaker dry-outs, {late} late packets, {concealed} concealed frames")
}

/// The text of "Copy report": names and numbers only.
pub fn report(f: &Facts) -> String {
    let mut out = format!(
        "Rustcord {} diagnostics\nSystem: {} {}\nUptime: {}\nConnection: {}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        duration_text(f.uptime_s),
        f.connection
    );
    match &f.window {
        Some(w) => {
            out += &format!("Frames: {}\n", frames_text(w));
            if w.frames > 0 {
                out += &format!("Frame time: mean {:.2} ms, p95 {:.2} ms\nRustcord time: mean {:.2} ms, p95 {:.2} ms\n", w.frame_ms.0, w.frame_ms.1, w.app_ms.0, w.app_ms.1);
                let top: Vec<String> = slowest(w, 5).into_iter().map(|(n, ms, slow)| format!("{n} {ms:.2}{}", if slow { " (slow)" } else { "" })).collect();
                out += &format!("Slowest sections (ms per frame): {}\n", top.join(", "));
            }
        }
        None => out += "Frames: measuring\n",
    }
    match f.memory {
        Some((ws, private)) => out += &format!("Memory: working set {ws:.1} MB, private {private:.1} MB\n"),
        None => out += "Memory: not measured on this system\n",
    }
    let parts: Vec<String> = f.parts.iter().map(|(n, mb)| format!("{n} {mb:.1} MB")).collect();
    out += &format!("State: {}\n", parts.join(", "));
    out += &format!("Voice since start: {}\n", voice_text(f.voice));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> Window {
        Window {
            frames: 12,
            secs: 2.0,
            frame_ms: (1.5, 3.0),
            app_ms: (1.0, 2.0),
            sections: vec![("rail", 0.4), ("chat", 9.5), ("+markdown", 1.2), ("title", 0.1), ("channels", 0.9), ("page", 2.5), ("end", 0.2)],
        }
    }

    #[test]
    fn the_top_five_are_sorted_and_only_over_8_ms_is_slow() {
        let top = slowest(&window(), 5);
        let names: Vec<_> = top.iter().map(|t| t.0).collect();
        assert_eq!(names, ["chat", "page", "+markdown", "channels", "rail"]);
        assert_eq!(top.iter().filter(|t| t.2).map(|t| t.0).collect::<Vec<_>>(), ["chat"]);
        assert!(slowest(&Window::default(), 5).is_empty());
        let parts = biggest_parts(&[("a", 1, 1 << 20), ("b", 1, 5 << 20), ("c", 1, 3 << 20)], 2);
        assert_eq!(parts, [("b", 5.0), ("c", 3.0)]);
    }

    #[test]
    fn texts() {
        assert_eq!(duration_text(45), "45 s");
        assert_eq!(duration_text(720), "12 min");
        assert_eq!(duration_text(3 * 3600 + 300), "3 h 5 min");
        assert_eq!(frames_text(&Window::default()), "0 frames (idle)");
        assert_eq!(frames_text(&window()), "12 frames in 2.0 s");
        assert_eq!(conn_text(&ConnState::Reconnecting { attempt: 2, in_secs: 8 }), "Reconnecting (attempt 2, in 8 s)");
    }

    #[test]
    fn the_report_holds_only_names_and_numbers() {
        let f = Facts { window: Some(window()), memory: Some((90.2, 70.1)), parts: vec![("messages", 4.25)], connection: "Connected".into(), uptime_s: 125, voice: (2, 0, 14) };
        let r = report(&f);
        let keys: Vec<_> = r.lines().map(|l| l.split(':').next().unwrap()).collect();
        let first = format!("Rustcord {} diagnostics", env!("CARGO_PKG_VERSION"));
        assert_eq!(keys[0], first);
        assert_eq!(&keys[1..], ["System", "Uptime", "Connection", "Frames", "Frame time", "Rustcord time", "Slowest sections (ms per frame)", "Memory", "State", "Voice since start"]);
        assert!(r.contains("chat 9.50 (slow)") && r.contains("Memory: working set 90.2 MB, private 70.1 MB"));
        assert!(r.contains("Voice since start: 2 speaker dry-outs, 0 late packets, 14 concealed frames"));
        let idle = report(&Facts { window: Some(Window::default()), memory: None, parts: vec![], connection: "Offline".into(), uptime_s: 3, voice: (0, 0, 0) });
        assert!(idle.contains("Frames: 0 frames (idle)") && idle.contains("not measured") && !idle.contains("Slowest"));
        assert!(report(&Facts { window: None, ..f }).contains("Frames: measuring"));
    }
}
