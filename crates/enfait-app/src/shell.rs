//! Main window: home screen (recent projects, new project), then the
//! steps of the open project, storyboard columns, timeline strip.
//!
//! Steps: Script, Voice (recording or import), Cut (takes and "last take"
//! edit), Storyboard (blocks, style, library), Montage (blocks sent to the
//! engine), Retouch and Finish. Which ones show depends on how the project
//! started (see [`Start`]). Settings and the onboarding are drawn over the
//! steps.

mod assets;
mod board;
mod bridge;
mod finish;
mod home;
mod lazy;
mod magic;
mod mention;
mod montage;
mod record;
mod retouch;
mod screens;
mod settings;
mod side;
mod split;
mod studio;

use crate::cut::{CutState, CutStatus};
use crate::fonts;
use crate::jobs::JobQueue;
use crate::player::Player;
use crate::prefs::{self, Prefs};
use crate::prompter::{self, Prompter};
use crate::session::{Session, Start};
use crate::storyboard::Storyboard;
use crate::theme::{TEXT_SIZE, color, tokens};
use crate::tr;
use crate::worker::{self, Worker};
use enfait_core::{BlockKind, Flicks, FrameRate, ProjectSettings, Timecode};
use enfait_media::Command;
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::{
    Context, Div, DragMoveEvent, Entity, ExternalPaths, FocusHandle, FontWeight, KeyBinding,
    PathPromptOptions, SharedString, Subscription, Window, actions, div, px, relative,
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
        Step1,
        Step2,
        Step3,
        Step4,
        Step5,
        Step6,
        Step7,
        GoHome,
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
        KeyBinding::new("alt-1", Step1, global),
        KeyBinding::new("alt-2", Step2, global),
        KeyBinding::new("alt-3", Step3, global),
        KeyBinding::new("alt-4", Step4, global),
        KeyBinding::new("alt-5", Step5, global),
        KeyBinding::new("alt-6", Step6, global),
        KeyBinding::new("alt-7", Step7, global),
        KeyBinding::new(crate::primary!("shift-h"), GoHome, global),
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

const STEP_SCRIPT: usize = 0;
const STEP_VOICE: usize = 1;
const STEP_CUT: usize = 2;
const STEP_STORYBOARD: usize = 3;
const STEP_MONTAGE: usize = 4;
const STEP_RETOUCH: usize = 5;
const STEP_FINISH: usize = 6;

fn step_name(step: usize) -> SharedString {
    match step {
        STEP_SCRIPT => tr!("steps.script"),
        STEP_VOICE => tr!("steps.voice"),
        STEP_CUT => tr!("steps.cut"),
        STEP_STORYBOARD => tr!("steps.storyboard"),
        STEP_MONTAGE => tr!("steps.montage"),
        STEP_RETOUCH => tr!("steps.retouch"),
        _ => tr!("steps.finish"),
    }
}

/// Steps that work on the storyboard (and show the preview).
const fn board_step(step: usize) -> bool {
    matches!(
        step,
        STEP_STORYBOARD | STEP_MONTAGE | STEP_RETOUCH | STEP_FINISH
    )
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
    /// Home screen (recent projects, new project), shown instead of the
    /// project.
    home: Option<home::Home>,
    /// Project list (`projects.json`).
    projects: enfait_config::projects::Projects,
    /// New project form: title, duration and subject of a silent video.
    new_title: Entity<InputState>,
    new_duration: Entity<InputState>,
    new_brief: Entity<TextareaState>,
    /// A project was created: the form is emptied at the next render.
    reset_wizard_inputs: bool,
    /// The project changed: the text fields and sliders are refilled at the
    /// next render (it needs the window).
    reload_inputs: bool,
    /// Short messages in a corner of the window (block ready…).
    toasts: Vec<montage::Toast>,
    /// Blocks finished since the Montage step was last seen.
    montage_badge: usize,
    /// "Send to the edit" dialog (care level), when open.
    care: Option<montage::CareDialog>,
    /// `@` typed in a text field: files of the library to mention.
    mention: Option<mention::Mention>,
    /// Voice being recorded.
    recording: Option<record::Recording>,
    /// Camera starting, or filmed take being written.
    recording_busy: Option<record::Busy>,
    /// Microphones and cameras offered on the Voice screen (listed when
    /// it opens, and on demand).
    devices: Option<record::Devices>,
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
    /// Styles saved by the user, shared by every project.
    saved_styles: Vec<enfait_assets::styles::Stored>,
    /// Name of the style being saved; shown while `naming_style`.
    style_name: Entity<InputState>,
    naming_style: bool,
    /// The style fields (rules, effect sliders) must be copied from the
    /// look at the next render.
    reload_look_inputs: bool,
    /// Saved style whose deletion awaits a confirmation.
    confirm_style_delete: Option<String>,
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
    /// Scroll states of the long lists.
    lists: lazy::Lists,
    /// Clips on disk, listed at most twice a second.
    clips: std::cell::RefCell<lazy::ClipIndex>,
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
    /// Panel sizes while a handle is dragged (saved when it is dropped).
    panels: Option<enfait_config::PanelSizes>,
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
        let native = prefs::settings(cx).player_native;
        let player = cx.new(|_| Player::new(media_cache(), native));
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let mut projects = enfait_config::projects::Projects::load();
        // `ENFAIT_SESSION` pins the project; otherwise the last one opened.
        let pinned = std::env::var_os("ENFAIT_SESSION").map(PathBuf::from);
        let dir = pinned
            .clone()
            .or_else(|| projects.current.clone().filter(|d| d.is_dir()))
            .unwrap_or_else(Session::default_dir);
        if pinned.is_some() {
            projects.touch(&dir);
            let _ = projects.save();
        }
        let session = Session::load(&dir);
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
        let style_name = cx
            .new(|cx| InputState::new(window, cx).placeholder(tr!("side.style_name_placeholder")));
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
                |shell: &mut Self, state, event: &InputEvent, cx| match event {
                    InputEvent::Blur => shell.commit_description(cx),
                    InputEvent::Change => {
                        let (text, cursor) = {
                            let s = state.read(cx);
                            (s.value().to_string(), s.cursor())
                        };
                        shell.update_mention(mention::Field::Description, &text, cursor, cx);
                    }
                    _ => {}
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
                |shell: &mut Self, state, event: &InputEvent, cx| match event {
                    InputEvent::Blur => shell.commit_style_rules(cx),
                    InputEvent::Change => {
                        let (text, cursor) = {
                            let s = state.read(cx);
                            (s.value().to_string(), s.cursor())
                        };
                        shell.update_mention(mention::Field::Style, &text, cursor, cx);
                    }
                    _ => {}
                },
            ),
            cx.subscribe(
                &style_name,
                |shell: &mut Self, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        shell.save_style(cx);
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
        let board = Storyboard::load(session.dir());
        let new_title =
            cx.new(|cx| InputState::new(window, cx).placeholder(tr!("home.title_placeholder")));
        let new_duration = cx.new(|cx| InputState::new(window, cx).default_value("20"));
        let new_brief =
            cx.new(|cx| TextareaState::new(window, cx).placeholder(tr!("home.brief_placeholder")));
        let open_home = pinned.is_none() && media.is_none();
        let mut shell = Self {
            home: open_home.then(|| home::Home::new(&projects)),
            projects,
            new_title,
            new_duration,
            new_brief,
            reset_wizard_inputs: false,
            reload_inputs: false,
            toasts: Vec::new(),
            montage_badge: 0,
            care: None,
            mention: None,
            recording: None,
            recording_busy: None,
            devices: None,
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
            saved_styles: enfait_assets::styles::list(&enfait_assets::styles::default_root()),
            style_name,
            naming_style: false,
            reload_look_inputs: false,
            confirm_style_delete: None,
            library_notice: None,
            search_input,
            search: assets::SearchState::default(),
            cutting: None,
            credits_copied: false,
            broll_sig: std::cell::Cell::new(None),
            lists: lazy::Lists::default(),
            clips: std::cell::RefCell::default(),
            finish_history: enfait_core::FinishHistory::default(),
            selected_cue: None,
            knobs,
            audition: None,
            finish_notice: None,
            export_sfx: true,
            watch: bridge::Watch::default(),
            sidebar_open: false,
            panels: None,
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
            // No step yet: the first one depends on the project.
            step: usize::MAX,
            session,
            script,
            cut: CutState::default(),
            prompter: None,
            settings,
            relocalize: false,
            _subscriptions: subscriptions,
        };
        match media {
            // `enfait <file>`: a new project on that take.
            Some(path) => shell.create_from_file(path, cx),
            None if !open_home => {
                let step = shell.first_step();
                shell.go_to(step, cx);
            }
            None => {}
        }
        shell.open_screen_from_env(cx);
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
        if self.settings.update_unchecked() {
            self.check_update(cx);
        }
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
            (&self.style_name, tr!("side.style_name_placeholder")),
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

    /// Steps of this project, in order.
    fn visible_steps(&self) -> Vec<usize> {
        let data = &self.session.data;
        let mut steps = Vec::with_capacity(7);
        match data.start {
            Start::Silent => {}
            Start::Script => steps.extend([STEP_SCRIPT, STEP_VOICE]),
            Start::Import => {
                // The script appears once there is one (written from the cut).
                if !self.session.script.trim().is_empty() {
                    steps.push(STEP_SCRIPT);
                }
                steps.push(STEP_VOICE);
            }
        }
        if data.start != Start::Silent && data.derush {
            steps.push(STEP_CUT);
        }
        steps.extend([STEP_STORYBOARD, STEP_MONTAGE, STEP_RETOUCH, STEP_FINISH]);
        steps
    }

    /// Where a project opens.
    fn first_step(&self) -> usize {
        let data = &self.session.data;
        if self.board.is_some() || data.start == Start::Silent {
            return STEP_STORYBOARD;
        }
        match (data.start, data.rush.is_some()) {
            (_, true) if data.derush => STEP_CUT,
            (_, true) => STEP_STORYBOARD,
            (Start::Script, false) => STEP_SCRIPT,
            _ => STEP_VOICE,
        }
    }

    /// The `n`-th visible step (Alt+1…7).
    fn go_to_nth(&mut self, n: usize, cx: &mut Context<Self>) {
        if self.home.is_some() {
            return;
        }
        if let Some(step) = self.visible_steps().get(n).copied() {
            self.go_to(step, cx);
        }
    }

    /// The take is cut automatically (Cut step); otherwise it is used whole.
    fn uses_cut(&self) -> bool {
        self.session.data.start != Start::Silent && self.session.data.derush
    }

    /// Pieces of the take forming the edit (seconds of the take).
    fn edit_ranges(&self) -> Vec<(f64, f64)> {
        if self.session.data.start == Start::Silent {
            return vec![(0.0, self.session.data.duration.max(0.5))];
        }
        if !self.session.data.derush {
            return self
                .cut
                .transcript
                .as_ref()
                .map(|t| t.media_duration.as_secs_f64())
                .filter(|d| *d > 0.0)
                .map(|d| vec![(0.0, d)])
                .unwrap_or_default();
        }
        self.cut.ranges()
    }

    /// Opens another project: its session, storyboard, job queue and
    /// engine. The engine of the previous project stops (its queued and
    /// running jobs resume when it is opened again).
    fn open_project(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        self.commit_inputs(cx);
        self.command(Command::Pause, cx);
        // Agents must not believe the previous project is still open.
        enfait_mcp::bridge::clear_heartbeat(self.session.dir());
        let session = Session::load(&dir);
        let studio_dir = session.studio_dir();
        let queue = std::sync::Arc::new(std::sync::Mutex::new(JobQueue::load(
            &studio_dir.join("jobs.json"),
        )));
        let worker = Worker::start(studio_dir, std::sync::Arc::clone(&queue));
        let previous = std::mem::replace(&mut self.worker, worker);
        // Stopping waits for the engine thread: away from the interface.
        std::thread::spawn(move || drop(previous));
        self.queue = queue;
        self.session = session;
        self.board = Storyboard::load(&dir);
        self.needs_sync = self.board.is_some();
        self.board_notice = None;
        self.selected_asset = None;
        self.library_notice = None;
        self.cutting = None;
        self.credits_copied = false;
        self.broll_sig = std::cell::Cell::new(None);
        self.lists = lazy::Lists::default();
        self.clips = std::cell::RefCell::default();
        self.finish_history = enfait_core::FinishHistory::default();
        self.selected_cue = None;
        self.audition = None;
        self.finish_notice = None;
        self.watch = bridge::Watch::default();
        self.retouches.clear();
        self.magic = None;
        self.magic_after_import = false;
        self.write_notice = None;
        self.showing_preview = false;
        self.preview_stamp = None;
        self.cut = CutState::default();
        self.montage_badge = 0;
        self.care = None;
        self.mention = None;
        self.recording = None;
        self.player.update(cx, |p, cx| p.close(cx));
        let fx = self.session.look.fx;
        self.player.update(cx, |p, cx| p.set_fx(fx, cx));
        self.reload_inputs = true;
        self.projects.touch(&dir);
        if let Err(e) = self.projects.save() {
            tracing::warn!("projects: {e}");
        }
        self.home = None;
        self.step = usize::MAX;
        let step = self.first_step();
        self.go_to(step, cx);
    }

    /// Refills the fields that show the project (after opening another
    /// one).
    fn reload_project_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let script = self.session.script.clone();
        self.script
            .update(cx, |s, cx| s.set_value(script, window, cx));
        self.reload_look_inputs(window, cx);
        self.sync_knobs(window, cx);
        self.sync_inputs(window, cx);
    }

    /// Style rules and effect sliders from the look.
    fn reload_look_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reload_look_inputs = false;
        let rules = self.session.look.style.rules.clone();
        self.style_rules
            .update(cx, |s, cx| s.set_value(rules, window, cx));
        let fx = self.session.look.fx;
        for (field, state) in &self.fx_sliders {
            let v = field.get(&fx);
            state.update(cx, |s, cx| s.set_value(v, window, cx));
        }
    }

    /// Back to the home screen; the project stays loaded (its engine keeps
    /// working).
    fn show_home(&mut self, cx: &mut Context<Self>) {
        self.commit_inputs(cx);
        self.command(Command::Pause, cx);
        self.settings.open = false;
        self.home = Some(home::Home::new(&self.projects));
        cx.notify();
    }

    fn go_to(&mut self, step: usize, cx: &mut Context<Self>) {
        if step != self.step {
            self.command(Command::Pause, cx);
        }
        self.settings.open = false;
        self.step = step;
        if step == STEP_MONTAGE {
            self.montage_badge = 0;
        }
        if step == STEP_RETOUCH {
            self.select_ready_block();
        }
        let preview = worker::preview_path(&self.session.studio_dir()).is_file();
        if step == STEP_CUT && self.showing_preview {
            // The cut works on the take, not on the preview.
            self.showing_preview = false;
            self.preview_stamp = None;
        }
        let montage_step = board_step(step);
        if step == STEP_VOICE && self.session.data.rush.is_some() && !self.showing_preview {
            self.open_rush(cx);
        }
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

    /// Imports a take: it replaces the previous one and the cut step opens
    /// (the storyboard without automatic cut).
    fn import_rush(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let path = std::path::absolute(&path).unwrap_or(path);
        if self.session.data.start == Start::Silent {
            // A voice brought to a silent project makes it a voice project.
            self.session.data.start = Start::Import;
        }
        self.session.data.rush = Some(path);
        self.session.data.choices.clear();
        self.session.save_data();
        self.cut = CutState::default();
        self.showing_preview = false;
        self.preview_stamp = None;
        let step = if self.uses_cut() {
            STEP_CUT
        } else {
            STEP_STORYBOARD
        };
        self.go_to(step, cx);
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
        if self.session.data.start == Start::Silent {
            // Nothing is said: the time base is the chosen duration.
            #[allow(clippy::cast_possible_truncation)]
            let ms = (self.session.data.duration.max(0.5) * 1000.0).round() as i64;
            self.cut.transcript = Some(Arc::new(enfait_core::Transcript {
                language: String::new(),
                words: Vec::new(),
                media_duration: Flicks::from_millis(ms).unwrap_or(Flicks::ZERO),
            }));
            self.cut.status = CutStatus::Ready;
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
        // Pieces done by an interrupted run are not transcribed again.
        let partial_path = dir.join("transcript.partial.json");
        let partial = enfait_speech::Partial::load(&partial_path);
        let progress = Arc::new(std::sync::atomic::AtomicU32::new(0));
        self.cut.progress = progress.clone();
        self.cut.shown = 0;
        self.cut.started = None;
        let source = rush.clone();
        let work = cx.background_spawn(async move {
            let pcm = enfait_speech::decode_pcm(&source).map_err(|e| e.to_string())?;
            let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
            let checkpoint = |p: &enfait_speech::Partial| {
                let saved = std::fs::create_dir_all(&dir).and_then(|()| p.save(&partial_path));
                if let Err(e) = saved {
                    tracing::warn!("partial transcript not saved: {e}");
                }
            };
            let options = enfait_speech::TranscribeOptions {
                language: None,
                // No script as prompt: it pushes whisper to merge the
                // retakes into one.
                prompt: None,
                threads: u16::try_from(threads).unwrap_or(4),
                takes: true,
                progress: Some(Arc::new(move |done: f32| {
                    progress.store(done.to_bits(), std::sync::atomic::Ordering::Relaxed);
                })),
                resume: partial.as_ref(),
                checkpoint: Some(&checkpoint),
            };
            let transcript =
                enfait_speech::transcribe(&model, &pcm, &options).map_err(|e| e.to_string())?;
            if let Ok(json) = serde_json::to_vec_pretty(&transcript) {
                let _ = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&cached, json));
                let _ = std::fs::remove_file(&partial_path);
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
        if self.uses_cut() && self.cut.analyzed.as_deref() != Some(self.session.script.as_str()) {
            self.cut
                .analyze(&self.session.script, &self.session.data.choices);
        }
        self.apply_cut(cx);
        if board_step(self.step) {
            self.refresh_board(cx);
        }
    }

    /// Sends the edit (or the whole take) to the player.
    fn apply_cut(&mut self, cx: &mut Context<Self>) {
        if self.showing_preview {
            return;
        }
        let ranges = self.cut.ranges();
        let ranges = (self.uses_cut() && self.cut.montage && !ranges.is_empty()).then_some(ranges);
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
        let at_home = self.home.is_some();
        let badge = self.montage_badge;
        let ready = self.ready_count();
        let steps = self
            .visible_steps()
            .into_iter()
            .enumerate()
            .filter(move |(_, i)| !at_home && (!compact || *i == current))
            .map(|(n, i)| {
                let active = i == current && !settings_open;
                let available = true;
                let count = match i {
                    STEP_MONTAGE if badge > 0 => Some((badge, true)),
                    STEP_RETOUCH if ready > 0 => Some((ready, false)),
                    _ => None,
                };
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
                    .child(mono(format!("{}", n.saturating_add(1))))
                    .child(step_name(i))
                    .when_some(count, |d, (count, fresh)| {
                        d.child(
                            mono(count.to_string())
                                .px(px(5.))
                                .h(px(16.))
                                .flex()
                                .items_center()
                                .rounded(px(8.))
                                .text_size(px(10.))
                                .map(|d| {
                                    if fresh {
                                        d.bg(color(tokens::SIGNAL))
                                            .text_color(color(tokens::ON_SIGNAL))
                                    } else {
                                        d.border_1()
                                            .border_color(color(tokens::LINE_STRONG))
                                            .text_color(color(tokens::FG2))
                                    }
                                }),
                        )
                    })
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
                    .id("home")
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .cursor_pointer()
                    .child(div().size(px(10.)).bg(color(tokens::SIGNAL)))
                    .child(
                        div()
                            .font_family(fonts::CONDENSED)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("ENFAIT"),
                    )
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tr!("home.projects"))
                            .build(window, cx)
                    })
                    .on_click(cx.listener(|shell, _: &gpui_kit::ClickEvent, _, cx| {
                        shell.show_home(cx);
                    })),
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
            .when(!at_home, |d| {
                d.child(div().text_color(color(tokens::LINE_STRONG)).child("/"))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .max_w(px(280.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(SharedString::from(self.title())),
                    )
            })
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
                    .when(!at_home, |d| {
                        d.child(
                            mono(timecode.unwrap_or_else(|| "00:00:00:00".to_owned()))
                                .text_size(px(15.)),
                        )
                    })
                    .when(!compact && !at_home, |d| {
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
            STEP_SCRIPT => self.script_screen(cx),
            STEP_VOICE => self.voice_screen(cx),
            _ => self.cut_screen(playing, cx),
        }
    }

    /// Files dropped on the window: a take on the voice steps, files for
    /// the library elsewhere.
    fn dropped(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        match self.step {
            STEP_SCRIPT | STEP_VOICE | STEP_CUT => {
                if let Some(path) = paths.first() {
                    self.import_rush(path.clone(), cx);
                }
            }
            _ => {
                self.import_paths(paths, cx);
            }
        }
    }

    /// Bottom strip: waveform of the player, then the storyboard blocks
    /// (click to select).
    fn timeline(&self, cx: &mut Context<Self>) -> Div {
        let strip = self.player.update(cx, |p, cx| p.strip(cx));
        let selected = self.board.as_ref().and_then(|b| b.selected);
        let marked: Vec<enfait_core::BlockId> = self
            .board
            .as_ref()
            .map(|b| b.marked.clone())
            .unwrap_or_default();
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
                    .on_click(cx.listener(
                        move |shell, event: &gpui_kit::ClickEvent, window, cx| {
                            shell.click_block(id, event.modifiers(), window, cx);
                        },
                    ))
                    .child(
                        div()
                            .size_full()
                            .rounded(px(2.))
                            .bg(color(kind_color(kind)))
                            .when(marked.contains(&id) && marked.len() > 1, |d| {
                                d.border_2().border_color(color(tokens::SIGNAL))
                            })
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
        if self.care.is_some() {
            self.care = None;
        } else if self.mention.is_some() {
            self.mention = None;
        } else if self.naming_style || self.confirm_style_delete.is_some() {
            self.naming_style = false;
            self.confirm_style_delete = None;
        } else if self.settings.open {
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
        if std::mem::take(&mut self.reload_inputs) {
            self.reload_project_inputs(window, cx);
        }
        if self.reload_look_inputs {
            self.reload_look_inputs(window, cx);
        }
        if std::mem::take(&mut self.reset_wizard_inputs) {
            self.new_title
                .update(cx, |s, cx| s.set_value("", window, cx));
            self.new_duration
                .update(cx, |s, cx| s.set_value("20", window, cx));
            self.new_brief
                .update(cx, |s, cx| s.set_value("", window, cx));
        }
        if let Some(pending) = self.mention.as_mut().and_then(|m| m.pending.take()) {
            self.apply_mention(pending, window, cx);
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
        self.expire_toasts();

        if self.home.is_some() {
            let root = root
                .on_action(cx.listener(|s, _: &OpenSettings, _, cx| s.open_settings(cx)))
                .on_action(cx.listener(|s, _: &CloseOverlay, _, cx| {
                    if s.settings.open {
                        s.settings.open = false;
                    } else if let Some(home) = &mut s.home {
                        home.back();
                    }
                    cx.notify();
                }))
                .on_drop(cx.listener(|s, paths: &ExternalPaths, _, cx| {
                    if let Some(path) = paths.paths().first() {
                        s.home_dropped(path.clone(), cx);
                    }
                }))
                .child(self.header(false, false, None, cx));
            if self.settings.open {
                return root.child(self.settings_page(window, cx));
            }
            return root
                .child(self.home_screen(window, cx))
                .children(self.toasts_layer());
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
        // The style column belongs to the storyboard; the inspector wins
        // over it: without it nothing can be edited.
        let style_step = self.step == STEP_STORYBOARD;
        let show_sidebar = style_step && width >= px(BOARD_SIDEBAR_MIN_WINDOW);
        let _ = SIDEBAR_MIN_WINDOW;
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
            .on_action(cx.listener(|s, _: &Step1, _, cx| s.go_to_nth(0, cx)))
            .on_action(cx.listener(|s, _: &Step2, _, cx| s.go_to_nth(1, cx)))
            .on_action(cx.listener(|s, _: &Step3, _, cx| s.go_to_nth(2, cx)))
            .on_action(cx.listener(|s, _: &Step4, _, cx| s.go_to_nth(3, cx)))
            .on_action(cx.listener(|s, _: &Step5, _, cx| s.go_to_nth(4, cx)))
            .on_action(cx.listener(|s, _: &Step6, _, cx| s.go_to_nth(5, cx)))
            .on_action(cx.listener(|s, _: &Step7, _, cx| s.go_to_nth(6, cx)))
            .on_action(cx.listener(|s, _: &GoHome, _, cx| s.show_home(cx)))
            .on_action(cx.listener(|s, _: &OpenSettings, _, cx| s.open_settings(cx)))
            .on_action(cx.listener(|s, _: &CloseOverlay, window, cx| {
                if s.player.read(cx).is_expanded() {
                    s.player
                        .update(cx, |p, cx| p.set_expanded(false, window, cx));
                } else {
                    s.close_overlay(cx);
                }
            }))
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
                s.dropped(paths.paths(), cx);
            }));

        // Player filling the window (button in its corner, Escape to leave).
        if self.player.read(cx).is_expanded() {
            return root.child(div().flex_1().min_h(px(0.)).child(self.player.clone()));
        }
        let root = root
            .child(self.header(!show_panel, style_step && !show_sidebar, timecode, cx))
            .children(self.magic_banner(cx));

        if self.settings.open {
            return root.child(self.settings_page(window, cx));
        }

        let inspector = match self.step {
            STEP_STORYBOARD => Some(self.inspector(side_inspector, cx)),
            STEP_RETOUCH => Some(self.retouch_panel(side_inspector, cx)),
            STEP_FINISH => Some(self.finish_inspector(side_inspector, cx)),
            _ => None,
        };
        let sidebar = (style_step && (show_sidebar || self.sidebar_open)).then(|| self.sidebar(cx));
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
        } else if self.step == STEP_MONTAGE {
            self.montage_screen(cx)
        } else if self.step == STEP_RETOUCH {
            self.retouch_screen(playing_output, below, cx)
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

        let strip = !matches!(self.step, STEP_SCRIPT | STEP_MONTAGE);
        let sidebar_width = self.panel_width(split::Splitter::Sidebar, cx);
        let inspector_width = self.panel_width(split::Splitter::Inspector, cx);
        root.child(
            div()
                .flex_1()
                .min_h(px(0.))
                .flex()
                .on_drag_move(
                    cx.listener(|shell, event: &DragMoveEvent<split::Resize>, _, cx| {
                        shell.drag_panel(event, cx);
                    }),
                )
                .on_drop(cx.listener(|shell, _: &split::Resize, _, cx| {
                    shell.drop_panel(cx);
                }))
                .when_some(sidebar, |d, s| {
                    d.child(
                        s.w(sidebar_width)
                            .relative()
                            .child(split::handle(split::Splitter::Sidebar, cx)),
                    )
                })
                .child(center)
                .when_some(inspector, |d, i| {
                    d.child(
                        i.w(inspector_width)
                            .relative()
                            .child(split::handle(split::Splitter::Inspector, cx)),
                    )
                }),
        )
        .when(strip, |d| d.child(self.timeline(cx)))
        .when_some(overlay, |d, s| {
            d.child(
                s.w(sidebar_width)
                    .absolute()
                    .top(px(44.))
                    .bottom_0()
                    .left_0()
                    .shadow_lg()
                    .border_r_1()
                    .border_color(color(tokens::LINE_STRONG)),
            )
        })
        .when_some(self.prompter.clone(), |d, p| d.child(p))
        .children(self.care_dialog(cx))
        .children(self.toasts_layer())
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
