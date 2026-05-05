use std::sync::Arc;

use operon_auth::email::{EmailSender, LogEmailSender};
use operon_store::repos::{
    SqliteInviteRepository, SqliteMembershipRepository, SqliteOrgRepository,
    SqliteSessionRepository, SqliteUserRepository,
};
use operon_store::{Store, StoreConfig, StoreMode};

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub hostname: String,
    pub users: Arc<SqliteUserRepository>,
    pub orgs: Arc<SqliteOrgRepository>,
    pub memberships: Arc<SqliteMembershipRepository>,
    pub sessions: Arc<SqliteSessionRepository>,
    pub invites: Arc<SqliteInviteRepository>,
    pub email: Arc<dyn EmailSender>,
}

impl AppState {
    pub async fn open(
        db_path: &str,
        hostname: impl Into<String>,
    ) -> Result<Self, operon_store::StoreError> {
        let cfg = if db_path == ":memory:" {
            StoreConfig::memory(StoreMode::NonLocal)
        } else {
            StoreConfig::non_local(db_path)
        };
        let store = Store::open(cfg)?;
        Ok(Self::from_store(store, hostname.into()))
    }

    pub fn from_store(store: Store, hostname: String) -> Self {
        let email: Arc<dyn EmailSender> = Arc::new(LogEmailSender);
        Self {
            users: Arc::new(SqliteUserRepository::new(store.clone())),
            orgs: Arc::new(SqliteOrgRepository::new(store.clone())),
            memberships: Arc::new(SqliteMembershipRepository::new(store.clone())),
            sessions: Arc::new(SqliteSessionRepository::new(store.clone())),
            invites: Arc::new(SqliteInviteRepository::new(store.clone())),
            email,
            hostname,
            store,
        }
    }

    pub fn for_test() -> Self {
        let store = Store::for_test().expect("test store opens");
        Self::from_store(store, "localhost".into())
    }

    pub fn with_email(mut self, email: Arc<dyn EmailSender>) -> Self {
        self.email = email;
        self
    }
}
