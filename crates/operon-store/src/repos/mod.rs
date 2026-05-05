//! Repository traits + SQLite implementations, one module per aggregate.

pub mod attachment;
pub mod audit;
pub mod department;
pub mod invite;
pub mod membership;
pub mod note;
pub mod org;
pub mod project;
pub mod session;
pub mod team;
pub mod team_member;
pub mod team_project;
pub mod user;

pub use attachment::{Attachment, AttachmentRepository, SqliteAttachmentRepository};
pub use audit::{AuditEntry, AuditLogRepository, AuditOutcome, SqliteAuditLogRepository};
pub use department::{Department, DepartmentRepository, SqliteDepartmentRepository};
pub use invite::{Invite, InviteRepository, SqliteInviteRepository};
pub use membership::{Membership, MembershipRepository, Role, SqliteMembershipRepository};
pub use note::{Note, NoteRepository, SqliteNoteRepository};
pub use org::{Org, OrgFlavour, OrgRepository, SqliteOrgRepository};
pub use project::{Project, ProjectRepository, SqliteProjectRepository};
pub use session::{Session, SessionRepository, SqliteSessionRepository};
pub use team::{SqliteTeamRepository, Team, TeamRepository};
pub use team_member::{SqliteTeamMemberRepository, TeamMember, TeamMemberRepository};
pub use team_project::{SqliteTeamProjectRepository, TeamProject, TeamProjectRepository};
pub use user::{SqliteUserRepository, User, UserRepository};
