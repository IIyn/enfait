//! Blocs du storyboard : une scène de la vidéo.

use crate::id::BlockId;
use crate::time::{Flicks, TimeRange};
use serde::{Deserialize, Serialize};

/// Ce qu'on voit pendant le bloc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    /// Le rush tel quel, sans animation. Ne passe pas par un modèle.
    Rush,
    /// Animation seule, sans face cam.
    #[default]
    Animation,
    /// Face cam avec une animation par-dessus.
    Mixed,
    /// Effet appliqué à la face cam.
    Effect,
    /// Plan d'illustration (b-roll) : une vidéo ou une image de la
    /// bibliothèque, montée sans modèle.
    Broll,
}

/// Passage d'un bloc au suivant.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(tag = "type", content = "description", rename_all = "snake_case")]
pub enum Transition {
    /// Coupe nette.
    #[default]
    Cut,
    /// L'agent choisit la transition.
    Auto,
    /// Transition décrite par l'utilisateur.
    Described(String),
}

/// Un moment précis : ce qui doit se passer à un instant de la voix
/// (« sur "aiguilleur", l'aiguillage bascule »).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Moment {
    /// Instant, dans l'intervalle du bloc.
    pub at: Flicks,
    /// Mot prononcé à cet instant : repère lisible.
    pub word: String,
    /// Ce qui se passe.
    pub note: String,
}

/// Le plan d'un bloc « B-roll » : un fichier de la bibliothèque.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Broll {
    /// Nom du fichier dans la bibliothèque.
    pub file: String,
    /// Instant de la vidéo où commence le plan (zéro pour une image).
    #[serde(default)]
    pub start: Flicks,
}

/// Une scène : un intervalle de la voix, ce qu'on y voit, et la transition qui suit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub id: BlockId,
    pub range: TimeRange,
    pub kind: BlockKind,
    /// Description libre de ce qui doit se passer à l'écran.
    pub description: String,
    pub transition_after: Transition,
    /// Moments précis, triés par instant, tous dans `range`.
    #[serde(default)]
    pub moments: Vec<Moment>,
    /// Plan montré par un bloc « B-roll ».
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broll: Option<Broll>,
    /// Bloc « Mixte » : la personne filmée est détourée, l'animation peut
    /// passer derrière elle.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cutout: bool,
}

impl Block {
    pub fn new(range: TimeRange, kind: BlockKind) -> Self {
        Self {
            id: BlockId::new(),
            range,
            kind,
            description: String::new(),
            transition_after: Transition::Cut,
            moments: Vec::new(),
            broll: None,
            cutout: false,
        }
    }
}
