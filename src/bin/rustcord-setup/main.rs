//! Rustcord's installer and uninstaller: one exe, own code, no external tools. Per user like
//! Discord (no admin): the app goes to `%LOCALAPPDATA%\Programs\Rustcord` with `uninstall.exe`
//! (this program without the payload), a Start menu shortcut, an optional desktop shortcut,
//! the Apps & features entry and the sign-in autostart value. Window: `ui.rs`; the packed app:
//! `payload.rs`.
//!
//! Flags: `--silent` (no window, default options), `--dir <path>`, `--no-desktop`,
//! `--no-autostart`, `--uninstall` (with `--purge`: also local data and the saved login),
//! `--pack <rustcord.exe> <setup.exe>` (the build step of scripts/build-installer.ps1).
//! `--update`: the self-updater's silent update of the copy in the Apps entry's folder
//! (docs/UPDATER_DESIGN.md §11): shortcuts, the sign-in value and the running Rustcord are left
//! alone, nothing is started (the running app waits for the exit code, quits once no call runs
//! and starts the new app itself). Exit 0 = the new app is in place; on failure the previous
//! app is put back and the reason goes to `setup-error.txt` next to this exe.
//! `RUSTCORD_SETUP_TEST=1` or `--test-identity`: the test identity "RustcordSetupTest" with its
//! own default folder, Run value, shortcut and Apps entry names, for automated tests; it never
//! touches the real ones. `RUSTCORD_SCREENSHOT=<png>` with
//! `RUSTCORD_SETUP_SCREEN=install|progress|done|error|uninstall|removed` renders that screen
//! into the PNG and exits, without installing anything.
//! OWNERSHIP: desktop engineer.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg_attr(not(windows), allow(dead_code))]
mod payload;
#[cfg(windows)]
mod ui;
#[cfg(windows)]
#[path = "../../winreg.rs"]
mod winreg;

#[cfg(not(windows))]
fn main() {
    eprintln!("The Rustcord setup is for Windows.");
}

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    setup::main()
}

#[cfg(windows)]
mod setup {
    use crate::payload::{self, Payload};
    use crate::winreg;
    use std::path::{Path, PathBuf};
    use std::process::ExitCode;
    use std::sync::OnceLock;

    pub const APP_EXE: &str = "rustcord.exe";
    pub const UNINSTALL_EXE: &str = "uninstall.exe";
    pub const VERSION: &str = env!("CARGO_PKG_VERSION");
    const OLD_EXE: &str = "rustcord.exe.old";
    const NEW_EXE: &str = "rustcord.exe.new";
    pub const NO_PAYLOAD: &str ="This setup file has no Rustcord inside. Build it with scripts/build-installer.ps1.";
    const STILL_RUNNING: &str = "Rustcord is still running. Quit it from the tray icon, then try again.";
    const NOT_INSTALLED: &str = "Rustcord is not installed.";

    /// What gets installed under which names: the real app, or the test identity.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Identity {
        /// Folder, Run value, shortcut and Apps entry name; also the keyring service.
        pub id: &'static str,
    }

    pub const REAL: Identity = Identity { id: "Rustcord" };
    pub const TEST: Identity = Identity { id: "RustcordSetupTest" };

    impl Identity {
        fn uninstall_key(self) -> String {
            format!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{}", self.id)
        }

        fn default_dir(self) -> Result<PathBuf, String> {
            Ok(known::local_app_data()?.join("Programs").join(self.id))
        }

        /// Start menu and desktop shortcut.
        fn shortcuts(self) -> Result<[PathBuf; 2], String> {
            let name = format!("{}.lnk", self.id);
            Ok([known::start_menu()?.join(&name), known::desktop()?.join(name)])
        }

        /// The app's settings and cache folders (`directories`' layout, config.rs).
        fn data_dirs(self) -> Result<[PathBuf; 2], String> {
            Ok([known::roaming_app_data()?.join(self.id), known::local_app_data()?.join(self.id)])
        }
    }

    #[derive(Clone, Debug)]
    pub struct Options {
        pub id: Identity,
        pub dir: PathBuf,
        pub desktop: bool,
        pub autostart: bool,
        /// Uninstall: also the local data and the saved login.
        pub purge: bool,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Mode {
        Install,
        Uninstall,
    }

    pub fn main() -> ExitCode {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let has = |flag: &str| args.iter().any(|a| a == flag);
        let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1));
        if let Some(i) = args.iter().position(|a| a == "--pack") {
            return pack(args.get(i + 1), args.get(i + 2));
        }
        let test = has("--test-identity") || std::env::var_os("RUSTCORD_SETUP_TEST").is_some_and(|v| v == "1");
        let id = if test { TEST } else { REAL };
        if has("--update") {
            return update(&args, id);
        }
        let mode = if has("--uninstall") { Mode::Uninstall } else { Mode::Install };
        // An existing install keeps its folder.
        let dir = match value("--dir").map(PathBuf::from).or_else(|| winreg::get_string(&id.uninstall_key(), "InstallLocation").map(PathBuf::from)) {
            Some(dir) => std::path::absolute(&dir).unwrap_or(dir),
            None => match id.default_dir() {
                Ok(dir) => dir,
                Err(e) => return fail(&e),
            },
        };
        let opts = Options { id, dir, desktop: !has("--no-desktop"), autostart: !has("--no-autostart"), purge: has("--purge") };
        if !has("--silent") {
            return match crate::ui::run(mode, opts) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => fail(&e.to_string()),
            };
        }
        let (stub, payload) = payload::split(own_exe());
        let result = match (mode, payload) {
            (Mode::Install, Some(p)) => install(&opts, stub, &p, &mut |_| {}),
            (Mode::Install, None) => Err(NO_PAYLOAD.to_owned()),
            (Mode::Uninstall, _) => uninstall(&opts),
        };
        match result {
            Ok(()) => {
                if mode == Mode::Uninstall {
                    remove_self(&opts.dir);
                }
                ExitCode::SUCCESS
            }
            Err(e) => fail(&e),
        }
    }

    fn fail(e: &str) -> ExitCode {
        eprintln!("rustcord-setup: {e}");
        ExitCode::FAILURE
    }

    /// This program's own file (stub + payload), read once.
    pub fn own_exe() -> &'static [u8] {
        static OWN: OnceLock<Vec<u8>> = OnceLock::new();
        OWN.get_or_init(|| std::env::current_exe().and_then(std::fs::read).unwrap_or_default())
    }

    /// `--pack <app> <out>`: this program's stub with `app` packed in.
    fn pack(app: Option<&String>, out: Option<&String>) -> ExitCode {
        let (Some(app), Some(out)) = (app, out) else {
            return fail("usage: rustcord-setup --pack <rustcord.exe> <setup.exe>");
        };
        let result = std::fs::read(app).map_err(|e| format!("{app}: {e}")).and_then(|bytes| {
            if !bytes.starts_with(b"MZ") {
                return Err(format!("{app} is not a Windows program"));
            }
            let packed = payload::pack(payload::split(own_exe()).0, &bytes).map_err(|e| e.to_string())?;
            std::fs::write(out, packed).map_err(|e| format!("{out}: {e}"))
        });
        match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => fail(&e),
        }
    }

    fn write_error(path: &Path, e: impl std::fmt::Display) -> String {
        format!("Could not write {}: {e}", path.display())
    }

    /// `--update`'s flags (§11): `--dir` is refused, an update goes where the Apps entry says.
    pub fn update_args(args: &[String]) -> Result<(), String> {
        if args.iter().any(|a| a == "--dir") {
            return Err("--dir can't be used with --update.".to_owned());
        }
        Ok(())
    }

    /// One step of an install, in `plan` order.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Step {
        /// Ask a running Rustcord to quit and wait for it.
        QuitRunning,
        /// The new rustcord.exe (the previous one kept as rustcord.exe.old) and uninstall.exe.
        Files,
        StartMenuShortcut,
        /// Create (true) or remove the desktop shortcut.
        DesktopShortcut(bool),
        /// Write (true) or remove the sign-in value.
        Autostart(bool),
        AppsEntry,
        RemoveOld,
    }

    /// An install, or `update` (§11): the app quits and restarts itself, the user's shortcuts
    /// and sign-in choice stay as they are (a deleted shortcut is not re-created).
    /// rustcord.exe.old stays for the new app to delete, so a broken version can be rolled back.
    pub fn plan(o: &Options, update: bool) -> Vec<Step> {
        if update {
            vec![Step::Files, Step::AppsEntry]
        } else {
            vec![Step::QuitRunning, Step::Files, Step::StartMenuShortcut, Step::DesktopShortcut(o.desktop), Step::Autostart(o.autostart), Step::AppsEntry, Step::RemoveOld]
        }
    }

    /// Installs: the app, the uninstaller, shortcuts, the sign-in value and the Apps entry.
    /// `progress` gets the share done (0–1).
    pub fn install(o: &Options, stub: &[u8], p: &Payload<'_>, progress: &mut dyn FnMut(f32)) -> Result<(), String> {
        run(o, false, stub, p, progress)
    }

    /// Runs `plan`. A step that fails after the files were replaced puts the previous app back,
    /// except an update's Apps entry: the new files are in place, a stale DisplayVersion is only
    /// cosmetic.
    fn run(o: &Options, update: bool, stub: &[u8], p: &Payload<'_>, progress: &mut dyn FnMut(f32)) -> Result<(), String> {
        let exe = o.dir.join(APP_EXE);
        let mut placed = false;
        for step in plan(o, update) {
            let done = match step {
                Step::QuitRunning => {
                    quit_running(o);
                    Ok(())
                }
                Step::Files => put_files(o, stub, p, progress).map(|()| placed = true),
                Step::StartMenuShortcut => o.id.shortcuts().and_then(|[start_menu, _]| shortcut(&start_menu, &exe)),
                Step::DesktopShortcut(true) => o.id.shortcuts().and_then(|[_, desktop]| shortcut(&desktop, &exe)),
                Step::DesktopShortcut(false) => o.id.shortcuts().map(|[_, desktop]| {
                    let _ = std::fs::remove_file(desktop);
                }),
                Step::Autostart(on) => winreg::set_autostart(o.id.id, on.then_some(exe.as_path()), true),
                Step::AppsEntry => match apps_entry(o, (p.len + stub.len() as u64).div_ceil(1024).min(u32::MAX as u64) as u32) {
                    Err(e) if update => {
                        eprintln!("rustcord-setup: {e}");
                        Ok(())
                    }
                    done => done,
                },
                Step::RemoveOld => {
                    let _ = std::fs::remove_file(o.dir.join(OLD_EXE));
                    Ok(())
                }
            };
            if let Err(e) = done {
                if placed {
                    restore(&o.dir);
                }
                return Err(e);
            }
        }
        progress(1.0);
        Ok(())
    }

    /// Unpacks the app next to the old one first, so a failure leaves the old one untouched;
    /// then swaps them. A copy that still runs can't be overwritten, but it can be renamed.
    fn put_files(o: &Options, stub: &[u8], p: &Payload<'_>, progress: &mut dyn FnMut(f32)) -> Result<(), String> {
        let (exe, new, old) = (o.dir.join(APP_EXE), o.dir.join(NEW_EXE), o.dir.join(OLD_EXE));
        std::fs::create_dir_all(&o.dir).map_err(|e| write_error(&o.dir, e))?;
        let unpacked = std::fs::File::create(&new).and_then(|f| {
            let mut w = std::io::BufWriter::new(f);
            payload::unpack(p, &mut w, |f| progress(f * 0.9))?;
            w.into_inner().map_err(|e| e.into_error())?.sync_all()
        });
        if let Err(e) = unpacked {
            let _ = std::fs::remove_file(&new);
            return Err(write_error(&exe, e));
        }
        let _ = std::fs::remove_file(&old);
        if exe.exists() && std::fs::rename(&exe, &old).is_err() {
            let _ = std::fs::remove_file(&new);
            return Err(STILL_RUNNING.to_owned());
        }
        let uninstaller = o.dir.join(UNINSTALL_EXE);
        let tmp = o.dir.join("uninstall.exe.new");
        let swapped = std::fs::rename(&new, &exe)
            .map_err(|e| write_error(&exe, e))
            .and_then(|()| std::fs::write(&tmp, stub).and_then(|()| std::fs::rename(&tmp, &uninstaller)).map_err(|e| write_error(&uninstaller, e)));
        if swapped.is_err() {
            let _ = std::fs::remove_file(&tmp);
            restore(&o.dir);
        }
        swapped
    }

    /// Undoes `put_files`: the previous app (if there was one) replaces the new one.
    fn restore(dir: &Path) {
        let _ = std::fs::remove_file(dir.join(NEW_EXE));
        let old = dir.join(OLD_EXE);
        if old.exists() {
            let _ = std::fs::rename(old, dir.join(APP_EXE));
        }
    }

    fn apps_entry(o: &Options, size_kb: u32) -> Result<(), String> {
        let key = o.id.uninstall_key();
        for (name, value) in uninstall_values(o.id, &o.dir, size_kb) {
            match value {
                Value::Str(s) => winreg::set_string(&key, name, &s)?,
                Value::Dword(d) => winreg::set_dword(&key, name, d)?,
            }
        }
        Ok(())
    }

    /// `--update` (§11), always silent.
    fn update(args: &[String], id: Identity) -> ExitCode {
        let error_file = std::env::current_exe().ok().map(|e| e.with_file_name("setup-error.txt"));
        if let Some(f) = &error_file {
            let _ = std::fs::remove_file(f);
        }
        let result = update_args(args).and_then(|()| {
            let dir = winreg::get_string(&id.uninstall_key(), "InstallLocation").map(PathBuf::from).ok_or_else(|| NOT_INSTALLED.to_owned())?;
            let (stub, payload) = payload::split(own_exe());
            let p = payload.ok_or_else(|| NO_PAYLOAD.to_owned())?;
            run(&Options { id, dir, desktop: false, autostart: false, purge: false }, true, stub, &p, &mut |_| {})
        });
        let Err(e) = result else { return ExitCode::SUCCESS };
        if let Some(f) = &error_file {
            let _ = std::fs::write(f, format!("{e}\n"));
        }
        fail(&e)
    }

    /// Removes the app, its shortcuts, the sign-in value and the Apps entry (and with `purge`
    /// the local data and the saved login). The uninstaller itself goes in `remove_self`.
    pub fn uninstall(o: &Options) -> Result<(), String> {
        quit_running(o);
        for name in [APP_EXE, OLD_EXE, NEW_EXE] {
            match std::fs::remove_file(o.dir.join(name)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(STILL_RUNNING.to_owned()),
                _ => {}
            }
        }
        for link in o.id.shortcuts()? {
            let _ = std::fs::remove_file(link);
        }
        winreg::delete_value(winreg::RUN_KEY, o.id.id)?;
        winreg::delete_key(&o.id.uninstall_key())?;
        if o.purge {
            for dir in o.id.data_dirs()? {
                match std::fs::remove_dir_all(&dir) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("Could not delete {}: {e}", dir.display())),
                    _ => {}
                }
            }
            // The login token (src/auth.rs: service = app name, account "user-token").
            match keyring::Entry::new(o.id.id, "user-token").and_then(|e| e.delete_credential()) {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(e) => return Err(format!("Could not remove the saved login: {e}")),
            }
            // The app lock's PIN hash (src/lock.rs: account "app-lock-pin").
            match keyring::Entry::new(o.id.id, "app-lock-pin").and_then(|e| e.delete_credential()) {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(e) => return Err(format!("Could not remove the app lock: {e}")),
            }
        }
        Ok(())
    }

    /// Deletes the uninstaller and then the folder, if empty. A running exe can't delete
    /// itself, so when that is this process, cmd does it two seconds after it exited (as
    /// Squirrel, Discord's installer, does).
    pub fn remove_self(dir: &Path) {
        let uninstaller = dir.join(UNINSTALL_EXE);
        let me = std::env::current_exe().and_then(std::fs::canonicalize).ok();
        if me.is_none() || me != std::fs::canonicalize(&uninstaller).ok() {
            let _ = std::fs::remove_file(&uninstaller);
            let _ = std::fs::remove_dir(dir);
            return;
        }
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let system = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into())).join("System32");
        // `rd` without /s only removes an empty folder.
        let script = format!(r#"/d /c "ping -n 3 127.0.0.1 >nul & del /f /q "{}" & rd "{}"""#, uninstaller.display(), dir.display());
        let _ = std::process::Command::new(system.join("cmd.exe")).raw_arg(script).current_dir(&system).creation_flags(CREATE_NO_WINDOW).spawn();
    }

    /// Asks an installed Rustcord that is running to quit, and waits until it has
    /// (`rustcord.exe --quit`, src/main.rs).
    fn quit_running(o: &Options) {
        let exe = o.dir.join(APP_EXE);
        if !exe.exists() {
            return;
        }
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("--quit");
        if o.id == TEST {
            // A data dir of its own: the test copy can never reach the user's running Rustcord.
            cmd.env("RUSTCORD_HOME", o.dir.join("test-home"));
        }
        let _ = cmd.status();
    }

    #[derive(Debug, PartialEq)]
    pub enum Value {
        Str(String),
        Dword(u32),
    }

    /// The Apps & features entry (HKCU `…\Uninstall\<id>`).
    pub fn uninstall_values(id: Identity, dir: &Path, size_kb: u32) -> Vec<(&'static str, Value)> {
        let uninstaller = dir.join(UNINSTALL_EXE);
        // The uninstaller must act on the identity that installed it, whatever its environment.
        let test = if id == TEST { " --test-identity" } else { "" };
        vec![
            ("DisplayName", Value::Str(id.id.to_owned())),
            ("DisplayVersion", Value::Str(VERSION.to_owned())),
            ("Publisher", Value::Str("Rustcord".to_owned())),
            ("DisplayIcon", Value::Str(format!("{},0", dir.join(APP_EXE).display()))),
            ("InstallLocation", Value::Str(dir.display().to_string())),
            ("UninstallString", Value::Str(format!("\"{}\" --uninstall{test}", uninstaller.display()))),
            ("QuietUninstallString", Value::Str(format!("\"{}\" --uninstall --silent{test}", uninstaller.display()))),
            ("EstimatedSize", Value::Dword(size_kb)),
            ("NoModify", Value::Dword(1)),
            ("NoRepair", Value::Dword(1)),
        ]
    }

    /// A shortcut `lnk` to `exe` (IShellLinkW).
    fn shortcut(lnk: &Path, exe: &Path) -> Result<(), String> {
        use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, IPersistFile};
        use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
        use windows::core::{HSTRING, Interface};
        // SAFETY: COM calls on this thread with valid arguments; the interfaces release themselves.
        let made = unsafe {
            // Already initialized (a second shortcut) is fine.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            (|| {
                let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
                link.SetPath(&HSTRING::from(exe))?;
                link.SetWorkingDirectory(&HSTRING::from(exe.parent().unwrap_or(exe)))?;
                link.SetIconLocation(&HSTRING::from(exe), 0)?;
                link.SetDescription(&HSTRING::from("Rustcord"))?;
                link.cast::<IPersistFile>()?.Save(&HSTRING::from(lnk), true)
            })()
        };
        made.map_err(|e| format!("Could not create the shortcut {}: {}", lnk.display(), e.message()))
    }

    /// The user's shell folders.
    mod known {
        use std::path::PathBuf;
        use windows::Win32::System::Com::CoTaskMemFree;
        use windows::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_LocalAppData, FOLDERID_Programs, FOLDERID_RoamingAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
        use windows::core::GUID;

        fn folder(id: &GUID) -> Result<PathBuf, String> {
            // SAFETY: the returned string is copied, then freed with CoTaskMemFree as documented.
            unsafe {
                let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).map_err(|e| format!("Could not find a Windows folder: {}", e.message()))?;
                let path = p.to_string();
                CoTaskMemFree(Some(p.0 as *const _));
                path.map(PathBuf::from).map_err(|e| e.to_string())
            }
        }

        pub fn local_app_data() -> Result<PathBuf, String> {
            folder(&FOLDERID_LocalAppData)
        }
        pub fn roaming_app_data() -> Result<PathBuf, String> {
            folder(&FOLDERID_RoamingAppData)
        }
        pub fn start_menu() -> Result<PathBuf, String> {
            folder(&FOLDERID_Programs)
        }
        pub fn desktop() -> Result<PathBuf, String> {
            folder(&FOLDERID_Desktop)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn uninstall_entry_values() {
            let dir = Path::new(r"C:\Users\Ann\AppData\Local\Programs\Rustcord");
            let v = uninstall_values(REAL, dir, 12_345);
            let get = |name: &str| v.iter().find(|(n, _)| *n == name).map(|(_, v)| v).unwrap();
            assert_eq!(get("DisplayName"), &Value::Str("Rustcord".into()));
            assert_eq!(get("DisplayVersion"), &Value::Str(VERSION.into()));
            assert_eq!(get("Publisher"), &Value::Str("Rustcord".into()));
            assert_eq!(get("DisplayIcon"), &Value::Str(r"C:\Users\Ann\AppData\Local\Programs\Rustcord\rustcord.exe,0".into()));
            assert_eq!(get("InstallLocation"), &Value::Str(dir.display().to_string()));
            assert_eq!(get("UninstallString"), &Value::Str(r#""C:\Users\Ann\AppData\Local\Programs\Rustcord\uninstall.exe" --uninstall"#.into()));
            assert_eq!(get("QuietUninstallString"), &Value::Str(r#""C:\Users\Ann\AppData\Local\Programs\Rustcord\uninstall.exe" --uninstall --silent"#.into()));
            assert_eq!((get("EstimatedSize"), get("NoModify"), get("NoRepair")), (&Value::Dword(12_345), &Value::Dword(1), &Value::Dword(1)));
            // The test identity's uninstaller stays the test identity.
            let t = uninstall_values(TEST, dir, 1);
            assert!(matches!(&t[0].1, Value::Str(s) if s == "RustcordSetupTest"));
            assert!(t.iter().filter(|(n, _)| n.contains("UninstallString")).all(|(_, v)| matches!(v, Value::Str(s) if s.ends_with("--test-identity"))));
            assert_ne!(REAL.uninstall_key(), TEST.uninstall_key());
        }

        fn args(s: &str) -> Vec<String> {
            s.split_whitespace().map(str::to_owned).collect()
        }

        #[test]
        fn update_flags() {
            assert_eq!(update_args(&args("--update")), Ok(()));
            assert_eq!(update_args(&args("--update --test-identity")), Ok(()));
            assert!(update_args(&args(r"--update --dir C:\x")).is_err());
        }

        #[test]
        fn update_leaves_shortcuts_autostart_and_the_running_app_alone() {
            let o = Options { id: TEST, dir: PathBuf::from(r"C:\x"), desktop: true, autostart: true, purge: false };
            let install = plan(&o, false);
            assert_eq!(install, [Step::QuitRunning, Step::Files, Step::StartMenuShortcut, Step::DesktopShortcut(true), Step::Autostart(true), Step::AppsEntry, Step::RemoveOld]);
            let off = Options { desktop: false, autostart: false, ..o.clone() };
            assert!(plan(&off, false).contains(&Step::DesktopShortcut(false)) && plan(&off, false).contains(&Step::Autostart(false)));
            // Whatever the options say: no quit, no shortcut or sign-in writes, .old kept, and
            // nothing started: the app restarts itself once no call runs.
            assert_eq!(plan(&o, true), [Step::Files, Step::AppsEntry]);
        }
    }
}
