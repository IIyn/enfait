//! `enfait-cli cut` : découpe d'une prise selon le script (règle « dernière
//! prise »), comme l'écran Découpe de l'interface. Sans script, les phrases
//! et leurs reprises sont déduites de la transcription.

use crate::paths::Dirs;
use anyhow::{Context as _, Result};
use enfait_core::Transcript;
use enfait_speech::takes;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[derive(Debug, clap::Args)]
pub struct CutArgs {
    /// La prise (vidéo ou son).
    pub rush: PathBuf,
    /// Le script (texte ou Markdown). Sans script, les reprises sont
    /// déduites de la transcription.
    #[arg(long)]
    pub script: Option<PathBuf>,
    /// Langue de la prise (`fr`, `en`…), détectée sinon.
    #[arg(long)]
    pub language: Option<String>,
    /// Exporte le montage dans ce fichier (réencodé).
    #[arg(long)]
    pub export: Option<PathBuf>,
}

pub async fn run(dirs: &Dirs, args: CutArgs) -> Result<()> {
    let script = match &args.script {
        Some(path) => Some(
            tokio::fs::read_to_string(path)
                .await
                .with_context(|| format!("lecture de {}", path.display()))?,
        ),
        None => None,
    };
    let rush = std::path::absolute(&args.rush)?;
    let transcript = transcript(dirs, &rush, args.language.as_deref()).await?;
    let analysis = match &script {
        Some(script) => takes::analyze(script, &transcript),
        None => enfait_speech::cleanup::analyze(&transcript),
    };
    let choice = analysis.default_choice();
    let words = &transcript.words;
    let at = |i: usize| words.get(i).map_or(0.0, |w| w.range.start().as_secs_f64());
    let mut report = String::new();
    for (s, sentence) in analysis.sentences.iter().enumerate() {
        let _ = writeln!(report, "{:>3}. {}", s.saturating_add(1), sentence.text);
        let takes: Vec<_> = analysis.takes_of(s).collect();
        if takes.is_empty() {
            let _ = writeln!(report, "       (non trouvée)");
        }
        for (n, (i, take)) in takes.iter().enumerate() {
            let text: Vec<&str> = words
                .get(take.words.clone())
                .unwrap_or_default()
                .iter()
                .map(|w| w.text.as_str())
                .collect();
            let _ = writeln!(
                report,
                "     {} P{} {:>7.2} s  {:>3.0} %{}  {}",
                if choice.get(s).copied().flatten() == Some(*i) {
                    "●"
                } else {
                    " "
                },
                n.saturating_add(1),
                at(take.words.start),
                take.coverage * 100.0,
                if take.complete { "" } else { " inc." },
                text.join(" ")
            );
        }
    }
    let parts = takes::cut(&analysis, &transcript, &choice)?;
    let kept: f64 = parts.iter().map(|r| r.duration().as_secs_f64()).sum();
    let _ = write!(
        report,
        "\n{} morceaux, montage {kept:.1} s sur {:.1} s",
        parts.len(),
        transcript.media_duration.as_secs_f64()
    );
    for line in report.lines() {
        tracing::info!("{line}");
    }
    if let Some(out) = args.export {
        let ranges: Vec<(f64, f64)> = parts
            .iter()
            .map(|r| (r.start().as_secs_f64(), r.end().as_secs_f64()))
            .collect();
        export(&rush, &ranges, &out).await?;
        tracing::info!("montage exporté : {}", out.display());
    }
    Ok(())
}

/// Transcription en cache à côté du proxy (partagée avec l'interface).
async fn transcript(dirs: &Dirs, rush: &Path, language: Option<&str>) -> Result<Transcript> {
    let dir = enfait_media::MediaCache::new(dirs.media()).dir_for(rush)?;
    let cached = dir.join("transcript.json");
    if let Ok(data) = tokio::fs::read(&cached).await {
        tracing::info!("transcription reprise du cache");
        return Ok(serde_json::from_slice(&data)?);
    }
    let model = dirs.whisper_model();
    anyhow::ensure!(
        model.is_file(),
        "modèle whisper absent : lancer `enfait-cli setup`"
    );
    tracing::info!("transcription…");
    let rush = rush.to_path_buf();
    let language = language.map(str::to_owned);
    let transcript = tokio::task::spawn_blocking(move || -> Result<Transcript> {
        let pcm = enfait_speech::decode_pcm(&rush)?;
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        let options = enfait_speech::TranscribeOptions {
            language: language.as_deref(),
            // Pas de script en amorce : il pousse whisper à suivre le texte
            // et à fondre les reprises en une seule.
            prompt: None,
            threads: u16::try_from(threads).unwrap_or(4),
            takes: true,
        };
        Ok(enfait_speech::transcribe(&model, &pcm, &options)?)
    })
    .await??;
    tokio::fs::create_dir_all(&dir).await?;
    tokio::fs::write(&cached, serde_json::to_vec_pretty(&transcript)?).await?;
    Ok(transcript)
}

/// Assemble les morceaux avec ffmpeg (filtres trim et concat).
async fn export(rush: &Path, ranges: &[(f64, f64)], out: &Path) -> Result<()> {
    anyhow::ensure!(!ranges.is_empty(), "montage vide");
    let info = enfait_media::probe::probe(rush)?;
    let video = info.video.is_some();
    let audio = info.audio.is_some();
    let mut graph = String::new();
    let mut inputs = String::new();
    for (i, (a, b)) in ranges.iter().enumerate() {
        if video {
            let _ = write!(
                graph,
                "[0:v]trim=start={a:.3}:end={b:.3},setpts=PTS-STARTPTS[v{i}];"
            );
            let _ = write!(inputs, "[v{i}]");
        }
        if audio {
            let _ = write!(
                graph,
                "[0:a]atrim=start={a:.3}:end={b:.3},asetpts=PTS-STARTPTS,\
                 afade=t=in:d=0.004,afade=t=out:st={:.3}:d=0.004[a{i}];",
                (b - a - 0.004).max(0.0)
            );
            let _ = write!(inputs, "[a{i}]");
        }
    }
    let _ = write!(
        graph,
        "{inputs}concat=n={}:v={}:a={}",
        ranges.len(),
        u8::from(video),
        u8::from(audio)
    );
    if video {
        graph.push_str("[v]");
    }
    if audio {
        graph.push_str("[a]");
    }
    let mut cmd = enfait_config::process::tokio_command("ffmpeg");
    cmd.args(["-nostdin", "-v", "error", "-y", "-i"])
        .arg(rush)
        .args(["-filter_complex", &graph]);
    if video {
        cmd.args([
            "-map", "[v]", "-c:v", "libx264", "-crf", "18", "-pix_fmt", "yuv420p",
        ]);
    }
    if audio {
        let wav = out
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("wav"));
        cmd.args(["-map", "[a]"]);
        if wav {
            cmd.args(["-c:a", "pcm_s16le"]);
        } else {
            cmd.args(["-c:a", "aac", "-b:a", "192k"]);
        }
    }
    let output = cmd.arg(out).output().await.context("lancement de ffmpeg")?;
    anyhow::ensure!(
        output.status.success(),
        "ffmpeg : {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}
