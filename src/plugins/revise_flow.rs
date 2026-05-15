//! Shared Revise / Cancel / Done flow for any note format whose
//! revisions are recorded inline as a `## Revision history` table.
//!
//! **The flow.** A note is read-only by default (View). Clicking
//! `Revise` snapshots the current body, flips the tab to Edit, and
//! reveals a `Cancel` + `Done` pair next to the Revise button. `Done`
//! opens a confirm dialog that kicks off a one-shot `claude --print`
//! call to draft a one-line summary of the diff; the user can accept,
//! edit, or regenerate the draft before clicking Confirm. On Confirm
//! we append a `manual` row to the body via
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

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use dioxus::prelude::*;
use similar::{ChangeTag, TextDiff};
use tokio::io::AsyncReadExt;
use tokio::time::timeout;

use crate::persistence::Persistence;
use crate::plugins::artifact::revision_table;
use crate::shell::companion_state::ActiveRepoPath;

/// Lifecycle of the LLM-drafted revision summary surfaced in the Done
/// dialog. Drives the loading spinner / error banner and gates the
/// Confirm button while we're still waiting on a draft.
#[derive(Clone, Debug, PartialEq)]
enum AiSummaryState {
    /// Dialog isn't asking for a draft (initial mount, or post-cancel).
    Idle,
    /// `claude --print` call in flight.
    Loading,
    /// Draft landed (or the user picked Regenerate after one).
    Ready,
    /// Draft failed; the textarea falls back to fully-manual entry.
    Error(String),
}

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
    // Best-effort: the active project repo gives `claude --print` a
    // cwd with the project's CLAUDE.md visible. Vault-only / standalone
    // surfaces (no project bound) fall through to current_dir below.
    let active_repo: Option<Signal<Option<PathBuf>>> =
        try_consume_context::<ActiveRepoPath>().map(|c| c.0);

    let mut prior_body: Signal<Option<String>> = use_signal(|| None);
    let mut dialog_open: Signal<bool> = use_signal(|| false);
    let mut draft_summary: Signal<String> = use_signal(String::new);
    let mut ai_state: Signal<AiSummaryState> = use_signal(|| AiSummaryState::Idle);
    // Set by the textarea's oninput so an in-flight draft doesn't
    // overwrite something the user already started typing.
    let mut user_edited: Signal<bool> = use_signal(|| false);

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
    let body_for_draft_open = body_now.clone();
    let on_button_click = move |_| {
        if !in_edit {
            prior_body.set(Some(body_for_button.clone()));
            tabs.write().set_mode(tab_id, crate::editor::EditorMode::Edit);
            return;
        }
        dialog_open.set(true);
        let cwd = active_repo
            .as_ref()
            .and_then(|s| s.read().clone())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        kick_off_draft(
            prior_body.read().clone().unwrap_or_default(),
            body_for_draft_open.clone(),
            cwd,
            ai_state,
            draft_summary,
            dialog_open,
            user_edited,
        );
    };

    let body_for_draft_regen = body_now.clone();
    let on_regenerate = move |_| {
        let cwd = active_repo
            .as_ref()
            .and_then(|s| s.read().clone())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        kick_off_draft(
            prior_body.read().clone().unwrap_or_default(),
            body_for_draft_regen.clone(),
            cwd,
            ai_state,
            draft_summary,
            dialog_open,
            user_edited,
        );
    };

    let persistence_for_confirm = persistence.clone();
    let body_for_confirm = body_now.clone();
    let note_id_for_confirm = props.note_id.clone();
    let on_confirm = move |_| {
        let final_summary = draft_summary.read().trim().to_string();
        if final_summary.is_empty() {
            return;
        }
        // Block Confirm while the draft is still inflight — the
        // button is disabled in the UI but defend in depth in case a
        // keyboard handler reaches us first.
        if matches!(*ai_state.read(), AiSummaryState::Loading) {
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
        ai_state.set(AiSummaryState::Idle);
        user_edited.set(false);
        tabs.write().set_mode(tab_id, crate::editor::EditorMode::View);
    };

    let on_dialog_cancel = move |_| {
        // Dialog Cancel = close the dialog only; the user stays in
        // Edit so they can keep tweaking the body and click Done
        // again. The toolbar Cancel below is the one that actually
        // abandons the revise session. Resetting ai_state to Idle
        // suppresses any late draft that lands after the close —
        // `kick_off_draft`'s spawn task bails when dialog_open is
        // false, but this also ensures the next Done opens cleanly.
        dialog_open.set(false);
        draft_summary.set(String::new());
        ai_state.set(AiSummaryState::Idle);
        user_edited.set(false);
    };

    let persistence_for_revise_cancel = persistence.clone();
    let note_id_for_revise_cancel = props.note_id.clone();
    let on_revise_cancel = move |_| {
        let prior = prior_body.read().clone();
        dialog_open.set(false);
        draft_summary.set(String::new());
        ai_state.set(AiSummaryState::Idle);
        user_edited.set(false);
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
                ai_state,
                user_edited,
                testid_prefix: dialog_testid,
                on_confirm: EventHandler::new(on_confirm),
                on_cancel: EventHandler::new(on_dialog_cancel),
                on_regenerate: EventHandler::new(on_regenerate),
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct ManualSummaryDialogProps {
    draft: Signal<String>,
    ai_state: Signal<AiSummaryState>,
    user_edited: Signal<bool>,
    testid_prefix: String,
    on_confirm: EventHandler<()>,
    on_cancel: EventHandler<()>,
    on_regenerate: EventHandler<()>,
}

/// Revision-summary dialog. Kicks off a one-shot `claude --print`
/// draft when opened, then lets the user edit / regenerate / replace
/// the suggestion before clicking Confirm. Reuses the artifact's
/// `operon-revision-dialog-*` CSS classes for visual consistency
/// across formats.
#[component]
fn ManualSummaryDialog(props: ManualSummaryDialogProps) -> Element {
    let mut draft = props.draft;
    let mut user_edited = props.user_edited;
    let ai_state = props.ai_state;
    let confirm_handler = props.on_confirm;
    let cancel_handler = props.on_cancel;
    let regen_handler = props.on_regenerate;
    let state = ai_state.read().clone();
    let is_loading = matches!(state, AiSummaryState::Loading);
    let error_msg = match &state {
        AiSummaryState::Error(e) => Some(e.clone()),
        _ => None,
    };
    let confirm_disabled = is_loading || draft.read().trim().is_empty();
    let regen_available = matches!(state, AiSummaryState::Ready | AiSummaryState::Error(_));
    let scrim_testid = format!("{}-scrim", props.testid_prefix);
    let textarea_testid = format!("{}-textarea", props.testid_prefix);
    let cancel_testid = format!("{}-cancel", props.testid_prefix);
    let confirm_testid = format!("{}-confirm", props.testid_prefix);
    let regen_testid = format!("{}-regenerate", props.testid_prefix);
    let loading_testid = format!("{}-loading", props.testid_prefix);
    let error_testid = format!("{}-error", props.testid_prefix);

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
                    "Claude drafts a one-line summary of your edits. Accept it, tweak it, or regenerate before saving as a `manual` row in the revision-history table."
                }
            }
            if is_loading {
                div {
                    class: "operon-revision-dialog-loading",
                    "data-testid": "{loading_testid}",
                    span { class: "operon-revision-dialog-loading-label",
                        "Drafting summary\u{2026}"
                    }
                }
            } else {
                if let Some(msg) = error_msg.as_ref() {
                    div {
                        class: "operon-revision-dialog-error",
                        "data-testid": "{error_testid}",
                        "Couldn't draft a summary: {msg}. Type one below."
                    }
                }
                textarea {
                    class: "operon-revision-dialog-textarea",
                    "data-testid": "{textarea_testid}",
                    rows: "3",
                    value: "{draft}",
                    placeholder: "Summary of revision\u{2026}",
                    oninput: move |e| {
                        user_edited.set(true);
                        draft.set(e.value());
                    },
                }
            }
            div { class: "operon-revision-dialog-actions",
                if regen_available {
                    button {
                        r#type: "button",
                        class: "operon-revision-dialog-cancel",
                        "data-testid": "{regen_testid}",
                        title: "Ask Claude for a fresh draft (overwrites the textarea).",
                        onclick: move |_| regen_handler.call(()),
                        "Regenerate"
                    }
                }
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

/// Flip `ai_state` to Loading, clear the textarea + edited flag, then
/// spawn the one-shot `claude --print` call. On success the draft is
/// written back into `draft_summary` unless the user has already
/// started typing (`user_edited == true`), so an in-flight call can't
/// stomp their manual entry. The task bails if `dialog_open` flipped
/// to false in the meantime — a dialog Cancel or Confirm both close
/// the surface and any late landing draft is discarded.
fn kick_off_draft(
    prior: String,
    current: String,
    cwd: PathBuf,
    mut ai_state: Signal<AiSummaryState>,
    mut draft_summary: Signal<String>,
    dialog_open: Signal<bool>,
    mut user_edited: Signal<bool>,
) {
    ai_state.set(AiSummaryState::Loading);
    draft_summary.set(String::new());
    user_edited.set(false);
    spawn(async move {
        let res = draft_summary_with_claude(prior, current, cwd).await;
        if !*dialog_open.read() {
            return;
        }
        match res {
            Ok(s) => {
                if !*user_edited.read() {
                    draft_summary.set(s);
                }
                ai_state.set(AiSummaryState::Ready);
            }
            Err(e) => {
                tracing::warn!(target: "operon::revise", "claude draft failed: {e}");
                ai_state.set(AiSummaryState::Error(e));
            }
        }
    });
}

/// Max lines of unified diff we hand to claude. The model only needs
/// the +/- shape to write one line; piling the whole file on top
/// blows the per-turn token budget on long notes for no extra signal.
const MAX_DIFF_LINES_FOR_SUMMARY: usize = 80;
/// Hard ceiling on the one-shot call. The textarea stays manually
/// editable the whole time so a slow claude doesn't strand the user;
/// the timeout just makes sure we eventually flip the loading spinner
/// to Error if the subprocess hangs.
const DRAFT_TIMEOUT: Duration = Duration::from_secs(30);

/// Spawn `claude --print "<prompt>"` once and return the first non-
/// empty trimmed line of its stdout. Errors carry a short, user-
/// facing reason — the dialog renders them verbatim in the error
/// banner above the manual textarea.
async fn draft_summary_with_claude(
    prior: String,
    current: String,
    cwd: PathBuf,
) -> Result<String, String> {
    if prior == current {
        return Err("no changes to summarize".into());
    }
    let diff_text = build_diff_for_prompt(&prior, &current);
    if diff_text.is_empty() {
        return Err("no textual change".into());
    }

    let prompt = format!(
        "You're summarizing a manual edit to a note for a `Revision history` log row.\n\
         Reply with EXACTLY one short imperative line (no markdown, no quotes, no \
         trailing period, target \u{2264}80 chars) describing what changed. Output \
         only that line \u{2014} no preamble, no explanation, no tool calls.\n\n\
         Diff:\n{diff_text}"
    );

    let bin = crate::shell::companion_chat::resolve_claude_bin();
    let mut cmd = tokio::process::Command::new(&bin);
    cmd.current_dir(&cwd)
        .arg("--print")
        // Summary is a pure text reply — no tools needed. Bypass keeps
        // the subprocess from waiting on permission prompts if the
        // model spuriously calls Read/Grep.
        .arg("--permission-mode")
        .arg("bypassPermissions")
        .arg(&prompt)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn {}: {e}", bin.display()))?;
    let mut stdout_pipe = child.stdout.take().ok_or("missing stdout pipe")?;
    let mut stderr_pipe = child.stderr.take().ok_or("missing stderr pipe")?;

    let wait = async move {
        let mut so = Vec::<u8>::new();
        let mut se = Vec::<u8>::new();
        let (_so_res, _se_res, status) = tokio::join!(
            stdout_pipe.read_to_end(&mut so),
            stderr_pipe.read_to_end(&mut se),
            child.wait(),
        );
        (so, se, status)
    };
    let (so, se, status) = match timeout(DRAFT_TIMEOUT, wait).await {
        Ok(v) => v,
        Err(_) => return Err(format!("claude timed out after {}s", DRAFT_TIMEOUT.as_secs())),
    };
    let exit = status.map_err(|e| format!("wait: {e}"))?;
    if !exit.success() {
        let stderr = String::from_utf8_lossy(&se);
        return Err(format!(
            "claude exited {} ({})",
            exit.code().unwrap_or(-1),
            stderr.trim()
        ));
    }
    let stdout_text = String::from_utf8_lossy(&so);
    stdout_text
        .lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .map(|l| l.to_string())
        .ok_or_else(|| "claude returned no text".into())
}

/// Build a `+`/`-` only diff (no context / equal lines) capped at
/// [`MAX_DIFF_LINES_FOR_SUMMARY`]. The summary model just needs the
/// shape of the change; full context bloats the prompt without
/// changing what it writes back.
fn build_diff_for_prompt(prior: &str, current: &str) -> String {
    let diff = TextDiff::from_lines(prior, current);
    let mut out = String::new();
    let mut emitted = 0usize;
    for change in diff.iter_all_changes() {
        if emitted >= MAX_DIFF_LINES_FOR_SUMMARY {
            out.push_str("(\u{2026} more changes elided \u{2026})\n");
            break;
        }
        let sign = match change.tag() {
            ChangeTag::Delete => '-',
            ChangeTag::Insert => '+',
            ChangeTag::Equal => continue,
        };
        out.push(sign);
        let v = change.value();
        out.push_str(v);
        if !v.ends_with('\n') {
            out.push('\n');
        }
        emitted += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_for_prompt_drops_equal_lines() {
        let prior = "hello\nworld\n";
        let current = "hello\nfriend\n";
        let d = build_diff_for_prompt(prior, current);
        assert!(d.contains("-world"), "expected delete line: {d}");
        assert!(d.contains("+friend"), "expected insert line: {d}");
        assert!(!d.contains(" hello"), "equal lines should be elided: {d}");
    }

    #[test]
    fn diff_for_prompt_caps_at_max_lines() {
        let prior = String::new();
        let mut current = String::new();
        // 200 inserted lines should be truncated to MAX + the elision marker.
        for i in 0..200 {
            current.push_str(&format!("line {i}\n"));
        }
        let d = build_diff_for_prompt(&prior, &current);
        let plus_lines = d.lines().filter(|l| l.starts_with('+')).count();
        assert!(plus_lines <= MAX_DIFF_LINES_FOR_SUMMARY);
        assert!(d.contains("more changes elided"), "expected truncation marker: {d}");
    }

    #[test]
    fn diff_for_prompt_empty_when_identical() {
        let s = "same\n";
        assert!(build_diff_for_prompt(s, s).is_empty());
    }
}
