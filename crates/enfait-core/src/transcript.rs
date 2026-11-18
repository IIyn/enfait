//! Transcription : les mots prononcés et leur position dans le temps.

use crate::time::{Flicks, TimeRange};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    /// Le mot tel que prononcé, ponctuation comprise, sans espace autour.
    pub text: String,
    pub range: TimeRange,
    /// Confiance du modèle, entre 0 et 1.
    pub confidence: f32,
}

impl Word {
    /// Le mot termine-t-il une phrase ?
    pub fn ends_sentence(&self) -> bool {
        self.text
            .trim_end_matches(['"', '»', ')', '’', '\''])
            .ends_with(['.', '!', '?', '…'])
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    /// Code de langue (`fr`, `en`…).
    pub language: String,
    /// Mots dans l'ordre, sans chevauchement.
    pub words: Vec<Word>,
    /// Durée totale du média transcrit.
    pub media_duration: Flicks,
}

impl Transcript {
    /// Mots qui commencent dans `range`.
    pub fn words_in(&self, range: TimeRange) -> impl Iterator<Item = &Word> {
        self.words
            .iter()
            .filter(move |w| range.contains(w.range.start()))
    }

    /// Texte de `range`, mots séparés par une espace.
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
