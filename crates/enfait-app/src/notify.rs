//! Desktop notifications: a block animated, an export written… while the
//! window is in the background or closed (tray).
//!
//! `notify-rust` speaks to the notification server over D-Bus on Linux, to
//! the Notification Center on macOS and shows a toast on Windows. Sending
//! can block for a moment, so it happens on its own thread. Clicking a
//! notification (where the platform reports it) brings the window back.

use std::sync::atomic::{AtomicBool, Ordering};

const ICON_PNG: &[u8] = include_bytes!("../../../assets/icon/enfait-128.png");

/// A notification was clicked: the window should come back.
static OPEN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// True once after a notification was clicked.
pub fn take_open_request() -> bool {
    OPEN_REQUESTED.swap(false, Ordering::AcqRel)
}

/// Icon file for the notification server (it wants a path).
fn icon_path() -> Option<String> {
    let path = enfait_config::paths::cache_dir().join("enfait.png");
    if !path.is_file() {
        std::fs::create_dir_all(path.parent()?).ok()?;
        std::fs::write(&path, ICON_PNG).ok()?;
    }
    Some(path.to_string_lossy().into_owned())
}

/// Shows a notification without waiting.
pub fn send(title: String, body: String) {
    let spawned = std::thread::Builder::new()
        .name("enfait-notify".into())
        .spawn(move || {
            let mut notification = notify_rust::Notification::new();
            notification.appname("enfait").summary(&title).body(&body);
            #[cfg(all(unix, not(target_os = "macos")))]
            {
                if let Some(icon) = icon_path() {
                    notification.icon(&icon);
                }
                notification.action("default", "enfait");
            }
            #[cfg(not(all(unix, not(target_os = "macos"))))]
            let _ = icon_path;
            match notification.show() {
                #[cfg(all(unix, not(target_os = "macos")))]
                Ok(handle) => handle.wait_for_action(|action| {
                    if action == "default" {
                        OPEN_REQUESTED.store(true, Ordering::Release);
                    }
                }),
                #[cfg(not(all(unix, not(target_os = "macos"))))]
                Ok(_) => {}
                Err(e) => tracing::debug!("notification: {e}"),
            }
        });
    if let Err(e) = spawned {
        tracing::debug!("notification: {e}");
    }
}
