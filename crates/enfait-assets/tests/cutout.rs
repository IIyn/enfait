//! Cutout of a sequence of images with the real fast model, if it is
//! already cached (`enfait-cli cutout --fast` downloads it); skipped otherwise.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use enfait_assets::cutout::{Cutter, Model, cut_frames};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn cuts_frames_in_parallel() {
    let model =
        Some(enfait_config::paths::cutout_models_dir().join("u2netp.onnx")).filter(|p| p.is_file());
    let Some(model) = model else {
        eprintln!("cutout model missing: test skipped");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let ok = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc2=size=480x270:rate=10:duration=0.6")
        .arg(dir.path().join("%06d.jpg"))
        .status()
        .unwrap();
    assert!(ok.success());
    let jobs: Vec<(PathBuf, PathBuf)> = (1..=6)
        .map(|i| {
            (
                dir.path().join(format!("{i:06}.jpg")),
                dir.path().join(format!("{i:06}.png")),
            )
        })
        .collect();
    let cutter = Cutter::load(&model, Model::Fast).unwrap();
    let seen = AtomicUsize::new(0);
    cut_frames(&cutter, &jobs, 3, &|n| {
        seen.fetch_max(n, Ordering::AcqRel);
    })
    .unwrap();
    assert_eq!(seen.load(Ordering::Acquire), 6);
    for (_, out) in &jobs {
        let image = image::open(out).unwrap();
        assert_eq!((image.width(), image.height()), (480, 270));
        assert!(image.color().has_alpha());
    }
}
