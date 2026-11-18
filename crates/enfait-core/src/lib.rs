//! Modèle de projet d'enfait.
//!
//! Ce crate ne dépend d'aucune couche média, IA ou interface : il porte le
//! temps ([`time`]), les blocs du storyboard ([`block`]), le projet
//! ([`project`]), la transcription ([`transcript`]) et l'historique annuler /
//! rétablir ([`history`]).

#![warn(clippy::arithmetic_side_effects)]

pub mod block;
pub mod error;
pub mod finish;
pub mod history;
pub mod id;
pub mod license;
pub mod look;
pub mod project;
pub mod time;
pub mod transcript;

pub use block::{Block, BlockKind, Broll, Moment, Transition};
pub use error::{EditError, ProjectError, TimeError};
pub use finish::{CueId, Ducking, Finish, FinishHistory, Music, Placed, Sfx};
pub use history::{Edit, History};
pub use id::{BlockId, ProjectId};
pub use license::{License, Origin, Terms};
pub use look::{Asset, AssetKind, FX_PRESETS, FxPreset, Look, PostFx, StyleChoice};
pub use project::{AspectRatio, Framing, PAN_LIMIT, Project, ProjectSettings, Resolution};
pub use time::{Flicks, FrameRate, TimeRange, Timecode};
pub use transcript::{Transcript, Word};
