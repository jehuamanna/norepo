//! `operon-agent` — CLI driver for the Operon agent runtime.
//!
//! ## Usage
//!
//! ```text
//! operon-agent --provider anthropic --model claude-sonnet-4-6 \
//!     --cwd /path/to/repo "summarise this repo"
//!
//! operon-agent --provider openai --model gpt-5 \
//!     --cwd /path "fix the broken test in src/foo.rs"
//!
//! operon-agent --provider google --model gemini-2.0-flash \
//!     --cwd /path "describe the architecture"
//!
//! operon-agent --provider local --api-url http://localhost:11434/v1 \
//!     --model qwen2.5-coder:32b --cwd /path "..."
//! ```
//!
//! Reads API keys from the `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` /
//! `GOOGLE_API_KEY` env vars. Tools are bound to the supplied `--cwd`.
//!
//! Streams events to stdout, indenting tool-uses and thinking blocks for
//! readability. Cancel with Ctrl-C — the runtime drops mid-turn.

use clap::{Parser, ValueEnum};
use futures::StreamExt;
use operon_core::{
    bus::EventBus,
    memory::InMemoryStore,
    runtime::{AgentRuntime, Step, StopReason},
    secrets::EnvSecretStore,
    traits::{CancellationToken, ChatPlugin, MemoryPlugin, Scope},
    Budget,
};
use operon_plugins_tools::default_tools;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(name = "operon-agent", version, about)]
struct Cli {
    /// Which provider to drive.
    #[arg(long, value_enum, default_value_t = Provider::Anthropic)]
    provider: Provider,

    /// Model id. Provider-specific (claude-sonnet-4-6 / gpt-5 / gemini-2.0-flash / etc.).
    #[arg(long)]
    model: Option<String>,

    /// Working directory the tools are bound to. Defaults to the current dir.
    /// Tool calls receive this as their cwd; absolute path required.
    #[arg(long)]
    cwd: Option<String>,

    /// Override the provider base URL — required for `local`, optional for others.
    #[arg(long)]
    api_url: Option<String>,

    /// Optional system prompt prepended to the conversation.
    #[arg(long)]
    system: Option<String>,

    /// Max tokens per response.
    #[arg(long, default_value_t = 4096)]
    max_tokens: u32,

    /// Cap on agent loop iterations (tool-call rounds).
    #[arg(long, default_value_t = 16)]
    max_steps: u32,

    /// Anthropic only — extended-thinking budget tokens. Disables when 0.
    #[arg(long, default_value_t = 0)]
    thinking_budget: u32,

    /// Print the raw `Step` JSON instead of the formatted view.
    #[arg(long)]
    json: bool,

    /// The user prompt. Pass via positional or read from stdin if empty.
    prompt: Vec<String>,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Provider {
    Anthropic,
    Openai,
    Google,
    /// OpenAI-compatible local endpoint (Ollama / vLLM / llama.cpp server).
    /// Pair with `--api-url http://localhost:11434/v1`.
    Local,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    init_tracing();

    if let Err(e) = run(cli).await {
        eprintln!("operon-agent: {e}");
        return std::process::ExitCode::from(1);
    }
    std::process::ExitCode::SUCCESS
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,operon=info"));
    let _ = fmt().with_env_filter(filter).with_writer(std::io::stderr).try_init();
}

async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    // Resolve the prompt.
    let prompt = if cli.prompt.is_empty() {
        let mut buf = String::new();
        use tokio::io::AsyncReadExt;
        tokio::io::stdin().read_to_string(&mut buf).await?;
        buf.trim().to_string()
    } else {
        cli.prompt.join(" ")
    };
    if prompt.is_empty() {
        return Err("empty prompt — pass on the command line or pipe via stdin".into());
    }

    // Resolve cwd. Tools use absolute paths; the prompt mentions cwd for the model.
    let cwd: std::path::PathBuf = match cli.cwd.as_deref() {
        Some(p) => std::path::PathBuf::from(p),
        None => std::env::current_dir()?,
    };
    if !cwd.is_absolute() {
        return Err(format!("--cwd must be absolute: {}", cwd.display()).into());
    }

    // Build the chat plugin per provider.
    let secrets: Arc<dyn operon_core::secrets::SecretStore> = Arc::new(EnvSecretStore::new(""));
    let chat: Arc<dyn ChatPlugin> = match cli.provider {
        Provider::Anthropic => {
            use operon_plugins_anthropic::{AnthropicChatPlugin, AnthropicConfig};
            let mut cfg = AnthropicConfig {
                max_tokens: cli.max_tokens,
                ..Default::default()
            };
            if let Some(m) = cli.model.clone() {
                cfg.model = m;
            }
            if let Some(u) = cli.api_url.clone() {
                cfg.api_url = u;
            }
            if cli.thinking_budget > 0 {
                cfg.thinking_budget_tokens = Some(cli.thinking_budget);
            }
            Arc::new(AnthropicChatPlugin::new(cfg, secrets.clone())?)
        }
        Provider::Openai => {
            use operon_plugins_openai::{OpenAIChatPlugin, OpenAIConfig};
            let mut cfg = OpenAIConfig {
                max_tokens: cli.max_tokens,
                ..Default::default()
            };
            if let Some(m) = cli.model.clone() {
                cfg.model = m;
            }
            if let Some(u) = cli.api_url.clone() {
                cfg.api_url = u;
            }
            Arc::new(OpenAIChatPlugin::new(cfg, secrets.clone())?)
        }
        Provider::Google => {
            use operon_plugins_google::{GoogleChatPlugin, GoogleConfig};
            let mut cfg = GoogleConfig {
                max_tokens: cli.max_tokens,
                ..Default::default()
            };
            if let Some(m) = cli.model.clone() {
                cfg.model = m;
            }
            if let Some(u) = cli.api_url.clone() {
                cfg.api_url = u;
            }
            Arc::new(GoogleChatPlugin::new(cfg, secrets.clone())?)
        }
        Provider::Local => {
            use operon_plugins_openai::{OpenAIChatPlugin, OpenAIConfig};
            let api_url = cli
                .api_url
                .clone()
                .unwrap_or_else(|| "http://localhost:11434/v1".to_string());
            let model = cli.model.clone().unwrap_or_else(|| "llama3.2".to_string());
            let cfg = OpenAIConfig {
                api_url,
                model,
                max_tokens: cli.max_tokens,
                require_api_key: false,
            };
            Arc::new(OpenAIChatPlugin::new(cfg, secrets.clone())?)
        }
    };

    let memory: Arc<dyn MemoryPlugin> = Arc::new(InMemoryStore::new());
    let bus = EventBus::new(64);

    // Hand the model an annotated prompt that names the cwd; tools rebind to it
    // via absolute paths the agent constructs.
    let tools = default_tools();
    let runtime = Arc::new(AgentRuntime::new(chat, tools, memory, bus));

    let session = uuid::Uuid::new_v4();
    let ct = CancellationToken::new();
    let ct_for_signal = ct.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        eprintln!("\n^C — cancelling agent");
        ct_for_signal.cancel();
    });

    let augmented_prompt = if let Some(sys) = cli.system {
        // No first-class system slot in the runtime's `run()` signature — fold
        // it into the user prompt's preamble.
        format!("[system]\n{}\n\n[user]\n{}\n\n[cwd]\n{}\n", sys, prompt, cwd.display())
    } else {
        format!("{}\n\n[cwd]\n{}\n", prompt, cwd.display())
    };

    let budget = Budget::new(None, None, None, Some(cli.max_steps));
    let mut stream = runtime.run(session, Scope::User, augmented_prompt, budget, ct);

    let mut step_count = 0u32;
    while let Some(step) = stream.next().await {
        step_count += 1;
        if cli.json {
            let v = serde_json::to_value(&step).unwrap_or(serde_json::Value::Null);
            println!("{v}");
            continue;
        }
        match &step {
            Step::Started => eprintln!("◇ session started"),
            Step::StreamDelta(t) => {
                use std::io::Write;
                let _ = std::io::stdout().write_all(t.as_bytes());
                let _ = std::io::stdout().flush();
            }
            Step::Thinking(t) => {
                eprintln!("\n┄ thinking ┄ {}", t.lines().next().unwrap_or(""));
            }
            Step::ToolCall { name, input, .. } => {
                eprintln!("\n→ tool {name}({})", short(&input.to_string()));
            }
            Step::ToolResult { is_error, output, .. } => {
                let prefix = if *is_error { "✗" } else { "✓" };
                eprintln!("{prefix} tool result: {}", short(&output.to_string()));
            }
            Step::PermissionRequest { kind, title, .. } => {
                eprintln!("? permission asked for {kind}: {title}");
            }
            Step::Done(reason) => {
                eprintln!("\n● done ({reason:?}, {step_count} steps)");
                if matches!(reason, StopReason::EndTurn) {
                    return Ok(());
                } else {
                    return Err(format!("agent stopped: {reason:?}").into());
                }
            }
        }
    }
    Ok(())
}

fn short(s: &str) -> String {
    const N: usize = 120;
    if s.chars().count() <= N {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(N).collect();
        out.push_str("…");
        out
    }
}
