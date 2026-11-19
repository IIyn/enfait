//! Script: sentences to say and normalised words for the alignment.
//!
//! The script is free text (or Markdown). What is not spoken is ignored:
//! headings (`# …`), directions in brackets (`[pause]`), HTML comments.
//! List markers are removed.

/// A sentence of the script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sentence {
    /// Text as written (normalised spaces).
    pub text: String,
    /// Normalised words (see [`tokens`]).
    pub tokens: Vec<String>,
}

/// Sentences of the script, in order. Sentences without words are ignored.
pub fn sentences(script: &str) -> Vec<Sentence> {
    let spoken = spoken_text(script);
    let mut out = Vec::new();
    for paragraph in spoken.split("\n\n").flat_map(|p| p.split('\n')) {
        let chars: Vec<char> = paragraph.chars().collect();
        let mut current = String::new();
        let mut i = 0;
        while let Some(&c) = chars.get(i) {
            current.push(c);
            i = i.saturating_add(1);
            if !matches!(c, '.' | '!' | '?' | '…') {
                continue;
            }
            // Keep with the sentence the punctuation that follows and the closing
            // quotes, even after a space ("« non. »").
            loop {
                match (chars.get(i), chars.get(i.saturating_add(1))) {
                    (Some(n), _) if matches!(n, '»' | '"' | ')' | '.' | '!' | '?') => {
                        current.push(*n);
                        i = i.saturating_add(1);
                    }
                    (Some(w), Some('»')) if w.is_whitespace() => {
                        current.push(*w);
                        current.push('»');
                        i = i.saturating_add(2);
                    }
                    _ => break,
                }
            }
            if chars.get(i).is_none_or(|n| n.is_whitespace()) {
                push_sentence(&mut out, &current);
                current.clear();
            }
        }
        push_sentence(&mut out, &current);
    }
    out
}

/// Paragraphs to say, without headings or notes: the prompter text.
pub fn spoken_paragraphs(script: &str) -> Vec<String> {
    spoken_text(script)
        .split("\n\n")
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|p| !p.is_empty())
        .collect()
}

fn push_sentence(out: &mut Vec<Sentence>, text: &str) {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let tokens = tokens(&text);
    if !tokens.is_empty() {
        out.push(Sentence { text, tokens });
    }
}

/// Removes what is not spoken.
fn spoken_text(script: &str) -> String {
    let mut text = String::with_capacity(script.len());
    let mut in_comment = false;
    for line in script.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            text.push('\n');
            continue;
        }
        let line = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("> "))
            .unwrap_or(trimmed);
        let mut depth = 0_u32;
        let mut rest = line;
        while !rest.is_empty() {
            if in_comment {
                match rest.find("-->") {
                    Some(i) => {
                        rest = rest.get(i.saturating_add(3)..).unwrap_or_default();
                        in_comment = false;
                    }
                    None => rest = "",
                }
                continue;
            }
            if let Some(after) = rest.strip_prefix("<!--") {
                in_comment = true;
                rest = after;
                continue;
            }
            let mut chars = rest.chars();
            let Some(c) = chars.next() else { break };
            match c {
                '[' => depth = depth.saturating_add(1),
                ']' => depth = depth.saturating_sub(1),
                '*' | '_' | '`' => {}
                _ if depth == 0 => text.push(c),
                _ => {}
            }
            rest = chars.as_str();
        }
        text.push('\n');
    }
    text
}

/// Normalised words: lowercase, no accents or punctuation, apostrophes
/// and hyphens separating words (`l'oxygène` → `l`, `oxygene`). Numbers
/// keep their digits (`3 300` → `3300`, `2,5` → `25`).
pub fn tokens(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut previous_digit_group = false;
    let flush = |current: &mut String, out: &mut Vec<String>, previous: &mut bool| {
        if current.is_empty() {
            return;
        }
        let digits = current.chars().all(|c| c.is_ascii_digit());
        // "3 300": a group of three digits after a number extends it.
        if digits && *previous && current.len() == 3 {
            if let Some(last) = out.last_mut() {
                last.push_str(current);
            }
        } else {
            out.push(current.clone());
        }
        *previous = digits;
        current.clear();
    };
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let lower = c.to_lowercase();
        let mut separator = true;
        for l in lower {
            if let Some(folded) = fold(l) {
                current.push_str(folded);
                separator = false;
            } else if l.is_alphanumeric() {
                current.push(l);
                separator = false;
            }
        }
        // Decimal or thousands separator inside a number.
        if separator
            && matches!(c, ',' | '.' | '\u{202f}' | '\u{a0}')
            && current.chars().last().is_some_and(|d| d.is_ascii_digit())
            && chars.peek().is_some_and(char::is_ascii_digit)
        {
            continue;
        }
        if separator {
            flush(&mut current, &mut out, &mut previous_digit_group);
            if !c.is_whitespace() {
                previous_digit_group = false;
            }
        }
    }
    flush(&mut current, &mut out, &mut previous_digit_group);
    out
}

/// Common accented letters to their plain form.
fn fold(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' => "a",
        'ç' => "c",
        'é' | 'è' | 'ê' | 'ë' => "e",
        'î' | 'ï' | 'í' | 'ì' => "i",
        'ô' | 'ö' | 'ó' | 'ò' | 'õ' => "o",
        'û' | 'ü' | 'ú' | 'ù' => "u",
        'ÿ' => "y",
        'ñ' => "n",
        'œ' => "oe",
        'æ' => "ae",
        'ß' => "ss",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_words() {
        assert_eq!(
            tokens("L'oxygène, c'est À-dire…"),
            ["l", "oxygene", "c", "est", "a", "dire"]
        );
        assert_eq!(
            tokens("vers 3 300 ans, 2,5 % et 4000"),
            ["vers", "3300", "ans", "25", "et", "4000"]
        );
        assert_eq!(tokens("Œuvre « vive » !"), ["oeuvre", "vive"]);
        assert_eq!(tokens("1 200 et 7 chats"), ["1200", "et", "7", "chats"]);
    }

    #[test]
    fn splits_sentences_and_skips_notes() {
        let script = "# Introduction\n\
            Bonjour à tous. Aujourd'hui, on parle d'eau [pause] douce !\n\
            <!-- note : ralentir -->\n\
            - Pourquoi ? Parce que… c'est vital.\n\
            Il a dit « non. » Puis il est parti\n";
        let s: Vec<String> = sentences(script).into_iter().map(|s| s.text).collect();
        assert_eq!(
            s,
            [
                "Bonjour à tous.",
                "Aujourd'hui, on parle d'eau douce !",
                "Pourquoi ?",
                "Parce que…",
                "c'est vital.",
                "Il a dit « non. »",
                "Puis il est parti",
            ]
        );
    }

    #[test]
    fn paragraphs_for_the_prompter() {
        let p = spoken_paragraphs("# Titre\n\nUn. [note] Deux.\nTrois.\n\n\nQuatre.");
        assert_eq!(p, ["Un. Deux. Trois.", "Quatre."]);
    }

    #[test]
    fn decimals_do_not_end_sentences() {
        let s = sentences("Il mesure 2.5 mètres. Voilà.");
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].tokens, ["il", "mesure", "25", "metres"]);
    }
}
