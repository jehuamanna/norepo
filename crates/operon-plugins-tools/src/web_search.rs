//! `web_search` tool — Tavily-backed web search.
//!
//! Reads the API key from `TAVILY_API_KEY` env var (Slice A4b will source it
//! from the SecretStore). Returns up to `max_results` hits as
//! `[{ title, url, snippet }]`.

use operon_core::error::{OperonError, OperonResult};
use operon_core::traits::{
    CancellationToken, Capabilities, Plugin, ToolDef, ToolPlugin,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

const TAVILY_ENDPOINT: &str = "https://api.tavily.com/search";

#[derive(Deserialize)]
struct WebSearchInput {
    query: String,
    #[serde(default)]
    max_results: Option<usize>,
}

pub struct WebSearchTool {
    client: reqwest::Client,
}

impl WebSearchTool {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .build()
                .expect("reqwest client build"),
        }
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Plugin for WebSearchTool {
    fn name(&self) -> &str { "web_search" }
    fn version(&self) -> &str { env!("CARGO_PKG_VERSION") }
    fn capabilities(&self) -> Capabilities { Capabilities::empty() }
}

#[async_trait]
impl ToolPlugin for WebSearchTool {
    fn schema(&self) -> ToolDef {
        ToolDef {
            name: "web_search".into(),
            description: "Search the web. Returns up to N results with title, snippet, and URL."
                          .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query":       { "type": "string" },
                    "max_results": { "type": "integer", "minimum": 1, "maximum": 20, "default": 5 }
                },
                "required": ["query"]
            }),
        }
    }

    async fn invoke(
        &self,
        args: serde_json::Value,
        _ct: CancellationToken,
    ) -> OperonResult<serde_json::Value> {
        let input: WebSearchInput = serde_json::from_value(args).map_err(|e| OperonError::Plugin {
            plugin: "web_search".into(),
            source: Box::new(e),
        })?;
        let api_key = match std::env::var("TAVILY_API_KEY") {
            Ok(k) => k,
            Err(_) => {
                return Ok(json!({
                    "error": "TAVILY_API_KEY not set; configure in settings (Slice A4b) or env",
                }));
            }
        };
        let max_results = input.max_results.unwrap_or(5).clamp(1, 20);
        let body = json!({
            "api_key": api_key,
            "query": input.query,
            "max_results": max_results,
            "search_depth": "basic",
        });
        let resp = self
            .client
            .post(TAVILY_ENDPOINT)
            .json(&body)
            .send()
            .await
            .map_err(|e| OperonError::Plugin {
                plugin: "web_search".into(),
                source: Box::new(std::io::Error::other(format!("tavily http: {e}"))),
            })?;
        if !resp.status().is_success() {
            let status = resp.status();
            let txt = resp.text().await.unwrap_or_default();
            return Ok(json!({ "error": format!("tavily http {status}: {txt}") }));
        }
        let body: serde_json::Value = resp.json().await.map_err(|e| OperonError::Plugin {
            plugin: "web_search".into(),
            source: Box::new(std::io::Error::other(format!("tavily decode: {e}"))),
        })?;
        // Tavily returns { results: [{title, url, content, ...}, ...] }
        let results = body
            .get("results")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|r| {
                        json!({
                            "title": r.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                            "url":   r.get("url").and_then(|v| v.as_str()).unwrap_or(""),
                            "snippet": r.get("content").and_then(|v| v.as_str()).unwrap_or(""),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(json!({
            "query": input.query,
            "results": results,
            "count": results.len(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn schema_is_well_formed() {
        let s = WebSearchTool::default().schema();
        assert_eq!(s.name, "web_search");
    }

    #[tokio::test]
    async fn missing_api_key_returns_clean_error() {
        // Ensure key is unset for this test.
        std::env::remove_var("TAVILY_API_KEY");
        let r = WebSearchTool::default()
            .invoke(json!({ "query": "test" }), CancellationToken::new())
            .await
            .unwrap();
        assert!(r.get("error").and_then(|v| v.as_str()).unwrap().contains("TAVILY_API_KEY"));
    }
}
