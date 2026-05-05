use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use operon_notes::NoteHub;
use operon_store::repos::department::{Department, DepartmentRepository};
use operon_store::repos::membership::{Membership, MembershipRepository};
use operon_store::repos::note::{Note, NoteRepository};
use operon_store::repos::org::OrgRepository;
use operon_store::repos::project::{Project, ProjectRepository};
use operon_store::repos::team::{Team, TeamRepository};
use operon_store::repos::team_member::{TeamMember, TeamMemberRepository};
use operon_store::repos::team_project::{TeamProject, TeamProjectRepository};
use operon_store::repos::user::{User, UserRepository};
use operon_store::OrgId;
use serde::{Deserialize, Serialize};
use zip::ZipArchive;

use crate::error::ExportError;
use crate::manifest::Manifest;

#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    pub allow_cross_org: bool,
    pub overwrite_local_markdown_on_collision: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ImportReport {
    pub created: BTreeMap<String, u32>,
    pub updated: BTreeMap<String, u32>,
    pub skipped: BTreeMap<String, u32>,
    pub conflicts: Vec<String>,
}

impl ImportReport {
    fn bump_created(&mut self, key: &str) {
        *self.created.entry(key.to_string()).or_insert(0) += 1;
    }
    fn bump_skipped(&mut self, key: &str) {
        *self.skipped.entry(key.to_string()).or_insert(0) += 1;
    }
    #[allow(dead_code)]
    fn bump_updated(&mut self, key: &str) {
        *self.updated.entry(key.to_string()).or_insert(0) += 1;
    }
}

pub struct ImportContext<'a> {
    pub orgs: &'a dyn OrgRepository,
    pub departments: &'a dyn DepartmentRepository,
    pub teams: &'a dyn TeamRepository,
    pub projects: &'a dyn ProjectRepository,
    pub notes: &'a dyn NoteRepository,
    pub memberships: &'a dyn MembershipRepository,
    pub team_members: &'a dyn TeamMemberRepository,
    pub team_projects: &'a dyn TeamProjectRepository,
    pub users: &'a dyn UserRepository,
    pub hub: Option<&'a NoteHub>,
}

#[derive(Deserialize)]
struct UserIn {
    id: String,
    email: String,
    display_name: Option<String>,
    created_at_ms: i64,
    updated_at_ms: i64,
}

#[derive(Deserialize)]
struct NoteMetaIn {
    id: String,
    project_id: String,
    parent_id: Option<String>,
    title: String,
    sibling_index: i64,
    #[serde(rename = "type")]
    kind: String,
    created_at_ms: i64,
    updated_at_ms: i64,
}

fn read_jsonl<T: for<'de> Deserialize<'de>, R: Read>(reader: R) -> Result<Vec<T>, ExportError> {
    use std::io::BufRead;
    let mut out = Vec::new();
    let buf = std::io::BufReader::new(reader);
    for line in buf.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        out.push(serde_json::from_str(&line)?);
    }
    Ok(out)
}

pub async fn import_archive(
    ctx: &ImportContext<'_>,
    archive_path: &Path,
    dest_org_id: &OrgId,
    opts: &ImportOptions,
) -> Result<ImportReport, ExportError> {
    let file = File::open(archive_path)?;
    let mut zip = ZipArchive::new(file)?;
    let mut report = ImportReport::default();

    // 1. Manifest
    let manifest: Manifest = {
        let mut f = zip.by_name("manifest.json")?;
        let mut s = String::new();
        f.read_to_string(&mut s)?;
        serde_json::from_str(&s)?
    };
    if manifest.format_version != crate::FORMAT_VERSION {
        return Err(ExportError::UnknownFormatVersion(manifest.format_version));
    }
    if manifest.schema_version > crate::SCHEMA_VERSION {
        return Err(ExportError::SchemaTooNew {
            archive: manifest.schema_version,
            current: crate::SCHEMA_VERSION,
        });
    }
    if &manifest.source_org_id != &dest_org_id.to_string() && !opts.allow_cross_org {
        return Err(ExportError::CrossOrgPayload);
    }

    // 2. Users (email-keyed reuse, no password overwrite)
    let users_in: Vec<UserIn> = {
        let f = zip.by_name("entities/users.jsonl").ok();
        match f {
            Some(f) => read_jsonl(f)?,
            None => vec![],
        }
    };
    let mut user_id_remap: BTreeMap<String, operon_store::UserId> = BTreeMap::new();
    for u in users_in {
        match ctx.users.by_email(&u.email)? {
            Some(existing) => {
                user_id_remap.insert(u.id.clone(), existing.id);
                report.bump_skipped("users");
            }
            None => {
                let new = User {
                    id: operon_store::UserId::from_str_strict(&u.id)
                        .map_err(|e| ExportError::Malformed(e.to_string()))?,
                    email: u.email,
                    display_name: u.display_name,
                    password_hash: None,
                    created_at_ms: u.created_at_ms,
                    updated_at_ms: u.updated_at_ms,
                };
                let new_id = new.id.clone();
                user_id_remap.insert(u.id, new_id.clone());
                if let Err(e) = ctx.users.create(&new) {
                    if !e.is_unique_violation() {
                        return Err(e.into());
                    }
                    report.bump_skipped("users");
                } else {
                    report.bump_created("users");
                }
            }
        }
    }

    // 3. Org row — we always rewrite to dest_org_id.
    // Department / team / project rows use their original ids.
    let _orgs_in: Vec<serde_json::Value> = read_jsonl(zip.by_name("entities/orgs.jsonl")?)?;

    // 4. Departments
    let depts_in: Vec<Department> = read_jsonl(zip.by_name("entities/departments.jsonl")?)?;
    for d in depts_in {
        let new = Department {
            org_id: dest_org_id.clone(),
            ..d
        };
        match ctx.departments.create(&new) {
            Ok(_) => report.bump_created("departments"),
            Err(e) if e.is_unique_violation() => report.bump_skipped("departments"),
            Err(e) => return Err(e.into()),
        }
    }

    // 5. Teams
    let teams_in: Vec<Team> = read_jsonl(zip.by_name("entities/teams.jsonl")?)?;
    for t in teams_in {
        let new = Team {
            org_id: dest_org_id.clone(),
            ..t
        };
        match ctx.teams.create(&new) {
            Ok(_) => report.bump_created("teams"),
            Err(e) if e.is_unique_violation() => report.bump_skipped("teams"),
            Err(e) => return Err(e.into()),
        }
    }

    // 6. Projects
    let projects_in: Vec<Project> = read_jsonl(zip.by_name("entities/projects.jsonl")?)?;
    for p in projects_in {
        let new = Project {
            org_id: dest_org_id.clone(),
            ..p
        };
        match ctx.projects.create(&new) {
            Ok(_) => report.bump_created("projects"),
            Err(e) if e.is_unique_violation() => report.bump_skipped("projects"),
            Err(e) => return Err(e.into()),
        }
    }

    // 7. Memberships (set-union by user_id+org_id)
    let memberships_in: Vec<Membership> = read_jsonl(zip.by_name("entities/memberships.jsonl")?)?;
    for m in memberships_in {
        let mapped_user = user_id_remap
            .get(&m.user_id.to_string())
            .cloned()
            .unwrap_or(m.user_id.clone());
        let new = Membership {
            user_id: mapped_user.clone(),
            org_id: dest_org_id.clone(),
            ..m
        };
        if ctx
            .memberships
            .by_user_org(&mapped_user, dest_org_id)?
            .is_some()
        {
            report.bump_skipped("memberships");
            continue;
        }
        match ctx.memberships.create(&new) {
            Ok(_) => report.bump_created("memberships"),
            Err(e) if e.is_unique_violation() => report.bump_skipped("memberships"),
            Err(e) => return Err(e.into()),
        }
    }

    // 8. team_members + team_projects (set-union)
    if let Ok(f) = zip.by_name("entities/team_members.jsonl") {
        let xs: Vec<TeamMember> = read_jsonl(f)?;
        for tm in xs {
            match ctx.team_members.create(&tm) {
                Ok(_) => report.bump_created("team_members"),
                Err(e) if e.is_unique_violation() => report.bump_skipped("team_members"),
                Err(e) => return Err(e.into()),
            }
        }
    }
    if let Ok(f) = zip.by_name("entities/team_projects.jsonl") {
        let xs: Vec<TeamProject> = read_jsonl(f)?;
        for tp in xs {
            match ctx.team_projects.create(&tp) {
                Ok(_) => report.bump_created("team_projects"),
                Err(e) if e.is_unique_violation() => report.bump_skipped("team_projects"),
                Err(e) => return Err(e.into()),
            }
        }
    }

    // 9. Notes (metadata + body/snapshot bodies)
    let notes_meta: Vec<NoteMetaIn> = read_jsonl(zip.by_name("entities/notes.jsonl")?)?;
    for meta in notes_meta {
        // Pull body if present
        let body_path = format!("bodies/{}.md", meta.id);
        let snap_path = format!("snapshots/{}.loro", meta.id);
        let mut body_markdown: Option<String> = None;
        if let Ok(mut f) = zip.by_name(&body_path) {
            let mut s = String::new();
            f.read_to_string(&mut s)?;
            body_markdown = Some(s);
        }
        let mut loro_snapshot: Option<Vec<u8>> = None;
        if let Ok(mut f) = zip.by_name(&snap_path) {
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            loro_snapshot = Some(buf);
        }
        let id = operon_store::NoteId::from_str_strict(&meta.id)
            .map_err(|e| ExportError::Malformed(e.to_string()))?;
        let project_id = operon_store::ProjectId::from_str_strict(&meta.project_id)
            .map_err(|e| ExportError::Malformed(e.to_string()))?;
        let parent_id = match meta.parent_id.as_deref() {
            Some(s) => Some(
                operon_store::NoteId::from_str_strict(s)
                    .map_err(|e| ExportError::Malformed(e.to_string()))?,
            ),
            None => None,
        };
        let note = Note {
            id: id.clone(),
            project_id,
            parent_id,
            title: meta.title,
            body_markdown: body_markdown.clone(),
            loro_snapshot: loro_snapshot.clone(),
            sibling_index: meta.sibling_index,
            kind: meta.kind,
            created_at_ms: meta.created_at_ms,
            updated_at_ms: meta.updated_at_ms,
        };
        match ctx.notes.create(&note) {
            Ok(_) => report.bump_created("notes"),
            Err(e) if e.is_unique_violation() => report.bump_skipped("notes"),
            Err(e) => return Err(e.into()),
        }
        // Live-session import broadcast.
        if let (Some(hub), Some(snap)) = (ctx.hub, loro_snapshot) {
            let _ = hub.import_into(&id, &snap).await;
        }
    }

    Ok(report)
}
