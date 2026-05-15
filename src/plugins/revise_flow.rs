//! Shared Revise / Cancel / Done flow for any note format whose
//! revisions are recorded inline as a `## Revision history` table.
//!
//! **The flow.** A note is read-only by default (View). Clicking
//! `Revise` snapshots the current body, flips the tab to Edit, and
//! reveals a `Cancel` + `Done` pair next to the Revise button. `Done`
//! opens a confirm dialog with a required single-line summary; on
//! Confirm we append a `manual` row to the body via
//! [`revision_table::append_revision_row`], persist, and flip back to
//! View. `Cancel` reverts buffer + disk to the pre-Revise snapshot
//! (no row recorded) and flips back to View.
//!
//! Reused from:
//! - `src/plugins/skill/view.rs::SkillToolbar` — inline alongside the
//!   skill's ▶ Run button.
//! - `src/shell/mode_toolbar.rs::ModeToolbar` — alongside the View /
//!   Revise / Split mode buttons, for every EDIT-capable format whose
//!   own surface doesn't already mount the flow (skill is the one
//!   exception; it mounts the buttons in its own toolbar so they sit
//!   next to ▶ Run).

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use dioxus::prelude::*;

use crate::persistence::Persistence;
use crate::plugins::artifact::revision_table;

#[derive(Props, Clone, PartialEq)]
pub struct RevisionFlowButtonsProps {
    /// Note id (UUID string) the buttons operate on. Used to scope
    /// the active-tab snapshot read so two open notes don't see each
    /// other's mode flips, and to route the persistence save.
    pub note_id: String,
    /// Optional CSS class root. Defaults to `operon-revise-flow`;
    /// callers that want format-specific styling (skill toolbar vs
    /// global mode toolbar) override this so the buttons inherit the
    /// surrounding chrome.
    #[props(default = "operon-revise-flow".to_string())]
    pub class_root: String,
    /// Optional `data-testid` prefix; defaults to `revise-flow`.
    /// Lets each call site name its instance distinctly so e2e
    /// selectors don't collide across formats.
    #[props(default = "revise-flow".to_string())]
    pub testid_prefix: String,
}

/// Revise / Cancel / Done button cluster + confirm dialog. Reads the
/// active tab from `Signal<TabManager>` and only renders when that
/// tab's `note_id` matches `props.note_id` — keeps two side-by-side
/// notes from cross-firing.
#[component]
pub fn RevisionFlowButtons(props: RevisionFlowButtonsProps) -> Element {
    let persistence: Arc<dyn Persistence> = use_context();
    let mut tabs: Signal<crate::tabs::TabManager> = use_context();

    let mut prior_body: Signal<Option<String>> = use_signal(|| None);
    let mut dialog_open: Signal<bool> = use_signal(|| false);
    let mut draft_summary: Signal<String> = use_signal(String::new);

    let active_snapshot: Option<(crate::tabs::TabId, crate::editor::EditorMode, String, String)> = {
        let snap = tabs.read();
        snap.active()
            .map(|t| (t.id, t.mode, t.note_id.clone(), t.content.clone()))
    };
    let Some((tab_id, mode, active_note_id, body_now)) = active_snapshot else {
        return rsx! {};
    };
    if active_note_id != props.note_id {
        return rsx! {};
    }
    let in_edit = matches!(mode, crate::editor::EditorMode::Edit);

    let body_for_button = body_now.clone();
    let on_button_click = move |_| {
        if !in_edit {
            prior_body.set(Some(body_for_button.clone()));
            tabs.write().set_mode(tab_id, crate::editor::EditorMode::Edit);
            return;
        }
        draft_summary.set(String::new());
        dialog_open.set(true);
    };

    let persistence_for_confirm = persistence.clone();
    let body_for_confirm = body_now.clone();
    let note_id_for_confirm = props.note_id.clone();
    let on_confirm = move |_| {
        let final_summary = draft_summary.read().trim().to_string();
        if final_summary.is_empty() {
            return;
        }
        let date = revision_table::format_revision_date(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
        );
        let row = revision_table::RevisionRow {
            revision: revision_table::next_revision_number(&body_for_confirm),
            date,
            derived_from: "manual".to_string(),
            summary: final_summary,
        };
        let body_with_row = revision_table::append_revision_row(&body_for_confirm, row);

        tabs.write()
            .reload_content(tab_id, body_with_row.clone());

        let note_id = note_id_for_confirm.clone();
        let pers = persistence_for_confirm.clone();
        let body_for_save = body_with_row.clone();
        let mut tabs_for_reassert = tabs;
        dioxus::core::spawn_forever(async move {
            if let Err(e) = pers.save(&note_id, body_for_save.as_bytes()).await {
                tracing::warn!(target: "operon::revise", "Done: save({note_id}): {e}");
            }
            // Re-assert buffer post-save so a late onChange firing
            // during the mode flip doesn't clobber the new row.
            tabs_for_reassert
                .write()
                .reload_content(tab_id, body_for_save);
            crate::shell::companion_state::LOCAL_NOTE_VERSION
                .with_mut(|v| *v = v.saturating_add(1));
        });

        dialog_open.set(false);
        prior_body.set(None);
        draft_summary.set(String::new());
        tabs.write().set_mode(tab_id, crate::editor::EditorMode::View);
    };

    let on_dialog_cancel = move |_| {
        // Dialog Cancel = close the dialog only; the user stays in
        // Edit so they can keep tweaking the body and click Done
        // again. The toolbar Cancel below is the one that actually
        // abandons the revise session.
        dialog_open.set(false);
        draft_summary.set(String::new());
    };

    let persistence_for_revise_cancel = persistence.clone();
    let note_id_for_revise_cancel = props.note_id.clone();
    let on_revise_cancel = move |_| {
        let prior = prior_body.read().clone();
        dialog_open.set(false);
        draft_summary.set(String::new());
        if let Some(prior) = prior {
            tabs.write().reload_content(tab_id, prior.clone());
            let note_id = note_id_for_revise_cancel.clone();
            let pers = persistence_for_revise_cancel.clone();
            let mut tabs_for_reassert = tabs;
            let prior_for_reassert = prior.clone();
            dioxus::core::spawn_forever(async move {
                if let Err(e) = pers.save(&note_id, prior.as_bytes()).await {
                    tracing::warn!(
                        target: "operon::revise",
                        "Revise cancel: revert save({note_id}): {e}"
                    );
                }
                tabs_for_reassert
                    .write()
                    .reload_content(tab_id, prior_for_reassert);
            });
        }
        prior_body.set(None);
        tabs.write().set_mode(tab_id, crate::editor::EditorMode::View);
    };

    let label = if in_edit { "Done" } else { "Edit" };
    let title_attr = if in_edit {
        "Open the revision-summary dialog to commit your edits."
    } else {
        "Switch to Edit mode. Click Done when finished to record the revision."
    };
    let primary_class = format!("{}-primary", props.class_root);
    let cancel_class = format!("{}-cancel", props.class_root);
    let primary_testid = format!("{}-primary", props.testid_prefix);
    let cancel_testid = format!("{}-cancel", props.testid_prefix);
    let dialog_testid = format!("{}-dialog", props.testid_prefix);

    rsx! {
        button {
            r#type: "button",
            class: "{primary_class}",
            "data-testid": "{primary_testid}",
            title: "{title_attr}",
            onclick: on_button_click,
            "{label}"
        }
        if in_edit {
            button {
                r#type: "button",
                class: "{cancel_class}",
                "data-testid": "{cancel_testid}",
                title: "Abandon this revise session: discard any in-progress edits and return to View.",
                onclick: on_revise_cancel,
                "Cancel"
            }
        }
        if *dialog_open.read() {
            ManualSummaryDialog {
                draft: draft_summary,
                testid_prefix: dialog_testid,
                on_confirm: EventHandler::new(on_confirm),
                on_cancel: EventHandler::new(on_dialog_cancel),
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct ManualSummaryDialogProps {
    draft: Signal<String>,
    testid_prefix: String,
    on_confirm: EventHandler<()>,
    on_cancel: EventHandler<()>,
}

/// Manual revision-summary dialog. Required, single-line, no LLM
/// drafting. Reuses the artifact's `operon-revision-dialog-*` CSS
/// classes for visual consistency across formats.
#[component]
fn ManualSummaryDialog(props: ManualSummaryDialogProps) -> Element {
    let mut draft = props.draft;
    let confirm_handler = props.on_confirm;
    let cancel_handler = props.on_cancel;
    let confirm_disabled = draft.read().trim().is_empty();
    let scrim_testid = format!("{}-scrim", props.testid_prefix);
    let textarea_testid = format!("{}-textarea", props.testid_prefix);
    let cancel_testid = format!("{}-cancel", props.testid_prefix);
    let confirm_testid = format!("{}-confirm", props.testid_prefix);

    rsx! {
        div {
            class: "operon-revision-dialog-scrim",
            "data-testid": "{scrim_testid}",
            onclick: move |_| cancel_handler.call(()),
        }
        div {
            class: "operon-revision-dialog",
            "data-testid": "{props.testid_prefix}",
            role: "dialog",
            "aria-modal": "true",
            onkeydown: move |evt| {
                if evt.key().to_string() == "Escape" {
                    cancel_handler.call(());
                }
            },
            div { class: "operon-revision-dialog-header",
                h3 { class: "operon-revision-dialog-title", "Record revision" }
                p { class: "operon-revision-dialog-hint",
                    "Type a one-line summary of what changed. Saved as a `manual` row in the revision-history table."
                }
            }
            textarea {
                class: "operon-revision-dialog-textarea",
                "data-testid": "{textarea_testid}",
                rows: "3",
                value: "{draft}",
                placeholder: "Summary of revision\u{2026}",
                oninput: move |e| draft.set(e.value()),
            }
            div { class: "operon-revision-dialog-actions",
                button {
                    r#type: "button",
                    class: "operon-revision-dialog-cancel",
                    "data-testid": "{cancel_testid}",
                    onclick: move |_| cancel_handler.call(()),
                    "Cancel"
                }
                button {
                    r#type: "button",
                    class: "operon-revision-dialog-confirm",
                    "data-testid": "{confirm_testid}",
                    disabled: confirm_disabled,
                    onclick: move |_| confirm_handler.call(()),
                    "Confirm"
                }
            }
        }
    }
}
