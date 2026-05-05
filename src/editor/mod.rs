//! Editor backends — renderer-agnostic surfaces for source-text and rich-text editing.
//!
//! The [`EditorBackend`] trait sits one level above any specific editor library so that
//! Monaco (source-text), CodeMirror 6 (live-preview), and Tiptap (rich-text) can all
//! plug in behind the same contract. Backend-specific configuration is injected at
//! construction time via [`BackendInit`]; the trait stays narrow.
//!
//! Implementations land in submodules: `monaco` (Phase 1), `codemirror` (Phase 4),
//! `tiptap` (Phase 5).

use std::future::Future;
use std::pin::Pin;

/// Editor mode the active tab is in. The shell renders different surfaces per mode and the
/// active plugin's [`crate::plugin::FormatPlugin`] capability flag determines which buttons
/// appear in the mode toolbar.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Default)]
pub enum EditorMode {
    #[default]
    View,
    Edit,
    LivePreview,
    Split,
}

/// Snapshot of editor state preserved across mode switches whenever both modes share a
/// backend. Cursor / selection are character offsets into the model (Monaco, CM6, ProseMirror
/// all expose offset-based positions; cross-backend swap is documented as dropping cursor).
#[derive(Copy, Clone, Eq, PartialEq, Debug, Default)]
pub struct EditorState {
    pub cursor: u32,
    pub selection: Option<(u32, u32)>,
    pub scroll: u32,
}

/// Editor commands dispatched via [`EditorBackend::dispatch`]. Backends route these to their
/// underlying library.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum EditorCommand {
    Undo,
    Redo,
    FormatDocument,
    FindReplace,
    ToggleComment,
}

/// Static descriptor a format plugin hands the editor backend at mount time. `monaco_language`
/// is one of Monaco's built-in language ids (`"markdown"`, `"json"`, `"plaintext"`);
/// `monarch_grammar` is the optional per-format Monarch tokenizer (used by Phase-6 MDX).
#[derive(Clone, Debug)]
pub struct LanguageDescriptor {
    pub id: &'static str,
    pub monaco_language: &'static str,
    pub monarch_grammar: Option<&'static str>,
}

impl LanguageDescriptor {
    pub const fn markdown() -> Self {
        Self { id: "markdown", monaco_language: "markdown", monarch_grammar: None }
    }
    pub const fn plaintext() -> Self {
        Self { id: "plaintext", monaco_language: "plaintext", monarch_grammar: None }
    }
    pub const fn json() -> Self {
        Self { id: "json", monaco_language: "json", monarch_grammar: None }
    }
}

/// Theme blob translated from the app's `Signal<Theme>` into a per-backend representation.
/// The translator lives in [`crate::theme::editor_theme`] (Phase 2).
#[derive(Clone, Debug, Default)]
pub struct EditorThemeBlob {
    /// Opaque JSON-stringifiable representation. Each backend's `set_theme` interprets it.
    pub blob: String,
}

/// Initial state passed to [`EditorBackend::mount`]. Backend-specific options live here so the
/// trait surface stays minimal.
#[derive(Clone, Debug)]
pub struct BackendInit {
    pub language: LanguageDescriptor,
    pub initial_content: String,
    pub theme: EditorThemeBlob,
    pub read_only: bool,
}

/// A renderer-agnostic editor surface. Mount once, manipulate via the methods below, dispose
/// to release JS-side resources. Implementations are expected to track all `Closure`s
/// crossing the wasm-bindgen boundary so `dispose` doesn't leak.
///
/// `mount` is async because every JS-backed implementation has to await its library's load
/// (Monaco's AMD loader, CM6's ESM dynamic imports, Tiptap's startup). Wasm-bindgen-test
/// gates all assertions on `mount` resolving — no `setTimeout` retry-loops.
pub trait EditorBackend {
    type Target;

    fn mount<'a>(
        &'a mut self,
        target: Self::Target,
        init: BackendInit,
    ) -> Pin<Box<dyn Future<Output = Result<(), EditorError>> + 'a>>;

    fn set_content(&self, content: &str);
    fn get_content(&self) -> String;
    fn on_change(&self, cb: Box<dyn Fn(String) + 'static>);
    fn snapshot(&self) -> EditorState;
    fn restore(&self, state: EditorState);
    fn set_read_only(&self, ro: bool);
    fn set_theme(&self, theme: EditorThemeBlob);
    fn dispatch(&self, cmd: EditorCommand);
    fn dispose(&mut self);
}

/// Editor-side error surface. JS-bridge errors round-trip through here.
#[derive(Debug)]
pub enum EditorError {
    Bridge(String),
    NotMounted,
}

impl std::fmt::Display for EditorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bridge(msg) => write!(f, "editor bridge error: {msg}"),
            Self::NotMounted => write!(f, "editor not mounted"),
        }
    }
}
impl std::error::Error for EditorError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_state_default_is_zero() {
        let s = EditorState::default();
        assert_eq!(s.cursor, 0);
        assert!(s.selection.is_none());
        assert_eq!(s.scroll, 0);
    }

    #[test]
    fn editor_mode_default_is_view() {
        assert_eq!(EditorMode::default(), EditorMode::View);
    }

    #[test]
    fn language_descriptor_constants() {
        assert_eq!(LanguageDescriptor::markdown().monaco_language, "markdown");
        assert_eq!(LanguageDescriptor::plaintext().monaco_language, "plaintext");
        assert_eq!(LanguageDescriptor::json().monaco_language, "json");
    }
}
