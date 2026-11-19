//! Choosing the model provider.

use crate::anthropic::AnthropicClient;
use crate::claude_code::ClaudeCodeClient;
use crate::llm::{AiError, Reply, Request};
use crate::local::LocalClient;

pub enum Provider {
    /// Anthropic API with a key.
    Anthropic(AnthropicClient),
    /// Claude Code installed locally, with the user's account.
    ClaudeCode(ClaudeCodeClient),
    /// Local model, served by an OpenAI-compatible API.
    Local(LocalClient),
    /// Answers prepared in advance (runs without a model, tests).
    Canned(Canned),
}

/// Function answering a request without calling a model.
pub type Canned = std::sync::Arc<dyn Fn(&Request) -> Result<String, AiError> + Send + Sync>;

impl Provider {
    pub async fn complete(&self, request: &Request) -> Result<Reply, AiError> {
        let started = std::time::Instant::now();
        let reply = match self {
            Self::Anthropic(client) => client.complete(request).await,
            Self::ClaudeCode(client) => client.complete(request).await,
            Self::Local(client) => client.complete(request).await,
            Self::Canned(answer) => answer(request).map(|text| Reply {
                text,
                model: "canned".into(),
                usage: crate::Usage::default(),
            }),
        }?;
        tracing::debug!(
            model = %reply.model,
            secs = started.elapsed().as_secs(),
            input = reply.usage.input_tokens,
            output = reply.usage.output_tokens,
            cache_read = reply.usage.cache_read_input_tokens,
            "reply received"
        );
        Ok(reply)
    }

    pub const fn name(&self) -> &'static str {
        match self {
            Self::Anthropic(_) => "API Anthropic",
            Self::ClaudeCode(_) => "Claude Code",
            Self::Local(_) => "local model",
            Self::Canned(_) => "canned answers",
        }
    }
}

/// Which provider to use (user setting).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ProviderChoice {
    /// The API if an Anthropic key is set, else Claude Code.
    #[default]
    Auto,
    Api,
    ClaudeCode,
    /// Local model (see [`crate::local`]).
    Local,
}

impl ProviderChoice {
    pub const ALL: [Self; 4] = [Self::Auto, Self::Api, Self::ClaudeCode, Self::Local];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Api => "API",
            Self::ClaudeCode => "Claude Code",
            Self::Local => "Local",
        }
    }

    /// Creates the provider; `work`: working folder of Claude Code.
    pub fn make(self, work: &std::path::Path) -> Result<Provider, AiError> {
        let api = || AnthropicClient::from_config().map(Provider::Anthropic);
        let claude = || {
            enfait_config::process::which("claude")
                .map(|program| Provider::ClaudeCode(ClaudeCodeClient::new(program, work)))
                .ok_or_else(|| {
                    AiError::ClaudeCode(
                        "`claude` command not found: install Claude Code, or choose the API \
                         or a local model"
                            .into(),
                    )
                })
        };
        match self {
            Self::Api => api(),
            Self::ClaudeCode => claude(),
            Self::Local => LocalClient::from_config().map(Provider::Local),
            Self::Auto => {
                if enfait_config::secrets::is_set(enfait_config::Secret::Anthropic) {
                    api()
                } else {
                    claude()
                }
            }
        }
    }
}
