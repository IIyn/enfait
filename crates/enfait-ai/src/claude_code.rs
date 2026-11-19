//! Bridge to Claude Code (`claude -p`), which uses the user's account.
//!
//! Each request runs a `claude -p` without a persistent session, with our
//! system instruction instead of Claude Code's and the `Read` tool only
//! (to open the images to critique).

use crate::llm::{AiError, Part, Reply, Request, Usage};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::AsyncWriteExt as _;

pub struct ClaudeCodeClient {
    program: PathBuf,
    /// Working folder of the subprocesses (instructions, images).
    work_dir: PathBuf,
}

#[derive(Deserialize)]
struct CliResult {
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    result: String,
    #[serde(default)]
    total_cost_usd: f64,
    #[serde(default)]
    usage: CliUsage,
}

#[derive(Deserialize, Default)]
struct CliUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
}

impl ClaudeCodeClient {
    pub fn new(program: impl Into<PathBuf>, work_dir: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            work_dir: work_dir.into(),
        }
    }

    pub async fn complete(&self, request: &Request) -> Result<Reply, AiError> {
        let prompts = self.work_dir.join("prompts");
        tokio::fs::create_dir_all(&prompts).await?;
        let hash = blake3::hash(request.system.as_bytes()).to_hex();
        let system_file = prompts.join(format!("system-{}.md", hash.get(..16).unwrap_or("x")));
        if !tokio::fs::try_exists(&system_file).await? {
            tokio::fs::write(&system_file, &request.system).await?;
        }

        let (prompt, image_dirs) = flatten(&request.user);
        let mut command = enfait_config::process::tokio_command(&self.program);
        command
            .current_dir(&self.work_dir)
            .args(["-p", "--output-format", "json", "--no-session-persistence"])
            .args(["--model", &request.model])
            .args(["--effort", request.effort.as_str()])
            .arg("--system-prompt-file")
            .arg(&system_file)
            .args(["--tools", "Read", "--allowedTools", "Read"])
            .arg("--strict-mcp-config");
        for dir in &image_dirs {
            command.arg("--add-dir").arg(dir);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command.spawn().map_err(|e| {
            AiError::ClaudeCode(format!("lancement de {} : {e}", self.program.display()))
        })?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(prompt.as_bytes()).await?;
            stdin.shutdown().await?;
        }
        let output = child.wait_with_output().await?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed: CliResult = serde_json::from_str(stdout.trim()).map_err(|_| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            AiError::ClaudeCode(format!(
                "sortie illisible (code {:?}) : {} {}",
                output.status.code(),
                stdout.chars().take(400).collect::<String>(),
                stderr.chars().take(400).collect::<String>()
            ))
        })?;
        if parsed.is_error {
            return Err(AiError::ClaudeCode(parsed.result));
        }
        Ok(Reply {
            text: parsed.result,
            model: request.model.clone(),
            usage: Usage {
                input_tokens: parsed.usage.input_tokens,
                output_tokens: parsed.usage.output_tokens,
                cache_read_input_tokens: parsed.usage.cache_read_input_tokens,
                cache_creation_input_tokens: parsed.usage.cache_creation_input_tokens,
                cost_usd: parsed.total_cost_usd,
            },
        })
    }
}

/// Flattens the message: images become paths to open with the `Read`
/// tool. Also returns the folders to allow.
fn flatten(parts: &[Part]) -> (String, Vec<PathBuf>) {
    let mut prompt = String::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for part in parts {
        match part {
            Part::Text(text) => {
                prompt.push_str(text);
                prompt.push_str("\n\n");
            }
            Part::Image(path) => {
                let path = std::path::absolute(path).unwrap_or_else(|_| path.clone());
                prompt.push_str("[Image to open with the Read tool: ");
                prompt.push_str(&path.to_string_lossy());
                prompt.push_str("]\n\n");
                if let Some(dir) = path.parent().map(Path::to_path_buf)
                    && !dirs.contains(&dir)
                {
                    dirs.push(dir);
                }
            }
        }
    }
    (prompt, dirs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_lists_images_and_dirs() {
        let (prompt, dirs) = flatten(&[
            Part::Text("Regarde :".into()),
            Part::Image("/tmp/a/x.jpg".into()),
            Part::Image("/tmp/a/y.jpg".into()),
        ]);
        assert!(prompt.contains("/tmp/a/x.jpg"));
        assert_eq!(dirs, vec![PathBuf::from("/tmp/a")]);
    }
}
