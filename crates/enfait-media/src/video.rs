//! Décodage des images du proxy, guidé par l'horloge.
//!
//! Un fil de travail garde un processus ffmpeg ouvert qui débite les images
//! dans l'ordre (BGRA, l'ordre attendu par gpui). Tant que l'horloge avance
//! normalement, il lit la suite ; s'il est trop en retard ou en avance, il
//! relance ffmpeg au bon endroit (le proxy a une image clé toutes les
//! demi-secondes, ce qui rend ces relances rapides).
//!
//! ffmpeg ne sait décoder qu'en avant. Pour la lecture arrière (et les pas
//! en arrière), le décodeur lit d'un coup un bloc d'images qui se termine à
//! l'image voulue et le garde en mémoire ; il lit le bloc précédent à
//! l'avance, en continuant d'afficher les images déjà en mémoire.

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

/// Une image décodée.
#[derive(Debug)]
pub struct Frame {
    /// Index de l'image dans le média.
    pub index: i64,
    pub width: u32,
    pub height: u32,
    /// Pixels BGRA, ligne par ligne.
    pub data: Vec<u8>,
}

/// Index de l'image affichée à l'instant `t` (secondes).
pub fn frame_at(t: f64, rate: FrameRate) -> i64 {
    let fps = f64::from(rate.num()) / f64::from(rate.den());
    // Petite marge : un instant calé sur le début d'une image doit la donner.
    let x = (t.max(0.0) * fps + 1e-6).floor();
    // Borné par la durée du média : la conversion ne déborde pas.
    #[allow(clippy::cast_possible_truncation)]
    let i = x as i64;
    i
}

/// Début de l'image `index`, en secondes.
pub fn frame_time(index: i64, rate: FrameRate) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let i = index as f64;
    i * f64::from(rate.den()) / f64::from(rate.num())
}

/// Lecteur séquentiel d'images à partir d'un index.
struct Reader {
    child: Child,
    stdout: ChildStdout,
    /// Index de la prochaine image lue.
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
        // `-ss` avant `-i` : recherche rapide jusqu'à l'image clé précédente,
        // puis décodage jusqu'à l'instant exact. Le léger retrait évite de
        // tomber sur l'image suivante à cause des arrondis.
        let start = (frame_time(index, rate) - 0.001).max(0.0);
        let mut child = enfait_config::process::command("ffmpeg")
            .args(["-nostdin", "-hide_banner", "-v", "error"])
            .args(["-ss", &format!("{start:.6}")])
            .arg("-i")
            .arg(path)
            .args(["-map", "0:v:0", "-an", "-sn"])
            // `fps` : une source d'origine peut avoir une cadence variable ;
            // on veut exactement une image par index.
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

    /// Lit l'image suivante dans `buf` ; `false` à la fin du flux.
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

/// Correspondance entre les images du montage et celles de la source.
#[derive(Debug, Clone)]
struct FrameMap {
    edl: Arc<Edl>,
    rate: FrameRate,
    /// Dernière image de la source.
    last_source: i64,
    /// Dernière image du montage.
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

    /// Image du montage à l'instant `t`.
    fn output_at(&self, t: f64) -> i64 {
        frame_at(t, self.rate).clamp(0, self.last_output)
    }

    /// Image de la source montrée à l'image `o` du montage, et première
    /// image de la source de son morceau.
    fn source(&self, o: i64) -> (i64, i64) {
        let half = 0.5 * f64::from(self.rate.den()) / f64::from(self.rate.num());
        // Le milieu de l'image évite les erreurs d'arrondi aux frontières.
        let t = frame_time(o, self.rate) + half;
        let source = frame_at(self.edl.to_source(t), self.rate).clamp(0, self.last_source);
        let part_start = self
            .edl
            .locate(t)
            .map_or(0, |(_, p)| frame_at(p.source_start + half, self.rate));
        (source, part_start.clamp(0, source))
    }
}

/// Décodeur en tâche de fond. S'arrête quand il est abandonné.
pub struct VideoDecoder {
    latest: Arc<Mutex<Option<Arc<Frame>>>>,
    /// Mémoire d'images partagée : [`VideoDecoder::latest`] y cherche
    /// directement l'image de l'horloge, sans attendre le fil de décodage
    /// (qui peut être bloqué quelques dizaines de millisecondes au lancement
    /// de ffmpeg).
    cache: Arc<Mutex<FrameCache>>,
    clock: Arc<Clock>,
    map: FrameMap,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl VideoDecoder {
    /// `frame_count` : images de la source ; `edl` : montage lu (l'horloge
    /// est en temps montage) ; `size` : dimensions des images produites ;
    /// `filter` : filtres ffmpeg ajoutés (post-FX en aperçu).
    /// `on_frame` est appelé depuis le fil de travail à chaque image publiée.
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
                .map_err(|e| tracing::error!("décodeur vidéo : {e}"))
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

    /// Image à afficher maintenant : celle de l'horloge si elle est en
    /// mémoire, sinon la dernière publiée.
    pub fn latest(&self) -> Option<Arc<Frame>> {
        let (target, _) = self.map.source(self.map.output_at(self.clock.position()));
        let cached = self.cache.lock().ok().and_then(|c| c.get(target));
        let mut slot = self.latest.lock().ok()?;
        if let Some(frame) = cached {
            *slot = Some(frame);
        }
        slot.clone()
    }

    /// Image de la source attendue à la position actuelle de l'horloge.
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

/// Images gardées en mémoire autour de la tête de lecture (environ 100 Mo
/// à 960×540).
const CACHE_FRAMES: usize = 48;
/// Images décodées d'un coup pour la lecture arrière.
const BLOCK: i64 = 20;
/// Images lues d'avance pendant la lecture avant (de quoi préparer la suite
/// d'une coupe sans à-coup).
const AHEAD: i64 = 8;
/// Images qu'on accepte de lire et jeter plutôt que de relancer ffmpeg.
const MAX_SKIP: i64 = 12;

/// Images décodées de la source, rangées par index.
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

    /// Range une image, puis oublie les plus éloignées de `around`.
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

/// Ce que le décodeur voit avant de choisir quoi faire.
#[derive(Debug, Clone, Copy)]
struct Situation {
    /// Image du montage voulue par l'horloge.
    target: i64,
    /// Dernière image du montage.
    last: i64,
    /// Image de la source publiée.
    shown: Option<i64>,
    /// Prochaine image de la source du flux ffmpeg ouvert.
    next: Option<i64>,
    speed: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plan {
    /// Rien à faire.
    Hold,
    /// Publier l'image voulue, déjà en mémoire.
    Show,
    /// Décoder les images de la source `start..=end` (en continuant le flux
    /// ouvert s'il est déjà à `start`, sinon en relançant ffmpeg).
    Decode { start: i64, end: i64 },
}

/// `source` : image de la source et début de son morceau, pour une image du
/// montage ; `cached` : l'image de la source est-elle en mémoire ?
fn plan(s: Situation, source: &dyn Fn(i64) -> (i64, i64), cached: &dyn Fn(i64) -> bool) -> Plan {
    let (wanted, _) = source(s.target);
    let need = if !cached(wanted) {
        s.target
    } else if s.shown != Some(wanted) {
        return Plan::Show;
    } else {
        // Préparer la suite dans le sens de lecture ; en pause, l'arrière
        // (pour que J et ← partent sans attendre).
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
        // En arrière (lecture ou pas à pas) : tout un bloc qui finit à
        // l'image voulue, sans remonter avant le début de son morceau.
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
    /// Image du montage voulue par l'horloge.
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
        // Début d'un décodage en échec : ne pas relancer ffmpeg en boucle.
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
                                tracing::warn!("décodage de l'image {start} : {e}");
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

    /// Lit les images de la source jusqu'à `end` en les gardant en mémoire,
    /// et publie au passage l'image voulue dès qu'elle est disponible.
    /// S'interrompt si l'horloge réclame une image absente hors de ce bloc.
    /// `false` : fin du flux ou erreur.
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
        assert!(!c.contains(9), "la plus éloignée part");
    }

    #[test]
    fn frame_map_follows_the_cut() {
        // 10 i/s ; montage : [2 s ; 3 s) puis [0,5 s ; 1 s).
        let rate = FrameRate::new(10, 1).unwrap();
        let edl = Arc::new(Edl::from_ranges(&[(2.0, 3.0), (0.5, 1.0)]));
        let map = FrameMap::new(edl, rate, 100);
        assert_eq!(map.last_output, 14);
        assert_eq!(map.source(0), (20, 20));
        assert_eq!(map.source(9), (29, 20));
        assert_eq!(map.source(10), (5, 5), "après la coupe");
        assert_eq!(map.source(14), (9, 5));
        assert_eq!(map.output_at(99.0), 14);
        let id = FrameMap::new(Arc::new(Edl::identity(10.0)), rate, 100);
        for o in [0, 1, 37, 99] {
            assert_eq!(id.source(o).0, o);
        }
    }

    /// Montage identité de 100 images, et ensemble d'images en mémoire.
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
        // Image affichée : on lit d'avance jusqu'à AHEAD images.
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
        // Pas en arrière, en pause : un bloc entier qui finit à l'image voulue.
        let step_back = Situation {
            next: Some(51),
            ..at(49)
        };
        assert_eq!(decide(step_back, &[]), Plan::Decode { start: 30, end: 49 });
        // Près du début du média : le bloc est tronqué.
        let early = Situation {
            speed: -1.0,
            ..at(5)
        };
        assert_eq!(decide(early, &[]), Plan::Decode { start: 0, end: 5 });
        // En mémoire mais pas encore publiée.
        let block: Vec<i64> = (30..=40).collect();
        let ready = Situation {
            speed: -1.0,
            ..at(40)
        };
        assert_eq!(decide(ready, &block), Plan::Show);
        // Publiée, avec de la marge : rien à faire.
        let shown = Situation {
            shown: Some(40),
            ..ready
        };
        assert_eq!(decide(shown, &block), Plan::Hold);
        // Marge faible : on lit le bloc précédent à l'avance (en pause aussi).
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
        // Montage : images 20 à 29 de la source, puis 5 à 14.
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
            "relance après la coupe"
        );
        // En arrière, le bloc ne remonte pas avant le début du morceau.
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
