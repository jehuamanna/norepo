use std::str::FromStr;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use operon_auth::password;
use operon_auth::session as auth_session;
use operon_auth::tempassword;
use operon_auth::Identity;
use operon_store::repos::invite::{Invite, InviteRepository};
use operon_store::repos::membership::{MembershipRepository, Role};
use operon_store::repos::user::UserRepository;
use operon_store::time::now_ms;
use operon_store::{DepartmentId, InviteId, OrgId, UserId};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

const INVITE_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000; // 7 days

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/admin/invites", post(create_invite))
        .route("/api/admin/temp-password", post(issue_temp_password))
}

#[derive(Deserialize)]
pub struct CreateInviteReq {
    pub email: String,
    pub org_id: String,
    pub role: String,
    pub department_id: Option<String>,
}

#[derive(Serialize)]
struct CreateInviteResp {
    invite_id: String,
    expires_at_ms: i64,
    /// Cleartext token. Production server emails it; tests use this directly.
    invite_token: String,
}

async fn create_invite(
    State(state): State<AppState>,
    identity: Identity,
    Json(req): Json<CreateInviteReq>,
) -> Result<(StatusCode, Json<CreateInviteResp>), ApiError> {
    let role = Role::from_str(&req.role).map_err(|_| ApiError::BadRequest("bad role".into()))?;
    let org_id =
        OrgId::from_str(&req.org_id).map_err(|_| ApiError::BadRequest("bad org_id".into()))?;
    let department_id = match req.department_id.as_deref() {
        Some(s) => Some(
            DepartmentId::from_str(s)
                .map_err(|_| ApiError::BadRequest("bad department_id".into()))?,
        ),
        None => None,
    };

    // Permission check: master_admin → any org/role; org_admin → own org, role=user only.
    let caller_role = identity.role_in_active_org;
    match caller_role {
        Some(Role::MasterAdmin) => {}
        Some(Role::OrgAdmin) => {
            if identity.active_org_id.as_ref() != Some(&org_id) {
                return Err(ApiError::Forbidden);
            }
            if role != Role::User {
                return Err(ApiError::Forbidden);
            }
        }
        _ => return Err(ApiError::Forbidden),
    }

    let token = auth_session::generate_token();
    let invite = Invite {
        id: InviteId::new(),
        email: req.email.trim().to_string(),
        org_id,
        role,
        department_id,
        invited_by: identity.user_id.clone(),
        token_hash: auth_session::hash_token(&token),
        expires_at_ms: now_ms() + INVITE_TTL_MS,
        accepted_at_ms: None,
        created_at_ms: now_ms(),
    };
    state.invites.create(&invite)?;

    // Email
    let url = format!("https://{}/invite/{}", state.hostname, token);
    let _ = state
        .email
        .send(
            &invite.email,
            "You're invited to Operon",
            &format!("<a href=\"{url}\">Accept invite</a>"),
            &format!("Accept invite: {url}"),
        )
        .await;

    Ok((
        StatusCode::CREATED,
        Json(CreateInviteResp {
            invite_id: invite.id.to_string(),
            expires_at_ms: invite.expires_at_ms,
            invite_token: token,
        }),
    ))
}

#[derive(Deserialize)]
pub struct IssueTempPasswordReq {
    pub user_id: String,
}

#[derive(Serialize)]
struct IssueTempPasswordResp {
    temp_password: String,
}

async fn issue_temp_password(
    State(state): State<AppState>,
    identity: Identity,
    Json(req): Json<IssueTempPasswordReq>,
) -> Result<Json<IssueTempPasswordResp>, ApiError> {
    if !matches!(identity.role_in_active_org, Some(Role::MasterAdmin)) {
        return Err(ApiError::Forbidden);
    }
    let target_id =
        UserId::from_str(&req.user_id).map_err(|_| ApiError::BadRequest("bad user_id".into()))?;
    let mut target = state.users.get(&target_id)?.ok_or(ApiError::NotFound)?;

    // Target must currently be a master_admin or org_admin.
    let target_memberships = state.memberships.by_user(&target.id)?;
    let is_admin = target_memberships
        .iter()
        .any(|m| matches!(m.role, Role::MasterAdmin | Role::OrgAdmin));
    if !is_admin {
        return Err(ApiError::Forbidden);
    }

    let temp = tempassword::generate();
    target.password_hash = Some(password::hash(&temp)?);
    target.updated_at_ms = now_ms();
    state.users.update(&target)?;
    let _ = state.store.conn()?.execute(
        "UPDATE users SET must_change_password = 1 WHERE id = ?1",
        rusqlite::params![target.id.as_str()],
    );

    Ok(Json(IssueTempPasswordResp { temp_password: temp }))
}
