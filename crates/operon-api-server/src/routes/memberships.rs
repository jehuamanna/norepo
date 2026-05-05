use std::str::FromStr;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use operon_auth::rbac::{Action, Scope};
use operon_auth::Identity;
use operon_store::repos::membership::{Membership, MembershipRepository, Role};
use operon_store::time::now_ms;
use operon_store::{DepartmentId, MembershipId, OrgId, UserId};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::permissions;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/memberships", post(create))
        .route(
            "/api/memberships/{id}",
            axum::routing::patch(update).delete(remove),
        )
        .route("/api/orgs/{org_id}/memberships", get(list_by_org))
}

#[derive(Deserialize)]
pub struct CreateReq {
    pub user_id: String,
    pub org_id: Option<String>,
    pub role: String,
    pub department_id: Option<String>,
}

#[derive(Serialize)]
pub struct Dto {
    pub id: String,
    pub user_id: String,
    pub org_id: String,
    pub role: String,
    pub department_id: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl From<Membership> for Dto {
    fn from(m: Membership) -> Self {
        Self {
            id: m.id.to_string(),
            user_id: m.user_id.to_string(),
            org_id: m.org_id.to_string(),
            role: m.role.as_str().to_string(),
            department_id: m.department_id.as_ref().map(|d| d.to_string()),
            created_at_ms: m.created_at_ms,
            updated_at_ms: m.updated_at_ms,
        }
    }
}

fn resolve_org(identity: &Identity, supplied: Option<&str>) -> Result<OrgId, ApiError> {
    match supplied {
        Some(s) => OrgId::from_str(s).map_err(|_| ApiError::BadRequest("bad org_id".into())),
        None => identity
            .active_org_id
            .clone()
            .ok_or_else(|| ApiError::BadRequest("no active org".into())),
    }
}

async fn create(
    State(state): State<AppState>,
    identity: Identity,
    Json(req): Json<CreateReq>,
) -> Result<(StatusCode, Json<Dto>), ApiError> {
    let org_id = resolve_org(&identity, req.org_id.as_deref())?;
    let role = Role::from_str(&req.role).map_err(|_| ApiError::BadRequest("bad role".into()))?;

    permissions::require(
        &state,
        &identity,
        Action::MembershipCreate,
        Scope::Org(org_id.clone()),
    )?;

    // Org_admin can only create role=user.
    if matches!(
        identity.role_in_active_org,
        Some(operon_store::repos::membership::Role::OrgAdmin)
    ) && role != Role::User
    {
        return Err(ApiError::Forbidden);
    }

    let user_id =
        UserId::from_str(&req.user_id).map_err(|_| ApiError::BadRequest("bad user_id".into()))?;
    let department_id = match req.department_id.as_deref() {
        Some(s) => Some(
            DepartmentId::from_str(s)
                .map_err(|_| ApiError::BadRequest("bad department_id".into()))?,
        ),
        None => None,
    };
    let m = Membership::new(user_id, org_id, role, department_id)?;
    state.memberships.create(&m)?;
    Ok((StatusCode::CREATED, Json(m.into())))
}

#[derive(Deserialize)]
pub struct UpdateReq {
    pub role: Option<String>,
    pub department_id: Option<Option<String>>,
}

async fn update(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    Json(req): Json<UpdateReq>,
) -> Result<Json<Dto>, ApiError> {
    let m_id =
        MembershipId::from_str(&id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let mut m = state.memberships.get(&m_id)?.ok_or(ApiError::NotFound)?;
    permissions::require(
        &state,
        &identity,
        Action::MembershipUpdate,
        Scope::Org(m.org_id.clone()),
    )?;

    let mut new_role = m.role;
    if let Some(r) = req.role.as_deref() {
        new_role = Role::from_str(r).map_err(|_| ApiError::BadRequest("bad role".into()))?;
    }
    let mut new_dept = m.department_id.clone();
    if let Some(d) = req.department_id {
        new_dept = match d {
            Some(s) => Some(
                DepartmentId::from_str(&s)
                    .map_err(|_| ApiError::BadRequest("bad department_id".into()))?,
            ),
            None => None,
        };
    }
    if new_role != Role::MasterAdmin && new_dept.is_none() {
        return Err(ApiError::BadRequest(
            "department_id required when role != master_admin".into(),
        ));
    }

    // Last master_admin invariant.
    if m.role == Role::MasterAdmin && new_role != Role::MasterAdmin {
        let count = state.memberships.count_master_admins()?;
        if count <= 1 {
            return Err(ApiError::Conflict("last_master_admin"));
        }
    }

    m.role = new_role;
    m.department_id = new_dept;
    m.updated_at_ms = now_ms();
    state.memberships.update(&m)?;
    Ok(Json(m.into()))
}

async fn remove(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let m_id =
        MembershipId::from_str(&id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let m = state.memberships.get(&m_id)?.ok_or(ApiError::NotFound)?;
    permissions::require(
        &state,
        &identity,
        Action::MembershipDelete,
        Scope::Org(m.org_id.clone()),
    )?;
    if m.role == Role::MasterAdmin && state.memberships.count_master_admins()? <= 1 {
        return Err(ApiError::Conflict("last_master_admin"));
    }
    state.memberships.delete(&m_id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_by_org(
    State(state): State<AppState>,
    identity: Identity,
    Path(org_id): Path<String>,
) -> Result<Json<Vec<Dto>>, ApiError> {
    let org = OrgId::from_str(&org_id).map_err(|_| ApiError::BadRequest("bad org_id".into()))?;
    permissions::require(
        &state,
        &identity,
        Action::MembershipRead,
        Scope::Org(org.clone()),
    )?;
    let ms = state.memberships.by_org(&org)?;
    Ok(Json(ms.into_iter().map(Dto::from).collect()))
}
