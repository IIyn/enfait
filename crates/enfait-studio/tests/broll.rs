//! B-roll shots rendered by the real ffmpeg: video too short looped, still
//! image with a zoom, in the project format.

#![allow(clippy::unwrap_used)]

use enfait_core::AssetKind;
use enfait_studio::broll::render;
use std::path::Path;
use std::process::Command;

fn probe(path: &Path) -> String {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-count_frames"])
        .args(["-show_entries", "stream=width,height,nb_read_frames"])
        .args(["-of", "csv=p=0"])
        .arg(path)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[tokio::test]
async fn video_and_still() {
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path().join("court.mp4");
    let ok = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc2=size=640x480:rate=25:duration=2")
        .arg(&video)
        .status()
        .unwrap();
    assert!(ok.success());
    let out = dir.path().join("plan.mp4");
    // 3 s from 1.5 s of a 2 s video: loops to the start.
    render(&video, AssetKind::Video, 1.5, 3.0, (1920, 1080), 30.0, &out)
        .await
        .unwrap();
    assert_eq!(probe(&out), "1920,1080,90");

    let still = dir.path().join("photo.png");
    let ok = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc2=size=800x600")
        .args(["-frames:v", "1"])
        .arg(&still)
        .status()
        .unwrap();
    assert!(ok.success());
    let out = dir.path().join("photo.mp4");
    render(&still, AssetKind::Image, 0.0, 2.0, (1080, 1920), 25.0, &out)
        .await
        .unwrap();
    assert_eq!(probe(&out), "1080,1920,50");
}
