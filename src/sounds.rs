//! Rustcord's own sounds (docs/DESIGN.md 7.1): synthesized in code (`synth`), or the user's own
//! file for a message, mention or incoming call (`file`, Settings > Notifications). Each
//! one-shot gets a short thread with its own speaker stream (`media::audio::AudioOut`, the
//! voice output device), the rings one loop thread each; with nothing playing no thread and
//! no stream exist, so idle CPU stays 0 %. Voice, mute and stream sounds come from comparing
//! one `Snap` of the state per pass with the last (`Cues`), not from hooks at the toggles.
//! Nothing here touches the network.

mod file;
mod synth;

use crate::backend::ConnState;
use crate::config::{Settings, SoundSettings};
use crate::model::{Id, UserStatus};
use crate::state::AppState;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sound {
    Message,
    Mention,
    IncomingRing,
    OutgoingRing,
    Join,
    Leave,
    Disconnect,
    Mute,
    Unmute,
    Deafen,
    Undeafen,
    PttOn,
    PttOff,
    StreamStart,
    StreamEnd,
}

impl Sound {
    /// In settings order.
    pub const ALL: [Sound; 15] = [
        Sound::Message,
        Sound::Mention,
        Sound::IncomingRing,
        Sound::OutgoingRing,
        Sound::Join,
        Sound::Leave,
        Sound::Disconnect,
        Sound::Mute,
        Sound::Unmute,
        Sound::Deafen,
        Sound::Undeafen,
        Sound::PttOn,
        Sound::PttOff,
        Sound::StreamStart,
        Sound::StreamEnd,
    ];

    /// The sounds that can be replaced by the user's own file.
    pub const CUSTOM: [Sound; 3] = [Sound::Message, Sound::Mention, Sound::IncomingRing];

    /// settings.toml key; never rename.
    pub fn key(self) -> &'static str {
        match self {
            Sound::Message => "message",
            Sound::Mention => "mention",
            Sound::IncomingRing => "incoming_ring",
            Sound::OutgoingRing => "outgoing_ring",
            Sound::Join => "join",
            Sound::Leave => "leave",
            Sound::Disconnect => "disconnect",
            Sound::Mute => "mute",
            Sound::Unmute => "unmute",
            Sound::Deafen => "deafen",
            Sound::Undeafen => "undeafen",
            Sound::PttOn => "ptt_on",
            Sound::PttOff => "ptt_off",
            Sound::StreamStart => "stream_start",
            Sound::StreamEnd => "stream_end",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Sound::Message => "Message",
            Sound::Mention => "Mention",
            Sound::IncomingRing => "Incoming call",
            Sound::OutgoingRing => "Outgoing call",
            Sound::Join => "Voice join",
            Sound::Leave => "Voice leave",
            Sound::Disconnect => "Disconnect",
            Sound::Mute => "Mute",
            Sound::Unmute => "Unmute",
            Sound::Deafen => "Deafen",
            Sound::Undeafen => "Undeafen",
            Sound::PttOn => "Push to talk on",
            Sound::PttOff => "Push to talk off",
            Sound::StreamStart => "Stream start",
            Sound::StreamEnd => "Stream end",
        }
    }

    pub fn meta(self) -> Option<&'static str> {
        Some(match self {
            Sound::Message => "New messages your notification settings allow, also with desktop notifications off.",
            Sound::Mention => "Messages that mention you.",
            Sound::IncomingRing => "Rings until you answer or decline.",
            Sound::OutgoingRing => "Rings until someone picks up.",
            Sound::Join => "You or someone else joins your voice channel or call.",
            Sound::Leave => "Someone else leaves your voice channel or call.",
            Sound::Disconnect => "You leave voice.",
            Sound::PttOn => "Your push-to-talk key opens your microphone.",
            Sound::StreamStart => "You or someone in your voice channel goes live.",
            _ => return None,
        })
    }
}

/// `gain = (percent / 100)^2`.
pub fn gain(percent: u8) -> f32 {
    (f32::from(percent.min(100)) / 100.0).powi(2)
}

/// The playback gain of `sound` now, or None when it stays silent.
pub fn volume(s: &Settings, sound: Sound, deafened: bool, dnd: bool) -> Option<f32> {
    let silent = !s.sounds.enabled
        || s.sounds.off.iter().any(|k| k == sound.key())
        || (s.streamer.enabled && s.streamer.disable_sounds)
        // The deafen switch itself is audible: it marks the change.
        || (deafened && !matches!(sound, Sound::Deafen | Sound::Undeafen))
        || (dnd && matches!(sound, Sound::Message | Sound::Mention | Sound::IncomingRing))
        || s.sounds.volume == 0;
    (!silent).then(|| gain(s.sounds.volume))
}

/// I'm deafened: by myself or by a moderator.
pub fn deafened(state: &AppState, self_deaf: bool) -> bool {
    let me = state.me.as_ref().map(|u| u.id);
    let server = state.voice.channel_id.zip(me).and_then(|(c, me)| state.voice_states.get(&c)?.get(&me)).is_some_and(|v| v.deaf);
    self_deaf || server
}

pub fn dnd(state: &AppState) -> bool {
    state.chat.my_status == Some(UserStatus::Dnd)
}

/// The ring that should loop now.
pub fn ring(state: &AppState) -> Option<Sound> {
    if state.incoming_call().is_some() {
        Some(Sound::IncomingRing)
    } else {
        state.outgoing_ring().then_some(Sound::OutgoingRing)
    }
}

const MIN_GAP: Duration = Duration::from_millis(200);
const MAX_PLAYING: usize = 3;
const TAIL: Duration = Duration::from_millis(50);
const RING_MAX: Duration = Duration::from_secs(60);

static PLAYING: AtomicUsize = AtomicUsize::new(0);
static LAST: Mutex<[Option<Instant>; 15]> = Mutex::new([None; 15]);
/// The looping ring: which sound, with which file, and its stop flag.
static RING: Mutex<Option<Ring>> = Mutex::new(None);
type Ring = (Sound, Option<String>, Arc<AtomicBool>);

/// Screenshot runs never play anything.
fn silent() -> bool {
    static S: OnceLock<bool> = OnceLock::new();
    *S.get_or_init(|| std::env::var_os("RUSTCORD_SCREENSHOT").is_some())
}

/// The user's own file for `sound`: the first of `places` (a channel, its parent, its server, or
/// a DM) that has one, else the one for everywhere. Only `Sound::CUSTOM` sounds have files.
pub fn file_for<'a>(s: &'a SoundSettings, sound: Sound, places: &[Id]) -> Option<&'a str> {
    if !Sound::CUSTOM.contains(&sound) {
        return None;
    }
    let key = sound.key();
    places.iter().find_map(|p| s.places.get(p)?.get(key)).or_else(|| s.files.get(key)).map(String::as_str)
}

/// Where a message in `channel` belongs, most specific first: the channel, its parent (threads)
/// and its server.
pub fn places(state: &AppState, channel: Id) -> Vec<Id> {
    let ch = state.channels.get(&channel);
    [Some(channel), ch.and_then(|c| c.parent_id), ch.and_then(|c| c.guild_id)].into_iter().flatten().collect()
}

/// Plays `sound` once if the gates let it through (no custom file: the voice cues).
pub fn play(sound: Sound, s: &Settings, deafened: bool, dnd: bool) {
    play_in(sound, s, deafened, dnd, &[]);
}

/// `play` for a sound that belongs to a place: its custom file, if the user set one.
pub fn play_in(sound: Sound, s: &Settings, deafened: bool, dnd: bool, places: &[Id]) {
    if let Some(v) = volume(s, sound, deafened, dnd) {
        start(sound, v, s.voice_output.clone(), file_for(&s.sounds, sound, places).map(Into::into));
    }
}

/// The user clicked a play button: ignores the switches, DND and Streamer mode. A ring plays
/// one cycle. Plays the file set for everywhere, if any.
pub fn preview(sound: Sound, s: &Settings) {
    preview_file(sound, file_for(&s.sounds, sound, &[]), s);
}

/// `preview` with a given file (a place's row of the Custom sounds section); None: Rustcord's sound.
pub fn preview_file(sound: Sound, path: Option<&str>, s: &Settings) {
    start(sound, gain(s.sounds.volume), s.voice_output.clone(), path.map(Into::into));
}

/// Checks a picked file at once, with the words for the toast.
pub fn check(path: &std::path::Path) -> Result<(), &'static str> {
    match file::load(path) {
        Ok(_) => Ok(()),
        Err(file::Fail::TooBig) => Err("This file is over 5 MB. Pick a shorter sound."),
        Err(file::Fail::Unreadable) => Err("Rustcord can't play this file. Pick a WAV or Ogg Opus file."),
    }
}

/// The file's samples, or Rustcord's sound when there is none or it can't be read now.
fn pcm_of(sound: Sound, file: Option<&std::path::Path>) -> file::Pcm {
    file.and_then(|p| file::load(p).ok()).unwrap_or_else(|| file::Pcm { rate: synth::RATE, channels: 1, samples: synth::render(sound) })
}

struct Playing;

impl Drop for Playing {
    fn drop(&mut self) {
        PLAYING.fetch_sub(1, Ordering::Relaxed);
    }
}

// shortcut: opens the speaker per sound (~20-60 ms, more with a named output device); keep one
// paused stream during calls if push-to-talk sounds feel late.
fn start(sound: Sound, v: f32, output: Option<String>, file: Option<std::path::PathBuf>) {
    if silent() || PLAYING.load(Ordering::Relaxed) >= MAX_PLAYING {
        return;
    }
    {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if last[sound as usize].is_some_and(|t| now.duration_since(t) < MIN_GAP) {
            return;
        }
        last[sound as usize] = Some(now);
    }
    PLAYING.fetch_add(1, Ordering::Relaxed);
    let guard = Playing;
    let spawned = std::thread::Builder::new().name("rustcord-sound".into()).spawn(move || {
        let _guard = guard;
        let pcm = pcm_of(sound, file.as_deref());
        // Sounds are optional: no speaker, no sound, no toast.
        let Ok(mut out) = crate::media::audio::AudioOut::open(output.as_deref(), pcm.rate) else { return };
        out.set_volume(v);
        out.push(&pcm.samples, pcm.channels);
        while out.queued_frames() > 0 {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(TAIL);
    });
    // The closure (and its guard) is dropped when the spawn fails, which decrements PLAYING.
    drop(spawned);
}

/// Keeps the ring loop in step with `want` (once per pass): starts, switches or stops it. `place`
/// is the call's channel, whose own file (a DM) rings instead of the one for everywhere.
pub fn set_ring(want: Option<Sound>, place: Option<Id>, s: &Settings, deafened: bool, dnd: bool) {
    let gated = want.filter(|_| !silent()).and_then(|r| volume(s, r, deafened, dnd).map(|v| (r, v)));
    let file = gated.and_then(|(r, _)| file_for(&s.sounds, r, place.as_slice())).map(str::to_owned);
    let mut cur = RING.lock().unwrap_or_else(|e| e.into_inner());
    if cur.as_ref().map(|c| (c.0, &c.1)) == gated.map(|g| (g.0, &file)) {
        return;
    }
    if let Some((_, _, stop)) = cur.take() {
        stop.store(true, Ordering::Relaxed);
    }
    let Some((sound, v)) = gated else { return };
    let stop = Arc::new(AtomicBool::new(false));
    let (flag, output, path) = (stop.clone(), s.voice_output.clone(), file.clone());
    let spawned = std::thread::Builder::new().name("rustcord-ring".into()).spawn(move || {
        let pcm = pcm_of(sound, path.as_deref().map(std::path::Path::new));
        let Ok(mut out) = crate::media::audio::AudioOut::open(output.as_deref(), pcm.rate) else { return };
        out.set_volume(v);
        let began = Instant::now();
        while !flag.load(Ordering::Relaxed) && began.elapsed() < RING_MAX {
            if out.queued_frames() < (out.rate() / 10) as usize {
                out.push(&pcm.samples, pcm.channels);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // A short fade instead of a click.
        for k in [0.5, 0.25, 0.1, 0.0] {
            out.set_volume(v * k);
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    if spawned.is_ok() {
        *cur = Some((sound, file, stop));
    }
}

/// What the cues compare from pass to pass.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snap {
    /// Joining or connected.
    pub channel: Option<Id>,
    pub connected: bool,
    /// Sorted: the others in my channel; empty unless connected.
    pub others: Vec<Id>,
    /// Sorted: who is live in my channel (me included); empty unless connected.
    pub streams: Vec<Id>,
    pub mute: bool,
    pub deaf: bool,
    /// Push to talk: the key is held.
    pub ptt: bool,
    pub gateway: bool,
}

impl Snap {
    pub fn of(state: &AppState, mute: bool, deaf: bool, ptt_mode: bool) -> Snap {
        let me = state.me.as_ref().map(|u| u.id);
        let channel = state.voice.channel_id;
        let connected = state.voice.connected;
        let (mut others, mut streams) = (Vec::new(), Vec::new());
        if let Some(members) = channel.filter(|_| connected).and_then(|c| state.voice_states.get(&c)) {
            others = members.keys().copied().filter(|u| Some(*u) != me).collect();
            streams = members.iter().filter(|(_, v)| v.self_stream).map(|(u, _)| *u).collect();
            others.sort();
            streams.sort();
        }
        Snap {
            channel,
            connected,
            others,
            streams,
            mute,
            deaf,
            ptt: ptt_mode && connected && me.is_some_and(|m| state.voice.speaking.contains(&m)),
            gateway: state.conn == ConnState::Connected,
        }
    }
}

/// Which sound a change between two passes makes. Pure.
#[derive(Default)]
pub struct Cues {
    prev: Option<Snap>,
    joined: Option<Id>,
    settle: Option<Instant>,
}

/// After a join or a gateway change the voice states are rebuilt; changes in them stay silent.
const SETTLE: Duration = Duration::from_secs(2);

impl Cues {
    /// At most one sound per call; `prev` always becomes `now`.
    pub fn step(&mut self, now: Snap, t: Instant) -> Option<Sound> {
        let mine = now.channel.filter(|_| now.connected);
        let Some(prev) = self.prev.replace(now.clone()) else {
            self.joined = mine;
            return None;
        };
        if now.gateway != prev.gateway {
            self.settle = Some(t + SETTLE);
        }
        let me = if now.channel.is_none() && self.joined.is_some() {
            self.joined = None;
            Some(Sound::Disconnect)
        } else if mine.is_some() && mine != self.joined {
            self.joined = mine;
            self.settle = Some(t + SETTLE);
            Some(Sound::Join)
        } else {
            None
        };
        let switch = if now.deaf != prev.deaf {
            Some(if now.deaf { Sound::Deafen } else { Sound::Undeafen })
        } else if now.mute != prev.mute {
            Some(if now.mute { Sound::Mute } else { Sound::Unmute })
        } else {
            None
        };
        let same_call = mine.is_some() && prev.connected && prev.channel == now.channel && me.is_none();
        let ptt = (same_call && now.ptt != prev.ptt).then_some(if now.ptt { Sound::PttOn } else { Sound::PttOff });
        let settled = now.gateway && self.settle.is_none_or(|s| t >= s);
        let crowd = (same_call && settled)
            .then(|| {
                let gone = |a: &[Id], b: &[Id]| a.iter().any(|u| !b.contains(u));
                if gone(&prev.others, &now.others) {
                    Some(Sound::Leave)
                } else if gone(&now.others, &prev.others) {
                    Some(Sound::Join)
                } else if gone(&prev.streams, &now.streams) {
                    Some(Sound::StreamEnd)
                } else if gone(&now.streams, &prev.streams) {
                    Some(Sound::StreamStart)
                } else {
                    None
                }
            })
            .flatten();
        switch.or(me).or(ptt).or(crowd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Id = Id(5);
    const B: Id = Id(6);
    const C1: Id = Id(100);
    const C2: Id = Id(101);

    #[test]
    fn gates() {
        let mut s = Settings::default();
        let g = |s: &Settings, x, deaf, dnd| volume(s, x, deaf, dnd);
        assert!((g(&s, Sound::Message, false, false).unwrap() - 0.49).abs() < 1e-6);
        for x in [Sound::Message, Sound::Mention, Sound::IncomingRing] {
            assert_eq!(g(&s, x, false, true), None);
        }
        assert!(g(&s, Sound::OutgoingRing, false, true).is_some() && g(&s, Sound::Mute, false, true).is_some());
        assert_eq!(g(&s, Sound::Message, true, false), None);
        assert!(g(&s, Sound::Deafen, true, false).is_some() && g(&s, Sound::Undeafen, true, false).is_some());
        s.sounds.off.push("mention".into());
        assert_eq!((g(&s, Sound::Mention, false, false), g(&s, Sound::Message, false, false).is_some()), (None, true));
        s.streamer.enabled = true;
        assert_eq!(g(&s, Sound::Message, false, false), None);
        s.streamer.disable_sounds = false;
        assert!(g(&s, Sound::Message, false, false).is_some());
        s.sounds.volume = 0;
        assert_eq!(g(&s, Sound::Message, false, false), None);
        s.sounds.volume = 70;
        s.sounds.enabled = false;
        assert_eq!(g(&s, Sound::Message, false, false), None);
        let keys: std::collections::HashSet<_> = Sound::ALL.iter().map(|x| x.key()).collect();
        assert_eq!(keys.len(), 15);
        assert!(Sound::ALL.iter().enumerate().all(|(i, x)| *x as usize == i));
    }

    #[test]
    fn file_for_prefers_channel_then_parent_then_server_then_everywhere() {
        let mut s = SoundSettings::default();
        let (thread, parent, guild) = (Id(3), Id(2), Id(1));
        s.files.insert("message".into(), "all.wav".into());
        s.files.insert("join".into(), "join.wav".into());
        s.places.insert(guild, [("message".to_owned(), "server.wav".to_owned())].into());
        let chain = [thread, parent, guild];
        assert_eq!(file_for(&s, Sound::Message, &chain), Some("server.wav"));
        s.places.insert(parent, [("message".to_owned(), "parent.wav".to_owned())].into());
        assert_eq!(file_for(&s, Sound::Message, &chain), Some("parent.wav"));
        s.places.insert(thread, [("message".to_owned(), "thread.wav".to_owned())].into());
        assert_eq!(file_for(&s, Sound::Message, &chain), Some("thread.wav"));
        assert_eq!(file_for(&s, Sound::Message, &[Id(9)]), Some("all.wav"), "everywhere");
        assert_eq!(file_for(&s, Sound::Mention, &chain), None, "nothing set: Rustcord's sound");
        assert_eq!(file_for(&s, Sound::Join, &chain), None, "voice cues never get a file");
    }

    fn snap() -> Snap {
        Snap { gateway: true, ..Default::default() }
    }

    fn in_call(others: &[Id], streams: &[Id]) -> Snap {
        Snap { channel: Some(C1), connected: true, others: others.to_vec(), streams: streams.to_vec(), ..snap() }
    }

    /// Steps `cues` through `snaps`, `dt` ms apart; the sounds that played.
    fn run(cues: &mut Cues, t0: Instant, at_ms: u64, snaps: Vec<Snap>) -> Vec<Option<Sound>> {
        snaps.into_iter().enumerate().map(|(i, s)| cues.step(s, t0 + Duration::from_millis(at_ms + i as u64))).collect()
    }

    #[test]
    fn cues() {
        let t0 = Instant::now();
        // The first step adopts silently, even muted; then unmute, mute.
        let mut c = Cues::default();
        assert_eq!(c.step(Snap { mute: true, ..snap() }, t0), None);
        assert_eq!(c.step(snap(), t0), Some(Sound::Unmute));
        assert_eq!(c.step(Snap { mute: true, ..snap() }, t0), Some(Sound::Mute));
        assert_eq!(c.step(Snap { mute: true, ..snap() }, t0), None);
        // Deafen alone; undeafen with both changing plays Undeafen only.
        let mut c = Cues::default();
        c.step(snap(), t0);
        assert_eq!(c.step(Snap { deaf: true, ..snap() }, t0), Some(Sound::Deafen));
        c.step(Snap { deaf: true, mute: true, ..snap() }, t0);
        assert_eq!(c.step(snap(), t0), Some(Sound::Undeafen));
        // Joining is silent, connected plays Join; a member within 2 s is silent.
        let mut c = Cues::default();
        c.step(snap(), t0);
        assert_eq!(c.step(Snap { channel: Some(C1), ..snap() }, t0), None);
        assert_eq!(c.step(in_call(&[], &[]), t0), Some(Sound::Join));
        assert_eq!(c.step(in_call(&[A], &[]), t0 + Duration::from_secs(1)), None);
        let later = t0 + Duration::from_secs(3);
        assert_eq!(c.step(in_call(&[A, B], &[]), later), Some(Sound::Join));
        assert_eq!(c.step(in_call(&[A], &[]), later), Some(Sound::Leave));
        // Streams: start and end; a streamer leaving is Leave.
        assert_eq!(c.step(in_call(&[A], &[A]), later), Some(Sound::StreamStart));
        assert_eq!(c.step(in_call(&[A], &[]), later), Some(Sound::StreamEnd));
        c.step(in_call(&[A], &[A]), later);
        assert_eq!(c.step(in_call(&[], &[]), later), Some(Sound::Leave));
        // Push to talk.
        assert_eq!(c.step(Snap { ptt: true, ..in_call(&[], &[]) }, later), Some(Sound::PttOn));
        assert_eq!(c.step(in_call(&[], &[]), later), Some(Sound::PttOff));
        // A voice reconnect is silent.
        assert_eq!(c.step(Snap { channel: Some(C1), ..snap() }, later), None);
        assert_eq!(c.step(in_call(&[], &[]), later), None);
        // Gateway down and up: members vanishing and returning within 2 s are silent.
        let mut c = Cues::default();
        c.step(in_call(&[A], &[]), t0);
        let out = run(
            &mut c,
            t0,
            10_000,
            vec![
                Snap { gateway: false, ..in_call(&[A], &[]) },
                Snap { gateway: false, ..in_call(&[], &[]) },
                in_call(&[], &[]),
                in_call(&[A], &[]),
            ],
        );
        assert_eq!(out, vec![None; 4]);
        // Another channel plays Join, leaving Disconnect; a join that never connected is silent.
        let mut c = Cues::default();
        c.step(in_call(&[], &[]), t0);
        assert_eq!(c.step(Snap { channel: Some(C2), connected: true, ..snap() }, t0), Some(Sound::Join));
        assert_eq!(c.step(snap(), t0), Some(Sound::Disconnect));
        c.step(Snap { channel: Some(C1), ..snap() }, t0);
        assert_eq!(c.step(snap(), t0), None);
    }
}
