//! Team-member and team-project assignment routes.

use std::str::FromStr;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use operon_auth::rbac::{Action, Scope};
use operon_auth::Identity;
use operon_store::repos::membership::MembershipRepository;
use operon_store::repos::team::TeamRepository;
use operon_store::repos::team_member::{TeamMember, TeamMemberRepository};
use operon_store::repos::team_project::{TeamProject, TeamProjectRepository};
use operon_store::repos::project::ProjectRepository;
use operon_store::{MembershipId, ProjectId, TeamId, TeamMemberId, TeamProjectId};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::permissions;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/team-members", post(create_member))
        .route("/api/team-members/{id}", delete(remove_member))
        .route("/api/teams/{team_id}/members", get(list_members))
        .route("/api/team-projects", post(create_assignment))
        .route("/api/team-projects/{id}", delete(remove_assignment))
        .route("/api/teams/{team_id}/projects", get(list_projects_of_team))
        .route("/api/projects/{project_id}/teams", get(list_teams_of_project))
}

#[derive(Deserialize)]
pub struct CreateMemberReq {
    pub membership_id: String,
    pub team_id: String,
}

#[derive(Serialize)]
pub struct MemberDto {
    pub id: String,
    pub membership_id: String,
    pub team_id: String,
    pub created_at_ms: i64,
}

impl From<TeamMember> for MemberDto {
    fn from(t: TeamMember) -> Self {
        Self {
            id: t.id.to_string(),
            membership_id: t.membership_id.to_string(),
            team_id: t.team_id.to_string(),
            created_at_ms: t.created_at_ms,
        }
    }
}

async fn create_member(
    State(state): State<AppState>,
    identity: Identity,
    Json(req): Json<CreateMemberReq>,
) -> Result<(StatusCode, Json<MemberDto>), ApiError> {
    let team_id =
        TeamId::from_str(&req.team_id).map_err(|_| ApiError::BadRequest("bad team_id".into()))?;
    let membership_id = MembershipId::from_str(&req.membership_id)
        .map_err(|_| ApiError::BadRequest("bad membership_id".into()))?;
    let team = state.teams.get(&team_id)?.ok_or(ApiError::NotFound)?;
    let membership = state
        .memberships
        .get(&membership_id)?
        .ok_or(ApiError::NotFound)?;
    if team.org_id != membership.org_id {
        return Err(ApiError::BadRequest("cross_org_team".into()));
    }
    permissions::require(
        &state,
        &identity,
        Action::TeamMemberCreate,
        Scope::Org(team.org_id.clone()),
    )?;
    let tm = TeamMember::new(membership_id, team_id);
    state.team_members.create(&tm)?;
    Ok((StatusCode::CREATED, Json(tm.into())))
}

async fn remove_member(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = TeamMemberId::from_str(&id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let tm = state.team_members.get(&id)?.ok_or(ApiError::NotFound)?;
    let team = state.teams.get(&tm.team_id)?.ok_or(ApiError::NotFound)?;
    permissions::require(
        &state,
        &identity,
        Action::TeamMemberDelete,
        Scope::Org(team.org_id),
    )?;
    state.team_members.delete(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_members(
    State(state): State<AppState>,
    identity: Identity,
    Path(team_id): Path<String>,
) -> Result<Json<Vec<MemberDto>>, ApiError> {
    let team_id = TeamId::from_str(&team_id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let team = state.teams.get(&team_id)?.ok_or(ApiError::NotFound)?;
    permissions::require(
        &state,
        &identity,
        Action::TeamMemberRead,
        Scope::Org(team.org_id),
    )?;
    let members = state.team_members.list_by_team(&team_id)?;
    Ok(Json(members.into_iter().map(MemberDto::from).collect()))
}

#[derive(Deserialize)]
pub struct CreateAssignmentReq {
    pub team_id: String,
    pub project_id: String,
}

#[derive(Serialize)]
pub struct AssignmentDto {
    pub id: String,
    pub team_id: String,
    pub project_id: String,
    pub created_at_ms: i64,
}

impl From<TeamProject> for AssignmentDto {
    fn from(t: TeamProject) -> Self {
        Self {
            id: t.id.to_string(),
            team_id: t.team_id.to_string(),
            project_id: t.project_id.to_string(),
            created_at_ms: t.created_at_ms,
        }
    }
}

async fn create_assignment(
    State(state): State<AppState>,
    identity: Identity,
    Json(req): Json<CreateAssignmentReq>,
) -> Result<(StatusCode, Json<AssignmentDto>), ApiError> {
    let team_id =
        TeamId::from_str(&req.team_id).map_err(|_| ApiError::BadRequest("bad team_id".into()))?;
    let project_id = ProjectId::from_str(&req.project_id)
        .map_err(|_| ApiError::BadRequest("bad project_id".into()))?;
    let team = state.teams.get(&team_id)?.ok_or(ApiError::NotFound)?;
    let project = state.projects.get(&project_id)?.ok_or(ApiError::NotFound)?;
    if team.org_id != project.org_id {
        return Err(ApiError::BadRequest("cross_org_team".into()));
    }
    permissions::require(
        &state,
        &identity,
        Action::TeamProjectCreate,
        Scope::Org(team.org_id.clone()),
    )?;
    let tp = TeamProject::new(team_id, project_id);
    state.team_projects.create(&tp)?;
    Ok((StatusCode::CREATED, Json(tp.into())))
}

async fn remove_assignment(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = TeamProjectId::from_str(&id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let tp = state.team_projects.get(&id)?.ok_or(ApiError::NotFound)?;
    let team = state.teams.get(&tp.team_id)?.ok_or(ApiError::NotFound)?;
    permissions::require(
        &state,
        &identity,
        Action::TeamProjectDelete,
        Scope::Org(team.org_id),
    )?;
    state.team_projects.delete(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_projects_of_team(
    State(state): State<AppState>,
    identity: Identity,
    Path(team_id): Path<String>,
) -> Result<Json<Vec<AssignmentDto>>, ApiError> {
    let team_id = TeamId::from_str(&team_id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let team = state.teams.get(&team_id)?.ok_or(ApiError::NotFound)?;
    permissions::require(
        &state,
        &identity,
        Action::TeamProjectRead,
        Scope::Org(team.org_id),
    )?;
    let xs = state.team_projects.list_by_team(&team_id)?;
    Ok(Json(xs.into_iter().map(AssignmentDto::from).collect()))
}

async fn list_teams_of_project(
    State(state): State<AppState>,
    identity: Identity,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<AssignmentDto>>, ApiError> {
    let project_id =
        ProjectId::from_str(&project_id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let project = state.projects.get(&project_id)?.ok_or(ApiError::NotFound)?;
    permissions::require(
        &state,
        &identity,
        Action::TeamProjectRead,
        Scope::Org(project.org_id),
    )?;
    let xs = state.team_projects.list_by_project(&project_id)?;
    Ok(Json(xs.into_iter().map(AssignmentDto::from).collect()))
}
