//! Animation job engine, on its own thread (with a tokio runtime: model
//! calls, Chromium, ffmpeg).
//!
//! The interface pushes jobs into the shared [`JobQueue`] and keeps a
//! [`Snapshot`] of the project up to date; the engine runs up to
//! [`MAX_JOBS`] jobs at once with `enfait-studio` (never two on the same
//! block), then rebuilds the storyboard preview. The interface polls
//! [`Worker::take_changed`] to refresh.

use crate::jobs::{Job, JobKind, JobQueue, JobStatus};
use anyhow::{Context as _, Result};
use enfait_ai::animate::{AnimatorConfig, SceneLayer, TransitionBrief, animator_system};
use enfait_ai::direction::{BlockIntent, Direction};
use enfait_ai::{Provider, ProviderChoice, Usage};
use enfait_core::{
    Block, BlockId, BlockKind, Finish, Look, Project, ProjectSettings, TimeRange, Transcript,
    Transition,
};
use enfait_motion::{Gpu, RendererPool, composition, frame_name, library};
use enfait_studio::Studio;
use enfait_studio::preview::{Output, assemble, pieces, preview_size};
use rust_i18n::t;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Jobs run at the same time (model calls and renders in parallel).
pub const MAX_JOBS: usize = 2;

/// The state of the project a job needs.
#[derive(Clone)]
pub struct Snapshot {
    pub project: Project,
    /// Transcript of the edit.
    pub transcript: Transcript,
    pub look: Look,
    pub library: PathBuf,
    pub script: String,
    pub rush: PathBuf,
    pub rush_has_video: bool,
    /// Frame size of the take.
    pub rush_size: Option<(u32, u32)>,
    /// Pieces of the take forming the edit.
    pub parts: Vec<(f64, f64)>,
    /// Sound finish (music, SFX, ducking).
    pub finish: Finish,
    /// Chosen model provider.
    pub provider: ProviderChoice,
}

/// Data shared between the interface and the engine.
struct Shared {
    queue: Arc<Mutex<JobQueue>>,
    snapshot: Mutex<Option<Arc<Snapshot>>>,
    /// Wake-up: true when there is work.
    wake: (Mutex<bool>, Condvar),
    stop: AtomicBool,
    changed: AtomicBool,
    preview_requested: AtomicBool,
    /// Only the sound of the preview must be redone (finish changed).
    mix_requested: AtomicBool,
    /// Activity of each running job (0: preview), for the interface.
    activity: Mutex<BTreeMap<u64, String>>,
    /// Phase of each running job, for the Montage screen.
    phases: Mutex<BTreeMap<u64, Phase>>,
    /// Export requested, not started yet.
    export_request: Mutex<Option<ExportRequest>>,
    export_state: Mutex<ExportState>,
    /// Storyboard to write (editor pass), not started yet.
    write_request: Mutex<Option<WriteRequest>>,
    write_state: Mutex<WriteState>,
    /// Thumbnail work requested, not started yet.
    thumbnail_request: Mutex<Option<ThumbnailRequest>>,
    thumbnail_state: Mutex<ThumbnailState>,
    /// Finished work, for notifications.
    events: Mutex<Vec<Event>>,
}

/// Where a running job stands: preparation (direction, frames of the
/// take), the edit itself (model calls), finalisation (render).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    Preparing,
    Editing,
    Finishing,
}

fn set_phase(shared: &Shared, id: u64, phase: Option<Phase>) {
    if let Ok(mut p) = shared.phases.lock() {
        match phase {
            Some(phase) => p.insert(id, phase),
            None => p.remove(&id),
        };
    }
    shared.changed.store(true, Ordering::Release);
}

/// Something the engine finished, worth a notification.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A block's animation (or the transition after it) is done or failed.
    Job {
        /// Number of the block, from 1 (0 if it no longer exists).
        number: usize,
        transition: bool,
        error: Option<String>,
    },
    Export(ExportState),
    Written {
        error: Option<String>,
    },
    /// A thumbnail proposal is ready or failed.
    Thumbnail {
        error: Option<String>,
    },
}

fn push_event(shared: &Shared, event: Event) {
    if let Ok(mut events) = shared.events.lock() {
        events.push(event);
    }
}

/// "Write the storyboard": the editor pass, then the search for the shots
/// and sounds it asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteRequest {
    /// Blocks already described keep kind and description (otherwise, magic
    /// mode: everything is written).
    pub keep_authored: bool,
    pub quality: enfait_ai::Quality,
    /// Blocks per request of the editor (`None`: the whole video at once).
    pub blocks_per_request: Option<usize>,
    /// Fingerprint of the edit: the result only applies to it.
    pub key: String,
}

/// A written storyboard, to be applied by the interface.
#[derive(Debug)]
pub struct Written {
    pub request: WriteRequest,
    pub plan: enfait_ai::editor::EditPlan,
    pub gathered: enfait_studio::magic::Gathered,
}

#[derive(Debug, Default)]
pub enum WriteState {
    #[default]
    Idle,
    /// What is happening right now.
    Running(String),
    Done(Box<Written>),
    Failed(String),
}

/// What is exported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportKind {
    /// The final video (MP4).
    Video,
    /// The edit for Premiere Pro (FCP7 XML and media).
    Premiere,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportRequest {
    pub kind: ExportKind,
    pub path: PathBuf,
    /// Keep the sound effects.
    pub sfx: bool,
}

/// Where the export stands, for the interface.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum ExportState {
    #[default]
    Idle,
    Running {
        kind: ExportKind,
        /// From 0 to 1.
        progress: f64,
    },
    Done {
        path: PathBuf,
        message: String,
    },
    Failed(String),
}

/// YouTube thumbnail work (see `enfait_studio::thumbnail`).
#[derive(Debug, Clone, PartialEq)]
pub enum ThumbnailRequest {
    /// A new proposal for `title`, following `notes`.
    Generate {
        title: String,
        notes: String,
        quality: enfait_ai::Quality,
    },
    /// Proposal `version` as a picture for YouTube.
    Image { version: u32, path: PathBuf },
    /// Proposal `version` as a layered Photoshop document.
    Psd { version: u32, path: PathBuf },
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum ThumbnailState {
    #[default]
    Idle,
    /// What is happening right now.
    Running(String),
    /// A new proposal is ready.
    Generated(u32),
    /// An export is written.
    Exported {
        path: PathBuf,
        message: String,
    },
    Failed(String),
}

pub struct Worker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

pub use enfait_studio::session::{
    block_name, broll_clip_path, broll_key, broll_ready, clip_path, source_pieces,
    transition_clip_path, transition_name, transition_window, visual_clip,
};

/// Preview without the sound finish (picture and voice).
fn preview_base_path(dir: &Path) -> PathBuf {
    dir.join("preview-base.mp4")
}

pub fn preview_path(dir: &Path) -> PathBuf {
    dir.join("preview.mp4")
}

/// Previous versions of the animation of a block.
pub fn versions(dir: &Path, id: BlockId, settings: &ProjectSettings) -> Vec<u32> {
    enfait_studio::versions(dir, &block_name(id, settings))
}

impl Snapshot {
    /// What is needed to edit the video (`dir`: the studio).
    pub fn sources<'a>(&'a self, dir: &'a Path) -> enfait_studio::session::Sources<'a> {
        enfait_studio::session::Sources {
            studio: dir,
            library: &self.library,
            project: &self.project,
            transcript: &self.transcript,
            look: &self.look,
            finish: &self.finish,
            rush: &self.rush,
            parts: &self.parts,
        }
    }

    /// The take does not have the project format: the preview reframes it.
    fn reframed(&self) -> bool {
        let settings = self.project.settings();
        self.rush_size
            .is_some_and(|(w, h)| enfait_core::AspectRatio::nearest(w, h) != settings.aspect)
    }
}

impl Worker {
    /// `dir`: studio folder (compositions, clips, preview).
    pub fn start(dir: PathBuf, queue: Arc<Mutex<JobQueue>>) -> Self {
        let shared = Arc::new(Shared {
            queue,
            snapshot: Mutex::new(None),
            wake: (Mutex::new(true), Condvar::new()),
            stop: AtomicBool::new(false),
            changed: AtomicBool::new(false),
            preview_requested: AtomicBool::new(false),
            mix_requested: AtomicBool::new(false),
            activity: Mutex::new(BTreeMap::new()),
            phases: Mutex::new(BTreeMap::new()),
            export_request: Mutex::new(None),
            export_state: Mutex::new(ExportState::Idle),
            write_request: Mutex::new(None),
            write_state: Mutex::new(WriteState::Idle),
            thumbnail_request: Mutex::new(None),
            thumbnail_state: Mutex::new(ThumbnailState::Idle),
            events: Mutex::new(Vec::new()),
        });
        let thread = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("enfait-studio".into())
                .spawn(move || run(&dir, &shared))
                .map_err(|e| tracing::error!("moteur d'animation : {e}"))
                .ok()
        };
        Self { shared, thread }
    }

    pub fn set_snapshot(&self, snapshot: Snapshot) {
        if let Ok(mut slot) = self.shared.snapshot.lock() {
            *slot = Some(Arc::new(snapshot));
        }
    }

    /// Signals new work (job added).
    pub fn wake(&self) {
        wake(&self.shared);
    }

    /// Asks to rebuild the preview (edit changed).
    pub fn request_preview(&self) {
        self.shared.preview_requested.store(true, Ordering::Release);
        self.wake();
    }

    /// Asks to redo the sound of the preview (finish changed): the picture
    /// is not re-encoded.
    pub fn request_mix(&self) {
        self.shared.mix_requested.store(true, Ordering::Release);
        self.wake();
    }

    /// True if something changed since the last call.
    pub fn take_changed(&self) -> bool {
        self.shared.changed.swap(false, Ordering::AcqRel)
    }

    /// Starts an export (ignored if one is running).
    pub fn export(&self, request: ExportRequest) {
        if matches!(self.export_state(), ExportState::Running { .. }) {
            return;
        }
        if let Ok(mut state) = self.shared.export_state.lock() {
            *state = ExportState::Running {
                kind: request.kind.clone(),
                progress: 0.0,
            };
        }
        if let Ok(mut slot) = self.shared.export_request.lock() {
            *slot = Some(request);
        }
        self.wake();
    }

    pub fn export_state(&self) -> ExportState {
        self.shared
            .export_state
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    /// Forgets the result of the last export.
    pub fn clear_export(&self) {
        if let Ok(mut state) = self.shared.export_state.lock()
            && !matches!(*state, ExportState::Running { .. })
        {
            *state = ExportState::Idle;
        }
    }

    /// Starts thumbnail work (ignored if some is running).
    pub fn thumbnail(&self, request: ThumbnailRequest) {
        if let Ok(mut state) = self.shared.thumbnail_state.lock() {
            if matches!(*state, ThumbnailState::Running(_)) {
                return;
            }
            *state = ThumbnailState::Running(t!("worker.thumb_starting").into_owned());
        }
        if let Ok(mut slot) = self.shared.thumbnail_request.lock() {
            *slot = Some(request);
        }
        self.wake();
    }

    pub fn thumbnail_state(&self) -> ThumbnailState {
        self.shared
            .thumbnail_state
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    /// Forgets the result of the last thumbnail work.
    pub fn clear_thumbnail(&self) {
        if let Ok(mut state) = self.shared.thumbnail_state.lock()
            && !matches!(*state, ThumbnailState::Running(_))
        {
            *state = ThumbnailState::Idle;
        }
    }

    /// Starts writing the storyboard (ignored if it is running).
    pub fn write_storyboard(&self, request: WriteRequest) {
        if let Ok(mut state) = self.shared.write_state.lock() {
            if matches!(*state, WriteState::Running(_)) {
                return;
            }
            *state = WriteState::Running(t!("worker.starting").into_owned());
        }
        if let Ok(mut slot) = self.shared.write_request.lock() {
            *slot = Some(request);
        }
        self.wake();
    }

    /// What the storyboard writing is doing, if it runs.
    pub fn writing(&self) -> Option<String> {
        match &*self.shared.write_state.lock().ok()? {
            WriteState::Running(text) => Some(text.clone()),
            _ => None,
        }
    }

    /// The result of the storyboard writing, only once: a written storyboard
    /// or an error.
    pub fn take_written(&self) -> Option<Result<Box<Written>, String>> {
        let mut state = self.shared.write_state.lock().ok()?;
        match std::mem::take(&mut *state) {
            WriteState::Done(written) => Some(Ok(written)),
            WriteState::Failed(e) => Some(Err(e)),
            other => {
                *state = other;
                None
            }
        }
    }

    /// Work finished since the last call.
    pub fn take_events(&self) -> Vec<Event> {
        self.shared
            .events
            .lock()
            .map(|mut e| std::mem::take(&mut *e))
            .unwrap_or_default()
    }

    /// Phase of a running job.
    pub fn phase(&self, job: u64) -> Option<Phase> {
        self.shared.phases.lock().ok()?.get(&job).copied()
    }

    /// Activity text of a running job.
    pub fn job_activity(&self, job: u64) -> Option<String> {
        self.shared.activity.lock().ok()?.get(&job).cloned()
    }

    pub fn activity(&self) -> Option<String> {
        let activity = self.shared.activity.lock().ok()?;
        (!activity.is_empty()).then(|| activity.values().cloned().collect::<Vec<_>>().join(" · "))
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.wake();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn wake(shared: &Shared) {
    let (lock, condvar) = &shared.wake;
    if let Ok(mut flag) = lock.lock() {
        *flag = true;
    }
    condvar.notify_one();
}

/// A provider choice and the configuration generation it was built with.
type ProviderKey = (ProviderChoice, u64);

/// Resources shared by the jobs, created when first needed.
struct Engine {
    dir: PathBuf,
    /// Model provider and the choice that created it.
    /// The choice and the configuration generation it was built with.
    provider: tokio::sync::Mutex<Option<(ProviderKey, Arc<Provider>)>>,
    renderers: tokio::sync::OnceCell<RendererPool>,
    usage: Mutex<Usage>,
    /// A single "director" pass at a time.
    direction: tokio::sync::Mutex<()>,
    /// Installation of the library in the compositions.
    install: Mutex<()>,
}

fn set_activity(shared: &Shared, id: u64, text: Option<String>) {
    if let Ok(mut a) = shared.activity.lock() {
        match text {
            Some(text) => a.insert(id, text),
            None => a.remove(&id),
        };
    }
    shared.changed.store(true, Ordering::Release);
}

fn run(dir: &Path, shared: &Arc<Shared>) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("moteur d'animation : {e}");
            return;
        }
    };
    let engine = Arc::new(Engine {
        dir: dir.to_path_buf(),
        provider: tokio::sync::Mutex::new(None),
        renderers: tokio::sync::OnceCell::new(),
        usage: Mutex::new(Usage::default()),
        direction: tokio::sync::Mutex::new(()),
        install: Mutex::new(()),
    });
    let running = Arc::new(AtomicUsize::new(0));
    while !shared.stop.load(Ordering::Acquire) {
        if running.load(Ordering::Acquire) < MAX_JOBS
            && let Some(job) = shared.queue.lock().ok().and_then(|mut q| q.start_next())
        {
            running.fetch_add(1, Ordering::AcqRel);
            shared.changed.store(true, Ordering::Release);
            let snapshot = shared.snapshot.lock().ok().and_then(|s| s.clone());
            let (engine, shared, running) = (
                Arc::clone(&engine),
                Arc::clone(shared),
                Arc::clone(&running),
            );
            set_phase(&shared, job.id, Some(Phase::Preparing));
            runtime.spawn(async move {
                let number = snapshot.as_ref().map_or(0, |s| {
                    s.project
                        .blocks()
                        .iter()
                        .position(|b| b.id == job.block)
                        .map_or(0, |i| i.saturating_add(1))
                });
                let result = match snapshot {
                    Some(snapshot) => run_job(&engine, &shared, &job, &snapshot).await,
                    None => Err(anyhow::anyhow!("storyboard unavailable")),
                };
                // Going back to a version is instant: nothing to announce.
                if !matches!(job.kind, JobKind::Restore { .. }) {
                    push_event(
                        &shared,
                        Event::Job {
                            number,
                            transition: job.kind == JobKind::Transition,
                            error: result.as_ref().err().map(describe),
                        },
                    );
                }
                let status = match result {
                    Ok(()) => JobStatus::Done,
                    Err(e) => {
                        tracing::warn!("job {}: {e:#}", job.id);
                        JobStatus::Failed(describe(&e))
                    }
                };
                if status == JobStatus::Done {
                    shared.preview_requested.store(true, Ordering::Release);
                }
                if let Ok(mut q) = shared.queue.lock() {
                    q.finish(job.id, status);
                }
                set_activity(&shared, job.id, None);
                set_phase(&shared, job.id, None);
                running.fetch_sub(1, Ordering::AcqRel);
                wake(&shared);
            });
            continue;
        }
        if let Some(request) = shared.export_request.lock().ok().and_then(|mut r| r.take()) {
            let snapshot = shared.snapshot.lock().ok().and_then(|s| s.clone());
            let (dir, shared) = (engine.dir.clone(), Arc::clone(shared));
            runtime.spawn(async move {
                let state = match snapshot {
                    Some(snapshot) => run_export(&dir, &shared, &snapshot, &request).await,
                    None => ExportState::Failed("storyboard unavailable".into()),
                };
                push_event(&shared, Event::Export(state.clone()));
                if let Ok(mut s) = shared.export_state.lock() {
                    *s = state;
                }
                shared.changed.store(true, Ordering::Release);
            });
            continue;
        }
        if let Some(request) = shared.write_request.lock().ok().and_then(|mut r| r.take()) {
            let snapshot = shared.snapshot.lock().ok().and_then(|s| s.clone());
            let (engine, shared) = (Arc::clone(&engine), Arc::clone(shared));
            runtime.spawn(async move {
                let state = match snapshot {
                    Some(snapshot) => {
                        match run_write(&engine, &shared, &snapshot, &request).await {
                            Ok((plan, gathered)) => WriteState::Done(Box::new(Written {
                                request,
                                plan,
                                gathered,
                            })),
                            Err(e) => {
                                tracing::warn!("storyboard: {e:#}");
                                WriteState::Failed(describe(&e))
                            }
                        }
                    }
                    None => WriteState::Failed("storyboard unavailable".into()),
                };
                push_event(
                    &shared,
                    Event::Written {
                        error: match &state {
                            WriteState::Failed(e) => Some(e.clone()),
                            _ => None,
                        },
                    },
                );
                if let Ok(mut s) = shared.write_state.lock() {
                    *s = state;
                }
                shared.changed.store(true, Ordering::Release);
            });
            continue;
        }
        if let Some(request) = shared
            .thumbnail_request
            .lock()
            .ok()
            .and_then(|mut r| r.take())
        {
            let snapshot = shared.snapshot.lock().ok().and_then(|s| s.clone());
            let (engine, shared) = (Arc::clone(&engine), Arc::clone(shared));
            runtime.spawn(async move {
                let state = match snapshot {
                    Some(snapshot) => run_thumbnail(&engine, &shared, &snapshot, &request)
                        .await
                        .unwrap_or_else(|e| {
                            tracing::warn!("thumbnail: {e:#}");
                            ThumbnailState::Failed(describe(&e))
                        }),
                    None => ThumbnailState::Failed("storyboard unavailable".into()),
                };
                if matches!(request, ThumbnailRequest::Generate { .. }) {
                    push_event(
                        &shared,
                        Event::Thumbnail {
                            error: match &state {
                                ThumbnailState::Failed(e) => Some(e.clone()),
                                _ => None,
                            },
                        },
                    );
                }
                if let Ok(mut s) = shared.thumbnail_state.lock() {
                    *s = state;
                }
                shared.changed.store(true, Ordering::Release);
            });
            continue;
        }
        if shared.preview_requested.swap(false, Ordering::AcqRel) {
            // The complete preview includes the mix.
            shared.mix_requested.store(false, Ordering::Release);
            rebuild_preview(&runtime, &engine.dir, shared);
            continue;
        }
        if shared.mix_requested.swap(false, Ordering::AcqRel) {
            remix_preview(&runtime, &engine.dir, shared);
            continue;
        }
        // Wait for work, the end of a job or the stop.
        let (lock, condvar) = &shared.wake;
        if let Ok(mut flag) = lock.lock() {
            if !*flag {
                flag = condvar
                    .wait_timeout(flag, Duration::from_millis(500))
                    .map_or_else(|e| e.into_inner().0, |(f, _)| f);
            }
            *flag = false;
        }
    }
    // Stop: give the jobs a few seconds, then close Chromium.
    runtime.block_on(async {
        for _ in 0..30 {
            if running.load(Ordering::Acquire) == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    runtime.shutdown_timeout(Duration::from_secs(1));
    if let Ok(engine) = Arc::try_unwrap(engine)
        && let Some(renderers) = engine.renderers.into_inner()
        && let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
    {
        rt.block_on(renderers.close());
    }
}

fn rebuild_preview(runtime: &tokio::runtime::Runtime, dir: &Path, shared: &Shared) {
    let Some(snapshot) = shared.snapshot.lock().ok().and_then(|s| s.clone()) else {
        return;
    };
    runtime.block_on(mount_brolls(dir, &snapshot, shared));
    set_activity(shared, 0, Some(t!("worker.preview").into_owned()));
    if let Err(e) = runtime.block_on(build_preview(dir, &snapshot)) {
        tracing::warn!("preview: {e:#}");
    }
    set_activity(shared, 0, None);
}

fn remix_preview(runtime: &tokio::runtime::Runtime, dir: &Path, shared: &Shared) {
    let Some(snapshot) = shared.snapshot.lock().ok().and_then(|s| s.clone()) else {
        return;
    };
    let base = preview_base_path(dir);
    if !base.is_file() {
        // No preview yet (no clip, no finish): redo everything.
        rebuild_preview(runtime, dir, shared);
        return;
    }
    set_activity(shared, 0, Some(t!("worker.mixing").into_owned()));
    if let Err(e) = runtime.block_on(mix_preview(dir, &snapshot)) {
        tracing::warn!("mixage : {e:#}");
    }
    set_activity(shared, 0, None);
}

/// The editor pass on the current storyboard, then the search for the
/// shots and sounds (copied into the library).
async fn run_write(
    engine: &Engine,
    shared: &Shared,
    snap: &Snapshot,
    request: &WriteRequest,
) -> Result<(enfait_ai::editor::EditPlan, enfait_studio::magic::Gathered)> {
    use enfait_assets::search::Source;
    let say = |text: &str| {
        if let Ok(mut s) = shared.write_state.lock() {
            *s = WriteState::Running(text.to_owned());
        }
        shared.changed.store(true, Ordering::Release);
    };
    say(&t!("worker.editor_writing"));
    let provider = provider_for(engine, snap.provider).await?;
    let blocks =
        enfait_studio::magic::editor_blocks(&snap.project, &snap.transcript, request.keep_authored);
    let settings = snap.project.settings();
    let preset = snap
        .look
        .style
        .preset
        .as_deref()
        .and_then(enfait_ai::styles::find);
    let style = enfait_ai::styles::combine(preset, snap.look.rules().as_deref());
    let library = enfait_studio::magic::library_lines(&snap.look);
    let input = enfait_ai::editor::EditorInput {
        width: settings.resolution.width,
        height: settings.resolution.height,
        language: &snap.transcript.language,
        blocks: &blocks,
        has_video: snap.rush_has_video,
        video_banks: Source::Pexels.available() || Source::Pixabay.available(),
        script: Some(snap.script.as_str()),
        user_style: style.as_deref(),
        library: library.as_deref(),
        max_transitions: enfait_studio::magic::max_transitions(blocks.len()),
    };
    let progress = |part: usize, parts: usize| {
        say(&t!("worker.editor_part", part = part, count = parts));
    };
    let plan = enfait_studio::magic::write_plan(
        &provider,
        &engine.dir,
        &input,
        request.quality,
        request.blocks_per_request,
        &progress,
    )
    .await?;
    let gathered = enfait_studio::magic::gather(
        &plan,
        &snap.project,
        &snap.look,
        &snap.library,
        &engine.dir.join("downloads"),
        &say,
    )
    .await?;
    Ok((plan, gathered))
}

/// Exports the video or the Premiere edit; returns the final state.
async fn run_export(
    dir: &Path,
    shared: &Shared,
    snapshot: &Snapshot,
    request: &ExportRequest,
) -> ExportState {
    // B-roll shots not edited yet (the preview had no time to).
    mount_brolls(dir, snapshot, shared).await;
    let sources = snapshot.sources(dir);
    let missing_note = |missing: &[usize]| match missing {
        [] => String::new(),
        [one] => t!("worker.missing_one", n = one).into_owned(),
        many => t!(
            "worker.missing_many",
            count = many.len(),
            list = many
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )
        .into_owned(),
    };
    let result = match request.kind {
        ExportKind::Video => {
            let progress = |p: f64| {
                if let Ok(mut s) = shared.export_state.lock() {
                    *s = ExportState::Running {
                        kind: ExportKind::Video,
                        progress: p,
                    };
                }
                shared.changed.store(true, Ordering::Release);
            };
            enfait_studio::export::video(
                &sources,
                enfait_studio::export::ExportOptions {
                    sfx: request.sfx,
                    ..Default::default()
                },
                &request.path,
                &progress,
            )
            .await
            .map(|r| {
                let minutes = (r.duration / 60.0).floor();
                let loudness = r
                    .measured_lufs
                    .map(|l| t!("worker.loudness", lufs = format!("{l:.1}")).into_owned())
                    .unwrap_or_default();
                t!(
                    "worker.video_done",
                    min = format!("{minutes:.0}"),
                    sec = format!("{:02.0}", r.duration - minutes * 60.0),
                    loudness = loudness,
                    missing = missing_note(&r.missing)
                )
                .into_owned()
            })
        }
        ExportKind::Premiere => {
            let (sources, path, sfx) = (sources.clone(), request.path.clone(), request.sfx);
            tokio::task::block_in_place(|| {
                enfait_studio::premiere::export(
                    &sources,
                    enfait_studio::premiere::PremiereOptions { sfx },
                    &path,
                )
            })
            .map(|r| {
                t!(
                    "worker.premiere_done",
                    media = r
                        .media
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    missing = missing_note(&r.missing)
                )
                .into_owned()
            })
        }
    };
    match result {
        Ok(message) => ExportState::Done {
            path: request.path.clone(),
            message,
        },
        Err(e) => {
            tracing::warn!("export : {e:#}");
            ExportState::Failed(format!("{e:#}"))
        }
    }
}

/// Mix plan of the finish, on the storyboard edit.
pub fn mix_plan(dir: &Path, snapshot: &Snapshot) -> enfait_studio::mix::MixPlan {
    enfait_studio::mix::plan(&snapshot.sources(dir), true)
}

/// Sound of the preview: the base (picture + voice) mixed with the finish.
async fn mix_preview(dir: &Path, snapshot: &Snapshot) -> Result<()> {
    enfait_studio::mix::apply(
        &preview_base_path(dir),
        &mix_plan(dir, snapshot),
        &preview_path(dir),
    )
    .await
}

/// Edits the missing or outdated b-roll shots (no model, ffmpeg only).
async fn mount_brolls(dir: &Path, snapshot: &Snapshot, shared: &Shared) {
    let settings = snapshot.project.settings();
    let rate = settings.frame_rate;
    let fps = f64::from(rate.num()) / f64::from(rate.den());
    let size = (settings.resolution.width, settings.resolution.height);
    for (i, block) in snapshot.project.blocks().iter().enumerate() {
        if block.kind != BlockKind::Broll || broll_ready(dir, block, settings) {
            continue;
        }
        let (Some(broll), Some(key)) = (&block.broll, broll_key(block, settings)) else {
            continue;
        };
        let Some(asset) = snapshot.look.assets.iter().find(|a| a.file == broll.file) else {
            tracing::warn!(
                "block {}: {} is no longer in the library",
                i.saturating_add(1),
                broll.file
            );
            continue;
        };
        set_activity(
            shared,
            0,
            Some(t!("worker.mounting", n = i.saturating_add(1)).into_owned()),
        );
        let clip = broll_clip_path(dir, block.id, settings);
        let result = enfait_studio::broll::render(
            &snapshot.library.join(&asset.file),
            asset.kind,
            broll.start.as_secs_f64(),
            block.range.duration().as_secs_f64(),
            size,
            fps,
            &clip,
        )
        .await
        .and_then(|()| Ok(std::fs::write(clip.with_extension("key"), &key)?));
        if let Err(e) = result {
            tracing::warn!("block {}: b-roll shot: {e:#}", i.saturating_add(1));
        }
    }
}

/// Storyboard preview, then its mix; removed if there is no clip, no
/// sound finish and no reframing of the take.
async fn build_preview(dir: &Path, snapshot: &Snapshot) -> Result<()> {
    let visuals = enfait_studio::session::Visuals::of(dir, &snapshot.project);
    let out = preview_base_path(dir);
    let sound = !mix_plan(dir, snapshot).is_empty();
    if visuals.rush_only() && !sound && !snapshot.reframed() {
        let _ = tokio::fs::remove_file(&out).await;
        let _ = tokio::fs::remove_file(preview_path(dir)).await;
        return Ok(());
    }
    let settings = snapshot.project.settings();
    let size = preview_size(settings.resolution.width, settings.resolution.height);
    let rate = settings.frame_rate;
    let fps = f64::from(rate.num()) / f64::from(rate.den());
    let mut output = Output::preview(size, fps);
    output.framing = enfait_media::fx::framing(size.0, size.1, settings.framing, settings.pan());
    assemble(
        &snapshot.rush,
        snapshot.rush_has_video,
        &snapshot.parts,
        &pieces(&visuals.blocks, &visuals.transitions),
        &output,
        &out,
    )
    .await?;
    mix_preview(dir, snapshot).await
}

fn cache_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "enfait").map_or_else(
        || std::env::temp_dir().join("enfait"),
        |d| d.cache_dir().to_path_buf(),
    )
}

/// The Chromium instances, started when first needed.
async fn renderers_for<'a>(
    engine: &'a Engine,
    say: &(dyn Fn(&str) + Sync),
) -> Result<&'a RendererPool> {
    engine
        .renderers
        .get_or_try_init(|| async {
            say(&t!("worker.preparing_render"));
            let cache = cache_dir();
            let chrome = enfait_motion::ensure_browser(&cache.join("chromium")).await?;
            let gpu = if enfait_config::process::which("nvidia-smi").is_some() {
                Gpu::Vulkan {
                    nvidia_offload: true,
                }
            } else {
                Gpu::Software
            };
            anyhow::Ok(RendererPool::launch(&chrome, &cache.join("profile"), gpu, MAX_JOBS).await?)
        })
        .await
}

/// YouTube thumbnail: a new proposal, or an export.
async fn run_thumbnail(
    engine: &Engine,
    shared: &Shared,
    snap: &Snapshot,
    request: &ThumbnailRequest,
) -> Result<ThumbnailState> {
    use enfait_studio::thumbnail::{self, Step};
    let say = |text: &str| {
        if let Ok(mut s) = shared.thumbnail_state.lock() {
            *s = ThumbnailState::Running(text.to_owned());
        }
        shared.changed.store(true, Ordering::Release);
    };
    let dir = thumbnail::dir(&engine.dir);
    match request {
        ThumbnailRequest::Generate {
            title,
            notes,
            quality,
        } => {
            let provider = provider_for(engine, snap.provider).await?;
            let renderers = renderers_for(engine, &say).await?;
            let mut fonts = enfait_studio::fonts_list();
            fonts.push_str(&enfait_studio::userlib::font_lines(&snap.look));
            let preset = snap
                .look
                .style
                .preset
                .as_deref()
                .and_then(enfait_ai::styles::find);
            let settings = quality.settings();
            let input = thumbnail::Input {
                sources: snap.sources(&engine.dir),
                has_video: snap.rush_has_video,
                title,
                notes,
                model: settings.model,
                effort: settings.effort,
                fonts,
                user_style: enfait_ai::styles::combine(preset, snap.look.rules().as_deref()),
            };
            let step = |step: Step| {
                say(&match step {
                    Step::Stills => t!("worker.thumb_stills"),
                    Step::Cutout => t!("worker.thumb_cutout"),
                    Step::Designing => t!("worker.thumb_designing"),
                    Step::Rendering => t!("worker.thumb_rendering"),
                    Step::Fixing => t!("worker.thumb_fixing"),
                });
            };
            let version =
                thumbnail::generate(&provider, renderers, &engine.usage, &dir, &input, &step)
                    .await?;
            enfait_studio::log_usage(&engine.usage);
            Ok(ThumbnailState::Generated(version.number))
        }
        ThumbnailRequest::Image { version, path } => {
            say(&t!("worker.thumb_exporting"));
            let (dir, version, out) = (dir.clone(), *version, path.clone());
            let bytes =
                tokio::task::spawn_blocking(move || thumbnail::export_image(&dir, version, &out))
                    .await??;
            #[allow(clippy::cast_precision_loss)]
            let kb = bytes as f64 / 1024.0;
            Ok(ThumbnailState::Exported {
                path: path.clone(),
                message: t!("worker.thumb_image_done", kb = format!("{kb:.0}")).into_owned(),
            })
        }
        ThumbnailRequest::Psd { version, path } => {
            let renderers = renderers_for(engine, &say).await?;
            say(&t!("worker.thumb_layers"));
            let renderer = renderers.acquire().await?;
            let layers = thumbnail::export_psd(&renderer, &dir, *version, path).await?;
            Ok(ThumbnailState::Exported {
                path: path.clone(),
                message: t!("worker.thumb_psd_done", count = layers).into_owned(),
            })
        }
    }
}

/// Chosen model provider, recreated when the choice changes.
/// An error for the interface: a local model too slow is said plainly.
fn describe(e: &anyhow::Error) -> String {
    let timed_out = e.chain().find_map(|c| match c.downcast_ref::<enfait_ai::AiError>() {
        Some(enfait_ai::AiError::TimedOut { secs }) => Some(*secs),
        _ => None,
    });
    match timed_out {
        Some(secs) => t!("worker.local_timeout", min = secs.div_ceil(60)).into_owned(),
        None => format!("{e:#}"),
    }
}

async fn provider_for(engine: &Engine, choice: ProviderChoice) -> Result<Arc<Provider>> {
    let key = (choice, enfait_config::generation());
    let mut slot = engine.provider.lock().await;
    if let Some((current, provider)) = slot.as_ref()
        && *current == key
    {
        return Ok(Arc::clone(provider));
    }
    let provider = Arc::new(choice.make(&engine.dir.join("claude"))?);
    *slot = Some((key, Arc::clone(&provider)));
    Ok(provider)
}

/// Direction kept from one job to the next, as long as script, style and
/// format do not change; intents are found again by block text.
#[derive(Serialize, Deserialize)]
struct SavedDirection {
    key: String,
    texts: Vec<String>,
    direction: Direction,
}

#[allow(clippy::too_many_arguments)]
async fn direction_for(
    engine: &Engine,
    provider: &Provider,
    snap: &Snapshot,
    ranges: &[TimeRange],
    texts: &[String],
    size: (u32, u32),
    style: Option<&str>,
    fonts: &str,
) -> Result<Direction> {
    let _guard = engine.direction.lock().await;
    let mut h = blake3::Hasher::new();
    for part in [snap.script.as_str(), style.unwrap_or_default(), fonts] {
        h.update(part.as_bytes());
        h.update(&[0]);
    }
    h.update(&size.0.to_le_bytes());
    h.update(&size.1.to_le_bytes());
    let key = h.finalize().to_hex().to_string();
    let path = engine.dir.join("direction.app.json");
    let saved = tokio::fs::read(&path)
        .await
        .ok()
        .and_then(|d| serde_json::from_slice::<SavedDirection>(&d).ok())
        .filter(|s| s.key == key);
    let saved = match saved {
        Some(saved) => saved,
        None => {
            let direction = enfait_studio::direct(
                provider,
                &engine.dir,
                ranges,
                texts,
                size.0,
                size.1,
                Some(&snap.script),
                style,
                fonts,
                0,
            )
            .await?;
            let saved = SavedDirection {
                key,
                texts: texts.to_vec(),
                direction,
            };
            tokio::fs::write(&path, serde_json::to_vec_pretty(&saved)?).await?;
            saved
        }
    };
    // Intents of the current blocks, found by their text; a new block
    // follows the general direction.
    let mut direction = saved.direction.clone();
    direction.blocks = texts
        .iter()
        .enumerate()
        .map(|(i, text)| BlockIntent {
            index: i.saturating_add(1),
            intent: saved
                .texts
                .iter()
                .position(|t| t == text)
                .map(|p| saved.direction.intent(p.saturating_add(1)).to_owned())
                .unwrap_or_default(),
        })
        .collect();
    direction.transitions.clear();
    Ok(direction)
}

async fn run_job(engine: &Engine, shared: &Shared, job: &Job, snap: &Snapshot) -> Result<()> {
    let blocks = snap.project.blocks();
    let index = blocks
        .iter()
        .position(|b| b.id == job.block)
        .with_context(|| t!("worker.block_gone").into_owned())?;
    let block = blocks
        .get(index)
        .with_context(|| t!("worker.block_missing").into_owned())?;
    let number = index.saturating_add(1);
    let what = if job.kind == JobKind::Transition {
        t!(
            "worker.what_transition",
            from = number,
            to = number.saturating_add(1)
        )
        .into_owned()
    } else {
        t!("worker.what_block", n = number).into_owned()
    };
    let say = |text: &str| {
        set_activity(
            shared,
            job.id,
            Some(t!("worker.activity", what = what, text = text).into_owned()),
        );
    };
    say(&t!("worker.starting"));

    let provider = provider_for(engine, snap.provider).await?;
    let provider = &*provider;
    let renderers = renderers_for(engine, &say).await?;

    let settings = snap.project.settings();
    let (width, height) = (settings.resolution.width, settings.resolution.height);
    let rate = settings.frame_rate;
    let fps = f64::from(rate.num()) / f64::from(rate.den());
    let ranges: Vec<TimeRange> = blocks.iter().map(|b| b.range).collect();
    let texts: Vec<String> = ranges.iter().map(|r| snap.transcript.text_in(*r)).collect();
    let mut fonts = enfait_studio::fonts_list();
    fonts.push_str(&enfait_studio::userlib::font_lines(&snap.look));
    let preset = snap
        .look
        .style
        .preset
        .as_deref()
        .and_then(enfait_ai::styles::find);
    let style = enfait_ai::styles::combine(preset, snap.look.rules().as_deref());

    {
        let _guard = engine.install.lock();
        let comps = engine.dir.join("comps");
        library::install(&comps).context("library of the compositions")?;
        enfait_studio::userlib::install(&snap.look, &snap.library, &comps)?;
    }

    say(&t!("worker.direction"));
    let direction = direction_for(
        engine,
        provider,
        snap,
        &ranges,
        &texts,
        (width, height),
        style.as_deref(),
        &fonts,
    )
    .await?;
    let art = direction.art_direction();
    let quality = job.quality.settings();
    let mut studio = Studio {
        work: engine.dir.clone(),
        provider,
        renderers,
        animator: AnimatorConfig {
            system: animator_system(composition::CONTRACT, &fonts),
            model: quality.model,
            effort: quality.effort,
        },
        critique_rounds: quality.critique_rounds,
        width,
        height,
        fps,
        usage: &engine.usage,
        scene_frames: None,
        scene_cutouts: None,
    };

    let phase = |p: Phase| set_phase(shared, job.id, Some(p));
    if job.kind == JobKind::Transition {
        phase(Phase::Editing);
        say(&t!("worker.transition"));
        return render_transition(engine, snap, &studio, &direction, index, true).await;
    }

    anyhow::ensure!(
        block.kind != BlockKind::Rush,
        t!("worker.rush_block").into_owned()
    );
    anyhow::ensure!(
        block.kind != BlockKind::Broll,
        t!("worker.broll_block").into_owned()
    );
    let mut brief = enfait_studio::brief(
        index,
        &ranges,
        &texts,
        &snap.transcript,
        &direction,
        &art,
        width,
        height,
        style.as_deref(),
    );
    if !block.description.trim().is_empty() {
        brief.intent = format!(
            "User's instruction (takes precedence): {}\n\nDirector's idea, to follow only \
             if it serves the instruction: {}",
            block.description.trim(),
            brief.intent
        );
    }
    let start = block.range.start().as_secs_f64();
    brief.moments = block
        .moments
        .iter()
        .map(|m| (m.at.as_secs_f64() - start, m.word.clone(), m.note.clone()))
        .collect();
    brief.library = enfait_studio::userlib::brief(&snap.look);
    if let Some(named) = snap.look.mentions_brief(&block.description) {
        brief.intent.push_str("\n\n");
        brief.intent.push_str(&named);
    }
    brief.layer = match block.kind {
        BlockKind::Mixed if block.cutout => Some(SceneLayer::MixedCutout),
        BlockKind::Mixed => Some(SceneLayer::Mixed),
        BlockKind::Effect => Some(SceneLayer::Effect),
        _ => None,
    };
    let name = block_name(block.id, snap.project.settings());
    if brief.layer.is_some() {
        anyhow::ensure!(snap.rush_has_video, t!("worker.needs_video").into_owned());
        say(&t!("worker.rush_frames"));
        let pieces = source_pieces(&snap.parts, start, block.range.end().as_secs_f64());
        let dir = engine.dir.join("rush-frames").join(&name);
        let count = enfait_studio::extract_frames_framed(
            &snap.rush,
            &pieces,
            fps,
            &rush_framing(snap),
            &dir,
        )
        .await?;
        if brief.layer == Some(SceneLayer::MixedCutout) {
            let out = engine.dir.join("rush-cutouts").join(&name);
            cut_person(&dir, count, &out, &say).await?;
            studio.scene_cutouts = Some(out);
        }
        studio.scene_frames = Some((dir, count));
    }
    phase(Phase::Editing);
    match &job.kind {
        JobKind::Create => {
            say(&t!("worker.animation"));
            enfait_studio::animate(&studio, &name, &brief, true).await?;
        }
        JobKind::Revise { notes } => {
            say(&t!("worker.retouch"));
            enfait_studio::revise(&studio, &name, &brief, notes).await?;
        }
        JobKind::Restore { version } => {
            say(&t!("worker.restore", version = version));
            enfait_studio::restore(&studio, &name, *version).await?;
        }
        JobKind::Transition => {}
    }
    phase(Phase::Finishing);
    say(&t!("worker.render_clip"));
    enfait_studio::render_clip(
        &studio,
        &name,
        brief.duration,
        &clip_path(&engine.dir, block.id, snap.project.settings()),
    )
    .await?;
    // Transitions already made that touch this block take its new frames
    // (same composition, no model call).
    for before in [index.checked_sub(1), Some(index)].into_iter().flatten() {
        let ready = blocks.get(before).is_some_and(|b| {
            transition_clip_path(&engine.dir, b.id, snap.project.settings()).is_file()
        });
        if ready {
            say(&t!("worker.update_transitions"));
            if let Err(e) =
                render_transition(engine, snap, &studio, &direction, before, false).await
            {
                tracing::warn!("transition after block {}: {e:#}", before.saturating_add(1));
            }
        }
    }
    Ok(())
}

/// Cuts the person out of the `count` frames of `frames` (fast model, on
/// several threads); transparent PNGs in `out`.
async fn cut_person(
    frames: &Path,
    count: usize,
    out: &Path,
    say: &(dyn Fn(&str) + Sync),
) -> Result<()> {
    use enfait_assets::cutout::{Cutter, Model, cut_frames, ensure_model};
    say(&t!("worker.cutout_model"));
    let models = enfait_config::paths::cutout_models_dir();
    tokio::fs::create_dir_all(&models).await?;
    let model = ensure_model(&models, Model::Fast).await?;
    if tokio::fs::try_exists(out).await.unwrap_or(false) {
        tokio::fs::remove_dir_all(out).await?;
    }
    tokio::fs::create_dir_all(out).await?;
    let jobs: Vec<(PathBuf, PathBuf)> = (0..count)
        .map(|i| {
            (
                frames.join(frame_name(i)),
                out.join(enfait_studio::cutout_name(i)),
            )
        })
        .collect();
    let threads = std::thread::available_parallelism()
        .map_or(2, |n| n.get() / 2)
        .clamp(1, 6);
    tokio::task::block_in_place(|| {
        let cutter = Cutter::load(&model, Model::Fast)?;
        cut_frames(&cutter, &jobs, threads, &|done| {
            if done % 10 == 0 || done == count {
                say(&t!("worker.cutout_progress", done = done, count = count));
            }
        })
    })?;
    Ok(())
}

/// Creates (`redo`) or re-renders the transition after block `index`.
async fn render_transition(
    engine: &Engine,
    snap: &Snapshot,
    studio: &Studio<'_>,
    direction: &Direction,
    index: usize,
    redo: bool,
) -> Result<()> {
    let blocks = snap.project.blocks();
    let block = blocks
        .get(index)
        .with_context(|| t!("worker.block_missing").into_owned())?;
    let next = blocks
        .get(index.saturating_add(1))
        .with_context(|| t!("worker.no_next").into_owned())?;
    let description = match &block.transition_after {
        Transition::Cut => anyhow::bail!(t!("worker.hard_cut").into_owned()),
        Transition::Auto => "Free: choose the transition that best serves the passage between \
                             these two scenes, within the art direction."
            .to_owned(),
        Transition::Described(text) => text.clone(),
    };
    let scenes = transition_scenes(
        &engine.dir,
        snap,
        block,
        next,
        studio.fps,
        (studio.width, studio.height),
    )
    .await?;
    let number = index.saturating_add(1);
    #[allow(clippy::cast_precision_loss)]
    let brief = TransitionBrief {
        width: studio.width,
        height: studio.height,
        duration: scenes.len() as f64 / studio.fps,
        art_direction: direction.art_direction(),
        description,
        from_intent: direction.intent(number).to_owned(),
        to_intent: direction.intent(number.saturating_add(1)).to_owned(),
    };
    enfait_studio::transition_clip(
        studio,
        &transition_name(block.id, snap.project.settings()),
        &brief,
        &scenes,
        redo,
        &transition_clip_path(&engine.dir, block.id, snap.project.settings()),
    )
    .await
}

/// Frames of both shots of a transition, one pair per frame: the outgoing
/// shot (its clip, else the take) up to the cut, then frozen; the
/// incoming shot frozen up to the cut, then moving.
async fn transition_scenes(
    dir: &Path,
    snap: &Snapshot,
    block: &Block,
    next: &Block,
    fps: f64,
    size: (u32, u32),
) -> Result<Vec<(PathBuf, PathBuf)>> {
    let (w0, w1) = transition_window(block, next);
    let boundary = block.range.end().as_secs_f64();
    let from_dir = dir
        .join("transition-frames")
        .join(format!("{}-depart", block.id));
    let to_dir = dir
        .join("transition-frames")
        .join(format!("{}-arrivee", block.id));
    let from_n = plan_frames(dir, snap, block, (w0, boundary), fps, size, &from_dir).await?;
    let to_n = plan_frames(dir, snap, next, (boundary, w1), fps, size, &to_dir).await?;
    let total = from_n.saturating_add(to_n);
    Ok((0..total)
        .map(|i| {
            let f = i.min(from_n.saturating_sub(1));
            let t = i.saturating_sub(from_n).min(to_n.saturating_sub(1));
            (from_dir.join(frame_name(f)), to_dir.join(frame_name(t)))
        })
        .collect())
}

/// Frames of a shot over the edit interval `span`: its clip if it is
/// animated, else the take. Returns their count.
async fn plan_frames(
    dir: &Path,
    snap: &Snapshot,
    block: &Block,
    span: (f64, f64),
    fps: f64,
    size: (u32, u32),
    out: &Path,
) -> Result<usize> {
    let (a, z) = span;
    if let Some(clip) = visual_clip(dir, block, snap.project.settings()) {
        let start = block.range.start().as_secs_f64();
        return enfait_studio::extract_frames(&clip, &[(a - start, z - start)], fps, size, out)
            .await;
    }
    anyhow::ensure!(snap.rush_has_video, t!("worker.no_picture").into_owned());
    enfait_studio::extract_frames_framed(
        &snap.rush,
        &source_pieces(&snap.parts, a, z),
        fps,
        &rush_framing(snap),
        out,
    )
    .await
}

/// Framing of the take in the project format.
fn rush_framing(snap: &Snapshot) -> String {
    let settings = snap.project.settings();
    enfait_media::fx::framing(
        settings.resolution.width,
        settings.resolution.height,
        settings.framing,
        settings.pan(),
    )
}
