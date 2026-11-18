//! IA d'enfait.
//!
//! - [`llm`] : requêtes indépendantes du fournisseur ;
//! - [`anthropic`], [`claude_code`] et [`local`] (serveur compatible
//!   OpenAI : Ollama, llama.cpp…) : les fournisseurs, réunis dans
//!   [`Provider`] ;
//! - [`editor`] : la passe « monteur » (storyboard écrit par le modèle) ;
//! - [`direction`] : la passe « réalisateur » ;
//! - [`animate`] : les animateurs (création, correction, critique).

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

/// Tous les textes qui orientent les modèles (prompts, guide, styles), avec
/// leur nom : de quoi dater une passe d'évaluation par une empreinte.
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
