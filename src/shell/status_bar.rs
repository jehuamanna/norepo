//! Status bar: bottom-spanning row.
//!
//! Surfaces metadata about the active tab (note kind, title, dirty state,
//! editor mode, word/char counts, full file path on hover) plus the global
//! controls (theme toggle, Local Mode badge). Reads from `TabManager`,
//! `AppState`, `Theme`, and (Local Mode only) `CurrentVaultRoot` so it
//! refreshes whenever the user types, switches tabs, or changes theme.

use dioxus::prelude::*;
use operon_store::repos::NoteKind;

use crate::editor::EditorMode;
use crate::rbag::state::{AppState, Mode};
use crate::tabs::TabManager;
use crate::theme::{self, Theme, ThemeKind};

#[component]
pub fn StatusBar() -> Element {
    let mut theme_signal: Signal<Theme> = use_context();
    let app_state: Signal<AppState> = use_context();
    let tabs: Signal<TabManager> = use_context();
    let local_username: Option<crate::local_mode::LocalUsername> = try_consume_context();
    #[cfg(not(target_arch = "wasm32"))]
    let vault_root: Option<crate::local_mode::CurrentVaultRoot> = try_consume_context();

    let mode_label = match theme_signal.read().kind {
        ThemeKind::Dark => "Dark",
        ThemeKind::Light => "Light",
        ThemeKind::HighContrast => "HC",
    };

    let is_local = app_state.read().mode == Mode::Local;
    let username_value = local_username
        .map(|u| u.0.read().clone())
        .unwrap_or_else(|| "Local user".to_string());

    // Active-tab snapshot: kind / title / dirty / mode / content metrics.
    // Computed once per render; if no tab is open, `meta` is None and the
    // tab section is skipped.
    let meta: Option<TabMeta> = {
        let snap = tabs.read();
        snap.active().map(|t| {
            let kind = NoteKind::from_str(&t.format_id);
            let (words, chars, lines) = body_metrics(&t.content);
            TabMeta {
                note_id: t.note_id.clone(),
                title: if t.title.trim().is_empty() {
                    "Untitled".to_string()
                } else {
                    t.title.clone()
                },
                kind,
                dirty: t.dirty,
                mode: t.mode,
                words,
                chars,
                lines,
            }
        })
    };

    // Build the full filesystem path for the active note when we're in
    // Local Mode and have a vault root. Markdown / MDX / Code notes are
    // `<vault>/notes/<id>.md|.mdx|...`; image notes live under
    // `<vault>/.operon/images/...` but we don't have the blob filename
    // in the tab snapshot, so we surface the parent directory for those.
    #[cfg(not(target_arch = "wasm32"))]
    let full_path: Option<String> = if is_local {
        match (&meta, &vault_root) {
            (Some(m), Some(crate::local_mode::CurrentVaultRoot(vr))) => {
                vr.read().as_ref().map(|root| {
                    let ext = path_extension_for_kind(m.kind);
                    let notes_dir = root.notes_dir();
                    match m.kind {
                        NoteKind::Image => root.images_dir().display().to_string(),
                        _ => notes_dir
                            .join(format!("{}.{}", m.note_id, ext))
                            .display()
                            .to_string(),
                    }
                })
            }
            _ => None,
        }
    } else {
        None
    };
    #[cfg(target_arch = "wasm32")]
    let full_path: Option<String> = None;

    rsx! {
        section {
            "data-region": "status-bar",
            class: "operon-region operon-status-bar",
            role: "status",
            "aria-live": "polite",
            span {
                class: "operon-status-bar-label",
                "data-testid": "status-bar-brand",
                "Operon"
            }
            if let Some(m) = meta {
                {
                    let kind_label = m.kind.display_name();
                    let kind_slug = m.kind.as_str();
                    let mode_label = mode_display(m.mode);
                    let dirty_label = if m.dirty { "Unsaved" } else { "Saved" };
                    let dirty_glyph = if m.dirty { "●" } else { "✓" };
                    let title_attr = full_path
                        .clone()
                        .unwrap_or_else(|| format!("Note id: {}", m.note_id));
                    let metrics_label = if matches!(m.kind, NoteKind::Markdown | NoteKind::Mdx | NoteKind::Code) {
                        Some(format!("{} words · {} chars · {} lines", m.words, m.chars, m.lines))
                    } else {
                        None
                    };
                    rsx! {
                        span {
                            class: "operon-status-section operon-status-kind",
                            "data-testid": "status-bar-kind",
                            "data-note-kind": "{kind_slug}",
                            title: "Note kind",
                            "[{kind_slug}] {kind_label}"
                        }
                        span {
                            class: "operon-status-section operon-status-title",
                            "data-testid": "status-bar-title",
                            title: "{title_attr}",
                            "{m.title}"
                        }
                        span {
                            class: "operon-status-section operon-status-dirty",
                            "data-testid": "status-bar-dirty",
                            "data-dirty": if m.dirty { "true" } else { "false" },
                            title: "{dirty_label}",
                            "{dirty_glyph} {dirty_label}"
                        }
                        if is_local {
                            span {
                                class: "operon-status-section operon-status-mode",
                                "data-testid": "status-bar-mode",
                                title: "Editor mode",
                                "{mode_label}"
                            }
                        }
                        if let Some(metrics) = metrics_label {
                            span {
                                class: "operon-status-section operon-status-metrics",
                                "data-testid": "status-bar-metrics",
                                title: "Body metrics",
                                "{metrics}"
                            }
                        }
                    }
                }
            }
            span { style: "flex: 1 1 auto;" }
            if is_local {
                span {
                    "data-testid": "top-right-badge",
                    style: "margin-right: 8px;",
                    span {
                        "data-testid": "status-bar-local-badge",
                        class: "operon-status-local-badge",
                        style: "border: 1px solid var(--vscode-panel-border); padding: 2px 8px; font: inherit; opacity: 0.85;",
                        "Local · "
                        "{username_value}"
                    }
                }
            }
            button {
                r#type: "button",
                class: "operon-status-toggle",
                "data-action": "toggle-theme",
                "aria-label": "Toggle theme",
                style: "background: transparent; color: inherit; border: 1px solid var(--vscode-panel-border); padding: 2px 8px; cursor: pointer; font: inherit;",
                onclick: move |_| {
                    let next = match theme_signal.read().kind {
                        ThemeKind::Dark | ThemeKind::HighContrast => theme::defaults::light(),
                        ThemeKind::Light => theme::defaults::dark(),
                    };
                    theme_signal.set(next);
                },
                "Theme: {mode_label}"
            }
        }
    }
}

#[derive(Clone)]
struct TabMeta {
    note_id: String,
    title: String,
    kind: NoteKind,
    dirty: bool,
    mode: EditorMode,
    words: usize,
    chars: usize,
    lines: usize,
}

const fn mode_display(m: EditorMode) -> &'static str {
    match m {
        EditorMode::View => "View",
        EditorMode::Edit => "Edit",
        EditorMode::LivePreview => "Live Preview",
        EditorMode::Split => "Split",
    }
}

const fn path_extension_for_kind(k: NoteKind) -> &'static str {
    match k {
        NoteKind::Markdown => "md",
        NoteKind::Mdx => "mdx",
        NoteKind::Code => "txt",
        NoteKind::Kanban => "json",
        NoteKind::Canvas => "json",
        NoteKind::Excalidraw => "json",
        NoteKind::Image => "bin",
        NoteKind::Skill => "md",
        NoteKind::Workflow => "json",
        NoteKind::Artifact => "md",
        NoteKind::Phase => "md",
    }
}

/// Cheap word / char / line counter over the full body. Skipped for
/// non-text kinds (image / canvas / etc.) so we don't run it on opaque
/// blobs. Word counter splits on ASCII whitespace; char count is byte-
/// length-of-content (close enough for ASCII-heavy markdown — a Unicode
/// scalar walk would be cleaner if we ever surface this for non-Latin
/// input).
fn body_metrics(body: &str) -> (usize, usize, usize) {
    let words = body.split_whitespace().count();
    let chars = body.chars().count();
    let lines = if body.is_empty() {
        0
    } else {
        body.lines().count()
    };
    (words, chars, lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_empty() {
        assert_eq!(body_metrics(""), (0, 0, 0));
    }

    #[test]
    fn metrics_simple() {
        let (w, c, l) = body_metrics("hello world\nsecond line");
        assert_eq!(w, 4);
        assert_eq!(c, "hello world\nsecond line".chars().count());
        assert_eq!(l, 2);
    }

    #[test]
    fn metrics_multibyte() {
        // Two CJK chars + space + emoji.
        let (w, c, l) = body_metrics("日本 🦀");
        assert_eq!(w, 2);
        assert_eq!(c, 4);
        assert_eq!(l, 1);
    }

    #[test]
    fn mode_display_covers_all_variants() {
        assert_eq!(mode_display(EditorMode::View), "View");
        assert_eq!(mode_display(EditorMode::Edit), "Edit");
        assert_eq!(mode_display(EditorMode::LivePreview), "Live Preview");
        assert_eq!(mode_display(EditorMode::Split), "Split");
    }

    #[test]
    fn path_extension_markdown_is_md() {
        assert_eq!(path_extension_for_kind(NoteKind::Markdown), "md");
        assert_eq!(path_extension_for_kind(NoteKind::Mdx), "mdx");
    }
}
