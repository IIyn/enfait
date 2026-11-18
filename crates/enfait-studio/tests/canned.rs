//! Chaîne complète sans modèle : réponses préenregistrées, vrai Chromium.
//! Création, critique, rendu en clip, retouche.

#![allow(clippy::unwrap_used, clippy::print_stderr, clippy::indexing_slicing)]

use enfait_ai::animate::{AnimatorConfig, SceneBrief, animator_system};
use enfait_ai::{AiError, Effort, Part, Provider, Request, Usage};
use enfait_motion::{Gpu, RendererPool, composition, library};
use enfait_studio::Studio;
use std::sync::{Arc, Mutex};

fn scene(color: &str) -> String {
    format!(
        r#"<!doctype html><html><head><style>
html,body{{margin:0;width:320px;height:180px;overflow:hidden;background:#111}}
#c{{position:absolute;left:20px;top:60px;width:60px;height:60px;background:{color}}}
</style></head><body><div id="c"></div><script>
const tl = anime.createTimeline({{ autoplay: false }});
tl.add('#c', {{ x: [0, 200], duration: 1000, ease: 'linear' }}, 0);
window.enfait = {{ duration: 1.0, seek(t) {{ tl.seek(t * 1000); }} }};
</script></body></html>"#
    )
}

fn text(request: &Request) -> String {
    request
        .user
        .iter()
        .filter_map(|p| match p {
            Part::Text(t) => Some(t.as_str()),
            Part::Image(_) => None,
        })
        .collect()
}

#[tokio::test]
async fn create_render_and_revise_without_a_model() {
    let Some(chromium) = directories::ProjectDirs::from("", "", "enfait")
        .map(|d| d.cache_dir().join("chromium"))
        .filter(|d| d.is_dir())
    else {
        eprintln!("Chromium absent (enfait-cli setup) : test ignoré");
        return;
    };
    let chrome = enfait_motion::ensure_browser(&chromium).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let renderers = RendererPool::launch(&chrome, &dir.path().join("profile"), Gpu::Software, 1)
        .await
        .unwrap();

    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = Arc::clone(&seen);
    let provider = Provider::Canned(Arc::new(
        move |request: &Request| -> Result<String, AiError> {
            let user = text(request);
            log.lock().unwrap().push(user.clone());
            Ok(if user.contains("Mode : CRÉER") {
                format!("```html\n{}\n```", scene("red"))
            } else if user.contains("Mode : RETOUCHER") {
                format!("```html\n{}\n```", scene("blue"))
            } else {
                "VERDICT: GARDER\nTrès bien.".into()
            })
        },
    ));

    let usage = Mutex::new(Usage::default());
    let work = dir.path().join("work");
    library::install(&work.join("comps")).unwrap();
    let studio = Studio {
        work: work.clone(),
        provider: &provider,
        renderers: &renderers,
        animator: AnimatorConfig {
            system: animator_system(composition::CONTRACT, "- \"Inter\""),
            model: "essai".into(),
            effort: Effort::Low,
        },
        critique_rounds: 1,
        width: 320,
        height: 180,
        fps: 10.0,
        usage: &usage,
        scene_frames: None,
        scene_cutouts: None,
    };
    let brief = SceneBrief {
        index: 1,
        total: 1,
        width: 320,
        height: 180,
        duration: 1.0,
        text: "Bonjour.".into(),
        words: Vec::new(),
        art_direction: String::new(),
        intent: "Un carré traverse.".into(),
        previous: None,
        next: None,
        user_style: None,
        library: None,
        moments: vec![(0.5, "Bonjour".into(), "le carré s'arrête".into())],
        layer: None,
    };

    let result = async {
        enfait_studio::animate(&studio, "blk", &brief, false).await?;
        let source = work.join("sources/blk.html");
        assert!(std::fs::read_to_string(&source)?.contains("red"));

        let clip = work.join("clips/blk.mp4");
        enfait_studio::render_clip(&studio, "blk", 1.0, &clip).await?;
        let probe = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-count_frames", "-select_streams", "v:0"])
            .args([
                "-show_entries",
                "stream=nb_read_frames,width",
                "-of",
                "csv=p=0",
            ])
            .arg(&clip)
            .output()?;
        let probe = String::from_utf8_lossy(&probe.stdout);
        assert_eq!(probe.trim().trim_end_matches(','), "320,10");

        enfait_studio::revise(&studio, "blk", &brief, &[(0.5, "le carré en bleu".into())]).await?;
        assert!(std::fs::read_to_string(&source)?.contains("blue"));
        assert!(std::fs::read_to_string(work.join("sources/blk.1.html"))?.contains("red"));
        anyhow::Ok(())
    }
    .await;
    renderers.close().await;
    result.unwrap();

    let requests = seen.lock().unwrap();
    assert!(
        requests[0].contains("le carré s'arrête"),
        "moments dans le brief"
    );
    assert!(requests.iter().any(|r| r.contains("Mode : CRITIQUER")));
    assert!(
        requests
            .iter()
            .any(|r| r.contains("Mode : RETOUCHER") && r.contains("à 0.50 s : le carré en bleu"))
    );
}

/// Aperçu : prise montée (deux morceaux), un bloc remplacé par un clip rouge.
#[tokio::test]
async fn preview_replaces_animated_blocks() {
    use enfait_studio::preview::{Output, assemble, pieces};
    let dir = tempfile::tempdir().unwrap();
    let ffmpeg = |args: &[&str]| {
        let ok = std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-v", "error", "-y"])
            .args(args)
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "{args:?}");
    };
    let rush = dir.path().join("rush.mp4");
    let clip = dir.path().join("clip.mp4");
    let rush_s = rush.to_string_lossy().into_owned();
    let clip_s = clip.to_string_lossy().into_owned();
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=640x360:rate=25:duration=6",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=6",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-shortest",
        &rush_s,
    ]);
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "color=c=red:size=320x180:rate=25:duration=1",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        &clip_s,
    ]);
    let out = dir.path().join("preview.mp4");
    let blocks = pieces(
        &[
            (0.0, 1.5, None),
            (1.5, 2.5, Some(clip.clone())),
            (2.5, 4.0, None),
        ],
        &[],
    );
    assemble(
        &rush,
        true,
        &[(0.0, 2.0), (3.0, 5.0)],
        &blocks,
        &Output::preview((480, 270), 25.0),
        &out,
    )
    .await
    .unwrap();
    let probe = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
        ])
        .arg(&out)
        .output()
        .unwrap();
    let duration: f64 = String::from_utf8_lossy(&probe.stdout)
        .trim()
        .parse()
        .unwrap();
    assert!((duration - 4.0).abs() < 0.15, "{duration}");
    // Image à 2 s : le clip rouge.
    let frame = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-ss", "2.0", "-i"])
        .arg(&out)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "scale=1:1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-",
        ])
        .output()
        .unwrap();
    let rgb = frame.stdout;
    assert!(rgb[0] > 200 && rgb[1] < 60 && rgb[2] < 60, "{rgb:?}");
}

/// Composition qui montre l'image de scène `from` (jusqu'à `switch` s) puis
/// `to`, en plein cadre.
fn scene_page(switch: f64) -> String {
    format!(
        r#"<!doctype html><html><head><style>
html,body{{margin:0;width:320px;height:180px;overflow:hidden;background:#000}}
img{{position:absolute;inset:0;width:100%;height:100%}}
</style></head><body>
<img data-enfait-scene="from"><img data-enfait-scene="to" style="opacity:0">
<script>
const to = document.querySelector('[data-enfait-scene="to"]');
window.enfait = {{ duration: 1.0, seek(t) {{ to.style.opacity = t < {switch} ? 0 : 1; }} }};
</script></body></html>"#
    )
}

fn color_clip(dir: &std::path::Path, name: &str, color: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    let ok = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!("color=c={color}:size=320x180:rate=10:duration=1"))
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .arg(&path)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok);
    path
}

/// Couleur moyenne de l'image `index` d'un clip.
fn pixel(clip: &std::path::Path, index: usize) -> Vec<u8> {
    std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(clip)
        .args(["-vf", &format!("select=eq(n\\,{index}),scale=1:1")])
        .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .output()
        .unwrap()
        .stdout
}

/// Couleur du pixel (`x`, `y`) de l'image `index` d'un clip.
fn pixel_at(clip: &std::path::Path, index: usize, x: u32, y: u32) -> Vec<u8> {
    std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(clip)
        .args([
            "-vf",
            &format!("select=eq(n\\,{index}),crop=2:2:{x}:{y},scale=1:1"),
        ])
        .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .output()
        .map(|o| {
            if o.stdout.is_empty() {
                eprintln!("{}", String::from_utf8_lossy(&o.stderr));
            }
            o.stdout
        })
        .unwrap()
}

#[tokio::test]
async fn scenes_reach_mixed_blocks_and_transitions() {
    let Some(chromium) = directories::ProjectDirs::from("", "", "enfait")
        .map(|d| d.cache_dir().join("chromium"))
        .filter(|d| d.is_dir())
    else {
        eprintln!("Chromium absent (enfait-cli setup) : test ignoré");
        return;
    };
    let chrome = enfait_motion::ensure_browser(&chromium).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let renderers = RendererPool::launch(&chrome, &dir.path().join("profile"), Gpu::Software, 1)
        .await
        .unwrap();
    let provider = Provider::Canned(Arc::new(|request: &Request| -> Result<String, AiError> {
        let user = text(request);
        Ok(if user.contains("transition") {
            format!("```html\n{}\n```", scene_page(0.5))
        } else if user.contains("Mode : CRÉER") || user.contains("Mode : RETOUCHER") {
            format!("```html\n{}\n```", scene_page(9.0))
        } else {
            "VERDICT: GARDER".into()
        })
    }));
    let usage = Mutex::new(Usage::default());
    let work = dir.path().join("work");
    library::install(&work.join("comps")).unwrap();
    let red = color_clip(dir.path(), "red.mp4", "red");
    let blue = color_clip(dir.path(), "blue.mp4", "blue");

    let result = async {
        // Bloc « Mixte » : la prise (ici rouge) arrive dans la composition.
        let frames = work.join("rush-frames");
        let count =
            enfait_studio::extract_frames(&red, &[(0.0, 1.0)], 10.0, (320, 180), &frames).await?;
        assert_eq!(count, 10);
        let studio = Studio {
            work: work.clone(),
            provider: &provider,
            renderers: &renderers,
            animator: AnimatorConfig {
                system: animator_system(composition::CONTRACT, "- \"Inter\""),
                model: "essai".into(),
                effort: Effort::Low,
            },
            critique_rounds: 0,
            width: 320,
            height: 180,
            fps: 10.0,
            usage: &usage,
            scene_frames: Some((frames, count)),
            scene_cutouts: None,
        };
        let mut brief = SceneBrief {
            index: 1,
            total: 1,
            width: 320,
            height: 180,
            duration: 1.0,
            text: "Bonjour.".into(),
            words: Vec::new(),
            art_direction: String::new(),
            intent: String::new(),
            previous: None,
            next: None,
            user_style: None,
            library: None,
            moments: Vec::new(),
            layer: Some(enfait_ai::animate::SceneLayer::Mixed),
        };
        enfait_studio::animate(&studio, "mixte", &brief, true).await?;
        let clip = work.join("clips/mixte.mp4");
        enfait_studio::render_clip(&studio, "mixte", 1.0, &clip).await?;
        let rgb = pixel(&clip, 5);
        assert!(
            rgb[0] > 200 && rgb[2] < 60,
            "la prise rouge est rendue : {rgb:?}"
        );

        // Personne détourée (ici : moitié gauche verte, droite transparente),
        // passée en `to` et posée par-dessus la prise.
        let cutouts = work.join("rush-cutouts");
        std::fs::create_dir_all(&cutouts)?;
        let half = cutouts.join("demi.png");
        let ok = std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg("color=c=0x00ff00:size=160x180")
            .args(["-vf", "format=rgba,pad=320:180:0:0:color=black@0"])
            .args(["-frames:v", "1"])
            .arg(&half)
            .status()?;
        assert!(ok.success());
        for i in 0..count {
            std::fs::copy(&half, cutouts.join(enfait_studio::cutout_name(i)))?;
        }
        let cut_studio = Studio {
            scene_cutouts: Some(cutouts),
            scene_frames: studio.scene_frames.clone(),
            animator: studio.animator.clone(),
            work: work.clone(),
            ..studio
        };
        std::fs::write(
            work.join("comps/detoure.html"),
            composition::prepare(&scene_page(0.0)),
        )?;
        let clip = work.join("clips/detoure.mp4");
        enfait_studio::render_clip(&cut_studio, "detoure", 1.0, &clip).await?;
        let left = pixel_at(&clip, 5, 40, 90);
        let right = pixel_at(&clip, 5, 280, 90);
        assert!(left[1] > 200 && left[0] < 60, "personne devant : {left:?}");
        assert!(
            right[0] > 200 && right[1] < 60,
            "prise derrière : {right:?}"
        );

        // Retouche puis retour à la version 1.
        brief.moments.clear();
        enfait_studio::revise(&studio, "mixte", &brief, &[(0.2, "rien".into())]).await?;
        assert_eq!(enfait_studio::versions(&work, "mixte"), [1]);
        enfait_studio::restore(&studio, "mixte", 1).await?;
        assert_eq!(enfait_studio::versions(&work, "mixte"), [1, 2]);

        // Transition : rouge, puis bleu après la moitié.
        let from = work.join("t-from");
        let to = work.join("t-to");
        enfait_studio::extract_frames(&red, &[(0.0, 0.5)], 10.0, (320, 180), &from).await?;
        enfait_studio::extract_frames(&blue, &[(0.0, 0.5)], 10.0, (320, 180), &to).await?;
        let scenes: Vec<_> = (0..10)
            .map(|i: usize| {
                (
                    from.join(enfait_motion::frame_name(i.min(4))),
                    to.join(enfait_motion::frame_name(i.saturating_sub(5).min(4))),
                )
            })
            .collect();
        let brief = enfait_ai::animate::TransitionBrief {
            width: 320,
            height: 180,
            duration: 1.0,
            art_direction: String::new(),
            description: "fondu".into(),
            from_intent: String::new(),
            to_intent: String::new(),
        };
        let clip = work.join("clips/transition.mp4");
        enfait_studio::transition_clip(&studio, "transition", &brief, &scenes, true, &clip).await?;
        let first = pixel(&clip, 1);
        let last = pixel(&clip, 8);
        assert!(first[0] > 200 && first[2] < 60, "départ rouge : {first:?}");
        assert!(last[2] > 200 && last[0] < 60, "arrivée bleue : {last:?}");
        anyhow::Ok(())
    }
    .await;
    renderers.close().await;
    result.unwrap();
}
