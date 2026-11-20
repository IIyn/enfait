//! Installing `chrome-headless-shell` from Chrome for Testing.
//!
//! Reads the official index of stable versions, downloads the archive of
//! the platform and extracts it into `dir/<version>/`. A version already
//! installed is not downloaded again.

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

/// Platform in the Chrome for Testing sense, for the running system.
fn platform() -> Result<&'static str, MotionError> {
    platform_for(std::env::consts::OS, std::env::consts::ARCH)
}

/// Platform in the Chrome for Testing sense. Linux arm64 is published
/// since Chrome 153 (August 2026).
fn platform_for(os: &str, arch: &str) -> Result<&'static str, MotionError> {
    match (os, arch) {
        ("linux", "x86_64") => Ok("linux64"),
        ("linux", "aarch64") => Ok("linux-arm64"),
        ("macos", "aarch64") => Ok("mac-arm64"),
        ("macos", "x86_64") => Ok("mac-x64"),
        ("windows", "x86_64") => Ok("win64"),
        ("windows", "x86") => Ok("win32"),
        (os, arch) => Err(MotionError::Fetch(format!(
            "no chrome-headless-shell for {os}/{arch}"
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

/// Returns the path of `chrome-headless-shell`, installing it into `dir`
/// on the first call.
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
        .ok_or_else(|| MotionError::Fetch("no Stable channel in the index".into()))?;
    let url = stable
        .downloads
        .get(PRODUCT)
        .and_then(|list| list.iter().find(|d| d.platform == platform))
        .map(|d| d.url.clone())
        .ok_or_else(|| MotionError::Fetch(format!("no download for {platform}")))?;
    tracing::info!("downloading chrome-headless-shell {}", stable.version);
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
            "executable missing from the archive: {}",
            path.display()
        )));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_platforms() {
        assert_eq!(platform_for("linux", "x86_64").unwrap(), "linux64");
        assert_eq!(platform_for("linux", "aarch64").unwrap(), "linux-arm64");
        assert_eq!(platform_for("macos", "aarch64").unwrap(), "mac-arm64");
        assert!(platform_for("linux", "riscv64").is_err());
    }

    #[test]
    fn executable_layout() {
        let path = executable(Path::new("/c/158.0"), "linux-arm64");
        assert_eq!(
            path,
            Path::new("/c/158.0/chrome-headless-shell-linux-arm64/chrome-headless-shell")
        );
    }
}
