//! Audio engine: cpal capture and playback, Opus (opus-rs) encode/decode, per-SSRC jitter
//! buffers, mixing, mute/deafen, input volume, voice activity and push-to-talk.
//!
//! OWNERSHIP: audio engineer. Keep the pub items' signatures (mod.rs and transport.rs use
//! them); add private helpers freely. Spec: VOICE_NOTES §2.8 (send), §2.9 (receive).
//!
//! Threads: one "rustcord-audio" std thread owns the cpal streams (not every cpal backend's
//! `Stream` is `Send`) and does Opus encode/decode + mixing every 10 ms; cpal callbacks only
//! convert and copy samples through lock-free bounded rings (`device::Ring`), never block and
//! never allocate. Received packets reach the thread through a bounded queue (`Playback`).
//! Push-to-talk polls `GetAsyncKeyState` (windows-sys `Win32_UI_Input_KeyboardAndMouse`) every
//! 10 ms on that thread; on Linux / macOS it reads the key as the focused window saw it.
//!
//! Memory bounds: capture queue `CAPTURE_QUEUE` frames, received queue `PACKET_QUEUE`
//! packets, jitter `JITTER_PACKETS` per SSRC, mic ring 200 ms, output ring ≤ 100 ms; a
//! decoder is dropped with its SSRC or after `STREAM_IDLE` without packets.

mod device;
mod dsp;

use super::{InputMode, VoiceEvent, VoiceEvents};
use crate::model::Id;
use dsp::{Jitter, Pop, Speaking, Trailer, Tx, Vad};
use opus_rs::{OpusDecoder, OpusEncoder};
use std::collections::HashMap;
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Discord voice format: 48 kHz, 2 channels (a mono mic is duplicated), 20 ms frames.
pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
/// Samples per channel in one 20 ms frame (RTP timestamp step).
pub const FRAME_SAMPLES: usize = 960;
/// Capture -> transport queue, in frames (200 ms). When full, the newest frame is dropped.
pub const CAPTURE_QUEUE: usize = 10;
/// Jitter buffer depth per SSRC, in packets.
pub const JITTER_PACKETS: usize = 5;
/// Opus silence; sent 5 times before `Captured::End` (DAVE passes it through unencrypted).
pub const SILENCE_FRAME: [u8; 3] = [0xF8, 0xFF, 0xFE];

/// Audio thread period: push-to-talk poll, capture drain, playback refill.
const TICK: Duration = Duration::from_millis(10);
/// Received packets waiting for the audio thread; more are dropped.
const PACKET_QUEUE: usize = 64;
/// Decode scratch per channel: the longest Opus packet (120 ms).
const MAX_DECODE: usize = 5760;
/// Push-to-talk stays open this long after the key is released.
const PTT_RELEASE: Duration = Duration::from_millis(20);
/// A remote stream without packets this long is dropped with its decoder.
const STREAM_IDLE: Duration = Duration::from_secs(10);
/// `VoiceEvent::LocalLevel` every 10 frames (200 ms).
const LEVEL_FRAMES: u8 = 10;
/// Smallest level change (5 dB) worth an event: each one repaints the whole UI.
// shortcut: no "meter shown" flag; when the voice settings meter exists, report only while it is open.
const LEVEL_STEP: f32 = 0.05;

/// Settings the engine runs with. The manager sends a complete new one on every change.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioConfig {
    /// cpal device names; None = the system default.
    pub input: Option<String>,
    pub output: Option<String>,
    pub mode: InputMode,
    /// Never transmit (self mute, deafen, server mute or stage suppress). Going muted while
    /// transmitting still ends with the silence frames and `Captured::End`.
    pub muted: bool,
    /// Play nothing (self or server deafen); packets are still decoded or dropped cheaply.
    pub deafened: bool,
}

/// Capture side output, in order.
#[derive(Debug, PartialEq)]
pub enum Captured {
    /// One 20 ms Opus frame (48 kHz stereo, VOIP, FEC on, 64 kbit/s), not yet E2EE-encrypted.
    Frame(Vec<u8>),
    /// Transmission stopped (after the 5 silence frames): the transport sends op 5 speaking 0.
    End,
}

/// The audio half the transport works with.
pub struct MediaLink {
    /// Frames to send; `None` from `recv` = the audio engine is gone.
    pub captured: mpsc::Receiver<Captured>,
    pub playback: Playback,
}

/// UDP task -> audio thread.
enum Packet {
    Opus { user_id: Id, ssrc: u32, seq: u16, opus: Vec<u8> },
    Remove(u32),
}

/// Receive path into the engine. Cheap to clone, `Send + Sync`; methods never block (called
/// from the UDP task).
#[derive(Clone)]
pub struct Playback {
    tx: std_mpsc::SyncSender<Packet>,
}

impl Playback {
    /// One received Opus frame, already transport- and E2EE-decrypted. `seq`/`timestamp` are
    /// the RTP fields (u16 wraparound ordering in the jitter buffer). Emits
    /// `VoiceEvent::Speaking` for `user_id` when its activity starts/stops (~250 ms rule).
    pub fn push(&self, user_id: Id, ssrc: u32, seq: u16, _timestamp: u32, opus: &[u8]) {
        // Full queue (audio thread behind): drop, the jitter buffer conceals it.
        let _ = self.tx.try_send(Packet::Opus { user_id, ssrc, seq, opus: opus.to_vec() });
    }

    /// The SSRC is gone (op 13 client disconnect): drop its buffer and decoder.
    pub fn remove(&self, ssrc: u32) {
        let _ = self.tx.try_send(Packet::Remove(ssrc));
    }
}

/// A running engine for one call. Dropping it stops capture and playback, closes the devices
/// and ends the audio thread.
pub struct Audio {
    /// Dropping it ends the audio thread (within one tick).
    config: std_mpsc::Sender<AudioConfig>,
}

impl Audio {
    /// Start capture and playback for one call. Returns at once (the audio thread opens the
    /// devices); device errors arrive as `VoiceEvent::Error` and the call continues without
    /// that direction. `me` is used for my own `VoiceEvent::Speaking`; `VoiceEvent::LocalLevel`
    /// at most ~5 per second, only on a 5 % change.
    pub fn start(cfg: AudioConfig, me: Id, events: VoiceEvents) -> (Audio, MediaLink) {
        let (frames, captured) = mpsc::channel(CAPTURE_QUEUE);
        let (packets, packet_rx) = std_mpsc::sync_channel(PACKET_QUEUE);
        let (config, config_rx) = std_mpsc::channel();
        let ev = events.clone();
        let spawned = std::thread::Builder::new()
            .name("rustcord-audio".into())
            .spawn(move || Engine::new(cfg, me, events, frames).run(config_rx, packet_rx));
        if spawned.is_err() {
            ev.send(VoiceEvent::Error("Could not start audio.".into()));
        }
        (Audio { config }, MediaLink { captured, playback: Playback { tx: packets } })
    }

    /// Apply a new config: mute/deafen/mode at once; devices re-opened only when the names
    /// changed (the `MediaLink` stays valid).
    pub fn set_config(&self, cfg: AudioConfig) {
        let _ = self.config.send(cfg);
    }
}

/// `(inputs, outputs)` device names of the default cpal host. Blocking (WASAPI): call it
/// from `spawn_blocking`.
pub fn list_devices() -> (Vec<String>, Vec<String>) {
    device::list(&cpal::default_host())
}

/// Whether the push-to-talk key is held (global; the key still reaches other apps).
#[cfg(windows)]
fn key_down(vk: u16) -> bool {
    // SAFETY: GetAsyncKeyState only reads the global key state; any key code is valid.
    unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(i32::from(vk)) < 0 }
}

/// Linux, macOS: the key as the window last saw it (`set_ptt_held`; no global key state).
#[cfg(not(windows))]
fn key_down(_vk: u16) -> bool {
    PTT_HELD.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(not(windows))]
static PTT_HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Linux, macOS: the push-to-talk key is held in the focused Rustcord window (set each pass by
/// `user_settings::logic`; false while the window is in the background).
#[cfg(not(windows))]
pub fn set_ptt_held(held: bool) {
    PTT_HELD.store(held, std::sync::atomic::Ordering::Relaxed);
}

/// Input volume slider maximum (percent; Settings > Voice & Video).
pub const MAX_INPUT_VOLUME: u16 = 200;

/// Input gain as f32 bits, read for every captured frame so a change applies at once (also mid-call).
static INPUT_GAIN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1.0f32.to_bits());

/// Microphone volume in percent (clamped to `MAX_INPUT_VOLUME`), from now on.
pub fn set_input_volume(percent: u16) {
    INPUT_GAIN.store(dsp::volume_gain(percent).to_bits(), std::sync::atomic::Ordering::Relaxed);
}

/// One remote SSRC.
struct Remote {
    user_id: Id,
    jitter: Jitter,
    /// Created on the first frame played.
    decoder: Option<OpusDecoder>,
    speaking: Speaking,
    last_packet: Instant,
}

/// Everything the audio thread owns.
struct Engine {
    cfg: AudioConfig,
    me: Id,
    events: VoiceEvents,
    captured: mpsc::Sender<Captured>,
    host: cpal::Host,
    input: Option<device::Input>,
    output: Option<device::Output>,
    encoder: Option<OpusEncoder>,
    vad: Vad,
    trailer: Trailer,
    /// Push-to-talk open until then.
    ptt_until: Option<Instant>,
    /// 48 kHz mono not yet cut into frames.
    pending: Vec<f32>,
    level_frames: u8,
    level_peak: f32,
    level_sent: f32,
    streams: HashMap<u32, Remote>,
    decoded: Vec<f32>,
    mix: Vec<f32>,
}

impl Engine {
    fn new(cfg: AudioConfig, me: Id, events: VoiceEvents, captured: mpsc::Sender<Captured>) -> Self {
        let encoder = dsp::encoder().map_err(|_| events.send(VoiceEvent::Error("Could not start the voice encoder.".into()))).ok();
        Self {
            cfg,
            me,
            events,
            captured,
            host: cpal::default_host(),
            input: None,
            output: None,
            encoder,
            vad: Vad::default(),
            trailer: Trailer::default(),
            ptt_until: None,
            pending: Vec::new(),
            level_frames: 0,
            level_peak: -100.0,
            level_sent: -1.0,
            streams: HashMap::new(),
            decoded: vec![0.0; MAX_DECODE * CHANNELS],
            mix: vec![0.0; FRAME_SAMPLES * CHANNELS],
        }
    }

    fn run(mut self, config: std_mpsc::Receiver<AudioConfig>, packets: std_mpsc::Receiver<Packet>) {
        self.open_input();
        self.open_output();
        loop {
            match config.recv_timeout(TICK) {
                Ok(cfg) => self.set_config(cfg),
                Err(std_mpsc::RecvTimeoutError::Timeout) => {}
                Err(std_mpsc::RecvTimeoutError::Disconnected) => return, // `Audio` dropped
            }
            let now = Instant::now();
            for p in packets.try_iter() {
                self.on_packet(p, now);
            }
            self.capture(now);
            self.play();
            self.expire(now);
        }
    }

    fn set_config(&mut self, cfg: AudioConfig) {
        let (new_input, new_output) = (cfg.input != self.cfg.input, cfg.output != self.cfg.output);
        if cfg.deafened && !self.cfg.deafened {
            for r in self.streams.values_mut() {
                r.jitter = Jitter::default();
            }
        }
        self.cfg = cfg;
        if new_input {
            self.open_input();
        }
        if new_output {
            self.open_output();
        }
    }

    fn open_input(&mut self) {
        self.input = None; // release the old device first
        match device::Input::open(&self.host, self.cfg.input.as_deref(), &self.events) {
            Ok(input) => self.input = Some(input),
            Err(e) => self.events.send(VoiceEvent::Error(e)),
        }
    }

    fn open_output(&mut self) {
        self.output = None;
        match device::Output::open(&self.host, self.cfg.output.as_deref(), &self.events) {
            Ok(output) => self.output = Some(output),
            Err(e) => self.events.send(VoiceEvent::Error(e)),
        }
    }

    fn speaking(&self, user_id: Id, speaking: bool) {
        self.events.send(VoiceEvent::Speaking { user_id, speaking });
    }

    // ---- send path ----

    fn capture(&mut self, now: Instant) {
        let Some(input) = &mut self.input else { return };
        input.read(&mut self.pending);
        if let InputMode::PushToTalk { vk } = self.cfg.mode
            && key_down(vk)
        {
            self.ptt_until = Some(now + PTT_RELEASE);
        }
        let gain = f32::from_bits(INPUT_GAIN.load(std::sync::atomic::Ordering::Relaxed));
        let mut frame = [0.0; FRAME_SAMPLES];
        let mut used = 0;
        while self.pending.len() - used >= FRAME_SAMPLES {
            frame.copy_from_slice(&self.pending[used..used + FRAME_SAMPLES]);
            used += FRAME_SAMPLES;
            dsp::apply_volume(&mut frame, gain);
            self.on_frame(&frame, now);
        }
        self.pending.drain(..used);
    }

    fn on_frame(&mut self, mono: &[f32; FRAME_SAMPLES], now: Instant) {
        let db = dsp::db(mono);
        self.report_level(db);
        let open = !self.cfg.muted
            && match self.cfg.mode {
                InputMode::VoiceActivity { threshold_db } => self.vad.open(db, threshold_db),
                InputMode::PushToTalk { .. } => self.ptt_until.is_some_and(|t| now < t),
            };
        let was_talking = self.trailer.talking();
        match self.trailer.next(open) {
            Tx::Voice => {
                if !was_talking {
                    self.speaking(self.me, true);
                }
                if let Some(opus) = self.encoder.as_mut().and_then(|enc| dsp::encode(enc, mono)) {
                    self.send(Captured::Frame(opus));
                }
            }
            Tx::Silence => self.send(Captured::Frame(SILENCE_FRAME.to_vec())),
            Tx::End => {
                self.send(Captured::End);
                self.speaking(self.me, false);
            }
            Tx::Nothing => {}
        }
    }

    /// Full queue (transport behind or not connected yet): the newest frame is dropped.
    fn send(&self, c: Captured) {
        let _ = self.captured.try_send(c);
    }

    /// Peak level of the last 200 ms, only when it moved by `LEVEL_STEP`; 0 while muted.
    fn report_level(&mut self, db: f32) {
        self.level_peak = self.level_peak.max(db);
        self.level_frames += 1;
        if self.level_frames < LEVEL_FRAMES {
            return;
        }
        let level = if self.cfg.muted { 0.0 } else { dsp::level(self.level_peak) };
        (self.level_frames, self.level_peak) = (0, -100.0);
        if (level - self.level_sent).abs() >= LEVEL_STEP {
            self.level_sent = level;
            self.events.send(VoiceEvent::LocalLevel(level));
        }
    }

    // ---- receive path ----

    fn on_packet(&mut self, p: Packet, now: Instant) {
        match p {
            Packet::Remove(ssrc) => {
                if let Some(r) = self.streams.remove(&ssrc)
                    && r.speaking.is_on()
                {
                    self.speaking(r.user_id, false);
                }
            }
            Packet::Opus { user_id, ssrc, seq, opus } => {
                let playing = !self.cfg.deafened && self.output.is_some();
                let r = self.streams.entry(ssrc).or_insert_with(|| Remote {
                    user_id,
                    jitter: Jitter::default(),
                    decoder: None,
                    speaking: Speaking::default(),
                    last_packet: now,
                });
                (r.user_id, r.last_packet) = (user_id, now);
                if let Some(speaking) = r.speaking.packet(opus != SILENCE_FRAME, now) {
                    self.events.send(VoiceEvent::Speaking { user_id, speaking });
                }
                // Deafened or no speaker: dropped cheaply, nothing would pull the buffer.
                if playing {
                    r.jitter.push(seq, opus);
                }
            }
        }
    }

    fn play(&mut self) {
        let Some(out) = self.output.as_mut().filter(|_| !self.cfg.deafened) else { return };
        for _ in 0..3 {
            if !out.wants_more() || !mix_frame(&mut self.streams, &mut self.decoded, &mut self.mix) {
                break;
            }
            out.write(&self.mix);
        }
    }

    /// Speaking timeouts; forgets streams that went quiet long ago.
    fn expire(&mut self, now: Instant) {
        let events = &self.events;
        self.streams.retain(|_, r| {
            if r.speaking.expired(now) {
                events.send(VoiceEvent::Speaking { user_id: r.user_id, speaking: false });
            }
            now.duration_since(r.last_packet) < STREAM_IDLE
        });
    }
}

/// Mixes the next 20 ms of every playing stream into `mix` (48 kHz stereo, clipped). False
/// when no stream is playing.
fn mix_frame(streams: &mut HashMap<u32, Remote>, decoded: &mut [f32], mix: &mut [f32]) -> bool {
    mix.fill(0.0);
    let mut playing = false;
    for r in streams.values_mut() {
        let pop = r.jitter.pop();
        if pop == Pop::Idle {
            continue;
        }
        playing = true;
        if r.decoder.is_none() {
            r.decoder = OpusDecoder::new(SAMPLE_RATE as i32, CHANNELS).ok();
        }
        let Some(dec) = r.decoder.as_mut() else { continue };
        let n = match (&pop, r.jitter.fec()) {
            (Pop::Frame(opus), _) => dec.decode(opus, MAX_DECODE, decoded),
            (_, Some(next)) => dec.decode_fec(next, FRAME_SAMPLES, decoded),
            _ => dec.decode(&[], FRAME_SAMPLES, decoded), // packet loss concealment
        };
        // shortcut: only the first 20 ms of a longer packet plays; Discord clients send 20 ms.
        if let Ok(n) = n {
            let pcm = &mut decoded[..n.min(FRAME_SAMPLES) * CHANNELS];
            // Plugins (VolumeBooster): the user's local volume; the mix is clipped below.
            let gain = crate::plugins::voice::gain(r.user_id);
            if gain != 1.0 {
                pcm.iter_mut().for_each(|s| *s *= gain);
            }
            dsp::mix_into(mix, pcm);
        }
    }
    dsp::clip(mix);
    playing
}

/// Test hooks for the transport's offline loopback test (no devices).
#[cfg(test)]
pub(super) mod test_hooks {
    use super::*;

    /// Encodes 20 ms mono frames like the capture path does.
    pub fn encoder() -> impl FnMut(&[f32; FRAME_SAMPLES]) -> Vec<u8> {
        let mut enc = dsp::encoder().unwrap();
        move |mono| dsp::encode(&mut enc, mono).unwrap()
    }

    /// A `Playback` and a speaker that plays what it received through the real jitter
    /// buffers, decoders and mixer: one 20 ms stereo frame per call, None while silent.
    pub fn speaker() -> (Playback, impl FnMut() -> Option<Vec<f32>>) {
        let (tx, rx) = std_mpsc::sync_channel(PACKET_QUEUE);
        let mut streams = HashMap::new();
        let (mut decoded, mut mix) = (vec![0.0; MAX_DECODE * CHANNELS], vec![0.0; FRAME_SAMPLES * CHANNELS]);
        let speaker = move || {
            for p in rx.try_iter() {
                if let Packet::Opus { user_id, ssrc, seq, opus } = p {
                    let r = streams.entry(ssrc).or_insert_with(|| tests::remote(user_id.0));
                    r.jitter.push(seq, opus);
                }
            }
            mix_frame(&mut streams, &mut decoded, &mut mix).then(|| mix.clone())
        };
        (Playback { tx }, speaker)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn remote(user: u64) -> Remote {
        Remote { user_id: Id(user), jitter: Jitter::default(), decoder: None, speaking: Speaking::default(), last_packet: Instant::now() }
    }

    #[test]
    fn mixes_streams_and_conceals_loss() {
        let mut enc = dsp::encoder().unwrap();
        let loud = [0.6; FRAME_SAMPLES];
        let mut streams = HashMap::from([(1, remote(10)), (2, remote(20))]);
        for seq in 0..3 {
            let opus = dsp::encode(&mut enc, &loud).unwrap();
            streams.get_mut(&1).unwrap().jitter.push(seq, opus.clone());
            streams.get_mut(&2).unwrap().jitter.push(seq, opus);
        }
        let (mut decoded, mut mix) = (vec![0.0; MAX_DECODE * CHANNELS], vec![0.0; FRAME_SAMPLES * CHANNELS]);
        for _ in 0..3 {
            assert!(mix_frame(&mut streams, &mut decoded, &mut mix));
            assert!(mix.iter().all(|s| (-1.0..=1.0).contains(s)), "clipped");
        }
        assert!(mix_frame(&mut streams, &mut decoded, &mut mix), "an underrun is concealed");
        assert!(streams.values().all(|r| r.decoder.is_some()));

        let mut quiet = HashMap::from([(3, remote(30))]);
        assert!(!mix_frame(&mut quiet, &mut decoded, &mut mix), "nothing buffered: nothing plays");
        assert!(mix.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn level_reports_coarse_steps_and_zero_while_muted() {
        let (ev_tx, ev_rx) = std_mpsc::channel();
        let events = VoiceEvents(crate::backend::EventSink::new(ev_tx, egui::Context::default()));
        let mut e = Engine::new(AudioConfig::default(), Id(1), events, mpsc::channel(1).0);
        let report = |e: &mut Engine, db| (0..LEVEL_FRAMES).for_each(|_| e.report_level(db));
        report(&mut e, -50.0); // 0.5
        report(&mut e, -48.0); // 0.52: below the step
        report(&mut e, -40.0); // 0.6
        e.cfg.muted = true;
        report(&mut e, -10.0);
        report(&mut e, -10.0);
        let levels: Vec<f32> = ev_rx
            .try_iter()
            .filter_map(|ev| if let crate::backend::BackendEvent::Voice(VoiceEvent::LocalLevel(l)) = ev { Some(l) } else { None })
            .collect();
        assert_eq!(levels, [0.5, 0.6, 0.0]);
    }
}
