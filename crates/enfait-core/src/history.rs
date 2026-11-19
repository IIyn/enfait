//! Undo / redo.
//!
//! Every applied [`Edit`] returns its exact inverse. The history stacks
//! these inverses: undoing is applying the inverse (which in turn returns
//! what is needed to redo).

use crate::block::{Block, BlockKind, Broll, Moment, Transition};
use crate::error::EditError;
use crate::id::BlockId;
use crate::project::{Project, ProjectSettings};
use crate::time::Flicks;
use serde::{Deserialize, Serialize};

/// A change to the project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Edit {
    SetDescription {
        block: BlockId,
        text: String,
    },
    SetKind {
        block: BlockId,
        kind: BlockKind,
    },
    SetTransition {
        block: BlockId,
        transition: Transition,
    },
    SetBroll {
        block: BlockId,
        broll: Option<Broll>,
    },
    SetCutout {
        block: BlockId,
        cutout: bool,
    },
    Split {
        block: BlockId,
        at: Flicks,
        new_block: BlockId,
    },
    MergeWithNext {
        block: BlockId,
    },
    AddMoment {
        block: BlockId,
        moment: Moment,
    },
    RemoveMoment {
        block: BlockId,
        at: Flicks,
    },
    SetMomentNote {
        block: BlockId,
        at: Flicks,
        note: String,
    },
    /// Format, framing and frame rate of the project.
    SetSettings {
        settings: ProjectSettings,
    },
    /// Several edits applied at once (the storyboard written by the editor):
    /// a single undo step. All or nothing.
    Batch {
        edits: Vec<Edit>,
    },
    /// Exact inverse of a merge: recreates the next block as it was.
    Unmerge {
        block: BlockId,
        first_description: String,
        first_transition: Transition,
        second: Box<Block>,
    },
}

impl Edit {
    /// Label shown in "Undo: …".
    pub fn label(&self) -> &'static str {
        match self {
            Self::SetDescription { .. } => "Edit the description",
            Self::SetKind { .. } => "Change the block kind",
            Self::SetTransition { .. } => "Change the transition",
            Self::SetBroll { .. } => "Change the b-roll shot",
            Self::SetCutout { .. } => "Cut out the person",
            Self::Split { .. } => "Split the block",
            Self::MergeWithNext { .. } => "Merge the blocks",
            Self::Unmerge { .. } => "Unmerge the blocks",
            Self::AddMoment { .. } => "Add a moment",
            Self::RemoveMoment { .. } => "Remove the moment",
            Self::SetMomentNote { .. } => "Edit the moment",
            Self::SetSettings { .. } => "Change the format",
            Self::Batch { .. } => "Write the storyboard",
        }
    }

    /// Applies the edit and returns its inverse. On error, the project is untouched.
    pub fn apply(self, project: &mut Project) -> Result<Edit, EditError> {
        match self {
            Self::SetDescription { block, text } => {
                let old = project.set_description(block, text)?;
                Ok(Self::SetDescription { block, text: old })
            }
            Self::SetKind { block, kind } => {
                let old = project.set_kind(block, kind)?;
                Ok(Self::SetKind { block, kind: old })
            }
            Self::SetTransition { block, transition } => {
                let old = project.set_transition(block, transition)?;
                Ok(Self::SetTransition {
                    block,
                    transition: old,
                })
            }
            Self::SetBroll { block, broll } => {
                let old = project.set_broll(block, broll)?;
                Ok(Self::SetBroll { block, broll: old })
            }
            Self::SetCutout { block, cutout } => {
                let old = project.set_cutout(block, cutout)?;
                Ok(Self::SetCutout { block, cutout: old })
            }
            Self::Split {
                block,
                at,
                new_block,
            } => {
                project.split(block, at, new_block)?;
                Ok(Self::MergeWithNext { block })
            }
            Self::MergeWithNext { block } => {
                let (first_description, first_transition, second) =
                    project.merge_with_next(block)?;
                Ok(Self::Unmerge {
                    block,
                    first_description,
                    first_transition,
                    second: Box::new(second),
                })
            }
            Self::Unmerge {
                block,
                first_description,
                first_transition,
                second,
            } => {
                project.unmerge(block, first_description, first_transition, *second)?;
                Ok(Self::MergeWithNext { block })
            }
            Self::AddMoment { block, moment } => {
                let at = moment.at;
                project.add_moment(block, moment)?;
                Ok(Self::RemoveMoment { block, at })
            }
            Self::RemoveMoment { block, at } => {
                let moment = project.remove_moment(block, at)?;
                Ok(Self::AddMoment { block, moment })
            }
            Self::Batch { edits } => {
                let mut inverses = Vec::with_capacity(edits.len());
                for edit in edits {
                    match edit.apply(project) {
                        Ok(inverse) => inverses.push(inverse),
                        Err(err) => {
                            // Undo what is already applied: the project
                            // stays untouched.
                            for inverse in inverses.into_iter().rev() {
                                let _ = inverse.apply(project);
                            }
                            return Err(err);
                        }
                    }
                }
                inverses.reverse();
                Ok(Self::Batch { edits: inverses })
            }
            Self::SetSettings { settings } => Ok(Self::SetSettings {
                settings: project.set_settings(settings),
            }),
            Self::SetMomentNote { block, at, note } => {
                let old = project.set_moment_note(block, at, note)?;
                Ok(Self::SetMomentNote {
                    block,
                    at,
                    note: old,
                })
            }
        }
    }
}

#[derive(Debug, Clone)]
struct Entry {
    label: &'static str,
    edit: Edit,
}

/// Undo / redo stacks, bounded.
#[derive(Debug, Clone)]
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    limit: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    pub const DEFAULT_LIMIT: usize = 500;

    pub fn new() -> Self {
        Self::with_limit(Self::DEFAULT_LIMIT)
    }

    pub fn with_limit(limit: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            limit,
        }
    }

    /// Applies an edit and records it in the history. Clears the redo stack.
    pub fn apply(&mut self, project: &mut Project, edit: Edit) -> Result<(), EditError> {
        let label = edit.label();
        let inverse = edit.apply(project)?;
        self.undo.push(Entry {
            label,
            edit: inverse,
        });
        let excess = self.undo.len().saturating_sub(self.limit);
        self.undo.drain(..excess);
        self.redo.clear();
        Ok(())
    }

    /// Undoes the last edit. `Ok(false)` if there is nothing to undo.
    pub fn undo(&mut self, project: &mut Project) -> Result<bool, EditError> {
        Self::step(project, &mut self.undo, &mut self.redo)
    }

    /// Redoes the last undone edit. `Ok(false)` if there is nothing to redo.
    pub fn redo(&mut self, project: &mut Project) -> Result<bool, EditError> {
        Self::step(project, &mut self.redo, &mut self.undo)
    }

    fn step(
        project: &mut Project,
        from: &mut Vec<Entry>,
        to: &mut Vec<Entry>,
    ) -> Result<bool, EditError> {
        let Some(entry) = from.pop() else {
            return Ok(false);
        };
        match entry.edit.clone().apply(project) {
            Ok(inverse) => {
                to.push(Entry {
                    label: entry.label,
                    edit: inverse,
                });
                Ok(true)
            }
            Err(err) => {
                from.push(entry);
                Err(err)
            }
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<&'static str> {
        self.undo.last().map(|e| e.label)
    }

    pub fn redo_label(&self) -> Option<&'static str> {
        self.redo.last().map(|e| e.label)
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use super::*;
    use crate::project::ProjectSettings;
    use crate::time::TimeRange;
    use proptest::prelude::*;

    fn secs(s: i64) -> Flicks {
        Flicks::from_secs(s).unwrap()
    }

    fn sample() -> Project {
        let cuts = [0, 6, 14, 23, 31];
        let kinds = [
            BlockKind::Rush,
            BlockKind::Animation,
            BlockKind::Mixed,
            BlockKind::Animation,
        ];
        let blocks = cuts
            .windows(2)
            .zip(kinds)
            .enumerate()
            .map(|(i, (w, kind))| {
                let mut b = Block::new(TimeRange::new(secs(w[0]), secs(w[1])).unwrap(), kind);
                b.description = format!("block {i}");
                b.transition_after = if i % 2 == 0 {
                    Transition::Cut
                } else {
                    Transition::Auto
                };
                b
            })
            .collect();
        Project::with_blocks("Test", ProjectSettings::default(), blocks).unwrap()
    }

    fn id(p: &Project, i: usize) -> BlockId {
        p.blocks()[i].id
    }

    #[test]
    fn format_change_is_undone_exactly() {
        use crate::project::{AspectRatio, Framing};
        let mut p = sample();
        let before = *p.settings();
        let mut tall = before.with_aspect(AspectRatio::Portrait9x16);
        tall.framing = Framing::Fill;
        tall.pan = -20;
        let mut h = History::new();
        h.apply(&mut p, Edit::SetSettings { settings: tall })
            .unwrap();
        assert_eq!(p.settings().resolution.height, 1920);
        assert_eq!(h.undo_label(), Some("Change the format"));
        h.undo(&mut p).unwrap();
        assert_eq!(*p.settings(), before);
        h.redo(&mut p).unwrap();
        assert_eq!(*p.settings(), tall);
        // Old project.json without framing: whole image.
        let json = p.to_json().unwrap().replace("\"framing\": \"fill\",", "");
        assert_eq!(
            Project::from_json(&json).unwrap().settings().framing,
            Framing::Fit
        );
    }

    #[test]
    fn split_then_undo_restores() {
        let mut p = sample();
        let original = p.clone();
        let mut h = History::new();
        let b = id(&p, 1);
        h.apply(
            &mut p,
            Edit::Split {
                block: b,
                at: secs(10),
                new_block: BlockId::new(),
            },
        )
        .unwrap();
        assert_eq!(p.blocks().len(), 5);
        assert_eq!(p.blocks()[1].transition_after, Transition::Cut);
        assert_eq!(p.blocks()[2].transition_after, Transition::Auto);
        assert_eq!(h.undo_label(), Some("Split the block"));
        assert!(h.undo(&mut p).unwrap());
        assert_eq!(p, original);
        assert!(h.redo(&mut p).unwrap());
        assert_eq!(p.blocks().len(), 5);
    }

    fn moment(s: i64, word: &str) -> Moment {
        Moment {
            at: secs(s),
            word: word.into(),
            note: String::new(),
        }
    }

    #[test]
    fn moments_follow_splits_and_merges() {
        let mut p = sample();
        let original = p.clone();
        let mut h = History::new();
        let b = id(&p, 1); // 6 → 14 s
        for (s, w) in [(7, "rail"), (12, "aiguilleur")] {
            h.apply(
                &mut p,
                Edit::AddMoment {
                    block: b,
                    moment: moment(s, w),
                },
            )
            .unwrap();
        }
        assert_eq!(
            h.apply(
                &mut p,
                Edit::AddMoment {
                    block: b,
                    moment: moment(7, "x")
                }
            ),
            Err(EditError::DuplicateMoment)
        );
        assert_eq!(
            h.apply(
                &mut p,
                Edit::AddMoment {
                    block: b,
                    moment: moment(20, "x")
                }
            ),
            Err(EditError::MomentOutsideBlock)
        );
        h.apply(
            &mut p,
            Edit::SetMomentNote {
                block: b,
                at: secs(12),
                note: "l'aiguillage bascule".into(),
            },
        )
        .unwrap();
        // Split at 10 s: the second moment goes with the second half.
        let tail = BlockId::new();
        h.apply(
            &mut p,
            Edit::Split {
                block: b,
                at: secs(10),
                new_block: tail,
            },
        )
        .unwrap();
        assert_eq!(p.blocks()[1].moments.len(), 1);
        assert_eq!(p.blocks()[2].moments[0].note, "l'aiguillage bascule");
        // Merging glues them back, in order.
        h.apply(&mut p, Edit::MergeWithNext { block: b }).unwrap();
        let words: Vec<&str> = p.blocks()[1]
            .moments
            .iter()
            .map(|m| m.word.as_str())
            .collect();
        assert_eq!(words, ["rail", "aiguilleur"]);
        // Undoing everything goes back to the original; the JSON keeps the moments.
        let json = p.to_json().unwrap();
        assert_eq!(Project::from_json(&json).unwrap(), p);
        while h.undo(&mut p).unwrap() {}
        assert_eq!(p, original);
    }

    #[test]
    fn broll_follows_splits() {
        let mut p = sample();
        let original = p.clone();
        let mut h = History::new();
        let b = id(&p, 1);
        let start = p.blocks()[1].range.start();
        h.apply(
            &mut p,
            Edit::SetBroll {
                block: b,
                broll: Some(Broll {
                    file: "train.webm".into(),
                    start: secs(3),
                }),
            },
        )
        .unwrap();
        h.apply(
            &mut p,
            Edit::SetCutout {
                block: b,
                cutout: true,
            },
        )
        .unwrap();
        let at = start.checked_add(secs(2)).unwrap();
        h.apply(
            &mut p,
            Edit::Split {
                block: b,
                at,
                new_block: BlockId::new(),
            },
        )
        .unwrap();
        // The second half resumes the video where the first one stops.
        let tail = &p.blocks()[2];
        assert_eq!(tail.broll.as_ref().unwrap().start, secs(5));
        assert!(tail.cutout);
        assert_eq!(h.undo_label(), Some("Split the block"));
        while h.undo(&mut p).unwrap() {}
        assert_eq!(p, original);
        assert!(p.to_json().unwrap().find("broll").is_none());
    }

    #[test]
    fn merge_then_undo_restores() {
        let mut p = sample();
        let original = p.clone();
        let mut h = History::new();
        let first = id(&p, 0);
        h.apply(&mut p, Edit::MergeWithNext { block: first })
            .unwrap();
        assert_eq!(p.blocks().len(), 3);
        assert_eq!(p.blocks()[0].description, "block 0\nblock 1");
        assert_eq!(p.blocks()[0].range.end(), secs(14));
        h.undo(&mut p).unwrap();
        assert_eq!(p, original);
    }

    #[test]
    fn failed_edit_changes_nothing() {
        let mut p = sample();
        let original = p.clone();
        let mut h = History::new();
        let last = id(&p, 3);
        assert_eq!(
            h.apply(&mut p, Edit::MergeWithNext { block: last }),
            Err(EditError::NoNextBlock)
        );
        assert_eq!(
            h.apply(
                &mut p,
                Edit::Split {
                    block: last,
                    at: secs(23),
                    new_block: BlockId::new()
                }
            ),
            Err(EditError::SplitOutsideBlock)
        );
        let existing = id(&p, 0);
        assert_eq!(
            h.apply(
                &mut p,
                Edit::Split {
                    block: last,
                    at: secs(25),
                    new_block: existing
                }
            ),
            Err(EditError::DuplicateBlock(existing))
        );
        assert_eq!(p, original);
        assert!(!h.can_undo());
    }

    #[test]
    fn batch_is_one_step_and_all_or_nothing() {
        let mut p = sample();
        let before = p.clone();
        let mut h = History::new();
        let (a, b) = (id(&p, 0), id(&p, 1));
        h.apply(
            &mut p,
            Edit::Batch {
                edits: vec![
                    Edit::SetDescription {
                        block: a,
                        text: "un".into(),
                    },
                    Edit::SetKind {
                        block: b,
                        kind: BlockKind::Broll,
                    },
                    Edit::SetDescription {
                        block: a,
                        text: "deux".into(),
                    },
                ],
            },
        )
        .unwrap();
        assert_eq!(p.blocks()[0].description, "deux");
        assert_eq!(p.blocks()[1].kind, BlockKind::Broll);
        assert_eq!(h.undo_label(), Some("Write the storyboard"));
        assert!(h.undo(&mut p).unwrap());
        assert_eq!(p, before, "a single step undoes everything, in order");
        assert!(h.redo(&mut p).unwrap());
        assert_eq!(p.blocks()[0].description, "deux");

        // An impossible edit in the middle: nothing is applied.
        let mut q = before.clone();
        let err = Edit::Batch {
            edits: vec![
                Edit::SetDescription {
                    block: a,
                    text: "x".into(),
                },
                Edit::SetDescription {
                    block: BlockId::new(),
                    text: "y".into(),
                },
            ],
        }
        .apply(&mut q);
        assert!(err.is_err());
        assert_eq!(q, before);
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut p = sample();
        let mut h = History::new();
        let b = id(&p, 1);
        h.apply(
            &mut p,
            Edit::SetDescription {
                block: b,
                text: "a".into(),
            },
        )
        .unwrap();
        h.undo(&mut p).unwrap();
        assert!(h.can_redo());
        h.apply(
            &mut p,
            Edit::SetKind {
                block: b,
                kind: BlockKind::Effect,
            },
        )
        .unwrap();
        assert!(!h.can_redo());
    }

    #[test]
    fn limit_drops_oldest() {
        let mut p = sample();
        let mut h = History::with_limit(2);
        let b = id(&p, 0);
        for text in ["a", "b", "c"] {
            h.apply(
                &mut p,
                Edit::SetDescription {
                    block: b,
                    text: text.into(),
                },
            )
            .unwrap();
        }
        assert!(h.undo(&mut p).unwrap());
        assert!(h.undo(&mut p).unwrap());
        assert!(!h.undo(&mut p).unwrap());
        assert_eq!(p.blocks()[0].description, "a");
    }

    #[test]
    fn project_json_roundtrip() {
        let p = sample();
        let json = p.to_json().unwrap();
        assert_eq!(Project::from_json(&json).unwrap(), p);
    }

    #[test]
    fn project_json_rejects_gaps() {
        let p = sample();
        let mut value: serde_json::Value = serde_json::from_str(&p.to_json().unwrap()).unwrap();
        value["blocks"][1]["range"]["start"] = serde_json::json!(secs(7).get());
        let err = Project::from_json(&value.to_string()).unwrap_err();
        assert!(matches!(
            err,
            crate::ProjectError::Invalid(EditError::NotContiguous)
        ));
    }

    #[derive(Debug, Clone)]
    enum Op {
        Describe(usize, String),
        Kind(usize, u8),
        Transition(usize, u8),
        Split(usize, u16),
        Merge(usize),
        Undo,
        Redo,
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            (any::<usize>(), "[a-z ]{0,8}").prop_map(|(i, s)| Op::Describe(i, s)),
            (any::<usize>(), 0_u8..4).prop_map(|(i, k)| Op::Kind(i, k)),
            (any::<usize>(), 0_u8..3).prop_map(|(i, t)| Op::Transition(i, t)),
            (any::<usize>(), 1_u16..1000).prop_map(|(i, f)| Op::Split(i, f)),
            any::<usize>().prop_map(Op::Merge),
            Just(Op::Undo),
            Just(Op::Redo),
        ]
    }

    fn to_edit(p: &Project, op: &Op) -> Option<Edit> {
        let n = p.blocks().len();
        let pick = |i: &usize| p.blocks()[i % n].clone();
        Some(match op {
            Op::Describe(i, s) => Edit::SetDescription {
                block: pick(i).id,
                text: s.clone(),
            },
            Op::Kind(i, k) => Edit::SetKind {
                block: pick(i).id,
                kind: [
                    BlockKind::Rush,
                    BlockKind::Animation,
                    BlockKind::Mixed,
                    BlockKind::Effect,
                ][usize::from(*k)],
            },
            Op::Transition(i, t) => Edit::SetTransition {
                block: pick(i).id,
                transition: match t {
                    0 => Transition::Cut,
                    1 => Transition::Auto,
                    _ => Transition::Described("zoom".into()),
                },
            },
            Op::Split(i, f) => {
                let b = pick(i);
                let len = b.range.duration().get();
                let at = b.range.start().get() + len * i64::from(*f) / 1000;
                Edit::Split {
                    block: b.id,
                    at: Flicks::new(at),
                    new_block: BlockId::new(),
                }
            }
            Op::Merge(i) => Edit::MergeWithNext { block: pick(i).id },
            Op::Undo | Op::Redo => return None,
        })
    }

    proptest! {
        #[test]
        fn undo_all_restores_original(ops in prop::collection::vec(op(), 0..60)) {
            let mut p = sample();
            let original = p.clone();
            let mut h = History::new();
            for o in &ops {
                match o {
                    Op::Undo => { h.undo(&mut p).unwrap(); }
                    Op::Redo => { h.redo(&mut p).unwrap(); }
                    other => {
                        if let Some(edit) = to_edit(&p, other) {
                            let before = p.clone();
                            if h.apply(&mut p, edit).is_err() {
                                prop_assert_eq!(&p, &before);
                            }
                        }
                    }
                }
                let json = p.to_json().unwrap();
                prop_assert!(Project::from_json(&json).is_ok());
            }
            while h.undo(&mut p).unwrap() {}
            prop_assert_eq!(p, original);
        }
    }
}
