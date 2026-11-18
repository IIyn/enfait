//! Moteur média d'enfait.
//!
//! ffmpeg et ffprobe tournent en sous-processus : pas de liaison C, et le
//! même binaire sert à l'analyse, aux proxys, à l'audio et au décodage.
//!
//! - [`probe`] : durée, flux vidéo et audio d'un fichier ;
//! - [`cache`] : préparation d'un média (proxy, audio, forme d'onde), mise
//!   en cache sur disque ;
//! - [`audio`] et [`waveform`] : échantillons en mémoire et crêtes ;
//! - [`video`] : décodage des images du proxy, à la demande de l'horloge ;
//! - [`transport`] : horloge de lecture partagée et commandes J/K/L ;
//! - [`output`] : sortie audio (cpal) qui fait avancer l'horloge.

pub mod audio;
pub mod cache;
pub mod edl;
mod error;
mod ffmpeg;
pub mod fx;
pub mod output;
pub mod probe;
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
