use crate::sql::Connection;

use crate::error::StoreError;

const MIGRATIONS: &[(i64, &str, &str)] = &[
    (
        1,
        "001_initial",
        include_str!("../migrations/001_initial.sql"),
    ),
    (
        2,
        "002_users_password_flags",
        include_str!("../migrations/002_users_password_flags.sql"),
    ),
    (
        3,
        "003_audit_log",
        include_str!("../migrations/003_audit_log.sql"),
    ),
    (
        4,
        "004_note_updates",
        include_str!("../migrations/004_note_updates.sql"),
    ),
    (
        5,
        "005_local_mode",
        include_str!("../migrations/005_local_mode.sql"),
    ),
    (
        6,
        "006_local_projects",
        include_str!("../migrations/006_local_projects.sql"),
    ),
    (
        7,
        "007_local_notes",
        include_str!("../migrations/007_local_notes.sql"),
    ),
    (
        8,
        "008_local_note_kind",
        include_str!("../migrations/008_local_note_kind.sql"),
    ),
    (
        9,
        "009_local_note_blob_path",
        include_str!("../migrations/009_local_note_blob_path.sql"),
    ),
    (
        10,
        "010_local_note_link",
        include_str!("../migrations/010_local_note_link.sql"),
    ),
    (
        11,
        "011_local_note_kind_extend",
        include_str!("../migrations/011_local_note_kind_extend.sql"),
    ),
    (
        12,
        "012_local_project_repo_path",
        include_str!("../migrations/012_local_project_repo_path.sql"),
    ),
    (
        13,
        "013_chat_sessions",
        include_str!("../migrations/013_chat_sessions.sql"),
    ),
    (
        14,
        "014_chat_messages",
        include_str!("../migrations/014_chat_messages.sql"),
    ),
    (
        15,
        "015_local_note_kind_skill_workflow",
        include_str!("../migrations/015_local_note_kind_skill_workflow.sql"),
    ),
    (
        16,
        "016_local_note_kind_artifact",
        include_str!("../migrations/016_local_note_kind_artifact.sql"),
    ),
    (
        17,
        "017_chat_session_model_and_permission",
        include_str!("../migrations/017_chat_session_model_and_permission.sql"),
    ),
    (
        18,
        "018_local_note_slug",
        include_str!("../migrations/018_local_note_slug.sql"),
    ),
    (
        19,
        "019_local_project_claude_defaults",
        include_str!("../migrations/019_local_project_claude_defaults.sql"),
    ),
    (
        20,
        "020_local_note_kind_phase",
        include_str!("../migrations/020_local_note_kind_phase.sql"),
    ),
];

fn ensure_migrations_table(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at_ms INTEGER NOT NULL
        );",
    )?;
    Ok(())
}

fn applied_versions(conn: &Connection) -> Result<Vec<i64>, StoreError> {
    let mut stmt = conn.prepare("SELECT version FROM _schema_migrations ORDER BY version")?;
    let rows = stmt.query_map(crate::sql::params![], |row| row.get::<_, i64>(0))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Apply any migrations whose version is not yet recorded.
pub fn migrate_up(conn: &mut Connection) -> Result<(), StoreError> {
    ensure_migrations_table(conn)?;
    let applied = applied_versions(conn)?;
    let known: Vec<i64> = MIGRATIONS.iter().map(|(v, _, _)| *v).collect();
    for v in &applied {
        if !known.contains(v) {
            return Err(StoreError::UnknownAppliedVersion(*v));
        }
    }
    for (version, _name, sql) in MIGRATIONS {
        if applied.contains(version) {
            continue;
        }
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.commit()?;
    }
    Ok(())
}

/// Drop every table this crate created. Test-only and irreversible.
#[doc(hidden)]
pub fn migrate_down_all(conn: &mut Connection) -> Result<(), StoreError> {
    let tx = conn.transaction()?;
    // Drop in reverse FK order.
    let drops = [
        "note_updates",
        "audit_log",
        "attachments",
        "sessions",
        "invites",
        "team_projects",
        "team_members",
        "memberships",
        "notes",
        "projects",
        "teams",
        "departments",
        "orgs",
        "chat_message",
        "chat_session",
        "local_app_settings",
        "local_tree_state",
        "local_note_link",
        "local_note",
        "local_project",
        "local_user",
        "users",
        "_schema_migrations",
    ];
    for t in &drops {
        tx.execute_batch(&format!("DROP TABLE IF EXISTS {};", t))?;
    }
    tx.commit()?;
    Ok(())
}
