//! enfait's media engine.
//!
//! ffmpeg and ffprobe run as subprocesses: no C binding, and the same
//! binary serves analysis, proxies, audio and decoding.
//!
//! - [`probe`]: duration, video and audio streams of a file;
//! - [`cache`]: preparation of a media (proxy, audio, waveform), cached on
//!   disk;
//! - [`audio`] and [`waveform`]: samples in memory and peaks;
//! - [`video`]: decoding proxy frames, as the clock asks;
//! - [`transport`]: shared playback clock and J/K/L commands;
//! - [`output`]: audio output (cpal) driving the clock.

pub mod audio;
pub mod cache;
pub mod edl;
mod error;
mod ffmpeg;
pub mod fx;
pub mod output;
pub mod probe;
pub mod record;
pub mod transport;
pub mod video;
pub mod waveform;

pub use audio::AudioBuffer;
pub use cache::{MediaCache, Prepared, Stage, VideoSource};
pub use edl::Edl;
pub use error::MediaError;
pub use output::Playback;
pub use probe::{AudioInfo, MediaInfo, VideoInfo};
pub use transport::{Clock, Command};
pub use video::{Frame, VideoDecoder};
pub use waveform::Peaks;
