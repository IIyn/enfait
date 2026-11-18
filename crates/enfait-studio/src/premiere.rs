//! Export vers Premiere Pro (et tout logiciel qui lit le XML de Final Cut
//! Pro 7, `xmeml` version 4 : DaVinci Resolve, Final Cut Pro 7).
//!
//! La séquence garde le montage ouvert :
//!
//! - V1 : la prise montée (chaque morceau gardé), mise au cadre du projet ;
//! - V2 : animations et plans de b-roll, sur leurs blocs ;
//! - V3 : transitions rendues, à cheval sur les coupes ;
//! - A1-A2 : la voix (son de la prise, une piste par voie) ;
//! - puis les musiques (fondus et ducking en images clés de niveau) et, si
//!   on le veut, les SFX, une paire de pistes par couche.
//!
//! Les clips et les sons sont copiés à côté du XML (`<nom>-medias/`) : un
//! clip regénéré ensuite ne change pas un montage déjà envoyé. La prise est
//! désignée à sa place d'origine. Les post-FX ne sont pas transmis (à
//! refaire dans le logiciel d'arrivée) : `LISEZMOI.txt` les décrit.

use crate::session::{Sources, Visuals};
use anyhow::{Context as _, Result};
use enfait_core::{BlockKind, Framing};
use enfait_media::Edl;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PremiereOptions {
    /// Garder les effets sonores.
    pub sfx: bool,
}

impl Default for PremiereOptions {
    fn default() -> Self {
        Self { sfx: true }
    }
}

#[derive(Debug, Clone)]
pub struct PremiereReport {
    pub xml: PathBuf,
    pub media: PathBuf,
    /// Blocs (rang à partir de 1) sans animation ni plan : seule la prise
    /// les couvre.
    pub missing: Vec<usize>,
}

/// Un fichier désigné par la séquence.
#[derive(Debug, Clone)]
struct FileRef {
    name: String,
    path: PathBuf,
    /// Durée, en images de la séquence.
    frames: i64,
    video: Option<(u32, u32)>,
    channels: u16,
}

/// Un clip posé sur une piste, en images de la séquence.
#[derive(Debug, Clone)]
struct Clip {
    name: String,
    file: usize,
    start: i64,
    end: i64,
    /// Point d'entrée dans le fichier.
    source_in: i64,
    /// Voie du fichier (pistes son), à partir de 1.
    channel: Option<u16>,
    /// Filtres (`<filter>…</filter>`) déjà écrits.
    filters: String,
    /// Clips liés (image et son d'un même plan, voies d'un même son).
    group: usize,
}

#[derive(Debug, Clone, Default)]
struct Track {
    clips: Vec<Clip>,
    /// Voie de sortie (1 gauche, 2 droite) d'une piste son.
    output: Option<u16>,
}

/// La séquence avant écriture.
#[derive(Debug, Clone, Default)]
struct Sequence {
    name: String,
    width: u32,
    height: u32,
    /// Cadence : `(timebase, ntsc)`.
    rate: (u32, bool),
    duration: i64,
    files: Vec<FileRef>,
    video: Vec<Track>,
    audio: Vec<Track>,
    groups: usize,
}

impl Sequence {
    fn group(&mut self) -> usize {
        self.groups = self.groups.saturating_add(1);
        self.groups
    }

    fn file(&mut self, file: FileRef) -> usize {
        if let Some(i) = self.files.iter().position(|f| f.path == file.path) {
            return i;
        }
        self.files.push(file);
        self.files.len().saturating_sub(1)
    }
}

/// Couches de pistes son : chaque couche est une paire (gauche, droite) ;
/// un son va dans la première couche libre sur son intervalle.
struct Layers {
    first: usize,
    busy: Vec<Vec<(i64, i64)>>,
}

impl Layers {
    fn new(seq: &Sequence) -> Self {
        Self {
            first: seq.audio.len(),
            busy: Vec::new(),
        }
    }

    /// Indices des deux pistes (dans `seq.audio`) qui reçoivent `[a, b)`.
    fn place(&mut self, seq: &mut Sequence, a: i64, b: i64) -> (usize, usize) {
        let free = self
            .busy
            .iter()
            .position(|spans| spans.iter().all(|&(x, y)| b <= x || a >= y));
        let layer = free.unwrap_or_else(|| {
            self.busy.push(Vec::new());
            seq.audio.push(Track {
                clips: Vec::new(),
                output: Some(1),
            });
            seq.audio.push(Track {
                clips: Vec::new(),
                output: Some(2),
            });
            self.busy.len().saturating_sub(1)
        });
        if let Some(spans) = self.busy.get_mut(layer) {
            spans.push((a, b));
        }
        let left = self.first.saturating_add(layer.saturating_mul(2));
        (left, left.saturating_add(1))
    }
}

fn frames(t: f64, fps: f64) -> i64 {
    // Durées d'une vidéo : la conversion ne déborde pas.
    #[allow(clippy::cast_possible_truncation)]
    let f = (t * fps).round() as i64;
    f
}

fn db_to_linear(db: f64) -> f64 {
    10_f64.powf(db / 20.0)
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// `file://localhost/…`, chemin encodé octet par octet.
fn path_url(path: &Path) -> String {
    let mut out = String::from("file://localhost");
    let text = path.to_string_lossy();
    if !text.starts_with('/') {
        out.push('/');
    }
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(char::from(byte));
            }
            b'\\' => out.push('/'),
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

/// Filtre « Audio Levels » : niveaux linéaires aux images `when` (depuis le
/// début du clip). Toujours au moins deux images clés : sans elles, certains
/// imports lisent le niveau à −∞.
fn audio_levels(keys: &[(i64, f64)], length: i64) -> String {
    let mut keys: Vec<(i64, f64)> = keys
        .iter()
        .map(|&(w, v)| (w.clamp(0, length), v.clamp(0.0, 3.981)))
        .collect();
    keys.sort_by_key(|k| k.0);
    keys.dedup_by_key(|k| k.0);
    let first = keys.first().map_or(1.0, |k| k.1);
    let last = keys.last().map_or(1.0, |k| k.1);
    if keys.first().is_none_or(|k| k.0 > 0) {
        keys.insert(0, (0, first));
    }
    if keys.last().is_none_or(|k| k.0 < length) {
        keys.push((length, last));
    }
    let mut s = String::from(
        "<filter><effect><name>Audio Levels</name><effectid>audiolevels</effectid>\
         <effectcategory>audiolevels</effectcategory><effecttype>audiolevels</effecttype>\
         <mediatype>audio</mediatype><parameter><parameterid>level</parameterid>\
         <name>Level</name><valuemin>0</valuemin><valuemax>3.98109</valuemax>",
    );
    let _ = write!(s, "<value>{first:.5}</value>");
    for (when, value) in keys {
        let _ = write!(
            s,
            "<keyframe><when>{when}</when><value>{value:.5}</value></keyframe>"
        );
    }
    s.push_str("</parameter></effect></filter>");
    s
}

/// Filtre « Basic Motion » : échelle (%) et centre (fraction de l'image).
fn basic_motion(scale: f64, center: (f64, f64)) -> String {
    format!(
        "<filter><effect><name>Basic Motion</name><effectid>basic</effectid>\
         <effectcategory>motion</effectcategory><effecttype>motion</effecttype>\
         <mediatype>video</mediatype>\
         <parameter><parameterid>scale</parameterid><name>Scale</name><valuemin>0</valuemin>\
         <valuemax>1000</valuemax><value>{scale:.3}</value></parameter>\
         <parameter><parameterid>center</parameterid><name>Center</name>\
         <value><horiz>{:.5}</horiz><vert>{:.5}</vert></value></parameter>\
         </effect></filter>",
        center.0, center.1
    )
}

/// Mise au cadre de la prise `(w, h)` dans la séquence `(sw, sh)`.
fn rush_motion(rush: (u32, u32), seq: (u32, u32), framing: Framing, pan: i8) -> Option<String> {
    let (w, h) = (f64::from(rush.0.max(1)), f64::from(rush.1.max(1)));
    let (sw, sh) = (f64::from(seq.0), f64::from(seq.1));
    let (fit, fill) = ((sw / w).min(sh / h), (sw / w).max(sh / h));
    let scale = match framing {
        Framing::Fit => fit,
        Framing::Fill => fill,
    };
    // Décalage de l'image rognée : `pan` % de la marge.
    let k = f64::from(pan) / 100.0;
    let center = match framing {
        Framing::Fill => (
            -k * (w * scale - sw).max(0.0) / sw,
            -k * (h * scale - sh).max(0.0) / sh,
        ),
        Framing::Fit => (0.0, 0.0),
    };
    let identity = (scale - 1.0).abs() < 1e-4 && center.0.abs() < 1e-6 && center.1.abs() < 1e-6;
    (!identity).then(|| basic_motion(scale * 100.0, center))
}

fn rate_xml((timebase, ntsc): (u32, bool)) -> String {
    format!(
        "<rate><timebase>{timebase}</timebase><ntsc>{}</ntsc></rate>",
        if ntsc { "TRUE" } else { "FALSE" }
    )
}

/// Écrit la séquence en XML.
fn render(seq: &Sequence) -> String {
    let rate = rate_xml(seq.rate);
    // Identifiants et places (piste, rang) de chaque clip, pour les liens.
    let mut places: BTreeMap<usize, Vec<(String, &str, usize, usize)>> = BTreeMap::new();
    let mut ids: Vec<Vec<String>> = Vec::new();
    let mut n = 0_usize;
    for (kind, tracks) in [("video", &seq.video), ("audio", &seq.audio)] {
        for (t, track) in tracks.iter().enumerate() {
            let mut track_ids = Vec::new();
            for (c, clip) in track.clips.iter().enumerate() {
                n = n.saturating_add(1);
                let id = format!("clipitem-{n}");
                places.entry(clip.group).or_default().push((
                    id.clone(),
                    kind,
                    t.saturating_add(1),
                    c.saturating_add(1),
                ));
                track_ids.push(id);
            }
            ids.push(track_ids);
        }
    }
    let mut written = vec![false; seq.files.len()];
    let mut x = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE xmeml>\n");
    x.push_str("<xmeml version=\"4\">\n<sequence id=\"sequence-1\">\n");
    let _ = writeln!(
        x,
        "<name>{}</name><duration>{}</duration>{rate}",
        escape(&seq.name),
        seq.duration
    );
    let _ = writeln!(
        x,
        "<timecode>{rate}<string>00:00:00:00</string><frame>0</frame>\
         <displayformat>NDF</displayformat></timecode>"
    );
    x.push_str("<media>\n<video>\n");
    let _ = writeln!(
        x,
        "<format><samplecharacteristics>{rate}<width>{}</width><height>{}</height>\
         <anamorphic>FALSE</anamorphic><pixelaspectratio>square</pixelaspectratio>\
         <fielddominance>none</fielddominance></samplecharacteristics></format>",
        seq.width, seq.height
    );
    let mut track_index = 0_usize;
    let mut write_track = |x: &mut String, track: &Track, kind: &str| {
        let track_ids = ids.get(track_index).cloned().unwrap_or_default();
        track_index = track_index.saturating_add(1);
        x.push_str("<track>\n");
        for (clip, id) in track.clips.iter().zip(&track_ids) {
            let length = clip.end.saturating_sub(clip.start);
            let Some(file) = seq.files.get(clip.file) else {
                continue;
            };
            let _ = write!(
                x,
                "<clipitem id=\"{id}\"><name>{}</name><enabled>TRUE</enabled>\
                 <duration>{}</duration>{rate}<start>{}</start><end>{}</end>\
                 <in>{}</in><out>{}</out>",
                escape(&clip.name),
                file.frames,
                clip.start,
                clip.end,
                clip.source_in,
                clip.source_in.saturating_add(length)
            );
            let file_id = format!("file-{}", clip.file.saturating_add(1));
            if written.get(clip.file).copied().unwrap_or(true) {
                let _ = write!(x, "<file id=\"{file_id}\"/>");
            } else {
                if let Some(w) = written.get_mut(clip.file) {
                    *w = true;
                }
                let _ = write!(
                    x,
                    "<file id=\"{file_id}\"><name>{}</name><pathurl>{}</pathurl>{rate}\
                     <duration>{}</duration><media>",
                    escape(&file.name),
                    escape(&path_url(&file.path)),
                    file.frames
                );
                if let Some((w, h)) = file.video {
                    let _ = write!(
                        x,
                        "<video><samplecharacteristics>{rate}<width>{w}</width>\
                         <height>{h}</height></samplecharacteristics></video>"
                    );
                }
                if file.channels > 0 {
                    let _ = write!(
                        x,
                        "<audio><samplecharacteristics><depth>16</depth>\
                         <samplerate>48000</samplerate></samplecharacteristics>\
                         <channelcount>{}</channelcount></audio>",
                        file.channels
                    );
                }
                x.push_str("</media></file>");
            }
            if let Some(channel) = clip.channel {
                let _ = write!(
                    x,
                    "<sourcetrack><mediatype>audio</mediatype>\
                     <trackindex>{channel}</trackindex></sourcetrack>"
                );
            }
            x.push_str(&clip.filters);
            if let Some(linked) = places.get(&clip.group).filter(|l| l.len() > 1) {
                for (other, media, t, c) in linked {
                    let _ = write!(
                        x,
                        "<link><linkclipref>{other}</linkclipref><mediatype>{media}</mediatype>\
                         <trackindex>{t}</trackindex><clipindex>{c}</clipindex></link>"
                    );
                }
            }
            x.push_str("</clipitem>\n");
        }
        x.push_str("<enabled>TRUE</enabled><locked>FALSE</locked>");
        if kind == "audio"
            && let Some(output) = track.output
        {
            let _ = write!(x, "<outputchannelindex>{output}</outputchannelindex>");
        }
        x.push_str("\n</track>\n");
    };
    for track in &seq.video {
        write_track(&mut x, track, "video");
    }
    x.push_str("</video>\n<audio>\n<numOutputChannels>2</numOutputChannels>\n");
    x.push_str(
        "<format><samplecharacteristics><depth>16</depth><samplerate>48000</samplerate>\
         </samplecharacteristics></format>\n",
    );
    for track in &seq.audio {
        write_track(&mut x, track, "audio");
    }
    x.push_str("</audio>\n</media>\n</sequence>\n</xmeml>\n");
    x
}

/// Informations d'un fichier pour la séquence (ffprobe).
fn file_ref(path: &Path, name: &str, fps: f64) -> Result<FileRef> {
    let info = enfait_media::probe::probe(path)
        .with_context(|| format!("fichier illisible : {}", path.display()))?;
    Ok(FileRef {
        name: name.to_owned(),
        path: path.to_path_buf(),
        frames: frames(info.duration, fps).max(1),
        video: info.video.map(|v| (v.width, v.height)),
        channels: info.audio.map_or(0, |a| a.channels),
    })
}

/// Copie `from` dans `dir` sous le nom `name` ; renvoie le chemin absolu.
fn copy_into(from: &Path, dir: &Path, name: &str) -> Result<PathBuf> {
    let to = dir.join(name);
    std::fs::copy(from, &to).with_context(|| format!("copie de {}", from.display()))?;
    Ok(std::path::absolute(&to).unwrap_or(to))
}

/// Ajoute un son stéréo ou mono (`file`) sur une couche de `layers`.
#[allow(clippy::too_many_arguments)]
fn place_sound(
    seq: &mut Sequence,
    layers: &mut Layers,
    file: usize,
    name: &str,
    start: i64,
    end: i64,
    source_in: i64,
    filters: &str,
) {
    if end <= start {
        return;
    }
    let channels = seq.files.get(file).map_or(2, |f| f.channels).clamp(1, 2);
    let (left, right) = layers.place(seq, start, end);
    let group = seq.group();
    for (channel, track) in [(1_u16, left), (2, right)] {
        // Un son mono va sur les deux pistes de la couche (même voie).
        let source = if channels == 1 { 1 } else { channel };
        if let Some(t) = seq.audio.get_mut(track) {
            t.clips.push(Clip {
                name: name.to_owned(),
                file,
                start,
                end,
                source_in,
                channel: Some(source),
                filters: filters.to_owned(),
                group,
            });
        }
    }
}

/// Construit la séquence et copie les médias dans `media`.
fn build(sources: &Sources<'_>, options: PremiereOptions, media: &Path) -> Result<Sequence> {
    let project = sources.project;
    let settings = project.settings();
    let rate = settings.frame_rate;
    let fps = f64::from(rate.num()) / f64::from(rate.den());
    let ntsc = rate.den() == 1001;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let timebase = fps.round() as u32;
    let mut seq = Sequence {
        name: project.name().to_owned(),
        width: settings.resolution.width,
        height: settings.resolution.height,
        rate: (timebase, ntsc),
        duration: frames(project.duration().as_secs_f64(), fps),
        ..Sequence::default()
    };
    let size = (seq.width, seq.height);
    let rush_path = std::path::absolute(sources.rush).unwrap_or_else(|_| sources.rush.into());
    let rush_name = rush_path
        .file_name()
        .map_or_else(|| "prise".to_owned(), |n| n.to_string_lossy().into_owned());
    let rush = seq.file(file_ref(&rush_path, &rush_name, fps)?);
    let (rush_video, rush_channels) = seq
        .files
        .get(rush)
        .map_or((None, 0), |f| (f.video, f.channels));

    // V1 et voix : la prise montée.
    let mut v1 = Track::default();
    let voice_tracks = usize::from(rush_channels.clamp(1, 2));
    let mut voice: Vec<Track> = (0..voice_tracks)
        .map(|i| Track {
            clips: Vec::new(),
            output: (voice_tracks == 2).then_some(if i == 0 { 1 } else { 2 }),
        })
        .collect();
    let motion = rush_video
        .and_then(|v| rush_motion(v, size, settings.framing, settings.pan()))
        .unwrap_or_default();
    let voice_level = db_to_linear(f64::from(sources.finish.voice_gain_db));
    for part in Edl::from_ranges(sources.parts).parts() {
        let (start, end) = (frames(part.start, fps), frames(part.end(), fps));
        if end <= start {
            continue;
        }
        let source_in = frames(part.source_start, fps);
        let group = seq.group();
        if rush_video.is_some() {
            v1.clips.push(Clip {
                name: rush_name.clone(),
                file: rush,
                start,
                end,
                source_in,
                channel: None,
                filters: motion.clone(),
                group,
            });
        }
        for (i, track) in voice.iter_mut().enumerate() {
            track.clips.push(Clip {
                name: rush_name.clone(),
                file: rush,
                start,
                end,
                source_in,
                channel: Some(u16::try_from(i.saturating_add(1)).unwrap_or(1)),
                filters: audio_levels(&[(0, voice_level)], end.saturating_sub(start)),
                group,
            });
        }
    }
    seq.video.push(v1);
    seq.audio.extend(voice);

    // V2 : animations et plans ; V3 : transitions.
    let visuals = Visuals::of(sources.studio, project);
    let mut v2 = Track::default();
    for (i, (block, (a, b, clip))) in project.blocks().iter().zip(&visuals.blocks).enumerate() {
        let Some(clip) = clip else { continue };
        let kind = match block.kind {
            BlockKind::Broll => "broll",
            BlockKind::Mixed => "mixte",
            BlockKind::Effect => "effet",
            _ => "animation",
        };
        let name = format!("bloc-{:02}-{kind}.mp4", i.saturating_add(1));
        let copied = copy_into(clip, media, &name)?;
        let file = seq.file(file_ref(&copied, &name, fps)?);
        let group = seq.group();
        v2.clips.push(Clip {
            name,
            file,
            start: frames(*a, fps),
            end: frames(*b, fps),
            source_in: 0,
            channel: None,
            filters: String::new(),
            group,
        });
    }
    seq.video.push(v2);
    let mut v3 = Track::default();
    for (a, b, clip) in &visuals.transitions {
        let rank = visuals
            .blocks
            .iter()
            .position(|blk| (blk.1 - *a).abs() < 1e-6 || (*a < blk.1 && blk.1 < *b))
            .map_or(0, |i| i.saturating_add(1));
        let name = format!("transition-{rank:02}.mp4");
        let copied = copy_into(clip, media, &name)?;
        let file = seq.file(file_ref(&copied, &name, fps)?);
        let group = seq.group();
        v3.clips.push(Clip {
            name,
            file,
            start: frames(*a, fps),
            end: frames(*b, fps),
            source_in: 0,
            channel: None,
            filters: String::new(),
            group,
        });
    }
    seq.video.push(v3);

    // Musiques : niveau, fondus et ducking en images clés.
    let plan = crate::mix::plan(sources, options.sfx);
    let mut sounds: BTreeMap<String, usize> = BTreeMap::new();
    let mut sound_file = |seq: &mut Sequence, file: &str, path: &Path| -> Result<usize> {
        if let Some(&i) = sounds.get(file) {
            return Ok(i);
        }
        let copied = copy_into(path, media, file)?;
        let i = seq.file(file_ref(&copied, file, fps)?);
        sounds.insert(file.to_owned(), i);
        Ok(i)
    };
    let mut music_layers = Layers::new(&seq);
    for (m, path) in &plan.music {
        let file = sound_file(&mut seq, &m.file, path)?;
        let file_len = seq.files.get(file).map_or(1, |f| f.frames).max(1);
        let len = (m.end - m.start).max(0.01);
        let (fade_in, fade_out) = (m.fade_in.min(len / 2.0), m.fade_out.min(len / 2.0));
        let gain = db_to_linear(f64::from(m.gain_db));
        let level = |t: f64| {
            let mut v = gain * plan.ducking.gain(&plan.duck_spans, t);
            if fade_in > 0.0 {
                v *= ((t - m.start) / fade_in).clamp(0.0, 1.0);
            }
            if fade_out > 0.0 {
                v *= ((m.end - t) / fade_out).clamp(0.0, 1.0);
            }
            v
        };
        // Instants où la courbe change de pente.
        let (attack, release) = (
            plan.ducking.attack.max(0.01),
            plan.ducking.release.max(0.01),
        );
        let mut corners = vec![m.start, m.start + fade_in, m.end - fade_out, m.end];
        if plan.ducking.enabled {
            for (a, b) in &plan.duck_spans {
                corners.extend([a - attack, *a, *b, b + release]);
            }
        }
        corners.retain(|t| (m.start..=m.end).contains(t));
        corners.sort_by(f64::total_cmp);
        // Morceau trop court : repris au début, un clip par passage.
        let (mut at, mut pos) = (
            frames(m.start, fps),
            frames(m.offset, fps).checked_rem(file_len).unwrap_or(0),
        );
        let stop = frames(m.end, fps);
        while at < stop {
            let end = stop.min(at.saturating_add(file_len.saturating_sub(pos)));
            #[allow(clippy::cast_precision_loss)]
            let keys: Vec<(i64, f64)> = corners
                .iter()
                .map(|&t| (frames(t, fps).saturating_sub(at), level(t)))
                .filter(|&(w, _)| w >= 0 && w <= end.saturating_sub(at))
                .chain([
                    (0, level(at as f64 / fps)),
                    (end.saturating_sub(at), level(end as f64 / fps)),
                ])
                .collect();
            let filters = audio_levels(&keys, end.saturating_sub(at));
            place_sound(
                &mut seq,
                &mut music_layers,
                file,
                &m.file,
                at,
                end,
                pos,
                &filters,
            );
            at = end;
            pos = 0;
        }
    }
    let mut sfx_layers = Layers::new(&seq);
    for (s, path) in &plan.sfx {
        let file = sound_file(&mut seq, &s.file, path)?;
        let file_len = seq.files.get(file).map_or(1, |f| f.frames).max(1);
        let start = frames(s.at, fps);
        let end = start.saturating_add(file_len).min(seq.duration);
        let filters = audio_levels(
            &[(0, db_to_linear(f64::from(s.gain_db)))],
            end.saturating_sub(start),
        );
        place_sound(
            &mut seq,
            &mut sfx_layers,
            file,
            &s.file,
            start,
            end,
            0,
            &filters,
        );
    }
    for track in seq.video.iter_mut().chain(seq.audio.iter_mut()) {
        track.clips.sort_by_key(|c| c.start);
    }
    Ok(seq)
}

/// Notes jointes au XML : contenu des pistes, post-FX à refaire, crédits.
fn readme(sources: &Sources<'_>, seq: &Sequence, options: PremiereOptions) -> String {
    let fx = sources.look.fx;
    let mut s = format!(
        "Montage enfait : {}\n\n\
         Ouvrir dans Premiere Pro : Fichier > Importer, choisir le fichier .xml.\n\n\
         Pistes\n\
         - V1 : la prise montée\n\
         - V2 : animations et plans de b-roll\n\
         - V3 : transitions\n\
         - A1{} : la voix\n\
         - puis les musiques (fondus et ducking en images clés de niveau){}\n\n\
         Les clips et les sons sont copiés dans ce dossier ; la prise est désignée à sa \
         place d'origine.\n",
        seq.name,
        if seq.audio.len() > 1 && seq.audio.first().is_some_and(|t| t.output.is_some()) {
            "-A2"
        } else {
            ""
        },
        if options.sfx {
            ", puis les SFX"
        } else {
            " (sans les SFX)"
        }
    );
    if !fx.is_neutral() {
        let _ = write!(
            s,
            "\nPost-FX à refaire (non transmis par le XML), réglages de -1 à 1 :\n\
             contraste {:.2}, saturation {:.2}, température {:.2}, grain {:.2}, \
             vignettage {:.2}, aberration {:.2}\n",
            fx.contrast, fx.saturation, fx.temperature, fx.grain, fx.vignette, fx.aberration
        );
    }
    let credits = enfait_core::license::credits(&sources.look.assets);
    if !credits.trim().is_empty() {
        let _ = write!(s, "\nCrédits\n{credits}\n");
    }
    s
}

/// Écrit `xml` et copie les médias à côté, dans `<nom>-medias/`.
pub fn export(
    sources: &Sources<'_>,
    options: PremiereOptions,
    xml: &Path,
) -> Result<PremiereReport> {
    let stem = xml.file_stem().map_or_else(
        || "montage".to_owned(),
        |s| s.to_string_lossy().into_owned(),
    );
    let media = xml.with_file_name(format!("{stem}-medias"));
    if media.exists() {
        std::fs::remove_dir_all(&media)
            .with_context(|| format!("remplacement de {}", media.display()))?;
    }
    std::fs::create_dir_all(&media)?;
    let seq = build(sources, options, &media)?;
    std::fs::write(xml, render(&seq)).with_context(|| format!("écriture de {}", xml.display()))?;
    std::fs::write(media.join("LISEZMOI.txt"), readme(sources, &seq, options))?;
    Ok(PremiereReport {
        xml: xml.to_path_buf(),
        media,
        missing: Visuals::of(sources.studio, sources.project).missing(sources.project),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_encoded() {
        assert_eq!(
            path_url(Path::new("/home/a/Prise été #1.mp4")),
            "file://localhost/home/a/Prise%20%C3%A9t%C3%A9%20%231.mp4"
        );
    }

    #[test]
    fn levels_always_have_keyframes() {
        let f = audio_levels(&[(0, 0.5)], 100);
        assert!(f.contains("<keyframe><when>0</when><value>0.50000</value></keyframe>"));
        assert!(f.contains("<keyframe><when>100</when><value>0.50000</value></keyframe>"));
        // Bornes : au-delà de +12 dB, ramené ; instants hors du clip, ramenés.
        let f = audio_levels(&[(-5, 9.0), (300, 0.1)], 100);
        assert!(f.contains("<when>0</when><value>3.98100</value>"), "{f}");
        assert!(f.contains("<when>100</when><value>0.10000</value>"), "{f}");
    }

    #[test]
    fn rush_is_framed_like_the_render() {
        // Prise 16:9 dans une vidéo verticale, rognée : 1920/1080 → 177,8 %.
        let m = rush_motion((1920, 1080), (1080, 1920), Framing::Fill, 0).unwrap();
        assert!(m.contains("<value>177.778</value>"), "{m}");
        // Bandes noires : 1080/1920 → 56,25 %.
        let m = rush_motion((1920, 1080), (1080, 1920), Framing::Fit, 0).unwrap();
        assert!(m.contains("<value>56.250</value>"), "{m}");
        // Même format : rien à faire.
        assert!(rush_motion((1920, 1080), (1920, 1080), Framing::Fill, 0).is_none());
        // Décalage vers la droite de l'image : le centre part à gauche.
        let m = rush_motion((1920, 1080), (1080, 1920), Framing::Fill, 50).unwrap();
        assert!(m.contains("<horiz>-1.08025"), "{m}");
    }

    #[test]
    fn sounds_share_layers_when_they_do_not_overlap() {
        let mut seq = Sequence::default();
        let mut layers = Layers::new(&seq);
        assert_eq!(layers.place(&mut seq, 0, 10), (0, 1));
        assert_eq!(layers.place(&mut seq, 5, 15), (2, 3));
        assert_eq!(layers.place(&mut seq, 10, 20), (0, 1));
        assert_eq!(seq.audio.len(), 4);
    }

    #[test]
    fn sequence_xml_links_and_reuses_files() {
        let mut seq = Sequence {
            name: "Essai & co".into(),
            width: 1080,
            height: 1920,
            rate: (30, false),
            duration: 90,
            ..Sequence::default()
        };
        let rush = seq.file(FileRef {
            name: "prise.mov".into(),
            path: "/r/prise.mov".into(),
            frames: 300,
            video: Some((1920, 1080)),
            channels: 1,
        });
        let g = seq.group();
        let clip = |start, end, channel| Clip {
            name: "prise.mov".into(),
            file: rush,
            start,
            end,
            source_in: start + 30,
            channel,
            filters: String::new(),
            group: g,
        };
        seq.video.push(Track {
            clips: vec![clip(0, 90, None)],
            output: None,
        });
        seq.audio.push(Track {
            clips: vec![clip(0, 90, Some(1))],
            output: None,
        });
        let x = render(&seq);
        assert!(x.contains("<name>Essai &amp; co</name>"));
        assert_eq!(x.matches("<pathurl>").count(), 1, "fichier décrit une fois");
        assert!(x.contains("<file id=\"file-1\"/>"));
        assert!(x.contains("<in>30</in><out>120</out>"));
        assert!(x.contains("<linkclipref>clipitem-2</linkclipref><mediatype>audio</mediatype>"));
        assert!(x.contains("<sourcetrack><mediatype>audio</mediatype><trackindex>1</trackindex>"));
    }
}
