//! `web_fetch` tool — fetch a URL, return content as Markdown.
//!
//! No external API. Uses `reqwest` + `html2md`. Caps response size at 4 MiB
//! and the converted Markdown at 1 MiB.

use operon_core::error::{OperonError, OperonResult};
use operon_core::traits::{
    CancellationToken, Capabilities, Plugin, ToolDef, ToolPlugin,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_MARKDOWN_BYTES: usize = 1024 * 1024;

#[derive(Deserialize)]
struct WebFetchInput {
    url: String,
    #[serde(default)]
    excerpt: Option<bool>,
}

pub struct WebFetchTool {
    client: reqwest::Client,
}

impl WebFetchTool {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("operon-plugins-tools/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client build");
        Self { client }
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Plugin for WebFetchTool {
    fn name(&self) -> &str { "web_fetch" }
    fn version(&self) -> &str { env!("CARGO_PKG_VERSION") }
    fn capabilities(&self) -> Capabilities { Capabilities::empty() }
}

#[async_trait]
impl ToolPlugin for WebFetchTool {
    fn schema(&self) -> ToolDef {
        ToolDef {
            name: "web_fetch".into(),
            description: "Fetch a URL. Returns content as Markdown (capped at 1 MiB)."
                          .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url":     { "type": "string", "description": "Absolute URL (http or https)." },
                    "excerpt": { "type": "boolean", "default": true,
                                 "description": "Reserved for future use; today we always return the full Markdown conversion." }
                },
                "required": ["url"]
            }),
        }
    }

    async fn invoke(
        &self,
        args: serde_json::Value,
        _ct: CancellationToken,
    ) -> OperonResult<serde_json::Value> {
        let input: WebFetchInput = serde_json::from_value(args).map_err(|e| OperonError::Plugin {
            plugin: "web_fetch".into(),
            source: Box::new(e),
        })?;
        let _ = input.excerpt; // reserved
        if !input.url.starts_with("http://") && !input.url.starts_with("https://") {
            return Ok(json!({ "error": "url must start with http:// or https://" }));
        }

        let resp = self.client.get(&input.url).send().await.map_err(|e| OperonError::Plugin {
            plugin: "web_fetch".into(),
            source: Box::new(std::io::Error::other(format!("get: {e}"))),
        })?;
        let status = resp.status();
        if !status.is_success() {
            return Ok(json!({ "error": format!("http {status}"), "url": input.url }));
        }
        let ctype = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let bytes = resp.bytes().await.map_err(|e| OperonError::Plugin {
            plugin: "web_fetch".into(),
            source: Box::new(std::io::Error::other(format!("read body: {e}"))),
        })?;
        if bytes.len() > MAX_BYTES {
            return Ok(json!({
                "error": "response exceeds 4 MiB cap",
                "size": bytes.len(),
            }));
        }
        let text = String::from_utf8_lossy(&bytes).to_string();

        let markdown = if ctype.contains("text/html") || ctype.contains("application/xhtml") || (ctype.is_empty() && looks_like_html(&text)) {
            html2md::parse_html(&text)
        } else {
            text.clone()
        };
        let truncated = markdown.len() > MAX_MARKDOWN_BYTES;
        let trimmed = if truncated {
            markdown.chars().take(MAX_MARKDOWN_BYTES).collect()
        } else {
            markdown
        };

        Ok(json!({
            "url": input.url,
            "content_type": ctype,
            "markdown": trimmed,
            "bytes": bytes.len(),
            "truncated": truncated,
        }))
    }
}

fn looks_like_html(s: &str) -> bool {
    let lower = s.to_lowercase();
    lower.contains("<html") || lower.contains("<!doctype html")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn schema_is_well_formed() {
        let s = WebFetchTool::default().schema();
        assert_eq!(s.name, "web_fetch");
    }

    #[tokio::test]
    async fn rejects_non_http_url() {
        let r = WebFetchTool::default()
            .invoke(json!({ "url": "ftp://example.com" }), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(r.get("error").and_then(|v| v.as_str()), Some("url must start with http:// or https://"));
    }

    #[test]
    fn looks_like_html_recognises_doctype() {
        assert!(looks_like_html("<!doctype html><html>"));
        assert!(looks_like_html("<HTML><body></body></HTML>"));
        assert!(!looks_like_html("plain text without markup"));
    }
}
