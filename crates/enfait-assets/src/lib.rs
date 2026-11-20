//! enfait's asset tools:
//!
//! - [`search`]: royalty-free image and video search (Wikimedia Commons,
//!   Openverse, Pexels, Pixabay, Freesound), with the licence of each
//!   result, and download;
//! - [`cutout`]: cutout of an image or of the frames of a take;
//! - [`library`]: adding a file to the user's library;
//! - [`styles`]: styles saved by the user, to reuse in other projects.
//!
//! The licences themselves (terms, credits) live in
//! `enfait_core::license`.

pub mod cutout;
pub mod library;
pub mod search;
pub mod styles;

#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error("network: {0}")]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Source(String),
    #[error("cutout model: {0}")]
    Model(String),
    #[error("image : {0}")]
    Image(String),
}
