//! enfait's animated compositions.
//!
//! A composition is an HTML document that follows the [contract]
//! (`window.enfait = { duration, seek(t) }`). This crate prepares the page
//! ([`composition`]), installs the [`library`] (fonts, anime.js), renders it
//! frame by frame in Chromium ([`render`]) and checks it ([`job`]).
//!
//! [contract]: composition::CONTRACT

pub mod browser;
pub mod composition;
pub mod error;
pub mod job;
pub mod library;
pub mod pool;
pub mod render;
pub mod still;
pub mod user;

pub use browser::{ensure_browser, installed_browser};
pub use error::MotionError;
pub use job::{CheckJob, FrameJob, FrameOutcome, Report, Sample, frame_name};
pub use pool::{Lease, RendererPool};
pub use render::{Gpu, Renderer};
pub use still::{Still, StillLayer, render_still};
