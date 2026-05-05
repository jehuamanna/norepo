use std::str::FromStr;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use operon_auth::rbac::{Action, Scope};
use operon_auth::Identity;
use operon_store::repos::department::{Department, DepartmentRepository};
use operon_store::time::now_ms;
use operon_store::{DepartmentId, OrgId};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::permissions;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/departments", post(create))
        .route(
            "/api/departments/{id}",
            get(read).patch(update).delete(remove),
        )
        .route("/api/orgs/{org_id}/departments", get(list_by_org))
}

#[derive(Deserialize)]
pub struct CreateReq {
    pub name: String,
    /// Optional for org_admin (server fills active_org_id); required for master_admin.
    pub org_id: Option<String>,
}

#[derive(Serialize)]
pub struct Dto {
    pub id: String,
    pub org_id: String,
    pub name: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl From<Department> for Dto {
    fn from(d: Department) -> Self {
        Self {
            id: d.id.to_string(),
            org_id: d.org_id.to_string(),
            name: d.name,
            created_at_ms: d.created_at_ms,
            updated_at_ms: d.updated_at_ms,
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
    permissions::require(
        &state,
        &identity,
        Action::DepartmentCreate,
        Scope::Org(org_id.clone()),
    )?;
    let d = Department::new(org_id, req.name);
    state.departments.create(&d)?;
    Ok((StatusCode::CREATED, Json(d.into())))
}

async fn read(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<Dto>, ApiError> {
    let dept_id =
        DepartmentId::from_str(&id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let dept = state.departments.get(&dept_id)?.ok_or(ApiError::NotFound)?;
    permissions::require(
        &state,
        &identity,
        Action::DepartmentRead,
        Scope::Org(dept.org_id.clone()),
    )?;
    Ok(Json(dept.into()))
}

#[derive(Deserialize)]
pub struct UpdateReq {
    pub name: Option<String>,
}

async fn update(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    Json(req): Json<UpdateReq>,
) -> Result<Json<Dto>, ApiError> {
    let dept_id =
        DepartmentId::from_str(&id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let mut dept = state.departments.get(&dept_id)?.ok_or(ApiError::NotFound)?;
    permissions::require(
        &state,
        &identity,
        Action::DepartmentUpdate,
        Scope::Org(dept.org_id.clone()),
    )?;
    if let Some(n) = req.name {
        dept.name = n;
    }
    dept.updated_at_ms = now_ms();
    state.departments.update(&dept)?;
    Ok(Json(dept.into()))
}

async fn remove(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let dept_id =
        DepartmentId::from_str(&id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let dept = state.departments.get(&dept_id)?.ok_or(ApiError::NotFound)?;
    permissions::require(
        &state,
        &identity,
        Action::DepartmentDelete,
        Scope::Org(dept.org_id.clone()),
    )?;
    state.departments.delete(&dept_id)?;
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
        Action::DepartmentRead,
        Scope::Org(org.clone()),
    )?;
    let depts = state.departments.list_by_org(&org)?;
    Ok(Json(depts.into_iter().map(Dto::from).collect()))
}
