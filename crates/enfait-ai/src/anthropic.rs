//! API Messages d'Anthropic, en HTTP direct avec streaming SSE.

use crate::llm::{AiError, Part, Reply, Request, Usage};
use base64::Engine as _;
use eventsource_stream::Eventsource as _;
use futures::StreamExt as _;
use serde_json::{Value, json};
use std::time::Duration;

const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2026-10-21";
const MAX_ATTEMPTS: u32 = 4;

pub struct AnthropicClient {
    http: reqwest::Client,
    api_key: String,
}

impl AnthropicClient {
    pub fn new(api_key: String) -> Result<Self, AiError> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .read_timeout(Duration::from_secs(300))
            .build()?;
        Ok(Self { http, api_key })
    }

    /// Client with the key from the settings (`ANTHROPIC_API_KEY` first).
    pub fn from_config() -> Result<Self, AiError> {
        let key = enfait_config::secrets::get(enfait_config::Secret::Anthropic)
            .ok_or(AiError::MissingKey)?;
        Self::new(key)
    }

    pub async fn complete(&self, request: &Request) -> Result<Reply, AiError> {
        let body = request_body(request).await?;
        let mut attempt = 0u32;
        loop {
            attempt = attempt.saturating_add(1);
            match self.send(&body).await {
                Err(Retry::Later(wait, error)) if attempt < MAX_ATTEMPTS => {
                    tracing::warn!("{error}, nouvel essai dans {}s", wait.as_secs());
                    tokio::time::sleep(wait).await;
                }
                Err(Retry::Later(_, error) | Retry::Never(error)) => return Err(error),
                Ok(reply) => return Ok(reply),
            }
        }
    }

    async fn send(&self, body: &Value) -> Result<Reply, Retry> {
        let response = self
            .http
            .post(ENDPOINT)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .json(body)
            .send()
            .await
            .map_err(|e| Retry::Later(Duration::from_secs(5), e.into()))?;
        let status = response.status();
        if !status.is_success() {
            let wait = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .map_or(Duration::from_secs(10), Duration::from_secs);
            let text = response.text().await.unwrap_or_default();
            let message = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| {
                    v.pointer("/error/message")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or(text);
            let error = AiError::Api {
                status: status.as_u16(),
                message,
            };
            return Err(match status.as_u16() {
                429 | 500 | 502 | 503 | 504 | 529 => Retry::Later(wait, error),
                _ => Retry::Never(error),
            });
        }

        let mut events = response.bytes_stream().eventsource();
        let mut text = String::new();
        let mut text_blocks = std::collections::HashSet::new();
        let mut model = String::new();
        let mut usage = Usage::default();
        let mut stop_reason = String::new();
        while let Some(event) = events.next().await {
            let event = event.map_err(|e| {
                Retry::Later(Duration::from_secs(5), AiError::Unexpected(e.to_string()))
            })?;
            let data: Value =
                serde_json::from_str(&event.data).map_err(|e| Retry::Never(AiError::Json(e)))?;
            match data.get("type").and_then(Value::as_str).unwrap_or_default() {
                "message_start" => {
                    let message = data.get("message").cloned().unwrap_or_default();
                    model = message
                        .get("model")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    read_usage(message.get("usage"), &mut usage);
                }
                "content_block_start" => {
                    let is_text =
                        data.pointer("/content_block/type").and_then(Value::as_str) == Some("text");
                    if let (true, Some(index)) =
                        (is_text, data.get("index").and_then(Value::as_u64))
                    {
                        text_blocks.insert(index);
                        if !text.is_empty() {
                            text.push('\n');
                        }
                    }
                }
                "content_block_delta" => {
                    let index = data
                        .get("index")
                        .and_then(Value::as_u64)
                        .unwrap_or(u64::MAX);
                    if text_blocks.contains(&index)
                        && let Some(delta) = data.pointer("/delta/text").and_then(Value::as_str)
                    {
                        text.push_str(delta);
                    }
                }
                "message_delta" => {
                    if let Some(reason) = data.pointer("/delta/stop_reason").and_then(Value::as_str)
                    {
                        reason.clone_into(&mut stop_reason);
                    }
                    read_usage(data.get("usage"), &mut usage);
                }
                "error" => {
                    let kind = data
                        .pointer("/error/type")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let message = data
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("erreur de flux")
                        .to_owned();
                    let error = AiError::Api { status: 0, message };
                    return Err(if kind == "overloaded_error" || kind == "api_error" {
                        Retry::Later(Duration::from_secs(15), error)
                    } else {
                        Retry::Never(error)
                    });
                }
                _ => {}
            }
        }
        match stop_reason.as_str() {
            "refusal" => Err(Retry::Never(AiError::Refusal)),
            "max_tokens" => Err(Retry::Never(AiError::Truncated)),
            _ => Ok(Reply { text, model, usage }),
        }
    }
}

enum Retry {
    Later(Duration, AiError),
    Never(AiError),
}

fn read_usage(value: Option<&Value>, usage: &mut Usage) {
    let Some(value) = value else { return };
    let get = |key: &str| value.get(key).and_then(Value::as_u64);
    if let Some(v) = get("input_tokens") {
        usage.input_tokens = v;
    }
    if let Some(v) = get("output_tokens") {
        usage.output_tokens = v;
    }
    if let Some(v) = get("cache_read_input_tokens") {
        usage.cache_read_input_tokens = v;
    }
    if let Some(v) = get("cache_creation_input_tokens") {
        usage.cache_creation_input_tokens = v;
    }
}

async fn request_body(request: &Request) -> Result<Value, AiError> {
    let mut content = Vec::with_capacity(request.user.len());
    for part in &request.user {
        content.push(match part {
            Part::Text(text) => json!({ "type": "text", "text": text }),
            Part::Image(path) => {
                let data = tokio::fs::read(path).await?;
                let media_type = match path.extension().and_then(|e| e.to_str()) {
                    Some("png") => "image/png",
                    _ => "image/jpeg",
                };
                json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": media_type,
                        "data": base64::engine::general_purpose::STANDARD.encode(data),
                    }
                })
            }
        });
    }
    Ok(json!({
        "model": request.model,
        "max_tokens": request.max_tokens,
        "stream": true,
        "thinking": { "type": "adaptive" },
        "output_config": { "effort": request.effort.as_str() },
        "system": [{
            "type": "text",
            "text": request.system,
            "cache_control": { "type": "ephemeral" },
        }],
        "messages": [{ "role": "user", "content": content }],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Effort;

    #[tokio::test]
    async fn body_shape() {
        let request = Request {
            model: "claude-opus-5-5".into(),
            effort: Effort::High,
            max_tokens: 1000,
            system: "sys".into(),
            user: vec![Part::Text("bonjour".into())],
        };
        let body = request_body(&request).await.unwrap();
        assert_eq!(body["output_config"]["effort"], "high");
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(body["messages"][0]["content"][0]["text"], "bonjour");
        assert!(body.get("temperature").is_none());
    }
}
