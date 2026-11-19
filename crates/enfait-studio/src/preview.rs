//! Storyboard preview: the edited take, where each animated block is
//! replaced by its clip, with the sound of the edit. A single file, which
//! the player opens like any media.

use anyhow::{Context as _, Result};
use enfait_media::Edl;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// A piece of the preview, in edit time.
pub struct Piece {
    pub start: f64,
    pub end: f64,
    pub source: Source,
}

/// Where the frames of a piece come from.
pub enum Source {
    /// The edited take.
    Rush,
    /// A rendered clip, played from `offset` seconds.
    Clip { path: PathBuf, offset: f64 },
}

/// Pieces of the preview: each block (its clip if it has one, else the
/// take), then the transition windows `(start, end, clip)` on top.
pub fn pieces(
    blocks: &[(f64, f64, Option<PathBuf>)],
    transitions: &[(f64, f64, PathBuf)],
) -> Vec<Piece> {
    let mut out = Vec::new();
    for (start, end, clip) in blocks {
        // Parts of the block outside the transition windows.
        let mut cursor = *start;
        let mut cuts: Vec<(f64, f64)> = transitions
            .iter()
            .filter(|(a, b, _)| *b > *start && *a < *end)
            .map(|(a, b, _)| (a.max(*start), b.min(*end)))
            .collect();
        cuts.sort_by(|a, b| a.0.total_cmp(&b.0));
        cuts.push((*end, *end));
        for (a, b) in cuts {
            if a - cursor > 1e-3 {
                out.push(Piece {
                    start: cursor,
                    end: a,
                    source: match clip {
                        Some(path) => Source::Clip {
                            path: path.clone(),
                            offset: cursor - start,
                        },
                        None => Source::Rush,
                    },
                });
            }
            cursor = cursor.max(b);
        }
    }
    for (a, b, path) in transitions {
        out.push(Piece {
            start: *a,
            end: *b,
            source: Source::Clip {
                path: path.clone(),
                offset: 0.0,
            },
        });
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// Size of the preview for a `width`×`height` image: 540 lines (or
/// columns in portrait), even.
pub fn preview_size(width: u32, height: u32) -> (u32, u32) {
    let even = |x: f64| {
        // Reasonable size: the conversion does not overflow.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let v = (x / 2.0).round().max(1.0) as u32 * 2;
        v
    };
    let (w, h) = (f64::from(width.max(1)), f64::from(height.max(1)));
    if w >= h {
        (even(540.0 * w / h), 540)
    } else {
        (540, even(540.0 * h / w))
    }
}

/// Quality of the assembly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    /// Fast preview, with the voice of the edit.
    Preview,
    /// Export: picture only, in high quality (the sound is mixed separately).
    Final,
}

/// Output format of the assembly.
pub struct Output<'a> {
    pub size: (u32, u32),
    pub fps: f64,
    /// Framing of the take (see [`enfait_media::fx::framing`]).
    pub framing: String,
    pub quality: Quality,
    /// Post-FX applied to the whole picture (see [`enfait_media::fx::filter`]).
    pub post_fx: Option<String>,
    /// Progress, from 0 to 1.
    pub progress: Option<&'a (dyn Fn(f64) + Sync)>,
}

impl Output<'_> {
    /// Fast preview at `size`, whole take (black bars if needed).
    pub fn preview(size: (u32, u32), fps: f64) -> Self {
        Self {
            size,
            fps,
            framing: enfait_media::fx::framing(size.0, size.1, enfait_core::Framing::Fit, 0),
            quality: Quality::Preview,
            post_fx: None,
            progress: None,
        }
    }
}

/// Sound of the voice of the edit: the pieces of input 0 end to end, with
/// 4 ms fades at the cuts. Output: `[a]`.
pub fn voice_graph(parts: &[(f64, f64)]) -> String {
    let edl = Edl::from_ranges(parts);
    let mut graph = String::new();
    let mut audio = String::new();
    for (i, part) in edl.parts().iter().enumerate() {
        let _ = write!(
            graph,
            "[0:a]atrim=start={:.4}:end={:.4},asetpts=PTS-STARTPTS,\
             afade=t=in:d=0.004,afade=t=out:st={:.4}:d=0.004[a{i}];",
            part.source_start,
            part.source_end,
            (part.duration() - 0.004).max(0.0)
        );
        let _ = write!(audio, "[a{i}]");
    }
    let _ = write!(graph, "{audio}concat=n={}:v=0:a=1[a]", edl.parts().len());
    graph
}

/// Assembles the preview (or the picture of the export). `rush`: the
/// take; `video`: it has pictures (else black background); `parts`: pieces
/// of the take forming the edit; `pieces`: see [`pieces`].
pub async fn assemble(
    rush: &Path,
    video: bool,
    parts: &[(f64, f64)],
    pieces: &[Piece],
    output: &Output<'_>,
    out: &Path,
) -> Result<()> {
    anyhow::ensure!(!parts.is_empty() && !pieces.is_empty(), "empty edit");
    let edl = Edl::from_ranges(parts);
    let (w, h) = output.size;
    let fps = output.fps;
    // Clips are already in the format: framed without cropping.
    let clip_fit = format!(
        "fps={fps},{}",
        enfait_media::fx::framing(w, h, enfait_core::Framing::Fit, 0)
    );
    let rush_fit = format!("fps={fps},{}", output.framing);
    let mut graph = String::new();
    let mut labels = String::new();
    let mut inputs: Vec<PathBuf> = vec![rush.to_path_buf()];
    let mut n = 0_usize;
    for piece in pieces {
        let duration = (piece.end - piece.start).max(0.0);
        if duration <= 1e-3 {
            continue;
        }
        if let Source::Clip { path, offset } = &piece.source {
            inputs.push(path.clone());
            let k = inputs.len().saturating_sub(1);
            let _ = write!(
                graph,
                // A slightly short clip keeps its last frame: the preview stays in
                // sync with the sound.
                "[{k}:v]{clip_fit},tpad=stop_mode=clone:stop_duration=2,\
                 trim=start={offset:.4}:duration={duration:.4},setpts=PTS-STARTPTS[v{n}];"
            );
            let _ = write!(labels, "[v{n}]");
            n = n.saturating_add(1);
            continue;
        }
        for part in edl.parts() {
            let a = piece.start.max(part.start);
            let b = piece.end.min(part.end());
            if b - a < 1e-3 {
                continue;
            }
            let (sa, sb) = (
                part.source_start + (a - part.start),
                part.source_start + (b - part.start),
            );
            if video {
                let _ = write!(
                    graph,
                    "[0:v]trim=start={sa:.4}:end={sb:.4},setpts=PTS-STARTPTS,{rush_fit}[v{n}];"
                );
            } else {
                let _ = write!(
                    graph,
                    "color=c=black:s={w}x{h}:r={fps}:d={:.4},setsar=1[v{n}];",
                    sb - sa
                );
            }
            let _ = write!(labels, "[v{n}]");
            n = n.saturating_add(1);
        }
    }
    let _ = write!(graph, "{labels}concat=n={n}:v=1:a=0");
    if let Some(fx) = &output.post_fx {
        let _ = write!(graph, ",{fx}");
    }
    graph.push_str("[v]");
    let with_voice = output.quality == Quality::Preview;
    if with_voice {
        let _ = write!(graph, ";{}", voice_graph(parts));
    }

    if let Some(dir) = out.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let tmp = out.with_extension("tmp.mp4");
    let mut cmd = enfait_config::process::tokio_command("ffmpeg");
    cmd.args(["-nostdin", "-v", "error", "-y"]);
    for input in &inputs {
        cmd.arg("-i").arg(input);
    }
    cmd.args(["-filter_complex", &graph]).args(["-map", "[v]"]);
    match output.quality {
        Quality::Preview => {
            cmd.args(["-map", "[a]"])
                .args([
                    "-c:v", "libx264", "-preset", "veryfast", "-crf", "23", "-g", "15",
                ])
                .args(["-c:a", "aac", "-b:a", "160k"]);
        }
        Quality::Final => {
            cmd.args(["-an", "-c:v", "libx264", "-preset", "slow", "-crf", "17"]);
        }
    }
    cmd.args(["-pix_fmt", "yuv420p", "-r", &format!("{fps}")])
        .args(["-movflags", "+faststart"])
        .arg(&tmp);
    let total: f64 = edl.parts().iter().map(|p| p.duration()).sum();
    crate::ffmpeg::run(cmd, total, output.progress)
        .await
        .context("assembling")?;
    tokio::fs::rename(&tmp, out).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transition_windows_cut_into_blocks() {
        let clip = PathBuf::from("b.mp4");
        let t = PathBuf::from("t.mp4");
        let p = pieces(
            &[(0.0, 2.0, None), (2.0, 4.0, Some(clip.clone()))],
            &[(1.6, 2.6, t.clone())],
        );
        let spans: Vec<(f64, f64)> = p.iter().map(|p| (p.start, p.end)).collect();
        assert_eq!(spans, [(0.0, 1.6), (1.6, 2.6), (2.6, 4.0)]);
        assert!(matches!(&p[1].source, Source::Clip { path, .. } if *path == t));
        // The clip of block 2 resumes 0.6 s after its start.
        assert!(matches!(&p[2].source, Source::Clip { offset, .. } if (offset - 0.6).abs() < 1e-9));
    }

    #[test]
    fn sizes() {
        assert_eq!(preview_size(1920, 1080), (960, 540));
        assert_eq!(preview_size(1080, 1920), (540, 960));
        assert_eq!(preview_size(1080, 1080), (540, 540));
    }
}
