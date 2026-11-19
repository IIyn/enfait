use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum MotionError {
    #[error("input-output: {0}")]
    Io(#[from] std::io::Error),
    #[error("browser: {0}")]
    Browser(String),
    #[error("Chromium install: {0}")]
    Fetch(String),
    #[error("download: {0}")]
    Http(#[from] reqwest::Error),
    #[error("archive: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("invalid path: {0}")]
    Path(PathBuf),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("image: {0}")]
    Image(#[from] image::ImageError),
    #[error("base64: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("the composition does not follow the contract: {0}")]
    Contract(String),
}

impl From<chromiumoxide::error::CdpError> for MotionError {
    fn from(e: chromiumoxide::error::CdpError) -> Self {
        Self::Browser(e.to_string())
    }
}
