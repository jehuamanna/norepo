use std::str::FromStr;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use operon_auth::rbac::{Action, Scope};
use operon_auth::Identity;
use operon_store::repos::membership::MembershipRepository;
use operon_store::repos::org::OrgRepository;
use operon_store::repos::project::{Project, ProjectRepository};
use operon_store::repos::team_member::TeamMemberRepository;
use operon_store::repos::team_project::TeamProjectRepository;
use operon_store::time::now_ms;
use operon_store::{OrgId, ProjectId};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::error::ApiError;
use crate::permissions;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/projects", post(create))
        .route(
            "/api/projects/{id}",
            get(read).patch(update).delete(remove),
        )
        .route("/api/orgs/{org_id}/projects", get(list_by_org))
        .route("/api/me/projects", get(my_projects))
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

impl From<Project> for Dto {
    fn from(p: Project) -> Self {
        Self {
            id: p.id.to_string(),
            org_id: p.org_id.to_string(),
            name: p.name,
            created_at_ms: p.created_at_ms,
            updated_at_ms: p.updated_at_ms,
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
        Action::ProjectCreate,
        Scope::Org(org_id.clone()),
    )?;
    let p = Project::new(org_id, req.name);
    state.projects.create(&p)?;
    Ok((StatusCode::CREATED, Json(p.into())))
}

async fn read(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<Dto>, ApiError> {
    let project_id =
        ProjectId::from_str(&id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let project = state.projects.get(&project_id)?.ok_or(ApiError::NotFound)?;
    let scope = Scope::Project {
        project_id: project.id.clone(),
        org_id: project.org_id.clone(),
    };
    let access = permissions::has_team_access(&state, &identity, &project.id)?;
    permissions::require_note(&state, &identity, Action::ProjectRead, scope, access)?;
    Ok(Json(project.into()))
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
    let project_id =
        ProjectId::from_str(&id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let mut project = state.projects.get(&project_id)?.ok_or(ApiError::NotFound)?;
    permissions::require(
        &state,
        &identity,
        Action::ProjectUpdate,
        Scope::Org(project.org_id.clone()),
    )?;
    if let Some(n) = req.name {
        project.name = n;
    }
    project.updated_at_ms = now_ms();
    state.projects.update(&project)?;
    Ok(Json(project.into()))
}

async fn remove(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let project_id =
        ProjectId::from_str(&id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let project = state.projects.get(&project_id)?.ok_or(ApiError::NotFound)?;
    permissions::require(
        &state,
        &identity,
        Action::ProjectDelete,
        Scope::Org(project.org_id.clone()),
    )?;
    state.projects.delete(&project_id)?;
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
        Action::ProjectRead,
        Scope::Org(org.clone()),
    )?;
    let projects = state.projects.list_by_org(&org)?;
    Ok(Json(projects.into_iter().map(Dto::from).collect()))
}

/// `GET /api/me/projects` — the projects the caller can read.
/// master_admin: every project. org_admin: every project in active org.
/// user: projects whose teams the caller is a member of.
async fn my_projects(
    State(state): State<AppState>,
    identity: Identity,
) -> Result<Json<Vec<Dto>>, ApiError> {
    use operon_auth::rbac;
    let mut projects: Vec<Project> = Vec::new();
    if rbac::is_master_admin(&identity) {
        // Every org's projects.
        let orgs = state.orgs.list(1000, None)?;
        for o in orgs {
            projects.extend(state.projects.list_by_org(&o.id)?);
        }
    } else if matches!(
        identity.role_in_active_org,
        Some(operon_store::repos::membership::Role::OrgAdmin)
    ) {
        if let Some(o) = &identity.active_org_id {
            projects.extend(state.projects.list_by_org(o)?);
        }
    } else {
        // user: union of team_projects for every team the user belongs to.
        let memberships = state.memberships.by_user(&identity.user_id)?;
        let mut seen = HashSet::new();
        for m in memberships {
            let team_members = state.team_members.list_by_membership(&m.id)?;
            for tm in team_members {
                let assigns = state.team_projects.list_by_team(&tm.team_id)?;
                for a in assigns {
                    if seen.insert(a.project_id.to_string()) {
                        if let Some(p) = state.projects.get(&a.project_id)? {
                            projects.push(p);
                        }
                    }
                }
            }
        }
    }
    Ok(Json(projects.into_iter().map(Dto::from).collect()))
}
