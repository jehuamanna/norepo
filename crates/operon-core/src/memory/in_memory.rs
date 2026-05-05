use crate::error::{OperonError, OperonResult};
use crate::traits::{
    Capabilities, ContentBlock, Hit, MemoryPlugin, Message, Plugin, Scope,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::RwLock;
use uuid::Uuid;

pub struct InMemoryStore {
    inner: RwLock<HashMap<(ScopeKey, Uuid), Message>>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum ScopeKey {
    User,
    Project(Uuid),
    Team(Uuid),
}

impl From<&Scope> for ScopeKey {
    fn from(s: &Scope) -> Self {
        match s {
            Scope::User => ScopeKey::User,
            Scope::Project(id) => ScopeKey::Project(*id),
            Scope::Team(id) => ScopeKey::Team(*id),
        }
    }
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Plugin for InMemoryStore {
    fn name(&self) -> &str {
        "in_memory_store"
    }
    fn version(&self) -> &str {
        "0.1.0"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::MULTI_TENANT
    }
}

#[async_trait]
impl MemoryPlugin for InMemoryStore {
    async fn write(&self, scope: Scope, mut msg: Message) -> OperonResult<Uuid> {
        let key = ScopeKey::from(&scope);
        let id = msg.id;
        let mut g = self
            .inner
            .write()
            .map_err(|_| OperonError::Secret("in_memory lock poisoned".into()))?;
        // Ensure uniqueness within (scope, id); keep msg.id stable.
        msg.id = id;
        g.insert((key, id), msg);
        Ok(id)
    }

    async fn read(&self, scope: Scope, id: Uuid) -> OperonResult<Option<Message>> {
        let key = ScopeKey::from(&scope);
        Ok(self
            .inner
            .read()
            .map_err(|_| OperonError::Secret("in_memory lock poisoned".into()))?
            .get(&(key, id))
            .cloned())
    }

    async fn search(&self, scope: Scope, query: &str, k: usize) -> OperonResult<Vec<Hit>> {
        let key = ScopeKey::from(&scope);
        let g = self
            .inner
            .read()
            .map_err(|_| OperonError::Secret("in_memory lock poisoned".into()))?;
        let mut hits: Vec<Hit> = g
            .iter()
            .filter(|((k, _), _)| *k == key)
            .filter_map(|(_, m)| {
                let text_match = m.content.iter().any(|cb| match cb {
                    ContentBlock::Text(t) => t.contains(query),
                    ContentBlock::ToolUse { name, .. } => name.contains(query),
                    ContentBlock::ToolResult { content, .. } => content.contains(query),
                });
                if text_match {
                    Some(Hit {
                        message: m.clone(),
                        score: 1.0,
                    })
                } else {
                    None
                }
            })
            .collect();
        hits.sort_by_key(|h| std::cmp::Reverse(h.message.created_at_ms));
        hits.truncate(k);
        Ok(hits)
    }

    async fn delete(&self, scope: Scope, id: Uuid) -> OperonResult<()> {
        let key = ScopeKey::from(&scope);
        self.inner
            .write()
            .map_err(|_| OperonError::Secret("in_memory lock poisoned".into()))?
            .remove(&(key, id));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::run_conformance;

    #[tokio::test]
    async fn conformance_in_memory() {
        let s = InMemoryStore::new();
        run_conformance(&s).await;
    }

    #[tokio::test]
    async fn search_returns_topk_limit() {
        let s = InMemoryStore::new();
        let scope = Scope::Project(Uuid::new_v4());
        let session = Uuid::new_v4();
        for i in 0..5 {
            let m = Message {
                id: Uuid::new_v4(),
                role: crate::traits::Role::User,
                content: vec![ContentBlock::Text(format!("hit-{i}"))],
                created_at_ms: i as u64,
                session,
                metadata: Default::default(),
            };
            s.write(scope.clone(), m).await.unwrap();
        }
        let hits = s.search(scope, "hit", 3).await.unwrap();
        assert_eq!(hits.len(), 3);
    }
}
