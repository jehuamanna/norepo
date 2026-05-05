use std::str::FromStr;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use operon_auth::rbac::{Action, Scope};
use operon_auth::Identity;
use operon_store::repos::team::{Team, TeamRepository};
use operon_store::time::now_ms;
use operon_store::{OrgId, TeamId};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::permissions;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/teams", post(create))
        .route("/api/teams/{id}", get(read).patch(update).delete(remove))
        .route("/api/orgs/{org_id}/teams", get(list_by_org))
}

#[derive(Deserialize)]
pub struct CreateReq {
    pub name: String,
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

impl From<Team> for Dto {
    fn from(t: Team) -> Self {
        Self {
            id: t.id.to_string(),
            org_id: t.org_id.to_string(),
            name: t.name,
            created_at_ms: t.created_at_ms,
            updated_at_ms: t.updated_at_ms,
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
        Action::TeamCreate,
        Scope::Org(org_id.clone()),
    )?;
    let t = Team::new(org_id, req.name);
    state.teams.create(&t)?;
    Ok((StatusCode::CREATED, Json(t.into())))
}

async fn read(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<Dto>, ApiError> {
    let team_id = TeamId::from_str(&id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let team = state.teams.get(&team_id)?.ok_or(ApiError::NotFound)?;
    permissions::require(
        &state,
        &identity,
        Action::TeamRead,
        Scope::Org(team.org_id.clone()),
    )?;
    Ok(Json(team.into()))
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
    let team_id = TeamId::from_str(&id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let mut team = state.teams.get(&team_id)?.ok_or(ApiError::NotFound)?;
    permissions::require(
        &state,
        &identity,
        Action::TeamUpdate,
        Scope::Org(team.org_id.clone()),
    )?;
    if let Some(n) = req.name {
        team.name = n;
    }
    team.updated_at_ms = now_ms();
    state.teams.update(&team)?;
    Ok(Json(team.into()))
}

async fn remove(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let team_id = TeamId::from_str(&id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let team = state.teams.get(&team_id)?.ok_or(ApiError::NotFound)?;
    permissions::require(
        &state,
        &identity,
        Action::TeamDelete,
        Scope::Org(team.org_id.clone()),
    )?;
    state.teams.delete(&team_id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_by_org(
    State(state): State<AppState>,
    identity: Identity,
    Path(org_id): Path<String>,
) -> Result<Json<Vec<Dto>>, ApiError> {
    let org = OrgId::from_str(&org_id).map_err(|_| ApiError::BadRequest("bad org_id".into()))?;
    permissions::require(&state, &identity, Action::TeamRead, Scope::Org(org.clone()))?;
    let teams = state.teams.list_by_org(&org)?;
    Ok(Json(teams.into_iter().map(Dto::from).collect()))
}
