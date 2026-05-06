use crate::sql::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::StoreError;
use crate::ids::{MembershipId, TeamId, TeamMemberId};
use crate::store::Store;
use crate::time::now_ms;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamMember {
    pub id: TeamMemberId,
    pub membership_id: MembershipId,
    pub team_id: TeamId,
    pub created_at_ms: i64,
}

impl TeamMember {
    pub fn new(membership_id: MembershipId, team_id: TeamId) -> Self {
        Self {
            id: TeamMemberId::new(),
            membership_id,
            team_id,
            created_at_ms: now_ms(),
        }
    }
}

pub trait TeamMemberRepository: Send + Sync {
    fn create(&self, t: &TeamMember) -> Result<(), StoreError>;
    fn get(&self, id: &TeamMemberId) -> Result<Option<TeamMember>, StoreError>;
    fn delete(&self, id: &TeamMemberId) -> Result<(), StoreError>;
    fn list_by_team(&self, team: &TeamId) -> Result<Vec<TeamMember>, StoreError>;
    fn list_by_membership(&self, m: &MembershipId) -> Result<Vec<TeamMember>, StoreError>;
}

pub struct SqliteTeamMemberRepository {
    store: Store,
}

impl SqliteTeamMemberRepository {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

fn invalid(e: StoreError) -> crate::sql::Error {
    crate::sql::Error::FromSqlConversionFailure(
        0,
        crate::sql::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e.to_string(),
        )),
    )
}

fn row(row: &crate::sql::Row<'_>) -> crate::sql::Result<TeamMember> {
    Ok(TeamMember {
        id: TeamMemberId::from_str_strict(&row.get::<_, String>(0)?).map_err(invalid)?,
        membership_id: MembershipId::from_str_strict(&row.get::<_, String>(1)?).map_err(invalid)?,
        team_id: TeamId::from_str_strict(&row.get::<_, String>(2)?).map_err(invalid)?,
        created_at_ms: row.get(3)?,
    })
}

impl TeamMemberRepository for SqliteTeamMemberRepository {
    fn create(&self, t: &TeamMember) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "INSERT INTO team_members (id, membership_id, team_id, created_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                t.id.as_str(),
                t.membership_id.as_str(),
                t.team_id.as_str(),
                t.created_at_ms,
            ],
        )?;
        Ok(())
    }

    fn get(&self, id: &TeamMemberId) -> Result<Option<TeamMember>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, membership_id, team_id, created_at_ms FROM team_members WHERE id = ?1",
        )?;
        Ok(stmt.query_row(params![id.as_str()], row).optional()?)
    }

    fn delete(&self, id: &TeamMemberId) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "DELETE FROM team_members WHERE id = ?1",
            params![id.as_str()],
        )?;
        Ok(())
    }

    fn list_by_team(&self, team: &TeamId) -> Result<Vec<TeamMember>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, membership_id, team_id, created_at_ms FROM team_members WHERE team_id = ?1",
        )?;
        let rows = stmt.query_map(params![team.as_str()], row)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    fn list_by_membership(&self, m: &MembershipId) -> Result<Vec<TeamMember>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, membership_id, team_id, created_at_ms FROM team_members WHERE membership_id = ?1",
        )?;
        let rows = stmt.query_map(params![m.as_str()], row)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}
