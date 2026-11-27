//! Linux: MPRIS players on the session bus. Two listener threads turn `PropertiesChanged` of any
//! player and players coming or going into `Msg::Changed`; the worker below blocks on the channel,
//! picks the player (`pick`) and reads its track. Picture only from a `file://` URL (read from
//! disk): no network request is ever made for it.

use super::{Msg, Track, art, pick, publish};
use std::collections::HashMap;
use std::sync::{Arc, mpsc};
use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::zvariant::OwnedValue;

const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";

pub(super) fn run(ctx: &egui::Context, tx: &mpsc::Sender<Msg>, rx: &mpsc::Receiver<Msg>) {
    let Ok(conn) = Connection::session() else { return };
    let signal = || zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal);
    listen(&conn, tx, signal().interface("org.freedesktop.DBus.Properties").and_then(|b| b.path(PATH)));
    listen(&conn, tx, signal().sender("org.freedesktop.DBus").and_then(|b| b.member("NameOwnerChanged")).and_then(|b| b.arg0ns("org.mpris.MediaPlayer2")));
    let _ = tx.send(Msg::Changed);
    let mut shown: Option<String> = None;
    let mut cache: (String, Option<Arc<egui::ColorImage>>) = Default::default();
    while let Ok(m) = rx.recv() {
        match m {
            Msg::Changed => {
                let (name, track) = read(&conn, shown.as_deref(), &mut cache).unzip();
                shown = name;
                publish(ctx, track.flatten());
            }
            Msg::Toggle => command(&conn, shown.as_deref(), "PlayPause"),
            Msg::Next => command(&conn, shown.as_deref(), "Next"),
        }
    }
}

/// One thread per match rule: every message wakes the worker.
fn listen(conn: &Connection, tx: &mpsc::Sender<Msg>, rule: zbus::Result<zbus::match_rule::Builder<'static>>) {
    let Ok(rule) = rule.map(|b| b.build()) else { return };
    let (conn, tx) = (conn.clone(), tx.clone());
    let _ = std::thread::Builder::new().name("rustcord-now-playing-bus".into()).spawn(move || {
        let Ok(messages) = MessageIterator::for_match_rule(rule, &conn, Some(16)) else { return };
        for _ in messages {
            if tx.send(Msg::Changed).is_err() {
                return;
            }
        }
    });
}

fn command(conn: &Connection, player: Option<&str>, method: &str) {
    if let Some(p) = player {
        let _ = conn.call_method(Some(p), PATH, Some(PLAYER), method, &());
    }
}

/// The player to show and its track (None inside: it has no title).
fn read(conn: &Connection, last: Option<&str>, cache: &mut (String, Option<Arc<egui::ColorImage>>)) -> Option<(String, Option<Track>)> {
    let names = zbus::blocking::fdo::DBusProxy::new(conn).ok()?.list_names().ok()?;
    let players: Vec<(String, bool)> = names
        .iter()
        .map(|n| n.as_str())
        .filter(|n| n.starts_with("org.mpris.MediaPlayer2."))
        .map(|n| (n.to_owned(), proxy(conn, n).and_then(|p| p.get_property::<String>("PlaybackStatus").ok()).as_deref() == Some("Playing")))
        .collect();
    let name = pick(&players, last)?.to_owned();
    let p = proxy(conn, &name)?;
    let playing = players.iter().any(|(n, on)| *n == name && *on);
    let meta: HashMap<String, OwnedValue> = p.get_property("Metadata").ok()?;
    let can_next = p.get_property::<bool>("CanGoNext").unwrap_or(false);
    let track = metadata(&meta).map(|(title, artist, art_url)| {
        if cache.0 != art_url {
            cache.1 = file_art(&art_url).map(Arc::new);
            cache.0 = art_url;
        }
        Track { title, artist, art: cache.1.clone(), can_next, playing }
    });
    Some((name, track))
}

fn proxy(conn: &Connection, name: &str) -> Option<Proxy<'static>> {
    Proxy::new(conn, name.to_owned(), PATH, PLAYER).ok()
}

/// Title, artists joined with ", " and the art URL (empty when there is none) of an MPRIS
/// `Metadata` map; None without a title.
fn metadata(m: &HashMap<String, OwnedValue>) -> Option<(String, String, String)> {
    let text = |k: &str| m.get(k).and_then(|v| v.try_clone().ok()).and_then(|v| String::try_from(v).ok());
    let title = text("xesam:title").filter(|t| !t.is_empty())?;
    let artist = m.get("xesam:artist").and_then(|v| v.try_clone().ok()).and_then(|v| Vec::<String>::try_from(v).ok()).map(|a| a.join(", ")).unwrap_or_default();
    Some((title, artist, text("mpris:artUrl").unwrap_or_default()))
}

/// The picture of a `file://` URL, at most 2 MB; http(s) pictures are skipped.
fn file_art(url: &str) -> Option<egui::ColorImage> {
    let path = percent_encoding::percent_decode_str(url.strip_prefix("file://")?).decode_utf8().ok()?;
    let path = std::path::Path::new(path.as_ref());
    if std::fs::metadata(path).ok()?.len() > 2 * 1024 * 1024 {
        return None;
    }
    art(&std::fs::read(path).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    #[test]
    fn mpris_metadata_reads_title_artist_and_local_art() {
        let owned = |v: Value<'_>| OwnedValue::try_from(v).unwrap();
        let mut m = HashMap::new();
        m.insert("xesam:title".to_owned(), owned(Value::from("Rewriting it in Rust")));
        m.insert("xesam:artist".to_owned(), owned(Value::from(vec!["Ferris", "The Crabs"])));
        m.insert("mpris:artUrl".to_owned(), owned(Value::from("file:///tmp/cover%20one.png")));
        assert_eq!(metadata(&m), Some(("Rewriting it in Rust".into(), "Ferris, The Crabs".into(), "file:///tmp/cover%20one.png".into())));
        assert!(file_art("https://example.com/a.png").is_none(), "no network art");
        m.remove("xesam:title");
        assert_eq!(metadata(&m), None);
    }
}
