//! `enfait-cli eval` : jeu d'évaluation des prompts.
//!
//! Chaque cas est un bloc isolé (texte, mots datés, durée, style). La commande
//! le fait diriger, animer et rendre comme `make`, puis assemble une planche
//! HTML où l'on note chaque scène. Une passe = un dossier ; l'empreinte des
//! prompts y est inscrite pour comparer les passes entre elles.

use crate::encode;
use crate::make::{self, Ctx, GpuArg, ProviderChoice, QualityArg};
use crate::paths::Dirs;
use crate::timeline::Timeline;
use anyhow::{Context as _, Result};
use enfait_ai::animate::{AnimatorConfig, SceneBrief, TimedWord, animator_system};
use enfait_ai::direction::Direction;
use enfait_ai::{Provider, Quality, Usage};
use enfait_core::{Flicks, FrameRate, TimeRange};
use enfait_motion::{RendererPool, composition, frame_name, library};
use futures::StreamExt as _;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tracing::Instrument as _;

const SHEET: &str = include_str!("sheet.html");

#[derive(Debug, clap::Args)]
pub struct EvalArgs {
    /// Dossier de la passe (une passe par version des prompts).
    pub work: PathBuf,
    /// Jeu de cas (voir `eval/generate.py`).
    #[arg(long, default_value = "eval/cases.json")]
    pub cases: PathBuf,
    /// Ne traiter que ces cas (identifiants séparés par des virgules).
    #[arg(long, value_delimiter = ',')]
    pub only: Vec<String>,
    /// Refaire direction et animation des cas traités, même déjà faits.
    #[arg(long)]
    pub redo: bool,
    #[arg(long, value_enum, default_value = "polished")]
    pub quality: QualityArg,
    #[arg(long, value_enum, default_value = "auto")]
    pub provider: ProviderChoice,
    #[arg(long, default_value_t = 30)]
    pub fps: u32,
    /// Cas traités en parallèle.
    #[arg(long, default_value_t = 3)]
    pub jobs: usize,
    #[arg(long, value_enum, default_value = "auto")]
    pub gpu: GpuArg,
    /// Refaire seulement la planche, à partir des rendus existants.
    #[arg(long)]
    pub sheet_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Case {
    id: String,
    theme: String,
    style: Option<String>,
    aspect: String,
    language: String,
    context: String,
    text: String,
    duration: f64,
    words: Vec<CaseWord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CaseWord {
    t: f64,
    text: String,
}

/// Ce qui définit une passe : à comparer avant de comparer des notes.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RunInfo {
    /// Empreinte de l'ensemble des prompts et styles.
    fingerprint: String,
    /// Empreinte de chaque texte, pour savoir lequel a changé.
    prompts: BTreeMap<String, String>,
    quality: String,
    model: String,
    provider: String,
    /// Secondes depuis l'époque Unix.
    created: u64,
}

fn short_hash(data: &[u8]) -> String {
    blake3::hash(data).to_hex().chars().take(10).collect()
}

fn fingerprint() -> (String, BTreeMap<String, String>) {
    let mut all = blake3::Hasher::new();
    let mut each = BTreeMap::new();
    for (name, text) in enfait_ai::prompt_sources() {
        all.update(name.as_bytes());
        all.update(&[0]);
        all.update(text.as_bytes());
        all.update(&[0]);
        each.insert(name.to_owned(), short_hash(text.as_bytes()));
    }
    (all.finalize().to_hex().chars().take(10).collect(), each)
}

fn dimensions(aspect: &str) -> Result<(u32, u32)> {
    match aspect {
        "16:9" => Ok((1920, 1080)),
        "9:16" => Ok((1080, 1920)),
        "1:1" => Ok((1080, 1080)),
        other => anyhow::bail!("format « {other} » inconnu (16:9, 9:16 ou 1:1)"),
    }
}

struct Shared<'a> {
    work: &'a Path,
    provider: &'a Provider,
    renderers: &'a RendererPool,
    animator: &'a AnimatorConfig,
    critique_rounds: u8,
    fonts: &'a str,
    fps: u32,
    redo: bool,
    usage: &'a Mutex<Usage>,
}

fn case_dir(work: &Path, id: &str) -> PathBuf {
    work.join("cases").join(id)
}

async fn run_case(s: &Shared<'_>, case: &Case) -> Result<()> {
    let dir = case_dir(s.work, &case.id);
    tokio::fs::create_dir_all(&dir).await?;
    if s.redo {
        let _ = tokio::fs::remove_file(dir.join("direction.json")).await;
    }
    let preset = match &case.style {
        Some(slug) => Some(
            enfait_ai::styles::find(slug).with_context(|| format!("style « {slug} » inconnu"))?,
        ),
        None => None,
    };
    let style = enfait_ai::styles::combine(preset, None);
    let (width, height) = dimensions(&case.aspect)?;
    // Durée de quelques secondes, positive : la conversion ne déborde pas.
    #[allow(clippy::cast_possible_truncation)]
    let millis = (case.duration * 1000.0).round().max(1.0) as i64;
    let range = TimeRange::new(Flicks::ZERO, Flicks::from_millis(millis)?)?;
    let script = format!(
        "Sujet de la vidéo : {}\n\nCe bloc est évalué isolément : traite-le comme une scène \
         d'une vidéo plus longue sur ce sujet.",
        case.context
    );
    let direction = make::direct(
        s.provider,
        &dir,
        &[range],
        std::slice::from_ref(&case.text),
        width,
        height,
        Some(&script),
        style.as_deref(),
        s.fonts,
        0,
    )
    .await?;

    library::install(&dir.join("comps")).context("bibliothèque des compositions")?;
    let ctx = Ctx {
        work: dir.clone(),
        provider: s.provider,
        renderers: s.renderers,
        animator: s.animator.clone(),
        critique_rounds: s.critique_rounds,
        width,
        height,
        fps: f64::from(s.fps),
        usage: s.usage,
        scene_frames: None,
        scene_cutouts: None,
    };
    let brief = SceneBrief {
        index: 1,
        total: 1,
        width,
        height,
        duration: range.duration().as_secs_f64(),
        text: case.text.clone(),
        words: case
            .words
            .iter()
            .map(|w| TimedWord {
                t: w.t,
                text: w.text.clone(),
            })
            .collect(),
        art_direction: direction.art_direction(),
        intent: direction.intent(1).to_owned(),
        previous: None,
        next: None,
        user_style: style,
        library: None,
        moments: Vec::new(),
        layer: None,
    };
    make::animate_block(&ctx, &brief, s.redo).await?;

    let timeline = Timeline::build(FrameRate::new(s.fps, 1)?, &[range], &[], 1.0)?;
    make::render_all(&ctx, &timeline, 1).await?;
    encode::encode(
        &dir,
        &timeline,
        None,
        range.duration().as_secs_f64(),
        None,
        &dir.join("video.mp4"),
    )
    .await?;
    Ok(())
}

pub async fn run(dirs: &Dirs, args: EvalArgs) -> Result<()> {
    let started = std::time::Instant::now();
    let work = std::path::absolute(&args.work)?;
    tokio::fs::create_dir_all(&work).await?;
    let data = tokio::fs::read(&args.cases)
        .await
        .with_context(|| format!("lecture de {}", args.cases.display()))?;
    let cases: Vec<Case> = serde_json::from_slice(&data).context("jeu de cas illisible")?;
    if let Some(unknown) = args
        .only
        .iter()
        .find(|id| !cases.iter().any(|c| &c.id == *id))
    {
        anyhow::bail!("cas « {unknown} » absent de {}", args.cases.display());
    }
    let selected: Vec<&Case> = cases
        .iter()
        .filter(|c| args.only.is_empty() || args.only.contains(&c.id))
        .collect();

    if !args.sheet_only {
        let quality: Quality = args.quality.into();
        let qs = quality.settings();
        let provider = make::make_provider(args.provider, &work)?;
        let (fingerprint, prompts) = fingerprint();
        let info_path = work.join("run.json");
        if let Ok(old) = tokio::fs::read(&info_path).await
            && let Ok(old) = serde_json::from_slice::<RunInfo>(&old)
            && old.fingerprint != fingerprint
        {
            let changed: Vec<&str> = prompts
                .iter()
                .filter(|(name, hash)| old.prompts.get(*name) != Some(*hash))
                .map(|(name, _)| name.as_str())
                .collect();
            tracing::warn!(
                "les prompts ont changé depuis cette passe ({}) : les cas déjà faits gardent \
                 l'ancienne version, les nouveaux suivront la nouvelle. Pour comparer, \
                 lancer une nouvelle passe dans un autre dossier.",
                changed.join(", ")
            );
        }
        let created = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let info = RunInfo {
            fingerprint,
            prompts,
            quality: format!("{quality:?}"),
            model: qs.model.clone(),
            provider: provider.name().to_owned(),
            created,
        };
        tokio::fs::write(&info_path, serde_json::to_vec_pretty(&info)?).await?;
        tracing::info!(
            "passe {} : {} cas, soin {:?} via {}",
            info.fingerprint,
            selected.len(),
            quality,
            provider.name()
        );

        let fonts = make::fonts_list();
        let animator = AnimatorConfig {
            system: animator_system(composition::CONTRACT, &fonts),
            model: qs.model.clone(),
            effort: qs.effort,
        };
        let chrome = enfait_motion::ensure_browser(&dirs.chromium()).await?;
        let gpu = if args.gpu.nvidia() {
            enfait_motion::Gpu::Vulkan {
                nvidia_offload: true,
            }
        } else {
            enfait_motion::Gpu::Software
        };
        let jobs = args.jobs.max(1);
        let renderers = RendererPool::launch(&chrome, &dirs.browser_profile(), gpu, jobs).await?;
        let usage = Mutex::new(Usage::default());
        let shared = Shared {
            work: &work,
            provider: &provider,
            renderers: &renderers,
            animator: &animator,
            critique_rounds: qs.critique_rounds,
            fonts: &fonts,
            fps: args.fps,
            redo: args.redo,
            usage: &usage,
        };
        let results: Vec<(&Case, Result<()>)> = futures::stream::iter(selected.iter().copied())
            .map(|case| {
                let shared = &shared;
                let span = tracing::info_span!("cas", id = %case.id);
                async move { (case, run_case(shared, case).await) }.instrument(span)
            })
            .buffer_unordered(jobs)
            .collect()
            .await;
        renderers.close().await;
        let mut failed = 0_usize;
        for (case, result) in results {
            let error_path = case_dir(&work, &case.id).join("error.txt");
            match result {
                Ok(()) => {
                    let _ = tokio::fs::remove_file(&error_path).await;
                }
                Err(e) => {
                    failed = failed.saturating_add(1);
                    tracing::error!("{} : {e:#}", case.id);
                    tokio::fs::create_dir_all(case_dir(&work, &case.id)).await?;
                    tokio::fs::write(&error_path, format!("{e:#}")).await?;
                }
            }
        }
        tracing::info!(
            "{} cas en {} s, {failed} en échec",
            selected.len(),
            started.elapsed().as_secs()
        );
        make::log_usage(&usage);
    }

    let sheet = write_sheet(&work, &cases).await?;
    tracing::info!("planche à noter : {}", sheet.display());
    Ok(())
}

/// Un cas tel que la planche l'affiche.
#[derive(Serialize)]
struct SheetCase<'a> {
    #[serde(flatten)]
    case: &'a Case,
    style_name: Option<&'static str>,
    concept: String,
    intent: String,
    video: Option<String>,
    poster: Option<String>,
    error: Option<String>,
}

async fn write_sheet(work: &Path, cases: &[Case]) -> Result<PathBuf> {
    // Polices de l'interface (IBM Plex) pour la planche.
    library::install(work).context("polices de la planche")?;
    let mut rows = Vec::with_capacity(cases.len());
    for case in cases {
        let dir = case_dir(work, &case.id);
        let direction: Option<Direction> = tokio::fs::read(dir.join("direction.json"))
            .await
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok());
        let has_video = tokio::fs::try_exists(dir.join("video.mp4"))
            .await
            .unwrap_or(false);
        let poster = poster(&dir).await;
        let rel = |p: &str| format!("cases/{}/{p}", case.id);
        rows.push(SheetCase {
            case,
            style_name: case
                .style
                .as_deref()
                .and_then(enfait_ai::styles::find)
                .map(|p| p.name),
            concept: direction
                .as_ref()
                .map(|d| d.concept.clone())
                .unwrap_or_default(),
            intent: direction
                .as_ref()
                .map(|d| d.intent(1).to_owned())
                .unwrap_or_default(),
            video: has_video.then(|| rel("video.mp4")),
            poster: poster.map(|p| rel(&p)),
            error: tokio::fs::read_to_string(dir.join("error.txt")).await.ok(),
        });
    }
    let run: Option<RunInfo> = tokio::fs::read(work.join("run.json"))
        .await
        .ok()
        .and_then(|d| serde_json::from_slice(&d).ok());
    // Notes déjà exportées et rangées dans le dossier de la passe.
    let notes: Option<serde_json::Value> = tokio::fs::read(work.join("notes.json"))
        .await
        .ok()
        .and_then(|d| serde_json::from_slice(&d).ok());
    let data = serde_json::json!({ "run": run, "notes": notes, "cases": rows });
    // Le JSON est placé dans une balise <script> : `</` ne doit pas la fermer.
    let json = serde_json::to_string(&data)?.replace("</", "<\\/");
    let path = work.join("planche.html");
    tokio::fs::write(&path, SHEET.replace("/*DATA*/null", &json)).await?;
    Ok(path)
}

/// Image représentative : aux trois cinquièmes du rendu, quand le geste
/// principal est en général posé.
async fn poster(dir: &Path) -> Option<String> {
    let frames = dir.join("frames").join("b01");
    let mut count = 0_usize;
    let mut entries = tokio::fs::read_dir(&frames).await.ok()?;
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry.path().extension().is_some_and(|e| e == "jpg") {
            count = count.saturating_add(1);
        }
    }
    let index = count.saturating_mul(3) / 5;
    (count > 0).then(|| format!("frames/b01/{}", frame_name(index)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_cases_are_consistent() {
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("../../../eval/cases.json")).unwrap();
        assert_eq!(cases.len(), 30);
        let mut ids: Vec<&str> = cases.iter().map(|c| c.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), cases.len(), "identifiants en double");
        for c in &cases {
            dimensions(&c.aspect).unwrap();
            if let Some(s) = &c.style {
                assert!(enfait_ai::styles::find(s).is_some(), "{} : style {s}", c.id);
            }
            assert!(!c.words.is_empty(), "{}", c.id);
            assert!(c.words.windows(2).all(|w| w[0].t < w[1].t), "{}", c.id);
            assert!(c.words.iter().all(|w| w.t >= 0.0 && w.t < c.duration));
            let joined: Vec<&str> = c.words.iter().map(|w| w.text.as_str()).collect();
            assert_eq!(joined.join(" "), c.text, "{}", c.id);
        }
    }

    #[test]
    fn fingerprint_covers_every_prompt() {
        let (all, each) = fingerprint();
        assert_eq!(all.len(), 10);
        assert!(each.contains_key("director") && each.contains_key("craie"));
    }

    #[test]
    fn sheet_has_a_data_slot() {
        assert_eq!(SHEET.matches("/*DATA*/null").count(), 1);
    }
}
