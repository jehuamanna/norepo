//! Cross-cutting permission helpers used by route handlers.

use operon_auth::rbac::{self, Action, Scope};
use operon_auth::Identity;
use operon_store::repos::membership::MembershipRepository;
use operon_store::repos::team_member::TeamMemberRepository;
use operon_store::repos::team_project::TeamProjectRepository;
use operon_store::ProjectId;

use crate::audit;
use crate::error::ApiError;
use crate::state::AppState;

/// Run the RBAC check; on denial, record an audit entry and return Forbidden.
pub fn require(
    state: &AppState,
    identity: &Identity,
    action: Action,
    scope: Scope,
) -> Result<(), ApiError> {
    match rbac::check(identity, &action, &scope) {
        Ok(()) => Ok(()),
        Err(_) => {
            audit::record_denied(state, Some(identity), &action, &scope);
            Err(ApiError::Forbidden)
        }
    }
}

/// Same as [`require`] but for project-/note-scoped actions where `user`-tier
/// access depends on team membership.
pub fn require_note(
    state: &AppState,
    identity: &Identity,
    action: Action,
    scope: Scope,
    has_team_access: bool,
) -> Result<(), ApiError> {
    match rbac::check_note(identity, &action, &scope, has_team_access) {
        Ok(()) => Ok(()),
        Err(_) => {
            audit::record_denied(state, Some(identity), &action, &scope);
            Err(ApiError::Forbidden)
        }
    }
}

/// Compute whether the given identity has any team assigned to the given
/// project.
pub fn has_team_access(
    state: &AppState,
    identity: &Identity,
    project_id: &ProjectId,
) -> Result<bool, ApiError> {
    let memberships = state.memberships.by_user(&identity.user_id)?;
    for m in memberships {
        let team_members = state.team_members.list_by_membership(&m.id)?;
        for tm in team_members {
            let assigns = state.team_projects.list_by_team(&tm.team_id)?;
            if assigns.iter().any(|tp| &tp.project_id == project_id) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
