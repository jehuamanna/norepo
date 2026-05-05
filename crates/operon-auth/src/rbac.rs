//! RBAC stub. Full truth-table implementation lives in Plans-Phase-3; this
//! crate exposes the enum + a permissive `check` that lets every Phase-2
//! action through. Phase 3 replaces the body.

use operon_store::repos::membership::Role;
use operon_store::{NoteId, OrgId, ProjectId, UserId};

use crate::error::AuthError;
use crate::identity::Identity;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    OrgCreate,
    OrgUpdate,
    OrgDelete,
    OrgRead,
    DepartmentCreate,
    DepartmentUpdate,
    DepartmentDelete,
    DepartmentRead,
    TeamCreate,
    TeamUpdate,
    TeamDelete,
    TeamRead,
    ProjectCreate,
    ProjectUpdate,
    ProjectDelete,
    ProjectRead,
    NoteCreate,
    NoteUpdate,
    NoteDelete,
    NoteRead,
    NoteWrite,
    MembershipCreate,
    MembershipUpdate,
    MembershipDelete,
    MembershipRead,
    TeamMemberCreate,
    TeamMemberDelete,
    TeamProjectCreate,
    TeamProjectDelete,
    InviteCreate,
    TempPasswordIssue,
    Export,
    Import,
}

#[derive(Debug, Clone)]
pub enum Scope {
    System,
    Org(OrgId),
    Project { project_id: ProjectId, org_id: OrgId },
    Note { note_id: NoteId, project_id: ProjectId, org_id: OrgId },
    User(UserId),
}

/// Phase-2 stub: returns Ok for every (identity, action, scope). Phase 3
/// replaces this with the real truth table.
pub fn check(_identity: &Identity, _action: &Action, _scope: &Scope) -> Result<(), AuthError> {
    Ok(())
}

/// Phase-3 also adds `check_note(... has_team_access: bool)`. Stubbed here.
pub fn check_note(
    identity: &Identity,
    action: &Action,
    scope: &Scope,
    _has_team_access: bool,
) -> Result<(), AuthError> {
    check(identity, action, scope)
}

/// Convenience: does this identity have a master_admin role anywhere?
pub fn is_master_admin(identity: &Identity) -> bool {
    matches!(identity.role_in_active_org, Some(Role::MasterAdmin))
}
