use std::str::FromStr;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use operon_auth::rbac::{Action, Scope};
use operon_auth::Identity;
use operon_store::repos::org::{Org, OrgFlavour, OrgRepository};
use operon_store::time::now_ms;
use operon_store::OrgId;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::permissions;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/orgs", post(create).get(list))
        .route("/api/orgs/{id}", get(read).patch(update).delete(remove))
}

#[derive(Deserialize)]
pub struct CreateOrgReq {
    pub name: String,
    pub flavour: Option<String>,
}

#[derive(Serialize)]
pub struct OrgDto {
    pub id: String,
    pub name: String,
    pub flavour: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl From<Org> for OrgDto {
    fn from(o: Org) -> Self {
        Self {
            id: o.id.to_string(),
            name: o.name,
            flavour: o.flavour.as_str().to_string(),
            created_at_ms: o.created_at_ms,
            updated_at_ms: o.updated_at_ms,
        }
    }
}

async fn create(
    State(state): State<AppState>,
    identity: Identity,
    Json(req): Json<CreateOrgReq>,
) -> Result<(StatusCode, Json<OrgDto>), ApiError> {
    permissions::require(&state, &identity, Action::OrgCreate, Scope::System)?;
    let flavour = match req.flavour.as_deref() {
        Some(s) => operon_store::repos::org::OrgFlavour::from_str(s)
            .map_err(|e| ApiError::BadRequest(e.to_string()))?,
        None => OrgFlavour::NonLocal,
    };
    let org = Org::new(req.name, flavour);
    state.orgs.create(&org)?;
    Ok((StatusCode::CREATED, Json(org.into())))
}

async fn read(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<OrgDto>, ApiError> {
    let org_id = OrgId::from_str(&id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    permissions::require(&state, &identity, Action::OrgRead, Scope::Org(org_id.clone()))?;
    let org = state.orgs.get(&org_id)?.ok_or(ApiError::NotFound)?;
    Ok(Json(org.into()))
}

#[derive(Deserialize)]
pub struct UpdateOrgReq {
    pub name: Option<String>,
}

async fn update(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    Json(req): Json<UpdateOrgReq>,
) -> Result<Json<OrgDto>, ApiError> {
    let org_id = OrgId::from_str(&id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    permissions::require(&state, &identity, Action::OrgUpdate, Scope::Org(org_id.clone()))?;
    let mut org = state.orgs.get(&org_id)?.ok_or(ApiError::NotFound)?;
    if let Some(n) = req.name {
        org.name = n;
    }
    org.updated_at_ms = now_ms();
    state.orgs.update(&org)?;
    Ok(Json(org.into()))
}

async fn remove(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let org_id = OrgId::from_str(&id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    permissions::require(&state, &identity, Action::OrgDelete, Scope::Org(org_id.clone()))?;
    state.orgs.delete(&org_id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list(
    State(state): State<AppState>,
    identity: Identity,
) -> Result<Json<Vec<OrgDto>>, ApiError> {
    permissions::require(&state, &identity, Action::OrgRead, Scope::System)?;
    let orgs = state.orgs.list(50, None)?;
    Ok(Json(orgs.into_iter().map(OrgDto::from).collect()))
}

