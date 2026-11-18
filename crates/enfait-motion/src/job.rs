//! Rendu d'une séquence d'images avec cache, et vérification d'une composition.

use crate::error::MotionError;
use crate::render::{Inspection, OutsideText, Renderer, Scenes};
use image::{GenericImageView as _, ImageFormat};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Version du moteur : la changer invalide tous les caches de frames.
const ENGINE_VERSION: &str = "2";

/// Nom du fichier de la frame `index` dans un dossier de rendu.
pub fn frame_name(index: usize) -> String {
    format!("{index:06}.jpg")
}

/// Une séquence d'images à produire depuis une composition.
pub struct FrameJob<'a> {
    pub html: &'a Path,
    pub width: u32,
    pub height: u32,
    /// Instant local (secondes) de chaque image à produire.
    pub times: Vec<f64>,
    /// Pour une transition : images des scènes voisines, une paire par frame.
    pub scenes: Option<Vec<(PathBuf, PathBuf)>>,
    /// Ajouté à la clé de cache (par exemple les clés des scènes voisines).
    pub extra_key: String,
    pub out_dir: &'a Path,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FrameOutcome {
    pub key: String,
    pub frames: usize,
    pub errors: Vec<String>,
    /// Vrai si le cache a servi.
    #[serde(skip)]
    pub cached: bool,
}

const DONE_FILE: &str = "done.json";

impl FrameJob<'_> {
    fn key(&self, html: &[u8]) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(ENGINE_VERSION.as_bytes());
        hasher.update(html);
        hasher.update(&self.width.to_le_bytes());
        hasher.update(&self.height.to_le_bytes());
        for t in &self.times {
            hasher.update(&t.to_le_bytes());
        }
        hasher.update(self.extra_key.as_bytes());
        hasher.finalize().to_hex().to_string()
    }

    /// Produit les images, ou réutilise le rendu précédent s'il correspond.
    pub async fn run(&self, renderer: &Renderer) -> Result<FrameOutcome, MotionError> {
        let html = tokio::fs::read(self.html).await?;
        let key = self.key(&html);
        let done = self.out_dir.join(DONE_FILE);
        if let Ok(previous) = tokio::fs::read(&done).await
            && let Ok(mut outcome) = serde_json::from_slice::<FrameOutcome>(&previous)
            && outcome.key == key
        {
            outcome.cached = true;
            return Ok(outcome);
        }
        if tokio::fs::try_exists(self.out_dir).await? {
            tokio::fs::remove_dir_all(self.out_dir).await?;
        }
        tokio::fs::create_dir_all(self.out_dir).await?;

        let stage = renderer.open(self.html, self.width, self.height).await?;
        let result = async {
            for (index, &t) in self.times.iter().enumerate() {
                let scenes = self
                    .scenes
                    .as_ref()
                    .and_then(|s| s.get(index))
                    .map(|(from, to)| Scenes { from, to });
                stage.seek(t, scenes).await?;
                let png = stage.capture_jpeg().await?;
                tokio::fs::write(self.out_dir.join(frame_name(index)), png).await?;
            }
            stage.errors().await
        }
        .await;
        stage.close().await;
        let errors = result?;
        let outcome = FrameOutcome {
            key,
            frames: self.times.len(),
            errors,
            cached: false,
        };
        tokio::fs::write(&done, serde_json::to_vec_pretty(&outcome)?).await?;
        Ok(outcome)
    }
}

/// Résultat de la vérification d'une composition.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Report {
    /// La page ne déclare pas le contrat, ou plante au chargement.
    pub contract_error: Option<String>,
    pub js_errors: Vec<String>,
    pub declared_duration: f64,
    pub expected_duration: f64,
    /// Texte hors zone sûre, par instant.
    pub outside: Vec<(f64, OutsideText)>,
    pub unknown_fonts: Vec<String>,
    /// Instants où l'image est d'une seule couleur.
    pub blank_at: Vec<f64>,
    /// Images échantillons (JPEG réduits) pour la critique visuelle.
    pub samples: Vec<Sample>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub t: f64,
    pub path: PathBuf,
}

impl Report {
    /// Problèmes à corriger avant toute critique esthétique.
    pub fn blocking(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(e) = &self.contract_error {
            out.push(format!("La page ne respecte pas le contrat : {e}"));
            return out;
        }
        for e in &self.js_errors {
            out.push(format!("Erreur JavaScript : {e}"));
        }
        if (self.declared_duration - self.expected_duration).abs() > 0.05 {
            out.push(format!(
                "window.enfait.duration vaut {:.3} s au lieu de {:.3} s.",
                self.declared_duration, self.expected_duration
            ));
        }
        if !self.unknown_fonts.is_empty() {
            out.push(format!(
                "Polices non disponibles (remplacées par une police système) : {}.",
                self.unknown_fonts.join(", ")
            ));
        }
        let samples = self.samples.len().max(1);
        if self.blank_at.len().saturating_mul(2) > samples {
            out.push(format!(
                "L'image est vide (une seule couleur) à {} des {} instants échantillonnés.",
                self.blank_at.len(),
                samples
            ));
        }
        out
    }

    /// Problèmes strictement techniques (contrat, JavaScript, polices), sans
    /// jugement sur l'image : pour les transitions, rendues ici sans scènes.
    pub fn technical(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        if let Some(e) = &self.contract_error {
            out.push(format!("La page ne respecte pas le contrat : {e}"));
            return out;
        }
        out.extend(
            self.js_errors
                .iter()
                .map(|e| format!("Erreur JavaScript : {e}")),
        );
        if !self.unknown_fonts.is_empty() {
            out.push(format!(
                "Polices non disponibles : {}.",
                self.unknown_fonts.join(", ")
            ));
        }
        out
    }

    /// Remarques utiles à la critique, non bloquantes.
    pub fn warnings(&self) -> Vec<String> {
        self.outside
            .iter()
            .map(|(t, o)| {
                format!(
                    "À {t:.2} s, le texte « {} » sort de la zone sûre (x {:.0}→{:.0}, y {:.0}→{:.0}).",
                    o.text, o.left, o.right, o.top, o.bottom
                )
            })
            .collect()
    }
}

/// Vérifie une composition et produit des échantillons pour la critique.
pub struct CheckJob<'a> {
    pub html: &'a Path,
    pub width: u32,
    pub height: u32,
    pub expected_duration: f64,
    pub sample_times: Vec<f64>,
    /// Largeur des échantillons JPEG (la hauteur suit).
    pub sample_width: u32,
    /// Images de scène (prise, plans voisins), une paire par échantillon.
    pub scenes: Option<Vec<(PathBuf, PathBuf)>>,
    pub out_dir: &'a Path,
}

impl CheckJob<'_> {
    pub async fn run(&self, renderer: &Renderer) -> Result<Report, MotionError> {
        let mut report = Report {
            expected_duration: self.expected_duration,
            ..Report::default()
        };
        let stage = match renderer.open(self.html, self.width, self.height).await {
            Ok(stage) => stage,
            Err(MotionError::Contract(e)) => {
                report.contract_error = Some(e);
                return Ok(report);
            }
            Err(e) => return Err(e),
        };
        tokio::fs::create_dir_all(self.out_dir).await?;
        report.declared_duration = stage.duration;
        let result = async {
            for (index, &t) in self.sample_times.iter().enumerate() {
                let scenes = self
                    .scenes
                    .as_ref()
                    .and_then(|s| s.get(index))
                    .map(|(from, to)| Scenes { from, to });
                stage.seek(t, scenes).await?;
                let png = stage.capture_jpeg().await?;
                let Inspection {
                    outside,
                    unknown_fonts,
                } = stage.inspect().await?;
                report.outside.extend(outside.into_iter().map(|o| (t, o)));
                for font in unknown_fonts {
                    if !report.unknown_fonts.contains(&font) {
                        report.unknown_fonts.push(font);
                    }
                }
                let path = self.out_dir.join(format!("sample-{index:02}.jpg"));
                let (blank, jpeg) = analyse(&png, self.sample_width)?;
                if blank {
                    report.blank_at.push(t);
                }
                tokio::fs::write(&path, jpeg).await?;
                report.samples.push(Sample { t, path });
            }
            stage.errors().await
        }
        .await;
        stage.close().await;
        report.js_errors = result?;
        Ok(report)
    }
}

/// Indique si l'image est unie et la réduit en JPEG de largeur `width`.
fn analyse(png: &[u8], width: u32) -> Result<(bool, Vec<u8>), MotionError> {
    let img = image::load_from_memory(png)?;
    let (w, h) = img.dimensions();
    let first = img.get_pixel(0, 0);
    let mut blank = true;
    'grid: for gy in 0..36u32 {
        for gx in 0..64u32 {
            let x = gx.saturating_mul(w) / 64;
            let y = gy.saturating_mul(h) / 36;
            let p = img.get_pixel(x.min(w.saturating_sub(1)), y.min(h.saturating_sub(1)));
            let differs =
                p.0.iter()
                    .zip(first.0.iter())
                    .take(3)
                    .any(|(a, b)| a.abs_diff(*b) > 6);
            if differs {
                blank = false;
                break 'grid;
            }
        }
    }
    let height = u64::from(h)
        .saturating_mul(u64::from(width))
        .checked_div(u64::from(w.max(1)))
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(h)
        .max(1);
    let small = img
        .resize_exact(width, height, image::imageops::FilterType::Triangle)
        .to_rgb8();
    let mut jpeg = Vec::new();
    small.write_to(&mut std::io::Cursor::new(&mut jpeg), ImageFormat::Jpeg)?;
    Ok((blank, jpeg))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(color: [u8; 3], dot: bool) -> Vec<u8> {
        let mut img = image::RgbImage::from_pixel(320, 180, image::Rgb(color));
        if dot {
            for x in 150..170 {
                for y in 80..100 {
                    img.put_pixel(x, y, image::Rgb([255, 255, 255]));
                }
            }
        }
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn detects_blank_frames() {
        assert!(analyse(&png([10, 20, 30], false), 160).unwrap().0);
        assert!(!analyse(&png([10, 20, 30], true), 160).unwrap().0);
    }

    #[test]
    fn blocking_lists_duration_mismatch() {
        let report = Report {
            declared_duration: 3.0,
            expected_duration: 4.0,
            samples: vec![Sample {
                t: 0.0,
                path: PathBuf::new(),
            }],
            ..Report::default()
        };
        assert_eq!(report.blocking().len(), 1);
    }
}
