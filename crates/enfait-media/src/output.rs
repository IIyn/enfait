//! Lecture : la sortie audio fait avancer l'horloge.
//!
//! Avec du son et une carte son, c'est le rappel de cpal qui avance
//! l'horloge, au rythme exact des échantillons joués. Sans son (vidéo muette)
//! ou sans carte, un fil de travail l'avance d'après l'horloge système.

use crate::{AudioBuffer, Clock, Edl, MediaError};
use cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};
use cpal::{FromSample, Sample, SizedSample};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Au-delà de cette vitesse, le son est atténué (navette rapide).
const LOUD_SPEED: f64 = 2.0;

/// Lecture en cours ; s'arrête quand elle est abandonnée.
pub struct Playback {
    inner: Inner,
}

/// Les deux variantes ne servent qu'à garder vivant ce qui fait avancer
/// l'horloge.
enum Inner {
    Audio { _stream: cpal::Stream },
    Ticker { _ticker: Ticker },
}

impl Playback {
    /// Démarre la lecture de `audio` (s'il y en a) en suivant `clock`,
    /// selon le montage `edl` (l'horloge est en temps montage).
    pub fn start(clock: Arc<Clock>, audio: Option<AudioBuffer>, edl: Arc<Edl>) -> Self {
        if let Some(audio) = audio {
            match open_stream(Arc::clone(&clock), audio, edl) {
                Ok(stream) => {
                    return Self {
                        inner: Inner::Audio { _stream: stream },
                    };
                }
                Err(e) => tracing::warn!("{e} ; lecture sans son"),
            }
        }
        Self {
            inner: Inner::Ticker {
                _ticker: Ticker::start(clock),
            },
        }
    }

    pub const fn has_sound(&self) -> bool {
        matches!(self.inner, Inner::Audio { .. })
    }
}

struct Ticker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Ticker {
    fn start(clock: Arc<Clock>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("enfait-clock".into())
                .spawn(move || {
                    let mut last = Instant::now();
                    while !stop.load(Ordering::Acquire) {
                        std::thread::sleep(Duration::from_millis(4));
                        let now = Instant::now();
                        clock.tick(now.duration_since(last).as_secs_f64(), 0.0);
                        last = now;
                    }
                })
                .map_err(|e| tracing::error!("horloge de lecture : {e}"))
                .ok()
        };
        Self { stop, thread }
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn open_stream(
    clock: Arc<Clock>,
    audio: AudioBuffer,
    edl: Arc<Edl>,
) -> Result<cpal::Stream, MediaError> {
    let err = |e: &dyn std::fmt::Display| MediaError::Output(e.to_string());
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| MediaError::Output("aucune sortie audio".into()))?;
    let supported = device.default_output_config().map_err(|e| err(&e))?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    match format {
        cpal::SampleFormat::F32 => build::<f32>(&device, config, clock, audio, edl),
        cpal::SampleFormat::I16 => build::<i16>(&device, config, clock, audio, edl),
        cpal::SampleFormat::I32 => build::<i32>(&device, config, clock, audio, edl),
        cpal::SampleFormat::U16 => build::<u16>(&device, config, clock, audio, edl),
        cpal::SampleFormat::F64 => build::<f64>(&device, config, clock, audio, edl),
        other => Err(MediaError::Output(format!(
            "format d'échantillon {other} non pris en charge"
        ))),
    }
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    clock: Arc<Clock>,
    audio: AudioBuffer,
    edl: Arc<Edl>,
) -> Result<cpal::Stream, MediaError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels).max(1);
    let device_rate = f64::from(config.sample_rate);
    let stream = device
        .build_output_stream(
            config,
            move |out: &mut [T], info: &cpal::OutputCallbackInfo| {
                let frames = out.len() / channels;
                // Temps entre cet appel et la sortie effective du son.
                let stamp = info.timestamp();
                let delay = (stamp.playback - stamp.callback).as_secs_f64();
                #[allow(clippy::cast_precision_loss)]
                let tick = clock.tick(frames as f64 / device_rate, delay);
                fill(out, channels, &audio, &edl, tick, device_rate);
            },
            |e| tracing::warn!("sortie audio : {e}"),
            None,
        )
        .map_err(|e| MediaError::Output(e.to_string()))?;
    stream
        .play()
        .map_err(|e| MediaError::Output(e.to_string()))?;
    Ok(stream)
}

/// Durée des fondus de part et d'autre d'une coupe (évite les clics).
const CUT_FADE: f64 = 0.004;

/// Remplit `out` (entrelacé, `channels` voies) avec le son à partir de
/// l'instant `tick.start` du montage, à la vitesse `tick.speed`.
fn fill<T>(
    out: &mut [T],
    channels: usize,
    audio: &AudioBuffer,
    edl: &Edl,
    tick: crate::transport::Tick,
    device_rate: f64,
) where
    T: Sample + FromSample<f32>,
{
    if tick.speed == 0.0 {
        out.fill(T::EQUILIBRIUM);
        return;
    }
    let gain = if tick.speed.abs() > LOUD_SPEED {
        0.5
    } else {
        1.0
    };
    let source_rate = f64::from(audio.rate());
    let step = tick.speed / device_rate;
    let mut t = tick.start;
    let end = audio.frames() as f64;
    let duration = edl.duration();
    for frame in out.chunks_mut(channels) {
        let position = edl.to_source(t) * source_rate;
        let [l, r] = if t >= 0.0 && t < duration && position < end {
            audio.sample_at(position)
        } else {
            [0.0, 0.0]
        };
        #[allow(clippy::cast_possible_truncation)]
        let fade = (edl.cut_distance(t) / CUT_FADE).min(1.0) as f32;
        let (l, r) = (l * gain * fade, r * gain * fade);
        match frame {
            [mono] => *mono = T::from_sample((l + r) * 0.5),
            [left, right, rest @ ..] => {
                *left = T::from_sample(l);
                *right = T::from_sample(r);
                rest.fill(T::EQUILIBRIUM);
            }
            [] => {}
        }
        t += step;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::Tick;

    #[test]
    fn fills_forward_backward_and_silence() {
        let audio = AudioBuffer::new(4, 1, vec![0, 8192, 16_384, 24_576]).unwrap();
        let edl = Edl::identity(1.0);
        let mut out = [0.0_f32; 8];
        let play = |start: f64, speed: f64| Tick { start, speed };

        fill(&mut out, 2, &audio, &edl, play(0.0, 1.0), 4.0);
        assert_eq!(out, [0.0, 0.0, 0.25, 0.25, 0.5, 0.5, 0.75, 0.75]);

        fill(&mut out, 2, &audio, &edl, play(0.75, -1.0), 4.0);
        assert_eq!(out, [0.75, 0.75, 0.5, 0.5, 0.25, 0.25, 0.0, 0.0]);

        // Sortie à 8 Hz : chaque instant source est joué deux fois.
        let mut mono = [0.0_f32; 4];
        fill(&mut mono, 1, &audio, &edl, play(0.0, 1.0), 8.0);
        assert_eq!(mono, [0.0, 0.125, 0.25, 0.375]);

        fill(&mut out, 2, &audio, &edl, play(0.5, 0.0), 4.0);
        assert_eq!(out, [0.0; 8]);

        // Au-delà de la fin : silence.
        fill(&mut out, 2, &audio, &edl, play(5.0, 1.0), 4.0);
        assert_eq!(out, [0.0; 8]);
    }

    #[test]
    fn follows_the_cut_and_fades_at_joins() {
        // 1 s à 1 000 Hz : la valeur d'un instant est son index / 1000.
        let samples: Vec<i16> = (0..1000).map(|i| i16::try_from(i * 32).unwrap()).collect();
        let audio = AudioBuffer::new(1000, 1, samples).unwrap();
        // Montage : [0,5 ; 0,6) puis [0,1 ; 0,2).
        let edl = Edl::from_ranges(&[(0.5, 0.6), (0.1, 0.2)]);
        let mut out = [0.0_f32; 200];
        fill(
            &mut out,
            1,
            &audio,
            &edl,
            Tick {
                start: 0.0,
                speed: 1.0,
            },
            1000.0,
        );
        let at = |i: usize| out[i] * 32768.0 / 32.0;
        assert!((at(50) - 550.0).abs() < 0.5, "{}", at(50));
        assert!((at(150) - 150.0).abs() < 0.5, "{}", at(150));
        // À la coupe (t = 0,1) : silence, puis remontée en 4 ms.
        assert!(at(100).abs() < 1e-3);
        assert!(at(102) > 0.0 && at(102) < 102.0);
        assert!(out[199] == 0.0 || at(199) > 0.0);
    }
}
