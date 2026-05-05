use operon_store::test_support::fresh_store;

const EXPECTED_TABLES: &[&str] = &[
    "_schema_migrations",
    "users",
    "orgs",
    "departments",
    "teams",
    "projects",
    "notes",
    "memberships",
    "team_members",
    "team_projects",
    "invites",
    "sessions",
    "attachments",
    "local_user",
    "local_app_settings",
    "local_project",
];

fn list_tables(store: &operon_store::Store) -> Vec<String> {
    let conn = store.conn().unwrap();
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
        .unwrap();
    let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
    let mut out: Vec<String> = rows.map(|r| r.unwrap()).collect();
    out.sort();
    out
}

#[test]
fn applies_001_from_fresh_db() {
    let store = fresh_store().unwrap();
    let tables = list_tables(&store);
    for t in EXPECTED_TABLES {
        assert!(tables.iter().any(|x| x == t), "missing table {t}");
    }
}

#[test]
fn down_then_up_is_idempotent() {
    let store = fresh_store().unwrap();
    store.migrate_down_test_only().unwrap();
    let tables_after_down = list_tables(&store);
    assert!(
        tables_after_down.is_empty(),
        "down left tables: {tables_after_down:?}"
    );
    store.migrate().unwrap();
    let tables = list_tables(&store);
    for t in EXPECTED_TABLES {
        assert!(tables.iter().any(|x| x == t));
    }
}

#[test]
fn re_running_up_is_no_op() {
    let store = fresh_store().unwrap();
    store.migrate().unwrap();
    store.migrate().unwrap();
    let first: i64 = store
        .conn()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM _schema_migrations", [], |r| r.get(0))
        .unwrap();
    store.migrate().unwrap();
    let second: i64 = store
        .conn()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM _schema_migrations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(first, second);
}

#[test]
fn system_org_seed_is_present() {
    let store = fresh_store().unwrap();
    let count: i64 = store
        .conn()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM orgs WHERE id = '00000000-0000-0000-0000-000000000000'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}
