//! Decoded sound in memory, ready for playback and the waveform.
//!
//! The sound is kept whole in memory, as 16-bit integers at 48 kHz, one
//! or two channels: about 11 MB per minute in stereo. That is enough for
//! voice-overs and rushes of a few tens of minutes, and allows backward
//! and fast playback (J/K/L) without restarting a decoder.

use crate::MediaError;
use std::path::Path;
use std::sync::Arc;

/// Working sample rate.
pub const RATE: u32 = 48_000;

#[derive(Debug, Clone)]
pub struct AudioBuffer {
    rate: u32,
    channels: u16,
    /// Interleaved samples.
    samples: Arc<[i16]>,
}

impl AudioBuffer {
    pub fn new(rate: u32, channels: u16, samples: Vec<i16>) -> Result<Self, MediaError> {
        if rate == 0 || channels == 0 || channels > 2 {
            return Err(MediaError::Cache(format!(
                "audio {rate} Hz, {channels} voies : non pris en charge"
            )));
        }
        Ok(Self {
            rate,
            channels,
            samples: samples.into(),
        })
    }

    pub const fn rate(&self) -> u32 {
        self.rate
    }

    pub const fn channels(&self) -> u16 {
        self.channels
    }

    pub fn samples(&self) -> &[i16] {
        &self.samples
    }

    /// Number of frames (one sample per channel).
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels)
    }

    pub fn duration(&self) -> f64 {
        self.frames() as f64 / f64::from(self.rate)
    }

    /// Value of frame `frame`, left and right channels (the same in mono),
    /// from -1 to 1. Out of range: silence.
    pub fn frame(&self, frame: usize) -> [f32; 2] {
        let scale = 1.0 / 32_768.0;
        if self.channels == 1 {
            let v = self
                .samples
                .get(frame)
                .map_or(0.0, |s| f32::from(*s) * scale);
            [v, v]
        } else {
            let i = frame.saturating_mul(2);
            let l = self.samples.get(i).map_or(0.0, |s| f32::from(*s) * scale);
            let r = self
                .samples
                .get(i.saturating_add(1))
                .map_or(0.0, |s| f32::from(*s) * scale);
            [l, r]
        }
    }

    /// Value at a fractional time (linear interpolation).
    pub fn sample_at(&self, position: f64) -> [f32; 2] {
        if position.is_nan() || position < 0.0 {
            return [0.0, 0.0];
        }
        // Positive, finite position: the conversion truncates towards zero.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let i = position.floor() as usize;
        #[allow(clippy::cast_possible_truncation)]
        let frac = (position - position.floor()) as f32;
        let [a0, a1] = self.frame(i);
        let [b0, b1] = self.frame(i.saturating_add(1));
        [a0 + (b0 - a0) * frac, a1 + (b1 - a1) * frac]
    }

    /// Reads a raw file written by [`AudioBuffer::write_raw`] or ffmpeg
    /// (`s16le`, interleaved).
    pub fn read_raw(path: &Path, rate: u32, channels: u16) -> Result<Self, MediaError> {
        let bytes = std::fs::read(path)?;
        Self::new(rate, channels, from_le_bytes(&bytes))
    }
}

pub(crate) fn from_le_bytes(bytes: &[u8]) -> Vec<i16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b))
        .collect()
}

/// Decodes the first audio stream of `input` to `s16le` 48 kHz, one or two
/// channels, into `out`.
pub(crate) fn decode_to_file(
    input: &Path,
    channels: u16,
    duration: f64,
    out: &Path,
    on_progress: &mut dyn FnMut(f32),
) -> Result<(), MediaError> {
    let mut args: Vec<std::ffi::OsString> = vec!["-i".into(), input.into()];
    args.extend(
        [
            "-map",
            "0:a:0",
            "-vn",
            "-ac",
            &channels.to_string(),
            "-ar",
            &RATE.to_string(),
            "-c:a",
            "pcm_s16le",
            "-f",
            "s16le",
        ]
        .map(Into::into),
    );
    args.push(out.into());
    crate::ffmpeg::run(args, duration, on_progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_and_stereo_frames() {
        let mono = AudioBuffer::new(4, 1, vec![0, 16_384, -32_768]).unwrap();
        assert_eq!(mono.frames(), 3);
        assert_eq!(mono.frame(1), [0.5, 0.5]);
        assert_eq!(mono.frame(9), [0.0, 0.0]);
        assert_eq!(mono.duration(), 0.75);

        let stereo = AudioBuffer::new(4, 2, vec![0, 16_384, 16_384, 0]).unwrap();
        assert_eq!(stereo.frames(), 2);
        assert_eq!(stereo.frame(1), [0.5, 0.0]);
        assert_eq!(stereo.sample_at(0.5), [0.25, 0.25]);
        assert_eq!(stereo.sample_at(-1.0), [0.0, 0.0]);
        assert_eq!(stereo.sample_at(f64::NAN), [0.0, 0.0]);
    }

    #[test]
    fn rejects_unsupported_layouts() {
        assert!(AudioBuffer::new(48_000, 6, vec![]).is_err());
        assert!(AudioBuffer::new(0, 1, vec![]).is_err());
    }

    #[test]
    fn little_endian_bytes() {
        assert_eq!(from_le_bytes(&[1, 0, 0xff, 0xff, 7]), vec![1, -1]);
    }
}
