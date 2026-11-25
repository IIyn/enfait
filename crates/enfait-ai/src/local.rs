//! Local models: any server speaking OpenAI's "chat completions" API
//! (Ollama, llama.cpp `llama-server`, LM Studio, vLLM…).
//!
//! The model asked by the pipeline (Opus, Sonnet) is replaced by the local
//! server's; effort has no equivalent and is ignored. Images (critiques on
//! renders) are sent as `data:` URLs: the model must read them, otherwise
//! the server answers with an error.
//!
//! Ollama is recognised (`/api/version`) and then spoken to through its
//! own API (`/api/chat`): its OpenAI endpoint ignores the context size and
//! keeps the server's default (a few thousand tokens), too small for a
//! storyboard or an animation, and the reply comes back truncated. The
//! context is sized for each request (see [`context_for`]). Its reply is
//! streamed: a slow model that keeps writing is waited for, one that
//! stops writing for [`STALL`] fails with [`AiError::TimedOut`] (never
//! retried: the same request would be as slow). Reasoning (`think`) is off
//! unless the user turns it on: it makes the replies much longer.
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
/// Longest silence of a streamed reply (Ollama): reading a long prompt on
/// the processor can take minutes before the first word.
pub const STALL: Duration = Duration::from_secs(15 * 60);
/// Longest wait of a reply that is not streamed (other servers).
const WHOLE: Duration = Duration::from_secs(30 * 60);
/// Context asked of Ollama: at least this much…
const MIN_CONTEXT: u64 = 8_192;
/// …and at most this much (memory of the machine), unless the model
/// allows less.
const MAX_CONTEXT: u64 = 32_768;
/// Tokens counted for one image.
const IMAGE_TOKENS: u64 = 1_500;

/// An Ollama server.
#[derive(Debug, Clone)]
struct Ollama {
    /// Address without `/v1`.
    root: String,
    /// Longest context of the model, if known.
    model_context: Option<u64>,
    /// The model can reason (`thinking` capability): `think` is sent.
    thinking: bool,
}

pub struct LocalClient {
    http: reqwest::Client,
    /// Base address, without a trailing `/` (`…/v1`).
    base_url: String,
    model: String,
    api_key: Option<String>,
    /// Let a reasoning model think before answering (Ollama only).
    think: bool,
    /// See [`STALL`] (shorter in the tests).
    stall: Duration,
    /// Ollama behind the address (detected on the first request).
    ollama: tokio::sync::OnceCell<Option<Ollama>>,
}

impl LocalClient {
    pub fn new(base_url: &str, model: &str, api_key: Option<String>) -> Result<Self, AiError> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_owned(),
            model: model.to_owned(),
            api_key: api_key.filter(|k| !k.trim().is_empty()),
            think: false,
            stall: STALL,
            ollama: tokio::sync::OnceCell::new(),
        })
    }

    /// Lets a reasoning model think before answering.
    #[must_use]
    pub const fn with_thinking(mut self, think: bool) -> Self {
        self.think = think;
        self
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
        .map(|client| client.with_thinking(settings.local_thinking))
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn complete(&self, request: &Request) -> Result<Reply, AiError> {
        let ollama = self.ollama.get_or_init(|| self.detect()).await.clone();
        let (url, body) = match &ollama {
            Some(o) => (
                format!("{}/api/chat", o.root),
                self.ollama_body(request, o).await?,
            ),
            None => (
                format!("{}/chat/completions", self.base_url),
                self.body(request).await?,
            ),
        };
        let mut attempt = 0_u32;
        loop {
            attempt = attempt.saturating_add(1);
            let reply = if ollama.is_some() {
                self.stream(&url, &body)
                    .await
                    .and_then(|value| parse_ollama_reply(&value, &self.model))
            } else {
                match tokio::time::timeout(WHOLE, self.send(&url, &body)).await {
                    Ok(value) => value.and_then(|value| parse_reply(&value, &self.model)),
                    Err(_) => Err(AiError::TimedOut {
                        secs: WHOLE.as_secs(),
                    }),
                }
            };
            match reply {
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

    /// Ollama behind the address? Its OpenAI endpoint is under `/v1`, and
    /// `/api/version` answers with a version.
    async fn detect(&self) -> Option<Ollama> {
        let root = self.base_url.strip_suffix("/v1")?.to_owned();
        let version: Value = self
            .http
            .get(format!("{root}/api/version"))
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;
        version.get("version")?.as_str()?;
        let show = self
            .http
            .post(format!("{root}/api/show"))
            .timeout(Duration::from_secs(10))
            .json(&json!({ "model": self.model }))
            .send()
            .await
            .ok()?
            .json::<Value>()
            .await
            .unwrap_or(Value::Null);
        let model_context = show
            .get("model_info")
            .and_then(Value::as_object)
            .and_then(|info| info.iter().find(|(k, _)| k.ends_with(".context_length")))
            .and_then(|(_, v)| v.as_u64());
        let thinking = show
            .get("capabilities")
            .and_then(Value::as_array)
            .is_some_and(|c| c.iter().any(|c| c.as_str() == Some("thinking")));
        tracing::debug!(?model_context, thinking, "local model served by Ollama");
        Some(Ollama {
            root,
            model_context,
            thinking,
        })
    }

    /// Request in Ollama's own API, with a context sized for it.
    async fn ollama_body(&self, request: &Request, ollama: &Ollama) -> Result<Value, AiError> {
        let mut text = Vec::new();
        let mut images = Vec::new();
        for part in &request.user {
            match part {
                Part::Text(t) => text.push(t.as_str()),
                Part::Image(path) => images.push(Value::String(
                    base64::engine::general_purpose::STANDARD.encode(tokio::fs::read(path).await?),
                )),
            }
        }
        let text = text.join("\n\n");
        let prompt = estimate_tokens(&request.system)
            .saturating_add(estimate_tokens(&text))
            .saturating_add(IMAGE_TOKENS.saturating_mul(images.len() as u64));
        let (num_ctx, num_predict) =
            context_for(prompt, u64::from(request.max_tokens), ollama.model_context)?;
        let mut user = serde_json::Map::new();
        user.insert("role".into(), "user".into());
        user.insert("content".into(), text.into());
        if !images.is_empty() {
            user.insert("images".into(), Value::Array(images));
        }
        let mut body = json!({
            "model": self.model,
            "stream": true,
            "messages": [ { "role": "system", "content": request.system }, user ],
            "options": { "num_ctx": num_ctx, "num_predict": num_predict },
        });
        // Sent only to a model that can reason: others refuse the field.
        if ollama.thinking
            && let Some(fields) = body.as_object_mut()
        {
            fields.insert("think".into(), Value::Bool(self.think));
        }
        Ok(body)
    }

    /// Ollama's streamed reply, gathered in the shape of a whole reply.
    /// Every line moves the wait on: only a silence of `self.stall` fails.
    async fn stream(&self, url: &str, body: &Value) -> Result<Value, AiError> {
        let timed_out = || AiError::TimedOut {
            secs: self.stall.as_secs(),
        };
        let mut post = self.http.post(url).json(body);
        if let Some(key) = &self.api_key {
            post = post.bearer_auth(key);
        }
        let mut response = tokio::time::timeout(self.stall, post.send())
            .await
            .map_err(|_| timed_out())??;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(AiError::Api {
                status: status.as_u16(),
                message: serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| v.get("error")?.as_str().map(str::to_owned))
                    .unwrap_or(text),
            });
        }
        let mut lines = crate::ollama::Lines::default();
        let mut reply = Gathered::default();
        while let Some(chunk) = tokio::time::timeout(self.stall, response.chunk())
            .await
            .map_err(|_| timed_out())??
        {
            for line in lines.push(&chunk) {
                reply.add(&line)?;
            }
        }
        if let Some(line) = lines.rest() {
            reply.add(&line)?;
        }
        Ok(reply.into_value())
    }

    async fn send(&self, url: &str, body: &Value) -> Result<Value, AiError> {
        let mut post = self.http.post(url).json(body);
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
        Ok(serde_json::from_str(&text)?)
    }
}

/// The lines of a streamed Ollama reply, as one reply: the text pieces
/// joined, the last values of the other fields (counts, `done_reason`).
#[derive(Default)]
struct Gathered {
    fields: serde_json::Map<String, Value>,
    text: String,
}

impl Gathered {
    fn add(&mut self, line: &str) -> Result<(), AiError> {
        let Ok(Value::Object(mut fields)) = serde_json::from_str::<Value>(line) else {
            return Ok(());
        };
        if let Some(error) = fields.get("error").and_then(Value::as_str) {
            return Err(AiError::Api {
                status: 500,
                message: error.to_owned(),
            });
        }
        if let Some(piece) = fields
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
        {
            self.text.push_str(piece);
        }
        fields.remove("message");
        self.fields.extend(fields);
        Ok(())
    }

    fn into_value(mut self) -> Value {
        self.fields.insert(
            "message".into(),
            json!({ "role": "assistant", "content": self.text }),
        );
        Value::Object(self.fields)
    }
}

/// Tokens of a text, generously (about three characters per token in
/// French; better too much context than a truncated reply).
fn estimate_tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(3)
}

/// Context (`num_ctx`) and longest reply (`num_predict`) for a prompt of
/// `prompt` tokens: the prompt plus the reply asked, rounded up, between
/// [`MIN_CONTEXT`] and [`MAX_CONTEXT`] (or the model's own limit). A
/// prompt that does not fit is refused: Ollama would cut it silently.
fn context_for(prompt: u64, wanted: u64, model: Option<u64>) -> Result<(u64, u64), AiError> {
    const STEP: u64 = 4_096;
    /// The shortest reply worth asking for.
    const MIN_REPLY: u64 = 2_048;
    let ceiling = model.map_or(MAX_CONTEXT, |m| m.min(MAX_CONTEXT));
    let need = prompt.saturating_add(wanted).div_ceil(STEP).saturating_mul(STEP);
    let num_ctx = need.clamp(MIN_CONTEXT.min(ceiling), ceiling);
    let room = num_ctx.saturating_sub(prompt);
    if room < MIN_REPLY {
        return Err(AiError::Config(format!(
            "prompt too big for the local model (about {prompt} tokens for a context of \
             {num_ctx}): lower the blocks per storyboard request (Settings → Models)"
        )));
    }
    Ok((num_ctx, wanted.min(room)))
}

/// Reads an Ollama `/api/chat` reply.
fn parse_ollama_reply(value: &Value, model: &str) -> Result<Reply, AiError> {
    if value.get("done_reason").and_then(Value::as_str) == Some("length") {
        return Err(AiError::Truncated);
    }
    let text = value
        .pointer("/message/content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if text.trim().is_empty() {
        return Err(AiError::Unexpected(
            "empty reply from the local model".into(),
        ));
    }
    let tokens = |key: &str| value.get(key).and_then(Value::as_u64).unwrap_or(0);
    Ok(Reply {
        text,
        model: value
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(model)
            .to_owned(),
        usage: Usage {
            input_tokens: tokens("prompt_eval_count"),
            output_tokens: tokens("eval_count"),
            ..Usage::default()
        },
    })
}

fn retryable(error: &AiError) -> bool {
    match error {
        // A timeout is not retried: the same request would be as slow.
        AiError::Http(e) => e.is_connect(),
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

    /// A fake local server: answers each request with `route(request line)`
    /// (status, body) and keeps the requests seen.
    async fn fake_server(
        route: fn(&str) -> (u16, String),
    ) -> (u16, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = std::sync::Arc::clone(&seen);
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut data = Vec::new();
                let mut buf = [0_u8; 4096];
                // Read up to the end of the body.
                loop {
                    let n = socket.read(&mut buf).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&data);
                    if let Some(head) = text.find("\r\n\r\n") {
                        let length = text
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if data.len() >= head + 4 + length {
                            break;
                        }
                    }
                }
                let text = String::from_utf8_lossy(&data).into_owned();
                let (status, body) = route(text.lines().next().unwrap_or_default());
                log.lock().unwrap().push(text);
                let response = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        (port, seen)
    }

    /// A real HTTP exchange with a server that is not Ollama.
    #[tokio::test]
    async fn talks_to_a_server() {
        let (port, seen) = fake_server(|line| {
            if line.starts_with("POST /v1/chat/completions") {
                (200, r#"{"model":"faux","choices":[{"message":{"role":"assistant","content":"bonjour"},"finish_reason":"stop"}]}"#.into())
            } else {
                (404, "{}".into())
            }
        })
        .await;
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
        let seen = seen.lock().unwrap();
        let chat = seen.last().unwrap();
        assert!(chat.starts_with("POST /v1/chat/completions"));
        assert!(
            chat.to_ascii_lowercase()
                .contains("authorization: bearer secret")
        );
        assert!(chat.contains("\"model\":\"faux\""));
    }

    /// Ollama: its own API, with a context sized for the request.
    #[tokio::test]
    async fn sizes_the_context_of_ollama() {
        let (port, seen) = fake_server(|line| {
            if line.starts_with("GET /api/version") {
                (200, r#"{"version":"0.40.0"}"#.into())
            } else if line.starts_with("POST /api/show") {
                (200, r#"{"model_info":{"qwen3vl.context_length":262144},"capabilities":["completion","thinking"]}"#.into())
            } else if line.starts_with("POST /api/chat") {
                // Streamed: one line per piece, the counts on the last one.
                (200, concat!(
                    r#"{"model":"qwen3","message":{"role":"assistant","content":"","thinking":"hmm"},"done":false}"#, "\n",
                    r#"{"model":"qwen3","message":{"role":"assistant","content":"o"},"done":false}"#, "\n",
                    r#"{"model":"qwen3","message":{"role":"assistant","content":"k"},"done":true,"done_reason":"stop","prompt_eval_count":40,"eval_count":2}"#, "\n",
                ).into())
            } else {
                (404, "{}".into())
            }
        })
        .await;
        let client =
            LocalClient::new(&format!("http://127.0.0.1:{port}/v1"), "qwen3", None).unwrap();
        let mut req = request(vec![Part::Text("x".repeat(30_000))]);
        req.max_tokens = 16_000;
        let reply = client.complete(&req).await.unwrap();
        assert_eq!((reply.text.as_str(), reply.usage.input_tokens), ("ok", 40));
        // Detected once: a second request goes straight to /api/chat.
        client.complete(&req).await.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 4);
        let chat = &seen[2];
        assert!(chat.starts_with("POST /api/chat"));
        // 10,000 tokens of message + 16,000 of reply: 28,672 of context.
        assert!(chat.contains("\"num_ctx\":28672"), "{chat}");
        assert!(chat.contains("\"num_predict\":16000"));
        assert!(chat.contains("\"stream\":true"));
        assert!(chat.contains("\"think\":false"), "reasoning off by default");

        let cut = json!({ "message": { "content": "{" }, "done_reason": "length" });
        assert!(matches!(
            parse_ollama_reply(&cut, "m"),
            Err(AiError::Truncated)
        ));
    }

    /// A model that says nothing fails with a timeout, once (not retried).
    #[tokio::test]
    async fn a_silent_model_times_out() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let chats = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = std::sync::Arc::clone(&chats);
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let seen = std::sync::Arc::clone(&seen);
                tokio::spawn(async move {
                    let mut buf = [0_u8; 8192];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    let head = String::from_utf8_lossy(&buf[..n]).into_owned();
                    let body = if head.starts_with("GET /api/version") {
                        r#"{"version":"0.40.0"}"#
                    } else if head.starts_with("POST /api/show") {
                        "{}"
                    } else {
                        // The chat: headers, then silence.
                        seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        let _ = socket
                            .write_all(b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n")
                            .await;
                        tokio::time::sleep(Duration::from_secs(30)).await;
                        return;
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });
        let mut client =
            LocalClient::new(&format!("http://127.0.0.1:{port}/v1"), "qwen3", None).unwrap();
        client.stall = Duration::from_millis(300);
        let error = client
            .complete(&request(vec![Part::Text("x".into())]))
            .await
            .unwrap_err();
        assert!(matches!(error, AiError::TimedOut { .. }), "{error}");
        assert_eq!(chats.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn context_fits_the_prompt() {
        assert_eq!(context_for(1_000, 4_000, None).unwrap(), (8_192, 4_000));
        assert_eq!(context_for(5_000, 32_000, None).unwrap(), (32_768, 27_768));
        assert_eq!(context_for(5_000, 32_000, Some(8_192)).unwrap(), (8_192, 3_192));
        assert!(matches!(
            context_for(40_000, 16_000, None),
            Err(AiError::Config(_))
        ));
    }
}
