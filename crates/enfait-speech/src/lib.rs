//! Speech: from the recording to dated words, then to storyboard blocks.

pub mod audio;
pub mod cleanup;
pub mod script;
pub mod segment;
pub mod takes;
pub mod transcribe;

pub use audio::{Pcm, decode_pcm};

/// Default whisper model (downloaded by the onboarding or
/// `enfait-cli setup` into `<cache>/models/`).
pub const MODEL_FILE: &str = "ggml-large-v3-turbo-q5_0.bin";
/// Where to download [`MODEL_FILE`] (≈ 550 MB).
pub const MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin";
pub use segment::{SegmentParams, segment};
pub use transcribe::{TranscribeOptions, transcribe};

#[derive(Debug, thiserror::Error)]
pub enum SpeechError {
    #[error("ffmpeg not found or failed: {0}")]
    Ffmpeg(String),
    #[error("input-output: {0}")]
    Io(#[from] std::io::Error),
    #[error("whisper : {0}")]
    Whisper(#[from] whisper_rs::WhisperError),
    #[error("temps : {0}")]
    Time(#[from] enfait_core::TimeError),
    #[error("no word recognised")]
    NoSpeech,
}
