//! Mixage réel par ffmpeg : la musique reprend au début, baisse sous la
//! voix (ducking), le SFX tombe à son instant, l'image n'est pas touchée.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use enfait_core::Ducking;
use enfait_core::finish::{PlacedMusic, PlacedSfx};
use enfait_studio::mix::{MixPlan, apply};
use std::path::Path;
use std::process::Command;

fn ffmpeg(args: &[&str], out: &Path) {
    let ok = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-y"])
        .args(args)
        .arg(out)
        .status()
        .unwrap();
    assert!(ok.success());
}

/// Niveau RMS (dB) du son de `file` entre `a` et `b` secondes.
fn rms(file: &Path, a: f64, b: f64) -> f64 {
    let out = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "info", "-ss", &a.to_string(), "-t"])
        .arg((b - a).to_string())
        .arg("-i")
        .arg(file)
        .args([
            "-af",
            "astats=measure_overall=RMS_level:measure_perchannel=none",
        ])
        .args(["-f", "null", "-"])
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&out.stderr);
    log.lines()
        .filter_map(|l| l.split("RMS level dB:").nth(1))
        .filter_map(|v| v.trim().parse::<f64>().ok())
        .next_back()
        .unwrap_or(f64::NEG_INFINITY)
}

#[tokio::test]
async fn music_ducks_and_sfx_lands() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    // Base : 6 s d'image et une « voix » muette.
    let base = d.join("base.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=gray:size=320x180:rate=25:duration=6",
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=48000:cl=mono",
            "-t",
            "6",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-shortest",
        ],
        &base,
    );
    // Musique : 1,5 s de la, reprise en boucle.
    let music = d.join("music.wav");
    ffmpeg(
        &["-f", "lavfi", "-i", "sine=frequency=440:duration=1.5"],
        &music,
    );
    // SFX : 0,3 s de bruit fort.
    let sfx = d.join("pop.wav");
    ffmpeg(&["-f", "lavfi", "-i", "anoisesrc=d=0.3:a=0.8"], &sfx);

    let plan = MixPlan {
        music: vec![(
            PlacedMusic {
                id: 1,
                file: "music.wav".into(),
                start: 0.0,
                end: 6.0,
                offset: 0.0,
                gain_db: -6.0,
                fade_in: 0.0,
                fade_out: 0.0,
            },
            music,
        )],
        sfx: vec![(
            PlacedSfx {
                id: 2,
                file: "pop.wav".into(),
                at: 5.0,
                gain_db: 0.0,
            },
            sfx,
        )],
        ducking: Ducking {
            depth_db: -20.0,
            ..Ducking::default()
        },
        // La voix « parle » de 1 à 2 s.
        duck_spans: vec![(1.0, 2.0)],
        voice_gain_db: 0.0,
    };
    let out = d.join("mix.mp4");
    apply(&base, &plan, &out).await.unwrap();

    let full = rms(&out, 3.0, 4.0);
    let ducked = rms(&out, 1.2, 1.8);
    assert!(
        full > -36.0,
        "musique audible, reprise après 1,5 s : {full}"
    );
    assert!(
        (full - ducked - 20.0).abs() < 3.0,
        "baisse de 20 dB sous la voix : {full} / {ducked}"
    );
    let pop = rms(&out, 5.0, 5.3);
    assert!(pop > full + 3.0, "le SFX tombe à 5 s : {pop} / {full}");
    // L'image est copiée telle quelle, la durée est celle de la base.
    let probe = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=codec_type,channels"])
        .args(["-of", "csv=p=0"])
        .arg(&out)
        .output()
        .unwrap();
    let streams = String::from_utf8_lossy(&probe.stdout);
    assert!(
        streams.contains("video") && streams.contains("audio,2"),
        "{streams}"
    );

    // Une voix mono garde son niveau une fois en stéréo.
    let voiced = d.join("voix.mp4");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=gray:size=320x180:rate=25:duration=3",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=300:duration=3",
            "-ac",
            "1",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-shortest",
        ],
        &voiced,
    );
    let plan = MixPlan {
        voice_gain_db: 0.001,
        sfx: vec![(
            PlacedSfx {
                id: 3,
                file: "pop.wav".into(),
                at: 2.5,
                gain_db: 0.0,
            },
            d.join("pop.wav"),
        )],
        ..MixPlan::default()
    };
    let mixed = d.join("voix-mix.mp4");
    apply(&voiced, &plan, &mixed).await.unwrap();
    let (before, after) = (rms(&voiced, 0.5, 2.0), rms(&mixed, 0.5, 2.0));
    assert!((before - after).abs() < 0.5, "voix : {before} → {after}");

    // Rien à mixer : simple copie.
    let copy = d.join("copie.mp4");
    apply(&base, &MixPlan::default(), &copy).await.unwrap();
    assert!(copy.is_file());
}
