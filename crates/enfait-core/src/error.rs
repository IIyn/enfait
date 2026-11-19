//! Model errors.

use crate::id::BlockId;

/// Error of a time computation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TimeError {
    #[error("overflow in a time computation")]
    Overflow,
    #[error("cadence invalide : {num}/{den}")]
    InvalidFrameRate { num: u32, den: u32 },
    #[error("zero sample rate")]
    InvalidSampleRate,
    #[error("empty or reversed interval")]
    EmptyRange,
    #[error("negative time")]
    Negative,
}

/// Error of a project edit. A failing edit changes nothing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    #[error("block not found: {0}")]
    BlockNotFound(BlockId),
    #[error("block id already used: {0}")]
    DuplicateBlock(BlockId),
    #[error("the cut point must be strictly inside the block")]
    SplitOutsideBlock,
    #[error("no next block to merge")]
    NoNextBlock,
    #[error("the blocks do not follow each other without a gap")]
    NotContiguous,
    #[error("the moment must fall inside the block")]
    MomentOutsideBlock,
    #[error("a moment already exists at this time")]
    DuplicateMoment,
    #[error("moment not found")]
    MomentNotFound,
    #[error(transparent)]
    Time(#[from] TimeError),
}

/// Error loading or validating a project.
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("unsupported format version {found} (expected: {expected})")]
    UnsupportedSchema { found: u32, expected: u32 },
    #[error("projet invalide : {0}")]
    Invalid(#[from] EditError),
    #[error("JSON invalide : {0}")]
    Json(#[from] serde_json::Error),
}
