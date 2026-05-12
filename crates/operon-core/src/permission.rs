//! Async permission gating for tool calls.
//!
//! Slice A0 scaffold — the type surface is defined here so dependent crates
//! can compile against it. Slice A3 wires `PermissionGate` into `AgentRuntime`
//! and the `Step::PermissionRequest` event flow.

use crate::error::OperonResult;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::oneshot;
use uuid::Uuid;

/// What the user (or rules engine) decided about a permission request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionDecision {
    /// Run this one tool call only.
    AllowOnce,
    /// Run this one and silently approve future calls matching the same pattern
    /// for the rest of the session.
    AllowAlways,
    /// Reject; the tool call returns an error to the agent.
    Reject,
}

/// Inputs the gate needs to evaluate / surface a request.
///
/// `kind` is a free-form short tag like `"shell"`, `"file_write"`, `"git_commit"`
/// used for both UI grouping and pattern matching against `RuleSet`s.
#[derive(Clone, Debug)]
pub struct AskInput {
    pub kind: String,
    pub title: String,
    pub locations: Vec<String>,
    pub raw_input: serde_json::Value,
}

/// A wildcard rule: pattern + action. Patterns match against `AskInput::kind`
/// using simple glob semantics (`shell:*`, `file_write:**/*.env`, etc.).
///
/// Slice A3 lands the matching engine; Slice A4b lands rule loading from the
/// settings UI.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PermissionRule {
    pub pattern: String,
    pub action: PermissionDecision,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RuleSet {
    pub rules: Vec<PermissionRule>,
}

/// What `ask()` does when no rule matches and no sticky allow applies.
/// `AllowOnce` mirrors the legacy claude-code behaviour (no gating); flip to
/// `Reject` for a strict default-deny.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefaultDecision {
    AllowOnce,
    Reject,
}

/// The async gate. `ask()` either resolves immediately (rule matched a deterministic
/// allow/deny) or parks on a `oneshot` channel waiting for the UI to call `reply()`.
///
/// One gate per session; clones share the same pending-requests map.
#[derive(Clone)]
pub struct PermissionGate {
    inner: std::sync::Arc<Mutex<GateInner>>,
}

struct GateInner {
    pending: HashMap<Uuid, oneshot::Sender<PermissionDecision>>,
    /// AllowAlways decisions accumulate here; subsequent matching `ask()` calls
    /// resolve immediately to `AllowOnce`.
    sticky_allows: Vec<String>,
    rules: RuleSet,
    default: DefaultDecision,
}

impl PermissionGate {
    pub fn new(rules: RuleSet) -> Self {
        Self {
            inner: std::sync::Arc::new(Mutex::new(GateInner {
                pending: HashMap::new(),
                sticky_allows: Vec::new(),
                rules,
                default: DefaultDecision::AllowOnce,
            })),
        }
    }

    /// Strict gate: every `ask()` returns `Reject` unless a rule or sticky allow
    /// matches. Useful for tests and for ratcheting down a session at runtime.
    pub fn strict() -> Self {
        Self {
            inner: std::sync::Arc::new(Mutex::new(GateInner {
                pending: HashMap::new(),
                sticky_allows: Vec::new(),
                rules: RuleSet::default(),
                default: DefaultDecision::Reject,
            })),
        }
    }

    pub fn with_default(self, default: DefaultDecision) -> Self {
        self.inner
            .lock()
            .expect("permission gate lock poisoned")
            .default = default;
        self
    }

    /// Ask for permission. Resolves immediately when a rule or sticky allow
    /// matches `input.kind`; otherwise applies the default decision.
    ///
    /// Slice A12 will route the `Ask` (no rule matched, no sticky allow, default
    /// is to ask UI) case through a `oneshot::channel` that the UI consumes. For
    /// now the default is `AllowOnce` for backwards compat — wire `strict()` or
    /// `with_default(DefaultDecision::Reject)` to opt into stricter behaviour.
    pub async fn ask(&self, input: AskInput) -> OperonResult<PermissionDecision> {
        let g = self.inner.lock().expect("permission gate lock poisoned");

        // 1. Sticky allows — exact-prefix or wildcard match against `kind`.
        for pat in &g.sticky_allows {
            if pattern_matches(pat, &input.kind) {
                return Ok(PermissionDecision::AllowOnce);
            }
        }
        // 2. Configured rules — first match wins.
        for rule in &g.rules.rules {
            if pattern_matches(&rule.pattern, &input.kind) {
                return Ok(rule.action);
            }
        }
        // 3. Fallback.
        Ok(match g.default {
            DefaultDecision::AllowOnce => PermissionDecision::AllowOnce,
            DefaultDecision::Reject => PermissionDecision::Reject,
        })
    }

    /// Provide the user's decision for an outstanding request.
    pub fn reply(&self, request_id: Uuid, decision: PermissionDecision) -> OperonResult<()> {
        let mut g = self.inner.lock().expect("permission gate lock poisoned");
        if let Some(tx) = g.pending.remove(&request_id) {
            let _ = tx.send(decision);
        }
        Ok(())
    }

    /// Add a sticky allow pattern (called when the user picks "Always allow").
    pub fn allow_always(&self, pattern: impl Into<String>) {
        let mut g = self.inner.lock().expect("permission gate lock poisoned");
        g.sticky_allows.push(pattern.into());
    }
}

impl Default for PermissionGate {
    fn default() -> Self {
        Self::new(RuleSet::default())
    }
}

/// Glob matching for permission patterns. Supports `*` (match any sequence
/// of characters within one segment) and `**` (match any sequence including
/// path separators). For now patterns and inputs use `:` as a segment
/// separator (e.g. `shell:rm -rf /`); `*` doesn't cross `:` boundaries
/// unless `**` is used.
fn pattern_matches(pattern: &str, input: &str) -> bool {
    if pattern == "*" || pattern == "**" || pattern == input {
        return true;
    }
    // Build a regex from the glob.
    let mut re = String::with_capacity(pattern.len() + 2);
    re.push('^');
    let bytes = pattern.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        match c {
            '*' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                    re.push_str(".*");
                    i += 2;
                    continue;
                }
                re.push_str("[^:]*");
            }
            '.' | '+' | '(' | ')' | '|' | '^' | '$' | '\\' | '[' | ']' | '{' | '}' | '?' => {
                re.push('\\');
                re.push(c);
            }
            other => re.push(other),
        }
        i += 1;
    }
    re.push('$');
    regex::Regex::new(&re).map(|r| r.is_match(input)).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ai(kind: &str) -> AskInput {
        AskInput {
            kind: kind.into(),
            title: kind.into(),
            locations: vec![],
            raw_input: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn default_gate_allows_unmatched_kinds() {
        let g = PermissionGate::default();
        let d = g.ask(ai("shell")).await.unwrap();
        assert_eq!(d, PermissionDecision::AllowOnce);
    }

    #[tokio::test]
    async fn strict_gate_rejects_unmatched_kinds() {
        let g = PermissionGate::strict();
        let d = g.ask(ai("shell")).await.unwrap();
        assert_eq!(d, PermissionDecision::Reject);
    }

    #[tokio::test]
    async fn rule_match_overrides_default() {
        let mut rules = RuleSet::default();
        rules.rules.push(PermissionRule {
            pattern: "shell".into(),
            action: PermissionDecision::Reject,
        });
        let g = PermissionGate::new(rules);
        assert_eq!(g.ask(ai("shell")).await.unwrap(), PermissionDecision::Reject);
        // Other kinds fall through to the default (AllowOnce).
        assert_eq!(g.ask(ai("read")).await.unwrap(), PermissionDecision::AllowOnce);
    }

    #[tokio::test]
    async fn wildcard_pattern_matches_kind_prefix() {
        let mut rules = RuleSet::default();
        rules.rules.push(PermissionRule {
            pattern: "shell:*".into(),
            action: PermissionDecision::Reject,
        });
        let g = PermissionGate::new(rules);
        assert_eq!(g.ask(ai("shell:rm -rf /")).await.unwrap(), PermissionDecision::Reject);
        // `shell` (no colon) doesn't match `shell:*`.
        assert_eq!(g.ask(ai("shell")).await.unwrap(), PermissionDecision::AllowOnce);
    }

    #[tokio::test]
    async fn double_star_crosses_segments() {
        let mut rules = RuleSet::default();
        rules.rules.push(PermissionRule {
            pattern: "git:**".into(),
            action: PermissionDecision::Reject,
        });
        let g = PermissionGate::new(rules);
        assert_eq!(g.ask(ai("git:commit:foo")).await.unwrap(), PermissionDecision::Reject);
    }

    #[tokio::test]
    async fn first_matching_rule_wins() {
        let mut rules = RuleSet::default();
        rules.rules.push(PermissionRule {
            pattern: "shell".into(),
            action: PermissionDecision::AllowAlways,
        });
        rules.rules.push(PermissionRule {
            pattern: "shell".into(),
            action: PermissionDecision::Reject,
        });
        let g = PermissionGate::new(rules);
        assert_eq!(g.ask(ai("shell")).await.unwrap(), PermissionDecision::AllowAlways);
    }

    #[tokio::test]
    async fn sticky_allow_short_circuits_rules() {
        let mut rules = RuleSet::default();
        rules.rules.push(PermissionRule {
            pattern: "*".into(),
            action: PermissionDecision::Reject,
        });
        let g = PermissionGate::new(rules);
        // Without sticky: rejected.
        assert_eq!(g.ask(ai("shell")).await.unwrap(), PermissionDecision::Reject);
        // With sticky: allowed.
        g.allow_always("shell");
        assert_eq!(g.ask(ai("shell")).await.unwrap(), PermissionDecision::AllowOnce);
    }

    #[tokio::test]
    async fn star_rule_matches_everything() {
        let mut rules = RuleSet::default();
        rules.rules.push(PermissionRule {
            pattern: "*".into(),
            action: PermissionDecision::AllowAlways,
        });
        let g = PermissionGate::new(rules);
        assert_eq!(g.ask(ai("anything")).await.unwrap(), PermissionDecision::AllowAlways);
    }

    #[test]
    fn reply_to_unknown_request_is_noop() {
        let g = PermissionGate::default();
        g.reply(Uuid::new_v4(), PermissionDecision::Reject).unwrap();
    }

    #[test]
    fn pattern_matches_handles_specials() {
        // Literal dots in the kind shouldn't be interpreted as regex wildcards.
        assert!(pattern_matches("foo.bar", "foo.bar"));
        assert!(!pattern_matches("foo.bar", "fooXbar"));
        // Star is one-segment greedy.
        assert!(pattern_matches("a:*:c", "a:b:c"));
        assert!(!pattern_matches("a:*:c", "a:b:x:c"));
        // Double-star crosses segments.
        assert!(pattern_matches("a:**:c", "a:b:x:c"));
    }
}
