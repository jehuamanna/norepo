//! Theme persistence — load and save the active [`ThemeId`] across reloads.
//!
//! Storage is abstracted behind [`ThemeStorage`]; the wasm build wires up
//! [`WebLocalStorage`] which talks to `window.localStorage`, while native unit tests use
//! [`MemoryStorage`] so the persistence logic stays test-coverable without a browser.
//!
//! Keys: `operon.theme.id`, `operon.theme.lastDark`, `operon.theme.lastLight`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::str::FromStr;

use super::{ThemeId, ThemeKind};

pub const KEY_ID: &str = "operon.theme.id";
pub const KEY_LAST_DARK: &str = "operon.theme.lastDark";
pub const KEY_LAST_LIGHT: &str = "operon.theme.lastLight";

/// Minimal key/value abstraction implemented by both wasm `localStorage` and an in-memory
/// fixture for native tests.
pub trait ThemeStorage {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&self, key: &str, value: &str);
}

/// Native test/fixture storage. RefCell lets `set()` keep the trait's `&self` shape.
#[derive(Default)]
pub struct MemoryStorage {
    inner: RefCell<HashMap<String, String>>,
}

impl MemoryStorage {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ThemeStorage for MemoryStorage {
    fn get(&self, key: &str) -> Option<String> {
        self.inner.borrow().get(key).cloned()
    }
    fn set(&self, key: &str, value: &str) {
        self.inner.borrow_mut().insert(key.to_string(), value.to_string());
    }
}

/// Real wasm implementation — talks to `window.localStorage`. Errors (private mode quota,
/// `SecurityError`) are absorbed silently into `None` / no-op so the app keeps booting.
#[cfg(target_arch = "wasm32")]
pub struct WebLocalStorage;

#[cfg(target_arch = "wasm32")]
impl ThemeStorage for WebLocalStorage {
    fn get(&self, key: &str) -> Option<String> {
        web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten())
            .and_then(|s| s.get_item(key).ok().flatten())
    }
    fn set(&self, key: &str, value: &str) {
        if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
            let _ = s.set_item(key, value);
        }
    }
}

/// Native build placeholder so non-wasm code (cargo check, integration tests under `tests/`)
/// can still reference the type. Reads always return `None`; writes are no-ops.
#[cfg(not(target_arch = "wasm32"))]
pub struct WebLocalStorage;

#[cfg(not(target_arch = "wasm32"))]
impl ThemeStorage for WebLocalStorage {
    fn get(&self, _key: &str) -> Option<String> {
        None
    }
    fn set(&self, _key: &str, _value: &str) {}
}

/// Read the persisted active theme id, if any. Unknown / corrupt slugs return `None`.
pub fn load_id<S: ThemeStorage>(storage: &S) -> Option<ThemeId> {
    storage
        .get(KEY_ID)
        .and_then(|raw| ThemeId::from_str(&raw).ok())
}

/// Persist the active theme id.
pub fn save_id<S: ThemeStorage>(storage: &S, id: ThemeId) {
    storage.set(KEY_ID, id.slug());
}

/// Update the LRU bookkeeping after a user theme switch. Always writes `KEY_ID`; additionally
/// writes `KEY_LAST_DARK` or `KEY_LAST_LIGHT` based on the new theme's kind so
/// `view.toggleTheme` can recall the user's preferred dark/light pair.
pub fn record_theme_change<S: ThemeStorage>(storage: &S, id: ThemeId) {
    storage.set(KEY_ID, id.slug());
    match id.kind() {
        ThemeKind::Light => storage.set(KEY_LAST_LIGHT, id.slug()),
        // Both Dark and HighContrast count as "dark" for the toggle alternation.
        ThemeKind::Dark | ThemeKind::HighContrast => storage.set(KEY_LAST_DARK, id.slug()),
    }
}

/// Read the user's last-selected dark theme, or fall back to `VscodeDarkPlus`.
pub fn last_dark<S: ThemeStorage>(storage: &S) -> ThemeId {
    storage
        .get(KEY_LAST_DARK)
        .and_then(|s| ThemeId::from_str(&s).ok())
        .unwrap_or(ThemeId::VscodeDarkPlus)
}

/// Read the user's last-selected light theme, or fall back to `VscodeLightPlus`.
pub fn last_light<S: ThemeStorage>(storage: &S) -> ThemeId {
    storage
        .get(KEY_LAST_LIGHT)
        .and_then(|s| ThemeId::from_str(&s).ok())
        .unwrap_or(ThemeId::VscodeLightPlus)
}

/// First-run resolution: honour the OS preference unless the user has previously chosen.
pub fn first_run_default(prefers_dark: bool) -> ThemeId {
    if prefers_dark {
        ThemeId::VscodeDarkPlus
    } else {
        ThemeId::VscodeLightPlus
    }
}

/// Resolve initial active theme id at boot: stored value if any, else `first_run_default`.
pub fn resolve_initial_id<S: ThemeStorage>(storage: &S, prefers_dark: bool) -> ThemeId {
    load_id(storage).unwrap_or_else(|| first_run_default(prefers_dark))
}

/// Query `prefers-color-scheme: dark`. Wasm-only; native default is `false` (irrelevant).
#[cfg(target_arch = "wasm32")]
pub fn prefers_dark() -> bool {
    web_sys::window()
        .and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten())
        .map(|m| m.matches())
        .unwrap_or(false)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn prefers_dark() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_run_default_dark_when_prefers_dark() {
        assert_eq!(first_run_default(true), ThemeId::VscodeDarkPlus);
    }

    #[test]
    fn first_run_default_light_when_prefers_light() {
        assert_eq!(first_run_default(false), ThemeId::VscodeLightPlus);
    }

    #[test]
    fn round_trip_via_memory_storage() {
        let s = MemoryStorage::new();
        save_id(&s, ThemeId::Nord);
        assert_eq!(load_id(&s), Some(ThemeId::Nord));
    }

    #[test]
    fn load_returns_none_for_missing_key() {
        let s = MemoryStorage::new();
        assert_eq!(load_id(&s), None);
    }

    #[test]
    fn load_returns_none_for_unknown_slug() {
        let s = MemoryStorage::new();
        s.set(KEY_ID, "not-a-real-theme");
        assert_eq!(load_id(&s), None);
    }

    #[test]
    fn record_theme_change_updates_last_dark_for_dark_kind() {
        let s = MemoryStorage::new();
        record_theme_change(&s, ThemeId::Abyss);
        assert_eq!(s.get(KEY_LAST_DARK).as_deref(), Some("abyss"));
        assert_eq!(s.get(KEY_LAST_LIGHT), None);
        assert_eq!(s.get(KEY_ID).as_deref(), Some("abyss"));
    }

    #[test]
    fn record_theme_change_updates_last_light_for_light_kind() {
        let s = MemoryStorage::new();
        record_theme_change(&s, ThemeId::SolarizedLight);
        assert_eq!(s.get(KEY_LAST_LIGHT).as_deref(), Some("solarized-light"));
        assert_eq!(s.get(KEY_LAST_DARK), None);
    }

    #[test]
    fn record_theme_change_treats_high_contrast_as_dark_lru() {
        let s = MemoryStorage::new();
        record_theme_change(&s, ThemeId::HighContrastDark);
        assert_eq!(s.get(KEY_LAST_DARK).as_deref(), Some("high-contrast-dark"));
        assert_eq!(s.get(KEY_LAST_LIGHT), None);
    }

    #[test]
    fn last_dark_falls_back_to_vscode_default() {
        let s = MemoryStorage::new();
        assert_eq!(last_dark(&s), ThemeId::VscodeDarkPlus);
    }

    #[test]
    fn last_light_falls_back_to_vscode_default() {
        let s = MemoryStorage::new();
        assert_eq!(last_light(&s), ThemeId::VscodeLightPlus);
    }

    #[test]
    fn resolve_initial_id_uses_stored_value_when_present() {
        let s = MemoryStorage::new();
        save_id(&s, ThemeId::Nord);
        assert_eq!(resolve_initial_id(&s, true), ThemeId::Nord);
        assert_eq!(resolve_initial_id(&s, false), ThemeId::Nord);
    }

    #[test]
    fn resolve_initial_id_uses_prefers_color_scheme_when_empty() {
        let s = MemoryStorage::new();
        assert_eq!(resolve_initial_id(&s, true), ThemeId::VscodeDarkPlus);
        assert_eq!(resolve_initial_id(&s, false), ThemeId::VscodeLightPlus);
    }
}
