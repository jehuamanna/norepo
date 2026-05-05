use std::str::FromStr;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use operon_auth::Identity;
use operon_store::repos::membership::MembershipRepository;
use operon_store::repos::session::SessionRepository;
use operon_store::OrgId;
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/session/active-org", post(set_active_org))
}

#[derive(Deserialize)]
pub struct SetActiveOrgReq {
    pub org_id: String,
}

async fn set_active_org(
    State(state): State<AppState>,
    identity: Identity,
    Json(req): Json<SetActiveOrgReq>,
) -> Result<StatusCode, ApiError> {
    let org_id = OrgId::from_str(&req.org_id)
        .map_err(|_| ApiError::BadRequest("org_id is not a valid uuid".into()))?;
    state
        .memberships
        .by_user_org(&identity.user_id, &org_id)?
        .ok_or(ApiError::Forbidden)?;
    state
        .sessions
        .set_active_org(&identity.session_id, Some(&org_id))?;
    Ok(StatusCode::NO_CONTENT)
}
