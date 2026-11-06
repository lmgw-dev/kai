//! A WAV file as Kai's playback takes it: 24 kHz mono PCM16. The direct speech path
//! (`crate::voice::speak`) asks lmgw's `/v1/audio/speech` for WAV, which lmgw passes on from the
//! row that answers as it came: audio.cpp writes the canonical 44-byte header, PCM16 mono at the
//! row's own rate (Supertonic 44.1 kHz, Pocket TTS 24 kHz; lmgw's `audio/rates.rs`), a cloud
//! route its own WAV. So a model of any rate plays at the right speed.
//!
//! - **Read:** PCM of 8, 16, 24 or 32 bits and IEEE float of 32 or 64 bits (`WAVE_FORMAT_EXTENSIBLE`
//!   with either sub-format too), any number of channels (averaged to one), `RIFF` and `RF64`
//!   (whose sizes are in its `ds64` chunk).
//! - **A `data` chunk whose size is unknown** runs to the end of the file, as lmgw's own reader
//!   takes a streaming writer's (`realtime/audio/pcm.rs`): a size of `0xFFFFFFFF`, or of 0 in a
//!   file whose `RIFF` size is unknown too (0 or `0xFFFFFFFF`: a header written before any length
//!   was known). A `data` size of 0 in a file whose `RIFF` size is known is an empty chunk.
//! - **Any rate** is resampled to 24 kHz through a windowed-sinc low-pass ([`resample`]): its
//!   cut-off is below the lower rate's Nyquist frequency, so downsampling a 44.1 kHz voice does
//!   not fold its upper band back into the audible one, as linear interpolation would. Kai has no
//!   other resampler (PipeWire converts its streams' rates).

use super::RATE;

/// The samples of `wav` as 24 kHz mono PCM16, or why they cannot be read.
pub fn pcm24k(wav: &[u8]) -> Result<Vec<i16>, String> {
    let rf64 = wav.len() >= 4 && &wav[..4] == b"RF64";
    if wav.len() < 12 || !(rf64 || &wav[..4] == b"RIFF") || &wav[8..12] != b"WAVE" {
        return Err(format!(
            "the answer is no WAV file ({} bytes, starting {:?})",
            wav.len(),
            String::from_utf8_lossy(&wav[..wav.len().min(4)])
        ));
    }
    let u32_at = |i: usize| u32::from_le_bytes([wav[i], wav[i + 1], wav[i + 2], wav[i + 3]]);
    let riff_unknown = matches!(u32_at(4), 0 | u32::MAX);
    let mut at = 12;
    let mut fmt: Option<Fmt> = None;
    // RF64: the data chunk's real size.
    let mut ds64: Option<u64> = None;
    while at + 8 <= wav.len() {
        let id = &wav[at..at + 4];
        let size = u32_at(at + 4);
        let body = at + 8;
        let rest = wav.len() - body;
        if id == b"ds64" && size >= 16 && rest >= 16 {
            let b = &wav[body + 8..body + 16];
            ds64 = Some(u64::from_le_bytes(b.try_into().expect("eight bytes")));
        } else if id == b"fmt " {
            let end = body.saturating_add(size as usize).min(wav.len());
            fmt = Some(Fmt::read(&wav[body..end])?);
        } else if id == b"data" {
            let f = fmt.ok_or("the WAV file has its data before its format")?;
            let len = match (size, ds64) {
                (u32::MAX, Some(n)) if rf64 => usize::try_from(n).unwrap_or(usize::MAX).min(rest),
                (u32::MAX, _) => rest,
                (0, _) if riff_unknown => rest,
                (n, _) => (n as usize).min(rest),
            };
            return Ok(resample(&f.mono(&wav[body..body + len]), f.rate, RATE));
        }
        // Chunks are padded to an even size.
        at = body
            .saturating_add(size as usize)
            .saturating_add(size as usize & 1);
    }
    Err("the WAV file has no data".into())
}

#[derive(Debug, Clone, Copy)]
struct Fmt {
    float: bool,
    channels: usize,
    rate: u32,
    bits: usize,
}

impl Fmt {
    fn read(b: &[u8]) -> Result<Self, String> {
        if b.len() < 16 {
            return Err("the WAV file's format chunk is too short".into());
        }
        let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
        let mut tag = u16_at(0);
        // WAVE_FORMAT_EXTENSIBLE: the sub-format's first two bytes are the tag.
        if tag == 0xFFFE && b.len() >= 26 {
            tag = u16_at(24);
        }
        let f = Self {
            float: tag == 3,
            channels: usize::from(u16_at(2)),
            rate: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            bits: usize::from(u16_at(14)),
        };
        let known = matches!((tag, f.bits), (1, 8 | 16 | 24 | 32) | (3, 32 | 64));
        if !known || f.channels == 0 || f.rate == 0 {
            return Err(format!(
                "the WAV file's format is one Kai does not play (format {tag}, {} bits, {} \
                 channels, {} Hz)",
                f.bits, f.channels, f.rate
            ));
        }
        Ok(f)
    }

    /// One frame's channels averaged, as -1.0 to 1.0.
    fn mono(&self, data: &[u8]) -> Vec<f32> {
        let width = self.bits / 8;
        let frame = width * self.channels;
        data.chunks_exact(frame)
            .map(|f| {
                let sum: f32 = f.chunks_exact(width).map(|s| self.sample(s)).sum();
                sum / self.channels as f32
            })
            .collect()
    }

    fn sample(&self, s: &[u8]) -> f32 {
        let v = match (self.float, self.bits) {
            (true, 32) => f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
            (true, _) => {
                f64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]) as f32
            }
            (false, 8) => (f32::from(s[0]) - 128.0) / 128.0,
            (false, 16) => f32::from(i16::from_le_bytes([s[0], s[1]])) / 32768.0,
            (false, 24) => (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f32 / 8_388_608.0,
            (false, _) => i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32 / 2_147_483_648.0,
        };
        if v.is_finite() { v } else { 0.0 }
    }
}

/// Zero crossings of the sinc on each side of a sample, at the lower of the two rates.
const ZEROS: f64 = 16.0;
/// The cut-off as a share of the lower rate's Nyquist frequency.
const ROLLOFF: f64 = 0.92;

/// `x` at rate `from` as PCM16 at rate `to` (module doc): each output sample is the input around
/// its place weighted by a Blackman-windowed sinc whose cut-off is `ROLLOFF` of the lower rate's
/// Nyquist frequency, the weights summed to one so a constant stays constant.
fn resample(x: &[f32], from: u32, to: u32) -> Vec<i16> {
    let pcm = |v: f32| (v.clamp(-1.0, 1.0) * 32767.0).round() as i16;
    if from == to || x.is_empty() {
        return x.iter().copied().map(pcm).collect();
    }
    let ratio = f64::from(to) / f64::from(from);
    // The cut-off in cycles per input sample, and the kernel's half width in input samples.
    let fc = 0.5 * ratio.min(1.0) * ROLLOFF;
    let half = ZEROS / ratio.min(1.0);
    let n = (x.len() as u64 * u64::from(to) / u64::from(from)) as usize;
    let last = x.len() as i64 - 1;
    (0..n)
        .map(|i| {
            let t = i as f64 / ratio;
            let lo = (t - half).ceil().max(0.0) as i64;
            let hi = ((t + half).floor() as i64).min(last);
            let (mut acc, mut sum) = (0.0f64, 0.0f64);
            for j in lo..=hi {
                let u = t - j as f64;
                let w = kernel(u, fc, half);
                acc += w * f64::from(x[j as usize]);
                sum += w;
            }
            pcm(if sum.abs() > f64::EPSILON {
                (acc / sum) as f32
            } else {
                0.0
            })
        })
        .collect()
}

/// The low-pass kernel at `u` input samples from the output's place.
fn kernel(u: f64, fc: f64, half: f64) -> f64 {
    use std::f64::consts::PI;
    let s = if u.abs() < 1e-12 {
        2.0 * fc
    } else {
        (2.0 * PI * fc * u).sin() / (PI * u)
    };
    // Blackman over [-half, half].
    let p = (u / half + 1.0) * 0.5;
    let w = 0.42 - 0.5 * (2.0 * PI * p).cos() + 0.08 * (4.0 * PI * p).cos();
    s * w.max(0.0)
}

/// A WAV file of `pcm` at `rate` with `channels` interleaved channels, as audio.cpp writes one:
/// the canonical 44-byte header with both sizes filled in (the tests' and the fakes').
#[cfg(test)]
pub fn wav16(pcm: &[i16], rate: u32, channels: u16) -> Vec<u8> {
    let data = pcm.len() * 2;
    let mut w = Vec::with_capacity(44 + data);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&channels.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 2 * u32::from(channels)).to_le_bytes());
    w.extend_from_slice(&(2 * channels).to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(data as u32).to_le_bytes());
    for s in pcm {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f64, rate: u32, secs: f64, amp: f64) -> Vec<i16> {
        let n = (f64::from(rate) * secs) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / f64::from(rate);
                ((2.0 * std::f64::consts::PI * hz * t).sin() * amp * 32767.0).round() as i16
            })
            .collect()
    }

    /// The RMS of `x` away from its edges (the kernel's half width), as a share of full scale.
    fn rms(x: &[i16]) -> f64 {
        let m = &x[200..x.len() - 200];
        let s: f64 = m.iter().map(|v| (f64::from(*v) / 32767.0).powi(2)).sum();
        (s / m.len() as f64).sqrt()
    }

    #[test]
    fn a_wav_of_any_rate_and_channels_plays_as_24k_mono() {
        let same = pcm24k(&wav16(&[1000, -1000, 2000], 24_000, 1)).unwrap();
        assert_eq!(same, [1000, -1000, 2000]);
        // Two channels averaged.
        let stereo = pcm24k(&wav16(&[1000, 3000, -2000, 0], 24_000, 2)).unwrap();
        assert_eq!(stereo, [2000, -1000]);
        // 48 kHz halves, 16 kHz grows by half; a second of audio stays a second; a constant
        // stays constant.
        assert_eq!(
            pcm24k(&wav16(&[500; 48_000], 48_000, 1)).unwrap().len(),
            24_000
        );
        let up = pcm24k(&wav16(&[500; 16_000], 16_000, 1)).unwrap();
        assert_eq!(up.len(), 24_000);
        assert!(
            up.iter().all(|s| (*s - 500).abs() <= 1),
            "a constant stays constant"
        );
    }

    /// What lmgw passes on from a Supertonic row (audio.cpp's canonical header, PCM16 mono at
    /// 44.1 kHz): a 1 kHz tone keeps its level, and a tone above 12 kHz, which 24 kHz cannot
    /// carry, is filtered out instead of folding back as an audible one (linear interpolation
    /// left a 20 kHz tone at about half its level as 4 kHz).
    #[test]
    fn resampling_keeps_the_voice_band_and_filters_what_would_fold_back() {
        let tone = pcm24k(&wav16(&sine(1000.0, 44_100, 0.5, 0.5), 44_100, 1)).unwrap();
        assert_eq!(tone.len(), 12_000);
        let level = rms(&tone);
        assert!(
            (level - 0.5 / 2f64.sqrt()).abs() < 0.01,
            "1 kHz keeps its level: {level}"
        );
        for (hz, rate) in [(20_000.0, 44_100), (20_000.0, 48_000), (16_000.0, 44_100)] {
            let high = pcm24k(&wav16(&sine(hz, rate, 0.5, 0.5), rate, 1)).unwrap();
            let level = rms(&high);
            assert!(
                level < 0.5 / 2f64.sqrt() * 0.01,
                "{hz} Hz at {rate} Hz is 40 dB down at least: {level}"
            );
        }
        // Upsampling a 16 kHz voice keeps it too.
        let up = pcm24k(&wav16(&sine(1000.0, 16_000, 0.5, 0.5), 16_000, 1)).unwrap();
        assert!((rms(&up) - 0.5 / 2f64.sqrt()).abs() < 0.01);
    }

    #[test]
    fn a_data_size_is_unknown_only_by_the_streaming_conventions() {
        // 0xFFFFFFFF: to the end of the file.
        let mut streamed = wav16(&[7, 8, 9], 24_000, 1);
        streamed[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(pcm24k(&streamed).unwrap(), [7, 8, 9]);
        // 0 with the RIFF size unknown too (a header written before any length was known).
        let mut zero = wav16(&[7, 8, 9], 24_000, 1);
        zero[4..8].copy_from_slice(&0u32.to_le_bytes());
        zero[40..44].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(pcm24k(&zero).unwrap(), [7, 8, 9]);
        // 0 in a file whose RIFF size is known is an empty chunk: what follows it is no audio.
        let mut empty = wav16(&[], 24_000, 1);
        empty.extend_from_slice(b"LIST\x04\0\0\0INFO");
        let riff = (empty.len() - 8) as u32;
        empty[4..8].copy_from_slice(&riff.to_le_bytes());
        assert_eq!(pcm24k(&empty).unwrap(), Vec::<i16>::new());
        // A known size stops before a chunk after it.
        let mut listed = wav16(&[1, 2], 24_000, 1);
        listed.extend_from_slice(b"LIST\x04\0\0\0INFO");
        assert_eq!(pcm24k(&listed).unwrap(), [1, 2]);
        // RF64: the size in ds64.
        let mut rf64 = b"RF64\xff\xff\xff\xffWAVEds64".to_vec();
        rf64.extend_from_slice(&28u32.to_le_bytes());
        rf64.extend_from_slice(&0u64.to_le_bytes());
        rf64.extend_from_slice(&4u64.to_le_bytes());
        rf64.extend_from_slice(&2u64.to_le_bytes());
        rf64.extend_from_slice(&0u32.to_le_bytes());
        let plain = wav16(&[5, 6, 7], 24_000, 1);
        rf64.extend_from_slice(&plain[12..36]);
        rf64.extend_from_slice(b"data\xff\xff\xff\xff");
        rf64.extend_from_slice(&plain[44..]);
        assert_eq!(pcm24k(&rf64).unwrap(), [5, 6], "two samples, as ds64 says");
    }

    #[test]
    fn what_is_no_wav_is_said() {
        let e = pcm24k(b"ID3\x04 an mp3").unwrap_err();
        assert!(e.contains("no WAV file"), "{e}");
        let mut odd = wav16(&[1], 24_000, 1);
        odd[34..36].copy_from_slice(&12u16.to_le_bytes());
        let e = pcm24k(&odd).unwrap_err();
        assert!(e.contains("12 bits"), "{e}");
    }
}
