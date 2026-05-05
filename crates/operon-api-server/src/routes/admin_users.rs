use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use operon_auth::rbac::{Action, Scope};
use operon_auth::Identity;
use operon_store::repos::user::UserRepository;
use serde::Serialize;

use crate::error::ApiError;
use crate::permissions;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/admin/users", get(list))
}

#[derive(Serialize)]
pub struct UserDto {
    pub id: String,
    pub email: String,
    pub display_name: Option<String>,
}

async fn list(
    State(state): State<AppState>,
    identity: Identity,
) -> Result<Json<Vec<UserDto>>, ApiError> {
    permissions::require(&state, &identity, Action::AdminUsersList, Scope::System)?;
    let users = state.users.list(200, None)?;
    Ok(Json(
        users
            .into_iter()
            .map(|u| UserDto {
                id: u.id.to_string(),
                email: u.email,
                display_name: u.display_name,
            })
            .collect(),
    ))
}
