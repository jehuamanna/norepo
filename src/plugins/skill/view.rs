//! Skill note view + editor: a markdown body with an optional YAML
//! frontmatter header, plus a ▶ Play toolbar that materializes the skill
//! to disk and pushes an invocation prompt into the active companion
//! chat session.
//!
//! **Revise/Cancel/Done flow.** Skills (like every other note) default
//! to View mode. The toolbar's `Revise` button flips the active tab to
//! Edit, snapshots the body as `prior_body`, and reveals a `Cancel`
//! button + a `Done` button. `Done` opens a confirm dialog with a
//! required single-line summary; on Confirm the revision row is
//! appended to the body via `revision_table::append_revision_row` with
//! `derived_from = "manual"`, persisted, and the tab flips back to
//! View. `Cancel` reverts buffer + disk to `prior_body` (no row
//! recorded) and flips back to View. The revision table lives in the
//! editable note body, but `materialize::to_claude_compat` strips it
//! before writing to `.claude/skills/<slug>.md` so it never reaches
//! the LLM prompt.

use dioxus::prelude::*;

use crate::plugins::markdown::MarkdownView;
use crate::plugins::revise_flow::RevisionFlowButtons;
use crate::plugins::skill::frontmatter;

#[derive(Props, Clone, PartialEq)]
pub struct SkillViewProps {
    pub note_id: String,
    pub content: String,
    /// `true` when used as the editor surface (renders the textarea
    /// alongside the rendered preview); `false` for read-only View mode.
    pub edit: bool,
}

#[component]
pub fn SkillView(props: SkillViewProps) -> Element {
    let (frontmatter_lines, body) = frontmatter::split(&props.content);
    let skill_name = frontmatter_lines
        .as_ref()
        .and_then(|lines| frontmatter::field(lines, "skill_name").map(str::to_string));
    let skill_version = frontmatter_lines
        .as_ref()
        .and_then(|lines| frontmatter::field(lines, "skill_version").map(str::to_string));
    let body_owned = body.to_string();

    let note_id = props.note_id.clone();
    let content_for_play = props.content.clone();
    rsx! {
        div { class: "operon-skill-surface",
            "data-testid": "skill-surface",
            SkillToolbar {
                note_id: note_id.clone(),
                content: content_for_play,
                skill_name: skill_name.clone(),
                skill_version: skill_version.clone(),
            }
            if let Some(name) = &skill_name {
                div { class: "operon-skill-meta",
                    span { class: "operon-skill-meta-label", "skill" }
                    span { class: "operon-skill-meta-value", "{name}" }
                    if let Some(v) = &skill_version {
                        span { class: "operon-skill-meta-label", "version" }
                        span { class: "operon-skill-meta-value", "{v}" }
                    }
                }
            }
            div { class: "operon-skill-body",
                MarkdownView { content: body_owned }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct SkillEditorProps {
    pub note_id: String,
    pub content: String,
    pub on_change: EventHandler<String>,
}

#[component]
pub fn SkillEditor(props: SkillEditorProps) -> Element {
    // The parent (`LocalNoteEditor` / its tab buffer) owns the authoritative
    // content. A local `use_signal` here would only initialize on first
    // mount, so switching between two open skills (same component instance,
    // different `note_id`/`content` props) would keep showing the first
    // skill's body. Bind the textarea to `props.content` directly and let
    // `on_change` push edits back to the parent.
    let on_change = props.on_change;
    let (frontmatter_lines, _body) = frontmatter::split(&props.content);
    let skill_name = frontmatter_lines
        .as_ref()
        .and_then(|lines| frontmatter::field(lines, "skill_name").map(str::to_string));
    let skill_version = frontmatter_lines
        .as_ref()
        .and_then(|lines| frontmatter::field(lines, "skill_version").map(str::to_string));
    let note_id = props.note_id.clone();
    let content_for_play = props.content.clone();
    let textarea_value = props.content.clone();
    rsx! {
        div { class: "operon-skill-surface operon-skill-surface-edit",
            "data-testid": "skill-editor",
            SkillToolbar {
                note_id: note_id.clone(),
                content: content_for_play,
                skill_name: skill_name.clone(),
                skill_version: skill_version.clone(),
            }
            textarea {
                class: "operon-skill-textarea",
                "data-testid": "skill-textarea",
                spellcheck: "false",
                value: "{textarea_value}",
                oninput: move |e| {
                    on_change.call(e.value());
                },
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct SkillToolbarProps {
    note_id: String,
    content: String,
    skill_name: Option<String>,
    skill_version: Option<String>,
}

/// Toolbar above a skill's body: hosts the Revise/Cancel/Done cluster
/// next to the skill's name+version meta. The historical `▶ Run skill`
/// button has been removed — skills are now invoked via the cascade
/// runner, the companion composer (`Use the skill named "<slug>"`),
/// or workflow nodes. `materialize::write_skill_to_repo` is still
/// available for callers that need to push the body to
/// `.claude/skills/<slug>.md` (the toolbar simply doesn't invoke it).
#[component]
fn SkillToolbar(props: SkillToolbarProps) -> Element {
    let _ = props.content; // skill body is consumed by the parent view, not here.
    let skill_name = props.skill_name.clone();
    let skill_version_display = props.skill_version.clone();
    rsx! {
        div { class: "operon-skill-toolbar",
            "data-testid": "skill-toolbar",
            RevisionFlowButtons {
                note_id: props.note_id.clone(),
                class_root: "operon-skill-revise".to_string(),
                testid_prefix: "skill-revise".to_string(),
            }
            if let Some(name) = skill_name.as_ref() {
                span { class: "operon-skill-toolbar-spacer" }
                span {
                    class: "operon-skill-toolbar-meta",
                    "data-testid": "skill-toolbar-name",
                    "{name}"
                }
            }
            if let Some(v) = skill_version_display.as_ref() {
                span { class: "operon-skill-toolbar-spacer" }
                span { class: "operon-skill-toolbar-meta", "v{v}" }
            }
        }
    }
}
