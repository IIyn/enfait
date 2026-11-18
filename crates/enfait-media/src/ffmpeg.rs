//! Lancement de ffmpeg avec suivi de progression.

use crate::MediaError;
use std::ffi::OsStr;
use std::io::{BufRead as _, BufReader, Read as _};
use std::process::Stdio;

/// Lance `ffmpeg` avec `args` (sans les options communes) et rapporte la
/// progression entre 0 et 1, d'après la durée attendue de la sortie.
pub(crate) fn run<I, S>(
    args: I,
    duration: f64,
    on_progress: &mut dyn FnMut(f32),
) -> Result<(), MediaError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut child = enfait_config::process::command("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-v", "error", "-y"])
        .args(["-progress", "pipe:1", "-nostats"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| MediaError::spawn("ffmpeg", &e))?;
    // stderr est lu à part pour que ffmpeg ne bloque jamais sur un tube plein.
    let stderr = child.stderr.take();
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut s) = stderr {
            let _ = s.read_to_string(&mut text);
        }
        text
    });
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if let Some(progress) = parse_progress(&line, duration) {
                on_progress(progress);
            }
        }
    }
    let status = child.wait()?;
    let errors = errors.join().unwrap_or_default();
    if status.success() {
        on_progress(1.0);
        Ok(())
    } else {
        Err(MediaError::Tool {
            tool: "ffmpeg",
            message: errors.trim().to_owned(),
        })
    }
}

/// Lit une ligne `out_time_us=…` de `-progress`.
fn parse_progress(line: &str, duration: f64) -> Option<f32> {
    let micros: f64 = line.strip_prefix("out_time_us=")?.trim().parse().ok()?;
    if duration <= 0.0 {
        return None;
    }
    // Rapport borné entre 0 et 1 : la conversion en f32 ne perd rien d'utile.
    #[allow(clippy::cast_possible_truncation)]
    Some((micros / 1e6 / duration).clamp(0.0, 1.0) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_lines() {
        assert_eq!(parse_progress("out_time_us=5000000", 10.0), Some(0.5));
        assert_eq!(parse_progress("out_time_us=N/A", 10.0), None);
        assert_eq!(parse_progress("frame=12", 10.0), None);
        assert_eq!(parse_progress("out_time_us=20000000", 10.0), Some(1.0));
    }
}
