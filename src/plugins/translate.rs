//! Translate: a message's text in your language, in the chat. Only the click on "Translate" in
//! the message menu starts it, one request per click; nothing is ever translated on its own.
//!
//! What is sent: only that message's text (mentions, channels and custom emoji as the names
//! shown) and the target language code. No author, channel, server, id or token. Where: Google
//! Translate's keyless web endpoint by default (unofficial, it can stop working), or DeepL when
//! the user saved their own DeepL key (kept in the OS credential store, never in a file or log).
//! No Discord request is involved. Results stay in memory, bounded, and are cleared when the
//! plugin is turned off. Under test and in offline mode nothing leaves the machine and the
//! credential store is never touched.

use super::{Plugin, opt_choice};
use crate::auth::Secret;
use crate::model::Id;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

/// "Translate to" choices; index 0 follows the app language, the others map to `CODES[i - 1]`.
pub const LANGUAGES: &[&str] = &[
    "App language", "Arabic", "Bulgarian", "Chinese", "Czech", "Danish", "Dutch", "English", "Estonian", "Finnish", "French", "German", "Greek", "Hungarian", "Indonesian",
    "Italian", "Japanese", "Korean", "Latvian", "Lithuanian", "Norwegian", "Polish", "Portuguese", "Romanian", "Russian", "Slovak", "Slovenian", "Spanish", "Swedish", "Turkish",
    "Ukrainian",
];

/// (Google code, DeepL code) of `LANGUAGES[1..]`.
const CODES: &[(&str, &str)] = &[
    ("ar", "AR"), ("bg", "BG"), ("zh-CN", "ZH-HANS"), ("cs", "CS"), ("da", "DA"), ("nl", "NL"), ("en", "EN-US"), ("et", "ET"), ("fi", "FI"), ("fr", "FR"), ("de", "DE"),
    ("el", "EL"), ("hu", "HU"), ("id", "ID"), ("it", "IT"), ("ja", "JA"), ("ko", "KO"), ("lv", "LV"), ("lt", "LT"), ("no", "NB"), ("pl", "PL"), ("pt", "PT-BR"), ("ro", "RO"),
    ("ru", "RU"), ("sk", "SK"), ("sl", "SL"), ("es", "ES"), ("sv", "SV"), ("tr", "TR"), ("uk", "UK"),
];

/// Cap of the answer read from a service.
const MAX_BODY: usize = 256 << 10;
/// Kept translations; past this the finished ones are dropped.
const MAX_KEPT: usize = 200;

const OFFLINE: &str = "Rustcord is offline. Try again when it's connected.";

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// (Google, DeepL) code for the "Translate to" choice, or for the app language: the account's
/// Discord language, else the OS locale, else English.
fn target(choice: usize, app: Option<&str>, os: &str) -> (String, String) {
    if let Some((g, d)) = choice.checked_sub(1).and_then(|i| CODES.get(i)) {
        return ((*g).to_owned(), (*d).to_owned());
    }
    let tag = app.map(str::trim).filter(|s| !s.is_empty()).map_or_else(|| os_lang(os), str::to_owned);
    let lower = tag.to_lowercase();
    let mut parts = lower.split(['-', '_']);
    let (lang, region) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let pair = |g: &str, d: &str| (g.to_owned(), d.to_owned());
    match (lang, region) {
        ("", _) => pair("en", "EN-US"),
        ("en", "gb") => pair("en", "EN-GB"),
        ("zh", "tw" | "hk" | "hant") => pair("zh-TW", "ZH-HANT"),
        ("pt", "pt") => pair("pt", "PT-PT"),
        ("no" | "nb" | "nn", _) => pair("no", "NB"),
        _ => CODES.iter().find(|(g, _)| g.split('-').next() == Some(lang)).map_or_else(|| (lang.to_owned(), lang.to_uppercase()), |(g, d)| pair(g, d)),
    }
}

/// The language of an OS locale tag, with the region kept where it picks a variant:
/// "de_DE.UTF-8" is "de", "en_GB.UTF-8" is "en-GB". "C" and "POSIX" are no language.
fn os_lang(tag: &str) -> String {
    let base = tag.split(['.', '@']).next().unwrap_or("").replace('_', "-");
    let mut parts = base.split('-');
    let lang = parts.next().unwrap_or("").to_lowercase();
    let region = parts.next().unwrap_or("").to_uppercase();
    if matches!(lang.as_str(), "c" | "posix") {
        return String::new();
    }
    if matches!((lang.as_str(), region.as_str()), ("en", "GB") | ("zh", "TW" | "HK") | ("pt", "PT")) { format!("{lang}-{region}") } else { lang }
}

/// The English name of a language code from either service ("zh-CN", "DE"), else the code in capitals.
pub fn language_name(code: &str) -> String {
    let lang = code.to_lowercase();
    let lang = lang.split(['-', '_']).next().unwrap_or("");
    let lang = if matches!(lang, "nb" | "nn") { "no" } else { lang };
    CODES.iter().position(|(g, _)| g.split('-').next() == Some(lang)).map_or_else(|| code.to_uppercase(), |i| LANGUAGES[i + 1].to_owned())
}

fn encode(s: &str) -> String {
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

/// Google's keyless endpoint: the URL holds only the language, the text goes in the form body.
fn google_request(text: &str, tl: &str) -> (String, String) {
    (format!("https://translate.googleapis.com/translate_a/single?client=gtx&sl=auto&tl={}&dt=t", encode(tl)), format!("q={}", encode(text)))
}

/// (translated text, source language) of Google's answer: `[[[part, original, ..], ..], null, "en"]`.
fn parse_google(body: &[u8]) -> Result<(String, String), &'static str> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|_| "unreadable")?;
    let parts = v.get(0).and_then(|p| p.as_array()).ok_or("unreadable")?;
    let text: String = parts.iter().filter_map(|p| p.get(0).and_then(|s| s.as_str())).collect();
    if text.is_empty() {
        return Err("empty");
    }
    Ok((text, v.get(2).and_then(|s| s.as_str()).unwrap_or("").to_owned()))
}

/// (translated text, detected source language) of DeepL's answer.
fn parse_deepl(body: &[u8]) -> Result<(String, String), &'static str> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|_| "unreadable")?;
    let t = v.get("translations").and_then(|t| t.get(0)).ok_or("unreadable")?;
    let text = t.get("text").and_then(|s| s.as_str()).ok_or("unreadable")?;
    Ok((text.to_owned(), t.get("detected_source_language").and_then(|s| s.as_str()).unwrap_or("").to_owned()))
}

/// Free keys end in ":fx" and use the free host.
fn deepl_url(key: &str) -> &'static str {
    if key.ends_with(":fx") { "https://api-free.deepl.com/v2/translate" } else { "https://api.deepl.com/v2/translate" }
}

/// What a failed answer says (the status), in the copy the chat shows.
fn status_error(deepl: bool, status: u16) -> &'static str {
    match (deepl, status) {
        (true, 403) => "DeepL didn't accept your key. Check it in Settings > Plugins > Translate.",
        (true, 456) => "Your DeepL quota for this month is used up.",
        (true, 400) => "DeepL can't translate into this language. Pick another one in Settings > Plugins > Translate.",
        (true, 429) => "DeepL is busy. Try again in a minute.",
        (false, 429) => "Google Translate is busy. Try again in a minute.",
        (true, _) => "DeepL didn't answer. Try again later.",
        (false, _) => "Google Translate didn't answer. Try again later.",
    }
}

/// A key as pasted from DeepL: 1 to 128 printable ASCII characters, no spaces.
fn valid_key(key: &str) -> bool {
    (1..=128).contains(&key.len()) && key.chars().all(|c| c.is_ascii_graphic())
}

#[cfg(windows)]
fn os_locale() -> String {
    // LOCALE_NAME_MAX_LENGTH
    let mut buf = [0u16; 85];
    // SAFETY: the buffer is valid for `buf.len()` UTF-16 units, which is what is passed.
    let n = unsafe { windows_sys::Win32::Globalization::GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
    if n > 1 { String::from_utf16_lossy(&buf[..n as usize - 1]) } else { String::new() }
}

#[cfg(not(windows))]
fn os_locale() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"].into_iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty())).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Kept translations
// ---------------------------------------------------------------------------

enum State {
    Pending,
    Done { text: String, from: String },
    Failed(String),
}

struct Entry {
    /// Hash of the message text this answers (an edit drops it) and the language it was asked for.
    src: u64,
    target: String,
    state: State,
    shown: bool,
}

/// What the chat draws for a message.
pub enum View {
    Pending,
    Shown { text: String, from: String },
    Failed(String),
}

static STORE: Mutex<Option<HashMap<u64, Entry>>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn store() -> MutexGuard<'static, Option<HashMap<u64, Entry>>> {
    STORE.lock().unwrap_or_else(|e| e.into_inner())
}

fn bump() {
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// Grows on every change, so message rows measure again (`messages::layout_key`).
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

fn hash(content: &str) -> u64 {
    let mut h = DefaultHasher::new();
    content.hash(&mut h);
    h.finish()
}

/// The translation of message `id` to draw, if the user asked for it and its text is unchanged.
pub fn view(id: Id, content: &str) -> Option<View> {
    let guard = store();
    let e = guard.as_ref()?.get(&id.0).filter(|e| e.shown && e.src == hash(content))?;
    Some(match &e.state {
        State::Pending => View::Pending,
        State::Done { text, from } => View::Shown { text: text.clone(), from: from.clone() },
        State::Failed(r) => View::Failed(r.clone()),
    })
}

/// "Show original" and "Dismiss": the message draws its own text again. The answer is kept.
pub fn dismiss(id: Id) {
    if let Some(e) = store().as_mut().and_then(|m| m.get_mut(&id.0)) {
        e.shown = false;
        bump();
    }
}

/// Forgets every translation (the plugin was turned off).
pub fn clear() {
    *store() = None;
    bump();
}

/// The Translate click: shows a kept translation, else asks the service once (a second click
/// while it is pending does nothing). `content` is the message as stored, `text` what is sent.
pub fn start(ctx: &egui::Context, id: Id, content: &str, text: String, app_locale: Option<&str>) {
    let (tl, dl) = target(opt_choice(Plugin::Translate, "to"), app_locale, &os_locale());
    let src = hash(content);
    let key = (!text.trim().is_empty()).then(current_key).flatten();
    let mut guard = store();
    let map = guard.get_or_insert_with(HashMap::new);
    if let Some(e) = map.get_mut(&id.0).filter(|e| e.src == src && e.target == tl && !matches!(e.state, State::Failed(_))) {
        e.shown = true;
        bump();
        return;
    }
    if text.trim().is_empty() {
        return;
    }
    // shortcut: past the cap every finished entry goes (not just the oldest); fine for a handful of clicks.
    if map.len() >= MAX_KEPT {
        map.retain(|_, e| matches!(e.state, State::Pending));
    }
    let offline = cfg!(test) || crate::config::offline_mode();
    let state = if offline { State::Failed(OFFLINE.into()) } else { State::Pending };
    let pending = matches!(state, State::Pending);
    map.insert(id.0, Entry { src, target: tl.clone(), state, shown: true });
    bump();
    drop(guard);
    if !pending {
        return;
    }
    let ctx = ctx.clone();
    let spawned = std::thread::Builder::new().name("rustcord-translate".into()).spawn(move || {
        let result = request(&text, &tl, &dl, key.as_ref());
        if let Some(e) = store().as_mut().and_then(|m| m.get_mut(&id.0)).filter(|e| e.src == src && matches!(e.state, State::Pending)) {
            e.state = match result {
                Ok((text, from)) => State::Done { text, from },
                Err(reason) => State::Failed(reason),
            };
            bump();
        }
        ctx.request_repaint();
    });
    if spawned.is_err()
        && let Some(e) = store().as_mut().and_then(|m| m.get_mut(&id.0))
    {
        e.state = State::Failed("Couldn't start the translation. Try again.".into());
        bump();
    }
}

/// One request to the chosen service. Errors are the reason the chat shows.
fn request(text: &str, tl: &str, dl: &str, key: Option<&Secret>) -> Result<(String, String), String> {
    let service = if key.is_some() { "DeepL" } else { "Google Translate" };
    let unreachable = format!("Couldn't reach {service}. Check your connection, then select Try again.");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|_| unreachable.clone())?;
    rt.block_on(async {
        let client = reqwest::Client::builder()
            .user_agent(concat!("Rustcord/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| unreachable.clone())?;
        let sent = match key {
            Some(k) => {
                let mut auth = reqwest::header::HeaderValue::from_str(&format!("DeepL-Auth-Key {}", k.expose())).map_err(|_| status_error(true, 403).to_owned())?;
                auth.set_sensitive(true);
                client.post(deepl_url(k.expose())).header(reqwest::header::AUTHORIZATION, auth).json(&serde_json::json!({ "text": [text], "target_lang": dl })).send().await
            }
            None => {
                let (url, body) = google_request(text, tl);
                client.post(url).header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded;charset=utf-8").body(body).send().await
            }
        };
        let mut resp = sent.map_err(|_| unreachable.clone())?;
        let status = resp.status();
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|_| unreachable.clone())? {
            if body.len() + chunk.len() > MAX_BODY {
                break;
            }
            body.extend_from_slice(&chunk);
        }
        let deepl = key.is_some();
        if !status.is_success() {
            return Err(status_error(deepl, status.as_u16()).to_owned());
        }
        let parsed = if deepl { parse_deepl(&body) } else { parse_google(&body) };
        parsed.map_err(|_| status_error(deepl, 0).to_owned())
    })
}

// ---------------------------------------------------------------------------
// The DeepL key
// ---------------------------------------------------------------------------

const SERVICE: &str = "Rustcord";
const ACCOUNT: &str = "deepl-api-key";

/// Read from the credential store once, then kept here (redacted, wiped on drop). `None` = not read yet.
static KEY: Mutex<Option<Option<Secret>>> = Mutex::new(None);

fn memory_only() -> bool {
    cfg!(test) || crate::config::offline_mode()
}

fn entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, ACCOUNT).map_err(|e| format!("credential store unavailable: {e}"))
}

fn current_key() -> Option<Secret> {
    let mut k = KEY.lock().unwrap_or_else(|e| e.into_inner());
    k.get_or_insert_with(|| if memory_only() { None } else { entry().ok().and_then(|e| e.get_password().ok()).filter(|s| valid_key(s)).map(Secret::new) }).clone()
}

pub fn has_key() -> bool {
    current_key().is_some()
}

/// Saves a DeepL key (trimmed) in the credential store. Err is the copy for the settings page.
pub fn set_key(key: &str) -> Result<(), String> {
    let key = key.trim();
    if !valid_key(key) {
        return Err("That doesn't look like a DeepL key.".into());
    }
    if !memory_only() {
        entry()?.set_password(key).map_err(|e| e.to_string())?;
    }
    *KEY.lock().unwrap_or_else(|e| e.into_inner()) = Some(Some(Secret::new(key.to_owned())));
    Ok(())
}

pub fn clear_key() -> Result<(), String> {
    if !memory_only() {
        match entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    *KEY.lock().unwrap_or_else(|e| e.into_inner()) = Some(None);
    Ok(())
}

// ---------------------------------------------------------------------------
// Test hook
// ---------------------------------------------------------------------------

/// `RUSTCORD_MSG_DEMO=translate|translate-wait|translate-error`: a state for the message,
/// as if Translate had been picked, without any request.
pub fn demo(id: Id, content: &str, which: &str) {
    let state = match which {
        "translate-wait" => State::Pending,
        "translate-error" => State::Failed("Couldn't reach Google Translate. Check your connection, then select Try again.".into()),
        _ => State::Done { text: "__Unterstrichen__, ***fett kursiv*** und ein Zeitstempel <t:1793000000:R>".into(), from: "en".into() },
    };
    store().get_or_insert_with(HashMap::new).insert(id.0, Entry { src: hash(content), target: String::new(), state, shown: true });
    bump();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_google_and_deepl_answers() {
        let g = br#"[[["Hallo ","Hello ",null,null,1],["Welt","world",null,null,1]],null,"en"]"#;
        assert_eq!(parse_google(g), Ok(("Hallo Welt".into(), "en".into())));
        assert!(parse_google(b"<html>").is_err() && parse_google(b"[]").is_err());
        let d = br#"{"translations":[{"detected_source_language":"EN","text":"Hallo"}]}"#;
        assert_eq!(parse_deepl(d), Ok(("Hallo".into(), "EN".into())));
        assert!(parse_deepl(b"{}").is_err());
    }

    #[test]
    fn target_follows_choice_app_language_os_then_english() {
        let t = |c, app, os| target(c, app, os);
        assert_eq!(t(0, Some("de"), ""), ("de".into(), "DE".into()));
        assert_eq!(t(0, Some("en-GB"), ""), ("en".into(), "EN-GB".into()));
        assert_eq!(t(0, Some("en-US"), ""), ("en".into(), "EN-US".into()));
        assert_eq!(t(0, Some("zh-TW"), ""), ("zh-TW".into(), "ZH-HANT".into()));
        assert_eq!(t(0, Some("zh-CN"), ""), ("zh-CN".into(), "ZH-HANS".into()));
        assert_eq!(t(0, Some("no"), ""), ("no".into(), "NB".into()));
        assert_eq!(t(0, Some("pt-BR"), ""), ("pt".into(), "PT-BR".into()));
        assert_eq!(t(0, None, "fr_FR.UTF-8"), ("fr".into(), "FR".into()));
        assert_eq!(t(0, None, "en_GB.UTF-8"), ("en".into(), "EN-GB".into()));
        assert_eq!(t(0, None, ""), ("en".into(), "EN-US".into()));
        assert_eq!(t(0, Some(""), ""), ("en".into(), "EN-US".into()));
        assert_eq!(t(0, None, "C"), ("en".into(), "EN-US".into()));
        assert_eq!(t(0, Some("hr"), ""), ("hr".into(), "HR".into()), "outside the list: used as is");
        let german = LANGUAGES.iter().position(|l| *l == "German").unwrap();
        assert_eq!(t(german, Some("fr"), "fr_FR"), ("de".into(), "DE".into()), "a choice beats the app language");
        assert_eq!(os_lang("de_DE.UTF-8"), "de");
    }

    #[test]
    fn language_names() {
        assert_eq!(language_name("zh-CN"), "Chinese");
        assert_eq!(language_name("DE"), "German");
        assert_eq!(language_name("NB"), "Norwegian");
        assert_eq!(language_name("xx"), "XX");
        assert_eq!(LANGUAGES.len(), CODES.len() + 1);
    }

    #[test]
    fn requests_keep_the_text_out_of_the_url() {
        let (url, body) = google_request("a&b ü", "de");
        assert_eq!(body, "q=a%26b%20%C3%BC");
        assert!(url.contains("tl=de") && !url.contains("a%26b"), "{url}");
        assert_eq!(deepl_url("k:fx"), "https://api-free.deepl.com/v2/translate");
        assert_eq!(deepl_url("k"), "https://api.deepl.com/v2/translate");
    }

    #[test]
    fn status_errors_and_keys() {
        assert!(status_error(true, 403).contains("key") && status_error(true, 456).contains("quota") && status_error(true, 400).contains("can't translate"));
        assert!(status_error(true, 429).starts_with("DeepL is busy") && status_error(false, 429).starts_with("Google Translate is busy"));
        assert!(status_error(false, 500).starts_with("Google Translate didn't answer"));
        assert!(!valid_key("") && !valid_key("a b") && !valid_key(&"k".repeat(200)));
        assert!(valid_key("0123abcd-0123-abcd-0123-0123456789ab:fx"));
        assert!(set_key("two words").is_err());
        assert!(set_key(" 0123abcd:fx ").is_ok() && has_key());
        assert!(clear_key().is_ok() && !has_key());
    }

    #[test]
    fn clicks_are_kept_deduped_and_never_leave_the_machine_in_tests() {
        let ctx = egui::Context::default();
        let id = Id(7);
        assert!(view(id, "hello").is_none());
        start(&ctx, id, "hello", "hello".into(), Some("de"));
        start(&ctx, id, "hello", "hello".into(), Some("de"));
        // Tests record the offline reason instead of asking a service.
        assert!(matches!(view(id, "hello"), Some(View::Failed(r)) if r == OFFLINE));
        assert_eq!(store().as_ref().unwrap().len(), 1);
        assert!(view(id, "changed").is_none(), "an edit drops it");
        // A kept answer shows again without a request.
        store().as_mut().unwrap().get_mut(&id.0).unwrap().state = State::Done { text: "hallo".into(), from: "en".into() };
        dismiss(id);
        assert!(view(id, "hello").is_none());
        let before = generation();
        start(&ctx, id, "hello", "hello".into(), Some("de"));
        assert!(matches!(view(id, "hello"), Some(View::Shown { text, .. }) if text == "hallo") && generation() > before);
        clear();
        assert!(view(id, "hello").is_none());
    }
}
