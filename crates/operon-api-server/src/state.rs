use std::sync::Arc;

use operon_auth::email::{EmailSender, LogEmailSender};
use operon_notes::NoteHub;

use crate::routes::export_import::ExportTokenStore;
use operon_store::repos::{
    SqliteAttachmentRepository, SqliteAuditLogRepository, SqliteDepartmentRepository,
    SqliteInviteRepository, SqliteMembershipRepository, SqliteNoteRepository,
    SqliteNoteUpdateRepository, SqliteOrgRepository, SqliteProjectRepository,
    SqliteSessionRepository, SqliteTeamMemberRepository, SqliteTeamProjectRepository,
    SqliteTeamRepository, SqliteUserRepository,
};
use operon_store::{Store, StoreConfig, StoreMode};

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub hostname: String,
    pub users: Arc<SqliteUserRepository>,
    pub orgs: Arc<SqliteOrgRepository>,
    pub departments: Arc<SqliteDepartmentRepository>,
    pub teams: Arc<SqliteTeamRepository>,
    pub projects: Arc<SqliteProjectRepository>,
    pub notes: Arc<SqliteNoteRepository>,
    pub memberships: Arc<SqliteMembershipRepository>,
    pub team_members: Arc<SqliteTeamMemberRepository>,
    pub team_projects: Arc<SqliteTeamProjectRepository>,
    pub sessions: Arc<SqliteSessionRepository>,
    pub invites: Arc<SqliteInviteRepository>,
    pub attachments: Arc<SqliteAttachmentRepository>,
    pub audit: Arc<SqliteAuditLogRepository>,
    pub note_updates: Arc<SqliteNoteUpdateRepository>,
    pub email: Arc<dyn EmailSender>,
    pub hub: Arc<NoteHub>,
    pub export_tokens: ExportTokenStore,
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
        let notes_arc: Arc<SqliteNoteRepository> =
            Arc::new(SqliteNoteRepository::new(store.clone()));
        let note_updates_arc: Arc<SqliteNoteUpdateRepository> =
            Arc::new(SqliteNoteUpdateRepository::new(store.clone()));
        let hub = Arc::new(NoteHub::new(notes_arc.clone(), note_updates_arc.clone()));
        Self {
            users: Arc::new(SqliteUserRepository::new(store.clone())),
            orgs: Arc::new(SqliteOrgRepository::new(store.clone())),
            departments: Arc::new(SqliteDepartmentRepository::new(store.clone())),
            teams: Arc::new(SqliteTeamRepository::new(store.clone())),
            projects: Arc::new(SqliteProjectRepository::new(store.clone())),
            notes: notes_arc,
            memberships: Arc::new(SqliteMembershipRepository::new(store.clone())),
            team_members: Arc::new(SqliteTeamMemberRepository::new(store.clone())),
            team_projects: Arc::new(SqliteTeamProjectRepository::new(store.clone())),
            sessions: Arc::new(SqliteSessionRepository::new(store.clone())),
            invites: Arc::new(SqliteInviteRepository::new(store.clone())),
            attachments: Arc::new(SqliteAttachmentRepository::new(store.clone())),
            audit: Arc::new(SqliteAuditLogRepository::new(store.clone())),
            note_updates: note_updates_arc,
            email,
            hub,
            export_tokens: ExportTokenStore::new(),
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
