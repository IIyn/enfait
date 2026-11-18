//! `enfait-cli search` et `enfait-cli cutout` : les outils d'assets sans
//! interface (recherche libre de droits, détourage).

use crate::paths::Dirs;
use anyhow::{Context as _, Result};
use enfait_assets::cutout::{Cutter, Model, cut_file, ensure_model};
use enfait_assets::search::{Media, Source, client, download, search};
use std::path::PathBuf;

#[derive(Debug, clap::Args)]
pub struct SearchArgs {
    /// Mots cherchés (en anglais de préférence pour Commons et Openverse).
    pub query: String,
    /// Chercher des vidéos plutôt que des images.
    #[arg(long, conflicts_with = "audio")]
    pub video: bool,
    /// Chercher des sons (musiques, effets).
    #[arg(long)]
    pub audio: bool,
    /// Télécharge les N premiers résultats dans ce dossier, avec leurs crédits.
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
            tracing::info!("{} ignoré : définis {var} pour l'utiliser", source.name());
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
    tracing::info!("crédits :\n{credits}");
    Ok(())
}

#[derive(Debug, clap::Args)]
pub struct CutoutArgs {
    /// Image à détourer.
    pub input: PathBuf,
    /// PNG transparent écrit.
    pub output: PathBuf,
    /// Modèle rapide (U²-Net-p) plutôt que précis (ISNet).
    #[arg(long)]
    pub fast: bool,
}

pub async fn cutout_cmd(dirs: &Dirs, args: CutoutArgs) -> Result<()> {
    let model = if args.fast {
        Model::Fast
    } else {
        Model::Precise
    };
    let dir = dirs.models().join("detourage");
    tokio::fs::create_dir_all(&dir).await?;
    let path = ensure_model(&dir, model).await?;
    let started = std::time::Instant::now();
    tokio::task::spawn_blocking(move || {
        let cutter = Cutter::load(&path, model)?;
        cut_file(&cutter, &args.input, &args.output)
    })
    .await??;
    tracing::info!("détouré en {:.1} s", started.elapsed().as_secs_f64());
    Ok(())
}
