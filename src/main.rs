//! Rustcord – a lightweight third-party chat client.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod activity;
mod alerts;
mod api;
mod auth;
mod autostart;
mod backend;
mod cache;
mod captcha;
mod config;
mod crash;
mod emoji;
mod gateway;
mod lock;
mod media;
mod model;
mod notify;
mod now_playing;
mod overrides;
mod plugins;
mod quests;
mod rpc;
mod saved;
#[allow(dead_code)] // wired up by the next commits
mod sounds;
mod state;
mod theme;
mod ui;
mod update;
mod voice;
#[cfg(windows)]
mod winreg;

use std::time::{Duration, Instant};

/// Flags: `--minimized` (sign-in autostart: start hidden in the tray), `--quit` (asks the running
/// Rustcord to quit and waits for it; the installer uses it before replacing files),
/// `--clear-login` (removes the saved login from the keyring and exits; `install.sh --uninstall --purge`),
/// `--after-update` (started by the updater: waits up to 30 s for the previous
/// version to release the instance lock instead of handing off to it, then cleans up its
/// leftovers; docs/UPDATER_DESIGN.md §10).
fn main() -> eframe::Result {
    let started = Instant::now();
    trace_startup("main", started);
    ui::images::mem_trace("main");
    if std::env::args().any(|a| a == "--clear-login") {
        if let Err(e) = auth::clear_token() {
            eprintln!("rustcord: {e}");
            std::process::exit(1);
        }
        // The purge should not leave the app lock's hash behind either.
        let _ = lock::clear_pin();
        return Ok(());
    }

    // One Rustcord per data dir: a second start brings the running one to the front and exits.
    // Screenshot runs skip it (and the tray): they must never touch another window.
    let test_run = std::env::var_os("RUSTCORD_SCREENSHOT").is_some();
    let quit = std::env::args().any(|a| a == "--quit");
    let after_update = std::env::args().any(|a| a == "--after-update");
    let claim_wait = if after_update { Duration::from_secs(30) } else { Duration::ZERO };
    if !test_run && !notify::claim_instance_within(claim_wait) {
        notify::signal_running(quit);
        return Ok(());
    }
    if quit {
        // Nothing ran: no socket file left behind (Linux / macOS).
        notify::shutdown();
        return Ok(());
    }

    // Read settings, the disk cache and the keyring while the window is being created.
    let preload = std::thread::spawn(|| {
        // A captcha webview's profile that a quit or crash left behind (src/captcha.rs).
        captcha::clean_profiles();
        let mut settings = config::Settings::load();
        // Locked from the first frame when the app lock is on (the hash is read here, off the UI thread).
        lock::arm(&settings.app_lock);
        // Autostart on once for an installed copy (never for dev builds or test runs).
        autostart::default_once(&mut settings);
        let snapshot = cache::load_snapshot();
        ui::images::mem_trace("snapshot_loaded");
        // The RUSTCORD_LOGIN_TAB test hook always shows the login screen, never a stored account.
        let skip = config::offline_mode() || std::env::var_os("RUSTCORD_LOGIN_TAB").is_some();
        let token = if skip { None } else { auth::load_token() };
        (settings, snapshot, token)
    });

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("rustcord-net")
        .enable_all()
        .build()
        .expect("tokio runtime");

    // Create the egui context up front so the backend can start connecting immediately.
    let ctx = egui::Context::default();
    // eframe sets this itself only on a context it creates; ours must open real windows for the game overlay's viewports (ui/overlay).
    #[cfg(windows)]
    ctx.set_embed_viewports(false);
    crash::install(&ctx);
    let backend = backend::spawn(rt.handle(), ctx.clone());

    // eframe's defaults are already the lean ones: no MSAA, depth or stencil buffer, vsync on
    // (measured: none of these, nor dithering, changes RAM; the GL driver's own ~80 MB does not move).
    let viewport = egui::ViewportBuilder::default()
        .with_title(config::APP_NAME)
        .with_app_id("rustcord")
        .with_inner_size([1180.0, 760.0])
        .with_min_inner_size([720.0, 420.0])
        // Frameless: the 32 px title bar row is the window's title bar (ui/chrome.rs).
        .with_decorations(false)
        .with_icon(window_icon());
    // macOS: the native traffic lights over that row instead (chrome.rs draws no buttons there).
    #[cfg(target_os = "macos")]
    let viewport = viewport.with_decorations(true).with_fullsize_content_view(true).with_titlebar_shown(false).with_title_shown(false);
    #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
    let mut options = eframe::NativeOptions { viewport, renderer: eframe::Renderer::Glow, ..Default::default() };
    // Linux: X11 (XWayland) whenever there is a display: winit on Wayland can't hide, un-minimize
    // or focus a window, so closing to the tray and the tray's Open would not work.
    #[cfg(target_os = "linux")]
    if std::env::var_os("DISPLAY").is_some() {
        options.event_loop_builder = Some(Box::new(|b| {
            use winit::platform::x11::EventLoopBuilderExtX11;
            b.with_x11();
        }));
    }

    let result = eframe::run_native_ext(
        config::APP_NAME,
        options,
        Some(ctx),
        Box::new(move |cc| {
            trace_startup("window_created", started);
            ui::images::mem_trace("gl_ready");
            let (settings, snapshot, token) = preload.join().unwrap_or_default();
            trace_startup("preload_joined", started);
            // On the main thread with the event loop running (macOS' menu bar icon needs both).
            if !test_run {
                notify::start(&cc.egui_ctx);
            }
            update::start(rt.handle(), &cc.egui_ctx, settings.auto_update);
            let app = ui::RustcordApp::new(&cc.egui_ctx, rt, backend, settings, snapshot, token, started);
            ui::images::mem_trace("app_built");
            Ok(Box::new(app))
        }),
    );
    // After an update (Windows, Linux): the new version (on_exit has run; Linux: this process).
    update::relaunch();
    result
}

fn window_icon() -> egui::IconData {
    let img = image::load_from_memory(include_bytes!("../assets/icons/rustcord-64.png")).map(|i| i.to_rgba8());
    match img {
        Ok(img) => egui::IconData { width: img.width(), height: img.height(), rgba: img.into_raw() },
        Err(_) => egui::IconData::default(),
    }
}

/// `RUSTCORD_TRACE_STARTUP=<file>` appends "<label> <ms since start>" lines (for measurements).
pub fn trace_startup(label: &str, started: Instant) {
    if let Some(path) = std::env::var_os("RUSTCORD_TRACE_STARTUP") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{label} {:.1}", started.elapsed().as_secs_f64() * 1000.0);
        }
    }
}
