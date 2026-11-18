//! Erreurs du modèle.

use crate::id::BlockId;

/// Erreur d'un calcul de temps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TimeError {
    #[error("dépassement de capacité dans un calcul de temps")]
    Overflow,
    #[error("cadence invalide : {num}/{den}")]
    InvalidFrameRate { num: u32, den: u32 },
    #[error("fréquence d'échantillonnage nulle")]
    InvalidSampleRate,
    #[error("intervalle vide ou inversé")]
    EmptyRange,
    #[error("temps négatif")]
    Negative,
}

/// Erreur d'une édition du projet. Une édition qui échoue ne modifie rien.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    #[error("bloc introuvable : {0}")]
    BlockNotFound(BlockId),
    #[error("identifiant de bloc déjà utilisé : {0}")]
    DuplicateBlock(BlockId),
    #[error("le point de coupe doit être strictement à l'intérieur du bloc")]
    SplitOutsideBlock,
    #[error("aucun bloc suivant à fusionner")]
    NoNextBlock,
    #[error("les blocs ne se suivent pas sans trou")]
    NotContiguous,
    #[error("le moment doit tomber dans le bloc")]
    MomentOutsideBlock,
    #[error("un moment existe déjà à cet instant")]
    DuplicateMoment,
    #[error("moment introuvable")]
    MomentNotFound,
    #[error(transparent)]
    Time(#[from] TimeError),
}

/// Erreur de chargement ou de validation d'un projet.
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("version de format {found} non prise en charge (attendue : {expected})")]
    UnsupportedSchema { found: u32, expected: u32 },
    #[error("projet invalide : {0}")]
    Invalid(#[from] EditError),
    #[error("JSON invalide : {0}")]
    Json(#[from] serde_json::Error),
}
