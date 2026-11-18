//! Requêtes et réponses, indépendantes du fournisseur.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

impl fmt::Display for Effort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Morceau du message utilisateur.
#[derive(Debug, Clone)]
pub enum Part {
    Text(String),
    /// Image JPEG ou PNG sur disque.
    Image(PathBuf),
}

/// Un échange en un seul tour : consigne système (mise en cache) et message.
#[derive(Debug, Clone)]
pub struct Request {
    pub model: String,
    pub effort: Effort,
    pub max_tokens: u32,
    pub system: String,
    pub user: Vec<Part>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    /// Coût annoncé par Claude Code, s'il est connu.
    pub cost_usd: f64,
}

impl Usage {
    pub fn add(&mut self, other: &Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_read_input_tokens = self
            .cache_read_input_tokens
            .saturating_add(other.cache_read_input_tokens);
        self.cache_creation_input_tokens = self
            .cache_creation_input_tokens
            .saturating_add(other.cache_creation_input_tokens);
        self.cost_usd += other.cost_usd;
    }
}

#[derive(Debug, Clone)]
pub struct Reply {
    pub text: String,
    pub model: String,
    pub usage: Usage,
}

#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("HTTP : {0}")]
    Http(#[from] reqwest::Error),
    #[error("l'API a répondu {status} : {message}")]
    Api { status: u16, message: String },
    #[error("le modèle a refusé la demande")]
    Refusal,
    #[error("réponse tronquée (max_tokens atteint)")]
    Truncated,
    #[error("Claude Code : {0}")]
    ClaudeCode(String),
    #[error("entrée-sortie : {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON : {0}")]
    Json(#[from] serde_json::Error),
    #[error("réponse inattendue : {0}")]
    Unexpected(String),
    #[error("ANTHROPIC_API_KEY absente")]
    MissingKey,
    #[error("{0}")]
    Config(String),
}
