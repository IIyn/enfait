//! Assembling the frames and the voice into an MP4 (ffmpeg as a subprocess).

use crate::timeline::{Source, Timeline};
use anyhow::{Context as _, Result};
use enfait_motion::frame_name;
use std::path::Path;
use std::process::Stdio;
use tokio::io::AsyncWriteExt as _;

/// Encodes to H.264 (x264), with the voice if given and the `filter`
/// filters (post-FX). NVENC was tried: not faster here (decoding the PNGs
/// dominates) and files twice as heavy.
pub async fn encode(
    work: &Path,
    timeline: &Timeline,
    voice: Option<&Path>,
    duration: f64,
    filter: Option<&str>,
    out: &Path,
) -> Result<()> {
    let frames = work.join("frames");
    let rate = format!("{}/{}", timeline.rate.num(), timeline.rate.den());
    tracing::info!("encodage de {} images…", timeline.total.end);
    let mut child = enfait_config::process::tokio_command("ffmpeg")
        .args(["-y", "-nostdin", "-v", "error"])
        .args([
            "-f",
            "image2pipe",
            "-framerate",
            &rate,
            "-c:v",
            "mjpeg",
            "-i",
            "-",
        ])
        .args(voice.map_or_else(Vec::new, |v| vec!["-i".into(), v.as_os_str().to_owned()]))
        .args(["-t", &format!("{duration:.3}")])
        .args(["-map", "0:v:0"])
        .args(if voice.is_some() {
            &["-map", "1:a:0"][..]
        } else {
            &[]
        })
        .args(filter.map_or_else(Vec::new, |f| vec!["-vf", f]))
        .args(["-c:v", "libx264", "-preset", "slow", "-crf", "16"])
        .args(["-pix_fmt", "yuv420p"])
        .args(if voice.is_some() {
            &["-c:a", "aac", "-b:a", "192k"][..]
        } else {
            &[]
        })
        .args(["-movflags", "+faststart"])
        .arg(out)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("starting ffmpeg")?;
    let mut stdin = child.stdin.take().context("ffmpeg input")?;
    for index in timeline.total.clone() {
        let path = match timeline.source(index)? {
            Source::Block(b, j) => frames
                .join(format!("b{:02}", b.saturating_add(1)))
                .join(frame_name(j)),
            Source::Transition(t, j) => {
                let after = timeline.transitions.get(t).map_or(0, |t| t.after);
                frames
                    .join(format!("t{:02}", after.saturating_add(1)))
                    .join(frame_name(j))
            }
        };
        let png = tokio::fs::read(&path)
            .await
            .with_context(|| format!("image manquante : {}", path.display()))?;
        if stdin.write_all(&png).await.is_err() {
            break; // ffmpeg stopped: its error message follows.
        }
    }
    drop(stdin);
    let output = child.wait_with_output().await?;
    if !output.status.success() {
        anyhow::bail!(
            "ffmpeg : {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}
