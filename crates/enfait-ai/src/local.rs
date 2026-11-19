//! Local models: any server speaking OpenAI's "chat completions" API
//! (Ollama, llama.cpp `llama-server`, LM Studio, vLLM…).
//!
//! The model asked by the pipeline (Opus, Sonnet) is replaced by the local
//! server's; effort has no equivalent and is ignored. Images (critiques on
//! renders) are sent as `data:` URLs: the model must read them, otherwise
//! the server answers with an error.
//!
//! Settings: Settings → Models, or `ENFAIT_LOCAL_URL` (default
//! `http://localhost:11434/v1`, Ollama), `ENFAIT_LOCAL_MODEL`,
//! `ENFAIT_LOCAL_KEY` (optional, for a protected server).

use crate::llm::{AiError, Part, Reply, Request, Usage};
use base64::Engine as _;
use serde_json::{Value, json};
use std::time::Duration;

pub const DEFAULT_URL: &str = enfait_config::DEFAULT_LOCAL_URL;
const MAX_ATTEMPTS: u32 = 3;

pub struct LocalClient {
    http: reqwest::Client,
    /// Base address, without a trailing `/` (`…/v1`).
    base_url: String,
    model: String,
    api_key: Option<String>,
}

impl LocalClient {
    pub fn new(base_url: &str, model: &str, api_key: Option<String>) -> Result<Self, AiError> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            // A big model on a small machine can take a long time.
            .read_timeout(Duration::from_secs(1800))
            .build()?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_owned(),
            model: model.to_owned(),
            api_key: api_key.filter(|k| !k.trim().is_empty()),
        })
    }

    /// Client from the settings (`ENFAIT_LOCAL_URL`, `ENFAIT_LOCAL_MODEL`
    /// and `ENFAIT_LOCAL_KEY` first).
    pub fn from_config() -> Result<Self, AiError> {
        let settings = enfait_config::Settings::load();
        let model = settings.local_model().ok_or_else(|| {
            AiError::Config(
                "no local model configured (Settings → Models, or ENFAIT_LOCAL_MODEL, \
                 e.g. \"qwen3-coder:30b\")"
                    .into(),
            )
        })?;
        Self::new(
            &settings.local_url(),
            &model,
            enfait_config::secrets::get(enfait_config::Secret::Local),
        )
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn complete(&self, request: &Request) -> Result<Reply, AiError> {
        let body = self.body(request).await?;
        let mut attempt = 0_u32;
        loop {
            attempt = attempt.saturating_add(1);
            match self.send(&body).await {
                Err(e) if attempt < MAX_ATTEMPTS && retryable(&e) => {
                    tracing::warn!("local model: {e}, retrying");
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
                other => return other,
            }
        }
    }

    async fn body(&self, request: &Request) -> Result<Value, AiError> {
        let mut content = Vec::with_capacity(request.user.len());
        for part in &request.user {
            content.push(match part {
                Part::Text(text) => json!({ "type": "text", "text": text }),
                Part::Image(path) => {
                    let data = tokio::fs::read(path).await?;
                    let media = match path.extension().and_then(|e| e.to_str()) {
                        Some("png") => "image/png",
                        _ => "image/jpeg",
                    };
                    let encoded = base64::engine::general_purpose::STANDARD.encode(data);
                    json!({
                        "type": "image_url",
                        "image_url": { "url": format!("data:{media};base64,{encoded}") },
                    })
                }
            });
        }
        // Text only: a string, which every server understands.
        let user = if request.user.iter().all(|p| matches!(p, Part::Text(_))) {
            Value::String(
                request
                    .user
                    .iter()
                    .filter_map(|p| match p {
                        Part::Text(t) => Some(t.as_str()),
                        Part::Image(_) => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            )
        } else {
            Value::Array(content)
        };
        Ok(json!({
            "model": self.model,
            "max_tokens": request.max_tokens,
            "stream": false,
            "messages": [
                { "role": "system", "content": request.system },
                { "role": "user", "content": user },
            ],
        }))
    }

    async fn send(&self, body: &Value) -> Result<Reply, AiError> {
        let mut post = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .json(body);
        if let Some(key) = &self.api_key {
            post = post.bearer_auth(key);
        }
        let response = post.send().await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            let message = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| {
                    v.pointer("/error/message")
                        .or_else(|| v.get("error"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or(text);
            return Err(AiError::Api {
                status: status.as_u16(),
                message,
            });
        }
        parse_reply(&serde_json::from_str(&text)?, &self.model)
    }
}

fn retryable(error: &AiError) -> bool {
    match error {
        AiError::Http(e) => e.is_connect() || e.is_timeout(),
        AiError::Api { status, .. } => *status == 429 || *status >= 500,
        _ => false,
    }
}

/// Reads a "chat completions" reply.
pub fn parse_reply(value: &Value, model: &str) -> Result<Reply, AiError> {
    let choice = value
        .pointer("/choices/0")
        .ok_or_else(|| AiError::Unexpected("reply without `choices`".into()))?;
    let text = choice
        .pointer("/message/content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    match choice.get("finish_reason").and_then(Value::as_str) {
        Some("length") => return Err(AiError::Truncated),
        Some("content_filter") => return Err(AiError::Refusal),
        _ => {}
    }
    if text.trim().is_empty() {
        return Err(AiError::Unexpected(
            "empty reply from the local model".into(),
        ));
    }
    let tokens = |key: &str| {
        value
            .pointer(&format!("/usage/{key}"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    Ok(Reply {
        text,
        model: value
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(model)
            .to_owned(),
        usage: Usage {
            input_tokens: tokens("prompt_tokens"),
            output_tokens: tokens("completion_tokens"),
            ..Usage::default()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Effort;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    fn request(user: Vec<Part>) -> Request {
        Request {
            model: "claude-opus-5-5".into(),
            effort: Effort::High,
            max_tokens: 1000,
            system: "Tu es animateur.".into(),
            user,
        }
    }

    #[tokio::test]
    async fn body_and_reply() {
        let client = LocalClient::new("http://x/v1/", "qwen3", None).unwrap();
        assert_eq!(client.base_url(), "http://x/v1");
        let body = client
            .body(&request(vec![
                Part::Text("a".into()),
                Part::Text("b".into()),
            ]))
            .await
            .unwrap();
        assert_eq!(body["model"], "qwen3", "the local model replaces Opus");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "a\n\nb");

        let dir = std::env::temp_dir().join(format!("enfait-local-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let image = dir.join("i.png");
        std::fs::write(&image, [1_u8, 2, 3]).unwrap();
        let body = client
            .body(&request(vec![
                Part::Text("vois".into()),
                Part::Image(image),
            ]))
            .await
            .unwrap();
        let url = body["messages"][1]["content"][1]["image_url"]["url"]
            .as_str()
            .unwrap();
        assert_eq!(url, "data:image/png;base64,AQID");
        let _ = std::fs::remove_dir_all(&dir);

        let ok = json!({ "model": "qwen3:30b", "choices": [{ "message": { "content": "```html\n<html></html>\n```" }, "finish_reason": "stop" }],
                          "usage": { "prompt_tokens": 12, "completion_tokens": 5 } });
        let reply = parse_reply(&ok, "qwen3").unwrap();
        assert_eq!(
            (reply.model.as_str(), reply.usage.output_tokens),
            ("qwen3:30b", 5)
        );
        let cut = json!({ "choices": [{ "message": { "content": "<html" }, "finish_reason": "length" }] });
        assert!(matches!(parse_reply(&cut, "m"), Err(AiError::Truncated)));
    }

    /// A real HTTP exchange with a fake local server.
    #[tokio::test]
    async fn talks_to_a_server() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut seen = Vec::new();
            let mut buf = [0_u8; 4096];
            // Read up to the end of the JSON body.
            loop {
                let n = socket.read(&mut buf).await.unwrap();
                seen.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&seen);
                if let Some(head) = text.find("\r\n\r\n") {
                    let length = text
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if seen.len() >= head + 4 + length {
                        break;
                    }
                }
            }
            let reply = r#"{"model":"faux","choices":[{"message":{"role":"assistant","content":"bonjour"},"finish_reason":"stop"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                reply.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&seen).into_owned()
        });
        let client = LocalClient::new(
            &format!("http://127.0.0.1:{port}/v1"),
            "faux",
            Some("secret".into()),
        )
        .unwrap();
        let reply = client
            .complete(&request(vec![Part::Text("salut".into())]))
            .await
            .unwrap();
        assert_eq!(reply.text, "bonjour");
        let seen = server.await.unwrap();
        assert!(seen.starts_with("POST /v1/chat/completions"));
        assert!(
            seen.to_ascii_lowercase()
                .contains("authorization: bearer secret")
        );
        assert!(seen.contains("\"model\":\"faux\""));
    }
}
