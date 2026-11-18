//! Choix du fournisseur de modèles.

use crate::anthropic::AnthropicClient;
use crate::claude_code::ClaudeCodeClient;
use crate::llm::{AiError, Reply, Request};
use crate::local::LocalClient;

pub enum Provider {
    /// API Anthropic avec une clé.
    Anthropic(AnthropicClient),
    /// Claude Code installé localement, avec le compte de l'utilisateur.
    ClaudeCode(ClaudeCodeClient),
    /// Modèle local, servi par une API compatible OpenAI.
    Local(LocalClient),
    /// Réponses préparées à l'avance (essais sans modèle, tests).
    Canned(Canned),
}

/// Fonction qui répond à une requête sans appeler de modèle.
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
                model: "préenregistré".into(),
                usage: crate::Usage::default(),
            }),
        }?;
        tracing::debug!(
            model = %reply.model,
            secs = started.elapsed().as_secs(),
            input = reply.usage.input_tokens,
            output = reply.usage.output_tokens,
            cache_read = reply.usage.cache_read_input_tokens,
            "réponse reçue"
        );
        Ok(reply)
    }

    pub const fn name(&self) -> &'static str {
        match self {
            Self::Anthropic(_) => "API Anthropic",
            Self::ClaudeCode(_) => "Claude Code",
            Self::Local(_) => "modèle local",
            Self::Canned(_) => "réponses préenregistrées",
        }
    }
}

/// Quel fournisseur utiliser (réglage de l'utilisateur).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ProviderChoice {
    /// L'API si `ANTHROPIC_API_KEY` existe, sinon Claude Code.
    #[default]
    Auto,
    Api,
    ClaudeCode,
    /// Modèle local (voir [`crate::local`]).
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

    /// Crée le fournisseur ; `work` : dossier de travail de Claude Code.
    pub fn make(self, work: &std::path::Path) -> Result<Provider, AiError> {
        let api = || AnthropicClient::from_config().map(Provider::Anthropic);
        let claude = || {
            enfait_config::process::which("claude")
                .map(|program| Provider::ClaudeCode(ClaudeCodeClient::new(program, work)))
                .ok_or_else(|| {
                    AiError::ClaudeCode(
                        "commande `claude` introuvable : installe Claude Code, ou choisis \
                         l'API ou un modèle local"
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
