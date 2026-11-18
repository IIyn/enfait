//! La session de l'interface vue par l'atelier : où sont rangés les clips
//! de chaque bloc, ce que montre chaque bloc, et lecture de la session sur
//! disque (pour l'export depuis le CLI ou le serveur MCP).
//!
//! Les fichiers d'un bloc (composition, images, clip) portent le format du
//! projet dans leur nom (rien pour le grand format 16:9, qui garde les noms
//! d'origine) : changer de format ne mélange pas les rendus, et revenir au
//! format précédent retrouve ses animations.

use anyhow::{Context as _, Result};
use enfait_core::{
    AspectRatio, Block, BlockId, BlockKind, Finish, Look, Project, ProjectSettings, Transcript,
    Transition,
};
use enfait_media::Edl;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Part de la transition placée avant la coupe.
pub const TRANSITION_BEFORE: f64 = 0.4;

fn suffix(settings: &ProjectSettings) -> &'static str {
    match settings.aspect {
        AspectRatio::Landscape16x9 => "",
        AspectRatio::Portrait9x16 => "-9x16",
        AspectRatio::Square1x1 => "-1x1",
    }
}

/// Nom des fichiers d'un bloc dans l'atelier.
pub fn block_name(id: BlockId, settings: &ProjectSettings) -> String {
    format!("bloc-{id}{}", suffix(settings))
}

/// Nom des fichiers de la transition qui suit un bloc.
pub fn transition_name(id: BlockId, settings: &ProjectSettings) -> String {
    format!("transition-{id}{}", suffix(settings))
}

/// Clip rendu d'un bloc.
pub fn clip_path(dir: &Path, id: BlockId, settings: &ProjectSettings) -> PathBuf {
    dir.join("clips")
        .join(format!("{}.mp4", block_name(id, settings)))
}

/// Clip de la transition qui suit un bloc.
pub fn transition_clip_path(dir: &Path, id: BlockId, settings: &ProjectSettings) -> PathBuf {
    dir.join("clips")
        .join(format!("{}.mp4", transition_name(id, settings)))
}

/// Clip du plan de b-roll d'un bloc.
pub fn broll_clip_path(dir: &Path, id: BlockId, settings: &ProjectSettings) -> PathBuf {
    dir.join("clips")
        .join(format!("broll-{id}{}.mp4", suffix(settings)))
}

/// Ce dont dépend le clip de b-roll d'un bloc (fichier, départ, durée,
/// format) : écrit à côté du clip, il dit si le clip est à jour.
pub fn broll_key(block: &Block, settings: &ProjectSettings) -> Option<String> {
    let broll = block.broll.as_ref()?;
    let rate = settings.frame_rate;
    Some(format!(
        "{}|{}|{}|{}x{}|{}/{}",
        broll.file,
        broll.start.get(),
        block.range.duration().get(),
        settings.resolution.width,
        settings.resolution.height,
        rate.num(),
        rate.den()
    ))
}

/// Le clip de b-roll du bloc existe et correspond à son plan actuel.
pub fn broll_ready(dir: &Path, block: &Block, settings: &ProjectSettings) -> bool {
    let Some(key) = broll_key(block, settings) else {
        return false;
    };
    let clip = broll_clip_path(dir, block.id, settings);
    clip.is_file() && std::fs::read_to_string(clip.with_extension("key")).is_ok_and(|k| k == key)
}

/// Images montrées pendant un bloc, si ce n'est pas la prise : son plan de
/// b-roll, ou son animation.
pub fn visual_clip(dir: &Path, block: &Block, settings: &ProjectSettings) -> Option<PathBuf> {
    match block.kind {
        BlockKind::Rush => None,
        BlockKind::Broll => {
            broll_ready(dir, block, settings).then(|| broll_clip_path(dir, block.id, settings))
        }
        _ => Some(clip_path(dir, block.id, settings)).filter(|c| c.is_file()),
    }
}

/// Fenêtre (temps montage) de la transition entre `block` et `next`.
pub fn transition_window(block: &Block, next: &Block) -> (f64, f64) {
    let boundary = block.range.end().as_secs_f64();
    let length = crate::TRANSITION_LENGTH;
    (
        (boundary - length * TRANSITION_BEFORE).max(block.range.start().as_secs_f64()),
        (boundary + length * (1.0 - TRANSITION_BEFORE)).min(next.range.end().as_secs_f64()),
    )
}

/// Morceaux de la prise (début, fin) qui donnent l'intervalle `[a, b)` du
/// montage.
pub fn source_pieces(parts: &[(f64, f64)], a: f64, b: f64) -> Vec<(f64, f64)> {
    let edl = Edl::from_ranges(parts);
    edl.parts()
        .iter()
        .filter_map(|p| {
            let (x, y) = (a.max(p.start), b.min(p.end()));
            (y - x > 1e-3).then_some((
                p.source_start + (x - p.start),
                p.source_start + (y - p.start),
            ))
        })
        .collect()
}

/// Ce qu'on voit, bloc par bloc, en temps montage.
#[derive(Debug, Clone, Default)]
pub struct Visuals {
    /// `(début, fin, clip)` de chaque bloc ; sans clip, la prise.
    pub blocks: Vec<(f64, f64, Option<PathBuf>)>,
    /// `(début, fin, clip)` des transitions rendues, par-dessus.
    pub transitions: Vec<(f64, f64, PathBuf)>,
}

impl Visuals {
    pub fn of(dir: &Path, project: &Project) -> Self {
        let settings = project.settings();
        let blocks = project.blocks();
        Self {
            blocks: blocks
                .iter()
                .map(|b| {
                    (
                        b.range.start().as_secs_f64(),
                        b.range.end().as_secs_f64(),
                        visual_clip(dir, b, settings),
                    )
                })
                .collect(),
            transitions: blocks
                .windows(2)
                .filter_map(|w| {
                    let [block, next] = w else { return None };
                    let clip = transition_clip_path(dir, block.id, settings);
                    (block.transition_after != Transition::Cut && clip.is_file()).then(|| {
                        let (a, b) = transition_window(block, next);
                        (a, b, clip)
                    })
                })
                .collect(),
        }
    }

    /// Aucun clip : seule la prise est montrée.
    pub fn rush_only(&self) -> bool {
        self.blocks.iter().all(|b| b.2.is_none()) && self.transitions.is_empty()
    }

    /// Blocs qui devraient montrer autre chose que la prise mais n'ont pas
    /// encore de clip (rang à partir de 1).
    pub fn missing(&self, project: &Project) -> Vec<usize> {
        project
            .blocks()
            .iter()
            .zip(&self.blocks)
            .enumerate()
            .filter(|(_, (b, v))| b.kind != BlockKind::Rush && v.2.is_none())
            .map(|(i, _)| i.saturating_add(1))
            .collect()
    }
}

/// Tout ce qu'il faut pour monter la vidéo finale.
#[derive(Debug, Clone)]
pub struct Sources<'a> {
    /// Atelier : clips des blocs et des transitions.
    pub studio: &'a Path,
    pub library: &'a Path,
    pub project: &'a Project,
    /// Transcription du montage.
    pub transcript: &'a Transcript,
    pub look: &'a Look,
    pub finish: &'a Finish,
    /// La prise d'origine.
    pub rush: &'a Path,
    /// Morceaux de la prise qui forment le montage.
    pub parts: &'a [(f64, f64)],
}

impl Sources<'_> {
    /// Chemin d'un fichier de la bibliothèque, s'il y est encore.
    pub fn library_file(&self, file: &str) -> Option<PathBuf> {
        self.look
            .assets
            .iter()
            .any(|a| a.file == file)
            .then(|| self.library.join(file))
            .filter(|p| p.is_file())
    }
}

/// La session lue sur disque.
#[derive(Debug, Clone)]
pub struct SessionFiles {
    pub dir: PathBuf,
    pub project: Project,
    pub transcript: Transcript,
    pub parts: Vec<(f64, f64)>,
    pub look: Look,
    pub finish: Finish,
    pub rush: PathBuf,
    pub studio: PathBuf,
    pub library: PathBuf,
}

/// `montage.json` : la transcription du montage et ses morceaux.
#[derive(Deserialize)]
struct Montage {
    key: String,
    #[serde(default)]
    parts: Option<Vec<(f64, f64)>>,
    transcript: Transcript,
}

#[derive(Deserialize)]
struct SessionData {
    rush: Option<PathBuf>,
}

/// Morceaux d'un montage d'après son empreinte (`a-b,c-d`, en secondes).
pub fn parts_from_key(key: &str) -> Option<Vec<(f64, f64)>> {
    key.split(',')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (a, b) = p.split_once('-')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        })
        .collect()
}

impl SessionFiles {
    pub fn load(dir: &Path) -> Result<Self> {
        let json = std::fs::read_to_string(dir.join("project.json")).with_context(|| {
            format!(
                "pas de storyboard dans {} (ouvrir la prise dans l'interface, écran Storyboard)",
                dir.display()
            )
        })?;
        let project = Project::from_json(&json).context("project.json")?;
        let montage: Montage = serde_json::from_slice(
            &std::fs::read(dir.join("montage.json")).context("montage.json manquant")?,
        )
        .context("montage.json")?;
        let parts = match montage.parts {
            Some(p) => p,
            None => parts_from_key(&montage.key).context("montage.json : morceaux illisibles")?,
        };
        let read = |name: &str| std::fs::read(dir.join(name)).ok();
        let look: Look = read("look.json")
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or_default();
        let finish: Finish = read("finish.json")
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or_default();
        let rush = read("session.json")
            .and_then(|d| serde_json::from_slice::<SessionData>(&d).ok())
            .and_then(|s| s.rush)
            .context("aucune prise dans la session")?;
        anyhow::ensure!(!parts.is_empty(), "montage vide");
        Ok(Self {
            dir: dir.to_path_buf(),
            project,
            transcript: montage.transcript,
            parts,
            look,
            finish,
            rush,
            studio: dir.join("studio"),
            library: dir.join("library"),
        })
    }

    pub fn sources(&self) -> Sources<'_> {
        Sources {
            studio: &self.studio,
            library: &self.library,
            project: &self.project,
            transcript: &self.transcript,
            look: &self.look,
            finish: &self.finish,
            rush: &self.rush,
            parts: &self.parts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_of_the_rush_follow_the_cut() {
        // Montage : [2 s ; 4 s) puis [10 s ; 11 s).
        let parts = [(2.0, 4.0), (10.0, 11.0)];
        assert_eq!(source_pieces(&parts, 0.5, 1.5), [(2.5, 3.5)]);
        let across = source_pieces(&parts, 1.5, 2.5);
        assert_eq!(across.len(), 2);
        assert!((across[0].0 - 3.5).abs() < 1e-9 && (across[0].1 - 4.0).abs() < 1e-9);
        assert!((across[1].0 - 10.0).abs() < 1e-9 && (across[1].1 - 10.5).abs() < 1e-9);
    }

    #[test]
    fn keys_give_parts() {
        assert_eq!(
            parts_from_key("1.250-3.000,10.000-12.500"),
            Some(vec![(1.25, 3.0), (10.0, 12.5)])
        );
        assert_eq!(parts_from_key("x"), None);
    }

    #[test]
    fn names_follow_the_format() {
        let id = BlockId::new();
        let wide = ProjectSettings::default();
        let tall = wide.with_aspect(AspectRatio::Portrait9x16);
        assert_eq!(block_name(id, &wide), format!("bloc-{id}"));
        assert_eq!(block_name(id, &tall), format!("bloc-{id}-9x16"));
        assert_ne!(
            clip_path(Path::new("s"), id, &wide),
            clip_path(Path::new("s"), id, &tall)
        );
    }
}
