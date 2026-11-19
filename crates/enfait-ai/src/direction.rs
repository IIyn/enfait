//! "Director" pass: art direction and intent of each block.

use crate::llm::{AiError, Effort, Part, Request};
use crate::text::{fill, last_fenced};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

pub const STYLE_GUIDE: &str = include_str!("prompts/style_guide.md");
pub(crate) const DIRECTOR: &str = include_str!("prompts/director.md");

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Direction {
    pub concept: String,
    pub palette: Palette,
    pub typography: Typography,
    pub motion: String,
    pub layout: String,
    pub motifs: String,
    pub blocks: Vec<BlockIntent>,
    pub transitions: Vec<TransitionIntent>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Palette {
    pub background: String,
    pub ink: String,
    pub accent: String,
    pub secondary: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Typography {
    pub display: String,
    pub text: String,
    pub notes: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BlockIntent {
    pub index: usize,
    pub intent: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TransitionIntent {
    /// Number (from 1) of the block after which the transition happens.
    pub after: usize,
    pub description: String,
}

impl Direction {
    /// Intent of block `index` (from 1).
    pub fn intent(&self, index: usize) -> &str {
        self.blocks
            .iter()
            .find(|b| b.index == index)
            .map_or("", |b| b.intent.as_str())
    }

    /// Transition after block `index`, if any.
    pub fn transition_after(&self, index: usize) -> Option<&str> {
        self.transitions
            .iter()
            .find(|t| t.after == index && !t.description.trim().is_empty())
            .map(|t| t.description.as_str())
    }

    /// Shared direction, as text, for the animators.
    pub fn art_direction(&self) -> String {
        let p = &self.palette;
        let t = &self.typography;
        format!(
            "- Concept: {}\n- Palette: background {}, ink {}, accent {}, secondary {}\n\
             - Typography: display “{}”, text “{}”. {}\n- Motion: {}\n\
             - Layout: {}\n- Recurring motifs: {}",
            self.concept,
            p.background,
            p.ink,
            p.accent,
            p.secondary.join(", "),
            t.display,
            t.text,
            t.notes,
            self.motion,
            self.layout,
            self.motifs
        )
    }
}

/// A block as the director sees it.
pub struct DirectorBlock {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

pub struct DirectorInput<'a> {
    pub width: u32,
    pub height: u32,
    pub blocks: &'a [DirectorBlock],
    pub script: Option<&'a str>,
    pub user_style: Option<&'a str>,
    pub fonts: &'a str,
    pub max_transitions: usize,
}

pub fn director_request(input: &DirectorInput<'_>, model: &str, effort: Effort) -> Request {
    let system = fill(
        DIRECTOR,
        &[
            ("STYLE_GUIDE", STYLE_GUIDE),
            ("FONTS", input.fonts),
            ("MAX_TRANSITIONS", &input.max_transitions.to_string()),
        ],
    );
    let mut user = format!(
        "Format: {}×{} px. {} blocks.{}\n\n## Blocks\n\n",
        input.width,
        input.height,
        input.blocks.len(),
        crate::animate::format_notes(input.width, input.height)
    );
    for (i, block) in input.blocks.iter().enumerate() {
        let _ = writeln!(
            user,
            "{}. [{:.2} s → {:.2} s, {:.1} s] {}",
            i.saturating_add(1),
            block.start,
            block.end,
            block.end - block.start,
            block.text
        );
    }
    if let Some(script) = input.script {
        let _ = write!(user, "\n## Original script\n\n{script}\n");
    }
    if let Some(style) = input.user_style {
        let _ = write!(
            user,
            "\n## Imposed style (fixed rules, take precedence)\n\n{style}\n"
        );
    }
    Request {
        model: model.to_owned(),
        effort,
        max_tokens: 32_000,
        system,
        user: vec![Part::Text(user)],
    }
}

/// Reads the direction in the director's answer.
pub fn parse_direction(reply: &str, block_count: usize) -> Result<Direction, AiError> {
    let json = last_fenced(reply, "json")
        .or_else(|| {
            let start = reply.find('{')?;
            let end = reply.rfind('}')?.checked_add(1)?;
            reply.get(start..end)
        })
        .ok_or_else(|| AiError::Unexpected("no JSON in the director's answer".into()))?;
    let direction: Direction = serde_json::from_str(json)?;
    let described = (1..=block_count)
        .filter(|i| !direction.intent(*i).trim().is_empty())
        .count();
    if described < block_count {
        tracing::warn!(
            "the director described {described} blocks out of {block_count}, the others will follow the general direction"
        );
    }
    Ok(direction)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fenced_direction() {
        let reply = r##"Voici.
```json
{"concept":"c","palette":{"background":"#000000","ink":"#ffffff","accent":"#ff0000"},
 "blocks":[{"index":1,"intent":"un"},{"index":2,"intent":"deux"}],
 "transitions":[{"after":1,"description":"zoom"}]}
```"##;
        let d = parse_direction(reply, 2).unwrap();
        assert_eq!(d.intent(2), "deux");
        assert_eq!(d.transition_after(1), Some("zoom"));
        assert_eq!(d.transition_after(2), None);
        assert!(d.art_direction().contains("#ff0000"));
    }

    #[test]
    fn request_contains_blocks_and_rules() {
        let blocks = [DirectorBlock {
            start: 0.0,
            end: 2.5,
            text: "Bonjour.".into(),
        }];
        let input = DirectorInput {
            width: 1920,
            height: 1080,
            blocks: &blocks,
            script: None,
            user_style: Some("toujours en majuscules"),
            fonts: "- Inter",
            max_transitions: 1,
        };
        let req = director_request(&input, "m", Effort::High);
        assert!(!req.system.contains("{{"));
        let Part::Text(user) = &req.user[0] else {
            panic!()
        };
        assert!(user.contains("Bonjour.") && user.contains("majuscules"));
    }
}
