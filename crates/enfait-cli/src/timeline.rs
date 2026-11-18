//! Correspondance entre les images de la vidéo finale et les rendus.
//!
//! Chaque bloc est rendu sur ses propres images, élargies d'une marge avant
//! et après quand une transition le touche. Une transition remplace les
//! images autour de la frontière entre deux blocs.

use anyhow::{Context as _, Result};
use enfait_core::{FrameRate, TimeRange};
use std::ops::Range;

/// Part de la transition avant la frontière (le reste est après).
const TRANSITION_BEFORE: f64 = 0.4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockFrames {
    /// Images de la vidéo qui appartiennent au bloc.
    pub own: Range<i64>,
    /// Images rendues (marges de transition comprises).
    pub rendered: Range<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionFrames {
    /// Index du bloc de départ (le suivant est `after + 1`).
    pub after: usize,
    pub frames: Range<i64>,
}

#[derive(Debug, Clone)]
pub struct Timeline {
    pub rate: FrameRate,
    pub total: Range<i64>,
    pub blocks: Vec<BlockFrames>,
    pub transitions: Vec<TransitionFrames>,
}

/// Source d'une image de la vidéo finale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Bloc et index de l'image dans son dossier de rendu.
    Block(usize, usize),
    /// Transition (index dans `transitions`) et index de l'image.
    Transition(usize, usize),
}

fn local(index: i64, start: i64) -> Option<usize> {
    usize::try_from(index.checked_sub(start)?).ok()
}

impl Timeline {
    /// `transitions` : index des blocs suivis d'une transition ; `length` :
    /// durée voulue d'une transition, en secondes.
    pub fn build(
        rate: FrameRate,
        ranges: &[TimeRange],
        transitions: &[usize],
        length: f64,
    ) -> Result<Self> {
        let mut blocks = ranges
            .iter()
            .map(|r| {
                let own = rate.frames_in(*r)?;
                Ok(BlockFrames {
                    rendered: own.clone(),
                    own,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let fps = f64::from(rate.num()) / f64::from(rate.den());
        // Petite durée positive, bornée : la conversion ne peut pas déborder.
        #[allow(clippy::cast_possible_truncation)]
        let frames = |secs: f64| (secs * fps).round().clamp(1.0, 600.0) as i64;
        let before_len = frames(length * TRANSITION_BEFORE);
        let after_len = frames(length * (1.0 - TRANSITION_BEFORE));
        let mut windows = Vec::new();
        for &after in transitions {
            let (Some(a), Some(b)) = (blocks.get(after), blocks.get(after.saturating_add(1)))
            else {
                continue;
            };
            let boundary = b.own.start;
            let start = boundary.saturating_sub(before_len).max(a.own.start);
            let end = boundary.saturating_add(after_len).min(b.own.end);
            if end.saturating_sub(start) < 4 {
                continue;
            }
            windows.push(TransitionFrames {
                after,
                frames: start..end,
            });
            if let Some(a) = blocks.get_mut(after) {
                a.rendered.end = a.rendered.end.max(end);
            }
            if let Some(b) = blocks.get_mut(after.saturating_add(1)) {
                b.rendered.start = b.rendered.start.min(start);
            }
        }
        let total_end = blocks.last().map_or(0, |b| b.own.end);
        Ok(Self {
            rate,
            total: 0..total_end,
            blocks,
            transitions: windows,
        })
    }

    /// Instant local (s) de l'image globale `index` pour un rendu qui commence
    /// à l'image `start` de la vidéo, avec `origin` le début du bloc.
    pub fn local_time(&self, index: i64, origin: i64) -> Result<f64> {
        let t = self.rate.frame_to_flicks(index)?;
        let o = self.rate.frame_to_flicks(origin)?;
        Ok(t.checked_sub(o)?.as_secs_f64())
    }

    /// D'où vient l'image `index` de la vidéo finale.
    pub fn source(&self, index: i64) -> Result<Source> {
        for (i, t) in self.transitions.iter().enumerate() {
            if t.frames.contains(&index) {
                return local(index, t.frames.start)
                    .map(|j| Source::Transition(i, j))
                    .context("index de transition");
            }
        }
        for (i, b) in self.blocks.iter().enumerate() {
            if b.own.contains(&index) {
                return local(index, b.rendered.start)
                    .map(|j| Source::Block(i, j))
                    .context("index de bloc");
            }
        }
        anyhow::bail!("aucune source pour l'image {index}")
    }

    /// Image `index` de la vidéo dans le rendu du bloc `block`.
    pub fn block_frame(&self, block: usize, index: i64) -> Option<usize> {
        let b = self.blocks.get(block)?;
        if b.rendered.contains(&index) {
            local(index, b.rendered.start)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enfait_core::Flicks;

    fn ranges(ms: &[i64]) -> Vec<TimeRange> {
        ms.windows(2)
            .map(|w| {
                TimeRange::new(
                    Flicks::from_millis(w[0]).unwrap(),
                    Flicks::from_millis(w[1]).unwrap(),
                )
                .unwrap()
            })
            .collect()
    }

    #[test]
    fn every_frame_has_a_source() {
        let r = ranges(&[0, 2000, 4500, 7000]);
        let tl = Timeline::build(FrameRate::FPS_30, &r, &[0], 1.0).unwrap();
        assert_eq!(tl.total, 0..210);
        assert_eq!(tl.transitions[0].frames, 48..78);
        assert_eq!(tl.blocks[0].rendered, 0..78);
        assert_eq!(tl.blocks[1].rendered, 48..135);
        for i in tl.total.clone() {
            tl.source(i).unwrap();
        }
        assert_eq!(tl.source(50).unwrap(), Source::Transition(0, 2));
        assert_eq!(tl.source(80).unwrap(), Source::Block(1, 32));
        assert_eq!(tl.block_frame(1, 48), Some(0));
        assert_eq!(tl.block_frame(0, 70), Some(70));
    }

    #[test]
    fn local_time_is_relative_to_origin() {
        let r = ranges(&[0, 1000, 2000]);
        let tl = Timeline::build(FrameRate::FPS_30, &r, &[], 1.0).unwrap();
        let t = tl.local_time(33, 30).unwrap();
        assert!((t - 0.1).abs() < 1e-9);
        let before = tl.local_time(27, 30).unwrap();
        assert!(before < 0.0);
    }
}
