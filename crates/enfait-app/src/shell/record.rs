//! Recording the voice in enfait: the microphone records while the
//! prompter scrolls the script; Escape ends the take, which becomes the
//! project's take (then the cut keeps the last takes).
//!
//! With the camera on, ffmpeg films at the same time: the camera starts
//! first (it can take a moment), then the microphone and the prompter. At
//! the end the sound is aligned on the first frame and both go into one
//! MP4 take.

use super::Shell;
use crate::prefs;
use crate::prompter::{self, Prompter};
use crate::theme::{color, tokens};
use crate::tr;
use enfait_core::AspectRatio;
use enfait_media::Command;
use enfait_media::camera::CameraDevice;
use enfait_media::camera::CameraRecorder;
use enfait_media::record::InputDevice;
use enfait_media::record::Recorder;
use gpui_kit::prelude::*;
use gpui_kit::{ClickEvent, Context, Div, SharedString, Window, div, px};
use std::path::Path;

pub(super) struct Recording {
    recorder: Recorder,
    /// Take number (`take-07`).
    stem: String,
    camera: Option<CameraRecorder>,
}

/// Recording being prepared, or being written once stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Busy {
    StartingCamera,
    Saving,
}

/// Microphones and cameras of the system.
#[derive(Default)]
pub(super) struct Devices {
    microphones: Vec<InputDevice>,
    cameras: Vec<CameraDevice>,
    loading: bool,
}

/// Device name short enough for a chip.
fn short(name: &str) -> String {
    const MAX: usize = 40;
    if name.chars().count() <= MAX {
        name.to_owned()
    } else {
        let mut s: String = name.chars().take(MAX - 1).collect();
        s.push('…');
        s
    }
}

/// Name of the next take: no `.wav` or `.mp4` of that number yet.
fn next_stem(dir: &Path) -> String {
    let mut n = 1u32;
    loop {
        let stem = format!("take-{n:02}");
        if !["wav", "mp4"]
            .iter()
            .any(|ext| dir.join(format!("{stem}.{ext}")).exists())
        {
            return stem;
        }
        n = n.saturating_add(1);
    }
}

/// Picture ratio filmed for the project's format.
pub(super) const fn camera_aspect(aspect: AspectRatio) -> (u32, u32) {
    match aspect {
        AspectRatio::Landscape16x9 => (16, 9),
        AspectRatio::Portrait9x16 => (9, 16),
        AspectRatio::Square1x1 => (1, 1),
    }
}

/// Message for a camera that cannot start.
fn camera_error(e: &enfait_media::MediaError) -> SharedString {
    let text = e.to_string();
    if text.contains("busy") {
        tr!("voice.camera_busy")
    } else {
        tr!("voice.camera_failed", error = text)
    }
}

impl Shell {
    /// Starts recording with the prompter (and the camera if chosen).
    pub(super) fn start_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.recording.is_some() || self.recording_busy.is_some() {
            return;
        }
        self.command(Command::Pause, cx);
        let dir = self.session.recordings_dir();
        let stem = next_stem(&dir);
        let settings = prefs::settings(cx);
        if !settings.record_camera {
            self.begin_recording(stem, None, window, cx);
            return;
        }
        let camera = settings.camera.clone();
        let aspect = camera_aspect(self.settings().aspect);
        let video = dir.join(format!("{stem}.camera.mkv"));
        self.recording_busy = Some(Busy::StartingCamera);
        cx.notify();
        let work =
            cx.background_spawn(
                async move { CameraRecorder::start(camera.as_deref(), aspect, &video) },
            );
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |shell, window, cx| {
                shell.recording_busy = None;
                match result {
                    Ok(camera) => {
                        tracing::info!("filming with {}", camera.device());
                        shell.begin_recording(stem, Some(camera), window, cx);
                    }
                    Err(e) => {
                        shell.toast(camera_error(&e));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// Starts the microphone and opens the prompter.
    fn begin_recording(
        &mut self,
        stem: String,
        camera: Option<CameraRecorder>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let microphone = prefs::settings(cx).microphone.clone();
        let recorder = match Recorder::start(microphone.as_deref()) {
            Ok(r) => r,
            Err(e) => {
                // The camera stops with its recorder.
                drop(camera);
                self.toast(tr!("voice.record_failed", error = e.to_string()));
                cx.notify();
                return;
            }
        };
        tracing::info!("recording from {}", recorder.device());
        let paragraphs = enfait_speech::script::spoken_paragraphs(&self.session.script);
        let meter = recorder.meter();
        let preview = camera.as_ref().map(CameraRecorder::preview);
        let prompter = cx.new(|cx| Prompter::recording(paragraphs, meter, preview, window, cx));
        cx.subscribe(&prompter, |shell, _, _: &prompter::Closed, cx| {
            shell.prompter = None;
            shell.refocus = true;
            shell.finish_recording(cx);
        })
        .detach();
        self.prompter = Some(prompter);
        self.recording = Some(Recording {
            recorder,
            stem,
            camera,
        });
        cx.notify();
    }

    /// Ends the recording: the file becomes the take.
    fn finish_recording(&mut self, cx: &mut Context<Self>) {
        let Some(Recording {
            recorder,
            stem,
            camera,
        }) = self.recording.take()
        else {
            return;
        };
        if recorder.seconds() < 0.5 {
            drop(camera);
            self.toast(tr!("voice.record_too_short"));
            cx.notify();
            return;
        }
        let dir = self.session.recordings_dir();
        let Some(camera) = camera else {
            let path = dir.join(format!("{stem}.wav"));
            match recorder.finish(&path) {
                Ok(()) => {
                    self.toast(tr!("voice.recorded"));
                    self.import_rush(path, cx);
                }
                Err(e) => self.toast(tr!("voice.record_failed", error = e.to_string())),
            }
            cx.notify();
            return;
        };
        // The sound is written here (the audio stream stays on this thread);
        // stopping the camera and muxing go to the background.
        let audio = dir.join(format!("{stem}.voice.wav"));
        if let Err(e) = recorder.finish_from(&audio, camera.first_frame()) {
            drop(camera);
            self.toast(tr!("voice.record_failed", error = e.to_string()));
            cx.notify();
            return;
        }
        let out = dir.join(format!("{stem}.mp4"));
        let project = dir.clone();
        self.recording_busy = Some(Busy::Saving);
        cx.notify();
        let work = cx.background_spawn({
            let out = out.clone();
            async move {
                let video = camera.finish()?;
                enfait_media::camera::mux(&video, &audio, &out)?;
                // The take holds both now.
                let _ = std::fs::remove_file(&video);
                let _ = std::fs::remove_file(&audio);
                Ok::<_, enfait_media::MediaError>(())
            }
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |shell, cx| {
                shell.recording_busy = None;
                match result {
                    Ok(()) => {
                        shell.toast(tr!("voice.recorded_video"));
                        // Another project may have been opened meanwhile: the
                        // take stays in its own folder.
                        if shell.session.recordings_dir() == project {
                            shell.import_rush(out, cx);
                        }
                    }
                    Err(e) => shell.toast(tr!("voice.record_failed", error = e.to_string())),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Lists the microphones and cameras, away from the interface thread
    /// (on Windows and macOS, ffmpeg lists the cameras).
    pub(super) fn refresh_devices(&mut self, cx: &mut Context<Self>) {
        if self.devices.as_ref().is_some_and(|d| d.loading) {
            return;
        }
        let devices = self.devices.get_or_insert_with(Devices::default);
        devices.loading = true;
        let work = cx.background_spawn(async move {
            (
                enfait_media::record::input_devices(),
                enfait_media::camera::cameras(),
            )
        });
        cx.spawn(async move |this, cx| {
            let (microphones, cameras) = work.await;
            let _ = this.update(cx, |shell, cx| {
                shell.devices = Some(Devices {
                    microphones,
                    cameras,
                    loading: false,
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// Choice of filming, of the microphone and of the camera, above the
    /// record button.
    pub(super) fn devices_panel(&mut self, cx: &mut Context<Self>) -> Div {
        if self.devices.is_none() {
            self.refresh_devices(cx);
        }
        let settings = prefs::settings(cx);
        let (filming, mic, cam) = (
            settings.record_camera,
            settings.microphone.clone(),
            settings.camera.clone(),
        );
        let empty = Devices::default();
        let devices = self.devices.as_ref().unwrap_or(&empty);
        let row = |title: SharedString| {
            div()
                .flex()
                .items_start()
                .gap(px(12.))
                .child(super::label(title).w(px(96.)).flex_none().pt(px(5.)))
        };
        let chips = || div().flex().flex_wrap().gap(px(4.)).flex_1().min_w(px(0.));
        let listeners: &Context<Self> = cx;
        let mode = row(tr!("voice.mode")).child(
            chips()
                .child(
                    super::studio::chip("mode-voice", tr!("voice.mode_voice"), !filming, true)
                        .on_click(listeners.listener(|_, _: &ClickEvent, _, cx| {
                            prefs::update(cx, |s| s.record_camera = false);
                        })),
                )
                .child(
                    super::studio::chip("mode-camera", tr!("voice.mode_camera"), filming, true)
                        .on_click(listeners.listener(|_, _: &ClickEvent, _, cx| {
                            prefs::update(cx, |s| s.record_camera = true);
                        })),
                )
                .child(
                    super::studio::chip(
                        "devices-refresh",
                        if devices.loading {
                            tr!("voice.devices_loading")
                        } else {
                            tr!("voice.devices_refresh")
                        },
                        false,
                        !devices.loading,
                    )
                    .on_click(listeners.listener(
                        |shell, _: &ClickEvent, _, cx| {
                            shell.refresh_devices(cx);
                        },
                    )),
                ),
        );
        // A chosen device that is gone counts as the default one.
        let mic_known = mic
            .as_ref()
            .is_some_and(|id| devices.microphones.iter().any(|d| &d.id == id));
        let default_mic = devices.microphones.first().map_or_else(
            || tr!("voice.default"),
            |d| tr!("voice.default_named", name = short(&d.name)),
        );
        let mics = devices.microphones.iter().enumerate().map(|(i, d)| {
            let id = d.id.clone();
            super::studio::chip(("mic", i), short(&d.name), mic.as_ref() == Some(&id), true)
                .on_click(listeners.listener(move |_, _: &ClickEvent, _, cx| {
                    let id = id.clone();
                    prefs::update(cx, |s| s.microphone = Some(id));
                }))
        });
        let microphone = row(tr!("voice.microphone")).child(
            chips()
                .child(
                    super::studio::chip("mic-default", default_mic, !mic_known, true).on_click(
                        listeners.listener(|_, _: &ClickEvent, _, cx| {
                            prefs::update(cx, |s| s.microphone = None);
                        }),
                    ),
                )
                .children(mics),
        );
        let camera = filming.then(|| {
            let known = cam
                .as_ref()
                .is_some_and(|id| devices.cameras.iter().any(|d| &d.id == id));
            let default = devices.cameras.first().map_or_else(
                || tr!("voice.default"),
                |d| tr!("voice.default_named", name = short(&d.name)),
            );
            let list = devices.cameras.iter().enumerate().map(|(i, d)| {
                let id = d.id.clone();
                super::studio::chip(("cam", i), short(&d.name), cam.as_ref() == Some(&id), true)
                    .on_click(listeners.listener(move |_, _: &ClickEvent, _, cx| {
                        let id = id.clone();
                        prefs::update(cx, |s| s.camera = Some(id));
                    }))
            });
            let none = !devices.loading && devices.cameras.is_empty();
            row(tr!("voice.camera")).child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(if none {
                        div()
                            .pt(px(5.))
                            .text_size(px(12.))
                            .text_color(color(tokens::MIXED))
                            .child(tr!("voice.no_camera"))
                    } else {
                        chips()
                            .child(
                                super::studio::chip("cam-default", default, !known, true).on_click(
                                    listeners.listener(|_, _: &ClickEvent, _, cx| {
                                        prefs::update(cx, |s| s.camera = None);
                                    }),
                                ),
                            )
                            .children(list)
                    })
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(color(tokens::FG2))
                            .child(tr!(
                                "voice.camera_framing",
                                format = super::studio::format_name(self.settings().aspect)
                            )),
                    ),
            )
        });
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(mode)
            .child(microphone)
            .children(camera)
    }
}
