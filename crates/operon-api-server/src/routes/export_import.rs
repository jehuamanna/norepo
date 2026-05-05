use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use dashmap::DashMap;
use operon_auth::rbac::{Action, Scope};
use operon_auth::Identity;
use operon_export::exporter::ExportContext;
use operon_export::importer::{ImportContext, ImportOptions};
use operon_export::manifest::Manifest;
use operon_export::ImportReport;
use operon_store::time::now_ms;
use operon_store::OrgId;
use serde::Serialize;
use uuid::Uuid;

use crate::error::ApiError;
use crate::permissions;
use crate::state::AppState;

const TOKEN_TTL_MS: i64 = 60 * 60 * 1000;

#[derive(Clone, Default)]
pub struct ExportTokenStore {
    inner: Arc<DashMap<String, ExportToken>>,
}

#[derive(Clone)]
struct ExportToken {
    path: PathBuf,
    expires_at_ms: i64,
}

impl ExportTokenStore {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn issue(&self, path: PathBuf) -> String {
        let token = Uuid::new_v4().to_string();
        self.inner.insert(
            token.clone(),
            ExportToken {
                path,
                expires_at_ms: now_ms() + TOKEN_TTL_MS,
            },
        );
        token
    }
    pub fn consume(&self, token: &str) -> Option<PathBuf> {
        let entry = self.inner.remove(token)?;
        let (_, t) = entry;
        if t.expires_at_ms < now_ms() {
            return None;
        }
        Some(t.path)
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/orgs/{org_id}/export", post(export))
        .route("/api/exports/{token}", get(download))
        .route("/api/orgs/{org_id}/import", post(import))
}

#[derive(Serialize)]
struct ExportResp {
    download_url: String,
    expires_at_ms: i64,
}

async fn export(
    State(state): State<AppState>,
    identity: Identity,
    Path(org_id): Path<String>,
) -> Result<(StatusCode, Json<ExportResp>), ApiError> {
    let org = OrgId::from_str(&org_id).map_err(|_| ApiError::BadRequest("bad org_id".into()))?;
    permissions::require(&state, &identity, Action::Export, Scope::Org(org.clone()))?;

    let dest = std::env::temp_dir().join(format!("opn-{}.opnpkg", Uuid::new_v4()));
    let ctx = ExportContext {
        store: &state.store,
        orgs: state.orgs.as_ref(),
        departments: state.departments.as_ref(),
        teams: state.teams.as_ref(),
        projects: state.projects.as_ref(),
        notes: state.notes.as_ref(),
        memberships: state.memberships.as_ref(),
        team_members: state.team_members.as_ref(),
        team_projects: state.team_projects.as_ref(),
        users: state.users.as_ref(),
    };
    operon_export::export_org(&ctx, &org, &dest).map_err(|e| ApiError::Internal(e.to_string()))?;

    let token = state.export_tokens.issue(dest);
    Ok((
        StatusCode::ACCEPTED,
        Json(ExportResp {
            download_url: format!("/api/exports/{token}"),
            expires_at_ms: now_ms() + TOKEN_TTL_MS,
        }),
    ))
}

async fn download(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Response, ApiError> {
    let path = state.export_tokens.consume(&token).ok_or(ApiError::Gone)?;
    let bytes = tokio::fs::read(&path).await.map_err(|_| ApiError::NotFound)?;
    let _ = tokio::fs::remove_file(&path).await;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        "application/octet-stream".parse().unwrap(),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{token}.opnpkg\"")
            .parse()
            .unwrap(),
    );
    Ok((StatusCode::OK, headers, Bytes::from(bytes)).into_response())
}

async fn import(
    State(state): State<AppState>,
    identity: Identity,
    Path(org_id): Path<String>,
    bytes: Bytes,
) -> Result<Json<ImportReport>, ApiError> {
    let org = OrgId::from_str(&org_id).map_err(|_| ApiError::BadRequest("bad org_id".into()))?;
    permissions::require(&state, &identity, Action::Import, Scope::Org(org.clone()))?;

    if bytes.len() as u64 > 2 * 1024 * 1024 * 1024 {
        return Err(ApiError::BadRequest("payload_too_large".into()));
    }

    let tmp = std::env::temp_dir().join(format!("opn-import-{}.opnpkg", Uuid::new_v4()));
    tokio::fs::write(&tmp, &bytes)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let ctx = ImportContext {
        orgs: state.orgs.as_ref(),
        departments: state.departments.as_ref(),
        teams: state.teams.as_ref(),
        projects: state.projects.as_ref(),
        notes: state.notes.as_ref(),
        memberships: state.memberships.as_ref(),
        team_members: state.team_members.as_ref(),
        team_projects: state.team_projects.as_ref(),
        users: state.users.as_ref(),
        hub: Some(state.hub.as_ref()),
    };
    let opts = ImportOptions {
        allow_cross_org: matches!(
            identity.role_in_active_org,
            Some(operon_store::repos::membership::Role::MasterAdmin)
        ),
        overwrite_local_markdown_on_collision: false,
    };
    let report = operon_export::import_archive(&ctx, &tmp, &org, &opts)
        .await
        .map_err(|e| match e {
            operon_export::ExportError::UnknownFormatVersion(_) => {
                ApiError::BadRequest("unknown_format_version".into())
            }
            operon_export::ExportError::SchemaTooNew { .. } => ApiError::Conflict("schema_too_new"),
            operon_export::ExportError::CrossOrgPayload => {
                ApiError::BadRequest("cross_org_payload".into())
            }
            other => ApiError::Internal(other.to_string()),
        })?;
    let _ = tokio::fs::remove_file(&tmp).await;

    Ok(Json(report))
}

#[allow(dead_code)]
fn _unused() {
    let _ = Manifest {
        format_version: 1,
        schema_version: 1,
        source_org_id: String::new(),
        source_org_name: String::new(),
        source_flavour: String::new(),
        exported_at_ms: 0,
        exporter_user_id: None,
        entity_counts: Default::default(),
    };
}
