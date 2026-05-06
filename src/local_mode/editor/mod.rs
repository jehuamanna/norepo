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

/// Plans-Phase-6-image-notes: if the given note id is an image, return an
/// inline image viewer. Reads the LocalNote row + the vault root from
/// already-resolved context handles, then base64-encodes the image bytes
/// for an inline `<img>` src. Returns `None` when the note isn't an image,
/// the row can't be found, or the blob is missing.
fn try_render_image_view(
    note_id: Uuid,
    note_repo_handle: &crate::local_mode::desktop::LocalNoteRepo,
    project_repo_handle: &crate::local_mode::desktop::LocalProjectRepo,
    vault_handle: &crate::local_mode::desktop::CurrentVaultRoot,
) -> Option<Element> {
    use operon_store::repos::NoteKind;

    let crate::local_mode::desktop::LocalNoteRepo(note_repo) = note_repo_handle;
    let crate::local_mode::desktop::LocalProjectRepo(project_repo) = project_repo_handle;
    let crate::local_mode::desktop::CurrentVaultRoot(vault_signal) = vault_handle;
    let vault = vault_signal.read().clone()?;

    let projects = project_repo.list().ok()?;
    let row = projects.into_iter().find_map(|p| {
        note_repo
            .list_for_project(p.id)
            .ok()?
            .into_iter()
            .find(|n| n.id == note_id)
    })?;

    if !matches!(row.kind, NoteKind::Image) {
        return None;
    }
    let rel = row.blob_path.clone()?;
    let rel_path = std::path::Path::new(&rel).to_path_buf();
    let bytes = crate::local_mode::images::read_image(&vault, &rel_path).ok()?;

    // Compose a data: URL for inline rendering. base64 inline encoder
    // (we don't pull a base64 crate just for this).
    let mime = match rel_path.extension().and_then(|s| s.to_str()).unwrap_or("") {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        _ => "application/octet-stream",
    };
    let data_url = format!("data:{mime};base64,{}", base64_encode(&bytes));

    Some(rsx! {
        div {
            class: "operon-local-image-view",
            "data-testid": "image-note-view",
            "data-note-id": "{note_id}",
            style: "display: flex; align-items: center; justify-content: center; height: 100%; overflow: auto; padding: 1rem; background: var(--operon-bg, #111);",
            img {
                src: "{data_url}",
                alt: "{row.title}",
                style: "max-width: 100%; max-height: 100%; object-fit: contain;",
            }
        }
    })
}

/// Tiny inline base64 encoder so the image-tab view can render via a
/// `data:` URL without pulling a base64 crate. Standard alphabet, padded.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHA: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = if chunk.len() > 1 { chunk[1] } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] } else { 0 };
        let triple = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(ALPHA[((triple >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((triple >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHA[((triple >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHA[(triple & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
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

/// Local-Mode editor body: a plain textarea bound to `Tab.content`, or
/// an image viewer when the active note is `NoteKind::Image`.
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
    // Plans-Phase-6-image-notes: image-tab view dependencies. Hooks must
    // run unconditionally; the actual rendering is gated below.
    let note_repo_for_image: crate::local_mode::desktop::LocalNoteRepo = use_context();
    let project_repo_for_image: crate::local_mode::desktop::LocalProjectRepo = use_context();
    let vault_for_image: crate::local_mode::desktop::CurrentVaultRoot = use_context();

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

    if let Ok(uuid) = Uuid::parse_str(&tab.note_id) {
        if let Some(view) = try_render_image_view(
            uuid,
            &note_repo_for_image,
            &project_repo_for_image,
            &vault_for_image,
        ) {
            return view;
        }
    }

    let content = tab.content.clone();

    // Plans-Phase-6-image-notes: Cmd/Ctrl+Shift+I opens an image picker,
    // writes the chosen file, mints a child image-note under the current
    // note, and appends an Obsidian-style `![[…]]` reference to the
    // current body. Caret-position insertion would require a JS bridge
    // (textarea selectionStart) — acceptable follow-up.
    let mut tabs_for_image = tabs;
    let crate::local_mode::desktop::LocalNoteRepo(note_repo_for_paste) =
        note_repo_for_image.clone();
    let crate::local_mode::desktop::CurrentVaultRoot(vault_signal_for_paste) =
        vault_for_image.clone();
    let on_insert_image_via_picker = move || {
        let Ok(parent_uuid) = Uuid::parse_str(&tab.note_id) else {
            return;
        };
        let Some(vault) = vault_signal_for_paste.read().clone() else {
            return;
        };
        let project_id_opt = {
            let crate::local_mode::desktop::LocalProjectRepo(prepo) =
                project_repo_for_image.clone();
            let projects = prepo.list().unwrap_or_default();
            projects.iter().find_map(|p| {
                note_repo_for_paste
                    .list_for_project(p.id)
                    .ok()
                    .and_then(|notes| notes.iter().find(|n| n.id == parent_uuid).map(|_| p.id))
            })
        };
        let Some(project_id) = project_id_opt else {
            return;
        };
        let note_repo = note_repo_for_paste.clone();
        spawn(async move {
            let Some(handle) = rfd::AsyncFileDialog::new()
                .set_title("Insert image")
                .add_filter("Image", &["png", "jpg", "jpeg", "webp", "gif", "svg", "avif"])
                .pick_file()
                .await
            else {
                return;
            };
            let path = handle.path().to_path_buf();
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("operon: insert image read failed: {e}");
                    return;
                }
            };
            let mime = match path
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.to_ascii_lowercase())
                .as_deref()
            {
                Some("png") => "image/png",
                Some("jpg") | Some("jpeg") => "image/jpeg",
                Some("webp") => "image/webp",
                Some("gif") => "image/gif",
                Some("svg") => "image/svg+xml",
                Some("avif") => "image/avif",
                _ => return,
            };
            let written = match crate::local_mode::images::write_image(&vault, &bytes, mime) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("operon: insert image write failed: {e}");
                    return;
                }
            };
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Image")
                .to_string();
            let new_note = match note_repo.create_with_kind(
                project_id,
                Some(parent_uuid),
                &stem,
                operon_store::repos::NoteKind::Image,
            ) {
                Ok(n) => n,
                Err(e) => {
                    eprintln!("operon: insert image create_with_kind failed: {e}");
                    return;
                }
            };
            let rel = written.relative_path.to_string_lossy().to_string();
            if let Err(e) = note_repo.set_blob_path(new_note.id, Some(&rel)) {
                eprintln!("operon: insert image set_blob_path failed: {e}");
            }
            let short = operon_store::vfs::short_id(new_note.id);
            let embed = format!("![[{}^{}]]", stem, short);
            // Append the embed to the current body. Caret-position insertion
            // is a follow-up.
            let current = tabs_for_image
                .read()
                .get(tab_id)
                .map(|t| t.content.clone())
                .unwrap_or_default();
            let next = if current.ends_with('\n') || current.is_empty() {
                format!("{current}{embed}\n")
            } else {
                format!("{current}\n{embed}\n")
            };
            tabs_for_image.write().set_content(tab_id, next);
            tabs_for_image.write().set_dirty(tab_id, true);
        });
    };

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
                    && mods.contains(Modifiers::SHIFT)
                    && !mods.contains(Modifiers::ALT)
                    && evt.key().to_string().eq_ignore_ascii_case("i")
                {
                    // Plans-Phase-6-image-notes: insert image via picker.
                    evt.prevent_default();
                    on_insert_image_via_picker();
                    return;
                }
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
