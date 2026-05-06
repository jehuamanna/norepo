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

use crate::editor::EditorMode;
use crate::persistence::Persistence;
use crate::tabs::{SaveScheduler, Tab, TabId, TabManager};

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
///
/// Plans-Phase-2-saving: routes through [`SaveScheduler::flush`] so the
/// manual save and the 150 ms debounced autosave converge on a single
/// `Persistence::save` call site. Calling `flush` first cancels any
/// in-flight debounce future for the tab so we never double-write.
pub fn install_save_action(
    mut tabs: Signal<TabManager>,
    _persistence: Arc<dyn Persistence>,
    note_repo: Arc<dyn LocalNoteRepository>,
    scheduler: SaveScheduler,
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
        let repo = note_repo.clone();
        let scheduler = scheduler.clone();

        spawn(async move {
            match scheduler.flush(tab_id, &note_id, &content).await {
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
    // Local Mode opens notes in Edit mode by default; the right-click menu
    // on the note row offers View / Split-view as alternatives.
    tabs.write().set_mode(id, EditorMode::Edit);
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

/// Local-Mode editor body: a plain textarea bound to `Tab.content`.
///
/// The shared cloud `MarkdownFormatPlugin::render_edit` mounts Monaco, which
/// today only initializes on `target_arch = "wasm32"` — the desktop build
/// renders a non-functional placeholder. Local Mode bypasses that placeholder
/// with this simple textarea so notes are actually editable. Save lives off
/// `Ctrl+S` (Shell-level binding + a textarea-local fallback) and the floating
/// `LocalSaveButton` rendered by `LocalShellOverlay`. Tab title chrome comes
/// from the existing `TabStrip` above this body.
#[component]
pub fn LocalNoteEditor(tab_id: TabId, action: LocalSaveAction) -> Element {
    let mut tabs: Signal<TabManager> = use_context();
    let snapshot = tabs.read().get(tab_id).cloned();
    let Some(tab) = snapshot else {
        return rsx! {
            div {
                class: "operon-local-editor-empty",
                "data-testid": "editor-empty",
                "No note selected."
            }
        };
    };
    let content = tab.content.clone();

    rsx! {
        textarea {
            class: "operon-local-editor",
            "data-testid": "local-note-textarea",
            "data-tab-id": "{tab_id.0}",
            value: "{content}",
            spellcheck: "false",
            autofocus: true,
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
