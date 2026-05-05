use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use operon_auth::Identity;
use operon_store::repos::membership::MembershipRepository;
use serde::Serialize;

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/me", get(me))
}

#[derive(Serialize)]
struct MePayload {
    user_id: String,
    active_org_id: Option<String>,
    role_in_active_org: Option<String>,
    memberships: Vec<MembershipBrief>,
}

#[derive(Serialize)]
struct MembershipBrief {
    org_id: String,
    role: String,
    department_id: Option<String>,
}

async fn me(
    State(state): State<AppState>,
    identity: Identity,
) -> Result<Json<MePayload>, ApiError> {
    let memberships = state.memberships.by_user(&identity.user_id)?;
    let memberships_brief = memberships
        .iter()
        .map(|m| MembershipBrief {
            org_id: m.org_id.to_string(),
            role: m.role.as_str().to_string(),
            department_id: m.department_id.as_ref().map(|d| d.to_string()),
        })
        .collect();
    Ok(Json(MePayload {
        user_id: identity.user_id.to_string(),
        active_org_id: identity.active_org_id.as_ref().map(|o| o.to_string()),
        role_in_active_org: identity
            .role_in_active_org
            .as_ref()
            .map(|r| r.as_str().to_string()),
        memberships: memberships_brief,
    }))
}
