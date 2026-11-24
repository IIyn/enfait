//! What the connection guide checks: Claude Code installed and signed in,
//! a local server installed, running, and holding models.
//!
//! Every check is read-only and calls no model: `claude auth status`, a
//! `GET /models` on the local server, and for Ollama a `POST /api/show`
//! per model (to know which ones read images).

use enfait_config::process::{find, find_claude, home, usual_dirs};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

/// A local server program the guide knows how to install.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Server {
    Ollama,
    LmStudio,
    LlamaCpp,
    /// Any other OpenAI-compatible server (vLLM, a remote machine…).
    Other,
}

impl Server {
    pub const ALL: [Self; 4] = [Self::Ollama, Self::LmStudio, Self::LlamaCpp, Self::Other];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Ollama => "Ollama",
            Self::LmStudio => "LM Studio",
            Self::LlamaCpp => "llama.cpp",
            Self::Other => "OpenAI-compatible",
        }
    }

    /// Address of the server when started with its defaults.
    pub const fn default_url(self) -> Option<&'static str> {
        match self {
            Self::Ollama => Some("http://localhost:11434/v1"),
            Self::LmStudio => Some("http://localhost:1234/v1"),
            Self::LlamaCpp => Some("http://localhost:8080/v1"),
            Self::Other => None,
        }
    }

    /// The server an address most likely points to (by its port).
    pub fn from_url(url: &str) -> Self {
        let port = url
            .split("://")
            .nth(1)
            .unwrap_or(url)
            .split('/')
            .next()
            .and_then(|host| host.rsplit_once(':'))
            .map(|(_, port)| port);
        match port {
            Some("11434") => Self::Ollama,
            Some("1234") => Self::LmStudio,
            Some("8080") => Self::LlamaCpp,
            _ => Self::Other,
        }
    }

    /// Where the program is installed, if found. `None` for
    /// [`Server::Other`]: only the address can be checked.
    pub fn installed(self) -> Option<PathBuf> {
        let mut dirs = usual_dirs();
        let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        match self {
            Self::Ollama => {
                if cfg!(target_os = "macos") {
                    let app = PathBuf::from("/Applications/Ollama.app");
                    if app.is_dir() {
                        return Some(app);
                    }
                }
                if let Some(base) = &local_app_data {
                    dirs.push(base.join("Programs").join("Ollama"));
                }
                find("ollama", &dirs)
            }
            Self::LmStudio => {
                if let Some(home) = home() {
                    dirs.push(home.join(".lmstudio").join("bin"));
                }
                if let Some(lms) = find("lms", &dirs) {
                    return Some(lms);
                }
                let mut apps = vec![PathBuf::from("/Applications/LM Studio.app")];
                if let Some(base) = &local_app_data {
                    apps.push(base.join("Programs").join("LM Studio"));
                    apps.push(base.join("Programs").join("lm-studio"));
                }
                // Created at the first launch, whatever the platform
                // (the Linux AppImage can live anywhere).
                apps.extend(home().map(|h| h.join(".lmstudio")));
                apps.into_iter().find(|p| p.exists())
            }
            Self::LlamaCpp => {
                if let Some(base) = &local_app_data {
                    dirs.push(base.join("Microsoft").join("WinGet").join("Links"));
                }
                find("llama-server", &dirs)
            }
            Self::Other => None,
        }
    }
}

/// Where Claude Code stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeStatus {
    /// The `claude` command, if found.
    pub program: Option<PathBuf>,
    /// Signed in, according to `claude auth status`; `None` when it could
    /// not tell (older version, error).
    pub logged_in: Option<bool>,
    /// How it is signed in (`claude.ai`, `console`, API key…).
    pub method: Option<String>,
}

/// Finds `claude` and asks it whether it is signed in.
pub async fn claude_status() -> ClaudeStatus {
    let Some(program) = find_claude() else {
        return ClaudeStatus {
            program: None,
            logged_in: None,
            method: None,
        };
    };
    let mut command = enfait_config::process::tokio_command(&program);
    command
        .args(["auth", "status"])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(15), command.output()).await;
    let (logged_in, method) = match output {
        Ok(Ok(output)) => parse_auth_status(&String::from_utf8_lossy(&output.stdout)),
        _ => (None, None),
    };
    ClaudeStatus {
        program: Some(program),
        logged_in,
        method,
    }
}

/// Reads the JSON of `claude auth status`.
pub fn parse_auth_status(stdout: &str) -> (Option<bool>, Option<String>) {
    let Ok(value) = serde_json::from_str::<Value>(stdout.trim()) else {
        return (None, None);
    };
    let logged_in = value.get("loggedIn").and_then(Value::as_bool);
    let method = value
        .get("authMethod")
        .and_then(Value::as_str)
        .filter(|m| !m.is_empty() && *m != "none")
        .map(str::to_owned);
    (logged_in, method)
}

/// A model served by the local server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServedModel {
    pub id: String,
    /// Reads images; `None` when the server does not say (only Ollama
    /// does).
    pub vision: Option<bool>,
}

/// Lists the models of an OpenAI-compatible server (`GET {base}/models`).
/// Embedding models are left out: they cannot write.
pub async fn list_models(
    client: &reqwest::Client,
    base_url: &str,
    key: Option<&str>,
) -> Result<Vec<ServedModel>, String> {
    let base = base_url.trim().trim_end_matches('/');
    let mut get = client
        .get(format!("{base}/models"))
        .timeout(Duration::from_secs(4));
    if let Some(key) = key.filter(|k| !k.trim().is_empty()) {
        get = get.bearer_auth(key);
    }
    let response = get.send().await.map_err(|e| {
        if e.is_connect() || e.is_timeout() {
            "unreachable".to_owned()
        } else {
            e.to_string()
        }
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP {}", status.as_u16()));
    }
    let value: Value = response.json().await.map_err(|e| e.to_string())?;
    let mut models = parse_models(&value);
    if Server::from_url(base) == Server::Ollama {
        let root = base.trim_end_matches("/v1");
        for model in &mut models {
            model.vision = ollama_vision(client, root, &model.id).await;
        }
    }
    Ok(models)
}

/// Reads the `data` list of a `/models` reply.
pub fn parse_models(value: &Value) -> Vec<ServedModel> {
    value
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| m.get("id").and_then(Value::as_str))
        .filter(|id| !id.to_ascii_lowercase().contains("embed"))
        .map(|id| ServedModel {
            id: id.to_owned(),
            vision: None,
        })
        .collect()
}

/// Whether an Ollama model reads images (`capabilities` of `/api/show`).
async fn ollama_vision(client: &reqwest::Client, root: &str, model: &str) -> Option<bool> {
    let value: Value = client
        .post(format!("{root}/api/show"))
        .json(&serde_json::json!({ "model": model }))
        .timeout(Duration::from_secs(4))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    let capabilities = value.get("capabilities")?.as_array()?;
    Some(capabilities.iter().any(|c| c.as_str() == Some("vision")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn server_from_its_port() {
        assert_eq!(
            Server::from_url("http://localhost:11434/v1"),
            Server::Ollama
        );
        assert_eq!(
            Server::from_url("http://127.0.0.1:1234/v1"),
            Server::LmStudio
        );
        assert_eq!(
            Server::from_url("http://localhost:8080/v1/"),
            Server::LlamaCpp
        );
        assert_eq!(Server::from_url("http://box:8000/v1"), Server::Other);
        assert_eq!(Server::from_url("https://example.com/v1"), Server::Other);
        for server in Server::ALL {
            if let Some(url) = server.default_url() {
                assert_eq!(Server::from_url(url), server);
            }
        }
    }

    #[test]
    fn reads_auth_status() {
        let signed =
            r#"{"loggedIn": true, "authMethod": "claude.ai", "apiProvider": "firstParty"}"#;
        assert_eq!(
            parse_auth_status(signed),
            (Some(true), Some("claude.ai".into()))
        );
        assert_eq!(
            parse_auth_status(r#"{"loggedIn": false, "authMethod": "none"}"#),
            (Some(false), None)
        );
        assert_eq!(
            parse_auth_status("error: unknown command 'auth'"),
            (None, None)
        );
    }

    #[test]
    fn reads_models_without_embeddings() {
        let reply = json!({ "object": "list", "data": [
            { "id": "qwen3-vl:8b", "object": "model" },
            { "id": "nomic-embed-text:latest", "object": "model" },
            { "id": "gemma3:12b", "object": "model" },
        ]});
        let ids: Vec<_> = parse_models(&reply).into_iter().map(|m| m.id).collect();
        assert_eq!(ids, ["qwen3-vl:8b", "gemma3:12b"]);
        assert!(parse_models(&json!({ "error": "x" })).is_empty());
    }
}
