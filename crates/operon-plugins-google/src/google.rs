//! Google Gemini ChatPlugin.
//!
//! Streams via `streamGenerateContent?alt=sse`. Translates Gemini's
//! `function_call` / `function_response` shape to Operon's canonical
//! `ChatDelta::ToolUse` / `ContentBlock::ToolResult`.

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
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct GoogleConfig {
    pub api_url: String,
    pub model: String,
    pub max_tokens: u32,
    pub long_context: bool,
}

impl Default for GoogleConfig {
    fn default() -> Self {
        Self {
            api_url: "https://generativelanguage.googleapis.com/v1beta".to_string(),
            model: "gemini-2.0-flash".to_string(),
            max_tokens: 8192,
            long_context: false,
        }
    }
}

pub struct GoogleChatPlugin {
    cfg: GoogleConfig,
    secrets: Arc<dyn SecretStore>,
    client: reqwest::Client,
    name: String,
}

impl GoogleChatPlugin {
    pub fn new(cfg: GoogleConfig, secrets: Arc<dyn SecretStore>) -> OperonResult<Self> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| OperonError::Provider {
                provider: "google".into(),
                message: format!("client build: {e}"),
                retryable: false,
            })?;
        Ok(Self {
            cfg,
            secrets,
            client,
            name: "google".to_string(),
        })
    }

    pub fn config(&self) -> &GoogleConfig {
        &self.cfg
    }

    async fn api_key(&self) -> OperonResult<String> {
        if let Some(k) = self.secrets.get(secret_keys::GOOGLE_API_KEY).await? {
            return Ok(k);
        }
        std::env::var("GOOGLE_API_KEY").map_err(|_| {
            OperonError::Secret(format!(
                "google api key missing ({} in SecretStore or GOOGLE_API_KEY env)",
                secret_keys::GOOGLE_API_KEY
            ))
        })
    }

    fn build_body(&self, req: &ChatRequest) -> serde_json::Value {
        let max_tokens = req.max_tokens.unwrap_or(self.cfg.max_tokens);
        let mut contents: Vec<serde_json::Value> = Vec::new();
        for m in &req.messages {
            if let Some(c) = message_to_gemini(m) {
                contents.push(c);
            }
        }
        let mut body = serde_json::json!({
            "contents": contents,
            "generationConfig": {
                "maxOutputTokens": max_tokens,
            }
        });
        if let Some(s) = req.system.as_deref() {
            body["systemInstruction"] = serde_json::json!({ "parts": [{ "text": s }] });
        }
        if !req.tools.is_empty() {
            let decls: Vec<serde_json::Value> = req
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    })
                })
                .collect();
            body["tools"] = serde_json::json!([{ "function_declarations": decls }]);
        }
        body
    }
}

fn message_to_gemini(m: &Message) -> Option<serde_json::Value> {
    let role = match m.role {
        Role::User => "user",
        Role::Assistant => "model",
        Role::Tool => "user", // Gemini routes function-responses through user role
        Role::System => return None,
    };
    let parts: Vec<serde_json::Value> = m
        .content
        .iter()
        .filter_map(|cb| match cb {
            ContentBlock::Text(t) => Some(serde_json::json!({ "text": t })),
            ContentBlock::ToolUse { id: _, name, input } => Some(serde_json::json!({
                "functionCall": {
                    "name": name,
                    "args": input,
                }
            })),
            ContentBlock::ToolResult {
                tool_use_id: _,
                content,
                is_error: _,
            } => {
                let response: serde_json::Value =
                    serde_json::from_str(content).unwrap_or(serde_json::json!({ "raw": content }));
                Some(serde_json::json!({
                    "functionResponse": {
                        "name": "tool",
                        "response": response,
                    }
                }))
            }
        })
        .collect();
    if parts.is_empty() {
        return None;
    }
    Some(serde_json::json!({ "role": role, "parts": parts }))
}

#[async_trait]
impl Plugin for GoogleChatPlugin {
    fn name(&self) -> &str { &self.name }
    fn version(&self) -> &str { env!("CARGO_PKG_VERSION") }
    fn capabilities(&self) -> Capabilities {
        Capabilities::STREAMING | Capabilities::TOOL_USE | Capabilities::VISION
    }
}

#[async_trait]
impl ChatPlugin for GoogleChatPlugin {
    async fn complete(&self, req: ChatRequest, ct: CancellationToken) -> OperonResult<ChatStream> {
        let api_key = self.api_key().await?;
        let model = req.model.clone().unwrap_or_else(|| self.cfg.model.clone());
        let body = self.build_body(&req);
        let url = format!(
            "{}/models/{}:streamGenerateContent?alt=sse&key={}",
            self.cfg.api_url, model, api_key
        );

        let resp = self
            .client
            .post(&url)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| OperonError::Provider {
                provider: "google".into(),
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
                provider: "google".into(),
                message: format!("http {status}: {text}"),
                retryable: status.as_u16() >= 500 || status.as_u16() == 429,
            });
        }

        let bytes_stream = resp
            .bytes_stream()
            .map(|r| r.map_err(|e| format!("body: {e}")));
        let sse = SseStream::new(bytes_stream);
        let stream = GoogleStream::new(sse, ct);
        Ok(Box::pin(stream))
    }
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
struct GoogleUsage {
    #[serde(rename = "promptTokenCount", default)]
    prompt: u64,
    #[serde(rename = "candidatesTokenCount", default)]
    completion: u64,
    #[serde(rename = "totalTokenCount", default)]
    total: u64,
}

struct GoogleStream<S> {
    inner: SseStream<S>,
    ct: CancellationToken,
    pending_emit: Vec<ChatDelta>,
    closed: bool,
    cumulative_usage: GoogleUsage,
    saw_finish: Option<String>,
}

impl<S> GoogleStream<S> {
    fn new(inner: SseStream<S>, ct: CancellationToken) -> Self {
        Self {
            inner,
            ct,
            pending_emit: Vec::new(),
            closed: false,
            cumulative_usage: GoogleUsage::default(),
            saw_finish: None,
        }
    }
}

impl<S, E> Stream for GoogleStream<S>
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
                    if let Some(d) = take_final_stop(me) {
                        return Poll::Ready(Some(Ok(d)));
                    }
                    return Poll::Ready(None);
                }
                Poll::Ready(Some(Err(e))) => {
                    me.closed = true;
                    return Poll::Ready(Some(Err(OperonError::Provider {
                        provider: "google".into(),
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

fn take_final_stop<S>(state: &mut GoogleStream<S>) -> Option<ChatDelta> {
    let reason = match state.saw_finish.as_deref() {
        Some("STOP") => StopReason::EndTurn,
        Some("MAX_TOKENS") => StopReason::MaxTokens,
        Some(other) => StopReason::Other(other.to_string()),
        None => StopReason::EndTurn,
    };
    let usage = if state.cumulative_usage.total > 0 {
        Some(Usage {
            prompt: state.cumulative_usage.prompt,
            prompt_cached: 0,
            completion: state.cumulative_usage.completion,
        })
    } else {
        None
    };
    Some(ChatDelta::Stop { reason, usage })
}

fn handle_event<S>(state: &mut GoogleStream<S>, ev: &SseEvent) -> Option<OperonResult<ChatDelta>> {
    let parsed: serde_json::Value = match serde_json::from_str(&ev.data) {
        Ok(v) => v,
        Err(_) => return None,
    };
    if let Some(u) = parsed.get("usageMetadata") {
        let usage: GoogleUsage = serde_json::from_value(u.clone()).unwrap_or_default();
        state.cumulative_usage = usage;
    }
    let candidates = parsed.get("candidates").and_then(|v| v.as_array())?;
    let cand = candidates.first()?;
    if let Some(reason) = cand.get("finishReason").and_then(|v| v.as_str()) {
        if !reason.is_empty() {
            state.saw_finish = Some(reason.to_string());
        }
    }
    let parts = cand
        .get("content")
        .and_then(|c| c.get("parts"))
        .and_then(|v| v.as_array())?;
    for part in parts {
        if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
            if !t.is_empty() {
                state.pending_emit.push(ChatDelta::Text(t.to_string()));
            }
        }
        if let Some(fc) = part.get("functionCall") {
            let name = fc.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let args = fc
                .get("args")
                .cloned()
                .unwrap_or(serde_json::Value::Object(Default::default()));
            // Gemini doesn't return a tool_call_id; synthesize one.
            let id = format!("call_{}", Uuid::new_v4().simple());
            state.pending_emit.push(ChatDelta::ToolUse { id, name, input: args });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use operon_core::secrets::MockSecretStore;
    use operon_core::traits::ToolDef;

    fn mock_secrets() -> Arc<dyn SecretStore> {
        Arc::new(MockSecretStore::new())
    }

    #[test]
    fn build_body_translates_user_message() {
        let plugin = GoogleChatPlugin::new(GoogleConfig::default(), mock_secrets()).unwrap();
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
        let contents = body.get("contents").and_then(|v| v.as_array()).unwrap();
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0].get("role").and_then(|v| v.as_str()), Some("user"));
    }

    #[test]
    fn build_body_translates_system_instruction() {
        let plugin = GoogleChatPlugin::new(GoogleConfig::default(), mock_secrets()).unwrap();
        let req = ChatRequest {
            system: Some("be helpful".into()),
            messages: vec![],
            tools: vec![],
            model: None,
            max_tokens: None,
        };
        let body = plugin.build_body(&req);
        assert!(body.get("systemInstruction").is_some());
    }

    #[test]
    fn build_body_translates_tools() {
        let plugin = GoogleChatPlugin::new(GoogleConfig::default(), mock_secrets()).unwrap();
        let req = ChatRequest {
            system: None,
            messages: vec![],
            tools: vec![ToolDef {
                name: "read".into(),
                description: "x".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }],
            model: None,
            max_tokens: None,
        };
        let body = plugin.build_body(&req);
        let tools = body.get("tools").and_then(|v| v.as_array()).unwrap();
        let decls = tools[0]
            .get("function_declarations")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].get("name").and_then(|v| v.as_str()), Some("read"));
    }

    #[test]
    fn assistant_with_tool_use_serialised_as_function_call() {
        let plugin = GoogleChatPlugin::new(GoogleConfig::default(), mock_secrets()).unwrap();
        let req = ChatRequest {
            system: None,
            messages: vec![Message {
                id: Uuid::new_v4(),
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "x".into(),
                    name: "read".into(),
                    input: serde_json::json!({"path": "/tmp/x"}),
                }],
                created_at_ms: 0,
                session: Uuid::new_v4(),
                metadata: Default::default(),
            }],
            tools: vec![],
            model: None,
            max_tokens: None,
        };
        let body = plugin.build_body(&req);
        let contents = body.get("contents").and_then(|v| v.as_array()).unwrap();
        let parts = contents[0].get("parts").and_then(|v| v.as_array()).unwrap();
        assert!(parts[0].get("functionCall").is_some());
    }
}
