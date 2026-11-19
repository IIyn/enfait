//! Cleaning up a take without a script (magic mode, or improvised voice):
//! sentences and their retakes are found in the transcript.
//!
//! The transcript is cut into utterances (sentence ends, long silences).
//! An utterance that says a recent one again almost word for word is a
//! retake, as is one that takes its start again after a silence and ends
//! differently; an utterance stopping midway that the next one takes from
//! the start is a false start. Each group becomes a sentence, each of its
//! readings a take: the resulting [`Analysis`] is that of a written script,
//! the "last take" rule and the manual choices apply as they are. Asides,
//! which look like nothing else, stay sentences of their own.

use crate::script::{Sentence, tokens};
use crate::takes::{Analysis, Take, similar};
use enfait_core::Transcript;

/// Silence that cuts an utterance even without punctuation (seconds).
/// Shorter, it would cut a sentence read with breaths into pieces that no
/// longer look like the sentence.
const SPLIT_GAP: f64 = 1.5;
/// Silence marking a hesitation before an utterance (seconds).
const PAUSE: f64 = 0.6;
/// Share of common words beyond which two utterances are two readings of
/// the same sentence.
const RETAKE_SIMILARITY: f32 = 0.85;
/// Rephrased retake: same start (at least as many words), after a
/// silence, and at least this share of common words.
const REPHRASED_PREFIX: usize = 3;
const REPHRASED_SIMILARITY: f32 = 0.6;
/// Recent groups compared to a new utterance: one retakes a sentence right
/// after missing it, not three minutes later.
const RECENT_GROUPS: usize = 3;
/// Beyond this delay since the last reading of a sentence, a similar
/// utterance is a new sentence (a chorus, a recap).
const RECENT_SECONDS: f64 = 45.0;
/// Maximum length (in words) of a false start whose retake lost its first
/// word.
const MAX_FRAGMENT: usize = 3;
/// Share of the sentence a reading must cover to be complete.
const COMPLETE: f32 = 0.8;
/// Hesitations removed from the found sentences.
const FILLERS: [&str; 9] = ["euh", "heu", "euhh", "hum", "hmm", "um", "uh", "erm", "bah"];

/// An utterance: consecutive words of the transcript.
#[derive(Debug, Clone, Default)]
struct Utterance {
    /// Kept words (index in the transcript), without hesitations.
    words: Vec<usize>,
    /// Number of tokens of each kept word (a word gives one or more:
    /// `l'oxygène` → `l`, `oxygene`).
    counts: Vec<usize>,
    /// Normalised words.
    tokens: Vec<String>,
    /// The last word is followed by no punctuation (or by an ellipsis, a
    /// dash): the sentence may have been cut mid-word.
    open_end: bool,
    /// A silence precedes the utterance.
    after_pause: bool,
    /// End of the last word (seconds).
    end: f64,
}

/// The readings of one sentence.
struct Group {
    /// Index (in `readings`) of the reference reading: the most complete, the
    /// most recent on equal length.
    best: usize,
    readings: Vec<Utterance>,
}

impl Group {
    fn reference(&self) -> Option<&Utterance> {
        self.readings.get(self.best)
    }

    fn last_end(&self) -> f64 {
        self.readings.last().map_or(0.0, |u| u.end)
    }
}

/// Sentences and takes of a take without a script, as if it had been
/// aligned on a script.
pub fn analyze(transcript: &Transcript) -> Analysis {
    let groups = groups(transcript);
    let mut sentences = Vec::with_capacity(groups.len());
    let mut takes = Vec::new();
    for group in &groups {
        let Some(reference) = group.reference() else {
            continue;
        };
        let text = sentence_text(transcript, reference);
        if text.is_empty() {
            continue;
        }
        let index = sentences.len();
        for reading in &group.readings {
            let (Some(&first), Some(&last)) = (reading.words.first(), reading.words.last()) else {
                continue;
            };
            let common = lcs(&reading.tokens, &reference.tokens);
            #[allow(clippy::cast_precision_loss)]
            let coverage = (common as f32 / reference.tokens.len().max(1) as f32).min(1.0);
            takes.push(Take {
                sentence: index,
                words: first..last.saturating_add(1),
                coverage,
                complete: coverage >= COMPLETE,
                missing_start: false,
            });
        }
        sentences.push(Sentence {
            text,
            tokens: reference.tokens.clone(),
        });
    }
    // Takes in the order of the take, as for a written script.
    takes.sort_by_key(|t| t.words.start);
    Analysis { sentences, takes }
}

/// Script found in the transcript: one sentence per line, in the order
/// they are first said. Empty if there is no word. A starting point for a
/// script corrected by hand.
pub fn derived_script(transcript: &Transcript) -> String {
    analyze(transcript)
        .sentences
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Groups the utterances into readings of the same sentence.
fn groups(transcript: &Transcript) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for utterance in utterances(transcript).into_iter().flat_map(split_restarts) {
        if utterance.tokens.is_empty() {
            continue;
        }
        let start = groups.len().saturating_sub(RECENT_GROUPS);
        let found = groups.get_mut(start..).and_then(|recent| {
            recent.iter_mut().rev().find(|g| {
                utterance.end - g.last_end() <= RECENT_SECONDS
                    && g.reference().is_some_and(|r| same_sentence(r, &utterance))
            })
        });
        match found {
            Some(group) => {
                let longer = group
                    .reference()
                    .is_none_or(|r| utterance.tokens.len() >= r.tokens.len());
                group.readings.push(utterance);
                if longer {
                    group.best = group.readings.len().saturating_sub(1);
                }
            }
            None => groups.push(Group {
                best: 0,
                readings: vec![utterance],
            }),
        }
    }
    groups
}

/// Cuts the transcript into utterances: after a sentence end, or at a
/// long silence.
fn utterances(transcript: &Transcript) -> Vec<Utterance> {
    let mut out = Vec::new();
    let mut current = Utterance::default();
    let mut previous_end: Option<f64> = None;
    for (i, word) in transcript.words.iter().enumerate() {
        let start = word.range.start().as_secs_f64();
        let gap = previous_end.map_or(f64::INFINITY, |e| start - e);
        if gap >= SPLIT_GAP {
            push_utterance(&mut out, &mut current);
        }
        previous_end = Some(word.range.end().as_secs_f64());
        let text = word.text.trim();
        // Noises written by whisper: "[Musique]", "(rires)".
        if text.starts_with(['[', '(']) {
            continue;
        }
        let toks: Vec<String> = tokens(text)
            .into_iter()
            .filter(|t| !FILLERS.contains(&t.as_str()))
            .collect();
        if toks.is_empty() {
            continue;
        }
        if current.words.is_empty() {
            current.after_pause = gap >= PAUSE;
        }
        current.words.push(i);
        current.counts.push(toks.len());
        current.tokens.extend(toks);
        current.open_end = open_end(text);
        current.end = word.range.end().as_secs_f64();
        if word.ends_sentence() && !text.ends_with("...") && !text.ends_with('…') {
            push_utterance(&mut out, &mut current);
        }
    }
    push_utterance(&mut out, &mut current);
    out
}

fn push_utterance(out: &mut Vec<Utterance>, current: &mut Utterance) {
    let done = std::mem::take(current);
    if !done.words.is_empty() {
        out.push(done);
    }
}

/// The word may have been cut: no punctuation after it, or an ellipsis.
fn open_end(text: &str) -> bool {
    let last = text.chars().last();
    text.ends_with("...")
        || matches!(last, Some('…' | '-' | '—' | '–'))
        || last.is_some_and(char::is_alphanumeric)
}

/// An utterance starting its own beginning again ("Le projet, le projet
/// est né") contains a false start: it is cut there, grouping does the
/// rest.
fn split_restarts(utterance: Utterance) -> Vec<Utterance> {
    let (Some(a), Some(b)) = (utterance.tokens.first(), utterance.tokens.get(1)) else {
        return vec![utterance];
    };
    // First token of each word, to cut between two words.
    let mut firsts = Vec::with_capacity(utterance.counts.len());
    let mut at = 0_usize;
    for &n in &utterance.counts {
        firsts.push(at);
        at = at.saturating_add(n);
    }
    let restart = firsts.iter().enumerate().skip(1).find(|&(_, &q)| {
        q >= 2
            && utterance.tokens.get(q).is_some_and(|t| similar(a, t))
            && utterance
                .tokens
                .get(q.saturating_add(1))
                .is_some_and(|t| similar(b, t))
    });
    let Some((k, &q)) = restart else {
        return vec![utterance];
    };
    let head = Utterance {
        words: utterance.words.get(..k).unwrap_or_default().to_vec(),
        counts: utterance.counts.get(..k).unwrap_or_default().to_vec(),
        tokens: utterance.tokens.get(..q).unwrap_or_default().to_vec(),
        // Cut just before its retake: its last word is whole
        // ("Le projet, le projet est né").
        open_end: false,
        after_pause: utterance.after_pause,
        end: utterance.end,
    };
    let tail = Utterance {
        words: utterance.words.get(k..).unwrap_or_default().to_vec(),
        counts: utterance.counts.get(k..).unwrap_or_default().to_vec(),
        tokens: utterance.tokens.get(q..).unwrap_or_default().to_vec(),
        open_end: utterance.open_end,
        // A retake straight away counts as a retake after a silence.
        after_pause: true,
        end: utterance.end,
    };
    let mut out = vec![head];
    out.extend(split_restarts(tail));
    out
}

/// Is `b` (said after `a`) another reading of the same sentence?
fn same_sentence(a: &Utterance, b: &Utterance) -> bool {
    let (la, lb) = (a.tokens.len(), b.tokens.len());
    if la.min(lb) < 2 {
        return false;
    }
    #[allow(clippy::cast_precision_loss)]
    let ratio = lcs(&a.tokens, &b.tokens) as f32 / la.max(lb) as f32;
    if ratio >= RETAKE_SIMILARITY {
        return true;
    }
    // Rephrased retake: starting again after a silence, same start, other
    // end. Without a silence, it is a parallelism ("C'est simple et
    // rapide. C'est simple et gratuit.").
    let prefix = a
        .tokens
        .iter()
        .zip(&b.tokens)
        .take_while(|(x, y)| similar(x, y))
        .count();
    if b.after_pause && prefix >= REPHRASED_PREFIX && ratio >= REPHRASED_SIMILARITY {
        return true;
    }
    let (short, long) = if la < lb { (a, b) } else { (b, a) };
    false_start(short, long) || lost_first_word(a, b)
}

/// False start whose retake lost its first word (misheard): "Armand
/// Emm… | (Armand) Emmanuel Sophie…" transcribed "Mont-Emmanuel |
/// Emmanuel Sophie…". The fragment is short, unpunctuated, and its last
/// word opens the next utterance.
fn lost_first_word(fragment: &Utterance, next: &Utterance) -> bool {
    let n = fragment.tokens.len();
    (2..=MAX_FRAGMENT).contains(&n)
        && fragment.open_end
        && next.tokens.len() > n
        && fragment
            .tokens
            .last()
            .zip(next.tokens.first())
            .is_some_and(|(a, b)| similar(a, b))
}

/// Is `short` the start of `long`, abandoned midway? All its words must
/// open `long` in order; the last may be missing if it may have been cut
/// (no punctuation after it).
fn false_start(short: &Utterance, long: &Utterance) -> bool {
    if short.tokens.len() >= long.tokens.len() {
        return false;
    }
    let core = if short.open_end {
        short.tokens.len().saturating_sub(1)
    } else {
        short.tokens.len()
    };
    if core < 2 {
        return false;
    }
    let (Some(short), Some(long)) = (short.tokens.get(..core), long.tokens.get(..core)) else {
        return false;
    };
    short.iter().zip(long).all(|(a, b)| similar(a, b))
}

/// Longest common subsequence (close words accepted).
fn lcs(a: &[String], b: &[String]) -> usize {
    let mut row = vec![0_usize; b.len().saturating_add(1)];
    for x in a {
        let mut diagonal = 0_usize;
        for (j, y) in b.iter().enumerate() {
            let above = row.get(j.saturating_add(1)).copied().unwrap_or(0);
            let left = row.get(j).copied().unwrap_or(0);
            let value = if similar(x, y) {
                diagonal.saturating_add(1)
            } else {
                above.max(left)
            };
            diagonal = above;
            if let Some(cell) = row.get_mut(j.saturating_add(1)) {
                *cell = value;
            }
        }
    }
    row.last().copied().unwrap_or(0)
}

/// Text of a found sentence, ending with a final punctuation mark.
fn sentence_text(transcript: &Transcript, utterance: &Utterance) -> String {
    let text = utterance
        .words
        .iter()
        .filter_map(|&i| transcript.words.get(i))
        .map(|w| w.text.trim())
        .collect::<Vec<_>>()
        .join(" ");
    let text = text
        .trim_end_matches([',', ';', ':', '-', '—', '–', ' '])
        .trim_end_matches("...")
        .trim_end_matches('…')
        .trim();
    // A leading "#" would read as a heading if the text becomes a script.
    let text = text.trim_start_matches('#').trim_start();
    if text.is_empty() {
        String::new()
    } else if text.ends_with(['.', '!', '?']) {
        text.to_owned()
    } else {
        format!("{text}.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::takes::{cut, montage_transcript};
    use enfait_core::{Flicks, TimeRange, Word};

    fn secs(t: f64) -> Flicks {
        #[allow(clippy::cast_possible_truncation)]
        Flicks::from_millis((t * 1000.0).round() as i64).unwrap()
    }

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
                range: TimeRange::new(secs(t), secs(t + 0.25)).unwrap(),
                confidence: 1.0,
            });
            t += 0.3;
        }
        Transcript {
            language: "fr".into(),
            words,
            media_duration: secs(t + 1.0),
        }
    }

    /// Text of the edit obtained without a script.
    fn cleaned(t: &Transcript) -> String {
        let a = analyze(t);
        let parts = cut(&a, t, &a.default_choice()).unwrap();
        let m = montage_transcript(t, &parts).unwrap();
        m.words
            .iter()
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn clean_read_is_kept_whole() {
        let t =
            rush("Le scrutin se déroule en deux tours. | Les deux premiers s'affrontent ensuite.");
        assert_eq!(
            derived_script(&t),
            "Le scrutin se déroule en deux tours.\nLes deux premiers s'affrontent ensuite."
        );
        assert_eq!(
            cleaned(&t),
            "Le scrutin se déroule en deux tours. Les deux premiers s'affrontent ensuite."
        );
    }

    #[test]
    fn false_start_and_full_retake_keep_the_last_reading() {
        let t = rush(
            "Le scrutin se déroule en deux tours. | Les deux premiers euh | \
             Les deux premiers s'affrontent ensuite. | Celui qui recueille le plus de voix gagne. | \
             Celui qui recueille le plus de voix l'emporte.",
        );
        let script = derived_script(&t);
        assert_eq!(script.lines().count(), 3, "{script}");
        assert_eq!(
            cleaned(&t),
            "Le scrutin se déroule en deux tours. Les deux premiers s'affrontent ensuite. \
             Celui qui recueille le plus de voix l'emporte."
        );
    }

    #[test]
    fn restart_inside_a_sentence_is_cut() {
        let t = rush("Le projet, le projet est né en 2020.");
        assert_eq!(cleaned(&t), "le projet est né en 2020.");
    }

    #[test]
    fn anaphora_and_parallel_phrasing_are_not_retakes() {
        let t = rush(
            "C'est simple. C'est rapide. C'est gratuit. | Plus on attend, plus on paie. | \
             C'est simple et rapide. C'est simple et gratuit.",
        );
        assert_eq!(
            cleaned(&t),
            t.words
                .iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        );
    }

    #[test]
    fn asides_and_noises_are_kept_or_ignored() {
        let t = rush(
            "Bonjour à tous. | [Musique] | Attends, je recommence. | Bonjour à toutes et à tous.",
        );
        let script = derived_script(&t);
        assert!(!script.contains("Musique"), "{script}");
        // "Bonjour à toutes et à tous" does not say "Bonjour à tous" again
        // closely enough to be a retake: the aside and both greetings stay.
        assert_eq!(script.lines().count(), 3, "{script}");
    }

    #[test]
    fn far_repetition_is_a_new_sentence() {
        let mut text = String::from("Votez pour vos idées. ");
        for _ in 0..50 {
            text.push_str("| ");
        }
        text.push_str("Votez pour vos idées.");
        let t = rush(&text);
        assert_eq!(derived_script(&t).lines().count(), 2);
    }

    #[test]
    fn retake_that_lost_its_first_word() {
        let t = rush(
            "Le duc de Richelieu. | Mont-Emmanuel | | \
             Emmanuel Sophie Septimani avait parcouru l'Europe.",
        );
        assert_eq!(
            cleaned(&t),
            "Le duc de Richelieu. Emmanuel Sophie Septimani avait parcouru l'Europe."
        );
    }

    #[test]
    fn empty_transcript_gives_empty_script() {
        assert_eq!(derived_script(&Transcript::default()), "");
    }
}
