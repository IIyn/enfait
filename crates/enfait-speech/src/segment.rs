//! Découpage d'une transcription en blocs contigus.
//!
//! On coupe en fin de phrase dès que le bloc a une durée suffisante, ou sur
//! une pause ou une virgule quand il devient long. Chaque coupe tombe au
//! milieu du silence entre deux mots.

use crate::SpeechError;
use enfait_core::{Flicks, TimeRange, Transcript};

#[derive(Debug, Clone, Copy)]
pub struct SegmentParams {
    /// En dessous, on ne coupe pas même en fin de phrase.
    pub min: Flicks,
    /// Au-delà, on coupe à la première pause ou virgule.
    pub target_max: Flicks,
    /// Au-delà, on coupe au prochain mot quoi qu'il arrive.
    pub hard_max: Flicks,
    /// Silence qui compte comme une pause.
    pub pause: Flicks,
}

impl SegmentParams {
    pub fn standard() -> Result<Self, SpeechError> {
        Ok(Self {
            min: Flicks::from_millis(2_000)?,
            target_max: Flicks::from_millis(6_500)?,
            hard_max: Flicks::from_millis(10_000)?,
            pause: Flicks::from_millis(450)?,
        })
    }
}

fn midpoint(a: Flicks, b: Flicks) -> Flicks {
    Flicks::new(a.get().saturating_add(b.get().saturating_sub(a.get()) / 2))
}

/// Découpe `[0, durée)` en blocs contigus.
pub fn segment(
    transcript: &Transcript,
    params: &SegmentParams,
) -> Result<Vec<TimeRange>, SpeechError> {
    let end_of_media = transcript
        .words
        .last()
        .map_or(transcript.media_duration, |w| {
            w.range.end().max(transcript.media_duration)
        });
    let mut cuts: Vec<Flicks> = Vec::new();
    let mut block_start = Flicks::ZERO;
    for pair in transcript.words.windows(2) {
        let [word, next] = pair else { continue };
        let cut = midpoint(word.range.end(), next.range.start());
        let elapsed = cut.checked_sub(block_start)?;
        let gap = next.range.start().checked_sub(word.range.end())?;
        let comma = word.text.ends_with([',', ';', ':']);
        let should_cut = elapsed >= params.min
            && (word.ends_sentence()
                || (elapsed >= params.target_max && (gap >= params.pause || comma))
                || elapsed >= params.hard_max);
        if should_cut {
            cuts.push(cut);
            block_start = cut;
        }
    }
    // Un dernier bloc trop court rejoint le précédent.
    if let Some(&last) = cuts.last()
        && end_of_media.checked_sub(last)?.get() < params.min.get().saturating_mul(3) / 5
    {
        cuts.pop();
    }
    let mut ranges = Vec::with_capacity(cuts.len().saturating_add(1));
    let mut start = Flicks::ZERO;
    for cut in cuts {
        if cut > start {
            ranges.push(TimeRange::new(start, cut)?);
            start = cut;
        }
    }
    if end_of_media > start {
        ranges.push(TimeRange::new(start, end_of_media)?);
    }
    Ok(ranges)
}

#[cfg(test)]
mod tests {
    use super::*;
    use enfait_core::Word;

    fn transcript(words: &[(&str, i64, i64)], total_ms: i64) -> Transcript {
        Transcript {
            language: "fr".into(),
            words: words
                .iter()
                .map(|(t, s, e)| Word {
                    text: (*t).to_owned(),
                    range: TimeRange::new(
                        Flicks::from_millis(*s).unwrap(),
                        Flicks::from_millis(*e).unwrap(),
                    )
                    .unwrap(),
                    confidence: 1.0,
                })
                .collect(),
            media_duration: Flicks::from_millis(total_ms).unwrap(),
        }
    }

    #[test]
    fn cuts_on_sentences_once_long_enough() {
        let t = transcript(
            &[
                ("Salut.", 0, 400), // trop tôt pour couper
                ("Aujourd'hui", 600, 1200),
                ("on", 1250, 1400),
                ("parle", 1450, 2000),
                ("vidéo.", 2050, 2600), // coupe ici (2,6 s écoulées)
                ("C'est", 3000, 3300),
                ("parti", 3350, 3800),
                ("maintenant.", 3850, 5200),
            ],
            5600,
        );
        let blocks = segment(&t, &SegmentParams::standard().unwrap()).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].start(), Flicks::ZERO);
        assert_eq!(blocks[0].end(), Flicks::from_millis(2800).unwrap());
        assert_eq!(blocks[1].end(), Flicks::from_millis(5600).unwrap());
    }

    #[test]
    fn long_run_on_sentence_is_split_on_pause() {
        let mut words = Vec::new();
        let mut t = 0;
        for i in 0..30 {
            let gap = if i == 20 { 600 } else { 50 };
            words.push(("mot", t, t + 300));
            t += 300 + gap;
        }
        let tr = transcript(&words, t);
        let blocks = segment(&tr, &SegmentParams::standard().unwrap()).unwrap();
        assert!(blocks.len() >= 2);
        for pair in blocks.windows(2) {
            assert_eq!(pair[0].end(), pair[1].start());
        }
        for b in &blocks {
            assert!(b.duration() <= Flicks::from_millis(10_400).unwrap());
        }
    }

    #[test]
    fn short_tail_merges_into_previous_block() {
        let t = transcript(
            &[
                ("Une", 0, 500),
                ("phrase", 550, 1500),
                ("complète.", 1550, 2500),
                ("Fin.", 2700, 3000),
            ],
            3100,
        );
        let blocks = segment(&t, &SegmentParams::standard().unwrap()).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].end(), Flicks::from_millis(3100).unwrap());
    }
}
