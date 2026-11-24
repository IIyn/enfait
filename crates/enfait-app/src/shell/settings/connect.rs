//! Connection guide (Settings → Models, onboarding): installing and
//! signing in to Claude Code, or setting up a local model server, with the
//! steps of this operating system. Each step is checked in the background
//! while the guide is on screen, and ticks itself off once done.

use super::{Section, chip, field, small};
use crate::prefs;
use crate::theme::{Token, color, tokens};
use crate::tr;
use enfait_ai::ProviderChoice;
use enfait_ai::connect::{ClaudeStatus, ServedModel, Server};
use enfait_config::Secret;
use enfait_config::secrets;
use gpui_kit::component::input::Input;
use gpui_kit::prelude::*;
use gpui_kit::{ClickEvent, ClipboardItem, Context, Div, FontWeight, SharedString, div, px};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// What the guide explains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Target {
    ClaudeCode,
    Local,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Os {
    Linux,
    Mac,
    Windows,
}

const OS: Os = if cfg!(target_os = "macos") {
    Os::Mac
} else if cfg!(windows) {
    Os::Windows
} else {
    Os::Linux
};

const OS_NAME: &str = match OS {
    Os::Linux => "Linux",
    Os::Mac => "macOS",
    Os::Windows => "Windows",
};

/// Time between two checks.
const TICK: Duration = Duration::from_secs(2);
/// Once Claude Code is signed in, `claude auth status` runs less often.
const CLAUDE_READY_EVERY: Duration = Duration::from_secs(15);

/// What the last check of a local server found.
#[derive(Clone)]
struct LocalProbe {
    server: Server,
    url: String,
    installed: Option<PathBuf>,
    models: Result<Vec<ServedModel>, String>,
}

#[derive(Default)]
pub(super) struct ConnectGuide {
    /// Chosen tab; `None` follows the provider of the project.
    target: Option<Target>,
    /// Chosen server; `None` follows the address in the settings.
    server: Option<Server>,
    polling: bool,
    claude: Option<ClaudeStatus>,
    claude_checked: Option<Instant>,
    local: Option<LocalProbe>,
    /// The command last copied (shows "Copied").
    copied: Option<String>,
}

impl ConnectGuide {
    pub(super) fn show_local(&mut self) {
        self.target = Some(Target::Local);
    }
}

/// One check to run in the background.
enum Job {
    Claude,
    Local { server: Server, url: String },
}

enum Found {
    Claude(ClaudeStatus),
    Local(LocalProbe),
}

impl Job {
    async fn run(self) -> Found {
        match self {
            Self::Claude => Found::Claude(enfait_ai::connect::claude_status().await),
            Self::Local { server, url } => {
                let installed = server.installed();
                let key = secrets::get(Secret::Local);
                let models = match reqwest::Client::builder().build() {
                    Ok(client) => {
                        enfait_ai::connect::list_models(&client, &url, key.as_deref()).await
                    }
                    Err(e) => Err(e.to_string()),
                };
                Found::Local(LocalProbe {
                    server,
                    url,
                    installed,
                    models,
                })
            }
        }
    }
}

/// A step of the guide.
struct Step {
    title: SharedString,
    done: bool,
    /// Line under the title: what was found, or why it is not done yet.
    detail: Option<(SharedString, Token)>,
    /// Instructions, shown for the first step not done.
    body: Div,
}

impl Step {
    fn new(title: SharedString, done: bool) -> Self {
        Self {
            title,
            done,
            detail: None,
            body: div(),
        }
    }

    fn detail(mut self, text: impl Into<SharedString>, tone: Token) -> Self {
        self.detail = Some((text.into(), tone));
        self
    }

    fn body(mut self, body: Div) -> Self {
        self.body = body;
        self
    }
}

fn column() -> Div {
    div().flex().flex_col().gap(px(8.))
}

fn row() -> Div {
    div().flex().flex_wrap().items_center().gap(px(6.))
}

fn link(id: &'static str, text: SharedString, url: &'static str) -> impl IntoElement {
    chip(id, text, false, true).on_click(move |_, _, cx| cx.open_url(url))
}

impl crate::shell::Shell {
    /// The guide is on screen: settings on "Models", or the onboarding
    /// step of the models.
    fn connect_visible(&self, cx: &Context<Self>) -> bool {
        if prefs::settings(cx).onboarded {
            self.settings.open && self.settings.section == Section::Models
        } else {
            self.settings.step == 2
        }
    }

    fn connect_target(&self) -> Target {
        self.settings.connect.target.unwrap_or(
            if self.session.data.provider == ProviderChoice::Local {
                Target::Local
            } else {
                Target::ClaudeCode
            },
        )
    }

    fn connect_server(&self, cx: &Context<Self>) -> Server {
        self.settings
            .connect
            .server
            .unwrap_or_else(|| Server::from_url(&prefs::settings(cx).local_url()))
    }

    /// Address to check for `server`: the one in the settings when it
    /// points to this server, else the server's default.
    fn connect_url(&self, server: Server, cx: &Context<Self>) -> String {
        let configured = prefs::settings(cx).local_url();
        match server.default_url() {
            Some(url) if Server::from_url(&configured) != server => url.to_owned(),
            _ => configured,
        }
    }

    /// The check to run now; `None` when none is due this time.
    fn connect_job(&self, cx: &Context<Self>) -> Option<Job> {
        Some(match self.connect_target() {
            Target::ClaudeCode => {
                let guide = &self.settings.connect;
                let ready = guide
                    .claude
                    .as_ref()
                    .is_some_and(|c| c.logged_in == Some(true));
                let recent = guide
                    .claude_checked
                    .is_some_and(|t| t.elapsed() < CLAUDE_READY_EVERY);
                if ready && recent {
                    return None;
                }
                Job::Claude
            }
            Target::Local => {
                let server = self.connect_server(cx);
                Job::Local {
                    server,
                    url: self.connect_url(server, cx),
                }
            }
        })
    }

    /// Starts the background checks if the guide is shown (once at a time;
    /// they stop by themselves when it is no longer shown).
    pub(super) fn poll_connect(&mut self, cx: &mut Context<Self>) {
        if self.settings.connect.polling || !self.connect_visible(cx) {
            return;
        }
        self.settings.connect.polling = true;
        cx.spawn(async move |this, cx| {
            loop {
                let next = this.update(cx, |shell, cx| {
                    let visible = shell.connect_visible(cx);
                    if !visible {
                        shell.settings.connect.polling = false;
                    }
                    visible.then(|| shell.connect_job(cx))
                });
                let job = match next {
                    Ok(Some(job)) => job,
                    _ => break,
                };
                if let Some(job) = job {
                    let found = cx
                        .background_spawn(async move { super::super::assets::block_on(job.run()) })
                        .await;
                    if let Ok(found) = found {
                        let alive = this.update(cx, |shell, cx| {
                            let guide = &mut shell.settings.connect;
                            match found {
                                Found::Claude(status) => {
                                    guide.claude = Some(status);
                                    guide.claude_checked = Some(Instant::now());
                                }
                                Found::Local(probe) => guide.local = Some(probe),
                            }
                            cx.notify();
                        });
                        if alive.is_err() {
                            break;
                        }
                    }
                }
                cx.background_executor().timer(TICK).await;
            }
        })
        .detach();
    }

    fn copy_command(&mut self, text: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
        self.settings.connect.copied = Some(text.to_owned());
        cx.notify();
    }

    /// A command to type, with its "Copy" button.
    fn command_line(&self, id: usize, text: &'static str, cx: &mut Context<Self>) -> Div {
        let copied = self.settings.connect.copied.as_deref() == Some(text);
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                super::super::mono(text)
                    .flex_1()
                    .min_w(px(0.))
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(3.))
                    .bg(color(tokens::RAISED))
                    .text_size(px(12.))
                    .line_height(px(18.)),
            )
            .child(
                chip(
                    ("connect-copy", id),
                    if copied {
                        tr!("connect.copied")
                    } else {
                        tr!("connect.copy")
                    },
                    false,
                    true,
                )
                .on_click(cx.listener(move |shell, _: &ClickEvent, _, cx| {
                    shell.copy_command(text, cx);
                })),
            )
    }

    fn choose_local_model(&mut self, url: String, model: String, cx: &mut Context<Self>) {
        prefs::update(cx, |s| {
            s.local_url = url;
            s.local_model = model;
        });
        self.settings.sync = true;
        self.set_provider(ProviderChoice::Local, cx);
    }

    /// The guide, in a settings field.
    pub(super) fn connect_field(&self, cx: &mut Context<Self>) -> Div {
        let target = self.connect_target();
        let tabs = [
            (Target::ClaudeCode, tr!("connect.tab.claude")),
            (Target::Local, tr!("connect.tab.local")),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (t, name))| {
            chip(("connect-tab", i), name, t == target, true).on_click(cx.listener(
                move |shell, _: &ClickEvent, _, cx| {
                    shell.settings.connect.target = Some(t);
                    cx.notify();
                },
            ))
        })
        .collect::<Vec<_>>();
        let content = match target {
            Target::ClaudeCode => self.claude_guide(cx),
            Target::Local => self.local_guide(cx),
        };
        field(
            tr!("connect.title"),
            Some(tr!("connect.help", os = OS_NAME)),
        )
        .child(row().children(tabs))
        .child(content)
        .child(small(tr!("connect.auto"), tokens::MUTE))
    }

    fn claude_guide(&self, cx: &mut Context<Self>) -> Div {
        let Some(status) = self.settings.connect.claude.clone() else {
            return column().child(small(tr!("connect.checking"), tokens::FG2));
        };
        let provider = self.session.data.provider;
        let api_key = matches!(
            self.settings.key_status.get(&Secret::Anthropic),
            Some(Some(_))
        );
        let in_use = provider == ProviderChoice::ClaudeCode
            || (provider == ProviderChoice::Auto && !api_key);

        let mut install = Step::new(tr!("connect.claude.install"), status.program.is_some());
        install = match &status.program {
            Some(path) => install.detail(
                tr!("settings.found_at", path = path.display().to_string()),
                tokens::OK,
            ),
            None => install.body(self.claude_install_body(cx)),
        };

        let mut login = Step::new(tr!("connect.claude.login"), status.logged_in == Some(true));
        login = match (status.logged_in, &status.method) {
            (Some(true), Some(method)) => login.detail(
                tr!("connect.claude.logged_in_with", method = method.clone()),
                tokens::OK,
            ),
            (Some(true), None) => login.detail(tr!("connect.claude.logged_in"), tokens::OK),
            (None, _) if status.program.is_some() => {
                login.detail(tr!("connect.claude.unknown"), tokens::MIXED)
            }
            _ => login,
        }
        .body(
            column()
                .child(small(tr!("connect.claude.login_how"), tokens::FG2))
                .child(self.command_line(10, "claude auth login", cx))
                .child(small(tr!("connect.claude.login_alt"), tokens::MUTE))
                .when(status.logged_in.is_none(), |d| {
                    d.child(self.command_line(11, "claude update", cx))
                }),
        );

        let mut use_it = Step::new(tr!("connect.claude.use"), in_use);
        use_it = if in_use {
            use_it.detail(tr!("connect.claude.in_use"), tokens::OK)
        } else {
            use_it.body(
                row().child(
                    chip(
                        "connect-use-claude",
                        tr!("connect.claude.use_button"),
                        true,
                        true,
                    )
                    .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| {
                        shell.set_provider(ProviderChoice::ClaudeCode, cx);
                    })),
                ),
            )
        };
        if provider == ProviderChoice::Auto && api_key {
            use_it = use_it.detail(tr!("connect.claude.api_first"), tokens::MIXED);
        }
        steps(vec![install, login, use_it])
    }

    fn claude_install_body(&self, cx: &mut Context<Self>) -> Div {
        let body = column().child(small(
            match OS {
                Os::Windows => tr!("connect.in_powershell"),
                _ => tr!("connect.in_terminal"),
            },
            tokens::FG2,
        ));
        let body = match OS {
            Os::Linux => body.child(self.command_line(
                0,
                "curl -fsSL https://claude.ai/install.sh | bash",
                cx,
            )),
            Os::Mac => body
                .child(self.command_line(0, "curl -fsSL https://claude.ai/install.sh | bash", cx))
                .child(small(tr!("connect.or_homebrew"), tokens::MUTE))
                .child(self.command_line(1, "brew install --cask claude-code", cx)),
            Os::Windows => body
                .child(self.command_line(0, "irm https://claude.ai/install.ps1 | iex", cx))
                .child(small(tr!("connect.or_winget"), tokens::MUTE))
                .child(self.command_line(1, "winget install Anthropic.ClaudeCode", cx)),
        };
        body.child(small(tr!("connect.claude.account"), tokens::MUTE))
            .child(row().child(link(
                "connect-claude-page",
                tr!("connect.claude.page"),
                "https://claude.com/claude-code",
            )))
    }

    fn local_guide(&self, cx: &mut Context<Self>) -> Div {
        let server = self.connect_server(cx);
        let servers = Server::ALL
            .iter()
            .enumerate()
            .map(|(i, &s)| {
                let name: SharedString = match s {
                    Server::Ollama => tr!("connect.local.ollama"),
                    Server::Other => tr!("connect.local.other"),
                    _ => s.label().into(),
                };
                chip(("connect-server", i), name, s == server, true).on_click(cx.listener(
                    move |shell, _: &ClickEvent, _, cx| {
                        shell.settings.connect.server = Some(s);
                        cx.notify();
                    },
                ))
            })
            .collect::<Vec<_>>();
        let url = self.connect_url(server, cx);
        let probe = self
            .settings
            .connect
            .local
            .clone()
            .filter(|p| p.server == server && p.url == url);
        let guide = match probe {
            None => column().child(small(tr!("connect.checking"), tokens::FG2)),
            Some(probe) => steps(self.local_steps(&probe, cx)),
        };
        column()
            .gap(px(12.))
            .child(small(tr!("connect.local.help"), tokens::FG2))
            .child(row().children(servers))
            .child(guide)
            .child(self.local_manual())
            .children(self.key_field(Secret::Local, cx))
    }

    fn local_steps(&self, probe: &LocalProbe, cx: &mut Context<Self>) -> Vec<Step> {
        let reachable = probe.models.is_ok();
        let models = probe.models.clone().unwrap_or_default();
        let has_models = !models.is_empty();
        let failure = match &probe.models {
            Err(e) if e == "unreachable" => Some((
                tr!("connect.local.unreachable", url = probe.url.clone()),
                tokens::MUTE,
            )),
            Err(e) if e.contains("401") || e.contains("403") => {
                Some((tr!("connect.local.key_needed"), tokens::ERROR))
            }
            Err(e) => Some((tr!("connect.local.error", error = e.clone()), tokens::ERROR)),
            Ok(_) => None,
        };
        let installed = || {
            let done = probe.installed.is_some() || reachable;
            let step = Step::new(
                tr!("connect.local.install", server = probe.server.label()),
                done,
            );
            match &probe.installed {
                Some(path) => step.detail(
                    tr!("settings.found_at", path = path.display().to_string()),
                    tokens::OK,
                ),
                None if reachable => step.detail(tr!("connect.local.running"), tokens::OK),
                None => step,
            }
        };
        let running = |title: SharedString, body: Div| {
            let step = Step::new(title, reachable).body(body);
            match &failure {
                Some((text, tone)) => step.detail(text.clone(), *tone),
                None => step.detail(
                    tr!("connect.local.answers", url = probe.url.clone()),
                    tokens::OK,
                ),
            }
        };
        let downloaded = |body: Div| {
            let step = Step::new(tr!("connect.local.download"), has_models).body(body);
            if has_models {
                step.detail(tr!("connect.local.count", count = models.len()), tokens::OK)
            } else {
                step
            }
        };
        let choose = self.choose_step(probe, &models, cx);

        match probe.server {
            Server::Ollama => vec![
                installed().body(self.ollama_install(cx)),
                running(tr!("connect.local.start"), self.ollama_start(cx)),
                downloaded(
                    column()
                        .child(small(tr!("connect.ollama.pull"), tokens::FG2))
                        .child(small(tr!("connect.ollama.small"), tokens::MUTE))
                        .child(self.command_line(30, "ollama pull qwen3-vl:8b", cx))
                        .child(small(tr!("connect.ollama.big"), tokens::MUTE))
                        .child(self.command_line(31, "ollama pull qwen3-vl:30b", cx))
                        .child(small(tr!("connect.local.wait_download"), tokens::MUTE)),
                ),
                choose,
            ],
            Server::LmStudio => vec![
                installed().body(
                    column()
                        .child(small(tr!("connect.lmstudio.install"), tokens::FG2))
                        .when(OS == Os::Linux, |d| {
                            d.child(small(tr!("connect.lmstudio.appimage"), tokens::MUTE))
                        })
                        .child(row().child(link(
                            "connect-lmstudio-page",
                            tr!("connect.download_from", site = "lmstudio.ai"),
                            "https://lmstudio.ai/download",
                        ))),
                ),
                running(
                    tr!("connect.local.start"),
                    column()
                        .child(small(tr!("connect.lmstudio.start"), tokens::FG2))
                        .child(small(tr!("connect.or_command"), tokens::MUTE))
                        .child(self.command_line(40, "lms server start", cx)),
                ),
                downloaded(column().child(small(tr!("connect.lmstudio.download"), tokens::FG2))),
                choose,
            ],
            Server::LlamaCpp => vec![
                installed().body(self.llamacpp_install(cx)),
                running(
                    tr!("connect.llamacpp.start"),
                    column()
                        .child(small(tr!("connect.llamacpp.start_how"), tokens::FG2))
                        .child(self.command_line(
                            50,
                            "llama-server -hf ggml-org/gemma-3-12b-it-GGUF --port 8080",
                            cx,
                        ))
                        .child(small(tr!("connect.local.wait_download"), tokens::MUTE)),
                ),
                choose,
            ],
            Server::Other => vec![
                running(
                    tr!("connect.other.start"),
                    column().child(small(tr!("connect.other.how"), tokens::FG2)),
                ),
                choose,
            ],
        }
    }

    fn ollama_install(&self, cx: &mut Context<Self>) -> Div {
        match OS {
            Os::Linux => column()
                .child(small(tr!("connect.in_terminal"), tokens::FG2))
                .child(self.command_line(20, "curl -fsSL https://ollama.com/install.sh | sh", cx)),
            Os::Mac => column()
                .child(small(tr!("connect.ollama.app"), tokens::FG2))
                .child(row().child(link(
                    "connect-ollama-page",
                    tr!("connect.download_from", site = "ollama.com"),
                    "https://ollama.com/download/mac",
                )))
                .child(small(tr!("connect.or_homebrew"), tokens::MUTE))
                .child(self.command_line(21, "brew install ollama", cx)),
            Os::Windows => column()
                .child(small(tr!("connect.ollama.app"), tokens::FG2))
                .child(row().child(link(
                    "connect-ollama-page",
                    tr!("connect.download_from", site = "ollama.com"),
                    "https://ollama.com/download/windows",
                )))
                .child(small(tr!("connect.or_winget"), tokens::MUTE))
                .child(self.command_line(21, "winget install Ollama.Ollama", cx)),
        }
    }

    fn ollama_start(&self, cx: &mut Context<Self>) -> Div {
        let how = match OS {
            Os::Linux => tr!("connect.ollama.start_linux"),
            Os::Mac => tr!("connect.ollama.start_macos"),
            Os::Windows => tr!("connect.ollama.start_windows"),
        };
        column()
            .child(small(how, tokens::FG2))
            .child(small(tr!("connect.or_command"), tokens::MUTE))
            .child(self.command_line(22, "ollama serve", cx))
    }

    fn llamacpp_install(&self, cx: &mut Context<Self>) -> Div {
        let releases = row().child(link(
            "connect-llamacpp-page",
            tr!("connect.llamacpp.releases"),
            "https://github.com/ggml-org/llama.cpp/releases",
        ));
        match OS {
            Os::Linux => column()
                .child(small(tr!("connect.llamacpp.linux"), tokens::FG2))
                .child(self.command_line(23, "brew install llama.cpp", cx))
                .child(releases),
            Os::Mac => column()
                .child(small(tr!("connect.in_terminal"), tokens::FG2))
                .child(self.command_line(23, "brew install llama.cpp", cx)),
            Os::Windows => column()
                .child(small(tr!("connect.in_powershell"), tokens::FG2))
                .child(self.command_line(23, "winget install llama.cpp", cx))
                .child(releases),
        }
    }

    /// Last step: pick one of the served models.
    fn choose_step(
        &self,
        probe: &LocalProbe,
        models: &[ServedModel],
        cx: &mut Context<Self>,
    ) -> Step {
        let settings = prefs::settings(cx);
        let current = settings.local_model();
        let same_url =
            settings.local_url().trim_end_matches('/') == probe.url.trim_end_matches('/');
        let chosen = models
            .iter()
            .find(|m| same_url && current.as_deref() == Some(m.id.as_str()));
        let done = chosen.is_some() && self.session.data.provider == ProviderChoice::Local;
        let mut step = Step::new(tr!("connect.local.choose"), done);
        if let Some(model) = chosen {
            let text = tr!("connect.local.chosen", model = model.id.clone());
            step = if model.vision == Some(false) {
                step.detail(
                    format!("{text} {}", tr!("connect.local.no_vision")),
                    tokens::MIXED,
                )
            } else {
                step.detail(text, tokens::OK)
            };
        }
        if chosen.is_some() && !done {
            step = step.body(
                row().child(
                    chip(
                        "connect-use-local",
                        tr!("connect.local.use_button"),
                        true,
                        true,
                    )
                    .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| {
                        shell.set_provider(ProviderChoice::Local, cx);
                    })),
                ),
            );
            return step;
        }
        let chips = models.iter().enumerate().map(|(i, model)| {
            let name = match model.vision {
                Some(true) => format!("{} · {}", model.id, tr!("connect.local.images")),
                Some(false) => format!("{} · {}", model.id, tr!("connect.local.text_only")),
                None => model.id.clone(),
            };
            let url = probe.url.clone();
            let id = model.id.clone();
            chip(
                ("connect-model", i),
                name,
                current.as_deref() == Some(model.id.as_str()) && same_url,
                true,
            )
            .on_click(cx.listener(move |shell, _: &ClickEvent, _, cx| {
                shell.choose_local_model(url.clone(), id.clone(), cx);
            }))
        });
        let no_vision = !models.is_empty() && models.iter().all(|m| m.vision == Some(false));
        step.body(
            column()
                .child(small(tr!("connect.local.choose_how"), tokens::FG2))
                .child(row().children(chips))
                .when(no_vision, |d| {
                    d.child(small(tr!("connect.local.no_vision"), tokens::MIXED))
                }),
        )
    }

    /// Address, model name and key typed by hand.
    fn local_manual(&self) -> Div {
        column()
            .pt(px(4.))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_size(px(12.))
                    .child(tr!("connect.manual")),
            )
            .child(small(tr!("settings.local_help"), tokens::MUTE))
            .child(small(tr!("settings.local_url"), tokens::FG2))
            .child(Input::new(&self.settings.local_url))
            .child(small(tr!("settings.local_model"), tokens::FG2))
            .child(Input::new(&self.settings.local_model))
    }
}

/// The numbered steps; the first one not done shows its instructions.
fn steps(steps: Vec<Step>) -> Div {
    let current = steps.iter().position(|s| !s.done);
    let all_done = current.is_none();
    column()
        .gap(px(2.))
        .children(steps.into_iter().enumerate().map(|(i, step)| {
            let active = current == Some(i);
            let number = i.saturating_add(1).to_string();
            let marker = div()
                .size(px(20.))
                .flex_none()
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.))
                .map(|d| {
                    if step.done {
                        d.bg(color(tokens::OK))
                            .text_color(color(tokens::BG))
                            .child("✓")
                    } else if active {
                        d.border_1()
                            .border_color(color(tokens::SIGNAL))
                            .text_color(color(tokens::SIGNAL))
                            .child(number)
                    } else {
                        d.border_1()
                            .border_color(color(tokens::LINE_STRONG))
                            .text_color(color(tokens::MUTE))
                            .child(number)
                    }
                });
            div().flex().gap(px(10.)).py(px(6.)).child(marker).child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_color(color(if step.done || active {
                                tokens::FG
                            } else {
                                tokens::MUTE
                            }))
                            .when(active, |d| d.font_weight(FontWeight::MEDIUM))
                            .child(step.title),
                    )
                    .when_some(step.detail, |d, (text, tone)| d.child(small(text, tone)))
                    .when(active, |d| d.child(step.body)),
            )
        }))
        .when(all_done, |d| {
            d.child(small(tr!("connect.ready"), tokens::OK).pt(px(4.)))
        })
}
