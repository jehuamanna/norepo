//! Operon agent runtime — UI-agnostic core.
//!
//! This crate is forbidden from depending on `dioxus` or any GUI crate.
//! `cargo-deny` enforces this at the workspace level (see `deny.toml`).

#[cfg(not(target_arch = "wasm32"))]
pub mod agent_event;
pub mod budget;
pub mod bus;
pub mod config;
pub mod echo;
pub mod error;
pub mod memory;
pub mod mock;
pub mod persona;
#[cfg(not(target_arch = "wasm32"))]
pub mod permission;
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
pub use echo::{EchoChatPlugin, EchoToolPlugin};
pub use error::{OperonError, OperonResult};
pub use memory::InMemoryStore;
#[cfg(all(feature = "sqlite-memory", not(target_arch = "wasm32")))]
pub use memory::SqliteMemoryStore;
pub use mock::MockChatPlugin;
pub use persona::{AgentMode, AgentPersona, PersonaRegistry};
#[cfg(not(target_arch = "wasm32"))]
pub use agent_event::{AgentBackend, AgentEvent};
#[cfg(not(target_arch = "wasm32"))]
pub use permission::{AskInput, PermissionDecision, PermissionGate, PermissionRule, RuleSet};
pub use registry::{register_agent_plugins, AgentRegistry};
pub use secrets::{keys as secret_keys, EnvSecretStore, LayeredSecretStore, MockSecretStore, SecretStore};
pub use traits::{
    Capabilities, ChatDelta, ChatPlugin, ChatRequest, ChatStream, ContentBlock, Hit, McpClient,
    Message, MemoryPlugin, Plugin, Role, Scope, StopReason, ToolDef, ToolPlugin, Usage,
    CancellationToken,
};

#[cfg(not(target_arch = "wasm32"))]
pub use secrets::{JsonFileSecretStore, KeyringSecretStore};
