use crate::traits::CancellationToken;
use uuid::Uuid;

/// Handle to a running agent session. Holds the cancellation token so callers
/// can stop the loop, plus the session id for filtering bus events.
#[cfg(not(target_arch = "wasm32"))]
pub struct AgentSession {
    pub id: Uuid,
    pub bus_rx: tokio::sync::broadcast::Receiver<crate::bus::BusEvent>,
    pub ct: CancellationToken,
}

#[cfg(target_arch = "wasm32")]
pub struct AgentSession {
    pub id: Uuid,
    pub ct: CancellationToken,
}
