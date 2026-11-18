//! Le « look » d'un projet : style des animations, effets globaux appliqués
//! à toute la vidéo (post-FX) et bibliothèque de l'utilisateur.

use crate::license::Origin;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Look {
    pub style: StyleChoice,
    pub fx: PostFx,
    pub assets: Vec<Asset>,
    /// Règles retenues à partir des retouches (« jamais de rose »),
    /// appliquées à toutes les animations.
    pub memory: Vec<String>,
}

impl Look {
    /// Règles de style données aux animateurs : celles de l'utilisateur puis
    /// les règles retenues.
    pub fn rules(&self) -> Option<String> {
        let mut rules = self.style.rules.trim().to_owned();
        if !self.memory.is_empty() {
            if !rules.is_empty() {
                rules.push_str("\n\n");
            }
            rules.push_str("Règles retenues lors des retouches :\n");
            for rule in &self.memory {
                rules.push_str("- ");
                rules.push_str(rule.trim());
                rules.push('\n');
            }
        }
        (!rules.trim().is_empty()).then_some(rules)
    }
}

/// Style des animations : un style prédéfini (voir `enfait-ai`) et des
/// règles personnelles, prioritaires.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StyleChoice {
    /// Identifiant du style prédéfini (`gravure`, `craie`…) ; aucun : libre.
    pub preset: Option<String>,
    /// Règles de l'utilisateur, en texte libre.
    pub rules: String,
}

/// Effets appliqués à toute la vidéo. Toutes les valeurs à 0 : aucun effet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PostFx {
    /// Grain de pellicule, 0 à 1.
    pub grain: f32,
    /// Assombrissement des bords, 0 à 1.
    pub vignette: f32,
    /// −1 à 1.
    pub contrast: f32,
    /// −1 (noir et blanc) à 1.
    pub saturation: f32,
    /// −1 (froid) à 1 (chaud).
    pub temperature: f32,
    /// Décalage des couches rouge et bleue, 0 à 1.
    pub aberration: f32,
}

impl PostFx {
    pub const NEUTRAL: Self = Self {
        grain: 0.0,
        vignette: 0.0,
        contrast: 0.0,
        saturation: 0.0,
        temperature: 0.0,
        aberration: 0.0,
    };

    /// Valeurs ramenées dans leurs bornes (NaN : 0).
    #[must_use]
    pub fn clamped(self) -> Self {
        let c = |v: f32, lo: f32| if v.is_nan() { 0.0 } else { v.clamp(lo, 1.0) };
        Self {
            grain: c(self.grain, 0.0),
            vignette: c(self.vignette, 0.0),
            contrast: c(self.contrast, -1.0),
            saturation: c(self.saturation, -1.0),
            temperature: c(self.temperature, -1.0),
            aberration: c(self.aberration, 0.0),
        }
    }

    pub fn is_neutral(&self) -> bool {
        let fx = self.clamped();
        [
            fx.grain,
            fx.vignette,
            fx.contrast,
            fx.saturation,
            fx.temperature,
            fx.aberration,
        ]
        .iter()
        .all(|v| v.abs() < 1e-3)
    }
}

/// Un look prêt à l'emploi.
pub struct FxPreset {
    pub slug: &'static str,
    pub name: &'static str,
    pub fx: PostFx,
}

pub const FX_PRESETS: &[FxPreset] = &[
    FxPreset {
        slug: "neutre",
        name: "Neutre",
        fx: PostFx::NEUTRAL,
    },
    FxPreset {
        slug: "pellicule",
        name: "Pellicule",
        fx: PostFx {
            grain: 0.45,
            vignette: 0.35,
            contrast: 0.1,
            saturation: -0.15,
            temperature: 0.25,
            aberration: 0.0,
        },
    },
    FxPreset {
        slug: "archive",
        name: "Archive",
        fx: PostFx {
            grain: 0.65,
            vignette: 0.5,
            contrast: 0.15,
            saturation: -0.6,
            temperature: 0.4,
            aberration: 0.0,
        },
    },
    FxPreset {
        slug: "neon",
        name: "Néon",
        fx: PostFx {
            grain: 0.1,
            vignette: 0.25,
            contrast: 0.25,
            saturation: 0.35,
            temperature: -0.2,
            aberration: 0.4,
        },
    },
    FxPreset {
        slug: "doux",
        name: "Doux",
        fx: PostFx {
            grain: 0.15,
            vignette: 0.15,
            contrast: -0.2,
            saturation: -0.1,
            temperature: 0.1,
            aberration: 0.0,
        },
    },
    FxPreset {
        slug: "noir-blanc",
        name: "Noir et blanc",
        fx: PostFx {
            grain: 0.3,
            vignette: 0.3,
            contrast: 0.2,
            saturation: -1.0,
            temperature: 0.0,
            aberration: 0.0,
        },
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Image,
    Svg,
    Video,
    Font,
    /// Musique ou effet sonore (finition).
    Audio,
}

impl AssetKind {
    /// Type d'après l'extension d'un fichier (sans le point).
    pub fn from_extension(ext: &str) -> Option<Self> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "png" | "jpg" | "jpeg" | "webp" | "gif" | "avif" => Self::Image,
            "svg" => Self::Svg,
            "mp4" | "mov" | "webm" | "m4v" | "ogv" => Self::Video,
            "ttf" | "otf" | "woff" | "woff2" => Self::Font,
            "mp3" | "wav" | "ogg" | "oga" | "flac" | "m4a" | "aac" | "opus" => Self::Audio,
            _ => return None,
        })
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Svg => "SVG",
            Self::Video => "vidéo",
            Self::Font => "police",
            Self::Audio => "son",
        }
    }
}

/// Un fichier de la bibliothèque de l'utilisateur.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    /// Nom du fichier, dans le dossier de la bibliothèque.
    pub file: String,
    pub kind: AssetKind,
    /// Nom affiché ; pour une police, la famille (`font-family`).
    pub name: String,
    /// À quoi il sert, pour les animateurs (« logo de la marque, à mettre
    /// en fin de vidéo »).
    #[serde(default)]
    pub description: String,
    /// Banque, auteur et licence d'un fichier trouvé par la recherche ;
    /// aucun : fichier de l'utilisateur.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_and_clamping() {
        assert!(PostFx::default().is_neutral());
        let wild = PostFx {
            grain: 3.0,
            contrast: -7.0,
            saturation: f32::NAN,
            ..PostFx::NEUTRAL
        }
        .clamped();
        assert_eq!(
            (wild.grain, wild.contrast, wild.saturation),
            (1.0, -1.0, 0.0)
        );
        assert!(!wild.is_neutral());
        for p in FX_PRESETS {
            assert_eq!(p.fx, p.fx.clamped(), "{} hors bornes", p.slug);
        }
    }

    #[test]
    fn asset_kinds_and_json() {
        assert_eq!(AssetKind::from_extension("SVG"), Some(AssetKind::Svg));
        assert_eq!(AssetKind::from_extension("woff2"), Some(AssetKind::Font));
        assert_eq!(AssetKind::from_extension("pdf"), None);
        assert_eq!(AssetKind::from_extension("MP3"), Some(AssetKind::Audio));
        let look: Look = serde_json::from_str(r#"{"fx":{"grain":0.5}}"#).unwrap();
        assert_eq!(look.fx.grain, 0.5);
        assert!(look.style.preset.is_none() && look.assets.is_empty());
    }
}
