use enfait_core::TimeError;

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("{tool} : {message}")]
    Tool { tool: &'static str, message: String },
    #[error("{0} introuvable : installer ffmpeg (ffmpeg et ffprobe dans le PATH)")]
    Missing(&'static str),
    #[error("analyse du média : {0}")]
    Probe(String),
    #[error("le média n'a pas de flux {0}")]
    NoStream(&'static str),
    #[error("fichier de cache invalide : {0}")]
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
    /// Erreur de lancement d'un outil : « introuvable » si le binaire manque.
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
