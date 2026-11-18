//! `enfait-cli export` : la vidéo finale (MP4) ou le montage pour Premiere
//! Pro (XML FCP7) à partir de la session de l'interface.

use anyhow::Result;
use enfait_studio::export::{ExportOptions, TARGET_LUFS};
use enfait_studio::premiere::PremiereOptions;
use enfait_studio::session::SessionFiles;
use std::path::PathBuf;

#[derive(Debug, clap::Args)]
pub struct ExportArgs {
    /// Vidéo à écrire (MP4).
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Montage Premiere à écrire (XML FCP7) ; les médias sont copiés à côté.
    #[arg(long)]
    pub premiere: Option<PathBuf>,
    /// Sans les effets sonores.
    #[arg(long)]
    pub sans_sfx: bool,
    /// Loudness visée, en LUFS.
    #[arg(long, default_value_t = TARGET_LUFS, allow_negative_numbers = true)]
    pub lufs: f64,
    /// Son laissé tel qu'il est mixé (pas de normalisation).
    #[arg(long)]
    pub sans_normalisation: bool,
    /// Dossier de la session (par défaut celle de l'interface,
    /// `ENFAIT_SESSION` sinon).
    #[arg(long)]
    pub session: Option<PathBuf>,
}

fn missing_note(missing: &[usize]) {
    if !missing.is_empty() {
        let list: Vec<String> = missing.iter().map(ToString::to_string).collect();
        tracing::warn!(
            "blocs sans animation ni plan (la prise les couvre) : {}",
            list.join(", ")
        );
    }
}

pub async fn run(args: ExportArgs) -> Result<()> {
    anyhow::ensure!(
        args.output.is_some() || args.premiere.is_some(),
        "rien à exporter : --output <vidéo.mp4> et/ou --premiere <montage.xml>"
    );
    let dir = args
        .session
        .clone()
        .unwrap_or_else(enfait_mcp::bridge::session_dir);
    let session = SessionFiles::load(&dir)?;
    let sources = session.sources();
    let settings = session.project.settings();
    tracing::info!(
        "{} : {}×{}, {:.1} s, {} blocs",
        settings.aspect.label(),
        settings.resolution.width,
        settings.resolution.height,
        session.project.duration().as_secs_f64(),
        session.project.blocks().len()
    );
    if let Some(xml) = &args.premiere {
        let report = enfait_studio::premiere::export(
            &sources,
            PremiereOptions {
                sfx: !args.sans_sfx,
            },
            xml,
        )?;
        tracing::info!(
            "montage Premiere : {} (médias : {})",
            report.xml.display(),
            report.media.display()
        );
        missing_note(&report.missing);
    }
    if let Some(out) = &args.output {
        let last = std::sync::atomic::AtomicU64::new(u64::MAX);
        let progress = |p: f64| {
            // Toutes les dizaines de pour cent.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let step = (p * 10.0).floor() as u64;
            if last.swap(step, std::sync::atomic::Ordering::Relaxed) != step {
                tracing::info!("export : {} %", step.saturating_mul(10));
            }
        };
        let report = enfait_studio::export::video(
            &sources,
            ExportOptions {
                sfx: !args.sans_sfx,
                loudness: (!args.sans_normalisation).then_some(args.lufs),
            },
            out,
            &progress,
        )
        .await?;
        tracing::info!(
            "vidéo : {} ({:.1} s{})",
            report.path.display(),
            report.duration,
            report
                .measured_lufs
                .map(|l| format!(", son mesuré à {l:.1} LUFS avant normalisation"))
                .unwrap_or_default()
        );
        missing_note(&report.missing);
    }
    Ok(())
}
