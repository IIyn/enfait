//! API keys.
//!
//! Lookup order: environment variable, then the system keychain, then
//! `secrets.json` in the config directory (only written when no keychain
//! is available, with owner-only permissions on Unix). Values are cached
//! for the life of the process.

use crate::{ConfigError, Result, paths, settings::env_nonempty, write_atomic};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// Keychain service name.
const SERVICE: &str = "enfait";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Secret {
    Anthropic,
    Pexels,
    Pixabay,
    Freesound,
    /// Optional key of the local model server.
    Local,
}

impl Secret {
    pub const ALL: [Self; 5] = [
        Self::Anthropic,
        Self::Pexels,
        Self::Pixabay,
        Self::Freesound,
        Self::Local,
    ];

    /// Environment variable that overrides the stored value.
    pub const fn env_var(self) -> &'static str {
        match self {
            Self::Anthropic => "ANTHROPIC_API_KEY",
            Self::Pexels => "PEXELS_API_KEY",
            Self::Pixabay => "PIXABAY_API_KEY",
            Self::Freesound => "FREESOUND_API_KEY",
            Self::Local => "ENFAIT_LOCAL_KEY",
        }
    }

    /// Account name in the keychain and key in `secrets.json`.
    const fn account(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Pexels => "pexels",
            Self::Pixabay => "pixabay",
            Self::Freesound => "freesound",
            Self::Local => "local",
        }
    }
}

/// Where a key comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Env,
    Keychain,
    File,
}

static CACHE: Mutex<BTreeMap<Secret, Option<(String, Source)>>> = Mutex::new(BTreeMap::new());

/// The key, if one is set anywhere.
pub fn get(secret: Secret) -> Option<String> {
    lookup(secret).map(|(value, _)| value)
}

/// Where the key comes from, if it is set.
pub fn source(secret: Secret) -> Option<Source> {
    lookup(secret).map(|(_, source)| source)
}

pub fn is_set(secret: Secret) -> bool {
    lookup(secret).is_some()
}

fn lookup(secret: Secret) -> Option<(String, Source)> {
    if let Some(value) = env_nonempty(secret.env_var()) {
        return Some((value, Source::Env));
    }
    if let Some(cached) = CACHE.lock().ok().and_then(|c| c.get(&secret).cloned()) {
        return cached;
    }
    let found = read_keychain(secret)
        .map(|v| (v, Source::Keychain))
        .or_else(|| read_file(secret).map(|v| (v, Source::File)));
    if let Ok(mut cache) = CACHE.lock() {
        cache.insert(secret, found.clone());
    }
    found
}

/// Stores (or, with `None` or an empty value, removes) a key. Returns
/// where it was stored.
pub fn set(secret: Secret, value: Option<&str>) -> Result<Option<Source>> {
    let value = value.map(str::trim).filter(|v| !v.is_empty());
    let stored = match keychain_entry(secret) {
        Ok(entry) => {
            let result = match value {
                Some(v) => entry.set_password(v),
                None => match entry.delete_credential() {
                    Err(keyring::Error::NoEntry) => Ok(()),
                    other => other,
                },
            };
            match result {
                Ok(()) => {
                    // A key saved to the file earlier would shadow nothing
                    // but would linger in clear text: remove it.
                    write_file(secret, None)?;
                    value.map(|_| Source::Keychain)
                }
                Err(e) => {
                    tracing::warn!("keychain unavailable ({e}), storing the key in a file");
                    write_file(secret, value)?;
                    value.map(|_| Source::File)
                }
            }
        }
        Err(e) => {
            tracing::warn!("keychain unavailable ({e}), storing the key in a file");
            write_file(secret, value)?;
            value.map(|_| Source::File)
        }
    };
    if let Ok(mut cache) = CACHE.lock() {
        cache.insert(
            secret,
            value.zip(stored).map(|(v, source)| (v.to_owned(), source)),
        );
    }
    crate::bump_generation();
    Ok(stored)
}

/// The system keychain answers (for the settings screen).
pub fn keychain_available() -> bool {
    keyring::Entry::store_status().is_ok()
}

fn keychain_entry(secret: Secret) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, secret.account()).map_err(|e| ConfigError::Keychain(e.to_string()))
}

fn read_keychain(secret: Secret) -> Option<String> {
    let entry = keychain_entry(secret).ok()?;
    match entry.get_password() {
        Ok(value) if !value.trim().is_empty() => Some(value),
        Ok(_) | Err(keyring::Error::NoEntry) => None,
        Err(e) => {
            tracing::debug!("keychain: {e}");
            None
        }
    }
}

fn secrets_file() -> std::path::PathBuf {
    paths::config_dir().join("secrets.json")
}

fn load_file() -> BTreeMap<String, String> {
    std::fs::read(secrets_file())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn read_file(secret: Secret) -> Option<String> {
    load_file()
        .remove(secret.account())
        .filter(|v| !v.trim().is_empty())
}

fn write_file(secret: Secret, value: Option<&str>) -> Result<()> {
    let path = secrets_file();
    let mut all = load_file();
    let changed = match value {
        Some(v) => {
            all.insert(secret.account().to_owned(), v.to_owned())
                .as_deref()
                != Some(v)
        }
        None => all.remove(secret.account()).is_some(),
    };
    if !changed {
        return Ok(());
    }
    if all.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(ConfigError::Io { path, source: e })
            }
            _ => Ok(()),
        };
    }
    let json = serde_json::to_vec_pretty(&all).map_err(|source| ConfigError::Json {
        path: path.clone(),
        source,
    })?;
    write_atomic(&path, &json)?;
    restrict(&path)
}

/// Owner-only permissions on the fallback file.
#[cfg(unix)]
fn restrict(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|source| {
        ConfigError::Io {
            path: path.to_path_buf(),
            source,
        }
    })
}

/// On Windows, `%APPDATA%` is already private to the user.
#[cfg(not(unix))]
fn restrict(_: &std::path::Path) -> Result<()> {
    Ok(())
}
