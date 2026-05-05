use rusqlite::Connection;

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
    let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
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
        "users",
        "_schema_migrations",
    ];
    for t in &drops {
        tx.execute_batch(&format!("DROP TABLE IF EXISTS {};", t))?;
    }
    tx.commit()?;
    Ok(())
}
