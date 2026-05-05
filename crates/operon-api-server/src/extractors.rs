use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::header;
use axum_extra::extract::CookieJar;
use operon_auth::session as token_session;
use operon_auth::Identity;
use operon_store::repos::membership::MembershipRepository;
use operon_store::repos::session::SessionRepository;
use operon_store::repos::user::UserRepository;
use operon_store::time::now_ms;

use crate::error::ApiError;
use crate::state::AppState;

pub const COOKIE_NAME: &str = "opn_session";

impl FromRequestParts<AppState> for Identity {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // 1. Auth-bypass build → synthetic identity.
        #[cfg(feature = "auth-bypass")]
        {
            return Ok(synthetic_local_identity(state));
        }

        // 2. Read token from cookie or Authorization header.
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .map(|s| s.to_string())
            .or_else(|| {
                let jar = CookieJar::from_headers(&parts.headers);
                jar.get(COOKIE_NAME).map(|c| c.value().to_string())
            })
            .ok_or(ApiError::Unauthorized)?;

        let token_hash = token_session::hash_token(&token);
        let session = state
            .sessions
            .by_token_hash(&token_hash)?
            .ok_or(ApiError::Unauthorized)?;
        if session.expires_at_ms < now_ms() {
            return Err(ApiError::Unauthorized);
        }
        let user = state
            .users
            .get(&session.user_id)?
            .ok_or(ApiError::Unauthorized)?;

        let role_in_active_org = if let Some(org_id) = &session.active_org_id {
            state
                .memberships
                .by_user_org(&user.id, org_id)?
                .map(|m| m.role)
        } else {
            None
        };

        Ok(Identity {
            user_id: user.id,
            session_id: session.id,
            active_org_id: session.active_org_id,
            role_in_active_org,
            must_change_password: false, // surfaced separately on login response
        })
    }
}

#[cfg(feature = "auth-bypass")]
fn synthetic_local_identity(state: &AppState) -> Identity {
    use operon_store::ids::LOCAL_ORG_ID;
    use std::str::FromStr;
    let org = operon_store::OrgId::from_str(LOCAL_ORG_ID).expect("constant uuid is valid");
    // Find or create the synthetic local user (bootstrap inserts it; this is a fallback).
    let user = state
        .users
        .by_email("local-user@localhost")
        .ok()
        .flatten()
        .map(|u| u.id)
        .unwrap_or_else(operon_store::UserId::new);
    Identity::synthetic_local(user, org)
}
