//! The user's own sound files (Settings > Notifications > Custom sounds): WAV (PCM 8 / 16 / 24 /
//! 32 bit or 32 bit float, also in the extensible form) and Ogg Opus, decoded here with what is
//! already in the tree. The format is read from the content, not the extension. A file over 5 MB
//! is refused, and only its first 10 s play (the last 10 ms fade out). Read and decoded locally.

use std::path::Path;

pub(super) struct Pcm {
    pub rate: u32,
    /// 1 or 2; `samples` are interleaved.
    pub channels: usize,
    pub samples: Vec<f32>,
}

pub(super) const MAX_BYTES: u64 = 5 * 1024 * 1024;
const MAX_SECS: usize = 10;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Fail {
    TooBig,
    Unreadable,
}

pub(super) fn load(path: &Path) -> Result<Pcm, Fail> {
    if std::fs::metadata(path).map_err(|_| Fail::Unreadable)?.len() > MAX_BYTES {
        return Err(Fail::TooBig);
    }
    let bytes = std::fs::read(path).map_err(|_| Fail::Unreadable)?;
    let pcm = if bytes.starts_with(b"OggS") {
        opus(&bytes)
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        wav(&bytes)
    } else {
        None
    };
    let mut pcm = pcm.ok_or(Fail::Unreadable)?;
    cut(&mut pcm);
    Ok(pcm)
}

fn le16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// RIFF chunks: `fmt ` (format 1 PCM, 3 float, 0xFFFE extensible with one of those) and `data`.
/// A chunk running past the end of the file is a broken file. The first two channels are kept.
fn wav(b: &[u8]) -> Option<Pcm> {
    let (mut at, mut fmt, mut data) = (12, None, None);
    while at + 8 <= b.len() {
        let size = le32(b, at + 4)? as usize;
        let body = b.get(at + 8..at + 8 + size)?;
        match &b[at..at + 4] {
            b"fmt " => fmt = Some(body),
            b"data" => data = Some(body),
            _ => {}
        }
        at += 8 + size + (size & 1);
    }
    let (fmt, data) = (fmt?, data?);
    let (channels, rate, bits) = (usize::from(le16(fmt, 2)?), le32(fmt, 4)?, usize::from(le16(fmt, 14)?));
    let tag = match le16(fmt, 0)? {
        0xFFFE => le16(fmt, 24)?,
        t => t,
    };
    let float = tag == 3;
    if !(float && bits == 32 || tag == 1 && matches!(bits, 8 | 16 | 24 | 32)) || channels == 0 || !(8_000..=192_000).contains(&rate) {
        return None;
    }
    let width = bits / 8;
    let used = channels.min(2);
    let sample = |s: &[u8]| match (float, bits) {
        (true, _) => f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
        (_, 8) => (f32::from(s[0]) - 128.0) / 128.0,
        (_, 16) => f32::from(i16::from_le_bytes([s[0], s[1]])) / 32_768.0,
        (_, 24) => (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f32 / 8_388_608.0,
        _ => i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32 / 2_147_483_648.0,
    };
    // One frame more than the cap, so `cut` knows to fade.
    let frames = rate as usize * MAX_SECS + 1;
    let samples: Vec<f32> = data.chunks_exact(width * channels).take(frames).flat_map(|f| f.chunks_exact(width).take(used).map(sample)).collect();
    (!samples.is_empty()).then_some(Pcm { rate, channels: used, samples })
}

/// The Opus packets of the Ogg stream (as the voice message player reads them), 48 kHz stereo,
/// minus the pre-skip.
fn opus(b: &[u8]) -> Option<Pcm> {
    let (packets, _) = crate::plugins::voice_messages::ogg_packets(b).ok()?;
    let head = packets.first().filter(|p| p.starts_with(b"OpusHead") && p.len() >= 19)?;
    let mut skip = usize::from(le16(head, 10)?);
    let mut dec = opus_rs::OpusDecoder::new(48_000, 2).ok()?;
    let (mut samples, mut buf) = (Vec::new(), vec![0f32; 5760 * 2]);
    for p in packets.iter().skip(2) {
        let n = dec.decode(p, 5760, &mut buf).ok()?;
        let start = skip.min(n);
        skip -= start;
        samples.extend_from_slice(&buf[2 * start..2 * n]);
        if samples.len() > 2 * 48_000 * MAX_SECS {
            break;
        }
    }
    (!samples.is_empty()).then_some(Pcm { rate: 48_000, channels: 2, samples })
}

/// At most 10 s; a cut sound fades over its last 10 ms.
fn cut(p: &mut Pcm) {
    let frames = p.rate as usize * MAX_SECS;
    if p.samples.len() <= frames * p.channels {
        return;
    }
    p.samples.truncate(frames * p.channels);
    let fade = p.rate as usize / 100;
    for (i, frame) in p.samples.chunks_exact_mut(p.channels).skip(frames - fade).enumerate() {
        let gain = (fade - 1 - i) as f32 / fade as f32;
        frame.iter_mut().for_each(|s| *s *= gain);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A RIFF/WAVE file: `fmt ` with `tag`, `channels`, `rate`, `bits`, then `data`.
    fn wave(tag: u16, channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
        let mut fmt = Vec::new();
        fmt.extend(tag.to_le_bytes());
        fmt.extend(channels.to_le_bytes());
        fmt.extend(rate.to_le_bytes());
        fmt.extend((rate * u32::from(channels) * u32::from(bits / 8)).to_le_bytes());
        fmt.extend((channels * bits / 8).to_le_bytes());
        fmt.extend(bits.to_le_bytes());
        let mut b = b"RIFF".to_vec();
        b.extend((4 + 8 + fmt.len() + 8 + data.len()).to_le_bytes()[..4].to_vec());
        b.extend(b"WAVEfmt ");
        b.extend((fmt.len() as u32).to_le_bytes());
        b.extend(fmt);
        b.extend(b"data");
        b.extend((data.len() as u32).to_le_bytes());
        b.extend(data);
        b
    }

    #[test]
    fn wav_pcm16_mono_and_float_stereo_decode() {
        let pcm = wav(&wave(1, 1, 44_100, 16, &[0x00, 0x40, 0x00, 0xC0])).unwrap();
        assert_eq!((pcm.rate, pcm.channels), (44_100, 1));
        assert_eq!(pcm.samples, vec![0.5, -0.5]);
        let floats: Vec<u8> = [0.25f32, -0.25, 1.0, 0.0].iter().flat_map(|f| f.to_le_bytes()).collect();
        let pcm = wav(&wave(3, 2, 48_000, 32, &floats)).unwrap();
        assert_eq!((pcm.rate, pcm.channels, pcm.samples.len()), (48_000, 2, 4));
        assert_eq!(&pcm.samples[..2], &[0.25, -0.25]);
        // 24 bit, and more than two channels keep the first two.
        let pcm = wav(&wave(1, 3, 8_000, 24, &[0, 0, 0x40, 1, 2, 3, 4, 5, 6])).unwrap();
        assert_eq!((pcm.channels, pcm.samples.len(), pcm.samples[0]), (2, 2, 0.5));
        assert!(pcm.samples[1] > 0.0);
    }

    #[test]
    fn wav_rejects_other_formats_and_truncated_chunks() {
        assert!(wav(&wave(7, 1, 8_000, 8, &[1, 2, 3])).is_none(), "mu-law");
        assert!(wav(&wave(1, 1, 1_000, 16, &[0, 0])).is_none(), "rate out of range");
        let mut b = wave(1, 1, 44_100, 16, &[0, 1, 2, 3]);
        let n = b.len();
        b[n - 8] = 200; // the data chunk claims more than the file holds
        assert!(wav(&b).is_none());
        assert_eq!(load(Path::new("no such file.wav")).err(), Some(Fail::Unreadable));
    }

    /// Ogg pages of one packet each (packets under 255 bytes), serial 1.
    fn ogg(packets: &[Vec<u8>], last_granule: u64) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, p) in packets.iter().enumerate() {
            assert!(p.len() < 255);
            out.extend(b"OggS");
            out.extend([0, if i == 0 { 2 } else { 0 }]);
            out.extend((if i + 1 == packets.len() { last_granule } else { 0 }).to_le_bytes());
            out.extend(1u32.to_le_bytes());
            out.extend((i as u32).to_le_bytes());
            out.extend([0u8; 4]);
            out.extend([1, p.len() as u8]);
            out.extend(p);
        }
        out
    }

    #[test]
    fn ogg_opus_decodes() {
        let mut head = b"OpusHead".to_vec();
        head.extend([1, 2]);
        head.extend(312u16.to_le_bytes());
        head.extend(48_000u32.to_le_bytes());
        head.extend([0, 0, 0]);
        let mut enc = opus_rs::OpusEncoder::new(48_000, 2, opus_rs::Application::Audio).unwrap();
        let mut packets = vec![head, b"OpusTagsxxxx".to_vec()];
        for f in 0..25 {
            let pcm: Vec<f32> = (0..960).flat_map(|i| [(((f * 960 + i) as f32) * 0.05).sin() * 0.5; 2]).collect();
            let mut out = [0u8; 250];
            let n = enc.encode(&pcm, 960, &mut out).unwrap();
            packets.push(out[..n].to_vec());
        }
        let pcm = opus(&ogg(&packets, 25 * 960)).expect("decodes");
        let frames = pcm.samples.len() / 2;
        assert_eq!((pcm.rate, pcm.channels), (48_000, 2));
        assert!((24_000 - 312 - 960..=24_000).contains(&frames), "{frames}");
        assert!(pcm.samples.iter().any(|s| s.abs() > 0.1), "not silence");
        assert!(opus(b"OggS but nothing else follows here, just padding bytes").is_none());
    }

    #[test]
    fn long_sounds_stop_at_10_s_with_a_fade() {
        let mut pcm = Pcm { rate: 8_000, channels: 1, samples: vec![1.0; 8_000 * 12] };
        cut(&mut pcm);
        assert_eq!(pcm.samples.len(), 80_000);
        assert_eq!(pcm.samples[0], 1.0);
        assert!(pcm.samples[79_999].abs() < 0.02 && pcm.samples[79_919] > 0.5);
        let mut short = Pcm { rate: 8_000, channels: 1, samples: vec![1.0; 800] };
        cut(&mut short);
        assert_eq!(short.samples.last(), Some(&1.0), "short sounds are untouched");
    }

    #[test]
    fn files_over_5_mb_are_refused() {
        let path = std::env::temp_dir().join(format!("rustcord-big-sound-{}.wav", std::process::id()));
        let f = std::fs::File::create(&path).unwrap();
        f.set_len(MAX_BYTES + 1).unwrap();
        drop(f);
        let got = load(&path).err();
        let _ = std::fs::remove_file(&path);
        assert_eq!(got, Some(Fail::TooBig));
    }
}
