//! MockChatPlugin — replays a recorded transcript so tests don't need a real LLM key.
//!
//! Two constructors:
//! - `from_deltas(Vec<Vec<ChatDelta>>)` — inline scripted turns
//! - `from_jsonl(path)` — reads a `.jsonl` file with one ChatDelta per line, blank lines
//!   separating turns.

use crate::agent::error::{OperonError, OperonResult};
use crate::agent::traits::{
    CancellationToken, Capabilities, ChatDelta, ChatPlugin, ChatRequest, ChatStream, Plugin,
};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;

pub struct MockChatPlugin {
    name: String,
    transcript: Mutex<VecDeque<Vec<ChatDelta>>>,
}

impl MockChatPlugin {
    pub fn from_deltas(name: impl Into<String>, turns: Vec<Vec<ChatDelta>>) -> Self {
        Self {
            name: name.into(),
            transcript: Mutex::new(VecDeque::from(turns)),
        }
    }

    pub fn from_jsonl(name: impl Into<String>, path: impl AsRef<Path>) -> OperonResult<Self> {
        let body = std::fs::read_to_string(path.as_ref())?;
        let mut turns: Vec<Vec<ChatDelta>> = Vec::new();
        let mut current: Vec<ChatDelta> = Vec::new();
        for line in body.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                if !current.is_empty() {
                    turns.push(std::mem::take(&mut current));
                }
                continue;
            }
            let d: ChatDelta = serde_json::from_str(trimmed).map_err(|e| {
                OperonError::Provider {
                    provider: "mock".into(),
                    message: format!("bad jsonl line: {e}"),
                    retryable: false,
                }
            })?;
            current.push(d);
        }
        if !current.is_empty() {
            turns.push(current);
        }
        Ok(Self::from_deltas(name, turns))
    }
}

#[async_trait]
impl Plugin for MockChatPlugin {
    fn name(&self) -> &str {
        &self.name
    }
    fn version(&self) -> &str {
        "0.1.0"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::STREAMING | Capabilities::TOOL_USE
    }
}

#[async_trait]
impl ChatPlugin for MockChatPlugin {
    async fn complete(
        &self,
        _req: ChatRequest,
        _ct: CancellationToken,
    ) -> OperonResult<ChatStream> {
        let next = self
            .transcript
            .lock()
            .map_err(|_| OperonError::Provider {
                provider: "mock".into(),
                message: "lock poisoned".into(),
                retryable: false,
            })?
            .pop_front()
            .ok_or_else(|| OperonError::Provider {
                provider: "mock".into(),
                message: "transcript exhausted".into(),
                retryable: false,
            })?;
        Ok(Box::pin(futures::stream::iter(next.into_iter().map(Ok))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::traits::{StopReason, Usage};
    use futures::StreamExt;

    #[tokio::test]
    async fn from_deltas_pops_in_order() {
        let p = MockChatPlugin::from_deltas(
            "m",
            vec![
                vec![ChatDelta::Text("a".into())],
                vec![ChatDelta::Stop {
                    reason: StopReason::EndTurn,
                    usage: Some(Usage::default()),
                }],
            ],
        );
        let req = ChatRequest {
            system: None,
            messages: vec![],
            tools: vec![],
            model: None,
            max_tokens: None,
        };
        let mut s1 = p.complete(req.clone(), CancellationToken::new()).await.unwrap();
        let mut count1 = 0;
        while let Some(_) = s1.next().await {
            count1 += 1;
        }
        let mut s2 = p.complete(req.clone(), CancellationToken::new()).await.unwrap();
        let mut count2 = 0;
        while let Some(_) = s2.next().await {
            count2 += 1;
        }
        let r3 = p.complete(req, CancellationToken::new()).await;
        assert_eq!(count1, 1);
        assert_eq!(count2, 1);
        assert!(matches!(r3, Err(OperonError::Provider { .. })));
    }

    #[tokio::test]
    async fn from_jsonl_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.jsonl");
        std::fs::write(
            &p,
            r#"{"Text":"hello"}
{"Stop":{"reason":"EndTurn","usage":{"prompt":1,"prompt_cached":0,"completion":1}}}
"#,
        )
        .unwrap();
        let plugin = MockChatPlugin::from_jsonl("m", &p).unwrap();
        let req = ChatRequest {
            system: None,
            messages: vec![],
            tools: vec![],
            model: None,
            max_tokens: None,
        };
        let mut s = plugin.complete(req, CancellationToken::new()).await.unwrap();
        let mut deltas = vec![];
        while let Some(d) = s.next().await {
            deltas.push(d.unwrap());
        }
        assert_eq!(deltas.len(), 2);
    }
}
