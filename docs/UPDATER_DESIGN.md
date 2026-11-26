# Rustcord self-updater: design

Status: implemented for v0.1.0. This file describes the updater as built, with the design
review's changes folded in; §16 says how it was tested and §18 what it does not do. Module map
and public API: §17.

## 0. What it does, in one paragraph

Every installed Rustcord checks a small signed file on GitHub
(`releases/latest/download/latest.json` + `.sig`) in the background at start and every 6 hours.
The file names the newest version and, per platform, one package with its SHA-256 and size.
After the Ed25519 signature checks out, a newer version is downloaded into the cache dir,
hashed, and staged. It is installed either right away behind a full-window update screen
(found in the first 3 s, before the user did anything; or after a click on the title-bar
"Update ready" button), silently after Rustcord has waited 10 minutes in the tray (never during
a call, never on macOS), or at the next start behind the same screen. Rustcord then restarts into the new version. Nothing
ever blocks the first frame, a failure never blocks the app, and portable / dev / read-only
copies only say that a new version exists.

Not in scope: delta updates, a beta channel, rollback to older versions, an update log file,
a "never check" switch (the decision is: the check always runs; only installing is optional).

---

## 1. Fixed decisions (recap; not reopened)

- Repo `https://github.com/r0sc/Rustcord` (public). Version 0.1.0, first tag `v0.1.0`.
- Feed: `https://github.com/r0sc/Rustcord/releases/latest/download/latest.json` and
  `…/latest.json.sig`. No GitHub REST API. `releases/latest` skips drafts and prereleases.
- Ed25519. Public key (raw 32 bytes, standard base64), one line in
  `assets/update-signing.pub`, `include_str!`-ed by the app and read by CI:
  `xhWhAUDE17E0HTQVGomlSG7pgrr517sISogYm9/PvxY=`.
  `latest.json.sig` = standard base64 (one line) of the 64-byte signature over the exact bytes
  of `latest.json`. The private key exists only as the Actions secret
  `RUSTCORD_UPDATE_SIGNING_KEY` (PEM PKCS#8).
- Crates: `ed25519-dalek 2.2` and `semver 1.0` (already in Cargo.lock), `sha2 0.11`, `base64
  0.23`, `flate2` (direct deps). No new crate in Cargo.lock. System `tar` is used for archives.
  `hex` is **not** a direct dep: format hashes with `format!("{b:02x}")` (as `notify.rs` does).

---

## 2. Release assets

| Platform key (manifest) | File in the release | Built by | Who installs it |
|---|---|---|---|
| `windows-x86_64` | `Rustcord-Setup-<v>.exe` | scripts/build-installer.ps1 (exists) | the setup, `--update` (§11) |
| `linux-x86_64` | `rustcord-<v>-linux-x86_64.tar.gz` (top folder `rustcord-<v>/`) | scripts/build-linux.sh (exists) | its own `install.sh --update` (§12) |
| `linux-x86_64-deb` | `rustcord_<v>_amd64.deb` | scripts/build-linux.sh (exists) | `pkexec apt-get install -y` after a click |
| `macos-universal` | `Rustcord-<v>-macos-universal.app.tar.gz` (top entry `Rustcord.app/`) | scripts/build-macos.sh (**new output**) | the app swaps bundles (§9.4) |
| (humans only) | `Rustcord-<v>-macos-universal.dmg` | scripts/build-macos.sh (exists) | drag to Applications |
| feed | `latest.json`, `latest.json.sig` | release job (§13) | – |
| checksums | `SHA256SUMS` (all files above, incl. `latest.json*`) | release job (exists) | – |

`<v>` is Cargo.toml's version, plain `X.Y.Z` (the release job fails on anything else).

**Frozen interfaces.** An installed app runs the *old* app's apply code against the *new*
release, so these never change incompatibly (add, never rename or remove):
- the feed URLs, manifest `schema: 1` and its field names, the platform keys and file-name
  templates above, the archive layouts (`rustcord-<v>/install.sh`, `Rustcord.app/`);
- `Rustcord-Setup-<v>.exe --update [--test-identity]`: exit 0 = the new app is in place and
  nothing was started (the running app starts it once it quit, §9.1); non-zero = the previous
  app is back and one line is in `setup-error.txt` next to the setup exe. A later setup must
  never start the app on `--update` (the old app would start a second copy);
- `sh install.sh --update`;
- the app flag `--after-update` (and `--minimized`).

---

## 3. Manifest

### 3.1 Schema (version 1)

```text
{
  "schema":    1,                       // integer; the app rejects any other value
  "version":   "X.Y.Z",                 // semver without prerelease/build suffix, no leading "v"
  "platforms": {                        // object; keys from §2; missing key = no package for it
    "<key>": {
      "url":    "<string>",             // must equal the expected URL (§3.3) byte for byte
      "sha256": "<64 lowercase hex>",
      "size":   <integer>               // bytes, 1 ..= 209715200 (200 MiB)
    }
  }
}
```
Unknown fields are ignored (forward compatible). The generator writes
`json.dumps(obj, indent=2, sort_keys=True) + "\n"`, UTF-8, LF line ends (§14), but the app
never depends on formatting: it verifies the signature over the bytes it received, then
parses them.

### 3.2 Full example for v0.1.0

The hashes and sizes below are illustrative (the real ones come from the CI build); this exact
text is also the parser test fixture (`feed.rs` tests).

```json
{
  "platforms": {
    "linux-x86_64": {
      "sha256": "9f2c4e1a7b3d5c6e8f0a1b2c3d4e5f60718293a4b5c6d7e8f9a0b1c2d3e4f5a6",
      "size": 16003285,
      "url": "https://github.com/r0sc/Rustcord/releases/download/v0.1.0/rustcord-0.1.0-linux-x86_64.tar.gz"
    },
    "linux-x86_64-deb": {
      "sha256": "0a1b2c3d4e5f60718293a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5",
      "size": 11452804,
      "url": "https://github.com/r0sc/Rustcord/releases/download/v0.1.0/rustcord_0.1.0_amd64.deb"
    },
    "macos-universal": {
      "sha256": "c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00",
      "size": 31457280,
      "url": "https://github.com/r0sc/Rustcord/releases/download/v0.1.0/Rustcord-0.1.0-macos-universal.app.tar.gz"
    },
    "windows-x86_64": {
      "sha256": "5d41402abc4b2a76b9719d911017c5925d41402abc4b2a76b9719d911017c592",
      "size": 24187210,
      "url": "https://github.com/r0sc/Rustcord/releases/download/v0.1.0/Rustcord-Setup-0.1.0.exe"
    }
  },
  "schema": 1,
  "version": "0.1.0"
}
```

### 3.3 How the app reads it (order matters)

1. GET `latest.json` (cap 64 KiB) and `latest.json.sig` (cap 512 bytes). HTTP status must be 200.
2. Signature: `sig = base64::STANDARD.decode(sig_text.trim())`, must be 64 bytes;
   `key = base64 decode of assets/update-signing.pub (trimmed)`, must be 32 bytes;
   `VerifyingKey::from_bytes(&key)?.verify_strict(&json_bytes, &Signature::from_bytes(&sig))`.
   **No field is read before this succeeds.**
3. Parse JSON (serde). `schema == 1`. `semver::Version::parse(version)` with empty `pre` and
   `build`.
4. Newer? `manifest_version > running_version` (semver precedence). Running version =
   `CARGO_PKG_VERSION`, or `RUSTCORD_UPDATE_FROM` in loopback test mode (§5). A running
   prerelease build (e.g. `0.2.0-beta.1`) therefore updates to `0.2.0`; there is no beta channel.
5. Pick the platform entry (§6 decides the key). Missing → the version is offered as a notice
   only (§8). Present → validate: `url == asset_base + file`, where
   - `asset_base = "https://github.com/r0sc/Rustcord/releases/download/v<version>/"`, or in
     loopback test mode the feed URL with its last path segment removed
     (`http://127.0.0.1:8765/latest.json` → `http://127.0.0.1:8765/`);
   - `file` = the template from §2 for that key and `<version>`;
   - `sha256` is 64 lowercase hex, `1 <= size <= 200 MiB`.
   A signed manifest with an invalid entry is a release bug: treated like a failed check.

### 3.4 Fetching

- One `reqwest::Client` per check / download (dropped afterwards: no idle RAM), built with:
  `user_agent("Rustcord/<CARGO_PKG_VERSION> (+https://github.com/r0sc/Rustcord)")`,
  `no_gzip()` (byte-exact bodies), `connect_timeout(10 s)`, `read_timeout(30 s)`, a 30 s
  total `timeout` on the two feed requests only, and
  `redirect::Policy::custom`: at most 10 hops, every hop `https` (loopback test mode: also
  `http` to a loopback host). GitHub answers `releases/latest/download/x` with 302 →
  `releases/download/v<v>/x` → 302 → its asset CDN; that is fine because integrity comes from
  the signature and the hash, not the host.
- Never Discord headers, the token, super-properties or cookies. Proxies from the environment
  are allowed (TLS + signature still hold).
- Results: transport error or 5xx → "check failed", retry in 15 min. 4xx (e.g. 404 before the
  first published release) → "no feed", next check in 6 h. Bad signature / bad manifest →
  "check failed", retry in 15 min (see §4: no download happened, so there is nothing to
  delete; a retry costs two tiny requests and covers the race where `latest.json` and `.sig`
  come from two different releases while one is being published).

### 3.5 Downloading

- GET the entry's `url`; stream with `Response::chunk()` into `<cache>/update/<file>.part`
  (file I/O and hashing in `spawn_blocking` or in chunks ≤ 64 KiB on the runtime; never on the
  UI thread). Abort as soon as more than `size` bytes arrive or `Content-Length > size`.
- At the end: byte count `== size` and SHA-256 `==` `sha256`, else delete the `.part`, record
  `bad_manifest = sha256(latest.json bytes)` and fail ("damaged"). Only a complete answer (the
  body ended, or `Content-Length > size`) counts as damaged; a connection that breaks mid-way is
  a network error and sets nothing. A later automatic check that returns the very same
  `latest.json` bytes is ignored silently; "Check for updates" ignores `bad_manifest` once; a
  changed manifest is tried again.
- Success: rename `.part` → `<file>`, write `staged` into `state.toml` (§7).
- Progress goes to `Phase::Downloading { done, total: size }`, at most every 100 ms.
- Before applying a staged file that was verified in an earlier run, hash it again
  (`size` + `sha256` from `state.toml`).

---

## 4. Threat model

| Threat | Mitigation |
|---|---|
| Network attacker (Wi-Fi, proxy, TLS-intercepting middlebox) | Signature verified before any field is trusted; per-file SHA-256 + size bound by the signed manifest; TLS (native-tls) on top. |
| Attacker uploads assets to the release / swaps `latest.json` (stolen GitHub session) | Can't sign without the key; the app rejects the manifest. |
| Attacker with repo write access changes the workflow to sign their build with the secret | The secret is used only in the tag-triggered release job; the release is created as a **draft** and published by hand. Recommended hardening (a GitHub setting, done by the owner, not by this work): move the secret into an environment `release` with a required reviewer and protect `v*` tags. |
| Downgrade / replay of an old, validly signed manifest | Only `version > running` is accepted. |
| Freeze (serving an old manifest forever) | Accepted risk: the app simply sees no update. |
| Manifest points the download elsewhere | URL must equal `https://github.com/r0sc/Rustcord/releases/download/v<version>/<expected file>`. Redirects must stay on https. |
| Huge responses / disk fill | Caps: 64 KiB manifest, 512 B signature, 200 MiB per asset and the manifest's `size`; download aborts past it. Disk-full is an ordinary error. |
| Env var tricks (`RUSTCORD_UPDATE_FEED`, `_PUBKEY`) | Accepted only for a loopback feed (127.0.0.1, [::1], localhost); the key override only together with it; then assets must come from that same loopback base. No env var can point a release build at another host or another key. |
| Archive path traversal | Archives are only extracted after their hash matched the signed manifest, into a fresh empty folder; GNU tar and bsdtar also strip leading `/` and refuse `..` by default. |
| Same-user local attacker (can write the cache dir, swap the staged file, plant DLLs next to the staged setup, or race the `.deb` before `pkexec`) | Out of scope: such an attacker can already replace the user-writable installed exe, autostart entries or shell config. Mitigations that are free are still done: re-hash before applying, absolute paths for every spawned program, no shell. The `.deb` + `pkexec` race is noted as accepted risk. |
| Token leakage | The updater never touches `auth`; separate client; logs contain versions, file names and error kinds only. |
| Privacy | GitHub sees the IP address and `Rustcord/<version>` every 6 h. Documented in the README. |
| Key compromise | Rotation: release a version whose `assets/update-signing.pub` holds the new key, signed with the old key; sign everything after it with the new key. Copies older than that release keep trusting the old key only. |

Signature failures do not set `bad_manifest` (see §3.4). Hash/size failures do.

---

## 5. When the updater runs (modes and hooks)

Computed once in `update::start` from the environment and the build type (pure function, unit
tested with a table of cases):

| Condition (first match wins) | Mode |
|---|---|
| `RUSTCORD_UPDATE_FEED` set: a valid loopback `http(s)://{127.0.0.1,[::1],localhost}[:port]/…` URL (the parsed host decides; no user info, query or fragment), `RUSTCORD_UPDATE_PUBKEY` (if set) a valid Ed25519 key in base64, `RUSTCORD_UPDATE_FROM` (if set) a version, and every other `RUSTCORD_*` variable is one of `RUSTCORD_HOME`, `RUSTCORD_SETUP_TEST`, `RUSTCORD_SETUP_PREFIX`, `RUSTCORD_OFFLINE`, `RUSTCORD_TRACE_STARTUP`, `RUSTCORD_SCREENSHOT`, `RUSTCORD_SCREENSHOT_FRAME`, `RUSTCORD_WINDOW_SIZE` | **Test**: full updater against the loopback feed (debug builds too), test install identities accepted (§6) |
| `RUSTCORD_UPDATE_FEED` set but invalid (non-loopback, bad URL, bad key, another hook) | **Off** (log one line why) |
| any other `RUSTCORD_*` set without a test feed (`OFFLINE`, `SCREENSHOT`, `UPDATE_DEMO`, `TRACE_STARTUP`, `OPEN`, `UPDATE_PUBKEY`/`UPDATE_FROM`, …) except only `RUSTCORD_HOME` | **Off** |
| debug build | **Off** |
| only `RUSTCORD_HOME` set (portable data) | **Notice**: checks GitHub, never downloads |
| otherwise | **Full**: checks GitHub; downloads/installs if this copy is installed (§6) |

Off: no task, no network, `Status { enabled: false, .. }`. Notice, or Full on a copy that is
not installed: `Kind::NoticeOnly`.

`RUSTCORD_OFFLINE` is accepted next to a test feed because an end-to-end run of the real app
must neither read the saved login (the keyring entry is shared by every data dir) nor reach
Discord (fonts, activity table, login); the feed is loopback only anyway. Without a test feed
it still turns the updater off. `RUSTCORD_TRACE_STARTUP` is accepted there to time the first
frame against a slow or dead feed, the screenshot hooks to capture the real update screen.

Test-only hooks (all ignored outside Test mode):
- `RUSTCORD_UPDATE_FEED=<loopback URL of latest.json>`; the signature is `<that URL>.sig`.
- `RUSTCORD_UPDATE_PUBKEY=<base64 of 32 bytes>`: replaces the embedded key.
- `RUSTCORD_UPDATE_FROM=<X.Y.Z>`: the version this run pretends to be (for the comparison and
  the loop guard). It is removed from the environment of every program the updater starts
  (setup, install.sh, apt, the relaunched app), so the restarted copy reports its real version.

UI-only hook (no network, no files; makes the mode Off): `RUSTCORD_UPDATE_DEMO=<state>` (§15.4).

---

## 6. Install kind detection

Done in `update::start` from `std::env::current_exe()` **once**; the path is kept, because on
Linux `current_exe()` turns into `… (deleted)` after the binary is replaced. Reuse
`autostart.rs` (`installed_copy` / `unix_installed`, made `pub(crate)`).

| Platform | Condition | Kind | Platform key |
|---|---|---|---|
| Windows | exe folder == `InstallLocation` of `HKCU\…\Uninstall\Rustcord` (`autostart::in_install_dir`) and that folder is writable (probed in the task, not in `start`; an elevated `--dir` install into Program Files is not) | Auto (else NoticeOnly) | `windows-x86_64` |
| Windows, Test mode | … or of `…\Uninstall\RustcordSetupTest` (then the setup gets `--test-identity`) | Auto | same |
| Linux | exe == `$HOME/.local/bin/rustcord`, both sides canonical (a symlinked `/home` still matches; Test mode with `RUSTCORD_SETUP_TEST=1`: `$RUSTCORD_SETUP_PREFIX/.local/bin/rustcord`) | Auto | `linux-x86_64` |
| Linux | exe == `/usr/bin/rustcord` and `/var/lib/dpkg/info/rustcord.list` exists | Package | `linux-x86_64-deb` |
| macOS | exe == `<parent>/<Name>.app/Contents/MacOS/rustcord`, `unix_installed` (parent `/Applications` or `~/Applications`; Test mode: any parent) and `libc::access(parent, W_OK) == 0` | Auto | `macos-universal` |
| macOS | same bundle, parent not writable | NoticeOnly | – |
| anything else (dev build in a target dir, `/opt`, a copied exe, non-x86_64 Linux/Windows) | | NoticeOnly | – |

Only x86_64 Windows and Linux packages exist; another arch gets `NoticeOnly`. A manifest
without our key makes the offer `NoticeOnly` too (`Status::kind` is the kind of the *current
offer*).

---

## 7. Files

Everything lives in `<cache>/update/` (`config::paths().cache_dir.join("update")`), never in the
roaming config dir (Windows roams `%APPDATA%`; this state is per machine). Logout's
`cache::clear_all` does not touch it.

| OS | Folder |
|---|---|
| Windows | `%LOCALAPPDATA%\Rustcord\cache\update\` |
| Linux | `~/.cache/rustcord/update/` |
| macOS | `~/Library/Caches/Rustcord/update/` |
| `RUSTCORD_HOME=<d>` | `<d>/cache/update/` |

Contents:
- `<file>.part`: a download in progress (deleted at start and on failure).
- `<file>`: the verified, staged package.
- `extract-<v>/`: Linux tarball extraction (deleted after install.sh).
- `setup-error.txt`: one line written by the Windows setup when `--update` failed (§11),
  read and deleted by the waiting app right after the setup's exit (or by the next start).
- `state.toml` (via `config::read_toml` / `write_toml`, atomic):

```toml
attempted = "0.1.1"   # apply started for this version; checked at the next start (loop guard)
blocked = "0.1.1"     # did not install: never applied automatically again (a click still may)
bad_manifest = "<sha256 hex of a latest.json whose package failed its hash>"

[staged]              # the verified package waiting in this folder
version = "0.1.1"
sha256 = "<64 hex>"
size = 24187210
```
The file is user-writable, so every value is checked on load (plain `X.Y.Z`, 64 lowercase
hex, `1 ..= 200 MiB`) and an invalid one counts as absent. The staged file's name is never
stored: it follows from the version and this copy's platform key (§2 template), so a
hand-edited `state.toml` can't point the updater at another path. `prune` keeps only
`state.toml` and that file.

macOS bundle swap folders sit next to the bundle (same volume, so renames are atomic):
`<parent>/.Rustcord-update-<v>/` (extraction) and `<parent>/.Rustcord-old-<running v>`
(the previous bundle, deleted by the new version after it started). Cleanup only ever removes
names with these exact prefixes.

---

## 8. State machine

### 8.1 Shared state (exact type: `src/update/state.rs`)

`Status { phase, kind, screen, enabled, note }`. `phase` is one of
`Idle, Checking, UpToDate, Available{version}, Downloading{version, done, total},
Ready{version}, Installing{version}, Restarting{version}, Manual{version, path}, Failed{message}`.
`screen == true` means the full-window update screen covers the app. `note` is the last
background download error while the phase went back to `Available` (shown in the settings),
cleared by the next phase change.

Inputs from the UI each `logic` pass (`sync_ui`): `hidden` (the window waits in the tray),
`busy` (in or joining a voice channel or call, a call I just started, a call ringing me, or,
only while hidden, a non-empty draft, which a restart would lose; screen share and camera imply
a call), `interacted` (a key or pointer button was pressed since start, sticky). Until the first
`sync_ui`, `hidden` = `--minimized` in argv.

Internal flags (core-private): `user_wants` (set by the title-bar button / Install, cleared by
dismiss), `hidden_since`, the retry count, and the saved `blocked`, `attempted`,
`bad_manifest`, `staged`.

Constants (`machine.rs`, `platform.rs`): `START_WINDOW = 3 s`, `SCREEN_DOWNLOAD_MAX = 10 s`,
`CHECK_EVERY = 6 h`, `RETRY_AFTER = 15 min` (download retries back off 15 min → 1 h → 6 h),
`ERROR_SCREEN = 5 s`, `HIDDEN_WAIT = 10 min`, `CLAIM_WAIT = 30 s` (main.rs), `STEP_WAIT = 120 s`
(tar, install.sh, open, the Windows setup), `PKEXEC_WAIT = 10 min`.

### 8.2 At start (`update::start`, synchronous part, < 1 ms: env, argv, one registry read or
a few `stat`s, `state.toml`; no hashing, no network)

1. Mode (§5). Off → done.
2. Kind (§6).
3. **Loop guard**: if `attempted = A`:
   - `running >= A` → success: clear `attempted`, `blocked` if it equals A, and `staged`
     (delete its file); schedule leftover cleanup (§7) in the task.
   - `running < A` → it did not install: `blocked = A`, clear `attempted` and `staged`
     (delete the file); `phase = Failed { "Rustcord <A> did not install[: <setup-error.txt
     line>]. Version <running> keeps running." }`, `screen = !hidden`. The first check waits
     until that error is gone (5 s or a dismiss).
   Save `state.toml`.
4. **Staged** `S` (file exists, size matches, `S > running`):
   - `S != blocked && auto && kind == Auto && !(macOS && hidden)` → `phase = Installing{S}`,
     `screen = !hidden`, the task applies it (re-hash first) **instead of checking**. Nothing
     is in memory yet, so a hidden start installs too, except on macOS (§9.4).
   - otherwise → `phase = Ready{S}` (title-bar button).
   - `S <= running` or file missing → delete, clear.
5. Spawn the task on the given runtime: cleanup, then (unless applying) check now.

### 8.3 Check result → next phase

| Result | Next |
|---|---|
| check failed (network, 5xx, signature, invalid manifest) | phase unchanged if it was Available/Ready/Manual/Downloading; else `Failed{msg}` only when the check came from "Check for updates", otherwise back to the previous phase; next check in 15 min |
| 4xx | as above, next check in 6 h |
| `latest.json` bytes hash == `bad_manifest` | unchanged, silent |
| `V <= running` | `UpToDate`; delete a stale staged file |
| `V == staged` | stay `Ready{V}` |
| no entry for our key, or kind NoticeOnly | `Available{V}`, `kind = NoticeOnly` |
| `V == blocked`, or `auto == false` | `Available{V}` (the button downloads on click) |
| otherwise (Auto or Package, auto on) | `Downloading{V}`; `screen = kind == Auto && !hidden && since_start < 3 s && !interacted`; this automatic screen is not a user wish (`user_wants = false`): it hands off to the background after 10 s, a dismiss hides it |

A newer `V` while an older version is staged replaces it (delete, download `V`).
`Checking` is only shown when nothing is pending (Idle, UpToDate, Failed); otherwise the check
runs without changing the phase. A check result that arrives after a click or a silent install
started a download or install never replaces that work.

### 8.4 Transitions

```text
                 ┌──────── check ok, not newer ───────► UpToDate
 Idle ─check─► Checking ── newer, auto off / blocked ─► Available ──click──┐
   ▲             │      ── newer, notice only ───────► Available ──click──► (opens release page)
   │             └───── newer, auto on ─────────────► Downloading ◄───────┘ (click: user_wants; screen unless busy)
   │                                                      │  progress (≤10 Hz); dismiss/10 s on auto screen → screen off
   │       dl error on screen ◄───────────────────────────┤  dl error in background → Available (+ note), retry later
   │               │                                      ▼ verified, staged
   │               ▼                                    Ready ──(Package, no pkexec/apt-get)──► Manual
   │            Failed                                     │  click / Install / 10 min hidden / next start (never busy)
   │  dismiss / 5 s: Ready if staged,  ▲                   ▼
   └──── Available if offer known, else Idle ┴── error ── Installing ──ok──► Restarting ──► app quits
```

Entering `Installing` is one function (`begin_apply`) used by every path, and it refuses while
`busy`: the update screen never covers a call.

| From | Event | Guard | To | Side effects |
|---|---|---|---|---|
| any non-busy phase | timer (6 h / retry) or `check_now` | not Downloading/Installing/Restarting | Checking (or silent) | fetch + verify |
| Available | `button_clicked` | kind Auto/Package | Downloading, `user_wants = true`, `screen = !busy` | download (during a call: in the background, installs after it) |
| Available | `button_clicked` | kind NoticeOnly | unchanged | `ctx.open_url("https://github.com/r0sc/Rustcord/releases/tag/v<V>")` |
| Downloading | progress | | Downloading | wake UI |
| Downloading | 10 s on screen | `screen && !user_wants` (the automatic start screen) | Downloading, `screen = false` | |
| Downloading | `dismiss` | | Downloading, `screen = false`, `user_wants = false` | |
| Downloading | done + verified | Package && (`/usr/bin/pkexec` or `/usr/bin/apt-get` missing) | `Manual{V, path}` (screen as it was) | |
| Downloading | done + verified | (`user_wants` or the screen is up) && `busy` | `Ready{V}`, `screen = false` | the button waits: "Installs after your call ends" |
| Downloading | done + verified | (`user_wants` or the screen is up) && `!busy` | Auto: Installing (screen unless hidden); Package: `Ready`, `screen = true` (Install button) | |
| Downloading | done + verified | otherwise | `Ready{V}` | title-bar button |
| Downloading | network / disk error | screen up | `Failed{msg}` | delete `.part`; after dismiss / 5 s → `Available{V}` |
| Downloading | network / disk error | in the background | `Available{V}`, `note = msg` | delete `.part`; retry at the next check: 15 min, then 1 h, then 6 h |
| Downloading | size / hash mismatch | | as above with the "damaged" message | delete `.part`, set `bad_manifest` |
| Ready | `button_clicked` | kind Auto, `!busy` (the button is disabled while busy) | Installing, `screen = true` | apply |
| Ready | `button_clicked` | kind Package | Ready, `screen = true` | screen shows Install (disabled while busy) |
| Ready | `install` | kind Package, `!busy` | Installing | `pkexec` |
| Ready | `busy` turns false | kind Auto, `user_wants` | Installing, `screen = !hidden` | |
| Ready | timer / `sync_ui` | hidden for `HIDDEN_WAIT` without a break, `!busy`, auto, kind Auto, `V != blocked`, not macOS, screen down | Installing (silent) | |
| Manual | `button_clicked` | | Manual, `screen = true` | screen shows the path |
| Installing | apply ok | | `Restarting{V}`, `screen = !hidden && !busy` | Windows, Linux: quit once `!busy`, then start the new version (`relaunch`); macOS: quit now (`open` already started it) |
| Installing | apply error | | `Failed{msg}` | clear `attempted`; `blocked = V` (never for Package); after dismiss / 5 s → `Ready{V}` |
| Installing | pkexec 126 / timeout | | `Failed{no password}` | clear `attempted`, no block; after dismiss → `Ready{V}` |
| Installing | pkexec 127, no pkexec | | `Manual{V, path}` | |
| Installing | staged file no longer matches | | `Failed{damaged}` | forget `staged` |
| Failed | `dismiss`, or 5 s while `screen` | | Ready / Available / Idle (see diagram) | `screen = false` |
| any | `dismiss` | screen up and phase not Installing/Restarting | same phase, `screen = false` | |
| any | `set_auto(on)` | | unchanged | applies to future decisions; a running download finishes |

**Apply** (any platform): write `attempted = V` first and apply only when that save
succeeded (else `Failed("Could not save the update: <e>.")`: without it a package that does not
install would be applied at every start); decide `--minimized` right before starting the new
process from the *current* `hidden`; never start while `busy`.

**Loop guard summary.** `attempted` is written before the new version is started and checked
by whichever version runs next (§8.2 step 3). A failed install sets `blocked`, so the same
version is never attempted twice in a row automatically; the error shows once; the title-bar
button ("Update ready" while the package is still staged, else "Update available") still lets
the user try that version by hand; a newer version is handled normally.

### 8.5 The task

One tokio task on the app's runtime (`rt.handle()`) loops on
`select!(events.recv(), sleep_until(engine.next_wake()))`. The pub fns (§17) send events over an
unbounded channel; `sync_ui` stores atomics and sends an event only when a value changed. The
machine (`machine.rs`) turns events into actions; the task runs them: network on the runtime,
blocking work (hashing, tar, `install.sh`, `pkexec`, the setup, renames) in `spawn_blocking`.
Every status change wakes the UI with `ctx.request_repaint_after(1 ms)` (ARCHITECTURE.md wake
rule); download progress at most every 100 ms. Idle cost: one sleeping timer.

---

## 9. Apply, per platform

`<u>` = `<cache>/update`, `V` = new version, `exe` = the path captured at start, `min` =
`--minimized` when hidden at that moment. Every program is started with `std::process::Command`
(absolute path where it is a system tool, arguments as args, no shell),
`env_remove("RUSTCORD_UPDATE_FROM")`, stdin/stdout null (stderr read for the error line where
noted), and waited for with a deadline (`STEP_WAIT` 120 s, pkexec 10 min); a child past it is
killed and the install fails (`Failed`, dismissable, continues after 5 s). The staged file is
hashed again before anything runs.

### 9.1 Windows (installer copy)

1. `attempted = V`.
2. Run `<u>\Rustcord-Setup-V.exe --update [--test-identity]` and wait for its exit code (at
   most 120 s) with the Installing screen up; the old app keeps running meanwhile, so there is
   never a moment without a window. The setup replaces the exe and rewrites the Apps entry; it
   starts nothing (§11).
3. Exit 0 → `Restarting{V}`, `notify::request_quit()` once `!busy` (a call joined while the
   setup ran is never cut off). After the event loop has ended, `main.rs` calls
   `update::relaunch()`, which spawns `exe --after-update [min]`; it waits for this one's
   instance lock (§10).
   Non-zero or timeout (the setup is killed) → `Failed("Could not install the update:
   <setup-error.txt line, or the exit code>.")`; a spawn error → `Failed("Could not start the
   update: <e>.")`. Either way `blocked = V`; this copy keeps running, nothing restarts.

(No Mark of the Web is set on files reqwest writes, so SmartScreen does not prompt for the
staged setup.)

### 9.2 Linux, tarball install (`~/.local`)

1. `attempted = V`.
2. `rm -rf <u>/extract-V; mkdir <u>/extract-V`;
   `tar -xzf <u>/rustcord-V-linux-x86_64.tar.gz -C <u>/extract-V` (`/usr/bin/tar` or `/bin/tar`:
   GNU tar; neither → `Failed("Could not install the update: tar was not found.")`).
3. `/bin/sh <u>/extract-V/rustcord-V/install.sh --update` (inherits the environment, so the
   test variables reach it). Non-zero → `Failed("Could not install the update: <last stderr
   line, max 200 chars>.")`, `blocked = V`.
4. Delete `extract-V`.
5. `Restarting{V}`, `notify::request_quit()` (once `!busy`). After the event loop has ended
   (`on_exit` saved the cache, removed the tray icon and the socket; the listener fd is
   `CLOEXEC`), `main.rs` calls `update::relaunch()`, which `exec`s
   `exe --after-update [min]` (`exe` = `~/.local/bin/rustcord`, the new file now) in the
   same process; if `exec` fails it spawns it instead. `relaunch` does nothing after a Quit by
   the user (the restart was still waiting for a call to end): the new version then runs at
   the next start.

`exec`, not spawn + quit: XDG autostart (on by default) runs Rustcord as a systemd unit
(`app-rustcord@autostart.service`); before systemd 250 such units stop their whole cgroup when
the main process exits, which would kill a spawned new copy. `exec` keeps the PID.

### 9.3 Linux, .deb (`/usr`, Package kind; only after a click on Install)

1. Requires `/usr/bin/pkexec` and `/usr/bin/apt-get`; otherwise the phase is `Manual` (§8.3).
2. `attempted = V`.
3. `/usr/bin/pkexec /usr/bin/apt-get -o DPkg::Lock::Timeout=120 install -y <u>/rustcord_V_amd64.deb`
   (absolute file path; the system's polkit dialog asks for the password; the lock timeout
   waits out unattended-upgrades). Deadline 10 min.
   - exit 0 → `Restarting`, quit, then `exec /usr/bin/rustcord --after-update [--minimized]`
     as in §9.2.
   - exit 126 (dialog dismissed) or the deadline → clear `attempted`, `Failed("The update was
     not installed: no password was given. Select Install to try again.")` → dismiss → `Ready`.
   - exit 127 (not authorized, no polkit agent) or pkexec can't start → `Manual{V, path}`:
     retrying can't help.
   - other → `Failed("apt-get could not install the update (exit code <n>).")` → `Ready`.
   A package is never `blocked`: every try is a click.

Never silent, never at start: installing needs the password.

### 9.4 macOS (`.app` in a writable `/Applications` or `~/Applications`)

`bundle` = `<parent>/<Name>.app` from `exe`; `work` = `<parent>/.Rustcord-update-V`;
`old` = `<parent>/.Rustcord-old-<running>`.

1. `attempted = V`.
2. `rm -rf work old; mkdir work`; `/usr/bin/tar -xzf <u>/Rustcord-V-macos-universal.app.tar.gz -C work`.
3. Check `work/Rustcord.app/Contents/MacOS/rustcord` is a file.
4. `rename(bundle, old)`; `rename(work/Rustcord.app, bundle)` (on error: rename `old` back,
   `Failed`).
5. `/usr/bin/open -n [-g] bundle --args --after-update [--minimized]` (`-g` when minimized: no
   activation). Non-zero or past the deadline → undo step 4 (new bundle back to `work`, `old`
   back to `bundle`), `Failed`.
6. `rmdir work`; `Restarting{V}`; `notify::request_quit()`.
7. The new version, started with `--after-update`, deletes `<parent>/.Rustcord-old-*` and
   `<parent>/.Rustcord-update-*` in its task (§8.2 cleanup).

`-n` is required: without it LaunchServices would just activate the still-running old process.
Files written by reqwest carry no quarantine attribute, so Gatekeeper does not block the new
bundle. The bundle is ad-hoc signed, so its designated requirement is the cdhash: after an
update macOS may ask once more for keychain access (the saved login) and the microphone.
That is why macOS never installs silently: a staged update waits for the next visible start
or a click on "Update ready", where the Restarting screen comes right before the prompt (a
password prompt from a hidden app would look like phishing).

### 9.5 Not writable / portable / dev

Nothing is downloaded. `Available{V}` with `kind = NoticeOnly`; the title-bar button opens
`https://github.com/r0sc/Rustcord/releases/tag/v<V>` (built from the verified version; never a
URL from the manifest).

---

## 10. Single-instance handoff (`--after-update`)

Problem: the new process starts while the old one still holds the instance lock (Windows named
mutex, Unix socket), so `claim_instance` fails and it would hand off to the exiting copy.

- Flag `--after-update`: `main.rs` calls `notify::claim_instance_within(30 s)` instead of
  `claim_instance()`: retry every 100 ms until it succeeds or 30 s passed; then, as before,
  `signal_running(false)` and exit.
  - Windows: a failed `CreateMutexW` closes its handle; the mutex disappears when the old
    process exits. The mutex handle is not inheritable, so the setup / new app never keep it
    alive. The old app starts the new one only after its event loop ended, so the mutex is
    gone a moment later, when the old process exits.
  - Unix: `connect` fails once the old listener is gone (or its `shutdown` removed the socket
    file); then `claim` binds a fresh socket. The listener fd is `CLOEXEC`, so an `exec`ed
    copy does not inherit it.
- `notify::request_quit()` = `push(TrayAction::Quit)`: the same path as the tray's Quit
  (`user_settings::app::logic` sets "quitting" and closes the viewport; `on_exit` saves the cache
  and removes the tray icon / socket). Works while the window is hidden.
- The new copy's task deletes the previous version's leftovers: `<dir>\rustcord.exe.old`
  (Windows; kept until then so a version that crashes at start can be rolled back by hand),
  `<parent>/.Rustcord-old-*` and `.Rustcord-update-*` (macOS).

`--after-update` is otherwise only a hint for cleanup; the loop guard relies on `state.toml`.

---

## 11. `rustcord-setup --update` (Windows)

`Rustcord-Setup-<v>.exe --update [--test-identity]`
(`RUSTCORD_SETUP_TEST=1` also selects the test identity, as for an install).

- Always silent (no window), handled before the `--silent` branch; does not use `ui.rs`.
- Folder: `InstallLocation` of the identity's Apps entry, required (`--dir` is refused with
  `--update`). Missing → "Rustcord is not installed.", exit 1.
- Never asks the running app to quit (it waits for this exit code), never starts the new app
  (the running one does, once it quit, never during a call: §9.1), never writes shortcuts (one
  the user deleted stays deleted) and never writes the Run value (autostart stays as the user
  left it).
- Steps: unpack the new app to `rustcord.exe.new` and flush it; rename `rustcord.exe` →
  `rustcord.exe.old` (works on the running exe); rename `.new` → `rustcord.exe`; write
  `uninstall.exe` (this stub) through a temporary file; rewrite every Apps-entry value
  (DisplayVersion, EstimatedSize, …); exit 0. The Apps entry is written only after the files
  are in place, and a failure there is logged to stderr, not rolled back (a stale
  DisplayVersion is cosmetic; the new files are what runs).
- Any failure before that: put the previous exe back, write one line to `setup-error.txt` next
  to the setup exe (and stderr), exit 1. A successful run deletes an old `setup-error.txt` and
  a stale `.old` from an earlier update.
- `rustcord.exe.old` stays for the new app to delete (§10).

A normal install uses the same unpack-then-swap steps and rolls back the same way. The old
setup never runs: the *new* version's setup does the update, so these flags are a frozen
interface (§2).

---

## 12. `install.sh --update` (Linux tarball)

`sh install.sh --update`: the install path with three differences:
- no `quit_running` (the app quits itself after the script returns);
- never writes the login entry (`~/.config/autostart/rustcord.desktop` stays as the app's
  switch left it, including absent);
- fails with exit 1 and a message on stderr when `~/.local/bin/rustcord` does not exist
  ("nothing to update").
Everything else as an install: binary copied to `rustcord.new` then `mv -f` (the running old
binary keeps its inode), desktop file with absolute `Exec`, icons, the copy of `install.sh` in
`~/.local/share/rustcord/`, icon cache refresh. `RUSTCORD_SETUP_TEST=1` +
`RUSTCORD_SETUP_PREFIX` work as today. The usage line and header comment list `--update`.

---

## 13. Release CI (`.github/workflows/release.yml`)

Build job (matrix, unchanged except):
- macOS artifact: `dist/*.dmg` **and** `dist/*.app.tar.gz` (build-macos.sh writes it, §2:
  `COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs -C "$staging" -czf
  "$dist/Rustcord-$version-macos-universal.app.tar.gz" Rustcord.app`, made after `codesign`
  and before the `Applications` link is added; then extracted into a temp folder and checked
  with `codesign --verify --strict`).
- "Check the Linux packages" also tests `install.sh --update`: after the test install, delete
  the test login entry, run the unpacked `install.sh --update`, assert the entry is still
  absent and the binary is there; and assert `--update` on an empty prefix exits non-zero.
- `check_only` keeps working: every new step is under `!inputs.check_only`.

Release job (tag `v*`, not `check_only`), in this order:
1. `actions/checkout` (needs `assets/update-signing.pub` and the script; before the download,
   since checkout cleans the workspace).
2. `actions/download-artifact` into `dist`.
3. `python3 scripts/make-update-manifest.py dist --require-all` → `dist/latest.json`
   (version from Cargo.toml, which the build job already matched against the tag).
4. Sign (secret only in this step's `env`):
   ```bash
   if [ -z "${SIGNING_KEY:-}" ]; then echo "::error::RUSTCORD_UPDATE_SIGNING_KEY is not set"; exit 1; fi
   umask 077
   trap 'rm -f "$RUNNER_TEMP/update-key.pem"' EXIT
   printf '%s\n' "$SIGNING_KEY" > "$RUNNER_TEMP/update-key.pem"
   openssl pkeyutl -sign -inkey "$RUNNER_TEMP/update-key.pem" -rawin -in dist/latest.json -out "$RUNNER_TEMP/sig.bin"
   rm -f "$RUNNER_TEMP/update-key.pem"
   base64 -w0 "$RUNNER_TEMP/sig.bin" > dist/latest.json.sig
   ```
5. Verify against the committed key (fails the job on mismatch):
   ```bash
   { printf '\x30\x2a\x30\x05\x06\x03\x2b\x65\x70\x03\x21\x00'; base64 -d assets/update-signing.pub; } > "$RUNNER_TEMP/pub.der"
   openssl pkey -pubin -inform DER -in "$RUNNER_TEMP/pub.der" -out "$RUNNER_TEMP/pub.pem"
   base64 -d dist/latest.json.sig > "$RUNNER_TEMP/sig.bin"
   openssl pkeyutl -verify -pubin -inkey "$RUNNER_TEMP/pub.pem" -rawin -in dist/latest.json -sigfile "$RUNNER_TEMP/sig.bin"
   ```
   (the 12 bytes are the DER SubjectPublicKeyInfo prefix of an Ed25519 key).
6. `SHA256SUMS` over everything in `dist` (exists; now includes `latest.json*`).
7. `gh release create … --draft …` (exists). The orchestrator publishes by hand;
   `releases/latest` only sees it then.

Never `set -x` in the signing step. The release job keeps `permissions: contents: write`.
Every action in release.yml is pinned to a full commit SHA with its tag as a comment (the
packages are installed automatically, so a hijacked action tag would reach every copy).

---

## 14. `scripts/make-update-manifest.py` (Python 3, stdlib only)

Python because it runs unchanged on the Ubuntu runner (`python3`), in WSL and on the Windows
dev box (`python`); `jq` is not installed on Windows.

```text
python3 scripts/make-update-manifest.py DIR [--version X.Y.Z] [--base-url URL] [--require-all] [--out FILE]
python3 scripts/make-update-manifest.py --self-test
```
- `DIR`: folder with the release files. Looks for exactly the four §2 file names for the
  version; includes each that exists (computes `sha256`, `size`); `--require-all` fails unless
  all four are there (CI); otherwise at least one is required.
- `--version`: default Cargo.toml's `version` (repo root = the script's parent's parent);
  must match `^\d+\.\d+\.\d+$`.
- `--base-url`: default `https://github.com/r0sc/Rustcord/releases/download/v<version>/`;
  must end with `/`; must be `https://`, or `http(s)://` on 127.0.0.1 / [::1] / localhost
  (local E2E).
- `--out`: default `DIR/latest.json`. Written as bytes:
  `(json.dumps(m, indent=2, sort_keys=True) + "\n").encode()` (binary mode: Windows text mode
  would write CRLF).
- Errors: one line on stderr, exit 1. Success prints nothing.
- `--self-test`: assert-based checks (temp dir with dummy files: hashes, sizes, URLs, key set,
  `--require-all`, bad version, bad base URL), exit 0.

Signing is not part of the script (CI uses openssl; local E2E too, §16.2).

---

## 15. UI

Binding look: docs/DESIGN.md (tokens via `Pal`, sentence case, §13 copy rules, purple only on
the logo and one primary button). English copy as below.

### 15.1 Update screen (`status.screen == true`)

Replaces the login / main UI under the normal 32 px title bar (title bar content: Rustcord's
`app_label`, like the login screen; window buttons and drag work). Flat `guild_bar` page, a
centred column, no card:

- logo: `assets/icons/rustcord-128.png` at 56 px (login's `app_icon`);
- 24 gap; title in the title role, `text`;
- 16 gap; progress bar 320 × 4, radius 2, track `edge` @ 50 %, fill `text` (the Spotify bar of
  DESIGN.md §11.2; not purple: it is the progress of a download, not "you");
- 8 gap; meta line, `text_muted`;
- 24 gap; buttons (40 tall like login; `messages::button` kinds); Enter = primary, Esc = the
  ghost / Continue action (`update::dismiss()`), where present.

| Phase (kind) | Title | Bar | Meta | Buttons |
|---|---|---|---|---|
| Downloading | `Downloading update 0.1.1` | done / total | `7.3 MB of 12.0 MB` (1 MB = 1 000 000 B, one decimal; total 0 → `7.3 MB`) | ghost `Not now` → dismiss |
| Installing | `Installing update` | full | `Rustcord 0.1.1` | – |
| Restarting | `Restarting` | full | `Rustcord 0.1.1 is starting.` | – |
| Failed | `Update failed` | – (instead: `Alert` 16 + the message in `danger`, ui role) | `Rustcord continues in a few seconds.` | primary `Continue` → dismiss (core also continues after 5 s) |
| Ready (Package) | `Update 0.1.1 is ready` | – | `Installing it asks for your password.` | primary `Install` → `update::install()`, ghost `Not now` → dismiss |
| Manual | `Update 0.1.1 is downloaded` | – | `Install it with your package manager:` then the path in a monospace `code_background` well, selectable | primary `Continue` → dismiss |
| Ready (Auto) / Available / Checking | `Installing update` / `Downloading update 0.1.1` (empty bar, `Not now`) / `Checking for updates` | full / empty / empty | – | – (not reached in the normal flow; drawn sensibly anyway) |

While `busy`, the Install button of Ready (Package) is disabled with the tooltip `Install it
after your call ends`.

Failure messages (one line, what happened + what to do; from core, shown verbatim):
- `Could not download the update. Check your connection; Rustcord tries again later.`
- `The update's signature is not valid, so it was not installed.` (only after "Check for updates")
- `The downloaded update was damaged, so it was not installed.`
- `Could not save the update: <io error>.` (disk full etc.)
- `Could not install the update: <reason>.` / `Could not start the update: <e>.`
- `Rustcord 0.1.1 did not install. Version 0.1.0 keeps running.` (loop guard, + `: <setup error>`)
- `The update was not installed: no password was given. Select Install to try again.`
- `apt-get could not install the update (exit code <n>).`
- `Could not check for updates. Check your connection.` (only after "Check for updates")

### 15.2 Title-bar button

Shown when `!screen` and phase is `Available`, `Ready` or `Manual`. Main screen: right-aligned
8 px left of the inbox button (when the inbox shows), else 8 px from the content rect's right
end; also on the login screen. A pill: height 24, padding 8, `Download` icon 16 in `positive`,
6 gap, label 13/600 in `positive_text`; hover `hover` fill, radius control, focus ring,
`WidgetInfo` button with the label. Click → `update::button_clicked()`.

| Phase (kind) | Label | Tooltip |
|---|---|---|
| Available (Auto / Package) | `Update available` | `Download and install Rustcord 0.1.1` |
| Available (NoticeOnly) | `Update available` | `Open the Rustcord 0.1.1 release page` |
| Ready (Auto) | `Update ready` | `Restart Rustcord to install 0.1.1` |
| Ready (Package) | `Update ready` | `Install Rustcord 0.1.1` |
| Manual | `Update available` | `Show where Rustcord 0.1.1 was saved` |
| Ready (Auto) while `busy` | `Update ready`, disabled (icon and label `text_muted`) | `Installs after your call ends` |
| Ready (Package) while `busy` | `Update ready`, disabled | `Install it after your call ends` |

Available stays clickable during a call: the download runs in the background and the install
follows when the call ends.

Never Discord's logo, wordmark, colours or font.

### 15.3 Settings: the Windows / Linux / macOS page (`Page::Windows`, `user_settings/app.rs`)

New section `Updates` after `Window`:
- Switch row `Install updates automatically` ↔ `Settings::auto_update` (save + `update::set_auto`).
  Meta by kind: Auto `Rustcord downloads new versions in the background and installs them
  when it starts or while it waits in the tray, never during a call.` (macOS, which never
  installs from the menu bar: without `or while it waits in the tray`); Package `Rustcord
  downloads new versions in the background. Installing asks for your password.`; NoticeOnly
  (disabled) `This copy of Rustcord can't update itself. New versions are on the releases
  page.`; `!enabled` (disabled) `Updates are off for development builds and test runs.`
- Row title `Version 0.1.0` (`CARGO_PKG_VERSION`), meta = the phase line below, control: a
  secondary button `Check for updates` → `update::check_now()`, disabled when `!enabled` or
  phase is Checking / Downloading / Installing / Restarting.

| Phase | Meta line |
|---|---|
| Idle | `Rustcord checks for updates every 6 hours.` |
| Checking | `Checking for updates` |
| UpToDate | `Rustcord is up to date.` |
| Available | `Version 0.1.1 is available.` (NoticeOnly: `Version 0.1.1 is on the releases page.`; after a background download error: `Version 0.1.1 is available. <note>`) |
| Downloading | `Downloading version 0.1.1 (61 %)` |
| Ready | `Version 0.1.1 is ready to install.` (not "installs when Rustcord restarts": with auto install off, or for a blocked version, it waits for a click) |
| Installing | `Installing version 0.1.1` |
| Restarting | `Restarting` |
| Manual | `Version 0.1.1 is downloaded to <path>.` |
| Failed | the message |

No meta line while the updater is off (`!enabled`): the switch's meta says why.

Search: the page's keywords gain `update updates version upgrade`; its local labels gain
`Install updates automatically` and `Check for updates`.

### 15.4 Screenshot hook `RUSTCORD_UPDATE_DEMO`

UI-only: `ui::update::status()` returns a fake `Status` instead of `crate::update::status()`;
nothing is downloaded or written (the mode is Off, so the pub fns are no-ops). Version `0.1.1`.

| Value | Status | What to screenshot |
|---|---|---|
| `checking` | Checking, Auto, no screen | with `RUSTCORD_OPEN=settings:windows`: the Updates rows |
| `uptodate` | UpToDate, Auto, no screen | same |
| `available` | Available, Auto, no screen | the title-bar button |
| `ready` | Ready, Auto, no screen | the title-bar button |
| `download` | Downloading 7 300 000 / 12 000 000, Auto, screen | the update screen |
| `install` | Installing, Auto, screen | |
| `restart` | Restarting, Auto, screen | |
| `error` | Failed(`Could not download the update. Check your connection; Rustcord tries again later.`), screen | |
| `deb` | Ready, Package, screen | the Install / Not now screen |
| `manual` | Manual(`/home/you/.cache/rustcord/update/rustcord_0.1.1_amd64.deb`), Package, screen | |

All `enabled: true`. Unknown values: no demo.

---

## 16. Tests

### 16.1 Unit tests (no network, no real install dirs, CI-safe)

- `update::feed`: the §3.2 fixture parses byte for byte; every rejection of §3.3; signature
  valid / flipped manifest byte / flipped signature byte / wrong key / bad base64 / wrong length
  / not UTF-8 (fixture signed with a throwaway openssl key; only its public half is in the
  test); the committed key is a valid point; version compare incl. prereleases; loopback URL
  rules (user info, `127.0.0.1.evil.com`, `[::ffff:127.0.0.1]`, query, fragment); the expected
  URL per key; the download sink (size cap, short or wrong bytes, rename, `.part` removed).
- `update::machine`: loop guard and staged package at start, every §8.3 / §8.4 transition,
  never covering a call, the 10-minute silent install, retry backoff, `note`, a late check
  result. `update::store`: `state.toml` round trip and validation, prune, `setup-error.txt`.
  `update::platform`: install kinds from paths (symlinked home on unix), macOS bundle and swap
  paths, pkexec exit codes, step messages; on unix also the runner (exit code, last stderr
  line, deadline kill) and the tarball apply against a fake package. `update`: the mode table
  of §5, `relaunch` only after the updater's own quit, a Prune done before the batch's next
  action. `notify`: `claim_instance_within` (on unix against a real socket). `config`:
  `auto_update` round trip.
- `ui::update`: every demo value, title-bar label / tooltip per phase, kind and busy, settings
  meta lines, the screen per §15.1 row, MB formatting, `busy`.
- Setup: `--update` argument handling and the step plan (no shortcut, Run value, quit or
  start for an update). `scripts/make-update-manifest.py --self-test`. CI's signing and verification
  steps, run locally with a throwaway key.

### 16.2 End-to-end (Windows, test identity only)

Never the real "Rustcord" identity, everything under a scratch folder, own processes only;
`RUSTCORD_OFFLINE=1` on every run so the app never reads the saved login or reaches Discord.
1. `scripts\build-installer.ps1` → `dist\Rustcord-Setup-0.1.0.exe` (the "old" version), and the
   same from a scratch copy of the tree whose `Cargo.toml` and `Cargo.lock` say `9.9.9` (the
   "new" one; `C:\rc-target\release` is rebuilt at 0.1.0 afterwards). Both builds share the
   target dir and an MSVC binary has no hash in its file name, so touch
   `src/bin/rustcord-setup/main.rs` before the second build: otherwise cargo keeps the other
   version's setup stub (its Apps entry then shows the wrong DisplayVersion).
2. Throwaway key: `openssl genpkey -algorithm ed25519 -out k.pem`;
   `openssl pkey -in k.pem -pubout -outform DER | tail -c 32 | base64 -w0` → `PUB`.
3. Feed folder with `Rustcord-Setup-9.9.9.exe`;
   `python scripts\make-update-manifest.py feed --version 9.9.9 --base-url http://127.0.0.1:8765/`;
   sign as §13 step 4 with `k.pem`; serve it on 127.0.0.1 only (a small `http.server`
   subclass that throttles to a few MB/s, so the download screen is visible, and logs each
   request's User-Agent and header names).
4. `Rustcord-Setup-0.1.0.exe --silent --test-identity --no-desktop --no-autostart`.
5. Start `%LOCALAPPDATA%\Programs\RustcordSetupTest\rustcord.exe` with
   `RUSTCORD_HOME=<scratch>\home RUSTCORD_OFFLINE=1 RUSTCORD_UPDATE_FEED=http://127.0.0.1:8765/latest.json
   RUSTCORD_UPDATE_PUBKEY=<PUB>` → the update screen within 3 s, download progress, "Installing
   update", "Restarting"; the old process exits, a new one runs `--after-update`; the exe hash,
   the Apps entry's DisplayVersion (9.9.9) and the shortcuts / Run value are as expected;
   `state.toml` has no `attempted`, `rustcord.exe.old` is gone, the next check says up to date.
6. Negative runs (`RUSTCORD_UPDATE_FROM=0.0.9` where the installed copy is already 9.9.9):
   corrupt `.sig`, another `RUSTCORD_UPDATE_PUBKEY`, a re-signed manifest with a wrong sha256,
   an older version, a URL on another host; a dead and a hanging feed port (first frame timed
   with `RUSTCORD_TRACE_STARTUP`); a staged package applied at the next start; a `--minimized`
   start; `auto_update = false`; a setup that fails; the loop guard (the 0.1.0 setup served as
   `Rustcord-Setup-9.9.9.exe`).
7. Quit (`rustcord.exe --quit` with the same `RUSTCORD_HOME`), uninstall
   (`uninstall.exe --uninstall --silent --test-identity --purge`), delete the scratch folder.

Linux (WSL, `CARGO_TARGET_DIR=~/rc-target-linux`): the same with the tarball,
`RUSTCORD_SETUP_TEST=1 RUSTCORD_SETUP_PREFIX=<scratch>/prefix`. macOS: CI build only (no Mac).

---

## 17. Module map and public API

| Where | What |
|---|---|
| `src/update/mod.rs` | mode (§5), `start`, the pub fns, the task (§8.5), `relaunch` |
| `src/update/state.rs` | `Status`, `Phase`, `Kind`, `status()`: the UI contract (change only additively) |
| `src/update/machine.rs` | every decision of §8, pure (events in, actions out) |
| `src/update/feed.rs` | key, signature, manifest, URL rules, HTTP client, download sink (§3) |
| `src/update/store.rs` | `state.toml`, prune, `setup-error.txt` (§7) |
| `src/update/platform.rs` | install kind (§6), apply per platform (§9), leftovers (§10) |
| `src/ui/update.rs` | update screen, title-bar button, settings copy, `RUSTCORD_UPDATE_DEMO` (§15) |
| `src/ui/user_settings/app.rs` | the Updates rows (§15.3) |
| `src/main.rs`, `src/notify.rs`, `src/autostart.rs` | `--after-update`, `claim_instance_within`, `request_quit`, `in_install_dir` / `unix_installed` |
| `src/bin/rustcord-setup/main.rs`, `scripts/linux/install.sh` | `--update` (§11, §12) |
| `scripts/make-update-manifest.py`, `scripts/build-macos.sh`, `.github/workflows/release.yml` | the feed and the assets (§2, §13, §14) |

Public API (the UI calls only these):
- `crate::update::{Status, Phase, Kind, status}` and `start(rt, ctx, auto)` (main.rs, in the
  eframe creator before `RustcordApp::new`), `sync_ui(hidden, busy, interacted)` (every
  `logic` pass), `button_clicked`, `install`, `dismiss`, `check_now`, `set_auto(on)`,
  `relaunch()` (main.rs, after the event loop).
- `crate::config::Settings::auto_update: bool` (default true).
- `crate::notify::{request_quit, claim_instance_within}`.
- `crate::ui::update::status()` (demo-aware) for every UI read.

---

## 18. Known limits

- macOS after an update may ask again for keychain and microphone access (ad-hoc signature);
  hence no silent installs on macOS. The macOS apply (bundle swap, `open -n`, bsdtar flags of
  the archive) has never run on a real Mac: it is unit-tested for its paths only and first
  runs with the first macOS release.
- macOS: if the new version crashes right after start, the old bundle stays at
  `<parent>/.Rustcord-old-<v>` (rename it back by hand); nothing deletes it until a later
  update starts successfully. Windows keeps `rustcord.exe.old` the same way until the new
  version's start.
- macOS: a call joined in the second or two while an install runs (only after a click, then
  hiding the window) does not stop the restart: `open` already started the new copy, which
  waits 30 s for this one's lock. Windows and Linux start the new version only after the quit,
  which waits for the call to end.
- Windows: the old app starts the new exe as it exits; if that start fails (an antivirus
  quarantined the new exe), nothing restarts and the next start fails too; the previous
  version is still `rustcord.exe.old` (rename it back by hand, or run the setup again).
- The systemd behaviour that §9.2's `exec` works around (Ubuntu 22.04, systemd 249) is taken from
  systemd's documentation, not observed.
- `busy` counts calls, ringing calls and drafts while hidden, not uploads in flight or
  attachments being prepared.
- A download error in the background shows only in the settings' version row (`note`); the
  title-bar button stays "Update available".
- An incoming call that rings while the update screen is up is not shown (its popup belongs to
  the main window); it counts as busy, so nothing installs meanwhile.
- The title-bar button can overlap a very long location name in a narrow window (never the
  window buttons). Gradient themes: the update screen, like the login screen, does not paint
  the window gradient under the title bar.
- The 6 h timer uses a monotonic clock; on Linux a suspended laptop delays the next check by
  the suspended time.
- `.deb` installs need a polkit agent; without `pkexec` / `apt-get` the package is only saved.
  A 10-minute pkexec timeout shows the "no password was given" message.
- A Windows or Linux copy on another architecture (ARM) only gets the notice.
- The check itself can't be switched off (only installing); a copy started with
  `RUSTCORD_HOME` only gets the notice.
