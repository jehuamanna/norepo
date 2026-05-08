//! Skill note view + editor: a markdown body with an optional YAML
//! frontmatter header, plus a ▶ Play toolbar that materializes the skill
//! to disk and pushes an invocation prompt into the active companion
//! chat session.

use dioxus::prelude::*;
use std::path::PathBuf;
use uuid::Uuid;

use crate::local_mode::desktop::{LocalNoteRepo, LocalProjectRepo};
use crate::local_mode::explorer::SelectedNote;
use crate::plugins::markdown::MarkdownView;
use crate::plugins::skill::frontmatter;
use crate::plugins::skill::materialize;
use crate::shell::companion_state::{
    ActiveChatScope, ActiveChatSession, ChatScope, ChatSessionRepo, ChatSessionVersion,
    CompanionComposerInbox,
};

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

#[component]
fn SkillToolbar(props: SkillToolbarProps) -> Element {
    let LocalProjectRepo(project_repo) = use_context();
    let LocalNoteRepo(note_repo) = use_context();
    let ChatSessionRepo(session_repo) = use_context();
    let SelectedNote(_selected_note) = use_context();
    let ActiveChatSession(mut active_session) = use_context();
    let ActiveChatScope(mut active_scope) = use_context();
    let ChatSessionVersion(mut session_version) = use_context();
    let CompanionComposerInbox(mut composer_inbox) = use_context();

    let last_status: Signal<Option<Result<String, String>>> = use_signal(|| None);
    let mut status_setter = last_status;

    let note_id_str = props.note_id.clone();
    let content_for_play = props.content.clone();
    let skill_name = props.skill_name.clone();
    let skill_version_display = props.skill_version.clone();
    let on_play = move |_| {
        play_skill(
            &note_id_str,
            &content_for_play,
            skill_name.as_deref(),
            &project_repo,
            &note_repo,
            &session_repo,
            &mut active_session,
            &mut active_scope,
            &mut session_version,
            &mut composer_inbox,
            &mut status_setter,
        );
    };

    rsx! {
        div { class: "operon-skill-toolbar",
            "data-testid": "skill-toolbar",
            button {
                r#type: "button",
                class: "operon-skill-play",
                "data-testid": "skill-play",
                title: "Run this skill in the project's Companion session",
                onclick: on_play,
                "\u{25B6} Run skill"
            }
            if let Some(v) = skill_version_display.as_ref() {
                span { class: "operon-skill-toolbar-spacer" }
                span { class: "operon-skill-toolbar-meta", "v{v}" }
            }
            if let Some(status) = last_status.read().as_ref() {
                span { class: "operon-skill-toolbar-spacer" }
                match status {
                    Ok(msg) => rsx! {
                        span {
                            class: "operon-skill-toolbar-status operon-skill-toolbar-status-ok",
                            "data-testid": "skill-status-ok",
                            "{msg}"
                        }
                    },
                    Err(msg) => rsx! {
                        span {
                            class: "operon-skill-toolbar-status operon-skill-toolbar-status-error",
                            "data-testid": "skill-status-error",
                            "{msg}"
                        }
                    },
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn play_skill(
    note_id_str: &str,
    content: &str,
    declared_skill_name: Option<&str>,
    project_repo: &std::sync::Arc<dyn operon_store::repos::LocalProjectRepository>,
    note_repo: &std::sync::Arc<dyn operon_store::repos::LocalNoteRepository>,
    session_repo: &std::sync::Arc<dyn operon_store::repos::ChatSessionRepository>,
    active_session: &mut Signal<Option<Uuid>>,
    active_scope: &mut Signal<ChatScope>,
    session_version: &mut Signal<u64>,
    composer_inbox: &mut Signal<Option<String>>,
    status: &mut Signal<Option<Result<String, String>>>,
) {
    // 1. Resolve the skill note's project.
    let note_uuid = match Uuid::parse_str(note_id_str) {
        Ok(u) => u,
        Err(_) => {
            status.set(Some(Err("invalid note id".into())));
            return;
        }
    };
    let project_id = match note_repo.find_project_for_note(note_uuid) {
        Ok(Some(pid)) => pid,
        Ok(None) => {
            status.set(Some(Err("skill note has no project".into())));
            return;
        }
        Err(e) => {
            status.set(Some(Err(format!("lookup project: {e}"))));
            return;
        }
    };

    // 2. Resolve the project's repo_path.
    let projects = match project_repo.list() {
        Ok(rows) => rows,
        Err(e) => {
            status.set(Some(Err(format!("list projects: {e}"))));
            return;
        }
    };
    let project = match projects.into_iter().find(|p| p.id == project_id) {
        Some(p) => p,
        None => {
            status.set(Some(Err("project no longer exists".into())));
            return;
        }
    };
    let repo_path: PathBuf = match project.repo_path {
        Some(p) => p,
        None => {
            status.set(Some(Err(
                "set the project's repository (right-click → Set repository\u{2026}) before running a skill".into(),
            )));
            return;
        }
    };

    // 3. Materialize the skill body to <repo>/.claude/skills/<slug>.md.
    let slug = declared_skill_name
        .map(frontmatter::slugify)
        .unwrap_or_else(|| frontmatter::slugify(note_id_str));
    if let Err(e) = materialize::write_skill_to_repo(&repo_path, &slug, content) {
        status.set(Some(Err(format!("materialize: {e}"))));
        return;
    }

    // 4. Switch the rail to this project's scope and create a fresh
    //    session named "Run: <slug>". The companion will pick it up via
    //    the ActiveChatSession signal.
    let scope = ChatScope::Project(project_id);
    let new_session = match session_repo.create(scope, &format!("Run: {slug}")) {
        Ok(s) => s,
        Err(e) => {
            status.set(Some(Err(format!("create session: {e}"))));
            return;
        }
    };
    active_scope.set(scope);
    active_session.set(Some(new_session.id));

    // 5. Pre-fill the companion composer with the invocation prompt
    //    AND tell claude to capture the run's output to disk so future
    //    workflows can chain to this skill's output without scraping
    //    the chat transcript. Path is unified with workflow runs:
    //    `<repo>/.operon/outputs/<slug>-output.md`. Re-running the same
    //    skill (Play button OR a workflow node referencing it)
    //    overwrites the same file.
    let outputs_dir = repo_path.join(".operon").join("outputs");
    let _ = std::fs::create_dir_all(&outputs_dir);
    let output_path = outputs_dir.join(format!("{slug}-output.md"));
    let prompt = format!(
        "Use the skill named \"{slug}\".\n\nWrite your output (markdown body, optionally with YAML frontmatter) to the absolute path: {}",
        output_path.display()
    );
    composer_inbox.set(Some(prompt));
    session_version.with_mut(|v| *v += 1);
    status.set(Some(Ok(format!(
        "Materialized {slug}.md \u{2192} session ready \u{2022} output will land at {}",
        output_path.display()
    ))));
}
