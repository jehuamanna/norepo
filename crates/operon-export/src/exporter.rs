use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::path::Path;

use operon_store::repos::department::DepartmentRepository;
use operon_store::repos::membership::MembershipRepository;
use operon_store::repos::note::NoteRepository;
use operon_store::repos::org::OrgRepository;
use operon_store::repos::project::ProjectRepository;
use operon_store::repos::team::TeamRepository;
use operon_store::repos::team_member::TeamMemberRepository;
use operon_store::repos::team_project::TeamProjectRepository;
use operon_store::repos::user::UserRepository;
use operon_store::time::now_ms;
use operon_store::{OrgId, Store};
use serde::Serialize;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use crate::error::ExportError;
use crate::manifest::Manifest;

#[derive(Serialize)]
struct UserOut<'a> {
    id: &'a str,
    email: &'a str,
    display_name: Option<&'a str>,
    created_at_ms: i64,
    updated_at_ms: i64,
}

pub struct ExportContext<'a> {
    pub store: &'a Store,
    pub orgs: &'a dyn OrgRepository,
    pub departments: &'a dyn DepartmentRepository,
    pub teams: &'a dyn TeamRepository,
    pub projects: &'a dyn ProjectRepository,
    pub notes: &'a dyn NoteRepository,
    pub memberships: &'a dyn MembershipRepository,
    pub team_members: &'a dyn TeamMemberRepository,
    pub team_projects: &'a dyn TeamProjectRepository,
    pub users: &'a dyn UserRepository,
}

pub fn export_org(ctx: &ExportContext, org_id: &OrgId, dest: &Path) -> Result<(), ExportError> {
    let _ = ctx.store; // reserved for future txn use
    let org = ctx
        .orgs
        .get(org_id)?
        .ok_or_else(|| ExportError::Malformed("org not found".into()))?;

    let tmp = dest.with_extension("opnpkg.tmp");
    let file = File::create(&tmp)?;
    let mut zip = ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();

    // Track referenced user_ids to limit user export to relevant rows.
    let memberships = ctx.memberships.by_org(org_id)?;
    let mut user_ids: HashSet<String> = HashSet::new();
    for m in &memberships {
        user_ids.insert(m.user_id.to_string());
    }

    // entities/users.jsonl — referenced users only, no password_hash.
    zip.start_file("entities/users.jsonl", opts)?;
    let mut count = 0u32;
    for uid_str in &user_ids {
        let uid = operon_store::UserId::from_str_strict(uid_str)
            .map_err(|e| ExportError::Malformed(e.to_string()))?;
        if let Some(u) = ctx.users.get(&uid)? {
            let row = UserOut {
                id: uid_str,
                email: &u.email,
                display_name: u.display_name.as_deref(),
                created_at_ms: u.created_at_ms,
                updated_at_ms: u.updated_at_ms,
            };
            writeln!(zip, "{}", serde_json::to_string(&row)?)?;
            count += 1;
        }
    }
    counts.insert("users".into(), count);

    // entities/orgs.jsonl
    zip.start_file("entities/orgs.jsonl", opts)?;
    writeln!(zip, "{}", serde_json::to_string(&org)?)?;
    counts.insert("orgs".into(), 1);

    // departments / teams / projects
    let depts = ctx.departments.list_by_org(org_id)?;
    zip.start_file("entities/departments.jsonl", opts)?;
    for d in &depts {
        writeln!(zip, "{}", serde_json::to_string(d)?)?;
    }
    counts.insert("departments".into(), depts.len() as u32);

    let teams = ctx.teams.list_by_org(org_id)?;
    zip.start_file("entities/teams.jsonl", opts)?;
    for t in &teams {
        writeln!(zip, "{}", serde_json::to_string(t)?)?;
    }
    counts.insert("teams".into(), teams.len() as u32);

    let projects = ctx.projects.list_by_org(org_id)?;
    zip.start_file("entities/projects.jsonl", opts)?;
    for p in &projects {
        writeln!(zip, "{}", serde_json::to_string(p)?)?;
    }
    counts.insert("projects".into(), projects.len() as u32);

    // memberships
    zip.start_file("entities/memberships.jsonl", opts)?;
    for m in &memberships {
        writeln!(zip, "{}", serde_json::to_string(m)?)?;
    }
    counts.insert("memberships".into(), memberships.len() as u32);

    // team_members + team_projects
    zip.start_file("entities/team_members.jsonl", opts)?;
    let mut tm_count = 0u32;
    for t in &teams {
        for tm in ctx.team_members.list_by_team(&t.id)? {
            writeln!(zip, "{}", serde_json::to_string(&tm)?)?;
            tm_count += 1;
        }
    }
    counts.insert("team_members".into(), tm_count);

    zip.start_file("entities/team_projects.jsonl", opts)?;
    let mut tp_count = 0u32;
    for t in &teams {
        for tp in ctx.team_projects.list_by_team(&t.id)? {
            writeln!(zip, "{}", serde_json::to_string(&tp)?)?;
            tp_count += 1;
        }
    }
    counts.insert("team_projects".into(), tp_count);

    // notes (per project) + bodies/snapshots
    zip.start_file("entities/notes.jsonl", opts)?;
    let mut note_count = 0u32;
    let mut note_ids: Vec<(String, Option<String>, Option<Vec<u8>>)> = Vec::new();
    for p in &projects {
        for n in ctx.notes.list_by_project(&p.id)? {
            // Strip body/snapshot from the metadata jsonl row.
            #[derive(Serialize)]
            struct NoteMeta<'a> {
                id: &'a str,
                project_id: &'a str,
                parent_id: Option<&'a str>,
                title: &'a str,
                sibling_index: i64,
                #[serde(rename = "type")]
                kind: &'a str,
                created_at_ms: i64,
                updated_at_ms: i64,
            }
            let id_str = n.id.to_string();
            let project_str = n.project_id.to_string();
            let parent_str = n.parent_id.as_ref().map(|p| p.to_string());
            let meta = NoteMeta {
                id: &id_str,
                project_id: &project_str,
                parent_id: parent_str.as_deref(),
                title: &n.title,
                sibling_index: n.sibling_index,
                kind: &n.kind,
                created_at_ms: n.created_at_ms,
                updated_at_ms: n.updated_at_ms,
            };
            writeln!(zip, "{}", serde_json::to_string(&meta)?)?;
            note_ids.push((id_str.clone(), n.body_markdown.clone(), n.loro_snapshot.clone()));
            note_count += 1;
        }
    }
    counts.insert("notes".into(), note_count);

    // bodies/<note_id>.md and snapshots/<note_id>.loro
    for (id, body, snap) in &note_ids {
        if let Some(body) = body {
            zip.start_file(format!("bodies/{id}.md"), opts)?;
            zip.write_all(body.as_bytes())?;
        }
        if let Some(snap) = snap {
            zip.start_file(format!("snapshots/{id}.loro"), opts)?;
            zip.write_all(snap)?;
        }
    }

    // attachments — skipped for v1 (blobs not yet on disk)
    counts.insert("attachments".into(), 0);

    // manifest (last so counts are accurate)
    let manifest = Manifest {
        format_version: crate::FORMAT_VERSION,
        schema_version: crate::SCHEMA_VERSION,
        source_org_id: org.id.to_string(),
        source_org_name: org.name.clone(),
        source_flavour: org.flavour.as_str().to_string(),
        exported_at_ms: now_ms(),
        exporter_user_id: None,
        entity_counts: counts,
    };
    zip.start_file("manifest.json", opts)?;
    zip.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;

    zip.finish()?;
    std::fs::rename(&tmp, dest)?;
    Ok(())
}
