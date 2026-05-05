//! Full Phase-3 RBAC implementation.

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
    TeamMemberRead,
    TeamProjectCreate,
    TeamProjectDelete,
    TeamProjectRead,
    InviteCreate,
    TempPasswordIssue,
    Export,
    Import,
    AdminUsersList,
}

impl Action {
    pub fn name(&self) -> &'static str {
        match self {
            Action::OrgCreate => "OrgCreate",
            Action::OrgUpdate => "OrgUpdate",
            Action::OrgDelete => "OrgDelete",
            Action::OrgRead => "OrgRead",
            Action::DepartmentCreate => "DepartmentCreate",
            Action::DepartmentUpdate => "DepartmentUpdate",
            Action::DepartmentDelete => "DepartmentDelete",
            Action::DepartmentRead => "DepartmentRead",
            Action::TeamCreate => "TeamCreate",
            Action::TeamUpdate => "TeamUpdate",
            Action::TeamDelete => "TeamDelete",
            Action::TeamRead => "TeamRead",
            Action::ProjectCreate => "ProjectCreate",
            Action::ProjectUpdate => "ProjectUpdate",
            Action::ProjectDelete => "ProjectDelete",
            Action::ProjectRead => "ProjectRead",
            Action::NoteCreate => "NoteCreate",
            Action::NoteUpdate => "NoteUpdate",
            Action::NoteDelete => "NoteDelete",
            Action::NoteRead => "NoteRead",
            Action::NoteWrite => "NoteWrite",
            Action::MembershipCreate => "MembershipCreate",
            Action::MembershipUpdate => "MembershipUpdate",
            Action::MembershipDelete => "MembershipDelete",
            Action::MembershipRead => "MembershipRead",
            Action::TeamMemberCreate => "TeamMemberCreate",
            Action::TeamMemberDelete => "TeamMemberDelete",
            Action::TeamMemberRead => "TeamMemberRead",
            Action::TeamProjectCreate => "TeamProjectCreate",
            Action::TeamProjectDelete => "TeamProjectDelete",
            Action::TeamProjectRead => "TeamProjectRead",
            Action::InviteCreate => "InviteCreate",
            Action::TempPasswordIssue => "TempPasswordIssue",
            Action::Export => "Export",
            Action::Import => "Import",
            Action::AdminUsersList => "AdminUsersList",
        }
    }
}

#[derive(Debug, Clone)]
pub enum Scope {
    System,
    Org(OrgId),
    Project { project_id: ProjectId, org_id: OrgId },
    Note { note_id: NoteId, project_id: ProjectId, org_id: OrgId },
    User(UserId),
}

impl Scope {
    pub fn type_name(&self) -> &'static str {
        match self {
            Scope::System => "System",
            Scope::Org(_) => "Org",
            Scope::Project { .. } => "Project",
            Scope::Note { .. } => "Note",
            Scope::User(_) => "User",
        }
    }

    pub fn id_str(&self) -> Option<String> {
        match self {
            Scope::System => None,
            Scope::Org(o) => Some(o.to_string()),
            Scope::Project { project_id, .. } => Some(project_id.to_string()),
            Scope::Note { note_id, .. } => Some(note_id.to_string()),
            Scope::User(u) => Some(u.to_string()),
        }
    }

    pub fn org_id(&self) -> Option<&OrgId> {
        match self {
            Scope::Org(o) => Some(o),
            Scope::Project { org_id, .. } => Some(org_id),
            Scope::Note { org_id, .. } => Some(org_id),
            _ => None,
        }
    }
}

fn role_of(identity: &Identity) -> Option<Role> {
    identity.role_in_active_org
}

fn org_matches(identity: &Identity, scope: &Scope) -> bool {
    match scope.org_id() {
        Some(o) => identity.active_org_id.as_ref() == Some(o),
        None => true,
    }
}

/// Phase-3 truth table. Returns Ok if the action is permitted, Forbidden
/// otherwise. For project-/note-scoped read or write by `user` role, callers
/// must use [`check_note`] passing a pre-computed `has_team_access`.
pub fn check(identity: &Identity, action: &Action, scope: &Scope) -> Result<(), AuthError> {
    let role = role_of(identity);

    // Master admin: blanket allow except a couple of resource-bound deny cases.
    if matches!(role, Some(Role::MasterAdmin)) {
        return Ok(());
    }

    use Action::*;
    match action {
        // System / cross-org actions: master_admin only.
        OrgCreate | OrgDelete | TempPasswordIssue | AdminUsersList => Err(forbidden(action)),

        // Org_admin in their own org.
        OrgUpdate
        | OrgRead
        | DepartmentCreate
        | DepartmentUpdate
        | DepartmentDelete
        | DepartmentRead
        | TeamCreate
        | TeamUpdate
        | TeamDelete
        | TeamRead
        | ProjectCreate
        | ProjectUpdate
        | ProjectDelete
        | MembershipCreate
        | MembershipUpdate
        | MembershipDelete
        | MembershipRead
        | TeamMemberCreate
        | TeamMemberDelete
        | TeamMemberRead
        | TeamProjectCreate
        | TeamProjectDelete
        | TeamProjectRead
        | InviteCreate
        | Export
        | Import => match role {
            Some(Role::OrgAdmin) if org_matches(identity, scope) => Ok(()),
            _ => Err(forbidden(action)),
        },

        // Project / Note read+write: handled in check_note for fine-grained
        // team-access. The pure `check` defaults to org_admin same-org Ok and
        // anything else Forbidden so callers cannot fall through accidentally.
        ProjectRead | NoteRead | NoteWrite | NoteCreate | NoteUpdate | NoteDelete => match role {
            Some(Role::OrgAdmin) if org_matches(identity, scope) => Ok(()),
            _ => Err(forbidden(action)),
        },
    }
}

/// Project- and note-scoped actions where the `user` role gains access via
/// team membership. Callers compute `has_team_access` from `team_members`
/// joined with `team_projects` for the target project, then call this.
pub fn check_note(
    identity: &Identity,
    action: &Action,
    scope: &Scope,
    has_team_access: bool,
) -> Result<(), AuthError> {
    let role = role_of(identity);
    if matches!(role, Some(Role::MasterAdmin)) {
        return Ok(());
    }
    if matches!(role, Some(Role::OrgAdmin)) && org_matches(identity, scope) {
        return Ok(());
    }
    if matches!(role, Some(Role::User)) && org_matches(identity, scope) && has_team_access {
        return Ok(());
    }
    Err(forbidden(action))
}

/// Convenience: does this identity hold a master_admin role in its active org?
pub fn is_master_admin(identity: &Identity) -> bool {
    matches!(identity.role_in_active_org, Some(Role::MasterAdmin))
}

fn forbidden(action: &Action) -> AuthError {
    let leak: &'static str = match action {
        Action::OrgCreate => "OrgCreate",
        Action::OrgDelete => "OrgDelete",
        Action::OrgUpdate => "OrgUpdate",
        Action::OrgRead => "OrgRead",
        _ => "Forbidden",
    };
    AuthError::Forbidden(leak)
}

#[cfg(test)]
mod tests {
    use super::*;
    use operon_store::ids::SYSTEM_ORG_ID;
    use std::str::FromStr;

    fn id(role: Option<Role>, active: Option<OrgId>) -> Identity {
        use operon_store::SessionId;
        Identity {
            user_id: UserId::new(),
            session_id: SessionId::new(),
            active_org_id: active,
            role_in_active_org: role,
            must_change_password: false,
        }
    }

    fn system_org() -> OrgId {
        OrgId::from_str(SYSTEM_ORG_ID).unwrap()
    }

    #[test]
    fn master_admin_allowed_everywhere() {
        let i = id(Some(Role::MasterAdmin), Some(system_org()));
        assert!(check(&i, &Action::OrgCreate, &Scope::System).is_ok());
        assert!(check(&i, &Action::DepartmentDelete, &Scope::Org(OrgId::new())).is_ok());
    }

    #[test]
    fn org_admin_blocked_cross_org() {
        let i = id(Some(Role::OrgAdmin), Some(OrgId::new()));
        let other = Scope::Org(OrgId::new());
        assert!(check(&i, &Action::DepartmentCreate, &other).is_err());
    }

    #[test]
    fn org_admin_allowed_same_org() {
        let org = OrgId::new();
        let i = id(Some(Role::OrgAdmin), Some(org.clone()));
        assert!(check(&i, &Action::DepartmentCreate, &Scope::Org(org)).is_ok());
    }

    #[test]
    fn user_blocked_for_admin_actions() {
        let org = OrgId::new();
        let i = id(Some(Role::User), Some(org.clone()));
        assert!(check(&i, &Action::ProjectCreate, &Scope::Org(org.clone())).is_err());
        assert!(check(&i, &Action::DepartmentCreate, &Scope::Org(org)).is_err());
    }

    #[test]
    fn user_with_team_access_can_read_project() {
        let org = OrgId::new();
        let project = ProjectId::new();
        let i = id(Some(Role::User), Some(org.clone()));
        let scope = Scope::Project {
            project_id: project,
            org_id: org,
        };
        assert!(check_note(&i, &Action::ProjectRead, &scope, true).is_ok());
        assert!(check_note(&i, &Action::ProjectRead, &scope, false).is_err());
    }

    #[test]
    fn no_role_is_always_forbidden() {
        let i = id(None, None);
        assert!(check(&i, &Action::OrgRead, &Scope::Org(OrgId::new())).is_err());
    }
}
