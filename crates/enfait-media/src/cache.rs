//! Préparation d'un média et cache sur disque.
//!
//! Pour chaque fichier source (identifié par son chemin, sa taille et sa
//! date de modification), un dossier du cache contient :
//!
//! - `info.json` : l'analyse ffprobe ;
//! - `proxy.mp4` : la vidéo réduite à 540 lignes, H.264 avec une image clé
//!   toutes les demi-secondes, rapide à décoder et à parcourir ;
//! - `audio.s16` : le son brut, 48 kHz, une ou deux voies ;
//! - `peaks.bin` : la forme d'onde.
//!
//! Chaque fichier est écrit sous un nom temporaire puis renommé : un fichier
//! présent est toujours complet.

use crate::MediaError;
use crate::audio::{self, AudioBuffer};
use crate::probe::{self, MediaInfo};
use crate::video::frame_at;
use crate::waveform::Peaks;
use enfait_core::FrameRate;
use std::path::{Path, PathBuf};

/// Hauteur maximale des proxys.
pub const PROXY_HEIGHT: u32 = 540;
/// Version du format du cache : la changer invalide les anciens dossiers.
const CACHE_VERSION: &str = "1";

/// Étape en cours, pour l'affichage de la progression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Probe,
    Proxy,
    Audio,
    Waveform,
}

impl Stage {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Probe => "analyse",
            Self::Proxy => "proxy vidéo",
            Self::Audio => "son",
            Self::Waveform => "forme d'onde",
        }
    }
}

/// Fichier d'où lire les images, et taille des images produites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoSource {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    /// `true` : le proxy ; `false` : le fichier d'origine, en attendant le
    /// proxy (décodage plus lourd, sauts plus lents).
    pub is_proxy: bool,
}

/// Un média prêt à être lu.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub source: PathBuf,
    pub info: MediaInfo,
    /// Images : le proxy s'il existe déjà, sinon le fichier d'origine
    /// (voir [`MediaCache::build_proxy`]).
    pub video: Option<VideoSource>,
    pub audio: Option<AudioBuffer>,
    pub peaks: Option<Peaks>,
}

impl Prepared {
    pub fn frame_rate(&self) -> Option<FrameRate> {
        self.info.video.map(|v| v.frame_rate)
    }

    /// Nombre d'images du média (0 sans vidéo).
    pub fn frame_count(&self) -> i64 {
        self.frame_rate().map_or(0, |r| {
            frame_at(self.info.duration - 1e-4, r).saturating_add(1)
        })
    }
}

pub struct MediaCache {
    root: PathBuf,
}

impl MediaCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Dossier du cache pour `source`.
    pub fn dir_for(&self, source: &Path) -> Result<PathBuf, MediaError> {
        let meta = std::fs::metadata(source)?;
        let modified = meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        let mut h = blake3::Hasher::new();
        h.update(CACHE_VERSION.as_bytes());
        h.update(source.as_os_str().as_encoded_bytes());
        h.update(&meta.len().to_le_bytes());
        h.update(&modified.to_le_bytes());
        let key: String = h.finalize().to_hex().chars().take(20).collect();
        Ok(self.root.join(key))
    }

    /// Prépare `source` pour une lecture immédiate : analyse, son et forme
    /// d'onde (rapides). Le proxy n'est pas fabriqué ici : s'il manque, les
    /// images viennent du fichier d'origine en attendant
    /// [`MediaCache::build_proxy`]. Bloquant : hors du fil de l'interface.
    pub fn prepare(
        &self,
        source: &Path,
        on_progress: &mut dyn FnMut(Stage, f32),
    ) -> Result<Prepared, MediaError> {
        let source = std::path::absolute(source)?;
        let dir = self.dir_for(&source)?;
        std::fs::create_dir_all(&dir)?;

        on_progress(Stage::Probe, 0.0);
        let info_path = dir.join("info.json");
        let info: MediaInfo = match std::fs::read(&info_path)
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
        {
            Some(info) => info,
            None => {
                let info = probe::probe(&source)?;
                write_atomic(&info_path, &serde_json::to_vec_pretty(&info)?)?;
                info
            }
        };
        on_progress(Stage::Probe, 1.0);

        let video = info.video.map(|v| {
            let (width, height) = proxy_size(v.width, v.height);
            let proxy = dir.join("proxy.mp4");
            let is_proxy = proxy.is_file();
            VideoSource {
                path: if is_proxy { proxy } else { source.clone() },
                width,
                height,
                is_proxy,
            }
        });

        let audio = match info.audio {
            Some(a) => {
                let channels = a.channels.min(2);
                let path = dir.join("audio.s16");
                if !path.is_file() {
                    let tmp = dir.join("audio.tmp.s16");
                    audio::decode_to_file(&source, channels, info.duration, &tmp, &mut |p| {
                        on_progress(Stage::Audio, p);
                    })?;
                    std::fs::rename(&tmp, &path)?;
                }
                Some(AudioBuffer::read_raw(&path, audio::RATE, channels)?)
            }
            None => None,
        };

        let peaks = match &audio {
            Some(buffer) => {
                on_progress(Stage::Waveform, 0.0);
                let path = dir.join("peaks.bin");
                let peaks = match Peaks::read(&path) {
                    Ok(p) => p,
                    Err(_) => {
                        let p = Peaks::compute(buffer);
                        let tmp = dir.join("peaks.tmp.bin");
                        p.write(&tmp)?;
                        std::fs::rename(&tmp, &path)?;
                        p
                    }
                };
                on_progress(Stage::Waveform, 1.0);
                Some(peaks)
            }
            None => None,
        };

        Ok(Prepared {
            source,
            info,
            video,
            audio,
            peaks,
        })
    }
}

impl MediaCache {
    /// Fabrique le proxy de `prepared` s'il manque (environ 18 fois le temps
    /// réel pour du 1080p) et renvoie la source d'images à utiliser ensuite.
    /// Bloquant : hors du fil de l'interface.
    pub fn build_proxy(
        &self,
        prepared: &Prepared,
        on_progress: &mut dyn FnMut(f32),
    ) -> Result<Option<VideoSource>, MediaError> {
        let (Some(info), Some(current)) = (prepared.info.video, &prepared.video) else {
            return Ok(None);
        };
        if current.is_proxy {
            return Ok(Some(current.clone()));
        }
        let dir = self.dir_for(&prepared.source)?;
        let path = dir.join("proxy.mp4");
        if !path.is_file() {
            let tmp = dir.join("proxy.tmp.mp4");
            make_proxy(
                &prepared.source,
                &tmp,
                info.frame_rate,
                (current.width, current.height),
                prepared.info.duration,
                on_progress,
            )?;
            std::fs::rename(&tmp, &path)?;
        }
        Ok(Some(VideoSource {
            path,
            width: current.width,
            height: current.height,
            is_proxy: true,
        }))
    }
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<(), MediaError> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Dimensions paires du proxy, sans agrandir la source.
pub fn proxy_size(width: u32, height: u32) -> (u32, u32) {
    let even = |x: u64| u32::try_from((x / 2).max(1) * 2).unwrap_or(2);
    if height <= PROXY_HEIGHT {
        return (even(u64::from(width)), even(u64::from(height)));
    }
    let w = (u64::from(width) * u64::from(PROXY_HEIGHT) + u64::from(height) / 2)
        / u64::from(height).max(1);
    (even(w), PROXY_HEIGHT)
}

fn make_proxy(
    source: &Path,
    out: &Path,
    rate: FrameRate,
    (width, height): (u32, u32),
    duration: f64,
    on_progress: &mut dyn FnMut(f32),
) -> Result<(), MediaError> {
    // Image clé toutes les demi-secondes, sans image B : un saut ne décode
    // jamais plus d'une demi-seconde.
    let gop = (rate.nominal_fps() / 2).max(1).to_string();
    let mut args: Vec<std::ffi::OsString> = vec!["-i".into(), source.into()];
    args.extend(
        [
            "-map",
            "0:v:0",
            "-an",
            "-sn",
            "-vf",
            &format!(
                "fps={}/{},scale={width}:{height}:flags=bicubic",
                rate.num(),
                rate.den()
            ),
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-tune",
            "fastdecode",
            "-crf",
            "23",
            "-g",
            &gop,
            "-keyint_min",
            &gop,
            "-sc_threshold",
            "0",
            "-bf",
            "0",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            "-f",
            "mp4",
        ]
        .map(Into::into),
    );
    args.push(out.into());
    crate::ffmpeg::run(args, duration, on_progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_dimensions() {
        assert_eq!(proxy_size(1920, 1080), (960, 540));
        assert_eq!(proxy_size(1080, 1920), (304, 540));
        assert_eq!(proxy_size(3840, 2160), (960, 540));
        assert_eq!(proxy_size(640, 360), (640, 360));
        assert_eq!(proxy_size(641, 361), (640, 360));
        assert_eq!(proxy_size(1440, 1080), (720, 540));
    }
}
