//! Pont entre le serveur MCP (ou tout autre outil) et l'interface ouverte,
//! par des fichiers de la session :
//!
//! - les modifications du storyboard, du look et de la finition sont
//!   écrites dans `project.json`, `look.json` et `finish.json` ; l'interface
//!   les relit quand ils changent ;
//! - les demandes de génération sont déposées dans `studio/requests/`,
//!   un fichier par demande, que l'interface met dans sa file de jobs ;
//! - l'interface touche `studio/bridge.alive` régulièrement : on sait si
//!   elle est ouverte.

use enfait_core::BlockId;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

/// L'interface est considérée ouverte si elle a donné signe de vie depuis…
const ALIVE: Duration = Duration::from_secs(15);

/// Dossier de la session (`ENFAIT_SESSION` pour en utiliser un autre).
pub fn session_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("ENFAIT_SESSION") {
        return dir.into();
    }
    directories::ProjectDirs::from("", "", "enfait")
        .map_or_else(
            || std::env::temp_dir().join("enfait"),
            |d| d.data_dir().to_path_buf(),
        )
        .join("session")
}

fn requests_dir(session: &Path) -> PathBuf {
    session.join("studio").join("requests")
}

fn heartbeat_path(session: &Path) -> PathBuf {
    session.join("studio").join("bridge.alive")
}

/// Ce qu'on demande à l'interface de générer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    /// L'animation (ou le plan) du bloc.
    Animation,
    /// La transition qui suit le bloc.
    Transition,
    /// Faire écrire par le monteur les blocs sans description (tout le
    /// storyboard, sans bloc).
    Storyboard,
    /// Le mode magique sur la prise de la session : storyboard, plans,
    /// sons, animations et export (sans bloc).
    Magic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// Le bloc visé ; aucun pour une demande sur tout le projet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<BlockId>,
    pub kind: RequestKind,
}

/// Écriture atomique (fichier temporaire puis renommage).
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

/// Dépose une demande pour l'interface.
pub fn send_request(session: &Path, request: &Request) -> std::io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = format!("{stamp}-{}-{n}.json", std::process::id());
    let json = serde_json::to_vec(request).map_err(std::io::Error::other)?;
    write_atomic(&requests_dir(session).join(name), &json)
}

/// Demandes en attente, dans l'ordre de dépôt ; elles sont retirées.
pub fn take_requests(session: &Path) -> Vec<Request> {
    let Ok(entries) = std::fs::read_dir(requests_dir(session)) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|path| {
            let request = std::fs::read(&path)
                .ok()
                .and_then(|d| serde_json::from_slice(&d).ok());
            let _ = std::fs::remove_file(&path);
            request
        })
        .collect()
}

/// L'interface signale qu'elle est ouverte.
pub fn touch_heartbeat(session: &Path) {
    let _ = write_atomic(
        &heartbeat_path(session),
        std::process::id().to_string().as_bytes(),
    );
}

/// L'interface s'en va.
pub fn clear_heartbeat(session: &Path) {
    let _ = std::fs::remove_file(heartbeat_path(session));
}

/// L'interface est ouverte sur cette session.
pub fn app_alive(session: &Path) -> bool {
    std::fs::metadata(heartbeat_path(session))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < ALIVE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_heartbeat() {
        let dir = std::env::temp_dir().join(format!("enfait-bridge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!app_alive(&dir));
        touch_heartbeat(&dir);
        assert!(app_alive(&dir));
        clear_heartbeat(&dir);
        assert!(!app_alive(&dir));
        let (a, b) = (BlockId::new(), BlockId::new());
        send_request(
            &dir,
            &Request {
                block: Some(a),
                kind: RequestKind::Animation,
            },
        )
        .unwrap();
        send_request(
            &dir,
            &Request {
                block: Some(b),
                kind: RequestKind::Transition,
            },
        )
        .unwrap();
        send_request(
            &dir,
            &Request {
                block: None,
                kind: RequestKind::Magic,
            },
        )
        .unwrap();
        let taken = take_requests(&dir);
        assert_eq!(taken.len(), 3);
        assert_eq!(taken[0].block, Some(a), "dans l'ordre de dépôt");
        assert_eq!(taken[1].kind, RequestKind::Transition);
        assert_eq!(taken[2].block, None);
        // Une demande sans bloc s'écrit sans champ `block`.
        let json = serde_json::to_string(&taken[2]).unwrap();
        assert_eq!(json, r#"{"kind":"magic"}"#);
        assert!(take_requests(&dir).is_empty(), "retirées une fois lues");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
