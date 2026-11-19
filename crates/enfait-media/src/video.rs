//! Decoding proxy frames, driven by the clock.
//!
//! A worker thread keeps an ffmpeg process open that outputs frames in
//! order (BGRA, the order gpui expects). While the clock moves normally,
//! it reads on; if it is too late or too early, it restarts ffmpeg at the
//! right place (the proxy has a keyframe every half second, which makes
//! these restarts fast).
//!
//! ffmpeg only decodes forward. For backward playback (and backward
//! steps), the decoder reads at once a block of frames ending at the
//! wanted frame and keeps it in memory; it reads the previous block ahead
//! of time, while still showing the frames already in memory.

use crate::{Clock, Edl, MediaError};
use enfait_core::FrameRate;
use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// A decoded frame.
#[derive(Debug)]
pub struct Frame {
    /// Index of the frame in the media.
    pub index: i64,
    pub width: u32,
    pub height: u32,
    /// BGRA pixels, row by row.
    pub data: Vec<u8>,
}

/// Index of the frame displayed at time `t` (seconds).
pub fn frame_at(t: f64, rate: FrameRate) -> i64 {
    let fps = f64::from(rate.num()) / f64::from(rate.den());
    // Small margin: a time snapped to the start of a frame must give it.
    let x = (t.max(0.0) * fps + 1e-6).floor();
    // Bounded by the media duration: the conversion does not overflow.
    #[allow(clippy::cast_possible_truncation)]
    let i = x as i64;
    i
}

/// Start of frame `index`, in seconds.
pub fn frame_time(index: i64, rate: FrameRate) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let i = index as f64;
    i * f64::from(rate.den()) / f64::from(rate.num())
}

/// Sequential frame reader from an index.
struct Reader {
    child: Child,
    stdout: ChildStdout,
    /// Index of the next frame read.
    next: i64,
}

impl Reader {
    fn open(
        path: &Path,
        index: i64,
        rate: FrameRate,
        width: u32,
        height: u32,
        extra: Option<&str>,
    ) -> Result<Self, MediaError> {
        // `-ss` before `-i`: fast seek to the previous keyframe, then decoding
        // up to the exact time. The slight step back avoids landing on the next
        // frame because of rounding.
        let start = (frame_time(index, rate) - 0.001).max(0.0);
        let mut child = enfait_config::process::command("ffmpeg")
            .args(["-nostdin", "-hide_banner", "-v", "error"])
            .args(["-ss", &format!("{start:.6}")])
            .arg("-i")
            .arg(path)
            .args(["-map", "0:v:0", "-an", "-sn"])
            // `fps`: an original source may have a variable frame rate; we want
            // exactly one frame per index.
            .args([
                "-vf",
                &format!(
                    "fps={}/{},scale={width}:{height}{}",
                    rate.num(),
                    rate.den(),
                    extra.map(|f| format!(",{f}")).unwrap_or_default()
                ),
            ])
            .args(["-f", "rawvideo", "-pix_fmt", "bgra", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| MediaError::spawn("ffmpeg", &e))?;
        let stdout = child.stdout.take().ok_or_else(|| MediaError::Tool {
            tool: "ffmpeg",
            message: "sortie indisponible".into(),
        })?;
        Ok(Self {
            child,
            stdout,
            next: index,
        })
    }

    /// Reads the next frame into `buf`; `false` at the end of the stream.
    fn read(&mut self, buf: &mut [u8]) -> bool {
        if self.stdout.read_exact(buf).is_ok() {
            self.next = self.next.saturating_add(1);
            true
        } else {
            false
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Mapping between the frames of the edit and those of the source.
#[derive(Debug, Clone)]
struct FrameMap {
    edl: Arc<Edl>,
    rate: FrameRate,
    /// Last frame of the source.
    last_source: i64,
    /// Last frame of the edit.
    last_output: i64,
}

impl FrameMap {
    fn new(edl: Arc<Edl>, rate: FrameRate, frame_count: i64) -> Self {
        let last_output = frame_at((edl.duration() - 1e-4).max(0.0), rate);
        Self {
            edl,
            rate,
            last_source: frame_count.saturating_sub(1).max(0),
            last_output,
        }
    }

    /// Frame of the edit at time `t`.
    fn output_at(&self, t: f64) -> i64 {
        frame_at(t, self.rate).clamp(0, self.last_output)
    }

    /// Source frame shown at edit frame `o`, and first source frame of its
    /// piece.
    fn source(&self, o: i64) -> (i64, i64) {
        let half = 0.5 * f64::from(self.rate.den()) / f64::from(self.rate.num());
        // The middle of the frame avoids rounding errors at boundaries.
        let t = frame_time(o, self.rate) + half;
        let source = frame_at(self.edl.to_source(t), self.rate).clamp(0, self.last_source);
        let part_start = self
            .edl
            .locate(t)
            .map_or(0, |(_, p)| frame_at(p.source_start + half, self.rate));
        (source, part_start.clamp(0, source))
    }
}

/// Background decoder. Stops when dropped.
pub struct VideoDecoder {
    latest: Arc<Mutex<Option<Arc<Frame>>>>,
    /// Shared frame memory: [`VideoDecoder::latest`] looks up the clock's
    /// frame there directly, without waiting for the decoding thread (which
    /// can be blocked a few tens of milliseconds while ffmpeg starts).
    cache: Arc<Mutex<FrameCache>>,
    clock: Arc<Clock>,
    map: FrameMap,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl VideoDecoder {
    /// `frame_count`: frames of the source; `edl`: edit being played (the
    /// clock is in edit time); `size`: size of the produced frames;
    /// `filter`: added ffmpeg filters (post-FX in preview). `on_frame` is
    /// called from the worker thread for each published frame.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        source: PathBuf,
        rate: FrameRate,
        frame_count: i64,
        edl: Arc<Edl>,
        size: (u32, u32),
        filter: Option<String>,
        clock: Arc<Clock>,
        on_frame: Box<dyn Fn() + Send>,
    ) -> Self {
        let latest = Arc::new(Mutex::new(None));
        let cache = Arc::new(Mutex::new(FrameCache::new(CACHE_FRAMES)));
        let stop = Arc::new(AtomicBool::new(false));
        let map = FrameMap::new(edl, rate, frame_count);
        let thread = {
            let worker = Worker {
                source,
                rate,
                map: map.clone(),
                size,
                filter,
                clock: Arc::clone(&clock),
                latest: Arc::clone(&latest),
                cache: Arc::clone(&cache),
                stop: Arc::clone(&stop),
                on_frame,
            };
            std::thread::Builder::new()
                .name("enfait-video".into())
                .spawn(move || worker.run())
                .map_err(|e| tracing::error!("video decoder: {e}"))
                .ok()
        };
        Self {
            latest,
            cache,
            clock,
            map,
            stop,
            thread,
        }
    }

    /// Frame to show now: the clock's if it is in memory, else the last
    /// published one.
    pub fn latest(&self) -> Option<Arc<Frame>> {
        let (target, _) = self.map.source(self.map.output_at(self.clock.position()));
        let cached = self.cache.lock().ok().and_then(|c| c.get(target));
        let mut slot = self.latest.lock().ok()?;
        if let Some(frame) = cached {
            *slot = Some(frame);
        }
        slot.clone()
    }

    /// Source frame expected at the current clock position.
    pub fn wanted(&self) -> i64 {
        self.map.source(self.map.output_at(self.clock.position())).0
    }
}

impl Drop for VideoDecoder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Frames kept in memory around the playhead (about 100 MB at 960×540).
const CACHE_FRAMES: usize = 48;
/// Frames decoded at once for backward playback.
const BLOCK: i64 = 20;
/// Frames read ahead during forward playback (enough to prepare what
/// follows a cut without a hitch).
const AHEAD: i64 = 8;
/// Frames we accept to read and drop rather than restart ffmpeg.
const MAX_SKIP: i64 = 12;

/// Decoded source frames, by index.
#[derive(Debug)]
struct FrameCache {
    frames: BTreeMap<i64, Arc<Frame>>,
    capacity: usize,
}

impl FrameCache {
    fn new(capacity: usize) -> Self {
        Self {
            frames: BTreeMap::new(),
            capacity: capacity.max(1),
        }
    }

    fn get(&self, index: i64) -> Option<Arc<Frame>> {
        self.frames.get(&index).cloned()
    }

    fn contains(&self, index: i64) -> bool {
        self.frames.contains_key(&index)
    }

    /// Stores a frame, then forgets those farthest from `around`.
    fn insert(&mut self, frame: Arc<Frame>, around: i64) {
        self.frames.insert(frame.index, frame);
        while self.frames.len() > self.capacity {
            let first = self.frames.first_key_value().map(|(k, _)| *k);
            let last = self.frames.last_key_value().map(|(k, _)| *k);
            let (Some(first), Some(last)) = (first, last) else {
                break;
            };
            let victim = if around.abs_diff(first) >= around.abs_diff(last) {
                first
            } else {
                last
            };
            self.frames.remove(&victim);
        }
    }
}

/// What the decoder sees before choosing what to do.
#[derive(Debug, Clone, Copy)]
struct Situation {
    /// Edit frame wanted by the clock.
    target: i64,
    /// Last frame of the edit.
    last: i64,
    /// Published source frame.
    shown: Option<i64>,
    /// Next source frame of the open ffmpeg stream.
    next: Option<i64>,
    speed: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plan {
    /// Nothing to do.
    Hold,
    /// Publish the wanted frame, already in memory.
    Show,
    /// Decode the source frames `start..=end` (continuing the open stream if
    /// it is already at `start`, else restarting ffmpeg).
    Decode { start: i64, end: i64 },
}

/// `source`: source frame and start of its piece, for an edit frame;
/// `cached`: is the source frame in memory?
fn plan(s: Situation, source: &dyn Fn(i64) -> (i64, i64), cached: &dyn Fn(i64) -> bool) -> Plan {
    let (wanted, _) = source(s.target);
    let need = if !cached(wanted) {
        s.target
    } else if s.shown != Some(wanted) {
        return Plan::Show;
    } else {
        // Prepare what follows in the playing direction; paused, backward (so
        // that J and ← start without waiting).
        let (step, reach) = if s.speed > 0.0 {
            (1, AHEAD)
        } else {
            (-1, BLOCK / 2)
        };
        let ahead = (1..=reach)
            .map(|d| s.target.saturating_add(step * d))
            .take_while(|o| (0..=s.last).contains(o))
            .find(|o| !cached(source(*o).0));
        match ahead {
            Some(o) => o,
            None => return Plan::Hold,
        }
    };
    let (frame, part_start) = source(need);
    let backward = s.speed < 0.0 || need < s.target;
    match s.next {
        Some(next) if !backward && frame >= next && frame.saturating_sub(next) < MAX_SKIP => {
            Plan::Decode {
                start: next,
                end: frame,
            }
        }
        // Backward (playing or stepping): a whole block ending at the wanted
        // frame, without going before the start of its piece.
        next if backward || (s.speed == 0.0 && next.is_some_and(|n| frame < n)) => Plan::Decode {
            start: frame.saturating_sub(BLOCK - 1).max(part_start).max(0),
            end: frame,
        },
        _ => Plan::Decode {
            start: frame,
            end: frame,
        },
    }
}

struct Worker {
    source: PathBuf,
    rate: FrameRate,
    map: FrameMap,
    size: (u32, u32),
    filter: Option<String>,
    clock: Arc<Clock>,
    latest: Arc<Mutex<Option<Arc<Frame>>>>,
    cache: Arc<Mutex<FrameCache>>,
    stop: Arc<AtomicBool>,
    on_frame: Box<dyn Fn() + Send>,
}

impl Worker {
    /// Edit frame wanted by the clock.
    fn target(&self) -> i64 {
        self.map.output_at(self.clock.position())
    }

    fn publish(&self, frame: Arc<Frame>) {
        if let Ok(mut slot) = self.latest.lock() {
            *slot = Some(frame);
        }
        (self.on_frame)();
    }

    fn run(&self) {
        let mut reader: Option<Reader> = None;
        let mut shown: Option<i64> = None;
        // Start of a failed decoding: do not restart ffmpeg in a loop.
        let mut failed: Option<i64> = None;
        while !self.stop.load(Ordering::Acquire) {
            let situation = Situation {
                target: self.target(),
                last: self.map.last_output,
                shown,
                next: reader.as_ref().map(|r| r.next),
                speed: self.clock.speed(),
            };
            let decision = {
                let Ok(cache) = self.cache.lock() else { return };
                plan(situation, &|o| self.map.source(o), &|i| cache.contains(i))
            };
            match decision {
                Plan::Hold => std::thread::sleep(Duration::from_millis(3)),
                Plan::Show => {
                    let (wanted, _) = self.map.source(situation.target);
                    let frame = self.cache.lock().ok().and_then(|c| c.get(wanted));
                    if let Some(frame) = frame {
                        self.publish(frame);
                        shown = Some(wanted);
                    }
                }
                Plan::Decode { start, end } => {
                    if failed == Some(start) {
                        std::thread::sleep(Duration::from_millis(20));
                        continue;
                    }
                    if reader.as_ref().map(|r| r.next) != Some(start) {
                        reader = None;
                        match Reader::open(
                            &self.source,
                            start,
                            self.rate,
                            self.size.0,
                            self.size.1,
                            self.filter.as_deref(),
                        ) {
                            Ok(r) => reader = Some(r),
                            Err(e) => {
                                tracing::warn!("decoding frame {start}: {e}");
                                failed = Some(start);
                                continue;
                            }
                        }
                    }
                    let Some(r) = reader.as_mut() else { continue };
                    if self.decode(r, start, end, &mut shown) {
                        failed = None;
                    } else {
                        reader = None;
                        failed = Some(start);
                    }
                }
            }
        }
    }

    /// Reads source frames up to `end`, keeping them in memory, and publishes
    /// the wanted frame as soon as it is available. Stops if the clock asks
    /// for a frame missing outside this block. `false`: end of the stream or
    /// error.
    fn decode(&self, reader: &mut Reader, start: i64, end: i64, shown: &mut Option<i64>) -> bool {
        let (width, height) = self.size;
        let len = usize::try_from(u64::from(width) * u64::from(height) * 4).unwrap_or(0);
        while reader.next <= end && !self.stop.load(Ordering::Acquire) {
            let mut data = vec![0_u8; len];
            let index = reader.next;
            if !reader.read(&mut data) {
                return false;
            }
            let (wanted, _) = self.map.source(self.target());
            let frame = Arc::new(Frame {
                index,
                width,
                height,
                data,
            });
            let Ok((ready, present)) = self.cache.lock().map(|mut c| {
                c.insert(frame, wanted);
                (c.get(wanted), c.contains(wanted))
            }) else {
                return false;
            };
            if *shown != Some(wanted)
                && let Some(frame) = ready
            {
                self.publish(frame);
                *shown = Some(wanted);
            }
            if !present && !(start..=end).contains(&wanted) {
                break;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_index_math() {
        let r = FrameRate::FPS_30;
        assert_eq!(frame_at(0.0, r), 0);
        assert_eq!(frame_at(1.0, r), 30);
        assert_eq!(frame_at(frame_time(7, r), r), 7);
        assert_eq!(frame_at(-1.0, r), 0);
        let ntsc = FrameRate::NTSC_30;
        for i in [0, 1, 29, 30, 1799, 107_892] {
            assert_eq!(frame_at(frame_time(i, ntsc), ntsc), i);
        }
    }

    fn frame(index: i64) -> Arc<Frame> {
        Arc::new(Frame {
            index,
            width: 1,
            height: 1,
            data: vec![],
        })
    }

    #[test]
    fn cache_keeps_frames_near_the_playhead() {
        let mut c = FrameCache::new(4);
        for i in 0..6 {
            c.insert(frame(i), 5);
        }
        assert_eq!(
            c.frames.keys().copied().collect::<Vec<_>>(),
            vec![2, 3, 4, 5]
        );
        c.insert(frame(9), 3);
        assert!(!c.contains(9), "the farthest one goes");
    }

    #[test]
    fn frame_map_follows_the_cut() {
        // 10 fps; edit: [2 s; 3 s) then [0.5 s; 1 s).
        let rate = FrameRate::new(10, 1).unwrap();
        let edl = Arc::new(Edl::from_ranges(&[(2.0, 3.0), (0.5, 1.0)]));
        let map = FrameMap::new(edl, rate, 100);
        assert_eq!(map.last_output, 14);
        assert_eq!(map.source(0), (20, 20));
        assert_eq!(map.source(9), (29, 20));
        assert_eq!(map.source(10), (5, 5), "after the cut");
        assert_eq!(map.source(14), (9, 5));
        assert_eq!(map.output_at(99.0), 14);
        let id = FrameMap::new(Arc::new(Edl::identity(10.0)), rate, 100);
        for o in [0, 1, 37, 99] {
            assert_eq!(id.source(o).0, o);
        }
    }

    /// Identity edit of 100 frames, and a set of frames in memory.
    fn decide(s: Situation, cached: &[i64]) -> Plan {
        plan(s, &|o| (o, 0), &|i| cached.contains(&i))
    }

    fn at(target: i64) -> Situation {
        Situation {
            target,
            last: 99,
            shown: None,
            next: None,
            speed: 0.0,
        }
    }

    #[test]
    fn forward_reads_on_ahead_or_reopens() {
        let playing = |target, next| Situation {
            next: Some(next),
            speed: 1.0,
            ..at(target)
        };
        assert_eq!(
            decide(playing(6, 6), &[]),
            Plan::Decode { start: 6, end: 6 }
        );
        assert_eq!(
            decide(playing(9, 6), &[]),
            Plan::Decode { start: 6, end: 9 }
        );
        assert_eq!(
            decide(playing(6 + MAX_SKIP, 6), &[]),
            Plan::Decode {
                start: 6 + MAX_SKIP,
                end: 6 + MAX_SKIP
            },
            "trop loin : relance"
        );
        // Displayed frame: read ahead up to AHEAD frames.
        let shown = Situation {
            shown: Some(6),
            ..playing(6, 7)
        };
        assert_eq!(decide(shown, &[6]), Plan::Decode { start: 7, end: 7 });
        let full: Vec<i64> = (6..=6 + AHEAD).collect();
        assert_eq!(decide(shown, &full), Plan::Hold);
        assert_eq!(decide(at(3), &[]), Plan::Decode { start: 3, end: 3 });
    }

    #[test]
    fn backward_decodes_blocks_and_prefetches() {
        // Step back, paused: a whole block ending at the wanted frame.
        let step_back = Situation {
            next: Some(51),
            ..at(49)
        };
        assert_eq!(decide(step_back, &[]), Plan::Decode { start: 30, end: 49 });
        // Near the start of the media: the block is truncated.
        let early = Situation {
            speed: -1.0,
            ..at(5)
        };
        assert_eq!(decide(early, &[]), Plan::Decode { start: 0, end: 5 });
        // In memory but not published yet.
        let block: Vec<i64> = (30..=40).collect();
        let ready = Situation {
            speed: -1.0,
            ..at(40)
        };
        assert_eq!(decide(ready, &block), Plan::Show);
        // Published, with margin: nothing to do.
        let shown = Situation {
            shown: Some(40),
            ..ready
        };
        assert_eq!(decide(shown, &block), Plan::Hold);
        // Little margin: read the previous block ahead (paused too).
        let low = Situation {
            shown: Some(35),
            ..at(35)
        };
        assert_eq!(
            decide(Situation { speed: -1.0, ..low }, &block),
            Plan::Decode { start: 10, end: 29 }
        );
        assert_eq!(decide(low, &block), Plan::Decode { start: 10, end: 29 });
    }

    #[test]
    fn prepares_the_frames_after_a_cut() {
        // Edit: source frames 20 to 29, then 5 to 14.
        let source = |o: i64| if o < 10 { (20 + o, 20) } else { (o - 5, 5) };
        let s = Situation {
            target: 8,
            last: 19,
            shown: Some(28),
            next: Some(30),
            speed: 1.0,
        };
        let cached: Vec<i64> = (20..=29).collect();
        let decided = plan(s, &source, &|i| cached.contains(&i));
        assert_eq!(
            decided,
            Plan::Decode { start: 5, end: 5 },
            "restart after the cut"
        );
        // Backward, the block does not go before the start of the piece.
        let back = Situation {
            target: 12,
            shown: None,
            next: None,
            speed: -1.0,
            ..s
        };
        assert_eq!(
            plan(back, &source, &|_| false),
            Plan::Decode { start: 5, end: 7 }
        );
    }
}
