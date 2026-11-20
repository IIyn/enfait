//! Update check: the latest GitHub release, compared with this version, and
//! the package of that release that fits this installation.

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Version of this build: the release tag (`ENFAIT_VERSION`, set by the
/// release workflow, `v` removed by the comparison), else the crate version.
pub const CURRENT: &str = match option_env!("ENFAIT_VERSION") {
    Some(tag) => tag,
    None => env!("CARGO_PKG_VERSION"),
};

/// [`CURRENT`] without the `v` of a tag, for display.
pub fn current() -> &'static str {
    CURRENT.trim_start_matches('v')
}

/// Latest published release (drafts and pre-releases excluded by GitHub).
const LATEST: &str = "https://api.github.com/repos/IIyn/enfait/releases/latest";

#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub body: Option<String>,
    pub html_url: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

impl Release {
    /// Version without the `v` of the tag.
    pub fn version(&self) -> &str {
        self.tag_name.trim_start_matches('v')
    }

    /// Release note as plain lines: Markdown headings, emphasis and list
    /// marks removed, list items as bullets. Empty without a note.
    pub fn notes(&self) -> Vec<String> {
        let body = self
            .body
            .as_deref()
            .unwrap_or_default()
            .replace("\r\n", "\n");
        let mut lines: Vec<String> = body
            .lines()
            .map(|line| {
                let line = line.trim_end();
                let trimmed = line.trim_start();
                let indent = &line[..line.len().saturating_sub(trimmed.len())];
                let text = if let Some(item) = trimmed
                    .strip_prefix("- ")
                    .or_else(|| trimmed.strip_prefix("* "))
                {
                    format!("{indent}• {item}")
                } else {
                    trimmed.trim_start_matches('#').trim_start().to_owned()
                };
                text.replace("**", "").replace('`', "")
            })
            .collect();
        // No blank lines at the ends, at most one in a row.
        lines.dedup_by(|a, b| a.is_empty() && b.is_empty());
        while lines.first().is_some_and(String::is_empty) {
            lines.remove(0);
        }
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines
    }
}

/// Asks GitHub for the latest release. `None`: nothing published that can
/// be seen without an account (no release yet, or private repository).
pub async fn latest(client: &reqwest::Client) -> Result<Option<Release>, String> {
    let response = client
        .get(LATEST)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    response
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json::<Release>()
        .await
        .map(Some)
        .map_err(|e| e.to_string())
}

/// `x.y.z` with an optional `-pre` suffix: numbers, then whether it is a
/// final release (after its pre-releases), then the suffix.
fn parse(version: &str) -> Option<(Vec<u64>, bool, String)> {
    let version = version.trim().trim_start_matches('v');
    let (core, pre) = version.split_once('-').unwrap_or((version, ""));
    let mut numbers = core
        .split('.')
        .map(|n| n.parse::<u64>().ok())
        .collect::<Option<Vec<u64>>>()?;
    if numbers.is_empty() {
        return None;
    }
    numbers.resize(numbers.len().max(3), 0);
    Some((numbers, pre.is_empty(), pre.to_owned()))
}

/// Whether `latest` is a later version than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse(latest), parse(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// How enfait was installed, hence which file of the release updates it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Package {
    WindowsSetup,
    MacDmg,
    Flatpak,
    Deb,
    Rpm,
    Arch,
    Tarball,
}

impl Package {
    pub fn detect() -> Self {
        if cfg!(windows) {
            return Self::WindowsSetup;
        }
        if cfg!(target_os = "macos") {
            return Self::MacDmg;
        }
        if std::env::var_os("FLATPAK_ID").is_some() {
            return Self::Flatpak;
        }
        // Installed by the package manager: binary in /usr.
        let system = std::env::current_exe().is_ok_and(|exe| exe.starts_with("/usr"));
        let exists = |path: &str| Path::new(path).exists();
        if !system {
            Self::Tarball
        } else if exists("/etc/arch-release") {
            Self::Arch
        } else if exists("/etc/debian_version") {
            Self::Deb
        } else if exists("/etc/redhat-release")
            || exists("/etc/fedora-release")
            || exists("/etc/SUSE-brand")
        {
            Self::Rpm
        } else {
            Self::Tarball
        }
    }

    fn fits(self, name: &str) -> bool {
        match self {
            Self::WindowsSetup => name.ends_with("-windows-setup.exe"),
            Self::MacDmg => {
                name.ends_with(".dmg")
                    && name.contains(if cfg!(target_arch = "aarch64") {
                        "arm64"
                    } else {
                        "x86_64"
                    })
            }
            Self::Flatpak => name.ends_with(".flatpak"),
            Self::Deb => name.ends_with(".deb"),
            Self::Rpm => name.ends_with(".rpm"),
            Self::Arch => name.ends_with(".pkg.tar.zst"),
            Self::Tarball => name.ends_with("-linux.tar.gz"),
        }
    }

    /// The file of `release` for this installation.
    pub fn asset(self, release: &Release) -> Option<&Asset> {
        release.assets.iter().find(|a| self.fits(&a.name))
    }

    /// The system opens the file in an installer (setup, disk image,
    /// software centre). Otherwise a command installs it.
    pub const fn opens_installer(self) -> bool {
        !matches!(self, Self::Arch | Self::Tarball)
    }

    /// Command installing `file` by hand (Linux).
    pub fn command(self, file: &Path) -> Option<String> {
        let file = file.display();
        match self {
            Self::WindowsSetup | Self::MacDmg => None,
            Self::Flatpak => Some(format!("flatpak install --user \"{file}\"")),
            Self::Deb => Some(format!("sudo apt install \"{file}\"")),
            Self::Rpm => Some(format!("sudo dnf install \"{file}\"")),
            Self::Arch => Some(format!("sudo pacman -U \"{file}\"")),
            Self::Tarball => {
                let dir = std::env::current_exe()
                    .ok()
                    .and_then(|exe| exe.parent().and_then(Path::parent).map(Path::to_path_buf))
                    .unwrap_or_else(|| PathBuf::from("."));
                Some(format!(
                    "tar xzf \"{file}\" -C \"{}\" --strip-components=1",
                    dir.display()
                ))
            }
        }
    }
}

/// Where the update is downloaded: the user's downloads folder.
pub fn download_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|dirs| dirs.download_dir().map(Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(body: &str, assets: &[&str]) -> Release {
        Release {
            tag_name: "v0.2.0".into(),
            body: Some(body.into()),
            html_url: "https://github.com/IIyn/enfait/releases/tag/v0.2.0".into(),
            assets: assets
                .iter()
                .map(|name| Asset {
                    name: (*name).into(),
                    browser_download_url: format!("https://example.invalid/{name}"),
                })
                .collect(),
        }
    }

    #[test]
    fn versions() {
        assert!(is_newer("v0.1.1", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.1"));
        assert!(
            is_newer("0.2.0", "0.2.0-rc1"),
            "final after its pre-release"
        );
        assert!(!is_newer("0.2.0-rc1", "0.2.0"));
        assert!(is_newer("0.2.0-rc2", "0.2.0-rc1"));
        assert!(!is_newer("nightly", "0.1.0"), "unreadable: no update");
    }

    #[test]
    fn notes_as_plain_lines() {
        let r = release(
            "\r\n## What's Changed\r\n* **Faster** playback by @x\r\n  - nested `item`\r\n\r\n\r\n**Full Changelog**: v0.1.0...v0.2.0\r\n\r\n",
            &[],
        );
        assert_eq!(
            r.notes(),
            [
                "What's Changed",
                "• Faster playback by @x",
                "  • nested item",
                "",
                "Full Changelog: v0.1.0...v0.2.0",
            ]
        );
        assert!(release("", &[]).notes().is_empty());
        assert_eq!(release("", &[]).version(), "0.2.0");
    }

    #[test]
    fn package_files() {
        let r = release(
            "",
            &[
                "enfait-0.2.0-1.x86_64.rpm",
                "enfait-0.2.0-arm64-macos.dmg",
                "enfait-0.2.0-x86_64-linux.tar.gz",
                "enfait-0.2.0-x86_64-windows-setup.exe",
                "enfait-0.2.0-x86_64-windows.zip",
                "enfait-0.2.0-x86_64.flatpak",
                "enfait-bin-0.2.0-1-x86_64.pkg.tar.zst",
                "enfait_0.2.0_amd64.deb",
                "SHA256SUMS",
            ],
        );
        let name = |p: Package| p.asset(&r).map(|a| a.name.as_str());
        assert_eq!(
            name(Package::WindowsSetup),
            Some("enfait-0.2.0-x86_64-windows-setup.exe")
        );
        assert_eq!(name(Package::Flatpak), Some("enfait-0.2.0-x86_64.flatpak"));
        assert_eq!(name(Package::Deb), Some("enfait_0.2.0_amd64.deb"));
        assert_eq!(name(Package::Rpm), Some("enfait-0.2.0-1.x86_64.rpm"));
        assert_eq!(
            name(Package::Arch),
            Some("enfait-bin-0.2.0-1-x86_64.pkg.tar.zst")
        );
        assert_eq!(
            name(Package::Tarball),
            Some("enfait-0.2.0-x86_64-linux.tar.gz")
        );
        assert!(Package::Arch.command(Path::new("/d/f.zst")).is_some());
        assert!(Package::WindowsSetup.command(Path::new("f.exe")).is_none());
    }
}
