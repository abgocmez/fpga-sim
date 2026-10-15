//! Host-side media: what the board's audio codec and HDMI connectors exchange
//! with the outside world.
//!
//! The RTL testbench contains small pin-level models (an SSM2603-style I2S/I2C
//! codec, an HDMI sink and source). They exchange *samples* and *lines* with
//! the host, not pins, so the host sees 48 kHz PCM frames and RGB lines rather
//! than millions of pin toggles per second.

use std::f64::consts::PI;

/// Remote-port device number: video lines going out of the PL (HDMI TX).
pub const DEV_VIDEO_OUT: u32 = 14;
/// Audio frames going out of the PL (codec DAC).
pub const DEV_AUDIO_OUT: u32 = 15;
/// Video lines requested by the PL (HDMI RX).
pub const DEV_VIDEO_IN: u32 = 16;
/// Audio frames requested by the PL (codec ADC).
pub const DEV_AUDIO_IN: u32 = 17;
/// Codec register writes reported by the testbench (`line` = register,
/// `vector` = 9-bit value).
pub const DEV_CODEC_REG: u32 = 18;

/// Audio sample rate of the board's codec path.
pub const SAMPLE_RATE: u32 = 48_000;

/// Something the PL's media models produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaEvent {
    /// One active video line, `rgb.len() == 3 * width`.
    VideoLine {
        /// Frame number.
        frame: u32,
        /// Line number within the frame.
        y: u32,
        /// PL time of the line in nanoseconds.
        t_ns: u64,
        /// R, G, B bytes.
        rgb: Vec<u8>,
    },
    /// The vertical sync of a frame: the frame is complete.
    VideoFrameEnd {
        /// Frame number.
        frame: u32,
        /// PL time in nanoseconds.
        t_ns: u64,
    },
    /// Stereo PCM from the codec DAC.
    Audio {
        /// Index of the first frame since the start of the run.
        first_frame: u64,
        /// PL time of the first frame in nanoseconds.
        t_ns: u64,
        /// Interleaved left/right signed 16-bit samples.
        samples: Vec<i16>,
    },
    /// A write to a codec register over I2C.
    CodecRegister {
        /// PL time in nanoseconds.
        t_ns: u64,
        /// Register number (0..=15).
        reg: u8,
        /// 9-bit register value.
        value: u16,
    },
}

/// A still image the HDMI RX connector presents to the PL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoSource {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `3 * width * height` bytes, row-major R, G, B.
    pub rgb: Vec<u8>,
}

impl VideoSource {
    /// Eight vertical colour bars (white, yellow, cyan, green, magenta, red,
    /// blue, black) over a horizontal grey ramp in the bottom quarter.
    pub fn color_bars(width: u32, height: u32) -> VideoSource {
        const BARS: [[u8; 3]; 8] = [
            [255, 255, 255],
            [255, 255, 0],
            [0, 255, 255],
            [0, 255, 0],
            [255, 0, 255],
            [255, 0, 0],
            [0, 0, 255],
            [0, 0, 0],
        ];
        let mut rgb = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                if y >= height * 3 / 4 {
                    let g = (x * 255 / width.max(1)) as u8;
                    rgb.extend_from_slice(&[g, g, g]);
                } else {
                    rgb.extend_from_slice(&BARS[(x * 8 / width.max(1)) as usize % 8]);
                }
            }
        }
        VideoSource { width, height, rgb }
    }

    /// A gradient: red grows with x, green with y, blue is the diagonal.
    pub fn gradient(width: u32, height: u32) -> VideoSource {
        let mut rgb = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                rgb.push((x * 255 / width.max(1)) as u8);
                rgb.push((y * 255 / height.max(1)) as u8);
                rgb.push(((x + y) * 255 / (width + height).max(1)) as u8);
            }
        }
        VideoSource { width, height, rgb }
    }

    /// One line of `w` pixels for output line `y` of a `h`-line frame, by
    /// nearest-neighbour scaling of the source. Returns `3 * w` bytes.
    pub fn line(&self, y: u32, w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity((w * 3) as usize);
        if self.width == 0 || self.height == 0 || w == 0 || h == 0 {
            out.resize((w * 3) as usize, 0);
            return out;
        }
        let sy = (u64::from(y.min(h - 1)) * u64::from(self.height) / u64::from(h)) as u32;
        for x in 0..w {
            let sx = (u64::from(x) * u64::from(self.width) / u64::from(w)) as u32;
            let i = ((sy * self.width + sx) * 3) as usize;
            out.extend_from_slice(&self.rgb[i..i + 3]);
        }
        out
    }
}

/// Where the codec ADC (LINE IN) gets its audio.
#[derive(Clone, Debug, PartialEq)]
pub enum AudioSource {
    /// Digital silence.
    Silence,
    /// A sine tone, the same on both channels.
    Tone {
        /// Frequency in hertz.
        freq_hz: f64,
        /// Peak amplitude, 0.0 to 1.0.
        amplitude: f64,
    },
    /// Interleaved stereo PCM at [`SAMPLE_RATE`], looped.
    Pcm(Vec<i16>),
}

impl AudioSource {
    /// Stereo frames `start .. start + n` as interleaved samples. A pure
    /// function of the frame index, so a run is reproducible whatever the host
    /// timing.
    pub fn block(&self, start: u64, n: usize) -> Vec<i16> {
        let mut out = Vec::with_capacity(n * 2);
        match self {
            AudioSource::Silence => out.resize(n * 2, 0),
            AudioSource::Tone { freq_hz, amplitude } => {
                for i in 0..n as u64 {
                    let t = (start + i) as f64 / f64::from(SAMPLE_RATE);
                    let v = (amplitude.clamp(0.0, 1.0)
                        * (2.0 * PI * freq_hz * t).sin()
                        * f64::from(i16::MAX))
                    .round() as i16;
                    out.push(v);
                    out.push(v);
                }
            }
            AudioSource::Pcm(p) => {
                let frames = p.len() / 2;
                if frames == 0 {
                    out.resize(n * 2, 0);
                } else {
                    for i in 0..n as u64 {
                        let f = ((start + i) % frames as u64) as usize;
                        out.push(p[2 * f]);
                        out.push(p[2 * f + 1]);
                    }
                }
            }
        }
        out
    }
}

/// Converts interleaved stereo PCM from `from_rate` to [`SAMPLE_RATE`] with
/// linear interpolation. Returns the input unchanged when the rates match.
pub fn resample_stereo(samples: &[i16], from_rate: u32) -> Vec<i16> {
    if from_rate == SAMPLE_RATE || from_rate == 0 || samples.len() < 4 {
        return samples.to_vec();
    }
    let frames = samples.len() / 2;
    let out_frames = (frames as u64 * u64::from(SAMPLE_RATE) / u64::from(from_rate)) as usize;
    let mut out = Vec::with_capacity(out_frames * 2);
    for i in 0..out_frames {
        let pos = i as f64 * f64::from(from_rate) / f64::from(SAMPLE_RATE);
        let i0 = (pos.floor() as usize).min(frames - 1);
        let i1 = (i0 + 1).min(frames - 1);
        let t = pos - pos.floor();
        for ch in 0..2 {
            let a = f64::from(samples[2 * i0 + ch]);
            let b = f64::from(samples[2 * i1 + ch]);
            out.push((a + (b - a) * t).round() as i16);
        }
    }
    out
}

/// Encodes interleaved stereo 16-bit PCM as a WAV file.
pub fn wav_bytes(samples: &[i16], sample_rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + samples.len() * 2);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&2u16.to_le_bytes()); // channels
    w.extend_from_slice(&sample_rate.to_le_bytes());
    w.extend_from_slice(&(sample_rate * 4).to_le_bytes()); // byte rate
    w.extend_from_slice(&4u16.to_le_bytes()); // block align
    w.extend_from_slice(&16u16.to_le_bytes()); // bits
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
}

/// Decodes a 16-bit PCM WAV (mono or stereo, any rate) into interleaved
/// stereo samples and the file's sample rate. Mono is duplicated.
pub fn parse_wav(bytes: &[u8]) -> Result<(Vec<i16>, u32), String> {
    if bytes.len() < 44 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".to_string());
    }
    let mut pos = 12;
    let (mut channels, mut rate, mut bits) = (0u16, 0u32, 0u16);
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body = pos + 8;
        let end = body.saturating_add(len).min(bytes.len());
        match id {
            b"fmt " if end - body >= 16 => {
                let f = &bytes[body..end];
                let format = u16::from_le_bytes([f[0], f[1]]);
                if format != 1 {
                    return Err(format!("WAV format {format} is not PCM"));
                }
                channels = u16::from_le_bytes([f[2], f[3]]);
                rate = u32::from_le_bytes([f[4], f[5], f[6], f[7]]);
                bits = u16::from_le_bytes([f[14], f[15]]);
            }
            b"data" => data = Some(&bytes[body..end]),
            _ => {}
        }
        pos = body + len + (len & 1);
    }
    if bits != 16 || !(1..=2).contains(&channels) || rate == 0 {
        return Err(format!(
            "unsupported WAV: {channels} channel(s), {bits}-bit, {rate} Hz (need 16-bit mono/stereo)"
        ));
    }
    let data = data.ok_or("WAV has no data chunk")?;
    let raw: Vec<i16> = data
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    let stereo = if channels == 1 {
        raw.iter().flat_map(|&s| [s, s]).collect()
    } else {
        raw
    };
    Ok((stereo, rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_is_a_pure_function_of_the_frame_index() {
        let t = AudioSource::Tone {
            freq_hz: 1000.0,
            amplitude: 0.5,
        };
        let a = t.block(100, 32);
        let b = t.block(100, 32);
        assert_eq!(a, b);
        let whole = t.block(96, 40);
        assert_eq!(&whole[8..8 + 64], &a[..]);
        // 1 kHz at 48 kHz: 48 frames per period; left == right.
        assert!(a.chunks(2).all(|f| f[0] == f[1]));
        let peak = t.block(0, 48).iter().map(|s| s.abs()).max().unwrap();
        assert!((peak as f64 - 0.5 * 32767.0).abs() < 600.0, "peak {peak}");
    }

    #[test]
    fn pcm_source_loops_and_silence_is_zero() {
        let p = AudioSource::Pcm(vec![1, 2, 3, 4]);
        assert_eq!(p.block(1, 3), vec![3, 4, 1, 2, 3, 4]);
        assert!(AudioSource::Silence.block(5, 4).iter().all(|&s| s == 0));
        assert_eq!(AudioSource::Pcm(vec![]).block(0, 2), vec![0; 4]);
    }

    #[test]
    fn wav_round_trip_and_mono_expansion() {
        let samples: Vec<i16> = (0..200).map(|i| (i * 100 - 10000) as i16).collect();
        let w = wav_bytes(&samples, 44_100);
        let (back, rate) = parse_wav(&w).unwrap();
        assert_eq!((back, rate), (samples, 44_100));

        // Hand-built mono file.
        let mono = [5i16, -5, 7];
        let mut f = wav_bytes(&[0; 0], 8000);
        f[22] = 1; // channels = 1
        f.truncate(36);
        f.extend_from_slice(b"data");
        f.extend_from_slice(&6u32.to_le_bytes());
        for s in mono {
            f.extend_from_slice(&s.to_le_bytes());
        }
        let (st, r) = parse_wav(&f).unwrap();
        assert_eq!((st, r), (vec![5, 5, -5, -5, 7, 7], 8000));
        assert!(parse_wav(b"junk").is_err());
    }

    #[test]
    fn resampling_keeps_duration_and_frequency() {
        // 1 s of 1 kHz at 24 kHz -> 48 kHz: twice the frames, same tone.
        let src: Vec<i16> = (0..24_000)
            .flat_map(|i| {
                let v = ((2.0 * PI * 1000.0 * f64::from(i) / 24_000.0).sin() * 10_000.0) as i16;
                [v, v]
            })
            .collect();
        let out = resample_stereo(&src, 24_000);
        assert_eq!(out.len(), 96_000);
        let rising = out
            .chunks(2)
            .map(|f| f[0])
            .collect::<Vec<_>>()
            .windows(2)
            .filter(|w| w[0] < 0 && w[1] >= 0)
            .count();
        assert!((999..=1001).contains(&rising), "{rising} cycles in 1 s");
        assert_eq!(resample_stereo(&src[..8], 48_000), src[..8].to_vec());
    }

    #[test]
    fn color_bars_and_scaling() {
        let v = VideoSource::color_bars(64, 48);
        assert_eq!(&v.line(0, 64, 48)[..3], &[255, 255, 255]); // white bar
        assert_eq!(&v.line(0, 64, 48)[(63 * 3)..], &[0, 0, 0]); // black bar
        // Bottom quarter is the ramp: left black, right near white.
        let ramp = v.line(47, 64, 48);
        assert_eq!(&ramp[..3], &[0, 0, 0]);
        assert!(ramp[63 * 3] > 240);
        // Nearest-neighbour upscale of a 2x1 image to 4 pixels.
        let two = VideoSource {
            width: 2,
            height: 1,
            rgb: vec![10, 11, 12, 20, 21, 22],
        };
        assert_eq!(
            two.line(0, 4, 1),
            vec![10, 11, 12, 10, 11, 12, 20, 21, 22, 20, 21, 22]
        );
        assert_eq!(VideoSource::gradient(8, 8).rgb.len(), 8 * 8 * 3);
    }
}
