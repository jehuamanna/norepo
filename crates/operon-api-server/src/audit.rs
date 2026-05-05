//! Helpers for emitting audit log entries from route handlers.

use operon_auth::rbac::{Action, Scope};
use operon_auth::Identity;
use operon_store::repos::audit::{AuditEntry, AuditLogRepository, AuditOutcome};

use crate::state::AppState;

pub fn record_denied(state: &AppState, identity: Option<&Identity>, action: &Action, scope: &Scope) {
    let entry = AuditEntry {
        user_id: identity.map(|i| i.user_id.clone()),
        org_id: scope.org_id().cloned().or_else(|| identity.and_then(|i| i.active_org_id.clone())),
        role: identity
            .and_then(|i| i.role_in_active_org.as_ref())
            .map(|r| r.as_str().to_string()),
        action: action.name().to_string(),
        scope_type: scope.type_name().to_string(),
        scope_id: scope.id_str(),
        outcome: AuditOutcome::Denied,
        payload_json: None,
    };
    if let Err(e) = state.audit.record(&entry) {
        tracing::warn!(err = %e, "failed to record audit entry");
    }
}
