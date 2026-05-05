//! axum HTTP API for Operon-dioxus. Exposes auth + (in later phases) admin and
//! note routes.

pub mod audit;
pub mod bootstrap;
pub mod error;
pub mod extractors;
pub mod permissions;
pub mod routes;
pub mod state;

pub use error::ApiError;
pub use state::AppState;

use axum::Router;

/// Build the full router. The entry point and tests both call this.
pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(routes::auth::router())
        .merge(routes::session::router())
        .merge(routes::admin_invites::router())
        .merge(routes::admin_users::router())
        .merge(routes::me::router())
        .merge(routes::orgs::router())
        .merge(routes::departments::router())
        .merge(routes::teams::router())
        .merge(routes::projects::router())
        .merge(routes::memberships::router())
        .merge(routes::team_assignments::router())
        .merge(routes::notes::router())
        .with_state(state)
}

/// Initialise tracing once. Idempotent — second call is a no-op.
pub fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let _ = fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(true)
        .try_init();
}
