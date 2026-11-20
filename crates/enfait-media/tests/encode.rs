//! Encoding frames and sound with the real ffmpeg (skipped without it).

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use enfait_core::FrameRate;
use enfait_media::encode::{AudioTrack, EncodeOptions, FrameEncoder, X264};
use enfait_media::loudness;
use std::path::Path;
use std::process::Command;

fn has_ffmpeg() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn ffmpeg(args: &[&str]) {
    let status = Command::new("ffmpeg")
        .args(["-y", "-v", "error"])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success());
}

fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn encodes_frames_with_normalised_sound() {
    if !has_ffmpeg() {
        eprintln!("ffmpeg missing: test skipped");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let frame = dir.path().join("f.jpg");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "color=c=teal:s=64x48",
        "-frames:v",
        "1",
        path(&frame),
    ]);
    // A quiet tone (about −26 LUFS), 3 s.
    let tone = dir.path().join("tone.wav");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=3",
        "-af",
        "volume=0.05",
        path(&tone),
    ]);
    let measure = loudness::measure(&tone, loudness::TARGET_LUFS).unwrap();
    let before = measure.integrated().unwrap();
    assert!(before < -20.0, "{before}");

    let out = dir.path().join("out.mp4");
    let jpeg = std::fs::read(&frame).unwrap();
    let mut encoder = FrameEncoder::start(&EncodeOptions {
        frame_rate: FrameRate::new(25, 1).unwrap(),
        video: X264::FAST,
        audio: Some(AudioTrack {
            path: &tone,
            filter: measure.filter(loudness::TARGET_LUFS),
            kbps: 128,
        }),
        video_filter: None,
        duration: Some(3.0),
        out: &out,
    })
    .unwrap();
    for _ in 0..75 {
        encoder.push(&jpeg).unwrap();
    }
    assert_eq!(encoder.frames(), 75);
    assert_eq!(encoder.finish().unwrap(), out);

    let info = enfait_media::probe::probe(&out).unwrap();
    assert!((info.duration - 3.0).abs() < 0.15, "{}", info.duration);
    assert_eq!(info.video.unwrap().width, 64);
    assert!(info.audio.is_some());
    let after = loudness::measure(&out, loudness::TARGET_LUFS)
        .unwrap()
        .integrated()
        .unwrap();
    assert!((after - loudness::TARGET_LUFS).abs() < 1.5, "{after}");
    assert!(!dir.path().join("out.part.mp4").exists());
}

#[test]
fn dropping_an_encoder_leaves_no_file() {
    if !has_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("never.mp4");
    let encoder = FrameEncoder::start(&EncodeOptions {
        frame_rate: FrameRate::new(30, 1).unwrap(),
        video: X264::FAST,
        audio: None,
        video_filter: None,
        duration: None,
        out: &out,
    })
    .unwrap();
    encoder.kill();
    assert!(!out.exists());
    assert!(!dir.path().join("never.part.mp4").exists());
}
