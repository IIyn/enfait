//! Requests and replies, independent of the provider.

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

/// Part of the user message.
#[derive(Debug, Clone)]
pub enum Part {
    Text(String),
    /// JPEG or PNG image on disk.
    Image(PathBuf),
}

/// A single-turn exchange: system instruction (cached) and message.
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
    /// Cost reported by Claude Code, if known.
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
    #[error("the API answered {status}: {message}")]
    Api { status: u16, message: String },
    #[error("the model refused the request")]
    Refusal,
    #[error("truncated reply (max_tokens reached)")]
    Truncated,
    #[error("Claude Code : {0}")]
    ClaudeCode(String),
    #[error("input-output: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON : {0}")]
    Json(#[from] serde_json::Error),
    #[error("unexpected reply: {0}")]
    Unexpected(String),
    #[error("no Anthropic API key (Settings → Models, or ANTHROPIC_API_KEY)")]
    MissingKey,
    #[error("{0}")]
    Config(String),
    /// The local model sent nothing for `secs` seconds.
    #[error("the local model stopped answering ({secs} s without a word): too slow for this request")]
    TimedOut { secs: u64 },
}
