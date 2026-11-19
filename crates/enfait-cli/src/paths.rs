//! Locations of the downloaded tools and the working folders.

use anyhow::{Context as _, Result};
use std::path::{Path, PathBuf};

pub const WHISPER_MODEL: &str = enfait_speech::MODEL_FILE;
pub const WHISPER_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin";

pub struct Dirs {
    pub cache: PathBuf,
}

impl Dirs {
    pub fn new() -> Result<Self> {
        let dirs =
            directories::ProjectDirs::from("", "", "enfait").context("no home folder found")?;
        Ok(Self {
            cache: dirs.cache_dir().to_path_buf(),
        })
    }

    pub fn chromium(&self) -> PathBuf {
        self.cache.join("chromium")
    }

    pub fn browser_profile(&self) -> PathBuf {
        self.cache.join("profile")
    }

    /// Cache of the prepared media, shared with the app.
    pub fn media(&self) -> PathBuf {
        self.cache.join("media")
    }

    pub fn models(&self) -> PathBuf {
        self.cache.join("models")
    }

    pub fn whisper_model(&self) -> PathBuf {
        self.models().join(WHISPER_MODEL)
    }
}

pub use enfait_config::process::which as find_in_path;

/// Default working folder: `<voice>.enfait/` next to the file.
pub fn default_work_dir(voice: &Path) -> PathBuf {
    let stem = voice
        .file_stem()
        .map_or_else(|| "projet".into(), |s| s.to_string_lossy().into_owned());
    voice
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{stem}.enfait"))
}
