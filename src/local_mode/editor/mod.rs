//! Editor surface for Local-Mode note tabs.
//!
//! Open a `LocalNote` -> a `Tab` in the shared `TabManager` with `manual_save = true`.
//! Saving runs through the bytes-only `Persistence` trait (the local-mode root is
//! `<store>/local/`); after a successful save we bump `local_note.updated_at_ms`
//! via the repo and clear `Tab.dirty`.
//!
//! The plan note pointed at `NoteHub::open(note_id)` for content storage; that
//! integration requires a local-mode-aware `NoteRepository` (the existing one is
//! cloud-side, keyed by `notes.id`). Threading that through Phase-3 is out of
//! scope, so we keep the existing `Persistence` trait for now and leave the
//! NoteHub wiring as a Phase-4 follow-up. From the user's perspective the save
//! contract is identical (explicit via button + Ctrl+S, dirty indicator).

use std::sync::Arc;

use dioxus::prelude::*;
use operon_store::repos::LocalNoteRepository;
use uuid::Uuid;

use crate::persistence::Persistence;
use crate::tabs::{Tab, TabId, TabManager};

/// Shared callback installed at LocalShell scope. The keyboard handler at the
/// shell root (Ctrl+S) and the explicit Save button both invoke this. It looks
/// up the active tab, snapshots its content, persists it, and bumps the
/// `local_note.updated_at_ms` row.
#[derive(Clone, PartialEq)]
pub struct LocalSaveAction {
    pub callback: Callback<()>,
}

/// Mount the explicit-save action at LocalShell scope. The returned callback
/// is what `Ctrl+S` and the Save button invoke.
pub fn install_save_action(
    mut tabs: Signal<TabManager>,
    persistence: Arc<dyn Persistence>,
    note_repo: Arc<dyn LocalNoteRepository>,
) -> Callback<()> {
    Callback::new(move |_| {
        let active: Option<Tab> = tabs.read().active().cloned();
        let Some(tab) = active else {
            return;
        };
        if !tab.manual_save {
            return;
        }
        let Ok(note_uuid) = Uuid::parse_str(&tab.note_id) else {
            eprintln!(
                "operon: local-save called on non-uuid note_id {}",
                tab.note_id
            );
            return;
        };
        let tab_id = tab.id;
        let note_id = tab.note_id.clone();
        let content = tab.content.clone();
        let persistence = persistence.clone();
        let repo = note_repo.clone();

        spawn(async move {
            match persistence.save(&note_id, content.as_bytes()).await {
                Ok(()) => {
                    if let Err(e) = repo.touch_updated(note_uuid) {
                        eprintln!("operon: touch_updated failed for {note_uuid}: {e}");
                    }
                    tabs.write().set_dirty(tab_id, false);
                }
                Err(e) => {
                    eprintln!("operon: local note save failed: {e}");
                }
            }
        });
    })
}

/// Open (or focus) a Local-Mode note tab for `note_uuid`. The tab uses the
/// `manual_save = true` path so the debounced autosave never fires.
pub fn open_local_note_tab(
    mut tabs: Signal<TabManager>,
    save_scheduler: crate::tabs::SaveScheduler,
    note_uuid: Uuid,
    title: String,
    initial_content: String,
) -> TabId {
    let id = tabs.write().open_manual_save(
        note_uuid.to_string(),
        "markdown".into(),
        title,
        initial_content,
    );
    save_scheduler.set_manual_save(id);
    id
}

/// Save button rendered inside the editor toolbar. Calls the installed
/// `LocalSaveAction` on click.
#[component]
pub fn LocalSaveButton(action: LocalSaveAction, dirty: bool) -> Element {
    let label = if dirty { "Save \u{2022}" } else { "Save" };
    rsx! {
        button {
            r#type: "button",
            class: "px-2 py-1 text-xs rounded border border-[var(--operon-border)] hover:bg-[var(--operon-hover)]",
            "data-testid": "editor-save-button",
            "data-dirty": if dirty { "true" } else { "false" },
            onclick: move |_| action.callback.call(()),
            "{label}"
        }
    }
}

/// The body view for an open Local-Mode note tab. Renders a textarea (the
/// shared `Shell`'s richer editor stack is gated on the cloud-mode plugin
/// pipeline; Phase-3 keeps the surface minimal and replaces it in Phase-4
/// when the editor backends mount under LocalShell).
#[component]
pub fn LocalNoteEditor(tab_id: TabId, action: LocalSaveAction) -> Element {
    let mut tabs: Signal<TabManager> = use_context();
    let snapshot = tabs.read().get(tab_id).cloned();
    let Some(tab) = snapshot else {
        return rsx! { div { class: "text-xs opacity-60", "data-testid": "editor-empty", "No note selected." } };
    };
    let dirty = tab.dirty;
    let content = tab.content.clone();
    let title = tab.title.clone();

    rsx! {
        div {
            class: "flex flex-col h-full w-full",
            "data-testid": "local-note-editor",
            "data-tab-id": "{tab_id.0}",
            div {
                class: "flex items-center justify-between px-2 py-1 border-b border-[var(--operon-border)]",
                div {
                    class: "flex items-center gap-2 text-sm truncate",
                    "data-testid": "tab-title",
                    if dirty {
                        span {
                            class: "opacity-80",
                            "data-testid": "tab-title-dirty-indicator",
                            "\u{2022} "
                        }
                    }
                    span { class: "truncate", "{title}" }
                }
                LocalSaveButton { action: action.clone(), dirty }
            }
            textarea {
                class: "flex-1 w-full p-2 text-sm font-mono bg-[var(--operon-bg)] text-[var(--operon-fg)] outline-none resize-none",
                "data-testid": "local-note-textarea",
                value: "{content}",
                oninput: move |evt| {
                    tabs.write().set_content(tab_id, evt.value());
                },
                onkeydown: move |evt| {
                    let mods = evt.modifiers();
                    let with_meta = mods.contains(Modifiers::META) || mods.contains(Modifiers::CONTROL);
                    if with_meta
                        && !mods.contains(Modifiers::SHIFT)
                        && !mods.contains(Modifiers::ALT)
                        && evt.key().to_string().eq_ignore_ascii_case("s")
                    {
                        evt.prevent_default();
                        action.callback.call(());
                    }
                },
            }
        }
    }
}
