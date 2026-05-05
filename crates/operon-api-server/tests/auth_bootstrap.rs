//! Integration test: bootstrap creates admin@localhost/admin, login returns
//! must_change_password, change-password switches to a real session.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use operon_api_server::{router, AppState};
use serde_json::Value;
use tower::ServiceExt;

async fn make_app() -> AppState {
    let state = AppState::for_test();
    operon_api_server::bootstrap::ensure_master_admin(&state)
        .await
        .unwrap();
    state
}

async fn post_json(state: AppState, path: &str, body: Value) -> (StatusCode, Value) {
    let app = router(state);
    let req = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

#[tokio::test]
async fn admin_admin_first_login_forces_password_change() {
    let state = make_app().await;
    let (status, body) = post_json(
        state,
        "/api/auth/login",
        serde_json::json!({ "email": "admin@localhost", "password": "admin" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "must_change_password");
    assert!(body["reset_token"].as_str().is_some());
}

#[tokio::test]
async fn change_password_then_login_succeeds() {
    let state = make_app().await;
    let (_, body) = post_json(
        state.clone(),
        "/api/auth/login",
        serde_json::json!({ "email": "admin@localhost", "password": "admin" }),
    )
    .await;
    let reset = body["reset_token"].as_str().unwrap().to_string();

    let (status, _) = post_json(
        state.clone(),
        "/api/auth/change-password",
        serde_json::json!({ "reset_token": reset, "new_password": "S3cret!!!" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = post_json(
        state,
        "/api/auth/login",
        serde_json::json!({ "email": "admin@localhost", "password": "S3cret!!!" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert!(body["session_token"].as_str().is_some());
}

#[tokio::test]
async fn wrong_password_returns_401() {
    let state = make_app().await;
    let (status, _) = post_json(
        state,
        "/api/auth/login",
        serde_json::json!({ "email": "admin@localhost", "password": "wrong" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn forgot_password_returns_200_for_unknown_email() {
    let state = make_app().await;
    let (status, body) = post_json(
        state,
        "/api/auth/forgot-password",
        serde_json::json!({ "email": "noone@example.com" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn bootstrap_is_idempotent() {
    let state = AppState::for_test();
    operon_api_server::bootstrap::ensure_master_admin(&state)
        .await
        .unwrap();
    operon_api_server::bootstrap::ensure_master_admin(&state)
        .await
        .unwrap();
    use operon_store::repos::membership::MembershipRepository;
    assert_eq!(state.memberships.count_master_admins().unwrap(), 1);
}
