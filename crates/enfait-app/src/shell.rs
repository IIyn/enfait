//! Main window: step bar, storyboard columns, timeline strip.
//!
//! Steps: Voice (script and prompter), Cut (takes and "last take" edit),
//! Storyboard and Finish. The take is imported with the Voice screen's
//! button, Ctrl/Cmd+I, `enfait <file>` or drag and drop. Settings and the
//! onboarding are drawn over the steps.

mod assets;
mod board;
mod bridge;
mod finish;
mod magic;
mod screens;
mod settings;
mod side;
mod studio;

use crate::cut::{CutState, CutStatus};
use crate::fonts;
use crate::jobs::JobQueue;
use crate::player::Player;
use crate::prefs::{self, Prefs};
use crate::prompter::{self, Prompter};
use crate::session::Session;
use crate::storyboard::Storyboard;
use crate::theme::{TEXT_SIZE, color, tokens};
use crate::tr;
use crate::worker::{self, Worker};
use enfait_core::{BlockKind, Flicks, FrameRate, ProjectSettings, Timecode};
use enfait_media::Command;
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::{
    Context, Div, Entity, ExternalPaths, FocusHandle, FontWeight, KeyBinding, PathPromptOptions,
    SharedString, Subscription, Window, actions, div, px, relative,
};
use std::path::PathBuf;
use std::sync::Arc;

actions!(
    player,
    [
        PlayForward,
        PlayBackward,
        Pause,
        TogglePlay,
        StepForward,
        StepBackward,
        JumpForward,
        JumpBackward,
        GoToStart,
        GoToEnd,
        ImportRush,
        OpenPrompter,
        StepVoice,
        StepCut,
        StepStoryboard,
        StepFinish,
        Undo,
        Redo,
        ToggleSidebar,
        OpenSettings,
        CloseOverlay,
    ]
);

/// Key context of the main window.
const KEY_CONTEXT: &str = "Shell";

pub fn key_bindings() -> Vec<KeyBinding> {
    // Single keys do not apply while typing text.
    let ctx = Some("Shell && !Input");
    let global = Some(KEY_CONTEXT);
    let mut keys = prompter::key_bindings();
    keys.extend([
        KeyBinding::new("l", PlayForward, ctx),
        KeyBinding::new("j", PlayBackward, ctx),
        KeyBinding::new("k", Pause, ctx),
        KeyBinding::new("space", TogglePlay, ctx),
        KeyBinding::new("right", StepForward, ctx),
        KeyBinding::new("left", StepBackward, ctx),
        KeyBinding::new("shift-right", JumpForward, ctx),
        KeyBinding::new("shift-left", JumpBackward, ctx),
        KeyBinding::new("home", GoToStart, ctx),
        KeyBinding::new("end", GoToEnd, ctx),
        KeyBinding::new(crate::primary!("z"), Undo, ctx),
        KeyBinding::new(crate::primary!("shift-z"), Redo, ctx),
        KeyBinding::new("ctrl-y", Redo, ctx),
        KeyBinding::new("escape", CloseOverlay, ctx),
        KeyBinding::new(crate::primary!("b"), ToggleSidebar, global),
        KeyBinding::new(crate::primary!("o"), ImportRush, global),
        KeyBinding::new(crate::primary!("i"), ImportRush, global),
        KeyBinding::new(crate::primary!(","), OpenSettings, global),
        KeyBinding::new("f5", OpenPrompter, global),
        KeyBinding::new("alt-1", StepVoice, global),
        KeyBinding::new("alt-2", StepCut, global),
        KeyBinding::new("alt-3", StepStoryboard, global),
        KeyBinding::new("alt-5", StepFinish, global),
    ]);
    keys
}

/// Models downloaded by the onboarding or `enfait-cli setup`.
fn models_dir() -> PathBuf {
    enfait_config::paths::models_dir()
}

/// Cache of prepared media (proxies, audio, waveforms).
fn media_cache() -> PathBuf {
    enfait_config::paths::media_cache()
}

const STEP_COUNT: usize = 5;
const STEP_VOICE: usize = 0;
const STEP_CUT: usize = 1;
const STEP_STORYBOARD: usize = 2;
const STEP_FINISH: usize = 4;

fn step_name(step: usize) -> SharedString {
    match step {
        STEP_VOICE => tr!("steps.voice"),
        STEP_CUT => tr!("steps.cut"),
        STEP_STORYBOARD => tr!("steps.storyboard"),
        STEP_FINISH => tr!("steps.finish"),
        _ => tr!("steps.retouch"),
    }
}

/// Window width below which the right panel folds.
const PANEL_MIN_WINDOW: f32 = 1180.;
/// Window width below which the left column folds.
const SIDEBAR_MIN_WINDOW: f32 = 860.;
/// Storyboard: left column only in a wide window.
const BOARD_SIDEBAR_MIN_WINDOW: f32 = 1440.;
/// Storyboard: below this width the inspector goes under the block list.
const INSPECTOR_MIN_WINDOW: f32 = 1000.;

pub struct Shell {
    /// Storyboard built from the edit (absent until there is a cut).
    board: Option<Storyboard>,
    /// Why there is no storyboard yet.
    board_notice: Option<SharedString>,
    /// The inspector fields must be copied from the selected block at the
    /// next render (changing them needs the window).
    needs_sync: bool,
    /// Inspector fields of the selected block.
    description: Entity<TextareaState>,
    transition_text: Entity<InputState>,
    moment_note: Entity<InputState>,
    /// Left column: style rules, post-FX sliders, note on the selected
    /// library file.
    style_rules: Entity<TextareaState>,
    fx_sliders: Vec<(
        side::FxField,
        Entity<gpui_kit::component::slider::SliderState>,
    )>,
    asset_note: Entity<InputState>,
    selected_asset: Option<String>,
    library_notice: Option<SharedString>,
    /// Royalty-free search (left column).
    search_input: Entity<InputState>,
    search: assets::SearchState,
    /// Library file being cut out.
    cutting: Option<String>,
    credits_copied: bool,
    /// Fingerprint of the b-roll shots sent to the engine: when it changes,
    /// the preview is rebuilt (and the shots edited again).
    broll_sig: std::cell::Cell<Option<blake3::Hash>>,
    /// Finish: history, selected music or SFX, sliders, audition.
    finish_history: enfait_core::FinishHistory,
    selected_cue: Option<enfait_core::CueId>,
    knobs: Vec<(
        finish::Knob,
        Entity<gpui_kit::component::slider::SliderState>,
    )>,
    audition: Option<finish::Audition>,
    finish_notice: Option<SharedString>,
    /// Export: keep the sound effects.
    export_sfx: bool,
    /// Bridge with agents (files changed by the MCP server).
    watch: bridge::Watch,
    /// Left column opened over the content (narrow window).
    sidebar_open: bool,
    /// Animations: job queue shared with the engine.
    queue: std::sync::Arc<std::sync::Mutex<JobQueue>>,
    worker: Worker,
    /// Retouch field and pending remarks for the selected block (time from
    /// the start of the block, remark).
    retouch: Entity<InputState>,
    retouches: Vec<(f64, String)>,
    /// Enter pressed in the retouch field: handled at the next render,
    /// where the window is available.
    pending_retouch: bool,
    /// Give the focus back to the window at the next render.
    refocus: bool,
    /// Remember the sent retouches as rules.
    remember: bool,
    /// Magic mode running (or its summary), and to start after the import.
    magic: Option<magic::MagicStage>,
    magic_after_import: bool,
    /// Summary of the last "Write the storyboard".
    write_notice: Option<SharedString>,
    /// The player shows the storyboard preview (and its date).
    showing_preview: bool,
    preview_stamp: Option<std::time::SystemTime>,
    player: Entity<Player>,
    focus: FocusHandle,
    step: usize,
    session: Session,
    script: Entity<TextareaState>,
    cut: CutState,
    prompter: Option<Entity<Prompter>>,
    /// Settings page and onboarding.
    settings: settings::SettingsUi,
    /// The language changed: placeholders are translated at the next render.
    relocalize: bool,
    _subscriptions: Vec<Subscription>,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>, media: Option<PathBuf>) -> Self {
        let player = cx.new(|_| Player::new(media_cache()));
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let session = Session::load(&Session::default_dir());
        let text = session.script.clone();
        let script = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(tr!("voice.script_placeholder"))
                .default_value(text)
        });
        let description = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder(tr!("board.description_placeholder"))
        });
        let transition_text = cx
            .new(|cx| InputState::new(window, cx).placeholder(tr!("board.transition_placeholder")));
        let moment_note =
            cx.new(|cx| InputState::new(window, cx).placeholder(tr!("board.moment_placeholder")));
        let rules = session.look.style.rules.clone();
        let style_rules = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(tr!("side.rules_placeholder"))
                .default_value(rules)
        });
        let asset_note =
            cx.new(|cx| InputState::new(window, cx).placeholder(tr!("assets.note_placeholder")));
        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(tr!("assets.search_placeholder")));
        let (fx_sliders, slider_subscriptions) = side::fx_sliders(&session.look.fx, cx);
        let (knobs, knob_subscriptions) = finish::knobs(&session.finish, cx);
        let fx = session.look.fx;
        player.update(cx, |p, cx| p.set_fx(fx, cx));
        let mut subscriptions = vec![
            cx.subscribe(
                &script,
                |shell: &mut Self, state, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        shell.session.script = state.read(cx).value().to_string();
                        shell.session.save_script();
                        cx.notify();
                    }
                },
            ),
            // Inspector fields are saved when leaving the field or on Enter:
            // one undo step per edit.
            cx.subscribe(
                &description,
                |shell: &mut Self, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Blur) {
                        shell.commit_description(cx);
                    }
                },
            ),
            cx.subscribe(
                &transition_text,
                |shell: &mut Self, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                        shell.commit_transition_text(cx);
                    }
                },
            ),
            cx.subscribe(
                &moment_note,
                |shell: &mut Self, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                        shell.commit_moment_note(cx);
                    }
                },
            ),
            cx.subscribe(
                &style_rules,
                |shell: &mut Self, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Blur) {
                        shell.commit_style_rules(cx);
                    }
                },
            ),
            cx.subscribe(
                &asset_note,
                |shell: &mut Self, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                        shell.commit_asset_note(cx);
                    }
                },
            ),
            cx.subscribe(
                &search_input,
                |shell: &mut Self, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        shell.run_search(cx);
                    }
                },
            ),
            // Language change: translate what was built once.
            cx.observe_global::<Prefs>(|shell: &mut Self, cx| {
                shell.relocalize = true;
                cx.notify();
            }),
        ];
        subscriptions.extend(slider_subscriptions);
        subscriptions.extend(knob_subscriptions);
        let retouch =
            cx.new(|cx| InputState::new(window, cx).placeholder(tr!("board.retouch_placeholder")));
        // Not bound to the window: the shell survives it when enfait keeps
        // running in the tray.
        subscriptions.push(cx.subscribe(
            &retouch,
            |shell: &mut Self, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    shell.pending_retouch = true;
                    cx.notify();
                }
            },
        ));
        let (mut settings, settings_subscriptions) = settings::SettingsUi::new(window, cx);
        settings.open_from_env();
        subscriptions.extend(settings_subscriptions);
        let studio_dir = session.studio_dir();
        let queue = std::sync::Arc::new(std::sync::Mutex::new(JobQueue::load(
            &studio_dir.join("jobs.json"),
        )));
        let worker = Worker::start(studio_dir, std::sync::Arc::clone(&queue));
        Self::start_polling(cx);
        let has_rush = session.data.rush.is_some();
        let board = Storyboard::load(&Session::default_dir());
        let mut shell = Self {
            needs_sync: board.is_some(),
            board,
            board_notice: None,
            description,
            transition_text,
            moment_note,
            style_rules,
            fx_sliders,
            asset_note,
            selected_asset: None,
            library_notice: None,
            search_input,
            search: assets::SearchState::default(),
            cutting: None,
            credits_copied: false,
            broll_sig: std::cell::Cell::new(None),
            finish_history: enfait_core::FinishHistory::default(),
            selected_cue: None,
            knobs,
            audition: None,
            finish_notice: None,
            export_sfx: true,
            watch: bridge::Watch::default(),
            sidebar_open: false,
            queue,
            worker,
            retouch,
            retouches: Vec::new(),
            pending_retouch: false,
            refocus: false,
            remember: false,
            magic: None,
            magic_after_import: false,
            write_notice: None,
            showing_preview: false,
            preview_stamp: None,
            player,
            focus,
            step: STEP_VOICE,
            session,
            script,
            cut: CutState::default(),
            prompter: None,
            settings,
            relocalize: false,
            _subscriptions: subscriptions,
        };
        match media {
            Some(path) => shell.import_rush(path, cx),
            None if has_rush => shell.go_to(STEP_CUT, cx),
            None => {}
        }
        shell
    }

    /// The window was reopened from the tray.
    pub fn attach_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
        self.relocalize = true;
        cx.notify();
    }

    /// Shows the settings page.
    pub fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.settings.open = true;
        self.command(Command::Pause, cx);
        cx.notify();
    }

    /// Translates the placeholders after a language change.
    fn translate_placeholders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let inputs = [
            (&self.transition_text, tr!("board.transition_placeholder")),
            (&self.moment_note, tr!("board.moment_placeholder")),
            (&self.asset_note, tr!("assets.note_placeholder")),
            (&self.search_input, tr!("assets.search_placeholder")),
            (&self.retouch, tr!("board.retouch_placeholder")),
        ];
        for (input, text) in inputs {
            input.update(cx, |s, cx| s.set_placeholder(text, window, cx));
        }
        let areas = [
            (&self.script, tr!("voice.script_placeholder")),
            (&self.description, tr!("board.description_placeholder")),
            (&self.style_rules, tr!("side.rules_placeholder")),
        ];
        for (area, text) in areas {
            area.update(cx, |s, cx| s.set_placeholder(text, window, cx));
        }
        self.settings.translate_placeholders(window, cx);
    }

    fn command(&mut self, command: Command, cx: &mut Context<Self>) {
        self.player.update(cx, |p, cx| p.command(command, cx));
    }

    fn go_to(&mut self, step: usize, cx: &mut Context<Self>) {
        if step != self.step {
            self.command(Command::Pause, cx);
        }
        self.settings.open = false;
        self.step = step;
        let preview = worker::preview_path(&self.session.studio_dir()).is_file();
        if step == STEP_CUT && self.showing_preview {
            // The cut works on the take, not on the preview.
            self.showing_preview = false;
            self.preview_stamp = None;
        }
        let montage_step = step == STEP_STORYBOARD || step == STEP_FINISH;
        if step == STEP_CUT || montage_step {
            if !(montage_step && preview) {
                self.open_rush(cx);
            }
            self.ensure_transcript(cx);
            self.refresh_cut(cx);
        }
        if montage_step {
            // The storyboard and the finish always follow the edit.
            if !self.cut.montage {
                self.set_montage(true, cx);
            }
            self.refresh_board(cx);
            self.push_snapshot();
            self.refresh_preview(cx);
        }
        cx.notify();
    }

    /// Imports a take: it replaces the previous one and the cut step opens.
    fn import_rush(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let path = std::path::absolute(&path).unwrap_or(path);
        self.session.data.rush = Some(path);
        self.session.data.choices.clear();
        self.session.save_data();
        self.cut = CutState::default();
        self.go_to(STEP_CUT, cx);
        if std::mem::take(&mut self.magic_after_import) {
            self.start_magic(cx);
        }
    }

    /// Opens the take in the player unless it already is.
    fn open_rush(&mut self, cx: &mut Context<Self>) {
        let Some(rush) = self.session.data.rush.clone() else {
            return;
        };
        if !self.player.read(cx).is_open(&rush) {
            self.player.update(cx, |p, cx| p.open(rush, cx));
        }
    }

    fn prompt_import(&mut self, cx: &mut Context<Self>) {
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
                let _ = this.update(cx, |shell, cx| shell.import_rush(path, cx));
            }
        })
        .detach();
    }

    fn open_prompter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.command(Command::Pause, cx);
        let paragraphs = enfait_speech::script::spoken_paragraphs(&self.session.script);
        let prompter = cx.new(|cx| Prompter::new(paragraphs, window, cx));
        cx.subscribe(&prompter, |shell, _, _: &prompter::Closed, cx| {
            shell.prompter = None;
            shell.refocus = true;
            cx.notify();
        })
        .detach();
        self.prompter = Some(prompter);
        cx.notify();
    }

    /// Loads the transcript of the take (cache) or starts it in the
    /// background.
    fn ensure_transcript(&mut self, cx: &mut Context<Self>) {
        let Some(rush) = self.session.data.rush.clone() else {
            self.cut.status = CutStatus::NoRush;
            return;
        };
        if self.cut.transcript.is_some() || self.cut.status == CutStatus::Transcribing {
            return;
        }
        let dir = match enfait_media::MediaCache::new(media_cache()).dir_for(&rush) {
            Ok(dir) => dir,
            Err(e) => {
                self.cut.status =
                    CutStatus::Failed(tr!("cut.unreadable_take", error = e.to_string()));
                return;
            }
        };
        let cached = dir.join("transcript.json");
        if let Some(t) = std::fs::read(&cached)
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
        {
            self.cut.transcript = Some(Arc::new(t));
            self.cut.status = CutStatus::Ready;
            return;
        }
        let model = models_dir().join(enfait_speech::MODEL_FILE);
        if !model.is_file() {
            self.cut.status = CutStatus::Failed(tr!("cut.no_model"));
            return;
        }
        self.cut.status = CutStatus::Transcribing;
        let source = rush.clone();
        let work = cx.background_spawn(async move {
            let pcm = enfait_speech::decode_pcm(&source).map_err(|e| e.to_string())?;
            let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
            let options = enfait_speech::TranscribeOptions {
                language: None,
                // No script as prompt: it pushes whisper to merge the
                // retakes into one.
                prompt: None,
                threads: u16::try_from(threads).unwrap_or(4),
                takes: true,
            };
            let transcript =
                enfait_speech::transcribe(&model, &pcm, &options).map_err(|e| e.to_string())?;
            if let Ok(json) = serde_json::to_vec_pretty(&transcript) {
                let _ = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&cached, json));
            }
            Ok::<_, String>(transcript)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |shell, cx| {
                if shell.session.data.rush.as_deref() != Some(rush.as_path()) {
                    return;
                }
                match result {
                    Ok(t) => {
                        shell.cut.transcript = Some(Arc::new(t));
                        shell.cut.status = CutStatus::Ready;
                        shell.refresh_cut(cx);
                    }
                    Err(e) => {
                        shell.cut.status =
                            CutStatus::Failed(tr!("cut.transcription_failed", error = e));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Analyses again if the script changed, then applies the edit.
    fn refresh_cut(&mut self, cx: &mut Context<Self>) {
        if self.cut.transcript.is_none() {
            return;
        }
        if self.cut.analyzed.as_deref() != Some(self.session.script.as_str()) {
            self.cut
                .analyze(&self.session.script, &self.session.data.choices);
        }
        self.apply_cut(cx);
        if self.step == STEP_STORYBOARD || self.step == STEP_FINISH {
            self.refresh_board(cx);
        }
    }

    /// Sends the edit (or the whole take) to the player.
    fn apply_cut(&mut self, cx: &mut Context<Self>) {
        if self.showing_preview {
            return;
        }
        let ranges = self.cut.ranges();
        let ranges = (self.cut.montage && !ranges.is_empty()).then_some(ranges);
        self.player.update(cx, |p, cx| p.set_cut(ranges, cx));
    }

    /// The sentences found in the take become the script (Voice screen),
    /// to be corrected by hand.
    fn script_from_take(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.cut.sentences_text();
        if text.is_empty() {
            return;
        }
        // The new value goes through the field's subscription, which saves
        // the script; the analysis follows at the next refresh.
        self.script
            .update(cx, |s, cx| s.set_value(text.clone(), window, cx));
        self.session.script = text;
        self.session.save_script();
        self.refresh_cut(cx);
        cx.notify();
    }

    fn choose_take(&mut self, sentence: usize, take: usize, cx: &mut Context<Self>) {
        self.cut
            .choose(sentence, take, &mut self.session.data.choices);
        self.session.save_data();
        self.apply_cut(cx);
        self.seek_sentence(sentence, cx);
    }

    fn reset_choices(&mut self, cx: &mut Context<Self>) {
        self.cut.reset(&mut self.session.data.choices);
        self.session.save_data();
        self.apply_cut(cx);
        cx.notify();
    }

    fn set_montage(&mut self, montage: bool, cx: &mut Context<Self>) {
        self.cut.montage = montage;
        self.apply_cut(cx);
        cx.notify();
    }

    /// Moves the playhead to the start of the take kept for a sentence.
    fn seek_sentence(&mut self, sentence: usize, cx: &mut Context<Self>) {
        let Some(source) = self.cut.sentence_start(sentence) else {
            return;
        };
        let Some(edl) = self.player.read(cx).edl() else {
            return;
        };
        if let Some(t) = edl.to_output(source + 1e-3) {
            self.command(Command::Seek((t - 0.05).max(0.0)), cx);
        }
    }

    /// `compact`: narrow window, only the current step is shown.
    /// `toggle`: the left column is folded, a button opens it.
    fn header(
        &self,
        compact: bool,
        toggle: bool,
        timecode: Option<String>,
        cx: &mut Context<Self>,
    ) -> Div {
        let open = self.sidebar_open;
        let current = self.step;
        let settings_open = self.settings.open;
        let steps = (0..STEP_COUNT)
            .filter(move |i| !compact || *i == current)
            .map(|i| {
                let active = i == current && !settings_open;
                let available = i <= STEP_STORYBOARD || i == STEP_FINISH;
                div()
                    .id(("step", i))
                    .when(available && !active, |d| {
                        d.cursor_pointer()
                            .hover(|d| d.text_color(color(tokens::FG2)))
                            .on_click(cx.listener(move |shell, _, _, cx| shell.go_to(i, cx)))
                    })
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h_full()
                    .px(px(10.))
                    .text_color(color(if active {
                        tokens::FG
                    } else if available {
                        tokens::MUTE
                    } else {
                        tokens::LINE_STRONG
                    }))
                    .when(active, |d| {
                        d.border_b_2().border_color(color(tokens::SIGNAL))
                    })
                    .child(mono(format!("{}", i.saturating_add(1))))
                    .child(step_name(i))
            });

        div()
            .flex()
            .items_center()
            .gap(px(16.))
            .h(px(44.))
            .px(px(16.))
            .bg(color(tokens::PANEL))
            .border_b_1()
            .border_color(color(tokens::LINE))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().size(px(10.)).bg(color(tokens::SIGNAL)))
                    .child(
                        div()
                            .font_family(fonts::CONDENSED)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("ENFAIT"),
                    ),
            )
            .when(toggle, |d| {
                d.child(
                    div()
                        .id("toggle-sidebar")
                        .px(px(8.))
                        .h(px(24.))
                        .flex()
                        .items_center()
                        .rounded(px(3.))
                        .cursor_pointer()
                        .text_size(px(12.))
                        .map(|d| {
                            if open {
                                d.bg(color(tokens::SIGNAL))
                                    .text_color(color(tokens::ON_SIGNAL))
                            } else {
                                d.border_1()
                                    .border_color(color(tokens::LINE_STRONG))
                                    .text_color(color(tokens::FG2))
                                    .hover(|d| d.bg(color(tokens::HOVER)))
                            }
                        })
                        .child(tr!("side.style_toggle"))
                        .on_click(cx.listener(|shell, _: &gpui_kit::ClickEvent, _, cx| {
                            shell.sidebar_open = !shell.sidebar_open;
                            cx.notify();
                        })),
                )
            })
            .child(div().text_color(color(tokens::LINE_STRONG)).child("/"))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child(SharedString::from(self.title())),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .ml(px(16.))
                    .overflow_hidden()
                    .children(steps),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        mono(timecode.unwrap_or_else(|| "00:00:00:00".to_owned()))
                            .text_size(px(15.)),
                    )
                    .when(!compact, |d| {
                        d.child(
                            mono(settings_label(&self.settings()))
                                .text_size(px(12.))
                                .text_color(color(tokens::MUTE)),
                        )
                    })
                    .child(
                        div()
                            .id("open-settings")
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.))
                            .cursor_pointer()
                            .text_color(color(if settings_open {
                                tokens::SIGNAL
                            } else {
                                tokens::FG2
                            }))
                            .hover(|d| d.bg(color(tokens::HOVER)))
                            .child(gpui_kit::component::IconName::Settings)
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(tr!("settings.title"))
                                    .build(window, cx)
                            })
                            .on_click(cx.listener(|shell, _: &gpui_kit::ClickEvent, _, cx| {
                                if shell.settings.open {
                                    shell.settings.open = false;
                                    cx.notify();
                                } else {
                                    shell.open_settings(cx);
                                }
                            })),
                    ),
            )
    }

    /// `playing`: source time being played.
    fn center_with(&mut self, playing: Option<f64>, cx: &mut Context<Self>) -> Div {
        match self.step {
            STEP_VOICE => self.voice_screen(cx),
            _ => self.cut_screen(playing, cx),
        }
    }

    /// Bottom strip: waveform of the player, then the storyboard blocks
    /// (click to select).
    fn timeline(&self, cx: &mut Context<Self>) -> Div {
        let strip = self.player.update(cx, |p, cx| p.strip(cx));
        let selected = self.board.as_ref().and_then(|b| b.selected);
        let (blocks, total) = self.board.as_ref().map_or((Vec::new(), 1.0), |b| {
            (
                b.project
                    .blocks()
                    .iter()
                    .map(|block| (block.id, block.kind, block.range.duration().as_secs_f64()))
                    .collect(),
                b.project.duration().as_secs_f64().max(1e-3),
            )
        });
        #[allow(clippy::cast_possible_truncation)]
        let clips = blocks
            .into_iter()
            .enumerate()
            .map(|(i, (id, kind, duration))| {
                div()
                    .id(("clip", i))
                    .w(relative((duration / total) as f32))
                    .h_full()
                    .pr(px(2.))
                    .cursor_pointer()
                    .on_click(
                        cx.listener(move |shell, _: &gpui_kit::ClickEvent, window, cx| {
                            shell.select_block(Some(id), true, window, cx);
                        }),
                    )
                    .child(
                        div()
                            .size_full()
                            .rounded(px(2.))
                            .bg(color(kind_color(kind)))
                            .when(selected == Some(id), |d| {
                                d.border_2().border_color(color(tokens::FG))
                            }),
                    )
            });
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(16.))
            .py(px(10.))
            .bg(color(tokens::PANEL))
            .border_t_1()
            .border_color(color(tokens::LINE))
            .child(strip)
            .child(div().h(px(22.)).flex().children(clips))
    }

    /// Escape: closes the settings page, then the left column.
    fn close_overlay(&mut self, cx: &mut Context<Self>) {
        if self.settings.open {
            self.settings.open = false;
        } else if self.sidebar_open {
            self.sidebar_open = false;
        }
        cx.notify();
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.relocalize) {
            self.translate_placeholders(window, cx);
        }
        if std::mem::take(&mut self.refocus) {
            self.focus.focus(window, cx);
        }
        if std::mem::take(&mut self.pending_retouch) {
            self.add_retouch(window, cx);
        }
        if self.needs_sync {
            self.sync_inputs(window, cx);
        }
        self.settings.sync_inputs(window, cx);

        let root = div()
            .size_full()
            .track_focus(&self.focus)
            .key_context(KEY_CONTEXT)
            .bg(color(tokens::BG))
            .text_color(color(tokens::FG))
            .font_family(fonts::SANS)
            .text_size(px(TEXT_SIZE))
            .line_height(px(20.))
            .relative()
            .flex()
            .flex_col();

        // First launch: the onboarding takes the whole window.
        if !prefs::settings(cx).onboarded {
            return root.child(self.onboarding(window, cx));
        }

        // The playhead and the timecode move during playback.
        let player = self.player.read(cx);
        if player.is_playing() {
            window.request_animation_frame();
        }
        let timecode = player.timecode();
        let playing_source = player.source_position();
        let playing_output = player.position();

        // Side columns fold in a narrow window (tiling window manager,
        // small screen).
        let width = window.viewport_size().width;
        let show_panel = width >= px(PANEL_MIN_WINDOW);
        let board_step = self.step == STEP_STORYBOARD || self.step == STEP_FINISH;
        // On the storyboard the inspector wins over the left column: without
        // it nothing can be edited.
        let show_sidebar = if board_step {
            width >= px(BOARD_SIDEBAR_MIN_WINDOW)
        } else {
            width >= px(SIDEBAR_MIN_WINDOW)
        };
        // Wide window: inspector on the right; narrow: under the list.
        let side_inspector = width >= px(INSPECTOR_MIN_WINDOW);

        let root = root
            .on_action(cx.listener(|s, _: &PlayForward, _, cx| s.command(Command::Forward, cx)))
            .on_action(cx.listener(|s, _: &PlayBackward, _, cx| s.command(Command::Backward, cx)))
            .on_action(cx.listener(|s, _: &Pause, _, cx| s.command(Command::Pause, cx)))
            .on_action(cx.listener(|s, _: &TogglePlay, _, cx| s.command(Command::Toggle, cx)))
            .on_action(cx.listener(|s, _: &StepForward, _, cx| s.command(Command::Step(1), cx)))
            .on_action(cx.listener(|s, _: &StepBackward, _, cx| s.command(Command::Step(-1), cx)))
            // Steps of ±2 are turned into one second by the player.
            .on_action(cx.listener(|s, _: &JumpForward, _, cx| s.command(Command::Step(2), cx)))
            .on_action(cx.listener(|s, _: &JumpBackward, _, cx| s.command(Command::Step(-2), cx)))
            .on_action(cx.listener(|s, _: &GoToStart, _, cx| s.command(Command::Seek(0.0), cx)))
            .on_action(cx.listener(|s, _: &GoToEnd, _, cx| {
                s.command(Command::Seek(f64::INFINITY), cx);
            }))
            .on_action(cx.listener(|s, _: &ImportRush, _, cx| s.prompt_import(cx)))
            .on_action(cx.listener(|s, _: &OpenPrompter, window, cx| s.open_prompter(window, cx)))
            .on_action(cx.listener(|s, _: &StepVoice, _, cx| s.go_to(STEP_VOICE, cx)))
            .on_action(cx.listener(|s, _: &StepCut, _, cx| s.go_to(STEP_CUT, cx)))
            .on_action(cx.listener(|s, _: &StepStoryboard, _, cx| s.go_to(STEP_STORYBOARD, cx)))
            .on_action(cx.listener(|s, _: &StepFinish, _, cx| s.go_to(STEP_FINISH, cx)))
            .on_action(cx.listener(|s, _: &OpenSettings, _, cx| s.open_settings(cx)))
            .on_action(cx.listener(|s, _: &CloseOverlay, _, cx| s.close_overlay(cx)))
            .on_action(cx.listener(|s, _: &Undo, window, cx| {
                if s.step == STEP_FINISH {
                    s.finish_undo(window, cx);
                } else {
                    s.undo(window, cx);
                }
            }))
            .on_action(cx.listener(|s, _: &ToggleSidebar, _, cx| {
                s.sidebar_open = !s.sidebar_open;
                cx.notify();
            }))
            .on_action(cx.listener(|s, _: &Redo, window, cx| {
                if s.step == STEP_FINISH {
                    s.finish_redo(window, cx);
                } else {
                    s.redo(window, cx);
                }
            }))
            .on_drop(cx.listener(|s, paths: &ExternalPaths, _, cx| {
                if let Some(path) = paths.paths().first() {
                    s.import_rush(path.clone(), cx);
                }
            }))
            .child(self.header(!show_panel, !show_sidebar, timecode, cx))
            .children(self.magic_banner(cx));

        if self.settings.open {
            return root.child(self.settings_page(window, cx));
        }

        let inspector = match self.step {
            STEP_STORYBOARD => Some(self.inspector(side_inspector, cx)),
            STEP_FINISH => Some(self.finish_inspector(side_inspector, cx)),
            _ => None,
        };
        let sidebar = (show_sidebar || self.sidebar_open).then(|| self.sidebar(cx));
        let (sidebar, overlay) = if show_sidebar {
            (sidebar, None)
        } else {
            (None, sidebar)
        };
        let (inspector, below) = if side_inspector {
            (inspector, None)
        } else {
            (None, inspector)
        };
        let center = if self.step == STEP_STORYBOARD {
            self.board_screen(playing_output, below, cx)
        } else if self.step == STEP_FINISH {
            let screen = self.finish_screen(cx);
            match below {
                Some(inspector) => screen.child(
                    inspector
                        .h(relative(0.45))
                        .border_t_1()
                        .border_color(color(tokens::LINE_STRONG)),
                ),
                None => screen,
            }
        } else {
            self.center_with(playing_source, cx)
        };

        root.child(
            div()
                .flex_1()
                .min_h(px(0.))
                .flex()
                .when_some(sidebar, |d, s| d.child(s))
                .child(center)
                .when_some(inspector, |d, i| d.child(i)),
        )
        .child(self.timeline(cx))
        .when_some(overlay, |d, s| {
            d.child(
                s.absolute()
                    .top(px(44.))
                    .bottom_0()
                    .left_0()
                    .shadow_lg()
                    .border_r_1()
                    .border_color(color(tokens::LINE_STRONG)),
            )
        })
        .when_some(self.prompter.clone(), |d, p| d.child(p))
    }
}

fn label(text: impl Into<SharedString>) -> Div {
    div()
        .font_family(fonts::CONDENSED)
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(px(11.))
        .line_height(px(14.))
        .text_color(color(tokens::MUTE))
        .child(text.into())
}

fn mono(text: impl Into<SharedString>) -> Div {
    div().font_family(fonts::MONO).child(text.into())
}

fn kind_color(kind: BlockKind) -> crate::theme::Token {
    match kind {
        BlockKind::Rush => tokens::RUSH,
        BlockKind::Animation => tokens::ANIMATION,
        BlockKind::Mixed => tokens::MIXED,
        BlockKind::Effect => tokens::EFFECT,
        BlockKind::Broll => tokens::BROLL,
    }
}

fn kind_label(kind: BlockKind) -> SharedString {
    match kind {
        BlockKind::Rush => tr!("kinds.rush"),
        BlockKind::Animation => tr!("kinds.animation"),
        BlockKind::Mixed => tr!("kinds.mixed"),
        BlockKind::Effect => tr!("kinds.effect"),
        BlockKind::Broll => tr!("kinds.broll"),
    }
}

/// `MM:SS`, or the full timecode if the conversion fails (time out of range).
fn short_time(t: Flicks, rate: FrameRate) -> String {
    match Timecode::from_flicks(t, rate) {
        Ok(tc) if tc.hours() == 0 => format!("{:02}:{:02}", tc.minutes(), tc.seconds()),
        Ok(tc) => tc.to_string(),
        Err(_) => "--:--".to_owned(),
    }
}

fn settings_label(settings: &ProjectSettings) -> String {
    let rate = settings.frame_rate;
    let fps = if rate.den() == 1 {
        rate.num().to_string()
    } else {
        format!("{:.2}", f64::from(rate.num()) / f64::from(rate.den()))
    };
    format!(
        "{}×{} · {}",
        settings.resolution.width,
        settings.resolution.height,
        tr!("shell.fps", fps = fps)
    )
}
