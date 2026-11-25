//! The YouTube thumbnail designer: one still page made of layers, from the
//! real title of the video, notes from the user and frames of the edit.

use crate::llm::{Effort, Part, Request};
use crate::text::fill;
use std::fmt::Write as _;
use std::path::PathBuf;

pub const THUMBNAIL: &str = include_str!("prompts/thumbnail.md");

/// A picture the designer may use, also shown to them.
#[derive(Debug, Clone)]
pub struct ThumbnailImage {
    /// Path relative to the page (`stills/01.jpg`).
    pub src: String,
    /// File on disk (JPEG or PNG), sent to the model.
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    /// What it shows.
    pub what: String,
}

/// Everything the designer must know.
#[derive(Debug, Clone, Default)]
pub struct ThumbnailBrief {
    pub width: u32,
    pub height: u32,
    /// Title of the video, as it will be published.
    pub title: String,
    /// Description, wishes of the user.
    pub notes: String,
    /// Language of the voice (`fr`, `en`…).
    pub language: String,
    /// What the voice says (possibly shortened).
    pub transcript: String,
    /// Art direction of the video, if known.
    pub art_direction: Option<String>,
    /// Style rules of the user.
    pub user_style: Option<String>,
    /// Files of the user's library usable in the page (Markdown list).
    pub library: Option<String>,
    /// What the `@file` mentions of the notes designate.
    pub mentions: Option<String>,
    pub images: Vec<ThumbnailImage>,
    /// Concepts of the proposals the user turned down.
    pub rejected: Vec<String>,
}

/// Longest transcript given to the designer, in characters.
const TRANSCRIPT_CHARS: usize = 6_000;

impl ThumbnailBrief {
    pub fn render(&self) -> String {
        let mut s = format!(
            "# Thumbnail to design\n\nFormat: {}×{} CSS px (YouTube, 16:9). Language of the \
             video: {}.\n\n## Title of the video\n\n“{}”\n",
            self.width,
            self.height,
            if self.language.is_empty() {
                "same as the transcript"
            } else {
                &self.language
            },
            self.title.trim()
        );
        let notes = self.notes.trim();
        if !notes.is_empty() {
            let _ = write!(
                s,
                "\n## Description and wishes of the author\n\nFollow them: they come first.\n\n{notes}\n"
            );
        }
        if let Some(mentions) = self.mentions.as_deref().filter(|t| !t.trim().is_empty()) {
            let _ = write!(
                s,
                "\n{}\nThe author named them on purpose: use them in the thumbnail.\n",
                mentions.trim()
            );
        }
        if let Some(library) = self.library.as_deref().filter(|t| !t.trim().is_empty()) {
            let _ = write!(
                s,
                "\n## Library of the author\n\nFiles usable in the page, by their relative \
                 path:\n\n{}\n",
                library.trim()
            );
        }
        if let Some(style) = self.user_style.as_deref().filter(|t| !t.trim().is_empty()) {
            let _ = write!(s, "\n## Style rules of the author\n\n{}\n", style.trim());
        }
        if let Some(art) = self
            .art_direction
            .as_deref()
            .filter(|t| !t.trim().is_empty())
        {
            let _ = write!(s, "\n## Art direction of the video\n\n{}\n", art.trim());
        }
        let transcript: String = self.transcript.chars().take(TRANSCRIPT_CHARS).collect();
        if !transcript.trim().is_empty() {
            let cut = if transcript.len() < self.transcript.len() {
                " […]"
            } else {
                ""
            };
            let _ = write!(
                s,
                "\n## What the video says\n\n“{}{cut}”\n",
                transcript.trim()
            );
        }
        if self.images.is_empty() {
            s.push_str("\n## Pictures\n\nNone: draw everything in HTML/CSS/SVG.\n");
        } else {
            s.push_str("\n## Pictures you may use\n\nThey follow this message, in this order.\n\n");
            for image in &self.images {
                let _ = writeln!(
                    s,
                    "- `{}` ({}×{} px): {}",
                    image.src, image.width, image.height, image.what
                );
            }
        }
        if !self.rejected.is_empty() {
            s.push_str(
                "\n## Proposals the author turned down\n\nFind a clearly different idea \
                 (another hook, another composition), not a variation of these:\n\n",
            );
            for concept in &self.rejected {
                let _ = writeln!(s, "- {concept}");
            }
        }
        s
    }
}

/// System instruction of the designer.
pub fn thumbnail_system(width: u32, height: u32, fonts: &str) -> String {
    fill(
        THUMBNAIL,
        &[
            ("WIDTH", &width.to_string()),
            ("HEIGHT", &height.to_string()),
            ("FONTS", fonts),
        ],
    )
}

/// The request for a thumbnail.
pub fn request(brief: &ThumbnailBrief, fonts: &str, model: &str, effort: Effort) -> Request {
    let mut user = vec![Part::Text(brief.render())];
    user.extend(brief.images.iter().map(|i| Part::Image(i.path.clone())));
    Request {
        model: model.to_owned(),
        effort,
        max_tokens: 24_000,
        system: thumbnail_system(brief.width, brief.height, fonts),
        user,
    }
}

/// The concept announced by the reply (`CONCEPT: …`).
pub fn concept(reply: &str) -> Option<String> {
    reply.lines().find_map(|line| {
        let line = line.trim().trim_start_matches(['*', '#', ' ']);
        let rest = line
            .strip_prefix("CONCEPT:")
            .or_else(|| line.strip_prefix("Concept:"))?;
        let rest = rest.trim().trim_matches('*').trim();
        (!rest.is_empty()).then(|| rest.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brief_mentions_everything() {
        let brief = ThumbnailBrief {
            width: 1280,
            height: 720,
            title: "J'ai testé 30 jours sans sucre".into(),
            notes: "ton sérieux".into(),
            language: "fr".into(),
            transcript: "Bonjour à tous".into(),
            images: vec![ThumbnailImage {
                src: "stills/person.png".into(),
                path: "/tmp/p.png".into(),
                width: 1920,
                height: 1080,
                what: "cut-out person".into(),
            }],
            rejected: vec!["big number 30 on red".into()],
            library: Some("- `assets/logo.png` (image): Logo".into()),
            mentions: Some("Files the user names with @: logo".into()),
            ..Default::default()
        };
        let text = brief.render();
        for needle in [
            "1280×720",
            "30 jours sans sucre",
            "ton sérieux",
            "Bonjour à tous",
            "`stills/person.png` (1920×1080 px)",
            "big number 30 on red",
            "`assets/logo.png`",
            "use them in the thumbnail",
        ] {
            assert!(text.contains(needle), "{needle}");
        }
        let request = request(&brief, "- Inter", "m", Effort::High);
        assert_eq!(request.user.len(), 2);
        assert!(request.system.contains("1280×720 CSS px"));
        assert!(request.system.contains("- Inter"));
        assert!(!request.system.contains("{{"));
    }

    #[test]
    fn reads_the_concept() {
        assert_eq!(
            concept("CONCEPT: a face and a number\n```html\n```").as_deref(),
            Some("a face and a number")
        );
        assert_eq!(concept("**CONCEPT:** shock").as_deref(), Some("shock"));
        assert_eq!(concept("nothing"), None);
    }
}
