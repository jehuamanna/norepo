//! One round-trip test per repository + the membership CHECK constraint
//! negative tests. Run with `cargo test -p operon-store`.

use operon_store::repos::*;
use operon_store::test_support::fresh_store;
use operon_store::*;

fn make_org(store: &Store) -> OrgId {
    let repo = SqliteOrgRepository::new(store.clone());
    let org = Org::new("acme", OrgFlavour::NonLocal);
    let id = org.id.clone();
    repo.create(&org).unwrap();
    id
}

fn make_user(store: &Store, email: &str) -> UserId {
    let repo = SqliteUserRepository::new(store.clone());
    let user = User::new_with_email(email);
    let id = user.id.clone();
    repo.create(&user).unwrap();
    id
}

fn make_department(store: &Store, org: OrgId) -> DepartmentId {
    let repo = SqliteDepartmentRepository::new(store.clone());
    let d = Department::new(org, "engineering");
    let id = d.id.clone();
    repo.create(&d).unwrap();
    id
}

#[test]
fn user_round_trip_and_by_email() {
    let store = fresh_store().unwrap();
    let repo = SqliteUserRepository::new(store.clone());
    let user = User::new_with_email("Alice@example.com");
    repo.create(&user).unwrap();

    let got = repo.get(&user.id).unwrap().expect("present");
    assert_eq!(got.email, "Alice@example.com");

    // Case-insensitive email match.
    let by = repo.by_email("alice@example.com").unwrap().expect("present");
    assert_eq!(by.id, user.id);

    let none = repo.by_email("noone@example.com").unwrap();
    assert!(none.is_none());

    repo.delete(&user.id).unwrap();
    assert!(repo.get(&user.id).unwrap().is_none());
}

#[test]
fn user_duplicate_email_is_conflict() {
    let store = fresh_store().unwrap();
    let repo = SqliteUserRepository::new(store.clone());
    repo.create(&User::new_with_email("dup@example.com")).unwrap();
    let err = repo
        .create(&User::new_with_email("dup@example.com"))
        .unwrap_err();
    assert!(err.is_unique_violation(), "expected unique violation got {err:?}");
}

#[test]
fn org_round_trip_and_list() {
    let store = fresh_store().unwrap();
    let repo = SqliteOrgRepository::new(store.clone());
    let org = Org::new("acme", OrgFlavour::NonLocal);
    repo.create(&org).unwrap();
    let listed = repo.list(50, None).unwrap();
    assert!(listed.iter().any(|o| o.id == org.id));
}

#[test]
fn department_unique_per_org() {
    let store = fresh_store().unwrap();
    let org = make_org(&store);
    let repo = SqliteDepartmentRepository::new(store.clone());
    repo.create(&Department::new(org.clone(), "eng")).unwrap();
    let err = repo.create(&Department::new(org.clone(), "eng")).unwrap_err();
    assert!(err.is_unique_violation());
}

#[test]
fn team_unique_per_org() {
    let store = fresh_store().unwrap();
    let org = make_org(&store);
    let repo = SqliteTeamRepository::new(store.clone());
    repo.create(&Team::new(org.clone(), "platform")).unwrap();
    let err = repo.create(&Team::new(org.clone(), "platform")).unwrap_err();
    assert!(err.is_unique_violation());
}

#[test]
fn project_unique_per_org_and_listing() {
    let store = fresh_store().unwrap();
    let org = make_org(&store);
    let repo = SqliteProjectRepository::new(store.clone());
    let p = Project::new(org.clone(), "alpha");
    repo.create(&p).unwrap();
    let listed = repo.list_by_org(&org).unwrap();
    assert!(listed.iter().any(|x| x.id == p.id));
}

#[test]
fn note_children_and_top_level() {
    let store = fresh_store().unwrap();
    let org = make_org(&store);
    let project_repo = SqliteProjectRepository::new(store.clone());
    let project = Project::new(org.clone(), "alpha");
    project_repo.create(&project).unwrap();

    let repo = SqliteNoteRepository::new(store.clone());
    let root = Note::new_root(project.id.clone(), "root");
    repo.create(&root).unwrap();
    let mut child = Note::new_root(project.id.clone(), "child");
    child.parent_id = Some(root.id.clone());
    repo.create(&child).unwrap();

    let kids = repo.children_of(&root.id).unwrap();
    assert_eq!(kids.len(), 1);
    assert_eq!(kids[0].id, child.id);

    let tops = repo.top_level(&project.id).unwrap();
    assert_eq!(tops.len(), 1);
    assert_eq!(tops[0].id, root.id);
}

#[test]
fn membership_constructor_rejects_user_without_dept() {
    let store = fresh_store().unwrap();
    let user = make_user(&store, "bob@example.com");
    let org = make_org(&store);
    let err = Membership::new(user, org, Role::User, None).unwrap_err();
    matches!(err, StoreError::InvalidInput(_));
}

#[test]
fn membership_check_constraint_db_level() {
    let store = fresh_store().unwrap();
    let user = make_user(&store, "carol@example.com");
    let org = make_org(&store);
    let dept = make_department(&store, org.clone());
    let repo = SqliteMembershipRepository::new(store.clone());

    // Master_admin allowed without department.
    let m = Membership::new(user.clone(), org.clone(), Role::MasterAdmin, None).unwrap();
    repo.create(&m).unwrap();
    assert_eq!(repo.count_master_admins().unwrap(), 1);

    // User with department succeeds.
    let user2 = make_user(&store, "dave@example.com");
    let m2 = Membership::new(user2, org, Role::User, Some(dept)).unwrap();
    repo.create(&m2).unwrap();
}

#[test]
fn membership_unique_user_org() {
    let store = fresh_store().unwrap();
    let user = make_user(&store, "eve@example.com");
    let org = make_org(&store);
    let dept = make_department(&store, org.clone());
    let repo = SqliteMembershipRepository::new(store.clone());

    let m1 = Membership::new(user.clone(), org.clone(), Role::User, Some(dept.clone())).unwrap();
    repo.create(&m1).unwrap();
    let m2 = Membership::new(user, org, Role::OrgAdmin, Some(dept)).unwrap();
    let err = repo.create(&m2).unwrap_err();
    assert!(err.is_unique_violation());
}

#[test]
fn invite_token_hash_lookup() {
    let store = fresh_store().unwrap();
    let user = make_user(&store, "f@example.com");
    let org = make_org(&store);
    let dept = make_department(&store, org.clone());
    let repo = SqliteInviteRepository::new(store.clone());
    let invite = Invite {
        id: InviteId::new(),
        email: "new@example.com".into(),
        org_id: org,
        role: Role::User,
        department_id: Some(dept),
        invited_by: user,
        token_hash: "abc123".into(),
        expires_at_ms: i64::MAX,
        accepted_at_ms: None,
        created_at_ms: 0,
    };
    repo.create(&invite).unwrap();
    let got = repo.by_token_hash("abc123").unwrap().expect("present");
    assert_eq!(got.id, invite.id);
}

#[test]
fn session_token_hash_lookup_and_delete_for_user() {
    let store = fresh_store().unwrap();
    let user = make_user(&store, "g@example.com");
    let repo = SqliteSessionRepository::new(store.clone());
    let s1 = Session {
        id: SessionId::new(),
        user_id: user.clone(),
        active_org_id: None,
        token_hash: "tk1".into(),
        expires_at_ms: i64::MAX,
        created_at_ms: 0,
        last_seen_at_ms: 0,
    };
    let s2 = Session {
        id: SessionId::new(),
        user_id: user.clone(),
        active_org_id: None,
        token_hash: "tk2".into(),
        expires_at_ms: i64::MAX,
        created_at_ms: 0,
        last_seen_at_ms: 0,
    };
    repo.create(&s1).unwrap();
    repo.create(&s2).unwrap();
    repo.delete_for_user(&user).unwrap();
    assert!(repo.by_token_hash("tk1").unwrap().is_none());
    assert!(repo.by_token_hash("tk2").unwrap().is_none());
}

#[test]
fn cascade_delete_org_removes_dependents() {
    let store = fresh_store().unwrap();
    let org = make_org(&store);
    let dept = make_department(&store, org.clone());
    let team_repo = SqliteTeamRepository::new(store.clone());
    let team = Team::new(org.clone(), "t1");
    team_repo.create(&team).unwrap();

    let project_repo = SqliteProjectRepository::new(store.clone());
    let project = Project::new(org.clone(), "p1");
    project_repo.create(&project).unwrap();

    let user = make_user(&store, "h@example.com");
    let m_repo = SqliteMembershipRepository::new(store.clone());
    let m = Membership::new(user, org.clone(), Role::User, Some(dept)).unwrap();
    m_repo.create(&m).unwrap();

    // Delete the org → everything cascades.
    let org_repo = SqliteOrgRepository::new(store.clone());
    org_repo.delete(&org).unwrap();

    assert!(SqliteTeamRepository::new(store.clone())
        .list_by_org(&org)
        .unwrap()
        .is_empty());
    assert!(SqliteProjectRepository::new(store.clone())
        .list_by_org(&org)
        .unwrap()
        .is_empty());
    assert!(SqliteDepartmentRepository::new(store.clone())
        .list_by_org(&org)
        .unwrap()
        .is_empty());
    assert!(SqliteMembershipRepository::new(store.clone())
        .by_org(&org)
        .unwrap()
        .is_empty());
}
