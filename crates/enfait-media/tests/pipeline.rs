//! Chaîne complète sur un petit média généré par ffmpeg : analyse, proxy,
//! son, forme d'onde, décodage d'images guidé par l'horloge.

#![allow(clippy::unwrap_used, clippy::print_stderr, clippy::indexing_slicing)]
use enfait_core::FrameRate;
use enfait_media::{Clock, Command, Edl, MediaCache, Stage, VideoDecoder};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn has_ffmpeg() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn make_clip(path: &Path) {
    let status = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=25:duration=2",
        ])
        .args([
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2:sample_rate=44100",
        ])
        .args([
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
}

fn wait_for(decoder: &VideoDecoder, index: i64) -> Arc<enfait_media::Frame> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(f) = decoder.latest().filter(|f| f.index == index) {
            return f;
        }
        assert!(Instant::now() < deadline, "image {index} jamais décodée");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn prepare_and_decode() {
    if !has_ffmpeg() {
        eprintln!("ffmpeg absent : test ignoré");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let clip = dir.path().join("clip.mp4");
    make_clip(&clip);

    let cache = MediaCache::new(dir.path().join("cache"));
    let mut stages = Vec::new();
    let prepared = cache
        .prepare(&clip, &mut |stage, _| {
            if stages.last() != Some(&stage) {
                stages.push(stage);
            }
        })
        .unwrap();
    assert_eq!(stages, [Stage::Probe, Stage::Audio, Stage::Waveform]);
    let original = prepared.video.clone().unwrap();
    assert!(
        !original.is_proxy,
        "lisible tout de suite, depuis l'original"
    );
    assert_eq!(original.path, std::path::absolute(&clip).unwrap());
    let video = prepared.info.video.unwrap();
    assert_eq!((video.width, video.height), (320, 240));
    assert_eq!(video.frame_rate, FrameRate::FPS_25);
    assert!((prepared.info.duration - 2.0).abs() < 0.1);
    assert_eq!(prepared.frame_count(), 50);

    let audio = prepared.audio.clone().unwrap();
    assert_eq!(audio.rate(), 48_000);
    assert!((audio.duration() - 2.0).abs() < 0.1);
    let peaks = prepared.peaks.clone().unwrap();
    let columns = peaks.columns(0.2, 1.8, 10);
    assert!(
        columns.iter().all(|(lo, hi)| *lo < -0.1 && *hi > 0.1),
        "sinus audible (amplitude 1/8) : {columns:?}"
    );

    // L'original se décode déjà, avant le proxy.
    let clock = Arc::new(Clock::new(prepared.info.duration, 1.0 / 25.0));
    let early = VideoDecoder::spawn(
        original.path.clone(),
        video.frame_rate,
        prepared.frame_count(),
        Arc::new(Edl::identity(prepared.info.duration)),
        (original.width, original.height),
        None,
        Arc::clone(&clock),
        Box::new(|| {}),
    );
    clock.apply(Command::Seek(1.0));
    wait_for(&early, 25);
    drop(early);

    let mut last = 0.0;
    let proxy = cache
        .build_proxy(&prepared, &mut |p| last = p)
        .unwrap()
        .unwrap();
    assert!(proxy.is_proxy && last == 1.0);

    // Deuxième préparation : tout vient du cache, proxy compris.
    let mut again = Vec::new();
    let reopened = cache
        .prepare(&clip, &mut |stage, _| again.push(stage))
        .unwrap();
    assert!(!again.contains(&Stage::Audio));
    assert_eq!(reopened.video, Some(proxy.clone()));

    let (proxy, w, h) = (proxy.path, proxy.width, proxy.height);
    clock.apply(Command::Seek(0.0));
    let decoder = VideoDecoder::spawn(
        proxy,
        video.frame_rate,
        prepared.frame_count(),
        Arc::new(Edl::identity(prepared.info.duration)),
        (w, h),
        None,
        Arc::clone(&clock),
        Box::new(|| {}),
    );
    let first = wait_for(&decoder, 0);
    assert_eq!(first.data.len(), (w * h * 4) as usize);
    clock.apply(Command::Seek(1.0));
    let later = wait_for(&decoder, 25);
    assert_ne!(first.data, later.data);
    // Retour en arrière : relance du décodeur.
    clock.apply(Command::Step(-13));
    wait_for(&decoder, 12);
    // Au-delà de la fin : la dernière image.
    clock.apply(Command::Seek(5.0));
    wait_for(&decoder, 49);
}

/// Lecture arrière à vitesse normale sur le proxy : la plupart des images
/// doivent s'afficher, sans long arrêt sur image.
#[test]
fn backward_playback_is_smooth() {
    if !has_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let clip = dir.path().join("clip.mp4");
    let status = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=1280x720:rate=25:duration=6",
        ])
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .arg(&clip)
        .status()
        .unwrap();
    assert!(status.success());
    let cache = MediaCache::new(dir.path().join("cache"));
    let prepared = cache.prepare(&clip, &mut |_, _| {}).unwrap();
    let proxy = cache.build_proxy(&prepared, &mut |_| {}).unwrap().unwrap();
    let rate = prepared.frame_rate().unwrap();

    let clock = Arc::new(Clock::new(prepared.info.duration, 1.0 / 25.0));
    clock.apply(Command::Seek(5.0));
    let decoder = VideoDecoder::spawn(
        proxy.path,
        rate,
        prepared.frame_count(),
        Arc::new(Edl::identity(prepared.info.duration)),
        (proxy.width, proxy.height),
        None,
        Arc::clone(&clock),
        Box::new(|| {}),
    );
    wait_for(&decoder, 125);
    // L'utilisateur est en pause un instant avant d'appuyer sur J.
    std::thread::sleep(Duration::from_millis(300));
    let _playback = enfait_media::Playback::start(
        Arc::clone(&clock),
        None,
        Arc::new(Edl::identity(prepared.info.duration)),
    );
    clock.apply(Command::Backward);

    let started = Instant::now();
    let mut seen = std::collections::BTreeSet::new();
    let mut last_change = Instant::now();
    let mut last_index = 125;
    let mut longest = Duration::ZERO;
    while started.elapsed() < Duration::from_secs(3) {
        if let Some(f) = decoder.latest()
            && f.index != last_index
        {
            assert!(f.index < last_index, "toujours en arrière");
            longest = longest.max(last_change.elapsed());
            last_change = Instant::now();
            last_index = f.index;
            seen.insert(f.index);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    // 3 s en arrière à 25 i/s : 75 images.
    eprintln!(
        "{} images distinctes, plus long arrêt {longest:?}",
        seen.len()
    );
    // Seuils larges : la machine de CI peut être lente.
    assert!(seen.len() >= 50, "{} images seulement", seen.len());
    assert!(longest < Duration::from_millis(200), "arrêt de {longest:?}");
}

/// Lecture avant d'un montage à trois morceaux : les images viennent des
/// bons endroits de la source, sans long arrêt aux coupes.
#[test]
fn cut_playback_crosses_cuts_smoothly() {
    if !has_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let clip = dir.path().join("clip.mp4");
    let status = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=1280x720:rate=25:duration=8",
        ])
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .arg(&clip)
        .status()
        .unwrap();
    assert!(status.success());
    let cache = MediaCache::new(dir.path().join("cache"));
    let prepared = cache.prepare(&clip, &mut |_, _| {}).unwrap();
    let proxy = cache.build_proxy(&prepared, &mut |_| {}).unwrap().unwrap();
    let rate = prepared.frame_rate().unwrap();
    // 1 s depuis 6 s, puis 1 s depuis 1 s, puis 1 s depuis 4 s.
    let edl = Arc::new(Edl::from_ranges(&[(6.0, 7.0), (1.0, 2.0), (4.0, 5.0)]));
    let clock = Arc::new(Clock::new(edl.duration(), 1.0 / 25.0));
    let decoder = VideoDecoder::spawn(
        proxy.path,
        rate,
        prepared.frame_count(),
        Arc::clone(&edl),
        (proxy.width, proxy.height),
        None,
        Arc::clone(&clock),
        Box::new(|| {}),
    );
    wait_for(&decoder, 150);
    let _playback = enfait_media::Playback::start(Arc::clone(&clock), None, Arc::clone(&edl));
    clock.apply(Command::Forward);
    let started = Instant::now();
    let mut seen = Vec::new();
    let mut last_change = Instant::now();
    let mut longest = Duration::ZERO;
    while clock.is_playing() && started.elapsed() < Duration::from_secs(5) {
        if let Some(f) = decoder.latest()
            && seen.last() != Some(&f.index)
        {
            longest = longest.max(last_change.elapsed());
            last_change = Instant::now();
            seen.push(f.index);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    eprintln!("{} images, plus long arrêt {longest:?}", seen.len());
    let from = |a: i64, b: i64| seen.iter().filter(|i| (a..b).contains(*i)).count();
    assert!(from(150, 175) >= 20, "premier morceau : {seen:?}");
    assert!(from(25, 50) >= 20, "deuxième morceau : {seen:?}");
    assert!(from(100, 125) >= 20, "troisième morceau : {seen:?}");
    assert_eq!(from(51, 100) + from(126, 150), 0, "rien hors du montage");
    assert!(longest < Duration::from_millis(200), "arrêt de {longest:?}");
}

/// Chaque look prêt à l'emploi passe dans le ffmpeg installé.
#[test]
fn fx_presets_run_in_ffmpeg() {
    if !has_ffmpeg() {
        return;
    }
    for preset in enfait_core::FX_PRESETS {
        let Some(chain) = enfait_media::fx::filter(&preset.fx, 360) else {
            continue;
        };
        let out = std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-v", "error"])
            .args([
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=640x360:rate=25:duration=0.2",
            ])
            .args(["-vf", &chain, "-f", "null", "-"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{} : {}",
            preset.slug,
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
