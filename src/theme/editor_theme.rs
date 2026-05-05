//! Per-editor-backend theme translators.
//!
//! Each backend (Monaco, CodeMirror 6, Tiptap) wants a different shape:
//! - **Monaco** takes a built-in theme name (`"vs"`, `"vs-dark"`, `"hc-black"`, `"hc-light"`)
//!   or a defined theme via `monaco.editor.defineTheme(name, data)`. v1 maps `ThemeKind` to
//!   a built-in name; the bridge's `setTheme` handles the string-name path. Defining a
//!   per-palette Monaco theme that pulls our `--vscode-editor-*` CSS variables into the
//!   editor's color rules is a v2 follow-up — the editor currently inherits the surrounding
//!   chrome by virtue of Monaco's transparent backgrounds.
//! - **CodeMirror 6** takes an extension (compartment-reconfigurable). Phase 4 lands.
//! - **Tiptap** is plain CSS scoped to `.operon-tiptap`. Phase 5 lands.

use crate::editor::EditorThemeBlob;
use crate::theme::{Theme, ThemeKind};

/// Translate the current [`Theme`] into a Monaco-side blob. Returns the built-in theme name
/// the bridge passes to `monaco.editor.setTheme` — `"vs"` for Light, `"vs-dark"` for Dark,
/// `"hc-black"` for HighContrast.
pub fn monaco_blob(theme: &Theme) -> EditorThemeBlob {
    EditorThemeBlob {
        blob: monaco_builtin_name(theme.kind).to_string(),
    }
}

const fn monaco_builtin_name(kind: ThemeKind) -> &'static str {
    match kind {
        ThemeKind::Dark => "vs-dark",
        ThemeKind::Light => "vs",
        ThemeKind::HighContrast => "hc-black",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeRegistry;

    #[test]
    fn dark_themes_pick_vs_dark() {
        let registry = ThemeRegistry::new();
        let dark = registry.get(crate::theme::ThemeId::VscodeDarkPlus).clone();
        assert_eq!(monaco_blob(&dark).blob, "vs-dark");
    }

    #[test]
    fn light_themes_pick_vs() {
        let registry = ThemeRegistry::new();
        let light = registry.get(crate::theme::ThemeId::VscodeLightPlus).clone();
        assert_eq!(monaco_blob(&light).blob, "vs");
    }

    #[test]
    fn high_contrast_picks_hc_black() {
        let registry = ThemeRegistry::new();
        let hc = registry.get(crate::theme::ThemeId::HighContrastDark).clone();
        assert_eq!(monaco_blob(&hc).blob, "hc-black");
    }

    #[test]
    fn monaco_builtin_name_mapping() {
        assert_eq!(monaco_builtin_name(ThemeKind::Dark), "vs-dark");
        assert_eq!(monaco_builtin_name(ThemeKind::Light), "vs");
        assert_eq!(monaco_builtin_name(ThemeKind::HighContrast), "hc-black");
    }
}
