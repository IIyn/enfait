//! Transcript: the spoken words and their position in time.

use crate::time::{Flicks, TimeRange};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    /// The word as spoken, punctuation included, no surrounding space.
    pub text: String,
    pub range: TimeRange,
    /// Confidence of the model, from 0 to 1.
    pub confidence: f32,
}

impl Word {
    /// Does the word end a sentence?
    pub fn ends_sentence(&self) -> bool {
        self.text
            .trim_end_matches(['"', '»', ')', '’', '\''])
            .ends_with(['.', '!', '?', '…'])
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    /// Language code (`fr`, `en`…).
    pub language: String,
    /// Words in order, without overlap.
    pub words: Vec<Word>,
    /// Total duration of the transcribed media.
    pub media_duration: Flicks,
}

impl Transcript {
    /// Words starting in `range` (binary search: the words are in order).
    pub fn words_in(&self, range: TimeRange) -> impl Iterator<Item = &Word> {
        let from = self
            .words
            .partition_point(|w| w.range.start() < range.start());
        self.words
            .get(from..)
            .unwrap_or_default()
            .iter()
            .take_while(move |w| w.range.start() < range.end())
    }

    /// Text of `range`, words separated by a space.
    pub fn text_in(&self, range: TimeRange) -> String {
        self.words_in(range)
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, start_ms: i64, end_ms: i64) -> Word {
        Word {
            text: text.to_owned(),
            range: TimeRange::new(
                Flicks::from_millis(start_ms).unwrap(),
                Flicks::from_millis(end_ms).unwrap(),
            )
            .unwrap(),
            confidence: 1.0,
        }
    }

    #[test]
    fn sentence_end_detection() {
        assert!(word("fin.", 0, 1).ends_sentence());
        assert!(word("vraiment ?", 0, 1).ends_sentence());
        assert!(word("oui.»", 0, 1).ends_sentence());
        assert!(!word("donc,", 0, 1).ends_sentence());
    }

    #[test]
    fn text_in_range() {
        let t = Transcript {
            language: "fr".into(),
            words: vec![
                word("Salut", 0, 400),
                word("à", 450, 500),
                word("tous.", 520, 900),
            ],
            media_duration: Flicks::from_millis(1000).unwrap(),
        };
        let r = TimeRange::new(
            Flicks::from_millis(440).unwrap(),
            Flicks::from_millis(1000).unwrap(),
        )
        .unwrap();
        assert_eq!(t.text_in(r), "à tous.");
    }
}
