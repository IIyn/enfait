//! "Write the storyboard" and magic mode.
//!
//! The storyboard button has the editor write the blocks that have no
//! description yet; magic mode chains everything from the take alone:
//! cleanup (last takes, found in the transcript when there is no script),
//! written storyboard, shots and sounds found, animations, export. Every
//! decision stays an ordinary edit: a single Ctrl+Z undoes the written
//! storyboard, the finish keeps its history.

use super::studio::{BlockState, chip};
use super::{STEP_FINISH, STEP_STORYBOARD, Shell};
use crate::theme::{color, tokens};
use crate::tr;
use crate::worker::{ExportKind, ExportRequest, ExportState, WriteRequest, Written};
use enfait_core::Edit;
use gpui_kit::prelude::*;
use gpui_kit::{ClickEvent, Context, Div, FontWeight, SharedString, div, px};
use std::path::{Path, PathBuf};

/// Where magic mode stands.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum MagicStage {
    /// Transcription and cut of the take, building the blocks.
    Preparing,
    /// The editor writes the storyboard, shots and sounds are searched.
    Writing,
    /// Animations and transitions are being made.
    Generating,
    Exporting,
    Done {
        path: PathBuf,
        message: String,
    },
    /// Stopped at step `step` (displayed number).
    Failed {
        step: usize,
        message: String,
    },
}

impl MagicStage {
    const fn number(&self) -> usize {
        match self {
            Self::Preparing => 1,
            Self::Writing => 2,
            Self::Generating => 3,
            Self::Exporting | Self::Done { .. } => 4,
            Self::Failed { step, .. } => *step,
        }
    }

    /// Failure at the current step.
    fn failed(&self, message: impl Into<String>) -> Self {
        Self::Failed {
            step: self.number(),
            message: message.into(),
        }
    }
}

/// Displayed steps.
const STAGES: usize = 4;

impl Shell {
    /// Starts magic mode: on the imported take, or after choosing one.
    pub(super) fn start_magic(&mut self, cx: &mut Context<Self>) {
        if self.session.data.rush.is_none() {
            self.magic_after_import = true;
            self.prompt_import(cx);
            return;
        }
        if matches!(
            self.magic,
            Some(
                MagicStage::Preparing
                    | MagicStage::Writing
                    | MagicStage::Generating
                    | MagicStage::Exporting
            )
        ) {
            return;
        }
        self.worker.clear_export();
        self.magic = Some(MagicStage::Preparing);
        // The storyboard is built as soon as the cut is ready.
        self.go_to(STEP_STORYBOARD, cx);
        self.poll_magic(cx);
    }

    pub(super) fn stop_magic(&mut self, cx: &mut Context<Self>) {
        if matches!(self.magic, Some(MagicStage::Generating))
            && let Ok(mut q) = self.queue.lock()
        {
            q.cancel_queued(None);
        }
        self.magic = None;
        cx.notify();
    }

    /// Asks the editor to write the blocks without description.
    pub(super) fn write_storyboard(&mut self, cx: &mut Context<Self>) {
        let Some(board) = &self.board else { return };
        let request = WriteRequest {
            keep_authored: true,
            quality: self.session.data.quality,
            blocks_per_request: crate::prefs::settings(cx).storyboard_part(),
            key: board.key.clone(),
        };
        self.write_notice = None;
        self.push_snapshot();
        self.worker.write_storyboard(request);
        cx.notify();
    }

    /// Applies a storyboard written by the engine, if there is one.
    pub(super) fn poll_written(&mut self, cx: &mut Context<Self>) {
        let Some(result) = self.worker.take_written() else {
            return;
        };
        match result {
            Ok(written) => {
                let notice = self.apply_written(&written, cx);
                self.write_notice = Some(notice);
                if self.magic == Some(MagicStage::Writing) {
                    self.generate_all(cx);
                    self.magic = Some(MagicStage::Generating);
                }
            }
            Err(e) => {
                let message = tr!("magic.board_failed", error = e);
                if self.magic == Some(MagicStage::Writing) {
                    self.magic = Some(MagicStage::Writing.failed(message.to_string()));
                }
                self.write_notice = Some(message);
            }
        }
        cx.notify();
    }

    /// Library, storyboard (one undo step) and finish.
    fn apply_written(&mut self, written: &Written, cx: &mut Context<Self>) -> SharedString {
        use enfait_studio::magic;
        // The files found join the library even if the storyboard changed
        // in the meantime: they stay useful.
        if !written.gathered.assets.is_empty() {
            self.session
                .look
                .assets
                .extend(written.gathered.assets.iter().cloned());
            self.session.save_look();
        }
        let Some(board) = &mut self.board else {
            return tr!("magic.no_board");
        };
        if board.key != written.request.key {
            return tr!("magic.edit_changed");
        }
        let edits = magic::edits(
            &board.project,
            &board.transcript,
            &written.plan,
            &written.gathered,
            written.request.keep_authored,
            magic::max_transitions(board.project.blocks().len()),
        );
        let count = edits.len();
        if count > 0
            && let Err(e) = board.apply(Edit::Batch { edits })
        {
            tracing::warn!("written storyboard refused: {e}");
            return tr!("magic.apply_failed", error = e.to_string());
        }
        self.needs_sync = true;
        let (project, transcript) = (board.project.clone(), board.transcript.clone());
        let mut sounds = 0;
        self.edit_finish(
            |finish| {
                sounds = magic::place_sounds(
                    &project,
                    &transcript,
                    &written.plan,
                    &written.gathered,
                    finish,
                );
            },
            cx,
        );
        self.save_board();
        self.worker.request_preview();
        written_summary(written, count, sounds)
    }

    /// Moves magic mode forward (polled with the engine).
    pub(super) fn poll_magic(&mut self, cx: &mut Context<Self>) {
        let Some(stage) = self.magic.clone() else {
            return;
        };
        let next = match stage {
            MagicStage::Preparing => self.magic_prepared(cx),
            MagicStage::Writing => None,
            MagicStage::Generating => self.magic_generated(cx),
            MagicStage::Exporting => match self.worker.export_state() {
                ExportState::Done { path, message } => {
                    self.go_to(STEP_FINISH, cx);
                    Some(MagicStage::Done { path, message })
                }
                ExportState::Failed(e) => Some(
                    MagicStage::Exporting.failed(tr!("magic.export_failed", error = e).to_string()),
                ),
                _ => None,
            },
            MagicStage::Done { .. } | MagicStage::Failed { .. } => None,
        };
        if let Some(next) = next {
            self.magic = Some(next);
            cx.notify();
        }
    }

    /// Cut and storyboard ready: the editor takes over.
    fn magic_prepared(&mut self, cx: &mut Context<Self>) -> Option<MagicStage> {
        use crate::cut::CutStatus;
        match &self.cut.status {
            CutStatus::Failed(e) => return Some(MagicStage::Preparing.failed(e.to_string())),
            CutStatus::NoRush => {
                return Some(MagicStage::Preparing.failed(tr!("magic.no_rush").to_string()));
            }
            CutStatus::Transcribing => return None,
            CutStatus::Ready if self.cut.analyzing.is_some() => return None,
            CutStatus::Ready => {}
        }
        if self.board.is_none() {
            self.refresh_board(cx);
        }
        if self.board.is_none() {
            return self
                .board_notice
                .clone()
                .map(|n| MagicStage::Preparing.failed(n.to_string()));
        }
        self.write_storyboard(cx);
        Some(MagicStage::Writing)
    }

    /// Nothing queued or running: export next to the take.
    fn magic_generated(&mut self, cx: &mut Context<Self>) -> Option<MagicStage> {
        let (running, queued) = self.queue.lock().map(|q| q.counts()).unwrap_or((1, 0));
        if running + queued > 0 || self.worker.activity().is_some() {
            return None;
        }
        let rush = self.session.data.rush.clone()?;
        let path = export_path(&rush);
        self.push_snapshot();
        self.worker.export(ExportRequest {
            kind: ExportKind::Video,
            path,
            sfx: true,
        });
        cx.notify();
        Some(MagicStage::Exporting)
    }

    /// Blocks whose animation failed (for the summary).
    fn failed_blocks(&self) -> usize {
        self.board.as_ref().map_or(0, |board| {
            board
                .project
                .blocks()
                .iter()
                .filter(|b| matches!(self.block_state(b), BlockState::Failed(_)))
                .count()
        })
    }

    /// Magic mode banner, at the top of every screen.
    pub(super) fn magic_banner(&self, cx: &mut Context<Self>) -> Option<Div> {
        let stage = self.magic.as_ref()?;
        let detail: String = match stage {
            MagicStage::Preparing if self.cut.status == crate::cut::CutStatus::Transcribing => {
                format!(
                    "{} · {}",
                    tr!("magic.preparing"),
                    self.cut.transcribing_label()
                )
            }
            MagicStage::Preparing => tr!("magic.preparing").to_string(),
            MagicStage::Writing => self
                .worker
                .writing()
                .map_or_else(|| tr!("magic.writing").to_string(), |w| capitalize(&w)),
            MagicStage::Generating => {
                let (running, queued) = self.queue.lock().map(|q| q.counts()).unwrap_or((0, 0));
                let activity = self.worker.activity().unwrap_or_default();
                format!(
                    "{}{}{}",
                    tr!("magic.generating", running = running, queued = queued),
                    if activity.is_empty() { "" } else { " · " },
                    activity
                )
            }
            MagicStage::Exporting => match self.worker.export_state() {
                ExportState::Running { progress, .. } => tr!(
                    "magic.exporting_pct",
                    pct = format!("{:.0}", progress * 100.0)
                )
                .to_string(),
                _ => tr!("magic.exporting").to_string(),
            },
            MagicStage::Done { message, .. } => {
                let failed = self.failed_blocks();
                if failed > 0 {
                    tr!("magic.failed_blocks", message = message, count = failed).to_string()
                } else {
                    message.clone()
                }
            }
            MagicStage::Failed { message, .. } => message.clone(),
        };
        let running = !matches!(stage, MagicStage::Done { .. } | MagicStage::Failed { .. });
        let tone = match stage {
            MagicStage::Failed { .. } => tokens::ERROR,
            _ => tokens::FG2,
        };
        let reveal = match stage {
            MagicStage::Done { path, .. } => Some(path.clone()),
            _ => None,
        };
        Some(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(10.))
                .px(px(16.))
                .py(px(6.))
                .bg(color(tokens::PANEL))
                .border_b_1()
                .border_color(color(tokens::LINE))
                .text_size(px(12.))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color(tokens::SIGNAL))
                        .child(tr!("magic.banner")),
                )
                .child(
                    div()
                        .text_color(color(tokens::MUTE))
                        .child(format!("{}/{STAGES}", stage.number())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_color(color(tone))
                        .child(SharedString::from(detail)),
                )
                .when_some(reveal, |d, path| {
                    d.child(
                        chip("magic-reveal", tr!("magic.reveal"), false, true).on_click(
                            cx.listener(move |_, _: &ClickEvent, _, cx| cx.reveal_path(&path)),
                        ),
                    )
                })
                .child(
                    chip(
                        "magic-stop",
                        if running {
                            tr!("magic.stop")
                        } else {
                            tr!("magic.close")
                        },
                        false,
                        true,
                    )
                    .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| shell.stop_magic(cx))),
                ),
        )
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

/// What the written storyboard changed, in a sentence.
fn written_summary(written: &Written, edits: usize, sounds: usize) -> SharedString {
    let brolls = written.gathered.brolls.len();
    let mut out = tr!("magic.summary.blocks", count = written.plan.blocks.len()).to_string();
    if brolls > 0 {
        out.push_str(&tr!("magic.summary.brolls", count = brolls));
    }
    if sounds > 0 {
        out.push_str(&tr!("magic.summary.sounds", count = sounds));
    }
    if edits == 0 {
        out.push_str(&tr!("magic.summary.nothing"));
    }
    out.push('.');
    if !written.gathered.missing.is_empty() {
        out.push_str(&tr!(
            "magic.summary.missing",
            list = written.gathered.missing.join(", ")
        ));
    }
    out.into()
}

/// Magic mode video: next to the take, numbered rather than overwriting an
/// existing file.
pub(super) fn export_path(rush: &Path) -> PathBuf {
    let dir = rush.parent().map_or_else(PathBuf::new, Path::to_path_buf);
    let stem = rush
        .file_stem()
        .map_or_else(|| "video".into(), |s| s.to_string_lossy().into_owned());
    let mut path = dir.join(format!("{stem}-enfait.mp4"));
    let mut n = 2_u32;
    while path.exists() {
        path = dir.join(format!("{stem}-enfait-{n}.mp4"));
        n = n.saturating_add(1);
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let rush = dir.path().join("prise.mov");
        assert_eq!(export_path(&rush), dir.path().join("prise-enfait.mp4"));
        std::fs::write(dir.path().join("prise-enfait.mp4"), b"").unwrap();
        assert_eq!(export_path(&rush), dir.path().join("prise-enfait-2.mp4"));
    }

    #[test]
    fn stages_are_numbered() {
        assert_eq!(MagicStage::Preparing.number(), 1);
        assert_eq!(MagicStage::Writing.failed("x").number(), 2);
        assert_eq!(MagicStage::Exporting.number(), STAGES);
        assert_eq!(capitalize("le monteur…"), "Le monteur…");
    }
}
