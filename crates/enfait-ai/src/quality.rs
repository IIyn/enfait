//! Quality levels: model, effort and number of critique passes.

use crate::llm::Effort;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    /// Smallest model, one pass, no visual critique: drafts and simple
    /// blocks.
    #[serde(alias = "leger")]
    Light,
    /// One pass, no visual critique.
    #[serde(alias = "rapide")]
    Fast,
    /// Two critiques on rendered frames.
    #[default]
    #[serde(alias = "soigne")]
    Polished,
    /// Four critiques, maximum effort.
    Ultra,
}

#[derive(Debug, Clone)]
pub struct QualitySettings {
    pub model: String,
    pub effort: Effort,
    pub critique_rounds: u8,
}

impl Quality {
    pub fn settings(self) -> QualitySettings {
        match self {
            Self::Light => QualitySettings {
                model: "claude-haiku-5-5".into(),
                effort: Effort::Low,
                critique_rounds: 0,
            },
            Self::Fast => QualitySettings {
                model: "claude-sonnet-5-5".into(),
                effort: Effort::Low,
                critique_rounds: 0,
            },
            Self::Polished => QualitySettings {
                model: "claude-opus-5-5".into(),
                effort: Effort::High,
                critique_rounds: 2,
            },
            Self::Ultra => QualitySettings {
                model: "claude-opus-5-5".into(),
                effort: Effort::Xhigh,
                critique_rounds: 4,
            },
        }
    }
}
