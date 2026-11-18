use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum MotionError {
    #[error("entrée-sortie : {0}")]
    Io(#[from] std::io::Error),
    #[error("navigateur : {0}")]
    Browser(String),
    #[error("installation de Chromium : {0}")]
    Fetch(String),
    #[error("téléchargement : {0}")]
    Http(#[from] reqwest::Error),
    #[error("archive : {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("chemin invalide : {0}")]
    Path(PathBuf),
    #[error("JSON : {0}")]
    Json(#[from] serde_json::Error),
    #[error("image : {0}")]
    Image(#[from] image::ImageError),
    #[error("base64 : {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("la composition ne respecte pas le contrat : {0}")]
    Contract(String),
}

impl From<chromiumoxide::error::CdpError> for MotionError {
    fn from(e: chromiumoxide::error::CdpError) -> Self {
        Self::Browser(e.to_string())
    }
}
