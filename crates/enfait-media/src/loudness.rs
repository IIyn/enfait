//! Loudness normalisation with ffmpeg's `loudnorm`, in two passes: a
//! measurement of the sound, then a linear correction from that
//! measurement (no compression of the dynamics).

use crate::MediaError;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Target loudness (integrated LUFS): that of YouTube, Instagram and TikTok.
pub const TARGET_LUFS: f64 = -14.0;
/// Maximum true peak (dBTP).
pub const TRUE_PEAK: f64 = -1.0;
/// Under this loudness (LUFS) the sound is taken as silence and not raised.
pub const SILENCE_LUFS: f64 = -70.0;

/// Measurements of the first `loudnorm` pass, as ffmpeg writes them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measure {
    pub input_i: String,
    pub input_tp: String,
    pub input_lra: String,
    pub input_thresh: String,
    pub target_offset: String,
}

impl Measure {
    /// Integrated loudness before normalisation (LUFS), if readable.
    pub fn integrated(&self) -> Option<f64> {
        self.input_i.parse::<f64>().ok().filter(|i| i.is_finite())
    }

    /// Audio filter of the second pass, bringing the sound to `target`
    /// LUFS. `None` for a (nearly) silent sound, which is left as is.
    pub fn filter(&self, target: f64) -> Option<String> {
        let integrated = self.integrated()?;
        if integrated <= SILENCE_LUFS {
            return None;
        }
        Some(format!(
            "loudnorm=I={target}:TP={TRUE_PEAK}:LRA=11:measured_I={}:measured_TP={}:\
             measured_LRA={}:measured_thresh={}:offset={}:linear=true",
            self.input_i, self.input_tp, self.input_lra, self.input_thresh, self.target_offset
        ))
    }
}

/// Audio filter of the first pass (measurement towards `target` LUFS).
pub fn measure_filter(target: f64) -> String {
    format!("loudnorm=I={target}:TP={TRUE_PEAK}:LRA=11:print_format=json")
}

/// Arguments of a measurement of `input` (to give to `ffmpeg`, whose
/// messages on stderr then go to [`parse_measure`]). For callers that run
/// ffmpeg themselves, with their own timeout.
pub fn measure_args(input: &Path, target: f64) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = ["-nostdin", "-hide_banner", "-nostats", "-i"]
        .iter()
        .map(Into::into)
        .collect();
    args.push(input.as_os_str().to_owned());
    args.extend(
        ["-vn", "-af", &measure_filter(target), "-f", "null", "-"]
            .iter()
            .map(Into::into),
    );
    args
}

/// Measures the loudness of the sound of `input` (any media with sound).
pub fn measure(input: &Path, target: f64) -> Result<Measure, MediaError> {
    let output = enfait_config::process::command("ffmpeg")
        .args(measure_args(input, target))
        .output()
        .map_err(|e| MediaError::spawn("ffmpeg", &e))?;
    let text = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(MediaError::Tool {
            tool: "ffmpeg",
            message: text.trim().to_owned(),
        });
    }
    parse_measure(&text).ok_or_else(|| MediaError::Tool {
        tool: "ffmpeg",
        message: "loudness: unreadable measurement".into(),
    })
}

/// The JSON block `loudnorm` writes at the end of its messages.
pub fn parse_measure(text: &str) -> Option<Measure> {
    let start = text.rfind('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(text.get(start..=end)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTPUT: &str = "[Parsed_loudnorm_0 @ 0x5]\n{\n\t\"input_i\" : \"-23.54\",\n\t\"input_tp\" : \
                          \"-5.21\",\n\t\"input_lra\" : \"3.10\",\n\t\"input_thresh\" : \"-33.9\",\n\t\
                          \"output_i\" : \"-14.0\",\n\t\"target_offset\" : \"0.12\"\n}\n";

    #[test]
    fn loudnorm_output_is_read() {
        let m = parse_measure(OUTPUT).unwrap();
        assert_eq!(m.input_i, "-23.54");
        assert_eq!(m.target_offset, "0.12");
        assert_eq!(m.integrated(), Some(-23.54));
        assert!(parse_measure("no measurement").is_none());
    }

    #[test]
    fn second_pass_filter() {
        let m = parse_measure(OUTPUT).unwrap();
        let filter = m.filter(TARGET_LUFS).unwrap();
        assert!(filter.starts_with("loudnorm=I=-14:TP=-1:LRA=11:measured_I=-23.54:"));
        assert!(filter.ends_with("offset=0.12:linear=true"));
        let silent = Measure {
            input_i: "-inf".into(),
            ..m.clone()
        };
        assert_eq!(silent.filter(TARGET_LUFS), None);
        let quiet = Measure {
            input_i: "-75.0".into(),
            ..m
        };
        assert_eq!(quiet.filter(TARGET_LUFS), None);
    }

    #[test]
    fn measure_arguments() {
        let args = measure_args(Path::new("/a/b.mp4"), -14.0);
        let text: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(text.contains(&"/a/b.mp4".to_owned()));
        assert!(text.contains(&"loudnorm=I=-14:TP=-1:LRA=11:print_format=json".to_owned()));
        assert_eq!(text.last().map(String::as_str), Some("-"));
    }
}
