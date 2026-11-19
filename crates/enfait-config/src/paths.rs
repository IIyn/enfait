//! Where enfait keeps its files, per platform (`directories`):
//!
//! | | Linux | macOS | Windows |
//! |---|---|---|---|
//! | config | `~/.config/enfait` | `~/Library/Application Support/enfait` | `%APPDATA%\enfait\config` |
//! | data | `~/.local/share/enfait` | `~/Library/Application Support/enfait` | `%APPDATA%\enfait\data` |
//! | cache | `~/.cache/enfait` | `~/Library/Caches/enfait` | `%LOCALAPPDATA%\enfait\cache` |

use std::path::PathBuf;

fn project() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from("", "", "enfait")
}

/// Fallback when no home directory exists (CI containers, services).
fn fallback() -> PathBuf {
    std::env::temp_dir().join("enfait")
}

pub fn config_dir() -> PathBuf {
    project().map_or_else(fallback, |d| d.config_dir().to_path_buf())
}

pub fn data_dir() -> PathBuf {
    project().map_or_else(fallback, |d| d.data_dir().to_path_buf())
}

pub fn cache_dir() -> PathBuf {
    project().map_or_else(fallback, |d| d.cache_dir().to_path_buf())
}

/// `settings.json`.
pub fn settings_file() -> PathBuf {
    config_dir().join("settings.json")
}

/// Downloaded models (whisper, cutout).
pub fn models_dir() -> PathBuf {
    cache_dir().join("models")
}

/// Cutout models. The folder used to be called `detourage`: an existing
/// one is renamed rather than downloaded again.
pub fn cutout_models_dir() -> PathBuf {
    let dir = models_dir().join("cutout");
    let old = models_dir().join("detourage");
    if !dir.exists() && old.is_dir() {
        let _ = std::fs::rename(&old, &dir);
    }
    dir
}

/// Prepared media (proxies, audio, waveforms), shared by the app and the CLI.
pub fn media_cache() -> PathBuf {
    cache_dir().join("media")
}

/// `chrome-headless-shell`, downloaded on first use.
pub fn chromium_dir() -> PathBuf {
    cache_dir().join("chromium")
}

/// Browser profile used by the renderer.
pub fn browser_profile() -> PathBuf {
    cache_dir().join("profile")
}

/// Default session folder (script, storyboard, studio).
pub fn default_session() -> PathBuf {
    data_dir().join("session")
}
