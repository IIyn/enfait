//! Local model picker: the models of the catalogue sized for this
//! machine's graphics card. A click downloads the model if needed, unloads
//! the other models enfait loaded, loads it and makes it the local model.
//! Shown where the care level is chosen (settings, montage dialog, new
//! project) once "Local" is the provider.

use super::Shell;
use super::studio::chip;
use crate::prefs;
use crate::theme::{Token, color, tokens};
use crate::tr;
use enfait_ai::ProviderChoice;
use enfait_ai::connect::Server;
use enfait_ai::ollama::{self, CATALOG, CatalogModel, Fit, Gpu};
use gpui_kit::prelude::*;
use gpui_kit::{ClickEvent, Context, Div, FontWeight, SharedString, Stateful, div, px};
use std::cell::Cell;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Time between two looks at Ollama while the picker is on screen.
const REFRESH: Duration = Duration::from_secs(6);

/// Where the picker is drawn (ids, and whether choosing sets the provider
/// of the open project).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PickerAt {
    Settings,
    Care,
    /// New project form: the provider is the form's, not the open project's.
    Home,
}

impl PickerAt {
    const fn id(self) -> &'static str {
        match self {
            Self::Settings => "local-settings",
            Self::Care => "local-care",
            Self::Home => "local-home",
        }
    }
}

/// A model being downloaded and loaded.
struct Op {
    model: String,
    /// Bytes downloaded, of the total (0 of 0: not downloading).
    progress: Arc<Mutex<(u64, u64)>>,
    failed: Option<SharedString>,
}

#[derive(Default)]
pub(super) struct LocalModels {
    /// `None`: not looked for yet.
    gpu: Option<Option<Gpu>>,
    /// Models in Ollama; `Err("unreachable")`: Ollama not running.
    installed: Option<Result<Vec<String>, String>>,
    probed: Option<Instant>,
    probing: bool,
    /// Last time the picker was drawn (the probes run only meanwhile).
    shown: Cell<Option<Instant>>,
    op: Option<Op>,
}

/// Ollama address: the one in the settings if it points to Ollama, else
/// Ollama's default.
fn ollama_url(cx: &Context<Shell>) -> String {
    let url = prefs::settings(cx).local_url();
    if Server::from_url(&url) == Server::Ollama {
        url
    } else {
        enfait_config::DEFAULT_LOCAL_URL.to_owned()
    }
}

/// Bytes as gigabytes, one decimal.
fn gigabytes(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1_073_741_824.0)
}

fn note(text: impl Into<SharedString>, tone: Token) -> Div {
    div()
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(color(tone))
        .child(text.into())
}

fn tag(text: impl Into<SharedString>, tone: Token) -> Div {
    div()
        .flex_none()
        .px(px(6.))
        .rounded(px(3.))
        .border_1()
        .border_color(color(tone))
        .text_color(color(tone))
        .text_size(px(11.))
        .child(text.into())
}

impl Shell {
    /// Looks at the card and at Ollama while the picker is on screen;
    /// redraws while a model is being downloaded (polled with the engine).
    pub(super) fn poll_local_models(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.local_models;
        if state.op.as_ref().is_some_and(|op| op.failed.is_none()) {
            cx.notify();
        }
        let shown = state
            .shown
            .get()
            .is_some_and(|t| t.elapsed() < Duration::from_secs(2));
        let stale = state.probed.is_none_or(|t| t.elapsed() > REFRESH);
        if !shown || !stale || state.probing {
            return;
        }
        state.probing = true;
        let need_gpu = state.gpu.is_none();
        let root = ollama::root(&ollama_url(cx));
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move {
                    let gpu = need_gpu.then(ollama::detect_gpu);
                    let installed =
                        super::assets::block_on(ollama::installed(&root)).unwrap_or_else(Err);
                    (gpu, installed)
                })
                .await;
            let _ = this.update(cx, |shell, cx| {
                let state = &mut shell.local_models;
                let (gpu, installed) = found;
                if let Some(gpu) = gpu {
                    state.gpu = Some(gpu);
                }
                state.installed = Some(installed);
                state.probed = Some(Instant::now());
                state.probing = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// Downloads if needed, unloads enfait's other models, loads `model`
    /// and makes it the local model.
    fn use_local_model(&mut self, model: &'static str, at: PickerAt, cx: &mut Context<Self>) {
        if self
            .local_models
            .op
            .as_ref()
            .is_some_and(|op| op.failed.is_none())
        {
            return;
        }
        let progress = Arc::new(Mutex::new((0, 0)));
        self.local_models.op = Some(Op {
            model: model.to_owned(),
            progress: Arc::clone(&progress),
            failed: None,
        });
        let url = ollama_url(cx);
        let root = ollama::root(&url);
        let ours = prefs::settings(cx).local_loaded.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let report = move |done: u64, total: u64| {
                        if let Ok(mut p) = progress.lock() {
                            *p = (done, total);
                        }
                    };
                    super::assets::block_on(ollama::switch_to(&root, model, &ours, &report))
                        .unwrap_or_else(Err)
                })
                .await;
            let _ = this.update(cx, |shell, cx| {
                match result {
                    Ok(()) => {
                        shell.local_models.op = None;
                        prefs::update(cx, |s| {
                            s.local_url = url;
                            s.local_model = model.to_owned();
                            if !s.local_loaded.iter().any(|m| ollama::same_model(m, model)) {
                                s.local_loaded.push(model.to_owned());
                            }
                        });
                        shell.settings.sync = true;
                        if at != PickerAt::Home {
                            shell.set_provider(ProviderChoice::Local, cx);
                        }
                    }
                    Err(e) => {
                        let message = if e == "unreachable" {
                            tr!("local.unreachable", url = url)
                        } else {
                            tr!("local.failed", error = e)
                        };
                        if let Some(op) = &mut shell.local_models.op {
                            op.failed = Some(message);
                        }
                    }
                }
                shell.local_models.probed = None;
                cx.notify();
            });
        })
        .detach();
    }

    /// The picker: card found, catalogue, reasoning switch.
    pub(super) fn local_picker(&self, at: PickerAt, cx: &mut Context<Self>) -> Div {
        let state = &self.local_models;
        state.shown.set(Some(Instant::now()));
        let settings = prefs::settings(cx);
        let current = settings.local_model();
        let thinking = settings.local_thinking;
        let gpu = state.gpu.clone().flatten();
        let card = match &state.gpu {
            None => note(tr!("local.detecting"), tokens::MUTE),
            Some(None) => note(tr!("local.no_gpu"), tokens::MIXED),
            Some(Some(gpu)) => note(
                if gpu.unified {
                    tr!("local.unified", size = gigabytes(gpu.memory))
                } else {
                    tr!(
                        "local.gpu",
                        name = gpu.name.clone(),
                        size = gigabytes(gpu.memory)
                    )
                },
                tokens::FG2,
            ),
        };
        let unreachable = matches!(&state.installed, Some(Err(e)) if e == "unreachable");
        let installed: &[String] = match &state.installed {
            Some(Ok(list)) => list,
            _ => &[],
        };
        let recommended = ollama::recommended(gpu.as_ref());
        let rows = CATALOG
            .iter()
            .enumerate()
            .map(|(i, &model)| {
                let chosen = current
                    .as_deref()
                    .is_some_and(|c| ollama::same_model(c, model.id));
                let is_installed = installed.iter().any(|m| ollama::same_model(m, model.id));
                let op = state.op.as_ref().filter(|op| op.model == model.id);
                self.model_row(
                    at,
                    i,
                    model,
                    RowState {
                        fit: model.fit(gpu.as_ref()),
                        chosen,
                        recommended: model.id == recommended.id,
                        installed: is_installed,
                        op,
                    },
                    cx,
                )
            })
            .collect::<Vec<_>>();
        let url = ollama_url(cx);
        let nothing_fits = gpu.is_some() && recommended.fit(gpu.as_ref()) != Fit::Full;
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(card)
            .when(nothing_fits, |d| {
                d.child(note(tr!("local.nothing_fits"), tokens::MIXED))
            })
            .when(unreachable, |d| {
                d.child(note(tr!("local.unreachable", url = url), tokens::ERROR))
            })
            .child(div().flex().flex_col().gap(px(4.)).children(rows))
            .child(div().flex().items_center().gap(px(8.)).pt(px(4.)).child(
                chip((at.id(), 100_usize), tr!("local.thinking"), thinking, true).on_click(
                    cx.listener(move |_, _: &ClickEvent, _, cx| {
                        prefs::update(cx, |s| s.local_thinking = !thinking);
                    }),
                ),
            ))
            .child(note(
                tr!("local.thinking_help"),
                if thinking {
                    tokens::MIXED
                } else {
                    tokens::MUTE
                },
            ))
    }

    fn model_row(
        &self,
        at: PickerAt,
        i: usize,
        model: CatalogModel,
        row: RowState<'_>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let busy = row.op.is_some_and(|op| op.failed.is_none());
        let status: Option<(SharedString, Token)> = match row.op {
            Some(Op {
                failed: Some(e), ..
            }) => Some((e.clone(), tokens::ERROR)),
            Some(op) => {
                let (done, total) = op.progress.lock().map_or((0, 0), |p| *p);
                Some((
                    if total > 0 && done < total {
                        tr!(
                            "local.downloading",
                            pct = (done.saturating_mul(100) / total).to_string(),
                            size = gigabytes(total)
                        )
                    } else {
                        tr!("local.starting")
                    },
                    tokens::SIGNAL,
                ))
            }
            None => None,
        };
        let (fit_text, fit_tone) = match row.fit {
            Fit::Full => (Some(tr!("local.fits")), tokens::OK),
            Fit::Partial => (Some(tr!("local.partial")), tokens::MIXED),
            Fit::Unknown => (None, tokens::MUTE),
        };
        div()
            .id((at.id(), i))
            .flex()
            .flex_col()
            .gap(px(2.))
            .px(px(10.))
            .py(px(6.))
            .rounded(px(3.))
            .border_1()
            .border_color(color(if row.chosen {
                tokens::SIGNAL
            } else {
                tokens::LINE
            }))
            .when(row.chosen, |d| d.bg(color(tokens::RAISED)))
            .when(!busy, |d| {
                d.cursor_pointer()
                    .hover(|d| d.bg(color(tokens::HOVER)))
                    .on_click(cx.listener(move |shell, _: &ClickEvent, _, cx| {
                        shell.use_local_model(model.id, at, cx);
                    }))
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(px(13.))
                            .child(model.id),
                    )
                    .when(row.recommended, |d| {
                        d.child(tag(tr!("local.recommended"), tokens::SIGNAL))
                    })
                    .when(row.installed, |d| {
                        d.child(tag(tr!("local.installed"), tokens::MUTE))
                    })
                    .when(row.chosen, |d| {
                        d.child(tag(tr!("local.chosen"), tokens::OK))
                    })
                    .when(model.thinking, |d| {
                        d.child(tag(tr!("local.can_think"), tokens::MUTE))
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .child(note(
                        tr!(
                            "local.sizes",
                            download = gigabytes(model.bytes),
                            need = gigabytes(model.needed())
                        ),
                        tokens::FG2,
                    ))
                    .when_some(fit_text, |d, text| d.child(note(text, fit_tone))),
            )
            .when_some(status, |d, (text, tone)| d.child(note(text, tone)))
    }
}

struct RowState<'a> {
    fit: Fit,
    chosen: bool,
    recommended: bool,
    installed: bool,
    op: Option<&'a Op>,
}
