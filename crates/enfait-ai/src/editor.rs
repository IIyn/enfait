//! "Editor" pass: the storyboard written by the model (kind and
//! description of each block, precise moments, transitions, real shots to
//! search, sound effects and music). This is the "Write the storyboard"
//! button and the heart of magic mode.

use crate::llm::{AiError, Effort, Part, Request};
use crate::text::{fill, last_fenced};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

pub(crate) const EDITOR: &str = include_str!("prompts/editor.md");
/// End of the instructions when the whole video is written at once.
pub(crate) const ANSWER_ALL: &str = include_str!("prompts/editor_answer_all.md");
/// End of the instructions when the video is written part by part.
pub(crate) const ANSWER_PART: &str = include_str!("prompts/editor_answer_part.md");

/// The proposed storyboard.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditPlan {
    pub blocks: Vec<BlockPlan>,
    pub music: Option<MusicPlan>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BlockPlan {
    /// Number of the block, from 1.
    pub index: usize,
    /// `animation`, `mixed`, `effect`, `rush` or `broll`.
    pub kind: String,
    pub description: String,
    pub moments: Vec<MomentPlan>,
    pub broll: Option<SearchPlan>,
    pub transition: Option<TransitionPlan>,
    pub sfx: Vec<SfxPlan>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MomentPlan {
    /// Word of the block, as written.
    pub word: String,
    pub note: String,
}

/// A real shot to search.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchPlan {
    /// Short query, in English.
    pub query: String,
    /// `video` or `image`.
    pub media: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TransitionPlan {
    /// `cut`, `auto` or `described`.
    #[serde(rename = "type")]
    pub kind: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SfxPlan {
    /// Word of the block where the sound falls (empty: start of the block).
    pub word: String,
    /// Short query, in English.
    pub query: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MusicPlan {
    pub query: String,
    pub volume_db: Option<f32>,
}

/// What is shown during a block, plainly for the model and for the code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanKind {
    Animation,
    Mixed,
    Effect,
    Rush,
    Broll,
}

impl PlanKind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Animation => "animation",
            Self::Mixed => "mixed",
            Self::Effect => "effect",
            Self::Rush => "rush",
            Self::Broll => "broll",
        }
    }

    /// Reads a kind written by the model (accents, French and English
    /// variants accepted).
    pub fn parse(text: &str) -> Option<Self> {
        Some(
            match text
                .trim()
                .to_lowercase()
                .replace(['é', 'è'], "e")
                .replace(['-', '_', ' '], "")
                .as_str()
            {
                "animation" | "anim" => Self::Animation,
                "mixte" | "mixed" | "mix" => Self::Mixed,
                "effet" | "effect" => Self::Effect,
                "rush" | "facecam" | "brut" => Self::Rush,
                "broll" | "plan" | "stock" => Self::Broll,
                _ => return None,
            },
        )
    }
}

impl BlockPlan {
    pub fn plan_kind(&self) -> Option<PlanKind> {
        PlanKind::parse(&self.kind)
    }
}

/// A block as the editor sees it.
pub struct EditorBlock {
    pub start: f64,
    pub end: f64,
    pub text: String,
    /// Written by the author (kind and description): to keep as is.
    pub authored: Option<(PlanKind, String)>,
    /// Transition to the next block chosen by the author (`auto` or its
    /// description); `None`: hard cut.
    pub transition: Option<String>,
}

/// A part of a long video, written in its own request.
pub struct EditorPart<'a> {
    /// Blocks to write (indexes in [`EditorInput::blocks`], from 0).
    pub range: std::ops::Range<usize>,
    /// Plan already written for the block just before the part.
    pub before: Option<&'a BlockPlan>,
    /// Kinds of the last blocks written, oldest first (rhythm).
    pub recent_kinds: Vec<&'a str>,
    /// Crafted transitions still allowed in the whole video.
    pub transitions_left: usize,
}

pub struct EditorInput<'a> {
    pub width: u32,
    pub height: u32,
    /// Language of the voice (`fr`, `en`…), empty if unknown.
    pub language: &'a str,
    pub blocks: &'a [EditorBlock],
    /// The take has pictures (filmed person).
    pub has_video: bool,
    /// Video banks are available (Pexels or Pixabay keys).
    pub video_banks: bool,
    pub script: Option<&'a str>,
    pub user_style: Option<&'a str>,
    /// Library files, described (one per line).
    pub library: Option<&'a str>,
    pub max_transitions: usize,
}

fn system(input: &EditorInput<'_>, answer: &str, music_schema: &str) -> String {
    let kinds = if input.has_video {
        "The person is filmed: keep them on screen for part of the blocks (`rush`, \
         `mixed`, `effect`), for example at the opening, on the moments of conviction and in \
         the conclusion, and switch to `animation` or `broll` to explain and show. A `rush` \
         block costs nothing and rests the eye; several filmed blocks in a row get \
         tiresome."
    } else {
        "There is no filmed picture (voice only): only use `animation` and `broll`."
    };
    let broll = if input.video_banks {
        "Video banks (Pexels, Pixabay) are available."
    } else {
        "Only Wikimedia Commons and Openverse are available: few videos, mostly photos and \
         documents. Keep `broll` for what is surely there (monuments, historical figures, \
         famous places), as `image` most of the time."
    };
    let authored = input.blocks.iter().any(|b| b.authored.is_some());
    let fill_rule = if authored {
        "## Blocks already written\n\nThe author already wrote some blocks (marked \"written \
         by the author\"). Take their kind and description **as they are** in your answer \
         (you may add moments, sound effects and a transition) and write the others \
         consistently with them."
    } else {
        ""
    };
    // ANSWER first: the answer templates hold keys of their own.
    fill(
        EDITOR,
        &[
            ("ANSWER", answer),
            ("MUSIC_SCHEMA", music_schema),
            ("KINDS", kinds),
            ("BROLL", broll),
            ("FILL", fill_rule),
            ("MAX_TRANSITIONS", &input.max_transitions.to_string()),
            ("LANGUAGE", "the language of the voice"),
        ],
    )
}

/// Format, duration and language of the video.
fn header(input: &EditorInput<'_>) -> String {
    let duration = input.blocks.last().map_or(0.0, |b| b.end);
    format!(
        "Format: {}×{} px. Duration: {duration:.1} s. {} blocks.{}{}\n",
        input.width,
        input.height,
        input.blocks.len(),
        if input.language.is_empty() {
            String::new()
        } else {
            format!(
                " Language of the voice: {} (on-screen text is in that language).",
                input.language
            )
        },
        crate::animate::format_notes(input.width, input.height),
    )
}

/// One block (`i` from 0), with what the author wrote.
fn block_lines(out: &mut String, i: usize, block: &EditorBlock) {
    let _ = writeln!(
        out,
        "{}. [{:.2} s → {:.2} s, {:.1} s] {}",
        i.saturating_add(1),
        block.start,
        block.end,
        block.end - block.start,
        block.text
    );
    if let Some((kind, description)) = &block.authored {
        let _ = writeln!(
            out,
            "   (written by the author: kind `{}`, “{}”)",
            kind.name(),
            description.trim()
        );
    }
    if let Some(transition) = &block.transition {
        let _ = writeln!(
            out,
            "   (transition to the next block chosen by the author: {})",
            transition_text("auto", transition)
        );
    }
}

fn transition_text(kind: &str, description: &str) -> String {
    let description = description.trim();
    if kind.trim().eq_ignore_ascii_case("auto") || description.is_empty() || description == "auto"
    {
        "`auto` (the animator invents it)".to_owned()
    } else {
        format!("“{description}”")
    }
}

/// Imposed style and library, shared by every request.
fn appendices(out: &mut String, input: &EditorInput<'_>) {
    if let Some(style) = input.user_style {
        let _ = write!(
            out,
            "\n## Imposed style (fixed rules, take precedence)\n\n{style}\n"
        );
    }
    if let Some(library) = input.library {
        let _ = write!(
            out,
            "\n## The author's library\n\nFiles the animators can use; mention them by name in \
             a description when they help. A video or an image of the library can also be the \
             shot of a `broll` block: then give its file name as the query.\n\n{library}\n"
        );
    }
}

/// The whole video in a single request.
pub fn editor_request(input: &EditorInput<'_>, model: &str, effort: Effort) -> Request {
    let mut user = header(input);
    user.push_str("\n## Blocks\n\n");
    for (i, block) in input.blocks.iter().enumerate() {
        block_lines(&mut user, i, block);
    }
    if let Some(script) = input.script.filter(|s| !s.trim().is_empty()) {
        let _ = write!(user, "\n## Original script\n\n{script}\n");
    }
    appendices(&mut user, input);
    Request {
        model: model.to_owned(),
        effort,
        max_tokens: 32_000,
        system: system(input, ANSWER_ALL, ""),
        user: vec![Part::Text(user)],
    }
}

/// One part of a long video: its blocks, and around them only what the
/// flow needs (the block before as written and its transition, the block
/// after). The original script is left out: the voice of the part is
/// enough, and the whole script would weigh as much as the whole video.
pub fn part_request(
    input: &EditorInput<'_>,
    part: &EditorPart<'_>,
    model: &str,
    effort: Effort,
) -> Request {
    let first = part.range.start == 0;
    let mut user = header(input);
    let _ = writeln!(
        user,
        "This part: blocks {} to {}. Crafted transitions left: {}.",
        part.range.start.saturating_add(1),
        part.range.end,
        part.transitions_left
    );
    if let Some(i) = part.range.start.checked_sub(1)
        && let Some(block) = input.blocks.get(i)
    {
        user.push_str("\n## Block just before\n\n");
        block_lines(&mut user, i, block);
        if let Some(plan) = part.before {
            let _ = writeln!(
                user,
                "   (written: kind `{}`, “{}”)",
                plan.kind,
                plan.description.trim()
            );
            if block.transition.is_none()
                && let Some(t) = plan.transition.as_ref().filter(|t| is_crafted(&t.kind))
            {
                let _ = writeln!(
                    user,
                    "   (transition into this part: {})",
                    transition_text(&t.kind, &t.description)
                );
            }
        }
    }
    if !part.recent_kinds.is_empty() {
        let _ = writeln!(
            user,
            "\nKinds of the last blocks written: {}.",
            part.recent_kinds.join(", ")
        );
    }
    user.push_str("\n## Blocks to write\n\n");
    for i in part.range.clone() {
        if let Some(block) = input.blocks.get(i) {
            block_lines(&mut user, i, block);
        }
    }
    if let Some(block) = input.blocks.get(part.range.end) {
        user.push_str("\n## Block just after\n\n");
        block_lines(&mut user, part.range.end, block);
    }
    if first {
        user.push_str(
            "\nThis part opens the video: also choose the music of the whole video \
             (`music` key).\n",
        );
    }
    appendices(&mut user, input);
    let music_schema = if first {
        ",\n  \"music\": { \"query\": \"calm piano ambient\", \"volume_db\": -18 }"
    } else {
        ""
    };
    Request {
        model: model.to_owned(),
        effort,
        max_tokens: 16_000,
        system: system(input, ANSWER_PART, music_schema),
        user: vec![Part::Text(user)],
    }
}

/// A transition other than a hard cut.
pub fn is_crafted(kind: &str) -> bool {
    !matches!(kind.trim().to_lowercase().as_str(), "" | "cut")
}

/// Reads the storyboard in the editor's answer and fits it to what is
/// allowed: blocks outside the video removed, unknown or impossible kinds
/// (filmed person without picture) brought back to `animation`, a single
/// plan per number.
pub fn parse_plan(reply: &str, block_count: usize, has_video: bool) -> Result<EditPlan, AiError> {
    let json = last_fenced(reply, "json")
        .or_else(|| {
            let start = reply.find('{')?;
            let end = reply.rfind('}')?.checked_add(1)?;
            reply.get(start..end)
        })
        .ok_or_else(|| AiError::Unexpected("no JSON in the editor's answer".into()))?;
    let mut plan: EditPlan = serde_json::from_str(json)?;
    let mut seen = vec![false; block_count];
    plan.blocks.retain(|b| {
        let slot = b.index.checked_sub(1).and_then(|i| seen.get_mut(i));
        match slot {
            Some(seen) if !*seen => {
                *seen = true;
                true
            }
            _ => false,
        }
    });
    plan.blocks.sort_by_key(|b| b.index);
    for block in &mut plan.blocks {
        let kind = match block.plan_kind() {
            Some(PlanKind::Mixed | PlanKind::Effect | PlanKind::Rush) if !has_video => {
                PlanKind::Animation
            }
            Some(PlanKind::Broll)
                if block
                    .broll
                    .as_ref()
                    .is_none_or(|b| b.query.trim().is_empty()) =>
            {
                PlanKind::Animation
            }
            Some(kind) => kind,
            None => PlanKind::Animation,
        };
        block.kind = kind.name().to_owned();
        block
            .moments
            .retain(|m| !m.word.trim().is_empty() && !m.note.trim().is_empty());
        block.moments.truncate(2);
        block.sfx.retain(|s| !s.query.trim().is_empty());
        block.sfx.truncate(2);
    }
    if plan
        .music
        .as_ref()
        .is_some_and(|m| m.query.trim().is_empty())
    {
        plan.music = None;
    }
    if plan.blocks.len() < block_count {
        tracing::warn!(
            "the editor wrote {} blocks out of {block_count}, the others stay as they are",
            plan.blocks.len()
        );
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(text: &str) -> EditorBlock {
        EditorBlock {
            start: 0.0,
            end: 3.0,
            text: text.into(),
            authored: None,
            transition: None,
        }
    }

    #[test]
    fn request_adapts_to_rush_and_authored_blocks() {
        let mut blocks = vec![block("Bonjour à tous."), block("La gare.")];
        let input = EditorInput {
            width: 1920,
            height: 1080,
            language: "fr",
            blocks: &blocks,
            has_video: false,
            video_banks: false,
            script: None,
            user_style: None,
            library: None,
            max_transitions: 2,
        };
        let req = editor_request(&input, "m", Effort::Medium);
        assert!(!req.system.contains("{{"));
        assert!(req.system.contains("voice only"));
        assert!(!req.system.contains("Blocks already written"));
        blocks[0].authored = Some((PlanKind::Rush, "to camera".into()));
        let input = EditorInput {
            width: 1920,
            height: 1080,
            language: "fr",
            blocks: &blocks,
            has_video: true,
            video_banks: false,
            script: None,
            user_style: None,
            library: None,
            max_transitions: 2,
        };
        let req = editor_request(&input, "m", Effort::Medium);
        assert!(req.system.contains("The person is filmed"));
        assert!(req.system.contains("Blocks already written"));
        let Part::Text(user) = &req.user[0] else {
            panic!()
        };
        assert!(user.contains("written by the author: kind `rush`, “to camera”"));
        assert!(user.contains("Language of the voice: fr"));
    }

    #[test]
    fn part_request_carries_only_its_neighbours() {
        let blocks: Vec<EditorBlock> = (1..=6).map(|i| block(&format!("Phrase {i}."))).collect();
        let input = EditorInput {
            width: 1920,
            height: 1080,
            language: "fr",
            blocks: &blocks,
            has_video: false,
            video_banks: false,
            script: Some("le script entier"),
            user_style: None,
            library: None,
            max_transitions: 2,
        };
        let before = BlockPlan {
            index: 2,
            kind: "animation".into(),
            description: "un cercle grandit".into(),
            transition: Some(TransitionPlan {
                kind: "described".into(),
                description: "le cercle devient le fond".into(),
            }),
            ..BlockPlan::default()
        };
        let part = EditorPart {
            range: 2..4,
            before: Some(&before),
            recent_kinds: vec!["rush", "animation"],
            transitions_left: 1,
        };
        let req = part_request(&input, &part, "m", Effort::Medium);
        assert!(!req.system.contains("{{"));
        assert!(req.system.contains("One part of the video"));
        assert!(!req.system.contains("\"music\""), "music only in the first part");
        let Part::Text(user) = &req.user[0] else {
            panic!()
        };
        for wanted in [
            "blocks 3 to 4",
            "Crafted transitions left: 1",
            "2. [",
            "un cercle grandit",
            "transition into this part: “le cercle devient le fond”",
            "3. [",
            "4. [",
            "5. [",
            "rush, animation",
        ] {
            assert!(user.contains(wanted), "{wanted} missing");
        }
        assert!(!user.contains("Phrase 1.") && !user.contains("Phrase 6."));
        assert!(!user.contains("le script entier"));

        let first = EditorPart {
            range: 0..2,
            before: None,
            recent_kinds: Vec::new(),
            transitions_left: 2,
        };
        let req = part_request(&input, &first, "m", Effort::Medium);
        assert!(req.system.contains("\"music\""));
    }

    #[test]
    fn plan_is_checked_against_the_video() {
        let reply = r#"Réflexion.
```json
{"blocks":[
 {"index":2,"kind":"Mixte","description":"titre à côté"},
 {"index":1,"kind":"b-roll","description":"gare","broll":{"query":"train station","media":"video"},
  "sfx":[{"word":"gare","query":"train horn"},{"query":""}],
  "moments":[{"word":"gare","note":"le train entre"},{"word":"","note":"x"}],
  "transition":{"type":"auto"}},
 {"index":2,"kind":"rush"},
 {"index":7,"kind":"rush"},
 {"index":3,"kind":"broll","broll":{"query":"  "}}
],
 "music":{"query":"calm piano","volume_db":-20}}
```"#;
        let plan = parse_plan(reply, 3, false).unwrap();
        assert_eq!(
            plan.blocks.len(),
            3,
            "duplicate and block outside the video removed"
        );
        assert_eq!(plan.blocks[0].index, 1);
        assert_eq!(plan.blocks[0].kind, "broll");
        assert_eq!(plan.blocks[0].sfx.len(), 1);
        assert_eq!(plan.blocks[0].moments.len(), 1);
        assert_eq!(plan.blocks[1].kind, "animation", "no filmed take");
        assert_eq!(plan.blocks[2].kind, "animation", "plan without a query");
        assert_eq!(plan.music.unwrap().volume_db, Some(-20.0));
        let filmed = parse_plan(reply, 3, true).unwrap();
        assert_eq!(filmed.blocks[1].kind, "mixed");
    }

    #[test]
    fn kinds_are_read_loosely() {
        assert_eq!(PlanKind::parse("Effet"), Some(PlanKind::Effect));
        assert_eq!(PlanKind::parse("B-Roll"), Some(PlanKind::Broll));
        assert_eq!(PlanKind::parse("face cam"), Some(PlanKind::Rush));
        assert_eq!(PlanKind::parse("dessin"), None);
    }
}
