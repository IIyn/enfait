//! Audio decoding by ffmpeg (subprocess).

use crate::SpeechError;
use enfait_core::Flicks;
use std::path::Path;

/// Sample rate expected by whisper.
pub const WHISPER_RATE: u32 = 16_000;

/// Mono float samples.
pub struct Pcm {
    pub samples: Vec<f32>,
    pub rate: u32,
}

impl Pcm {
    pub fn duration(&self) -> Result<Flicks, SpeechError> {
        let n = i64::try_from(self.samples.len()).map_err(|_| enfait_core::TimeError::Overflow)?;
        Ok(Flicks::from_samples(n, self.rate)?)
    }
}

/// Decodes any media to mono 16 kHz.
pub fn decode_pcm(input: &Path) -> Result<Pcm, SpeechError> {
    let output = enfait_config::process::command("ffmpeg")
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(input)
        .args([
            "-vn",
            "-f",
            "f32le",
            "-acodec",
            "pcm_f32le",
            "-ac",
            "1",
            "-ar",
        ])
        .arg(WHISPER_RATE.to_string())
        .arg("-")
        .output()
        .map_err(|e| SpeechError::Ffmpeg(e.to_string()))?;
    if !output.status.success() {
        return Err(SpeechError::Ffmpeg(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    let samples = output
        .stdout
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
        .collect();
    Ok(Pcm {
        samples,
        rate: WHISPER_RATE,
    })
}
