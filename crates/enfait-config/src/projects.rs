//! The list of projects (`projects.json` in the data directory).
//!
//! A project is a session folder (script, storyboard, library, studio).
//! New projects live in `<data>/projects/<id>/`; the single session of
//! older versions (`<data>/session/`) is picked up as a project when it
//! holds something. The registry remembers the project last opened in the
//! app: the MCP server and the CLI work on it.

use crate::{ConfigError, Result, paths, write_atomic};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectEntry {
    pub dir: PathBuf,
    /// Title shown on the home screen (may be empty: untitled).
    #[serde(default)]
    pub title: String,
    /// Seconds since the Unix epoch.
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub opened: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Projects {
    /// Project open in the app (or last opened).
    pub current: Option<PathBuf>,
    pub projects: Vec<ProjectEntry>,
}

/// Now, in seconds since the Unix epoch.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `projects.json`.
pub fn registry_file() -> PathBuf {
    paths::data_dir().join("projects.json")
}

/// Folder where new projects are created.
pub fn projects_dir() -> PathBuf {
    paths::data_dir().join("projects")
}

/// True if a session folder holds something worth listing.
fn has_content(dir: &Path) -> bool {
    ["script.md", "session.json", "project.json"]
        .iter()
        .any(|f| dir.join(f).is_file())
}

impl Projects {
    /// The registry of the user (the legacy session is added if needed).
    pub fn load() -> Self {
        let mut projects = Self::load_from(&registry_file()).unwrap_or_else(|e| {
            tracing::warn!("{e}; empty project list");
            Self::default()
        });
        let legacy = paths::default_session();
        if has_content(&legacy) && !projects.contains(&legacy) {
            let stamp = std::fs::metadata(legacy.join("session.json"))
                .or_else(|_| std::fs::metadata(legacy.join("script.md")))
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or_else(now, |d| d.as_secs());
            projects.projects.push(ProjectEntry {
                dir: legacy,
                title: String::new(),
                created: stamp,
                opened: stamp,
            });
            let _ = projects.save();
        }
        projects
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|source| ConfigError::Json {
                path: path.to_path_buf(),
                source,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&registry_file())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(|source| ConfigError::Json {
            path: path.to_path_buf(),
            source,
        })?;
        write_atomic(path, &json)
    }

    pub fn contains(&self, dir: &Path) -> bool {
        self.projects.iter().any(|p| p.dir == dir)
    }

    /// Projects, most recently opened first (folders that disappeared are
    /// left out).
    pub fn recent(&self) -> Vec<ProjectEntry> {
        let mut list: Vec<ProjectEntry> = self
            .projects
            .iter()
            .filter(|p| p.dir.is_dir())
            .cloned()
            .collect();
        list.sort_by_key(|a| std::cmp::Reverse(a.opened));
        list
    }

    /// A new, empty project folder, registered and made current.
    pub fn create(&mut self, title: &str) -> Result<PathBuf> {
        self.create_in(&projects_dir(), title)
    }

    pub fn create_in(&mut self, parent: &Path, title: &str) -> Result<PathBuf> {
        let stamp = now();
        let slug = slug(title);
        let base = if slug.is_empty() {
            format!("project-{stamp}")
        } else {
            format!("{slug}-{stamp}")
        };
        let mut dir = parent.join(&base);
        let mut n = 2u32;
        while dir.exists() {
            dir = parent.join(format!("{base}-{n}"));
            n = n.saturating_add(1);
        }
        std::fs::create_dir_all(&dir).map_err(|source| ConfigError::Io {
            path: dir.clone(),
            source,
        })?;
        self.projects.push(ProjectEntry {
            dir: dir.clone(),
            title: title.trim().to_owned(),
            created: stamp,
            opened: stamp,
        });
        self.current = Some(dir.clone());
        Ok(dir)
    }

    /// Marks a project as opened now and current (registers it if needed).
    pub fn touch(&mut self, dir: &Path) {
        let stamp = now();
        match self.projects.iter_mut().find(|p| p.dir == dir) {
            Some(p) => p.opened = stamp,
            None => self.projects.push(ProjectEntry {
                dir: dir.to_path_buf(),
                title: String::new(),
                created: stamp,
                opened: stamp,
            }),
        }
        self.current = Some(dir.to_path_buf());
    }

    pub fn set_title(&mut self, dir: &Path, title: &str) {
        if let Some(p) = self.projects.iter_mut().find(|p| p.dir == dir) {
            title.trim().clone_into(&mut p.title);
        }
    }

    /// Removes a project from the list; its folder is deleted when it lives
    /// in the projects folder (never a folder chosen elsewhere).
    pub fn remove(&mut self, dir: &Path) -> Result<()> {
        self.projects.retain(|p| p.dir != dir);
        if self.current.as_deref() == Some(dir) {
            self.current = None;
        }
        let owned = dir.starts_with(projects_dir()) || dir == paths::default_session();
        if owned && dir.is_dir() {
            std::fs::remove_dir_all(dir).map_err(|source| ConfigError::Io {
                path: dir.to_path_buf(),
                source,
            })?;
        }
        Ok(())
    }
}

/// The project the tools work on: `ENFAIT_SESSION`, else the project
/// last opened in the app, else the legacy session folder.
pub fn current_session() -> PathBuf {
    if let Some(dir) = std::env::var_os("ENFAIT_SESSION") {
        return dir.into();
    }
    Projects::load_from(&registry_file())
        .ok()
        .and_then(|p| p.current)
        .filter(|d| d.is_dir())
        .unwrap_or_else(paths::default_session)
}

/// Folder-friendly form of a title: lowercase letters, digits and dashes.
fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.trim().chars().flat_map(char::to_lowercase).map(fold) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 40 {
            break;
        }
    }
    out.trim_end_matches('-').to_owned()
}

/// Accented Latin letter → its base letter.
fn fold(c: char) -> char {
    match c {
        'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'î' | 'ï' | 'í' | 'ì' => 'i',
        'ô' | 'ö' | 'ó' | 'ò' | 'õ' => 'o',
        'ù' | 'û' | 'ü' | 'ú' => 'u',
        'ç' => 'c',
        'ñ' => 'n',
        'ÿ' => 'y',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn create_touch_and_remove() {
        let root = tempfile::tempdir().unwrap();
        let registry = root.path().join("projects.json");
        let mut p = Projects::load_from(&registry).unwrap();
        let a = p.create_in(root.path(), "Mon tuto : Hum !").unwrap();
        assert!(a.is_dir());
        assert!(
            a.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("mon-tuto-hum-")
        );
        let b = p.create_in(root.path(), "").unwrap();
        assert_ne!(a, b);
        assert_eq!(p.current.as_deref(), Some(b.as_path()));
        p.projects.iter_mut().for_each(|e| e.opened = 1);
        p.touch(&a);
        assert_eq!(p.recent().first().map(|e| e.dir.clone()), Some(a.clone()));
        p.set_title(&b, "  Court ");
        p.save_to(&registry).unwrap();
        let again = Projects::load_from(&registry).unwrap();
        assert_eq!(again, p);
        assert_eq!(again.projects[1].title, "Court");
        // A folder outside the projects folder is only forgotten.
        p.remove(&a).unwrap();
        assert!(a.is_dir());
        assert!(!p.contains(&a));
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("  Été 2026 / vlog "), "ete-2026-vlog");
        assert_eq!(slug("ABC def"), "abc-def");
        assert_eq!(slug("!!!"), "");
    }
}
