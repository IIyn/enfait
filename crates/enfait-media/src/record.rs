//! Voice recording from the default audio input (microphone).
//!
//! The sound is kept in memory as mono samples (channels averaged) while
//! recording, with the current level for a meter, then written as a
//! 16-bit WAV file at the device rate.

use crate::MediaError;
use cpal::SizedSample;
use cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

pub struct Recorder {
    _stream: cpal::Stream,
    samples: Arc<Mutex<Vec<f32>>>,
    /// Peak of the last buffer, as `f32` bits (0 to 1).
    level: Arc<AtomicU32>,
    rate: u32,
    device: String,
}

impl Recorder {
    /// Starts recording from the default input.
    pub fn start() -> Result<Self, MediaError> {
        let err = |e: &dyn std::fmt::Display| MediaError::Output(e.to_string());
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| MediaError::Output("no microphone found".into()))?;
        let name = device
            .description()
            .map(|d| d.name().to_owned())
            .unwrap_or_default();
        let supported = device.default_input_config().map_err(|e| err(&e))?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let rate = config.sample_rate;
        let samples = Arc::new(Mutex::new(Vec::with_capacity(
            usize::try_from(rate).unwrap_or(48_000).saturating_mul(60),
        )));
        let level = Arc::new(AtomicU32::new(0));
        let stream = match format {
            cpal::SampleFormat::F32 => build::<f32>(&device, config, &samples, &level),
            cpal::SampleFormat::I16 => build::<i16>(&device, config, &samples, &level),
            cpal::SampleFormat::I32 => build::<i32>(&device, config, &samples, &level),
            cpal::SampleFormat::U16 => build::<u16>(&device, config, &samples, &level),
            cpal::SampleFormat::F64 => build::<f64>(&device, config, &samples, &level),
            other => Err(MediaError::Output(format!(
                "unsupported sample format {other}"
            ))),
        }?;
        Ok(Self {
            _stream: stream,
            samples,
            level,
            rate,
            device: name,
        })
    }

    /// A view of the level and length, for a meter drawn elsewhere.
    pub fn meter(&self) -> Meter {
        Meter {
            samples: Arc::clone(&self.samples),
            level: Arc::clone(&self.level),
            rate: self.rate,
        }
    }

    /// Name of the microphone.
    pub fn device(&self) -> &str {
        &self.device
    }

    /// Peak level of the last buffer, 0 to 1.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    /// Seconds recorded so far.
    #[allow(clippy::cast_precision_loss)]
    pub fn seconds(&self) -> f64 {
        let n = self.samples.lock().map_or(0, |s| s.len());
        n as f64 / f64::from(self.rate.max(1))
    }

    /// Stops and writes the recording to `path` (WAV, 16-bit mono).
    pub fn finish(self, path: &Path) -> Result<(), MediaError> {
        let samples = self
            .samples
            .lock()
            .map(|s| s.clone())
            .map_err(|_| MediaError::Output("recording lost".into()))?;
        drop(self._stream);
        write_wav(path, &samples, self.rate)?;
        Ok(())
    }
}

/// Level and length of a recording in progress.
#[derive(Clone)]
pub struct Meter {
    samples: Arc<Mutex<Vec<f32>>>,
    level: Arc<AtomicU32>,
    rate: u32,
}

impl Meter {
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    #[allow(clippy::cast_precision_loss)]
    pub fn seconds(&self) -> f64 {
        let n = self.samples.lock().map_or(0, |s| s.len());
        n as f64 / f64::from(self.rate.max(1))
    }
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    samples: &Arc<Mutex<Vec<f32>>>,
    level: &Arc<AtomicU32>,
) -> Result<cpal::Stream, MediaError>
where
    T: SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = usize::from(config.channels).max(1);
    let (samples, level) = (Arc::clone(samples), Arc::clone(level));
    let stream = device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                let mut peak = 0.0f32;
                if let Ok(mut out) = samples.lock() {
                    for frame in data.chunks(channels) {
                        #[allow(clippy::cast_precision_loss)]
                        let v = frame.iter().map(|s| s.to_sample::<f32>()).sum::<f32>()
                            / frame.len().max(1) as f32;
                        peak = peak.max(v.abs());
                        out.push(v);
                    }
                }
                level.store(peak.min(1.0).to_bits(), Ordering::Relaxed);
            },
            |e| tracing::warn!("audio input: {e}"),
            None,
        )
        .map_err(|e| MediaError::Output(e.to_string()))?;
    stream
        .play()
        .map_err(|e| MediaError::Output(e.to_string()))?;
    Ok(stream)
}

/// Writes mono samples (−1 to 1) as a 16-bit WAV file.
pub fn write_wav(path: &Path, samples: &[f32], rate: u32) -> std::io::Result<()> {
    let data = u32::try_from(samples.len().saturating_mul(2)).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(samples.len().saturating_mul(2).saturating_add(44));
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&data.saturating_add(36).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&rate.saturating_mul(2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        #[allow(clippy::cast_possible_truncation)]
        let v = (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("wav.tmp");
    std::fs::write(&tmp, out)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// Records one second from the default microphone (needs one):
    /// `cargo test -p enfait-media record -- --ignored`.
    #[test]
    #[ignore = "needs a microphone"]
    fn records_from_the_microphone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mic.wav");
        let recorder = Recorder::start().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1200));
        assert!(recorder.seconds() > 0.5, "{} s", recorder.seconds());
        recorder.finish(&path).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 44 + 16_000);
    }

    #[test]
    fn wav_header_and_samples() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.wav");
        write_wav(&path, &[0.0, 1.0, -1.0, 2.0], 48_000).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.len(), 44 + 8);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            48_000
        );
        assert_eq!(i16::from_le_bytes([bytes[46], bytes[47]]), i16::MAX);
        assert_eq!(i16::from_le_bytes([bytes[48], bytes[49]]), -i16::MAX);
        assert_eq!(i16::from_le_bytes([bytes[50], bytes[51]]), i16::MAX);
    }
}
