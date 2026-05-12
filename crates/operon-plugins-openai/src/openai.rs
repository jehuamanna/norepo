//! OpenAI Chat Completions ChatPlugin (also serves OpenAI-compatible endpoints
//! like Ollama's `/v1` API, vLLM, llama.cpp server, LM Studio).
//!
//! Streaming via SSE. Tool calls translated from OpenAI's `function`-style
//! schema to Operon's canonical `ChatDelta::ToolUse` shape.

use crate::sse::{SseEvent, SseStream};
use operon_core::error::{OperonError, OperonResult};
use operon_core::secrets::{keys as secret_keys, SecretStore};
use operon_core::traits::{
    CancellationToken, Capabilities, ChatDelta, ChatPlugin, ChatRequest, ChatStream, ContentBlock,
    Message, Plugin, Role, StopReason, Usage,
};
use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

#[derive(Clone, Debug)]
pub struct OpenAIConfig {
    pub api_url: String,
    pub model: String,
    pub max_tokens: u32,
    /// Local servers (Ollama et al.) often allow empty / default API keys.
    pub require_api_key: bool,
}

impl Default for OpenAIConfig {
    fn default() -> Self {
        Self {
            api_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-5".to_string(),
            max_tokens: 4096,
            require_api_key: true,
        }
    }
}

impl OpenAIConfig {
    /// Slice A7: point the OpenAI plugin at Ollama's OpenAI-compatible endpoint.
    /// Defaults to `http://localhost:11434/v1`. No API key required.
    pub fn ollama(model: impl Into<String>) -> Self {
        Self {
            api_url: "http://localhost:11434/v1".to_string(),
            model: model.into(),
            max_tokens: 4096,
            require_api_key: false,
        }
    }

    /// Slice A7: vLLM serves an OpenAI-compatible API at `/v1`.
    pub fn vllm(api_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_url: api_url.into(),
            model: model.into(),
            max_tokens: 4096,
            require_api_key: false,
        }
    }

    /// Slice A7: llama.cpp's `server` example exposes `/v1`.
    pub fn llama_cpp(api_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_url: api_url.into(),
            model: model.into(),
            max_tokens: 4096,
            require_api_key: false,
        }
    }
}

pub struct OpenAIChatPlugin {
    cfg: OpenAIConfig,
    secrets: Arc<dyn SecretStore>,
    client: reqwest::Client,
    name: String,
}

impl OpenAIChatPlugin {
    pub fn new(cfg: OpenAIConfig, secrets: Arc<dyn SecretStore>) -> OperonResult<Self> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| OperonError::Provider {
                provider: "openai".into(),
                message: format!("client build: {e}"),
                retryable: false,
            })?;
        Ok(Self {
            cfg,
            secrets,
            client,
            name: "openai".to_string(),
        })
    }

    pub fn config(&self) -> &OpenAIConfig {
        &self.cfg
    }

    async fn api_key(&self) -> OperonResult<Option<String>> {
        if let Some(k) = self.secrets.get(secret_keys::OPENAI_API_KEY).await? {
            return Ok(Some(k));
        }
        Ok(std::env::var("OPENAI_API_KEY").ok())
    }

    fn build_body(&self, req: &ChatRequest) -> serde_json::Value {
        let model = req
            .model
            .clone()
            .unwrap_or_else(|| self.cfg.model.clone());
        let max_tokens = req.max_tokens.unwrap_or(self.cfg.max_tokens);

        let mut messages: Vec<serde_json::Value> = Vec::new();
        if let Some(s) = req.system.as_deref() {
            messages.push(serde_json::json!({ "role": "system", "content": s }));
        }
        for m in &req.messages {
            messages.extend(message_to_openai(m));
        }

        let mut body = serde_json::json!({
            "model": model,
            "max_tokens": max_tokens,
            "messages": messages,
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        if !req.tools.is_empty() {
            let tools: Vec<serde_json::Value> = req
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.input_schema,
                        }
                    })
                })
                .collect();
            body["tools"] = serde_json::Value::Array(tools);
        }
        body
    }
}

/// Translate one Operon `Message` into one or more OpenAI-shaped messages.
/// Returns multiple when the message is `Tool` with several tool_results
/// (OpenAI requires one `role:tool` message per `tool_call_id`).
fn message_to_openai(m: &Message) -> Vec<serde_json::Value> {
    match m.role {
        Role::System => {
            let text = collect_text(&m.content);
            if text.is_empty() {
                vec![]
            } else {
                vec![serde_json::json!({ "role": "system", "content": text })]
            }
        }
        Role::User => {
            // Filter out any ToolUse/ToolResult blocks (shouldn't appear on user msgs).
            let text = collect_text(&m.content);
            vec![serde_json::json!({ "role": "user", "content": text })]
        }
        Role::Assistant => {
            // Assistant messages can carry text + tool_calls.
            let text = collect_text(&m.content);
            let tool_calls: Vec<serde_json::Value> = m
                .content
                .iter()
                .filter_map(|cb| match cb {
                    ContentBlock::ToolUse { id, name, input } => Some(serde_json::json!({
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": serde_json::to_string(input).unwrap_or_default(),
                        }
                    })),
                    _ => None,
                })
                .collect();
            let mut msg = serde_json::json!({ "role": "assistant" });
            if !text.is_empty() {
                msg["content"] = serde_json::Value::String(text);
            } else {
                msg["content"] = serde_json::Value::Null;
            }
            if !tool_calls.is_empty() {
                msg["tool_calls"] = serde_json::Value::Array(tool_calls);
            }
            vec![msg]
        }
        Role::Tool => {
            // Each ToolResult becomes a separate role:tool message.
            m.content
                .iter()
                .filter_map(|cb| match cb {
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error: _,
                    } => Some(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": tool_use_id,
                        "content": content,
                    })),
                    _ => None,
                })
                .collect()
        }
    }
}

fn collect_text(content: &[ContentBlock]) -> String {
    let mut out = String::new();
    for cb in content {
        if let ContentBlock::Text(t) = cb {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(t);
        }
    }
    out
}

#[async_trait]
impl Plugin for OpenAIChatPlugin {
    fn name(&self) -> &str { &self.name }
    fn version(&self) -> &str { env!("CARGO_PKG_VERSION") }
    fn capabilities(&self) -> Capabilities {
        Capabilities::STREAMING | Capabilities::TOOL_USE | Capabilities::VISION
    }
}

#[async_trait]
impl ChatPlugin for OpenAIChatPlugin {
    async fn complete(&self, req: ChatRequest, ct: CancellationToken) -> OperonResult<ChatStream> {
        let api_key = self.api_key().await?;
        let body = self.build_body(&req);
        let url = format!("{}/chat/completions", self.cfg.api_url);

        let mut req_builder = self
            .client
            .post(&url)
            .header("content-type", "application/json")
            .json(&body);
        if let Some(k) = api_key {
            req_builder = req_builder.header("authorization", format!("Bearer {k}"));
        } else if self.cfg.require_api_key {
            return Err(OperonError::Secret(format!(
                "openai api key missing ({} in SecretStore or OPENAI_API_KEY env)",
                secret_keys::OPENAI_API_KEY
            )));
        }

        let resp = req_builder
            .send()
            .await
            .map_err(|e| OperonError::Provider {
                provider: "openai".into(),
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
                provider: "openai".into(),
                message: format!("http {status}: {text}"),
                retryable: status.as_u16() >= 500 || status.as_u16() == 429,
            });
        }

        let bytes_stream = resp
            .bytes_stream()
            .map(|r| r.map_err(|e| format!("body: {e}")));
        let sse = SseStream::new(bytes_stream);
        let stream = OpenAIStream::new(sse, ct);
        Ok(Box::pin(stream))
    }
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
struct OpenAIUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

#[derive(Default, Clone, Debug)]
struct PendingToolCall {
    id: String,
    name: String,
    arguments: String,
}

struct OpenAIStream<S> {
    inner: SseStream<S>,
    ct: CancellationToken,
    pending: HashMap<u64, PendingToolCall>,
    pending_emit: Vec<ChatDelta>,
    closed: bool,
    cumulative_usage: OpenAIUsage,
    seen_finish: Option<String>,
}

impl<S> OpenAIStream<S> {
    fn new(inner: SseStream<S>, ct: CancellationToken) -> Self {
        Self {
            inner,
            ct,
            pending: HashMap::new(),
            pending_emit: Vec::new(),
            closed: false,
            cumulative_usage: OpenAIUsage::default(),
            seen_finish: None,
        }
    }
}

impl<S, E> Stream for OpenAIStream<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    type Item = OperonResult<ChatDelta>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let me = self.get_mut();
        loop {
            if let Some(d) = me.pending_emit.pop() {
                return Poll::Ready(Some(Ok(d)));
            }
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
                        provider: "openai".into(),
                        message: e,
                        retryable: false,
                    })));
                }
                Poll::Ready(Some(Ok(ev))) => {
                    if let Some(out) = handle_event(me, &ev) {
                        return Poll::Ready(Some(out));
                    }
                }
            }
        }
    }
}

fn handle_event<S>(state: &mut OpenAIStream<S>, ev: &SseEvent) -> Option<OperonResult<ChatDelta>> {
    if ev.data == "[DONE]" {
        // Flush pending tool calls (one ChatDelta::ToolUse per).
        for (_idx, pc) in state.pending.drain() {
            let input = serde_json::from_str(&pc.arguments)
                .unwrap_or(serde_json::Value::Object(Default::default()));
            state.pending_emit.push(ChatDelta::ToolUse {
                id: pc.id,
                name: pc.name,
                input,
            });
        }
        let reason = match state.seen_finish.as_deref() {
            Some("stop") => StopReason::EndTurn,
            Some("length") => StopReason::MaxTokens,
            Some("tool_calls") => StopReason::Tool,
            Some(other) => StopReason::Other(other.to_string()),
            None => StopReason::EndTurn,
        };
        let usage = if state.cumulative_usage.total_tokens > 0 {
            Some(Usage {
                prompt: state.cumulative_usage.prompt_tokens,
                prompt_cached: 0,
                completion: state.cumulative_usage.completion_tokens,
            })
        } else {
            None
        };
        state.pending_emit.push(ChatDelta::Stop { reason, usage });
        state.closed = true;
        return None; // pending_emit will be drained by the outer loop
    }

    let parsed: serde_json::Value = match serde_json::from_str(&ev.data) {
        Ok(v) => v,
        Err(_) => return None,
    };

    // Usage may arrive on a final frame with `choices: []`.
    if let Some(u) = parsed.get("usage") {
        let usage: OpenAIUsage = serde_json::from_value(u.clone()).unwrap_or_default();
        state.cumulative_usage = usage;
    }

    let choices = parsed.get("choices").and_then(|v| v.as_array())?;
    let choice = choices.first()?;
    if let Some(reason) = choice.get("finish_reason").and_then(|v| v.as_str()) {
        if !reason.is_empty() {
            state.seen_finish = Some(reason.to_string());
        }
    }
    let delta = choice.get("delta")?;

    // Text content
    if let Some(content) = delta.get("content").and_then(|v| v.as_str()) {
        if !content.is_empty() {
            return Some(Ok(ChatDelta::Text(content.to_string())));
        }
    }

    // Reasoning summary (o-series). Field name: `reasoning_content` (legacy)
    // or `reasoning.content` (newer Responses-style on /chat/completions).
    if let Some(r) = delta.get("reasoning_content").and_then(|v| v.as_str()) {
        if !r.is_empty() {
            return Some(Ok(ChatDelta::Thinking(r.to_string())));
        }
    }

    // Tool call deltas
    if let Some(tcs) = delta.get("tool_calls").and_then(|v| v.as_array()) {
        for tc in tcs {
            let idx = tc.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
            let entry = state.pending.entry(idx).or_default();
            if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                if !id.is_empty() && entry.id.is_empty() {
                    entry.id = id.to_string();
                }
            }
            if let Some(func) = tc.get("function") {
                if let Some(name) = func.get("name").and_then(|v| v.as_str()) {
                    if !name.is_empty() && entry.name.is_empty() {
                        entry.name = name.to_string();
                    }
                }
                if let Some(args) = func.get("arguments").and_then(|v| v.as_str()) {
                    entry.arguments.push_str(args);
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use operon_core::secrets::MockSecretStore;
    use operon_core::traits::ToolDef;
    use uuid::Uuid;

    fn mock_secrets() -> Arc<dyn SecretStore> {
        Arc::new(MockSecretStore::new())
    }

    #[test]
    fn ollama_factory_sets_local_url() {
        let cfg = OpenAIConfig::ollama("qwen2.5-coder:32b");
        assert!(cfg.api_url.contains("localhost:11434"));
        assert_eq!(cfg.model, "qwen2.5-coder:32b");
        assert!(!cfg.require_api_key);
    }

    #[test]
    fn vllm_factory_uses_provided_url() {
        let cfg = OpenAIConfig::vllm("http://gpu-node:8000/v1", "Llama-3.1-70B");
        assert_eq!(cfg.api_url, "http://gpu-node:8000/v1");
        assert!(!cfg.require_api_key);
    }

    #[test]
    fn build_body_translates_user_message() {
        let plugin = OpenAIChatPlugin::new(OpenAIConfig::default(), mock_secrets()).unwrap();
        let req = ChatRequest {
            system: None,
            messages: vec![Message {
                id: Uuid::new_v4(),
                role: Role::User,
                content: vec![ContentBlock::Text("hi".into())],
                created_at_ms: 0,
                session: Uuid::new_v4(),
                metadata: Default::default(),
            }],
            tools: vec![],
            model: None,
            max_tokens: None,
        };
        let body = plugin.build_body(&req);
        let messages = body.get("messages").and_then(|v| v.as_array()).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].get("role").and_then(|v| v.as_str()), Some("user"));
        assert_eq!(messages[0].get("content").and_then(|v| v.as_str()), Some("hi"));
        assert!(body.get("stream").and_then(|v| v.as_bool()).unwrap());
    }

    #[test]
    fn build_body_translates_system_prompt() {
        let plugin = OpenAIChatPlugin::new(OpenAIConfig::default(), mock_secrets()).unwrap();
        let req = ChatRequest {
            system: Some("you are helpful".into()),
            messages: vec![],
            tools: vec![],
            model: Some("gpt-4o-mini".into()),
            max_tokens: Some(100),
        };
        let body = plugin.build_body(&req);
        let messages = body.get("messages").and_then(|v| v.as_array()).unwrap();
        assert_eq!(messages[0].get("role").and_then(|v| v.as_str()), Some("system"));
        assert_eq!(body.get("model").and_then(|v| v.as_str()), Some("gpt-4o-mini"));
        assert_eq!(body.get("max_tokens").and_then(|v| v.as_u64()), Some(100));
    }

    #[test]
    fn build_body_translates_tools() {
        let plugin = OpenAIChatPlugin::new(OpenAIConfig::default(), mock_secrets()).unwrap();
        let req = ChatRequest {
            system: None,
            messages: vec![],
            tools: vec![ToolDef {
                name: "read".into(),
                description: "read a file".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }],
            model: None,
            max_tokens: None,
        };
        let body = plugin.build_body(&req);
        let tools = body.get("tools").and_then(|v| v.as_array()).unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].get("type").and_then(|v| v.as_str()), Some("function"));
        let func = tools[0].get("function").unwrap();
        assert_eq!(func.get("name").and_then(|v| v.as_str()), Some("read"));
    }

    #[test]
    fn build_body_translates_assistant_with_tool_calls() {
        let plugin = OpenAIChatPlugin::new(OpenAIConfig::default(), mock_secrets()).unwrap();
        let req = ChatRequest {
            system: None,
            messages: vec![Message {
                id: Uuid::new_v4(),
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Text("calling".into()),
                    ContentBlock::ToolUse {
                        id: "call_42".into(),
                        name: "read".into(),
                        input: serde_json::json!({"path": "/tmp/x"}),
                    },
                ],
                created_at_ms: 0,
                session: Uuid::new_v4(),
                metadata: Default::default(),
            }],
            tools: vec![],
            model: None,
            max_tokens: None,
        };
        let body = plugin.build_body(&req);
        let messages = body.get("messages").and_then(|v| v.as_array()).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].get("role").and_then(|v| v.as_str()), Some("assistant"));
        let tcs = messages[0].get("tool_calls").and_then(|v| v.as_array()).unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].get("id").and_then(|v| v.as_str()), Some("call_42"));
        assert_eq!(
            tcs[0].get("function").and_then(|f| f.get("name")).and_then(|v| v.as_str()),
            Some("read")
        );
    }

    #[test]
    fn build_body_translates_tool_role_messages() {
        let plugin = OpenAIChatPlugin::new(OpenAIConfig::default(), mock_secrets()).unwrap();
        let req = ChatRequest {
            system: None,
            messages: vec![Message {
                id: Uuid::new_v4(),
                role: Role::Tool,
                content: vec![
                    ContentBlock::ToolResult {
                        tool_use_id: "call_42".into(),
                        content: "{\"ok\":true}".into(),
                        is_error: false,
                    },
                    ContentBlock::ToolResult {
                        tool_use_id: "call_43".into(),
                        content: "{\"ok\":false}".into(),
                        is_error: true,
                    },
                ],
                created_at_ms: 0,
                session: Uuid::new_v4(),
                metadata: Default::default(),
            }],
            tools: vec![],
            model: None,
            max_tokens: None,
        };
        let body = plugin.build_body(&req);
        let messages = body.get("messages").and_then(|v| v.as_array()).unwrap();
        assert_eq!(messages.len(), 2, "one role:tool message per ToolResult");
        assert_eq!(messages[0].get("role").and_then(|v| v.as_str()), Some("tool"));
        assert_eq!(
            messages[0].get("tool_call_id").and_then(|v| v.as_str()),
            Some("call_42")
        );
    }
}
