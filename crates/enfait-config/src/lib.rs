//! Configuration shared by the app, the CLI and the MCP server.
//!
//! - [`Settings`]: user preferences (language, theme, local model…), stored
//!   as JSON in the platform config directory.
//! - [`secrets`]: API keys, kept in the system keychain (macOS Keychain,
//!   Windows Credential Manager, Secret Service on Linux) with a private
//!   file as fallback; environment variables always win.
//! - [`paths`]: cache, config and data directories.
//! - [`process`]: finding executables and spawning them without a console
//!   window on Windows.

pub mod paths;
pub mod process;
pub mod secrets;
mod settings;

pub use secrets::Secret;
pub use settings::{Accent, CloseAction, DEFAULT_LOCAL_URL, Language, Settings, ThemeId};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("no home directory found")]
    NoHome,
    #[error("{path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Json {
        path: std::path::PathBuf,
        source: serde_json::Error,
    },
    #[error("keychain: {0}")]
    Keychain(String),
}

pub type Result<T, E = ConfigError> = std::result::Result<T, E>;

static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Changes every time a key or a model setting changes in this process:
/// clients built from the configuration compare it to know when to be
/// rebuilt.
pub fn generation() -> u64 {
    GENERATION.load(std::sync::atomic::Ordering::Acquire)
}

/// Marks the configuration as changed (see [`generation`]).
pub fn bump_generation() {
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
}

/// Writes `bytes` to `path` atomically (temporary file, then rename).
pub(crate) fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let io = |source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)
}
