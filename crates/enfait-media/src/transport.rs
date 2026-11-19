//! Playback clock and transport commands (J/K/L).
//!
//! The clock is shared between the interface (which reads the position
//! and sends commands), the audio output (which drives it, and is thus the
//! reference: the picture follows the sound) and the frame decoder (which
//! reads it). It takes no lock: the audio output must never wait.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Shuttle speeds, as multiples of normal speed.
const SHUTTLE: [f64; 4] = [1.0, 2.0, 4.0, 8.0];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    /// L: forward; repeated, speeds up (×2, ×4, ×8). From backward, restarts at ×1.
    Forward,
    /// J: backward, same logic.
    Backward,
    /// K: pause.
    Pause,
    /// Space: play or pause.
    Toggle,
    /// One frame forward (positive) or backward, paused.
    Step(i64),
    /// Go to a time, in seconds, without changing the speed.
    Seek(f64),
}

/// Speed after a shuttle command.
pub fn shuttle(speed: f64, command: Command) -> f64 {
    let faster = |sign: f64| {
        let current = speed * sign;
        if current <= 0.0 {
            sign
        } else {
            SHUTTLE
                .iter()
                .find(|s| **s > current + 1e-9)
                .map_or(SHUTTLE.last().copied().unwrap_or(1.0), |s| *s)
                * sign
        }
    };
    match command {
        Command::Forward => faster(1.0),
        Command::Backward => faster(-1.0),
        Command::Pause | Command::Step(_) => 0.0,
        Command::Toggle => {
            if speed == 0.0 {
                1.0
            } else {
                0.0
            }
        }
        Command::Seek(_) => speed,
    }
}

/// Progress of a clock over an interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tick {
    /// Position at the start of the interval, in seconds.
    pub start: f64,
    /// Speed during the interval (0 when paused).
    pub speed: f64,
}

/// `f64` stored in an `AtomicU64`.
#[derive(Debug)]
struct AtomicF64(AtomicU64);

impl AtomicF64 {
    fn new(v: f64) -> Self {
        Self(AtomicU64::new(v.to_bits()))
    }
    fn load(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Acquire))
    }
    fn store(&self, v: f64) {
        self.0.store(v.to_bits(), Ordering::Release);
    }
    fn swap(&self, v: f64) -> f64 {
        f64::from_bits(self.0.swap(v.to_bits(), Ordering::AcqRel))
    }
}

#[derive(Debug)]
pub struct Clock {
    duration: f64,
    /// Duration of a frame (for frame steps).
    frame: f64,
    /// Origin of the times stored below.
    epoch: Instant,
    /// Position of the output cursor: the end of the last interval sent to
    /// the sound card. It is ahead of what is heard.
    cursor: AtomicF64,
    speed: AtomicF64,
    /// Seek asked by the interface, applied at the next [`Clock::tick`] (NaN:
    /// none).
    pending: AtomicF64,
    /// What is heard, as the position it would have at `epoch` at the
    /// current speed: at time `now`, `origin + speed × now`. A single atomic,
    /// so a reader never sees half of an update.
    origin: AtomicF64,
}

/// While playing, the anchor of what is heard never moves back against the
/// playing direction (timer or audio callback jitter would make the picture
/// shake), unless it is this far behind (a real stall of the output).
const STALL: f64 = 0.25;

impl Clock {
    /// `frame_duration`: duration of a frame in seconds (1/25 without video).
    pub fn new(duration: f64, frame_duration: f64) -> Self {
        Self {
            duration: duration.max(0.0),
            frame: if frame_duration > 0.0 {
                frame_duration
            } else {
                0.04
            },
            epoch: Instant::now(),
            cursor: AtomicF64::new(0.0),
            speed: AtomicF64::new(0.0),
            pending: AtomicF64::new(f64::NAN),
            origin: AtomicF64::new(0.0),
        }
    }

    pub const fn duration(&self) -> f64 {
        self.duration
    }

    fn now(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    /// Position of what is heard (or would be seen) now, in seconds: the
    /// picture and the interface follow it.
    pub fn position(&self) -> f64 {
        self.position_at(self.now())
    }

    fn position_at(&self, now: f64) -> f64 {
        let speed = self.speed();
        if speed == 0.0 {
            return self.cursor.load();
        }
        self.clamp(self.origin.load() + speed * now)
    }

    pub fn speed(&self) -> f64 {
        self.speed.load()
    }

    pub fn is_playing(&self) -> bool {
        self.speed() != 0.0
    }

    fn clamp(&self, t: f64) -> f64 {
        if t.is_nan() {
            0.0
        } else {
            t.clamp(0.0, self.duration)
        }
    }

    /// Goes to `t`, about to play at `speed`.
    fn jump(&self, t: f64, speed: f64) {
        let t = self.clamp(t);
        self.cursor.store(t);
        self.pending.store(t);
        self.origin.store(t - speed * self.now());
    }

    pub fn apply(&self, command: Command) {
        let speed = self.speed();
        let position = self.position();
        let next = shuttle(speed, command);
        match command {
            Command::Seek(t) => self.jump(t, next),
            Command::Step(n) => {
                // Snap to the start of a frame, then move by `n`.
                #[allow(clippy::cast_precision_loss)]
                let frames = (position / self.frame + 1e-6).floor() + n as f64;
                self.jump(frames * self.frame, next);
            }
            Command::Forward | Command::Toggle if next > 0.0 && position >= self.duration => {
                // Start again from the beginning at the end.
                self.jump(0.0, next);
            }
            // At the start, J does nothing (as in editing software).
            Command::Backward if position <= 0.0 => {
                self.speed.store(0.0);
                return;
            }
            // Speed change: restart from what is heard, not from the output
            // cursor, which is ahead.
            _ if next != speed => self.jump(position, next),
            _ => {}
        }
        self.speed.store(next);
    }

    /// Moves the clock forward by `dt` seconds and returns what to play during
    /// that interval. `delay`: time before the start of the interval is heard
    /// (output latency). Stops at the ends.
    pub fn tick(&self, dt: f64, delay: f64) -> Tick {
        self.tick_at(dt, delay, self.now())
    }

    fn tick_at(&self, dt: f64, delay: f64, now: f64) -> Tick {
        let pending = self.pending.swap(f64::NAN);
        let seeked = !pending.is_nan();
        let start = if !seeked { self.cursor.load() } else { pending };
        let speed = self.speed();
        if speed == 0.0 {
            return Tick { start, speed };
        }
        let end = start + speed * dt.max(0.0);
        let clamped = self.clamp(end);
        // A seek asked during the interval wins over the advance.
        if self.pending.load().is_nan() {
            self.cursor.store(clamped);
            // `start` will be heard in `delay` seconds.
            let origin = start - speed * delay.max(0.0) - speed * now;
            let ahead = (origin - self.origin.load()) * speed.signum();
            if seeked || ahead >= 0.0 || ahead < -STALL {
                self.origin.store(origin);
            }
        }
        if clamped != end {
            self.speed.store(0.0);
        }
        Tick { start, speed }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jkl_shuttle() {
        use Command::*;
        assert_eq!(shuttle(0.0, Forward), 1.0);
        assert_eq!(shuttle(1.0, Forward), 2.0);
        assert_eq!(shuttle(4.0, Forward), 8.0);
        assert_eq!(shuttle(8.0, Forward), 8.0);
        assert_eq!(shuttle(2.0, Backward), -1.0);
        assert_eq!(shuttle(-1.0, Backward), -2.0);
        assert_eq!(shuttle(-4.0, Forward), 1.0);
        assert_eq!(shuttle(4.0, Pause), 0.0);
        assert_eq!(shuttle(0.0, Toggle), 1.0);
        assert_eq!(shuttle(-2.0, Toggle), 0.0);
        assert_eq!(shuttle(2.0, Seek(1.0)), 2.0);
    }

    #[test]
    fn tick_advances_and_stops_at_end() {
        let c = Clock::new(1.0, 0.04);
        assert_eq!(c.tick(0.5, 0.0).start, 0.0);
        assert_eq!(c.position(), 0.0, "paused, nothing moves");
        c.apply(Command::Forward);
        c.tick(0.5, 0.0);
        assert!((c.cursor.load() - 0.5).abs() < 1e-9);
        c.apply(Command::Forward); // ×2, repart de ce qu'on entend
        let t = c.tick(0.5, 0.0);
        assert_eq!(t.speed, 2.0);
        c.tick(0.5, 0.0);
        assert_eq!(c.cursor.load(), 1.0);
        assert!(!c.is_playing(), "stop at the end");
        assert_eq!(c.position(), 1.0);
        // Playing again at the end restarts from the start.
        c.apply(Command::Toggle);
        assert_eq!(c.tick(0.1, 0.0).start, 0.0);
    }

    #[test]
    fn backward_and_seek() {
        let c = Clock::new(2.0, 0.04);
        c.apply(Command::Seek(1.0));
        assert_eq!(c.position(), 1.0, "visible tout de suite");
        c.apply(Command::Backward);
        let t = c.tick_at(0.25, 0.0, 10.0);
        assert!((t.start - 1.0).abs() < 0.01);
        assert!((c.cursor.load() - t.start + 0.25).abs() < 1e-9);
        assert!((c.position_at(10.1) - (t.start - 0.1)).abs() < 1e-9);
        c.apply(Command::Seek(9.0));
        // Still playing backward: the position restarts from 2 at once.
        assert!((c.position() - 2.0).abs() < 0.01, "bounded to the duration");
    }

    #[test]
    fn backward_at_start_does_nothing() {
        let c = Clock::new(2.0, 0.04);
        c.apply(Command::Backward);
        assert!(!c.is_playing());
        assert_eq!(c.position(), 0.0);
        // Reaching the start playing backward: stop.
        c.apply(Command::Seek(0.1));
        c.apply(Command::Backward);
        c.tick(0.5, 0.0);
        assert_eq!(c.position(), 0.0);
        assert!(!c.is_playing());
    }

    #[test]
    fn frame_steps_snap_to_frames() {
        let c = Clock::new(1.0, 0.1);
        c.apply(Command::Seek(0.25));
        c.apply(Command::Step(1));
        assert!((c.position() - 0.3).abs() < 1e-9);
        c.apply(Command::Step(-2));
        assert!((c.position() - 0.1).abs() < 1e-9);
        c.apply(Command::Step(-5));
        assert_eq!(c.position(), 0.0);
        assert!(!c.is_playing());
    }

    #[test]
    fn late_ticks_never_move_backward_playback_forward() {
        let c = Clock::new(10.0, 0.04);
        c.apply(Command::Seek(5.0));
        c.apply(Command::Backward);
        // Timer ticks of 4 ms, some arriving late: each anchors `start` (the
        // previous cursor) at `now`, behind the extrapolation.
        let mut now = 1.0;
        c.tick_at(0.004, 0.0, now); // applies the seek
        let mut last = c.position_at(now);
        for late in [0.0, 0.0, 0.02, 0.0, 0.03, 0.001, 0.0, 0.015] {
            let dt = 0.004 + late;
            now += dt;
            let before = c.position_at(now);
            assert!(before <= last + 1e-12, "{before} after {last}");
            c.tick_at(dt, 0.0, now);
            let after = c.position_at(now);
            assert!(after <= before + 1e-12, "tick moved {before} to {after}");
            last = after;
        }
    }

    #[test]
    fn position_follows_what_is_heard() {
        let c = Clock::new(10.0, 0.04);
        c.apply(Command::Forward);
        // Interval [0, 0.1] sent at t = 1 s, heard 0.05 s later.
        c.tick_at(0.1, 0.05, 1.0);
        assert!((c.cursor.load() - 0.1).abs() < 1e-9, "curseur en avance");
        assert!((c.position_at(1.0) + 0.05).abs() < 1e-9 || c.position_at(1.0) == 0.0);
        assert!((c.position_at(1.05) - 0.0).abs() < 1e-9);
        assert!((c.position_at(1.08) - 0.03).abs() < 1e-9);
        // Paused, restart from what was heard, not from the cursor.
        c.origin.store(0.03 - c.now());
        c.apply(Command::Pause);
        let p = c.position();
        assert!(p < 0.1 && (p - 0.03).abs() < 0.01, "{p}");
        assert_eq!(c.tick(0.1, 0.05).start, p, "the sound will resume there");
    }
}
