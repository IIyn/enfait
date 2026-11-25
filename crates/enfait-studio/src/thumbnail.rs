//! YouTube thumbnail of a 16:9 video: designed by the model as a still
//! page made of layers (see `enfait_motion::still`), from the real title,
//! the author's notes and frames of the edit; exported as a picture for
//! YouTube or as a layered Photoshop document.
//!
//! Everything lives in `<studio>/thumbnail/`: the frames given to the
//! designer (`stills/`), then one page and its picture per proposal
//! (`v1.html`, `v1.png`…), listed in `versions.json`.

use crate::psd::{self, PsdLayer};
use crate::session::{Sources, Visuals, source_pieces};
use anyhow::{Context as _, Result};
use enfait_ai::thumbnail::{ThumbnailBrief, ThumbnailImage, concept, request};
use enfait_ai::{Effort, Provider, Usage};
use enfait_core::BlockKind;
use enfait_motion::{Renderer, RendererPool, composition, library, render_still, still};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Size of the page, in CSS pixels.
pub const WIDTH: u32 = 1280;
pub const HEIGHT: u32 = 720;
/// Device pixels per CSS pixel: pictures in 1920×1080.
pub const SCALE: f64 = 1.5;
/// Largest thumbnail accepted by YouTube.
pub const MAX_BYTES: usize = 2 * 1024 * 1024;
/// Frames of the edit given to the designer.
const MAX_STILLS: usize = 5;

/// A proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    pub number: u32,
    /// The idea, in one sentence (from the model).
    pub concept: String,
    /// Title it was made for.
    pub title: String,
}

/// Where a generation stands, for the interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Frames of the edit.
    Stills,
    /// Cutting the person out.
    Cutout,
    /// The model designs.
    Designing,
    Rendering,
    /// The page had errors: the model fixes it.
    Fixing,
}

/// Folder of the thumbnails of a studio.
pub fn dir(studio: &Path) -> PathBuf {
    studio.join("thumbnail")
}

pub fn html_path(dir: &Path, number: u32) -> PathBuf {
    dir.join(format!("v{number}.html"))
}

/// Picture of a proposal (PNG, 1920×1080).
pub fn png_path(dir: &Path, number: u32) -> PathBuf {
    dir.join(format!("v{number}.png"))
}

fn versions_path(dir: &Path) -> PathBuf {
    dir.join("versions.json")
}

/// The proposals, oldest first (those whose picture is there).
pub fn versions(dir: &Path) -> Vec<Version> {
    std::fs::read(versions_path(dir))
        .ok()
        .and_then(|d| serde_json::from_slice::<Vec<Version>>(&d).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|v| png_path(dir, v.number).is_file())
        .collect()
}

/// What a generation needs.
pub struct Input<'a> {
    pub sources: Sources<'a>,
    /// The take has pictures (filmed person).
    pub has_video: bool,
    pub title: &'a str,
    pub notes: &'a str,
    pub model: String,
    pub effort: Effort,
    /// Available fonts (see [`crate::fonts_list`]), with those of the user.
    pub fonts: String,
    pub user_style: Option<String>,
}

/// Designs a new proposal and renders its picture.
pub async fn generate(
    provider: &Provider,
    renderers: &RendererPool,
    usage: &Mutex<Usage>,
    dir: &Path,
    input: &Input<'_>,
    say: &(dyn Fn(Step) + Sync),
) -> Result<Version> {
    tokio::fs::create_dir_all(dir).await?;
    library::install(dir).context("library of the thumbnail")?;
    crate::userlib::install(input.sources.look, input.sources.library, dir)?;
    say(Step::Stills);
    let mut images = stills(&input.sources, input.has_video, dir, say).await?;
    images.extend(mentioned_images(&input.sources, input.notes));
    let previous = versions(dir);
    let brief = ThumbnailBrief {
        width: WIDTH,
        height: HEIGHT,
        title: input.title.to_owned(),
        notes: input.notes.to_owned(),
        language: input.sources.transcript.language.clone(),
        transcript: transcript_text(&input.sources),
        art_direction: art_direction(input.sources.studio),
        user_style: input.user_style.clone(),
        library: crate::userlib::brief(input.sources.look),
        mentions: input.sources.look.mentions_brief(input.notes),
        images,
        rejected: previous.iter().map(|v| v.concept.clone()).collect(),
    };
    say(Step::Designing);
    let ask = |request: enfait_ai::Request| async move {
        let reply = provider.complete(&request).await?;
        if let Ok(mut u) = usage.lock() {
            u.add(&reply.usage);
        }
        anyhow::Ok(reply.text)
    };
    let mut reply = ask(request(&brief, &input.fonts, &input.model, input.effort)).await?;
    let number = previous
        .iter()
        .map(|v| v.number)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let html = html_path(dir, number);
    let mut fixed = false;
    let picture = loop {
        let page = composition::extract_html(&reply)
            .context("the model sent no HTML document for the thumbnail")?;
        tokio::fs::write(&html, still::make_static(&composition::prepare(&page))).await?;
        say(Step::Rendering);
        let rendered = {
            let renderer = renderers.acquire().await?;
            render_still(&renderer, &html, WIDTH, HEIGHT, SCALE, false).await
        };
        let problem = match rendered {
            Ok(rendered) if rendered.errors.is_empty() || fixed => {
                for e in &rendered.errors {
                    tracing::warn!("thumbnail v{number}: {e}");
                }
                break rendered.composite;
            }
            Ok(rendered) => rendered.errors.join("\n"),
            Err(e) if !fixed => e.to_string(),
            Err(e) => return Err(e.into()),
        };
        // One fix, with the errors of the page.
        fixed = true;
        say(Step::Fixing);
        let mut fix = request(&brief, &input.fonts, &input.model, input.effort);
        fix.user.push(enfait_ai::Part::Text(format!(
            "Your document:\n\n```html\n{page}\n```\n\nIt fails in the browser:\n\n{problem}\n\n\
             FIX it: same idea, the complete corrected document."
        )));
        reply = ask(fix).await?;
    };
    let png = png_path(dir, number);
    tokio::task::block_in_place(|| picture.save_with_format(&png, image::ImageFormat::Png))
        .context("thumbnail picture")?;
    let version = Version {
        number,
        concept: concept(&reply).unwrap_or_default(),
        title: input.title.trim().to_owned(),
    };
    let mut all = previous;
    all.push(version.clone());
    tokio::fs::write(versions_path(dir), serde_json::to_vec_pretty(&all)?).await?;
    Ok(version)
}

/// Writes the picture of proposal `number` for YouTube: JPEG under
/// [`MAX_BYTES`] (PNG if `out` asks for it and it fits). Returns the size
/// in bytes.
pub fn export_image(dir: &Path, number: u32, out: &Path) -> Result<usize> {
    let picture = image::open(png_path(dir, number))
        .context("thumbnail picture")?
        .to_rgb8();
    let png = out
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("png"));
    if png {
        let mut bytes = std::io::Cursor::new(Vec::new());
        picture.write_to(&mut bytes, image::ImageFormat::Png)?;
        let bytes = bytes.into_inner();
        anyhow::ensure!(
            bytes.len() <= MAX_BYTES,
            "the PNG is larger than the 2 MB YouTube accepts: export as JPEG"
        );
        std::fs::write(out, &bytes)?;
        return Ok(bytes.len());
    }
    let mut picture = picture;
    for attempt in 0..8_u8 {
        let quality = 92_u8.saturating_sub(attempt.saturating_mul(4));
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality)
            .encode_image(&picture)?;
        if bytes.len() <= MAX_BYTES {
            std::fs::write(out, &bytes)?;
            return Ok(bytes.len());
        }
        // Very textured picture: the YouTube size, 1280×720.
        if attempt == 3 {
            picture = image::imageops::resize(
                &picture,
                WIDTH,
                HEIGHT,
                image::imageops::FilterType::Lanczos3,
            );
        }
    }
    anyhow::bail!("the thumbnail stays larger than 2 MB")
}

/// Writes proposal `number` as a layered Photoshop document. Returns the
/// number of layers.
pub async fn export_psd(renderer: &Renderer, dir: &Path, number: u32, out: &Path) -> Result<usize> {
    let html = html_path(dir, number);
    anyhow::ensure!(html.is_file(), "thumbnail page missing: {}", html.display());
    let rendered = render_still(renderer, &html, WIDTH, HEIGHT, SCALE, true).await?;
    let layers: Vec<PsdLayer<'_>> = rendered
        .layers
        .iter()
        .map(|l| PsdLayer {
            name: &l.name,
            image: &l.image,
        })
        .collect();
    let bytes = tokio::task::block_in_place(|| psd::write(&rendered.composite, &layers))?;
    tokio::fs::write(out, bytes).await?;
    Ok(layers.len())
}

/// Pictures of the library named with `@` in the notes, shown to the
/// designer (JPEG and PNG: what the models read).
fn mentioned_images(sources: &Sources<'_>, notes: &str) -> Vec<ThumbnailImage> {
    sources
        .look
        .mentioned(notes)
        .into_iter()
        .filter(|a| a.kind == enfait_core::AssetKind::Image)
        .filter_map(|a| {
            let path = sources.library_file(&a.file)?;
            let ext = path.extension()?.to_str()?.to_ascii_lowercase();
            if !matches!(ext.as_str(), "png" | "jpg" | "jpeg") {
                return None;
            }
            let (width, height) = image::image_dimensions(&path).ok()?;
            Some(ThumbnailImage {
                src: format!("{}/{}", enfait_motion::user::ASSETS_DIR, a.file),
                path,
                width,
                height,
                what: format!(
                    "library file named by the author with @{}: {}",
                    a.file, a.name
                ),
            })
        })
        .collect()
}

/// What the voice says, in order.
fn transcript_text(sources: &Sources<'_>) -> String {
    sources
        .transcript
        .words
        .iter()
        .map(|w| w.text.trim())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Art direction of the video, if the director made one.
fn art_direction(studio: &Path) -> Option<String> {
    #[derive(Deserialize)]
    struct Saved {
        direction: enfait_ai::direction::Direction,
    }
    let data = std::fs::read(studio.join("direction.app.json")).ok()?;
    let saved: Saved = serde_json::from_slice(&data).ok()?;
    Some(saved.direction.art_direction())
}

/// A frame of the edit, as stored in `stills/list.json`.
#[derive(Serialize, Deserialize)]
struct Stored {
    src: String,
    width: u32,
    height: u32,
    what: String,
}

#[derive(Serialize, Deserialize)]
struct StillList {
    key: String,
    images: Vec<Stored>,
}

/// Where a frame comes from.
enum Source {
    /// Rendered clip of a block, at a time of the clip.
    Clip(PathBuf, f64),
    /// The take, at a time of the take.
    Rush(f64),
}

fn timecode(t: f64) -> String {
    // Positive duration of a video: a few hours at most.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let s = t.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Frames of the edit spread over the video, and the person cut out of the
/// take; reused as long as the edit does not change.
async fn stills(
    sources: &Sources<'_>,
    has_video: bool,
    dir: &Path,
    say: &(dyn Fn(Step) + Sync),
) -> Result<Vec<ThumbnailImage>> {
    let project = sources.project;
    let settings = project.settings();
    let visuals = Visuals::of(sources.studio, project);
    let mut candidates: Vec<(usize, f64, Source)> = Vec::new();
    for (i, (block, (a, b, clip))) in project.blocks().iter().zip(&visuals.blocks).enumerate() {
        // Late in the block: the animation has settled.
        let local = (b - a) * 0.6;
        match clip {
            Some(clip) => candidates.push((i, a + local, Source::Clip(clip.clone(), local))),
            None if block.kind == BlockKind::Rush && has_video => {
                if let Some((s, e)) = source_pieces(sources.parts, *a, *b).first() {
                    candidates.push((i, a + local, Source::Rush((s + e) / 2.0)));
                }
            }
            None => {}
        }
    }
    // Evenly spread picks.
    let picks: Vec<&(usize, f64, Source)> = if candidates.len() <= MAX_STILLS {
        candidates.iter().collect()
    } else {
        (0..MAX_STILLS)
            .filter_map(|k| candidates.get(k * (candidates.len() - 1) / (MAX_STILLS - 1)))
            .collect()
    };
    // For the cutout: a frame of the take, the first filmed one, else at a
    // third of the take.
    let person_at = has_video.then(|| {
        picks
            .iter()
            .find_map(|(_, _, s)| match s {
                Source::Rush(t) => Some(*t),
                Source::Clip(..) => None,
            })
            .or_else(|| sources.parts.first().map(|(s, e)| s + (e - s) / 3.0))
            .unwrap_or(0.0)
    });
    let mut h = blake3::Hasher::new();
    h.update(sources.rush.to_string_lossy().as_bytes());
    for (i, t, source) in &picks {
        h.update(&i.to_le_bytes());
        h.update(&t.to_le_bytes());
        if let Source::Clip(clip, _) = source {
            h.update(clip.to_string_lossy().as_bytes());
            if let Ok(modified) = std::fs::metadata(clip).and_then(|m| m.modified()) {
                h.update(format!("{modified:?}").as_bytes());
            }
        }
    }
    h.update(format!("{person_at:?}").as_bytes());
    let key = h.finalize().to_hex().to_string();
    let folder = dir.join("stills");
    let list_path = folder.join("list.json");
    let resolve = |list: StillList| {
        list.images
            .into_iter()
            .map(|s| ThumbnailImage {
                path: dir.join(&s.src),
                src: s.src,
                width: s.width,
                height: s.height,
                what: s.what,
            })
            .collect::<Vec<_>>()
    };
    if let Ok(data) = tokio::fs::read(&list_path).await
        && let Ok(list) = serde_json::from_slice::<StillList>(&data)
        && list.key == key
        && list.images.iter().all(|s| dir.join(&s.src).is_file())
    {
        return Ok(resolve(list));
    }
    if tokio::fs::try_exists(&folder).await.unwrap_or(false) {
        tokio::fs::remove_dir_all(&folder).await?;
    }
    tokio::fs::create_dir_all(&folder).await?;
    let (w, h) = (
        (f64::from(WIDTH) * SCALE).round(),
        (f64::from(HEIGHT) * SCALE).round(),
    );
    // Whole numbers of pixels, a few thousand at most.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (w, h) = (w as u32, h as u32);
    let framing = enfait_media::fx::framing(w, h, settings.framing, settings.pan());
    let fit = enfait_media::fx::framing(w, h, enfait_core::Framing::Fit, 0);
    let mut images = Vec::new();
    for (k, (i, t, source)) in picks.iter().enumerate() {
        let src = format!("stills/{:02}.jpg", k.saturating_add(1));
        let out = dir.join(&src);
        let what = match source {
            Source::Clip(clip, local) => {
                grab(clip, *local, &fit, &out).await?;
                "animation"
            }
            Source::Rush(at) => {
                grab(sources.rush, *at, &framing, &out).await?;
                "filmed take"
            }
        };
        images.push(Stored {
            src,
            width: w,
            height: h,
            what: format!(
                "frame of the video at {} (block {}, {what})",
                timecode(*t),
                i.saturating_add(1)
            ),
        });
    }
    if let Some(at) = person_at {
        say(Step::Cutout);
        let frame = folder.join("person-frame.jpg");
        grab(sources.rush, at, &framing, &frame).await?;
        match cut_person(&frame, &folder.join("person.png")).await {
            Ok(()) => images.push(Stored {
                src: "stills/person.png".into(),
                width: w,
                height: h,
                what: format!(
                    "the filmed person alone, cut out (transparent background), from the take \
                     at {}; the uncut frame is `stills/person-frame.jpg`",
                    timecode(at)
                ),
            }),
            // Without the cutout the thumbnail can still be made.
            Err(e) => tracing::warn!("thumbnail cutout: {e:#}"),
        }
    }
    let list = StillList { key, images };
    tokio::fs::write(&list_path, serde_json::to_vec_pretty(&list)?).await?;
    Ok(resolve(list))
}

/// One frame of `media` at `t` seconds, framed by `filter`, as JPEG.
async fn grab(media: &Path, t: f64, filter: &str, out: &Path) -> Result<()> {
    let output = enfait_config::process::tokio_command("ffmpeg")
        .args([
            "-nostdin",
            "-v",
            "error",
            "-y",
            "-ss",
            &format!("{:.3}", t.max(0.0)),
        ])
        .arg("-i")
        .arg(media)
        .args(["-frames:v", "1", "-vf", filter, "-q:v", "2"])
        .arg(out)
        .output()
        .await
        .context("starting ffmpeg")?;
    anyhow::ensure!(
        output.status.success() && out.is_file(),
        "frame of {}: {}",
        media.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

async fn cut_person(frame: &Path, out: &Path) -> Result<()> {
    use enfait_assets::cutout::{Cutter, Model, cut_file, ensure_model};
    let models = enfait_config::paths::cutout_models_dir();
    tokio::fs::create_dir_all(&models).await?;
    let model = ensure_model(&models, Model::Precise).await?;
    tokio::task::block_in_place(|| {
        let cutter = Cutter::load(&model, Model::Precise)?;
        cut_file(&cutter, frame, out)
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_need_their_picture() {
        let dir = tempfile::tempdir().unwrap();
        let all = vec![
            Version {
                number: 1,
                concept: "a".into(),
                title: "t".into(),
            },
            Version {
                number: 2,
                concept: "b".into(),
                title: "t".into(),
            },
        ];
        std::fs::write(versions_path(dir.path()), serde_json::to_vec(&all).unwrap()).unwrap();
        image::RgbImage::new(4, 4)
            .save(png_path(dir.path(), 2))
            .unwrap();
        assert_eq!(versions(dir.path()), all[1..]);
        assert_eq!(timecode(83.4), "1:23");
    }

    #[test]
    fn export_stays_under_the_youtube_limit() {
        let dir = tempfile::tempdir().unwrap();
        // Noise: the hardest picture to compress.
        let mut seed = 1_u32;
        let noisy = image::RgbImage::from_fn(1920, 1080, |_, _| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let [a, b, c, _] = seed.to_le_bytes();
            image::Rgb([a, b, c])
        });
        noisy.save(png_path(dir.path(), 1)).unwrap();
        let out = dir.path().join("mini.jpg");
        let size = export_image(dir.path(), 1, &out).unwrap();
        assert!(size <= MAX_BYTES && out.is_file());
        assert!(export_image(dir.path(), 1, &dir.path().join("mini.png")).is_err());
    }
}
