//! Subprocess-driven service for managing MCP servers via the
//! `claude mcp` CLI family.
//!
//! Operates by spawning `claude mcp <subcommand>` with `tokio::process`
//! and parsing the human-readable output (claude has no `--json` flag on
//! `list` / `get` as of the current CLI). Mutations (`add`, `remove`)
//! return success/failure based on the exit code; the stderr is surfaced
//! verbatim on failure so the user can self-diagnose.

#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_OUTPUT_BYTES: usize = 256 * 1024;

/// Configuration scope claude writes the entry under. Mirrors
/// `claude mcp add -s {local,user,project}` exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    Local,
    User,
    Project,
}

impl Scope {
    pub fn as_arg(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::User => "user",
            Self::Project => "project",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Local => "Local (this project, private)",
            Self::User => "User (global)",
            Self::Project => "Project (.mcp.json, shared)",
        }
    }
}

/// Transport accepted by `claude mcp add -t <transport>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Transport {
    Stdio,
    Sse,
    Http,
}

impl Transport {
    pub fn as_arg(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Sse => "sse",
            Self::Http => "http",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Sse => "sse",
            Self::Http => "http",
        }
    }
}

/// Typed view of `claude mcp add` arguments. The service composes the
/// argv element-by-element so values containing spaces / quotes can't
/// break out into another flag.
#[derive(Clone, Debug)]
pub struct AddArgs {
    pub scope: Scope,
    pub transport: Transport,
    pub name: String,
    /// stdio: command path. sse/http: full URL.
    pub command_or_url: String,
    /// stdio only — passed after the `--` separator. Ignored for sse/http.
    pub args: Vec<String>,
    /// stdio only — `KEY=VALUE` env pairs (each emitted as `-e KEY=VALUE`).
    pub env: Vec<(String, String)>,
    /// sse/http only — header rows, emitted as `-H "Key: Value"`.
    pub headers: Vec<(String, String)>,
}

impl AddArgs {
    /// Compose the argv (excluding the leading `claude mcp add`).
    /// Pulled out so unit tests can assert on the result without
    /// spawning a subprocess.
    ///
    /// The CLI signature is
    /// `claude mcp add [options] <name> <commandOrUrl> [args...]` —
    /// `name` and `commandOrUrl` are the first positional arguments,
    /// and the `-e`/`-H` flags are *variadic* (`<env...>` / `<header...>`).
    /// If `-e KEY=VAL` is placed *before* the positionals, commander
    /// greedily consumes the name into the env list and the add call
    /// fails with `"Invalid environment variable format: <name>"`.
    /// We sidestep that by emitting the positionals FIRST, then the
    /// `-s`/`-t` flags, then variadics, then `--` + subprocess args
    /// last (so `--` only terminates parsing for the subprocess).
    pub fn argv(&self) -> Vec<String> {
        let mut argv: Vec<String> = Vec::new();
        argv.push(self.name.clone());
        argv.push(self.command_or_url.clone());
        argv.push("-s".into());
        argv.push(self.scope.as_arg().into());
        argv.push("-t".into());
        argv.push(self.transport.as_arg().into());
        for (k, v) in &self.env {
            argv.push("-e".into());
            argv.push(format!("{k}={v}"));
        }
        for (k, v) in &self.headers {
            argv.push("-H".into());
            argv.push(format!("{k}: {v}"));
        }
        if matches!(self.transport, Transport::Stdio) && !self.args.is_empty() {
            argv.push("--".into());
            for a in &self.args {
                argv.push(a.clone());
            }
        }
        argv
    }
}

/// One row in `claude mcp list`'s output. Lacks env/headers/args — those
/// only appear in `claude mcp get <name>`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpEntry {
    pub name: String,
    /// Raw command line as printed by claude (e.g. `npx -y @nodex-studio/mcp`
    /// or `https://mcp.sentry.dev/mcp`). Preserved verbatim because the
    /// transport isn't on this line.
    pub command_or_url: String,
    /// Claude's status string with the leading glyph stripped — typically
    /// `Connected`, `Failed`, or `Connecting…`. Used for the active dot
    /// when no live `system/init` snapshot is available.
    pub status: String,
    /// Whether the status reads as connected. Drives the green-vs-red dot
    /// when only `mcp list` is available.
    pub connected: bool,
}

/// Detailed view from `claude mcp get <name>`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpDetails {
    pub name: String,
    pub scope_label: String,
    pub status: String,
    pub connected: bool,
    pub transport: String,
    pub command: Option<String>,
    pub args: Option<String>,
    pub url: Option<String>,
    pub env: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct McpService {
    claude_bin: PathBuf,
}

impl McpService {
    pub fn new(claude_bin: PathBuf) -> Self {
        Self { claude_bin }
    }

    /// `claude mcp list` → parsed entries. `cwd` is the working directory
    /// claude is spawned in; project-scoped entries are read from
    /// `<cwd>/.mcp.json`.
    pub async fn list(&self, cwd: Option<&std::path::Path>) -> Result<Vec<McpEntry>, String> {
        let out = self.run(&["mcp", "list"], cwd).await?;
        Ok(parse_list(&out.stdout))
    }

    /// `claude mcp get <name>` → parsed details.
    pub async fn get(
        &self,
        name: &str,
        cwd: Option<&std::path::Path>,
    ) -> Result<McpDetails, String> {
        let out = self.run(&["mcp", "get", name], cwd).await?;
        parse_get(name, &out.stdout)
            .ok_or_else(|| format!("could not parse `claude mcp get` output:\n{}", out.stdout))
    }

    /// `claude mcp add` with composed argv. `cwd` matters for
    /// `--scope project` / `local` writes.
    pub async fn add(
        &self,
        args: &AddArgs,
        cwd: Option<&std::path::Path>,
    ) -> Result<(), String> {
        let mut argv: Vec<String> = vec!["mcp".into(), "add".into()];
        argv.extend(args.argv());
        let argv_ref: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
        self.run(&argv_ref, cwd).await.map(|_| ())
    }

    /// `claude mcp remove [-s scope] <name>`.
    pub async fn remove(
        &self,
        name: &str,
        scope: Option<Scope>,
        cwd: Option<&std::path::Path>,
    ) -> Result<(), String> {
        let mut argv: Vec<&str> = vec!["mcp", "remove"];
        if let Some(s) = scope {
            argv.push("-s");
            argv.push(s.as_arg());
        }
        argv.push(name);
        self.run(&argv, cwd).await.map(|_| ())
    }

    async fn run(
        &self,
        args: &[&str],
        cwd: Option<&std::path::Path>,
    ) -> Result<RunOutput, String> {
        let mut cmd = Command::new(&self.claude_bin);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        let mut child = cmd.spawn().map_err(|e| {
            format!(
                "spawn `{}` failed: {e}",
                self.claude_bin.display(),
            )
        })?;
        let mut stdout_pipe = child.stdout.take().ok_or("missing stdout pipe")?;
        let mut stderr_pipe = child.stderr.take().ok_or("missing stderr pipe")?;

        let read_stdout = read_capped(&mut stdout_pipe);
        let read_stderr = read_capped(&mut stderr_pipe);

        let wait = async {
            let (so, se, status) = tokio::join!(read_stdout, read_stderr, child.wait());
            (so, se, status)
        };

        match timeout(DEFAULT_TIMEOUT, wait).await {
            Err(_) => Err(format!(
                "`claude {}` timed out after {}s",
                args.join(" "),
                DEFAULT_TIMEOUT.as_secs(),
            )),
            Ok((stdout, stderr, status)) => {
                let exit = status.map_err(|e| format!("wait: {e}"))?;
                if exit.success() {
                    Ok(RunOutput { stdout, stderr })
                } else {
                    let code = exit.code().unwrap_or(-1);
                    let body = if !stderr.trim().is_empty() {
                        stderr
                    } else {
                        stdout
                    };
                    Err(format!(
                        "`claude {}` exited {code}: {}",
                        args.join(" "),
                        body.trim()
                    ))
                }
            }
        }
    }
}

struct RunOutput {
    stdout: String,
    #[allow(dead_code)]
    stderr: String,
}

async fn read_capped(pipe: &mut (impl AsyncReadExt + Unpin)) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                if buf.len() + n <= MAX_OUTPUT_BYTES {
                    buf.extend_from_slice(&chunk[..n]);
                } else {
                    let take = MAX_OUTPUT_BYTES.saturating_sub(buf.len());
                    buf.extend_from_slice(&chunk[..take]);
                    break;
                }
            }
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}

/// Parse `claude mcp list` output. Each server is a single line:
///   `<name>: <commandOrUrl> - <statusGlyph> <statusText>`
/// A leading "Checking MCP server health…" line is ignored.
pub(crate) fn parse_list(stdout: &str) -> Vec<McpEntry> {
    let mut out = Vec::new();
    for raw in stdout.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("Checking ") {
            continue;
        }
        // Empty-state messages start with "No MCP servers" — emit nothing.
        if line.to_ascii_lowercase().starts_with("no mcp ") {
            continue;
        }
        // Split off the trailing `- status` segment first; otherwise a
        // command containing ": " would confuse the name split.
        let (lhs, status_seg) = match line.rsplit_once(" - ") {
            Some(pair) => pair,
            None => continue,
        };
        let (name, command_or_url) = match lhs.split_once(": ") {
            Some((n, c)) => (n.trim(), c.trim()),
            None => continue,
        };
        if name.is_empty() {
            continue;
        }
        let (connected, status_text) = parse_status(status_seg);
        out.push(McpEntry {
            name: name.to_string(),
            command_or_url: command_or_url.to_string(),
            status: status_text,
            connected,
        });
    }
    out
}

/// Parse `claude mcp get <name>` output. Returns `None` if the response
/// doesn't look structured (e.g., an error message).
pub(crate) fn parse_get(name: &str, stdout: &str) -> Option<McpDetails> {
    let mut details = McpDetails {
        name: name.to_string(),
        ..McpDetails::default()
    };
    let mut lines = stdout.lines().peekable();
    let mut found_header = false;
    while let Some(raw) = lines.next() {
        let line = raw.trim_end();
        if !found_header {
            // First non-empty line that ends in ":" and matches the
            // requested name is the header (`<name>:`).
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.trim_end_matches(':') == name {
                found_header = true;
            }
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("To remove this server") {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("Scope:") {
            details.scope_label = rest.trim().to_string();
        } else if let Some(rest) = trimmed.strip_prefix("Status:") {
            let (connected, text) = parse_status(rest);
            details.connected = connected;
            details.status = text;
        } else if let Some(rest) = trimmed.strip_prefix("Type:") {
            details.transport = rest.trim().to_string();
        } else if let Some(rest) = trimmed.strip_prefix("Command:") {
            details.command = Some(rest.trim().to_string());
        } else if let Some(rest) = trimmed.strip_prefix("Args:") {
            details.args = Some(rest.trim().to_string());
        } else if let Some(rest) = trimmed.strip_prefix("URL:") {
            details.url = Some(rest.trim().to_string());
        } else if trimmed == "Environment:" {
            collect_kv_block(&mut lines, &mut details.env, '=');
        } else if trimmed == "Headers:" {
            collect_kv_block(&mut lines, &mut details.headers, ':');
        }
    }
    if !found_header {
        return None;
    }
    Some(details)
}

/// Pull subsequent indented lines out of `lines` until the indent ends.
/// Each line is split on `sep` once; trailing whitespace on the value is
/// trimmed.
fn collect_kv_block<'a, I>(lines: &mut std::iter::Peekable<I>, out: &mut Vec<(String, String)>, sep: char)
where
    I: Iterator<Item = &'a str>,
{
    while let Some(peek) = lines.peek() {
        if !peek.starts_with("  ") && !peek.starts_with('\t') {
            break;
        }
        let line = lines.next().unwrap().trim();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(sep) {
            out.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
}

/// Strip the leading status glyph ("✓"/"✗"/"!") and return
/// `(connected, status_text)`.
fn parse_status(seg: &str) -> (bool, String) {
    let s = seg.trim();
    let lower = s.to_ascii_lowercase();
    let connected = lower.starts_with('✓') || lower.contains("connected");
    let mut text = s.to_string();
    if let Some(rest) = text
        .strip_prefix('✓')
        .or_else(|| text.strip_prefix('✗'))
        .or_else(|| text.strip_prefix('!'))
    {
        text = rest.trim().to_string();
    }
    let connected = connected
        && !lower.contains("failed")
        && !lower.contains("not connected");
    (connected, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_stdio_with_env_and_args() {
        let a = AddArgs {
            scope: Scope::User,
            transport: Transport::Stdio,
            name: "time".into(),
            command_or_url: "uvx".into(),
            args: vec!["mcp-server-time".into(), "--utc".into()],
            env: vec![("API_KEY".into(), "abc".into())],
            headers: vec![],
        };
        assert_eq!(
            a.argv(),
            vec![
                "time", "uvx", "-s", "user", "-t", "stdio", "-e", "API_KEY=abc", "--",
                "mcp-server-time", "--utc"
            ]
        );
    }

    #[test]
    fn argv_http_with_headers() {
        let a = AddArgs {
            scope: Scope::Project,
            transport: Transport::Http,
            name: "sentry".into(),
            command_or_url: "https://mcp.sentry.dev/mcp".into(),
            args: vec![],
            env: vec![],
            headers: vec![("Authorization".into(), "Bearer xyz".into())],
        };
        assert_eq!(
            a.argv(),
            vec![
                "sentry",
                "https://mcp.sentry.dev/mcp",
                "-s",
                "project",
                "-t",
                "http",
                "-H",
                "Authorization: Bearer xyz",
            ]
        );
    }

    #[test]
    fn argv_sse_no_extras() {
        let a = AddArgs {
            scope: Scope::Local,
            transport: Transport::Sse,
            name: "rt".into(),
            command_or_url: "https://example.com/sse".into(),
            args: vec!["ignored".into()], // ignored for sse
            env: vec![],
            headers: vec![],
        };
        assert_eq!(
            a.argv(),
            vec!["rt", "https://example.com/sse", "-s", "local", "-t", "sse"]
        );
    }

    /// Regression test for the figma-MCP add failure: variadic `-e`
    /// must NOT precede the name positional or commander will
    /// greedily consume it as another env-var value and the CLI
    /// errors with "Invalid environment variable format: <name>".
    #[test]
    fn argv_emits_name_before_env_so_variadic_flag_doesnt_eat_it() {
        let a = AddArgs {
            scope: Scope::Project,
            transport: Transport::Stdio,
            name: "figma".into(),
            command_or_url: "npx".into(),
            args: vec![
                "-y".into(),
                "figma-developer-mcp".into(),
                "--stdio".into(),
            ],
            env: vec![("FIGMA_API_KEY".into(), "figd_secret".into())],
            headers: vec![],
        };
        let argv = a.argv();
        let env_idx = argv.iter().position(|s| s == "-e").unwrap();
        let name_idx = argv.iter().position(|s| s == "figma").unwrap();
        assert!(name_idx < env_idx, "name must appear before -e (argv={argv:?})");
    }

    #[test]
    fn parse_list_handles_health_header_and_connected_glyph() {
        let s = "Checking MCP server health…\n\narchon: npx -y @nodex-studio/mcp - ✓ Connected\n";
        let v = parse_list(s);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "archon");
        assert_eq!(v[0].command_or_url, "npx -y @nodex-studio/mcp");
        assert!(v[0].connected);
        assert!(v[0].status.contains("Connected"));
    }

    #[test]
    fn parse_list_handles_failed_status() {
        let s = "foo: /broken/cmd - ✗ Failed: ENOENT\n";
        let v = parse_list(s);
        assert_eq!(v.len(), 1);
        assert!(!v[0].connected);
        assert!(v[0].status.starts_with("Failed"));
    }

    #[test]
    fn parse_list_skips_empty_state() {
        let s = "No MCP servers configured. Use `claude mcp add` to add one.\n";
        assert!(parse_list(s).is_empty());
    }

    #[test]
    fn parse_get_stdio_with_env() {
        let s = "archon:\n  Scope: Local config (private to you in this project)\n  Status: ✓ Connected\n  Type: stdio\n  Command: npx\n  Args: -y @nodex-studio/mcp\n  Environment:\n    KEY1=val1\n    KEY2=val2\n\nTo remove this server, run: claude mcp remove \"archon\" -s local\n";
        let d = parse_get("archon", s).expect("should parse");
        assert_eq!(d.name, "archon");
        assert!(d.connected);
        assert_eq!(d.transport, "stdio");
        assert_eq!(d.command.as_deref(), Some("npx"));
        assert_eq!(d.args.as_deref(), Some("-y @nodex-studio/mcp"));
        assert_eq!(
            d.env,
            vec![
                ("KEY1".into(), "val1".into()),
                ("KEY2".into(), "val2".into())
            ]
        );
        assert!(d.scope_label.contains("Local"));
    }

    #[test]
    fn parse_get_http_with_headers() {
        let s = "sentry:\n  Scope: User config\n  Status: ✓ Connected\n  Type: http\n  URL: https://mcp.sentry.dev/mcp\n  Headers:\n    Authorization: Bearer abc\n";
        let d = parse_get("sentry", s).expect("should parse");
        assert_eq!(d.transport, "http");
        assert_eq!(d.url.as_deref(), Some("https://mcp.sentry.dev/mcp"));
        assert_eq!(
            d.headers,
            vec![("Authorization".into(), "Bearer abc".into())]
        );
    }

    #[test]
    fn parse_get_returns_none_when_header_missing() {
        let s = "No MCP server found with name: \"missing\".\n";
        assert!(parse_get("missing", s).is_none());
    }
}
