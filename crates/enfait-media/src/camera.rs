//! Filming with a camera (webcam) while the voice is recorded.
//!
//! ffmpeg captures the camera (v4l2 on Linux, DirectShow on Windows,
//! AVFoundation on macOS), crops it to the project's format (centred: a
//! vertical strip of a landscape webcam for 9:16) and writes two outputs:
//! the video file (H.264, no sound) and small mirrored frames on its
//! standard output, for a preview of exactly what is kept. The moment the first frame arrives is kept, so that the sound
//! recorded apart (see [`crate::record`]) can be aligned on it before both
//! are put together by [`mux`].

use crate::{Frame, MediaError};
use std::ffi::OsString;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// A camera the user can pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraDevice {
    /// What ffmpeg opens (`/dev/video0`, a DirectShow or AVFoundation
    /// name), kept in the settings.
    pub id: String,
    pub name: String,
}

/// Longest side of the preview frames.
const PREVIEW_SIDE: u32 = 320;
/// Largest picture height asked of the camera.
const MAX_HEIGHT: u32 = 1080;
/// Time allowed for the camera to deliver its first frame.
const STARTUP: Duration = Duration::from_secs(10);

/// Cameras of the system, in the order of the system (the first one is the
/// default).
pub fn cameras() -> Vec<CameraDevice> {
    platform::cameras()
}

/// Way of opening a camera: its format, then the device.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Mode {
    /// Compressed (`mjpeg`) rather than raw frames.
    mjpeg: bool,
    size: Option<(u32, u32)>,
}

/// Size of the preview frames for a picture of `aspect` (width, height):
/// even sides, the longest one [`PREVIEW_SIDE`].
fn preview_size((w, h): (u32, u32)) -> (u32, u32) {
    let (w, h) = (w.max(1), h.max(1));
    let even = |n: u32| (n / 2 * 2).max(2);
    if w >= h {
        (PREVIEW_SIDE, even(PREVIEW_SIDE * h / w))
    } else {
        (even(PREVIEW_SIDE * w / h), PREVIEW_SIDE)
    }
}

/// Filter cropping the centre of the picture to `aspect`, even sides.
fn crop_filter((w, h): (u32, u32)) -> String {
    format!("crop='trunc(min(iw,ih*{w}/{h})/2)*2':'trunc(min(ih,iw*{h}/{w})/2)*2'")
}

/// Biggest size not above [`MAX_HEIGHT`] in a list of `WxH` words; other
/// words are ignored.
fn best_size<'a>(words: impl IntoIterator<Item = &'a str>) -> Option<(u32, u32)> {
    words
        .into_iter()
        .filter_map(|w| {
            let (a, b) = w
                .trim_matches(|c: char| !c.is_ascii_digit())
                .split_once('x')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        })
        .filter(|&(_, h)| h <= MAX_HEIGHT)
        .max_by_key(|&(w, h): &(u32, u32)| u64::from(w) * u64::from(h))
}

/// Runs `ffmpeg` and returns what it wrote on stderr (device listings).
fn ffmpeg_stderr(args: &[&std::ffi::OsStr]) -> String {
    enfait_config::process::command("ffmpeg")
        .arg("-hide_banner")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stderr).into_owned())
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{CameraDevice, Mode, best_size, ffmpeg_stderr};
    use std::ffi::{OsStr, OsString};

    pub(super) fn cameras() -> Vec<CameraDevice> {
        let Ok(dir) = std::fs::read_dir("/sys/class/video4linux") else {
            return Vec::new();
        };
        let mut list: Vec<(u32, CameraDevice)> = dir
            .filter_map(Result::ok)
            .filter_map(|e| {
                let node = e.file_name().to_string_lossy().into_owned();
                let n: u32 = node.strip_prefix("video")?.parse().ok()?;
                let read = |f: &str| {
                    std::fs::read_to_string(e.path().join(f))
                        .map(|s| s.trim().to_owned())
                        .unwrap_or_default()
                };
                // A camera often has a second node for its metadata (index 1).
                if read("index") != "0" {
                    return None;
                }
                let name = read("name");
                let name = name.split_once(": ").map_or(name.as_str(), |(a, _)| a);
                Some((
                    n,
                    CameraDevice {
                        id: format!("/dev/{node}"),
                        name: if name.is_empty() {
                            node
                        } else {
                            name.to_owned()
                        },
                    },
                ))
            })
            .collect();
        list.sort_by_key(|(n, _)| *n);
        list.into_iter().map(|(_, c)| c).collect()
    }

    pub(super) fn mode(id: &str) -> Mode {
        let text = ffmpeg_stderr(&[
            OsStr::new("-f"),
            OsStr::new("v4l2"),
            OsStr::new("-list_formats"),
            OsStr::new("all"),
            OsStr::new("-i"),
            OsStr::new(id),
        ]);
        parse_formats(&text)
    }

    /// Reads `-list_formats all`: `Compressed: mjpeg : Motion-JPEG : 1280x720 640x480`.
    pub(super) fn parse_formats(text: &str) -> Mode {
        let sizes = |kind: &str, codec: &str| {
            text.lines()
                .filter(|l| l.contains(kind))
                .filter_map(|l| {
                    let mut parts = l.split(" : ");
                    let head = parts.next()?;
                    head.trim_end().ends_with(codec).then_some(())?;
                    best_size(parts.nth(1)?.split_whitespace())
                })
                .next()
        };
        if let Some(size) = sizes("Compressed:", "mjpeg") {
            Mode {
                mjpeg: true,
                size: Some(size),
            }
        } else {
            Mode {
                mjpeg: false,
                size: text
                    .lines()
                    .filter(|l| l.contains("Raw"))
                    .find_map(|l| best_size(l.rsplit(" : ").next()?.split_whitespace())),
            }
        }
    }

    pub(super) fn input(id: &str, mode: &Mode) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec!["-f".into(), "v4l2".into()];
        if mode.mjpeg {
            args.extend(["-input_format".into(), "mjpeg".into()]);
        }
        if let Some((w, h)) = mode.size {
            args.extend(["-video_size".into(), format!("{w}x{h}").into()]);
            args.extend(["-framerate".into(), "30".into()]);
        }
        args.extend(["-i".into(), id.into()]);
        args
    }
}

#[cfg(windows)]
mod platform {
    use super::{CameraDevice, Mode, best_size, ffmpeg_stderr};
    use std::ffi::{OsStr, OsString};

    pub(super) fn cameras() -> Vec<CameraDevice> {
        let text = ffmpeg_stderr(&[
            OsStr::new("-list_devices"),
            OsStr::new("true"),
            OsStr::new("-f"),
            OsStr::new("dshow"),
            OsStr::new("-i"),
            OsStr::new("dummy"),
        ]);
        parse_devices(&text)
    }

    /// Reads `-list_devices`: `[dshow @ …] "Integrated Camera" (video)`, or,
    /// with older ffmpeg, names under `DirectShow video devices`.
    pub(super) fn parse_devices(text: &str) -> Vec<CameraDevice> {
        let mut video_section = false;
        let mut list = Vec::new();
        for line in text.lines() {
            if line.contains("DirectShow video devices") {
                video_section = true;
                continue;
            }
            if line.contains("DirectShow audio devices") {
                video_section = false;
                continue;
            }
            if line.contains("Alternative name") {
                continue;
            }
            let (Some(a), Some(b)) = (line.find('"'), line.rfind('"')) else {
                continue;
            };
            if b <= a {
                continue;
            }
            let video = line.contains("(video)") || (video_section && !line.contains("(audio)"));
            if video {
                let name = line[a + 1..b].to_owned();
                list.push(CameraDevice {
                    id: name.clone(),
                    name,
                });
            }
        }
        list
    }

    pub(super) fn mode(id: &str) -> Mode {
        let device = format!("video={id}");
        let text = ffmpeg_stderr(&[
            OsStr::new("-list_options"),
            OsStr::new("true"),
            OsStr::new("-f"),
            OsStr::new("dshow"),
            OsStr::new("-i"),
            OsStr::new(&device),
        ]);
        let size = |codec: &str| {
            best_size(
                text.lines()
                    .filter(|l| l.contains(codec))
                    .filter_map(|l| l.split("max s=").nth(1)?.split_whitespace().next()),
            )
        };
        match size("vcodec=mjpeg") {
            Some(size) => Mode {
                mjpeg: true,
                size: Some(size),
            },
            None => Mode {
                mjpeg: false,
                size: size("pixel_format="),
            },
        }
    }

    pub(super) fn input(id: &str, mode: &Mode) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec![
            "-f".into(),
            "dshow".into(),
            "-rtbufsize".into(),
            "256M".into(),
        ];
        if mode.mjpeg {
            args.extend(["-vcodec".into(), "mjpeg".into()]);
        }
        if let Some((w, h)) = mode.size {
            args.extend(["-video_size".into(), format!("{w}x{h}").into()]);
            args.extend(["-framerate".into(), "30".into()]);
        }
        args.extend(["-i".into(), format!("video={id}").into()]);
        args
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{CameraDevice, Mode, ffmpeg_stderr};
    use std::ffi::{OsStr, OsString};

    pub(super) fn cameras() -> Vec<CameraDevice> {
        let text = ffmpeg_stderr(&[
            OsStr::new("-f"),
            OsStr::new("avfoundation"),
            OsStr::new("-list_devices"),
            OsStr::new("true"),
            OsStr::new("-i"),
            OsStr::new(""),
        ]);
        parse_devices(&text)
    }

    /// Reads `-list_devices`: `[AVFoundation indev @ …] [0] FaceTime HD Camera`
    /// under `AVFoundation video devices:`; screens are left out.
    pub(super) fn parse_devices(text: &str) -> Vec<CameraDevice> {
        let mut video_section = false;
        let mut list = Vec::new();
        for line in text.lines() {
            if line.contains("AVFoundation video devices") {
                video_section = true;
                continue;
            }
            if line.contains("AVFoundation audio devices") {
                video_section = false;
                continue;
            }
            if !video_section {
                continue;
            }
            let Some(rest) = line.rsplit_once("] [").map(|(_, r)| r) else {
                continue;
            };
            let Some((_, name)) = rest.split_once("] ") else {
                continue;
            };
            if name.starts_with("Capture screen") {
                continue;
            }
            list.push(CameraDevice {
                id: name.to_owned(),
                name: name.to_owned(),
            });
        }
        list
    }

    pub(super) fn mode(_: &str) -> Mode {
        Mode {
            mjpeg: false,
            size: Some((1280, 720)),
        }
    }

    pub(super) fn input(id: &str, mode: &Mode) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec![
            "-f".into(),
            "avfoundation".into(),
            "-framerate".into(),
            "30".into(),
        ];
        if let Some((w, h)) = mode.size {
            args.extend(["-video_size".into(), format!("{w}x{h}").into()]);
        }
        args.extend(["-i".into(), format!("{id}:none").into()]);
        args
    }
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
mod platform {
    use super::{CameraDevice, Mode};
    use std::ffi::OsString;

    pub(super) fn cameras() -> Vec<CameraDevice> {
        Vec::new()
    }

    pub(super) fn mode(_: &str) -> Mode {
        Mode::default()
    }

    pub(super) fn input(id: &str, _: &Mode) -> Vec<OsString> {
        vec!["-i".into(), id.into()]
    }
}

/// Last preview frame, shared with the interface.
#[derive(Clone, Default)]
pub struct Preview {
    frame: Arc<Mutex<Option<Arc<Frame>>>>,
}

impl Preview {
    pub fn latest(&self) -> Option<Arc<Frame>> {
        self.frame.lock().ok()?.clone()
    }
}

/// A camera being filmed.
pub struct CameraRecorder {
    child: Child,
    stdin: Option<ChildStdin>,
    preview: Preview,
    first_frame: Arc<OnceLock<Instant>>,
    reader: Option<JoinHandle<()>>,
    errors: Option<JoinHandle<String>>,
    path: PathBuf,
    device: String,
}

impl CameraRecorder {
    /// Starts filming with `camera` (a [`CameraDevice::id`], the first
    /// camera when `None` or no longer there) into `path` (Matroska, no
    /// sound), cropped to `aspect` (width, height: `(9, 16)` for a vertical
    /// video). Blocks until the first frame arrives: call it away from the
    /// interface thread.
    pub fn start(
        camera: Option<&str>,
        aspect: (u32, u32),
        path: &Path,
    ) -> Result<Self, MediaError> {
        let list = cameras();
        let device = camera
            .and_then(|id| {
                let found = list.iter().find(|c| c.id == id);
                if found.is_none() {
                    tracing::warn!("camera {id} not found; using the first one");
                }
                found
            })
            .or_else(|| list.first())
            .cloned()
            .ok_or_else(|| MediaError::Output("no camera found".into()))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mode = platform::mode(&device.id);
        tracing::info!("camera {} ({}): {mode:?}", device.name, device.id);
        match Self::spawn(&device, &mode, aspect, path) {
            Ok(recorder) => Ok(recorder),
            // The format read from the camera may be refused: let ffmpeg
            // choose.
            Err(e) if mode != Mode::default() => {
                tracing::warn!("camera with {mode:?}: {e}; trying its default format");
                Self::spawn(&device, &Mode::default(), aspect, path)
            }
            Err(e) => Err(e),
        }
    }

    fn spawn(
        device: &CameraDevice,
        mode: &Mode,
        aspect: (u32, u32),
        path: &Path,
    ) -> Result<Self, MediaError> {
        let crop = crop_filter(aspect);
        let (pw, ph) = preview_size(aspect);
        // The crop gives the exact ratio up to rounding: `pad` absorbs the
        // last pixel.
        let preview_filter = format!(
            "{crop},fps=15,hflip,scale={pw}:{ph}:force_original_aspect_ratio=decrease,\
             pad={pw}:{ph}:(ow-iw)/2:(oh-ih)/2"
        );
        let mut args: Vec<OsString> = vec![
            "-hide_banner".into(),
            "-v".into(),
            "error".into(),
            "-y".into(),
            "-thread_queue_size".into(),
            "512".into(),
        ];
        args.extend(platform::input(&device.id, mode));
        args.extend(["-map", "0:v:0", "-vf"].map(OsString::from));
        args.push(crop.into());
        args.extend(
            [
                "-c:v",
                "libx264",
                "-preset",
                "superfast",
                "-crf",
                "21",
                "-pix_fmt",
                "yuv420p",
                "-g",
                "30",
            ]
            .map(OsString::from),
        );
        args.push(path.into());
        args.extend(["-map", "0:v:0", "-vf"].map(OsString::from));
        args.push(preview_filter.into());
        args.extend(["-f", "rawvideo", "-pix_fmt", "bgra", "pipe:1"].map(OsString::from));
        let mut child = enfait_config::process::command("ffmpeg")
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| MediaError::spawn("ffmpeg", &e))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let errors = std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut s) = stderr {
                let _ = s.read_to_string(&mut text);
            }
            text
        });
        let preview = Preview::default();
        let first_frame = Arc::new(OnceLock::new());
        let reader = {
            let (slot, first) = (preview.frame.clone(), Arc::clone(&first_frame));
            std::thread::spawn(move || {
                let Some(mut out) = stdout else { return };
                let size = (pw * ph * 4) as usize;
                let mut index = 0i64;
                loop {
                    let mut data = vec![0u8; size];
                    if out.read_exact(&mut data).is_err() {
                        break;
                    }
                    first.get_or_init(Instant::now);
                    let frame = Arc::new(Frame {
                        index,
                        width: pw,
                        height: ph,
                        data,
                    });
                    index = index.saturating_add(1);
                    if let Ok(mut s) = slot.lock() {
                        *s = Some(frame);
                    }
                }
            })
        };
        let mut recorder = Self {
            child,
            stdin,
            preview,
            first_frame,
            reader: Some(reader),
            errors: Some(errors),
            path: path.to_path_buf(),
            device: device.name.clone(),
        };
        let deadline = Instant::now() + STARTUP;
        while recorder.first_frame.get().is_none() {
            if let Ok(Some(_)) = recorder.child.try_wait() {
                return Err(recorder.failure("the camera stopped"));
            }
            if Instant::now() > deadline {
                let _ = recorder.child.kill();
                return Err(recorder.failure("the camera sends no picture"));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(recorder)
    }

    /// Error from what ffmpeg wrote, after it ended.
    fn failure(&mut self, fallback: &str) -> MediaError {
        let _ = self.child.wait();
        let text = self
            .errors
            .take()
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        let text = text.trim();
        MediaError::Tool {
            tool: "ffmpeg",
            message: if text.is_empty() {
                fallback.to_owned()
            } else {
                text.lines().last().unwrap_or(fallback).to_owned()
            },
        }
    }

    pub fn preview(&self) -> Preview {
        self.preview.clone()
    }

    /// Moment the first frame arrived.
    pub fn first_frame(&self) -> Option<Instant> {
        self.first_frame.get().copied()
    }

    /// Name of the camera.
    pub fn device(&self) -> &str {
        &self.device
    }

    /// Stops filming (ffmpeg finishes the file) and returns its path.
    pub fn finish(mut self) -> Result<PathBuf, MediaError> {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.write_all(b"q");
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            match self.child.try_wait()? {
                Some(status) => break status,
                None if Instant::now() > deadline => {
                    tracing::warn!("ffmpeg does not stop: killed");
                    let _ = self.child.kill();
                    break self.child.wait()?;
                }
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        };
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        // Stopped by `q`, ffmpeg exits cleanly; the file is usable even after
        // a kill (Matroska).
        let usable = std::fs::metadata(&self.path).is_ok_and(|m| m.len() > 0);
        if status.success() || usable {
            Ok(self.path.clone())
        } else {
            Err(self.failure("the camera recording failed"))
        }
    }
}

impl Drop for CameraRecorder {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Puts the filmed `video` and the recorded `audio` together into `out`
/// (MP4, the video copied, the sound in AAC).
pub fn mux(video: &Path, audio: &Path, out: &Path) -> Result<(), MediaError> {
    let mut args: Vec<OsString> = vec!["-i".into(), video.into(), "-i".into(), audio.into()];
    args.extend(
        [
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-shortest",
            "-movflags",
            "+faststart",
        ]
        .map(OsString::from),
    );
    args.push(out.into());
    crate::ffmpeg::run(args, 0.0, &mut |_| {})
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn preview_follows_the_format() {
        assert_eq!(preview_size((16, 9)), (320, 180));
        assert_eq!(preview_size((9, 16)), (180, 320));
        assert_eq!(preview_size((1, 1)), (320, 320));
        assert!(crop_filter((9, 16)).contains("ih*9/16"));
    }

    #[test]
    fn picks_the_biggest_size_up_to_1080p() {
        assert_eq!(
            best_size("1280x720 640x360 3840x2160 1920x1080".split(' ')),
            Some((1920, 1080))
        );
        assert_eq!(best_size(["s=640x480", "junk"]), Some((640, 480)));
        assert_eq!(best_size(["{640-1920,+8}x{480-1080,+8}"]), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reads_v4l2_formats() {
        let text = "[in#0 @ 0x55] Compressed:       mjpeg :          Motion-JPEG : 1280x720 640x360 640x480\n\
                    [in#0 @ 0x55] Raw       :     yuyv422 :           YUYV 4:2:2 : 1920x1080 640x480\n";
        assert_eq!(
            platform::parse_formats(text),
            Mode {
                mjpeg: true,
                size: Some((1280, 720))
            }
        );
        let raw =
            "[in#0 @ 0x55] Raw       :     yuyv422 :           YUYV 4:2:2 : 640x480 320x240\n";
        assert_eq!(
            platform::parse_formats(raw),
            Mode {
                mjpeg: false,
                size: Some((640, 480))
            }
        );
    }

    #[cfg(windows)]
    #[test]
    fn reads_dshow_devices() {
        let text = "[dshow @ 0x1] \"Integrated Camera\" (video)\n\
                    [dshow @ 0x1]   Alternative name \"@device_pnp_x\"\n\
                    [dshow @ 0x1] \"Microphone (Realtek)\" (audio)\n";
        let list = platform::parse_devices(text);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Integrated Camera");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reads_avfoundation_devices() {
        let text = "[AVFoundation indev @ 0x1] AVFoundation video devices:\n\
                    [AVFoundation indev @ 0x1] [0] FaceTime HD Camera\n\
                    [AVFoundation indev @ 0x1] [1] Capture screen 0\n\
                    [AVFoundation indev @ 0x1] AVFoundation audio devices:\n\
                    [AVFoundation indev @ 0x1] [0] MacBook Pro Microphone\n";
        let list = platform::parse_devices(text);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "FaceTime HD Camera");
    }

    /// Films two seconds with the first camera (needs one):
    /// `cargo test -p enfait-media films -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs a camera"]
    #[allow(clippy::print_stdout)]
    fn films_with_the_camera() {
        println!("{:?}", cameras());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cam.mkv");
        let recorder = CameraRecorder::start(None, (9, 16), &path).unwrap();
        std::thread::sleep(Duration::from_secs(2));
        let frame = recorder.preview().latest().unwrap();
        assert_eq!((frame.width, frame.height), (180, 320));
        let video = recorder.finish().unwrap();
        let audio = dir.path().join("a.wav");
        crate::wav::write_wav(&audio, &vec![0.0; 48_000 * 2], 48_000).unwrap();
        let out = dir.path().join("take.mp4");
        mux(&video, &audio, &out).unwrap();
        let info = crate::probe::probe(&out).unwrap();
        println!("{info:?}");
        let video = info.video.unwrap();
        assert!(
            video.height > video.width,
            "{}x{}",
            video.width,
            video.height
        );
    }
}
