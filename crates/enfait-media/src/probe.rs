//! Analyse d'un média par ffprobe.

use crate::MediaError;
use enfait_core::FrameRate;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaInfo {
    /// Durée en secondes.
    pub duration: f64,
    pub video: Option<VideoInfo>,
    pub audio: Option<AudioInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoInfo {
    /// Dimensions affichées (rotation appliquée).
    pub width: u32,
    pub height: u32,
    pub frame_rate: FrameRate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioInfo {
    pub sample_rate: u32,
    pub channels: u16,
}

pub fn probe(path: &Path) -> Result<MediaInfo, MediaError> {
    let output = enfait_config::process::command("ffprobe")
        .args(["-v", "error", "-print_format", "json"])
        .args(["-show_format", "-show_streams"])
        .arg(path)
        .output()
        .map_err(|e| MediaError::spawn("ffprobe", &e))?;
    if !output.status.success() {
        return Err(MediaError::Tool {
            tool: "ffprobe",
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    parse(&String::from_utf8_lossy(&output.stdout))
}

/// Lit la sortie JSON de ffprobe.
pub fn parse(json: &str) -> Result<MediaInfo, MediaError> {
    let root: Value = serde_json::from_str(json)?;
    let streams = root
        .get("streams")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let kind =
        |s: &&Value, wanted: &str| s.get("codec_type").and_then(Value::as_str) == Some(wanted);
    // Une pochette d'album (mp3, m4a) se déclare comme un flux vidéo.
    let is_cover = |s: &&Value| {
        s.get("disposition")
            .and_then(|d| d.get("attached_pic"))
            .and_then(Value::as_i64)
            == Some(1)
    };
    let video = streams
        .iter()
        .find(|s| kind(s, "video") && !is_cover(s))
        .map(video_info)
        .transpose()?;
    let audio = streams
        .iter()
        .find(|s| kind(s, "audio"))
        .map(audio_info)
        .transpose()?;
    if video.is_none() && audio.is_none() {
        return Err(MediaError::Probe("ni image ni son".into()));
    }
    let duration = root
        .get("format")
        .and_then(|f| number(f.get("duration")))
        .or_else(|| streams.iter().find_map(|s| number(s.get("duration"))))
        .filter(|d| d.is_finite() && *d > 0.0)
        .ok_or_else(|| MediaError::Probe("durée inconnue".into()))?;
    Ok(MediaInfo {
        duration,
        video,
        audio,
    })
}

/// ffprobe écrit les nombres en chaînes (`"12.345"`).
fn number(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

fn unsigned(v: Option<&Value>) -> Option<u32> {
    v?.as_u64().and_then(|n| u32::try_from(n).ok())
}

fn rate(s: &str) -> Option<FrameRate> {
    let (num, den) = s.split_once('/')?;
    FrameRate::new(num.parse().ok()?, den.parse().ok()?).ok()
}

fn video_info(stream: &Value) -> Result<VideoInfo, MediaError> {
    let width = unsigned(stream.get("width")).filter(|w| *w > 0);
    let height = unsigned(stream.get("height")).filter(|h| *h > 0);
    let (Some(width), Some(height)) = (width, height) else {
        return Err(MediaError::Probe("dimensions de l'image inconnues".into()));
    };
    // La cadence moyenne reflète mieux les fichiers de téléphone (cadence
    // variable) ; `r_frame_rate` sert de repli.
    let frame_rate = ["avg_frame_rate", "r_frame_rate"]
        .iter()
        .find_map(|k| stream.get(*k).and_then(Value::as_str).and_then(rate))
        .filter(|r| (1..=240).contains(&r.nominal_fps()))
        .ok_or_else(|| MediaError::Probe("cadence d'images inconnue".into()))?;
    let rotated = rotation(stream).is_some_and(|r| r.rem_euclid(180) == 90);
    let (width, height) = if rotated {
        (height, width)
    } else {
        (width, height)
    };
    Ok(VideoInfo {
        width,
        height,
        frame_rate,
    })
}

/// Rotation en degrés : matrice d'affichage (récent) ou étiquette `rotate`.
fn rotation(stream: &Value) -> Option<i64> {
    let from_side_data = stream
        .get("side_data_list")
        .and_then(Value::as_array)
        .and_then(|list| list.iter().find_map(|d| number(d.get("rotation"))));
    #[allow(clippy::cast_possible_truncation)]
    from_side_data
        .or_else(|| number(stream.get("tags").and_then(|t| t.get("rotate"))))
        .map(|r| r.round() as i64)
}

fn audio_info(stream: &Value) -> Result<AudioInfo, MediaError> {
    let sample_rate = number(stream.get("sample_rate"))
        .filter(|r| *r >= 1.0)
        // Fréquence positive et réaliste : la conversion ne déborde pas.
        .map(|r| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let r = r.round() as u32;
            r
        })
        .ok_or_else(|| MediaError::Probe("fréquence audio inconnue".into()))?;
    let channels = stream
        .get("channels")
        .and_then(Value::as_u64)
        .and_then(|c| u16::try_from(c).ok())
        .filter(|c| *c > 0)
        .ok_or_else(|| MediaError::Probe("nombre de canaux inconnu".into()))?;
    Ok(AudioInfo {
        sample_rate,
        channels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phone_video_with_rotation() {
        let json = r#"{
          "streams": [
            {"codec_type":"video","width":1920,"height":1080,
             "avg_frame_rate":"30000/1001","r_frame_rate":"30/1",
             "side_data_list":[{"rotation":-90}]},
            {"codec_type":"audio","sample_rate":"48000","channels":2}
          ],
          "format": {"duration":"12.500000"}
        }"#;
        let info = parse(json).unwrap();
        assert_eq!(info.duration, 12.5);
        let v = info.video.unwrap();
        assert_eq!((v.width, v.height), (1080, 1920));
        assert_eq!(v.frame_rate, FrameRate::NTSC_30);
        assert_eq!(info.audio.unwrap().channels, 2);
    }

    #[test]
    fn audio_with_cover_art() {
        let json = r#"{
          "streams": [
            {"codec_type":"audio","sample_rate":"44100","channels":1},
            {"codec_type":"video","width":500,"height":500,"avg_frame_rate":"0/0",
             "r_frame_rate":"90000/1","disposition":{"attached_pic":1}}
          ],
          "format": {"duration":"3.0"}
        }"#;
        let info = parse(json).unwrap();
        assert!(info.video.is_none());
        assert_eq!(info.audio.unwrap().sample_rate, 44100);
    }

    #[test]
    fn rejects_empty_and_unknown_rate() {
        assert!(parse(r#"{"streams":[],"format":{"duration":"1"}}"#).is_err());
        let json = r#"{"streams":[{"codec_type":"video","width":10,"height":10,
            "avg_frame_rate":"0/0","r_frame_rate":"0/0"}],"format":{"duration":"1"}}"#;
        assert!(parse(json).is_err());
    }
}
