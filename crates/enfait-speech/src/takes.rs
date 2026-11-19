//! Takes: aligning a transcribed rush on the script, and the "last take"
//! rule.
//!
//! The rush is read left to right. At each word, look for a sentence of the
//! script starting there (its first two words, one typo allowed) and align
//! it on the rest of the transcript (local alignment: identical or close
//! words, skipped words, added words). If the same sentence starts again
//! before the end of the alignment, it is a false start: the take stops
//! there and the next one begins. Words matching no sentence (asides,
//! "um") belong to no take.
//!
//! For each sentence, the last complete take is kept (or, failing that,
//! the most complete one); the user can pick another one. The edit puts
//! the sentences in script order, keeping in one piece the sentences said
//! in a row.

use crate::script::{Sentence, sentences, tokens};
use enfait_core::{Flicks, TimeError, TimeRange, Transcript};
use std::ops::Range;

/// Margin kept before the first word of a take (seconds).
const PAD_BEFORE: f64 = 0.12;
/// Margin kept before a take whose first word was not recognised.
const PAD_MISSING_START: f64 = 0.9;
/// Margin kept after the last word.
const PAD_AFTER: f64 = 0.25;
/// Beyond this silence, two sentences said in a row are cut apart.
const MAX_JOINED_GAP: f64 = 1.5;
/// Silence after which a sentence starting again is a retake.
const RESTART_PAUSE: f64 = 0.5;
/// Share of the words of a sentence a take must contain to be complete.
const COMPLETE: f32 = 0.8;

/// A take of a sentence.
#[derive(Debug, Clone, PartialEq)]
pub struct Take {
    /// Index of the sentence in the script.
    pub sentence: usize,
    /// Words of the transcript, from the first to the last recognised word.
    pub words: Range<usize>,
    /// Share of the sentence's words found, from 0 to 1.
    pub coverage: f32,
    /// Enough words, up to the end of the sentence.
    pub complete: bool,
    /// The first word of the sentence is missing (misheard or swallowed): it
    /// is probably just before, the edit keeps more margin.
    pub missing_start: bool,
}

#[derive(Debug, Clone)]
pub struct Analysis {
    pub sentences: Vec<Sentence>,
    /// Takes in rush order.
    pub takes: Vec<Take>,
}

impl Analysis {
    /// Takes of a sentence: (index in [`Analysis::takes`], take).
    pub fn takes_of(&self, sentence: usize) -> impl Iterator<Item = (usize, &Take)> {
        self.takes
            .iter()
            .enumerate()
            .filter(move |(_, t)| t.sentence == sentence)
    }

    /// "Last take" rule: for each sentence, the last complete take, else the
    /// most complete (the last one on a tie), else nothing.
    pub fn default_choice(&self) -> Vec<Option<usize>> {
        (0..self.sentences.len())
            .map(|s| {
                let takes: Vec<(usize, &Take)> = self.takes_of(s).collect();
                takes
                    .iter()
                    .rev()
                    .find(|(_, t)| t.complete)
                    .or_else(|| {
                        takes.iter().rev().max_by(|a, b| {
                            a.1.coverage
                                .partial_cmp(&b.1.coverage)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                    })
                    .map(|(i, _)| *i)
            })
            .collect()
    }
}

/// A normalised word of the transcript and the word it comes from.
struct Token {
    text: String,
    word: usize,
    /// Silence before this token (0 inside a word), in seconds.
    pause: f64,
}

/// Alignment of a sentence from a position of the rush.
#[derive(Debug, Clone, Copy)]
struct Alignment {
    score: i32,
    matches: usize,
    /// First and last recognised tokens of the rush.
    first: usize,
    last: usize,
    /// First and last recognised words of the sentence.
    first_script: usize,
    last_script: usize,
}

pub fn analyze(script: &str, transcript: &Transcript) -> Analysis {
    let sentences = sentences(script);
    let words = &transcript.words;
    let rush: Vec<Token> = words
        .iter()
        .enumerate()
        .flat_map(|(i, w)| {
            let pause = i
                .checked_sub(1)
                .and_then(|p| words.get(p))
                .map_or(0.0, |prev| {
                    (w.range.start().as_secs_f64() - prev.range.end().as_secs_f64()).max(0.0)
                });
            tokens(&w.text)
                .into_iter()
                .enumerate()
                .map(move |(k, text)| Token {
                    text,
                    word: i,
                    pause: if k == 0 { pause } else { 0.0 },
                })
        })
        .collect();
    let mut takes = Vec::new();
    let mut p = 0;
    while p < rush.len() {
        let Some((sentence, mut found)) = best_start(&sentences, &rush, p, rush.len()) else {
            p = p.saturating_add(1);
            continue;
        };
        let Some(script) = sentences.get(sentence).map(|s| s.tokens.as_slice()) else {
            break;
        };
        // False start: the same sentence starts again before the end of the
        // take, and the retake goes at least as far.
        let restart = ((found.first.saturating_add(1))..=found.last).find(|&q| {
            // The retake may have lost its first word (misheard); it must go back
            // in the sentence (more than `j0` words of the take already said
            // before it) or follow a silence.
            let after_pause = rush.get(q).is_some_and(|t| t.pause >= RESTART_PAUSE);
            [0, 1].into_iter().any(|j0| {
                (q.saturating_sub(found.first) > j0 || after_pause)
                    && starts(script, &rush, q, j0)
                    && align(script, &rush, q, j0, rush.len()).is_some_and(|again| {
                        again.matches.saturating_add(j0) >= found.matches.min(script.len())
                    })
            })
        });
        let next = match restart {
            Some(q) => {
                match align(script, &rush, p, start_offset(script, &rush, p), q) {
                    Some(cut) => found = cut,
                    None => found.matches = 0,
                }
                q
            }
            None => found.last.saturating_add(1),
        };
        if found.matches >= script.len().min(2) {
            #[allow(clippy::cast_precision_loss)]
            let coverage = (found.matches as f32 / script.len().max(1) as f32).min(1.0);
            let complete = coverage >= COMPLETE
                && found.last_script.saturating_add(2) >= script.len()
                && (script.len() > 2 || found.matches == script.len());
            let first = rush.get(found.first).map_or(0, |t| t.word);
            let last = rush.get(found.last).map_or(first, |t| t.word);
            takes.push(Take {
                sentence,
                words: first..last.saturating_add(1),
                coverage,
                complete,
                missing_start: found.first_script > 0,
            });
        }
        p = next.max(p.saturating_add(1));
    }
    Analysis { sentences, takes }
}

/// Index of the first word of `script` recognised at `p` (0, or 1 if the
/// first word of the sentence was swallowed).
fn start_offset(script: &[String], rush: &[Token], p: usize) -> usize {
    usize::from(!starts(script, rush, p, 0))
}

/// Does the sentence start (from its word `j0`) at token `p`?
fn starts(script: &[String], rush: &[Token], p: usize, j0: usize) -> bool {
    let word = |i: usize| rush.get(i).map(|t| t.text.as_str());
    let (Some(first), Some(here)) = (script.get(j0), word(p)) else {
        return false;
    };
    if !similar(first, here) {
        return false;
    }
    match script.get(j0.saturating_add(1)) {
        // The second word follows, possibly after a filler word.
        Some(second) => [1_usize, 2]
            .iter()
            .any(|d| word(p.saturating_add(*d)).is_some_and(|w| similar(second, w))),
        None => true,
    }
}

/// Best sentence starting at `p`.
fn best_start(
    sentences: &[Sentence],
    rush: &[Token],
    p: usize,
    end: usize,
) -> Option<(usize, Alignment)> {
    let mut best: Option<(usize, Alignment)> = None;
    for (i, s) in sentences.iter().enumerate() {
        for j0 in [0, 1] {
            if !starts(&s.tokens, rush, p, j0) {
                continue;
            }
            let Some(a) = align(&s.tokens, rush, p, j0, end) else {
                continue;
            };
            let better = best.is_none_or(|(_, b)| (a.score, a.matches) > (b.score, b.matches));
            if better {
                best = Some((i, a));
            }
        }
    }
    best
}

#[derive(Debug, Clone, Copy, Default)]
struct Cell {
    score: i32,
    matches: usize,
    /// (sentence word, rush token) of the first and last match.
    first: Option<(usize, usize)>,
    last: Option<(usize, usize)>,
}

/// Local alignment of `script[j0..]` on `rush[p..end]`, anchored at the start.
fn align(script: &[String], rush: &[Token], p: usize, j0: usize, end: usize) -> Option<Alignment> {
    let s = script.get(j0..)?;
    let m = s.len();
    let n = end
        .min(rush.len())
        .saturating_sub(p)
        .min(m.saturating_mul(3) / 2 + 4);
    if m == 0 || n == 0 {
        return None;
    }
    let width = n.saturating_add(1);
    let mut dp = vec![Cell::default(); m.saturating_add(1).saturating_mul(width)];
    let at = |j: usize, k: usize| j.saturating_mul(width).saturating_add(k);
    // Anchoring: the take starts at token `p`, without skipping a rush word
    // before the first word of the sentence.
    for k in 1..=n {
        if let Some(c) = dp.get_mut(at(0, k)) {
            c.score = i32::MIN / 2;
        }
    }
    let mut best = Cell::default();
    for j in 1..=m {
        if let Some(c) = dp.get_mut(at(j, 0)) {
            c.score = -i32::try_from(j).unwrap_or(i32::MAX);
        }
        for k in 1..=n {
            let get = |jj: usize, kk: usize| dp.get(at(jj, kk)).copied().unwrap_or_default();
            let hit = match (s.get(j.wrapping_sub(1)), rush.get(p.saturating_add(k) - 1)) {
                (Some(a), Some(b)) => similar(a, &b.text),
                _ => false,
            };
            let diag = get(j - 1, k - 1);
            let mut cell = if hit {
                Cell {
                    score: diag.score.saturating_add(2),
                    matches: diag.matches.saturating_add(1),
                    first: diag.first.or(Some((j - 1, k - 1))),
                    last: Some((j - 1, k - 1)),
                }
            } else {
                Cell {
                    score: diag.score.saturating_sub(1),
                    ..diag
                }
            };
            for other in [get(j - 1, k), get(j, k - 1)] {
                if other.score.saturating_sub(1) > cell.score {
                    cell = Cell {
                        score: other.score.saturating_sub(1),
                        ..other
                    };
                }
            }
            if let Some(c) = dp.get_mut(at(j, k)) {
                *c = cell;
            }
            if cell.last.is_some() && (cell.score, cell.matches) > (best.score, best.matches) {
                best = cell;
            }
        }
    }
    let ((first_j, first_k), (last_j, last_k)) = (best.first?, best.last?);
    Some(Alignment {
        score: best.score,
        matches: best.matches,
        first: p.saturating_add(first_k),
        last: p.saturating_add(last_k),
        first_script: j0.saturating_add(first_j),
        last_script: j0.saturating_add(last_j),
    })
}

/// Identical words, or one typo apart (two for long words).
pub(crate) fn similar(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let shortest = a.chars().count().min(b.chars().count());
    let allowed = match shortest {
        0..=3 => return false,
        4..=7 => 1,
        _ => 2,
    };
    levenshtein(a, b) <= allowed
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row.first().copied().unwrap_or(0);
        if let Some(first) = row.first_mut() {
            *first = i.saturating_add(1);
        }
        for (j, cb) in b.iter().enumerate() {
            let above = row.get(j.saturating_add(1)).copied().unwrap_or(0);
            let left = row.get(j).copied().unwrap_or(0);
            let cost = previous.saturating_add(usize::from(ca != *cb));
            let value = cost
                .min(above.saturating_add(1))
                .min(left.saturating_add(1));
            previous = above;
            if let Some(cell) = row.get_mut(j.saturating_add(1)) {
                *cell = value;
            }
        }
    }
    row.last().copied().unwrap_or(0)
}

/// Parts of the rush to keep, in edit order. `choice`: take kept for each
/// sentence (index in [`Analysis::takes`]).
pub fn cut(
    analysis: &Analysis,
    transcript: &Transcript,
    choice: &[Option<usize>],
) -> Result<Vec<TimeRange>, TimeError> {
    let words = &transcript.words;
    let start_of = |i: usize| words.get(i).map(|w| w.range.start().as_secs_f64());
    let end_of = |i: usize| words.get(i).map(|w| w.range.end().as_secs_f64());
    let total = transcript.media_duration.as_secs_f64();
    // (first word, last word, missing sentence start) of each part.
    let mut parts: Vec<(usize, usize, bool)> = Vec::new();
    for take in choice
        .iter()
        .flatten()
        .filter_map(|&i| analysis.takes.get(i))
    {
        let (first, last) = (take.words.start, take.words.end.saturating_sub(1));
        if let Some(previous) = parts.last_mut()
            && previous.1.saturating_add(1) == first
            && !take.missing_start
            && start_of(first)
                .zip(end_of(previous.1))
                .is_some_and(|(s, e)| s - e <= MAX_JOINED_GAP)
        {
            previous.1 = last;
            continue;
        }
        parts.push((first, last, take.missing_start));
    }
    parts
        .into_iter()
        .filter_map(|(first, last, missing_start)| {
            let start = start_of(first)?;
            let end = end_of(last)?;
            // Margins cut down to half of the neighbouring silence, never to take
            // a bit of a word that is not in the take. If the first word is
            // missing, go back almost to the previous word.
            let previous = first.checked_sub(1).and_then(end_of);
            let before = if missing_start {
                previous.map_or(PAD_MISSING_START, |e| {
                    (start - e - 0.05).clamp(0.0, PAD_MISSING_START)
                })
            } else {
                previous.map_or(PAD_BEFORE, |e| ((start - e) / 2.0).clamp(0.0, PAD_BEFORE))
            };
            let after = start_of(last.saturating_add(1))
                .map_or(PAD_AFTER, |s| ((s - end) / 2.0).clamp(0.0, PAD_AFTER));
            let a = (start - before).max(0.0);
            let b = if total > 0.0 {
                (end + after).min(total)
            } else {
                end + after
            };
            Some((a, b))
        })
        .map(|(a, b)| TimeRange::new(secs(a)?, secs(b)?))
        .collect()
}

/// Transcript of the edit: the words of the kept pieces (in order),
/// shifted onto edit time.
pub fn montage_transcript(
    transcript: &Transcript,
    parts: &[TimeRange],
) -> Result<Transcript, TimeError> {
    let mut words = Vec::new();
    let mut offset = Flicks::ZERO;
    for part in parts {
        for word in transcript.words_in(*part) {
            let start = offset.checked_add(word.range.start().checked_sub(part.start())?)?;
            let end =
                offset.checked_add(word.range.end().min(part.end()).checked_sub(part.start())?)?;
            let end = if end > start {
                end
            } else {
                start.checked_add(Flicks::from_millis(10)?)?
            };
            words.push(enfait_core::Word {
                text: word.text.clone(),
                range: TimeRange::new(start, end)?,
                confidence: word.confidence,
            });
        }
        offset = offset.checked_add(part.duration())?;
    }
    Ok(Transcript {
        language: transcript.language.clone(),
        words,
        media_duration: offset,
    })
}

fn secs(t: f64) -> Result<Flicks, TimeError> {
    // Time of a media: positive and bounded.
    #[allow(clippy::cast_possible_truncation)]
    let millis = (t * 1000.0).round() as i64;
    Flicks::from_millis(millis)
}

#[cfg(test)]
mod tests {
    use super::*;
    use enfait_core::Word;

    /// Transcript: words separated by spaces, `|` marks a one-second pause.
    /// 0.3 s per word.
    fn rush(text: &str) -> Transcript {
        let mut t = 0.0;
        let mut words = Vec::new();
        for w in text.split_whitespace() {
            if w == "|" {
                t += 1.0;
                continue;
            }
            words.push(Word {
                text: w.to_owned(),
                range: TimeRange::new(secs(t).unwrap(), secs(t + 0.25).unwrap()).unwrap(),
                confidence: 1.0,
            });
            t += 0.3;
        }
        Transcript {
            language: "fr".into(),
            words,
            media_duration: secs(t + 1.0).unwrap(),
        }
    }

    fn chosen(a: &Analysis, t: &Transcript) -> Vec<Option<String>> {
        a.default_choice()
            .iter()
            .map(|c| {
                c.and_then(|i| a.takes.get(i)).map(|take| {
                    t.words[take.words.clone()]
                        .iter()
                        .map(|w| w.text.as_str())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
            })
            .collect()
    }

    const SCRIPT: &str = "Le scrutin se déroule en deux tours. \
        Les deux premiers s'affrontent ensuite. Celui qui recueille le plus de voix l'emporte.";

    #[test]
    fn single_clean_read_is_one_part() {
        let t = rush(
            "Le scrutin se déroule en deux tours. | Les deux premiers s'affrontent ensuite. | \
             Celui qui recueille le plus de voix l'emporte.",
        );
        let a = analyze(SCRIPT, &t);
        assert_eq!(a.takes.len(), 3);
        assert!(a.takes.iter().all(|t| t.complete));
        let parts = cut(&a, &t, &a.default_choice()).unwrap();
        assert_eq!(parts.len(), 1, "sentences said in a row: one piece");
    }

    #[test]
    fn false_start_then_retake() {
        let t = rush(
            "Le scrutin se déroule en deux tours. | Les deux premiers euh | \
             Les deux premiers s'affrontent ensuite. | Celui qui recueille le plus de voix l'emporte.",
        );
        let a = analyze(SCRIPT, &t);
        let takes: Vec<_> = a.takes_of(1).collect();
        assert_eq!(takes.len(), 2);
        assert!(!takes[0].1.complete && takes[1].1.complete);
        assert_eq!(
            chosen(&a, &t)[1].as_deref(),
            Some("Les deux premiers s'affrontent ensuite.")
        );
        let parts = cut(&a, &t, &a.default_choice()).unwrap();
        assert_eq!(parts.len(), 2, "the false start is removed");
        // The "euh" is in no take.
        assert!(
            a.takes
                .iter()
                .all(|take| !t.words[take.words.clone()].iter().any(|w| w.text == "euh"))
        );
    }

    #[test]
    fn retake_missing_its_first_word_is_still_a_retake() {
        // False start, then a retake whose first word whisper lost.
        let t = rush(
            "Les deux premiers | deux premiers s'affrontent ensuite. | \
             Celui qui recueille le plus de voix l'emporte.",
        );
        let a = analyze(SCRIPT, &t);
        let second: Vec<_> = a.takes_of(1).collect();
        assert_eq!(second.len(), 2, "{:?}", a.takes);
        assert_eq!(t.words[second[1].1.words.start].text, "deux");
        assert!(second[1].1.complete);
        assert_eq!(a.default_choice()[1], Some(second[1].0));
    }

    #[test]
    fn complete_retake_wins_and_stutter_is_trimmed() {
        let t = rush(
            "Le le scrutin se déroule en deux tours. | Le scrutin se déroule en deux tours. | \
             Les deux premiers s'affrontent ensuite.",
        );
        let a = analyze(SCRIPT, &t);
        let first: Vec<_> = a.takes_of(0).collect();
        assert_eq!(first.len(), 2);
        assert_eq!(
            t.words[first[0].1.words.start].text, "le",
            "stutter removed"
        );
        assert_eq!(first[0].1.words.start, 1);
        assert_eq!(a.default_choice()[0], Some(first[1].0), "the last one wins");
        assert_eq!(a.default_choice()[2], None, "sentence never said");
    }

    #[test]
    fn redone_sentence_keeps_script_order() {
        // 1, 2, 3 in one go, then 2 redone at the end.
        let t = rush(
            "Le scrutin se déroule en deux tours. Les deux premiers s'affrontent ensuite. \
             Celui qui recueille le plus de voix l'emporte. | \
             Les deux premiers s'affrontent ensuite.",
        );
        let a = analyze(SCRIPT, &t);
        let choice = a.default_choice();
        let parts = cut(&a, &t, &choice).unwrap();
        assert_eq!(parts.len(), 3, "1, then 2 redone, then 3");
        assert!(
            parts[1].start() > parts[2].start(),
            "2 comes from the end of the rush"
        );
        // Choosing by hand the first take of 2 glues everything back.
        let mut manual = choice.clone();
        manual[1] = a.takes_of(1).next().map(|(i, _)| i);
        assert_eq!(cut(&a, &t, &manual).unwrap().len(), 1);
    }

    #[test]
    fn whisper_variants_still_match() {
        let script = "L'inflation, c'est la hausse générale et durable des prix.";
        let t = rush("Linflation c'est la hausse générales et durable des prix");
        let a = analyze(script, &t);
        assert_eq!(a.takes.len(), 1);
        assert!(a.takes[0].complete, "{:?}", a.takes[0]);
    }

    #[test]
    fn margins_stay_between_neighbours() {
        let t = rush("Le scrutin se déroule en deux tours. Bon.");
        let a = analyze(SCRIPT, &t);
        let parts = cut(&a, &t, &a.default_choice()).unwrap();
        let end = parts[0].end().as_secs_f64();
        // Last word: 1.8 → 2.05 s; "Bon." starts at 2.1 s.
        assert!(end > 2.05 && end <= 2.075 + 1e-9, "{end}");
    }

    #[test]
    fn montage_transcript_follows_the_parts() {
        let t = rush(
            "Le scrutin se déroule en deux tours. | Les deux premiers euh | \
             Les deux premiers s'affrontent ensuite.",
        );
        let a = analyze(SCRIPT, &t);
        let parts = cut(&a, &t, &a.default_choice()).unwrap();
        let m = montage_transcript(&t, &parts).unwrap();
        let text: Vec<&str> = m.words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(
            text.join(" "),
            "Le scrutin se déroule en deux tours. Les deux premiers s'affrontent ensuite."
        );
        let total: f64 = parts.iter().map(|p| p.duration().as_secs_f64()).sum();
        assert!((m.media_duration.as_secs_f64() - total).abs() < 1e-6);
        assert!(
            m.words
                .windows(2)
                .all(|w| w[0].range.end() <= w[1].range.start())
        );
        // The first word of the 2nd part starts right after the margin.
        let second = &m.words[7];
        assert!(
            (second.range.start().as_secs_f64() - (parts[0].duration().as_secs_f64() + 0.12)).abs()
                < 0.01
        );
    }

    #[test]
    fn similarity() {
        assert!(similar("generale", "generales"));
        assert!(similar("affrontent", "affronte"));
        assert!(!similar("les", "des"));
        assert_eq!(levenshtein("chat", "chats"), 1);
        assert_eq!(levenshtein("", "abc"), 3);
    }
}
