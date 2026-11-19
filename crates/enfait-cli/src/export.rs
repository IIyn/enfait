//! `enfait-cli export`: the final video (MP4) or the edit for Premiere Pro
//! (FCP7 XML) from the app's session.

use anyhow::Result;
use enfait_studio::export::{ExportOptions, TARGET_LUFS};
use enfait_studio::premiere::PremiereOptions;
use enfait_studio::session::SessionFiles;
use std::path::PathBuf;

#[derive(Debug, clap::Args)]
pub struct ExportArgs {
    /// Video to write (MP4).
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Premiere edit to write (FCP7 XML); the media are copied next to it.
    #[arg(long)]
    pub premiere: Option<PathBuf>,
    /// Without the sound effects.
    #[arg(long)]
    pub no_sfx: bool,
    /// Target loudness, in LUFS.
    #[arg(long, default_value_t = TARGET_LUFS, allow_negative_numbers = true)]
    pub lufs: f64,
    /// Sound left as mixed (no normalisation).
    #[arg(long)]
    pub no_normalize: bool,
    /// Session folder (by default the app's, or `ENFAIT_SESSION`).
    #[arg(long)]
    pub session: Option<PathBuf>,
}

fn missing_note(missing: &[usize]) {
    if !missing.is_empty() {
        let list: Vec<String> = missing.iter().map(ToString::to_string).collect();
        tracing::warn!(
            "blocks with neither animation nor shot (the take covers them): {}",
            list.join(", ")
        );
    }
}

pub async fn run(args: ExportArgs) -> Result<()> {
    anyhow::ensure!(
        args.output.is_some() || args.premiere.is_some(),
        "nothing to export: --output <video.mp4> and/or --premiere <edit.xml>"
    );
    let dir = args
        .session
        .clone()
        .unwrap_or_else(enfait_mcp::bridge::session_dir);
    let session = SessionFiles::load(&dir)?;
    let sources = session.sources();
    let settings = session.project.settings();
    tracing::info!(
        "{}: {}×{}, {:.1} s, {} blocks",
        settings.aspect.label(),
        settings.resolution.width,
        settings.resolution.height,
        session.project.duration().as_secs_f64(),
        session.project.blocks().len()
    );
    if let Some(xml) = &args.premiere {
        let report =
            enfait_studio::premiere::export(&sources, PremiereOptions { sfx: !args.no_sfx }, xml)?;
        tracing::info!(
            "Premiere edit: {} (media: {})",
            report.xml.display(),
            report.media.display()
        );
        missing_note(&report.missing);
    }
    if let Some(out) = &args.output {
        let last = std::sync::atomic::AtomicU64::new(u64::MAX);
        let progress = |p: f64| {
            // Every ten percent.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let step = (p * 10.0).floor() as u64;
            if last.swap(step, std::sync::atomic::Ordering::Relaxed) != step {
                tracing::info!("export : {} %", step.saturating_mul(10));
            }
        };
        let report = enfait_studio::export::video(
            &sources,
            ExportOptions {
                sfx: !args.no_sfx,
                loudness: (!args.no_normalize).then_some(args.lufs),
            },
            out,
            &progress,
        )
        .await?;
        tracing::info!(
            "video: {} ({:.1} s{})",
            report.path.display(),
            report.duration,
            report
                .measured_lufs
                .map(|l| format!(", sound measured at {l:.1} LUFS before normalisation"))
                .unwrap_or_default()
        );
        missing_note(&report.missing);
    }
    Ok(())
}
