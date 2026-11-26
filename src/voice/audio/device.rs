//! cpal side: device lookup by name, stream setup, and the lock-free rings between the cpal
//! callbacks and the audio thread. The callbacks only convert and copy samples: no locks, no
//! allocation.

use super::SAMPLE_RATE;
use super::dsp::{self, Resampler};
use super::{Events, boost_thread};
use crate::voice::VoiceEvent;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, ErrorKind, FromSample, Host, SampleFormat, SizedSample, Stream, StreamConfig};
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};

/// Microphone ring: 200 ms of mono samples at the device rate.
const INPUT_RING_MS: usize = 200;
/// Speaker ring: at most 100 ms; the audio thread refills it while it holds less than 40 ms.
const OUTPUT_RING_MS: usize = 100;
const OUTPUT_TARGET_MS: usize = 40;

/// Lock-free single-producer single-consumer sample ring (a cpal callback on one side, the
/// audio thread on the other). Never allocates after `new`.
pub struct Ring {
    slots: Box<[AtomicU32]>,
    read: AtomicUsize,
    write: AtomicUsize,
}

impl Ring {
    pub fn new(capacity: usize) -> Arc<Self> {
        let slots = (0..capacity.max(1)).map(|_| AtomicU32::new(0)).collect();
        Arc::new(Self { slots, read: AtomicUsize::new(0), write: AtomicUsize::new(0) })
    }

    pub fn len(&self) -> usize {
        let read = self.read.load(Acquire);
        self.write.load(Acquire).wrapping_sub(read).min(self.slots.len())
    }

    /// Producer: appends samples while there is room; the rest is dropped.
    pub fn push(&self, samples: impl IntoIterator<Item = f32>) {
        let write = self.write.load(Relaxed);
        let free = self.slots.len() - write.wrapping_sub(self.read.load(Acquire));
        let mut n = 0;
        for s in samples.into_iter().take(free) {
            self.slots[write.wrapping_add(n) % self.slots.len()].store(s.to_bits(), Relaxed);
            n += 1;
        }
        self.write.store(write.wrapping_add(n), Release);
    }

    /// Consumer: hands up to `max` samples to `f`, oldest first.
    pub fn pop(&self, max: usize, mut f: impl FnMut(f32)) {
        let read = self.read.load(Relaxed);
        let n = self.write.load(Acquire).wrapping_sub(read).min(max);
        for i in 0..n {
            f(f32::from_bits(self.slots[read.wrapping_add(i) % self.slots.len()].load(Relaxed)));
        }
        self.read.store(read.wrapping_add(n), Release);
    }
}

/// A device's name as shown in the settings (None if it vanished).
fn name(d: &Device) -> Option<String> {
    d.description().ok().map(|d| d.name().to_owned())
}

fn names<I: Iterator<Item = Device>>(devices: Result<I, cpal::Error>) -> Vec<String> {
    devices.map(|it| it.filter_map(|d| name(&d)).collect()).unwrap_or_default()
}

/// `(inputs, outputs)` device names.
pub fn list(host: &Host) -> (Vec<String>, Vec<String>) {
    (names(host.input_devices()), names(host.output_devices()))
}

/// The device called `wanted`, else (with a notice) the system default.
fn pick(host: &Host, input: bool, wanted: Option<&str>, events: &Events) -> Option<Device> {
    if let Some(wanted) = wanted {
        let found = host.devices().ok().and_then(|mut all| {
            all.find(|d| name(d).as_deref() == Some(wanted) && if input { d.supports_input() } else { d.supports_output() })
        });
        if found.is_some() {
            return found;
        }
        let what = if input { "Microphone" } else { "Speaker" };
        events.send(VoiceEvent::Error(format!("{what} \"{wanted}\" not found, using the default.")));
    }
    if input { host.default_input_device() } else { host.default_output_device() }
}

/// Stream error callback: reports the first real failure once.
fn on_error(events: Events, what: &'static str) -> impl FnMut(cpal::Error) + Send + 'static {
    let mut reported = false;
    move |e| {
        // Glitches and a reroute to the new default device are not worth a toast.
        if reported || matches!(e.kind(), ErrorKind::Xrun | ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied) {
            return;
        }
        reported = true;
        events.send(VoiceEvent::Error(format!("{what} stopped working: {e}")));
    }
}

/// The open microphone; `read` yields 48 kHz mono.
pub struct Input {
    _stream: Stream,
    ring: Arc<Ring>,
    resampler: Resampler,
    scratch: Vec<f32>,
}

impl Input {
    pub fn open(host: &Host, wanted: Option<&str>, events: &Events) -> Result<Self, String> {
        let failed = |e: cpal::Error| format!("Could not open the microphone: {e}");
        let dev = pick(host, true, wanted, events).ok_or("No microphone found.")?;
        // WASAPI capture only accepts the device's own format: take it and convert here.
        let sc = dev.default_input_config().map_err(failed)?;
        let ring = Ring::new(sc.sample_rate() as usize * INPUT_RING_MS / 1000);
        let (config, r, err) = (sc.config(), ring.clone(), on_error(events.clone(), "Microphone"));
        let stream = match sc.sample_format() {
            SampleFormat::F32 => input_stream::<f32>(&dev, config, r, err),
            SampleFormat::I16 => input_stream::<i16>(&dev, config, r, err),
            SampleFormat::I32 => input_stream::<i32>(&dev, config, r, err),
            f => return Err(format!("The microphone's sample format {f} is not supported.")),
        };
        let stream = stream.and_then(|s| s.play().map(|()| s)).map_err(failed)?;
        let resampler = Resampler::new(sc.sample_rate(), SAMPLE_RATE, 1);
        Ok(Self { _stream: stream, ring, resampler, scratch: Vec::new() })
    }

    /// Appends everything captured since the last call to `out` (48 kHz mono).
    pub fn read(&mut self, out: &mut Vec<f32>) {
        self.scratch.clear();
        self.ring.pop(usize::MAX, |s| self.scratch.push(s));
        self.resampler.process(&self.scratch, out);
    }
}

fn input_stream<T>(
    dev: &Device,
    config: StreamConfig,
    ring: Arc<Ring>,
    err: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let ch = usize::from(config.channels.max(1));
    let mut boosted = false;
    let data = move |data: &[T], _: &_| {
        if !std::mem::replace(&mut boosted, true) {
            boost_thread(); // cpal's own callback thread
        }
        // Downmix to mono; a full ring (stalled audio thread) drops the newest samples.
        ring.push(data.chunks_exact(ch).map(|f| f.iter().map(|&s| f32::from_sample_(s)).sum::<f32>() / ch as f32));
    };
    dev.build_input_stream(config, data, err, None)
}

/// The open speaker; `write` takes 48 kHz stereo.
pub struct Output {
    _stream: Stream,
    ring: Arc<Ring>,
    channels: usize,
    target: usize,
    resampler: Resampler,
    resampled: Vec<f32>,
    frames: Vec<f32>,
}

impl Output {
    pub fn open(host: &Host, wanted: Option<&str>, events: &Events) -> Result<Self, String> {
        let failed = |e: cpal::Error| format!("Could not open the speaker: {e}");
        let dev = pick(host, false, wanted, events).ok_or("No speaker found.")?;
        let sc = dev.default_output_config().map_err(failed)?;
        let channels = usize::from(sc.channels().max(1));
        let per_ms = sc.sample_rate() as usize * channels / 1000;
        let ring = Ring::new(per_ms * OUTPUT_RING_MS);
        let (config, r, err) = (sc.config(), ring.clone(), on_error(events.clone(), "Speaker"));
        let stream = match sc.sample_format() {
            SampleFormat::F32 => output_stream::<f32>(&dev, config, r, err),
            SampleFormat::I16 => output_stream::<i16>(&dev, config, r, err),
            SampleFormat::I32 => output_stream::<i32>(&dev, config, r, err),
            f => return Err(format!("The speaker's sample format {f} is not supported.")),
        };
        let stream = stream.and_then(|s| s.play().map(|()| s)).map_err(failed)?;
        let resampler = Resampler::new(SAMPLE_RATE, sc.sample_rate(), 2);
        let target = per_ms * OUTPUT_TARGET_MS;
        Ok(Self { _stream: stream, ring, channels, target, resampler, resampled: Vec::new(), frames: Vec::new() })
    }

    /// The device took everything queued.
    pub fn is_empty(&self) -> bool {
        self.ring.len() == 0
    }

    /// Time to mix the next 20 ms.
    pub fn wants_more(&self) -> bool {
        self.ring.len() < self.target
    }

    /// Queues 48 kHz stereo audio, converted to the device's rate and channels.
    pub fn write(&mut self, stereo: &[f32]) {
        self.resampled.clear();
        self.resampler.process(stereo, &mut self.resampled);
        self.frames.clear();
        dsp::upmix(&self.resampled, self.channels, &mut self.frames);
        // All or nothing, so the ring always holds whole frames (channels stay aligned).
        if self.ring.len() + self.frames.len() <= self.ring.slots.len() {
            self.ring.push(self.frames.iter().copied());
        }
    }
}

fn output_stream<T>(
    dev: &Device,
    config: StreamConfig,
    ring: Arc<Ring>,
    err: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32>,
{
    let mut boosted = false;
    let data = move |data: &mut [T], _: &_| {
        if !std::mem::replace(&mut boosted, true) {
            boost_thread(); // cpal's own callback thread
        }
        // What the ring lacks stays silent: cpal pre-fills the buffer with silence.
        let n = data.len();
        let mut slots = data.iter_mut();
        ring.pop(n, |s| {
            if let Some(d) = slots.next() {
                *d = T::from_sample_(s);
            }
        });
    };
    dev.build_output_stream(config, data, err, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(ring: &Ring, max: usize) -> Vec<f32> {
        let mut out = Vec::new();
        ring.pop(max, |s| out.push(s));
        out
    }

    #[test]
    fn ring_keeps_order_bounds_and_wraps() {
        let ring = Ring::new(4);
        ring.push([1.0, 2.0, 3.0]);
        assert_eq!(drain(&ring, 2), [1.0, 2.0]);
        ring.push([4.0, 5.0, 6.0, 7.0]);
        assert_eq!(ring.len(), 4, "full: 7.0 was dropped");
        assert_eq!(drain(&ring, 10), [3.0, 4.0, 5.0, 6.0]);
        assert_eq!((ring.len(), drain(&ring, 10)), (0, vec![]));
    }
}
