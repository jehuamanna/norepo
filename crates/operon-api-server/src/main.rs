//! Operon-dioxus non-local API server entry point.

use std::net::SocketAddr;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    operon_api_server::init_tracing();
    let addr: SocketAddr = std::env::var("OPN_BIND_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:7878".to_string())
        .parse()?;
    let db_path = std::env::var("OPN_DB_PATH").unwrap_or_else(|_| "./operon.db".to_string());
    let hostname = std::env::var("OPN_HOSTNAME").unwrap_or_else(|_| "localhost".to_string());

    let state = operon_api_server::AppState::open(&db_path, hostname).await?;
    operon_api_server::bootstrap::ensure_master_admin(&state).await?;

    let app = operon_api_server::router(state);
    tracing::info!("operon-api-server listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
