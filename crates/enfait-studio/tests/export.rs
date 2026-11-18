//! Export réel par ffmpeg sur une session synthétique : vidéo verticale,
//! prise 16:9 rognée, un bloc animé, musique et SFX ; puis le montage
//! Premiere de la même session.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use enfait_core::{
    AspectRatio, Asset, AssetKind, Block, BlockKind, Finish, Flicks, Framing, Look, Project,
    ProjectSettings, Resolution, TimeRange, Transcript,
};
use enfait_studio::export::{ExportOptions, video};
use enfait_studio::premiere::{PremiereOptions, export};
use enfait_studio::session::{SessionFiles, clip_path};
use std::path::Path;
use std::process::Command;

fn ffmpeg(args: &[&str], out: &Path) {
    let ok = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-y"])
        .args(args)
        .arg(out)
        .status()
        .unwrap();
    assert!(ok.success(), "{args:?}");
}

fn probe(file: &Path, entries: &str) -> String {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", entries, "-of", "csv=p=0"])
        .arg(file)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// Couleur moyenne de l'image à `t` secondes.
fn pixel(file: &Path, t: f64) -> Vec<u8> {
    Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-ss", &t.to_string(), "-i"])
        .arg(file)
        .args(["-frames:v", "1", "-vf", "scale=1:1", "-f", "rawvideo"])
        .args(["-pix_fmt", "rgb24", "-"])
        .output()
        .unwrap()
        .stdout
}

/// Loudness intégrée (LUFS) du son de `file`.
fn lufs(file: &Path) -> f64 {
    let out = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-nostats", "-i"])
        .arg(file)
        .args(["-af", "ebur128", "-f", "null", "-"])
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&out.stderr);
    let summary = log.rsplit("Summary:").next().unwrap_or_default();
    summary
        .lines()
        .find_map(|l| l.trim().strip_prefix("I:"))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|v| v.parse().ok())
        .unwrap()
}

fn secs(ms: i64) -> Flicks {
    Flicks::from_millis(ms).unwrap()
}

/// Session : prise de 6 s (mire 16:9 + voix à 440 Hz faible), montage
/// [0 ; 2 s) + [3 ; 5 s), bloc 1 sur la prise, bloc 2 animé (clip rouge).
fn session(d: &Path) -> SessionFiles {
    let rush = d.join("prise.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x360:rate=25:duration=6",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=6,volume=0.05",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
        ],
        &rush,
    );
    let mut settings = ProjectSettings::default().with_aspect(AspectRatio::Portrait9x16);
    // Petite définition : l'essai reste rapide.
    settings.resolution = Resolution {
        width: 180,
        height: 320,
    };
    settings.frame_rate = enfait_core::FrameRate::new(25, 1).unwrap();
    settings.framing = Framing::Fill;
    let blocks = vec![
        Block::new(
            TimeRange::new(secs(0), secs(2000)).unwrap(),
            BlockKind::Rush,
        ),
        Block::new(
            TimeRange::new(secs(2000), secs(4000)).unwrap(),
            BlockKind::Animation,
        ),
    ];
    let project = Project::with_blocks("Essai export", settings, blocks).unwrap();
    let studio = d.join("studio");
    let clip = clip_path(&studio, project.blocks()[1].id, &settings);
    std::fs::create_dir_all(clip.parent().unwrap()).unwrap();
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=red:size=180x320:rate=25:duration=2",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
        ],
        &clip,
    );
    let library = d.join("library");
    std::fs::create_dir_all(&library).unwrap();
    ffmpeg(
        &["-f", "lavfi", "-i", "sine=frequency=220:duration=1.5"],
        &library.join("musique.wav"),
    );
    ffmpeg(
        &["-f", "lavfi", "-i", "anoisesrc=d=0.3:a=0.5"],
        &library.join("pop.wav"),
    );
    let asset = |file: &str| Asset {
        file: file.into(),
        kind: AssetKind::Audio,
        name: file.into(),
        description: String::new(),
        origin: None,
    };
    let look = Look {
        assets: vec![asset("musique.wav"), asset("pop.wav")],
        ..Look::default()
    };
    let mut finish = Finish::default();
    let (first, second) = (project.blocks()[0].id, project.blocks()[1].id);
    finish.add_music("musique.wav".into(), first, second);
    finish.add_sfx("pop.wav".into(), second, 0.0);
    std::fs::write(d.join("project.json"), project.to_json().unwrap()).unwrap();
    std::fs::write(
        d.join("montage.json"),
        serde_json::json!({
            "key": "0.000-2.000,3.000-5.000",
            "transcript": Transcript::default(),
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(d.join("look.json"), serde_json::to_vec(&look).unwrap()).unwrap();
    std::fs::write(d.join("finish.json"), serde_json::to_vec(&finish).unwrap()).unwrap();
    std::fs::write(
        d.join("session.json"),
        serde_json::json!({ "rush": rush }).to_string(),
    )
    .unwrap();
    SessionFiles::load(d).unwrap()
}

#[tokio::test]
async fn video_is_framed_mixed_and_normalized() {
    let dir = tempfile::tempdir().unwrap();
    let session = session(dir.path());
    let out = dir.path().join("sortie").join("film.mp4");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    let last = std::sync::Mutex::new(0.0_f64);
    let report = video(&session.sources(), ExportOptions::default(), &out, &|p| {
        *last.lock().unwrap() = p;
    })
    .await
    .unwrap();
    assert!((*last.lock().unwrap() - 1.0).abs() < 1e-9);
    assert!(report.missing.is_empty());
    assert_eq!(
        probe(&out, "stream=width,height").lines().next(),
        Some("180,320")
    );
    let duration: f64 = probe(&out, "format=duration").parse().unwrap();
    assert!((duration - 4.0).abs() < 0.1, "{duration}");
    // Le bloc animé montre son clip rouge.
    let red = pixel(&out, 3.0);
    assert!(red[0] > 200 && red[1] < 60 && red[2] < 60, "{red:?}");
    // Loudness ramenée à −14 LUFS (le mixage, musique pleine sans ducking
    // faute de mots, était bien plus fort).
    assert!(report.measured_lufs.unwrap() > -11.0, "{report:?}");
    let level = lufs(&out);
    assert!((level + 14.0).abs() < 1.0, "{level}");
    // Le dossier de travail est retiré.
    let left: Vec<_> = std::fs::read_dir(out.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, ["film.mp4"]);
}

#[tokio::test]
async fn premiere_sequence_with_and_without_sfx() {
    let dir = tempfile::tempdir().unwrap();
    let session = session(dir.path());
    let xml = dir.path().join("montage.xml");
    let report = export(&session.sources(), PremiereOptions { sfx: true }, &xml).unwrap();
    let text = std::fs::read_to_string(&xml).unwrap();
    assert!(text.starts_with("<?xml"));
    assert!(text.contains("<width>180</width><height>320</height>"));
    // V1 (prise, deux morceaux), V2 (le bloc animé), V3 (vide).
    assert_eq!(text.matches("<track>").count(), 3 + 1 + 2 + 2);
    assert!(
        text.contains("<in>75</in><out>125</out>"),
        "second morceau de la prise"
    );
    // Prise 640×360 rognée dans 180×320 : 320/360 = 88,9 %.
    assert!(text.contains("<value>88.889</value>"));
    for file in [
        "bloc-02-animation.mp4",
        "musique.wav",
        "pop.wav",
        "LISEZMOI.txt",
    ] {
        assert!(report.media.join(file).is_file(), "{file}");
    }
    // Musique d'1,5 s sur 4 s : trois passages.
    assert_eq!(text.matches("<name>musique.wav</name>").count(), 2 * 3 + 1);
    let without = dir.path().join("sans.xml");
    let report = export(&session.sources(), PremiereOptions { sfx: false }, &without).unwrap();
    let text = std::fs::read_to_string(&without).unwrap();
    assert!(!text.contains("pop.wav"));
    assert!(!report.media.join("pop.wav").exists());
    let readme = std::fs::read_to_string(report.media.join("LISEZMOI.txt")).unwrap();
    assert!(readme.contains("sans les SFX"), "{readme}");
}
