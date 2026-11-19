//! Voice (script, prompter, take import) and Cut (takes and edit) screens.

use super::{Shell, mono};
use crate::cut::{CutStatus, clock};
use crate::theme::{ROW_HEIGHT, color, tokens};
use crate::tr;
use enfait_speech::script::sentences;
use gpui_kit::component::input::Textarea;
use gpui_kit::prelude::*;
use gpui_kit::{
    ClickEvent, Context, Div, ElementId, FontWeight, SharedString, Stateful, Window, div, px,
    relative,
};

/// Average reading rate, to estimate the duration of a script.
const WORDS_PER_MINUTE: f64 = 150.0;

/// Plain button; `primary`: in the signal colour.
fn button(id: impl Into<ElementId>, text: impl Into<SharedString>, primary: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .h(px(28.))
        .px(px(12.))
        .rounded(px(3.))
        .cursor_pointer()
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .map(|d| {
            if primary {
                d.bg(color(tokens::SIGNAL))
                    .text_color(color(tokens::ON_SIGNAL))
                    .hover(|d| d.bg(color(tokens::SIGNAL_HOVER)))
            } else {
                d.border_1()
                    .border_color(color(tokens::LINE_STRONG))
                    .text_color(color(tokens::FG))
                    .hover(|d| d.bg(color(tokens::HOVER)))
            }
        })
        .child(text.into())
}

/// Title bar of a screen.
fn toolbar(title: impl Into<SharedString>, detail: String) -> Div {
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
                .child(title.into()),
        )
        .child(
            mono(detail)
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .text_size(px(12.))
                .text_color(color(tokens::MUTE)),
        )
}

impl Shell {
    pub(super) fn voice_screen(&mut self, cx: &mut Context<Self>) -> Div {
        let found = sentences(&self.session.script);
        let words: usize = found.iter().map(|s| s.tokens.len()).sum();
        #[allow(clippy::cast_precision_loss)]
        let minutes = words as f64 / WORDS_PER_MINUTE;
        let detail = tr!(
            "voice.stats",
            sentences = found.len(),
            words = words,
            duration = clock(minutes * 60.0)
        )
        .to_string();
        let rush = self
            .session
            .data
            .rush
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned());
        div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .child(
                toolbar(tr!("voice.title"), detail)
                    .child(
                        button("prompter", tr!("voice.prompter"), false).on_click(cx.listener(
                            |shell, _: &ClickEvent, window: &mut Window, cx| {
                                shell.open_prompter(window, cx);
                            },
                        )),
                    )
                    .child(button("import", tr!("voice.import"), true).on_click(
                        cx.listener(|shell, _: &ClickEvent, _, cx| shell.prompt_import(cx)),
                    ))
                    .child(button("magic", tr!("voice.magic"), false).on_click(
                        cx.listener(|shell, _: &ClickEvent, _, cx| shell.start_magic(cx)),
                    )),
            )
            .when_some(rush, |d, name| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(20.))
                        .h(px(ROW_HEIGHT))
                        .bg(color(tokens::PANEL))
                        .border_b_1()
                        .border_color(color(tokens::LINE))
                        .text_size(px(12.))
                        .text_color(color(tokens::FG2))
                        .child(tr!("voice.take", name = name))
                        .child(
                            div()
                                .id("to-cut")
                                .cursor_pointer()
                                .text_color(color(tokens::SIGNAL))
                                .child(tr!("voice.see_cut"))
                                .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| {
                                    shell.go_to(super::STEP_CUT, cx);
                                })),
                        ),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .p(px(16.))
                    .child(Textarea::new(&self.script).h_full()),
            )
    }

    /// `playing`: time of the take being played.
    pub(super) fn cut_screen(&mut self, playing: Option<f64>, cx: &mut Context<Self>) -> Div {
        let montage_duration = self
            .player
            .read(cx)
            .edl()
            .map_or(0.0, |e| if self.cut.montage { e.duration() } else { 0.0 });
        let detail = self.cut.summary(montage_duration);
        let montage = self.cut.montage;
        let current = playing.and_then(|t| self.cut.sentence_at(t));

        let notice: Option<(SharedString, crate::theme::Token)> = match &self.cut.status {
            CutStatus::NoRush => {
                Some((tr!("cut.no_rush", key = crate::shortcut("I")), tokens::MUTE))
            }
            CutStatus::Transcribing => Some((tr!("cut.transcribing"), tokens::FG2)),
            CutStatus::Failed(e) => Some((e.clone(), tokens::ERROR)),
            CutStatus::Ready
                if self
                    .cut
                    .analysis
                    .as_ref()
                    .is_some_and(|a| a.sentences.is_empty()) =>
            {
                Some((tr!("cut.no_sentence"), tokens::MUTE))
            }
            CutStatus::Ready if self.cut.derived() => Some((tr!("cut.derived"), tokens::MUTE)),
            CutStatus::Ready => None,
        };

        // Shared reference: the row closures only create listeners.
        let listeners: &Context<Self> = cx;
        let rows: Vec<Div> = self
            .cut
            .analysis
            .as_ref()
            .map(|analysis| {
                analysis
                    .sentences
                    .iter()
                    .enumerate()
                    .map(|(s, sentence)| {
                        let chosen = self.cut.choice.get(s).copied().flatten();
                        let takes: Vec<(usize, bool)> =
                            analysis.takes_of(s).map(|(i, t)| (i, t.complete)).collect();
                        let missing = takes.is_empty();
                        let chips = takes
                            .into_iter()
                            .enumerate()
                            .map(move |(n, (i, complete))| {
                                let selected = chosen == Some(i);
                                div()
                                    .id(("take", i))
                                    .flex_none()
                                    .px(px(7.))
                                    .h(px(22.))
                                    .flex()
                                    .items_center()
                                    .rounded(px(3.))
                                    .cursor_pointer()
                                    .font_family(crate::fonts::MONO)
                                    .text_size(px(11.))
                                    .map(|d| {
                                        if selected {
                                            d.bg(color(tokens::SIGNAL))
                                                .text_color(color(tokens::ON_SIGNAL))
                                        } else {
                                            d.border_1()
                                                .border_color(color(tokens::LINE_STRONG))
                                                .text_color(color(if complete {
                                                    tokens::FG2
                                                } else {
                                                    tokens::MUTE
                                                }))
                                                .hover(|d| d.bg(color(tokens::HOVER)))
                                        }
                                    })
                                    .child(SharedString::from(format!(
                                        "{}{}",
                                        tr!("cut.take_chip", n = n.saturating_add(1)),
                                        if complete {
                                            SharedString::default()
                                        } else {
                                            tr!("cut.incomplete")
                                        }
                                    )))
                                    .on_click(listeners.listener(
                                        move |shell, _: &ClickEvent, _, cx| {
                                            shell.choose_take(s, i, cx);
                                        },
                                    ))
                            });
                        let active = current == Some(s);
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .px(px(20.))
                            .py(px(8.))
                            .border_b_1()
                            .border_color(color(tokens::LINE))
                            .when(active, |d| d.bg(color(tokens::RAISED)))
                            .child(
                                mono(format!("{:02}", s.saturating_add(1)))
                                    .flex_none()
                                    .w(px(24.))
                                    .text_size(px(12.))
                                    .text_color(color(if active {
                                        tokens::SIGNAL
                                    } else {
                                        tokens::MUTE
                                    })),
                            )
                            .child(
                                div()
                                    .id(("sentence", s))
                                    .flex_1()
                                    .min_w(px(0.))
                                    .when(!missing, |d| {
                                        d.cursor_pointer().on_click(listeners.listener(
                                            move |shell, _: &ClickEvent, _, cx| {
                                                shell.seek_sentence(s, cx);
                                            },
                                        ))
                                    })
                                    .text_color(color(if missing {
                                        tokens::MUTE
                                    } else {
                                        tokens::FG
                                    }))
                                    .child(SharedString::from(sentence.text.clone())),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .gap(px(4.))
                                    .when(missing, |d| {
                                        d.text_size(px(11.))
                                            .text_color(color(tokens::ERROR))
                                            .child(tr!("cut.not_found"))
                                    })
                                    .children(chips),
                            )
                    })
                    .collect()
            })
            .unwrap_or_default();

        div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .h(relative(0.5))
                    .border_b_1()
                    .border_color(color(tokens::LINE))
                    .child(self.player.clone()),
            )
            .child(
                toolbar(tr!("cut.title"), detail)
                    .child(button("montage", tr!("cut.montage"), montage).on_click(
                        cx.listener(|shell, _: &ClickEvent, _, cx| shell.set_montage(true, cx)),
                    ))
                    .child(button("rush", tr!("cut.whole_take"), !montage).on_click(
                        cx.listener(|shell, _: &ClickEvent, _, cx| shell.set_montage(false, cx)),
                    ))
                    .child(button("reset", tr!("cut.last_takes"), false).on_click(
                        cx.listener(|shell, _: &ClickEvent, _, cx| shell.reset_choices(cx)),
                    ))
                    .when(self.cut.derived(), |d| {
                        d.child(
                            button("derive-script", tr!("cut.write_script"), false).on_click(
                                cx.listener(|shell, _: &ClickEvent, window, cx| {
                                    shell.script_from_take(window, cx);
                                }),
                            ),
                        )
                    }),
            )
            .when_some(notice, |d, (text, tone)| {
                d.child(
                    div()
                        .px(px(20.))
                        .py(px(10.))
                        .text_color(color(tone))
                        .child(text),
                )
            })
            .child(
                div()
                    .id("sentences")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .children(rows),
            )
    }
}
