use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use axum_extra::extract::cookie::{Cookie, SameSite};
use time as cookie_time;
use axum_extra::extract::CookieJar;
use operon_auth::password;
use operon_auth::session as auth_session;
use operon_store::repos::invite::InviteRepository;
use operon_store::repos::membership::{Membership, MembershipRepository};
use operon_store::repos::session::{Session, SessionRepository};
use operon_store::repos::user::{User, UserRepository};
use operon_store::time::now_ms;
use operon_store::{OrgId, SessionId};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::extractors::COOKIE_NAME;
use crate::state::AppState;

const SESSION_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;
const RESET_TOKEN_TTL_MS: i64 = 60 * 60 * 1000;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/change-password", post(change_password))
        .route("/api/auth/forgot-password", post(forgot_password))
        .route("/api/auth/reset-password", post(reset_password))
        .route("/api/auth/accept-invite", post(accept_invite))
}

#[derive(Deserialize)]
pub struct LoginReq {
    pub email: String,
    pub password: String,
}

#[derive(Serialize)]
struct LoginOk {
    status: &'static str,
    session_token: String,
    user_id: String,
    active_org_id: Option<String>,
}

#[derive(Serialize)]
struct LoginMustChange {
    status: &'static str,
    reset_token: String,
}

async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginReq>,
) -> Result<Response, ApiError> {
    let user = state
        .users
        .by_email(&req.email)?
        .ok_or(operon_auth::AuthError::InvalidCredentials)?;
    let hash = user
        .password_hash
        .as_ref()
        .ok_or(operon_auth::AuthError::InvalidCredentials)?;
    password::verify(&req.password, hash)?;

    if read_must_change(&state, &user)? {
        let token = auth_session::generate_token();
        let s = Session {
            id: SessionId::new(),
            user_id: user.id.clone(),
            active_org_id: None,
            token_hash: auth_session::hash_token(&token),
            expires_at_ms: now_ms() + RESET_TOKEN_TTL_MS,
            created_at_ms: now_ms(),
            last_seen_at_ms: now_ms(),
        };
        state.sessions.create(&s)?;
        return Ok(Json(LoginMustChange {
            status: "must_change_password",
            reset_token: token,
        })
        .into_response());
    }

    let active_org = pick_default_active_org(&state, &user)?;
    let (token, _session) = create_session(&state, &user, active_org.clone())?;
    let body = Json(LoginOk {
        status: "ok",
        session_token: token.clone(),
        user_id: user.id.to_string(),
        active_org_id: active_org.as_ref().map(|o| o.to_string()),
    });
    Ok(with_cookie(body.into_response(), &token))
}

#[derive(Deserialize)]
pub struct ChangePasswordReq {
    pub reset_token: String,
    pub new_password: String,
}

async fn change_password(
    State(state): State<AppState>,
    Json(req): Json<ChangePasswordReq>,
) -> Result<Response, ApiError> {
    if req.new_password.len() < 8 {
        return Err(ApiError::BadRequest("password must be ≥ 8 chars".into()));
    }
    let token_hash = auth_session::hash_token(&req.reset_token);
    let session = state
        .sessions
        .by_token_hash(&token_hash)?
        .ok_or(operon_auth::AuthError::InvalidToken)?;
    if session.expires_at_ms < now_ms() {
        return Err(operon_auth::AuthError::Expired.into());
    }
    let mut user = state
        .users
        .get(&session.user_id)?
        .ok_or(operon_auth::AuthError::InvalidToken)?;
    user.password_hash = Some(password::hash(&req.new_password)?);
    user.updated_at_ms = now_ms();
    state.users.update(&user)?;
    let _ = state.store.conn()?.execute(
        "UPDATE users SET must_change_password = 0 WHERE id = ?1",
        rusqlite::params![user.id.as_str()],
    );
    state.sessions.delete(&session.id)?;
    let active = pick_default_active_org(&state, &user)?;
    let (token, _) = create_session(&state, &user, active.clone())?;
    let body = Json(LoginOk {
        status: "ok",
        session_token: token.clone(),
        user_id: user.id.to_string(),
        active_org_id: active.as_ref().map(|o| o.to_string()),
    });
    Ok(with_cookie(body.into_response(), &token))
}

#[derive(Deserialize)]
pub struct ForgotPasswordReq {
    pub email: String,
}

async fn forgot_password(
    State(state): State<AppState>,
    Json(req): Json<ForgotPasswordReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if let Some(user) = state.users.by_email(&req.email)? {
        let token = auth_session::generate_token();
        let s = Session {
            id: SessionId::new(),
            user_id: user.id,
            active_org_id: None,
            token_hash: auth_session::hash_token(&token),
            expires_at_ms: now_ms() + RESET_TOKEN_TTL_MS,
            created_at_ms: now_ms(),
            last_seen_at_ms: now_ms(),
        };
        state.sessions.create(&s)?;
        let url = format!("https://{}/reset/{}", state.hostname, token);
        let _ = state
            .email
            .send(
                &req.email,
                "Reset your Operon password",
                &format!("<a href=\"{url}\">Reset password</a>"),
                &format!("Reset link: {url}"),
            )
            .await;
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct ResetPasswordReq {
    pub token: String,
    pub new_password: String,
}

async fn reset_password(
    State(state): State<AppState>,
    Json(req): Json<ResetPasswordReq>,
) -> Result<StatusCode, ApiError> {
    if req.new_password.len() < 8 {
        return Err(ApiError::BadRequest("password must be ≥ 8 chars".into()));
    }
    let token_hash = auth_session::hash_token(&req.token);
    let session = state
        .sessions
        .by_token_hash(&token_hash)?
        .ok_or(operon_auth::AuthError::InvalidToken)?;
    if session.expires_at_ms < now_ms() {
        return Err(operon_auth::AuthError::Expired.into());
    }
    let mut user = state
        .users
        .get(&session.user_id)?
        .ok_or(operon_auth::AuthError::InvalidToken)?;
    user.password_hash = Some(password::hash(&req.new_password)?);
    user.updated_at_ms = now_ms();
    state.users.update(&user)?;
    state.sessions.delete_for_user(&user.id)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct AcceptInviteReq {
    pub token: String,
    pub password: String,
    pub display_name: Option<String>,
}

async fn accept_invite(
    State(state): State<AppState>,
    Json(req): Json<AcceptInviteReq>,
) -> Result<Response, ApiError> {
    let token_hash = auth_session::hash_token(&req.token);
    let invite = state
        .invites
        .by_token_hash(&token_hash)?
        .ok_or(operon_auth::AuthError::InvalidToken)?;
    if invite.expires_at_ms < now_ms() {
        return Err(operon_auth::AuthError::Expired.into());
    }
    if invite.accepted_at_ms.is_some() {
        return Err(ApiError::Conflict("already_accepted"));
    }
    let user = match state.users.by_email(&invite.email)? {
        Some(u) => u,
        None => {
            let mut u = User::new_with_email(&invite.email);
            u.password_hash = Some(password::hash(&req.password)?);
            u.display_name = req.display_name.clone();
            state.users.create(&u)?;
            u
        }
    };
    let m = Membership::new(
        user.id.clone(),
        invite.org_id.clone(),
        invite.role,
        invite.department_id.clone(),
    )?;
    state.memberships.create(&m)?;
    state.invites.mark_accepted(&invite.id)?;

    let active = Some(invite.org_id.clone());
    let (token, _) = create_session(&state, &user, active.clone())?;
    let body = Json(LoginOk {
        status: "ok",
        session_token: token.clone(),
        user_id: user.id.to_string(),
        active_org_id: active.as_ref().map(|o| o.to_string()),
    });
    Ok(with_cookie(body.into_response(), &token))
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, ApiError> {
    let jar = CookieJar::from_headers(&headers);
    if let Some(c) = jar.get(COOKIE_NAME) {
        let token_hash = auth_session::hash_token(c.value());
        if let Some(s) = state.sessions.by_token_hash(&token_hash)? {
            state.sessions.delete(&s.id)?;
        }
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        expire_cookie()
            .to_string()
            .parse()
            .expect("cookie value valid"),
    );
    Ok(response)
}

fn read_must_change(state: &AppState, user: &User) -> Result<bool, ApiError> {
    let conn = state.store.conn()?;
    let v: Option<i64> = conn
        .query_row(
            "SELECT must_change_password FROM users WHERE id = ?1",
            rusqlite::params![user.id.as_str()],
            |row| row.get(0),
        )
        .ok();
    Ok(v.unwrap_or(0) != 0)
}

fn pick_default_active_org(state: &AppState, user: &User) -> Result<Option<OrgId>, ApiError> {
    let memberships = state.memberships.by_user(&user.id)?;
    Ok(memberships.into_iter().next().map(|m| m.org_id))
}

fn create_session(
    state: &AppState,
    user: &User,
    active_org_id: Option<OrgId>,
) -> Result<(String, Session), ApiError> {
    let token = auth_session::generate_token();
    let s = Session {
        id: SessionId::new(),
        user_id: user.id.clone(),
        active_org_id,
        token_hash: auth_session::hash_token(&token),
        expires_at_ms: now_ms() + SESSION_TTL_MS,
        created_at_ms: now_ms(),
        last_seen_at_ms: now_ms(),
    };
    state.sessions.create(&s)?;
    Ok((token, s))
}

fn with_cookie(mut resp: Response, token: &str) -> Response {
    resp.headers_mut().insert(
        header::SET_COOKIE,
        build_session_cookie(token)
            .to_string()
            .parse()
            .expect("cookie value valid"),
    );
    resp
}

fn build_session_cookie(token: &str) -> Cookie<'static> {
    Cookie::build((COOKIE_NAME, token.to_owned()))
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(false)
        .path("/")
        .max_age(cookie_time::Duration::days(30))
        .build()
}

fn expire_cookie() -> Cookie<'static> {
    Cookie::build((COOKIE_NAME, ""))
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(cookie_time::Duration::seconds(0))
        .build()
}
