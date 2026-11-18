//! Generating animations from the interface: job queue, engine, retouches
//! anchored in time, remembered rules, preview.

use super::{STEP_FINISH, STEP_STORYBOARD, Shell, label, mono};
use crate::jobs::{JobKind, JobStatus};
use crate::theme::{color, tokens};
use crate::tr;
use crate::worker::{
    Snapshot, broll_ready, clip_path, preview_path, transition_clip_path, versions,
};
use enfait_ai::{ProviderChoice, Quality};
use enfait_core::{
    AspectRatio, Block, BlockId, BlockKind, Edit, Framing, PAN_LIMIT, ProjectSettings, Transition,
};
use gpui_kit::component::input::Input;
use gpui_kit::prelude::*;
use gpui_kit::{ClickEvent, Context, Div, ElementId, SharedString, Stateful, Window, div, px};
use std::time::Duration;

/// Where the animation of a block stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BlockState {
    /// Not an animated block.
    NotAnimated,
    ToGenerate,
    Queued,
    Running,
    Failed(String),
    Ready,
    /// B-roll block without a chosen shot.
    NeedsBroll,
    /// B-roll block: shot chosen, clip being edited.
    Mounting,
    /// B-roll block: shot edited.
    Mounted,
}

impl BlockState {
    pub(super) fn label(&self) -> Option<SharedString> {
        Some(match self {
            Self::NotAnimated => return None,
            Self::ToGenerate => tr!("studio.state.to_generate"),
            Self::Queued => tr!("studio.state.queued"),
            Self::Running => tr!("studio.state.running"),
            Self::Failed(_) => tr!("studio.state.failed"),
            Self::Ready => tr!("studio.state.ready"),
            Self::NeedsBroll => tr!("studio.state.needs_broll"),
            Self::Mounting => tr!("studio.state.mounting"),
            Self::Mounted => tr!("studio.state.mounted"),
        })
    }

    pub(super) const fn tone(&self) -> crate::theme::Token {
        match self {
            Self::NotAnimated | Self::ToGenerate | Self::NeedsBroll => tokens::MUTE,
            Self::Queued | Self::Running | Self::Mounting => tokens::FG2,
            Self::Failed(_) => tokens::ERROR,
            Self::Ready | Self::Mounted => tokens::SIGNAL,
        }
    }
}

pub(super) const QUALITIES: [Quality; 3] = [Quality::Fast, Quality::Polished, Quality::Ultra];

pub(super) fn quality_name(quality: Quality) -> SharedString {
    match quality {
        Quality::Fast => tr!("studio.quality.fast"),
        Quality::Polished => tr!("studio.quality.polished"),
        Quality::Ultra => tr!("studio.quality.ultra"),
    }
}

pub(super) fn provider_name(provider: ProviderChoice) -> SharedString {
    match provider {
        ProviderChoice::Auto => tr!("studio.provider.auto"),
        ProviderChoice::Api => tr!("studio.provider.api"),
        ProviderChoice::ClaudeCode => tr!("studio.provider.claude_code"),
        ProviderChoice::Local => tr!("studio.provider.local"),
    }
}

/// Name of a video format in the interface.
pub(super) fn format_name(aspect: AspectRatio) -> SharedString {
    match aspect {
        AspectRatio::Landscape16x9 => tr!("studio.format.landscape"),
        AspectRatio::Portrait9x16 => tr!("studio.format.vertical"),
        AspectRatio::Square1x1 => tr!("studio.format.square"),
    }
}

pub(super) fn chip(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    on: bool,
    enabled: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .h(px(24.))
        .px(px(8.))
        .rounded(px(3.))
        .text_size(px(12.))
        .map(|d| {
            if on {
                d.bg(color(tokens::SIGNAL))
                    .text_color(color(tokens::ON_SIGNAL))
            } else if enabled {
                d.border_1()
                    .border_color(color(tokens::LINE_STRONG))
                    .text_color(color(tokens::FG))
                    .cursor_pointer()
                    .hover(|d| d.bg(color(tokens::HOVER)))
            } else {
                d.border_1()
                    .border_color(color(tokens::LINE))
                    .text_color(color(tokens::LINE_STRONG))
            }
        })
        .child(text.into())
}

/// What is missing to use the chosen provider.
pub(super) fn provider_hint(
    choice: ProviderChoice,
    settings: &enfait_config::Settings,
) -> Option<SharedString> {
    use enfait_config::{Secret, secrets};
    match choice {
        ProviderChoice::Api if !secrets::is_set(Secret::Anthropic) => Some(tr!("studio.missing.api")),
        ProviderChoice::ClaudeCode if enfait_config::process::which("claude").is_none() => {
            Some(tr!("studio.missing.claude"))
        }
        ProviderChoice::Local if settings.local_model().is_none() => {
            Some(tr!("studio.missing.local"))
        }
        _ => None,
    }
}

/// `M:SS.d`.
fn at(t: f64) -> String {
    let t = t.max(0.0);
    format!("{}:{:04.1}", (t / 60.0).floor(), t % 60.0)
}

impl Shell {
    /// Polls the engine: refreshes the screen and the preview when it moved on.
    pub(super) fn start_polling(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(400))
                    .await;
                let alive = this.update(cx, |shell, cx| {
                    shell.poll_bridge(cx);
                    if shell.worker.take_changed() {
                        shell.poll_written(cx);
                        shell.refresh_preview(cx);
                        cx.notify();
                    }
                    shell.poll_magic(cx);
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Copies the state of the project for the engine.
    pub(super) fn push_snapshot(&self) {
        let (Some(board), Some(rush)) = (&self.board, &self.session.data.rush) else {
            return;
        };
        let rush_video = enfait_media::MediaCache::new(super::media_cache())
            .dir_for(rush)
            .ok()
            .and_then(|dir| std::fs::read(dir.join("info.json")).ok())
            .and_then(|d| serde_json::from_slice::<enfait_media::MediaInfo>(&d).ok())
            .and_then(|info| info.video);
        // B-roll shots changed (choice, start, duration, block kind): edit
        // them again and rebuild the preview.
        let settings = *board.project.settings();
        let mut h = blake3::Hasher::new();
        for block in board.project.blocks() {
            if block.kind == BlockKind::Broll {
                h.update(block.id.to_string().as_bytes());
                h.update(
                    crate::worker::broll_key(block, &settings)
                        .unwrap_or_default()
                        .as_bytes(),
                );
                h.update(&[0]);
            }
        }
        // Format or framing changed: the preview must be rebuilt too.
        h.update(format!("{settings:?}").as_bytes());
        let sig = h.finalize();
        let brolls_changed = self.broll_sig.replace(Some(sig)) != Some(sig);
        self.worker.set_snapshot(Snapshot {
            project: board.project.clone(),
            transcript: board.transcript.clone(),
            look: self.session.look.clone(),
            library: self.session.library_dir(),
            script: self.session.script.clone(),
            rush: rush.clone(),
            rush_has_video: rush_video.is_some(),
            rush_size: rush_video.map(|v| (v.width, v.height)),
            parts: self.cut.ranges(),
            finish: self.session.finish.clone(),
            provider: self.session.data.provider,
        });
        if brolls_changed {
            self.worker.request_preview();
        }
    }

    pub(super) fn block_state(&self, block: &Block) -> BlockState {
        let (id, kind) = (block.id, block.kind);
        match kind {
            BlockKind::Rush => return BlockState::NotAnimated,
            BlockKind::Broll => {
                let settings = self.settings();
                return if block.broll.is_none() {
                    BlockState::NeedsBroll
                } else if broll_ready(&self.session.studio_dir(), block, &settings) {
                    BlockState::Mounted
                } else {
                    BlockState::Mounting
                };
            }
            _ => {}
        }
        let latest = self.queue.lock().ok().and_then(|q| q.latest(id).cloned());
        match latest.map(|j| j.status) {
            Some(JobStatus::Queued) => return BlockState::Queued,
            Some(JobStatus::Running) => return BlockState::Running,
            Some(JobStatus::Failed(e)) if !self.has_clip(id) => return BlockState::Failed(e),
            _ => {}
        }
        if self.has_clip(id) {
            BlockState::Ready
        } else {
            BlockState::ToGenerate
        }
    }

    fn has_clip(&self, id: BlockId) -> bool {
        clip_path(&self.session.studio_dir(), id, &self.settings()).is_file()
    }

    /// State of the transition after a block (`None`: hard cut).
    pub(super) fn transition_state(
        &self,
        id: BlockId,
        transition: &Transition,
    ) -> Option<BlockState> {
        if *transition == Transition::Cut {
            return None;
        }
        let latest = self
            .queue
            .lock()
            .ok()
            .and_then(|q| q.latest_transition(id).cloned());
        let ready =
            transition_clip_path(&self.session.studio_dir(), id, &self.settings()).is_file();
        Some(match latest.map(|j| j.status) {
            Some(JobStatus::Queued) => BlockState::Queued,
            Some(JobStatus::Running) => BlockState::Running,
            Some(JobStatus::Failed(e)) if !ready => BlockState::Failed(e),
            _ if ready => BlockState::Ready,
            _ => BlockState::ToGenerate,
        })
    }

    pub(super) fn generate_transition(&mut self, block: BlockId, cx: &mut Context<Self>) {
        self.enqueue(block, JobKind::Transition, cx);
    }

    pub(super) fn enqueue(&mut self, block: BlockId, kind: JobKind, cx: &mut Context<Self>) {
        self.push_snapshot();
        let quality = self.session.data.quality;
        if let Ok(mut q) = self.queue.lock() {
            q.push(block, kind, quality);
        }
        self.worker.wake();
        cx.notify();
    }

    pub(super) fn generate(&mut self, block: BlockId, cx: &mut Context<Self>) {
        self.enqueue(block, JobKind::Create, cx);
    }

    /// Queues every block to generate, then their transitions (they come
    /// after the animations of both shots in the queue).
    pub(super) fn generate_all(&mut self, cx: &mut Context<Self>) {
        let Some(board) = &self.board else { return };
        let blocks = board.project.blocks();
        let ids: Vec<BlockId> = blocks
            .iter()
            .filter(|b| self.block_state(b) == BlockState::ToGenerate)
            .map(|b| b.id)
            .collect();
        let transitions: Vec<BlockId> = blocks
            .iter()
            .take(blocks.len().saturating_sub(1))
            .filter(|b| {
                self.transition_state(b.id, &b.transition_after) == Some(BlockState::ToGenerate)
            })
            .map(|b| b.id)
            .collect();
        for id in ids {
            self.enqueue(id, JobKind::Create, cx);
        }
        for id in transitions {
            self.enqueue(id, JobKind::Transition, cx);
        }
    }

    pub(super) fn set_provider(&mut self, provider: ProviderChoice, cx: &mut Context<Self>) {
        self.session.data.provider = provider;
        self.session.save_data();
        self.push_snapshot();
        cx.notify();
    }

    /// Changes the format or framing of the project (undoable). Animations
    /// of another format are kept aside and come back with it.
    fn set_settings(&mut self, settings: ProjectSettings, cx: &mut Context<Self>) {
        let Some(board) = &mut self.board else {
            return;
        };
        if *board.project.settings() == settings {
            return;
        }
        if board.apply(Edit::SetSettings { settings }).is_ok() {
            self.save_board();
            cx.notify();
        }
    }

    /// Project format; a take of another format is cropped to fill the frame.
    fn set_format(&mut self, aspect: AspectRatio, cx: &mut Context<Self>) {
        let mut settings = self.settings().with_aspect(aspect);
        settings.framing = match self.rush_aspect(cx) {
            Some(rush) if rush != aspect => Framing::Fill,
            _ => Framing::Fit,
        };
        settings.pan = 0;
        self.set_settings(settings, cx);
    }

    /// Format closest to the frames of the take.
    fn rush_aspect(&self, cx: &Context<Self>) -> Option<AspectRatio> {
        let video = self.player.read(cx).media_info()?.video?;
        Some(AspectRatio::nearest(video.width, video.height))
    }

    pub(super) fn set_quality(&mut self, quality: Quality, cx: &mut Context<Self>) {
        self.session.data.quality = quality;
        self.session.save_data();
        cx.notify();
    }

    /// Adds the remark of the retouch field, at the playhead.
    pub(super) fn add_retouch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.retouch.read(cx).value().trim().to_owned();
        if text.is_empty() {
            return;
        }
        let Some(range) = self.board.as_ref().and_then(|b| b.block()).map(|b| b.range) else {
            return;
        };
        let start = range.start().as_secs_f64();
        let duration = range.duration().as_secs_f64();
        let position = self.player.read(cx).position().unwrap_or(start);
        let t = (position - start).clamp(0.0, duration);
        self.retouches.push((t, text));
        self.retouch.update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
    }

    fn send_retouches(&mut self, cx: &mut Context<Self>) {
        let Some(block) = self.board.as_ref().and_then(|b| b.selected) else {
            return;
        };
        let notes = std::mem::take(&mut self.retouches);
        if notes.is_empty() {
            return;
        }
        if self.remember {
            for (_, note) in &notes {
                if !self.session.look.memory.contains(note) {
                    self.session.look.memory.push(note.clone());
                }
            }
            self.session.save_look();
            self.remember = false;
        }
        self.enqueue(block, JobKind::Revise { notes }, cx);
    }

    pub(super) fn forget_rule(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.session.look.memory.len() {
            self.session.look.memory.remove(index);
            self.session.save_look();
            cx.notify();
        }
    }

    /// Storyboard: the preview (if any) replaces the edited take.
    pub(super) fn refresh_preview(&mut self, cx: &mut Context<Self>) {
        if self.step != STEP_STORYBOARD && self.step != STEP_FINISH {
            return;
        }
        let path = preview_path(&self.session.studio_dir());
        let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        match stamp {
            Some(stamp) if Some(stamp) != self.preview_stamp || !self.showing_preview => {
                let position = self.player.read(cx).position();
                self.preview_stamp = Some(stamp);
                self.showing_preview = true;
                self.player.update(cx, |p, cx| {
                    p.set_cut(None, cx);
                    p.open(path, cx);
                    if let Some(t) = position {
                        p.queue_seek(t);
                    }
                });
            }
            Some(_) => {}
            None if self.showing_preview => {
                self.showing_preview = false;
                self.preview_stamp = None;
                self.open_rush(cx);
                self.apply_cut(cx);
            }
            None => {}
        }
    }

    /// Storyboard bar: quality, "Generate all", engine activity.
    pub(super) fn studio_toolbar(&self, cx: &mut Context<Self>) -> Div {
        let quality = self.session.data.quality;
        let (running, queued) = self.queue.lock().map(|q| q.counts()).unwrap_or((0, 0));
        let writing = self.worker.writing();
        let activity = writing
            .clone()
            .map(|w| tr!("studio.writing", what = w).to_string())
            .or_else(|| self.worker.activity())
            .or_else(|| {
                (running + queued > 0)
                    .then(|| tr!("studio.counts", running = running, queued = queued).to_string())
            });
        let notice = if writing.is_some() {
            None
        } else {
            self.write_notice.clone()
        };
        let listeners: &Context<Self> = cx;
        let provider = self.session.data.provider;
        let providers = ProviderChoice::ALL.iter().enumerate().map(|(i, p)| {
            let p = *p;
            chip(("provider", i), provider_name(p), provider == p, true).on_click(
                listeners.listener(move |shell, _: &ClickEvent, _, cx| shell.set_provider(p, cx)),
            )
        });
        let missing = provider_hint(provider, crate::prefs::settings(cx));
        let qualities = QUALITIES.iter().enumerate().map(|(i, q)| {
            let q = *q;
            chip(("quality", i), quality_name(q), quality == q, true).on_click(
                listeners.listener(move |shell, _: &ClickEvent, _, cx| shell.set_quality(q, cx)),
            )
        });
        let settings = self.settings();
        let formats = AspectRatio::ALL.iter().enumerate().map(|(i, a)| {
            let a = *a;
            chip(("format", i), format_name(a), settings.aspect == a, true).on_click(
                listeners.listener(move |shell, _: &ClickEvent, _, cx| shell.set_format(a, cx)),
            )
        });
        let reframed = self
            .rush_aspect(cx)
            .is_some_and(|rush| rush != settings.aspect);
        let framing = reframed.then(|| self.framing_chips(settings, listeners));
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .px(px(20.))
            .py(px(8.))
            .border_b_1()
            .border_color(color(tokens::LINE))
            .child(label(tr!("studio.label.format")))
            .children(formats)
            .when_some(framing, |d, f| d.child(f))
            .child(label(tr!("studio.label.animations")).ml(px(8.)))
            .children(qualities)
            .child(label(tr!("studio.label.models")).ml(px(8.)))
            .children(providers)
            .when_some(missing, |d, m| {
                d.child(
                    div()
                        .id("provider-missing")
                        .cursor_pointer()
                        .text_size(px(11.))
                        .text_color(color(tokens::MIXED))
                        .hover(|d| d.underline())
                        .child(m)
                        .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| {
                            shell.open_settings(cx);
                        })),
                )
            })
            .child({
                let writing = writing.is_some();
                chip("write-board", tr!("studio.write_board"), false, !writing).when(!writing, |d| {
                    d.on_click(
                        cx.listener(|shell, _: &ClickEvent, _, cx| shell.write_storyboard(cx)),
                    )
                })
            })
            .child(
                chip("generate-all", tr!("studio.generate_all"), false, true)
                    .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| shell.generate_all(cx))),
            )
            .when(queued > 0, |d| {
                d.child(
                    chip("cancel-all", tr!("studio.cancel_queue"), false, true).on_click(cx.listener(
                        |shell, _: &ClickEvent, _, cx| {
                            if let Ok(mut q) = shell.queue.lock() {
                                q.cancel_queued(None);
                            }
                            cx.notify();
                        },
                    )),
                )
            })
            .when_some(activity, |d, a| {
                d.child(
                    div()
                        .text_size(px(12.))
                        .text_color(color(tokens::FG2))
                        .child(SharedString::from(a)),
                )
            })
            .when_some(notice, |d, n| {
                d.child(
                    div()
                        .w_full()
                        .text_size(px(12.))
                        .text_color(color(tokens::FG2))
                        .child(n),
                )
            })
    }

    /// Framing of a take of another format: cropped (can be shifted) or
    /// whole with black bars.
    fn framing_chips(&self, settings: ProjectSettings, cx: &Context<Self>) -> Div {
        let fill = settings.framing == Framing::Fill;
        let set = |framing: Framing, pan: i8| {
            cx.listener(move |shell, _: &ClickEvent, _, cx| {
                let mut s = shell.settings();
                s.framing = framing;
                s.pan = pan.clamp(-PAN_LIMIT, PAN_LIMIT);
                shell.set_settings(s, cx);
            })
        };
        let pan = settings.pan();
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .ml(px(4.))
            .child(chip("framing-fill", tr!("studio.framing.fill"), fill, true).on_click(set(Framing::Fill, pan)))
            .child(chip("framing-fit", tr!("studio.framing.fit"), !fill, true).on_click(set(Framing::Fit, 0)))
            .when(fill, |d| {
                d.child(
                    chip("pan-left", "◀", false, pan > -PAN_LIMIT)
                        .on_click(set(Framing::Fill, pan.saturating_sub(10))),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(color(tokens::FG2))
                        .child(match pan {
                            0 => tr!("studio.pan.center"),
                            p if p < 0 => tr!("studio.pan.left", pct = -i16::from(p) * 2),
                            p => tr!("studio.pan.right", pct = i16::from(p) * 2),
                        }),
                )
                .child(
                    chip("pan-right", "▶", false, pan < PAN_LIMIT)
                        .on_click(set(Framing::Fill, pan.saturating_add(10))),
                )
            })
    }

    /// State and button of the transition to the next block.
    pub(super) fn transition_controls(&self, cx: &mut Context<Self>) -> Option<Div> {
        let block = self.board.as_ref()?.block()?;
        let id = block.id;
        let state = self.transition_state(id, &block.transition_after)?;
        let busy = matches!(state, BlockState::Queued | BlockState::Running);
        let ready = state == BlockState::Ready;
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(color(state.tone()))
                    .child(match state {
                        BlockState::Ready => tr!("studio.transition_ready"),
                        _ => state.label().unwrap_or_default(),
                    }),
            )
            .child(
                chip(
                    "generate-transition",
                    if ready {
                        tr!("studio.regenerate_transition")
                    } else {
                        tr!("studio.generate_transition")
                    },
                    false,
                    !busy,
                )
                .when(!busy, |d| {
                    d.on_click(cx.listener(move |shell, _: &ClickEvent, _, cx| {
                        shell.generate_transition(id, cx);
                    }))
                }),
            );
        if let BlockState::Failed(e) = state {
            row = row.child(
                div()
                    .text_size(px(11.))
                    .text_color(color(tokens::ERROR))
                    .child(SharedString::from(e)),
            );
        }
        Some(row)
    }

    /// "Animation" section of the inspector.
    pub(super) fn studio_inspector(&self, cx: &mut Context<Self>) -> Option<Div> {
        let block = self.board.as_ref()?.block()?;
        let id = block.id;
        if block.kind == BlockKind::Broll {
            return Some(self.broll_inspector(block, cx));
        }
        let state = self.block_state(block);
        let cutout = (block.kind == BlockKind::Mixed).then(|| self.cutout_toggle(block, cx));
        let listeners: &Context<Self> = cx;
        let mut section = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(label(tr!("studio.label.animation")))
            .when_some(cutout, |d, c| d.child(c));
        if state == BlockState::NotAnimated {
            let text = match block.kind {
                BlockKind::Rush => tr!("studio.rush_note"),
                _ => tr!("studio.overlay_note"),
            };
            return Some(
                section.child(
                    div()
                        .text_size(px(12.))
                        .text_color(color(tokens::FG2))
                        .child(text),
                ),
            );
        }
        let busy = matches!(state, BlockState::Queued | BlockState::Running);
        let ready = state == BlockState::Ready;
        section = section.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(color(state.tone()))
                        .child(state.label().unwrap_or_default()),
                )
                .child(
                    chip(
                        "generate",
                        if ready {
                            tr!("studio.regenerate")
                        } else {
                            tr!("studio.generate")
                        },
                        false,
                        !busy,
                    )
                    .when(!busy, |d| {
                        d.on_click(listeners.listener(move |shell, _: &ClickEvent, _, cx| {
                            shell.generate(id, cx);
                        }))
                    }),
                ),
        );
        if let BlockState::Failed(e) = &state {
            section = section.child(
                div()
                    .text_size(px(11.))
                    .text_color(color(tokens::ERROR))
                    .child(SharedString::from(e.clone())),
            );
        }
        if !ready {
            return Some(section);
        }
        let versions = versions(&self.session.studio_dir(), id, &self.settings());
        if !versions.is_empty() {
            let chips = versions.iter().map(|&v| {
                chip(("version", v as usize), format!("v{v}"), false, !busy).when(!busy, |d| {
                    d.on_click(listeners.listener(move |shell, _: &ClickEvent, _, cx| {
                        shell.enqueue(id, JobKind::Restore { version: v }, cx);
                    }))
                })
            });
            section = section
                .child(label(tr!("studio.label.versions")))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(4.))
                        .children(chips)
                        .child(chip("current", tr!("studio.current_version"), true, false)),
                );
        }
        let start = block.range.start().as_secs_f64();
        let notes = self.retouches.iter().enumerate().map(|(i, (t, note))| {
            div()
                .flex()
                .gap(px(8.))
                .text_size(px(12.))
                .child(mono(at(start + t)).text_color(color(tokens::MUTE)))
                .child(div().flex_1().child(SharedString::from(note.clone())))
                .child(
                    div()
                        .id(("drop-retouch", i))
                        .px(px(4.))
                        .cursor_pointer()
                        .text_color(color(tokens::MUTE))
                        .hover(|d| d.text_color(color(tokens::ERROR)))
                        .child("×")
                        .on_click(listeners.listener(move |shell, _: &ClickEvent, _, cx| {
                            if i < shell.retouches.len() {
                                shell.retouches.remove(i);
                            }
                            cx.notify();
                        })),
                )
        });
        let count = self.retouches.len();
        let remember = self.remember;
        Some(
            section
                .child(label(tr!("studio.label.retouches")))
                .child(Input::new(&self.retouch))
                .children(notes)
                .when(count > 0, |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(4.))
                            .child(
                                chip("remember", tr!("studio.remember"), remember, true).on_click(
                                    listeners.listener(|shell, _: &ClickEvent, _, cx| {
                                        shell.remember = !shell.remember;
                                        cx.notify();
                                    }),
                                ),
                            )
                            .child(
                                chip(
                                    "send-retouches",
                                    tr!("studio.send_retouches", count = count),
                                    false,
                                    !busy,
                                )
                                .when(!busy, |d| {
                                    d.on_click(listeners.listener(
                                        |shell, _: &ClickEvent, _, cx| shell.send_retouches(cx),
                                    ))
                                }),
                            ),
                    )
                }),
        )
    }
}
