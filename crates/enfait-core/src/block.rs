//! Storyboard blocks: a scene of the video.

use crate::id::BlockId;
use crate::time::{Flicks, TimeRange};
use serde::{Deserialize, Serialize};

/// What is shown during the block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    /// The rush as is, no animation. Does not go through a model.
    Rush,
    /// Animation only, no face cam.
    #[default]
    Animation,
    /// Face cam with an animation on top.
    Mixed,
    /// Effect applied to the face cam.
    Effect,
    /// Illustration shot (b-roll): a video or an image of the library,
    /// edited without a model.
    Broll,
}

/// Passage from a block to the next.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(tag = "type", content = "description", rename_all = "snake_case")]
pub enum Transition {
    /// Hard cut.
    #[default]
    Cut,
    /// The agent chooses the transition.
    Auto,
    /// Transition described by the user.
    Described(String),
}

/// A precise moment: what must happen at a time of the voice ("on
/// 'switchman', the switch flips").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Moment {
    /// Time, within the block interval.
    pub at: Flicks,
    /// Word spoken at that time: readable landmark.
    pub word: String,
    /// What happens.
    pub note: String,
}

/// The shot of a B-roll block: a library file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Broll {
    /// File name in the library.
    pub file: String,
    /// Time of the video where the shot starts (zero for an image).
    #[serde(default)]
    pub start: Flicks,
}

/// A scene: an interval of the voice, what is shown, and the transition after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub id: BlockId,
    pub range: TimeRange,
    pub kind: BlockKind,
    /// Free description of what must happen on screen.
    pub description: String,
    pub transition_after: Transition,
    /// Precise moments, sorted by time, all within `range`.
    #[serde(default)]
    pub moments: Vec<Moment>,
    /// Shot shown by a B-roll block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broll: Option<Broll>,
    /// Mixed block: the filmed person is cut out, the animation can pass
    /// behind them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cutout: bool,
}

impl Block {
    pub fn new(range: TimeRange, kind: BlockKind) -> Self {
        Self {
            id: BlockId::new(),
            range,
            kind,
            description: String::new(),
            transition_after: Transition::Cut,
            moments: Vec::new(),
            broll: None,
            cutout: false,
        }
    }
}
