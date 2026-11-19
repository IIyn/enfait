//! Bridge between the MCP server (or any other tool) and the open app,
//! through session files:
//!
//! - changes to the storyboard, look and finish are written to
//!   `project.json`, `look.json` and `finish.json`; the app reloads them
//!   when they change;
//! - generation requests are dropped in `studio/requests/`, one file per
//!   request, which the app puts in its job queue;
//! - the app touches `studio/bridge.alive` regularly: we know whether it
//!   is open.

use enfait_core::BlockId;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

/// The app is considered open if it gave a sign of life within…
const ALIVE: Duration = Duration::from_secs(15);

/// Session folder: `ENFAIT_SESSION`, else the project open in the app
/// (or last opened), else the legacy session folder.
pub fn session_dir() -> PathBuf {
    enfait_config::projects::current_session()
}

fn requests_dir(session: &Path) -> PathBuf {
    session.join("studio").join("requests")
}

fn heartbeat_path(session: &Path) -> PathBuf {
    session.join("studio").join("bridge.alive")
}

/// What the app is asked to generate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    /// The animation (or the shot) of the block.
    Animation,
    /// The transition after the block.
    Transition,
    /// Have the editor write the blocks without a description (the whole
    /// storyboard, no block).
    Storyboard,
    /// Magic mode on the take of the session: storyboard, shots, sounds,
    /// animations and export (no block).
    Magic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// The block aimed at; none for a request on the whole project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<BlockId>,
    pub kind: RequestKind,
}

/// Atomic write (temporary file, then rename).
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

/// Drops a request for the app.
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

/// Pending requests, in drop order; they are removed.
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

/// The app signals that it is open.
pub fn touch_heartbeat(session: &Path) {
    let _ = write_atomic(
        &heartbeat_path(session),
        std::process::id().to_string().as_bytes(),
    );
}

/// The app goes away.
pub fn clear_heartbeat(session: &Path) {
    let _ = std::fs::remove_file(heartbeat_path(session));
}

/// The app is open on this session.
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
        assert_eq!(taken[0].block, Some(a), "in drop order");
        assert_eq!(taken[1].kind, RequestKind::Transition);
        assert_eq!(taken[2].block, None);
        // A request without a block is written without a `block` field.
        let json = serde_json::to_string(&taken[2]).unwrap();
        assert_eq!(json, r#"{"kind":"magic"}"#);
        assert!(take_requests(&dir).is_empty(), "removed once read");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
