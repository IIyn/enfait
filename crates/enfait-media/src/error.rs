use enfait_core::TimeError;

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("{tool} : {message}")]
    Tool { tool: &'static str, message: String },
    #[error("{0} not found: install ffmpeg (ffmpeg and ffprobe in the PATH)")]
    Missing(&'static str),
    #[error("media analysis: {0}")]
    Probe(String),
    #[error("the media has no {0} stream")]
    NoStream(&'static str),
    #[error("invalid cache file: {0}")]
    Cache(String),
    #[error("sortie audio : {0}")]
    Output(String),
    #[error(transparent)]
    Time(#[from] TimeError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl MediaError {
    /// Error launching a tool: "not found" if the binary is missing.
    pub(crate) fn spawn(tool: &'static str, err: &std::io::Error) -> Self {
        if err.kind() == std::io::ErrorKind::NotFound {
            Self::Missing(tool)
        } else {
            Self::Tool {
                tool,
                message: err.to_string(),
            }
        }
    }
}
