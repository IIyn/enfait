//! Prises : alignement d'un rush transcrit sur le script, et règle
//! « dernière prise ».
//!
//! Le rush est lu de gauche à droite. À chaque mot, on cherche une phrase du
//! script qui commence là (ses deux premiers mots, à une faute près) et on
//! l'aligne sur la suite de la transcription (alignement local : mots
//! identiques ou proches, mots sautés, mots ajoutés). Si la même phrase
//! recommence avant la fin de l'alignement, c'est un faux départ : la prise
//! s'arrête là et la suivante commence. Les mots qui ne correspondent à
//! aucune phrase (apartés, « euh ») ne font partie d'aucune prise.
//!
//! Pour chaque phrase, on garde la dernière prise complète (ou, à défaut, la
//! plus complète) ; l'utilisateur peut en choisir une autre. Le montage met
//! les phrases dans l'ordre du script, en gardant d'un seul tenant les
//! phrases dites à la suite.

use crate::script::{Sentence, sentences, tokens};
use enfait_core::{Flicks, TimeError, TimeRange, Transcript};
use std::ops::Range;

/// Marge gardée avant le premier mot d'une prise (secondes).
const PAD_BEFORE: f64 = 0.12;
/// Marge gardée avant une prise dont le premier mot n'a pas été reconnu.
const PAD_MISSING_START: f64 = 0.9;
/// Marge gardée après le dernier mot.
const PAD_AFTER: f64 = 0.25;
/// Au-delà de ce silence, deux phrases dites à la suite sont coupées.
const MAX_JOINED_GAP: f64 = 1.5;
/// Silence après lequel une phrase qui recommence est une reprise.
const RESTART_PAUSE: f64 = 0.5;
/// Part des mots d'une phrase qu'une prise doit contenir pour être complète.
const COMPLETE: f32 = 0.8;

/// Une prise d'une phrase.
#[derive(Debug, Clone, PartialEq)]
pub struct Take {
    /// Index de la phrase dans le script.
    pub sentence: usize,
    /// Mots de la transcription, du premier au dernier mot reconnu.
    pub words: Range<usize>,
    /// Part des mots de la phrase retrouvés, entre 0 et 1.
    pub coverage: f32,
    /// Assez de mots, jusqu'à la fin de la phrase.
    pub complete: bool,
    /// Le premier mot de la phrase manque (mal entendu ou avalé) : il est
    /// sans doute juste avant, le montage garde plus de marge.
    pub missing_start: bool,
}

#[derive(Debug, Clone)]
pub struct Analysis {
    pub sentences: Vec<Sentence>,
    /// Prises dans l'ordre du rush.
    pub takes: Vec<Take>,
}

impl Analysis {
    /// Prises d'une phrase : (index dans [`Analysis::takes`], prise).
    pub fn takes_of(&self, sentence: usize) -> impl Iterator<Item = (usize, &Take)> {
        self.takes
            .iter()
            .enumerate()
            .filter(move |(_, t)| t.sentence == sentence)
    }

    /// Règle « dernière prise » : pour chaque phrase, la dernière prise
    /// complète, sinon la plus complète (la dernière à égalité), sinon rien.
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

/// Un mot normalisé de la transcription et le mot dont il vient.
struct Token {
    text: String,
    word: usize,
    /// Silence avant ce jeton (0 à l'intérieur d'un mot), en secondes.
    pause: f64,
}

/// Alignement d'une phrase à partir d'une position du rush.
#[derive(Debug, Clone, Copy)]
struct Alignment {
    score: i32,
    matches: usize,
    /// Premier et dernier jeton du rush reconnus.
    first: usize,
    last: usize,
    /// Premier et dernier mots de la phrase reconnus.
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
        // Faux départ : la même phrase recommence avant la fin de la prise,
        // et la reprise va au moins aussi loin.
        let restart = ((found.first.saturating_add(1))..=found.last).find(|&q| {
            // La reprise peut avoir perdu son premier mot (mal entendu) ; elle
            // doit revenir en arrière dans la phrase (plus de `j0` mots de la
            // prise déjà dits avant elle) ou suivre un silence.
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

/// Indice du premier mot de `script` reconnu en `p` (0, ou 1 si le premier
/// mot de la phrase a été avalé).
fn start_offset(script: &[String], rush: &[Token], p: usize) -> usize {
    usize::from(!starts(script, rush, p, 0))
}

/// La phrase commence-t-elle (à partir de son mot `j0`) au jeton `p` ?
fn starts(script: &[String], rush: &[Token], p: usize, j0: usize) -> bool {
    let word = |i: usize| rush.get(i).map(|t| t.text.as_str());
    let (Some(first), Some(here)) = (script.get(j0), word(p)) else {
        return false;
    };
    if !similar(first, here) {
        return false;
    }
    match script.get(j0.saturating_add(1)) {
        // Le deuxième mot suit, éventuellement après un mot parasite.
        Some(second) => [1_usize, 2]
            .iter()
            .any(|d| word(p.saturating_add(*d)).is_some_and(|w| similar(second, w))),
        None => true,
    }
}

/// Meilleure phrase qui commence en `p`.
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
    /// (mot de la phrase, jeton du rush) du premier et du dernier accord.
    first: Option<(usize, usize)>,
    last: Option<(usize, usize)>,
}

/// Alignement local de `script[j0..]` sur `rush[p..end]`, ancré au début.
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
    // Ancrage : la prise commence au jeton `p`, sans sauter de mot du rush
    // avant le premier mot de la phrase.
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

/// Mots identiques, ou à une faute près (deux pour les mots longs).
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

/// Parties du rush à garder, dans l'ordre du montage. `choice` : prise
/// retenue pour chaque phrase (index dans [`Analysis::takes`]).
pub fn cut(
    analysis: &Analysis,
    transcript: &Transcript,
    choice: &[Option<usize>],
) -> Result<Vec<TimeRange>, TimeError> {
    let words = &transcript.words;
    let start_of = |i: usize| words.get(i).map(|w| w.range.start().as_secs_f64());
    let end_of = |i: usize| words.get(i).map(|w| w.range.end().as_secs_f64());
    let total = transcript.media_duration.as_secs_f64();
    // (premier mot, dernier mot, début de phrase manquant) de chaque partie.
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
            // Marges réduites de moitié du silence voisin, pour ne jamais
            // reprendre un bout de mot qui n'est pas dans la prise. Si le
            // premier mot manque, on remonte presque jusqu'au mot précédent.
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

/// Transcription du montage : les mots des morceaux gardés (dans l'ordre),
/// recalés sur le temps du montage.
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
    // Instant d'un média : positif et borné.
    #[allow(clippy::cast_possible_truncation)]
    let millis = (t * 1000.0).round() as i64;
    Flicks::from_millis(millis)
}

#[cfg(test)]
mod tests {
    use super::*;
    use enfait_core::Word;

    /// Transcription : mots séparés par des espaces, `|` marque une pause
    /// d'une seconde. 0,3 s par mot.
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
        assert_eq!(parts.len(), 1, "phrases dites à la suite : un seul morceau");
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
        assert_eq!(parts.len(), 2, "le faux départ est retiré");
        // Le « euh » n'est dans aucune prise.
        assert!(
            a.takes
                .iter()
                .all(|take| !t.words[take.words.clone()].iter().any(|w| w.text == "euh"))
        );
    }

    #[test]
    fn retake_missing_its_first_word_is_still_a_retake() {
        // Faux départ, puis reprise dont whisper a perdu le premier mot.
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
            "bégaiement retiré"
        );
        assert_eq!(first[0].1.words.start, 1);
        assert_eq!(a.default_choice()[0], Some(first[1].0), "la dernière gagne");
        assert_eq!(a.default_choice()[2], None, "phrase jamais dite");
    }

    #[test]
    fn redone_sentence_keeps_script_order() {
        // 1, 2, 3 d'un trait, puis la 2 refaite à la fin.
        let t = rush(
            "Le scrutin se déroule en deux tours. Les deux premiers s'affrontent ensuite. \
             Celui qui recueille le plus de voix l'emporte. | \
             Les deux premiers s'affrontent ensuite.",
        );
        let a = analyze(SCRIPT, &t);
        let choice = a.default_choice();
        let parts = cut(&a, &t, &choice).unwrap();
        assert_eq!(parts.len(), 3, "1, puis la 2 refaite, puis la 3");
        assert!(
            parts[1].start() > parts[2].start(),
            "la 2 vient de la fin du rush"
        );
        // Choisir à la main la première prise de la 2 recolle tout.
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
        // Dernier mot : 1,8 → 2,05 s ; « Bon. » commence à 2,1 s.
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
        // Le premier mot de la 2e partie commence juste après la marge.
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
