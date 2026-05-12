//! LSP client over stdio.
//!
//! Spawns a language server, frames JSON-RPC with `Content-Length` headers,
//! drives the `initialize` handshake, and exposes synchronous request methods
//! for `textDocument/definition`, `textDocument/hover`, etc.

use crate::codec::{encode, try_decode};
use bytes::BytesMut;
use operon_core::error::{OperonError, OperonResult};
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};
use tokio::time::timeout;

#[derive(Clone, Debug)]
pub struct LspServerConfig {
    /// Stable id for this client (e.g. `"rust-analyzer"`, `"pyright"`).
    pub id: String,
    /// Binary to spawn.
    pub command: String,
    pub args: Vec<String>,
    /// Project root; passed in `initialize`'s `rootUri` as `file://...`.
    pub root: PathBuf,
}

pub struct LspClient {
    cfg: LspServerConfig,
    state: Mutex<Option<LspState>>,
}

struct LspState {
    child: Child,
    stdin: ChildStdin,
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<serde_json::Value>>>>,
    next_id: i64,
}

impl LspClient {
    pub fn new(cfg: LspServerConfig) -> Self {
        Self {
            cfg,
            state: Mutex::new(None),
        }
    }

    pub fn id(&self) -> &str {
        &self.cfg.id
    }

    /// Spawn the server, drive `initialize`, and emit `initialized`.
    pub async fn connect(&self) -> OperonResult<()> {
        let mut child = Command::new(&self.cfg.command)
            .args(&self.cfg.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| OperonError::Plugin {
                plugin: format!("lsp/{}", self.cfg.id),
                source: Box::new(std::io::Error::other(format!("spawn: {e}"))),
            })?;
        let stdin = child.stdin.take().ok_or_else(|| OperonError::Plugin {
            plugin: format!("lsp/{}", self.cfg.id),
            source: Box::new(std::io::Error::other("no stdin")),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| OperonError::Plugin {
            plugin: format!("lsp/{}", self.cfg.id),
            source: Box::new(std::io::Error::other("no stdout")),
        })?;

        let pending: Arc<Mutex<HashMap<i64, oneshot::Sender<serde_json::Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_reader = pending.clone();
        let id_for_log = self.cfg.id.clone();
        tokio::spawn(async move {
            let mut reader = stdout;
            let mut buf = BytesMut::with_capacity(8192);
            let mut chunk = [0u8; 4096];
            loop {
                let n = match reader.read(&mut chunk).await {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(_) => break,
                };
                buf.extend_from_slice(&chunk[..n]);
                loop {
                    match try_decode(&mut buf) {
                        Ok(Some(msg)) => {
                            if let Some(id_v) = msg.get("id") {
                                if let Some(id) = id_v.as_i64() {
                                    if let Some(tx) = pending_reader.lock().await.remove(&id) {
                                        let _ = tx.send(msg);
                                        continue;
                                    }
                                }
                            }
                            // Notification or unmatched response → log + drop.
                            tracing::trace!(target: "operon::lsp", server = %id_for_log, "notification or unmatched: {msg}");
                        }
                        Ok(None) => break,
                        Err(e) => {
                            tracing::warn!(target: "operon::lsp", server = %id_for_log, error = %e, "decode error; resyncing");
                            buf.clear();
                            break;
                        }
                    }
                }
            }
        });

        let mut state = LspState {
            child,
            stdin,
            pending,
            next_id: 1,
        };

        // initialize
        let root_uri = format!("file://{}", self.cfg.root.display());
        let init_params = json!({
            "processId": std::process::id(),
            "clientInfo": { "name": "operon", "version": env!("CARGO_PKG_VERSION") },
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": {
                    "definition": { "dynamicRegistration": false },
                    "hover":      { "dynamicRegistration": false, "contentFormat": ["plaintext", "markdown"] },
                    "documentSymbol": { "dynamicRegistration": false },
                    "references": { "dynamicRegistration": false },
                    "publishDiagnostics": { "relatedInformation": true }
                }
            }
        });
        let _resp = send_request(&mut state, "initialize", init_params)
            .await
            .map_err(|e| wrap(self, "initialize", e))?;
        send_notification(&mut state, "initialized", json!({}))
            .await
            .map_err(|e| wrap(self, "initialized", e))?;

        *self.state.lock().await = Some(state);
        Ok(())
    }

    /// Tell the server we've opened a text document.
    pub async fn did_open(&self, path: &std::path::Path, language_id: &str, text: &str) -> OperonResult<()> {
        let mut g = self.state.lock().await;
        let st = g.as_mut().ok_or_else(|| OperonError::Plugin {
            plugin: format!("lsp/{}", self.cfg.id),
            source: Box::new(std::io::Error::other("not connected")),
        })?;
        let uri = format!("file://{}", path.display());
        send_notification(
            st,
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": 1,
                    "text": text,
                }
            }),
        )
        .await
        .map_err(|e| wrap(self, "didOpen", e))
    }

    pub async fn definition(
        &self,
        path: &std::path::Path,
        line: u32,
        character: u32,
    ) -> OperonResult<serde_json::Value> {
        self.text_document_request(
            "textDocument/definition",
            path,
            line,
            character,
        )
        .await
    }

    pub async fn hover(
        &self,
        path: &std::path::Path,
        line: u32,
        character: u32,
    ) -> OperonResult<serde_json::Value> {
        self.text_document_request("textDocument/hover", path, line, character)
            .await
    }

    pub async fn document_symbol(
        &self,
        path: &std::path::Path,
    ) -> OperonResult<serde_json::Value> {
        let mut g = self.state.lock().await;
        let st = g.as_mut().ok_or_else(|| OperonError::Plugin {
            plugin: format!("lsp/{}", self.cfg.id),
            source: Box::new(std::io::Error::other("not connected")),
        })?;
        let uri = format!("file://{}", path.display());
        send_request(
            st,
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": uri } }),
        )
        .await
        .map_err(|e| wrap(self, "documentSymbol", e))
    }

    pub async fn references(
        &self,
        path: &std::path::Path,
        line: u32,
        character: u32,
    ) -> OperonResult<serde_json::Value> {
        let mut g = self.state.lock().await;
        let st = g.as_mut().ok_or_else(|| OperonError::Plugin {
            plugin: format!("lsp/{}", self.cfg.id),
            source: Box::new(std::io::Error::other("not connected")),
        })?;
        let uri = format!("file://{}", path.display());
        send_request(
            st,
            "textDocument/references",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character },
                "context": { "includeDeclaration": true }
            }),
        )
        .await
        .map_err(|e| wrap(self, "references", e))
    }

    async fn text_document_request(
        &self,
        method: &str,
        path: &std::path::Path,
        line: u32,
        character: u32,
    ) -> OperonResult<serde_json::Value> {
        let mut g = self.state.lock().await;
        let st = g.as_mut().ok_or_else(|| OperonError::Plugin {
            plugin: format!("lsp/{}", self.cfg.id),
            source: Box::new(std::io::Error::other("not connected")),
        })?;
        let uri = format!("file://{}", path.display());
        send_request(
            st,
            method,
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character }
            }),
        )
        .await
        .map_err(|e| wrap(self, method, e))
    }

    pub async fn shutdown(&self) -> OperonResult<()> {
        let mut g = self.state.lock().await;
        if let Some(mut st) = g.take() {
            // Best-effort `shutdown` then `exit`. Don't wait long; if the server is
            // wedged we kill it.
            let _ = timeout(Duration::from_secs(3), send_request(&mut st, "shutdown", json!(null))).await;
            let _ = send_notification(&mut st, "exit", json!(null)).await;
            let _ = st.stdin.shutdown().await;
            let _ = st.child.kill().await;
            let _ = st.child.wait().await;
        }
        Ok(())
    }
}

fn wrap(client: &LspClient, ctx: &str, e: OperonError) -> OperonError {
    match e {
        OperonError::Plugin { source, .. } => OperonError::Plugin {
            plugin: format!("lsp/{}/{ctx}", client.cfg.id),
            source,
        },
        other => other,
    }
}

async fn send_request(
    state: &mut LspState,
    method: &str,
    params: serde_json::Value,
) -> OperonResult<serde_json::Value> {
    let id = state.next_id;
    state.next_id += 1;
    let (tx, rx) = oneshot::channel();
    state.pending.lock().await.insert(id, tx);
    let payload = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    let frame = encode(&payload);
    state
        .stdin
        .write_all(&frame)
        .await
        .map_err(|e| OperonError::Plugin {
            plugin: "lsp".into(),
            source: Box::new(std::io::Error::other(format!("write: {e}"))),
        })?;
    state.stdin.flush().await.ok();
    match timeout(Duration::from_secs(15), rx).await {
        Ok(Ok(v)) => {
            if let Some(err) = v.get("error") {
                Err(OperonError::Plugin {
                    plugin: "lsp".into(),
                    source: Box::new(std::io::Error::other(format!("server error: {err}"))),
                })
            } else {
                Ok(v.get("result").cloned().unwrap_or(serde_json::Value::Null))
            }
        }
        Ok(Err(_)) => Err(OperonError::Plugin {
            plugin: "lsp".into(),
            source: Box::new(std::io::Error::other("response sender dropped")),
        }),
        Err(_) => Err(OperonError::Plugin {
            plugin: "lsp".into(),
            source: Box::new(std::io::Error::other("timeout")),
        }),
    }
}

async fn send_notification(
    state: &mut LspState,
    method: &str,
    params: serde_json::Value,
) -> OperonResult<()> {
    let payload = json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    });
    let frame = encode(&payload);
    state
        .stdin
        .write_all(&frame)
        .await
        .map_err(|e| OperonError::Plugin {
            plugin: "lsp".into(),
            source: Box::new(std::io::Error::other(format!("write: {e}"))),
        })?;
    state.stdin.flush().await.ok();
    Ok(())
}
