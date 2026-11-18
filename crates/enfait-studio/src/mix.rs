//! Mixage de la finition : la voix du montage, les musiques (avec fondus et
//! ducking sous la voix) et les effets sonores, en un seul passage ffmpeg.
//! L'image n'est pas réencodée : seul le son change, ce qui rend chaque
//! réglage rapide à entendre.

use anyhow::{Context as _, Result};
use enfait_core::Ducking;
use enfait_core::finish::{PlacedMusic, PlacedSfx};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Ce qu'il faut mixer, en secondes du montage, avec les fichiers sur disque.
#[derive(Debug, Clone, Default)]
pub struct MixPlan {
    pub music: Vec<(PlacedMusic, PathBuf)>,
    pub sfx: Vec<(PlacedSfx, PathBuf)>,
    pub ducking: Ducking,
    /// Passages où la musique baisse (voir [`Ducking::spans`]).
    pub duck_spans: Vec<(f64, f64)>,
    pub voice_gain_db: f32,
}

impl MixPlan {
    pub fn is_empty(&self) -> bool {
        self.music.is_empty() && self.sfx.is_empty() && self.voice_gain_db.abs() < 0.01
    }
}

/// Mise en stéréo 48 kHz d'une entrée de `channels` voies : un son mono
/// est copié sur les deux voies à son niveau (la conversion par défaut de
/// ffmpeg le baisserait de 3 dB).
fn stereo(channels: u16) -> &'static str {
    if channels == 1 {
        "aresample=48000,pan=stereo|c0=c0|c1=c0,aformat=sample_fmts=fltp"
    } else {
        "aresample=48000,aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo"
    }
}

/// Graphe de filtres audio : l'entrée 0 porte la voix ; les musiques sont
/// les entrées 1…, puis les SFX. `channels` : nombre de voies de chaque
/// entrée, dans cet ordre (2 si inconnu). Sortie : `[a]`.
pub fn graph(plan: &MixPlan, channels: &[u16]) -> String {
    let format = |i: usize| stereo(channels.get(i).copied().unwrap_or(2));
    let mut g = format!(
        "[0:a]{},volume={:.2}dB[voice];",
        format(0),
        plan.voice_gain_db
    );
    let mut index = 1_usize;
    let mut music = Vec::new();
    for (i, (m, _)) in plan.music.iter().enumerate() {
        let len = (m.end - m.start).max(0.01);
        // Fondus bornés par la durée du morceau placé.
        let fade_in = m.fade_in.min(len / 2.0);
        let fade_out = m.fade_out.min(len / 2.0);
        let _ = write!(
            g,
            "[{index}:a]{},atrim=start={:.3}:duration={len:.3},asetpts=PTS-STARTPTS,\
             volume={:.2}dB",
            format(index),
            m.offset,
            m.gain_db
        );
        if fade_in > 0.0 {
            let _ = write!(g, ",afade=t=in:st=0:d={fade_in:.3}");
        }
        if fade_out > 0.0 {
            let _ = write!(g, ",afade=t=out:st={:.3}:d={fade_out:.3}", len - fade_out);
        }
        let _ = write!(g, ",adelay={}:all=1[m{i}];", millis(m.start));
        music.push(format!("[m{i}]"));
        index = index.saturating_add(1);
    }
    let mut layers = vec!["[voice]".to_owned()];
    if !music.is_empty() {
        let bus = if music.len() == 1 {
            music.concat()
        } else {
            let _ = write!(
                g,
                "{}amix=inputs={}:normalize=0:duration=longest[mus];",
                music.concat(),
                music.len()
            );
            "[mus]".to_owned()
        };
        let _ = write!(
            g,
            "{bus}volume='{}':eval=frame[ducked];",
            plan.ducking.expression(&plan.duck_spans)
        );
        layers.push("[ducked]".to_owned());
    }
    for (i, (s, _)) in plan.sfx.iter().enumerate() {
        let _ = write!(
            g,
            "[{index}:a]{},volume={:.2}dB,adelay={}:all=1[s{i}];",
            format(index),
            s.gain_db,
            millis(s.at)
        );
        layers.push(format!("[s{i}]"));
        index = index.saturating_add(1);
    }
    if layers.len() == 1 {
        g.push_str("[voice]alimiter=limit=0.95:level=0[a]");
    } else {
        // La voix d'abord : elle donne la durée du mixage.
        let _ = write!(
            g,
            "{}amix=inputs={}:normalize=0:duration=first:dropout_transition=0,\
             alimiter=limit=0.95:level=0[a]",
            layers.concat(),
            layers.len()
        );
    }
    g
}

fn millis(t: f64) -> u64 {
    // Instant positif et raisonnable : la conversion ne déborde pas.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    ms
}

/// Plan de mixage de la finition sur le montage de `sources` ; sans les
/// SFX si `sfx` est faux. Les sons retirés de la bibliothèque sont ignorés.
pub fn plan(sources: &crate::session::Sources<'_>, sfx: bool) -> MixPlan {
    let placed = sources.finish.resolve(sources.project);
    MixPlan {
        music: placed
            .music
            .into_iter()
            .filter_map(|m| sources.library_file(&m.file).map(|p| (m, p)))
            .collect(),
        sfx: if sfx {
            placed
                .sfx
                .into_iter()
                .filter_map(|s| sources.library_file(&s.file).map(|p| (s, p)))
                .collect()
        } else {
            Vec::new()
        },
        ducking: sources.finish.ducking,
        duck_spans: sources.finish.ducking.spans(sources.transcript),
        voice_gain_db: sources.finish.voice_gain_db,
    }
}

/// Ajoute à `cmd` les entrées du mixage (`base` porte la voix) et son
/// graphe ; la sortie du graphe est `[a]`.
fn mix_inputs(cmd: &mut tokio::process::Command, base: &Path, plan: &MixPlan) {
    let inputs: Vec<&Path> = std::iter::once(base)
        .chain(plan.music.iter().map(|(_, p)| p.as_path()))
        .chain(plan.sfx.iter().map(|(_, p)| p.as_path()))
        .collect();
    let channels: Vec<u16> = inputs
        .iter()
        .map(|p| {
            enfait_media::probe::probe(p)
                .ok()
                .and_then(|i| i.audio)
                .map_or(2, |a| a.channels)
        })
        .collect();
    cmd.arg("-i").arg(base);
    for (_, path) in &plan.music {
        // Un morceau plus court que les blocs couverts reprend au début.
        cmd.args(["-stream_loop", "-1"]).arg("-i").arg(path);
    }
    for (_, path) in &plan.sfx {
        cmd.arg("-i").arg(path);
    }
    cmd.args(["-filter_complex", &graph(plan, &channels)]);
}

/// Remplace le son de `base` (vidéo + voix) par le mixage, dans `out`.
/// Sans rien à mixer, `out` est une copie de `base`.
pub async fn apply(base: &Path, plan: &MixPlan, out: &Path) -> Result<()> {
    let tmp = out.with_extension("mix.mp4");
    let mut cmd = enfait_config::process::tokio_command("ffmpeg");
    cmd.args(["-nostdin", "-v", "error", "-y"]);
    if plan.is_empty() {
        cmd.arg("-i").arg(base).args(["-map", "0", "-c", "copy"]);
    } else {
        mix_inputs(&mut cmd, base, plan);
        cmd.args(["-map", "0:v?", "-map", "[a]", "-c:v", "copy"])
            .args(["-c:a", "aac", "-b:a", "192k"]);
    }
    let output = cmd
        .args(["-movflags", "+faststart"])
        .arg(&tmp)
        .output()
        .await
        .context("lancement de ffmpeg")?;
    anyhow::ensure!(
        output.status.success(),
        "mixage : {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    tokio::fs::rename(&tmp, out).await?;
    Ok(())
}

/// Mixe la voix `voice` (fichier son) avec le plan, en WAV stéréo 48 kHz
/// (flottant 32 bits, sans perte avant la normalisation).
pub async fn render(voice: &Path, plan: &MixPlan, out: &Path) -> Result<()> {
    let mut cmd = enfait_config::process::tokio_command("ffmpeg");
    cmd.args(["-nostdin", "-v", "error", "-y"]);
    mix_inputs(&mut cmd, voice, plan);
    let output = cmd
        .args(["-map", "[a]", "-c:a", "pcm_f32le", "-ar", "48000"])
        .arg(out)
        .output()
        .await
        .context("lancement de ffmpeg")?;
    anyhow::ensure!(
        output.status.success(),
        "mixage : {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn music(start: f64, end: f64) -> PlacedMusic {
        PlacedMusic {
            id: 1,
            file: "m.mp3".into(),
            start,
            end,
            offset: 2.0,
            gain_db: -14.0,
            fade_in: 1.0,
            fade_out: 10.0,
        }
    }

    #[test]
    fn graphs() {
        let empty = MixPlan::default();
        assert!(empty.is_empty());
        let g = graph(&empty, &[1]);
        assert!(g.starts_with("[0:a]aresample=48000,pan=stereo|c0=c0|c1=c0"));
        assert!(g.ends_with("[voice]alimiter=limit=0.95:level=0[a]"));
        let plan = MixPlan {
            music: vec![(music(4.0, 12.0), "m.mp3".into())],
            sfx: vec![(
                PlacedSfx {
                    id: 2,
                    file: "s.wav".into(),
                    at: 3.8,
                    gain_db: -3.0,
                },
                "s.wav".into(),
            )],
            ducking: Ducking::default(),
            duck_spans: vec![(5.0, 6.0)],
            voice_gain_db: 0.0,
        };
        let g = graph(&plan, &[2, 2, 1]);
        assert!(g.contains("atrim=start=2.000:duration=8.000"));
        // Fondu de sortie borné à la moitié du morceau.
        assert!(g.contains("afade=t=out:st=4.000:d=4.000"));
        assert!(g.contains("adelay=4000:all=1[m0]"));
        assert!(g.contains("[m0]volume='1-"));
        assert!(g.contains("[2:a]") && g.contains("adelay=3800:all=1[s0]"));
        assert!(g.contains("[voice][ducked][s0]amix=inputs=3"));
    }
}
