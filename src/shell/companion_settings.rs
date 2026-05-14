//! Layered resolution of Claude defaults: chat → project → global.
//!
//! The three tiers are persisted in:
//! - `chat_session.{model, permission_mode}` (per-chat row, migration 017)
//! - `local_project.{default_model, default_permission_mode}` (project,
//!   migration 019)
//! - `local_app_settings` rows `claude.default_model` /
//!   `claude.default_permission_mode` (global)
//!
//! At spawn time the plugin only needs the effective value. This helper
//! flattens the layers in one place so the picker UI, the bind
//! `use_effect`, and the artifact runner all agree on which value wins.
//!
//! Vault-scope chats skip the project tier — there's no project row to
//! consult, so it's just chat → global.

use operon_store::repos::ChatScope;

/// Resolve a single setting (model OR permission_mode) given the three
/// tiers. Each tier is the value persisted at that level; `None` at any
/// level means "inherit from the next."
pub fn resolve(
    chat: Option<&str>,
    project: Option<&str>,
    global: Option<&str>,
    scope: ChatScope,
) -> Option<String> {
    if let Some(v) = chat {
        return Some(v.to_string());
    }
    resolve_inherited(project, global, scope)
}

/// The value the chat tier would inherit from below if its own column
/// is NULL. Used by the picker to label the "Inherit (X)" option so
/// the user can see what they're falling back to.
pub fn resolve_inherited(
    project: Option<&str>,
    global: Option<&str>,
    scope: ChatScope,
) -> Option<String> {
    if matches!(scope, ChatScope::Project(_)) {
        if let Some(v) = project {
            return Some(v.to_string());
        }
    }
    global.map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn chat_value_wins_over_project_and_global() {
        let pid = Uuid::new_v4();
        let got = resolve(
            Some("chat"),
            Some("project"),
            Some("global"),
            ChatScope::Project(pid),
        );
        assert_eq!(got.as_deref(), Some("chat"));
    }

    #[test]
    fn project_wins_when_chat_unset_for_project_scope() {
        let pid = Uuid::new_v4();
        let got = resolve(None, Some("project"), Some("global"), ChatScope::Project(pid));
        assert_eq!(got.as_deref(), Some("project"));
    }

    #[test]
    fn vault_scope_skips_project_tier() {
        // Project value is set but vault chats must ignore it — there's
        // no project they belong to. Falls through to global.
        let got = resolve(None, Some("project"), Some("global"), ChatScope::Vault);
        assert_eq!(got.as_deref(), Some("global"));
    }

    #[test]
    fn global_wins_when_no_lower_tier_set() {
        let pid = Uuid::new_v4();
        let got = resolve(None, None, Some("global"), ChatScope::Project(pid));
        assert_eq!(got.as_deref(), Some("global"));
    }

    #[test]
    fn returns_none_when_all_tiers_empty() {
        let pid = Uuid::new_v4();
        assert!(resolve(None, None, None, ChatScope::Project(pid)).is_none());
        assert!(resolve(None, None, None, ChatScope::Vault).is_none());
    }

    #[test]
    fn inherited_skips_chat_tier() {
        let pid = Uuid::new_v4();
        // resolve_inherited never consults the chat tier.
        let got = resolve_inherited(Some("project"), Some("global"), ChatScope::Project(pid));
        assert_eq!(got.as_deref(), Some("project"));
        let got = resolve_inherited(None, Some("global"), ChatScope::Project(pid));
        assert_eq!(got.as_deref(), Some("global"));
        let got = resolve_inherited(Some("project"), Some("global"), ChatScope::Vault);
        assert_eq!(got.as_deref(), Some("global"));
    }

    #[test]
    fn empty_string_at_lower_tier_does_not_override_higher() {
        // The picker writes empty string as "unset" via `local_app_settings`
        // shims, but the chat_session and local_project repos store NULL
        // when cleared. This helper only sees Option<&str>, so the caller
        // is responsible for filtering empty strings to None when reading
        // from the global settings table. The helper itself treats Some("")
        // as a real value — documented here for the caller's sake.
        let pid = Uuid::new_v4();
        let got = resolve(Some(""), Some("project"), Some("global"), ChatScope::Project(pid));
        assert_eq!(got.as_deref(), Some(""));
    }
}
