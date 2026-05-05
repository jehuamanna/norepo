use operon_store::repos::membership::Role;
use operon_store::{OrgId, SessionId, UserId};
use serde::{Deserialize, Serialize};

/// Resolved identity attached to every authenticated request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub user_id: UserId,
    pub session_id: SessionId,
    pub active_org_id: Option<OrgId>,
    pub role_in_active_org: Option<Role>,
    pub must_change_password: bool,
}

impl Identity {
    pub fn synthetic_local(user_id: UserId, org_id: OrgId) -> Self {
        Self {
            user_id,
            session_id: SessionId::new(),
            active_org_id: Some(org_id),
            role_in_active_org: Some(Role::OrgAdmin),
            must_change_password: false,
        }
    }
}
