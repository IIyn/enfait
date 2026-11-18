//! `enfait-cli media` : prépare un média comme le fera l'interface.

use crate::paths::Dirs;
use anyhow::Result;
use std::path::PathBuf;

#[derive(Debug, clap::Args)]
pub struct MediaArgs {
    /// Vidéo ou son.
    pub file: PathBuf,
}

pub async fn run(dirs: &Dirs, args: MediaArgs) -> Result<()> {
    let cache = enfait_media::MediaCache::new(dirs.media());
    let started = std::time::Instant::now();
    let (prepared, video) = tokio::task::spawn_blocking(move || {
        let mut last = None;
        let prepared = cache.prepare(&args.file, &mut |stage, _| {
            if last != Some(stage) {
                tracing::info!("{}…", stage.label());
                last = Some(stage);
            }
        })?;
        tracing::info!("lisible après {:.1} s", started.elapsed().as_secs_f64());
        if prepared.video.as_ref().is_some_and(|v| !v.is_proxy) {
            tracing::info!("{}…", enfait_media::Stage::Proxy.label());
        }
        let video = cache.build_proxy(&prepared, &mut |_| {})?;
        Ok::<_, enfait_media::MediaError>((prepared, video))
    })
    .await??;
    let info = &prepared.info;
    tracing::info!("durée : {:.2} s", info.duration);
    if let Some(v) = info.video {
        tracing::info!(
            "image : {}×{}, {}/{} i/s, {} images",
            v.width,
            v.height,
            v.frame_rate.num(),
            v.frame_rate.den(),
            prepared.frame_count()
        );
    }
    if let Some(v) = &video {
        tracing::info!("proxy : {}×{}, {}", v.width, v.height, v.path.display());
    }
    if let Some(a) = &prepared.audio {
        tracing::info!(
            "son : {} Hz, {} voie(s), {:.2} s",
            a.rate(),
            a.channels(),
            a.duration()
        );
    }
    tracing::info!("prêt en {:.1} s", started.elapsed().as_secs_f64());
    Ok(())
}
