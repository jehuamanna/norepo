pub mod in_memory;
#[cfg(all(feature = "sqlite-memory", not(target_arch = "wasm32")))]
pub mod sqlite;

pub use in_memory::InMemoryStore;
#[cfg(all(feature = "sqlite-memory", not(target_arch = "wasm32")))]
pub use sqlite::SqliteMemoryStore;

use crate::agent::traits::{ContentBlock, MemoryPlugin, Message, Role, Scope};
use std::collections::HashSet;
use uuid::Uuid;

/// Conformance harness used by every MemoryPlugin impl. Each impl test invokes this
/// against a fresh store; failures pinpoint the misbehaving impl.
pub async fn run_conformance(store: &(dyn MemoryPlugin + Send + Sync)) {
    let project = Uuid::new_v4();
    let scope = Scope::Project(project);
    let session = Uuid::new_v4();
    let now = || web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    fn msg(session: Uuid, text: &str, ts: u64) -> Message {
        Message {
            id: Uuid::new_v4(),
            role: Role::User,
            content: vec![ContentBlock::Text(text.to_string())],
            created_at_ms: ts,
            session,
            metadata: Default::default(),
        }
    }

    // a. write+read round-trip
    let m1 = msg(session, "alpha", now());
    let id1 = store.write(scope.clone(), m1.clone()).await.expect("write");
    let got = store.read(scope.clone(), id1).await.expect("read");
    assert!(got.is_some(), "round-trip read returned None");

    // b. read missing → None
    let missing = store.read(scope.clone(), Uuid::new_v4()).await.expect("read");
    assert!(missing.is_none(), "read of missing id should be None");

    // c. delete then read → None
    store.delete(scope.clone(), id1).await.expect("delete");
    let after_del = store.read(scope.clone(), id1).await.expect("read after del");
    assert!(after_del.is_none(), "read after delete should be None");

    // d. scope isolation: User vs Project
    let m_user = msg(session, "scoped-user", now());
    let m_project = msg(session, "scoped-project", now());
    store.write(Scope::User, m_user.clone()).await.expect("write user");
    let pid = store
        .write(scope.clone(), m_project.clone())
        .await
        .expect("write project");
    let user_hits = store
        .search(Scope::User, "scoped-project", 10)
        .await
        .expect("search user");
    assert_eq!(
        user_hits.len(),
        0,
        "scope isolation broken: user scope returned project text"
    );
    let project_hits = store
        .search(scope.clone(), "scoped-project", 10)
        .await
        .expect("search project");
    assert!(
        project_hits.iter().any(|h| h.message.id == pid),
        "project scope failed to find own message"
    );

    // e. search substring match
    store
        .write(scope.clone(), msg(session, "alpha-beta", now()))
        .await
        .expect("write alpha-beta");
    store
        .write(scope.clone(), msg(session, "gamma", now()))
        .await
        .expect("write gamma");
    let hits = store
        .search(scope.clone(), "alpha", 10)
        .await
        .expect("search alpha");
    assert!(!hits.is_empty(), "substring search failed");

    // f. search top-k limit
    for i in 0..5 {
        store
            .write(scope.clone(), msg(session, &format!("delta-{i}"), now()))
            .await
            .expect("write delta");
    }
    let top2 = store
        .search(scope.clone(), "delta", 2)
        .await
        .expect("search delta");
    assert_eq!(top2.len(), 2, "top-k limit not enforced");

    // g. concurrent writes (10 tasks) — sequential here for portability
    let mut ids: HashSet<Uuid> = HashSet::new();
    for i in 0..10 {
        let id = store
            .write(scope.clone(), msg(session, &format!("concurrent-{i}"), now()))
            .await
            .expect("concurrent write");
        ids.insert(id);
    }
    assert_eq!(ids.len(), 10, "concurrent writes lost ids");

    // h. delete invalid id is idempotent
    store
        .delete(scope.clone(), Uuid::new_v4())
        .await
        .expect("delete invalid id should be Ok");
}
