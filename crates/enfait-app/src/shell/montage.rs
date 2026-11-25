//! Montage: the blocks chosen in the storyboard are sent to the engine
//! with a care level and edited in the background, while the storyboard
//! goes on. The Montage screen shows where each block stands (prepared,
//! edited, finalised), an estimate of the time left, and leads to the
//! Retouch step once a block is ready. A badge on the step counts the
//! blocks finished meanwhile.

/// Up to this many blocks, the strip shows each one with its number and
/// progress; beyond, it is painted in one pass.
const STRIP_DETAIL: usize = 150;

use super::studio::{BlockState, QUALITIES, chip, provider_hint, provider_name, quality_name};
use super::{STEP_RETOUCH, STEP_STORYBOARD, Shell, label, mono};
use crate::jobs::{Job, JobKind, JobStatus};
use crate::theme::{color, tokens};
use crate::tr;
use crate::worker::Phase;
use enfait_ai::{ProviderChoice, Quality};
use enfait_core::{BlockId, BlockKind, Transition};
use gpui_kit::prelude::*;
use gpui_kit::{ClickEvent, Context, Div, FontWeight, SharedString, div, px, relative};
use std::time::{Duration, Instant};

/// How long a toast stays.
const TOAST_TIME: Duration = Duration::from_secs(5);

/// A short message in a corner of the window.
pub(super) struct Toast {
    text: SharedString,
    until: Instant,
}

/// The "Send to the edit" dialog.
pub(super) struct CareDialog {
    /// Blocks to edit (those that need a model).
    blocks: Vec<BlockId>,
    /// Chosen blocks that need no edit (video only, b-roll).
    skipped: usize,
    quality: Quality,
    /// Also make the transitions after these blocks.
    transitions: bool,
}

/// Blocks whose picture is made by a model.
const fn needs_edit(kind: BlockKind) -> bool {
    matches!(
        kind,
        BlockKind::Animation | BlockKind::Mixed | BlockKind::Effect
    )
}

/// Usual length of an animation job, before any has been measured.
const fn default_seconds(quality: Quality) -> u64 {
    match quality {
        Quality::Light => 90,
        Quality::Fast => 150,
        Quality::Polished => 600,
        Quality::Ultra => 1200,
    }
}

/// `~N min` or `~N s`.
fn about(seconds: u64) -> SharedString {
    if seconds < 60 {
        tr!("montage.about_seconds", n = seconds.max(5))
    } else {
        tr!("montage.about_minutes", n = seconds.saturating_add(30) / 60)
    }
}

fn care_text(quality: Quality) -> SharedString {
    match quality {
        Quality::Light => tr!("montage.care.light"),
        Quality::Fast => tr!("montage.care.fast"),
        Quality::Polished => tr!("montage.care.polished"),
        Quality::Ultra => tr!("montage.care.ultra"),
    }
}

/// `0,0 → 22,7 s`.
fn span(start: f64, end: f64) -> String {
    let fr = |v: f64| format!("{v:.1}");
    if &*rust_i18n::locale() == "fr" {
        format!(
            "{} → {} s",
            fr(start).replace('.', ","),
            fr(end).replace('.', ",")
        )
    } else {
        format!("{} → {} s", fr(start), fr(end))
    }
}

/// A running block: its job, phase and progress estimate.
struct Running {
    block: BlockId,
    phase: Phase,
    /// Seconds since it started.
    elapsed: u64,
    /// Expected length in seconds.
    expected: u64,
    activity: Option<String>,
}

impl Running {
    #[allow(clippy::cast_precision_loss)]
    fn fraction(&self) -> f32 {
        let f = self.elapsed as f32 / self.expected.max(1) as f32;
        // Never full before it is done; the phase gives a floor.
        let floor = match self.phase {
            Phase::Preparing => 0.02,
            Phase::Editing => 0.15,
            Phase::Finishing => 0.85,
        };
        f.clamp(floor, 0.97)
    }

    fn left(&self) -> u64 {
        self.expected.saturating_sub(self.elapsed).max(10)
    }
}

impl Shell {
    pub(super) fn toast(&mut self, text: SharedString) {
        self.toasts.push(Toast {
            text,
            until: Instant::now() + TOAST_TIME,
        });
        if self.toasts.len() > 3 {
            self.toasts.remove(0);
        }
    }

    pub(super) fn expire_toasts(&mut self) {
        let now = Instant::now();
        self.toasts.retain(|t| t.until > now);
    }

    /// The toasts, bottom right.
    pub(super) fn toasts_layer(&self) -> Option<Div> {
        if self.toasts.is_empty() {
            return None;
        }
        Some(
            div()
                .absolute()
                .right(px(16.))
                .bottom(px(110.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .children(self.toasts.iter().map(|t| {
                    div()
                        .px(px(12.))
                        .py(px(8.))
                        .max_w(px(360.))
                        .rounded(px(3.))
                        .bg(color(tokens::RAISED))
                        .border_1()
                        .border_color(color(tokens::LINE_STRONG))
                        .text_size(px(12.))
                        .child(t.text.clone())
                })),
        )
    }

    /// The take has frames (video projects offer "video only" and "video
    /// with animation").
    pub(super) fn take_has_video(&self) -> bool {
        let Some(rush) = &self.session.data.rush else {
            return false;
        };
        enfait_media::MediaCache::new(super::media_cache())
            .dir_for(rush)
            .ok()
            .and_then(|dir| std::fs::read(dir.join("info.json")).ok())
            .and_then(|d| serde_json::from_slice::<enfait_media::MediaInfo>(&d).ok())
            .is_some_and(|info| info.video.is_some())
    }

    /// Animated blocks ready to be checked (Retouch step).
    pub(super) fn ready_count(&self) -> usize {
        self.board.as_ref().map_or(0, |b| {
            b.project
                .blocks()
                .iter()
                .filter(|block| {
                    needs_edit(block.kind) && self.block_state(block) == BlockState::Ready
                })
                .count()
        })
    }

    /// Opens the dialog for the chosen blocks (or the one shown).
    pub(super) fn open_care(&mut self, cx: &mut Context<Self>) {
        self.commit_description(cx);
        let Some(board) = &self.board else { return };
        let chosen: Vec<BlockId> = if board.marked.is_empty() {
            board.selected.into_iter().collect()
        } else {
            board.marked.clone()
        };
        let (blocks, skipped): (Vec<BlockId>, Vec<BlockId>) = chosen
            .into_iter()
            .partition(|id| board.project.block(*id).is_some_and(|b| needs_edit(b.kind)));
        self.care = Some(CareDialog {
            blocks,
            skipped: skipped.len(),
            quality: self.session.data.quality,
            transitions: true,
        });
        cx.notify();
    }

    /// Sends the blocks of the dialog to the engine.
    fn send_to_montage(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.care.take() else {
            return;
        };
        let Some(board) = &self.board else { return };
        // In timeline order, so that the limit keeps the first ones.
        let chosen: Vec<BlockId> = board
            .project
            .blocks()
            .iter()
            .map(|b| b.id)
            .filter(|id| dialog.blocks.contains(id))
            .collect();
        let chosen = self.limit_blocks(chosen, cx);
        let Some(board) = &self.board else { return };
        let blocks = board.project.blocks();
        let transitions: Vec<BlockId> = if dialog.transitions {
            blocks
                .iter()
                .take(blocks.len().saturating_sub(1))
                .filter(|b| b.transition_after != Transition::Cut && chosen.contains(&b.id))
                .map(|b| b.id)
                .collect()
        } else {
            Vec::new()
        };
        self.session.data.quality = dialog.quality;
        self.session.save_data();
        for id in &chosen {
            self.enqueue(*id, JobKind::Create, cx);
        }
        for id in transitions {
            self.enqueue(id, JobKind::Transition, cx);
        }
        if !chosen.is_empty() {
            self.toast(tr!("montage.sent", count = chosen.len()));
        }
        cx.notify();
    }

    /// The dialog, over the window.
    pub(super) fn care_dialog(&self, cx: &mut Context<Self>) -> Option<Div> {
        let dialog = self.care.as_ref()?;
        let count = dialog.blocks.len();
        let chosen = dialog.quality;
        let provider = self.session.data.provider;
        let missing = provider_hint(provider, crate::prefs::settings(cx));
        let numbers: Vec<String> = self.board.as_ref().map_or_else(Vec::new, |b| {
            b.project
                .blocks()
                .iter()
                .enumerate()
                .filter(|(_, block)| dialog.blocks.contains(&block.id))
                .map(|(i, _)| i.saturating_add(1).to_string())
                .collect()
        });
        let levels = QUALITIES.iter().enumerate().map(|(i, &q)| {
            let on = q == chosen;
            div()
                .id(("care", i))
                .flex()
                .gap(px(10.))
                .px(px(12.))
                .py(px(8.))
                .rounded(px(3.))
                .cursor_pointer()
                .border_1()
                .border_color(color(if on { tokens::SIGNAL } else { tokens::LINE }))
                .when(on, |d| d.bg(color(tokens::RAISED)))
                .hover(|d| d.bg(color(tokens::HOVER)))
                .child(
                    div()
                        .mt(px(4.))
                        .flex_none()
                        .size(px(10.))
                        .rounded(px(5.))
                        .border_1()
                        .border_color(color(tokens::LINE_STRONG))
                        .when(on, |d| d.bg(color(tokens::SIGNAL))),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(div().font_weight(FontWeight::MEDIUM).child(quality_name(q)))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(color(tokens::FG2))
                                .child(care_text(q)),
                        ),
                )
                .on_click(cx.listener(move |shell, _: &ClickEvent, _, cx| {
                    if let Some(d) = &mut shell.care {
                        d.quality = q;
                    }
                    cx.notify();
                }))
        });
        let providers = ProviderChoice::ALL.iter().enumerate().map(|(i, &p)| {
            chip(("care-provider", i), provider_name(p), provider == p, true).on_click(
                cx.listener(move |shell, _: &ClickEvent, _, cx| shell.set_provider(p, cx)),
            )
        });
        let transitions = dialog.transitions;
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(color(tokens::BG).opacity(0.8))
                .child(
                    div()
                        .id("care-dialog")
                        .w(px(500.))
                        .max_h(relative(0.9))
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .gap(px(12.))
                        .p(px(20.))
                        .rounded(px(4.))
                        .bg(color(tokens::PANEL))
                        .border_1()
                        .border_color(color(tokens::LINE_STRONG))
                        .child(
                            div()
                                .text_size(px(18.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(tr!("montage.send_title")),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(color(tokens::FG2))
                                .child(if count == 0 {
                                    tr!("montage.nothing_to_send")
                                } else {
                                    tr!(
                                        "montage.send_blocks",
                                        count = count,
                                        list = numbers.join(", ")
                                    )
                                })
                                .when(dialog.skipped > 0, |d| {
                                    d.child(
                                        div().child(tr!("montage.skipped", count = dialog.skipped)),
                                    )
                                }),
                        )
                        .child(label(tr!("montage.care_label")))
                        .children(levels)
                        .child(label(tr!("studio.label.models")))
                        .child(div().flex().flex_wrap().gap(px(4.)).children(providers))
                        .when_some(missing, |d, m| {
                            d.child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(color(tokens::MIXED))
                                    .child(m),
                            )
                        })
                        .child(
                            chip(
                                "care-transitions",
                                tr!("montage.with_transitions"),
                                transitions,
                                true,
                            )
                            .on_click(cx.listener(
                                |shell, _: &ClickEvent, _, cx| {
                                    if let Some(d) = &mut shell.care {
                                        d.transitions = !d.transitions;
                                    }
                                    cx.notify();
                                },
                            )),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(8.))
                                .pt(px(4.))
                                .child(
                                    super::screens::button(
                                        "care-cancel",
                                        tr!("common.cancel"),
                                        false,
                                    )
                                    .on_click(cx.listener(
                                        |shell, _: &ClickEvent, _, cx| {
                                            shell.care = None;
                                            cx.notify();
                                        },
                                    )),
                                )
                                .when(count > 0, |d| {
                                    d.child(
                                        super::screens::button(
                                            "care-send",
                                            tr!("montage.start"),
                                            true,
                                        )
                                        .on_click(
                                            cx.listener(|shell, _: &ClickEvent, _, cx| {
                                                shell.send_to_montage(cx);
                                            }),
                                        ),
                                    )
                                }),
                        ),
                ),
        )
    }

    /// Expected length of a job, from the finished ones of that quality.
    fn expected_seconds(jobs: &[Job], quality: Quality, transition: bool) -> u64 {
        let took: Vec<u64> = jobs
            .iter()
            .rev()
            .filter(|j| {
                j.status == JobStatus::Done
                    && j.quality == quality
                    && (j.kind == JobKind::Transition) == transition
                    && !matches!(j.kind, JobKind::Restore { .. })
            })
            .filter_map(Job::took)
            .filter(|t| *t > 0)
            .take(10)
            .collect();
        if took.is_empty() {
            let base = default_seconds(quality);
            return if transition { base / 2 } else { base };
        }
        took.iter().sum::<u64>() / took.len() as u64
    }

    /// Animation jobs running now, by block.
    fn running(&self) -> Vec<Running> {
        let Ok(queue) = self.queue.lock() else {
            return Vec::new();
        };
        let now = enfait_config::projects::now();
        queue
            .jobs()
            .iter()
            .filter(|j| j.status == JobStatus::Running && j.kind != JobKind::Transition)
            .map(|j| Running {
                block: j.block,
                phase: self.worker.phase(j.id).unwrap_or(Phase::Preparing),
                elapsed: j.started_at.map_or(0, |s| now.saturating_sub(s)),
                expected: Self::expected_seconds(queue.jobs(), j.quality, false),
                activity: self.worker.job_activity(j.id),
            })
            .collect()
    }

    /// Seconds before the queue is empty (running and queued jobs, two at a
    /// time).
    fn time_left(&self, running: &[Running]) -> u64 {
        let Ok(queue) = self.queue.lock() else {
            return 0;
        };
        let queued: u64 = queue
            .jobs()
            .iter()
            .filter(|j| j.status == JobStatus::Queued)
            .map(|j| Self::expected_seconds(queue.jobs(), j.quality, j.kind == JobKind::Transition))
            .sum();
        let now: u64 = running.iter().map(Running::left).max().unwrap_or(0);
        now.saturating_add(queued / crate::worker::MAX_JOBS as u64)
    }

    fn check_block(&mut self, id: BlockId, cx: &mut Context<Self>) {
        if let Some(board) = &mut self.board {
            board.select(Some(id));
        }
        self.needs_sync = true;
        self.go_to(STEP_RETOUCH, cx);
    }

    /// The Montage screen.
    #[allow(clippy::too_many_lines)]
    /// Row of block `i` and of its transition on the montage screen (built
    /// only while in view).
    fn montage_row(&mut self, i: usize, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(board) = &self.board else {
            return div().into_any_element();
        };
        let blocks = board.project.blocks();
        let Some(b) = blocks.get(i) else {
            return div().into_any_element();
        };
        let selected = board.selected;
        let listeners: &Context<Self> = cx;
        let mut rows: Vec<gpui_kit::AnyElement> = Vec::new();
        let id = b.id;
        let state = self.block_state(b);
        let text = if b.description.trim().is_empty() {
            board.text_of(b)
        } else {
            b.description.clone()
        };
        let (dot, action): (crate::theme::Token, Option<gpui_kit::AnyElement>) = match &state {
            BlockState::Ready | BlockState::Mounted => (
                tokens::SIGNAL,
                Some(
                    div()
                        .id(("check", i))
                        .cursor_pointer()
                        .text_size(px(12.))
                        .text_color(color(tokens::SIGNAL))
                        .hover(|d| d.underline())
                        .child(tr!("montage.check"))
                        .on_click(listeners.listener(move |shell, _: &ClickEvent, _, cx| {
                            shell.check_block(id, cx);
                        }))
                        .into_any_element(),
                ),
            ),
            BlockState::Running => (
                tokens::MIXED,
                Some(small_tag(tr!("studio.state.running")).into_any_element()),
            ),
            BlockState::Queued => (
                tokens::FG2,
                Some(small_tag(tr!("studio.state.queued")).into_any_element()),
            ),
            BlockState::Failed(e) => (
                tokens::ERROR,
                Some(
                    div()
                        .id(("retry", i))
                        .cursor_pointer()
                        .text_size(px(12.))
                        .text_color(color(tokens::ERROR))
                        .hover(|d| d.underline())
                        .child(tr!("montage.retry"))
                        .tooltip({
                            let e = SharedString::from(e.clone());
                            move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(e.clone())
                                    .build(window, cx)
                            }
                        })
                        .on_click(listeners.listener(move |shell, _: &ClickEvent, _, cx| {
                            if let Some(board) = &mut shell.board {
                                board.select(Some(id));
                            }
                            shell.open_care(cx);
                        }))
                        .into_any_element(),
                ),
            ),
            BlockState::ToGenerate => (
                tokens::LINE_STRONG,
                Some(
                    div()
                        .id(("send", i))
                        .cursor_pointer()
                        .text_size(px(12.))
                        .text_color(color(tokens::FG2))
                        .hover(|d| d.underline())
                        .child(tr!("montage.send_one"))
                        .on_click(listeners.listener(move |shell, _: &ClickEvent, _, cx| {
                            if let Some(board) = &mut shell.board {
                                board.select(Some(id));
                            }
                            shell.open_care(cx);
                        }))
                        .into_any_element(),
                ),
            ),
            BlockState::NotAnimated => (
                tokens::RUSH,
                Some(small_tag(tr!("montage.no_edit")).into_any_element()),
            ),
            BlockState::NeedsBroll | BlockState::Mounting => (
                tokens::FG2,
                state.label().map(|l| small_tag(l).into_any_element()),
            ),
        };
        let chosen = selected == Some(id);
        rows.push(
            div()
                .id(("montage-row", i))
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(14.))
                .py(px(10.))
                .rounded(px(3.))
                .bg(color(if chosen {
                    tokens::RAISED
                } else {
                    tokens::PANEL
                }))
                .border_1()
                .border_color(color(if chosen {
                    tokens::LINE_STRONG
                } else {
                    tokens::LINE
                }))
                .child(
                    div()
                        .flex_none()
                        .size(px(8.))
                        .rounded(px(4.))
                        .bg(color(dot)),
                )
                .child(
                    div()
                        .flex_none()
                        .w(px(60.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(tr!("montage.block", n = i.saturating_add(1))),
                )
                .child(
                    mono(span(
                        b.range.start().as_secs_f64(),
                        b.range.end().as_secs_f64(),
                    ))
                    .flex_none()
                    .w(px(100.))
                    .text_size(px(11.))
                    .text_color(color(tokens::MUTE)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(12.))
                        .text_color(color(tokens::FG2))
                        .child(SharedString::from(text)),
                )
                .children(action)
                .on_click(listeners.listener(move |shell, _: &ClickEvent, _, cx| {
                    if let Some(board) = &mut shell.board {
                        board.select(Some(id));
                    }
                    cx.notify();
                }))
                .into_any_element(),
        );
        let last = i.saturating_add(1) == blocks.len();
        if !last && let Some(t) = self.transition_state(id, &b.transition_after) {
            let end = b.range.end().as_secs_f64();
            rows.push(
                div()
                    .ml(px(18.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(14.))
                    .py(px(6.))
                    .rounded(px(3.))
                    .bg(color(tokens::PANEL))
                    .border_1()
                    .border_color(color(tokens::LINE))
                    .child(div().size(px(6.)).rounded(px(3.)).bg(color(t.tone())))
                    .child(div().text_size(px(12.)).child(tr!(
                        "montage.transition",
                        from = i.saturating_add(1),
                        to = i.saturating_add(2)
                    )))
                    .child(
                        mono(span(end - 0.4, end + 0.6))
                            .flex_1()
                            .text_size(px(11.))
                            .text_color(color(tokens::MUTE)),
                    )
                    .children(t.label().map(small_tag))
                    .into_any_element(),
            );
        }
        div()
            .pb(px(6.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .children(rows)
            .into_any_element()
    }

    pub(super) fn montage_screen(&mut self, cx: &mut Context<Self>) -> Div {
        let screen = div().flex_1().min_w(px(0.)).flex().flex_col();
        let Some(board) = &self.board else {
            return screen.child(
                div()
                    .p(px(20.))
                    .text_color(color(tokens::MUTE))
                    .child(tr!("montage.no_board")),
            );
        };
        let running = self.running();
        let blocks = board.project.blocks();
        let total = board.project.duration().as_secs_f64().max(1e-3);
        let states: Vec<BlockState> = blocks.iter().map(|b| self.block_state(b)).collect();
        let edited: Vec<bool> = blocks.iter().map(|b| needs_edit(b.kind)).collect();
        let count = |f: &dyn Fn(&BlockState) -> bool| {
            states
                .iter()
                .zip(&edited)
                .filter(|(s, e)| **e && f(s))
                .count()
        };
        let ready = count(&|s| *s == BlockState::Ready);
        let busy = count(&|s| *s == BlockState::Running);
        let queued = count(&|s| *s == BlockState::Queued);
        let failed = count(&|s| matches!(s, BlockState::Failed(_)));
        let unsent = count(&|s| *s == BlockState::ToGenerate);
        let left = self.time_left(&running);
        let quality = self.session.data.quality;
        let rate = board.project.settings().frame_rate;
        let _ = rate;

        // Strip of the blocks, the one being edited fills up.
        #[allow(clippy::cast_possible_truncation)]
        let detailed = blocks.iter().enumerate().map(|(i, b)| {
            let id = b.id;
            let state = states.get(i).cloned().unwrap_or(BlockState::NotAnimated);
            let progress = running
                .iter()
                .find(|r| r.block == id)
                .map(Running::fraction);
            let fill = match state {
                BlockState::Ready | BlockState::Mounted => Some(tokens::SIGNAL),
                BlockState::Failed(_) => Some(tokens::ERROR),
                _ => None,
            };
            div()
                .id(("montage-strip", i))
                .relative()
                .w(relative((b.range.duration().as_secs_f64() / total) as f32))
                .h_full()
                .border_r_1()
                .border_color(color(tokens::BG))
                .overflow_hidden()
                .cursor_pointer()
                .bg(color(match state {
                    BlockState::Queued | BlockState::Running => tokens::LINE_STRONG,
                    BlockState::NotAnimated => tokens::RUSH,
                    _ => tokens::LINE,
                }))
                .when_some(fill, |d, t| d.bg(color(t).opacity(0.55)))
                .when_some(progress, |d, p| {
                    d.child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left_0()
                            .w(relative(p))
                            .bg(color(tokens::MIXED).opacity(0.6)),
                    )
                })
                .child(
                    mono(i.saturating_add(1).to_string())
                        .relative()
                        .px(px(4.))
                        .text_size(px(11.))
                        .text_color(color(tokens::FG)),
                )
                .on_click(cx.listener(move |shell, _: &ClickEvent, _, cx| {
                    if let Some(board) = &mut shell.board {
                        board.select(Some(id));
                    }
                    cx.notify();
                }))
        });
        let strip = if blocks.len() <= STRIP_DETAIL {
            div()
                .size_full()
                .flex()
                .children(detailed)
                .into_any_element()
        } else {
            let marks = blocks
                .iter()
                .zip(&states)
                .map(|(b, state)| super::lazy::Mark {
                    start: (b.range.start().as_secs_f64() / total) as f32,
                    width: (b.range.duration().as_secs_f64() / total) as f32,
                    tone: match state {
                        BlockState::Ready | BlockState::Mounted => tokens::SIGNAL,
                        BlockState::Failed(_) => tokens::ERROR,
                        BlockState::Queued | BlockState::Running => tokens::MIXED,
                        BlockState::NotAnimated => tokens::RUSH,
                        _ => tokens::LINE_STRONG,
                    },
                })
                .collect();
            let shell = cx.entity().downgrade();
            super::lazy::painted_track(marks, 0.7, 1.0, move |share, _, cx| {
                let _ = shell.update(cx, |shell, cx| {
                    if let Some(board) = &mut shell.board {
                        let t = f64::from(share) * board.project.duration().as_secs_f64();
                        let id = board.block_at(t);
                        board.select(id);
                    }
                    cx.notify();
                });
            })
            .into_any_element()
        };

        let selected = board.selected;
        let current = running
            .iter()
            .find(|r| Some(r.block) == selected)
            .or_else(|| running.first());
        let current_card = current.map(|r| {
            let number = blocks
                .iter()
                .position(|b| b.id == r.block)
                .map_or(0, |i| i.saturating_add(1));
            let phases = [
                (Phase::Preparing, tr!("montage.phase.preparing")),
                (Phase::Editing, tr!("montage.phase.editing")),
                (Phase::Finishing, tr!("montage.phase.finishing")),
            ]
            .into_iter()
            .map(|(phase, name)| {
                let tone = if phase < r.phase {
                    tokens::SIGNAL
                } else if phase == r.phase {
                    tokens::MIXED
                } else {
                    tokens::LINE_STRONG
                };
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(12.))
                    .text_color(color(if phase > r.phase {
                        tokens::MUTE
                    } else {
                        tokens::FG
                    }))
                    .child(div().size(px(8.)).rounded(px(4.)).bg(color(tone)))
                    .child(name)
            });
            card()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().size(px(8.)).rounded(px(4.)).bg(color(tokens::MIXED)))
                        .child(
                            div()
                                .text_size(px(18.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(tr!("montage.block_title", n = number)),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .child(tr!("montage.block_left", left = about(r.left())))
                        .child(
                            div()
                                .text_color(color(tokens::FG2))
                                .child(tr!("montage.started", ago = about(r.elapsed))),
                        ),
                )
                .child(div().flex().flex_col().gap(px(6.)).children(phases))
                .when_some(r.activity.clone(), |d, a| {
                    d.child(
                        div()
                            .text_size(px(11.))
                            .text_color(color(tokens::MUTE))
                            .child(SharedString::from(a)),
                    )
                })
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(color(tokens::MUTE))
                        .child(tr!("montage.estimate_note")),
                )
        });
        let idle_card = current_card.is_none().then(|| {
            card()
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .child(tr!("montage.idle_title")),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(color(tokens::FG2))
                        .child(tr!("montage.idle")),
                )
                .child(
                    super::screens::button("to-board", tr!("montage.to_storyboard"), false)
                        .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| {
                            shell.go_to(STEP_STORYBOARD, cx);
                        })),
                )
        });
        let batch = ready + busy + queued + failed;
        #[allow(clippy::cast_precision_loss)]
        let done_fraction = if batch == 0 {
            0.0
        } else {
            ready as f32 / batch as f32
        };
        let progress_card = card()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(tr!("montage.progress")),
                    )
                    .child(
                        mono(if busy + queued > 0 {
                            tr!("montage.running_tag")
                        } else {
                            tr!("montage.idle_tag")
                        })
                        .px(px(6.))
                        .rounded(px(3.))
                        .border_1()
                        .border_color(color(tokens::LINE_STRONG))
                        .text_size(px(11.))
                        .text_color(color(tokens::FG2)),
                    ),
            )
            .child(
                div()
                    .h(px(4.))
                    .w_full()
                    .rounded(px(2.))
                    .bg(color(tokens::LINE))
                    .child(
                        div()
                            .h_full()
                            .w(relative(done_fraction))
                            .rounded(px(2.))
                            .bg(color(tokens::SIGNAL)),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(color(tokens::FG2))
                    .child(tr!("montage.done_of", done = ready, total = batch))
                    .when(busy + queued > 0, |d| {
                        d.child(div().child(tr!("montage.queue_left", left = about(left))))
                    }),
            );

        // List of the blocks and their transitions.
        let rows = super::lazy::lazy_list(
            &self.lists.montage,
            blocks.len(),
            |shell, i, _, cx| shell.montage_row(i, cx),
            cx,
        );

        let legend = |tone: crate::theme::Token, text: SharedString| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(div().size(px(8.)).bg(color(tone)))
                .child(text)
        };
        let busy_now = busy + queued > 0;
        screen
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
                            .child(tr!("montage.title")),
                    )
                    .child(
                        mono(quality_name(quality))
                            .flex_1()
                            .text_size(px(12.))
                            .text_color(color(tokens::MUTE)),
                    )
                    .child(
                        super::screens::button(
                            "to-retouch",
                            tr!("montage.retouch_link", count = ready),
                            false,
                        )
                        .on_click(cx.listener(
                            |shell, _: &ClickEvent, _, cx| {
                                shell.go_to(STEP_RETOUCH, cx);
                            },
                        )),
                    )
                    .when(busy_now, |d| {
                        d.child(
                            super::screens::button("stop-montage", tr!("montage.stop"), false)
                                .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| {
                                    if let Ok(mut q) = shell.queue.lock() {
                                        q.cancel_queued(None);
                                    }
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .px(px(20.))
                    .pt(px(12.))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(16.))
                            .text_size(px(12.))
                            .text_color(color(tokens::FG2))
                            .child(legend(
                                tokens::SIGNAL,
                                tr!("montage.legend.ready", n = ready),
                            ))
                            .child(legend(
                                tokens::MIXED,
                                tr!("montage.legend.running", n = busy),
                            ))
                            .child(legend(
                                tokens::LINE_STRONG,
                                tr!("montage.legend.queued", n = queued),
                            ))
                            .child(legend(
                                tokens::LINE,
                                tr!("montage.legend.unsent", n = unsent),
                            ))
                            .when(failed > 0, |d| {
                                d.child(legend(
                                    tokens::ERROR,
                                    tr!("montage.legend.failed", n = failed),
                                ))
                            }),
                    )
                    .child(
                        div()
                            .h(px(26.))
                            .flex()
                            .rounded(px(2.))
                            .overflow_hidden()
                            .child(strip),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .gap(px(16.))
                    .p(px(20.))
                    .child(
                        div()
                            .flex_none()
                            .w(px(320.))
                            .flex()
                            .flex_col()
                            .gap(px(12.))
                            .children(current_card)
                            .children(idle_card)
                            .child(progress_card),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(tr!("montage.your_blocks")),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_h(px(0.))
                                    .flex()
                                    .flex_col()
                                    .child(rows.flex_1()),
                            ),
                    ),
            )
    }
}

fn card() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .p(px(16.))
        .rounded(px(4.))
        .bg(color(tokens::PANEL))
        .border_1()
        .border_color(color(tokens::LINE))
}

fn small_tag(text: SharedString) -> Div {
    div()
        .px(px(6.))
        .rounded(px(3.))
        .border_1()
        .border_color(color(tokens::LINE_STRONG))
        .text_size(px(11.))
        .text_color(color(tokens::FG2))
        .child(text)
}
