//! AnthropicChatPlugin — streams responses from the Anthropic Messages API.
//!
//! Implements prompt caching via `cache_control: { type: "ephemeral" }` on the system
//! prompt and the last tool definition, and surfaces cache hit telemetry through the
//! `Stop` ChatDelta's `Usage` (prompt, prompt_cached, completion).

use crate::agent::error::{OperonError, OperonResult};
use crate::agent::plugins::sse::{SseEvent, SseStream};
use crate::agent::secrets::SecretStore;
use crate::agent::traits::{
    CancellationToken, Capabilities, ChatDelta, ChatPlugin, ChatRequest, ChatStream, ContentBlock,
    Message, Plugin, Role, StopReason, Usage,
};
use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

#[derive(Clone, Debug)]
pub struct AnthropicConfig {
    pub api_url: String,
    pub model: String,
    pub max_tokens: u32,
    pub anthropic_version: String,
    pub anthropic_beta: Option<String>,
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            api_url: "https://api.anthropic.com".to_string(),
            model: "claude-opus-4-7".to_string(),
            max_tokens: 4096,
            anthropic_version: "2023-06-01".to_string(),
            anthropic_beta: None,
        }
    }
}

pub struct AnthropicChatPlugin {
    cfg: AnthropicConfig,
    secrets: Arc<dyn SecretStore>,
    client: reqwest::Client,
    name: String,
}

impl AnthropicChatPlugin {
    pub fn new(cfg: AnthropicConfig, secrets: Arc<dyn SecretStore>) -> OperonResult<Self> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| OperonError::Provider {
                provider: "anthropic".into(),
                message: format!("client build: {e}"),
                retryable: false,
            })?;
        Ok(Self {
            cfg,
            secrets,
            client,
            name: "anthropic".to_string(),
        })
    }

    async fn api_key(&self) -> OperonResult<String> {
        // Try SecretStore first; fall back to ANTHROPIC_API_KEY env.
        if let Some(k) = self.secrets.get("provider/anthropic/api-key").await? {
            return Ok(k);
        }
        std::env::var("ANTHROPIC_API_KEY").map_err(|_| {
            OperonError::Secret(
                "anthropic api key missing (provider/anthropic/api-key in SecretStore or ANTHROPIC_API_KEY env)".into(),
            )
        })
    }

    fn build_body(&self, req: &ChatRequest) -> serde_json::Value {
        let model = req
            .model
            .clone()
            .unwrap_or_else(|| self.cfg.model.clone());
        let max_tokens = req.max_tokens.unwrap_or(self.cfg.max_tokens);

        // System prompt with cache_control: ephemeral
        let system: serde_json::Value = if let Some(s) = req.system.as_deref() {
            serde_json::json!([{
                "type": "text",
                "text": s,
                "cache_control": { "type": "ephemeral" }
            }])
        } else {
            serde_json::Value::Array(vec![])
        };

        // Messages
        let messages: Vec<serde_json::Value> = req
            .messages
            .iter()
            .filter_map(message_to_anthropic)
            .collect();

        // Tools with cache_control on last
        let mut tools: Vec<serde_json::Value> = req
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.input_schema,
                })
            })
            .collect();
        if let Some(last) = tools.last_mut() {
            if let Some(obj) = last.as_object_mut() {
                obj.insert(
                    "cache_control".to_string(),
                    serde_json::json!({ "type": "ephemeral" }),
                );
            }
        }

        let mut body = serde_json::json!({
            "model": model,
            "max_tokens": max_tokens,
            "messages": messages,
            "stream": true,
        });
        if !system.as_array().map(|a| a.is_empty()).unwrap_or(true) {
            body["system"] = system;
        }
        if !tools.is_empty() {
            body["tools"] = serde_json::Value::Array(tools);
        }
        body
    }
}

fn message_to_anthropic(m: &Message) -> Option<serde_json::Value> {
    let role = match m.role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "user", // tool results go in a user-role message per Anthropic spec
        Role::System => return None,
    };
    let content: Vec<serde_json::Value> = m
        .content
        .iter()
        .map(|cb| match cb {
            ContentBlock::Text(t) => serde_json::json!({"type": "text", "text": t}),
            ContentBlock::ToolUse { id, name, input } => {
                serde_json::json!({"type": "tool_use", "id": id, "name": name, "input": input})
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => serde_json::json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": content,
                "is_error": is_error,
            }),
        })
        .collect();
    Some(serde_json::json!({"role": role, "content": content}))
}

#[async_trait]
impl Plugin for AnthropicChatPlugin {
    fn name(&self) -> &str {
        &self.name
    }
    fn version(&self) -> &str {
        "0.1.0"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::STREAMING | Capabilities::TOOL_USE | Capabilities::PROMPT_CACHE
    }
}

#[async_trait]
impl ChatPlugin for AnthropicChatPlugin {
    async fn complete(&self, req: ChatRequest, ct: CancellationToken) -> OperonResult<ChatStream> {
        let api_key = self.api_key().await?;
        let body = self.build_body(&req);
        let url = format!("{}/v1/messages", self.cfg.api_url);

        let mut req_builder = self
            .client
            .post(&url)
            .header("x-api-key", api_key)
            .header("anthropic-version", &self.cfg.anthropic_version)
            .header("content-type", "application/json")
            .json(&body);
        if let Some(beta) = &self.cfg.anthropic_beta {
            req_builder = req_builder.header("anthropic-beta", beta);
        }

        let resp = req_builder
            .send()
            .await
            .map_err(|e| OperonError::Provider {
                provider: "anthropic".into(),
                message: format!("send: {e}"),
                retryable: matches!(
                    e.status(),
                    Some(s) if s.as_u16() >= 500 || s.as_u16() == 429
                ),
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(OperonError::Provider {
                provider: "anthropic".into(),
                message: format!("http {status}: {text}"),
                retryable: status.as_u16() >= 500 || status.as_u16() == 429,
            });
        }

        let bytes_stream = resp
            .bytes_stream()
            .map(|r| r.map_err(|e| format!("body: {e}")));
        let sse = SseStream::new(bytes_stream);
        let stream = AnthropicStream::new(sse, ct);
        Ok(Box::pin(stream))
    }
}

// === streaming state machine ===

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

struct AnthropicStream<S> {
    inner: SseStream<S>,
    ct: CancellationToken,
    pending_tool_uses: std::collections::HashMap<u64, PendingToolUse>,
    closed: bool,
    cumulative_usage: AnthropicUsage,
}

#[derive(Default, Clone, Debug)]
struct PendingToolUse {
    id: String,
    name: String,
    partial_json: String,
}

impl<S> AnthropicStream<S> {
    fn new(inner: SseStream<S>, ct: CancellationToken) -> Self {
        Self {
            inner,
            ct,
            pending_tool_uses: std::collections::HashMap::new(),
            closed: false,
            cumulative_usage: AnthropicUsage::default(),
        }
    }
}

impl<S, E> Stream for AnthropicStream<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    type Item = OperonResult<ChatDelta>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let me = self.get_mut();
        loop {
            if me.closed {
                return Poll::Ready(None);
            }
            if me.ct.is_cancelled() {
                me.closed = true;
                return Poll::Ready(Some(Err(OperonError::Cancelled)));
            }
            match Pin::new(&mut me.inner).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => {
                    me.closed = true;
                    return Poll::Ready(None);
                }
                Poll::Ready(Some(Err(e))) => {
                    me.closed = true;
                    return Poll::Ready(Some(Err(OperonError::Provider {
                        provider: "anthropic".into(),
                        message: e,
                        retryable: false,
                    })));
                }
                Poll::Ready(Some(Ok(ev))) => {
                    if let Some(out) = handle_event(me, &ev) {
                        return Poll::Ready(Some(out));
                    }
                    // Otherwise loop and pull next event.
                }
            }
        }
    }
}

fn handle_event<S>(state: &mut AnthropicStream<S>, ev: &SseEvent) -> Option<OperonResult<ChatDelta>> {
    let parsed: serde_json::Value = match serde_json::from_str(&ev.data) {
        Ok(v) => v,
        Err(_) => return None,
    };
    match ev.event.as_str() {
        "message_start" => None,
        "content_block_start" => {
            let idx = parsed.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
            let block = parsed.get("content_block")?;
            let kind = block.get("type")?.as_str()?;
            if kind == "tool_use" {
                let id = block
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                state.pending_tool_uses.insert(
                    idx,
                    PendingToolUse {
                        id,
                        name,
                        partial_json: String::new(),
                    },
                );
            }
            None
        }
        "content_block_delta" => {
            let idx = parsed.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
            let delta = parsed.get("delta")?;
            let kind = delta.get("type")?.as_str()?;
            match kind {
                "text_delta" => {
                    let text = delta.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    Some(Ok(ChatDelta::Text(text.to_string())))
                }
                "input_json_delta" => {
                    let partial = delta
                        .get("partial_json")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if let Some(p) = state.pending_tool_uses.get_mut(&idx) {
                        p.partial_json.push_str(partial);
                    }
                    None
                }
                _ => None,
            }
        }
        "content_block_stop" => {
            let idx = parsed.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
            if let Some(pending) = state.pending_tool_uses.remove(&idx) {
                let input = serde_json::from_str(&pending.partial_json)
                    .unwrap_or(serde_json::Value::Object(Default::default()));
                Some(Ok(ChatDelta::ToolUse {
                    id: pending.id,
                    name: pending.name,
                    input,
                }))
            } else {
                None
            }
        }
        "message_delta" => {
            if let Some(usage_v) = parsed.get("usage") {
                if let Ok(u) = serde_json::from_value::<AnthropicUsage>(usage_v.clone()) {
                    state.cumulative_usage.output_tokens += u.output_tokens;
                    state.cumulative_usage.input_tokens =
                        state.cumulative_usage.input_tokens.max(u.input_tokens);
                    state.cumulative_usage.cache_creation_input_tokens = state
                        .cumulative_usage
                        .cache_creation_input_tokens
                        .max(u.cache_creation_input_tokens);
                    state.cumulative_usage.cache_read_input_tokens = state
                        .cumulative_usage
                        .cache_read_input_tokens
                        .max(u.cache_read_input_tokens);
                }
            }
            let stop_reason = parsed
                .get("delta")
                .and_then(|d| d.get("stop_reason"))
                .and_then(|v| v.as_str())
                .map(map_stop_reason)
                .unwrap_or(StopReason::Other("unknown".into()));
            let usage = Usage {
                prompt: state.cumulative_usage.input_tokens
                    + state.cumulative_usage.cache_creation_input_tokens
                    + state.cumulative_usage.cache_read_input_tokens,
                prompt_cached: state.cumulative_usage.cache_read_input_tokens,
                completion: state.cumulative_usage.output_tokens,
            };
            Some(Ok(ChatDelta::Stop {
                reason: stop_reason,
                usage: Some(usage),
            }))
        }
        "message_stop" => {
            state.closed = true;
            None
        }
        "error" => {
            let msg = parsed
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            state.closed = true;
            Some(Err(OperonError::Provider {
                provider: "anthropic".into(),
                message: msg.to_string(),
                retryable: false,
            }))
        }
        _ => None,
    }
}

fn map_stop_reason(s: &str) -> StopReason {
    match s {
        "end_turn" => StopReason::EndTurn,
        "max_tokens" => StopReason::MaxTokens,
        "stop_sequence" => StopReason::StopSequence,
        "tool_use" => StopReason::Tool,
        other => StopReason::Other(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::secrets::MockSecretStore;
    use crate::agent::traits::{Message, Role};
    use std::collections::HashMap;

    #[test]
    fn build_body_includes_cache_control_on_system_and_last_tool() {
        let secrets: Arc<dyn SecretStore> = Arc::new(MockSecretStore::new());
        let p = AnthropicChatPlugin::new(AnthropicConfig::default(), secrets).unwrap();
        let req = ChatRequest {
            system: Some("you are operon".into()),
            messages: vec![],
            tools: vec![
                crate::agent::traits::ToolDef {
                    name: "a".into(),
                    description: "first".into(),
                    input_schema: serde_json::json!({}),
                },
                crate::agent::traits::ToolDef {
                    name: "b".into(),
                    description: "second".into(),
                    input_schema: serde_json::json!({}),
                },
            ],
            model: None,
            max_tokens: None,
        };
        let body = p.build_body(&req);
        assert!(body["system"][0]["cache_control"]["type"]
            .as_str()
            .unwrap()
            .eq("ephemeral"));
        assert!(body["tools"][0].get("cache_control").is_none());
        assert!(body["tools"][1]["cache_control"]["type"]
            .as_str()
            .unwrap()
            .eq("ephemeral"));
    }

    #[test]
    fn message_to_anthropic_user_text() {
        let m = Message {
            id: uuid::Uuid::new_v4(),
            role: Role::User,
            content: vec![ContentBlock::Text("hi".into())],
            created_at_ms: 0,
            session: uuid::Uuid::new_v4(),
            metadata: HashMap::new(),
        };
        let v = message_to_anthropic(&m).unwrap();
        assert_eq!(v["role"], "user");
        assert_eq!(v["content"][0]["type"], "text");
        assert_eq!(v["content"][0]["text"], "hi");
    }

    #[test]
    fn message_to_anthropic_tool_result_role_user() {
        let m = Message {
            id: uuid::Uuid::new_v4(),
            role: Role::Tool,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                content: "ok".into(),
                is_error: false,
            }],
            created_at_ms: 0,
            session: uuid::Uuid::new_v4(),
            metadata: HashMap::new(),
        };
        let v = message_to_anthropic(&m).unwrap();
        assert_eq!(v["role"], "user");
        assert_eq!(v["content"][0]["type"], "tool_result");
    }

    #[test]
    fn map_stop_reason_known() {
        assert!(matches!(map_stop_reason("end_turn"), StopReason::EndTurn));
        assert!(matches!(map_stop_reason("max_tokens"), StopReason::MaxTokens));
        assert!(matches!(map_stop_reason("tool_use"), StopReason::Tool));
        match map_stop_reason("custom") {
            StopReason::Other(s) => assert_eq!(s, "custom"),
            _ => panic!(),
        }
    }
}
