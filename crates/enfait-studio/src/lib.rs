//! Animation studio: the pipeline shared by the CLI and the interface.
//!
//! - [`direct`]: "director" pass (art direction, intent of each block),
//!   cached;
//! - [`animate`]: one composition per block (creation, technical fixes,
//!   critiques on rendered frames);
//! - [`revise`]: the user's retouches anchored in time;
//! - [`render_clip`]: rendering a composition into a video clip.
//!
//! Everything is written in the working folder of a [`Studio`]: `comps/`
//! (compositions ready to render), `sources/` (raw compositions),
//! `checks/` (check frames), `logs/`, `frames/`.

pub mod broll;
pub mod export;
mod ffmpeg;
pub mod magic;
pub mod mix;
pub mod premiere;
pub mod preview;
pub mod psd;
pub mod session;
pub mod thumbnail;
pub mod userlib;

use anyhow::{Context as _, Result};
use enfait_ai::animate::{
    AnimatorConfig, SceneBrief, TimedWord, TransitionBrief, Verdict, parse_verdict,
};
use enfait_ai::direction::{
    Direction, DirectorBlock, DirectorInput, director_request, parse_direction,
};
use enfait_ai::{Effort, Part, Provider, Usage};
use enfait_core::{TimeRange, Transcript};
use enfait_motion::{CheckJob, FrameJob, RendererPool, Report, composition, library};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Duration of a crafted transition, in seconds.
pub const TRANSITION_LENGTH: f64 = 1.0;

/// Frame size of a media, if it has pictures (ffprobe).
pub fn probe_video(path: &Path) -> Option<(u32, u32)> {
    let video = enfait_media::probe::probe(path).ok()?.video?;
    Some((video.width, video.height))
}
/// Technical fixes tried before giving up on a version.
const MAX_FIXES: usize = 2;

pub struct Studio<'a> {
    pub work: PathBuf,
    pub provider: &'a Provider,
    pub renderers: &'a RendererPool,
    pub animator: AnimatorConfig,
    pub critique_rounds: u8,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// Tokens used, shared by all the blocks (and all the cases of an evaluation).
    pub usage: &'a Mutex<Usage>,
    /// Frames of the take for a mixed or effect scene: folder (`000000.jpg`,
    /// …, one per frame at `fps`) and number of frames.
    pub scene_frames: Option<(PathBuf, usize)>,
    /// The cut-out person on those same frames (`000000.png`, …): passed as the
    /// second layer (`to`) of a mixed scene with cutout.
    pub scene_cutouts: Option<PathBuf>,
}

impl Studio<'_> {
    /// Frame of the take at time `t` of the scene (the last one beyond).
    fn scene_at(&self, t: f64) -> Option<(PathBuf, PathBuf)> {
        let (dir, count) = self.scene_frames.as_ref()?;
        // Positive time bounded by the scene.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let index = ((t.max(0.0) * self.fps).floor() as usize).min(count.saturating_sub(1));
        let path = dir.join(enfait_motion::frame_name(index));
        let second = self
            .scene_cutouts
            .as_ref()
            .map_or_else(|| path.clone(), |cut| cut.join(cutout_name(index)));
        Some((path, second))
    }

    /// Frames of the take for a sequence of times.
    fn scenes_for(&self, times: &[f64]) -> Option<Vec<(PathBuf, PathBuf)>> {
        self.scene_frames.as_ref()?;
        times.iter().map(|t| self.scene_at(*t)).collect()
    }

    pub async fn ask(&self, request: &enfait_ai::Request) -> Result<String> {
        let reply = self.provider.complete(request).await?;
        if let Ok(mut usage) = self.usage.lock() {
            usage.add(&reply.usage);
        }
        Ok(reply.text)
    }

    pub fn comps(&self) -> PathBuf {
        self.work.join("comps")
    }
}

/// Name of the cut-out frame `index` of a take.
pub fn cutout_name(index: usize) -> String {
    format!("{index:06}.png")
}

/// Summary of the tokens used.
pub fn log_usage(usage: &Mutex<Usage>) {
    let usage = usage.lock().map(|u| *u).unwrap_or_default();
    tracing::info!(
        "tokens: {} in ({} read from cache), {} out{}",
        usage
            .input_tokens
            .saturating_add(usage.cache_read_input_tokens),
        usage.cache_read_input_tokens,
        usage.output_tokens,
        if usage.cost_usd > 0.0 {
            format!(", reported cost ${:.2}", usage.cost_usd)
        } else {
            String::new()
        }
    );
}

/// Available fonts, one line per family.
pub fn fonts_list() -> String {
    let mut out = String::new();
    for family in library::font_families() {
        let faces: Vec<_> = library::FONTS
            .iter()
            .filter(|f| f.family == family)
            .collect();
        let weights: Vec<&str> = faces
            .iter()
            .filter(|f| !f.italic)
            .map(|f| f.weight)
            .collect();
        let italic = faces.iter().any(|f| f.italic);
        let stretch = faces.iter().find_map(|f| f.stretch);
        let _ = write!(out, "- \"{family}\" : graisses {}", weights.join(", "));
        if italic {
            out.push_str(", italique");
        }
        if let Some(s) = stretch {
            let _ = write!(out, ", chasse {s}");
        }
        out.push('\n');
    }
    out
}

#[allow(clippy::too_many_arguments)]
pub async fn direct(
    provider: &Provider,
    work: &Path,
    ranges: &[TimeRange],
    texts: &[String],
    width: u32,
    height: u32,
    script: Option<&str>,
    user_style: Option<&str>,
    fonts: &str,
    max_transitions: usize,
) -> Result<Direction> {
    let blocks: Vec<DirectorBlock> = ranges
        .iter()
        .zip(texts)
        .map(|(r, t)| DirectorBlock {
            start: r.start().as_secs_f64(),
            end: r.end().as_secs_f64(),
            text: t.clone(),
        })
        .collect();
    let input = DirectorInput {
        width,
        height,
        blocks: &blocks,
        script,
        user_style,
        fonts,
        max_transitions,
    };
    let request = director_request(&input, "claude-opus-5-5", Effort::High);
    // The system instruction is part of the signature: changing the
    // director's prompt invalidates the cached direction.
    let signature = std::iter::once(format!(
        "<!-- consigne {} -->\n",
        blake3::hash(request.system.as_bytes()).to_hex()
    ))
    .chain(request.user.iter().filter_map(|p| match p {
        Part::Text(t) => Some(t.clone()),
        Part::Image(_) => None,
    }))
    .collect::<String>();
    let cached = work.join("direction.json");
    let cached_input = work.join("direction.input.md");
    if let (Ok(old), Ok(data)) = (
        tokio::fs::read_to_string(&cached_input).await,
        tokio::fs::read(&cached).await,
    ) && old == signature
    {
        tracing::info!("direction reprise de {}", cached.display());
        return Ok(serde_json::from_slice(&data)?);
    }
    tracing::info!("director pass…");
    let mut last_error = None;
    for _ in 0..2 {
        let reply = provider.complete(&request).await?;
        // The discarded concepts and the references, useful to understand the choice.
        tokio::fs::write(work.join("direction.notes.md"), &reply.text).await?;
        match parse_direction(&reply.text, blocks.len()) {
            Ok(direction) => {
                tokio::fs::write(&cached, serde_json::to_vec_pretty(&direction)?).await?;
                tokio::fs::write(&cached_input, &signature).await?;
                tracing::info!("concept : {}", direction.concept);
                return Ok(direction);
            }
            Err(e) => {
                tracing::warn!("unreadable director answer: {e}");
                last_error = Some(e);
            }
        }
    }
    Err(last_error.map_or_else(|| anyhow::anyhow!("no direction"), Into::into))
}

/// Sampling times for the check and the critique.
pub fn sample_times(duration: f64, fps: f64) -> Vec<f64> {
    let last = (duration - 1.0 / fps).max(0.0);
    let mut times = vec![0.1_f64.min(last)];
    for k in 1..=4 {
        times.push(duration * f64::from(k) / 5.0);
    }
    times.push(last);
    times
}

struct Checked {
    html: String,
    report: Report,
}

impl Studio<'_> {
    /// Writes `html` (prepared) to `comps/<name>.html` and checks it.
    async fn check(&self, name: &str, round: &str, html: &str, duration: f64) -> Result<Report> {
        let path = self.comps().join(format!("{name}.html"));
        tokio::fs::write(&path, composition::prepare(html)).await?;
        let job = CheckJob {
            html: &path,
            width: self.width,
            height: self.height,
            expected_duration: duration,
            scenes: self.scenes_for(&sample_times(duration, self.fps)),
            sample_times: sample_times(duration, self.fps),
            sample_width: self.width / 2,
            out_dir: &self.work.join("checks").join(name).join(round),
        };
        Ok(job.run(&*self.renderers.acquire().await?).await?)
    }

    /// Asks for fixes as long as the check blocks. `strict`: also judge the
    /// picture (duration, empty frame), not only the technique.
    #[allow(clippy::too_many_arguments)]
    async fn fix_until_clean(
        &self,
        strict: bool,
        name: &str,
        round: &str,
        brief: &str,
        mut html: String,
        mut report: Report,
        duration: f64,
        log: &mut String,
    ) -> Result<Checked> {
        for attempt in 1..=MAX_FIXES {
            let problems = if strict {
                report.blocking()
            } else {
                report.technical()
            };
            if problems.is_empty() {
                break;
            }
            let _ = writeln!(
                log,
                "\n### Correction {attempt}\n\n- {}",
                problems.join("\n- ")
            );
            tracing::info!("{name} : correction {attempt} ({})", problems.join(" ; "));
            let reply = self
                .ask(&self.animator.fix(brief, &html, &problems))
                .await?;
            let Some(fixed) = composition::extract_html(&reply) else {
                let _ = writeln!(log, "(answer without HTML)");
                continue;
            };
            html = fixed;
            report = self
                .check(name, &format!("{round}-fix{attempt}"), &html, duration)
                .await?;
        }
        Ok(Checked { html, report })
    }
}

/// Animates block `brief.index` (files `b01`, `b02`…).
pub async fn animate_block(ctx: &Studio<'_>, brief: &SceneBrief, redo: bool) -> Result<()> {
    animate(ctx, &format!("b{:02}", brief.index), brief, redo).await
}

/// Creates composition `name` (creation, technical fixes, critiques on
/// rendered frames), or takes `sources/<name>.html` again unless `redo`.
pub async fn animate(ctx: &Studio<'_>, name: &str, brief: &SceneBrief, redo: bool) -> Result<()> {
    let name = name.to_owned();
    let sources = ctx.work.join("sources");
    tokio::fs::create_dir_all(&sources).await?;
    let source = sources.join(format!("{name}.html"));
    if !redo && let Ok(html) = tokio::fs::read_to_string(&source).await {
        tokio::fs::write(
            ctx.comps().join(format!("{name}.html")),
            composition::prepare(&html),
        )
        .await?;
        tracing::info!("{name} : composition reprise");
        return Ok(());
    }
    let text = brief.render();
    let mut log = format!("# {name}\n\n{text}\n");
    tracing::info!("{name}: creating…");
    let reply = ctx.ask(&ctx.animator.create(&text)).await?;
    let html = composition::extract_html(&reply)
        .with_context(|| format!("{name}: the answer contains no HTML"))?;
    let report = ctx.check(&name, "r0", &html, brief.duration).await?;
    let mut best = ctx
        .fix_until_clean(
            true,
            &name,
            "r0",
            &text,
            html,
            report,
            brief.duration,
            &mut log,
        )
        .await?;

    for round in 1..=ctx.critique_rounds {
        if !best.report.blocking().is_empty() {
            break;
        }
        tracing::info!("{name} : critique {round}…");
        let samples: Vec<(f64, PathBuf)> = best
            .report
            .samples
            .iter()
            .map(|s| (s.t, s.path.clone()))
            .collect();
        // A failing critique (quota reached, network) does not cancel the
        // already valid version: it is kept.
        let reply = match ctx
            .ask(
                &ctx.animator
                    .critique(&text, &best.html, &samples, &best.report.warnings()),
            )
            .await
        {
            Ok(reply) => reply,
            Err(e) => {
                tracing::warn!("{name}: critique {round} failed ({e:#}), current version kept");
                let _ = writeln!(log, "\n(critique {round} failed: {e:#})");
                break;
            }
        };
        let notes: String = reply
            .split("```html")
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned();
        let _ = writeln!(log, "\n## Critique {round}\n\n{notes}");
        if parse_verdict(&reply) == Verdict::Keep {
            tracing::info!("{name}: kept at critique {round}");
            break;
        }
        let Some(candidate) = composition::extract_html(&reply) else {
            break;
        };
        let round_name = format!("r{round}");
        let report = ctx
            .check(&name, &round_name, &candidate, brief.duration)
            .await?;
        let checked = match ctx
            .fix_until_clean(
                true,
                &name,
                &round_name,
                &text,
                candidate,
                report,
                brief.duration,
                &mut log,
            )
            .await
        {
            Ok(checked) => checked,
            Err(e) => {
                tracing::warn!("{name}: fix failed ({e:#}), previous version kept");
                break;
            }
        };
        if checked.report.blocking().is_empty() {
            best = checked;
        } else {
            let _ = writeln!(log, "\n(new version dropped: technical errors)");
            break;
        }
    }

    let remaining = best.report.blocking();
    if !remaining.is_empty() {
        tracing::warn!("{name}: remaining problems: {}", remaining.join("; "));
    }
    tokio::fs::write(
        ctx.comps().join(format!("{name}.html")),
        composition::prepare(&best.html),
    )
    .await?;
    tokio::fs::write(&source, &best.html).await?;
    let logs = ctx.work.join("logs");
    tokio::fs::create_dir_all(&logs).await?;
    tokio::fs::write(logs.join(format!("{name}.md")), log).await?;
    tracing::info!("{name}: ready");
    Ok(())
}

pub async fn animate_transition(
    ctx: &Studio<'_>,
    after: usize,
    direction: &Direction,
    art: &str,
    redo: bool,
) -> Result<()> {
    let index = after.saturating_add(1);
    let name = format!("t{index:02}");
    let source = ctx.work.join("sources").join(format!("{name}.html"));
    if !redo && let Ok(html) = tokio::fs::read_to_string(&source).await {
        tokio::fs::write(
            ctx.comps().join(format!("{name}.html")),
            composition::prepare(&html),
        )
        .await?;
        tracing::info!("{name} : transition reprise");
        return Ok(());
    }
    let brief = TransitionBrief {
        width: ctx.width,
        height: ctx.height,
        duration: TRANSITION_LENGTH,
        art_direction: art.to_owned(),
        description: direction
            .transition_after(index)
            .unwrap_or_default()
            .to_owned(),
        from_intent: direction.intent(index).to_owned(),
        to_intent: direction.intent(index.saturating_add(1)).to_owned(),
    };
    tracing::info!("{name} : transition…");
    let reply = ctx.ask(&ctx.animator.transition(&brief)).await?;
    let html = composition::extract_html(&reply)
        .with_context(|| format!("{name}: the answer contains no HTML"))?;
    // The real duration depends on rounding to frames: check the contract
    // and the errors, not the exact duration.
    let report = ctx.check(&name, "r0", &html, TRANSITION_LENGTH).await?;
    let mut log = String::new();
    let checked = ctx
        .fix_until_clean(
            false,
            &name,
            "r0",
            &brief.render(),
            html,
            report,
            TRANSITION_LENGTH,
            &mut log,
        )
        .await?;
    tokio::fs::write(
        ctx.comps().join(format!("{name}.html")),
        composition::prepare(&checked.html),
    )
    .await?;
    tokio::fs::write(&source, &checked.html).await?;
    tracing::info!("{name}: ready");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
/// Brief of an animator for block `i`: text, dated words, direction,
/// neighbours and transitions.
pub fn brief(
    i: usize,
    ranges: &[TimeRange],
    texts: &[String],
    transcript: &Transcript,
    direction: &Direction,
    art: &str,
    width: u32,
    height: u32,
    user_style: Option<&str>,
) -> SceneBrief {
    let index = i.saturating_add(1);
    let range = ranges.get(i).copied();
    let start = range.map_or(0.0, |r| r.start().as_secs_f64());
    let neighbour = |j: usize| -> Option<String> {
        let text = texts.get(j)?;
        Some(format!(
            "Voice: “{text}”\nIntent: {}",
            direction.intent(j.saturating_add(1))
        ))
    };
    let mut previous = i.checked_sub(1).and_then(neighbour);
    let mut next = neighbour(i.saturating_add(1));
    if let (Some(p), Some(t)) = (
        previous.as_mut(),
        i.checked_sub(1).and_then(|_| direction.transition_after(i)),
    ) {
        let _ = write!(
            p,
            "\nTransition into your scene: {t} (handled separately, start your scene normally)."
        );
    }
    if let (Some(n), Some(t)) = (next.as_mut(), direction.transition_after(index)) {
        let _ = write!(
            n,
            "\nTransition out of your scene: {t} (handled separately, end your scene settled)."
        );
    }
    SceneBrief {
        index,
        total: ranges.len(),
        width,
        height,
        duration: range.map_or(0.0, |r| r.duration().as_secs_f64()),
        text: texts.get(i).cloned().unwrap_or_default(),
        words: range
            .map(|r| {
                transcript
                    .words_in(r)
                    .map(|w| TimedWord {
                        t: w.range.start().as_secs_f64() - start,
                        text: w.text.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        art_direction: art.to_owned(),
        intent: direction.intent(index).to_owned(),
        previous,
        next,
        user_style: user_style.map(str::to_owned),
        library: None,
        moments: Vec::new(),
        layer: None,
    }
}

/// Retouches composition `name` following the user's remarks (time in s
/// from the start of the scene, remark), then checks and fixes it if
/// needed. The previous version stays in `sources/` as
/// `<name>.<n>.html`.
pub async fn revise(
    ctx: &Studio<'_>,
    name: &str,
    brief: &SceneBrief,
    notes: &[(f64, String)],
) -> Result<()> {
    let sources = ctx.work.join("sources");
    let source = sources.join(format!("{name}.html"));
    let html = tokio::fs::read_to_string(&source)
        .await
        .with_context(|| format!("{name}: no composition to retouch"))?;
    // Keep the previous version.
    let n = archive(&ctx.work, name).await?;

    let text = brief.render();
    tracing::info!("{name} : retouche…");
    let reply = ctx.ask(&ctx.animator.revise(&text, &html, notes)).await?;
    let revised = composition::extract_html(&reply)
        .with_context(|| format!("{name}: the retouch contains no HTML"))?;
    let round = format!("v{n}");
    let report = ctx.check(name, &round, &revised, brief.duration).await?;
    let mut log = String::new();
    let checked = ctx
        .fix_until_clean(
            true,
            name,
            &round,
            &text,
            revised,
            report,
            brief.duration,
            &mut log,
        )
        .await?;
    let remaining = checked.report.blocking();
    if !remaining.is_empty() {
        tracing::warn!("{name}: remaining problems: {}", remaining.join("; "));
    }
    tokio::fs::write(
        ctx.comps().join(format!("{name}.html")),
        composition::prepare(&checked.html),
    )
    .await?;
    tokio::fs::write(&source, &checked.html).await?;
    let logs = ctx.work.join("logs");
    tokio::fs::create_dir_all(&logs).await?;
    let mut entry = format!("\n## Retouch {n}\n\n");
    for (t, note) in notes {
        let _ = writeln!(entry, "- at {t:.2} s: {note}");
    }
    entry.push_str(&log);
    let path = logs.join(format!("{name}.md"));
    let old = tokio::fs::read_to_string(&path).await.unwrap_or_default();
    tokio::fs::write(&path, old + &entry).await?;
    tracing::info!("{name}: retouch ready");
    Ok(())
}

/// Renders `comps/<name>.html` over `duration` seconds into a silent H.264
/// clip (`out`), at the frame rate and size of the studio.
pub async fn render_clip(ctx: &Studio<'_>, name: &str, duration: f64, out: &Path) -> Result<()> {
    // Number of frames of the scene: positive and bounded.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let count = (duration * ctx.fps).round().max(1.0) as usize;
    #[allow(clippy::cast_precision_loss)]
    let times: Vec<f64> = (0..count).map(|i| i as f64 / ctx.fps).collect();
    let frames = ctx.work.join("frames").join(name);
    let job = FrameJob {
        html: &ctx.comps().join(format!("{name}.html")),
        width: ctx.width,
        height: ctx.height,
        scenes: ctx.scenes_for(&times),
        times,
        // The frames of the take are part of the render.
        extra_key: ctx
            .scene_frames
            .as_ref()
            .map(|(dir, n)| {
                let cut = ctx
                    .scene_cutouts
                    .as_ref()
                    .map(|c| c.display().to_string())
                    .unwrap_or_default();
                format!("{}:{n}:{cut}", dir.display())
            })
            .unwrap_or_default(),
        out_dir: &frames,
    };
    let outcome = job.run(&*ctx.renderers.acquire().await?).await?;
    for e in &outcome.errors {
        tracing::warn!("{name} : {e}");
    }
    encode_frames(&frames, ctx.fps, count, out).await
}

/// Extracts the frames of the take for a scene: `pieces` (start, end in
/// seconds in the take) end to end, at `fps`, framed in `size` (whole
/// picture), into `out` (`000000.jpg`…). Returns the number of frames.
pub async fn extract_frames(
    rush: &Path,
    pieces: &[(f64, f64)],
    fps: f64,
    size: (u32, u32),
    out: &Path,
) -> Result<usize> {
    let fit = enfait_media::fx::framing(size.0, size.1, enfait_core::Framing::Fit, 0);
    extract_frames_framed(rush, pieces, fps, &fit, out).await
}

/// Like [`extract_frames`], with the framing `framing` (see
/// [`enfait_media::fx::framing`]).
pub async fn extract_frames_framed(
    rush: &Path,
    pieces: &[(f64, f64)],
    fps: f64,
    framing: &str,
    out: &Path,
) -> Result<usize> {
    if tokio::fs::try_exists(out).await.unwrap_or(false) {
        tokio::fs::remove_dir_all(out).await?;
    }
    tokio::fs::create_dir_all(out).await?;
    let mut count = 0_usize;
    for (i, (a, b)) in pieces.iter().enumerate() {
        let tmp = out.join(format!("part{i}"));
        tokio::fs::create_dir_all(&tmp).await?;
        let output = enfait_config::process::tokio_command("ffmpeg")
            .args(["-nostdin", "-v", "error", "-y"])
            .args([
                "-ss",
                &format!("{a:.4}"),
                "-t",
                &format!("{:.4}", (b - a).max(0.0)),
            ])
            .arg("-i")
            .arg(rush)
            .args([
                "-vf",
                &format!("fps={fps},{framing}"),
                "-q:v",
                "3",
                "-start_number",
                "0",
            ])
            .arg(tmp.join("%06d.jpg"))
            .output()
            .await
            .context("starting ffmpeg")?;
        anyhow::ensure!(
            output.status.success(),
            "frames of the take: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        let mut k = 0_usize;
        loop {
            let from = tmp.join(enfait_motion::frame_name(k));
            if !tokio::fs::try_exists(&from).await.unwrap_or(false) {
                break;
            }
            tokio::fs::rename(&from, out.join(enfait_motion::frame_name(count))).await?;
            count = count.saturating_add(1);
            k = k.saturating_add(1);
        }
        let _ = tokio::fs::remove_dir_all(&tmp).await;
    }
    anyhow::ensure!(count > 0, "no frame extracted from the take");
    Ok(count)
}

/// Previous versions of `name` kept by [`revise`] and [`restore`]
/// (`sources/<name>.<n>.html`), in order.
pub fn versions(work: &Path, name: &str) -> Vec<u32> {
    let sources = work.join("sources");
    let mut found: Vec<u32> = std::fs::read_dir(&sources)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let file = e.file_name().to_string_lossy().into_owned();
                    let rest = file.strip_prefix(&format!("{name}."))?;
                    rest.strip_suffix(".html")?.parse().ok()
                })
                .collect()
        })
        .unwrap_or_default();
    found.sort_unstable();
    found
}

/// Keeps the current composition as a new version; returns its number.
async fn archive(work: &Path, name: &str) -> Result<u32> {
    let sources = work.join("sources");
    let n = versions(work, name)
        .last()
        .map_or(1, |v| v.saturating_add(1));
    tokio::fs::copy(
        sources.join(format!("{name}.html")),
        sources.join(format!("{name}.{n}.html")),
    )
    .await?;
    Ok(n)
}

/// Goes back to version `version` of `name` (the current version is kept
/// in turn). No model call.
pub async fn restore(ctx: &Studio<'_>, name: &str, version: u32) -> Result<()> {
    let sources = ctx.work.join("sources");
    let html = tokio::fs::read_to_string(sources.join(format!("{name}.{version}.html")))
        .await
        .with_context(|| format!("{name}: version {version} not found"))?;
    archive(&ctx.work, name).await?;
    tokio::fs::write(sources.join(format!("{name}.html")), &html).await?;
    tokio::fs::write(
        ctx.comps().join(format!("{name}.html")),
        composition::prepare(&html),
    )
    .await?;
    Ok(())
}

/// Creates (or takes again unless `redo`) transition `name` and renders it
/// into a clip: `scenes` gives, for each frame, the two connected shots.
pub async fn transition_clip(
    ctx: &Studio<'_>,
    name: &str,
    brief: &TransitionBrief,
    scenes: &[(PathBuf, PathBuf)],
    redo: bool,
    out: &Path,
) -> Result<()> {
    let sources = ctx.work.join("sources");
    tokio::fs::create_dir_all(&sources).await?;
    let source = sources.join(format!("{name}.html"));
    let html = match tokio::fs::read_to_string(&source).await {
        Ok(html) if !redo => html,
        _ => {
            let reply = ctx.ask(&ctx.animator.transition(brief)).await?;
            let html = composition::extract_html(&reply)
                .with_context(|| format!("{name}: the answer contains no HTML"))?;
            let path = ctx.comps().join(format!("{name}.html"));
            tokio::fs::write(&path, composition::prepare(&html)).await?;
            let times = sample_times(brief.duration, ctx.fps);
            let pick = |t: f64| {
                // Positive time bounded by the transition.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let i = ((t * ctx.fps).floor() as usize).min(scenes.len().saturating_sub(1));
                scenes.get(i).cloned()
            };
            let job = CheckJob {
                html: &path,
                width: ctx.width,
                height: ctx.height,
                expected_duration: brief.duration,
                scenes: times.iter().map(|t| pick(*t)).collect(),
                sample_times: times,
                sample_width: ctx.width / 2,
                out_dir: &ctx.work.join("checks").join(name).join("r0"),
            };
            let report = job.run(&*ctx.renderers.acquire().await?).await?;
            let mut log = String::new();
            let checked = ctx
                .fix_until_clean(
                    false,
                    name,
                    "r0",
                    &brief.render(),
                    html,
                    report,
                    brief.duration,
                    &mut log,
                )
                .await?;
            tokio::fs::write(&source, &checked.html).await?;
            checked.html
        }
    };
    let comp = ctx.comps().join(format!("{name}.html"));
    tokio::fs::write(&comp, composition::prepare(&html)).await?;
    #[allow(clippy::cast_precision_loss)]
    let times: Vec<f64> = (0..scenes.len()).map(|i| i as f64 / ctx.fps).collect();
    let frames = ctx.work.join("frames").join(name);
    let key: String = scenes
        .iter()
        .flat_map(|(a, b)| [a.display().to_string(), b.display().to_string()])
        .collect();
    let job = FrameJob {
        html: &comp,
        width: ctx.width,
        height: ctx.height,
        times,
        scenes: Some(scenes.to_vec()),
        extra_key: blake3::hash(key.as_bytes()).to_hex().to_string(),
        out_dir: &frames,
    };
    let outcome = job.run(&*ctx.renderers.acquire().await?).await?;
    for e in &outcome.errors {
        tracing::warn!("{name} : {e}");
    }
    encode_frames(&frames, ctx.fps, scenes.len(), out).await
}

/// Encodes `count` JPEG frames of `frames` into a silent H.264 clip.
async fn encode_frames(frames: &Path, fps: f64, count: usize, out: &Path) -> Result<()> {
    if let Some(dir) = out.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let output = enfait_config::process::tokio_command("ffmpeg")
        .args(["-nostdin", "-v", "error", "-y"])
        .args(["-framerate", &format!("{fps}")])
        .arg("-i")
        .arg(frames.join("%06d.jpg"))
        .args(["-frames:v", &count.to_string()])
        .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "18"])
        .args(["-pix_fmt", "yuv420p", "-g", "15", "-movflags", "+faststart"])
        .arg(out)
        .output()
        .await
        .context("starting ffmpeg")?;
    anyhow::ensure!(
        output.status.success(),
        "ffmpeg : {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}
