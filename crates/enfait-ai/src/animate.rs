//! Animator agents: creating, fixing and critiquing a composition.

use crate::direction::STYLE_GUIDE;
use crate::llm::{Effort, Part, Request};
use crate::text::fill;
use std::fmt::Write as _;
use std::path::PathBuf;

pub(crate) const ANIMATOR: &str = include_str!("prompts/animator.md");

/// System instruction shared by every animation call. Identical for all
/// the blocks of a video, it benefits from the prompt cache.
pub fn animator_system(contract: &str, fonts: &str) -> String {
    fill(
        ANIMATOR,
        &[
            ("STYLE_GUIDE", STYLE_GUIDE),
            ("FONTS", fonts),
            ("CONTRACT", contract),
        ],
    )
}

#[derive(Debug, Clone)]
pub struct TimedWord {
    /// Start of the word, in seconds from the start of the scene.
    pub t: f64,
    pub text: String,
}

/// Everything an animator must know about their scene.
#[derive(Debug, Clone)]
pub struct SceneBrief {
    pub index: usize,
    pub total: usize,
    pub width: u32,
    pub height: u32,
    pub duration: f64,
    pub text: String,
    pub words: Vec<TimedWord>,
    pub art_direction: String,
    pub intent: String,
    pub previous: Option<String>,
    pub next: Option<String>,
    pub user_style: Option<String>,
    /// User files usable in the scene (Markdown list: path and description),
    /// see [`library_brief`].
    pub library: Option<String>,
    /// Precise moments wanted by the user: time (s, from the start of the
    /// scene), spoken word, what must happen.
    pub moments: Vec<(f64, String, String)>,
    /// Scene with the filmed take (see [`SceneLayer`]).
    pub layer: Option<SceneLayer>,
}

/// A scene showing the filmed take, provided as images by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneLayer {
    /// The animation accompanies the filmed person (beside or on top).
    Mixed,
    /// Like `Mixed`, with the person cut out as well: the animation can pass
    /// behind them.
    MixedCutout,
    /// An effect transforms the take itself.
    Effect,
}

impl SceneLayer {
    fn brief(self) -> &'static str {
        match self {
            Self::Mixed => {
                "MIXED scene: the filmed take (face cam) is provided on every frame in \
                 `img[data-enfait-scene=\"from\"]` (see the contract). It stays visible and \
                 readable: at least half of the screen, the face never covered. The animation \
                 comes beside it (split screen, panel) or on top (light elements), within the \
                 art direction of the video."
            }
            Self::MixedCutout => {
                "MIXED scene WITH CUTOUT: the filmed take (face cam) is provided on every frame \
                 in `img[data-enfait-scene=\"from\"]`, and the person alone, cut out \
                 (transparent background, exactly aligned on `from`), in \
                 `img[data-enfait-scene=\"to\"]`. Stack: `from` at the back, your elements \
                 (large title, shapes, numbers) in the middle, `to` on top: they pass behind \
                 the person. You may also replace the background: your own background, then \
                 your elements, then `to`, without showing `from`. Do not transform `to` \
                 differently from `from` (same position, same size), the face never covered."
            }
            Self::Effect => {
                "EFFECT scene: the filmed take is provided on every frame in \
                 `img[data-enfait-scene=\"from\"]` (see the contract). It stays the subject: \
                 apply an effect to it that serves the sentence (reframing, slow zoom, frame, \
                 colouring, superimposed word, transformation of the picture) without replacing \
                 it."
            }
        }
    }
}

/// Format-specific instructions: for a vertical video (Shorts, Reels,
/// TikTok), the areas covered by the apps' interface.
pub fn format_notes(width: u32, height: u32) -> &'static str {
    if height > width {
        "\n\nVertical phone video (Shorts, Reels, TikTok): the app interface covers the \
         top (about 8 % of the height), the bottom (about 20 %) and the right (about 12 % of \
         the width). Keep text and important elements in the remaining central area; large, \
         short text, readable on a small screen."
    } else {
        ""
    }
}

impl SceneBrief {
    pub fn render(&self) -> String {
        let mut s = format!(
            "# Scene {} of {}\n\nFormat: {}×{} px. Duration: **{:.3} s** \
             (`window.enfait.duration = {:.3}`).{}\n\n## Art direction of the video\n\n{}\n\n\
             ## Intent for this scene\n\n{}\n\n## What the voice says\n\n“{}”\n\n\
             ## Times of the words (seconds from the start of the scene)\n\n",
            self.index,
            self.total,
            self.width,
            self.height,
            self.duration,
            self.duration,
            format_notes(self.width, self.height),
            self.art_direction,
            if self.intent.trim().is_empty() {
                "(free, in the spirit of the direction)"
            } else {
                self.intent.as_str()
            },
            self.text,
        );
        for w in &self.words {
            let _ = writeln!(s, "- {:.2} {}", w.t, w.text);
        }
        if let Some(layer) = self.layer {
            let _ = write!(s, "\n## Scene kind\n\n{}\n", layer.brief());
        }
        if !self.moments.is_empty() {
            s.push_str(
                "\n## Precise moments (wanted by the user, to respect at the given time)\n\n",
            );
            for (t, word, note) in &self.moments {
                let _ = writeln!(s, "- at {t:.2} s, on “{word}”: {note}");
            }
        }
        if let Some(prev) = &self.previous {
            let _ = write!(s, "\n## Previous scene\n\n{prev}\n");
        }
        if let Some(next) = &self.next {
            let _ = write!(s, "\n## Next scene\n\n{next}\n");
        }
        if let Some(style) = &self.user_style {
            let _ = write!(
                s,
                "\n## Imposed style (fixed rules, take precedence)\n\n{style}\n"
            );
        }
        if let Some(library) = &self.library {
            let _ = write!(
                s,
                "\n## The user's library\n\nFiles provided by the user, to use when they serve \
                 the scene (paths relative to the composition; do not distort or recolour them \
                 unless asked):\n\n{library}\n"
            );
        }
        s
    }
}

/// Markdown list of the library: `(path, kind, name, description)`.
pub fn library_brief(items: &[(String, &str, String, String)]) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let mut s = String::new();
    for (path, kind, name, description) in items {
        let _ = write!(s, "- `{path}` ({kind}): {name}");
        if !description.trim().is_empty() {
            let _ = write!(s, " — {}", description.trim());
        }
        s.push('\n');
    }
    Some(s)
}

/// Common settings of the animation calls.
#[derive(Debug, Clone)]
pub struct AnimatorConfig {
    pub system: String,
    pub model: String,
    pub effort: Effort,
}

impl AnimatorConfig {
    fn request(&self, user: Vec<Part>) -> Request {
        Request {
            model: self.model.clone(),
            effort: self.effort,
            max_tokens: 64_000,
            system: self.system.clone(),
            user,
        }
    }

    pub fn create(&self, brief: &str) -> Request {
        self.request(vec![Part::Text(format!(
            "Mode: CREATE.\n\n{brief}\n\nMake the composition."
        ))])
    }

    pub fn fix(&self, brief: &str, html: &str, problems: &[String]) -> Request {
        self.request(vec![Part::Text(format!(
            "Mode: FIX.\n\n{brief}\n\n## Current composition\n\n```html\n{html}\n```\n\n\
             ## Problems found while rendering\n\n- {}\n\nFix these problems without degrading \
             the scene and return the complete document.",
            problems.join("\n- ")
        ))])
    }

    /// `samples`: times (s) and the matching rendered frames.
    pub fn critique(
        &self,
        brief: &str,
        html: &str,
        samples: &[(f64, PathBuf)],
        warnings: &[String],
    ) -> Request {
        let mut parts = vec![Part::Text(format!(
            "Mode: CRITIQUE.\n\n{brief}\n\n## Current composition\n\n```html\n{html}\n```\n\n\
             ## Rendered frames\n\nHere are {} frames of the scene, at half size.",
            samples.len()
        ))];
        for (t, path) in samples {
            parts.push(Part::Text(format!("Frame at t = {t:.2} s:")));
            parts.push(Part::Image(path.clone()));
        }
        let mut closing = String::new();
        if !warnings.is_empty() {
            let _ = write!(
                closing,
                "## Automatic remarks\n\n- {}\n\n",
                warnings.join("\n- ")
            );
        }
        closing.push_str(
            "Judge the scene: readability, sync with the voice, composition, quality of the \
             motion (infer it from the code and the frames), respect of the direction and the \
             guide. If a style is imposed, check its fixed rules and pitfalls one by one.",
        );
        parts.push(Part::Text(closing));
        self.request(parts)
    }

    /// Retouche asked by the user: `notes` = (time in s from the start of the
    /// scene, remark).
    pub fn revise(&self, brief: &str, html: &str, notes: &[(f64, String)]) -> Request {
        let mut list = String::new();
        for (t, note) in notes {
            let _ = writeln!(list, "- at {t:.2} s: {note}");
        }
        self.request(vec![Part::Text(format!(
            "Mode: RETOUCH.\n\n{brief}\n\n## Current composition\n\n```html\n{html}\n```\n\n\
             ## Retouches asked by the user\n\n{list}\n\
             Apply these retouches at the given time, without changing the rest, and return the \
             complete document."
        ))])
    }

    pub fn transition(&self, brief: &TransitionBrief) -> Request {
        self.request(vec![Part::Text(brief.render())])
    }
}

/// Verdict of a critique.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Keep,
    Redo,
}

/// Reads the verdict; without a `VERDICT` line, redo only if the answer
/// contains a new composition.
pub fn parse_verdict(reply: &str) -> Verdict {
    for line in reply.lines().take(10) {
        let upper = line.trim().trim_matches('*').to_uppercase();
        if let Some(rest) = upper.strip_prefix("VERDICT") {
            // `REFAIRE`: answers to the former French prompt.
            return if rest.contains("REDO") || rest.contains("REFAIRE") {
                Verdict::Redo
            } else {
                Verdict::Keep
            };
        }
    }
    if reply.contains("```html") {
        Verdict::Redo
    } else {
        Verdict::Keep
    }
}

/// Brief of a transition between two scenes.
#[derive(Debug, Clone)]
pub struct TransitionBrief {
    pub width: u32,
    pub height: u32,
    pub duration: f64,
    pub art_direction: String,
    pub description: String,
    pub from_intent: String,
    pub to_intent: String,
}

impl TransitionBrief {
    pub fn render(&self) -> String {
        format!(
            "Mode: CREATE (transition).\n\n# Transition between two scenes\n\n\
             Format: {}×{} px. Duration: **{:.3} s** (`window.enfait.duration = {:.3}`).{}\n\n\
             This composition connects two already rendered scenes, provided as images \
             (`img[data-enfait-scene=\"from\"]` and `img[data-enfait-scene=\"to\"]`, see the \
             contract). At t = 0 only `from` is visible; at the end, only `to`. Add your own \
             elements on top if the transition asks for it.\n\n\
             ## Art direction\n\n{}\n\n## Wanted transition\n\n{}\n\n\
             ## Outgoing scene\n\n{}\n\n## Incoming scene\n\n{}\n\nMake the composition.",
            self.width,
            self.height,
            self.duration,
            self.duration,
            format_notes(self.width, self.height),
            self.art_direction,
            self.description,
            self.from_intent,
            self.to_intent
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts() {
        assert_eq!(parse_verdict("VERDICT: KEEP\nGood."), Verdict::Keep);
        assert_eq!(parse_verdict("**VERDICT: REDO**\n```html\n"), Verdict::Redo);
        assert_eq!(parse_verdict("**VERDICT : REFAIRE**\n"), Verdict::Redo);
        assert_eq!(
            parse_verdict("Redoing it.\n```html\n<html></html>\n```"),
            Verdict::Redo
        );
        assert_eq!(parse_verdict("Perfect."), Verdict::Keep);
    }

    #[test]
    fn system_has_no_placeholder() {
        let s = animator_system("CONTRACT", "- Inter");
        assert!(!s.contains("{{"));
        assert!(s.contains("CONTRACT") && s.contains("Inter"));
    }
}
