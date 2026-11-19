//! Sound finish: music, sound effects (SFX) and ducking.
//!
//! Music and SFX are anchored on storyboard blocks, not on times: they
//! follow the edit when a take or a split changes. A music covers a run of
//! blocks; a SFX sits at an offset (possibly negative) from the start of a
//! block. [`Finish::resolve`] turns them into seconds for the mix.
//!
//! Ducking (the music dips under the voice) is computed from the dated
//! words of the transcript: deterministic, adjustable in dB.

use crate::id::BlockId;
use crate::project::Project;
use crate::transcript::Transcript;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

/// Id of a music or a SFX in the finish.
pub type CueId = u64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Finish {
    pub music: Vec<Music>,
    pub sfx: Vec<Sfx>,
    pub ducking: Ducking,
    /// Gain of the voice, in dB.
    pub voice_gain_db: f32,
    next_id: CueId,
}

impl Default for Finish {
    fn default() -> Self {
        Self {
            music: Vec::new(),
            sfx: Vec::new(),
            ducking: Ducking::default(),
            voice_gain_db: 0.0,
            next_id: 1,
        }
    }
}

/// A music, from the start of block `from` to the end of block `to`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Music {
    pub id: CueId,
    /// Library file.
    pub file: String,
    pub from: BlockId,
    pub to: BlockId,
    /// Time of the track where it starts (seconds). Loops if it is shorter
    /// than the covered blocks.
    #[serde(default)]
    pub offset: f64,
    #[serde(default = "default_music_gain")]
    pub gain_db: f32,
    #[serde(default = "default_fade")]
    pub fade_in: f64,
    #[serde(default = "default_fade")]
    pub fade_out: f64,
}

/// A sound effect, `offset` seconds from the start of block `block`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sfx {
    pub id: CueId,
    pub file: String,
    pub block: BlockId,
    #[serde(default)]
    pub offset: f64,
    #[serde(default)]
    pub gain_db: f32,
}

const fn default_music_gain() -> f32 {
    -14.0
}

const fn default_fade() -> f64 {
    1.0
}

/// The music dips while the voice speaks.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ducking {
    pub enabled: bool,
    /// Dip under the voice, in dB (negative).
    pub depth_db: f32,
    /// Ramp-down time before the first word, in seconds.
    pub attack: f64,
    /// Ramp-up time after the last word, in seconds.
    pub release: f64,
}

impl Default for Ducking {
    fn default() -> Self {
        Self {
            enabled: true,
            depth_db: -10.0,
            attack: 0.15,
            release: 0.6,
        }
    }
}

/// A placed music, in edit seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedMusic {
    pub id: CueId,
    pub file: String,
    pub start: f64,
    pub end: f64,
    pub offset: f64,
    pub gain_db: f32,
    pub fade_in: f64,
    pub fade_out: f64,
}

/// A placed SFX, in edit seconds (never before 0).
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedSfx {
    pub id: CueId,
    pub file: String,
    pub at: f64,
    pub gain_db: f32,
}

/// The finish turned into seconds for a given edit.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Placed {
    pub music: Vec<PlacedMusic>,
    pub sfx: Vec<PlacedSfx>,
    /// What no longer finds its block (edit changed).
    pub orphans: Vec<CueId>,
}

impl Placed {
    pub fn is_empty(&self) -> bool {
        self.music.is_empty() && self.sfx.is_empty()
    }
}

impl Finish {
    pub fn is_empty(&self) -> bool {
        self.music.is_empty() && self.sfx.is_empty()
    }

    fn take_id(&mut self) -> CueId {
        let id = self.next_id.max(1);
        self.next_id = id.saturating_add(1);
        id
    }

    /// Adds a music and returns its id.
    pub fn add_music(&mut self, file: String, from: BlockId, to: BlockId) -> CueId {
        let id = self.take_id();
        self.music.push(Music {
            id,
            file,
            from,
            to,
            offset: 0.0,
            gain_db: default_music_gain(),
            fade_in: default_fade(),
            fade_out: default_fade(),
        });
        id
    }

    /// Adds a SFX and returns its id.
    pub fn add_sfx(&mut self, file: String, block: BlockId, offset: f64) -> CueId {
        let id = self.take_id();
        self.sfx.push(Sfx {
            id,
            file,
            block,
            offset,
            gain_db: 0.0,
        });
        id
    }

    pub fn music_mut(&mut self, id: CueId) -> Option<&mut Music> {
        self.music.iter_mut().find(|m| m.id == id)
    }

    pub fn sfx_mut(&mut self, id: CueId) -> Option<&mut Sfx> {
        self.sfx.iter_mut().find(|s| s.id == id)
    }

    /// Removes a music or a SFX.
    pub fn remove(&mut self, id: CueId) {
        self.music.retain(|m| m.id != id);
        self.sfx.retain(|s| s.id != id);
    }

    /// Removes what uses a file (removed from the library).
    pub fn forget_file(&mut self, file: &str) {
        self.music.retain(|m| m.file != file);
        self.sfx.retain(|s| s.file != file);
    }

    /// Places music and SFX on the blocks of `project`.
    pub fn resolve(&self, project: &Project) -> Placed {
        let mut placed = Placed::default();
        let end = project.duration().as_secs_f64();
        for m in &self.music {
            let (Some(from), Some(to)) = (project.block(m.from), project.block(m.to)) else {
                placed.orphans.push(m.id);
                continue;
            };
            let (a, b) = (
                from.range.start().as_secs_f64(),
                to.range.end().as_secs_f64(),
            );
            // Blocks given backwards: still cover the interval.
            let (start, stop) = (a.min(b), a.max(b).min(end));
            if stop - start < 1e-3 {
                continue;
            }
            placed.music.push(PlacedMusic {
                id: m.id,
                file: m.file.clone(),
                start,
                end: stop,
                offset: m.offset.max(0.0),
                gain_db: m.gain_db,
                fade_in: m.fade_in.max(0.0),
                fade_out: m.fade_out.max(0.0),
            });
        }
        for s in &self.sfx {
            let Some(block) = project.block(s.block) else {
                placed.orphans.push(s.id);
                continue;
            };
            let at = (block.range.start().as_secs_f64() + s.offset).clamp(0.0, end);
            placed.sfx.push(PlacedSfx {
                id: s.id,
                file: s.file.clone(),
                at,
                gain_db: s.gain_db,
            });
        }
        placed
    }
}

/// Spoken passages `(start, end)` in seconds, from the words: two words
/// less than `gap` seconds apart are in the same passage.
pub fn speech_spans(transcript: &Transcript, gap: f64) -> Vec<(f64, f64)> {
    let mut spans: Vec<(f64, f64)> = Vec::new();
    for word in &transcript.words {
        let (a, b) = (
            word.range.start().as_secs_f64(),
            word.range.end().as_secs_f64(),
        );
        match spans.last_mut() {
            Some(last) if a - last.1 < gap => last.1 = last.1.max(b),
            _ => spans.push((a, b)),
        }
    }
    spans
}

impl Ducking {
    /// Passages where the music dips: those of the voice, merged when their
    /// ramps would touch.
    pub fn spans(&self, transcript: &Transcript) -> Vec<(f64, f64)> {
        let (attack, release) = (self.attack.max(0.01), self.release.max(0.01));
        speech_spans(transcript, attack + release + 0.05)
    }

    /// Damping at time `t`: 0 (full music) to 1 (dipped).
    pub fn amount(&self, spans: &[(f64, f64)], t: f64) -> f64 {
        let (attack, release) = (self.attack.max(0.01), self.release.max(0.01));
        spans
            .iter()
            .map(|(a, b)| {
                let up = ((t - (a - attack)) / attack).clamp(0.0, 1.0);
                let down = (((b + release) - t) / release).clamp(0.0, 1.0);
                up.min(down)
            })
            .fold(0.0, f64::max)
    }

    /// Linear gain of the music at time `t`.
    pub fn gain(&self, spans: &[(f64, f64)], t: f64) -> f64 {
        if !self.enabled {
            return 1.0;
        }
        let floor = 10_f64.powf(f64::from(self.depth_db.min(0.0)) / 20.0);
        1.0 - (1.0 - floor) * self.amount(spans, t)
    }

    /// The same gain as an ffmpeg expression (`volume` filter, `eval=frame`).
    /// The passages do not overlap (see [`Ducking::spans`]), so their ramps
    /// add up.
    pub fn expression(&self, spans: &[(f64, f64)]) -> String {
        if !self.enabled || spans.is_empty() {
            return "1".to_owned();
        }
        let (attack, release) = (self.attack.max(0.01), self.release.max(0.01));
        let floor = 10_f64.powf(f64::from(self.depth_db.min(0.0)) / 20.0);
        let mut sum = String::new();
        for (a, b) in spans {
            if !sum.is_empty() {
                sum.push('+');
            }
            let _ = write!(
                sum,
                "min(clip((t-{:.3})/{attack:.3},0,1),clip(({:.3}-t)/{release:.3},0,1))",
                a - attack,
                b + release
            );
        }
        format!("1-{:.5}*min(1,{sum})", 1.0 - floor)
    }
}

/// Undo / redo history of the finish (whole states: the finish is
/// small).
#[derive(Debug, Default)]
pub struct FinishHistory {
    undo: Vec<Finish>,
    redo: Vec<Finish>,
}

impl FinishHistory {
    const LIMIT: usize = 200;

    /// To call before changing `finish`.
    pub fn record(&mut self, finish: &Finish) {
        self.undo.push(finish.clone());
        let excess = self.undo.len().saturating_sub(Self::LIMIT);
        self.undo.drain(..excess);
        self.redo.clear();
    }

    pub fn undo(&mut self, finish: &mut Finish) -> bool {
        match self.undo.pop() {
            Some(previous) => {
                self.redo.push(std::mem::replace(finish, previous));
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self, finish: &mut Finish) -> bool {
        match self.redo.pop() {
            Some(next) => {
                self.undo.push(std::mem::replace(finish, next));
                true
            }
            None => false,
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{Block, BlockKind};
    use crate::project::ProjectSettings;
    use crate::time::{Flicks, TimeRange};
    use crate::transcript::Word;

    fn secs(s: f64) -> Flicks {
        #[allow(clippy::cast_possible_truncation)]
        Flicks::from_millis((s * 1000.0) as i64).unwrap()
    }

    fn project(bounds: &[f64]) -> Project {
        let blocks = bounds
            .windows(2)
            .map(|w| {
                Block::new(
                    TimeRange::new(secs(w[0]), secs(w[1])).unwrap(),
                    BlockKind::Animation,
                )
            })
            .collect();
        Project::with_blocks("essai", ProjectSettings::default(), blocks).unwrap()
    }

    #[test]
    fn cues_follow_blocks() {
        let p = project(&[0.0, 4.0, 9.0, 12.0]);
        let ids: Vec<BlockId> = p.blocks().iter().map(|b| b.id).collect();
        let mut f = Finish::default();
        let m = f.add_music("theme.mp3".into(), ids[1], ids[2]);
        let s = f.add_sfx("whoosh.wav".into(), ids[1], -0.2);
        let lost = f.add_sfx("pop.wav".into(), BlockId::new(), 0.0);
        assert_ne!(m, s);
        let placed = f.resolve(&p);
        assert_eq!(placed.music.len(), 1);
        assert_eq!((placed.music[0].start, placed.music[0].end), (4.0, 12.0));
        assert!((placed.sfx[0].at - 3.8).abs() < 1e-9);
        assert_eq!(placed.orphans, [lost]);
        // A SFX before the start stays at 0.
        f.sfx_mut(s).unwrap().offset = -10.0;
        assert_eq!(f.resolve(&p).sfx[0].at, 0.0);
        let json = serde_json::to_string(&f).unwrap();
        let back: Finish = serde_json::from_str(&json).unwrap();
        assert_eq!(back, f);
        f.forget_file("theme.mp3");
        assert!(f.music.is_empty());
        // A new id after reloading does not reuse the old ones.
        let mut back = back;
        assert!(back.add_sfx("x.wav".into(), ids[0], 0.0) > lost);
    }

    fn transcript(words: &[(f64, f64)]) -> Transcript {
        Transcript {
            language: "fr".into(),
            media_duration: secs(20.0),
            words: words
                .iter()
                .map(|(a, b)| Word {
                    text: "mot".into(),
                    range: TimeRange::new(secs(*a), secs(*b)).unwrap(),
                    confidence: 1.0,
                })
                .collect(),
        }
    }

    #[test]
    fn ducking_follows_the_voice() {
        let t = transcript(&[(1.0, 1.5), (1.6, 2.0), (6.0, 7.0)]);
        assert_eq!(speech_spans(&t, 0.3), [(1.0, 2.0), (6.0, 7.0)]);
        let d = Ducking::default();
        let spans = d.spans(&t);
        assert_eq!(spans.len(), 2);
        assert!((d.gain(&spans, 0.0) - 1.0).abs() < 1e-9, "musique pleine");
        let floor = 10_f64.powf(-10.0 / 20.0);
        assert!(
            (d.gain(&spans, 1.5) - floor).abs() < 1e-9,
            "under the voice"
        );
        let mid = d.gain(&spans, 2.3);
        assert!(mid > floor && mid < 1.0, "ramp-up");
        assert!((d.gain(&spans, 4.0) - 1.0).abs() < 1e-9);
        // Two close passages are merged: no ramp-up between them.
        let close = transcript(&[(1.0, 2.0), (2.5, 3.0)]);
        assert_eq!(d.spans(&close), [(1.0, 3.0)]);
        let off = Ducking {
            enabled: false,
            ..d
        };
        assert_eq!(off.gain(&spans, 1.5), 1.0);
        assert_eq!(off.expression(&spans), "1");
        let expr = d.expression(&spans);
        assert!(expr.starts_with("1-0.68377*min(1,min(clip((t-0.850)/0.150"));
        assert_eq!(expr.matches("clip((t-").count(), 2);
    }

    #[test]
    fn history_restores_states() {
        let mut f = Finish::default();
        let mut h = FinishHistory::default();
        h.record(&f);
        f.voice_gain_db = 3.0;
        h.record(&f);
        f.ducking.enabled = false;
        assert!(h.undo(&mut f));
        assert!(f.ducking.enabled && f.voice_gain_db == 3.0);
        assert!(h.undo(&mut f));
        assert_eq!(f, Finish::default());
        assert!(!h.undo(&mut f));
        assert!(h.redo(&mut f) && h.redo(&mut f));
        assert!(!f.ducking.enabled);
        assert!(!h.can_redo());
    }
}
