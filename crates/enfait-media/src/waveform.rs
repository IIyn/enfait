//! Forme d'onde : crêtes minimales et maximales par paquets d'échantillons.
//!
//! Un seul niveau de détail (256 instants par paquet, soit 5,3 ms à 48 kHz)
//! suffit : pour l'affichage, on regroupe les paquets d'une colonne, ce qui
//! reste rapide même pour une heure de son (560 000 paquets).

use crate::{AudioBuffer, MediaError};
use std::io::Write as _;
use std::path::Path;

pub const BUCKET: u32 = 256;
const MAGIC: &[u8; 4] = b"EFPK";
const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peaks {
    rate: u32,
    bucket: u32,
    /// Paires (min, max) par paquet, toutes voies confondues.
    pairs: Vec<(i16, i16)>,
}

impl Peaks {
    pub fn compute(audio: &AudioBuffer) -> Self {
        let channels = usize::from(audio.channels());
        let per_bucket = usize::try_from(BUCKET)
            .unwrap_or(256)
            .saturating_mul(channels);
        let pairs = audio
            .samples()
            .chunks(per_bucket.max(1))
            .map(|chunk| {
                chunk
                    .iter()
                    .fold((i16::MAX, i16::MIN), |(lo, hi), s| (lo.min(*s), hi.max(*s)))
            })
            .collect();
        Self {
            rate: audio.rate(),
            bucket: BUCKET,
            pairs,
        }
    }

    pub fn duration(&self) -> f64 {
        self.pairs.len() as f64 * f64::from(self.bucket) / f64::from(self.rate)
    }

    /// `columns` paires (min, max) entre -1 et 1 pour l'intervalle
    /// `[start, end)` en secondes. Une colonne sans donnée vaut (0, 0).
    pub fn columns(&self, start: f64, end: f64, columns: usize) -> Vec<(f32, f32)> {
        if columns == 0 || start.is_nan() || end.is_nan() || end <= start {
            return Vec::new();
        }
        let per_second = f64::from(self.rate) / f64::from(self.bucket);
        let step = (end - start) / columns as f64;
        let scale = 1.0 / 32_768.0;
        (0..columns)
            .map(|c| {
                let t0 = start + step * c as f64;
                let a = bucket_index(t0 * per_second);
                let b = bucket_index((t0 + step) * per_second).max(a.saturating_add(1));
                let slice = self
                    .pairs
                    .get(a.min(self.pairs.len())..b.min(self.pairs.len()))
                    .unwrap_or_default();
                if slice.is_empty() {
                    return (0.0, 0.0);
                }
                let (lo, hi) = slice.iter().fold((i16::MAX, i16::MIN), |(lo, hi), (l, h)| {
                    (lo.min(*l), hi.max(*h))
                });
                (f32::from(lo) * scale, f32::from(hi) * scale)
            })
            .collect()
    }

    pub fn write(&self, path: &Path) -> Result<(), MediaError> {
        let mut out = Vec::with_capacity(self.pairs.len().saturating_mul(4).saturating_add(20));
        out.write_all(MAGIC)?;
        out.write_all(&VERSION.to_le_bytes())?;
        out.write_all(&self.rate.to_le_bytes())?;
        out.write_all(&self.bucket.to_le_bytes())?;
        for (lo, hi) in &self.pairs {
            out.write_all(&lo.to_le_bytes())?;
            out.write_all(&hi.to_le_bytes())?;
        }
        std::fs::write(path, out)?;
        Ok(())
    }

    pub fn read(path: &Path) -> Result<Self, MediaError> {
        let bytes = std::fs::read(path)?;
        let invalid = || MediaError::Cache(format!("forme d'onde {}", path.display()));
        let (header, body) = bytes.split_first_chunk::<16>().ok_or_else(invalid)?;
        let (magic, rest) = header.split_first_chunk::<4>().ok_or_else(invalid)?;
        let words: Vec<u32> = rest
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_le_bytes(*b))
            .collect();
        let [version, rate, bucket] = words.as_slice() else {
            return Err(invalid());
        };
        if magic != MAGIC || *version != VERSION || *rate == 0 || *bucket == 0 {
            return Err(invalid());
        }
        let values = crate::audio::from_le_bytes(body);
        let pairs = values
            .as_chunks::<2>()
            .0
            .iter()
            .map(|[lo, hi]| (*lo, *hi))
            .collect();
        Ok(Self {
            rate: *rate,
            bucket: *bucket,
            pairs,
        })
    }
}

fn bucket_index(x: f64) -> usize {
    if x <= 0.0 {
        0
    } else {
        // Positif et fini (borné par la durée) : troncature voulue.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let i = x.floor() as usize;
        i
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp() -> AudioBuffer {
        // 1 s à 1 024 Hz, mono : quatre paquets de 256, chacun à son niveau.
        let samples = (0..1024_i32)
            .map(|i| i16::try_from((i / 256) * 1000 - (i % 2) * 2000).unwrap())
            .collect();
        AudioBuffer::new(1024, 1, samples).unwrap()
    }

    #[test]
    fn buckets_hold_extremes() {
        let p = Peaks::compute(&ramp());
        assert_eq!(
            p.pairs,
            vec![(-2000, 0), (-1000, 1000), (0, 2000), (1000, 3000)]
        );
        assert!((p.duration() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn columns_aggregate_and_pad() {
        let p = Peaks::compute(&ramp());
        let two = p.columns(0.0, 1.0, 2);
        assert_eq!(two.len(), 2);
        assert!((two[0].0 - (-2000.0 / 32768.0)).abs() < 1e-6);
        assert!((two[1].1 - (3000.0 / 32768.0)).abs() < 1e-6);
        // Plus de colonnes que de paquets : chaque colonne lit au moins un paquet.
        assert_eq!(p.columns(0.0, 1.0, 16).len(), 16);
        // Au-delà de la fin : silence.
        assert_eq!(p.columns(2.0, 3.0, 3), vec![(0.0, 0.0); 3]);
        assert!(p.columns(1.0, 1.0, 3).is_empty());
    }

    #[test]
    fn file_roundtrip_and_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.bin");
        let p = Peaks::compute(&ramp());
        p.write(&path).unwrap();
        assert_eq!(Peaks::read(&path).unwrap(), p);
        std::fs::write(&path, b"EFPK").unwrap();
        assert!(Peaks::read(&path).is_err());
    }
}
