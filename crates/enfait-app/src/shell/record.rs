//! Recording the voice in enfait: the microphone records while the
//! prompter scrolls the script; Escape ends the take, which becomes the
//! project's take (then the cut keeps the last takes).

use super::Shell;
use crate::prompter::{self, Prompter};
use crate::tr;
use enfait_media::Command;
use enfait_media::record::Recorder;
use gpui_kit::prelude::*;
use gpui_kit::{Context, Window};
use std::path::PathBuf;

pub(super) struct Recording {
    recorder: Recorder,
    path: PathBuf,
}

impl Shell {
    /// Starts recording with the prompter.
    pub(super) fn start_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.recording.is_some() {
            return;
        }
        self.command(Command::Pause, cx);
        let recorder = match Recorder::start() {
            Ok(r) => r,
            Err(e) => {
                self.toast(tr!("voice.record_failed", error = e.to_string()));
                cx.notify();
                return;
            }
        };
        tracing::info!("recording from {}", recorder.device());
        let dir = self.session.recordings_dir();
        let mut n = 1u32;
        let path = loop {
            let candidate = dir.join(format!("take-{n:02}.wav"));
            if !candidate.exists() {
                break candidate;
            }
            n = n.saturating_add(1);
        };
        let paragraphs = enfait_speech::script::spoken_paragraphs(&self.session.script);
        let meter = recorder.meter();
        let prompter = cx.new(|cx| Prompter::recording(paragraphs, meter, window, cx));
        cx.subscribe(&prompter, |shell, _, _: &prompter::Closed, cx| {
            shell.prompter = None;
            shell.refocus = true;
            shell.finish_recording(cx);
        })
        .detach();
        self.prompter = Some(prompter);
        self.recording = Some(Recording { recorder, path });
        cx.notify();
    }

    /// Ends the recording: the file becomes the take.
    fn finish_recording(&mut self, cx: &mut Context<Self>) {
        let Some(Recording { recorder, path }) = self.recording.take() else {
            return;
        };
        if recorder.seconds() < 0.5 {
            self.toast(tr!("voice.record_too_short"));
            cx.notify();
            return;
        }
        match recorder.finish(&path) {
            Ok(()) => {
                self.toast(tr!("voice.recorded"));
                self.import_rush(path, cx);
            }
            Err(e) => self.toast(tr!("voice.record_failed", error = e.to_string())),
        }
        cx.notify();
    }
}
