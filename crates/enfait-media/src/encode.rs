//! Encoding to MP4 (H.264 by x264, AAC sound) with ffmpeg as a subprocess.
//!
//! - [`X264`]: the video settings, as ffmpeg arguments, for callers that
//!   build their own filter graph;
//! - [`FrameEncoder`]: JPEG frames sent one by one (rendered compositions),
//!   with an optional sound track, normalised if asked (see
//!   [`crate::loudness`]).
//!
//! The file is written under a temporary name next to the output and
//! renamed at the end: an interrupted encoding never leaves a truncated
//! file under the final name.

use crate::MediaError;
use enfait_core::FrameRate;
use std::ffi::OsString;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::thread::JoinHandle;

/// x264 speed preset: slower gives a smaller file at the same quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Ultrafast,
    Superfast,
    Veryfast,
    Faster,
    Fast,
    Medium,
    Slow,
    Slower,
}

impl Preset {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ultrafast => "ultrafast",
            Self::Superfast => "superfast",
            Self::Veryfast => "veryfast",
            Self::Faster => "faster",
            Self::Fast => "fast",
            Self::Medium => "medium",
            Self::Slow => "slow",
            Self::Slower => "slower",
        }
    }
}

/// H.264 settings (x264, in software).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct X264 {
    pub preset: Preset,
    /// Constant quality, 0 (lossless) to 51; 18 is visually transparent.
    pub crf: u8,
}

impl X264 {
    /// Final export on a desktop machine.
    pub const FINAL: Self = Self {
        preset: Preset::Slow,
        crf: 16,
    };
    /// Fast encoding for a small machine (no hardware encoder).
    pub const FAST: Self = Self {
        preset: Preset::Veryfast,
        crf: 20,
    };

    /// Output arguments of the video stream.
    pub fn args(&self) -> Vec<String> {
        vec![
            "-c:v".into(),
            "libx264".into(),
            "-preset".into(),
            self.preset.as_str().into(),
            "-crf".into(),
            self.crf.min(51).to_string(),
            "-pix_fmt".into(),
            "yuv420p".into(),
        ]
    }
}

/// Output arguments of an AAC sound stream at 48 kHz.
pub fn aac_args(kbps: u32) -> Vec<String> {
    vec![
        "-ar".into(),
        "48000".into(),
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        format!("{kbps}k"),
    ]
}

/// Sound of an encoding: the first audio stream of a media.
#[derive(Debug, Clone)]
pub struct AudioTrack<'a> {
    pub path: &'a Path,
    /// Audio filter, for example the second pass of
    /// [`crate::loudness::Measure::filter`].
    pub filter: Option<String>,
    pub kbps: u32,
}

/// What to encode, and where.
#[derive(Debug, Clone)]
pub struct EncodeOptions<'a> {
    pub frame_rate: FrameRate,
    pub video: X264,
    pub audio: Option<AudioTrack<'a>>,
    /// Video filter applied to the frames (post-FX, scaling…).
    pub video_filter: Option<&'a str>,
    /// Duration of the output in seconds (else: that of the frames).
    pub duration: Option<f64>,
    pub out: &'a Path,
}

/// Temporary name of an output being written, same extension (ffmpeg
/// deduces the container from it): `film.mp4` → `film.part.mp4`.
pub fn partial_path(out: &Path) -> PathBuf {
    let stem = out
        .file_stem()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    let mut name = stem;
    name.push(".part");
    if let Some(ext) = out.extension() {
        name.push(".");
        name.push(ext);
    }
    out.with_file_name(name)
}

/// Full ffmpeg argument list of an encoding of frames read on stdin.
fn arguments(options: &EncodeOptions<'_>, partial: &Path) -> Vec<OsString> {
    let rate = format!("{}/{}", options.frame_rate.num(), options.frame_rate.den());
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |items: &[&str]| args.extend(items.iter().map(OsString::from));
    push(&["-y", "-nostdin", "-hide_banner", "-v", "error"]);
    push(&[
        "-f",
        "image2pipe",
        "-framerate",
        &rate,
        "-c:v",
        "mjpeg",
        "-i",
        "-",
    ]);
    if let Some(audio) = &options.audio {
        args.push("-i".into());
        args.push(audio.path.as_os_str().to_owned());
    }
    let mut push = |items: &[String]| args.extend(items.iter().map(OsString::from));
    if let Some(duration) = options.duration.filter(|d| d.is_finite() && *d > 0.0) {
        push(&["-t".into(), format!("{duration:.3}")]);
    }
    push(&["-map".into(), "0:v:0".into()]);
    if let Some(filter) = options.video_filter {
        push(&["-vf".into(), filter.into()]);
    }
    push(&options.video.args());
    if let Some(audio) = &options.audio {
        push(&["-map".into(), "1:a:0".into()]);
        if let Some(filter) = &audio.filter {
            push(&["-af".into(), filter.clone()]);
        }
        push(&aac_args(audio.kbps));
    }
    push(&["-movflags".into(), "+faststart".into()]);
    args.push(partial.as_os_str().to_owned());
    args
}

/// An ffmpeg process receiving JPEG frames. Dropping it without
/// [`FrameEncoder::finish`] kills ffmpeg and removes the partial file.
pub struct FrameEncoder {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stderr: Option<JoinHandle<String>>,
    partial: PathBuf,
    out: PathBuf,
    frames: u64,
}

impl FrameEncoder {
    pub fn start(options: &EncodeOptions<'_>) -> Result<Self, MediaError> {
        let partial = partial_path(options.out);
        let mut child = enfait_config::process::command("ffmpeg")
            .args(arguments(options, &partial))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| MediaError::spawn("ffmpeg", &e))?;
        // stderr is read separately so that ffmpeg never blocks on a full pipe.
        let stderr = child.stderr.take().map(|mut s| {
            std::thread::spawn(move || {
                let mut text = String::new();
                let _ = s.read_to_string(&mut text);
                text
            })
        });
        let stdin = child.stdin.take();
        Ok(Self {
            child: Some(child),
            stdin,
            stderr,
            partial,
            out: options.out.to_path_buf(),
            frames: 0,
        })
    }

    /// Number of frames sent so far.
    pub const fn frames(&self) -> u64 {
        self.frames
    }

    /// Sends the next frame (a JPEG file's bytes).
    pub fn push(&mut self, jpeg: &[u8]) -> Result<(), MediaError> {
        let written = match self.stdin.as_mut() {
            Some(stdin) => stdin.write_all(jpeg),
            None => Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe)),
        };
        match written {
            Ok(()) => {
                self.frames = self.frames.saturating_add(1);
                Ok(())
            }
            // ffmpeg stopped: its own message says why.
            Err(e) => Err(self.wait().err().unwrap_or(MediaError::Io(e))),
        }
    }

    /// Closes the input, waits for ffmpeg and gives the file its final name.
    pub fn finish(mut self) -> Result<PathBuf, MediaError> {
        self.wait()?;
        std::fs::rename(&self.partial, &self.out)?;
        Ok(self.out.clone())
    }

    /// Kills ffmpeg (for a timeout); the partial file is removed.
    pub fn kill(mut self) {
        self.abort();
    }

    fn wait(&mut self) -> Result<(), MediaError> {
        drop(self.stdin.take());
        let Some(mut child) = self.child.take() else {
            return Err(MediaError::Tool {
                tool: "ffmpeg",
                message: "encoding already finished".into(),
            });
        };
        let status = child.wait()?;
        let errors = self
            .stderr
            .take()
            .and_then(|t| t.join().ok())
            .unwrap_or_default();
        if status.success() {
            Ok(())
        } else {
            let _ = std::fs::remove_file(&self.partial);
            Err(MediaError::Tool {
                tool: "ffmpeg",
                message: errors.trim().to_owned(),
            })
        }
    }

    fn abort(&mut self) {
        drop(self.stdin.take());
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&self.partial);
        }
    }
}

impl Drop for FrameEncoder {
    fn drop(&mut self) {
        self.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn partial_keeps_the_extension() {
        assert_eq!(
            partial_path(Path::new("/v/render.mp4")),
            PathBuf::from("/v/render.part.mp4")
        );
        assert_eq!(
            partial_path(Path::new("/v/render")),
            PathBuf::from("/v/render.part")
        );
    }

    #[test]
    fn x264_arguments() {
        let args = X264::FAST.args();
        assert_eq!(
            args,
            [
                "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p"
            ]
        );
        let capped = X264 {
            preset: Preset::Slow,
            crf: 90,
        };
        assert!(capped.args().contains(&"51".to_owned()));
    }

    #[test]
    fn arguments_with_and_without_sound() {
        let out = Path::new("/v/o.mp4");
        let mut options = EncodeOptions {
            frame_rate: FrameRate::new(30, 1).unwrap(),
            video: X264::FAST,
            audio: None,
            video_filter: None,
            duration: Some(2.5),
            out,
        };
        let silent = text(&arguments(&options, &partial_path(out)));
        assert!(silent.contains(&"30/1".to_owned()));
        assert!(silent.contains(&"2.500".to_owned()));
        assert!(!silent.contains(&"1:a:0".to_owned()));
        assert_eq!(silent.last().map(String::as_str), Some("/v/o.part.mp4"));

        options.audio = Some(AudioTrack {
            path: Path::new("/v/src.mp4"),
            filter: Some("loudnorm=I=-14".into()),
            kbps: 192,
        });
        options.duration = None;
        let with_sound = text(&arguments(&options, &partial_path(out)));
        let pos = |s: &str| with_sound.iter().position(|a| a == s).unwrap();
        assert!(pos("/v/src.mp4") > pos("-"), "sound is the second input");
        assert!(pos("1:a:0") < pos("loudnorm=I=-14"));
        assert!(with_sound.contains(&"192k".to_owned()));
        assert!(!with_sound.contains(&"-t".to_owned()));
    }
}
