use crate::agent::error::{OperonError, OperonResult};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::RwLock;

#[async_trait]
pub trait SecretStore: Send + Sync {
    async fn get(&self, key: &str) -> OperonResult<Option<String>>;
    async fn put(&self, key: &str, value: &str) -> OperonResult<()>;
    async fn delete(&self, key: &str) -> OperonResult<()>;
}

pub struct MockSecretStore {
    inner: RwLock<HashMap<String, String>>,
}

impl MockSecretStore {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for MockSecretStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SecretStore for MockSecretStore {
    async fn get(&self, key: &str) -> OperonResult<Option<String>> {
        Ok(self
            .inner
            .read()
            .map_err(|_| OperonError::Secret("mock lock poisoned".into()))?
            .get(key)
            .cloned())
    }
    async fn put(&self, key: &str, value: &str) -> OperonResult<()> {
        self.inner
            .write()
            .map_err(|_| OperonError::Secret("mock lock poisoned".into()))?
            .insert(key.to_string(), value.to_string());
        Ok(())
    }
    async fn delete(&self, key: &str) -> OperonResult<()> {
        self.inner
            .write()
            .map_err(|_| OperonError::Secret("mock lock poisoned".into()))?
            .remove(key);
        Ok(())
    }
}

pub struct EnvSecretStore {
    prefix: String,
}

impl EnvSecretStore {
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
        }
    }

    fn key_to_env(&self, key: &str) -> String {
        format!("{}{}", self.prefix, key.to_uppercase().replace(['/', '-', '.'], "_"))
    }
}

#[async_trait]
impl SecretStore for EnvSecretStore {
    async fn get(&self, key: &str) -> OperonResult<Option<String>> {
        Ok(std::env::var(self.key_to_env(key)).ok())
    }
    async fn put(&self, _key: &str, _value: &str) -> OperonResult<()> {
        Err(OperonError::Secret(
            "EnvSecretStore is read-only".into(),
        ))
    }
    async fn delete(&self, _key: &str) -> OperonResult<()> {
        Err(OperonError::Secret(
            "EnvSecretStore is read-only".into(),
        ))
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub struct KeyringSecretStore {
    service: String,
}

#[cfg(not(target_arch = "wasm32"))]
impl KeyringSecretStore {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl SecretStore for KeyringSecretStore {
    async fn get(&self, key: &str) -> OperonResult<Option<String>> {
        let entry = keyring::Entry::new(&self.service, key)
            .map_err(|e| OperonError::Secret(format!("keyring entry: {e}")))?;
        match entry.get_password() {
            Ok(p) => Ok(Some(p)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(OperonError::Secret(format!("keyring get: {e}"))),
        }
    }
    async fn put(&self, key: &str, value: &str) -> OperonResult<()> {
        let entry = keyring::Entry::new(&self.service, key)
            .map_err(|e| OperonError::Secret(format!("keyring entry: {e}")))?;
        entry
            .set_password(value)
            .map_err(|e| OperonError::Secret(format!("keyring set: {e}")))
    }
    async fn delete(&self, key: &str) -> OperonResult<()> {
        let entry = keyring::Entry::new(&self.service, key)
            .map_err(|e| OperonError::Secret(format!("keyring entry: {e}")))?;
        match entry.delete_password() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(OperonError::Secret(format!("keyring delete: {e}"))),
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_put_get_delete() {
        let s = MockSecretStore::new();
        s.put("k", "v").await.unwrap();
        assert_eq!(s.get("k").await.unwrap().as_deref(), Some("v"));
        s.delete("k").await.unwrap();
        assert!(s.get("k").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn mock_get_missing_returns_none() {
        let s = MockSecretStore::new();
        assert!(s.get("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn env_store_reads_env() {
        // Use a hard-to-collide name.
        let key = "_";
        // SAFETY: tests run in a single process; isolate name to avoid collision.
        std::env::set_var(key, "value-from-env");
        let s = EnvSecretStore::new("OPERON_TEST_PHASE1_");
        assert_eq!(s.get("KEY").await.unwrap().as_deref(), Some("value-from-env"));
        std::env::remove_var(key);
    }

    #[tokio::test]
    async fn env_store_put_returns_error() {
        let s = EnvSecretStore::new("OPERON_TEST_");
        assert!(s.put("k", "v").await.is_err());
    }
}
