//! Export of the final video: the edited take, the animations, b-roll
//! shots and transitions in full definition, the post-FX, then the mix of
//! the finish (with or without SFX) normalised in loudness.
//!
//! Four ffmpeg passes: the picture (the longest), the voice of the edit,
//! the mix as WAV, then the loudness measurement and the final assembly
//! (the sound is normalised in two passes, without compressing dynamics).

use crate::preview::{Output, Quality, assemble, pieces};
use crate::session::{Sources, Visuals};
use anyhow::{Context as _, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Target loudness (integrated LUFS): that of YouTube, Instagram and TikTok.
pub const TARGET_LUFS: f64 = -14.0;
/// Maximum true peak (dBTP).
pub const TRUE_PEAK: f64 = -1.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExportOptions {
    /// Keep the sound effects.
    pub sfx: bool,
    /// Target loudness; `None`: sound left as mixed.
    pub loudness: Option<f64>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            sfx: true,
            loudness: Some(TARGET_LUFS),
        }
    }
}

/// What the export produced.
#[derive(Debug, Clone)]
pub struct ExportReport {
    pub path: PathBuf,
    pub duration: f64,
    /// Blocks (rank from 1) that do not have their animation or shot yet: the
    /// take is shown instead.
    pub missing: Vec<usize>,
    /// Loudness of the mix before normalisation (LUFS), if measured.
    pub measured_lufs: Option<f64>,
}

/// Measurements of the first `loudnorm` pass.
#[derive(Debug, Clone, Deserialize)]
struct Measure {
    input_i: String,
    input_tp: String,
    input_lra: String,
    input_thresh: String,
    target_offset: String,
}

/// Working folder of the export, next to the final file.
fn work_dir(out: &Path) -> PathBuf {
    let mut name = out
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".enfait-export");
    out.with_file_name(name)
}

/// Exports the video to `out` (MP4, H.264 + AAC). `progress`: progress
/// from 0 to 1.
pub async fn video(
    sources: &Sources<'_>,
    options: ExportOptions,
    out: &Path,
    progress: &(dyn Fn(f64) + Sync),
) -> Result<ExportReport> {
    let project = sources.project;
    let settings = project.settings();
    let (w, h) = (settings.resolution.width, settings.resolution.height);
    let rate = settings.frame_rate;
    let fps = f64::from(rate.num()) / f64::from(rate.den());
    let info = enfait_media::probe::probe(sources.rush)
        .with_context(|| format!("unreadable take: {}", sources.rush.display()))?;
    anyhow::ensure!(info.audio.is_some(), "the take has no sound");
    let visuals = Visuals::of(sources.studio, project);
    let work = work_dir(out);
    tokio::fs::create_dir_all(&work).await?;
    let result = async {
        // 1. The picture: 90 % of the time.
        let image = work.join("image.mp4");
        let step = |p: f64| progress(0.9 * p);
        assemble(
            sources.rush,
            info.video.is_some(),
            sources.parts,
            &pieces(&visuals.blocks, &visuals.transitions),
            &Output {
                size: (w, h),
                fps,
                framing: enfait_media::fx::framing(w, h, settings.framing, settings.pan()),
                quality: Quality::Final,
                post_fx: enfait_media::fx::filter(&sources.look.fx, h),
                progress: Some(&step),
            },
            &image,
        )
        .await?;
        // 2. The voice of the edit, then the mix.
        let voice = work.join("voice.wav");
        ffmpeg(
            enfait_config::process::tokio_command("ffmpeg")
                .args(["-nostdin", "-v", "error", "-y", "-i"])
                .arg(sources.rush)
                .args([
                    "-filter_complex",
                    &crate::preview::voice_graph(sources.parts),
                ])
                .args(["-map", "[a]", "-c:a", "pcm_f32le"])
                .arg(&voice),
            "voice",
        )
        .await?;
        progress(0.93);
        let mix = work.join("mix.wav");
        crate::mix::render(&voice, &crate::mix::plan(sources, options.sfx), &mix).await?;
        progress(0.96);
        // 3. Loudness, then assembly.
        let measure = match options.loudness {
            Some(_) => Some(measure(&mix).await?),
            None => None,
        };
        let measured = measure
            .as_ref()
            .and_then(|m| m.input_i.parse::<f64>().ok())
            .filter(|i| i.is_finite());
        let total: f64 = enfait_media::Edl::from_ranges(sources.parts)
            .parts()
            .iter()
            .map(|p| p.duration())
            .sum();
        let mut cmd = enfait_config::process::tokio_command("ffmpeg");
        cmd.args(["-nostdin", "-v", "error", "-y", "-i"])
            .arg(&image)
            .arg("-i")
            .arg(&mix)
            .args(["-map", "0:v:0", "-map", "1:a:0", "-c:v", "copy"]);
        // An almost silent mix (under −70 LUFS) is not raised.
        if let (Some(target), Some(m), Some(i)) = (options.loudness, &measure, measured)
            && i > -70.0
        {
            cmd.args([
                "-af",
                &format!(
                    "loudnorm=I={target}:TP={TRUE_PEAK}:LRA=11:measured_I={}:measured_TP={}:\
                     measured_LRA={}:measured_thresh={}:offset={}:linear=true",
                    m.input_i, m.input_tp, m.input_lra, m.input_thresh, m.target_offset
                ),
            ]);
        }
        cmd.args(["-ar", "48000", "-c:a", "aac", "-b:a", "320k"])
            .args(["-t", &format!("{total:.3}"), "-movflags", "+faststart"]);
        let tmp = out.with_extension("tmp.mp4");
        cmd.arg(&tmp);
        ffmpeg(&mut cmd, "assemblage").await?;
        tokio::fs::rename(&tmp, out).await?;
        progress(1.0);
        Ok(ExportReport {
            path: out.to_path_buf(),
            duration: total,
            missing: visuals.missing(project),
            measured_lufs: measured,
        })
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&work).await;
    result
}

async fn ffmpeg(cmd: &mut tokio::process::Command, what: &str) -> Result<()> {
    let output = cmd.output().await.context("starting ffmpeg")?;
    anyhow::ensure!(
        output.status.success(),
        "{what} : {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

/// First `loudnorm` pass: measurements of the mix.
async fn measure(audio: &Path) -> Result<Measure> {
    let output = enfait_config::process::tokio_command("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-nostats", "-i"])
        .arg(audio)
        .args([
            "-af",
            &format!("loudnorm=I={TARGET_LUFS}:TP={TRUE_PEAK}:LRA=11:print_format=json"),
            "-f",
            "null",
            "-",
        ])
        .output()
        .await
        .context("starting ffmpeg")?;
    let text = String::from_utf8_lossy(&output.stderr);
    anyhow::ensure!(output.status.success(), "loudness : {}", text.trim());
    parse_measure(&text).context("loudness: unreadable measurement")
}

/// The JSON block `loudnorm` writes at the end of its messages.
fn parse_measure(text: &str) -> Option<Measure> {
    let start = text.rfind('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(text.get(start..=end)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loudnorm_output_is_read() {
        let text = "[Parsed_loudnorm_0 @ 0x5]\n{\n\t\"input_i\" : \"-23.54\",\n\t\"input_tp\" : \
                    \"-5.21\",\n\t\"input_lra\" : \"3.10\",\n\t\"input_thresh\" : \"-33.9\",\n\t\
                    \"output_i\" : \"-14.0\",\n\t\"target_offset\" : \"0.12\"\n}\n";
        let m = parse_measure(text).unwrap();
        assert_eq!(m.input_i, "-23.54");
        assert_eq!(m.target_offset, "0.12");
        assert!(parse_measure("no measurement").is_none());
    }

    #[test]
    fn work_dir_sits_next_to_the_output() {
        assert_eq!(
            work_dir(Path::new("/v/film.mp4")),
            PathBuf::from("/v/film.mp4.enfait-export")
        );
    }
}
