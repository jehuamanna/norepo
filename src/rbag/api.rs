//! HTTP client for the operon-api-server REST surface.
//!
//! Phase 6 ships the typed function signatures + DTO types. The actual
//! transport (reqwest on wasm-fetch + gloo-net::websocket) is left as a
//! follow-up — for now every call returns `ApiClientError::NotImplemented`,
//! which the auth screens surface to the user as a "transport unconfigured"
//! toast. This lets the rest of the screen wiring compile and render against
//! the real types while the transport is brought up incrementally.

use serde::{Deserialize, Serialize};

use super::types::{LoginResponse, MePayload, NoteBrief, ProjectBrief};

#[derive(Debug, Clone)]
pub enum ApiClientError {
    NotImplemented,
    Network(String),
    Status { code: u16, body: String },
}

impl std::fmt::Display for ApiClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiClientError::NotImplemented => write!(f, "transport not yet configured"),
            ApiClientError::Network(s) => write!(f, "network: {s}"),
            ApiClientError::Status { code, body } => {
                write!(f, "status {code}: {body}")
            }
        }
    }
}

impl std::error::Error for ApiClientError {}

/// Configured base URL + bearer token.
#[derive(Debug, Clone, Default)]
pub struct ApiClient {
    pub base_url: String,
    pub bearer: Option<String>,
}

impl ApiClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            bearer: None,
        }
    }

    pub fn with_bearer(mut self, token: impl Into<String>) -> Self {
        self.bearer = Some(token.into());
        self
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChangePasswordRequest {
    pub reset_token: String,
    pub new_password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ForgotPasswordRequest {
    pub email: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResetPasswordRequest {
    pub token: String,
    pub new_password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AcceptInviteRequest {
    pub token: String,
    pub password: String,
    pub display_name: Option<String>,
}

#[allow(unused_variables)]
impl ApiClient {
    pub async fn login(&self, req: LoginRequest) -> Result<LoginResponse, ApiClientError> {
        Err(ApiClientError::NotImplemented)
    }
    pub async fn change_password(
        &self,
        req: ChangePasswordRequest,
    ) -> Result<LoginResponse, ApiClientError> {
        Err(ApiClientError::NotImplemented)
    }
    pub async fn forgot_password(&self, req: ForgotPasswordRequest) -> Result<(), ApiClientError> {
        Err(ApiClientError::NotImplemented)
    }
    pub async fn reset_password(&self, req: ResetPasswordRequest) -> Result<(), ApiClientError> {
        Err(ApiClientError::NotImplemented)
    }
    pub async fn accept_invite(
        &self,
        req: AcceptInviteRequest,
    ) -> Result<LoginResponse, ApiClientError> {
        Err(ApiClientError::NotImplemented)
    }
    pub async fn me(&self) -> Result<MePayload, ApiClientError> {
        Err(ApiClientError::NotImplemented)
    }
    pub async fn set_active_org(&self, org_id: &str) -> Result<(), ApiClientError> {
        Err(ApiClientError::NotImplemented)
    }
    pub async fn my_projects(&self) -> Result<Vec<ProjectBrief>, ApiClientError> {
        Err(ApiClientError::NotImplemented)
    }
    pub async fn project_notes(&self, project_id: &str) -> Result<Vec<NoteBrief>, ApiClientError> {
        Err(ApiClientError::NotImplemented)
    }
}
