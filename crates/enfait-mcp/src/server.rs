//! MCP protocol over standard input and output: JSON-RPC 2.0, one message
//! per line. Only tools are offered (`tools/list`, `tools/call`); logs go
//! to standard error.

use crate::tools::Tools;
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt as _, AsyncWrite, AsyncWriteExt as _};

/// Understood protocol versions, from the most recent to the oldest.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "enfait is a local video editor with motion design. These tools \
work on the open session: storyboard (blocks of a voice-over, what is seen, precise moments, \
transitions), library (images, videos, sounds, with their licence), royalty-free search and \
sound finish. Start with `storyboard`. Blocks are referred to by their number (1, 2…). \
Animations are generated in the enfait app: `generate` drops requests for it, and it must be \
open to process them.";

/// The server: the state of the tools and the message loop.
pub struct Server {
    tools: Tools,
}

impl Server {
    pub fn new(tools: Tools) -> Self {
        Self { tools }
    }

    /// Reads the messages of `input` until its end, answers on `output`.
    pub async fn serve<R, W>(&mut self, input: R, mut output: W) -> std::io::Result<()>
    where
        R: AsyncBufRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut lines = input.lines();
        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            let reply = match serde_json::from_str::<Value>(&line) {
                Ok(message) => self.handle(message).await,
                Err(e) => Some(error(Value::Null, -32700, &format!("JSON illisible : {e}"))),
            };
            if let Some(reply) = reply {
                let mut text = reply.to_string();
                text.push('\n');
                output.write_all(text.as_bytes()).await?;
                output.flush().await?;
            }
        }
        Ok(())
    }

    /// Answers a message (`None` for a notification).
    pub async fn handle(&mut self, message: Value) -> Option<Value> {
        // A batch of messages (former protocol version).
        if let Value::Array(batch) = message {
            let mut replies = Vec::new();
            for m in batch {
                if let Some(r) = Box::pin(self.handle(m)).await {
                    replies.push(r);
                }
            }
            return (!replies.is_empty()).then_some(Value::Array(replies));
        }
        let id = message.get("id").cloned()?;
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        Some(match method {
            "initialize" => {
                let asked = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let version = PROTOCOL_VERSIONS
                    .iter()
                    .find(|v| **v == asked)
                    .copied()
                    .unwrap_or(PROTOCOL_VERSIONS[0]);
                result(
                    id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": { "tools": { "listChanged": false } },
                        "serverInfo": { "name": "enfait", "version": env!("CARGO_PKG_VERSION") },
                        "instructions": INSTRUCTIONS,
                    }),
                )
            }
            "ping" => result(id, json!({})),
            "tools/list" => result(id, json!({ "tools": Tools::list() })),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                let (text, is_error) = match self.tools.call(name, &args).await {
                    Ok(text) => (text, false),
                    Err(text) => (text, true),
                };
                result(
                    id,
                    json!({
                        "content": [{ "type": "text", "text": text }],
                        "isError": is_error,
                    }),
                )
            }
            _ => error(id, -32601, &format!("unknown method: {method}")),
        })
    }
}

fn result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}
