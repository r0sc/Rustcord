//! Pure audio logic (no devices, no threads): level, voice gate, silence trailer, speaking
//! indicator, jitter buffer, resampling, mixing and the Opus settings.

use super::{CHANNELS, FRAME_SAMPLES, JITTER_PACKETS, MAX_INPUT_VOLUME, SAMPLE_RATE};
use opus_rs::{Application, OpusEncoder};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Silence frames sent after the last voice frame (VOICE_NOTES §2.8).
const TRAILING_SILENCE: u8 = 5;
/// Voice-activity hangover in 20 ms frames (300 ms).
const HANGOVER_FRAMES: u8 = 15;
/// Packets buffered before a stream starts playing (60 ms).
const PREFILL: usize = 3;
/// Frames concealed on an empty buffer before a stream counts as finished (100 ms).
const MAX_LOST: u8 = 5;
/// Speaking turns off this long after the last non-silence packet (VOICE_NOTES §2.9).
const SPEAKING_HOLD: Duration = Duration::from_millis(250);
/// Encoder output buffer (opus-rs caps packets at 1276 bytes).
const MAX_PACKET: usize = 1276;

/// Above this a boosted sample bends toward full scale instead of clipping (-6 dBFS).
const KNEE: f32 = 0.5;

/// Input volume (percent) as a gain: the square of the fraction, so 200 % is +12 dB, 50 % -12 dB, 100 % exactly 1.
pub fn volume_gain(percent: u16) -> f32 {
    let v = f32::from(percent.min(MAX_INPUT_VOLUME)) / 100.0;
    v * v
}

/// Applies the input gain; above 1 a soft limiter (tanh past KNEE) keeps every sample within +-1. Gain 1: untouched.
pub fn apply_volume(samples: &mut [f32], gain: f32) {
    if gain == 1.0 {
        return;
    }
    for s in samples {
        *s *= gain;
        if gain > 1.0 && s.abs() > KNEE {
            *s = s.signum() * (KNEE + (1.0 - KNEE) * ((s.abs() - KNEE) / (1.0 - KNEE)).tanh());
        }
    }
}

/// dBFS of a block of samples, floored at -100.
pub fn db(samples: &[f32]) -> f32 {
    let mean_sq = samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32;
    (10.0 * mean_sq.log10()).max(-100.0)
}

/// dBFS as the 0..=1 level the UI shows: the scale of the -100..=0 dB threshold slider.
pub fn level(db: f32) -> f32 {
    ((db + 100.0) / 100.0).clamp(0.0, 1.0)
}

/// Voice-activity gate (RMS threshold with hangover).
#[derive(Default)]
pub struct Vad {
    hang: u8,
}

impl Vad {
    /// One 20 ms frame: open at or above the threshold and for `HANGOVER_FRAMES` frames after.
    pub fn open(&mut self, db: f32, threshold_db: f32) -> bool {
        if db >= threshold_db {
            self.hang = HANGOVER_FRAMES;
            true
        } else if self.hang > 0 {
            self.hang -= 1;
            true
        } else {
            false
        }
    }
}

/// What the capture side sends for one 20 ms frame.
#[derive(Debug, PartialEq)]
pub enum Tx {
    Voice,
    Silence,
    End,
    Nothing,
}

/// Ends every transmission with the silence frames and `End`.
#[derive(Default)]
pub struct Trailer {
    talking: bool,
    silence_left: u8,
}

impl Trailer {
    /// Between the first voice frame and `End`.
    pub fn talking(&self) -> bool {
        self.talking
    }

    pub fn next(&mut self, open: bool) -> Tx {
        if open {
            (self.talking, self.silence_left) = (true, TRAILING_SILENCE);
            Tx::Voice
        } else if !self.talking {
            Tx::Nothing
        } else if self.silence_left > 0 {
            self.silence_left -= 1;
            Tx::Silence
        } else {
            self.talking = false;
            Tx::End
        }
    }
}

/// Speaking indicator of a remote stream: on with a non-silence packet, off with a silence
/// frame or after `SPEAKING_HOLD` without voice. Holds the time of the last voice packet.
#[derive(Default)]
pub struct Speaking(Option<Instant>);

impl Speaking {
    pub fn is_on(&self) -> bool {
        self.0.is_some()
    }

    /// A packet arrived; `Some(state)` when the indicator changed.
    pub fn packet(&mut self, voice: bool, now: Instant) -> Option<bool> {
        let was = self.0.is_some();
        self.0 = voice.then_some(now);
        (was != voice).then_some(voice)
    }

    /// True when the indicator just timed out.
    pub fn expired(&mut self, now: Instant) -> bool {
        let expired = self.0.is_some_and(|t| now.duration_since(t) > SPEAKING_HOLD);
        if expired {
            self.0 = None;
        }
        expired
    }
}

/// Next 20 ms of a stream.
#[derive(Debug, PartialEq)]
pub enum Pop {
    /// Nothing to play (not started, or finished).
    Idle,
    Frame(Vec<u8>),
    /// Missing: conceal (FEC from `Jitter::fec` when it has the next packet, else PLC).
    Lost,
}

/// Per-SSRC jitter buffer: at most `JITTER_PACKETS` packets ordered by RTP sequence number
/// (u16 wraparound). Starts playing after `PREFILL` packets, stops after `MAX_LOST`
/// concealed frames on an empty buffer.
#[derive(Default)]
pub struct Jitter {
    buf: VecDeque<(u16, Vec<u8>)>,
    /// Sequence number to play next; None while idle.
    next: Option<u16>,
    lost: u8,
}

/// `a` comes before `b` in u16 wraparound order.
fn before(a: u16, b: u16) -> bool {
    a != b && b.wrapping_sub(a) < 0x8000
}

impl Jitter {
    pub fn push(&mut self, seq: u16, opus: Vec<u8>) {
        if self.next.is_some_and(|next| before(seq, next)) {
            return; // late: its slot was played or concealed
        }
        let at = self.buf.iter().position(|&(s, _)| !before(s, seq)).unwrap_or(self.buf.len());
        if self.buf.get(at).is_some_and(|&(s, _)| s == seq) {
            return; // duplicate
        }
        self.buf.insert(at, (seq, opus));
        if self.buf.len() > JITTER_PACKETS
            && let Some((oldest, _)) = self.buf.pop_front()
            && self.next.is_some()
        {
            self.next = Some(oldest.wrapping_add(1));
        }
    }

    pub fn pop(&mut self) -> Pop {
        let next = match (self.next, self.buf.front()) {
            (Some(next), _) => next,
            (None, Some(&(first, _))) if self.buf.len() >= PREFILL => first,
            (None, _) => return Pop::Idle,
        };
        match self.buf.front() {
            Some(&(s, _)) if s == next => {
                (self.next, self.lost) = (Some(next.wrapping_add(1)), 0);
                Pop::Frame(self.buf.pop_front().map(|(_, p)| p).unwrap_or_default())
            }
            // A gap: conceal one frame, then go on with the packets we have.
            Some(&(s, _)) => {
                self.next = Some(s);
                Pop::Lost
            }
            None if self.lost < MAX_LOST => {
                (self.next, self.lost) = (Some(next.wrapping_add(1)), self.lost + 1);
                Pop::Lost
            }
            None => {
                (self.next, self.lost) = (None, 0);
                Pop::Idle
            }
        }
    }

    /// After `Pop::Lost`: the following packet, whose in-band FEC can rebuild the lost frame.
    pub fn fec(&self) -> Option<&[u8]> {
        let next = self.next?;
        self.buf.front().filter(|(s, _)| *s == next).map(|(_, p)| p.as_slice())
    }
}

/// Streaming linear resampler for 1 or 2 interleaved channels.
/// shortcut: linear interpolation (no anti-aliasing filter); fine for voice between 44.1 and
/// 48 kHz, use rubato if devices at other rates sound bad.
pub struct Resampler {
    /// Input frames per output frame.
    step: f64,
    /// Position of the next output frame; 0.0 = `prev`, 1.0 = the first new input frame.
    pos: f64,
    prev: [f32; 2],
    channels: usize,
}

impl Resampler {
    pub fn new(from: u32, to: u32, channels: usize) -> Self {
        Self { step: f64::from(from) / f64::from(to), pos: 0.0, prev: [0.0; 2], channels: channels.clamp(1, 2) }
    }

    /// Appends `input` (interleaved), converted to the target rate, to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let ch = self.channels;
        let n = input.len() / ch;
        if n == 0 {
            return;
        }
        while self.pos < n as f64 {
            let i = self.pos as usize;
            let t = (self.pos - i as f64) as f32;
            for c in 0..ch {
                let a = if i == 0 { self.prev[c] } else { input[(i - 1) * ch + c] };
                out.push(a + (input[i * ch + c] - a) * t);
            }
            self.pos += self.step;
        }
        self.pos -= n as f64;
        self.prev[..ch].copy_from_slice(&input[(n - 1) * ch..n * ch]);
    }
}

/// Stereo frames to `channels` interleaved device channels: mono gets the average, channels
/// past the first two stay silent.
pub fn upmix(stereo: &[f32], channels: usize, out: &mut Vec<f32>) {
    for f in stereo.as_chunks::<2>().0 {
        if channels <= 1 {
            out.push((f[0] + f[1]) * 0.5);
        } else {
            out.extend_from_slice(f);
            out.extend(std::iter::repeat_n(0.0, channels - 2));
        }
    }
}

/// Adds `src` into `mix`.
pub fn mix_into(mix: &mut [f32], src: &[f32]) {
    for (m, s) in mix.iter_mut().zip(src) {
        *m += s;
    }
}

/// Clamps the mix to ±1.0.
pub fn clip(mix: &mut [f32]) {
    for m in mix {
        *m = m.clamp(-1.0, 1.0);
    }
}

/// The send encoder: 48 kHz stereo, VOIP, 64 kbit/s, in-band FEC for ~15 % loss.
pub fn encoder() -> Result<OpusEncoder, &'static str> {
    let mut enc = OpusEncoder::new(SAMPLE_RATE as i32, CHANNELS, Application::Voip)?;
    (enc.bitrate_bps, enc.use_inband_fec, enc.packet_loss_perc) = (64_000, true, 15);
    Ok(enc)
}

/// One 20 ms mono frame as a stereo Opus packet (the mic is duplicated to both channels).
pub fn encode(enc: &mut OpusEncoder, mono: &[f32; FRAME_SAMPLES]) -> Option<Vec<u8>> {
    let mut stereo = [0.0; FRAME_SAMPLES * CHANNELS];
    for (pair, &s) in stereo.as_chunks_mut::<CHANNELS>().0.iter_mut().zip(mono) {
        *pair = [s; CHANNELS];
    }
    let mut out = [0u8; MAX_PACKET];
    let n = enc.encode(&stereo, FRAME_SAMPLES, &mut out).ok()?;
    Some(out[..n].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::audio::SILENCE_FRAME;
    use opus_rs::OpusDecoder;

    #[test]
    fn input_volume_gain() {
        assert_eq!([0, 50, 100, 200, 999].map(volume_gain), [0.0, 0.25, 1.0, 4.0, 4.0]);
        assert!((20.0 * volume_gain(200).log10() - 12.04).abs() < 0.01);
    }

    #[test]
    fn full_volume_is_bit_exact() {
        let src: Vec<f32> = (0..960).map(|i| (i as f32 * 0.37).sin() * 1.3).collect();
        let mut out = src.clone();
        apply_volume(&mut out, volume_gain(100));
        assert_eq!(src.iter().map(|s| s.to_bits()).collect::<Vec<_>>(), out.iter().map(|s| s.to_bits()).collect::<Vec<_>>());
    }

    #[test]
    fn boost_never_exceeds_full_scale() {
        let xs: Vec<f32> = (-2000..=2000).map(|i| i as f32 * 0.001).collect();
        let mut ys = xs.clone();
        apply_volume(&mut ys, 4.0);
        assert!(ys.iter().all(|y| y.abs() <= 1.0));
        assert!(ys.windows(2).all(|w| w[0] <= w[1]));
        for (x, y) in xs.iter().zip(&ys) {
            if (x * 4.0).abs() <= KNEE {
                assert_eq!(*y, x * 4.0);
            }
        }
        let mut k = [(KNEE + 1e-4) / 4.0];
        apply_volume(&mut k, 4.0);
        assert!((k[0] - KNEE).abs() < 1e-3);
        let mut big = [1e6, -1e6, f32::MAX];
        apply_volume(&mut big, 4.0);
        assert!(big.iter().all(|y| y.abs() <= 1.0));
        let mut q = [0.8];
        apply_volume(&mut q, volume_gain(50));
        assert!((q[0] - 0.2).abs() < 1e-6);
    }

    #[test]
    fn level_in_db() {
        assert_eq!(db(&[0.0; 960]), -100.0);
        assert_eq!(db(&[]), -100.0);
        assert!((db(&[1.0, -1.0]) - 0.0).abs() < 1e-4);
        assert!((db(&[0.1; 4]) + 20.0).abs() < 1e-3);
        assert_eq!((level(-100.0), level(-50.0), level(3.0)), (0.0, 0.5, 1.0));
    }

    #[test]
    fn vad_hangover() {
        let mut vad = Vad::default();
        assert!(!vad.open(-60.0, -50.0));
        assert!(vad.open(-40.0, -50.0));
        for _ in 0..HANGOVER_FRAMES {
            assert!(vad.open(-90.0, -50.0), "held open for 300 ms");
        }
        assert!(!vad.open(-90.0, -50.0));
        assert!(vad.open(-50.0, -50.0), "the threshold itself opens");
    }

    #[test]
    fn trailer_sends_five_silence_frames_then_end() {
        let mut t = Trailer::default();
        assert_eq!(t.next(false), Tx::Nothing);
        assert_eq!(t.next(true), Tx::Voice);
        let tail: Vec<Tx> = (0..7).map(|_| t.next(false)).collect();
        assert_eq!(tail, [Tx::Silence, Tx::Silence, Tx::Silence, Tx::Silence, Tx::Silence, Tx::End, Tx::Nothing]);

        assert_eq!(t.next(true), Tx::Voice);
        assert_eq!(t.next(false), Tx::Silence);
        assert_eq!(t.next(true), Tx::Voice, "voice again inside the trailer: no End");
        assert!(t.talking());
    }

    #[test]
    fn speaking_indicator() {
        let t0 = Instant::now();
        let mut s = Speaking::default();
        assert_eq!(s.packet(false, t0), None);
        assert_eq!(s.packet(true, t0), Some(true));
        assert_eq!(s.packet(true, t0 + Duration::from_millis(20)), None);
        assert!(!s.expired(t0 + Duration::from_millis(200)));
        assert!(s.expired(t0 + Duration::from_millis(300)));
        assert!(!s.is_on());
        assert_eq!(s.packet(true, t0), Some(true));
        assert_eq!(s.packet(false, t0), Some(false), "a silence frame ends it");
    }

    fn frame(seq: u16) -> Pop {
        Pop::Frame(vec![seq as u8])
    }

    fn push(j: &mut Jitter, seqs: &[u16]) {
        for &s in seqs {
            j.push(s, vec![s as u8]);
        }
    }

    #[test]
    fn jitter_orders_and_drops() {
        let mut j = Jitter::default();
        push(&mut j, &[2, 1]);
        assert_eq!(j.pop(), Pop::Idle, "prefill");
        push(&mut j, &[3, 3]);
        assert_eq!((j.pop(), j.pop()), (frame(1), frame(2)));
        push(&mut j, &[1, 2]);
        assert_eq!(j.pop(), frame(3), "late packets are dropped, duplicates too");

        // A gap is concealed once, with the next packet available for FEC.
        push(&mut j, &[5, 6]);
        assert_eq!(j.pop(), Pop::Lost);
        assert_eq!(j.fec(), Some(&[5u8][..]));
        assert_eq!((j.pop(), j.pop()), (frame(5), frame(6)));

        // Underrun: concealed up to MAX_LOST frames, then idle until the next prefill.
        for _ in 0..MAX_LOST {
            assert_eq!(j.pop(), Pop::Lost);
            assert_eq!(j.fec(), None);
        }
        assert_eq!(j.pop(), Pop::Idle);
        push(&mut j, &[20, 21, 22]);
        assert_eq!(j.pop(), frame(20));
    }

    #[test]
    fn jitter_is_bounded_and_wraps() {
        let mut j = Jitter::default();
        push(&mut j, &[65533, 65534, 65535, 0, 1, 2, 3]);
        assert_eq!(j.buf.len(), JITTER_PACKETS);
        let played: Vec<Pop> = (0..5).map(|_| j.pop()).collect();
        assert_eq!(played, [frame(65535), frame(0), frame(1), frame(2), frame(3)]);

        // Overflow while playing skips ahead instead of stalling.
        push(&mut j, &[4, 5, 6, 7, 8, 9]);
        assert_eq!(j.pop(), frame(5));
    }

    #[test]
    fn resampler_rates() {
        let mut out = Vec::new();
        let mut same = Resampler::new(48_000, 48_000, 1);
        same.process(&[0.1, 0.2, 0.3], &mut out);
        same.process(&[0.4], &mut out);
        assert_eq!(out, [0.0, 0.1, 0.2, 0.3], "1:1 is exact, one sample late");

        let mut up = Resampler::new(44_100, 48_000, 2);
        out.clear();
        for _ in 0..100 {
            up.process(&[0.5; 882], &mut out); // 10 ms stereo chunks
        }
        assert!((out.len() as i64 / 2 - 48_000).abs() <= 1, "{} frames for 1 s", out.len() / 2);
        assert!(out[4..].iter().all(|&s| (s - 0.5).abs() < 1e-6), "a constant stays constant");
    }

    #[test]
    fn upmix_to_device_channels() {
        let mut out = Vec::new();
        upmix(&[0.2, 0.4], 1, &mut out);
        upmix(&[0.2, 0.4], 2, &mut out);
        upmix(&[0.2, 0.4], 4, &mut out);
        assert_eq!(out, [0.3, 0.2, 0.4, 0.2, 0.4, 0.0, 0.0]);
    }

    #[test]
    fn mixer_clips() {
        let mut mix = [0.0; 4];
        mix_into(&mut mix, &[0.8, -0.7, 0.2, 0.5]);
        mix_into(&mut mix, &[0.8, -0.6, 0.3, -0.5]);
        clip(&mut mix);
        assert_eq!(mix, [1.0, -1.0, 0.5, 0.0]);
    }

    #[test]
    fn opus_round_trip() {
        let mut enc = encoder().unwrap();
        let mut dec = OpusDecoder::new(48_000, 2).unwrap();
        let mut pcm = vec![0.0; 5760 * 2];
        let mut last = 0.0;
        for f in 0..10 {
            let tone: [f32; FRAME_SAMPLES] = std::array::from_fn(|i| {
                0.5 * (2.0 * std::f32::consts::PI * 440.0 * (f * FRAME_SAMPLES + i) as f32 / 48_000.0).sin()
            });
            let packet = encode(&mut enc, &tone).unwrap();
            assert!(packet.len() < 400, "64 kbit/s is ~160 bytes per frame, got {}", packet.len());
            assert_eq!(dec.decode(&packet, 5760, &mut pcm), Ok(FRAME_SAMPLES));
            last = db(&pcm[..FRAME_SAMPLES * 2]);
        }
        assert!((last - db(&[0.5 / std::f32::consts::SQRT_2])).abs() < 3.0, "tone level survives: {last} dB");

        // Loss concealment and the silence frame decode to 20 ms.
        assert_eq!(dec.decode(&[], FRAME_SAMPLES, &mut pcm), Ok(FRAME_SAMPLES));
        for _ in 0..3 {
            assert_eq!(dec.decode(&SILENCE_FRAME, 5760, &mut pcm), Ok(FRAME_SAMPLES));
        }
        assert!(db(&pcm[..FRAME_SAMPLES * 2]) < -60.0);
    }
}
