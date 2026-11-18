//! Outils d'assets d'enfait :
//!
//! - [`search`] : recherche d'images et de vidéos libres de droits
//!   (Wikimedia Commons, Openverse, Pexels, Pixabay, Freesound), avec la licence de
//!   chaque résultat, et téléchargement ;
//! - [`cutout`] : détourage d'une image ou des images d'une prise ;
//! - [`library`] : ajout d'un fichier à la bibliothèque de l'utilisateur.
//!
//! Les licences elles-mêmes (conditions, crédits) sont dans
//! `enfait_core::license`.

pub mod cutout;
pub mod library;
pub mod search;

#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error("réseau : {0}")]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Source(String),
    #[error("modèle de détourage : {0}")]
    Model(String),
    #[error("image : {0}")]
    Image(String),
}
