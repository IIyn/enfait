//! Nettoyage d'une prise sans script (mode magique, ou voix improvisée) :
//! les phrases et leurs reprises se déduisent de la transcription.
//!
//! La transcription est coupée en énoncés (fins de phrase, longs silences). Un
//! énoncé qui redit un énoncé récent presque mot pour mot est une reprise,
//! comme celui qui en reprend le début après un silence et finit
//! autrement ; un énoncé qui s'arrête en route et que le suivant reprend
//! depuis le début est un faux départ. Chaque groupe devient une phrase,
//! chacune de ses lectures une prise : l'[`Analysis`] obtenue est celle d'un
//! script écrit, la règle « dernière prise » et les choix à la main s'y
//! appliquent tels quels. Les apartés, qui ne ressemblent à rien d'autre,
//! restent des phrases à part entière.

use crate::script::{Sentence, tokens};
use crate::takes::{Analysis, Take, similar};
use enfait_core::Transcript;

/// Silence qui coupe un énoncé même sans ponctuation (secondes). Plus court,
/// il couperait une phrase relue avec des respirations en morceaux qui ne
/// ressemblent plus à la phrase.
const SPLIT_GAP: f64 = 1.5;
/// Silence qui marque une hésitation avant un énoncé (secondes).
const PAUSE: f64 = 0.6;
/// Part des mots en commun au-delà de laquelle deux énoncés sont deux
/// lectures de la même phrase.
const RETAKE_SIMILARITY: f32 = 0.85;
/// Reprise reformulée : même début (au moins autant de mots), après un
/// silence, et au moins cette part de mots en commun.
const REPHRASED_PREFIX: usize = 3;
const REPHRASED_SIMILARITY: f32 = 0.6;
/// Groupes récents comparés à un nouvel énoncé : on reprend une phrase juste
/// après l'avoir ratée, pas trois minutes plus tard.
const RECENT_GROUPS: usize = 3;
/// Au-delà de ce délai depuis la dernière lecture d'une phrase, un énoncé
/// semblable est une phrase nouvelle (un refrain, une récapitulation).
const RECENT_SECONDS: f64 = 45.0;
/// Longueur maximale (en mots) d'un faux départ dont la reprise a perdu son
/// premier mot.
const MAX_FRAGMENT: usize = 3;
/// Part de la phrase qu'une lecture doit couvrir pour être complète.
const COMPLETE: f32 = 0.8;
/// Hésitations retirées des phrases déduites.
const FILLERS: [&str; 9] = ["euh", "heu", "euhh", "hum", "hmm", "um", "uh", "erm", "bah"];

/// Un énoncé : des mots consécutifs de la transcription.
#[derive(Debug, Clone, Default)]
struct Utterance {
    /// Mots gardés (index dans la transcription), sans hésitations.
    words: Vec<usize>,
    /// Nombre de jetons de chaque mot gardé (un mot en donne un ou plus :
    /// `l'oxygène` → `l`, `oxygene`).
    counts: Vec<usize>,
    /// Mots normalisés.
    tokens: Vec<String>,
    /// Le dernier mot n'est suivi d'aucune ponctuation (ou de points de
    /// suspension, d'un tiret) : la phrase a pu être coupée en plein mot.
    open_end: bool,
    /// Un silence précède l'énoncé.
    after_pause: bool,
    /// Fin du dernier mot (secondes).
    end: f64,
}

/// Les lectures d'une même phrase.
struct Group {
    /// Index (dans `readings`) de la lecture qui sert de référence : la plus
    /// complète, la plus récente à longueur égale.
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

/// Phrases et prises d'une prise sans script, comme si elle avait été
/// alignée sur un script.
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
    // Les prises dans l'ordre de la prise, comme pour un script écrit.
    takes.sort_by_key(|t| t.words.start);
    Analysis { sentences, takes }
}

/// Script déduit de la transcription : une phrase par ligne, dans l'ordre où
/// elles sont dites pour la première fois. Vide s'il n'y a aucun mot. Sert
/// de point de départ à un script qu'on corrige à la main.
pub fn derived_script(transcript: &Transcript) -> String {
    analyze(transcript)
        .sentences
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Regroupe les énoncés en lectures d'une même phrase.
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

/// Découpe la transcription en énoncés : après une fin de phrase, ou à un
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
        // Bruits notés par whisper : « [Musique] », « (rires) ».
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

/// Le mot peut avoir été coupé : pas de ponctuation après lui, ou une
/// suspension.
fn open_end(text: &str) -> bool {
    let last = text.chars().last();
    text.ends_with("...")
        || matches!(last, Some('…' | '-' | '—' | '–'))
        || last.is_some_and(char::is_alphanumeric)
}

/// Un énoncé qui recommence son propre début (« Le projet, le projet est
/// né ») contient un faux départ : on le coupe là, le regroupement fera le
/// reste.
fn split_restarts(utterance: Utterance) -> Vec<Utterance> {
    let (Some(a), Some(b)) = (utterance.tokens.first(), utterance.tokens.get(1)) else {
        return vec![utterance];
    };
    // Premier jeton de chaque mot, pour couper entre deux mots.
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
        // Coupé juste avant sa reprise : son dernier mot est entier
        // (« Le projet, le projet est né »).
        open_end: false,
        after_pause: utterance.after_pause,
        end: utterance.end,
    };
    let tail = Utterance {
        words: utterance.words.get(k..).unwrap_or_default().to_vec(),
        counts: utterance.counts.get(k..).unwrap_or_default().to_vec(),
        tokens: utterance.tokens.get(q..).unwrap_or_default().to_vec(),
        open_end: utterance.open_end,
        // Une reprise dans la foulée vaut une reprise après un silence.
        after_pause: true,
        end: utterance.end,
    };
    let mut out = vec![head];
    out.extend(split_restarts(tail));
    out
}

/// `b` (dit après `a`) est-il une autre lecture de la même phrase ?
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
    // Reprise reformulée : on recommence après un silence, même début,
    // autre fin. Sans silence, c'est un parallélisme (« C'est simple et
    // rapide. C'est simple et gratuit. »).
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

/// Faux départ dont la reprise a perdu son premier mot (mal entendu) :
/// « Armand Emm… | (Armand) Emmanuel Sophie… » transcrit « Mont-Emmanuel |
/// Emmanuel Sophie… ». Le fragment est court, non ponctué, et son dernier
/// mot ouvre l'énoncé suivant.
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

/// `short` est-il le début de `long`, abandonné en route ? Tous ses mots
/// doivent ouvrir `long` dans l'ordre ; le dernier peut manquer s'il a pu
/// être coupé (pas de ponctuation après lui).
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

/// Plus longue sous-suite commune (mots proches acceptés).
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

/// Texte d'une phrase déduite, terminé par une ponctuation de fin.
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
    // Un « # » en tête serait lu comme un titre si le texte devient un script.
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

    /// Texte du montage obtenu sans script.
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
        // « Bonjour à toutes et à tous » ne redit pas assez « Bonjour à
        // tous » pour être une reprise : l'aparté et les deux saluts restent.
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
