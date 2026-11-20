//! Player: viewer, J/K/L transport and clickable waveform.
//!
//! The [`Player`] is a gpui entity owning the prepared media, the clock, the
//! frame decoder and the audio output. It renders itself (frame and
//! transport bar); the [`Player::strip`] (waveform and playhead) is placed
//! by the window in the bottom strip.

use crate::fonts;
use crate::theme::{color, tokens};
use crate::tr;
use enfait_core::{FrameRate, PostFx, Timecode};
use enfait_media::video::frame_at;
use enfait_media::{
    Clock, Command, Edl, MediaCache, Peaks, Playback, Prepared, Stage, VideoDecoder, VideoSource,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Bounds, Context, FontWeight, MouseButton, MouseDownEvent, MouseMoveEvent,
    ObjectFit, Pixels, RenderImage, SharedString, Window, canvas, div, fill, img, point, px, size,
};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Height of the waveform strip.
pub const STRIP_HEIGHT: f32 = 56.;

pub struct Player {
    cache: PathBuf,
    state: State,
    /// Number of the last load: a replaced load is not applied.
    generation: u64,
    wave: Rc<RefCell<WaveCache>>,
    /// Images to release from the atlas at the next render.
    stale: Vec<Arc<RenderImage>>,
    /// Edit requested before the load finished.
    pending_cut: Option<Vec<(f64, f64)>>,
    /// File being loaded.
    loading: Option<PathBuf>,
    /// Time to seek to once the media is loaded.
    pending_seek: Option<f64>,
    /// Post-FX applied to the frames (preview).
    fx: PostFx,
    /// Frames decoded from the original file at its own size, rather than
    /// from the proxy (sharper, heavier).
    native: bool,
    /// The player fills the window.
    expanded: bool,
    /// The window was put fullscreen by the player (and goes back on exit).
    made_fullscreen: bool,
}

enum State {
    Empty,
    Loading {
        name: SharedString,
        progress: Arc<Mutex<(Stage, f32)>>,
    },
    Failed(SharedString),
    Ready(Box<Loaded>),
}

struct Loaded {
    name: SharedString,
    prepared: Prepared,
    peaks: Option<Arc<Peaks>>,
    /// Edit being played; the clock is in edit time.
    edl: Arc<Edl>,
    clock: Arc<Clock>,
    /// Frame rate of the video, or 30 fps for sound only (timecode).
    rate: FrameRate,
    decoder: Option<VideoDecoder>,
    /// Progress of the proxy while it is made (frames read from the
    /// original meanwhile).
    proxy_progress: Option<Arc<Mutex<f32>>>,
    playback: Playback,
    image: Option<(i64, Arc<RenderImage>)>,
}

/// Waveform columns, recomputed when the width changes.
#[derive(Default)]
struct WaveCache {
    /// Width, duration and edit the columns are computed for.
    key: Option<(u64, u64, usize)>,
    columns: Rc<Vec<(f32, f32)>>,
    bounds: Option<Bounds<Pixels>>,
}

fn file_name(path: &std::path::Path) -> SharedString {
    path.file_name()
        .map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
        .into()
}

impl Player {
    pub fn new(cache: PathBuf, native: bool) -> Self {
        Self {
            cache,
            state: State::Empty,
            generation: 0,
            wave: Rc::default(),
            stale: Vec::new(),
            pending_cut: None,
            loading: None,
            pending_seek: None,
            fx: PostFx::NEUTRAL,
            native,
            expanded: false,
            made_fullscreen: false,
        }
    }

    /// Empties the player (another project without media).
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        self.release();
        self.loading = None;
        self.pending_cut = None;
        self.pending_seek = None;
        self.state = State::Empty;
        cx.notify();
    }

    /// Prepares `path` in the background, then makes it playable.
    pub fn open(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.release();
        let progress = Arc::new(Mutex::new((Stage::Probe, 0.0)));
        self.loading = std::path::absolute(&path).ok();
        self.state = State::Loading {
            name: file_name(&path),
            progress: Arc::clone(&progress),
        };
        let cache = MediaCache::new(self.cache.clone());
        let work = cx.background_spawn(async move {
            cache.prepare(&path, &mut |stage, p| {
                if let Ok(mut slot) = progress.lock() {
                    *slot = (stage, p);
                }
            })
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |player, cx| {
                if player.generation == generation {
                    player.finish(result, cx);
                }
            });
        })
        .detach();
        // Refreshes the progress while loading.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let loading = this.update(cx, |player, cx| {
                    cx.notify();
                    player.generation == generation && matches!(player.state, State::Loading { .. })
                });
                if loading.ok() != Some(true) {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// Stops playback and keeps the displayed frame to release it.
    fn release(&mut self) {
        if let State::Ready(loaded) = &mut self.state
            && let Some((_, image)) = loaded.image.take()
        {
            self.stale.push(image);
        }
        self.state = State::Empty;
        *self.wave.borrow_mut() = WaveCache::default();
    }

    fn finish(
        &mut self,
        result: Result<Prepared, enfait_media::MediaError>,
        cx: &mut Context<Self>,
    ) {
        self.state = match result {
            Err(e) => {
                tracing::warn!("cannot open: {e}");
                State::Failed(e.to_string().into())
            }
            Ok(prepared) => {
                let rate = prepared.frame_rate().unwrap_or(FrameRate::FPS_30);
                let edl = Arc::new(match self.pending_cut.take() {
                    Some(ranges) => Edl::from_ranges(&ranges),
                    None => Edl::identity(prepared.info.duration),
                });
                let clock = Arc::new(Clock::new(edl.duration(), frame_duration(rate)));
                if let Some(t) = self.pending_seek.take() {
                    clock.apply(Command::Seek(t));
                }
                let decoder = video_source(&prepared, self.native)
                    .map(|v| decoder_for(&v, &prepared, rate, &edl, &clock, &self.fx));
                let playback =
                    Playback::start(Arc::clone(&clock), prepared.audio.clone(), Arc::clone(&edl));
                let proxy_progress = prepared
                    .video
                    .as_ref()
                    .filter(|v| !v.is_proxy)
                    .map(|_| self.build_proxy(&prepared, cx));
                State::Ready(Box::new(Loaded {
                    proxy_progress,
                    name: file_name(&prepared.source),
                    peaks: prepared.peaks.clone().map(Arc::new),
                    prepared,
                    edl,
                    clock,
                    rate,
                    decoder,
                    playback,
                    image: None,
                }))
            }
        };
        cx.notify();
    }

    /// Makes the proxy in the background, then replaces the decoder of the
    /// original with the proxy's. Returns the shared progress.
    fn build_proxy(&self, prepared: &Prepared, cx: &mut Context<Self>) -> Arc<Mutex<f32>> {
        let generation = self.generation;
        let progress = Arc::new(Mutex::new(0.0_f32));
        let shared = Arc::clone(&progress);
        let cache = MediaCache::new(self.cache.clone());
        let prepared = prepared.clone();
        let work = cx.background_spawn(async move {
            cache.build_proxy(&prepared, &mut |p| {
                if let Ok(mut slot) = shared.lock() {
                    *slot = p;
                }
            })
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |player, cx| {
                if player.generation != generation {
                    return;
                }
                let State::Ready(l) = &mut player.state else {
                    return;
                };
                l.proxy_progress = None;
                match result {
                    Ok(Some(video)) => {
                        // In native quality the original keeps being read.
                        if !player.native {
                            l.decoder = Some(decoder_for(
                                &video,
                                &l.prepared,
                                l.rate,
                                &l.edl,
                                &l.clock,
                                &player.fx,
                            ));
                        }
                        l.prepared.video = Some(video);
                    }
                    Ok(None) => {}
                    // The original stays playable: carry on without a proxy.
                    Err(e) => tracing::warn!("no proxy, playing from the original: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
        // Refreshes the transport bar while the proxy is made.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let building = this.update(cx, |player, cx| {
                    cx.notify();
                    player.generation == generation
                        && matches!(&player.state, State::Ready(l) if l.proxy_progress.is_some())
                });
                if building.ok() != Some(true) {
                    break;
                }
            }
        })
        .detach();
        progress
    }

    pub fn command(&mut self, command: Command, cx: &mut Context<Self>) {
        if let State::Ready(loaded) = &self.state {
            let command = match command {
                // One-second jumps are expressed in frames.
                Command::Step(n) if n.abs() > 1 => Command::Step(
                    n.signum()
                        .saturating_mul(i64::from(loaded.rate.nominal_fps())),
                ),
                other => other,
            };
            loaded.clock.apply(command);
            cx.notify();
        }
    }

    /// Plays `ranges` (pieces of the source, in seconds) end to end, or the
    /// whole source with `None`. Playback pauses, on the same source time if
    /// it is part of the new edit.
    pub fn set_cut(&mut self, ranges: Option<Vec<(f64, f64)>>, cx: &mut Context<Self>) {
        let fx = self.fx;
        let State::Ready(l) = &mut self.state else {
            self.pending_cut = ranges;
            return;
        };
        let source = l.edl.to_source(l.clock.position());
        let edl = Arc::new(match ranges {
            Some(r) => Edl::from_ranges(&r),
            None => Edl::identity(l.prepared.info.duration),
        });
        if *edl == *l.edl {
            return;
        }
        let clock = Arc::new(Clock::new(edl.duration(), frame_duration(l.rate)));
        clock.apply(Command::Seek(edl.to_output(source).unwrap_or(0.0)));
        // Stop the old playback before opening the new audio output.
        l.decoder = None;
        l.playback = Playback::start(
            Arc::clone(&clock),
            l.prepared.audio.clone(),
            Arc::clone(&edl),
        );
        l.decoder = video_source(&l.prepared, self.native)
            .map(|v| decoder_for(&v, &l.prepared, l.rate, &edl, &clock, &fx));
        l.edl = edl;
        l.clock = clock;
        *self.wave.borrow_mut() = WaveCache::default();
        cx.notify();
    }

    /// Seeks to `t` (edit time), now or as soon as the media being opened
    /// is ready.
    pub fn queue_seek(&mut self, t: f64) {
        match &self.state {
            State::Ready(l) => l.clock.apply(Command::Seek(t)),
            _ => self.pending_seek = Some(t),
        }
    }

    /// Preview post-FX: restarts the frame decoder with these filters.
    pub fn set_fx(&mut self, fx: PostFx, cx: &mut Context<Self>) {
        if fx == self.fx {
            return;
        }
        self.fx = fx;
        if let State::Ready(l) = &mut self.state {
            l.decoder = None;
            l.decoder = video_source(&l.prepared, self.native)
                .map(|v| decoder_for(&v, &l.prepared, l.rate, &l.edl, &l.clock, &fx));
        }
        cx.notify();
    }

    /// Native quality: frames from the original file at its own size.
    pub fn set_native(&mut self, native: bool, cx: &mut Context<Self>) {
        if native == self.native {
            return;
        }
        self.native = native;
        crate::prefs::update(cx, |s| s.player_native = native);
        if let State::Ready(l) = &mut self.state {
            l.decoder = None;
            l.decoder = video_source(&l.prepared, native)
                .map(|v| decoder_for(&v, &l.prepared, l.rate, &l.edl, &l.clock, &self.fx));
        }
        cx.notify();
    }

    pub const fn is_expanded(&self) -> bool {
        self.expanded
    }

    /// The player fills the window, put fullscreen (Escape goes back).
    pub fn set_expanded(&mut self, expanded: bool, window: &mut Window, cx: &mut Context<Self>) {
        if expanded == self.expanded {
            return;
        }
        self.expanded = expanded;
        if expanded {
            self.made_fullscreen = !window.is_fullscreen();
            if self.made_fullscreen {
                window.toggle_fullscreen();
            }
        } else if std::mem::take(&mut self.made_fullscreen) && window.is_fullscreen() {
            window.toggle_fullscreen();
        }
        cx.notify();
    }

    /// Edit being played (if a media is ready).
    pub fn edl(&self) -> Option<Arc<Edl>> {
        match &self.state {
            State::Ready(l) => Some(Arc::clone(&l.edl)),
            _ => None,
        }
    }

    /// Analysis of the open media.
    pub fn media_info(&self) -> Option<&enfait_media::MediaInfo> {
        match &self.state {
            State::Ready(l) => Some(&l.prepared.info),
            _ => None,
        }
    }

    /// Playhead, in edit time.
    pub fn position(&self) -> Option<f64> {
        match &self.state {
            State::Ready(l) => Some(l.clock.position()),
            _ => None,
        }
    }

    /// Source time being played.
    pub fn source_position(&self) -> Option<f64> {
        match &self.state {
            State::Ready(l) => Some(l.edl.to_source(l.clock.position())),
            _ => None,
        }
    }

    /// File open or being opened.
    pub fn is_open(&self, path: &std::path::Path) -> bool {
        match &self.state {
            State::Ready(l) => l.prepared.source == path,
            State::Loading { .. } => self.loading.as_deref() == Some(path),
            _ => false,
        }
    }

    pub fn is_playing(&self) -> bool {
        matches!(&self.state, State::Ready(l) if l.clock.is_playing())
    }

    /// Timecode of the playhead.
    pub fn timecode(&self) -> Option<String> {
        let State::Ready(l) = &self.state else {
            return None;
        };
        let frame = frame_at(l.clock.position(), l.rate);
        let t = l.rate.frame_to_flicks(frame).ok()?;
        Timecode::from_flicks(t, l.rate)
            .ok()
            .map(|tc| tc.to_string())
    }

    /// Seeks to abscissa `x` (window coordinates) of the strip.
    fn seek_at(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let Some(bounds) = self.wave.borrow().bounds else {
            return;
        };
        let State::Ready(l) = &self.state else { return };
        let width = f32::from(bounds.size.width).max(1.0);
        let frac = ((f32::from(x) - f32::from(bounds.origin.x)) / width).clamp(0.0, 1.0);
        l.clock
            .apply(Command::Seek(f64::from(frac) * l.clock.duration()));
        cx.notify();
    }

    /// Updates the displayed frame; returns `true` if more renders are needed.
    fn update_image(&mut self, window: &mut Window) -> bool {
        for image in self.stale.drain(..) {
            let _ = window.drop_image(image);
        }
        let State::Ready(l) = &mut self.state else {
            return false;
        };
        let Some(decoder) = &l.decoder else {
            return l.clock.is_playing();
        };
        let target = decoder.wanted();
        let shown = l.image.as_ref().map(|(i, _)| *i);
        if let Some(frame) = decoder.latest()
            && shown != Some(frame.index)
            && let Some(image) = render_image(&frame)
            && let Some((_, old)) = l.image.replace((frame.index, image))
        {
            let _ = window.drop_image(old);
        }
        l.clock.is_playing() || l.image.as_ref().map(|(i, _)| *i) != Some(target)
    }

    /// Bottom strip: waveform, played part, playhead. Clicking or dragging
    /// moves the playhead.
    pub fn strip(&self, cx: &mut Context<Self>) -> AnyElement {
        let base = div()
            .relative()
            .h(px(STRIP_HEIGHT))
            .w_full()
            .rounded(px(2.))
            .bg(color(tokens::BG));
        let State::Ready(l) = &self.state else {
            return base.into_any_element();
        };
        let peaks = l.peaks.clone();
        let duration = l.clock.duration();
        let position = l.clock.position();
        let edl = Arc::clone(&l.edl);
        let cuts: Vec<f64> = edl.parts().iter().skip(1).map(|p| p.start).collect();
        let wave = Rc::clone(&self.wave);
        base.child(
            canvas(
                move |bounds, _, _| {
                    let mut cache = wave.borrow_mut();
                    cache.bounds = Some(bounds);
                    let width = f32::from(bounds.size.width).max(0.0);
                    // Positive, reasonable width: no useful loss in the conversion.
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let columns = width.round() as usize;
                    let key = (
                        columns as u64,
                        duration.to_bits(),
                        Arc::as_ptr(&edl) as usize,
                    );
                    if cache.key != Some(key) {
                        cache.columns = Rc::new(
                            peaks
                                .as_ref()
                                .map(|p| montage_columns(p, &edl, columns))
                                .unwrap_or_default(),
                        );
                        cache.key = Some(key);
                    }
                    Rc::clone(&cache.columns)
                },
                move |bounds, columns, window, _| {
                    paint_strip(bounds, &columns, &cuts, position, duration, window);
                },
            )
            .size_full(),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|player, e: &MouseDownEvent, _, cx| player.seek_at(e.position.x, cx)),
        )
        .on_mouse_move(cx.listener(|player, e: &MouseMoveEvent, _, cx| {
            if e.pressed_button == Some(MouseButton::Left) {
                player.seek_at(e.position.x, cx);
            }
        }))
        .into_any_element()
    }

    fn stage(&self) -> AnyElement {
        let centered = |text: SharedString, tone: crate::theme::Token| {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .px(px(24.))
                .text_color(color(tone))
                .child(text)
                .into_any_element()
        };
        match &self.state {
            State::Empty => centered(
                tr!("player.empty", key = crate::shortcut("O")),
                tokens::MUTE,
            ),
            State::Loading { name, progress } => {
                let (stage, p) = progress.lock().map(|g| *g).unwrap_or((Stage::Probe, 0.0));
                centered(
                    tr!(
                        "player.loading",
                        name = name,
                        stage = stage_label(stage),
                        pct = format!("{:.0}", p * 100.0)
                    ),
                    tokens::FG2,
                )
            }
            State::Failed(e) => centered(e.clone(), tokens::ERROR),
            State::Ready(l) => match &l.image {
                // Absolute position: the image size does not weigh on the
                // layout, it fits the frame.
                Some((_, image)) => img(Arc::clone(image))
                    .absolute()
                    .inset_0()
                    .size_full()
                    .object_fit(ObjectFit::Contain)
                    .into_any_element(),
                None if l.decoder.is_some() => centered("…".into(), tokens::MUTE),
                None => centered(tr!("player.audio_only", name = l.name), tokens::FG2),
            },
        }
    }

    fn transport(&self) -> impl IntoElement {
        let (name, speed, duration, mute, proxy) = match &self.state {
            State::Ready(l) => (
                l.name.clone(),
                speed_label(l.clock.speed()),
                short(l.clock.duration()),
                l.prepared.audio.is_some() && !l.playback.has_sound(),
                l.proxy_progress
                    .as_ref()
                    .map(|p| p.lock().map_or(0.0, |g| *g)),
            ),
            _ => (
                SharedString::default(),
                String::new(),
                String::new(),
                false,
                None,
            ),
        };
        div()
            .flex()
            .items_center()
            .gap(px(14.))
            .h(px(32.))
            .px(px(12.))
            .bg(color(tokens::PANEL))
            .border_t_1()
            .border_color(color(tokens::LINE))
            .child(
                div()
                    .font_family(fonts::MONO)
                    .text_size(px(15.))
                    .child(SharedString::from(
                        self.timecode().unwrap_or_else(|| "--:--:--:--".to_owned()),
                    )),
            )
            .child(
                div()
                    .w(px(56.))
                    .font_family(fonts::MONO)
                    .text_size(px(12.))
                    .text_color(color(tokens::SIGNAL))
                    .child(SharedString::from(speed)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .font_weight(FontWeight::MEDIUM)
                    .child(name),
            )
            .when_some(proxy, |d, p| {
                d.child(
                    div()
                        .font_family(fonts::MONO)
                        .text_size(px(12.))
                        .text_color(color(tokens::MUTE))
                        .child(tr!("player.proxy", pct = format!("{:.0}", p * 100.0))),
                )
            })
            .when(mute, |d| {
                d.child(
                    div()
                        .text_size(px(12.))
                        .text_color(color(tokens::ERROR))
                        .child(tr!("player.no_output")),
                )
            })
            .child(
                div()
                    .font_family(fonts::MONO)
                    .text_size(px(12.))
                    .text_color(color(tokens::MUTE))
                    .child(SharedString::from(duration)),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(color(tokens::MUTE))
                    .child(tr!("player.keys")),
            )
    }
}

impl Player {
    /// Small buttons in the corner of the picture, shown on hover: native
    /// quality and fullscreen.
    fn corner(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let button = |id: &'static str, text: SharedString, on: bool| {
            div()
                .id(id)
                .px(px(6.))
                .h(px(20.))
                .flex()
                .items_center()
                .rounded(px(3.))
                .cursor_pointer()
                .text_size(px(11.))
                .bg(gpui_kit::black().opacity(0.55))
                .text_color(color(if on { tokens::SIGNAL } else { tokens::FG2 }))
                .hover(|d| d.text_color(color(tokens::FG)))
                .child(text)
        };
        let has_video = matches!(&self.state, State::Ready(l) if l.prepared.video.is_some());
        div()
            .absolute()
            .top(px(8.))
            .right(px(8.))
            .flex()
            .gap(px(4.))
            .invisible()
            .group_hover("stage", |s| s.visible())
            .when(has_video, |d| {
                d.child(
                    button("player-native", tr!("player.native"), self.native)
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(tr!("player.native_help"))
                                .build(window, cx)
                        })
                        .on_click(cx.listener(|player, _: &gpui_kit::ClickEvent, _, cx| {
                            player.set_native(!player.native, cx);
                        })),
                )
            })
            .child(
                button(
                    "player-expand",
                    if self.expanded {
                        tr!("player.collapse")
                    } else {
                        tr!("player.expand")
                    },
                    false,
                )
                .on_click(cx.listener(
                    |player, _: &gpui_kit::ClickEvent, window, cx| {
                        player.set_expanded(!player.expanded, window, cx);
                    },
                )),
            )
    }
}

impl Render for Player {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.update_image(window) {
            window.request_animation_frame();
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .group("stage")
                    .relative()
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_hidden()
                    .bg(gpui_kit::black())
                    .child(self.stage())
                    .child(self.corner(cx)),
            )
            .child(self.transport())
    }
}

fn frame_duration(rate: FrameRate) -> f64 {
    f64::from(rate.den()) / f64::from(rate.num())
}

/// Where the frames come from: the original at its own size in native
/// quality, else the proxy (or the original at proxy size until it exists).
fn video_source(prepared: &Prepared, native: bool) -> Option<VideoSource> {
    let video = prepared.video.clone()?;
    match prepared.info.video {
        Some(info) if native => Some(VideoSource {
            path: prepared.source.clone(),
            // Even sizes, as for the proxy.
            width: (info.width / 2).max(1) * 2,
            height: (info.height / 2).max(1) * 2,
            is_proxy: false,
        }),
        _ => Some(video),
    }
}

fn decoder_for(
    video: &VideoSource,
    prepared: &Prepared,
    rate: FrameRate,
    edl: &Arc<Edl>,
    clock: &Arc<Clock>,
    fx: &PostFx,
) -> VideoDecoder {
    VideoDecoder::spawn(
        video.path.clone(),
        rate,
        prepared.frame_count(),
        Arc::clone(edl),
        (video.width, video.height),
        enfait_media::fx::filter(fx, video.height),
        Arc::clone(clock),
        Box::new(|| {}),
    )
}

/// gpui image (BGRA) from a decoded frame.
pub(crate) fn render_image(frame: &enfait_media::Frame) -> Option<Arc<RenderImage>> {
    let buffer = image::RgbaImage::from_raw(frame.width, frame.height, frame.data.clone())?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

/// Waveform of the edit: each column reads the source at the matching
/// place.
fn montage_columns(peaks: &Peaks, edl: &Edl, columns: usize) -> Vec<(f32, f32)> {
    let duration = edl.duration();
    if edl.is_identity() {
        return peaks.columns(0.0, duration, columns);
    }
    #[allow(clippy::cast_precision_loss)]
    let step = duration / columns.max(1) as f64;
    (0..columns)
        .map(|c| {
            #[allow(clippy::cast_precision_loss)]
            let t = step * c as f64;
            let source = edl.to_source(t);
            peaks
                .columns(source, source + step, 1)
                .first()
                .copied()
                .unwrap_or((0.0, 0.0))
        })
        .collect()
}

fn paint_strip(
    bounds: Bounds<Pixels>,
    columns: &[(f32, f32)],
    cuts: &[f64],
    position: f64,
    duration: f64,
    window: &mut Window,
) {
    let height = f32::from(bounds.size.height);
    let mid = f32::from(bounds.origin.y) + height / 2.0;
    let left = f32::from(bounds.origin.x);
    #[allow(clippy::cast_possible_truncation)]
    let played = if duration > 0.0 {
        (position / duration).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    let head = left + f32::from(bounds.size.width) * played;
    for (i, (lo, hi)) in columns.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let x = left + i as f32;
        // A silent column keeps a one-pixel line.
        let top = mid - hi.max(0.0) * height * 0.45;
        let bottom = (mid - lo.min(0.0) * height * 0.45).max(top + 1.0);
        let tone = if x < head {
            tokens::FG2
        } else {
            tokens::LINE_STRONG
        };
        window.paint_quad(fill(
            Bounds::new(point(px(x), px(top)), size(px(1.0), px(bottom - top))),
            color(tone),
        ));
    }
    // Cuts of the edit.
    for cut in cuts {
        #[allow(clippy::cast_possible_truncation)]
        let x = left + f32::from(bounds.size.width) * (cut / duration.max(1e-9)) as f32;
        window.paint_quad(fill(
            Bounds::new(
                point(px(x), bounds.origin.y),
                size(px(1.0), bounds.size.height),
            ),
            color(tokens::MIXED),
        ));
    }
    window.paint_quad(fill(
        Bounds::new(
            point(px(head - 1.0), bounds.origin.y),
            size(px(2.0), bounds.size.height),
        ),
        color(tokens::SIGNAL),
    ));
}

/// Name of a media preparation stage.
fn stage_label(stage: Stage) -> SharedString {
    match stage {
        Stage::Probe => tr!("player.stage.probe"),
        Stage::Proxy => tr!("player.stage.proxy"),
        Stage::Audio => tr!("player.stage.audio"),
        Stage::Waveform => tr!("player.stage.waveform"),
    }
}

fn speed_label(speed: f64) -> String {
    if speed == 0.0 {
        tr!("player.pause").to_string()
    } else if speed > 0.0 {
        format!("▶ ×{speed}")
    } else {
        format!("◀ ×{}", -speed)
    }
}

/// `MM:SS` (or `H:MM:SS`).
fn short(secs: f64) -> String {
    // Duration of a media: positive and bounded.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let total = secs.max(0.0).round() as u64;
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(speed_label(0.0), "pause");
        assert_eq!(speed_label(2.0), "▶ ×2");
        assert_eq!(speed_label(-1.0), "◀ ×1");
        assert_eq!(short(65.4), "01:05");
        assert_eq!(short(3723.0), "1:02:03");
    }
}
