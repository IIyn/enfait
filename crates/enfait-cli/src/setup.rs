//! `enfait-cli setup` : installe ce qui n'est pas dans le binaire.

use crate::paths::{Dirs, WHISPER_URL, find_in_path};
use anyhow::{Context as _, Result};
use futures::StreamExt as _;
use std::path::Path;
use tokio::io::AsyncWriteExt as _;

pub async fn run(dirs: &Dirs) -> Result<()> {
    if find_in_path("ffmpeg").is_none() {
        anyhow::bail!(
            "ffmpeg est introuvable dans le PATH : installe-le avec ton gestionnaire de paquets"
        );
    }
    tracing::info!("ffmpeg : ok");

    let chrome = enfait_motion::ensure_browser(&dirs.chromium()).await?;
    tracing::info!("chromium : {}", chrome.display());

    let model = dirs.whisper_model();
    if model.is_file() {
        tracing::info!("modèle whisper : {}", model.display());
    } else {
        download(WHISPER_URL, &model).await?;
    }

    match find_in_path("claude") {
        Some(path) => tracing::info!("Claude Code : {}", path.display()),
        None if enfait_config::secrets::is_set(enfait_config::Secret::Anthropic) => {
            tracing::info!("API Anthropic : clé trouvée");
        }
        None => tracing::warn!(
            "ni Claude Code ni ANTHROPIC_API_KEY : `make` ne pourra pas appeler de modèle"
        ),
    }
    Ok(())
}

/// Téléchargement en flux vers un fichier `.part`, renommé à la fin.
pub async fn download(url: &str, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tracing::info!("téléchargement de {url}");
    let response = reqwest::Client::builder()
        .user_agent(concat!("enfait/", env!("CARGO_PKG_VERSION")))
        .build()?
        .get(url)
        .send()
        .await?
        .error_for_status()?;
    let total = response.content_length();
    let part = dest.with_extension("part");
    let mut file = tokio::fs::File::create(&part).await?;
    let mut stream = response.bytes_stream();
    let mut done: u64 = 0;
    let mut next_report: u64 = 10;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        done = done.saturating_add(chunk.len() as u64);
        if let Some(total) = total.filter(|t| *t > 0) {
            let pct = done.saturating_mul(100) / total;
            if pct >= next_report {
                tracing::info!("{pct} % ({} Mo)", done / 1_000_000);
                next_report = next_report.saturating_add(10);
            }
        }
    }
    file.flush().await?;
    drop(file);
    tokio::fs::rename(&part, dest)
        .await
        .with_context(|| format!("renommage vers {}", dest.display()))?;
    Ok(())
}
