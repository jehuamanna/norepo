use std::str::FromStr;

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::{SinkExt, StreamExt};
use operon_auth::rbac::{Action, Scope};
use operon_auth::Identity;
use operon_notes::frame::{decode, encode, FrameKind, HubFrame};
use operon_notes::hub::PresenceDelta;
use operon_store::repos::note::{Note, NoteRepository};
use operon_store::repos::org::{OrgFlavour, OrgRepository};
use operon_store::repos::project::ProjectRepository;
use operon_store::time::now_ms;
use operon_store::{NoteId, ProjectId};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::permissions;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/notes", post(create))
        .route("/api/notes/{id}", get(read).patch(update_meta).delete(remove))
        .route("/api/notes/{id}/snapshot", get(snapshot))
        .route("/api/notes/{id}/body", get(get_body).put(put_body))
        .route("/api/notes/{id}/children", get(children))
        .route("/api/projects/{project_id}/notes", get(list_by_project))
        .route("/ws/notes/{id}", get(ws_upgrade))
}

#[derive(Deserialize)]
pub struct CreateReq {
    pub project_id: String,
    pub title: String,
    pub parent_id: Option<String>,
    pub sibling_index: Option<i64>,
}

#[derive(Serialize)]
pub struct Dto {
    pub id: String,
    pub project_id: String,
    pub parent_id: Option<String>,
    pub title: String,
    pub sibling_index: i64,
    pub kind: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl From<&Note> for Dto {
    fn from(n: &Note) -> Self {
        Self {
            id: n.id.to_string(),
            project_id: n.project_id.to_string(),
            parent_id: n.parent_id.as_ref().map(|p| p.to_string()),
            title: n.title.clone(),
            sibling_index: n.sibling_index,
            kind: n.kind.clone(),
            created_at_ms: n.created_at_ms,
            updated_at_ms: n.updated_at_ms,
        }
    }
}

async fn project_org_flavour(
    state: &AppState,
    project_id: &ProjectId,
) -> Result<(OrgFlavour, operon_store::OrgId), ApiError> {
    let project = state.projects.get(project_id)?.ok_or(ApiError::NotFound)?;
    let org = state.orgs.get(&project.org_id)?.ok_or(ApiError::NotFound)?;
    Ok((org.flavour, org.id))
}

async fn create(
    State(state): State<AppState>,
    identity: Identity,
    Json(req): Json<CreateReq>,
) -> Result<(StatusCode, Json<Dto>), ApiError> {
    let project_id =
        ProjectId::from_str(&req.project_id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let (flavour, org_id) = project_org_flavour(&state, &project_id).await?;
    let scope = Scope::Project {
        project_id: project_id.clone(),
        org_id: org_id.clone(),
    };
    let access = permissions::has_team_access(&state, &identity, &project_id)?;
    permissions::require_note(&state, &identity, Action::NoteCreate, scope, access)?;

    let mut note = Note::new_root(project_id, req.title);
    if let Some(sib) = req.sibling_index {
        note.sibling_index = sib;
    }
    if let Some(p) = req.parent_id.as_deref() {
        note.parent_id =
            Some(NoteId::from_str(p).map_err(|_| ApiError::BadRequest("bad parent_id".into()))?);
    }

    if matches!(flavour, OrgFlavour::NonLocal) {
        // Initialise an empty Loro snapshot so subsequent WS opens have something to load.
        let doc = loro::LoroDoc::new();
        let snapshot = doc
            .export(loro::ExportMode::Snapshot)
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        note.loro_snapshot = Some(snapshot);
    } else {
        note.body_markdown = Some(String::new());
    }
    state.notes.create(&note)?;
    Ok((StatusCode::CREATED, Json(Dto::from(&note))))
}

async fn read(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<Dto>, ApiError> {
    let note_id = NoteId::from_str(&id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let note = state.notes.get(&note_id)?.ok_or(ApiError::NotFound)?;
    let (_, org_id) = project_org_flavour(&state, &note.project_id).await?;
    let scope = Scope::Note {
        note_id: note_id.clone(),
        project_id: note.project_id.clone(),
        org_id,
    };
    let access = permissions::has_team_access(&state, &identity, &note.project_id)?;
    permissions::require_note(&state, &identity, Action::NoteRead, scope, access)?;
    Ok(Json(Dto::from(&note)))
}

#[derive(Deserialize)]
pub struct UpdateMetaReq {
    pub title: Option<String>,
    pub parent_id: Option<Option<String>>,
    pub sibling_index: Option<i64>,
}

async fn update_meta(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    Json(req): Json<UpdateMetaReq>,
) -> Result<Json<Dto>, ApiError> {
    let note_id = NoteId::from_str(&id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let mut note = state.notes.get(&note_id)?.ok_or(ApiError::NotFound)?;
    let (_, org_id) = project_org_flavour(&state, &note.project_id).await?;
    let scope = Scope::Note {
        note_id: note_id.clone(),
        project_id: note.project_id.clone(),
        org_id,
    };
    let access = permissions::has_team_access(&state, &identity, &note.project_id)?;
    permissions::require_note(&state, &identity, Action::NoteUpdate, scope, access)?;

    if let Some(t) = req.title {
        note.title = t;
    }
    if let Some(p) = req.parent_id {
        note.parent_id = match p {
            Some(s) => Some(
                NoteId::from_str(&s).map_err(|_| ApiError::BadRequest("bad parent_id".into()))?,
            ),
            None => None,
        };
    }
    if let Some(s) = req.sibling_index {
        note.sibling_index = s;
    }
    note.updated_at_ms = now_ms();
    state.notes.update(&note)?;
    Ok(Json(Dto::from(&note)))
}

async fn remove(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let note_id = NoteId::from_str(&id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let note = state.notes.get(&note_id)?.ok_or(ApiError::NotFound)?;
    let (_, org_id) = project_org_flavour(&state, &note.project_id).await?;
    let scope = Scope::Note {
        note_id: note_id.clone(),
        project_id: note.project_id.clone(),
        org_id,
    };
    let access = permissions::has_team_access(&state, &identity, &note.project_id)?;
    permissions::require_note(&state, &identity, Action::NoteDelete, scope, access)?;
    state.notes.delete(&note_id)?;
    state.hub.evict(&note_id);
    Ok(StatusCode::NO_CONTENT)
}

async fn list_by_project(
    State(state): State<AppState>,
    identity: Identity,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<Dto>>, ApiError> {
    let project_id = ProjectId::from_str(&project_id)
        .map_err(|_| ApiError::BadRequest("bad project_id".into()))?;
    let (_, org_id) = project_org_flavour(&state, &project_id).await?;
    let scope = Scope::Project {
        project_id: project_id.clone(),
        org_id,
    };
    let access = permissions::has_team_access(&state, &identity, &project_id)?;
    permissions::require_note(&state, &identity, Action::ProjectRead, scope, access)?;
    let notes = state.notes.list_by_project(&project_id)?;
    Ok(Json(notes.iter().map(Dto::from).collect()))
}

async fn children(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<Vec<Dto>>, ApiError> {
    let note_id = NoteId::from_str(&id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let parent = state.notes.get(&note_id)?.ok_or(ApiError::NotFound)?;
    let (_, org_id) = project_org_flavour(&state, &parent.project_id).await?;
    let scope = Scope::Project {
        project_id: parent.project_id.clone(),
        org_id,
    };
    let access = permissions::has_team_access(&state, &identity, &parent.project_id)?;
    permissions::require_note(&state, &identity, Action::ProjectRead, scope, access)?;
    let kids = state.notes.children_of(&note_id)?;
    Ok(Json(kids.iter().map(Dto::from).collect()))
}

async fn snapshot(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let note_id = NoteId::from_str(&id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let note = state.notes.get(&note_id)?.ok_or(ApiError::NotFound)?;
    let (flavour, org_id) = project_org_flavour(&state, &note.project_id).await?;
    if !matches!(flavour, OrgFlavour::NonLocal) {
        return Err(ApiError::Conflict("wrong_flavour"));
    }
    let scope = Scope::Note {
        note_id: note_id.clone(),
        project_id: note.project_id.clone(),
        org_id,
    };
    let access = permissions::has_team_access(&state, &identity, &note.project_id)?;
    permissions::require_note(&state, &identity, Action::NoteRead, scope, access)?;
    let blob = state
        .hub
        .export_snapshot(&note_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        "application/octet-stream".parse().unwrap(),
    );
    headers.insert(
        header::CACHE_CONTROL,
        "private, max-age=10".parse().unwrap(),
    );
    Ok((StatusCode::OK, headers, blob).into_response())
}

#[derive(Serialize)]
pub struct BodyDto {
    pub markdown: String,
}

#[derive(Deserialize)]
pub struct PutBodyReq {
    pub markdown: String,
}

async fn get_body(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
) -> Result<Json<BodyDto>, ApiError> {
    let note_id = NoteId::from_str(&id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let note = state.notes.get(&note_id)?.ok_or(ApiError::NotFound)?;
    let (flavour, org_id) = project_org_flavour(&state, &note.project_id).await?;
    if !matches!(flavour, OrgFlavour::Local) {
        return Err(ApiError::Conflict("wrong_flavour"));
    }
    let scope = Scope::Note {
        note_id: note_id.clone(),
        project_id: note.project_id.clone(),
        org_id,
    };
    let access = permissions::has_team_access(&state, &identity, &note.project_id)?;
    permissions::require_note(&state, &identity, Action::NoteRead, scope, access)?;
    Ok(Json(BodyDto {
        markdown: note.body_markdown.unwrap_or_default(),
    }))
}

async fn put_body(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    Json(req): Json<PutBodyReq>,
) -> Result<StatusCode, ApiError> {
    let note_id = NoteId::from_str(&id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let mut note = state.notes.get(&note_id)?.ok_or(ApiError::NotFound)?;
    let (flavour, org_id) = project_org_flavour(&state, &note.project_id).await?;
    if !matches!(flavour, OrgFlavour::Local) {
        return Err(ApiError::Conflict("wrong_flavour"));
    }
    let scope = Scope::Note {
        note_id: note_id.clone(),
        project_id: note.project_id.clone(),
        org_id,
    };
    let access = permissions::has_team_access(&state, &identity, &note.project_id)?;
    permissions::require_note(&state, &identity, Action::NoteWrite, scope, access)?;
    note.body_markdown = Some(req.markdown);
    note.updated_at_ms = now_ms();
    state.notes.update(&note)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn ws_upgrade(
    State(state): State<AppState>,
    identity: Identity,
    Path(id): Path<String>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let note_id = NoteId::from_str(&id).map_err(|_| ApiError::BadRequest("bad id".into()))?;
    let note = state.notes.get(&note_id)?.ok_or(ApiError::NotFound)?;
    let (flavour, org_id) = project_org_flavour(&state, &note.project_id).await?;
    if matches!(flavour, OrgFlavour::Local) {
        return Err(ApiError::Conflict("wrong_flavour"));
    }
    let scope = Scope::Note {
        note_id: note_id.clone(),
        project_id: note.project_id.clone(),
        org_id,
    };
    let access = permissions::has_team_access(&state, &identity, &note.project_id)?;
    permissions::require_note(&state, &identity, Action::NoteWrite, scope, access)?;

    let client_id = format!("{}-{}", identity.user_id, uuid::Uuid::new_v4());
    Ok(ws.on_upgrade(move |socket| run_session(socket, state, note_id, client_id)))
}

async fn run_session(
    socket: WebSocket,
    state: AppState,
    note_id: NoteId,
    client_id: String,
) {
    let (mut tx, mut rx) = socket.split();

    // Initial snapshot
    let initial = match state.hub.export_snapshot(&note_id).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(err = %e, "ws snapshot export failed");
            return;
        }
    };
    let snap_frame = encode(&HubFrame {
        kind: FrameKind::Snapshot,
        client_id: String::new(),
        payload: initial,
    });
    if tx.send(Message::Binary(Bytes::from(snap_frame))).await.is_err() {
        return;
    }

    // Subscribe to broadcasts.
    let mut sub = match state.hub.open(&note_id) {
        Ok((_, sender)) => sender.subscribe(),
        Err(_) => return,
    };

    state
        .hub
        .broadcast_presence(&note_id, PresenceDelta::Joined(client_id.clone()));

    loop {
        tokio::select! {
            inbound = rx.next() => {
                let Some(msg) = inbound else { break };
                let Ok(msg) = msg else { break };
                match msg {
                    Message::Binary(bytes) => {
                        let Ok((kind, payload)) = decode(&bytes) else { continue };
                        match kind {
                            FrameKind::Update => {
                                let _ = state
                                    .hub
                                    .apply_and_broadcast(&note_id, &client_id, payload)
                                    .await;
                            }
                            FrameKind::Awareness => {
                                state.hub.broadcast_awareness(&note_id, &client_id, payload);
                            }
                            _ => {}
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            outbound = sub.recv() => {
                match outbound {
                    Ok(frame) => {
                        // Don't echo updates back to the originator.
                        if frame.client_id == client_id && matches!(frame.kind, FrameKind::Update | FrameKind::Awareness) {
                            continue;
                        }
                        let bytes = encode(&frame);
                        if tx.send(Message::Binary(Bytes::from(bytes))).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    }

    state
        .hub
        .broadcast_presence(&note_id, PresenceDelta::Left(client_id));
}
