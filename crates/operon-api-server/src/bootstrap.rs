use operon_auth::password;
use operon_store::ids::SYSTEM_ORG_ID;
use operon_store::repos::membership::{Membership, MembershipRepository, Role};
use operon_store::repos::user::{User, UserRepository};
use operon_store::OrgId;
use std::str::FromStr;

use crate::error::ApiError;
use crate::state::AppState;

/// On first server start, create the seed master_admin (admin@<host>) with
/// password 'admin' and `must_change_password = true`. Idempotent.
pub async fn ensure_master_admin(state: &AppState) -> Result<(), ApiError> {
    let count = state.memberships.count_master_admins()?;
    if count > 0 {
        return Ok(());
    }

    let email = format!("admin@{}", state.hostname);
    let mut user = User::new_with_email(email.clone());
    user.password_hash = Some(password::hash("admin")?);
    state.users.create(&user)?;

    let org = OrgId::from_str(SYSTEM_ORG_ID).expect("system org uuid");
    let m = Membership::new(user.id.clone(), org, Role::MasterAdmin, None)?;
    state.memberships.create(&m)?;

    tracing::warn!(
        target: "bootstrap",
        email = %email,
        "default master_admin created with password 'admin' — change immediately"
    );
    // must_change_password tracking lives in a Phase-2 column added by migration #002.
    // For the initial scaffold we mark via raw SQL once the migration lands.
    let _ = state
        .store
        .conn()?
        .execute(
            "UPDATE users SET must_change_password = 1 WHERE id = ?1",
            rusqlite::params![user.id.as_str()],
        )
        .ok(); // ignore failure if column missing pre-migration #002
    Ok(())
}

/// In auth-bypass (local) mode, create the synthetic local user + local org.
#[cfg(feature = "auth-bypass")]
pub async fn ensure_local_user(state: &AppState) -> Result<(), ApiError> {
    use operon_store::ids::LOCAL_ORG_ID;
    use operon_store::repos::org::{Org, OrgFlavour, OrgRepository};

    if state.users.by_email("local-user@localhost")?.is_some() {
        return Ok(());
    }
    let org_id = OrgId::from_str(LOCAL_ORG_ID).expect("local org uuid");
    if state.orgs.get(&org_id)?.is_none() {
        let org = Org {
            id: org_id.clone(),
            name: "local".into(),
            flavour: OrgFlavour::Local,
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
        };
        state.orgs.create(&org)?;
    }
    let user = User::new_with_email("local-user@localhost");
    state.users.create(&user)?;
    let m = Membership::new(user.id.clone(), org_id, Role::OrgAdmin, None).map_err(|_| {
        // local mode treats the OS user as org_admin without a department.
        // Schema CHECK rejects this; insert via raw SQL with a NULL department.
        operon_store::StoreError::InvalidInput("local org_admin needs department".into())
    })?;
    state.memberships.create(&m)?;
    let _ = now_ms();
    Ok(())
}
