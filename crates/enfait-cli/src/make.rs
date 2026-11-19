//! `enfait-cli make`: voice-over → video.
//!
//! Each step writes its result into the working folder and reuses it on
//! the next run: you can run again after an error, or redo only some
//! blocks with `--redo`.

use crate::encode;
use crate::paths::{Dirs, default_work_dir, find_in_path};
use crate::timeline::Timeline;
use anyhow::{Context as _, Result};
use enfait_ai::animate::{AnimatorConfig, SceneBrief, animator_system};
use enfait_ai::{Provider, Quality, Usage};
use enfait_core::{
    AspectRatio, Block, BlockKind, FrameRate, Project, ProjectSettings, Resolution, Transcript,
};
use enfait_motion::{FrameJob, RendererPool, composition, frame_name, library};
use futures::StreamExt as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub(crate) use enfait_studio::{
    Studio as Ctx, TRANSITION_LENGTH, animate_block, animate_transition, brief, direct, fonts_list,
    log_usage,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ProviderChoice {
    /// API if an Anthropic key is set, else Claude Code.
    Auto,
    Api,
    ClaudeCode,
    /// Local model (Settings → Models, or `ENFAIT_LOCAL_MODEL`, `ENFAIT_LOCAL_URL`).
    Local,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Aspect {
    #[value(name = "16:9")]
    Landscape,
    #[value(name = "9:16")]
    Portrait,
    #[value(name = "1:1")]
    Square,
}

#[derive(Debug, clap::Args)]
pub struct MakeArgs {
    /// Voice-over (any format ffmpeg reads).
    pub voice: PathBuf,
    /// Original script (text or Markdown), for vocabulary and context.
    #[arg(long)]
    pub script: Option<PathBuf>,
    /// The user's style rules (Markdown), taking precedence.
    #[arg(long)]
    pub style: Option<PathBuf>,
    /// Style preset (see `enfait-cli styles`).
    #[arg(long)]
    pub preset: Option<String>,
    /// Look of the app (`look.json` of the session): style, post-FX and
    /// library (in `library/` next to it). `--preset` and `--style` win over
    /// its style.
    #[arg(long)]
    pub look: Option<PathBuf>,
    #[arg(long, value_enum, default_value = "polished")]
    pub quality: QualityArg,
    #[arg(long, value_enum, default_value = "16:9")]
    pub aspect: Aspect,
    #[arg(long, default_value_t = 30)]
    pub fps: u32,
    #[arg(long, value_enum, default_value = "auto")]
    pub provider: ProviderChoice,
    /// Produced video file (by default `<working folder>/video.mp4`).
    #[arg(long, short)]
    pub out: Option<PathBuf>,
    /// Working folder (by default `<voice>.enfait/`).
    #[arg(long)]
    pub work: Option<PathBuf>,
    /// Blocks processed in parallel.
    #[arg(long, default_value_t = 3)]
    pub jobs: usize,
    /// Language of the voice (`fr`, `en`…), detected otherwise.
    #[arg(long)]
    pub language: Option<String>,
    /// Blocks to redo (numbers from 1), for example `--redo 2,5`.
    #[arg(long, value_delimiter = ',')]
    pub redo: Vec<usize>,
    /// Maximum number of crafted transitions.
    #[arg(long, default_value_t = 1)]
    pub transitions: usize,
    /// Only process the first N blocks (quick tests).
    #[arg(long)]
    pub limit: Option<usize>,
    /// GPU acceleration of the Chromium render.
    #[arg(long, value_enum, default_value = "auto")]
    pub gpu: GpuArg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum GpuArg {
    /// NVIDIA card if `nvidia-smi` is present, else the processor.
    Auto,
    /// NVIDIA card, through Vulkan.
    Nvidia,
    /// Everything on the processor.
    Cpu,
}

impl GpuArg {
    pub(crate) fn nvidia(self) -> bool {
        match self {
            Self::Auto => find_in_path("nvidia-smi").is_some(),
            Self::Nvidia => true,
            Self::Cpu => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum QualityArg {
    Fast,
    Polished,
    Ultra,
}

impl From<QualityArg> for Quality {
    fn from(q: QualityArg) -> Self {
        match q {
            QualityArg::Fast => Self::Fast,
            QualityArg::Polished => Self::Polished,
            QualityArg::Ultra => Self::Ultra,
        }
    }
}

pub async fn run(dirs: &Dirs, args: MakeArgs) -> Result<()> {
    let started = std::time::Instant::now();
    let voice = std::path::absolute(&args.voice)?;
    let work = match &args.work {
        Some(w) => std::path::absolute(w)?,
        None => default_work_dir(&voice),
    };
    tokio::fs::create_dir_all(&work).await?;
    tracing::info!("working folder: {}", work.display());
    let script = read_optional(args.script.as_deref()).await?;
    let look = match &args.look {
        Some(path) => Some(Look::load(path).await?),
        None => None,
    };
    let preset_slug = args
        .preset
        .clone()
        .or_else(|| look.as_ref().and_then(|l| l.look.style.preset.clone()));
    let preset = match &preset_slug {
        Some(slug) => Some(enfait_ai::styles::find(slug).with_context(|| {
            format!("unknown style \"{slug}\": `enfait-cli styles` lists the styles")
        })?),
        None => None,
    };
    if let Some(p) = preset {
        tracing::info!("imposed style: {}", p.name);
    }
    let user_rules = match read_optional(args.style.as_deref()).await? {
        Some(rules) => Some(rules),
        None => look
            .as_ref()
            .map(|l| l.look.style.rules.trim().to_owned())
            .filter(|r| !r.is_empty()),
    };
    let user_style = enfait_ai::styles::combine(preset, user_rules.as_deref());

    // 1. Transcription.
    let transcript = transcribe(dirs, &voice, &work, &args, script.as_deref()).await?;

    // 2. Blocks.
    let mut ranges =
        enfait_speech::segment(&transcript, &enfait_speech::SegmentParams::standard()?)?;
    if let Some(limit) = args.limit {
        ranges.truncate(limit.max(1));
    }
    let (width, height, aspect) = match args.aspect {
        Aspect::Landscape => (1920, 1080, AspectRatio::Landscape16x9),
        Aspect::Portrait => (1080, 1920, AspectRatio::Portrait9x16),
        Aspect::Square => (1080, 1080, AspectRatio::Square1x1),
    };
    let rate = FrameRate::new(args.fps, 1)?;
    let settings = ProjectSettings {
        aspect,
        resolution: Resolution { width, height },
        frame_rate: rate,
        ..ProjectSettings::default()
    };
    let texts: Vec<String> = ranges.iter().map(|r| transcript.text_in(*r)).collect();
    tracing::info!("{} blocks:", ranges.len());
    for (i, (r, text)) in ranges.iter().zip(&texts).enumerate() {
        tracing::info!(
            "  {:>2}. {:>6.2} → {:>6.2}  {}",
            i.saturating_add(1),
            r.start().as_secs_f64(),
            r.end().as_secs_f64(),
            text
        );
    }

    // 3. Direction.
    let provider = make_provider(args.provider, &work)?;
    let quality: Quality = args.quality.into();
    let qs = quality.settings();
    tracing::info!(
        "models through {}; quality {:?} ({}, effort {}, {} critique(s))",
        provider.name(),
        quality,
        qs.model,
        qs.effort,
        qs.critique_rounds
    );
    let mut fonts = fonts_list();
    if let Some(look) = &look {
        fonts.push_str(&look.font_lines());
    }
    let library = look.as_ref().and_then(Look::brief);
    let direction = direct(
        &provider,
        &work,
        &ranges,
        &texts,
        width,
        height,
        script.as_deref(),
        user_style.as_deref(),
        &fonts,
        args.transitions,
    )
    .await?;

    let blocks: Vec<Block> = ranges
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut block = Block::new(*r, BlockKind::Animation);
            direction
                .intent(i.saturating_add(1))
                .clone_into(&mut block.description);
            block
        })
        .collect();
    let name = voice
        .file_stem()
        .map_or_else(|| "projet".into(), |s| s.to_string_lossy().into_owned());
    let project = Project::with_blocks(name, settings, blocks)?;
    tokio::fs::write(work.join("project.json"), project.to_json()?).await?;

    // 4. Compositions.
    let chrome = enfait_motion::ensure_browser(&dirs.chromium()).await?;
    library::install(&work.join("comps"))
        .context("library of the compositions (run `enfait-cli setup`)")?;
    if let Some(look) = &look {
        look.install(&work.join("comps"))?;
    }
    let nvidia = args.gpu.nvidia();
    let gpu = if nvidia {
        enfait_motion::Gpu::Vulkan {
            nvidia_offload: true,
        }
    } else {
        enfait_motion::Gpu::Software
    };
    tracing::info!(
        "rendu {}",
        if nvidia {
            "on the NVIDIA card"
        } else {
            "on the processor"
        }
    );
    let renderers = RendererPool::launch(&chrome, &dirs.browser_profile(), gpu, args.jobs).await?;
    let usage = Mutex::new(Usage::default());
    let result = async {
        let ctx = Ctx {
            work: work.clone(),
            provider: &provider,
            renderers: &renderers,
            animator: AnimatorConfig {
                system: animator_system(composition::CONTRACT, &fonts),
                model: qs.model.clone(),
                effort: qs.effort,
            },
            critique_rounds: qs.critique_rounds,
            width,
            height,
            fps: f64::from(args.fps),
            usage: &usage,
            scene_frames: None,
            scene_cutouts: None,
        };
        let total = ranges.len();
        let art = direction.art_direction();
        let briefs: Vec<SceneBrief> = (0..total)
            .map(|i| {
                let mut b = brief(
                    i,
                    &ranges,
                    &texts,
                    &transcript,
                    &direction,
                    &art,
                    width,
                    height,
                    user_style.as_deref(),
                );
                b.library.clone_from(&library);
                b
            })
            .collect();
        let outcomes: Vec<Result<()>> = futures::stream::iter(briefs.iter())
            .map(|b| animate_block(&ctx, b, args.redo.contains(&b.index)))
            .buffer_unordered(args.jobs.max(1))
            .collect()
            .await;
        for outcome in outcomes {
            outcome?;
        }

        let transition_after: Vec<usize> = (0..total.saturating_sub(1))
            .filter(|i| direction.transition_after(i.saturating_add(1)).is_some())
            .take(args.transitions)
            .collect();
        for &after in &transition_after {
            animate_transition(
                &ctx,
                after,
                &direction,
                &art,
                args.redo.contains(&after.saturating_add(1)),
            )
            .await?;
        }

        // 5. Render.
        let timeline = Timeline::build(rate, &ranges, &transition_after, TRANSITION_LENGTH)?;
        render_all(&ctx, &timeline, args.jobs).await?;

        // 6. Encoding.
        let out = match &args.out {
            Some(o) => std::path::absolute(o)?,
            None => work.join("video.mp4"),
        };
        let duration = ranges.last().map_or(0.0, |r| r.end().as_secs_f64());
        let fx = look
            .as_ref()
            .and_then(|l| enfait_media::fx::filter(&l.look.fx, height));
        if fx.is_some() {
            tracing::info!("post-FX of the look applied to the encoding");
        }
        encode::encode(
            &ctx.work,
            &timeline,
            Some(&voice),
            duration,
            fx.as_deref(),
            &out,
        )
        .await?;
        tracing::info!(
            "done in {} s: {}",
            started.elapsed().as_secs(),
            out.display()
        );
        log_usage(&usage);
        Ok(())
    }
    .await;
    renderers.close().await;
    result
}

pub(crate) async fn read_optional(path: Option<&Path>) -> Result<Option<String>> {
    match path {
        Some(p) => Ok(Some(
            tokio::fs::read_to_string(p)
                .await
                .with_context(|| format!("reading {}", p.display()))?,
        )),
        None => Ok(None),
    }
}

async fn transcribe(
    dirs: &Dirs,
    voice: &Path,
    work: &Path,
    args: &MakeArgs,
    script: Option<&str>,
) -> Result<Transcript> {
    let cached = work.join("transcript.json");
    if let Ok(data) = tokio::fs::read(&cached).await {
        tracing::info!("transcription reprise de {}", cached.display());
        return Ok(serde_json::from_slice(&data)?);
    }
    let model = dirs.whisper_model();
    if !model.is_file() {
        anyhow::bail!(
            "whisper model missing ({}): run `enfait-cli setup`",
            model.display()
        );
    }
    tracing::info!("transcription de {}…", voice.display());
    let voice = voice.to_path_buf();
    let language = args.language.clone();
    // The start of the script steers whisper's vocabulary.
    let prompt: Option<String> = script.map(|s| s.chars().take(400).collect());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let transcript = tokio::task::spawn_blocking(move || -> Result<Transcript> {
        let pcm = enfait_speech::decode_pcm(&voice)?;
        let options = enfait_speech::TranscribeOptions {
            language: language.as_deref(),
            prompt: prompt.as_deref(),
            threads: u16::try_from(threads).unwrap_or(4),
            takes: false,
        };
        Ok(enfait_speech::transcribe(&model, &pcm, &options)?)
    })
    .await??;
    tracing::info!(
        "{} words, language {}",
        transcript.words.len(),
        transcript.language
    );
    tokio::fs::write(&cached, serde_json::to_vec_pretty(&transcript)?).await?;
    Ok(transcript)
}

pub(crate) fn make_provider(choice: ProviderChoice, work: &Path) -> Result<Provider> {
    let choice = match choice {
        ProviderChoice::Auto => enfait_ai::ProviderChoice::Auto,
        ProviderChoice::Api => enfait_ai::ProviderChoice::Api,
        ProviderChoice::ClaudeCode => enfait_ai::ProviderChoice::ClaudeCode,
        ProviderChoice::Local => enfait_ai::ProviderChoice::Local,
    };
    Ok(choice.make(&work.join("claude"))?)
}

/// Look of the app and folder of its library.
pub(crate) struct Look {
    look: enfait_core::Look,
    library: PathBuf,
}

impl Look {
    async fn load(path: &Path) -> Result<Self> {
        let data = tokio::fs::read(path)
            .await
            .with_context(|| format!("reading {}", path.display()))?;
        let look = serde_json::from_slice(&data).context("look illisible")?;
        let library = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("library");
        Ok(Self { look, library })
    }

    /// The user's fonts, in the format of [`fonts_list`].
    fn font_lines(&self) -> String {
        enfait_studio::userlib::font_lines(&self.look)
    }

    /// Files (fonts aside) for the animators' brief.
    fn brief(&self) -> Option<String> {
        enfait_studio::userlib::brief(&self.look)
    }

    /// Copies fonts and files into the compositions folder.
    fn install(&self, comps: &Path) -> Result<()> {
        enfait_studio::userlib::install(&self.look, &self.library, comps)
    }
}

pub(crate) async fn render_all(ctx: &Ctx<'_>, timeline: &Timeline, jobs: usize) -> Result<()> {
    let frames_dir = ctx.work.join("frames");
    let comps = ctx.comps();
    let block_jobs = timeline.blocks.iter().enumerate().map(|(i, b)| {
        let name = format!("b{:02}", i.saturating_add(1));
        let html = comps.join(format!("{name}.html"));
        let out = frames_dir.join(&name);
        async move {
            let times = b
                .rendered
                .clone()
                .map(|f| timeline.local_time(f, b.own.start))
                .collect::<Result<Vec<_>>>()?;
            let job = FrameJob {
                html: &html,
                width: ctx.width,
                height: ctx.height,
                times,
                scenes: None,
                extra_key: String::new(),
                out_dir: &out,
            };
            let started = std::time::Instant::now();
            let outcome = job.run(&*ctx.renderers.acquire().await?).await?;
            if outcome.cached {
                tracing::info!("{name} : images en cache");
            } else {
                tracing::info!(
                    "{name} : {} images en {:.1} s",
                    outcome.frames,
                    started.elapsed().as_secs_f64()
                );
            }
            for e in &outcome.errors {
                tracing::warn!("{name} : {e}");
            }
            Ok::<_, anyhow::Error>((i, outcome.key))
        }
    });
    let mut keys: Vec<(usize, String)> = futures::stream::iter(block_jobs)
        .buffer_unordered(jobs.max(1))
        .collect::<Vec<Result<_>>>()
        .await
        .into_iter()
        .collect::<Result<_>>()?;
    keys.sort();

    for t in &timeline.transitions {
        let from = t.after;
        let to = t.after.saturating_add(1);
        let name = format!("t{:02}", to);
        let from_dir = frames_dir.join(format!("b{:02}", from.saturating_add(1)));
        let to_dir = frames_dir.join(format!("b{:02}", to.saturating_add(1)));
        let mut scenes = Vec::new();
        let mut times = Vec::new();
        for f in t.frames.clone() {
            let (Some(a), Some(b)) = (timeline.block_frame(from, f), timeline.block_frame(to, f))
            else {
                anyhow::bail!("{name}: frame {f} missing from the neighbouring renders");
            };
            scenes.push((from_dir.join(frame_name(a)), to_dir.join(frame_name(b))));
            times.push(timeline.local_time(f, t.frames.start)?);
        }
        let extra_key = keys
            .iter()
            .filter(|(i, _)| *i == from || *i == to)
            .map(|(_, k)| k.as_str())
            .collect::<Vec<_>>()
            .join("+");
        let html = comps.join(format!("{name}.html"));
        let out = frames_dir.join(&name);
        let job = FrameJob {
            html: &html,
            width: ctx.width,
            height: ctx.height,
            times,
            scenes: Some(scenes),
            extra_key,
            out_dir: &out,
        };
        let outcome = job.run(&*ctx.renderers.acquire().await?).await?;
        tracing::info!(
            "{name} : {} images{}",
            outcome.frames,
            if outcome.cached { " (cache)" } else { "" }
        );
        for e in &outcome.errors {
            tracing::warn!("{name} : {e}");
        }
    }
    Ok(())
}
