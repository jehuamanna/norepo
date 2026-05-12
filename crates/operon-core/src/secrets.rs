use crate::error::{OperonError, OperonResult};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::RwLock;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

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
        Err(OperonError::ReadOnly("EnvSecretStore".into()))
    }
    async fn delete(&self, _key: &str) -> OperonResult<()> {
        Err(OperonError::ReadOnly("EnvSecretStore".into()))
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
        run_off_runtime(self.service.clone(), key.to_string(), |service, key| {
            let entry = keyring::Entry::new(&service, &key)
                .map_err(|e| OperonError::Secret(format!("keyring entry: {e}")))?;
            match entry.get_password() {
                Ok(p) => Ok(Some(p)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(OperonError::Secret(format!("keyring get: {e}"))),
            }
        })
        .await
    }
    async fn put(&self, key: &str, value: &str) -> OperonResult<()> {
        let value = value.to_string();
        run_off_runtime(self.service.clone(), key.to_string(), move |service, key| {
            let entry = keyring::Entry::new(&service, &key)
                .map_err(|e| OperonError::Secret(format!("keyring entry: {e}")))?;
            entry
                .set_password(&value)
                .map_err(|e| OperonError::Secret(format!("keyring set: {e}")))
        })
        .await
    }
    async fn delete(&self, key: &str) -> OperonResult<()> {
        run_off_runtime(self.service.clone(), key.to_string(), |service, key| {
            let entry = keyring::Entry::new(&service, &key)
                .map_err(|e| OperonError::Secret(format!("keyring entry: {e}")))?;
            match entry.delete_password() {
                Ok(()) => Ok(()),
                Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(OperonError::Secret(format!("keyring delete: {e}"))),
            }
        })
        .await
    }
}

/// Run a keyring closure on a fresh OS thread (`std::thread::spawn`) so it
/// has **no** tokio runtime context. `tokio::task::spawn_blocking` is not
/// enough: those threads inherit the runtime, so `zbus`'s internal
/// `block_on` still finds tokio and panics with "Cannot start a runtime
/// from within a runtime." A bare std thread has no `Handle::current()`,
/// which lets zbus spin up its own runtime cleanly.
#[cfg(not(target_arch = "wasm32"))]
async fn run_off_runtime<F, T>(service: String, key: String, f: F) -> OperonResult<T>
where
    F: FnOnce(String, String) -> OperonResult<T> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f(service, key));
    });
    rx.await
        .map_err(|_| OperonError::Secret("keyring thread dropped result".into()))?
}

/// Plaintext JSON secret store, used as a persistent fallback when the OS
/// keyring is unavailable or locked (common on headless Linux / dev VMs).
///
/// File at `~/.config/operon/secrets.json` (overridable via the constructor
/// for tests). Same trust model as `~/.aws/credentials` or
/// `~/.anthropic/config.json`: file mode 600, plaintext, lives under the
/// user's home. Encryption-at-rest is a follow-up — see the plan for the
/// AES-GCM backend.
#[cfg(not(target_arch = "wasm32"))]
pub struct JsonFileSecretStore {
    path: PathBuf,
    inner: tokio::sync::Mutex<HashMap<String, String>>,
    loaded: tokio::sync::OnceCell<()>,
}

#[cfg(not(target_arch = "wasm32"))]
impl JsonFileSecretStore {
    /// Default path: `$XDG_CONFIG_HOME/operon/secrets.json` (falls back to
    /// `~/.config/operon/secrets.json`).
    pub fn default_path() -> PathBuf {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            return PathBuf::from(xdg).join("operon").join("secrets.json");
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        PathBuf::from(home)
            .join(".config")
            .join("operon")
            .join("secrets.json")
    }

    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            inner: tokio::sync::Mutex::new(HashMap::new()),
            loaded: tokio::sync::OnceCell::new(),
        }
    }

    async fn ensure_loaded(&self) -> OperonResult<()> {
        self.loaded
            .get_or_try_init(|| async {
                let map = match tokio::fs::read_to_string(&self.path).await {
                    Ok(s) if s.trim().is_empty() => HashMap::new(),
                    Ok(s) => serde_json::from_str::<HashMap<String, String>>(&s)
                        .map_err(|e| {
                            OperonError::Secret(format!(
                                "parse {}: {e}",
                                self.path.display()
                            ))
                        })?,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
                    Err(e) => {
                        return Err(OperonError::Secret(format!(
                            "read {}: {e}",
                            self.path.display()
                        )));
                    }
                };
                *self.inner.lock().await = map;
                Ok::<_, OperonError>(())
            })
            .await
            .map(|_| ())
    }

    async fn flush(&self, snapshot: &HashMap<String, String>) -> OperonResult<()> {
        if let Some(parent) = self.path.parent() {
            if !parent.exists() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    OperonError::Secret(format!("mkdir {}: {e}", parent.display()))
                })?;
            }
        }
        let pretty = serde_json::to_string_pretty(snapshot)
            .map_err(|e| OperonError::Secret(format!("serialize: {e}")))?;
        let tmp = self.path.with_extension("json.tmp");
        tokio::fs::write(&tmp, pretty).await.map_err(|e| {
            OperonError::Secret(format!("write {}: {e}", tmp.display()))
        })?;
        // Best-effort 0600 mode on Unix so the file isn't world-readable.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = tokio::fs::set_permissions(
                &tmp,
                std::fs::Permissions::from_mode(0o600),
            )
            .await;
        }
        tokio::fs::rename(&tmp, &self.path).await.map_err(|e| {
            OperonError::Secret(format!(
                "rename {} → {}: {e}",
                tmp.display(),
                self.path.display()
            ))
        })?;
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl SecretStore for JsonFileSecretStore {
    async fn get(&self, key: &str) -> OperonResult<Option<String>> {
        self.ensure_loaded().await?;
        Ok(self.inner.lock().await.get(key).cloned())
    }
    async fn put(&self, key: &str, value: &str) -> OperonResult<()> {
        self.ensure_loaded().await?;
        let snapshot = {
            let mut g = self.inner.lock().await;
            g.insert(key.to_string(), value.to_string());
            g.clone()
        };
        self.flush(&snapshot).await
    }
    async fn delete(&self, key: &str) -> OperonResult<()> {
        self.ensure_loaded().await?;
        let snapshot = {
            let mut g = self.inner.lock().await;
            g.remove(key);
            g.clone()
        };
        self.flush(&snapshot).await
    }
}

/// A `LayeredSecretStore` chains multiple stores in priority order:
///   `get()` returns the first non-None hit;
///   `put()` writes to the first writable store (skips read-only `EnvSecretStore`s);
///   `delete()` deletes from every store (idempotent).
///
/// Typical order on desktop: `[KeyringSecretStore, EnvSecretStore]`.
/// On headless Linux without a Secret Service: `[MockSecretStore, EnvSecretStore]`
/// (in-memory cache + env fallback) until an encrypted-file backend ships.
pub struct LayeredSecretStore {
    layers: Vec<std::sync::Arc<dyn SecretStore>>,
}

impl LayeredSecretStore {
    pub fn new(layers: Vec<std::sync::Arc<dyn SecretStore>>) -> Self {
        Self { layers }
    }
}

#[async_trait]
impl SecretStore for LayeredSecretStore {
    async fn get(&self, key: &str) -> OperonResult<Option<String>> {
        for (idx, layer) in self.layers.iter().enumerate() {
            match layer.get(key).await {
                Ok(Some(v)) => {
                    eprintln!("[secrets] get({key}) hit layer {idx} (len={})", v.len());
                    return Ok(Some(v));
                }
                Ok(None) => {
                    eprintln!("[secrets] get({key}) layer {idx} miss");
                    continue;
                }
                Err(e) => {
                    eprintln!("[secrets] get({key}) layer {idx} error: {e}");
                    continue;
                }
            }
        }
        eprintln!("[secrets] get({key}) ALL layers miss");
        Ok(None)
    }
    async fn put(&self, key: &str, value: &str) -> OperonResult<()> {
        let mut last_err: Option<OperonError> = None;
        for (idx, layer) in self.layers.iter().enumerate() {
            match layer.put(key, value).await {
                Ok(()) => {
                    eprintln!("[secrets] put({key}) accepted by layer {idx}");
                    return Ok(());
                }
                Err(OperonError::ReadOnly(_)) => {
                    eprintln!("[secrets] put({key}) layer {idx} read-only, skip");
                    continue;
                }
                Err(e) => {
                    eprintln!("[secrets] put({key}) layer {idx} error: {e}");
                    last_err = Some(e);
                    continue;
                }
            }
        }
        Err(last_err.unwrap_or_else(|| {
            OperonError::Secret("no writable layer in LayeredSecretStore".into())
        }))
    }
    async fn delete(&self, key: &str) -> OperonResult<()> {
        let mut last_err: Option<OperonError> = None;
        for layer in &self.layers {
            if let Err(e) = layer.delete(key).await {
                if !matches!(&e, OperonError::ReadOnly(_)) {
                    last_err = Some(e);
                }
            }
        }
        match last_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

/// Well-known SecretStore keys used by Operon plugins. Centralised so the
/// settings UI (Slice A12) and provider plugins agree on naming.
pub mod keys {
    pub const ANTHROPIC_API_KEY: &str = "provider/anthropic/api-key";
    pub const OPENAI_API_KEY: &str = "provider/openai/api-key";
    pub const GOOGLE_API_KEY: &str = "provider/google/api-key";
    pub const TAVILY_API_KEY: &str = "tool/tavily/api-key";
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::Arc;

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
        let key = "_";
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

    #[tokio::test]
    async fn layered_get_returns_first_hit() {
        let primary = Arc::new(MockSecretStore::new());
        let fallback = Arc::new(MockSecretStore::new());
        primary.put("k", "primary").await.unwrap();
        fallback.put("k", "fallback").await.unwrap();
        let store = LayeredSecretStore::new(vec![primary.clone(), fallback.clone()]);
        assert_eq!(store.get("k").await.unwrap().as_deref(), Some("primary"));
    }

    #[tokio::test]
    async fn layered_get_falls_through_on_miss() {
        let primary = Arc::new(MockSecretStore::new());
        let fallback = Arc::new(MockSecretStore::new());
        fallback.put("k", "fallback").await.unwrap();
        let store = LayeredSecretStore::new(vec![primary, fallback]);
        assert_eq!(store.get("k").await.unwrap().as_deref(), Some("fallback"));
    }

    #[tokio::test]
    async fn layered_put_writes_to_first_writable_layer() {
        std::env::remove_var("OPERON_LSST_KEY");
        let env_layer: Arc<dyn SecretStore> = Arc::new(EnvSecretStore::new("OPERON_LSST_"));
        let mock_layer = Arc::new(MockSecretStore::new());
        // Order: env (read-only) → mock. put() should land in mock.
        let store = LayeredSecretStore::new(vec![env_layer, mock_layer.clone()]);
        store.put("KEY", "value").await.unwrap();
        assert_eq!(mock_layer.get("KEY").await.unwrap().as_deref(), Some("value"));
    }

    #[tokio::test]
    async fn layered_get_returns_none_when_all_miss() {
        let a = Arc::new(MockSecretStore::new());
        let b = Arc::new(MockSecretStore::new());
        let store = LayeredSecretStore::new(vec![a, b]);
        assert!(store.get("nope").await.unwrap().is_none());
    }
}
