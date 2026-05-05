use serde::{Deserialize, Serialize};

/// Mirror of `operon-api-server`'s `GET /api/me` response.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MePayload {
    pub user_id: String,
    pub active_org_id: Option<String>,
    pub role_in_active_org: Option<String>,
    pub memberships: Vec<MembershipBrief>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MembershipBrief {
    pub org_id: String,
    pub role: String,
    pub department_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LoginResponse {
    Ok {
        session_token: String,
        user_id: String,
        active_org_id: Option<String>,
    },
    MustChangePassword {
        reset_token: String,
    },
}

/// Project DTO mirror.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectBrief {
    pub id: String,
    pub org_id: String,
    pub name: String,
}

/// Note DTO mirror.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NoteBrief {
    pub id: String,
    pub project_id: String,
    pub parent_id: Option<String>,
    pub title: String,
    pub sibling_index: i64,
}
