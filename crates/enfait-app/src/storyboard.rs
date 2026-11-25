//! Storyboard: the project built from the edit of the Cut step.
//!
//! Blocks split the transcript of the edit (sentence ends, pauses). When
//! the edit changes (another take, script edited), blocks are recomputed
//! and each new block takes over what had been written for an old block
//! with the same text: kind, description, transition, moments, b-roll shot
//! and cutout.

use anyhow::Result;
use enfait_core::{
    Block, BlockId, BlockKind, Edit, Flicks, History, Moment, Project, ProjectSettings, TimeRange,
    Transcript, Word,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub struct Storyboard {
    pub project: Project,
    /// Transcript of the edit (edit time).
    pub transcript: Transcript,
    /// Fingerprint of the original edit (see [`key`]).
    pub key: String,
    pub history: History,
    /// Block shown in the inspector.
    pub selected: Option<BlockId>,
    /// Blocks chosen for the edit (Shift+click: range, Ctrl/Cmd+click:
    /// one more), in storyboard order; contains `selected`.
    pub marked: Vec<BlockId>,
    /// Start of a Shift+click range.
    pub anchor: Option<BlockId>,
    /// Word chosen in the selected block (index in `transcript.words`).
    pub word: Option<usize>,
    /// Moment chosen in the selected block.
    pub moment: Option<Flicks>,
}

/// File stored next to `project.json`.
#[derive(Serialize, Deserialize)]
struct Montage {
    key: String,
    transcript: Transcript,
}

/// Fingerprint of an edit: its pieces, to the millisecond.
pub fn key(ranges: &[(f64, f64)]) -> String {
    ranges
        .iter()
        .map(|(a, b)| format!("{:.3}-{:.3}", a, b))
        .collect::<Vec<_>>()
        .join(",")
}

impl Storyboard {
    /// Splits `transcript` (that of the edit) into blocks, taking over what
    /// `previous` held for blocks with the same text.
    pub fn build(
        name: &str,
        settings: ProjectSettings,
        transcript: Transcript,
        key: String,
        previous: Option<&Self>,
    ) -> Result<Self> {
        let mut ranges = if transcript.words.is_empty() {
            Vec::new()
        } else {
            enfait_speech::segment(&transcript, &enfait_speech::SegmentParams::standard()?)?
        };
        if ranges.is_empty() && transcript.media_duration > Flicks::ZERO {
            // Nothing said (video without voice): blocks of about five
            // seconds, to be merged or split by hand.
            ranges = even_blocks(transcript.media_duration)?;
        }
        let mut used = vec![false; previous.map_or(0, |p| p.project.blocks().len())];
        let blocks = ranges
            .into_iter()
            .map(|range| {
                let mut block = Block::new(range, BlockKind::Animation);
                let text = transcript.text_in(range);
                let old = previous.and_then(|p| {
                    p.project.blocks().iter().enumerate().find(|(i, b)| {
                        !used.get(*i).copied().unwrap_or(true)
                            && p.transcript.text_in(b.range) == text
                    })
                });
                if let Some((i, old)) = old {
                    if let Some(u) = used.get_mut(i) {
                        *u = true;
                    }
                    // Same id: the animation already made stays attached.
                    block.id = old.id;
                    block.kind = old.kind;
                    block.description.clone_from(&old.description);
                    block.transition_after = old.transition_after.clone();
                    block.moments = shift_moments(&old.moments, old.range.start(), range);
                    block.broll.clone_from(&old.broll);
                    block.cutout = old.cutout;
                }
                block
            })
            .collect();
        let project = Project::with_blocks(name, settings, blocks)?;
        Ok(Self {
            project,
            transcript,
            key,
            history: History::new(),
            selected: None,
            marked: Vec::new(),
            anchor: None,
            word: None,
            moment: None,
        })
    }

    pub fn load(dir: &Path) -> Option<Self> {
        let project = std::fs::read_to_string(dir.join("project.json")).ok()?;
        let project = Project::from_json(&project)
            .map_err(|e| tracing::warn!("unreadable project.json: {e}"))
            .ok()?;
        let montage: Montage =
            serde_json::from_slice(&std::fs::read(dir.join("montage.json")).ok()?).ok()?;
        let first = project.blocks().first().map(|b| b.id);
        let mut board = Self {
            project,
            transcript: montage.transcript,
            key: montage.key,
            history: History::new(),
            selected: None,
            marked: Vec::new(),
            anchor: None,
            word: None,
            moment: None,
        };
        board.select(first);
        Some(board)
    }

    pub fn save(&self, dir: &Path) {
        let write = |name: &str, data: Result<Vec<u8>>| {
            let result = data.and_then(|data| {
                std::fs::create_dir_all(dir)?;
                let tmp = dir.join(format!("{name}.tmp"));
                std::fs::write(&tmp, data)?;
                std::fs::rename(&tmp, dir.join(name))?;
                Ok(())
            });
            if let Err(e) = result {
                tracing::warn!("cannot save {name}: {e:#}");
            }
        };
        write(
            "project.json",
            self.project
                .to_json()
                .map(String::into_bytes)
                .map_err(Into::into),
        );
        let montage = Montage {
            key: self.key.clone(),
            transcript: self.transcript.clone(),
        };
        write(
            "montage.json",
            serde_json::to_vec(&montage).map_err(Into::into),
        );
    }

    pub fn block(&self) -> Option<&Block> {
        self.selected.and_then(|id| self.project.block(id))
    }

    pub fn select(&mut self, id: Option<BlockId>) {
        if self.selected != id {
            self.word = None;
            self.moment = None;
        }
        self.selected = id;
        self.marked = id.into_iter().collect();
        self.anchor = id;
    }

    /// Shift+click: every block from the anchor to `id`.
    pub fn select_range(&mut self, id: BlockId) {
        let blocks = self.project.blocks();
        let to = blocks.iter().position(|b| b.id == id);
        let from = self
            .anchor
            .and_then(|a| blocks.iter().position(|b| b.id == a))
            .or(to);
        let (Some(from), Some(to)) = (from, to) else {
            return;
        };
        let (a, b) = (from.min(to), from.max(to));
        self.marked = blocks
            .get(a..=b)
            .map(|s| s.iter().map(|b| b.id).collect())
            .unwrap_or_default();
        if self.selected != Some(id) {
            self.word = None;
            self.moment = None;
        }
        self.selected = Some(id);
    }

    /// Ctrl/Cmd+click: adds or removes one block.
    pub fn toggle_marked(&mut self, id: BlockId) {
        if let Some(i) = self.marked.iter().position(|m| *m == id) {
            if self.marked.len() > 1 {
                self.marked.remove(i);
                if self.selected == Some(id) {
                    self.selected = self.marked.last().copied();
                    self.word = None;
                    self.moment = None;
                }
            }
            return;
        }
        self.marked.push(id);
        let order: Vec<BlockId> = self.project.blocks().iter().map(|b| b.id).collect();
        self.marked
            .sort_by_key(|m| order.iter().position(|o| o == m).unwrap_or(usize::MAX));
        if self.selected != Some(id) {
            self.word = None;
            self.moment = None;
        }
        self.selected = Some(id);
        self.anchor = Some(id);
    }

    /// Every block chosen.
    pub fn mark_all(&mut self) {
        self.marked = self.project.blocks().iter().map(|b| b.id).collect();
        if self.selected.is_none() {
            self.selected = self.marked.first().copied();
        }
    }

    /// Spoken words of a block: (index, word).
    pub fn words_of(&self, block: &Block) -> Vec<(usize, &Word)> {
        self.transcript
            .words
            .iter()
            .enumerate()
            .filter(|(_, w)| block.range.contains(w.range.start()))
            .collect()
    }

    pub fn text_of(&self, block: &Block) -> String {
        self.transcript.text_in(block.range)
    }

    /// Block containing time `t` (edit seconds).
    pub fn block_at(&self, t: f64) -> Option<BlockId> {
        // Blocks follow each other: binary search on their ends.
        let blocks = self.project.blocks();
        let i = blocks.partition_point(|b| b.range.end().as_secs_f64() <= t);
        blocks
            .get(i)
            .filter(|b| t >= b.range.start().as_secs_f64())
            .or_else(|| {
                self.project
                    .blocks()
                    .last()
                    .filter(|b| t >= b.range.end().as_secs_f64())
            })
            .map(|b| b.id)
    }

    pub fn apply(&mut self, edit: Edit) -> Result<(), enfait_core::EditError> {
        self.history.apply(&mut self.project, edit)?;
        self.repair_selection();
        Ok(())
    }

    pub fn undo(&mut self) -> bool {
        let done = self.history.undo(&mut self.project).unwrap_or(false);
        self.repair_selection();
        done
    }

    pub fn redo(&mut self) -> bool {
        let done = self.history.redo(&mut self.project).unwrap_or(false);
        self.repair_selection();
        done
    }

    /// After an edit, the selection may point to a block or moment that is gone.
    fn repair_selection(&mut self) {
        let project = &self.project;
        self.marked.retain(|id| project.block(*id).is_some());
        if let Some(id) = self.selected
            && project.block(id).is_some()
            && !self.marked.contains(&id)
        {
            self.marked.push(id);
        }
        let Some(block) = self.block() else {
            self.selected = self.project.blocks().first().map(|b| b.id);
            self.word = None;
            self.moment = None;
            return;
        };
        let range = block.range;
        let moment_exists = self
            .moment
            .is_some_and(|at| block.moments.iter().any(|m| m.at == at));
        if !moment_exists {
            self.moment = None;
        }
        let word_inside = self
            .word
            .and_then(|i| self.transcript.words.get(i))
            .is_some_and(|w| range.contains(w.range.start()));
        if !word_inside {
            self.word = None;
        }
    }

    /// Cuts the selected block just before the chosen word (in the middle of
    /// the silence before it). The next block is selected.
    pub fn split_at_word(&mut self) -> Result<(), enfait_core::EditError> {
        let (Some(id), Some(i)) = (self.selected, self.word) else {
            return Ok(());
        };
        let words = &self.transcript.words;
        let (Some(word), Some(previous)) =
            (words.get(i), i.checked_sub(1).and_then(|p| words.get(p)))
        else {
            return Err(enfait_core::EditError::SplitOutsideBlock);
        };
        let a = previous.range.end().get();
        let b = word.range.start().get();
        let at = Flicks::new(a.saturating_add(b.saturating_sub(a) / 2));
        let new_block = BlockId::new();
        self.apply(Edit::Split {
            block: id,
            at,
            new_block,
        })?;
        self.select(Some(new_block));
        self.word = Some(i);
        Ok(())
    }

    /// Cuts the block under time `t` (edit seconds) there: for passages
    /// without words. The second part is selected.
    pub fn split_at_time(&mut self, t: f64) -> Result<(), enfait_core::EditError> {
        #[allow(clippy::cast_possible_truncation)]
        let at = Flicks::from_millis((t * 1000.0).round() as i64)?;
        let Some(block) = self
            .project
            .blocks()
            .iter()
            .find(|b| b.range.contains(at) && at > b.range.start())
        else {
            return Err(enfait_core::EditError::SplitOutsideBlock);
        };
        let new_block = BlockId::new();
        self.apply(Edit::Split {
            block: block.id,
            at,
            new_block,
        })?;
        self.select(Some(new_block));
        Ok(())
    }

    /// Sets a moment at time `t` (edit seconds) in the selected block,
    /// anchored on the word said then if there is one.
    pub fn add_moment_at(&mut self, t: f64) -> Result<(), enfait_core::EditError> {
        let Some(block) = self.block() else {
            return Ok(());
        };
        #[allow(clippy::cast_possible_truncation)]
        let at = Flicks::from_millis((t * 1000.0).round() as i64)?;
        if !block.range.contains(at) || block.moments.iter().any(|m| m.at == at) {
            return Ok(());
        }
        let word = self
            .transcript
            .words
            .iter()
            .find(|w| w.range.contains(at))
            .map(|w| w.text.clone())
            .unwrap_or_default();
        let id = block.id;
        let moment = Moment {
            at,
            word,
            note: String::new(),
        };
        self.apply(Edit::AddMoment { block: id, moment })?;
        self.moment = Some(at);
        Ok(())
    }

    /// Sets a moment at the start of the chosen word, and selects it.
    pub fn add_moment(&mut self) -> Result<(), enfait_core::EditError> {
        let (Some(id), Some(word)) = (
            self.selected,
            self.word.and_then(|i| self.transcript.words.get(i)),
        ) else {
            return Ok(());
        };
        let moment = Moment {
            at: word.range.start(),
            word: word.text.clone(),
            note: String::new(),
        };
        let at = moment.at;
        self.apply(Edit::AddMoment { block: id, moment })?;
        self.moment = Some(at);
        Ok(())
    }
}

/// Target length of the blocks of a passage without words.
const EVEN_BLOCK_SECONDS: f64 = 5.0;

/// `duration` in blocks of about five seconds.
fn even_blocks(duration: Flicks) -> Result<Vec<TimeRange>> {
    let total = duration.as_secs_f64();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let count = ((total / EVEN_BLOCK_SECONDS).round() as i64).max(1);
    let step = duration.get() / count;
    let mut out = Vec::new();
    let mut start = Flicks::ZERO;
    for i in 1..=count {
        let end = if i == count {
            duration
        } else {
            Flicks::new(step.saturating_mul(i))
        };
        out.push(TimeRange::new(start, end)?);
        start = end;
    }
    Ok(out)
}

/// Moments of an old block, shifted onto the new one, if they fall in it.
fn shift_moments(moments: &[Moment], old_start: Flicks, range: TimeRange) -> Vec<Moment> {
    moments
        .iter()
        .filter_map(|m| {
            let offset = m.at.checked_sub(old_start).ok()?;
            let at = range.start().checked_add(offset).ok()?;
            range.contains(at).then(|| Moment { at, ..m.clone() })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use enfait_core::Transition;

    /// Transcript: 0.3 s per word, `|` = one-second pause.
    fn transcript(text: &str) -> Transcript {
        let ms = |t: i64| Flicks::from_millis(t).unwrap();
        let mut t = 0_i64;
        let mut words = Vec::new();
        for w in text.split_whitespace() {
            if w == "|" {
                t += 1000;
                continue;
            }
            words.push(Word {
                text: w.into(),
                range: TimeRange::new(ms(t), ms(t + 250)).unwrap(),
                confidence: 1.0,
            });
            t += 300;
        }
        Transcript {
            language: "fr".into(),
            words,
            media_duration: ms(t + 300),
        }
    }

    const A: &str = "Un paquet en carton glisse sur un rail et file vers la sortie.";
    const B: &str = "Sur aiguilleur un aiguillage bascule et l'envoie à droite tout de suite.";

    #[test]
    fn edits_survive_a_new_montage() {
        let t = transcript(&format!("{A} | {B}"));
        let mut s =
            Storyboard::build("Essai", ProjectSettings::default(), t, "k1".into(), None).unwrap();
        assert_eq!(s.project.blocks().len(), 2, "one sentence per block");
        let second = s.project.blocks()[1].id;
        s.select(Some(second));
        s.apply(Edit::SetDescription {
            block: second,
            text: "L'aiguillage bascule.".into(),
        })
        .unwrap();
        s.apply(Edit::SetTransition {
            block: s.project.blocks()[0].id,
            transition: Transition::Auto,
        })
        .unwrap();
        // Moment on "aiguilleur".
        s.word = s
            .words_of(s.block().unwrap())
            .iter()
            .find(|(_, w)| w.text == "aiguilleur")
            .map(|(i, _)| *i);
        s.add_moment().unwrap();
        assert_eq!(s.block().unwrap().moments.len(), 1);

        // New edit: the first sentence changes, the second moves 0.6 s earlier.
        let t2 = transcript(&format!(
            "Un paquet glisse sur un rail et file vers la sortie. | {B}"
        ));
        let rebuilt = Storyboard::build(
            "Essai",
            ProjectSettings::default(),
            t2,
            "k2".into(),
            Some(&s),
        )
        .unwrap();
        let blocks = rebuilt.project.blocks();
        assert_eq!(
            blocks[0].transition_after,
            Transition::Cut,
            "text changed: nothing taken over"
        );
        assert_eq!(blocks[1].description, "L'aiguillage bascule.");
        let m = &blocks[1].moments[0];
        assert_eq!(m.word, "aiguilleur");
        assert!(blocks[1].range.contains(m.at));
        assert_eq!(rebuilt.text_of(&blocks[1]), B);
    }

    #[test]
    fn split_at_word_undo_and_save() {
        let t = transcript(A);
        let mut s =
            Storyboard::build("Essai", ProjectSettings::default(), t, "k".into(), None).unwrap();
        let id = s.project.blocks()[0].id;
        s.select(Some(id));
        // The first word cannot cut.
        s.word = Some(0);
        assert!(s.split_at_word().is_err());
        s.word = Some(5); // "sur"
        s.split_at_word().unwrap();
        assert_eq!(s.project.blocks().len(), 2);
        assert_eq!(
            s.text_of(&s.project.blocks()[1]),
            "sur un rail et file vers la sortie."
        );
        assert_eq!(s.selected, Some(s.project.blocks()[1].id));
        assert!(s.undo());
        assert_eq!(s.project.blocks().len(), 1);
        assert_eq!(
            s.selected,
            Some(id),
            "the selection goes back to an existing block"
        );
        assert!(s.redo());

        let dir = std::env::temp_dir().join(format!("enfait-storyboard-{}", std::process::id()));
        s.save(&dir);
        let loaded = Storyboard::load(&dir).unwrap();
        assert_eq!(loaded.project, s.project);
        assert_eq!(loaded.key, "k");
        assert_eq!(loaded.block_at(0.1), Some(s.project.blocks()[0].id));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
