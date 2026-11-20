//! User preferences, stored in `settings.json` (see [`crate::paths`]).
//!
//! Every field has a default, so an old or partial file still loads; an
//! unreadable file is reported and replaced by the defaults.

use crate::{ConfigError, Result, paths, write_atomic};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Interface language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    #[default]
    En,
    Fr,
}

impl Language {
    pub const ALL: [Self; 2] = [Self::En, Self::Fr];

    /// Locale code used by the translation tables.
    pub const fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Fr => "fr",
        }
    }

    /// Name of the language in that language.
    pub const fn native_name(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::Fr => "Français",
        }
    }

    /// Language of the system (`fr-FR` → French), English otherwise.
    pub fn from_system() -> Self {
        let locale = sys_locale().unwrap_or_default().to_ascii_lowercase();
        if locale.starts_with("fr") {
            Self::Fr
        } else {
            Self::En
        }
    }
}

/// Reads the system locale from the usual environment variables; on
/// Windows and macOS, where they are rarely set, the app falls back to
/// English and the onboarding lets the user pick.
fn sys_locale() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|var| std::env::var(var).ok())
        .find(|v| !v.is_empty() && v != "C" && v != "POSIX")
}

/// Colour theme of the interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeId {
    /// Dark or light, following the system appearance.
    #[default]
    System,
    Dark,
    Light,
    CatppuccinLatte,
    CatppuccinFrappe,
    CatppuccinMacchiato,
    CatppuccinMocha,
    GruvboxDark,
    Nord,
}

impl ThemeId {
    pub const ALL: [Self; 9] = [
        Self::System,
        Self::Dark,
        Self::Light,
        Self::CatppuccinLatte,
        Self::CatppuccinFrappe,
        Self::CatppuccinMacchiato,
        Self::CatppuccinMocha,
        Self::GruvboxDark,
        Self::Nord,
    ];
}

/// Accent colour: main action, playhead, selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Accent {
    Chartreuse,
    Mint,
    Sky,
    #[default]
    Amber,
    Rose,
    Lavender,
}

impl Accent {
    pub const ALL: [Self; 6] = [
        Self::Chartreuse,
        Self::Mint,
        Self::Sky,
        Self::Amber,
        Self::Rose,
        Self::Lavender,
    ];
}

/// What closing the main window does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseAction {
    /// Keep running in the system tray (jobs and exports go on).
    #[default]
    Tray,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub language: Language,
    pub theme: ThemeId,
    pub accent: Accent,
    pub close_action: CloseAction,
    /// The onboarding has been completed (or skipped).
    pub onboarded: bool,
    /// Desktop notifications when work finishes in the background.
    pub notifications: bool,
    /// OpenAI-compatible server for local models (Ollama by default).
    pub local_url: String,
    /// Local model name (`qwen3-coder:30b`…); empty: not configured.
    pub local_model: String,
    /// Recording with the prompter also films with the camera.
    pub record_camera: bool,
    /// Microphone for recording (an audio device id); `None`: the system's.
    pub microphone: Option<String>,
    /// Camera for filming (a device id); `None`: the first one.
    pub camera: Option<String>,
    /// Sizes of the panels resized with the mouse.
    pub panels: PanelSizes,
    /// The player decodes the original file at its own size rather than
    /// the proxy.
    pub player_native: bool,
}

/// Sizes of the panels the user resizes with the mouse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelSizes {
    /// Width of the left column (style, library), in logical pixels.
    pub sidebar: u32,
    /// Width of the right panel (inspector, retouches, finish), in logical
    /// pixels.
    pub inspector: u32,
    /// Height of the player, in thousandths of the height of the screen
    /// under the header.
    pub player: u32,
    /// Height of the "on screen" field of the storyboard, in logical
    /// pixels.
    pub description: u32,
}

impl PanelSizes {
    pub const SIDEBAR: (u32, u32) = (200, 560);
    pub const INSPECTOR: (u32, u32) = (260, 720);
    pub const PLAYER: (u32, u32) = (200, 800);
    pub const DESCRIPTION: (u32, u32) = (80, 720);

    /// Sizes within their bounds (a hand-edited file stays usable).
    #[must_use]
    pub fn clamped(self) -> Self {
        let clamp = |v: u32, (min, max): (u32, u32)| v.clamp(min, max);
        Self {
            sidebar: clamp(self.sidebar, Self::SIDEBAR),
            inspector: clamp(self.inspector, Self::INSPECTOR),
            player: clamp(self.player, Self::PLAYER),
            description: clamp(self.description, Self::DESCRIPTION),
        }
    }
}

impl Default for PanelSizes {
    fn default() -> Self {
        Self {
            sidebar: 280,
            inspector: 340,
            player: 500,
            description: 180,
        }
    }
}

/// Ollama's OpenAI-compatible endpoint.
pub const DEFAULT_LOCAL_URL: &str = "http://localhost:11434/v1";

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: Language::from_system(),
            theme: ThemeId::default(),
            accent: Accent::default(),
            close_action: CloseAction::default(),
            onboarded: false,
            notifications: true,
            local_url: DEFAULT_LOCAL_URL.to_owned(),
            local_model: String::new(),
            record_camera: false,
            microphone: None,
            camera: None,
            panels: PanelSizes::default(),
            player_native: false,
        }
    }
}

impl Settings {
    /// Settings of the user (defaults if the file is missing or unreadable).
    pub fn load() -> Self {
        let path = paths::settings_file();
        match Self::load_from(&path) {
            Ok(settings) => settings.unwrap_or_default(),
            Err(e) => {
                tracing::warn!("{e}; using default settings");
                Self::default()
            }
        }
    }

    /// `Ok(None)` if the file does not exist.
    pub fn load_from(path: &Path) -> Result<Option<Self>> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(ConfigError::Io {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|source| ConfigError::Json {
                path: path.to_path_buf(),
                source,
            })
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&paths::settings_file())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(|source| ConfigError::Json {
            path: path.to_path_buf(),
            source,
        })?;
        write_atomic(path, &json)
    }

    /// Local model URL, `ENFAIT_LOCAL_URL` first.
    pub fn local_url(&self) -> String {
        env_nonempty("ENFAIT_LOCAL_URL").unwrap_or_else(|| {
            if self.local_url.trim().is_empty() {
                DEFAULT_LOCAL_URL.to_owned()
            } else {
                self.local_url.trim().to_owned()
            }
        })
    }

    /// Local model name, `ENFAIT_LOCAL_MODEL` first.
    pub fn local_model(&self) -> Option<String> {
        env_nonempty("ENFAIT_LOCAL_MODEL").or_else(|| {
            let model = self.local_model.trim();
            (!model.is_empty()).then(|| model.to_owned())
        })
    }
}

pub(crate) fn env_nonempty(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|v| !v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn round_trips_and_tolerates_missing_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(Settings::load_from(&path).unwrap(), None);

        let settings = Settings {
            language: Language::Fr,
            theme: ThemeId::CatppuccinMocha,
            accent: Accent::Sky,
            close_action: CloseAction::Quit,
            onboarded: true,
            notifications: false,
            local_url: "http://box:8080/v1".into(),
            local_model: "qwen".into(),
            record_camera: true,
            microphone: Some("pulseaudio:mic".into()),
            camera: Some("/dev/video2".into()),
            panels: PanelSizes {
                sidebar: 300,
                inspector: 420,
                player: 640,
                description: 260,
            },
            player_native: true,
        };
        settings.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path).unwrap(), Some(settings));

        std::fs::write(&path, r#"{ "theme": "nord" }"#).unwrap();
        let partial = Settings::load_from(&path).unwrap().unwrap();
        assert_eq!(partial.theme, ThemeId::Nord);
        assert_eq!(partial.panels, PanelSizes::default());
        assert_eq!(partial.local_url, DEFAULT_LOCAL_URL);
        assert!(!partial.onboarded);
        assert!(partial.notifications);
        assert!(!partial.record_camera);
        assert_eq!(partial.microphone, None);
    }

    #[test]
    fn panel_sizes_are_clamped() {
        let wild = PanelSizes {
            sidebar: 0,
            inspector: 9_999,
            player: 1_000,
            description: 5,
        }
        .clamped();
        assert_eq!(wild.sidebar, PanelSizes::SIDEBAR.0);
        assert_eq!(wild.inspector, PanelSizes::INSPECTOR.1);
        assert_eq!(wild.player, PanelSizes::PLAYER.1);
        assert_eq!(wild.description, PanelSizes::DESCRIPTION.0);
        assert_eq!(PanelSizes::default().clamped(), PanelSizes::default());
    }
}
