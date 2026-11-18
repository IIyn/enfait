//! Plans de b-roll : une vidéo ou une image de la bibliothèque, montée sur
//! la durée d'un bloc sans passer par un modèle. Une vidéo est lue à partir
//! d'un instant (et reprise au début si elle est trop courte) ; une image
//! fixe reçoit un lent zoom avant. Le cadre est rempli (recadrage au centre).

use anyhow::{Context as _, Result};
use enfait_core::AssetKind;
use std::path::Path;

/// Filtres ffmpeg du plan : remplir `w`×`h` à `fps`, et pour une image un
/// zoom de 1 à 1,08 sur `frames` images (calculé sur une image deux fois
/// plus grande, pour un mouvement sans à-coups).
pub fn filter(kind: AssetKind, size: (u32, u32), fps: f64, frames: usize) -> String {
    let (w, h) = size;
    match kind {
        AssetKind::Video => format!(
            "scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},fps={fps},\
             setsar=1,format=yuv420p"
        ),
        _ => {
            let (w2, h2) = (w.saturating_mul(2), h.saturating_mul(2));
            let n = frames.max(2).saturating_sub(1);
            format!(
                "scale={w2}:{h2}:force_original_aspect_ratio=increase,crop={w2}:{h2},setsar=1,\
                 zoompan=z='1+0.08*on/{n}':x='iw/2-(iw/zoom/2)':y='ih/2-(ih/zoom/2)':\
                 d={frames}:s={w}x{h}:fps={fps},format=yuv420p"
            )
        }
    }
}

/// Rend le plan `source` en clip H.264 muet de `duration` secondes dans
/// `out`. `start` : instant de départ dans une vidéo.
pub async fn render(
    source: &Path,
    kind: AssetKind,
    start: f64,
    duration: f64,
    size: (u32, u32),
    fps: f64,
    out: &Path,
) -> Result<()> {
    anyhow::ensure!(
        !matches!(kind, AssetKind::Font | AssetKind::Audio),
        "une police ou un son ne peut pas servir de plan"
    );
    // Nombre d'images du plan : positif et borné.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let frames = (duration * fps).round().max(1.0) as usize;
    if let Some(dir) = out.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let tmp = out.with_extension("tmp.mp4");
    let mut cmd = enfait_config::process::tokio_command("ffmpeg");
    cmd.args(["-nostdin", "-v", "error", "-y"]);
    if kind == AssetKind::Video {
        cmd.args([
            "-stream_loop",
            "-1",
            "-ss",
            &format!("{:.4}", start.max(0.0)),
        ]);
    } else {
        cmd.args(["-loop", "1", "-framerate", &format!("{fps}")]);
    }
    let output = cmd
        .arg("-i")
        .arg(source)
        .args(["-vf", &filter(kind, size, fps, frames)])
        .args(["-frames:v", &frames.to_string(), "-an"])
        .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "18"])
        .args(["-g", "15", "-movflags", "+faststart"])
        .arg(&tmp)
        .output()
        .await
        .context("lancement de ffmpeg")?;
    anyhow::ensure!(
        output.status.success(),
        "plan de b-roll : {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    tokio::fs::rename(&tmp, out).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters() {
        let video = filter(AssetKind::Video, (1920, 1080), 30.0, 90);
        assert!(video.starts_with("scale=1920:1080:force_original_aspect_ratio=increase"));
        let still = filter(AssetKind::Image, (1080, 1920), 25.0, 50);
        assert!(still.contains("crop=2160:3840"));
        assert!(still.contains("z='1+0.08*on/49'") && still.contains("s=1080x1920"));
    }
}
