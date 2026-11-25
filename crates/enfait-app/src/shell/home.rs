//! Home screen: the recent projects and the creation of a new one.
//!
//! A new project is either custom (optional title, 9:16 or 16:9 format,
//! then what it starts from: a script to write and record in enfait, a
//! voice or video recorded elsewhere with or without automatic cut, or no
//! voice at all for an animation of a chosen duration) or magic (a voice or
//! a take, and everything is automatic up to the export).

use super::screens::button;
use super::studio::chip;
use super::{STEP_SCRIPT, STEP_STORYBOARD, STEP_VOICE, Shell, label, mono};
use crate::session::{ProjectMode, SessionData, Start};
use crate::theme::{color, tokens};
use crate::tr;
use enfait_ai::ProviderChoice;
use enfait_config::projects::{ProjectEntry, Projects};
use enfait_core::AspectRatio;
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::prelude::*;
use gpui_kit::{
    ClickEvent, Context, Div, FontWeight, PathPromptOptions, SharedString, Stateful, Window, div,
    px,
};
use std::path::{Path, PathBuf};

/// Longest silent video offered, in seconds.
const MAX_SILENT_SECONDS: f64 = 600.0;

/// What the home screen shows of a project.
pub(super) struct Card {
    dir: PathBuf,
    title: String,
    aspect: Option<AspectRatio>,
    mode: ProjectMode,
    start: Start,
    blocks: usize,
    seconds: f64,
    opened: u64,
}

/// The new project form.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Wizard {
    /// Not chosen yet: the first screen offers both.
    mode: Option<ProjectMode>,
    aspect: AspectRatio,
    start: Start,
    derush: bool,
    /// Model provider, chosen by hand: nothing is picked for the user.
    provider: Option<ProviderChoice>,
    /// Voice or take to import (custom "recorded" and magic).
    file: Option<PathBuf>,
    error: Option<SharedString>,
}

impl Default for Wizard {
    fn default() -> Self {
        Self {
            mode: None,
            aspect: AspectRatio::Portrait9x16,
            start: Start::Script,
            derush: true,
            provider: None,
            file: None,
            error: None,
        }
    }
}

pub(super) struct Home {
    cards: Vec<Card>,
    pub wizard: Option<Wizard>,
    /// Project whose deletion awaits confirmation.
    confirm_delete: Option<PathBuf>,
}

impl Home {
    pub fn new(projects: &Projects) -> Self {
        let cards: Vec<Card> = projects.recent().iter().map(card).collect();
        Self {
            // No project yet: straight to the form.
            wizard: cards.is_empty().then(Wizard::default),
            cards,
            confirm_delete: None,
        }
    }

    /// Escape: one level back in the form.
    pub fn back(&mut self) {
        match &mut self.wizard {
            Some(w) if w.mode.is_some() => w.mode = None,
            Some(_) if !self.cards.is_empty() => self.wizard = None,
            _ => self.confirm_delete = None,
        }
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Summary of a project folder.
fn card(entry: &ProjectEntry) -> Card {
    let data: SessionData = read_json(&entry.dir.join("session.json")).unwrap_or_default();
    let project = std::fs::read_to_string(entry.dir.join("project.json"))
        .ok()
        .and_then(|j| enfait_core::Project::from_json(&j).ok());
    let heading = || {
        std::fs::read_to_string(entry.dir.join("script.md"))
            .ok()?
            .lines()
            .find_map(|l| l.trim().strip_prefix("# ").map(|t| t.trim().to_owned()))
            .filter(|t| !t.is_empty())
    };
    let title = [entry.title.trim(), data.title.trim()]
        .into_iter()
        .find(|t| !t.is_empty())
        .map(str::to_owned)
        .or_else(heading)
        .or_else(|| {
            data.rush
                .as_ref()
                .filter(|_| data.start != Start::Silent)
                .and_then(|p| p.file_stem())
                .map(|s| s.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    Card {
        dir: entry.dir.clone(),
        title,
        aspect: project
            .as_ref()
            .map(|p| p.settings().aspect)
            .or(data.aspect),
        mode: data.mode,
        start: data.start,
        blocks: project.as_ref().map_or(0, |p| p.blocks().len()),
        seconds: project
            .as_ref()
            .map_or(data.duration, |p| p.duration().as_secs_f64()),
        opened: entry.opened,
    }
}

/// "3 h ago", "2 days ago"…
fn ago(stamp: u64) -> SharedString {
    let s = enfait_config::projects::now().saturating_sub(stamp);
    match s {
        0..60 => tr!("home.ago.now"),
        60..3600 => tr!("home.ago.minutes", n = s / 60),
        3600..86_400 => tr!("home.ago.hours", n = s / 3600),
        _ => tr!("home.ago.days", n = s / 86_400),
    }
}

fn format_tag(aspect: AspectRatio) -> &'static str {
    match aspect {
        AspectRatio::Landscape16x9 => "16:9",
        AspectRatio::Portrait9x16 => "9:16",
        AspectRatio::Square1x1 => "1:1",
    }
}

fn start_name(mode: ProjectMode, start: Start) -> SharedString {
    match (mode, start) {
        (ProjectMode::Magic, _) => tr!("home.kind.magic"),
        (_, Start::Script) => tr!("home.kind.script"),
        (_, Start::Import) => tr!("home.kind.import"),
        (_, Start::Silent) => tr!("home.kind.silent"),
    }
}

/// A large choice: title and explanation; `on`: chosen.
fn option(id: &'static str, title: SharedString, text: SharedString, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .gap(px(10.))
        .p(px(14.))
        .rounded(px(4.))
        .cursor_pointer()
        .border_1()
        .border_color(color(if on {
            tokens::SIGNAL
        } else {
            tokens::LINE_STRONG
        }))
        .bg(color(if on { tokens::RAISED } else { tokens::PANEL }))
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
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().font_weight(FontWeight::MEDIUM).child(title))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(color(tokens::FG2))
                        .child(text),
                ),
        )
}

impl Shell {
    fn wizard_mut(&mut self) -> Option<&mut Wizard> {
        self.home.as_mut().and_then(|h| h.wizard.as_mut())
    }

    fn edit_wizard(&mut self, change: impl FnOnce(&mut Wizard), cx: &mut Context<Self>) {
        if let Some(w) = self.wizard_mut() {
            change(w);
            w.error = None;
        }
        cx.notify();
    }

    /// A file dropped on the home screen: it opens the form for it.
    pub(super) fn home_dropped(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(home) = &mut self.home else { return };
        let wizard = home.wizard.get_or_insert_with(Wizard::default);
        if wizard.mode.is_none() {
            wizard.mode = Some(ProjectMode::Custom);
        }
        if wizard.mode == Some(ProjectMode::Custom) {
            wizard.start = Start::Import;
        }
        wizard.file = Some(path);
        wizard.error = None;
        cx.notify();
    }

    fn choose_wizard_file(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(tr!("voice.import_prompt")),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |shell, cx| {
                    shell.edit_wizard(|w| w.file = Some(path), cx);
                });
            }
        })
        .detach();
    }

    /// `ENFAIT_OPEN=new/custom[/import|silent]`, `new/magic` or
    /// `step/<name>` opens a screen directly (screenshots).
    pub(super) fn open_screen_from_env(&mut self, cx: &mut Context<Self>) {
        let Ok(target) = std::env::var("ENFAIT_OPEN") else {
            return;
        };
        let parts: Vec<&str> = target.split('/').collect();
        match parts.as_slice() {
            ["new", mode, rest @ ..] => {
                let home = self.home.get_or_insert_with(|| Home::new(&self.projects));
                let mut wizard = Wizard {
                    mode: Some(if *mode == "magic" {
                        ProjectMode::Magic
                    } else {
                        ProjectMode::Custom
                    }),
                    ..Wizard::default()
                };
                match rest.first() {
                    Some(&"import") => wizard.start = Start::Import,
                    Some(&"silent") => wizard.start = Start::Silent,
                    _ => {}
                }
                home.wizard = Some(wizard);
            }
            ["step", name, ..] => {
                let step = match *name {
                    "script" => STEP_SCRIPT,
                    "voice" => STEP_VOICE,
                    "cut" => super::STEP_CUT,
                    "montage" => super::STEP_MONTAGE,
                    "retouch" => super::STEP_RETOUCH,
                    "finish" => super::STEP_FINISH,
                    _ => STEP_STORYBOARD,
                };
                self.home = None;
                self.go_to(step, cx);
                if parts.get(2) == Some(&"care") {
                    if let Some(board) = &mut self.board {
                        board.mark_all();
                    }
                    self.open_care(cx);
                }
            }
            _ => {}
        }
    }

    /// `enfait <file>`: a new project on that take.
    pub(super) fn create_from_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let title = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let wizard = Wizard {
            mode: Some(ProjectMode::Custom),
            start: Start::Import,
            file: Some(path),
            ..Wizard::default()
        };
        if let Err(e) = self.create_project(&title, &wizard, 0.0, String::new(), cx) {
            tracing::error!("cannot create the project: {e}");
        }
    }

    /// Creates the project of the form and opens it.
    fn submit_wizard(&mut self, cx: &mut Context<Self>) {
        let Some(wizard) = self.home.as_ref().and_then(|h| h.wizard.clone()) else {
            return;
        };
        let title = self.new_title.read(cx).value().trim().to_owned();
        let duration_text = self.new_duration.read(cx).value().replace(',', ".");
        let brief = self.new_brief.read(cx).value().trim().to_owned();
        let magic = wizard.mode == Some(ProjectMode::Magic);
        let error = if (magic || (wizard.start == Start::Import && wizard.file.is_some()))
            && wizard.file.as_ref().is_some_and(|f| !f.is_file())
        {
            Some(tr!("home.error.file_missing"))
        } else if magic && wizard.file.is_none() {
            Some(tr!("home.error.need_file"))
        } else if wizard.provider.is_none() {
            Some(tr!("home.error.need_model"))
        } else {
            None
        };
        let duration = duration_text.trim().parse::<f64>().unwrap_or(0.0);
        let error = error.or_else(|| {
            (wizard.start == Start::Silent
                && !magic
                && !(1.0..=MAX_SILENT_SECONDS).contains(&duration))
            .then(|| tr!("home.error.duration", max = MAX_SILENT_SECONDS))
        });
        if let Some(error) = error {
            self.edit_wizard(|w| w.error = Some(error), cx);
            return;
        }
        if let Err(e) = self.create_project(&title, &wizard, duration, brief, cx) {
            self.edit_wizard(
                |w| w.error = Some(tr!("home.error.create", error = e.to_string())),
                cx,
            );
        }
    }

    fn create_project(
        &mut self,
        title: &str,
        wizard: &Wizard,
        duration: f64,
        brief: String,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let dir = self.projects.create(title)?;
        self.projects.save()?;
        let magic = wizard.mode == Some(ProjectMode::Magic);
        let start = if magic { Start::Import } else { wizard.start };
        let data = SessionData {
            title: title.to_owned(),
            mode: wizard.mode.unwrap_or_default(),
            start,
            derush: magic || wizard.derush,
            aspect: Some(wizard.aspect),
            provider: wizard.provider.unwrap_or_default(),
            duration: if start == Start::Silent {
                duration
            } else {
                0.0
            },
            brief: if start == Start::Silent {
                brief
            } else {
                String::new()
            },
            ..SessionData::default()
        };
        // The folder is written before it is opened.
        let mut session = crate::session::Session::load(&dir);
        session.data = data;
        if start == Start::Silent {
            let silence = dir.join("silence.wav");
            crate::session::write_silence(&silence, duration)?;
            session.data.rush = Some(silence);
        }
        session.save_data();
        self.open_project(dir, cx);
        // A fresh form for the next time.
        self.reset_wizard_inputs = true;
        match (start, &wizard.file) {
            (Start::Silent, _) => self.go_to(STEP_STORYBOARD, cx),
            (_, Some(file)) => {
                self.magic_after_import = magic;
                self.import_rush(file.clone(), cx);
            }
            (Start::Script, None) => self.go_to(STEP_SCRIPT, cx),
            (_, None) => self.go_to(STEP_VOICE, cx),
        }
        Ok(())
    }

    fn delete_project(&mut self, dir: &Path, cx: &mut Context<Self>) {
        if self.session.dir() == dir {
            // Leave it first: its engine must not write into it again.
            let next = self
                .projects
                .recent()
                .into_iter()
                .map(|p| p.dir)
                .find(|d| d != dir)
                .unwrap_or_else(enfait_config::paths::default_session);
            self.open_project_quietly(next, cx);
        }
        if let Err(e) = self.projects.remove(dir) {
            self.toast(tr!("home.error.delete", error = e.to_string()));
        }
        if let Err(e) = self.projects.save() {
            tracing::warn!("projects: {e}");
        }
        self.home = Some(Home::new(&self.projects));
        cx.notify();
    }

    /// Loads another project without leaving the home screen (nor
    /// registering it, nor making it current).
    fn open_project_quietly(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        let home = self.home.take();
        let before = self.projects.clone();
        self.open_project(dir, cx);
        self.projects = before;
        let _ = self.projects.save();
        self.home = home;
    }

    pub(super) fn home_screen(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let content = match self.home.as_ref().and_then(|h| h.wizard.clone()) {
            Some(wizard) => self.wizard_view(&wizard, cx),
            None => self.projects_view(cx),
        };
        div()
            .id("home-scroll")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .justify_center()
                    .px(px(16.))
                    .py(px(32.))
                    .child(div().w_full().max_w(px(760.)).child(content)),
            )
    }

    fn projects_view(&self, cx: &mut Context<Self>) -> Div {
        let Some(home) = &self.home else {
            return div();
        };
        let current = self.session.dir().to_path_buf();
        let confirm = home.confirm_delete.clone();
        let cards = home.cards.iter().enumerate().map(|(i, card)| {
            let dir = card.dir.clone();
            let open_dir = dir.clone();
            let asking = confirm.as_deref() == Some(dir.as_path());
            let (running, queued) = if card.dir == current {
                self.queue.lock().map(|q| q.counts()).unwrap_or((0, 0))
            } else {
                (0, 0)
            };
            let title: SharedString = if card.title.is_empty() {
                tr!("common.untitled")
            } else {
                card.title.clone().into()
            };
            let details = {
                let mut parts: Vec<String> = vec![start_name(card.mode, card.start).to_string()];
                if card.blocks > 0 {
                    parts.push(tr!("home.blocks", count = card.blocks).to_string());
                }
                if card.seconds > 0.0 {
                    parts.push(crate::cut::clock(card.seconds));
                }
                parts.join(" · ")
            };
            div()
                .id(("project", i))
                .flex()
                .items_center()
                .gap(px(14.))
                .px(px(14.))
                .py(px(12.))
                .rounded(px(4.))
                .bg(color(tokens::PANEL))
                .border_1()
                .border_color(color(tokens::LINE))
                .cursor_pointer()
                .hover(|d| d.border_color(color(tokens::LINE_STRONG)))
                .on_click(cx.listener(move |shell, _: &ClickEvent, _, cx| {
                    if shell.session.dir() == open_dir {
                        shell.home = None;
                        shell.projects.touch(&open_dir);
                        let _ = shell.projects.save();
                        let step = if shell.step <= super::STEP_FINISH {
                            shell.step
                        } else {
                            shell.first_step()
                        };
                        shell.go_to(step, cx);
                    } else {
                        shell.open_project(open_dir.clone(), cx);
                    }
                }))
                .child(
                    div()
                        .flex_none()
                        .w(px(44.))
                        .h(px(44.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(color(tokens::LINE_STRONG))
                        .child(
                            mono(card.aspect.map_or("—", format_tag))
                                .text_size(px(11.))
                                .text_color(color(tokens::FG2)),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(color(tokens::MUTE))
                                .child(SharedString::from(details)),
                        ),
                )
                .when(running + queued > 0, |d| {
                    d.child(
                        div()
                            .text_size(px(11.))
                            .text_color(color(tokens::SIGNAL))
                            .child(tr!("home.editing", count = running + queued)),
                    )
                })
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(color(tokens::MUTE))
                        .child(ago(card.opened)),
                )
                .map(|d| {
                    if asking {
                        let yes = dir.clone();
                        d.child(
                            chip(("delete-yes", i), tr!("home.delete_confirm"), false, true)
                                .text_color(color(tokens::ERROR))
                                .on_click(cx.listener(move |shell, _: &ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    shell.delete_project(&yes, cx);
                                })),
                        )
                        .child(
                            chip(("delete-no", i), tr!("common.cancel"), false, true).on_click(
                                cx.listener(|shell, _: &ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    if let Some(h) = &mut shell.home {
                                        h.confirm_delete = None;
                                    }
                                    cx.notify();
                                }),
                            ),
                        )
                    } else {
                        let ask = dir.clone();
                        d.child(
                            div()
                                .id(("delete", i))
                                .px(px(6.))
                                .text_color(color(tokens::MUTE))
                                .hover(|d| d.text_color(color(tokens::ERROR)))
                                .child("×")
                                .tooltip(|window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(tr!("home.delete"))
                                        .build(window, cx)
                                })
                                .on_click(cx.listener(move |shell, _: &ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    if let Some(h) = &mut shell.home {
                                        h.confirm_delete = Some(ask.clone());
                                    }
                                    cx.notify();
                                })),
                        )
                    }
                })
        });
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(22.))
                            .line_height(px(28.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(tr!("home.projects")),
                    )
                    .child(
                        button("new-project", tr!("home.new"), true).on_click(cx.listener(
                            |shell, _: &ClickEvent, _, cx| {
                                if let Some(h) = &mut shell.home {
                                    h.wizard = Some(Wizard::default());
                                }
                                cx.notify();
                            },
                        )),
                    ),
            )
            .child(div().flex().flex_col().gap(px(6.)).children(cards))
    }

    #[allow(clippy::too_many_lines)]
    fn wizard_view(&self, wizard: &Wizard, cx: &mut Context<Self>) -> Div {
        let can_cancel = self.home.as_ref().is_some_and(|h| !h.cards.is_empty());
        let header = |title: SharedString| {
            div()
                .text_size(px(22.))
                .line_height(px(28.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title)
        };
        let Some(mode) = wizard.mode else {
            return div()
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(header(tr!("home.new_title")))
                .child(
                    option(
                        "mode-custom",
                        tr!("home.custom"),
                        tr!("home.custom_text"),
                        false,
                    )
                    .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| {
                        shell.edit_wizard(|w| w.mode = Some(ProjectMode::Custom), cx);
                    })),
                )
                .child(
                    option(
                        "mode-magic",
                        tr!("home.magic"),
                        tr!("home.magic_text"),
                        false,
                    )
                    .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| {
                        shell.edit_wizard(|w| w.mode = Some(ProjectMode::Magic), cx);
                    })),
                )
                .when(can_cancel, |d| {
                    d.child(div().flex().child(
                        button("wizard-cancel", tr!("common.cancel"), false).on_click(cx.listener(
                            |shell, _: &ClickEvent, _, cx| {
                                if let Some(h) = &mut shell.home {
                                    h.wizard = None;
                                }
                                cx.notify();
                            },
                        )),
                    ))
                });
        };
        let magic = mode == ProjectMode::Magic;
        let formats = [AspectRatio::Portrait9x16, AspectRatio::Landscape16x9]
            .into_iter()
            .enumerate()
            .map(|(i, a)| {
                chip(
                    ("new-format", i),
                    format!("{} · {}", format_tag(a), super::studio::format_name(a)),
                    wizard.aspect == a,
                    true,
                )
                .on_click(cx.listener(move |shell, _: &ClickEvent, _, cx| {
                    shell.edit_wizard(|w| w.aspect = a, cx);
                }))
            });
        let providers = [
            ProviderChoice::Api,
            ProviderChoice::ClaudeCode,
            ProviderChoice::Local,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            chip(
                ("new-provider", i),
                super::studio::provider_name(p),
                wizard.provider == Some(p),
                true,
            )
            .on_click(cx.listener(move |shell, _: &ClickEvent, _, cx| {
                shell.edit_wizard(|w| w.provider = Some(p), cx);
            }))
        });
        let provider_note = match wizard.provider {
            None => Some((tr!("home.model_help"), tokens::MUTE)),
            Some(p) => super::studio::provider_hint(p, crate::prefs::settings(cx))
                .map(|hint| (hint, tokens::ERROR)),
        };
        let needs_file = magic || wizard.start == Start::Import;
        let file_name = wizard
            .file
            .as_ref()
            .and_then(|f| f.file_name())
            .map(|n| n.to_string_lossy().into_owned());
        let file_zone = needs_file.then(|| {
            div()
                .id("drop-zone")
                .flex()
                .items_center()
                .gap(px(12.))
                .p(px(14.))
                .rounded(px(4.))
                .border_1()
                .border_dashed()
                .border_color(color(tokens::LINE_STRONG))
                .drag_over::<gpui_kit::ExternalPaths>(|style, _, _, _| {
                    style.bg(color(tokens::HOVER))
                })
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.))
                        .text_color(color(if file_name.is_some() {
                            tokens::FG
                        } else {
                            tokens::FG2
                        }))
                        .child(match &file_name {
                            Some(name) => tr!("home.file_chosen", name = name),
                            None if magic => tr!("home.drop_magic"),
                            None => tr!("home.drop_import"),
                        }),
                )
                .child(
                    button("choose-file", tr!("home.choose_file"), false).on_click(
                        cx.listener(|shell, _: &ClickEvent, _, cx| shell.choose_wizard_file(cx)),
                    ),
                )
        });
        let starts = (!magic).then(|| {
            let s = wizard.start;
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(label(tr!("home.start_label")))
                .child(
                    option(
                        "start-script",
                        tr!("home.start.script"),
                        tr!("home.start.script_text"),
                        s == Start::Script,
                    )
                    .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| {
                        shell.edit_wizard(|w| w.start = Start::Script, cx);
                    })),
                )
                .child(
                    option(
                        "start-import",
                        tr!("home.start.import"),
                        tr!("home.start.import_text"),
                        s == Start::Import,
                    )
                    .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| {
                        shell.edit_wizard(|w| w.start = Start::Import, cx);
                    })),
                )
                .when(s == Start::Import, |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .pl(px(20.))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(4.))
                                    .child(
                                        chip(
                                            "derush-on",
                                            tr!("home.derush_on"),
                                            wizard.derush,
                                            true,
                                        )
                                        .on_click(
                                            cx.listener(|shell, _: &ClickEvent, _, cx| {
                                                shell.edit_wizard(|w| w.derush = true, cx);
                                            }),
                                        ),
                                    )
                                    .child(
                                        chip(
                                            "derush-off",
                                            tr!("home.derush_off"),
                                            !wizard.derush,
                                            true,
                                        )
                                        .on_click(
                                            cx.listener(|shell, _: &ClickEvent, _, cx| {
                                                shell.edit_wizard(|w| w.derush = false, cx);
                                            }),
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(color(tokens::MUTE))
                                    .child(if wizard.derush {
                                        tr!("home.derush_on_text")
                                    } else {
                                        tr!("home.derush_off_text")
                                    }),
                            ),
                    )
                })
                .child(
                    option(
                        "start-silent",
                        tr!("home.start.silent"),
                        tr!("home.start.silent_text"),
                        s == Start::Silent,
                    )
                    .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| {
                        shell.edit_wizard(|w| w.start = Start::Silent, cx);
                    })),
                )
                .when(s == Start::Silent, |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .pl(px(20.))
                            .child(label(tr!("home.duration")))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(div().w(px(100.)).child(Input::new(&self.new_duration)))
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(color(tokens::FG2))
                                            .child(tr!("home.seconds")),
                                    ),
                            )
                            .child(label(tr!("home.brief")))
                            .child(
                                div()
                                    .h(px(90.))
                                    .child(Textarea::new(&self.new_brief).h_full()),
                            ),
                    )
                })
        });
        let action = if magic {
            tr!("home.create_magic")
        } else {
            tr!("home.create")
        };
        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(header(if magic {
                tr!("home.magic")
            } else {
                tr!("home.custom")
            }))
            .when(magic, |d| {
                d.child(
                    div()
                        .text_size(px(12.))
                        .text_color(color(tokens::FG2))
                        .child(tr!("home.magic_text")),
                )
            })
            .child(label(tr!("home.title_label")))
            .child(Input::new(&self.new_title))
            .child(label(tr!("home.format")))
            .child(div().flex().gap(px(4.)).children(formats))
            .child(label(tr!("home.model")))
            .child(div().flex().gap(px(4.)).children(providers))
            .when_some(provider_note, |d, (text, tone)| {
                d.child(div().text_size(px(11.)).text_color(color(tone)).child(text))
            })
            .when(wizard.provider == Some(ProviderChoice::Local), |d| {
                d.child(self.local_picker(super::local_models::PickerAt::Home, cx))
            })
            .children(starts)
            .children(file_zone)
            .when_some(wizard.error.clone(), |d, e| {
                d.child(
                    div()
                        .text_size(px(12.))
                        .text_color(color(tokens::ERROR))
                        .child(e),
                )
            })
            .child(
                div()
                    .flex()
                    .justify_between()
                    .pt(px(6.))
                    .child(
                        button("wizard-back", tr!("common.back"), false).on_click(cx.listener(
                            |shell, _: &ClickEvent, _, cx| {
                                if let Some(h) = &mut shell.home {
                                    h.back();
                                }
                                cx.notify();
                            },
                        )),
                    )
                    .child(button("wizard-create", action, true).on_click(
                        cx.listener(|shell, _: &ClickEvent, _, cx| shell.submit_wizard(cx)),
                    )),
            )
    }
}
