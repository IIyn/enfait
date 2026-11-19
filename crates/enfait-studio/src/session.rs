//! The interface session seen by the studio: where the clips of each block
//! are stored, what each block shows, and reading the session from disk
//! (for the export from the CLI or the MCP server).
//!
//! The files of a block (composition, frames, clip) carry the project
//! format in their name (nothing for the 16:9 landscape format, which
//! keeps the original names): changing format does not mix renders, and
//! going back to the previous format finds its animations again.

use anyhow::{Context as _, Result};
use enfait_core::{
    AspectRatio, Block, BlockId, BlockKind, Finish, Look, Project, ProjectSettings, Transcript,
    Transition,
};
use enfait_media::Edl;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Share of the transition placed before the cut.
pub const TRANSITION_BEFORE: f64 = 0.4;

fn suffix(settings: &ProjectSettings) -> &'static str {
    match settings.aspect {
        AspectRatio::Landscape16x9 => "",
        AspectRatio::Portrait9x16 => "-9x16",
        AspectRatio::Square1x1 => "-1x1",
    }
}

/// Name of the files of a block in the studio (`bloc-` kept from the
/// first versions: existing renders stay attached).
pub fn block_name(id: BlockId, settings: &ProjectSettings) -> String {
    format!("bloc-{id}{}", suffix(settings))
}

/// Name of the files of the transition after a block.
pub fn transition_name(id: BlockId, settings: &ProjectSettings) -> String {
    format!("transition-{id}{}", suffix(settings))
}

/// Rendered clip of a block.
pub fn clip_path(dir: &Path, id: BlockId, settings: &ProjectSettings) -> PathBuf {
    dir.join("clips")
        .join(format!("{}.mp4", block_name(id, settings)))
}

/// Clip of the transition after a block.
pub fn transition_clip_path(dir: &Path, id: BlockId, settings: &ProjectSettings) -> PathBuf {
    dir.join("clips")
        .join(format!("{}.mp4", transition_name(id, settings)))
}

/// Clip of the b-roll shot of a block.
pub fn broll_clip_path(dir: &Path, id: BlockId, settings: &ProjectSettings) -> PathBuf {
    dir.join("clips")
        .join(format!("broll-{id}{}.mp4", suffix(settings)))
}

/// What the b-roll clip of a block depends on (file, start, duration,
/// format): written next to the clip, it tells whether the clip is current.
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

/// The b-roll clip of the block exists and matches its current shot.
pub fn broll_ready(dir: &Path, block: &Block, settings: &ProjectSettings) -> bool {
    let Some(key) = broll_key(block, settings) else {
        return false;
    };
    let clip = broll_clip_path(dir, block.id, settings);
    clip.is_file() && std::fs::read_to_string(clip.with_extension("key")).is_ok_and(|k| k == key)
}

/// Frames shown during a block, if not the take: its b-roll shot, or its
/// animation.
pub fn visual_clip(dir: &Path, block: &Block, settings: &ProjectSettings) -> Option<PathBuf> {
    match block.kind {
        BlockKind::Rush => None,
        BlockKind::Broll => {
            broll_ready(dir, block, settings).then(|| broll_clip_path(dir, block.id, settings))
        }
        _ => Some(clip_path(dir, block.id, settings)).filter(|c| c.is_file()),
    }
}

/// Window (edit time) of the transition between `block` and `next`.
pub fn transition_window(block: &Block, next: &Block) -> (f64, f64) {
    let boundary = block.range.end().as_secs_f64();
    let length = crate::TRANSITION_LENGTH;
    (
        (boundary - length * TRANSITION_BEFORE).max(block.range.start().as_secs_f64()),
        (boundary + length * (1.0 - TRANSITION_BEFORE)).min(next.range.end().as_secs_f64()),
    )
}

/// Pieces of the take (start, end) giving the interval `[a, b)` of the
/// edit.
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

/// What is shown, block by block, in edit time.
#[derive(Debug, Clone, Default)]
pub struct Visuals {
    /// `(start, end, clip)` of each block; without a clip, the take.
    pub blocks: Vec<(f64, f64, Option<PathBuf>)>,
    /// `(start, end, clip)` of the rendered transitions, on top.
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

    /// No clip: only the take is shown.
    pub fn rush_only(&self) -> bool {
        self.blocks.iter().all(|b| b.2.is_none()) && self.transitions.is_empty()
    }

    /// Blocks that should show something other than the take but have no
    /// clip yet (rank from 1).
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

/// Everything needed to edit the final video.
#[derive(Debug, Clone)]
pub struct Sources<'a> {
    /// Studio: clips of the blocks and transitions.
    pub studio: &'a Path,
    pub library: &'a Path,
    pub project: &'a Project,
    /// Transcript of the edit.
    pub transcript: &'a Transcript,
    pub look: &'a Look,
    pub finish: &'a Finish,
    /// The original take.
    pub rush: &'a Path,
    /// Pieces of the take forming the edit.
    pub parts: &'a [(f64, f64)],
}

impl Sources<'_> {
    /// Path of a library file, if it is still there.
    pub fn library_file(&self, file: &str) -> Option<PathBuf> {
        self.look
            .assets
            .iter()
            .any(|a| a.file == file)
            .then(|| self.library.join(file))
            .filter(|p| p.is_file())
    }
}

/// The session read from disk.
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

/// `montage.json`: the transcript of the edit and its pieces.
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

/// Pieces of an edit from its fingerprint (`a-b,c-d`, in seconds).
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
                "no storyboard in {} (open the take in the app, Storyboard screen)",
                dir.display()
            )
        })?;
        let project = Project::from_json(&json).context("project.json")?;
        let montage: Montage = serde_json::from_slice(
            &std::fs::read(dir.join("montage.json")).context("montage.json missing")?,
        )
        .context("montage.json")?;
        let parts = match montage.parts {
            Some(p) => p,
            None => parts_from_key(&montage.key).context("montage.json: unreadable pieces")?,
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
            .context("no take in the session")?;
        anyhow::ensure!(!parts.is_empty(), "empty edit");
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
        // Edit: [2 s; 4 s) then [10 s; 11 s).
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
