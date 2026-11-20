//! Retouch: each block back from the edit is watched in the preview;
//! remarks are anchored at the playhead and sent back to the engine, an
//! older version can be restored, or the block edited again with another
//! care level.

use super::studio::BlockState;
use super::{STEP_FINISH, STEP_MONTAGE, Shell, kind_color, label, mono, short_time};
use crate::theme::{color, tokens};
use crate::tr;
use enfait_core::BlockKind;
use enfait_media::Command;
use gpui_kit::prelude::*;
use gpui_kit::{ClickEvent, Context, Div, FontWeight, SharedString, div, px, relative};

impl Shell {
    /// Retouch opens on a block to check: the one shown if it is ready,
    /// else the first ready one.
    pub(super) fn select_ready_block(&mut self) {
        let Some(board) = &self.board else { return };
        let ready = |b: &enfait_core::Block| self.block_state(b) == BlockState::Ready;
        if board.block().is_some_and(ready) {
            return;
        }
        let first = board
            .project
            .blocks()
            .iter()
            .find(|b| ready(b))
            .map(|b| b.id);
        if let (Some(id), Some(board)) = (first, &mut self.board) {
            board.select(Some(id));
            self.retouches.clear();
            self.needs_sync = true;
        }
    }

    /// Player, then the blocks to check.
    pub(super) fn retouch_screen(
        &mut self,
        playing: Option<f64>,
        below: Option<Div>,
        cx: &mut Context<Self>,
    ) -> Div {
        let narrow = below.is_some();
        let player = self.player_pane(narrow, cx);
        let screen = div().flex_1().min_w(px(0.)).flex().flex_col().child(player);
        let Some(board) = &self.board else {
            return screen.child(
                div()
                    .p(px(20.))
                    .text_color(color(tokens::MUTE))
                    .child(tr!("montage.no_board")),
            );
        };
        let rate = board.project.settings().frame_rate;
        let playing = playing.and_then(|t| board.block_at(t));
        let selected = board.selected;
        let listeners: &Context<Self> = cx;
        let mut ready = 0;
        let rows: Vec<Div> = board
            .project
            .blocks()
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let state = self.block_state(b);
                if matches!(state, BlockState::Ready | BlockState::Mounted) {
                    ready += 1;
                }
                let id = b.id;
                let chosen = selected == Some(id);
                let tag = match &state {
                    BlockState::NotAnimated => Some((tr!("montage.no_edit"), tokens::MUTE)),
                    other => other.label().map(|l| (l, other.tone())),
                };
                div().child(
                    div()
                        .id(("retouch-row", i))
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .px(px(20.))
                        .py(px(8.))
                        .border_b_1()
                        .border_color(color(tokens::LINE))
                        .cursor_pointer()
                        .when(chosen, |d| d.bg(color(tokens::RAISED)))
                        .when(!chosen, |d| d.hover(|d| d.bg(color(tokens::PANEL))))
                        .child(div().flex_none().size(px(8.)).bg(color(kind_color(b.kind))))
                        .child(
                            mono(tr!(
                                "board.scene",
                                n = format!("{:02}", i.saturating_add(1))
                            ))
                            .flex_none()
                            .w(px(80.))
                            .text_color(color(
                                if playing == Some(id) {
                                    tokens::SIGNAL
                                } else {
                                    tokens::FG
                                },
                            )),
                        )
                        .child(
                            mono(format!(
                                "{} → {}",
                                short_time(b.range.start(), rate),
                                short_time(b.range.end(), rate)
                            ))
                            .flex_none()
                            .w(px(110.))
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
                                .child(SharedString::from(if b.description.trim().is_empty() {
                                    board.text_of(b)
                                } else {
                                    b.description.clone()
                                })),
                        )
                        .children(tag.map(|(text, tone)| {
                            div()
                                .px(px(5.))
                                .rounded(px(3.))
                                .border_1()
                                .border_color(color(tone))
                                .text_size(px(11.))
                                .text_color(color(tone))
                                .child(text)
                        }))
                        .on_click(
                            listeners.listener(move |shell, _: &ClickEvent, window, cx| {
                                shell.retouches.clear();
                                shell.select_block(Some(id), true, window, cx);
                            }),
                        ),
                )
            })
            .collect();
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
                            .child(tr!("retouch.title")),
                    )
                    .child(
                        mono(tr!("retouch.ready", count = ready))
                            .flex_1()
                            .text_size(px(12.))
                            .text_color(color(tokens::MUTE)),
                    )
                    .child(
                        super::screens::button("retouch-montage", tr!("steps.montage"), false)
                            .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| {
                                shell.go_to(STEP_MONTAGE, cx);
                            })),
                    )
                    .child(
                        super::screens::button("retouch-finish", tr!("retouch.to_finish"), false)
                            .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| {
                                shell.go_to(STEP_FINISH, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .id("retouch-list")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .children(rows),
            )
            .when_some(below, |d, panel| {
                d.child(
                    panel
                        .h(relative(0.45))
                        .border_t_1()
                        .border_color(color(tokens::LINE_STRONG)),
                )
            })
    }

    /// Right panel: what was asked, remarks, versions.
    pub(super) fn retouch_panel(&mut self, side: bool, cx: &mut Context<Self>) -> Div {
        let panel = div()
            .flex_none()
            .flex()
            .flex_col()
            .bg(color(tokens::PANEL))
            .when(side, |d| d.border_l_1().border_color(color(tokens::LINE)));
        let Some((index, block)) = self.board.as_ref().and_then(|b| {
            let block = b.block()?.clone();
            let index = b.project.blocks().iter().position(|x| x.id == block.id)?;
            Some((index, block))
        }) else {
            return panel.child(
                div()
                    .p(px(16.))
                    .text_size(px(12.))
                    .text_color(color(tokens::FG2))
                    .child(tr!("board.select_block")),
            );
        };
        let state = self.block_state(&block);
        let controls = self.retouch_controls(cx);
        let id = block.id;
        let start = block.range.start().as_secs_f64();
        let message = match &state {
            BlockState::Ready => None,
            BlockState::NotAnimated => Some(tr!("retouch.not_edited")),
            BlockState::Mounted | BlockState::Mounting | BlockState::NeedsBroll => {
                Some(tr!("retouch.broll"))
            }
            BlockState::Failed(e) => Some(tr!("retouch.failed", error = e)),
            BlockState::Queued | BlockState::Running => Some(tr!("retouch.in_edit")),
            BlockState::ToGenerate => Some(tr!("retouch.not_sent")),
        };
        let editable = matches!(
            block.kind,
            BlockKind::Animation | BlockKind::Mixed | BlockKind::Effect
        ) && !matches!(state, BlockState::Queued | BlockState::Running);
        panel.child(
            div()
                .id("retouch-panel")
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .p(px(16.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(
                            mono(tr!(
                                "board.scene",
                                n = format!("{:02}", index.saturating_add(1))
                            ))
                            .text_size(px(16.))
                            .font_weight(FontWeight::MEDIUM),
                        )
                        .child(div().flex_1())
                        .child(
                            super::studio::chip("replay", tr!("retouch.replay"), false, true)
                                .on_click(cx.listener(move |shell, _: &ClickEvent, _, cx| {
                                    shell.command(Command::Seek(start), cx);
                                    shell.command(Command::Forward, cx);
                                })),
                        ),
                )
                .child(label(tr!("board.label.visual")))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(color(if block.description.is_empty() {
                            tokens::MUTE
                        } else {
                            tokens::FG
                        }))
                        .child(if block.description.is_empty() {
                            tr!("retouch.no_description")
                        } else {
                            SharedString::from(block.description.clone())
                        }),
                )
                .when_some(message, |d, m| {
                    d.child(
                        div()
                            .text_size(px(12.))
                            .text_color(color(tokens::FG2))
                            .child(m),
                    )
                })
                .when_some(controls, |d, c| {
                    d.child(label(tr!("retouch.remarks_help"))).child(c)
                })
                .when(editable, |d| {
                    d.child(
                        super::studio::chip("edit-again", tr!("retouch.edit_again"), false, true)
                            .on_click(cx.listener(move |shell, _: &ClickEvent, _, cx| {
                                if let Some(board) = &mut shell.board {
                                    board.select(Some(id));
                                }
                                shell.open_care(cx);
                            })),
                    )
                })
                .child(
                    div()
                        .id("to-finish")
                        .pt(px(8.))
                        .cursor_pointer()
                        .text_size(px(12.))
                        .text_color(color(tokens::FG2))
                        .hover(|d| d.text_color(color(tokens::SIGNAL)))
                        .child(tr!("retouch.tracks_hint"))
                        .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| {
                            shell.go_to(STEP_FINISH, cx);
                        })),
                ),
        )
    }
}
