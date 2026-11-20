//! Voice (script, prompter, take import) and Cut (takes and edit) screens.

use super::{Shell, mono};
use crate::cut::{CutStatus, clock};
use crate::session::Start;
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
pub(super) fn button(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    primary: bool,
) -> Stateful<Div> {
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
    /// Script: the text to read, its length, the prompter.
    pub(super) fn script_screen(&mut self, cx: &mut Context<Self>) -> Div {
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
        let record = self.session.data.start == Start::Script;
        div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .child(
                toolbar(tr!("steps.script"), detail)
                    .child(
                        button("prompter", tr!("voice.prompter"), false).on_click(cx.listener(
                            |shell, _: &ClickEvent, window: &mut Window, cx| {
                                shell.open_prompter(window, cx);
                            },
                        )),
                    )
                    .when(record, |d| {
                        d.child(button("to-voice", tr!("voice.go_record"), true).on_click(
                            cx.listener(|shell, _: &ClickEvent, _, cx| {
                                shell.go_to(super::STEP_VOICE, cx);
                            }),
                        ))
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .p(px(16.))
                    .child(Textarea::new(&self.script).h_full()),
            )
    }

    /// Takes recorded in enfait, newest first.
    fn recordings(&self) -> Vec<std::path::PathBuf> {
        let mut list: Vec<std::path::PathBuf> = std::fs::read_dir(self.session.recordings_dir())
            .map(|dir| {
                dir.filter_map(Result::ok)
                    .map(|e| e.path())
                    // Takes only: not the `.voice.wav` and `.camera.mkv` of a
                    // filmed take being written.
                    .filter(|p| {
                        p.extension().is_some_and(|e| e == "wav" || e == "mp4")
                            && p.file_stem()
                                .and_then(|s| s.to_str())
                                .is_some_and(|s| !s.contains('.'))
                    })
                    .collect()
            })
            .unwrap_or_default();
        list.sort();
        list.reverse();
        list
    }

    /// Voice: record it here (with the prompter) or import a voice or a
    /// video; whether the take is cut automatically.
    #[allow(clippy::too_many_lines)]
    pub(super) fn voice_screen(&mut self, cx: &mut Context<Self>) -> Div {
        let data = &self.session.data;
        let record = data.start == Start::Script;
        let derush = data.derush;
        let rush = data.rush.clone();
        let rush_name = rush
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned());
        let recordings = if record {
            self.recordings()
        } else {
            Vec::new()
        };
        let next = if derush {
            (super::STEP_CUT, tr!("voice.see_cut"))
        } else {
            (super::STEP_STORYBOARD, tr!("voice.to_storyboard"))
        };
        let devices = record.then(|| self.devices_panel(cx));
        let filming = crate::prefs::settings(cx).record_camera;
        let busy = self.recording_busy;
        let record_label = match busy {
            Some(super::record::Busy::StartingCamera) => tr!("voice.camera_starting"),
            Some(super::record::Busy::Saving) => tr!("voice.saving_take"),
            None if filming => tr!("voice.record_camera"),
            None => tr!("voice.record"),
        };
        let listeners: &Context<Self> = cx;
        let no_takes = recordings.is_empty();
        let takes = recordings.into_iter().enumerate().map(|(i, path)| {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let current = rush.as_deref() == Some(path.as_path());
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(12.))
                .h(px(ROW_HEIGHT))
                .border_b_1()
                .border_color(color(tokens::LINE))
                .child(mono(name).flex_1().text_size(px(12.)))
                .child(if current {
                    div()
                        .text_size(px(11.))
                        .text_color(color(tokens::SIGNAL))
                        .child(tr!("voice.current_take"))
                        .into_any_element()
                } else {
                    super::studio::chip(("use-take", i), tr!("voice.use_take"), false, true)
                        .on_click(listeners.listener(move |shell, _: &ClickEvent, _, cx| {
                            shell.import_rush(path.clone(), cx);
                        }))
                        .into_any_element()
                })
        });
        let derush_chips = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(super::label(tr!("voice.cut_label")).mr(px(4.)))
            .child(
                super::studio::chip("derush-on", tr!("home.derush_on"), derush, true).on_click(
                    cx.listener(|shell, _: &ClickEvent, _, cx| shell.set_derush(true, cx)),
                ),
            )
            .child(
                super::studio::chip("derush-off", tr!("home.derush_off"), !derush, true).on_click(
                    cx.listener(|shell, _: &ClickEvent, _, cx| shell.set_derush(false, cx)),
                ),
            );
        div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .child(
                toolbar(
                    if record {
                        tr!("voice.title_record")
                    } else {
                        tr!("voice.title_import")
                    },
                    String::new(),
                )
                .child(
                    button("import", tr!("voice.import"), !record).on_click(
                        cx.listener(|shell, _: &ClickEvent, _, cx| shell.prompt_import(cx)),
                    ),
                )
                .when(!record, |d| {
                    d.child(button("magic", tr!("voice.magic"), false).on_click(
                        cx.listener(|shell, _: &ClickEvent, _, cx| shell.start_magic(cx)),
                    ))
                }),
            )
            .when_some(rush_name, |d, name| {
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
                                .id("to-next")
                                .cursor_pointer()
                                .text_color(color(tokens::SIGNAL))
                                .child(next.1)
                                .on_click(cx.listener(move |shell, _: &ClickEvent, _, cx| {
                                    shell.go_to(next.0, cx);
                                })),
                        ),
                )
            })
            .child(
                div()
                    .id("voice-body")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .p(px(20.))
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .children(devices)
                    .when(record, |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(14.))
                                .child(
                                    div()
                                        .id("record")
                                        .flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .h(px(40.))
                                        .px(px(18.))
                                        .rounded(px(3.))
                                        .bg(color(tokens::SIGNAL))
                                        .text_color(color(tokens::ON_SIGNAL))
                                        .when(busy.is_none(), |d| {
                                            d.cursor_pointer()
                                                .hover(|d| d.bg(color(tokens::SIGNAL_HOVER)))
                                        })
                                        .when(busy.is_some(), |d| d.opacity(0.6))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(
                                            div()
                                                .size(px(10.))
                                                .rounded(px(5.))
                                                .bg(color(tokens::ERROR)),
                                        )
                                        .child(record_label)
                                        .on_click(cx.listener(
                                            |shell, _: &ClickEvent, window, cx| {
                                                shell.start_recording(window, cx);
                                            },
                                        )),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .text_size(px(12.))
                                        .text_color(color(tokens::FG2))
                                        .child(tr!("voice.record_help")),
                                ),
                        )
                        .when(self.session.script.trim().is_empty(), |d| {
                            d.child(
                                div()
                                    .id("write-first")
                                    .cursor_pointer()
                                    .text_size(px(12.))
                                    .text_color(color(tokens::MIXED))
                                    .child(tr!("voice.script_empty"))
                                    .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| {
                                        shell.go_to(super::STEP_SCRIPT, cx);
                                    })),
                            )
                        })
                        .child(super::label(tr!("voice.takes")))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .border_1()
                                .border_color(color(tokens::LINE))
                                .rounded(px(3.))
                                .when(no_takes, |d| {
                                    d.child(
                                        div()
                                            .px(px(12.))
                                            .py(px(8.))
                                            .text_size(px(12.))
                                            .text_color(color(tokens::MUTE))
                                            .child(tr!("voice.no_takes")),
                                    )
                                })
                                .children(takes),
                        )
                    })
                    .when(!record, |d| {
                        d.child(
                            div()
                                .id("voice-drop")
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .gap(px(8.))
                                .h(px(160.))
                                .rounded(px(4.))
                                .border_1()
                                .border_dashed()
                                .border_color(color(tokens::LINE_STRONG))
                                .text_color(color(tokens::FG2))
                                .child(tr!("voice.drop_here"))
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(color(tokens::MUTE))
                                        .child(tr!("voice.drop_help")),
                                ),
                        )
                    })
                    .child(derush_chips)
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(color(tokens::MUTE))
                            .child(if derush {
                                tr!("home.derush_on_text")
                            } else {
                                tr!("home.derush_off_text")
                            }),
                    ),
            )
    }

    /// Turns the automatic cut on or off.
    fn set_derush(&mut self, derush: bool, cx: &mut Context<Self>) {
        if self.session.data.derush == derush {
            return;
        }
        self.session.data.derush = derush;
        self.session.save_data();
        self.apply_cut(cx);
        cx.notify();
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
