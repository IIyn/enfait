//! The project: settings and storyboard blocks.
//!
//! Blocks follow each other without gap or overlap. They only change
//! through [`Edit`](crate::history::Edit)s, which validate everything before
//! writing: a refused edit leaves the project untouched.

use crate::block::{Block, BlockKind, Broll, Moment, Transition};
use crate::error::{EditError, ProjectError};
use crate::id::{BlockId, ProjectId};
use crate::time::{Flicks, FrameRate, TimeRange};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::mem;

/// Version of the `project.json` file format.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AspectRatio {
    #[default]
    Landscape16x9,
    Portrait9x16,
    Square1x1,
}

impl AspectRatio {
    /// Offered formats, in interface order.
    pub const ALL: [Self; 3] = [Self::Landscape16x9, Self::Portrait9x16, Self::Square1x1];

    /// Working and export resolution.
    pub const fn resolution(self) -> Resolution {
        match self {
            Self::Landscape16x9 => Resolution {
                width: 1920,
                height: 1080,
            },
            Self::Portrait9x16 => Resolution {
                width: 1080,
                height: 1920,
            },
            Self::Square1x1 => Resolution {
                width: 1080,
                height: 1080,
            },
        }
    }

    /// Name of the format (CLI, MCP; the app translates it).
    pub const fn label(self) -> &'static str {
        match self {
            Self::Landscape16x9 => "Landscape",
            Self::Portrait9x16 => "Vertical short",
            Self::Square1x1 => "Square",
        }
    }

    /// Format closest to a `width`×`height` image.
    pub fn nearest(width: u32, height: u32) -> Self {
        let ratio = f64::from(width) / f64::from(height.max(1));
        if ratio > 1.3 {
            Self::Landscape16x9
        } else if ratio < 0.77 {
            Self::Portrait9x16
        } else {
            Self::Square1x1
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

/// Framing of the take when its format differs from the project's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Framing {
    /// Whole image, black bars around.
    #[default]
    Fit,
    /// Frame filled, the image overflows and is cropped (16:9 take in a
    /// vertical video).
    Fill,
}

/// Shift of the cropped frame, as a percentage of the margin: −50 (left
/// or top edge) to 50 (right or bottom edge).
pub const PAN_LIMIT: i8 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSettings {
    pub aspect: AspectRatio,
    pub resolution: Resolution,
    pub frame_rate: FrameRate,
    #[serde(default)]
    pub framing: Framing,
    /// See [`PAN_LIMIT`].
    #[serde(default)]
    pub pan: i8,
}

impl ProjectSettings {
    /// Same settings in another format (standard resolution of the format).
    #[must_use]
    pub const fn with_aspect(mut self, aspect: AspectRatio) -> Self {
        self.aspect = aspect;
        self.resolution = aspect.resolution();
        self
    }

    /// Frame shift bounded to [`PAN_LIMIT`].
    pub fn pan(&self) -> i8 {
        self.pan.clamp(-PAN_LIMIT, PAN_LIMIT)
    }
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self {
            aspect: AspectRatio::Landscape16x9,
            resolution: Resolution {
                width: 1920,
                height: 1080,
            },
            frame_rate: FrameRate::FPS_30,
            framing: Framing::Fit,
            pan: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    schema_version: u32,
    id: ProjectId,
    name: String,
    settings: ProjectSettings,
    blocks: Vec<Block>,
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            id: ProjectId::new(),
            name: name.into(),
            settings: ProjectSettings::default(),
            blocks: Vec::new(),
        }
    }

    /// Project with existing blocks, after validation.
    pub fn with_blocks(
        name: impl Into<String>,
        settings: ProjectSettings,
        blocks: Vec<Block>,
    ) -> Result<Self, EditError> {
        let project = Self {
            schema_version: SCHEMA_VERSION,
            id: ProjectId::new(),
            name: name.into(),
            settings,
            blocks,
        };
        project.validate()?;
        Ok(project)
    }

    pub fn from_json(json: &str) -> Result<Self, ProjectError> {
        let project: Self = serde_json::from_str(json)?;
        if project.schema_version != SCHEMA_VERSION {
            return Err(ProjectError::UnsupportedSchema {
                found: project.schema_version,
                expected: SCHEMA_VERSION,
            });
        }
        project.validate()?;
        Ok(project)
    }

    pub fn to_json(&self) -> Result<String, ProjectError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub const fn id(&self) -> ProjectId {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn settings(&self) -> &ProjectSettings {
        &self.settings
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    pub fn block(&self, id: BlockId) -> Option<&Block> {
        self.blocks.iter().find(|b| b.id == id)
    }

    /// Total duration covered by the blocks.
    pub fn duration(&self) -> Flicks {
        self.blocks.last().map_or(Flicks::ZERO, |b| b.range.end())
    }

    fn validate(&self) -> Result<(), EditError> {
        let mut seen = HashSet::with_capacity(self.blocks.len());
        for block in &self.blocks {
            if !seen.insert(block.id) {
                return Err(EditError::DuplicateBlock(block.id));
            }
        }
        for pair in self.blocks.windows(2) {
            if let [a, b] = pair
                && a.range.end() != b.range.start()
            {
                return Err(EditError::NotContiguous);
            }
        }
        for block in &self.blocks {
            if block.moments.iter().any(|m| !block.range.contains(m.at)) {
                return Err(EditError::MomentOutsideBlock);
            }
            if block
                .moments
                .windows(2)
                .any(|w| matches!(w, [a, b] if a.at >= b.at))
            {
                return Err(EditError::DuplicateMoment);
            }
        }
        Ok(())
    }

    fn index_of(&self, id: BlockId) -> Result<usize, EditError> {
        self.blocks
            .iter()
            .position(|b| b.id == id)
            .ok_or(EditError::BlockNotFound(id))
    }

    fn block_mut(&mut self, id: BlockId) -> Result<&mut Block, EditError> {
        self.blocks
            .iter_mut()
            .find(|b| b.id == id)
            .ok_or(EditError::BlockNotFound(id))
    }

    // --- Elementary operations. Each returns what is needed to undo it. ---

    pub(crate) fn set_description(
        &mut self,
        id: BlockId,
        text: String,
    ) -> Result<String, EditError> {
        let block = self.block_mut(id)?;
        Ok(mem::replace(&mut block.description, text))
    }

    pub(crate) fn set_kind(
        &mut self,
        id: BlockId,
        kind: BlockKind,
    ) -> Result<BlockKind, EditError> {
        let block = self.block_mut(id)?;
        Ok(mem::replace(&mut block.kind, kind))
    }

    pub(crate) fn set_transition(
        &mut self,
        id: BlockId,
        transition: Transition,
    ) -> Result<Transition, EditError> {
        let block = self.block_mut(id)?;
        Ok(mem::replace(&mut block.transition_after, transition))
    }

    pub(crate) fn set_settings(&mut self, settings: ProjectSettings) -> ProjectSettings {
        mem::replace(&mut self.settings, settings)
    }

    pub(crate) fn set_broll(
        &mut self,
        id: BlockId,
        broll: Option<Broll>,
    ) -> Result<Option<Broll>, EditError> {
        let block = self.block_mut(id)?;
        Ok(mem::replace(&mut block.broll, broll))
    }

    pub(crate) fn set_cutout(&mut self, id: BlockId, cutout: bool) -> Result<bool, EditError> {
        let block = self.block_mut(id)?;
        Ok(mem::replace(&mut block.cutout, cutout))
    }

    pub(crate) fn add_moment(&mut self, id: BlockId, moment: Moment) -> Result<(), EditError> {
        let block = self.block_mut(id)?;
        if !block.range.contains(moment.at) {
            return Err(EditError::MomentOutsideBlock);
        }
        match block.moments.binary_search_by_key(&moment.at, |m| m.at) {
            Ok(_) => Err(EditError::DuplicateMoment),
            Err(i) => {
                block.moments.insert(i, moment);
                Ok(())
            }
        }
    }

    pub(crate) fn remove_moment(&mut self, id: BlockId, at: Flicks) -> Result<Moment, EditError> {
        let block = self.block_mut(id)?;
        let i = block
            .moments
            .binary_search_by_key(&at, |m| m.at)
            .map_err(|_| EditError::MomentNotFound)?;
        Ok(block.moments.remove(i))
    }

    pub(crate) fn set_moment_note(
        &mut self,
        id: BlockId,
        at: Flicks,
        note: String,
    ) -> Result<String, EditError> {
        let block = self.block_mut(id)?;
        let moment = block
            .moments
            .iter_mut()
            .find(|m| m.at == at)
            .ok_or(EditError::MomentNotFound)?;
        Ok(mem::replace(&mut moment.note, note))
    }

    /// Splits the block in two at `at`. The second half keeps the kind, the
    /// cutout and the b-roll shot (resuming where the first one stops), gets
    /// an empty description and inherits the transition; the first one gets
    /// a hard cut.
    pub(crate) fn split(
        &mut self,
        id: BlockId,
        at: Flicks,
        new_id: BlockId,
    ) -> Result<(), EditError> {
        if self.block(new_id).is_some() {
            return Err(EditError::DuplicateBlock(new_id));
        }
        let index = self.index_of(id)?;
        let block = self
            .blocks
            .get_mut(index)
            .ok_or(EditError::BlockNotFound(id))?;
        let (first, second) = block
            .range
            .split_at(at)
            .map_err(|_| EditError::SplitOutsideBlock)?;
        let cut = block.moments.partition_point(|m| m.at < at);
        let broll = match &block.broll {
            Some(b) => Some(Broll {
                file: b.file.clone(),
                start: b.start.checked_add(first.duration())?,
            }),
            None => None,
        };
        let tail = Block {
            id: new_id,
            range: second,
            kind: block.kind,
            description: String::new(),
            transition_after: mem::take(&mut block.transition_after),
            moments: block.moments.split_off(cut),
            broll,
            cutout: block.cutout,
        };
        block.range = first;
        self.blocks.insert(index.saturating_add(1), tail);
        Ok(())
    }

    /// Merges the block with the next one. Returns what is needed to undo the
    /// merge: the original description and transition of the first block, and the second block.
    pub(crate) fn merge_with_next(
        &mut self,
        id: BlockId,
    ) -> Result<(String, Transition, Block), EditError> {
        let index = self.index_of(id)?;
        let next_index = index.saturating_add(1);
        let next = self
            .blocks
            .get(next_index)
            .cloned()
            .ok_or(EditError::NoNextBlock)?;
        let first = self.blocks.get(index).ok_or(EditError::BlockNotFound(id))?;
        if first.range.end() != next.range.start() {
            return Err(EditError::NotContiguous);
        }
        let merged_range = TimeRange::new(first.range.start(), next.range.end())?;

        // Everything is validated: we can write.
        self.blocks.remove(next_index);
        let first = self
            .blocks
            .get_mut(index)
            .ok_or(EditError::BlockNotFound(id))?;
        let old_description = first.description.clone();
        let old_transition =
            mem::replace(&mut first.transition_after, next.transition_after.clone());
        first.range = merged_range;
        first.moments.extend(next.moments.iter().cloned());
        if !next.description.is_empty() {
            if !first.description.is_empty() {
                first.description.push('\n');
            }
            first.description.push_str(&next.description);
        }
        Ok((old_description, old_transition, next))
    }

    /// Exact inverse of [`Project::merge_with_next`].
    pub(crate) fn unmerge(
        &mut self,
        id: BlockId,
        first_description: String,
        first_transition: Transition,
        second: Block,
    ) -> Result<(), EditError> {
        if self.block(second.id).is_some() {
            return Err(EditError::DuplicateBlock(second.id));
        }
        let index = self.index_of(id)?;
        let first = self
            .blocks
            .get_mut(index)
            .ok_or(EditError::BlockNotFound(id))?;
        if second.range.end() != first.range.end() {
            return Err(EditError::NotContiguous);
        }
        let (head, _) = first
            .range
            .split_at(second.range.start())
            .map_err(|_| EditError::SplitOutsideBlock)?;
        first.range = head;
        first.moments.retain(|m| m.at < second.range.start());
        first.description = first_description;
        first.transition_after = first_transition;
        self.blocks.insert(index.saturating_add(1), second);
        Ok(())
    }
}
