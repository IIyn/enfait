//! Voice recording from an audio input (microphone).
//!
//! The sound is kept in memory as mono samples (channels averaged) while
//! recording, with the current level for a meter, then written as a
//! 16-bit WAV file at the device rate.

use crate::MediaError;
use cpal::SizedSample;
use cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// An audio input the user can pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDevice {
    /// Stable identifier (`cpal::DeviceId` as text), kept in the settings.
    pub id: String,
    pub name: String,
}

/// Audio inputs of the system, the default one first.
pub fn input_devices() -> Vec<InputDevice> {
    let host = cpal::default_host();
    let default = host.default_input_device().and_then(|d| d.id().ok());
    let mut list: Vec<(bool, InputDevice)> = host
        .input_devices()
        .map(|devices| {
            devices
                .filter_map(|d| {
                    let id = d.id().ok()?;
                    // PulseAudio monitors replay an output, not a microphone.
                    if id.to_string().ends_with(".monitor") {
                        return None;
                    }
                    let name = d
                        .description()
                        .map(|d| d.name().to_owned())
                        .unwrap_or_else(|_| id.to_string());
                    Some((
                        default.as_ref() == Some(&id),
                        InputDevice {
                            id: id.to_string(),
                            name,
                        },
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    list.sort_by_key(|(is_default, _)| !is_default);
    let mut seen = std::collections::HashSet::new();
    list.into_iter()
        .map(|(_, d)| d)
        .filter(|d| seen.insert(d.name.clone()))
        .collect()
}

pub struct Recorder {
    _stream: cpal::Stream,
    samples: Arc<Mutex<Vec<f32>>>,
    /// Peak of the last buffer, as `f32` bits (0 to 1).
    level: Arc<AtomicU32>,
    /// Moment the first sample was captured.
    started: Arc<OnceLock<Instant>>,
    rate: u32,
    device: String,
}

impl Recorder {
    /// Starts recording from the input `device` (an [`InputDevice::id`]),
    /// or from the default input when it is `None` or no longer exists.
    pub fn start(device: Option<&str>) -> Result<Self, MediaError> {
        let err = |e: &dyn std::fmt::Display| MediaError::Output(e.to_string());
        let host = cpal::default_host();
        let chosen = device.and_then(|id| {
            let found = id
                .parse::<cpal::DeviceId>()
                .ok()
                .and_then(|id| host.device_by_id(&id));
            if found.is_none() {
                tracing::warn!("microphone {id} not found; using the default one");
            }
            found
        });
        let device = chosen
            .or_else(|| host.default_input_device())
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
        let started = Arc::new(OnceLock::new());
        let shared = Shared {
            samples: &samples,
            level: &level,
            started: &started,
        };
        let stream = match format {
            cpal::SampleFormat::F32 => build::<f32>(&device, config, &shared),
            cpal::SampleFormat::I16 => build::<i16>(&device, config, &shared),
            cpal::SampleFormat::I32 => build::<i32>(&device, config, &shared),
            cpal::SampleFormat::U16 => build::<u16>(&device, config, &shared),
            cpal::SampleFormat::F64 => build::<f64>(&device, config, &shared),
            other => Err(MediaError::Output(format!(
                "unsupported sample format {other}"
            ))),
        }?;
        Ok(Self {
            _stream: stream,
            samples,
            level,
            started,
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
        self.finish_from(path, None)
    }

    /// Like [`Self::finish`], with the sound starting at the moment
    /// `start` (the first frame of a camera): silence is added before the
    /// first sample, or the samples captured earlier are dropped.
    pub fn finish_from(self, path: &Path, start: Option<Instant>) -> Result<(), MediaError> {
        let mut samples = self
            .samples
            .lock()
            .map(|s| s.clone())
            .map_err(|_| MediaError::Output("recording lost".into()))?;
        drop(self._stream);
        if let (Some(start), Some(&first)) = (start, self.started.get()) {
            align(&mut samples, self.rate, first, start);
        }
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

/// Moves the start of `samples` (first captured at `first`) to `start`.
fn align(samples: &mut Vec<f32>, rate: u32, first: Instant, start: Instant) {
    let frames = |d: Duration| {
        // A few seconds at most: no overflow.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let n = (d.as_secs_f64() * f64::from(rate)).round() as usize;
        n
    };
    if first > start {
        let pad = frames(first - start);
        samples.splice(0..0, std::iter::repeat_n(0.0, pad));
    } else {
        let cut = frames(start - first).min(samples.len());
        samples.drain(..cut);
    }
}

/// State shared with the audio callback.
struct Shared<'a> {
    samples: &'a Arc<Mutex<Vec<f32>>>,
    level: &'a Arc<AtomicU32>,
    started: &'a Arc<OnceLock<Instant>>,
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    shared: &Shared<'_>,
) -> Result<cpal::Stream, MediaError>
where
    T: SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = usize::from(config.channels).max(1);
    let rate = f64::from(config.sample_rate.max(1));
    let samples = Arc::clone(shared.samples);
    let level = Arc::clone(shared.level);
    let started = Arc::clone(shared.started);
    let stream = device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                // The buffer ends now: its first sample was captured one
                // buffer earlier.
                started.get_or_init(|| {
                    #[allow(clippy::cast_precision_loss)]
                    let length = (data.len() / channels) as f64 / rate;
                    Instant::now()
                        .checked_sub(Duration::from_secs_f64(length))
                        .unwrap_or_else(Instant::now)
                });
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
        let recorder = Recorder::start(None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1200));
        assert!(recorder.seconds() > 0.5, "{} s", recorder.seconds());
        recorder.finish(&path).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 44 + 16_000);
    }

    /// Lists the inputs: `cargo test -p enfait-media lists_inputs -- --ignored --nocapture`.
    #[test]
    #[ignore = "depends on the machine"]
    #[allow(clippy::print_stdout)]
    fn lists_inputs() {
        for d in input_devices() {
            println!("{} — {}", d.name, d.id);
        }
    }

    #[test]
    fn aligns_on_a_later_or_earlier_start() {
        let first = Instant::now();
        let mut samples = vec![1.0; 10];
        align(&mut samples, 10, first, first + Duration::from_millis(300));
        assert_eq!(samples.len(), 7);
        let mut samples = vec![1.0; 10];
        align(&mut samples, 10, first + Duration::from_millis(200), first);
        assert_eq!(samples.len(), 12);
        assert_eq!(&samples[..3], &[0.0, 0.0, 1.0]);
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
