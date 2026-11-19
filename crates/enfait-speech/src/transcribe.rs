//! Word-by-word transcription with whisper.cpp.

use crate::SpeechError;
use crate::audio::Pcm;
use enfait_core::{Flicks, TimeRange, Transcript, Word};
use std::path::Path;
use whisper_rs::{
    DtwMode, DtwModelPreset, FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters,
};

pub struct TranscribeOptions<'a> {
    /// Language code (`fr`) or `None` for automatic detection.
    pub language: Option<&'a str>,
    /// Text steering the vocabulary (start of the script, proper names).
    pub prompt: Option<&'a str>,
    pub threads: u16,
    /// Take with retakes: each passage is transcribed without the previous
    /// text, so that whisper does not squash the repetitions.
    pub takes: bool,
}

/// DTW preset deduced from the model name, for more precise word times.
fn dtw_preset(model: &Path) -> Option<DtwModelPreset> {
    let name = model.file_name()?.to_str()?.to_ascii_lowercase();
    let en = name.contains(".en");
    Some(if name.contains("large-v3-turbo") {
        DtwModelPreset::LargeV3Turbo
    } else if name.contains("large-v3") {
        DtwModelPreset::LargeV3
    } else if name.contains("large-v2") {
        DtwModelPreset::LargeV2
    } else if name.contains("medium") {
        if en {
            DtwModelPreset::MediumEn
        } else {
            DtwModelPreset::Medium
        }
    } else if name.contains("small") {
        if en {
            DtwModelPreset::SmallEn
        } else {
            DtwModelPreset::Small
        }
    } else if name.contains("base") {
        if en {
            DtwModelPreset::BaseEn
        } else {
            DtwModelPreset::Base
        }
    } else {
        return None;
    })
}

/// whisper's hundredths of a second to flicks.
fn centis(value: i64) -> Result<Flicks, SpeechError> {
    Ok(Flicks::from_millis(value.max(0).saturating_mul(10))?)
}

pub fn transcribe(
    model: &Path,
    pcm: &Pcm,
    options: &TranscribeOptions<'_>,
) -> Result<Transcript, SpeechError> {
    // Without a log backend enabled, these hooks silence whisper.cpp and ggml.
    whisper_rs::install_logging_hooks();
    let mut ctx_params = WhisperContextParameters::default();
    if let Some(model_preset) = dtw_preset(model) {
        ctx_params.dtw_parameters.mode = DtwMode::ModelPreset { model_preset };
    }
    let ctx = WhisperContext::new_with_params(model, ctx_params)?;
    let mut state = ctx.create_state()?;
    let media_duration = pcm.duration()?;

    let mut language = None;
    let mut words = run(&mut state, &pcm.samples, 0.0, options, &mut language)?;
    if options.takes {
        // whisper avoids repeating words within a window: the start of a
        // retake can disappear. Spoken passages that nothing covers are
        // transcribed again on their own.
        let gaps = uncovered(&speech_regions(pcm), &words);
        for (a, b) in gaps {
            let start = (a - RESCUE_PAD).max(0.0);
            let end = b + RESCUE_PAD;
            let slice = sample_range(pcm, start, end);
            let Some(samples) = pcm.samples.get(slice) else {
                continue;
            };
            let found = run(&mut state, samples, start, options, &mut language)?;
            // Only the words starting in the gap: the margin may take back a
            // word already transcribed.
            let found: Vec<Word> = found
                .into_iter()
                .filter(|w| {
                    let t = w.range.start().as_secs_f64();
                    t >= a - 0.15 && t < b + 0.05
                })
                .collect();
            tracing::debug!(
                "{} word(s) recovered between {a:.2} and {b:.2} s",
                found.len()
            );
            words.extend(found);
        }
        words.sort_by_key(|w| w.range.start());
    }
    let words = tidy(words, media_duration)?;
    let language = language.unwrap_or_else(|| "?".to_owned());
    if words.is_empty() {
        return Err(SpeechError::NoSpeech);
    }
    Ok(Transcript {
        language,
        words,
        media_duration,
    })
}

/// Margin around a passage transcribed on its own.
const RESCUE_PAD: f64 = 0.25;
/// Minimum duration of an uncovered spoken passage to be transcribed.
const MIN_GAP: f64 = 0.4;

/// One whisper pass over `samples`, starting at `offset` seconds. Words
/// are returned raw (times possibly out of order).
fn run(
    state: &mut whisper_rs::WhisperState,
    samples: &[f32],
    offset: f64,
    options: &TranscribeOptions<'_>,
    language: &mut Option<String>,
) -> Result<Vec<Word>, SpeechError> {
    let mut params = FullParams::new(SamplingStrategy::BeamSearch {
        beam_size: 5,
        patience: -1.0,
    });
    params.set_n_threads(i32::from(options.threads.max(1)));
    params.set_language(Some(
        language.as_deref().or(options.language).unwrap_or("auto"),
    ));
    params.set_token_timestamps(true);
    params.set_split_on_word(true);
    params.set_max_len(1);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);
    if let Some(prompt) = options.prompt {
        params.set_initial_prompt(prompt);
    }
    if options.takes {
        params.set_no_context(true);
    }
    state.full(params, samples)?;
    if language.is_none() {
        *language = whisper_rs::get_lang_str(state.full_lang_id_from_state()).map(str::to_owned);
    }
    // Positive, bounded offset: conversion to milliseconds without useful loss.
    #[allow(clippy::cast_possible_truncation)]
    let shift = Flicks::from_millis((offset * 1000.0).round() as i64)?;
    let mut words: Vec<Word> = Vec::new();
    for segment in state.as_iter() {
        let text = segment.to_str_lossy()?.trim().to_owned();
        if text.is_empty() || text.starts_with('[') || text.starts_with('(') {
            continue;
        }
        let mut start = centis(segment.start_timestamp())?;
        let end = centis(segment.end_timestamp())?;
        let mut probability = 0.0f32;
        let mut count = 0u16;
        for i in 0..segment.n_tokens() {
            let Some(token) = segment.get_token(i) else {
                continue;
            };
            let token_text = token.to_str_lossy()?;
            if token_text.starts_with("[_") || token_text.starts_with("<|") {
                continue;
            }
            if count == 0 {
                let dtw = token.token_data().t_dtw;
                if dtw >= 0 {
                    start = centis(dtw)?;
                }
            }
            probability += token.token_probability();
            count = count.saturating_add(1);
        }
        // Detached punctuation: glued to the previous word.
        if text
            .chars()
            .all(|c| c.is_ascii_punctuation() || c == '…' || c == '»')
            && let Some(last) = words.last_mut()
        {
            last.text.push_str(&text);
            continue;
        }
        let start = start.checked_add(shift)?;
        let end = end.checked_add(shift)?.max(start);
        words.push(Word {
            text,
            range: TimeRange::new(start, start.max(end).checked_add(Flicks::new(1))?)?,
            confidence: if count == 0 {
                0.0
            } else {
                probability / f32::from(count)
            },
        });
    }
    Ok(words)
}

/// Increasing times, no overlap, never empty, within the media.
fn tidy(raw: Vec<Word>, media_duration: Flicks) -> Result<Vec<Word>, SpeechError> {
    let mut words: Vec<Word> = Vec::with_capacity(raw.len());
    for mut word in raw {
        let mut start = word.range.start();
        let end = word.range.end();
        if let Some(last) = words.last() {
            start = start.max(last.range.end());
        }
        let end = end.max(start).min(media_duration.max(start));
        let end = if end > start {
            end
        } else {
            start.checked_add(Flicks::from_millis(10)?)?
        };
        word.range = TimeRange::new(start, end)?;
        words.push(word);
    }
    Ok(words)
}

/// Indices of the samples between `start` and `end` seconds.
fn sample_range(pcm: &Pcm, start: f64, end: f64) -> std::ops::Range<usize> {
    let rate = f64::from(pcm.rate);
    // Positive and bounded by the length: truncation intended.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let at = |t: f64| ((t.max(0.0) * rate) as usize).min(pcm.samples.len());
    at(start)..at(end)
}

/// Spoken passages `(start, end)` in seconds, from the energy per 20 ms
/// slice: above three times the noise floor.
pub fn speech_regions(pcm: &Pcm) -> Vec<(f64, f64)> {
    let frame = usize::try_from(pcm.rate / 50).unwrap_or(320).max(1);
    let energies: Vec<f32> = pcm
        .samples
        .chunks(frame)
        .map(|c| {
            #[allow(clippy::cast_precision_loss)]
            let n = c.len().max(1) as f32;
            (c.iter().map(|s| s * s).sum::<f32>() / n).sqrt()
        })
        .collect();
    let mut sorted = energies.clone();
    sorted.sort_by(f32::total_cmp);
    let floor = sorted.get(sorted.len() / 7).copied().unwrap_or(0.0);
    let threshold = (floor * 3.0).max(0.004);
    let step = 1.0 / 50.0;
    let mut regions: Vec<(f64, f64)> = Vec::new();
    for (i, e) in energies.iter().enumerate() {
        if *e < threshold {
            continue;
        }
        #[allow(clippy::cast_precision_loss)]
        let t = i as f64 * step;
        match regions.last_mut() {
            // Silences under 0.15 s do not break a passage.
            Some(last) if t - last.1 < 0.15 => last.1 = t + step,
            _ => regions.push((t, t + step)),
        }
    }
    regions.retain(|(a, b)| b - a >= 0.1);
    regions
}

/// Pieces of spoken passages that no word covers (at least
/// [`MIN_GAP`]). A word covers at least 0.6 s after its start: whisper
/// often gives word ends that are too short.
fn uncovered(regions: &[(f64, f64)], words: &[Word]) -> Vec<(f64, f64)> {
    let mut covered: Vec<(f64, f64)> = words
        .iter()
        .map(|w| {
            let s = w.range.start().as_secs_f64();
            let e = w.range.end().as_secs_f64().max(s + 0.6);
            (s - 0.1, e + 0.1)
        })
        .collect();
    covered.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut gaps = Vec::new();
    for &(a, b) in regions {
        let mut cursor = a;
        for &(s, e) in &covered {
            if e <= cursor || s >= b {
                continue;
            }
            if s > cursor {
                gaps.push((cursor, s.min(b)));
            }
            cursor = cursor.max(e);
            if cursor >= b {
                break;
            }
        }
        if cursor < b {
            gaps.push((cursor, b));
        }
    }
    gaps.retain(|(a, b)| b - a >= MIN_GAP);
    gaps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(start: f64, end: f64) -> Word {
        let ms = |t: f64| Flicks::from_millis((t * 1000.0) as i64).unwrap();
        Word {
            text: "x".into(),
            range: TimeRange::new(ms(start), ms(end)).unwrap(),
            confidence: 1.0,
        }
    }

    #[test]
    fn finds_speech_that_no_word_covers() {
        // Speech from 0 to 4 s; words at 0 and 0.5 s, then at 3.5 s.
        let words = [word(0.0, 0.4), word(0.5, 0.9), word(3.5, 3.6)];
        let gaps = uncovered(&[(0.0, 4.0)], &words);
        assert_eq!(gaps.len(), 1);
        assert!((gaps[0].0 - 1.2).abs() < 1e-9 && (gaps[0].1 - 3.4).abs() < 1e-9);
        // A short gap is not kept.
        assert!(uncovered(&[(0.0, 1.3)], &words).is_empty());
    }

    #[test]
    fn speech_regions_follow_energy() {
        // 1 s of silence, 1 s of sound, 0.1 s of silence, 1 s of sound, 1 s of silence.
        let rate = 1000_u32;
        let mut samples = vec![0.0_f32; 1000];
        samples.extend((0..1000).map(|i| if i % 2 == 0 { 0.3 } else { -0.3 }));
        samples.extend(vec![0.0; 100]);
        samples.extend((0..1000).map(|i| if i % 2 == 0 { 0.3 } else { -0.3 }));
        samples.extend(vec![0.0; 1000]);
        let regions = speech_regions(&Pcm { samples, rate });
        assert_eq!(regions.len(), 1, "{regions:?}");
        assert!((regions[0].0 - 1.0).abs() < 0.03 && (regions[0].1 - 3.1).abs() < 0.03);
    }
}
