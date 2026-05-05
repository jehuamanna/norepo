#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::broadcast;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub enum BusEvent {
    AgentStarted {
        session: Uuid,
    },
    AgentStepCompleted {
        session: Uuid,
        step: u32,
    },
    ToolInvoked {
        session: Uuid,
        tool: String,
        latency_ms: u64,
    },
    MemoryWritten {
        session: Uuid,
        scope: String,
        id: Uuid,
    },
    ChatStreamDelta {
        session: Uuid,
        text: String,
    },
    TokenUsage {
        session: Uuid,
        provider: String,
        model: String,
        prompt: u64,
        prompt_cached: u64,
        completion: u64,
    },
    BudgetExceeded {
        session: Uuid,
        reason: String,
    },
    Cancelled {
        session: Uuid,
    },
    McpToolInvoked {
        session: Uuid,
        server: String,
        tool: String,
        latency_ms: u64,
    },
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<BusEvent>,
}

#[cfg(not(target_arch = "wasm32"))]
impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity);
        Self { tx }
    }

    pub fn publish(&self, event: BusEvent) {
        // Lossy send: ignore SendError when no subscribers exist.
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BusEvent> {
        self.tx.subscribe()
    }

    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for EventBus {
    fn default() -> Self {
        Self::new(256)
    }
}

// On WASM target, tokio::sync::broadcast is unavailable through our minimal feature set;
// we provide a stub so the type still names; later phases can add a wasm-friendly bus.
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Default)]
pub struct EventBus;

#[cfg(target_arch = "wasm32")]
impl EventBus {
    pub fn new(_capacity: usize) -> Self {
        Self
    }
    pub fn publish(&self, _event: BusEvent) {}
    pub fn receiver_count(&self) -> usize {
        0
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publish_subscribe_roundtrip() {
        let bus = EventBus::new(8);
        let mut rx = bus.subscribe();
        let session = Uuid::new_v4();
        bus.publish(BusEvent::AgentStarted { session });
        bus.publish(BusEvent::AgentStepCompleted { session, step: 1 });
        let e1 = rx.recv().await.unwrap();
        let e2 = rx.recv().await.unwrap();
        assert!(matches!(e1, BusEvent::AgentStarted { .. }));
        assert!(matches!(e2, BusEvent::AgentStepCompleted { step: 1, .. }));
    }

    #[tokio::test]
    async fn lossy_under_capacity_overflow() {
        let bus = EventBus::new(2);
        let mut rx = bus.subscribe();
        let s = Uuid::new_v4();
        for i in 0..5 {
            bus.publish(BusEvent::AgentStepCompleted { session: s, step: i });
        }
        // First recv should report Lagged
        match rx.recv().await {
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                assert!(n >= 3);
            }
            other => panic!("expected Lagged, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn publish_with_no_subscriber_does_not_error() {
        let bus = EventBus::new(2);
        bus.publish(BusEvent::AgentStarted {
            session: Uuid::new_v4(),
        });
    }
}
