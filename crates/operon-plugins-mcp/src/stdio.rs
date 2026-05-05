//! StdioMcpClient — JSON-RPC over a subprocess's stdin/stdout.
//!
//! Frames messages as newline-delimited JSON (one JSON-RPC message per line). Many
//! MCP servers use this; the LSP-style `Content-Length` framing is not implemented
//! here (defer to a follow-up).

use operon_core::error::{OperonError, OperonResult};
use crate::grant::GrantHandler;
use operon_core::traits::{
    CancellationToken, Capabilities, McpClient, Plugin, ToolDef,
};
use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};

#[derive(Clone, Debug, Default)]
pub struct StdioMcpServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

pub struct StdioMcpClient {
    cfg: StdioMcpServerConfig,
    grants: Arc<dyn GrantHandler>,
    state: Mutex<Option<StdioState>>,
}

struct StdioState {
    child: Child,
    stdin: ChildStdin,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    next_id: u64,
    tools: Vec<ToolDef>,
}

impl StdioMcpClient {
    pub fn new(cfg: StdioMcpServerConfig, grants: Arc<dyn GrantHandler>) -> Self {
        Self {
            cfg,
            grants,
            state: Mutex::new(None),
        }
    }

    async fn send_request(&self, method: &str, params: serde_json::Value) -> OperonResult<serde_json::Value> {
        let mut g = self.state.lock().await;
        let st = g.as_mut().ok_or_else(|| OperonError::Mcp {
            server: self.cfg.name.clone(),
            message: "not connected".into(),
        })?;
        let id = st.next_id;
        st.next_id += 1;
        let (tx, rx) = oneshot::channel();
        st.pending.lock().await.insert(id, tx);
        let payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut line = serde_json::to_string(&payload).map_err(|e| OperonError::Mcp {
            server: self.cfg.name.clone(),
            message: format!("serialize: {e}"),
        })?;
        line.push('\n');
        st.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| OperonError::Mcp {
                server: self.cfg.name.clone(),
                message: format!("write: {e}"),
            })?;
        st.stdin.flush().await.ok();
        drop(g);
        match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(_)) => Err(OperonError::Mcp {
                server: self.cfg.name.clone(),
                message: "response sender dropped".into(),
            }),
            Err(_) => Err(OperonError::Mcp {
                server: self.cfg.name.clone(),
                message: "timeout".into(),
            }),
        }
    }
}

#[async_trait]
impl Plugin for StdioMcpClient {
    fn name(&self) -> &str {
        &self.cfg.name
    }
    fn version(&self) -> &str {
        "0.1.0"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::TOOL_USE
    }
}

#[async_trait]
impl McpClient for StdioMcpClient {
    async fn connect(&self) -> OperonResult<()> {
        let mut child = Command::new(&self.cfg.command)
            .args(&self.cfg.args)
            .envs(&self.cfg.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| OperonError::Mcp {
                server: self.cfg.name.clone(),
                message: format!("spawn: {e}"),
            })?;
        let stdin = child.stdin.take().ok_or_else(|| OperonError::Mcp {
            server: self.cfg.name.clone(),
            message: "no stdin".into(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| OperonError::Mcp {
            server: self.cfg.name.clone(),
            message: "no stdout".into(),
        })?;
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_reader = pending.clone();
        let server_name = self.cfg.name.clone();

        // Reader task: parse lines, demux by id.
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        let v: serde_json::Value = match serde_json::from_str(line.trim()) {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::warn!(target: "operon::mcp", server = %server_name, line = %line.trim(), error = %e, "bad json from mcp server");
                                continue;
                            }
                        };
                        if let Some(id) = v.get("id").and_then(|i| i.as_u64()) {
                            if let Some(tx) = pending_reader.lock().await.remove(&id) {
                                let _ = tx.send(v);
                            }
                        }
                        // Notifications (no id) silently ignored for now.
                    }
                    Err(_) => break,
                }
            }
        });

        let mut state = StdioState {
            child,
            stdin,
            pending,
            next_id: 1,
            tools: Vec::new(),
        };

        // Send initialize then tools/list.
        // We'll do initialize directly by writing then awaiting.
        let init_id = state.next_id;
        state.next_id += 1;
        let (init_tx, init_rx) = oneshot::channel();
        state.pending.lock().await.insert(init_id, init_tx);
        let init_payload = json!({
            "jsonrpc": "2.0",
            "id": init_id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "operon", "version": "0.1.0" },
            }
        });
        let mut init_line = serde_json::to_string(&init_payload).unwrap();
        init_line.push('\n');
        state
            .stdin
            .write_all(init_line.as_bytes())
            .await
            .map_err(|e| OperonError::Mcp {
                server: self.cfg.name.clone(),
                message: format!("init write: {e}"),
            })?;
        state.stdin.flush().await.ok();

        let _init_resp = match tokio::time::timeout(std::time::Duration::from_secs(10), init_rx).await {
            Ok(Ok(v)) => v,
            _ => {
                return Err(OperonError::Mcp {
                    server: self.cfg.name.clone(),
                    message: "initialize timed out".into(),
                });
            }
        };

        // Stash the state without tools first; list_tools will be called separately.
        *self.state.lock().await = Some(state);

        // Now request tools/list to populate the cache.
        let resp = self.send_request("tools/list", json!({})).await?;
        let tools_v = resp
            .get("result")
            .and_then(|r| r.get("tools"))
            .cloned()
            .unwrap_or(serde_json::Value::Array(vec![]));
        let tools: Vec<ToolDef> = serde_json::from_value(tools_v).unwrap_or_default();
        if let Some(st) = self.state.lock().await.as_mut() {
            st.tools = tools;
        }
        Ok(())
    }

    async fn list_tools(&self) -> OperonResult<Vec<ToolDef>> {
        let g = self.state.lock().await;
        Ok(g.as_ref().map(|s| s.tools.clone()).unwrap_or_default())
    }

    async fn call_tool(
        &self,
        name: &str,
        args: serde_json::Value,
        ct: CancellationToken,
    ) -> OperonResult<serde_json::Value> {
        if !self.grants.check(&self.cfg.name, name).await? {
            return Err(OperonError::Mcp {
                server: self.cfg.name.clone(),
                message: format!("grant denied for tool '{name}'"),
            });
        }
        if ct.is_cancelled() {
            return Err(OperonError::Cancelled);
        }
        let resp = tokio::select! {
            _ = ct.cancelled() => return Err(OperonError::Cancelled),
            r = self.send_request("tools/call", json!({
                "name": name,
                "arguments": args,
            })) => r?,
        };
        if let Some(err) = resp.get("error") {
            return Err(OperonError::Mcp {
                server: self.cfg.name.clone(),
                message: err.to_string(),
            });
        }
        Ok(resp.get("result").cloned().unwrap_or(serde_json::Value::Null))
    }

    async fn disconnect(&self) -> OperonResult<()> {
        if let Some(mut st) = self.state.lock().await.take() {
            let _ = st.stdin.shutdown().await;
            let _ = st.child.kill().await;
            let _ = st.child.wait().await;
        }
        Ok(())
    }
}
