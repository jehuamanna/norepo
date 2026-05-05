//! Operon agent runtime substrate (Plans-Phase-1).
//!
//! UI-agnostic by contract: this module and its descendants must not import
//! `dioxus` or anything from `crate::ui`/`crate::shell`/`crate::commands`/etc.
//! Pre-workspace-split, a `compile_fail` doc-test would still pass because
//! the parent crate depends on dioxus. The source-level check below greps
//! for forbidden imports; the real dependency-level guard lands in
//! Plans-Phase-4 with cargo-deny + post-split crate boundary.

pub mod budget;
pub mod bus;
pub mod config;
pub mod error;
pub mod mcp;
pub mod memory;
pub mod plugins;
pub mod registry;
#[cfg(not(target_arch = "wasm32"))]
pub mod runtime;
pub mod secrets;
pub mod session;
pub mod tracing_init;
pub mod traits;

pub use budget::Budget;
pub use bus::{BusEvent, EventBus};
pub use config::{
    AnthropicProviderConfig, BudgetConfig, McpConfig, McpServerConfig, MemoryConfig,
    OperonConfig, ProvidersConfig, RuntimeConfig,
};
pub use error::{OperonError, OperonResult};
pub use memory::InMemoryStore;
#[cfg(all(feature = "sqlite-memory", not(target_arch = "wasm32")))]
pub use memory::SqliteMemoryStore;
pub use registry::{register_agent_plugins, AgentRegistry};
pub use secrets::{EnvSecretStore, MockSecretStore, SecretStore};
pub use traits::{
    Capabilities, ChatDelta, ChatPlugin, ChatRequest, ChatStream, ContentBlock, Hit, McpClient,
    Message, MemoryPlugin, Plugin, Role, Scope, StopReason, ToolDef, ToolPlugin, Usage,
    CancellationToken,
};

#[cfg(not(target_arch = "wasm32"))]
pub use secrets::KeyringSecretStore;

#[cfg(test)]
mod ui_agnostic_guard {
    /// Source-level check: scans every .rs file under src/agent/ and asserts
    /// no `use dioxus` or `use crate::{ui,shell,commands,plugin,plugins,tabs,panel,editor,theme,app,log}` lines.
    /// The real dependency-level guard lands in Plans-Phase-4 (cargo-deny + post-split).
    #[test]
    fn agent_module_does_not_import_ui_layers() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let agent_dir = std::path::Path::new(manifest_dir).join("src/agent");
        let mut violations: Vec<String> = Vec::new();
        let forbidden_prefixes = [
            "use dioxus",
            "use crate::ui",
            "use crate::shell",
            "use crate::commands",
            "use crate::plugin",
            "use crate::plugins",
            "use crate::tabs",
            "use crate::panel",
            "use crate::editor",
            "use crate::theme",
            "use crate::app",
        ];
        fn walk(dir: &std::path::Path, forbidden: &[&str], out: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).expect("read agent dir") {
                let entry = entry.expect("dir entry");
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, forbidden, out);
                    continue;
                }
                if path.extension().and_then(|s| s.to_str()) != Some("rs") {
                    continue;
                }
                let body = std::fs::read_to_string(&path).expect("read .rs");
                for (lineno, line) in body.lines().enumerate() {
                    let trimmed = line.trim_start();
                    for prefix in forbidden {
                        if trimmed.starts_with(prefix) {
                            out.push(format!("{}:{} {}", path.display(), lineno + 1, trimmed));
                        }
                    }
                }
            }
        }
        walk(&agent_dir, &forbidden_prefixes, &mut violations);
        assert!(
            violations.is_empty(),
            "src/agent/ must not import UI layers; found:\n{}",
            violations.join("\n")
        );
    }
}
