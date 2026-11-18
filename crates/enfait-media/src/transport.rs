//! Horloge de lecture et commandes de transport (J/K/L).
//!
//! L'horloge est partagée entre l'interface (qui lit la position et envoie
//! des commandes), la sortie audio (qui la fait avancer, et sert donc de
//! référence : l'image suit le son) et le décodeur d'images (qui la lit).
//! Elle ne prend aucun verrou : la sortie audio ne doit jamais attendre.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Vitesses de navette, en multiples de la vitesse normale.
const SHUTTLE: [f64; 4] = [1.0, 2.0, 4.0, 8.0];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    /// L : avant ; répété, accélère (×2, ×4, ×8). Depuis l'arrière, repart à ×1.
    Forward,
    /// J : arrière, même logique.
    Backward,
    /// K : pause.
    Pause,
    /// Espace : lecture ou pause.
    Toggle,
    /// Une image en avant (positif) ou en arrière, en pause.
    Step(i64),
    /// Aller à un instant, en secondes, sans changer la vitesse.
    Seek(f64),
}

/// Vitesse après une commande de navette.
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

/// Avance d'une horloge pendant un intervalle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tick {
    /// Position au début de l'intervalle, en secondes.
    pub start: f64,
    /// Vitesse pendant l'intervalle (0 en pause).
    pub speed: f64,
}

/// `f64` rangé dans un `AtomicU64`.
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
    /// Durée d'une image (pour les pas image par image).
    frame: f64,
    /// Origine des instants mémorisés ci-dessous.
    epoch: Instant,
    /// Position du curseur de la sortie : la fin du dernier intervalle
    /// envoyé à la carte son. Elle est en avance sur ce qu'on entend.
    cursor: AtomicF64,
    speed: AtomicF64,
    /// Saut demandé par l'interface, appliqué au prochain [`Clock::tick`]
    /// (NaN : aucun).
    pending: AtomicF64,
    /// Ce qu'on entend : position `heard` à l'instant `heard_at` (secondes
    /// depuis `epoch`), extrapolée ensuite à la vitesse courante.
    heard: AtomicF64,
    heard_at: AtomicF64,
}

impl Clock {
    /// `frame_duration` : durée d'une image en secondes (1/25 sans vidéo).
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
            heard: AtomicF64::new(0.0),
            heard_at: AtomicF64::new(0.0),
        }
    }

    pub const fn duration(&self) -> f64 {
        self.duration
    }

    fn now(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    /// Position de ce qu'on entend (ou verrait) maintenant, en secondes :
    /// c'est elle que l'image et l'interface suivent.
    pub fn position(&self) -> f64 {
        self.position_at(self.now())
    }

    fn position_at(&self, now: f64) -> f64 {
        let speed = self.speed();
        if speed == 0.0 {
            return self.cursor.load();
        }
        let elapsed = (now - self.heard_at.load()).max(0.0);
        self.clamp(self.heard.load() + speed * elapsed)
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

    fn jump(&self, t: f64) {
        let t = self.clamp(t);
        self.cursor.store(t);
        self.pending.store(t);
        self.heard.store(t);
        self.heard_at.store(self.now());
    }

    pub fn apply(&self, command: Command) {
        let speed = self.speed();
        let position = self.position();
        let next = shuttle(speed, command);
        match command {
            Command::Seek(t) => self.jump(t),
            Command::Step(n) => {
                // Se caler sur le début d'une image, puis avancer de `n`.
                #[allow(clippy::cast_precision_loss)]
                let frames = (position / self.frame + 1e-6).floor() + n as f64;
                self.jump(frames * self.frame);
            }
            Command::Forward | Command::Toggle if next > 0.0 && position >= self.duration => {
                // Relancer depuis le début quand on est au bout.
                self.jump(0.0);
            }
            // Au début, J ne fait rien (comme dans les logiciels de montage).
            Command::Backward if position <= 0.0 => {
                self.speed.store(0.0);
                return;
            }
            // Changement de vitesse : repartir de ce qu'on entend, pas du
            // curseur de la sortie qui a de l'avance.
            _ if next != speed => self.jump(position),
            _ => {}
        }
        self.speed.store(next);
    }

    /// Fait avancer l'horloge de `dt` secondes et renvoie ce qu'il faut
    /// jouer pendant cet intervalle. `delay` : temps avant que le début de
    /// l'intervalle soit entendu (latence de la sortie). S'arrête aux
    /// extrémités.
    pub fn tick(&self, dt: f64, delay: f64) -> Tick {
        self.tick_at(dt, delay, self.now())
    }

    fn tick_at(&self, dt: f64, delay: f64, now: f64) -> Tick {
        let pending = self.pending.swap(f64::NAN);
        let start = if pending.is_nan() {
            self.cursor.load()
        } else {
            pending
        };
        let speed = self.speed();
        if speed == 0.0 {
            return Tick { start, speed };
        }
        let end = start + speed * dt.max(0.0);
        let clamped = self.clamp(end);
        // Un saut demandé pendant l'intervalle l'emporte sur l'avance.
        if self.pending.load().is_nan() {
            self.cursor.store(clamped);
            // `start` sera entendu dans `delay` secondes.
            self.heard.store(start - speed * delay.max(0.0));
            self.heard_at.store(now);
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
        assert_eq!(c.position(), 0.0, "en pause, rien ne bouge");
        c.apply(Command::Forward);
        c.tick(0.5, 0.0);
        assert!((c.cursor.load() - 0.5).abs() < 1e-9);
        c.apply(Command::Forward); // ×2, repart de ce qu'on entend
        let t = c.tick(0.5, 0.0);
        assert_eq!(t.speed, 2.0);
        c.tick(0.5, 0.0);
        assert_eq!(c.cursor.load(), 1.0);
        assert!(!c.is_playing(), "arrêt au bout");
        assert_eq!(c.position(), 1.0);
        // Relancer au bout repart du début.
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
        // Toujours en lecture arrière : la position repart aussitôt de 2.
        assert!((c.position() - 2.0).abs() < 0.01, "borné à la durée");
    }

    #[test]
    fn backward_at_start_does_nothing() {
        let c = Clock::new(2.0, 0.04);
        c.apply(Command::Backward);
        assert!(!c.is_playing());
        assert_eq!(c.position(), 0.0);
        // Arrivée au début en lecture arrière : arrêt.
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
    fn position_follows_what_is_heard() {
        let c = Clock::new(10.0, 0.04);
        c.apply(Command::Forward);
        // Intervalle [0, 0.1] envoyé à t = 1 s, entendu 0,05 s plus tard.
        c.tick_at(0.1, 0.05, 1.0);
        assert!((c.cursor.load() - 0.1).abs() < 1e-9, "curseur en avance");
        assert!((c.position_at(1.0) + 0.05).abs() < 1e-9 || c.position_at(1.0) == 0.0);
        assert!((c.position_at(1.05) - 0.0).abs() < 1e-9);
        assert!((c.position_at(1.08) - 0.03).abs() < 1e-9);
        // En pause, on repart de ce qu'on entendait, pas du curseur.
        c.heard_at.store(c.now() - 0.08);
        c.apply(Command::Pause);
        let p = c.position();
        assert!(p < 0.1 && (p - 0.03).abs() < 0.01, "{p}");
        assert_eq!(c.tick(0.1, 0.05).start, p, "le son reprendra là");
    }
}
