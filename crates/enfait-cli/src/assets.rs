//! `enfait-cli search` and `enfait-cli cutout`: the asset tools without
//! interface (royalty-free search, cutout).

use crate::paths::Dirs;
use anyhow::{Context as _, Result};
use enfait_assets::cutout::{Cutter, Model, cut_file, ensure_model};
use enfait_assets::search::{Media, Source, client, download, search};
use std::path::PathBuf;

#[derive(Debug, clap::Args)]
pub struct SearchArgs {
    /// Searched words (in English preferably for Commons and Openverse).
    pub query: String,
    /// Search videos rather than images.
    #[arg(long, conflicts_with = "audio")]
    pub video: bool,
    /// Search sounds (music, effects).
    #[arg(long)]
    pub audio: bool,
    /// Downloads the first N results into this folder, with their credits.
    #[arg(long)]
    pub download: Option<PathBuf>,
    #[arg(long, default_value_t = 3)]
    pub count: usize,
}

pub async fn search_cmd(args: SearchArgs) -> Result<()> {
    for source in Source::ALL {
        if !source.available()
            && let Some(var) = source.key_var()
        {
            tracing::info!(
                "{} skipped: set its key in the app's Settings or {var}",
                source.name()
            );
        }
    }
    let client = client()?;
    let media = if args.video {
        Media::Video
    } else if args.audio {
        Media::Audio
    } else {
        Media::Image
    };
    let results = search(&client, &args.query, media).await;
    for e in &results.errors {
        tracing::warn!("{e}");
    }
    for (i, hit) in results.hits.iter().enumerate() {
        let terms = hit.origin.license.terms();
        tracing::info!(
            "{:>2}. [{}] {} — {}×{}{} — {} ({}){}",
            i.saturating_add(1),
            hit.source.name(),
            hit.origin.title,
            hit.width,
            hit.height,
            hit.duration
                .map(|d| format!(", {d:.1} s"))
                .unwrap_or_default(),
            hit.origin.license.name,
            terms.summary(),
            if hit.origin.author.is_empty() {
                String::new()
            } else {
                format!(" — {}", hit.origin.author)
            }
        );
    }
    let Some(dir) = args.download else {
        return Ok(());
    };
    let mut assets = Vec::new();
    for (i, hit) in results.hits.iter().take(args.count).enumerate() {
        let path = dir.join(format!("{:02}.{}", i.saturating_add(1), hit.ext));
        download(&client, &hit.file, &path)
            .await
            .with_context(|| hit.file.clone())?;
        tracing::info!("{}", path.display());
        assets.push(enfait_core::Asset {
            file: path.display().to_string(),
            kind: hit.kind,
            name: hit.origin.title.clone(),
            description: String::new(),
            origin: Some(hit.origin.clone()),
        });
    }
    let credits = enfait_core::license::credits(&assets);
    std::fs::write(dir.join("credits.txt"), &credits)?;
    tracing::info!("credits:\n{credits}");
    Ok(())
}

#[derive(Debug, clap::Args)]
pub struct CutoutArgs {
    /// Image to cut out.
    pub input: PathBuf,
    /// Written transparent PNG.
    pub output: PathBuf,
    /// Fast model (U²-Net-p) rather than precise (ISNet).
    #[arg(long)]
    pub fast: bool,
}

pub async fn cutout_cmd(_dirs: &Dirs, args: CutoutArgs) -> Result<()> {
    let model = if args.fast {
        Model::Fast
    } else {
        Model::Precise
    };
    let dir = enfait_config::paths::cutout_models_dir();
    tokio::fs::create_dir_all(&dir).await?;
    let path = ensure_model(&dir, model).await?;
    let started = std::time::Instant::now();
    tokio::task::spawn_blocking(move || {
        let cutter = Cutter::load(&path, model)?;
        cut_file(&cutter, &args.input, &args.output)
    })
    .await??;
    tracing::info!("cut out in {:.1} s", started.elapsed().as_secs_f64());
    Ok(())
}
