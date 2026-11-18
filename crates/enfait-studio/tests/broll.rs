//! Plans de b-roll rendus par le vrai ffmpeg : vidéo trop courte reprise au
//! début, image fixe avec zoom, au format du projet.

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
    // 3 s à partir de 1,5 s d'une vidéo de 2 s : reprise au début.
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
