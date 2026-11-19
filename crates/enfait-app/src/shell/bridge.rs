//! Bridge with agents (MCP server `enfait-cli mcp`): the interface reloads
//! the storyboard, look and finish when another program changed them,
//! queues the generation requests dropped in the session, and signals that
//! it is open. A change coming from an agent is undone like any other
//! (finish) or resets the storyboard history.

use super::Shell;
use crate::jobs::JobKind;
use crate::storyboard::Storyboard;
use enfait_core::{Finish, History, Look};
use enfait_mcp::bridge::{self, RequestKind};
use gpui_kit::Context;
use std::path::Path;
use std::time::SystemTime;

/// Heartbeat every `HEARTBEAT` polls (400 ms each).
const HEARTBEAT: u32 = 10;

/// Modification times seen of the shared files.
#[derive(Default)]
pub(super) struct Watch {
    project: Option<SystemTime>,
    look: Option<SystemTime>,
    finish: Option<SystemTime>,
    ticks: u32,
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// True if `path` changed since last time (and records it).
fn changed(path: &Path, seen: &mut Option<SystemTime>) -> bool {
    let now = modified(path);
    if now == *seen {
        return false;
    }
    *seen = now;
    now.is_some()
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    std::fs::read(path)
        .ok()
        .and_then(|d| serde_json::from_slice(&d).ok())
}

impl Shell {
    /// Called at every engine poll (every 400 ms).
    pub(super) fn poll_bridge(&mut self, cx: &mut Context<Self>) {
        let dir = self.session.dir().to_path_buf();
        if self.watch.ticks.is_multiple_of(HEARTBEAT) {
            bridge::touch_heartbeat(&dir);
        }
        self.watch.ticks = self.watch.ticks.wrapping_add(1);

        for request in bridge::take_requests(&dir) {
            let block = request.block.filter(|id| {
                self.board
                    .as_ref()
                    .and_then(|b| b.project.block(*id))
                    .is_some()
            });
            match (request.kind, block) {
                (RequestKind::Animation, Some(id)) => self.enqueue(id, JobKind::Create, cx),
                (RequestKind::Transition, Some(id)) => self.enqueue(id, JobKind::Transition, cx),
                (RequestKind::Storyboard, _) => {
                    if self.board.is_none() {
                        self.go_to(super::STEP_STORYBOARD, cx);
                    }
                    self.write_storyboard(cx);
                }
                (RequestKind::Magic, _) => self.start_magic(cx),
                // Block gone (storyboard rebuilt): request ignored.
                (RequestKind::Animation | RequestKind::Transition, None) => {}
            }
        }

        let mut seen = std::mem::take(&mut self.watch);
        if changed(&dir.join("look.json"), &mut seen.look)
            && let Some(look) = read::<Look>(&dir.join("look.json"))
            && look != self.session.look
        {
            tracing::info!("library or look changed by another program: reloaded");
            self.session.look = look;
            let fx = self.session.look.fx;
            self.player.update(cx, |p, cx| p.set_fx(fx, cx));
            self.push_snapshot();
            cx.notify();
        }
        if changed(&dir.join("finish.json"), &mut seen.finish)
            && let Some(finish) = read::<Finish>(&dir.join("finish.json"))
            && finish != self.session.finish
        {
            tracing::info!("finish changed by another program: reloaded");
            let before = std::mem::replace(&mut self.session.finish, finish);
            // Undoable like a change made by hand.
            self.finish_history.record(&before);
            self.push_snapshot();
            self.worker.request_mix();
            cx.notify();
        }
        if changed(&dir.join("project.json"), &mut seen.project)
            && let Some(board) = &mut self.board
            && let Some(loaded) = Storyboard::load(&dir)
            && loaded.project != board.project
            && loaded.key == board.key
        {
            tracing::info!("storyboard changed by another program: reloaded");
            board.project = loaded.project;
            board.history = History::new();
            if board
                .selected
                .is_some_and(|id| board.project.block(id).is_none())
            {
                board.select(None);
            }
            self.needs_sync = true;
            self.push_snapshot();
            cx.notify();
        }
        self.watch = seen;
    }
}
