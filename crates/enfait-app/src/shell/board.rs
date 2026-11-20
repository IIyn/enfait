//! Storyboard screen: the blocks of the edit and the inspector of the
//! selected block (kind, description, words, moments, transition, split and
//! merge), with undo / redo.

use super::{Shell, kind_color, kind_label, label, mono, short_time};
use crate::session::Start;
use crate::storyboard::{self, Storyboard};
use crate::theme::{ROW_HEIGHT, color, tokens};
use crate::tr;
use enfait_core::{
    AspectRatio, Block, BlockId, BlockKind, Edit, Flicks, FrameRate, ProjectSettings, TimeRange,
    Transition,
};
use enfait_media::Command;
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::prelude::*;
use gpui_kit::{
    ClickEvent, Context, Div, DragMoveEvent, ElementId, FontWeight, SharedString, Stateful, Window,
    anchored, deferred, div, px, relative,
};

/// Block kinds offered when the take is filmed: video only, video with an
/// animation, animation only, then effect and b-roll.
const VIDEO_KINDS: [BlockKind; 5] = [
    BlockKind::Rush,
    BlockKind::Mixed,
    BlockKind::Animation,
    BlockKind::Effect,
    BlockKind::Broll,
];

/// Without a filmed take there is only animation or a b-roll shot.
const VOICE_KINDS: [BlockKind; 2] = [BlockKind::Animation, BlockKind::Broll];

/// Small toggle button; `on`: chosen; `enabled`: clickable.
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

fn heading(text: impl Into<SharedString>) -> Div {
    label(text).pt(px(4.))
}

/// Time to the tenth of a second: `M:SS.d`.
fn seconds(t: Flicks) -> String {
    let s = t.as_secs_f64().max(0.0);
    format!("{}:{:04.1}", (s / 60.0).floor(), s % 60.0)
}

impl Shell {
    /// Title of the project: the one given at creation, else the first
    /// heading of the script, else the take name.
    pub(super) fn title(&self) -> String {
        let given = self.session.data.title.trim();
        if !given.is_empty() {
            return given.to_owned();
        }
        let heading = self
            .session
            .script
            .lines()
            .find_map(|l| l.trim().strip_prefix("# "))
            .map(str::trim)
            .filter(|t| !t.is_empty());
        heading
            .map(str::to_owned)
            .or_else(|| {
                self.session
                    .data
                    .rush
                    .as_ref()
                    .and_then(|p| p.file_stem())
                    .map(|s| s.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| tr!("common.untitled").to_string())
    }

    /// Project settings: those of the take video (format, frame rate).
    pub(super) fn settings(&self) -> ProjectSettings {
        if let Some(board) = &self.board {
            return *board.project.settings();
        }
        ProjectSettings::default().with_aspect(self.session.data.aspect.unwrap_or_default())
    }

    /// First settings of the project: the format chosen at creation (else
    /// that of the take's frames), the frame rate of the take.
    fn settings_from_media(&self, cx: &Context<Self>) -> ProjectSettings {
        let chosen = self.session.data.aspect;
        let Some(video) = self.player.read(cx).media_info().and_then(|i| i.video) else {
            return ProjectSettings::default().with_aspect(chosen.unwrap_or_default());
        };
        let frame_rate = if (20..=60).contains(&video.frame_rate.nominal_fps()) {
            video.frame_rate
        } else {
            FrameRate::FPS_30
        };
        let rush = AspectRatio::nearest(video.width, video.height);
        let aspect = chosen.unwrap_or(rush);
        let mut settings = ProjectSettings {
            frame_rate,
            ..ProjectSettings::default()
        }
        .with_aspect(aspect);
        if aspect != rush {
            settings.framing = enfait_core::Framing::Fill;
        }
        settings
    }

    /// Builds (or rebuilds) the storyboard if the edit changed.
    pub(super) fn refresh_board(&mut self, cx: &mut Context<Self>) {
        use crate::cut::CutStatus;
        let analysed = self.cut.analysis.is_some() || !self.uses_cut();
        let (Some(transcript), true) = (self.cut.transcript.clone(), analysed) else {
            self.board_notice = Some(match &self.cut.status {
                CutStatus::Transcribing => tr!("board.transcribing"),
                CutStatus::Failed(e) => e.clone(),
                _ => tr!("board.import_first"),
            });
            return;
        };
        let ranges = self.edit_ranges();
        if ranges.is_empty() {
            self.board_notice = Some(tr!("board.nothing_found"));
            return;
        }
        self.board_notice = None;
        let key = storyboard::key(&ranges);
        if self.board.as_ref().is_some_and(|b| b.key == key) {
            return;
        }
        let parts: Result<Vec<TimeRange>, _> = ranges
            .iter()
            .map(|&(a, b)| {
                #[allow(clippy::cast_possible_truncation)]
                let ms = |t: f64| Flicks::from_millis((t * 1000.0).round() as i64);
                TimeRange::new(ms(a)?, ms(b)?)
            })
            .collect();
        let built = parts
            .map_err(anyhow::Error::from)
            .and_then(|parts| {
                Ok(enfait_speech::takes::montage_transcript(
                    &transcript,
                    &parts,
                )?)
            })
            .and_then(|montage| {
                let settings = self
                    .board
                    .as_ref()
                    .map_or_else(|| self.settings_from_media(cx), |b| *b.project.settings());
                Storyboard::build(&self.title(), settings, montage, key, self.board.as_ref())
            });
        match built {
            Ok(mut board) => {
                let first = board.project.blocks().first().map(|b| b.id);
                // Silent project: its subject goes to the first block.
                let brief = self.session.data.brief.trim().to_owned();
                if self.board.is_none()
                    && self.session.data.start == Start::Silent
                    && !brief.is_empty()
                    && let Some(id) = first
                {
                    let _ = board.apply(Edit::SetDescription {
                        block: id,
                        text: brief,
                    });
                    board.history = enfait_core::History::new();
                }
                board.select(first);
                board.save(self.session.dir());
                self.board = Some(board);
                self.needs_sync = true;
                // New edit: the preview must be rebuilt.
                self.push_snapshot();
                self.worker.request_preview();
            }
            Err(e) => {
                tracing::warn!("storyboard: {e:#}");
                self.board_notice = Some(tr!("board.failed", error = e.to_string()));
            }
        }
    }

    pub(super) fn save_board(&self) {
        if let Some(board) = &self.board {
            board.save(self.session.dir());
        }
        self.push_snapshot();
    }

    /// Applies an edit to the storyboard, saves it and refreshes the
    /// inspector.
    pub(super) fn edit_board(&mut self, edit: Edit, window: &mut Window, cx: &mut Context<Self>) {
        let Some(board) = &mut self.board else { return };
        if let Err(e) = board.apply(edit) {
            tracing::warn!("edit refused: {e}");
        }
        self.save_board();
        self.sync_inputs(window, cx);
        cx.notify();
    }

    /// Copies the selected block into the inspector fields.
    pub(super) fn sync_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.needs_sync = false;
        let block = self.board.as_ref().and_then(Storyboard::block);
        let description = block.map(|b| b.description.clone()).unwrap_or_default();
        let transition = match block.map(|b| &b.transition_after) {
            Some(Transition::Described(t)) => t.clone(),
            _ => String::new(),
        };
        let note = self
            .board
            .as_ref()
            .and_then(|board| {
                let at = board.moment?;
                board
                    .block()?
                    .moments
                    .iter()
                    .find(|m| m.at == at)
                    .map(|m| m.note.clone())
            })
            .unwrap_or_default();
        self.description
            .update(cx, |s, cx| s.set_value(description, window, cx));
        self.transition_text
            .update(cx, |s, cx| s.set_value(transition, window, cx));
        self.moment_note
            .update(cx, |s, cx| s.set_value(note, window, cx));
    }

    pub(super) fn commit_description(&mut self, cx: &mut Context<Self>) {
        let text = self.description.read(cx).value().to_string();
        let Some(board) = &mut self.board else { return };
        let Some(block) = board.block() else { return };
        if block.description == text {
            return;
        }
        let edit = Edit::SetDescription {
            block: block.id,
            text,
        };
        if board.apply(edit).is_ok() {
            self.save_board();
            cx.notify();
        }
    }

    pub(super) fn commit_transition_text(&mut self, cx: &mut Context<Self>) {
        let text = self.transition_text.read(cx).value().trim().to_owned();
        let Some(board) = &mut self.board else { return };
        let Some(block) = board.block() else { return };
        let Transition::Described(current) = &block.transition_after else {
            return;
        };
        if *current == text {
            return;
        }
        let edit = Edit::SetTransition {
            block: block.id,
            transition: Transition::Described(text),
        };
        if board.apply(edit).is_ok() {
            self.save_board();
            cx.notify();
        }
    }

    pub(super) fn commit_moment_note(&mut self, cx: &mut Context<Self>) {
        let note = self.moment_note.read(cx).value().trim().to_owned();
        let Some(board) = &mut self.board else { return };
        let (Some(block), Some(at)) = (board.block(), board.moment) else {
            return;
        };
        if block.moments.iter().any(|m| m.at == at && m.note == note) {
            return;
        }
        let edit = Edit::SetMomentNote {
            block: block.id,
            at,
            note,
        };
        if board.apply(edit).is_ok() {
            self.save_board();
            cx.notify();
        }
    }

    /// Saves what is being typed before switching context.
    pub(super) fn commit_inputs(&mut self, cx: &mut Context<Self>) {
        self.commit_description(cx);
        self.commit_transition_text(cx);
        self.commit_moment_note(cx);
    }

    /// Click on a block (list or timeline): select it; Shift extends the
    /// choice to a range, Ctrl/Cmd adds or removes it.
    pub(super) fn click_block(
        &mut self,
        id: BlockId,
        modifiers: gpui_kit::Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if modifiers.shift || modifiers.secondary(cx) {
            self.commit_inputs(cx);
            let Some(board) = &mut self.board else { return };
            if modifiers.shift {
                board.select_range(id);
            } else {
                board.toggle_marked(id);
            }
            self.sync_inputs(window, cx);
            cx.notify();
            return;
        }
        self.select_block(Some(id), true, window, cx);
    }

    pub(super) fn select_block(
        &mut self,
        id: Option<BlockId>,
        seek: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_inputs(cx);
        let Some(board) = &mut self.board else { return };
        board.select(id);
        let start = board.block().map(|b| b.range.start().as_secs_f64());
        self.sync_inputs(window, cx);
        if seek && let Some(t) = start {
            self.command(Command::Seek(t), cx);
        }
        cx.notify();
    }

    pub(super) fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_inputs(cx);
        if let Some(board) = &mut self.board
            && board.undo()
        {
            self.save_board();
            self.sync_inputs(window, cx);
            cx.notify();
        }
    }

    pub(super) fn redo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_inputs(cx);
        if let Some(board) = &mut self.board
            && board.redo()
        {
            self.save_board();
            self.sync_inputs(window, cx);
            cx.notify();
        }
    }

    /// Word clicked in a block row: that block, then that word.
    fn pick_word(
        &mut self,
        block: BlockId,
        word: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.board.as_ref().and_then(|b| b.selected) != Some(block) {
            self.select_block(Some(block), false, window, cx);
        }
        self.select_word(word, window, cx);
    }

    fn select_word(&mut self, word: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_inputs(cx);
        let Some(board) = &mut self.board else { return };
        board.word = Some(word);
        board.moment = None;
        let start = board
            .transcript
            .words
            .get(word)
            .map(|w| w.range.start().as_secs_f64());
        self.sync_inputs(window, cx);
        if let Some(t) = start {
            self.command(Command::Seek((t - 0.05).max(0.0)), cx);
        }
        cx.notify();
    }

    fn select_moment(&mut self, at: Flicks, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_inputs(cx);
        let Some(board) = &mut self.board else { return };
        board.moment = Some(at);
        self.sync_inputs(window, cx);
        self.command(Command::Seek((at.as_secs_f64() - 0.05).max(0.0)), cx);
        cx.notify();
    }

    /// Cuts the selected block (or the one under the playhead) at the
    /// playhead.
    fn split_at_playhead(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.player.read(cx).position() else {
            return;
        };
        self.commit_inputs(cx);
        let Some(board) = &mut self.board else { return };
        if let Err(e) = board.split_at_time(t) {
            tracing::warn!("edit refused: {e}");
        }
        self.save_board();
        self.sync_inputs(window, cx);
        cx.notify();
    }

    /// A moment at the playhead in the selected block.
    fn moment_at_playhead(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.player.read(cx).position() else {
            return;
        };
        self.commit_inputs(cx);
        let Some(board) = &mut self.board else { return };
        if let Err(e) = board.add_moment_at(t) {
            tracing::warn!("edit refused: {e}");
        }
        self.save_board();
        self.sync_inputs(window, cx);
        cx.notify();
    }

    fn board_action(
        &mut self,
        action: fn(&mut Storyboard) -> Result<(), enfait_core::EditError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_inputs(cx);
        let Some(board) = &mut self.board else { return };
        if let Err(e) = action(board) {
            tracing::warn!("edit refused: {e}");
        }
        self.save_board();
        self.sync_inputs(window, cx);
        cx.notify();
    }

    /// Central column: the player, then the block list, and the inspector
    /// below it in a narrow window (`below`). `playing`: playhead (edit
    /// time).
    pub(super) fn board_screen(
        &mut self,
        playing: Option<f64>,
        below: Option<Div>,
        cx: &mut Context<Self>,
    ) -> Div {
        let narrow = below.is_some();
        let player = self.player_pane(narrow, cx);
        let Some(board) = &self.board else {
            let notice = self
                .board_notice
                .clone()
                .unwrap_or_else(|| tr!("board.none_yet"));
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
                        .child(notice),
                );
        };
        let rate = board.project.settings().frame_rate;
        let playing = playing.and_then(|t| board.block_at(t));
        let detail = tr!(
            "board.stats",
            blocks = board.project.blocks().len(),
            duration = short_time(board.project.duration(), rate)
        )
        .to_string();
        let can_undo = board.history.undo_label();
        let can_redo = board.history.redo_label();
        let studio_bar = self.studio_toolbar(cx);
        let states: Vec<super::studio::BlockState> = board
            .project
            .blocks()
            .iter()
            .map(|b| self.block_state(b))
            .collect();
        let listeners: &Context<Self> = cx;
        let rows: Vec<Div> = board
            .project
            .blocks()
            .iter()
            .enumerate()
            .map(|(i, block)| {
                let id = block.id;
                block_row(
                    i,
                    block,
                    board,
                    board.selected == Some(block.id),
                    board.marked.len() > 1 && board.marked.contains(&block.id),
                    playing == Some(block.id),
                    rate,
                    states.get(i),
                    listeners,
                )
                .on_click(
                    listeners.listener(move |shell, event: &ClickEvent, window, cx| {
                        shell.click_block(id, event.modifiers(), window, cx);
                    }),
                )
                .drag_over::<gpui_kit::ExternalPaths>(|style, _, _, _| {
                    style.bg(color(tokens::HOVER))
                })
                .on_drop(listeners.listener(
                    move |shell, paths: &gpui_kit::ExternalPaths, window, cx| {
                        cx.stop_propagation();
                        shell.drop_on_block(id, paths.paths(), window, cx);
                    },
                ))
            })
            .map(|row| div().child(row))
            .collect();
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
                            .child(tr!("board.title")),
                    )
                    .child(
                        mono(detail)
                            .flex_1()
                            .text_size(px(12.))
                            .text_color(color(tokens::MUTE)),
                    )
                    .child(
                        chip("undo", tr!("common.undo"), false, can_undo.is_some()).when(
                            can_undo.is_some(),
                            |d| {
                                d.on_click(cx.listener(|shell, _: &ClickEvent, window, cx| {
                                    shell.undo(window, cx);
                                }))
                            },
                        ),
                    )
                    .child(
                        chip("redo", tr!("common.redo"), false, can_redo.is_some()).when(
                            can_redo.is_some(),
                            |d| {
                                d.on_click(cx.listener(|shell, _: &ClickEvent, window, cx| {
                                    shell.redo(window, cx);
                                }))
                            },
                        ),
                    ),
            )
            .child(studio_bar)
            .child(
                div()
                    .id("blocks")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .children(rows),
            )
            .when_some(below, |d, inspector| {
                d.child(
                    inspector
                        .h(relative(0.45))
                        .border_t_1()
                        .border_color(color(tokens::LINE_STRONG)),
                )
            })
    }

    /// Inspector of the selected block: right panel (`side`) or strip under
    /// the block list.
    pub(super) fn inspector(&mut self, side: bool, cx: &mut Context<Self>) -> Div {
        let panel = div()
            .flex_none()
            .flex()
            .flex_col()
            .bg(color(tokens::PANEL))
            .when(side, |d| d.border_l_1().border_color(color(tokens::LINE)));
        let Some(board) = &self.board else {
            return panel.child(
                div()
                    .p(px(16.))
                    .text_size(px(12.))
                    .text_color(color(tokens::FG2))
                    .child(tr!("board.after_cut")),
            );
        };
        let Some(block) = board.block() else {
            return panel.child(
                div()
                    .p(px(16.))
                    .text_size(px(12.))
                    .text_color(color(tokens::FG2))
                    .child(tr!("board.select_block")),
            );
        };
        let index = board
            .project
            .blocks()
            .iter()
            .position(|b| b.id == block.id)
            .unwrap_or(0);
        let last = index.saturating_add(1) == board.project.blocks().len();
        let rate = board.project.settings().frame_rate;
        let words = board.words_of(block);
        let first_word = words.first().map(|(i, _)| *i);
        let chosen_word = board
            .word
            .and_then(|i| words.iter().find(|(w, _)| *w == i))
            .map(|(i, w)| (*i, w.text.clone(), w.range.start()));
        let moment_words: Vec<Flicks> = block.moments.iter().map(|m| m.at).collect();
        let can_split = chosen_word
            .as_ref()
            .is_some_and(|(i, _, _)| Some(*i) != first_word);
        let can_moment = chosen_word
            .as_ref()
            .is_some_and(|(_, _, at)| !moment_words.contains(at));
        let word_label = chosen_word.as_ref().map_or_else(String::new, |(_, t, _)| {
            format!(" {}", tr!("common.quoted", text = t))
        });
        let transition_controls = self.transition_controls(cx);
        let studio = self.studio_inspector(cx);
        let suggestions = self.mention_suggestions(super::mention::Field::Description, cx);
        let mentioned = self.mentioned_files(&block.description);
        let field_height = self.panel_width(super::split::Splitter::Description, cx);
        // The list of files floats under the field: in the panel it would
        // fall out of sight when the panel is short.
        let suggestions_width = if side {
            (self.panel_width(super::split::Splitter::Inspector, cx) - px(32.)).max(px(200.))
        } else {
            px(360.)
        };
        let has_words = !words.is_empty();
        let kinds: &[BlockKind] = if self.take_has_video()
            || matches!(
                block.kind,
                BlockKind::Rush | BlockKind::Mixed | BlockKind::Effect
            ) {
            &VIDEO_KINDS
        } else {
            &VOICE_KINDS
        };
        let listeners: &Context<Self> = cx;

        let kinds = kinds.iter().map(|&kind| {
            let id = block.id;
            chip(
                ("kind", kind as usize),
                kind_label(kind),
                block.kind == kind,
                true,
            )
            .on_click(
                listeners.listener(move |shell, _: &ClickEvent, window, cx| {
                    shell.edit_board(Edit::SetKind { block: id, kind }, window, cx);
                }),
            )
        });
        let word_chips = words.iter().map(|(i, w)| {
            let i = *i;
            let selected = board.word == Some(i);
            let marked = moment_words.contains(&w.range.start());
            div()
                .id(("word", i))
                .px(px(4.))
                .rounded(px(2.))
                .cursor_pointer()
                .text_size(px(13.))
                .map(|d| {
                    if selected {
                        d.bg(color(tokens::SIGNAL))
                            .text_color(color(tokens::ON_SIGNAL))
                    } else if marked {
                        d.text_color(color(tokens::MIXED))
                            .hover(|d| d.bg(color(tokens::HOVER)))
                    } else {
                        d.text_color(color(tokens::FG2))
                            .hover(|d| d.bg(color(tokens::HOVER)))
                    }
                })
                .child(SharedString::from(w.text.clone()))
                .on_click(
                    listeners.listener(move |shell, _: &ClickEvent, window, cx| {
                        shell.select_word(i, window, cx);
                    }),
                )
        });
        let moments = block.moments.iter().map(|m| {
            let at = m.at;
            let id = block.id;
            let selected = board.moment == Some(at);
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .id(("moment", usize::try_from(at.get()).unwrap_or(0)))
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(3.))
                        .cursor_pointer()
                        .when(selected, |d| d.bg(color(tokens::RAISED)))
                        .hover(|d| d.bg(color(tokens::HOVER)))
                        .child(
                            div()
                                .flex()
                                .gap(px(6.))
                                .text_size(px(12.))
                                .child(div().text_color(color(tokens::MIXED)).child(
                                    if m.word.is_empty() {
                                        SharedString::from("◆")
                                    } else {
                                        tr!("common.quoted", text = m.word)
                                    },
                                ))
                                .child(
                                    mono(seconds(at))
                                        .text_size(px(11.))
                                        .text_color(color(tokens::MUTE)),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(color(if m.note.is_empty() {
                                    tokens::MUTE
                                } else {
                                    tokens::FG
                                }))
                                .child(if m.note.is_empty() {
                                    tr!("board.no_note")
                                } else {
                                    SharedString::from(m.note.clone())
                                }),
                        )
                        .on_click(
                            listeners.listener(move |shell, _: &ClickEvent, window, cx| {
                                shell.select_moment(at, window, cx);
                            }),
                        ),
                )
                .child(
                    div()
                        .id(("remove-moment", usize::try_from(at.get()).unwrap_or(0)))
                        .px(px(6.))
                        .cursor_pointer()
                        .text_color(color(tokens::MUTE))
                        .hover(|d| d.text_color(color(tokens::ERROR)))
                        .child("×")
                        .on_click(
                            listeners.listener(move |shell, _: &ClickEvent, window, cx| {
                                shell.edit_board(Edit::RemoveMoment { block: id, at }, window, cx);
                            }),
                        ),
                )
        });
        let has_moment = board.moment.is_some();
        let transition = block.transition_after.clone();
        let id = block.id;
        let transitions = [
            (tr!("board.transition.cut"), Transition::Cut),
            (tr!("board.transition.auto"), Transition::Auto),
            (
                tr!("board.transition.described"),
                Transition::Described(String::new()),
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(n, (name, kind))| {
            let on = std::mem::discriminant(&kind) == std::mem::discriminant(&transition);
            chip(("transition", n), name, on, true).on_click(listeners.listener(
                move |shell, _: &ClickEvent, window, cx| {
                    let transition = match &kind {
                        Transition::Described(_) => {
                            let text = shell.transition_text.read(cx).value().trim().to_owned();
                            Transition::Described(text)
                        }
                        other => other.clone(),
                    };
                    shell.edit_board(
                        Edit::SetTransition {
                            block: id,
                            transition,
                        },
                        window,
                        cx,
                    );
                },
            ))
        });
        let described = matches!(transition, Transition::Described(_));
        let range = format!(
            "{} → {}",
            short_time(block.range.start(), rate),
            short_time(block.range.end(), rate)
        );

        panel.child(
            div()
                .id("inspector")
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
                        .items_baseline()
                        .gap(px(10.))
                        .child(
                            mono(tr!(
                                "board.scene",
                                n = format!("{:02}", index.saturating_add(1))
                            ))
                            .text_size(px(16.))
                            .font_weight(FontWeight::MEDIUM),
                        )
                        .child(
                            mono(range)
                                .text_size(px(12.))
                                .text_color(color(tokens::MUTE)),
                        ),
                )
                .child(heading(tr!("board.label.type")))
                .child(div().flex().flex_wrap().gap(px(4.)).children(kinds))
                .when_some(studio, |d, s| d.child(s))
                .child(
                    div()
                        .flex()
                        .items_end()
                        .justify_between()
                        .child(heading(tr!("board.label.visual")))
                        .child(
                            div()
                                .id("add-files")
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(22.))
                                .rounded(px(3.))
                                .border_1()
                                .border_color(color(tokens::LINE_STRONG))
                                .text_size(px(15.))
                                .text_color(color(tokens::FG))
                                .cursor_pointer()
                                .hover(|d| d.bg(color(tokens::HOVER)))
                                .child("+")
                                .tooltip(|window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(tr!(
                                        "board.add_files"
                                    ))
                                    .build(window, cx)
                                })
                                .on_click(listeners.listener(
                                    move |shell, _: &ClickEvent, window, cx| {
                                        shell.pick_for_block(id, window, cx);
                                    },
                                )),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .relative()
                                .h(field_height)
                                .on_drag_move(listeners.listener(
                                    |shell, event: &DragMoveEvent<super::split::Resize>, _, cx| {
                                        shell.drag_description(event, cx);
                                    },
                                ))
                                .child(Textarea::new(&self.description).h_full())
                                .when_some(suggestions, |d, s| {
                                    d.child(
                                        div().absolute().left_0().top_full().mt(px(2.)).child(
                                            deferred(
                                                anchored()
                                                    .snap_to_window_with_margin(px(8.))
                                                    .child(s.w(suggestions_width)),
                                            )
                                            .with_priority(1),
                                        ),
                                    )
                                }),
                        )
                        .child(super::split::description_grip(listeners)),
                )
                .when(!mentioned.is_empty(), |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(4.))
                            .text_size(px(11.))
                            .text_color(color(tokens::FG2))
                            .child(tr!("board.mentioned"))
                            .children(mentioned.into_iter().map(|name| {
                                mono(format!("@{name}"))
                                    .px(px(4.))
                                    .rounded(px(2.))
                                    .bg(color(tokens::RAISED))
                                    .text_color(color(tokens::SIGNAL))
                            })),
                    )
                })
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(color(tokens::MUTE))
                        .child(tr!("board.mention_hint")),
                )
                .when(has_words, |d| d.child(heading(tr!("board.label.words"))))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_y(px(2.))
                        .line_height(px(22.))
                        .children(word_chips),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(4.))
                        .when(!has_words, |d| {
                            d.child(
                                chip("split-playhead", tr!("board.split_playhead"), false, true)
                                    .on_click(listeners.listener(
                                        |shell, _: &ClickEvent, window, cx| {
                                            shell.split_at_playhead(window, cx);
                                        },
                                    )),
                            )
                            .child(
                                chip("moment-playhead", tr!("board.moment_playhead"), false, true)
                                    .on_click(listeners.listener(
                                        |shell, _: &ClickEvent, window, cx| {
                                            shell.moment_at_playhead(window, cx);
                                        },
                                    )),
                            )
                        })
                        .when(has_words, |d| {
                            d.child(
                                chip(
                                    "split",
                                    tr!("board.split_before", word = word_label.clone()),
                                    false,
                                    can_split,
                                )
                                .when(can_split, |d| {
                                    d.on_click(listeners.listener(
                                        |shell, _: &ClickEvent, window, cx| {
                                            shell.board_action(
                                                Storyboard::split_at_word,
                                                window,
                                                cx,
                                            );
                                        },
                                    ))
                                }),
                            )
                        })
                        .when(has_words, |d| {
                            d.child(
                                chip(
                                    "moment",
                                    tr!("board.moment_on", word = word_label),
                                    false,
                                    can_moment,
                                )
                                .when(can_moment, |d| {
                                    d.on_click(listeners.listener(
                                        |shell, _: &ClickEvent, window, cx| {
                                            shell.board_action(Storyboard::add_moment, window, cx);
                                        },
                                    ))
                                }),
                            )
                        }),
                )
                .when(!block.moments.is_empty(), |d| {
                    d.child(heading(tr!("board.label.moments")))
                        .children(moments)
                })
                .when(has_moment, |d| d.child(Input::new(&self.moment_note)))
                .when(!last, |d| {
                    d.child(heading(tr!("board.label.transition")))
                        .child(div().flex().gap(px(4.)).children(transitions))
                        .when(described, |d| d.child(Input::new(&self.transition_text)))
                        .when_some(transition_controls, |d, c| d.child(c))
                        .child(
                            chip("merge", tr!("board.merge"), false, true)
                                .mt(px(6.))
                                .on_click(listeners.listener(
                                    move |shell, _: &ClickEvent, window, cx| {
                                        shell.edit_board(
                                            Edit::MergeWithNext { block: id },
                                            window,
                                            cx,
                                        );
                                    },
                                )),
                        )
                }),
        )
    }
}

/// One row of the storyboard.
#[allow(clippy::too_many_arguments)]
fn block_row(
    index: usize,
    block: &Block,
    board: &Storyboard,
    selected: bool,
    marked: bool,
    playing: bool,
    rate: FrameRate,
    state: Option<&super::studio::BlockState>,
    cx: &Context<Shell>,
) -> Stateful<Div> {
    let words = selected.then(|| row_words(index, block, board, cx));
    let badge = state.and_then(|s| s.label().map(|l| (l, s.tone())));
    let range = format!(
        "{} → {}",
        short_time(block.range.start(), rate),
        short_time(block.range.end(), rate)
    );
    let description: SharedString = if block.description.is_empty() {
        tr!("board.describe_or_suggest")
    } else {
        block.description.clone().into()
    };
    let transition = match &block.transition_after {
        Transition::Cut => None,
        Transition::Auto => Some(tr!("board.transition_auto")),
        Transition::Described(text) => Some(tr!("board.transition_text", text = text)),
    };
    let moments: Vec<SharedString> = block
        .moments
        .iter()
        .map(|m| {
            let word = if m.word.is_empty() {
                seconds(m.at)
            } else {
                m.word.clone()
            };
            SharedString::from(if m.note.is_empty() {
                format!("◆ {word}")
            } else {
                tr!("board.moment_line", word = word, note = m.note).to_string()
            })
        })
        .collect();
    div()
        .id(("block", index))
        .cursor_pointer()
        .border_b_1()
        .border_color(color(tokens::LINE))
        .when(selected || marked, |d| d.bg(color(tokens::RAISED)))
        .when(!selected && !marked, |d| {
            d.hover(|d| d.bg(color(tokens::PANEL)))
        })
        .child(
            div()
                .flex()
                .child(div().flex_none().w(px(3.)).when(marked || selected, |d| {
                    d.bg(color(if marked {
                        tokens::SIGNAL
                    } else {
                        tokens::LINE_STRONG
                    }))
                }))
                .child(
                    div()
                        .w(px(129.))
                        .flex_none()
                        .px(px(20.))
                        .py(px(10.))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            mono(tr!(
                                "board.scene",
                                n = format!("{:02}", index.saturating_add(1))
                            ))
                            .text_size(px(15.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(color(if playing {
                                tokens::SIGNAL
                            } else {
                                tokens::FG
                            })),
                        )
                        .child(
                            mono(range)
                                .text_size(px(11.))
                                .text_color(color(tokens::MUTE)),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .py(px(10.))
                        .pr(px(20.))
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .text_size(px(12.))
                                .child(div().size(px(8.)).bg(color(kind_color(block.kind))))
                                .child(kind_label(block.kind))
                                .when_some(badge, |d, (text, tone)| {
                                    d.child(
                                        div()
                                            .px(px(5.))
                                            .rounded(px(3.))
                                            .border_1()
                                            .border_color(color(tone))
                                            .text_size(px(11.))
                                            .text_color(color(tone))
                                            .child(text),
                                    )
                                })
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .overflow_hidden()
                                        .text_color(color(tokens::MUTE))
                                        .when(words.is_none(), |d| {
                                            d.child({
                                                let said = board.text_of(block);
                                                if said.is_empty() {
                                                    SharedString::default()
                                                } else {
                                                    tr!("common.quoted", text = said)
                                                }
                                            })
                                        }),
                                ),
                        )
                        .when_some(words.flatten(), |d, words| d.child(words))
                        .child(
                            div()
                                .text_color(color(if block.description.is_empty() {
                                    tokens::MUTE
                                } else {
                                    tokens::FG
                                }))
                                .child(description),
                        )
                        .children(moments.into_iter().map(|m| {
                            div()
                                .text_size(px(12.))
                                .text_color(color(tokens::MIXED))
                                .child(m)
                        })),
                ),
        )
        .when_some(transition, |d, text| {
            d.child(
                div()
                    .flex()
                    .items_center()
                    .h(px(ROW_HEIGHT))
                    .pl(px(132.))
                    .bg(color(tokens::PANEL))
                    .text_size(px(12.))
                    .text_color(color(tokens::FG2))
                    .child(text),
            )
        })
}

/// Words of the selected block in its row, clickable (go to the word, choose
/// where to cut), with « Cut before » and « Merge with the next one ».
/// `None` without words (silent passage: the inspector cuts at the playhead).
fn row_words(index: usize, block: &Block, board: &Storyboard, cx: &Context<Shell>) -> Option<Div> {
    let words = board.words_of(block);
    let first = words.first().map(|(i, _)| *i)?;
    let id = block.id;
    let chosen = board
        .word
        .and_then(|i| words.iter().find(|(w, _)| *w == i))
        .map(|(i, w)| (*i, w.text.clone()));
    let can_split = chosen.as_ref().is_some_and(|(i, _)| *i != first);
    let last = index.saturating_add(1) >= board.project.blocks().len();
    let chips = words.iter().map(|(i, w)| {
        let i = *i;
        let on = chosen.as_ref().is_some_and(|(c, _)| *c == i);
        div()
            .id(("row-word", i))
            .px(px(3.))
            .rounded(px(2.))
            .cursor_pointer()
            .map(|d| {
                if on {
                    d.bg(color(tokens::SIGNAL))
                        .text_color(color(tokens::ON_SIGNAL))
                } else {
                    d.text_color(color(tokens::FG2))
                        .hover(|d| d.bg(color(tokens::HOVER)))
                }
            })
            .child(SharedString::from(w.text.clone()))
            .on_click(cx.listener(move |shell, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                shell.pick_word(id, i, window, cx);
            }))
    });
    let split_label = chosen.as_ref().map_or_else(
        || tr!("board.row_hint"),
        |(_, text)| {
            tr!(
                "board.split_before",
                word = format!(" {}", tr!("common.quoted", text = text))
            )
        },
    );
    Some(
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_y(px(2.))
                    .line_height(px(22.))
                    .text_size(px(13.))
                    .children(chips),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(4.))
                    .child(
                        chip(("row-split", index), split_label, false, can_split).when(
                            can_split,
                            |d| {
                                d.on_click(cx.listener(|shell, _: &ClickEvent, window, cx| {
                                    cx.stop_propagation();
                                    shell.board_action(Storyboard::split_at_word, window, cx);
                                }))
                            },
                        ),
                    )
                    .when(!last, |d| {
                        d.child(
                            chip(("row-merge", index), tr!("board.merge"), false, true).on_click(
                                cx.listener(move |shell, _: &ClickEvent, window, cx| {
                                    cx.stop_propagation();
                                    shell.edit_board(Edit::MergeWithNext { block: id }, window, cx);
                                }),
                            ),
                        )
                    }),
            ),
    )
}
