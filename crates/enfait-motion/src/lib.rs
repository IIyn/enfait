//! Compositions animées d'enfait.
//!
//! Une composition est un document HTML qui respecte le [contrat]
//! (`window.enfait = { duration, seek(t) }`). Ce crate prépare la page
//! ([`composition`]), installe la [`library`] (polices, anime.js), la rend image
//! par image dans Chromium ([`render`]) et la vérifie ([`job`]).
//!
//! [contrat]: composition::CONTRACT

pub mod browser;
pub mod composition;
pub mod error;
pub mod job;
pub mod library;
pub mod pool;
pub mod render;
pub mod user;

pub use browser::{ensure_browser, installed_browser};
pub use error::MotionError;
pub use job::{CheckJob, FrameJob, FrameOutcome, Report, Sample, frame_name};
pub use pool::{Lease, RendererPool};
pub use render::{Gpu, Renderer};
