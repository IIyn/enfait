//! Cut screen: takes of each sentence, "last take" rule, manual choices
//! and the resulting edit.

use crate::tr;
use enfait_core::Transcript;
use enfait_speech::takes::{self, Analysis};
use gpui_kit::SharedString;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CutStatus {
    /// No take imported.
    NoRush,
    Transcribing,
    Failed(SharedString),
    Ready,
}

pub struct CutState {
    pub status: CutStatus,
    pub transcript: Option<Arc<Transcript>>,
    pub analysis: Option<Analysis>,
    /// Take kept for each sentence (index in `analysis.takes`).
    pub choice: Vec<Option<usize>>,
    /// Script of the last analysis: redo it if it changed.
    pub analyzed: Option<String>,
    /// Play the edit (else the whole take).
    pub montage: bool,
    /// Transcription progress, from 0 to 1 (bits of an `f32`), written by
    /// the background task.
    pub progress: Arc<AtomicU32>,
    /// When the running transcription started, and from which share.
    pub started: Option<(Instant, f32)>,
    /// Progress last drawn, in tenths of a percent.
    pub shown: u32,
}

impl Default for CutState {
    fn default() -> Self {
        Self {
            status: CutStatus::NoRush,
            transcript: None,
            analysis: None,
            choice: Vec::new(),
            analyzed: None,
            montage: true,
            progress: Arc::new(AtomicU32::new(0)),
            started: None,
            shown: 0,
        }
    }
}

impl CutState {
    pub fn transcription_progress(&self) -> f32 {
        f32::from_bits(self.progress.load(Ordering::Relaxed))
    }

    /// True when the progress moved enough to be drawn again.
    pub fn progress_moved(&mut self) -> bool {
        if self.status != CutStatus::Transcribing {
            return false;
        }
        // In [0, 1]: truncation intended.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let now = (self.transcription_progress() * 1000.0) as u32;
        let moved = now != self.shown;
        self.shown = now;
        moved
    }

    /// "Transcribing… 42 % · about 12 min left".
    pub fn transcribing_label(&self) -> SharedString {
        let done = self.transcription_progress();
        let pct = format!("{:.0}", done * 100.0);
        let left = self.started.and_then(|(at, from)| {
            let span = done - from;
            // Too early for an honest estimate.
            if span < 0.02 || done >= 1.0 {
                return None;
            }
            let elapsed = at.elapsed().as_secs_f32();
            let minutes = (elapsed * (1.0 - done) / span / 60.0).ceil().max(1.0);
            Some(format!("{minutes:.0}"))
        });
        match left {
            Some(minutes) => tr!("cut.transcribing_left", pct = pct, minutes = minutes),
            None => tr!("cut.transcribing_pct", pct = pct),
        }
    }

    /// Aligns the transcript on `script` (without a script, sentences and
    /// retakes are found in the take), applies the "last take" rule, then
    /// the saved choices.
    pub fn analyze(&mut self, script: &str, saved: &BTreeMap<String, usize>) {
        let Some(transcript) = &self.transcript else {
            return;
        };
        let analysis = if script.trim().is_empty() {
            enfait_speech::cleanup::analyze(transcript)
        } else {
            takes::analyze(script, transcript)
        };
        let mut choice = analysis.default_choice();
        for (s, slot) in choice.iter_mut().enumerate() {
            let rank = analysis
                .sentences
                .get(s)
                .and_then(|sentence| saved.get(&sentence.text));
            if let Some(take) = rank.and_then(|r| analysis.takes_of(s).nth(*r)) {
                *slot = Some(take.0);
            }
        }
        self.choice = choice;
        self.analysis = Some(analysis);
        self.analyzed = Some(script.to_owned());
    }

    /// The sentences were found in the take (no script).
    pub fn derived(&self) -> bool {
        self.analyzed
            .as_deref()
            .is_some_and(|s| s.trim().is_empty())
            && self.analysis.is_some()
    }

    /// Text of the analysed sentences, one per line: a script to correct.
    pub fn sentences_text(&self) -> String {
        self.analysis
            .as_ref()
            .map(|a| {
                a.sentences
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    }

    /// Pieces of the take to play, in edit order.
    pub fn ranges(&self) -> Vec<(f64, f64)> {
        let (Some(analysis), Some(transcript)) = (&self.analysis, &self.transcript) else {
            return Vec::new();
        };
        match takes::cut(analysis, transcript, &self.choice) {
            Ok(parts) => parts
                .iter()
                .map(|r| (r.start().as_secs_f64(), r.end().as_secs_f64()))
                .collect(),
            Err(e) => {
                tracing::warn!("edit: {e}");
                Vec::new()
            }
        }
    }

    /// Keeps take `take` for sentence `sentence` and updates the saved
    /// choices (only those departing from the rule stay there).
    pub fn choose(&mut self, sentence: usize, take: usize, saved: &mut BTreeMap<String, usize>) {
        let Some(analysis) = &self.analysis else {
            return;
        };
        let Some(text) = analysis.sentences.get(sentence).map(|s| s.text.clone()) else {
            return;
        };
        let Some(rank) = analysis.takes_of(sentence).position(|(i, _)| i == take) else {
            return;
        };
        let default = analysis.default_choice().get(sentence).copied().flatten();
        if default == Some(take) {
            saved.remove(&text);
        } else {
            saved.insert(text, rank);
        }
        if let Some(slot) = self.choice.get_mut(sentence) {
            *slot = Some(take);
        }
    }

    /// Back to the "last take" rule everywhere.
    pub fn reset(&mut self, saved: &mut BTreeMap<String, usize>) {
        saved.clear();
        if let Some(analysis) = &self.analysis {
            self.choice = analysis.default_choice();
        }
    }

    /// Start (in the source) of the take kept for a sentence.
    pub fn sentence_start(&self, sentence: usize) -> Option<f64> {
        let take = self.chosen(sentence)?;
        let word = self.transcript.as_ref()?.words.get(take.words.start)?;
        Some(word.range.start().as_secs_f64())
    }

    fn chosen(&self, sentence: usize) -> Option<&takes::Take> {
        let i = self.choice.get(sentence).copied().flatten()?;
        self.analysis.as_ref()?.takes.get(i)
    }

    /// Sentence whose kept take contains time `source`.
    pub fn sentence_at(&self, source: f64) -> Option<usize> {
        let words = &self.transcript.as_ref()?.words;
        (0..self.choice.len()).find(|&s| {
            self.chosen(s).is_some_and(|take| {
                let start = words.get(take.words.start);
                let end = words.get(take.words.end.saturating_sub(1));
                match (start, end) {
                    (Some(a), Some(b)) => {
                        source >= a.range.start().as_secs_f64() - 0.15
                            && source < b.range.end().as_secs_f64() + 0.2
                    }
                    _ => false,
                }
            })
        })
    }

    /// Summary: sentences found, takes discarded, durations.
    pub fn summary(&self, montage: f64) -> String {
        let Some(analysis) = &self.analysis else {
            return String::new();
        };
        let total = analysis.sentences.len();
        let found = self.choice.iter().filter(|c| c.is_some()).count();
        let discarded = analysis.takes.len().saturating_sub(found);
        let rush = self
            .transcript
            .as_ref()
            .map_or(0.0, |t| t.media_duration.as_secs_f64());
        crate::tr!(
            "cut.summary",
            found = found,
            total = total,
            discarded = discarded,
            montage = clock(montage),
            rush = clock(rush)
        )
        .to_string()
    }
}

/// `M:SS`.
pub fn clock(secs: f64) -> String {
    // Duration of a media: positive and bounded.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let total = secs.max(0.0).round() as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use enfait_core::{Flicks, TimeRange, Word};

    fn transcript(text: &str) -> Arc<Transcript> {
        let mut t = 0_i64;
        let words = text
            .split_whitespace()
            .map(|w| {
                let word = Word {
                    text: w.to_owned(),
                    range: TimeRange::new(
                        Flicks::from_millis(t).unwrap(),
                        Flicks::from_millis(t + 250).unwrap(),
                    )
                    .unwrap(),
                    confidence: 1.0,
                };
                t += 300;
                word
            })
            .collect();
        Arc::new(Transcript {
            language: "fr".into(),
            words,
            media_duration: Flicks::from_millis(t + 500).unwrap(),
        })
    }

    const SCRIPT: &str = "Un deux trois quatre. Cinq six sept huit.";

    #[test]
    fn manual_choice_is_saved_by_sentence_text() {
        let mut state = CutState {
            transcript: Some(transcript(
                "Un deux trois quatre. Un deux trois quatre. Cinq six sept huit.",
            )),
            ..CutState::default()
        };
        let mut saved = BTreeMap::new();
        state.analyze(SCRIPT, &saved);
        assert_eq!(state.choice, vec![Some(1), Some(2)], "last take");
        assert_eq!(
            state.ranges().len(),
            1,
            "2nd take + sentence 2: in one piece"
        );
        // Choose the first take of sentence 1.
        state.choose(0, 0, &mut saved);
        assert_eq!(saved.get("Un deux trois quatre."), Some(&0));
        assert_eq!(state.ranges().len(), 2);
        assert_eq!(state.sentence_start(0), Some(0.0));
        // The choice survives a new analysis (script changed elsewhere).
        let mut again = CutState {
            transcript: state.transcript.clone(),
            ..CutState::default()
        };
        again.analyze(&format!("{SCRIPT} Neuf."), &saved);
        assert_eq!(again.choice, vec![Some(0), Some(2), None]);
        // Going back to the rule clears the saved choice.
        state.choose(0, 1, &mut saved);
        assert!(saved.is_empty());
        state.reset(&mut saved);
        assert_eq!(state.choice, vec![Some(1), Some(2)]);
        assert_eq!(state.sentence_at(1.3), Some(0));
        assert!(
            state
                .summary(2.5)
                .starts_with("2/2 sentences · 1 take(s) discarded")
        );
    }
}
