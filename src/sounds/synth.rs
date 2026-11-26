//! The recipes of Rustcord's sounds (docs/DESIGN.md 7.1): additive sine partials with an eased
//! attack and an exponential decay, mono at 48 kHz, rendered on demand in a few milliseconds.
//! Bell for chat, rings and streams; knock (bell plus a soft hammer tick) for voice presence;
//! click for switches. Pitches are D major pentatonic.

use super::Sound;
use std::f32::consts::{PI, TAU};

pub(super) const RATE: u32 = 48_000;

/// (ratio, amplitude, decay multiplier) per partial.
type Partial = (f32, f32, f32);

struct Timbre {
    partials: &'static [Partial],
    attack_ms: f32,
    tick: f32,
}

const BELL: Timbre = Timbre { partials: &[(1.0, 1.0, 1.0), (2.0, 0.30, 0.6), (2.76, 0.18, 0.45), (5.40, 0.05, 0.25)], attack_ms: 3.0, tick: 0.0 };
const KNOCK: Timbre = Timbre { partials: &[(1.0, 1.0, 1.0), (2.0, 0.45, 0.7), (3.0, 0.20, 0.5), (4.16, 0.10, 0.3)], attack_ms: 2.0, tick: 0.25 };
const CLICK: Timbre = Timbre { partials: &[(1.0, 1.0, 1.0), (2.0, 0.25, 0.5)], attack_ms: 1.5, tick: 0.0 };

const D4: f32 = 293.66;
const FS4: f32 = 369.99;
const A4: f32 = 440.0;
const B4: f32 = 493.88;
const D5: f32 = 587.33;
const E5: f32 = 659.26;
const FS5: f32 = 739.99;
const A5: f32 = 880.0;
const B5: f32 = 987.77;
const D6: f32 = 1174.66;
const E6: f32 = 1318.51;
const FS6: f32 = 1479.98;

/// A note: onset ms, pitch, glide target (same pitch: none), decay time constant ms, amplitude.
struct Note(f32, f32, f32, f32, f32);

const fn n(at: f32, hz: f32, tau: f32, amp: f32) -> Note {
    Note(at, hz, hz, tau, amp)
}

struct Recipe {
    timbre: &'static Timbre,
    notes: Vec<Note>,
    /// Loop length for the rings.
    cycle_ms: Option<f32>,
    peak_db: f32,
}

fn recipe(sound: Sound) -> Recipe {
    let (timbre, notes, cycle_ms, peak_db) = match sound {
        Sound::Message => (&BELL, vec![n(0.0, B5, 60.0, 0.8), n(55.0, E6, 140.0, 1.0)], None, -8.0),
        Sound::Mention => (&BELL, vec![n(0.0, B5, 50.0, 0.7), n(50.0, E6, 50.0, 0.8), n(100.0, FS6, 180.0, 1.0)], None, -6.0),
        Sound::IncomingRing => (
            &BELL,
            vec![n(0.0, D5, 200.0, 0.8), n(110.0, FS5, 200.0, 0.8), n(220.0, A5, 200.0, 0.9), n(330.0, D6, 450.0, 1.0)],
            Some(2200.0),
            -4.0,
        ),
        Sound::OutgoingRing => (
            &KNOCK,
            vec![n(0.0, A4, 350.0, 0.6), n(0.0, D5, 350.0, 0.6), n(450.0, A4, 600.0, 0.6), n(450.0, D5, 600.0, 0.6)],
            Some(3000.0),
            -10.0,
        ),
        Sound::Join => (&KNOCK, vec![n(0.0, D5, 80.0, 0.8), n(85.0, A5, 160.0, 1.0)], None, -8.0),
        Sound::Leave => (&KNOCK, vec![n(0.0, A5, 60.0, 0.9), n(60.0, D5, 100.0, 1.0)], None, -10.0),
        Sound::Disconnect => (&KNOCK, vec![n(0.0, A4, 80.0, 0.8), n(70.0, FS4, 80.0, 0.8), n(140.0, D4, 260.0, 1.0)], None, -8.0),
        Sound::Mute => (&CLICK, vec![Note(0.0, A5, D5, 35.0, 1.0)], None, -12.0),
        Sound::Unmute => (&CLICK, vec![Note(0.0, D5, A5, 45.0, 1.0)], None, -12.0),
        Sound::Deafen => (&CLICK, vec![n(0.0, E5, 40.0, 0.9), n(45.0, B4, 70.0, 1.0)], None, -10.0),
        Sound::Undeafen => (&CLICK, vec![n(0.0, B4, 40.0, 0.9), n(45.0, E5, 90.0, 1.0)], None, -10.0),
        Sound::PttOn => (&CLICK, vec![n(0.0, D6, 14.0, 1.0)], None, -15.0),
        Sound::PttOff => (&CLICK, vec![n(0.0, A5, 10.0, 1.0)], None, -17.0),
        Sound::StreamStart => {
            (&BELL, vec![n(0.0, E5, 45.0, 0.7), n(35.0, A5, 45.0, 0.75), n(70.0, B5, 45.0, 0.8), n(105.0, E6, 260.0, 1.0)], None, -8.0)
        }
        Sound::StreamEnd => {
            (&BELL, vec![n(0.0, E6, 40.0, 0.8), n(30.0, B5, 40.0, 0.8), n(60.0, A5, 40.0, 0.8), n(90.0, E5, 150.0, 1.0)], None, -10.0)
        }
    };
    Recipe { timbre, notes, cycle_ms, peak_db }
}

/// Loop length of a ring in ms.
pub(super) fn cycle_ms(sound: Sound) -> Option<f32> {
    recipe(sound).cycle_ms
}

fn ms(x: f32) -> usize {
    (x * RATE as f32 / 1000.0).round() as usize
}

/// Deterministic mono PCM of `sound`, normalized to its peak level, last 10 ms faded out.
pub(super) fn render(sound: Sound) -> Vec<f32> {
    let r = recipe(sound);
    let len_ms = r.cycle_ms.unwrap_or_else(|| r.notes.iter().map(|x| x.0 + 4.0 * x.3).fold(0.0, f32::max) + 10.0);
    let mut buf = vec![0.0f32; ms(len_ms)];
    let rate = RATE as f32;
    let attack = r.timbre.attack_ms * rate / 1000.0;
    let ramp = |i: f32| if i < attack { 0.5 - 0.5 * (PI * i / attack).cos() } else { 1.0 };
    let mut rng = 0x2545_F491u32;
    for note in &r.notes {
        let start = ms(note.0);
        let count = ms(4.0 * note.3 + 10.0).min(buf.len().saturating_sub(start));
        let glide = if note.1 == note.2 { 1.0 } else { ms(50.0) as f32 };
        for &(ratio, amp, decay) in r.timbre.partials {
            let tau = note.3 * decay * rate / 1000.0;
            let mut phase = 0.0f32;
            for i in 0..count {
                let t = i as f32;
                let f = note.1 * (note.2 / note.1).powf((t / glide).min(1.0)) * ratio;
                if f >= 0.45 * rate {
                    break;
                }
                phase += TAU * f / rate;
                buf[start + i] += note.4 * amp * ramp(t) * (-t / tau).exp() * phase.sin();
            }
        }
        if r.timbre.tick > 0.0 {
            let mut y = 0.0f32;
            for i in 0..ms(8.0).min(count) {
                rng ^= rng << 13;
                rng ^= rng >> 17;
                rng ^= rng << 5;
                let x = (rng as f32 / u32::MAX as f32) * 2.0 - 1.0;
                y += 0.35 * (x - y);
                buf[start + i] += r.timbre.tick * note.4 * ramp(i as f32) * (-(i as f32) / (0.002 * rate)).exp() * y;
            }
        }
    }
    let peak = buf.iter().fold(0.0f32, |m, x| m.max(x.abs())).max(1e-9);
    let k = 10f32.powf(r.peak_db / 20.0) / peak;
    buf.iter_mut().for_each(|x| *x *= k);
    let fade = ms(10.0).min(buf.len());
    let at = buf.len() - fade;
    for (i, x) in buf[at..].iter_mut().enumerate() {
        *x *= 1.0 - (i + 1) as f32 / fade as f32;
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audible_ms(s: Sound) -> f32 {
        let pcm = render(s);
        let peak = pcm.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        pcm.iter().rposition(|x| x.abs() > peak * 0.0316).unwrap_or(0) as f32 * 1000.0 / RATE as f32
    }

    #[test]
    fn clean_and_at_level() {
        for s in Sound::ALL {
            let pcm = render(s);
            assert!(pcm.iter().all(|x| x.is_finite()), "{s:?}");
            let peak_db = 20.0 * pcm.iter().fold(0.0f32, |m, x| m.max(x.abs())).log10();
            assert!((peak_db - recipe(s).peak_db).abs() < 0.1, "{s:?} {peak_db}");
            assert!(pcm[0].abs() < 1e-3 && pcm[pcm.len() - 1].abs() < 1e-3, "{s:?} edges");
            let len_ms = pcm.len() as f32 * 1000.0 / RATE as f32;
            match (cycle_ms(s), s) {
                (Some(c), _) => assert_eq!(pcm.len(), (c * 48.0) as usize, "{s:?}"),
                (None, Sound::PttOn | Sound::PttOff) => assert!(len_ms <= 80.0, "{s:?} {len_ms}"),
                (None, _) => assert!(len_ms <= 1200.0, "{s:?} {len_ms}"),
            }
        }
    }

    #[test]
    fn exits_are_shorter() {
        for (out, on) in [
            (Sound::Leave, Sound::Join),
            (Sound::StreamEnd, Sound::StreamStart),
            (Sound::Mute, Sound::Unmute),
            (Sound::Deafen, Sound::Undeafen),
            (Sound::PttOff, Sound::PttOn),
        ] {
            assert!(audible_ms(out) < audible_ms(on), "{out:?} {} vs {on:?} {}", audible_ms(out), audible_ms(on));
        }
    }

    /// For listening: `cargo test export_wavs -- --ignored`.
    #[test]
    #[ignore]
    fn export_wavs() {
        let dir = std::env::var_os("RUSTCORD_SOUND_OUT").map_or_else(|| std::env::temp_dir().join("rustcord-sounds"), Into::into);
        std::fs::create_dir_all(&dir).unwrap();
        for s in Sound::ALL {
            let pcm: Vec<u8> = render(s).iter().flat_map(|x| ((x * 32767.0) as i16).to_le_bytes()).collect();
            let mut w = Vec::new();
            w.extend(b"RIFF");
            w.extend((36 + pcm.len() as u32).to_le_bytes());
            w.extend(b"WAVEfmt ");
            w.extend(16u32.to_le_bytes());
            w.extend([1, 0, 1, 0]);
            w.extend(RATE.to_le_bytes());
            w.extend((RATE * 2).to_le_bytes());
            w.extend([2, 0, 16, 0]);
            w.extend(b"data");
            w.extend((pcm.len() as u32).to_le_bytes());
            w.extend(pcm);
            std::fs::write(dir.join(format!("{}.wav", s.key())), w).unwrap();
        }
    }
}
