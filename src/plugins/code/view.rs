//! View / Edit components for `CodeFormatPlugin`. The editor is a thin wrapper
//! over `MonacoEditorHost` plus a small language picker that re-mounts the
//! host when the user changes language.

use dioxus::prelude::*;

use crate::editor::LanguageDescriptor;

#[component]
pub fn CodeView(content: String) -> Element {
    rsx! {
        pre { class: "operon-code-view", style: "white-space: pre; overflow: auto; padding: 1rem;", "{content}" }
    }
}

const KNOWN_LANGUAGES: &[(&str, &str)] = &[
    ("plaintext", "Plain text"),
    ("rust", "Rust"),
    ("python", "Python"),
    ("javascript", "JavaScript"),
    ("typescript", "TypeScript"),
    ("go", "Go"),
    ("java", "Java"),
    ("cpp", "C / C++"),
    ("ruby", "Ruby"),
    ("shell", "Shell"),
    ("yaml", "YAML"),
    ("toml", "TOML"),
    ("html", "HTML"),
    ("css", "CSS"),
    ("sql", "SQL"),
    ("json", "JSON"),
];

#[component]
pub fn CodeEditor(
    note_id: String,
    content: String,
    language: LanguageDescriptor,
    on_change: EventHandler<String>,
) -> Element {
    // Track the user-selected language. `monaco_language` is what Monaco
    // wants ("rust", "python", …); the LanguageDescriptor's `id` stays
    // "code" so the host's identity tracking groups all code tabs together.
    let mut selected_lang: Signal<&'static str> =
        use_signal(|| language.monaco_language);
    let lang_now = *selected_lang.read();
    let descriptor = LanguageDescriptor::code_with(lang_now);

    rsx! {
        div {
            class: "operon-code-editor",
            "data-testid": "code-editor",
            style: "display: flex; flex-direction: column; height: 100%;",
            div {
                class: "operon-code-toolbar",
                style: "display: flex; align-items: center; gap: 0.5rem; padding: 0.25rem 0.5rem; border-bottom: 1px solid var(--operon-border);",
                label {
                    style: "font-size: 0.85em; opacity: 0.7;",
                    "Language:"
                }
                select {
                    "data-testid": "code-language-picker",
                    value: "{lang_now}",
                    onchange: move |evt| {
                        let v = evt.value();
                        if let Some((id, _)) = KNOWN_LANGUAGES.iter().find(|(id, _)| *id == v) {
                            selected_lang.set(*id);
                        }
                    },
                    for (id, label) in KNOWN_LANGUAGES.iter() {
                        option { value: "{id}", "{label}" }
                    }
                }
            }
            div {
                style: "flex: 1; min-height: 0;",
                // Re-mount the Monaco host whenever language changes so it
                // picks up the new descriptor cleanly.
                crate::shell::editor_host::MonacoEditorHost {
                    key: "{lang_now}",
                    note_id: note_id.clone(),
                    content: content.clone(),
                    language: descriptor,
                    on_change,
                }
            }
        }
    }
}
