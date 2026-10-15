//! Speaker output for the codec's DAC samples (feature `audio`, via cpal).
//!
//! The simulation produces 48 kHz stereo frames at whatever pace it runs. They
//! are queued here and played at the sound card's rate with linear resampling.
//! When the simulation runs slower than real time the queue runs dry and the
//! speaker plays silence for the gap; when it runs faster, the oldest frames
//! are dropped so the sound never lags more than [`MAX_LATENCY_FRAMES`].

use std::collections::VecDeque;

/// Sample rate of the frames the simulation produces.
pub const SOURCE_RATE: u32 = 48_000;
/// Most queued frames; older ones are dropped (half a second).
pub const MAX_LATENCY_FRAMES: usize = (SOURCE_RATE / 2) as usize;

/// Interleaved stereo frames waiting to be played.
#[derive(Default)]
pub struct Queue {
    frames: VecDeque<[i16; 2]>,
    /// Frames dropped because the queue was full.
    pub dropped: u64,
}

impl Queue {
    /// Appends interleaved stereo samples, dropping the oldest frames beyond
    /// [`MAX_LATENCY_FRAMES`].
    pub fn push(&mut self, interleaved: &[i16]) {
        for f in interleaved.chunks_exact(2) {
            self.frames.push_back([f[0], f[1]]);
        }
        while self.frames.len() > MAX_LATENCY_FRAMES {
            self.frames.pop_front();
            self.dropped += 1;
        }
    }

    /// Queued frames.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    fn pop(&mut self) -> Option<[i16; 2]> {
        self.frames.pop_front()
    }
}

/// Converts the queue to the device's rate and channel count.
pub struct Resampler {
    /// Source frames consumed per output frame.
    step: f64,
    phase: f64,
    cur: [f32; 2],
    next: [f32; 2],
    primed: bool,
}

impl Resampler {
    /// A resampler producing `device_rate` frames per second.
    pub fn new(device_rate: u32) -> Resampler {
        Resampler {
            step: f64::from(SOURCE_RATE) / f64::from(device_rate.max(1)),
            phase: 0.0,
            cur: [0.0; 2],
            next: [0.0; 2],
            primed: false,
        }
    }

    /// Fills `out` (`channels` interleaved samples per frame) from `queue`.
    /// Any channel beyond the second repeats the left channel; when the queue
    /// runs dry the output is silence.
    pub fn fill(&mut self, queue: &mut Queue, out: &mut [f32], channels: usize) {
        let channels = channels.max(1);
        let to_f = |f: [i16; 2]| [f32::from(f[0]) / 32768.0, f32::from(f[1]) / 32768.0];
        for frame in out.chunks_mut(channels) {
            if !self.primed {
                match (queue.pop(), queue.pop()) {
                    (Some(a), Some(b)) => {
                        self.cur = to_f(a);
                        self.next = to_f(b);
                        self.primed = true;
                    }
                    (Some(a), None) => {
                        // Not enough yet: put the single frame back in front.
                        queue.frames.push_front(a);
                    }
                    _ => {}
                }
            }
            if !self.primed {
                frame.fill(0.0);
                continue;
            }
            let t = self.phase as f32;
            let l = self.cur[0] + (self.next[0] - self.cur[0]) * t;
            let r = self.cur[1] + (self.next[1] - self.cur[1]) * t;
            for (c, s) in frame.iter_mut().enumerate() {
                *s = match c {
                    1 => r,
                    0 => l,
                    _ => l,
                };
            }
            self.phase += self.step;
            while self.phase >= 1.0 {
                self.phase -= 1.0;
                self.cur = self.next;
                match queue.pop() {
                    Some(f) => self.next = to_f(f),
                    None => {
                        // Underrun: hold the last value and re-prime later.
                        self.next = self.cur;
                        self.primed = false;
                        self.phase = 0.0;
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(feature = "audio")]
pub use device::Speaker;

#[cfg(feature = "audio")]
mod device {
    use super::{Queue, Resampler};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, SampleFormat, SizedSample};
    use std::sync::{Arc, Mutex};

    /// An open output stream on the default sound device.
    pub struct Speaker {
        _stream: cpal::Stream,
        queue: Arc<Mutex<Queue>>,
        /// Human-readable device name.
        pub name: String,
        /// The device's sample rate.
        pub rate: u32,
    }

    impl Speaker {
        /// Opens the default output device and starts playing silence.
        pub fn open() -> Result<Speaker, String> {
            let host = cpal::default_host();
            let device = host
                .default_output_device()
                .ok_or_else(|| "no sound output device".to_string())?;
            let cfg = device
                .default_output_config()
                .map_err(|e| format!("sound device: {e}"))?;
            let name = device
                .description()
                .map(|d| d.to_string())
                .unwrap_or_else(|_| "default output".to_string());
            let queue = Arc::new(Mutex::new(Queue::default()));
            let rate = cfg.sample_rate();
            let stream = match cfg.sample_format() {
                SampleFormat::F32 => build::<f32>(&device, cfg.into(), &queue),
                SampleFormat::I16 => build::<i16>(&device, cfg.into(), &queue),
                SampleFormat::I32 => build::<i32>(&device, cfg.into(), &queue),
                SampleFormat::U16 => build::<u16>(&device, cfg.into(), &queue),
                other => Err(format!("unsupported sample format {other}")),
            }?;
            stream.play().map_err(|e| format!("sound device: {e}"))?;
            Ok(Speaker {
                _stream: stream,
                queue,
                name,
                rate,
            })
        }

        /// Queues interleaved stereo 48 kHz samples for playback.
        pub fn push(&self, interleaved: &[i16]) {
            if let Ok(mut q) = self.queue.lock() {
                q.push(interleaved);
            }
        }

        /// Frames waiting to be played.
        pub fn queued(&self) -> usize {
            self.queue.lock().map_or(0, |q| q.len())
        }

        /// Frames dropped to stay within the latency limit.
        pub fn dropped(&self) -> u64 {
            self.queue.lock().map_or(0, |q| q.dropped)
        }
    }

    fn build<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        queue: &Arc<Mutex<Queue>>,
    ) -> Result<cpal::Stream, String>
    where
        T: SizedSample + FromSample<f32>,
    {
        let channels = usize::from(config.channels);
        let mut rs = Resampler::new(config.sample_rate);
        let q = Arc::clone(queue);
        let mut scratch: Vec<f32> = Vec::new();
        device
            .build_output_stream(
                config,
                move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                    scratch.resize(data.len(), 0.0);
                    match q.lock() {
                        Ok(mut q) => rs.fill(&mut q, &mut scratch, channels),
                        Err(_) => scratch.fill(0.0),
                    }
                    for (o, s) in data.iter_mut().zip(&scratch) {
                        *o = T::from_sample(*s);
                    }
                },
                |e| eprintln!("zynqsim-gui: sound stream: {e}"),
                None,
            )
            .map_err(|e| format!("sound device: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f64, frames: usize) -> Vec<i16> {
        (0..frames)
            .flat_map(|i| {
                let v = ((2.0 * std::f64::consts::PI * hz * i as f64 / 48_000.0).sin() * 12_000.0)
                    as i16;
                [v, v]
            })
            .collect()
    }

    fn crossings(left: &[f32]) -> usize {
        left.windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count()
    }

    #[test]
    fn the_queue_caps_latency_by_dropping_the_oldest() {
        let mut q = Queue::default();
        q.push(&vec![1; 2 * (MAX_LATENCY_FRAMES + 100)]);
        assert_eq!(q.len(), MAX_LATENCY_FRAMES);
        assert_eq!(q.dropped, 100);
    }

    #[test]
    fn resampling_to_44100_keeps_the_pitch() {
        let mut q = Queue::default();
        q.push(&sine(1000.0, 19_200)); // 0.4 s, within the latency cap
        let mut rs = Resampler::new(44_100);
        let mut out = vec![0.0f32; 17_000 * 2]; // 0.385 s of output
        rs.fill(&mut q, &mut out, 2);
        let left: Vec<f32> = out.chunks(2).map(|f| f[0]).collect();
        let c = crossings(&left);
        let want = 17_000.0 / 44_100.0 * 1000.0; // cycles of 1 kHz in that time
        assert!(
            (c as f64 - want).abs() <= 2.0,
            "{c} cycles, expected about {want:.0}"
        );
        // Left and right match for a mono source.
        assert!(out.chunks(2).all(|f| f[0] == f[1]));
    }

    #[test]
    fn same_rate_is_a_straight_copy() {
        let src = sine(440.0, 960);
        let mut q = Queue::default();
        q.push(&src);
        let mut rs = Resampler::new(SOURCE_RATE);
        let mut out = vec![0.0f32; 960 * 2];
        rs.fill(&mut q, &mut out, 2);
        // The last frame has no successor to interpolate towards: skip it.
        for (i, f) in out.chunks(2).enumerate().take(958) {
            let want = f32::from(src[2 * i]) / 32768.0;
            assert!((f[0] - want).abs() < 1e-6, "frame {i}: {} vs {want}", f[0]);
        }
    }

    #[test]
    fn an_empty_queue_plays_silence_and_recovers() {
        let mut q = Queue::default();
        let mut rs = Resampler::new(SOURCE_RATE);
        let mut out = vec![1.0f32; 64];
        rs.fill(&mut q, &mut out, 2);
        assert!(out.iter().all(|&s| s == 0.0));
        q.push(&sine(1000.0, 200));
        rs.fill(&mut q, &mut out, 2);
        assert!(out.iter().any(|&s| s != 0.0), "audio resumes");
    }

    #[test]
    fn extra_channels_repeat_the_left_channel() {
        let mut q = Queue::default();
        q.push(&[1000, -1000, 2000, -2000, 3000, -3000]);
        let mut rs = Resampler::new(SOURCE_RATE);
        let mut out = vec![0.0f32; 4 * 2];
        rs.fill(&mut q, &mut out, 4);
        let f = &out[4..8];
        assert_eq!(f[0], f[2]);
        assert_eq!(f[0], f[3]);
        assert!(f[1] < 0.0 && f[0] > 0.0);
    }
}
