//! Installation de `chrome-headless-shell` depuis Chrome for Testing.
//!
//! On lit l'index officiel des versions stables, on télécharge l'archive de
//! la plateforme et on l'extrait dans `dir/<version>/`. Une version déjà
//! installée n'est pas retéléchargée.

use crate::error::MotionError;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const INDEX: &str = "https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json";
const PRODUCT: &str = "chrome-headless-shell";

#[derive(Deserialize)]
struct Index {
    channels: HashMap<String, Channel>,
}

#[derive(Deserialize)]
struct Channel {
    version: String,
    downloads: HashMap<String, Vec<Download>>,
}

#[derive(Deserialize)]
struct Download {
    platform: String,
    url: String,
}

/// Plateforme au sens de Chrome for Testing.
fn platform() -> Result<&'static str, MotionError> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("linux64"),
        ("macos", "aarch64") => Ok("mac-arm64"),
        ("macos", "x86_64") => Ok("mac-x64"),
        ("windows", "x86_64") => Ok("win64"),
        ("windows", "x86") => Ok("win32"),
        (os, arch) => Err(MotionError::Fetch(format!(
            "pas de chrome-headless-shell pour {os}/{arch}"
        ))),
    }
}

fn executable(version_dir: &Path, platform: &str) -> PathBuf {
    let name = if platform.starts_with("win") {
        "chrome-headless-shell.exe"
    } else {
        "chrome-headless-shell"
    };
    version_dir.join(format!("{PRODUCT}-{platform}")).join(name)
}

/// Path of the installed `chrome-headless-shell` in `dir`, without
/// downloading anything.
pub fn installed_browser(dir: &Path) -> Option<PathBuf> {
    installed(dir, platform().ok()?)
}

/// Latest version installed in `dir`, if any.
fn installed(dir: &Path, platform: &str) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| executable(&e.path(), platform))
        .filter(|p| p.is_file())
        .collect();
    found.sort();
    found.pop()
}

/// Renvoie le chemin de `chrome-headless-shell`, en l'installant dans `dir`
/// au premier appel.
pub async fn ensure_browser(dir: &Path) -> Result<PathBuf, MotionError> {
    let platform = platform()?;
    if let Some(path) = installed(dir, platform) {
        return Ok(path);
    }
    let client = reqwest::Client::builder()
        .user_agent(concat!("enfait/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let index: Index = client
        .get(INDEX)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let stable = index
        .channels
        .get("Stable")
        .ok_or_else(|| MotionError::Fetch("canal Stable absent de l'index".into()))?;
    let url = stable
        .downloads
        .get(PRODUCT)
        .and_then(|list| list.iter().find(|d| d.platform == platform))
        .map(|d| d.url.clone())
        .ok_or_else(|| MotionError::Fetch(format!("aucun téléchargement pour {platform}")))?;
    tracing::info!("téléchargement de chrome-headless-shell {}", stable.version);
    let archive = client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    let version_dir = dir.join(&stable.version);
    let partial = dir.join(format!("{}.part", stable.version));
    let target = partial.clone();
    tokio::task::spawn_blocking(move || -> Result<(), MotionError> {
        if target.exists() {
            std::fs::remove_dir_all(&target)?;
        }
        std::fs::create_dir_all(&target)?;
        zip::ZipArchive::new(std::io::Cursor::new(archive))?.extract(&target)?;
        Ok(())
    })
    .await
    .map_err(|e| MotionError::Fetch(e.to_string()))??;
    if version_dir.exists() {
        tokio::fs::remove_dir_all(&version_dir).await?;
    }
    tokio::fs::rename(&partial, &version_dir).await?;
    let path = executable(&version_dir, platform);
    if !path.is_file() {
        return Err(MotionError::Fetch(format!(
            "exécutable absent de l'archive : {}",
            path.display()
        )));
    }
    Ok(path)
}
