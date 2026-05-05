use crate::error::{OperonError, OperonResult};
use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OperonConfig {
    #[serde(default)]
    pub runtime: RuntimeConfig,
    #[serde(default)]
    pub providers: ProvidersConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub mcp: McpConfig,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RuntimeConfig {
    pub default_model: Option<String>,
    pub log_filter: Option<String>,
    #[serde(default)]
    pub default_budget: BudgetConfig,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BudgetConfig {
    pub max_tokens: Option<u64>,
    pub max_seconds: Option<u64>,
    pub max_tool_calls: Option<u32>,
    pub max_steps: Option<u32>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProvidersConfig {
    pub anthropic: Option<AnthropicProviderConfig>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AnthropicProviderConfig {
    pub api_url: Option<String>,
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
    pub anthropic_version: Option<String>,
    pub anthropic_beta: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MemoryConfig {
    #[serde(default = "default_memory_kind")]
    pub kind: String,
    pub sqlite_path: Option<String>,
}

fn default_memory_kind() -> String {
    "in_memory".to_string()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub transport: String, // "stdio" | "websocket"
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub url: Option<String>,
}

impl OperonConfig {
    pub fn load() -> OperonResult<Self> {
        Figment::new()
            .merge(Toml::file("operon.toml"))
            .merge(Env::prefixed("OPERON_").split("__"))
            .extract()
            .map_err(|e| OperonError::Config(e.to_string()))
    }

    pub fn from_toml_str(s: &str) -> OperonResult<Self> {
        Figment::new()
            .merge(Toml::string(s))
            .extract()
            .map_err(|e| OperonError::Config(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let c = OperonConfig::default();
        assert_eq!(c.memory.kind, "");
        assert!(c.providers.anthropic.is_none());
        assert!(c.mcp.servers.is_empty());
    }

    #[test]
    fn from_toml_str_parses() {
        let s = r#"
[runtime]
default_model = "claude-opus-4-7"

[providers.anthropic]
model = "claude-opus-4-7"
max_tokens = 4096

[memory]
kind = "in_memory"

[[mcp.servers]]
name = "stub"
transport = "stdio"
command = "/usr/bin/echo"
args = []
"#;
        let c = OperonConfig::from_toml_str(s).expect("parse");
        assert_eq!(c.runtime.default_model.as_deref(), Some("claude-opus-4-7"));
        assert_eq!(c.providers.anthropic.unwrap().max_tokens, Some(4096));
        assert_eq!(c.memory.kind, "in_memory");
        assert_eq!(c.mcp.servers[0].name, "stub");
    }
}
