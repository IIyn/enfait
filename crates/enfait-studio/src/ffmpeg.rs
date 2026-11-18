//! Lancement de ffmpeg avec suivi de l'avancement.

use anyhow::{Context as _, Result};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, BufReader};

/// Lance `cmd` (une commande ffmpeg complète) ; `total` : durée produite,
/// en secondes, pour rapporter l'avancement (0 à 1) à `progress`.
pub async fn run(
    mut cmd: tokio::process::Command,
    total: f64,
    progress: Option<&(dyn Fn(f64) + Sync)>,
) -> Result<()> {
    let Some(progress) = progress else {
        let output = cmd.output().await.context("lancement de ffmpeg")?;
        anyhow::ensure!(
            output.status.success(),
            "ffmpeg : {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return Ok(());
    };
    // `-progress` doit précéder la sortie : on l'ajoute en tête des options
    // globales, qui valent pour toute la commande.
    let mut child = {
        let std = cmd.as_std();
        let mut with = enfait_config::process::tokio_command(std.get_program());
        with.args(["-progress", "pipe:1", "-nostats"]);
        with.args(std.get_args());
        with.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("lancement de ffmpeg")?
    };
    let stdout = child.stdout.take().context("sortie de ffmpeg")?;
    let mut stderr = child.stderr.take().context("erreurs de ffmpeg")?;
    let errors = tokio::spawn(async move {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text).await;
        text
    });
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(us) = line.strip_prefix("out_time_us=")
            && let Ok(us) = us.trim().parse::<f64>()
            && total > 0.0
        {
            progress((us / 1e6 / total).clamp(0.0, 1.0));
        }
    }
    let status = child.wait().await?;
    let errors = errors.await.unwrap_or_default();
    anyhow::ensure!(status.success(), "ffmpeg : {}", errors.trim());
    progress(1.0);
    Ok(())
}
