//! Finish screen: voice, music and sound effects on the edit, ducking, and
//! auditioning the library sounds.
//!
//! Music and SFX are anchored on storyboard blocks (see
//! `enfait_core::finish`). Every change is saved (`finish.json`), undoable,
//! and only rebuilds the sound of the preview.

use super::{Shell, label, mono};
use crate::theme::{color, tokens};
use crate::tr;
use crate::worker::{ExportKind, ExportRequest, ExportState, ThumbnailRequest, ThumbnailState};
use enfait_core::finish::Placed;
use enfait_core::{AssetKind, BlockId, CueId, Finish, Transition};
use enfait_media::{AudioBuffer, Clock, Command, Edl, Playback};
use enfait_studio::thumbnail::{self, Version};
use gpui_kit::component::input::Input;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui_kit::prelude::*;
use gpui_kit::{
    ClickEvent, Context, Div, ElementId, Entity, FontWeight, ObjectFit, SharedString, Stateful,
    Subscription, Window, div, img, px, relative,
};
use std::sync::Arc;

/// State of the YouTube thumbnail panel.
#[derive(Debug, Default)]
pub(super) struct ThumbUi {
    /// The panel is unfolded.
    pub open: bool,
    /// Proposal shown.
    pub shown: Option<u32>,
    /// Proposals on disk, oldest first.
    pub versions: Vec<Version>,
    /// Last proposal announced by the engine, already taken into account.
    seen: Option<u32>,
}

/// Lead of a SFX placed on a transition (the "whoosh" peaks on the cut), in
/// seconds.
const TRANSITION_LEAD: f64 = 0.25;
/// Longest audition of a library sound.
const AUDITION_SECONDS: u32 = 60;

/// A finish setting, on a slider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Knob {
    /// Gain of the selected music or SFX.
    Gain,
    FadeIn,
    FadeOut,
    /// Dip of the music under the voice.
    Depth,
    /// Gain of the voice.
    Voice,
}

impl Knob {
    const ALL: [Self; 5] = [
        Self::Gain,
        Self::FadeIn,
        Self::FadeOut,
        Self::Depth,
        Self::Voice,
    ];

    const fn range(self) -> (f32, f32, f32) {
        match self {
            Self::Gain => (-30.0, 6.0, 1.0),
            Self::FadeIn | Self::FadeOut => (0.0, 5.0, 0.1),
            Self::Depth => (-30.0, 0.0, 1.0),
            Self::Voice => (-12.0, 12.0, 0.5),
        }
    }

    fn label(self) -> SharedString {
        match self {
            Self::Gain => tr!("finish.knob.gain"),
            Self::FadeIn => tr!("finish.knob.fade_in"),
            Self::FadeOut => tr!("finish.knob.fade_out"),
            Self::Depth => tr!("finish.knob.depth"),
            Self::Voice => tr!("finish.knob.voice"),
        }
    }

    fn unit(self, v: f32) -> String {
        match self {
            Self::FadeIn | Self::FadeOut => {
                tr!("finish.seconds", value = format!("{v:.1}")).to_string()
            }
            _ => format!("{v:+.0} dB"),
        }
    }

    /// Current value in `finish` (for the music or SFX `cue`).
    fn get(self, finish: &Finish, cue: Option<CueId>) -> f32 {
        let music = cue.and_then(|id| finish.music.iter().find(|m| m.id == id));
        let sfx = cue.and_then(|id| finish.sfx.iter().find(|s| s.id == id));
        // Short fade durations: the conversion loses nothing useful.
        #[allow(clippy::cast_possible_truncation)]
        match self {
            Self::Gain => music
                .map(|m| m.gain_db)
                .or_else(|| sfx.map(|s| s.gain_db))
                .unwrap_or(0.0),
            Self::FadeIn => music.map_or(0.0, |m| m.fade_in as f32),
            Self::FadeOut => music.map_or(0.0, |m| m.fade_out as f32),
            Self::Depth => finish.ducking.depth_db,
            Self::Voice => finish.voice_gain_db,
        }
    }

    fn set(self, finish: &mut Finish, cue: Option<CueId>, v: f32) {
        match self {
            Self::Gain => {
                if let Some(m) = cue.and_then(|id| finish.music_mut(id)) {
                    m.gain_db = v;
                } else if let Some(s) = cue.and_then(|id| finish.sfx_mut(id)) {
                    s.gain_db = v;
                }
            }
            Self::FadeIn | Self::FadeOut => {
                if let Some(m) = cue.and_then(|id| finish.music_mut(id)) {
                    if self == Self::FadeIn {
                        m.fade_in = f64::from(v);
                    } else {
                        m.fade_out = f64::from(v);
                    }
                }
            }
            Self::Depth => finish.ducking.depth_db = v,
            Self::Voice => finish.voice_gain_db = v,
        }
    }
}

/// Finish sliders and their subscriptions (release = apply).
pub(super) fn knobs(
    finish: &Finish,
    cx: &mut Context<Shell>,
) -> (Vec<(Knob, Entity<SliderState>)>, Vec<Subscription>) {
    let mut out = Vec::new();
    let mut subscriptions = Vec::new();
    for knob in Knob::ALL {
        let (min, max, step) = knob.range();
        let state = cx.new(|_| {
            SliderState::new()
                .min(min)
                .max(max)
                .step(step)
                .default_value(knob.get(finish, None).clamp(min, max))
        });
        subscriptions.push(cx.subscribe(
            &state,
            move |shell: &mut Shell, _, event: &SliderEvent, cx| {
                if let SliderEvent::Release(SliderValue::Single(v) | SliderValue::Range(_, v)) =
                    event
                {
                    let cue = shell.selected_cue;
                    if (knob.get(&shell.session.finish, cue) - v).abs() > 1e-4 {
                        shell.edit_finish(|f| knob.set(f, cue, *v), cx);
                    }
                }
            },
        ));
        out.push((knob, state));
    }
    (out, subscriptions)
}

/// A library sound being auditioned.
pub(super) struct Audition {
    file: String,
    clock: Arc<Clock>,
    _playback: Playback,
}

impl Audition {
    fn playing(&self) -> bool {
        self.clock.is_playing() && self.clock.position() < self.clock.duration()
    }
}

/// Decodes the start of a sound (48 kHz, stereo) to audition it.
fn decode(path: &std::path::Path) -> Result<AudioBuffer, String> {
    let output = enfait_config::process::command("ffmpeg")
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args(["-t", &AUDITION_SECONDS.to_string()])
        .args(["-f", "s16le", "-ac", "2", "-ar", "48000", "-"])
        .output()
        .map_err(|e| format!("ffmpeg: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    let samples = output
        .stdout
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b))
        .collect();
    AudioBuffer::new(48_000, 2, samples).map_err(|e| e.to_string())
}

fn chip(
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
        .h(px(22.))
        .px(px(7.))
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

fn small(text: impl Into<SharedString>, tone: crate::theme::Token) -> Div {
    div()
        .text_size(px(11.))
        .text_color(color(tone))
        .child(text.into())
}

/// Fraction of the total duration, to place an item on a track.
#[allow(clippy::cast_possible_truncation)]
fn frac(t: f64, total: f64) -> f32 {
    (t / total.max(1e-3)).clamp(0.0, 1.0) as f32
}

impl Shell {
    /// Changes the finish: history, save, new mix.
    pub(super) fn edit_finish(&mut self, change: impl FnOnce(&mut Finish), cx: &mut Context<Self>) {
        let before = self.session.finish.clone();
        change(&mut self.session.finish);
        if self.session.finish == before {
            return;
        }
        self.finish_history.record(&before);
        self.finish_changed(cx);
    }

    fn finish_changed(&mut self, cx: &mut Context<Self>) {
        self.session.save_finish();
        self.push_snapshot();
        self.worker.request_mix();
        cx.notify();
    }

    pub(super) fn finish_undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.finish_history.undo(&mut self.session.finish) {
            self.finish_changed(cx);
            self.sync_knobs(window, cx);
        }
    }

    pub(super) fn finish_redo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.finish_history.redo(&mut self.session.finish) {
            self.finish_changed(cx);
            self.sync_knobs(window, cx);
        }
    }

    /// Copies the finish values into the sliders.
    pub(super) fn sync_knobs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cue = self.selected_cue;
        for (knob, state) in &self.knobs {
            let (min, max, _) = knob.range();
            let v = knob.get(&self.session.finish, cue).clamp(min, max);
            state.update(cx, |s, cx| s.set_value(v, window, cx));
        }
    }

    fn select_cue(&mut self, cue: Option<CueId>, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_cue = cue;
        self.sync_knobs(window, cx);
        cx.notify();
    }

    /// Playhead, in edit time.
    fn playhead(&self, cx: &Context<Self>) -> f64 {
        self.player.read(cx).position().unwrap_or(0.0)
    }

    fn add_music(&mut self, file: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(board) = &self.board else { return };
        let blocks = board.project.blocks();
        let (Some(first), Some(last)) = (blocks.first(), blocks.last()) else {
            return;
        };
        let (from, to) = (first.id, last.id);
        let mut id = None;
        self.edit_finish(|f| id = Some(f.add_music(file, from, to)), cx);
        self.select_cue(id, window, cx);
    }

    fn add_sfx_here(&mut self, file: String, window: &mut Window, cx: &mut Context<Self>) {
        let t = self.playhead(cx);
        let Some(board) = &self.board else { return };
        let Some(block) = board.block_at(t).and_then(|id| board.project.block(id)) else {
            return;
        };
        let (id, offset) = (block.id, t - block.range.start().as_secs_f64());
        let mut cue = None;
        self.edit_finish(|f| cue = Some(f.add_sfx(file, id, offset)), cx);
        self.select_cue(cue, window, cx);
    }

    /// Places the sound on every transition (not hard cuts), just before the
    /// cut; no duplicate on a transition that already has it.
    fn sfx_on_transitions(&mut self, file: String, cx: &mut Context<Self>) {
        let Some(board) = &self.board else { return };
        let targets: Vec<BlockId> = board
            .project
            .blocks()
            .windows(2)
            .filter_map(|w| match w {
                [a, b] if a.transition_after != Transition::Cut => Some(b.id),
                _ => None,
            })
            .collect();
        if targets.is_empty() {
            self.finish_notice = Some(tr!("finish.no_transition"));
            cx.notify();
            return;
        }
        self.finish_notice = None;
        self.edit_finish(
            |f| {
                for block in targets {
                    let exists = f
                        .sfx
                        .iter()
                        .any(|s| s.block == block && s.file == file && s.offset < 0.0);
                    if !exists {
                        f.add_sfx(file.clone(), block, -TRANSITION_LEAD);
                    }
                }
            },
            cx,
        );
    }

    /// Auditions a library sound (or stops the current one).
    fn audition(&mut self, file: String, cx: &mut Context<Self>) {
        let same = self
            .audition
            .as_ref()
            .is_some_and(|a| a.file == file && a.playing());
        self.audition = None;
        if same {
            cx.notify();
            return;
        }
        self.command(Command::Pause, cx);
        let path = self.session.library_dir().join(&file);
        let work = cx.background_spawn(async move { decode(&path) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |shell, cx| {
                match result {
                    Ok(audio) => {
                        let duration = audio.duration();
                        let clock = Arc::new(Clock::new(duration, 0.04));
                        clock.apply(Command::Toggle);
                        let playback = Playback::start(
                            Arc::clone(&clock),
                            Some(audio),
                            Arc::new(Edl::identity(duration)),
                        );
                        shell.audition = Some(Audition {
                            file,
                            clock,
                            _playback: playback,
                        });
                    }
                    Err(e) => shell.finish_notice = Some(tr!("finish.audition_failed", error = e)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn asset_name(&self, file: &str) -> String {
        self.session
            .look
            .assets
            .iter()
            .find(|a| a.file == file)
            .map_or_else(|| file.to_owned(), |a| a.name.clone())
    }

    /// Finish screen: player, tracks, list of placed sounds.
    pub(super) fn finish_screen(&mut self, cx: &mut Context<Self>) -> Div {
        let player = self.player_pane(false, cx);
        let Some(board) = &self.board else {
            return div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .child(player)
                .child(
                    div()
                        .px(px(20.))
                        .py(px(16.))
                        .text_color(color(tokens::MUTE))
                        .child(tr!("finish.needs_board")),
                );
        };
        let finish = &self.session.finish;
        let placed = finish.resolve(&board.project);
        let total = board.project.duration().as_secs_f64().max(1e-3);
        let head = frac(self.playhead(cx), total);
        let selected = self.selected_cue;
        let listeners: &Context<Self> = cx;

        let lane = |name: SharedString| {
            div()
                .flex()
                .h(px(30.))
                .border_b_1()
                .border_color(color(tokens::LINE))
                .child(
                    div()
                        .w(px(80.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .px(px(12.))
                        .child(label(name)),
                )
        };
        let track = || div().flex_1().relative().h_full().overflow_hidden();

        // Painted in one pass: a long take gives thousands of blocks.
        let marks = board
            .project
            .blocks()
            .iter()
            .map(|b| super::lazy::Mark {
                start: frac(b.range.start().as_secs_f64(), total),
                width: frac(b.range.duration().as_secs_f64(), total),
                tone: super::kind_color(b.kind),
            })
            .collect();
        let starts: Vec<f64> = board
            .project
            .blocks()
            .iter()
            .map(|b| b.range.start().as_secs_f64())
            .collect();
        let shell = cx.entity().downgrade();
        let blocks = div()
            .absolute()
            .top(px(5.))
            .bottom(px(5.))
            .left_0()
            .right_0()
            .cursor_pointer()
            .child(super::lazy::painted_track(
                marks,
                0.55,
                1.0,
                move |share, _, cx| {
                    // Start of the block clicked.
                    let t = f64::from(share) * total;
                    let i = starts.partition_point(|s| *s <= t).saturating_sub(1);
                    let start = starts.get(i).copied().unwrap_or(0.0);
                    let _ = shell.update(cx, |shell, cx| {
                        shell.command(Command::Seek(start), cx);
                    });
                },
            ));
        let music_bars = placed.music.iter().map(|m| {
            let id = m.id;
            let on = selected == Some(id);
            div()
                .id(("music-bar", usize::try_from(id).unwrap_or(0)))
                .absolute()
                .top(px(4.))
                .bottom(px(4.))
                .left(relative(frac(m.start, total)))
                .w(relative(frac(m.end - m.start, total)))
                .px(px(6.))
                .flex()
                .items_center()
                .overflow_hidden()
                .rounded(px(3.))
                .cursor_pointer()
                .bg(color(tokens::MUSIC))
                .text_size(px(11.))
                .text_color(color(tokens::ON_SIGNAL))
                .when(on, |d| d.border_2().border_color(color(tokens::FG)))
                .child(SharedString::from(self.asset_name(&m.file)))
                .on_click(
                    listeners.listener(move |shell, _: &ClickEvent, window, cx| {
                        shell.select_cue(Some(id), window, cx);
                    }),
                )
        });
        let sfx_marks = placed.sfx.iter().map(|s| {
            let id = s.id;
            let on = selected == Some(id);
            div()
                .id(("sfx-mark", usize::try_from(id).unwrap_or(0)))
                .absolute()
                .top(px(4.))
                .bottom(px(4.))
                .left(relative(frac(s.at, total)))
                .w(px(8.))
                .rounded(px(2.))
                .cursor_pointer()
                .bg(color(tokens::SFX))
                .when(on, |d| d.border_2().border_color(color(tokens::FG)))
                .on_click(
                    listeners.listener(move |shell, _: &ClickEvent, window, cx| {
                        shell.select_cue(Some(id), window, cx);
                    }),
                )
        });
        let playhead = || {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(relative(head))
                .w(px(1.))
                .bg(color(tokens::SIGNAL))
        };
        let lanes = div()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(color(tokens::LINE))
            .child(lane(tr!("finish.lane.blocks")).child(track().child(blocks).child(playhead())))
            .child(
                lane(tr!("finish.lane.music"))
                    .child(track().children(music_bars).child(playhead())),
            )
            .child(
                lane(tr!("finish.lane.sfx")).child(track().children(sfx_marks).child(playhead())),
            );

        let rows = self.cue_rows(&placed, cx);
        let detail = tr!(
            "finish.stats",
            music = placed.music.len(),
            sfx = placed.sfx.len(),
            ducking = if finish.ducking.enabled {
                format!("{:+.0} dB", finish.ducking.depth_db)
            } else {
                tr!("finish.ducking_off").to_string()
            }
        )
        .to_string();
        let (can_undo, can_redo) = (
            self.finish_history.can_undo(),
            self.finish_history.can_redo(),
        );
        div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .child(player)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(20.))
                    .py(px(12.))
                    .border_b_1()
                    .border_color(color(tokens::LINE))
                    .child(
                        div()
                            .text_size(px(22.))
                            .line_height(px(28.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(tr!("finish.title")),
                    )
                    .child(
                        mono(detail)
                            .flex_1()
                            .text_size(px(12.))
                            .text_color(color(tokens::MUTE)),
                    )
                    .child(
                        chip("finish-undo", tr!("common.undo"), false, can_undo).when(
                            can_undo,
                            |d| {
                                d.on_click(cx.listener(|shell, _: &ClickEvent, window, cx| {
                                    shell.finish_undo(window, cx);
                                }))
                            },
                        ),
                    )
                    .child(
                        chip("finish-redo", tr!("common.redo"), false, can_redo).when(
                            can_redo,
                            |d| {
                                d.on_click(cx.listener(|shell, _: &ClickEvent, window, cx| {
                                    shell.finish_redo(window, cx);
                                }))
                            },
                        ),
                    ),
            )
            .child(lanes)
            .child(
                div()
                    .id("cues")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .children(rows),
            )
    }

    /// One row per music and per SFX (orphans last).
    fn cue_rows(&self, placed: &Placed, cx: &mut Context<Self>) -> Vec<Stateful<Div>> {
        let finish = &self.session.finish;
        let selected = self.selected_cue;
        let listeners: &Context<Self> = cx;
        let mut entries: Vec<(CueId, SharedString, crate::theme::Token, String, String)> =
            Vec::new();
        for m in &placed.music {
            entries.push((
                m.id,
                tr!("finish.kind.music"),
                tokens::MUSIC,
                self.asset_name(&m.file),
                format!("{} → {}", seconds(m.start), seconds(m.end)),
            ));
        }
        let mut sfx: Vec<_> = placed.sfx.iter().collect();
        sfx.sort_by(|a, b| a.at.total_cmp(&b.at));
        for s in sfx {
            entries.push((
                s.id,
                tr!("finish.kind.sfx"),
                tokens::SFX,
                self.asset_name(&s.file),
                seconds(s.at),
            ));
        }
        for id in &placed.orphans {
            let file = finish
                .music
                .iter()
                .find(|m| m.id == *id)
                .map(|m| m.file.clone())
                .or_else(|| {
                    finish
                        .sfx
                        .iter()
                        .find(|s| s.id == *id)
                        .map(|s| s.file.clone())
                })
                .unwrap_or_default();
            entries.push((
                *id,
                tr!("finish.kind.lost"),
                tokens::ERROR,
                self.asset_name(&file),
                tr!("finish.lost").to_string(),
            ));
        }
        if entries.is_empty() {
            return vec![
                div()
                    .id("no-cues")
                    .px(px(20.))
                    .py(px(14.))
                    .text_size(px(12.))
                    .text_color(color(tokens::FG2))
                    .child(tr!("finish.no_cues")),
            ];
        }
        entries
            .into_iter()
            .map(|(id, kind, tone, name, when)| {
                let on = selected == Some(id);
                div()
                    .id(("cue", usize::try_from(id).unwrap_or(0)))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(20.))
                    .h(px(30.))
                    .cursor_pointer()
                    .border_b_1()
                    .border_color(color(tokens::LINE))
                    .when(on, |d| d.bg(color(tokens::RAISED)))
                    .hover(|d| d.bg(color(tokens::HOVER)))
                    .child(div().size(px(8.)).bg(color(tone)))
                    .child(
                        div()
                            .w(px(64.))
                            .text_size(px(11.))
                            .text_color(color(tokens::MUTE))
                            .child(kind),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .child(SharedString::from(name)),
                    )
                    .child(
                        mono(when)
                            .text_size(px(11.))
                            .text_color(color(tokens::MUTE)),
                    )
                    .on_click(
                        listeners.listener(move |shell, _: &ClickEvent, window, cx| {
                            shell.select_cue(Some(id), window, cx);
                        }),
                    )
            })
            .collect()
    }

    fn knob(&self, knob: Knob) -> Option<Div> {
        let (_, state) = self.knobs.iter().find(|(k, _)| *k == knob)?;
        let value = knob.get(&self.session.finish, self.selected_cue);
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .text_size(px(12.))
                        .text_color(color(tokens::FG2))
                        .child(knob.label())
                        .child(
                            mono(knob.unit(value))
                                .text_size(px(11.))
                                .text_color(color(tokens::MUTE)),
                        ),
                )
                .child(Slider::new(state)),
        )
    }

    /// Right panel of the finish.
    pub(super) fn finish_inspector(&mut self, side: bool, cx: &mut Context<Self>) -> Div {
        // Audition over: "Listen" button again.
        if self.audition.as_ref().is_some_and(|a| !a.playing()) {
            self.audition = None;
        }
        let panel = div()
            .flex_none()
            .flex()
            .flex_col()
            .bg(color(tokens::PANEL))
            .when(side, |d| d.border_l_1().border_color(color(tokens::LINE)));
        let selection = self.selection_section(cx);
        let sounds = self.sounds_section(cx);
        let ducking = self.ducking_section(cx);
        self.poll_thumbnail();
        let export = self.export_section(cx);
        let youtube = self.youtube_section(cx);
        let notice = self.finish_notice.clone();
        panel.child(
            div()
                .id("finish-inspector")
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .p(px(16.))
                .flex()
                .flex_col()
                .gap(px(14.))
                .when_some(notice, |d, n| d.child(small(n, tokens::MIXED)))
                .when_some(selection, |d, s| d.child(s))
                .child(export)
                .when_some(youtube, |d, y| d.child(y))
                .child(sounds)
                .child(ducking),
        )
    }

    /// File name of an export, from the title of the project.
    fn export_stem(&self) -> String {
        let stem: String = self
            .title()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == ' ' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        if stem.trim().is_empty() {
            "enfait".to_owned()
        } else {
            stem.trim().to_owned()
        }
    }

    /// Asks where to write the export, then starts it.
    fn prompt_export(&mut self, kind: ExportKind, cx: &mut Context<Self>) {
        let dir = export_dir();
        let stem = self.export_stem();
        let name = match kind {
            ExportKind::Video => format!("{stem}.mp4"),
            ExportKind::Premiere => format!("{stem}.xml"),
        };
        let path = cx.prompt_for_new_path(&dir, Some(&name));
        let sfx = self.export_sfx;
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut path))) = path.await {
                let ext = match kind {
                    ExportKind::Video => "mp4",
                    ExportKind::Premiere => "xml",
                };
                if path
                    .extension()
                    .is_none_or(|e| !e.eq_ignore_ascii_case(ext))
                {
                    path.set_extension(ext);
                }
                let _ = this.update(cx, |shell, cx| {
                    shell.push_snapshot();
                    shell.worker.export(ExportRequest { kind, path, sfx });
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Export: final video or Premiere edit, with or without SFX.
    fn export_section(&self, cx: &mut Context<Self>) -> Div {
        let settings = self.settings();
        let state = self.worker.export_state();
        let running = matches!(state, ExportState::Running { .. });
        let sfx = self.export_sfx;
        let duration = self
            .board
            .as_ref()
            .map_or(0.0, |b| b.project.duration().as_secs_f64());
        let vertical = settings.aspect == enfait_core::AspectRatio::Portrait9x16;
        let rate = settings.frame_rate;
        let summary = tr!(
            "finish.export_summary",
            format = super::studio::format_name(settings.aspect),
            width = settings.resolution.width,
            height = settings.resolution.height,
            fps = if rate.den() == 1 {
                rate.num().to_string()
            } else {
                format!("{:.2}", f64::from(rate.num()) / f64::from(rate.den()))
            }
        );
        let listeners: &Context<Self> = cx;
        let start = |kind: ExportKind| {
            listeners.listener(move |shell, _: &ClickEvent, _, cx| {
                shell.prompt_export(kind.clone(), cx);
            })
        };
        let status: Option<Div> = match state {
            ExportState::Idle => None,
            ExportState::Running { kind, progress } => Some(small(
                tr!(
                    "finish.export_progress",
                    what = match kind {
                        ExportKind::Video => tr!("finish.export.video"),
                        ExportKind::Premiere => tr!("finish.export.premiere"),
                    },
                    pct = format!("{:.0}", progress * 100.0)
                ),
                tokens::FG2,
            )),
            ExportState::Done { path, message } => Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(small(message, tokens::FG2))
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(
                                chip("export-reveal", tr!("finish.reveal"), false, true).on_click(
                                    listeners.listener(move |_, _: &ClickEvent, _, cx| {
                                        cx.reveal_path(&path);
                                    }),
                                ),
                            )
                            .child(
                                chip("export-clear", tr!("finish.ok"), false, true).on_click(
                                    listeners.listener(|shell, _: &ClickEvent, _, cx| {
                                        shell.worker.clear_export();
                                        cx.notify();
                                    }),
                                ),
                            ),
                    ),
            ),
            ExportState::Failed(e) => {
                Some(small(tr!("finish.export_failed", error = e), tokens::ERROR))
            }
        };
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(label(tr!("finish.label.export")))
            .child(small(summary, tokens::MUTE))
            .when(vertical && duration > 180.0, |d| {
                d.child(small(tr!("finish.too_long"), tokens::MIXED))
            })
            .child(
                chip("export-sfx", tr!("finish.with_sfx"), sfx, true).on_click(cx.listener(
                    move |shell, _: &ClickEvent, _, cx| {
                        shell.export_sfx = !sfx;
                        cx.notify();
                    },
                )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .child(
                        chip("export-video", tr!("finish.export_mp4"), false, !running)
                            .when(!running, |d| d.on_click(start(ExportKind::Video))),
                    )
                    .child(
                        chip("export-premiere", tr!("finish.export_xml"), false, !running)
                            .when(!running, |d| d.on_click(start(ExportKind::Premiere))),
                    ),
            )
            .when_some(status, |d, s| d.child(s))
    }

    /// Settings of the selected music or SFX.
    fn selection_section(&self, cx: &mut Context<Self>) -> Option<Div> {
        let id = self.selected_cue?;
        let finish = &self.session.finish;
        let board = self.board.as_ref()?;
        let blocks = board.project.blocks();
        let number = |b: BlockId| {
            blocks.iter().position(|x| x.id == b).map_or_else(
                || "—".to_owned(),
                |i| tr!("board.scene", n = format!("{:02}", i.saturating_add(1))).to_string(),
            )
        };
        let listeners: &Context<Self> = cx;
        let remove = chip("remove-cue", tr!("finish.remove"), false, true).on_click(
            listeners.listener(move |shell, _: &ClickEvent, window, cx| {
                shell.edit_finish(|f| f.remove(id), cx);
                shell.select_cue(None, window, cx);
            }),
        );
        if let Some(m) = finish.music.iter().find(|m| m.id == id) {
            let file = m.file.clone();
            let listen = self.listen_chip("listen-cue", &file, listeners);
            let (from, to) = (m.from, m.to);
            let step = move |which: bool, delta: isize| {
                listeners.listener(move |shell: &mut Self, _: &ClickEvent, _, cx| {
                    let Some(board) = &shell.board else { return };
                    let ids: Vec<BlockId> = board.project.blocks().iter().map(|b| b.id).collect();
                    let current = if which { from } else { to };
                    let Some(i) = ids.iter().position(|b| *b == current) else {
                        return;
                    };
                    let Some(next) = i
                        .checked_add_signed(delta)
                        .and_then(|j| ids.get(j))
                        .copied()
                    else {
                        return;
                    };
                    shell.edit_finish(
                        |f| {
                            if let Some(m) = f.music_mut(id) {
                                if which {
                                    m.from = next;
                                } else {
                                    m.to = next;
                                }
                            }
                        },
                        cx,
                    );
                })
            };
            let bound = |name: SharedString, which: bool, block: BlockId, key: &'static str| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        div()
                            .w(px(48.))
                            .text_size(px(12.))
                            .text_color(color(tokens::FG2))
                            .child(name),
                    )
                    .child(chip((key, 0_usize), "◀", false, true).on_click(step(which, -1)))
                    .child(mono(number(block)).text_size(px(12.)).px(px(4.)))
                    .child(chip((key, 1_usize), "▶", false, true).on_click(step(which, 1)))
            };
            let offset = m.offset;
            let shift = move |delta: f64| {
                listeners.listener(move |shell: &mut Self, _: &ClickEvent, _, cx| {
                    shell.edit_finish(
                        |f| {
                            if let Some(m) = f.music_mut(id) {
                                m.offset = (offset + delta).max(0.0);
                            }
                        },
                        cx,
                    );
                })
            };
            return Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(label(tr!("finish.label.music")))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(SharedString::from(self.asset_name(&file))),
                    )
                    .child(bound(tr!("finish.from"), true, from, "music-from"))
                    .child(bound(tr!("finish.to"), false, to, "music-to"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(
                                mono(tr!("finish.offset", seconds = format!("{offset:.1}")))
                                    .text_size(px(12.))
                                    .text_color(color(tokens::FG2))
                                    .mr(px(4.)),
                            )
                            .child(chip("music-back", "−5 s", false, true).on_click(shift(-5.0)))
                            .child(chip("music-fwd", "+5 s", false, true).on_click(shift(5.0))),
                    )
                    .children(self.knob(Knob::Gain))
                    .children(self.knob(Knob::FadeIn))
                    .children(self.knob(Knob::FadeOut))
                    .child(div().flex().gap(px(4.)).child(listen).child(remove)),
            );
        }
        let s = finish.sfx.iter().find(|s| s.id == id)?;
        let file = s.file.clone();
        let listen = self.listen_chip("listen-cue", &file, listeners);
        let block = s.block;
        let offset = s.offset;
        let nudge = move |delta: f64| {
            listeners.listener(move |shell: &mut Self, _: &ClickEvent, _, cx| {
                shell.edit_finish(
                    |f| {
                        if let Some(s) = f.sfx_mut(id) {
                            s.offset = ((offset + delta) * 100.0).round() / 100.0;
                        }
                    },
                    cx,
                );
            })
        };
        let to_playhead = listeners.listener(move |shell: &mut Self, _: &ClickEvent, _, cx| {
            let t = shell.playhead(cx);
            let Some(board) = &shell.board else { return };
            let Some(target) = board.block_at(t).and_then(|b| board.project.block(b)) else {
                return;
            };
            let (b, o) = (target.id, t - target.range.start().as_secs_f64());
            shell.edit_finish(
                |f| {
                    if let Some(s) = f.sfx_mut(id) {
                        s.block = b;
                        s.offset = o;
                    }
                },
                cx,
            );
        });
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(label(tr!("finish.label.sfx")))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .child(SharedString::from(self.asset_name(&file))),
                )
                .child(
                    mono(format!("{} {:+.2} s", number(block), offset))
                        .text_size(px(12.))
                        .text_color(color(tokens::FG2)),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(4.))
                        .child(
                            chip("sfx-back", tr!("finish.nudge_back"), false, true)
                                .on_click(nudge(-0.1)),
                        )
                        .child(
                            chip("sfx-fwd", tr!("finish.nudge_fwd"), false, true)
                                .on_click(nudge(0.1)),
                        )
                        .child(
                            chip("sfx-here", tr!("finish.to_playhead"), false, true)
                                .on_click(to_playhead),
                        ),
                )
                .children(self.knob(Knob::Gain))
                .child(div().flex().gap(px(4.)).child(listen).child(remove)),
        )
    }

    fn listen_chip(&self, id: &'static str, file: &str, cx: &Context<Self>) -> Stateful<Div> {
        let playing = self
            .audition
            .as_ref()
            .is_some_and(|a| a.file == file && a.playing());
        let file = file.to_owned();
        chip(
            id,
            if playing {
                tr!("finish.stop")
            } else {
                tr!("finish.listen")
            },
            playing,
            true,
        )
        .on_click(cx.listener(move |shell, _: &ClickEvent, _, cx| shell.audition(file.clone(), cx)))
    }

    /// The library sounds, to audition and place.
    fn sounds_section(&self, cx: &mut Context<Self>) -> Div {
        let sounds: Vec<(String, String, Option<Div>)> = self
            .session
            .look
            .assets
            .iter()
            .filter(|a| a.kind == AssetKind::Audio)
            .map(|a| (a.file.clone(), a.name.clone(), Self::asset_licence(a)))
            .collect();
        let mut section = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(label(tr!("finish.label.sounds")));
        if sounds.is_empty() {
            return section.child(small(tr!("finish.no_sounds"), tokens::FG2));
        }
        for (i, (file, name, licence)) in sounds.into_iter().enumerate() {
            let listen = self.listen_chip_indexed(i, &file, cx);
            let (m, h, t) = (file.clone(), file.clone(), file);
            section = section.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .pb(px(6.))
                    .border_b_1()
                    .border_color(color(tokens::LINE))
                    .child(div().text_size(px(12.)).child(SharedString::from(name)))
                    .when_some(licence, |d, l| d.child(l))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(4.))
                            .child(listen)
                            .child(
                                chip(("as-music", i), tr!("finish.as_music"), false, true)
                                    .on_click(cx.listener(
                                        move |shell, _: &ClickEvent, window, cx| {
                                            shell.add_music(m.clone(), window, cx);
                                        },
                                    )),
                            )
                            .child(
                                chip(("sfx-here", i), tr!("finish.sfx_here"), false, true)
                                    .on_click(cx.listener(
                                        move |shell, _: &ClickEvent, window, cx| {
                                            shell.add_sfx_here(h.clone(), window, cx);
                                        },
                                    )),
                            )
                            .child(
                                chip(
                                    ("sfx-transitions", i),
                                    tr!("finish.on_transitions"),
                                    false,
                                    true,
                                )
                                .on_click(cx.listener(
                                    move |shell, _: &ClickEvent, _, cx| {
                                        shell.sfx_on_transitions(t.clone(), cx);
                                    },
                                )),
                            ),
                    ),
            );
        }
        section
    }

    fn listen_chip_indexed(&self, i: usize, file: &str, cx: &Context<Self>) -> Stateful<Div> {
        let playing = self
            .audition
            .as_ref()
            .is_some_and(|a| a.file == file && a.playing());
        let file = file.to_owned();
        chip(
            ("listen", i),
            if playing { "■" } else { "▶" },
            playing,
            true,
        )
        .on_click(cx.listener(move |shell, _: &ClickEvent, _, cx| shell.audition(file.clone(), cx)))
    }

    /// Voice and ducking.
    fn ducking_section(&self, cx: &mut Context<Self>) -> Div {
        let enabled = self.session.finish.ducking.enabled;
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(label(tr!("finish.label.ducking")))
            .child(
                chip("ducking", tr!("finish.ducking"), enabled, true).on_click(cx.listener(
                    move |shell, _: &ClickEvent, _, cx| {
                        shell.edit_finish(|f| f.ducking.enabled = !enabled, cx);
                    },
                )),
            )
            .when(enabled, |d| d.children(self.knob(Knob::Depth)))
            .children(self.knob(Knob::Voice))
            .child(small(tr!("finish.ducking_note"), tokens::MUTE))
    }
}

/// `M:SS.d`.
/// Folder offered for exports: Videos, else the home folder.
fn export_dir() -> std::path::PathBuf {
    directories::UserDirs::new()
        .and_then(|d| d.video_dir().map(std::path::Path::to_path_buf))
        .or_else(|| directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()))
        .unwrap_or_else(std::env::temp_dir)
}

/// What a thumbnail export writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThumbExport {
    /// JPEG for YouTube.
    Image,
    /// Layered Photoshop document.
    Psd,
}

impl Shell {
    fn thumb_dir(&self) -> std::path::PathBuf {
        thumbnail::dir(&self.session.studio_dir())
    }

    /// Lists the proposals on disk again; keeps the shown one if it is
    /// still there, else the validated one, else the last one.
    pub(super) fn reload_thumbnails(&mut self) {
        self.thumb.versions = thumbnail::versions(&self.thumb_dir());
        let has = |n: Option<u32>| n.filter(|n| self.thumb.versions.iter().any(|v| v.number == *n));
        self.thumb.shown = has(self.thumb.shown)
            .or_else(|| has(self.session.data.youtube.accepted))
            .or_else(|| self.thumb.versions.last().map(|v| v.number));
    }

    /// A new proposal from the engine: show it.
    fn poll_thumbnail(&mut self) {
        if let ThumbnailState::Generated(n) = self.worker.thumbnail_state()
            && self.thumb.seen != Some(n)
        {
            self.thumb.seen = Some(n);
            self.reload_thumbnails();
            if self.thumb.versions.iter().any(|v| v.number == n) {
                self.thumb.shown = Some(n);
            }
            self.thumb.open = true;
        }
    }

    /// Saves the title and description typed for YouTube.
    pub(super) fn commit_youtube(&mut self, cx: &Context<Self>) {
        let title = self.yt_title.read(cx).value().trim().to_owned();
        let notes = self.yt_notes.read(cx).value().trim().to_owned();
        let youtube = &mut self.session.data.youtube;
        if youtube.title != title || youtube.notes != notes {
            youtube.title = title;
            youtube.notes = notes;
            self.session.save_data();
        }
    }

    /// Asks the model for a new proposal (different from the previous ones).
    fn generate_thumbnail(&mut self, cx: &mut Context<Self>) {
        self.commit_youtube(cx);
        let youtube = self.session.data.youtube.clone();
        if youtube.title.is_empty() {
            return;
        }
        self.push_snapshot();
        self.worker.thumbnail(ThumbnailRequest::Generate {
            title: youtube.title,
            notes: youtube.notes,
            quality: self.session.data.quality,
        });
        cx.notify();
    }

    fn accept_thumbnail(&mut self, number: Option<u32>, cx: &mut Context<Self>) {
        self.session.data.youtube.accepted = number;
        self.session.save_data();
        cx.notify();
    }

    /// Asks where to write the thumbnail, then writes it.
    fn prompt_thumbnail_export(&mut self, number: u32, what: ThumbExport, cx: &mut Context<Self>) {
        let ext = match what {
            ThumbExport::Image => "jpg",
            ThumbExport::Psd => "psd",
        };
        let name = format!(
            "{} - {}.{ext}",
            self.export_stem(),
            tr!("finish.thumb.file_name")
        );
        let path = cx.prompt_for_new_path(&export_dir(), Some(&name));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut path))) = path.await {
                let keep_png = what == ThumbExport::Image
                    && path
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("png"));
                if !keep_png
                    && path.extension().is_none_or(|e| {
                        !e.eq_ignore_ascii_case(ext) && !e.eq_ignore_ascii_case("jpeg")
                    })
                {
                    path.set_extension(ext);
                }
                let _ = this.update(cx, |shell, cx| {
                    shell.worker.thumbnail(match what {
                        ThumbExport::Image => ThumbnailRequest::Image {
                            version: number,
                            path,
                        },
                        ThumbExport::Psd => ThumbnailRequest::Psd {
                            version: number,
                            path,
                        },
                    });
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// YouTube (16:9 only): title and description of the video, then
    /// thumbnail proposals to validate or redo, and their exports.
    fn youtube_section(&self, cx: &mut Context<Self>) -> Option<Div> {
        if self.settings().aspect != enfait_core::AspectRatio::Landscape16x9 || self.board.is_none()
        {
            return None;
        }
        let open = self.thumb.open;
        let head = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(label(tr!("finish.label.youtube")))
            .child(
                chip(
                    "thumb-open",
                    if open {
                        tr!("finish.thumb.close")
                    } else {
                        tr!("finish.thumb.open")
                    },
                    open,
                    true,
                )
                .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| {
                    shell.thumb.open = !shell.thumb.open;
                    cx.notify();
                })),
            );
        if !open {
            return Some(head);
        }
        let state = self.worker.thumbnail_state();
        let running = matches!(state, ThumbnailState::Running(_));
        let title_set = !self.yt_title.read(cx).value().trim().is_empty();
        let accepted = self.session.data.youtube.accepted;
        let shown = self
            .thumb
            .shown
            .and_then(|n| self.thumb.versions.iter().position(|v| v.number == n));
        let count = self.thumb.versions.len();
        let notes_text = self.yt_notes.read(cx).value().to_string();
        let form = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(small(tr!("finish.thumb.title"), tokens::FG2))
            .child(Input::new(&self.yt_title))
            .child(
                div()
                    .flex()
                    .items_end()
                    .justify_between()
                    .child(small(tr!("finish.thumb.notes"), tokens::FG2))
                    .child(
                        super::mention::add_files_button("thumb-add-files").on_click(cx.listener(
                            |shell, _: &ClickEvent, window, cx| {
                                shell.pick_for_thumbnail(window, cx);
                            },
                        )),
                    ),
            )
            .child(
                self.prompt_field(
                    super::mention::PromptField {
                        field: super::mention::Field::Thumbnail,
                        area: &self.yt_notes,
                        text: &notes_text,
                        height: px(96.),
                        suggestions_width: px(280.),
                        hint: tr!("finish.thumb.mention_hint"),
                    },
                    |d| d,
                    None,
                    cx,
                )
                .id("thumb-notes")
                .drag_over::<gpui_kit::ExternalPaths>(|style, _, _, _| {
                    style.bg(color(tokens::HOVER))
                })
                .on_drop(cx.listener(
                    |shell, paths: &gpui_kit::ExternalPaths, window, cx| {
                        cx.stop_propagation();
                        shell.add_to_thumbnail(paths.paths(), window, cx);
                    },
                )),
            );
        let can_generate = title_set && !running;
        let generate = chip(
            "thumb-generate",
            if count == 0 {
                tr!("finish.thumb.generate")
            } else {
                tr!("finish.thumb.another")
            },
            false,
            can_generate,
        )
        .when(can_generate, |d| {
            d.on_click(cx.listener(|shell, _: &ClickEvent, _, cx| {
                shell.generate_thumbnail(cx);
            }))
        });
        let status: Option<Div> = match &state {
            ThumbnailState::Idle | ThumbnailState::Generated(_) => {
                (!title_set).then(|| small(tr!("finish.thumb.need_title"), tokens::MUTE))
            }
            ThumbnailState::Running(text) => Some(small(text.clone(), tokens::FG2)),
            ThumbnailState::Exported { path, message } => {
                let path = path.clone();
                Some(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(small(message.clone(), tokens::FG2))
                        .child(
                            div()
                                .flex()
                                .gap(px(6.))
                                .child(
                                    chip("thumb-reveal", tr!("finish.reveal"), false, true)
                                        .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                            cx.reveal_path(&path)
                                        })),
                                )
                                .child(
                                    chip("thumb-clear", tr!("finish.ok"), false, true).on_click(
                                        cx.listener(|shell, _: &ClickEvent, _, cx| {
                                            shell.worker.clear_thumbnail();
                                            cx.notify();
                                        }),
                                    ),
                                ),
                        ),
                )
            }
            ThumbnailState::Failed(e) => Some(small(
                tr!("finish.thumb.failed", error = e.clone()),
                tokens::ERROR,
            )),
        };
        let proposal = shown
            .and_then(|i| self.thumb.versions.get(i))
            .map(|version| {
                let number = version.number;
                let png = thumbnail::png_path(&self.thumb_dir(), number);
                let index = shown.unwrap_or(0);
                let is_accepted = accepted == Some(number);
                let step = |id: &'static str, text: &'static str, to: Option<usize>| {
                    let target = to
                        .and_then(|i| self.thumb.versions.get(i))
                        .map(|v| v.number);
                    chip(id, text, false, target.is_some()).when_some(target, |d, n| {
                        d.on_click(cx.listener(move |shell, _: &ClickEvent, _, cx| {
                            shell.thumb.shown = Some(n);
                            cx.notify();
                        }))
                    })
                };
                let nav = div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(step("thumb-prev", "‹", index.checked_sub(1)))
                    .child(
                        mono(format!("{} / {count}", index.saturating_add(1))).text_size(px(11.)),
                    )
                    .child(step("thumb-next", "›", Some(index.saturating_add(1))))
                    .when(is_accepted, |d| {
                        d.child(small(tr!("finish.thumb.accepted"), tokens::SIGNAL))
                    });
                let actions = if is_accepted {
                    let export = |id: &'static str, text: SharedString, what: ThumbExport| {
                        chip(id, text, false, !running).when(!running, |d| {
                            d.on_click(cx.listener(move |shell, _: &ClickEvent, _, cx| {
                                shell.prompt_thumbnail_export(number, what, cx);
                            }))
                        })
                    };
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(6.))
                        .child(export(
                            "thumb-jpg",
                            tr!("finish.thumb.export_jpg"),
                            ThumbExport::Image,
                        ))
                        .child(export(
                            "thumb-psd",
                            tr!("finish.thumb.export_psd"),
                            ThumbExport::Psd,
                        ))
                        .child(
                            chip("thumb-unaccept", tr!("finish.thumb.unaccept"), false, true)
                                .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| {
                                    shell.accept_thumbnail(None, cx)
                                })),
                        )
                } else {
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(6.))
                        .child(
                            chip("thumb-accept", tr!("finish.thumb.accept"), true, true).on_click(
                                cx.listener(move |shell, _: &ClickEvent, _, cx| {
                                    shell.accept_thumbnail(Some(number), cx);
                                }),
                            ),
                        )
                        .child(generate_again(cx, can_generate))
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .w_full()
                            .rounded(px(3.))
                            .overflow_hidden()
                            .border_1()
                            .border_color(color(if is_accepted {
                                tokens::SIGNAL
                            } else {
                                tokens::LINE
                            }))
                            .child(img(png).w_full().h(px(168.)).object_fit(ObjectFit::Contain)),
                    )
                    .child(nav)
                    .when(!version.concept.is_empty(), |d| {
                        d.child(small(version.concept.clone(), tokens::MUTE))
                    })
                    .child(actions)
            });
        Some(
            head.child(form)
                .when_some(proposal, |d, p| d.child(p))
                .when(shown.is_none(), |d| d.child(generate))
                .when_some(status, |d, s| d.child(s)),
        )
    }
}

/// "Another one" next to "Validate".
fn generate_again(cx: &mut Context<Shell>, enabled: bool) -> Stateful<Div> {
    chip("thumb-again", tr!("finish.thumb.another"), false, enabled).when(enabled, |d| {
        d.on_click(cx.listener(|shell, _: &ClickEvent, _, cx| {
            shell.generate_thumbnail(cx);
        }))
    })
}

fn seconds(t: f64) -> String {
    let t = t.max(0.0);
    format!("{}:{:04.1}", (t / 60.0).floor(), t % 60.0)
}
