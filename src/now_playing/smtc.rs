//! Windows: the system media session (GlobalSystemMediaTransportControls). The manager and the
//! current session raise events; each one only wakes the loop below, which blocks on a channel
//! (no polling). Any error reads as "no session": the row hides, nothing is shown to the user.

use super::{Msg, Track, art, publish};
use std::sync::{Arc, mpsc};
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager, GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status};
use windows::Storage::Streams::DataReader;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

/// The session whose events are wired: its app id and the two event tokens.
struct Wired {
    session: Session,
    app: String,
    tokens: (i64, i64),
}

/// The last picture read, kept while title and artist stay the same.
#[derive(Default)]
struct ArtCache {
    key: (String, String),
    art: Option<Arc<egui::ColorImage>>,
}

pub(super) fn run(ctx: &egui::Context, tx: &mpsc::Sender<Msg>, rx: &mpsc::Receiver<Msg>) {
    // SAFETY: plain COM start-up on this thread, once.
    if unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_err() {
        return;
    }
    let Some(mgr) = Manager::RequestAsync().and_then(|op| op.join()).ok() else { return };
    let wake = |tx: mpsc::Sender<Msg>| move || drop(tx.send(Msg::Changed));
    let on_session = wake(tx.clone());
    let _ = mgr.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
        on_session();
        Ok(())
    }));
    let (mut wired, mut cache): (Option<Wired>, ArtCache) = (None, ArtCache::default());
    let _ = tx.send(Msg::Changed);
    while let Ok(m) = rx.recv() {
        match m {
            Msg::Changed => {
                wire(&mgr, tx, &mut wired);
                publish(ctx, read(&mgr, &mut cache));
            }
            Msg::Toggle => drop(mgr.GetCurrentSession().and_then(|s| s.TryTogglePlayPauseAsync()).and_then(|op| op.join())),
            Msg::Next => drop(mgr.GetCurrentSession().and_then(|s| s.TrySkipNextAsync()).and_then(|op| op.join())),
        }
    }
}

/// Moves the property and playback event handlers to the current session.
fn wire(mgr: &Manager, tx: &mpsc::Sender<Msg>, wired: &mut Option<Wired>) {
    let session = mgr.GetCurrentSession().ok();
    let app = session.as_ref().and_then(|s| s.SourceAppUserModelId().ok()).map(|a| a.to_string());
    if wired.as_ref().map(|w| &w.app) == app.as_ref() {
        return;
    }
    if let Some(w) = wired.take() {
        let _ = w.session.RemoveMediaPropertiesChanged(w.tokens.0);
        let _ = w.session.RemovePlaybackInfoChanged(w.tokens.1);
    }
    let (Some(session), Some(app)) = (session, app) else { return };
    let (a, b) = (tx.clone(), tx.clone());
    let props = session.MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
        let _ = a.send(Msg::Changed);
        Ok(())
    }));
    let info = session.PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
        let _ = b.send(Msg::Changed);
        Ok(())
    }));
    if let (Ok(p), Ok(i)) = (props, info) {
        *wired = Some(Wired { session, app, tokens: (p, i) });
    }
}

fn read(mgr: &Manager, cache: &mut ArtCache) -> Option<Track> {
    let session = mgr.GetCurrentSession().ok()?;
    let props = session.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
    let title = props.Title().ok()?.to_string();
    let artist = props.Artist().map(|a| a.to_string()).unwrap_or_default();
    if title.is_empty() {
        return None;
    }
    let info = session.GetPlaybackInfo().ok()?;
    let playing = info.PlaybackStatus().ok()? == Status::Playing;
    let can_next = info.Controls().and_then(|c| c.IsNextEnabled()).unwrap_or(false);
    if cache.key != (title.clone(), artist.clone()) {
        cache.key = (title.clone(), artist.clone());
        cache.art = thumbnail(&props).and_then(|b| art(&b)).map(Arc::new);
    }
    Some(Track { title, artist, art: cache.art.clone(), can_next, playing })
}

/// The session's picture, at most 2 MB.
fn thumbnail(props: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties) -> Option<Vec<u8>> {
    let stream = props.Thumbnail().ok()?.OpenReadAsync().ok()?.join().ok()?;
    let size = u32::try_from(stream.Size().ok()?).ok().filter(|s| *s <= 2 * 1024 * 1024)?;
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    reader.LoadAsync(size).ok()?.join().ok()?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "reads this computer's media session: run by hand"]
    fn the_media_session_reads_without_panicking() {
        // SAFETY: COM start-up of the test thread.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let mgr = Manager::RequestAsync().unwrap().join().unwrap();
        let _ = read(&mgr, &mut ArtCache::default());
    }
}
