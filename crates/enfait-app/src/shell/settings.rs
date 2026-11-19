//! Settings page and first-launch onboarding.
//!
//! Both draw the same sections: general (language, closing the window),
//! appearance (theme, accent), models (provider, quality, Anthropic key,
//! local model), asset banks (keys) and tools (ffmpeg, renderer,
//! transcription model, Claude Code). Keys go to the system keychain
//! (`enfait_config::secrets`) and are never shown back.

use super::studio::{QUALITIES, chip, provider_name, quality_name};
use super::{Shell, label};
use crate::prefs;
use crate::theme::{self, color, tokens};
use crate::tr;
use enfait_ai::ProviderChoice;
use enfait_config::secrets::{self, Source};
use enfait_config::{Accent, CloseAction, Language, Secret, ThemeId};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{
    ClickEvent, Context, Div, Entity, FontWeight, SharedString, Subscription, Window, div, px,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Section {
    General,
    Appearance,
    Models,
    Banks,
    Tools,
}

const SECTIONS: [Section; 5] = [
    Section::General,
    Section::Appearance,
    Section::Models,
    Section::Banks,
    Section::Tools,
];

impl Section {
    fn title(self) -> SharedString {
        match self {
            Self::General => tr!("settings.section.general"),
            Self::Appearance => tr!("settings.section.appearance"),
            Self::Models => tr!("settings.section.models"),
            Self::Banks => tr!("settings.section.banks"),
            Self::Tools => tr!("settings.section.tools"),
        }
    }
}

/// Onboarding steps: welcome and language, then the sections, then the
/// closing behaviour.
const STEPS: usize = 6;

/// Keys typed in the settings, in display order.
const KEYS: [Secret; 5] = [
    Secret::Anthropic,
    Secret::Local,
    Secret::Pexels,
    Secret::Pixabay,
    Secret::Freesound,
];

/// A model or browser download in progress.
#[derive(Clone)]
struct Download {
    done: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    result: Arc<Mutex<Option<Result<(), String>>>>,
}

impl Download {
    fn new() -> Self {
        Self {
            done: Arc::new(AtomicU64::new(0)),
            total: Arc::new(AtomicU64::new(0)),
            result: Arc::new(Mutex::new(None)),
        }
    }

    fn finished(&self) -> Option<Result<(), String>> {
        self.result.lock().ok().and_then(|r| r.clone())
    }

    /// Progress from 0 to 1, if the size is known.
    #[allow(clippy::cast_precision_loss)]
    fn fraction(&self) -> Option<f64> {
        let total = self.total.load(Ordering::Relaxed);
        (total > 0).then(|| self.done.load(Ordering::Relaxed) as f64 / total as f64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Tool {
    Whisper,
    Browser,
}

pub(super) struct SettingsUi {
    pub(super) open: bool,
    section: Section,
    /// Current onboarding step.
    step: usize,
    keys: Vec<(Secret, Entity<InputState>)>,
    /// Where each key comes from, read in the background (the keychain can
    /// be slow to answer).
    key_status: BTreeMap<Secret, Option<Source>>,
    /// Key fields to empty at the next render (needs the window).
    clear: Vec<Secret>,
    local_url: Entity<InputState>,
    local_model: Entity<InputState>,
    /// Copy the settings into the fields at the next render.
    sync: bool,
    notice: Option<(SharedString, bool)>,
    downloads: BTreeMap<Tool, Download>,
}

fn key_input(window: &mut Window, cx: &mut Context<Shell>) -> Entity<InputState> {
    cx.new(|cx| {
        InputState::new(window, cx)
            .masked(true)
            .placeholder(tr!("settings.key_placeholder"))
    })
}

impl SettingsUi {
    /// `ENFAIT_OPEN=settings/models` or `onboarding/3` opens a page directly
    /// (screenshots, tests by hand).
    pub(super) fn open_from_env(&mut self) {
        let Ok(target) = std::env::var("ENFAIT_OPEN") else {
            return;
        };
        let (page, rest) = target.split_once('/').unwrap_or((target.as_str(), ""));
        match page {
            "settings" => {
                self.open = true;
                self.section = match rest {
                    "appearance" => Section::Appearance,
                    "models" => Section::Models,
                    "banks" => Section::Banks,
                    "tools" => Section::Tools,
                    _ => Section::General,
                };
            }
            "onboarding" => self.step = rest.parse().unwrap_or(0),
            _ => {}
        }
    }

    pub(super) fn new(window: &mut Window, cx: &mut Context<Shell>) -> (Self, Vec<Subscription>) {
        let mut subscriptions = Vec::new();
        let keys: Vec<(Secret, Entity<InputState>)> = KEYS
            .iter()
            .map(|&secret| {
                let input = key_input(window, cx);
                subscriptions.push(cx.subscribe(
                    &input,
                    move |shell: &mut Shell, state, event: &InputEvent, cx| {
                        if matches!(event, InputEvent::PressEnter { .. }) {
                            let value = state.read(cx).value().to_string();
                            shell.save_key(secret, Some(value), cx);
                        }
                    },
                ));
                (secret, input)
            })
            .collect();
        let local_url =
            cx.new(|cx| InputState::new(window, cx).placeholder(enfait_config::DEFAULT_LOCAL_URL));
        let local_model = cx.new(|cx| {
            InputState::new(window, cx).placeholder(tr!("settings.local_model_placeholder"))
        });
        for input in [&local_url, &local_model] {
            subscriptions.push(cx.subscribe(
                input,
                |shell: &mut Shell, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                        shell.commit_local_model(cx);
                    }
                },
            ));
        }
        // Key status, away from the interface thread.
        cx.spawn(async move |this, cx| {
            let status = cx
                .background_spawn(async move {
                    KEYS.iter()
                        .map(|&s| (s, secrets::source(s)))
                        .collect::<BTreeMap<_, _>>()
                })
                .await;
            let _ = this.update(cx, |shell, cx| {
                shell.settings.key_status = status;
                cx.notify();
            });
        })
        .detach();
        (
            Self {
                open: false,
                section: Section::General,
                step: 0,
                keys,
                key_status: BTreeMap::new(),
                clear: Vec::new(),
                local_url,
                local_model,
                sync: true,
                notice: None,
                downloads: BTreeMap::new(),
            },
            subscriptions,
        )
    }

    /// Fields that need the window: settings copied in, saved keys emptied.
    pub(super) fn sync_inputs(&mut self, window: &mut Window, cx: &mut Context<Shell>) {
        if std::mem::take(&mut self.sync) {
            let settings = prefs::settings(cx).clone();
            self.local_url
                .update(cx, |s, cx| s.set_value(settings.local_url, window, cx));
            self.local_model
                .update(cx, |s, cx| s.set_value(settings.local_model, window, cx));
        }
        for secret in std::mem::take(&mut self.clear) {
            if let Some((_, input)) = self.keys.iter().find(|(s, _)| *s == secret) {
                input.update(cx, |s, cx| s.set_value("", window, cx));
            }
        }
    }

    pub(super) fn translate_placeholders(&self, window: &mut Window, cx: &mut Context<Shell>) {
        for (_, input) in &self.keys {
            input.update(cx, |s, cx| {
                s.set_placeholder(tr!("settings.key_placeholder"), window, cx);
            });
        }
        self.local_model.update(cx, |s, cx| {
            s.set_placeholder(tr!("settings.local_model_placeholder"), window, cx);
        });
    }
}

fn theme_name(theme: ThemeId) -> SharedString {
    match theme {
        ThemeId::System => tr!("settings.theme.system"),
        ThemeId::Dark => tr!("settings.theme.dark"),
        ThemeId::Light => tr!("settings.theme.light"),
        ThemeId::CatppuccinLatte => "Catppuccin Latte".into(),
        ThemeId::CatppuccinFrappe => "Catppuccin Frappé".into(),
        ThemeId::CatppuccinMacchiato => "Catppuccin Macchiato".into(),
        ThemeId::CatppuccinMocha => "Catppuccin Mocha".into(),
        ThemeId::GruvboxDark => "Gruvbox".into(),
        ThemeId::Nord => "Nord".into(),
    }
}

fn accent_name(accent: Accent) -> SharedString {
    match accent {
        Accent::Chartreuse => tr!("settings.accent.chartreuse"),
        Accent::Mint => tr!("settings.accent.mint"),
        Accent::Sky => tr!("settings.accent.sky"),
        Accent::Amber => tr!("settings.accent.amber"),
        Accent::Rose => tr!("settings.accent.rose"),
        Accent::Lavender => tr!("settings.accent.lavender"),
    }
}

fn key_name(secret: Secret) -> SharedString {
    match secret {
        Secret::Anthropic => tr!("settings.key.anthropic"),
        Secret::Pexels => "Pexels".into(),
        Secret::Pixabay => "Pixabay".into(),
        Secret::Freesound => "Freesound".into(),
        Secret::Local => tr!("settings.key.local"),
    }
}

/// Where to get a key.
const fn key_page(secret: Secret) -> Option<&'static str> {
    match secret {
        Secret::Anthropic => Some("https://console.anthropic.com/settings/keys"),
        Secret::Pexels => Some("https://www.pexels.com/api/"),
        Secret::Pixabay => Some("https://pixabay.com/api/docs/"),
        Secret::Freesound => Some("https://freesound.org/apiv2/apply"),
        Secret::Local => None,
    }
}

fn key_help(secret: Secret) -> SharedString {
    match secret {
        Secret::Anthropic => tr!("settings.key_help.anthropic"),
        Secret::Pexels => tr!("settings.key_help.pexels"),
        Secret::Pixabay => tr!("settings.key_help.pixabay"),
        Secret::Freesound => tr!("settings.key_help.freesound"),
        Secret::Local => tr!("settings.key_help.local"),
    }
}

/// How to install ffmpeg on this system.
fn ffmpeg_hint() -> SharedString {
    if cfg!(target_os = "macos") {
        tr!("settings.tools.ffmpeg_macos")
    } else if cfg!(windows) {
        tr!("settings.tools.ffmpeg_windows")
    } else {
        tr!("settings.tools.ffmpeg_linux")
    }
}

fn small(text: impl Into<SharedString>, tone: theme::Token) -> Div {
    div()
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(color(tone))
        .child(text.into())
}

/// A titled setting: name, explanation, then its controls.
fn field(title: impl Into<SharedString>, help: Option<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .py(px(12.))
        .border_b_1()
        .border_color(color(tokens::LINE))
        .child(div().font_weight(FontWeight::MEDIUM).child(title.into()))
        .when_some(help, |d, h| d.child(small(h, tokens::MUTE)))
}

fn heading(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(22.))
        .line_height(px(28.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

fn whisper_model() -> PathBuf {
    super::models_dir().join(enfait_speech::MODEL_FILE)
}

impl Shell {
    fn save_key(&mut self, secret: Secret, value: Option<String>, cx: &mut Context<Self>) {
        let value = value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
        let removing = value.is_none();
        let work = cx.background_spawn(async move {
            secrets::set(secret, value.as_deref())
                .map(|_| secrets::source(secret))
                .map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |shell, cx| {
                match result {
                    Ok(source) => {
                        shell.settings.key_status.insert(secret, source);
                        shell.settings.clear.push(secret);
                        shell.settings.notice = Some((
                            if removing {
                                tr!("settings.key_removed", name = key_name(secret))
                            } else {
                                tr!("settings.key_saved", name = key_name(secret))
                            },
                            false,
                        ));
                    }
                    Err(e) => {
                        shell.settings.notice = Some((tr!("settings.key_failed", error = e), true));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn commit_local_model(&mut self, cx: &mut Context<Self>) {
        let url = self.settings.local_url.read(cx).value().trim().to_owned();
        let model = self.settings.local_model.read(cx).value().trim().to_owned();
        prefs::update(cx, |s| {
            s.local_url = if url.is_empty() {
                enfait_config::DEFAULT_LOCAL_URL.to_owned()
            } else {
                url
            };
            s.local_model = model;
        });
    }

    /// Downloads the transcription model or the renderer in the background.
    fn start_download(&mut self, tool: Tool, cx: &mut Context<Self>) {
        if self
            .settings
            .downloads
            .get(&tool)
            .is_some_and(|d| d.finished().is_none())
        {
            return;
        }
        let download = Download::new();
        self.settings.downloads.insert(tool, download.clone());
        let job = download.clone();
        let _ = std::thread::Builder::new()
            .name("enfait-download".into())
            .spawn(move || {
                let result = super::assets::block_on(async {
                    match tool {
                        Tool::Whisper => {
                            let client = enfait_assets::search::download_client()
                                .map_err(|e| e.to_string())?;
                            let progress = |done: u64, total: Option<u64>| {
                                job.done.store(done, Ordering::Relaxed);
                                job.total.store(total.unwrap_or(0), Ordering::Relaxed);
                            };
                            enfait_assets::search::download_large(
                                &client,
                                enfait_speech::MODEL_URL,
                                &whisper_model(),
                                &progress,
                            )
                            .await
                            .map_err(|e| e.to_string())
                        }
                        Tool::Browser => {
                            enfait_motion::ensure_browser(&enfait_config::paths::chromium_dir())
                                .await
                                .map(|_| ())
                                .map_err(|e| e.to_string())
                        }
                    }
                })
                .and_then(|r| r);
                if let Ok(mut slot) = job.result.lock() {
                    *slot = Some(result);
                }
            });
        // Refresh while it runs.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
                let done = download.finished().is_some();
                if this.update(cx, |_, cx| cx.notify()).is_err() || done {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// The settings page: sections on the left, the chosen one on the right.
    pub(super) fn settings_page(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Div {
        let current = self.settings.section;
        let content = self.section_content(current, cx);
        let nav = SECTIONS.iter().enumerate().map(|(i, &section)| {
            let on = section == current;
            div()
                .id(("settings-section", i))
                .px(px(12.))
                .py(px(6.))
                .rounded(px(3.))
                .cursor_pointer()
                .text_color(color(if on { tokens::FG } else { tokens::FG2 }))
                .when(on, |d| d.bg(color(tokens::RAISED)))
                .hover(|d| d.bg(color(tokens::HOVER)))
                .child(section.title())
                .on_click(cx.listener(move |shell, _: &ClickEvent, _, cx| {
                    shell.settings.section = section;
                    shell.settings.notice = None;
                    cx.notify();
                }))
        });
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .child(
                div()
                    .w(px(220.))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .p(px(12.))
                    .bg(color(tokens::PANEL))
                    .border_r_1()
                    .border_color(color(tokens::LINE))
                    .child(label(tr!("settings.title_caps")).px(px(12.)).pb(px(8.)))
                    .children(nav)
                    .child(div().flex_1())
                    .child(
                        chip("settings-close", tr!("settings.close"), false, true).on_click(
                            cx.listener(|shell, _: &ClickEvent, _, cx| {
                                shell.settings.open = false;
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .id("settings-content")
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_y_scroll()
                    .child(
                        div()
                            .max_w(px(760.))
                            .px(px(32.))
                            .py(px(24.))
                            .flex()
                            .flex_col()
                            .child(heading(current.title()))
                            .children(self.notice())
                            .child(content),
                    ),
            )
    }

    fn notice(&self) -> Option<Div> {
        let (text, error) = self.settings.notice.clone()?;
        Some(small(text, if error { tokens::ERROR } else { tokens::OK }).pt(px(8.)))
    }

    fn section_content(&self, section: Section, cx: &mut Context<Self>) -> Div {
        match section {
            Section::General => self.general_section(cx),
            Section::Appearance => self.appearance_section(cx),
            Section::Models => self.models_section(cx),
            Section::Banks => self.banks_section(cx),
            Section::Tools => self.tools_section(cx),
        }
    }

    fn language_field(&self, cx: &mut Context<Self>) -> Div {
        let current = prefs::settings(cx).language;
        let choices = Language::ALL.iter().enumerate().map(|(i, &language)| {
            chip(
                ("language", i),
                language.native_name(),
                language == current,
                true,
            )
            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                prefs::update(cx, |s| s.language = language);
            }))
        });
        field(tr!("settings.language"), None).child(div().flex().gap(px(6.)).children(choices))
    }

    fn close_field(&self, cx: &mut Context<Self>) -> Div {
        let current = prefs::settings(cx).close_action;
        let tray = crate::tray::available(cx);
        let choices = [
            (CloseAction::Tray, tr!("settings.close_tray")),
            (CloseAction::Quit, tr!("settings.close_quit")),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (action, name))| {
            chip(("close-action", i), name, action == current, true).on_click(cx.listener(
                move |_, _: &ClickEvent, _, cx| {
                    prefs::update(cx, |s| s.close_action = action);
                },
            ))
        });
        field(
            tr!("settings.close_window"),
            Some(tr!("settings.close_window_help")),
        )
        .child(div().flex().gap(px(6.)).children(choices))
        .when(!tray, |d| {
            d.child(small(tr!("settings.no_tray"), tokens::MIXED))
        })
    }

    fn notifications_field(&self, cx: &mut Context<Self>) -> Div {
        let on = prefs::settings(cx).notifications;
        field(
            tr!("settings.notifications"),
            Some(tr!("settings.notifications_help")),
        )
        .child(div().flex().child(
            chip("notifications", tr!("settings.notifications_on"), on, true).on_click(
                cx.listener(move |_, _: &ClickEvent, _, cx| {
                    prefs::update(cx, |s| s.notifications = !on);
                }),
            ),
        ))
    }

    fn general_section(&self, cx: &mut Context<Self>) -> Div {
        let session = self.session.studio_dir();
        let session = session.parent().map(PathBuf::from).unwrap_or(session);
        let shown = session.display().to_string();
        div()
            .flex()
            .flex_col()
            .child(self.language_field(cx))
            .child(self.close_field(cx))
            .child(self.notifications_field(cx))
            .child(
                field(tr!("settings.session"), Some(tr!("settings.session_help")))
                    .child(super::mono(shown).text_size(px(12.)))
                    .child(div().flex().gap(px(6.)).child(
                        chip("session-reveal", tr!("settings.show_folder"), false, true).on_click(
                            cx.listener(move |_, _: &ClickEvent, _, cx| {
                                cx.reveal_path(&session);
                            }),
                        ),
                    )),
            )
            .child(
                field(tr!("settings.onboarding"), None).child(
                    div().flex().child(
                        chip(
                            "onboarding-again",
                            tr!("settings.onboarding_again"),
                            false,
                            true,
                        )
                        .on_click(cx.listener(
                            |shell, _: &ClickEvent, _, cx| {
                                shell.settings.step = 0;
                                shell.settings.open = false;
                                prefs::update(cx, |s| s.onboarded = false);
                            },
                        )),
                    ),
                ),
            )
    }

    fn appearance_section(&self, cx: &mut Context<Self>) -> Div {
        let settings = prefs::settings(cx).clone();
        let appearance = prefs::appearance(cx);
        let cards = ThemeId::ALL.iter().enumerate().map(|(i, &theme_id)| {
            let on = theme_id == settings.theme;
            let swatch = theme::swatch(theme_id, settings.accent, appearance);
            div()
                .id(("theme", i))
                .w(px(150.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .p(px(6.))
                .rounded(px(4.))
                .cursor_pointer()
                .border_2()
                .border_color(color(if on { tokens::SIGNAL } else { tokens::LINE }))
                .hover(|d| d.bg(color(tokens::HOVER)))
                .child(
                    div()
                        .h(px(52.))
                        .rounded(px(3.))
                        .bg(swatch[0])
                        .p(px(6.))
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(div().h(px(8.)).w(px(70.)).rounded(px(2.)).bg(swatch[2]))
                        .child(div().h(px(8.)).w(px(46.)).rounded(px(2.)).bg(swatch[1]))
                        .child(div().h(px(10.)).w(px(30.)).rounded(px(2.)).bg(swatch[3])),
                )
                .child(div().text_size(px(12.)).child(theme_name(theme_id)))
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    prefs::update(cx, |s| s.theme = theme_id);
                }))
        });
        let accents = Accent::ALL.iter().enumerate().map(|(i, &accent)| {
            let on = accent == settings.accent;
            div()
                .id(("accent", i))
                .flex()
                .items_center()
                .gap(px(6.))
                .h(px(28.))
                .px(px(8.))
                .rounded(px(3.))
                .cursor_pointer()
                .border_1()
                .border_color(color(if on { tokens::FG2 } else { tokens::LINE_STRONG }))
                .when(on, |d| d.bg(color(tokens::RAISED)))
                .hover(|d| d.bg(color(tokens::HOVER)))
                .child(div().size(px(12.)).rounded_full().bg(theme::accent_color(
                    settings.theme,
                    accent,
                    appearance,
                )))
                .child(div().text_size(px(12.)).child(accent_name(accent)))
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    prefs::update(cx, |s| s.accent = accent);
                }))
        });
        div()
            .flex()
            .flex_col()
            .child(
                field(tr!("settings.theme"), Some(tr!("settings.theme_help")))
                    .child(div().flex().flex_wrap().gap(px(8.)).children(cards)),
            )
            .child(
                field(tr!("settings.accent"), Some(tr!("settings.accent_help")))
                    .child(div().flex().flex_wrap().gap(px(6.)).children(accents)),
            )
    }

    fn key_field(&self, secret: Secret, cx: &mut Context<Self>) -> Option<Div> {
        let input = self
            .settings
            .keys
            .iter()
            .find(|(s, _)| *s == secret)?
            .1
            .clone();
        let status = match self.settings.key_status.get(&secret) {
            None => (tr!("settings.key_status.checking"), tokens::MUTE),
            Some(None) => (tr!("settings.key_status.none"), tokens::MUTE),
            Some(Some(Source::Keychain)) => (tr!("settings.key_status.keychain"), tokens::OK),
            Some(Some(Source::File)) => (tr!("settings.key_status.file"), tokens::OK),
            Some(Some(Source::Env)) => (
                tr!("settings.key_status.env", var = secret.env_var()),
                tokens::OK,
            ),
        };
        let set = matches!(self.settings.key_status.get(&secret), Some(Some(_)));
        let page = key_page(secret);
        let save_input = input.clone();
        Some(
            field(key_name(secret), Some(key_help(secret)))
                .child(small(status.0, status.1))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(div().flex_1().min_w(px(0.)).child(Input::new(&input)))
                        .child(
                            chip(
                                ("key-save", secret as usize),
                                tr!("settings.save"),
                                false,
                                true,
                            )
                            .on_click(cx.listener(
                                move |shell, _: &ClickEvent, _, cx| {
                                    let value = save_input.read(cx).value().to_string();
                                    shell.save_key(secret, Some(value), cx);
                                },
                            )),
                        )
                        .when(set, |d| {
                            d.child(
                                chip(
                                    ("key-remove", secret as usize),
                                    tr!("settings.remove"),
                                    false,
                                    true,
                                )
                                .on_click(cx.listener(
                                    move |shell, _: &ClickEvent, _, cx| {
                                        shell.save_key(secret, None, cx);
                                    },
                                )),
                            )
                        }),
                )
                .when_some(page, |d, page| {
                    d.child(
                        div().flex().child(
                            chip(
                                ("key-page", secret as usize),
                                tr!("settings.get_key"),
                                false,
                                true,
                            )
                            .on_click(move |_, _, cx| cx.open_url(page)),
                        ),
                    )
                }),
        )
    }

    fn models_section(&self, cx: &mut Context<Self>) -> Div {
        let provider = self.session.data.provider;
        let quality = self.session.data.quality;
        let providers = ProviderChoice::ALL.iter().enumerate().map(|(i, &p)| {
            chip(
                ("settings-provider", i),
                provider_name(p),
                p == provider,
                true,
            )
            .on_click(cx.listener(move |shell, _: &ClickEvent, _, cx| shell.set_provider(p, cx)))
        });
        let qualities = QUALITIES.iter().enumerate().map(|(i, &q)| {
            chip(("settings-quality", i), quality_name(q), q == quality, true)
                .on_click(cx.listener(move |shell, _: &ClickEvent, _, cx| shell.set_quality(q, cx)))
        });
        let missing = super::studio::provider_hint(provider, prefs::settings(cx));
        let claude = enfait_config::process::which("claude");
        div()
            .flex()
            .flex_col()
            .child(
                field(
                    tr!("settings.provider"),
                    Some(tr!("settings.provider_help")),
                )
                .child(div().flex().flex_wrap().gap(px(6.)).children(providers))
                .when_some(missing, |d, m| d.child(small(m, tokens::MIXED))),
            )
            .child(
                field(tr!("settings.quality"), Some(tr!("settings.quality_help")))
                    .child(div().flex().gap(px(6.)).children(qualities)),
            )
            .children(self.key_field(Secret::Anthropic, cx))
            .child(
                field("Claude Code", Some(tr!("settings.claude_help"))).child(match claude {
                    Some(path) => small(
                        tr!("settings.found_at", path = path.display().to_string()),
                        tokens::OK,
                    ),
                    None => small(tr!("settings.claude_missing"), tokens::MUTE),
                }),
            )
            .child(
                field(tr!("settings.local"), Some(tr!("settings.local_help")))
                    .child(small(tr!("settings.local_url"), tokens::FG2))
                    .child(Input::new(&self.settings.local_url))
                    .child(small(tr!("settings.local_model"), tokens::FG2))
                    .child(Input::new(&self.settings.local_model)),
            )
            .children(self.key_field(Secret::Local, cx))
    }

    fn banks_section(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .child(small(tr!("settings.banks_help"), tokens::FG2).pt(px(8.)))
            .children(self.key_field(Secret::Pexels, cx))
            .children(self.key_field(Secret::Pixabay, cx))
            .children(self.key_field(Secret::Freesound, cx))
    }

    fn download_row(
        &self,
        tool: Tool,
        installed: Option<SharedString>,
        size: SharedString,
        cx: &mut Context<Self>,
    ) -> Div {
        let download = self.settings.downloads.get(&tool);
        let running = download.is_some_and(|d| d.finished().is_none());
        let failed = download.and_then(|d| match d.finished() {
            Some(Err(e)) => Some(e),
            _ => None,
        });
        let row = div().flex().flex_wrap().items_center().gap(px(8.));
        if running {
            let text = download.and_then(Download::fraction).map_or_else(
                || tr!("settings.tools.downloading"),
                |f| {
                    tr!(
                        "settings.tools.downloading_pct",
                        pct = format!("{:.0}", f * 100.0)
                    )
                },
            );
            return row.child(small(text, tokens::FG2));
        }
        let row = match installed {
            Some(text) => row.child(small(text, tokens::OK)),
            None => row
                .child(small(tr!("settings.tools.missing"), tokens::MIXED))
                .child(
                    chip(
                        ("download", tool as usize),
                        tr!("settings.tools.download", size = size),
                        false,
                        true,
                    )
                    .on_click(cx.listener(
                        move |shell, _: &ClickEvent, _, cx| {
                            shell.start_download(tool, cx);
                        },
                    )),
                ),
        };
        row.when_some(failed, |d, e| {
            d.child(small(
                tr!("settings.tools.failed", error = e),
                tokens::ERROR,
            ))
        })
    }

    fn tools_section(&self, cx: &mut Context<Self>) -> Div {
        let ffmpeg = enfait_config::process::which("ffmpeg");
        let ffprobe = enfait_config::process::which("ffprobe");
        let whisper = whisper_model();
        let whisper_ok = whisper.is_file().then(|| tr!("settings.tools.installed"));
        let browser = enfait_motion::installed_browser(&enfait_config::paths::chromium_dir())
            .map(|_| tr!("settings.tools.installed"));
        div()
            .flex()
            .flex_col()
            .child(
                field("ffmpeg", Some(tr!("settings.tools.ffmpeg_help"))).child(
                    match (ffmpeg, ffprobe) {
                        (Some(path), Some(_)) => small(
                            tr!("settings.found_at", path = path.display().to_string()),
                            tokens::OK,
                        ),
                        _ => small(ffmpeg_hint(), tokens::MIXED),
                    },
                ),
            )
            .child(
                field(
                    tr!("settings.tools.whisper"),
                    Some(tr!("settings.tools.whisper_help")),
                )
                .child(self.download_row(
                    Tool::Whisper,
                    whisper_ok,
                    "550 MB".into(),
                    cx,
                )),
            )
            .child(
                field(
                    tr!("settings.tools.browser"),
                    Some(tr!("settings.tools.browser_help")),
                )
                .child(self.download_row(
                    Tool::Browser,
                    browser,
                    "100 MB".into(),
                    cx,
                )),
            )
    }

    /// First launch: one step per screen, everything skippable.
    pub(super) fn onboarding(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let step = self.settings.step.min(STEPS - 1);
        let (title, help, content): (SharedString, SharedString, Div) = match step {
            0 => (
                tr!("onboarding.welcome"),
                tr!("onboarding.welcome_help"),
                div().flex().flex_col().child(self.language_field(cx)),
            ),
            1 => (
                Section::Appearance.title(),
                tr!("onboarding.appearance_help"),
                self.appearance_section(cx),
            ),
            2 => (
                Section::Models.title(),
                tr!("onboarding.models_help"),
                self.models_section(cx),
            ),
            3 => (
                Section::Banks.title(),
                tr!("onboarding.banks_help"),
                self.banks_section(cx),
            ),
            4 => (
                Section::Tools.title(),
                tr!("onboarding.tools_help"),
                self.tools_section(cx),
            ),
            _ => (
                tr!("onboarding.ready"),
                tr!("onboarding.ready_help", key = crate::shortcut(",")),
                div()
                    .flex()
                    .flex_col()
                    .child(self.close_field(cx))
                    .child(self.notifications_field(cx)),
            ),
        };
        let last = step == STEPS - 1;
        let dots = (0..STEPS).map(|i| {
            div()
                .h(px(3.))
                .flex_1()
                .rounded(px(2.))
                .bg(color(if i <= step {
                    tokens::SIGNAL
                } else {
                    tokens::LINE_STRONG
                }))
        });
        // A column: the card keeps its height and overflows into the scroll.
        div()
            .id("onboarding")
            .flex_1()
            .min_h(px(0.))
            .w_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .flex_none()
                    .w_full()
                    .max_w(px(760.))
                    .px(px(32.))
                    .py(px(40.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .pb(px(16.))
                            .child(div().size(px(10.)).bg(color(tokens::SIGNAL)))
                            .child(
                                div()
                                    .font_family(crate::fonts::CONDENSED)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("ENFAIT"),
                            )
                            .child(div().flex_1())
                            .child(
                                super::mono(format!("{}/{STEPS}", step.saturating_add(1)))
                                    .text_size(px(12.))
                                    .text_color(color(tokens::MUTE)),
                            ),
                    )
                    .child(div().flex().gap(px(4.)).pb(px(16.)).children(dots))
                    .child(heading(title))
                    .child(small(help, tokens::FG2))
                    .children(self.notice())
                    .child(content)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .pt(px(24.))
                            .when(step > 0, |d| {
                                d.child(
                                    chip("onboarding-back", tr!("onboarding.back"), false, true)
                                        .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| {
                                            shell.settings.step =
                                                shell.settings.step.saturating_sub(1);
                                            shell.settings.notice = None;
                                            cx.notify();
                                        })),
                                )
                            })
                            .child(div().flex_1())
                            .when(!last, |d| {
                                d.child(
                                    chip("onboarding-skip", tr!("onboarding.skip"), false, true)
                                        .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| {
                                            shell.finish_onboarding(cx);
                                        })),
                                )
                            })
                            .child(
                                chip(
                                    "onboarding-next",
                                    if last {
                                        tr!("onboarding.start")
                                    } else {
                                        tr!("onboarding.next")
                                    },
                                    true,
                                    true,
                                )
                                .cursor_pointer()
                                .on_click(cx.listener(
                                    move |shell, _: &ClickEvent, _, cx| {
                                        if last {
                                            shell.finish_onboarding(cx);
                                        } else {
                                            shell.settings.step = step.saturating_add(1);
                                            shell.settings.notice = None;
                                            cx.notify();
                                        }
                                    },
                                )),
                            ),
                    ),
            )
    }

    fn finish_onboarding(&mut self, cx: &mut Context<Self>) {
        self.settings.notice = None;
        self.settings.step = 0;
        prefs::update(cx, |s| s.onboarded = true);
        cx.notify();
    }
}
