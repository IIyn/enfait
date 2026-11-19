//! enfait's AI.
//!
//! - [`llm`]: provider-independent requests;
//! - [`anthropic`], [`claude_code`] and [`local`] (OpenAI-compatible
//!   server: Ollama, llama.cpp…): the providers, gathered in
//!   [`Provider`];
//! - [`editor`]: the "editor" pass (storyboard written by the model);
//! - [`direction`]: the "director" pass;
//! - [`animate`]: the animators (creation, fix, critique).

pub mod animate;
pub mod anthropic;
pub mod claude_code;
pub mod direction;
pub mod editor;
pub mod llm;
pub mod local;
pub mod provider;
pub mod quality;
pub mod styles;
mod text;

pub use llm::{AiError, Effort, Part, Reply, Request, Usage};
pub use provider::{Provider, ProviderChoice};
pub use quality::{Quality, QualitySettings};

/// All the texts steering the models (prompts, guide, styles), with their
/// name: enough to date an evaluation pass by a fingerprint.
pub fn prompt_sources() -> Vec<(&'static str, &'static str)> {
    let mut sources = vec![
        ("director", direction::DIRECTOR),
        ("editor", editor::EDITOR),
        ("animator", animate::ANIMATOR),
        ("style_guide", direction::STYLE_GUIDE),
    ];
    sources.extend(styles::PRESETS.iter().map(|p| (p.slug, p.rules)));
    sources
}
