use crate::sql::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::StoreError;
use crate::ids::{OrgId, TeamId};
use crate::store::Store;
use crate::time::now_ms;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Team {
    pub id: TeamId,
    pub org_id: OrgId,
    pub name: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl Team {
    pub fn new(org_id: OrgId, name: impl Into<String>) -> Self {
        let now = now_ms();
        Self {
            id: TeamId::new(),
            org_id,
            name: name.into(),
            created_at_ms: now,
            updated_at_ms: now,
        }
    }
}

pub trait TeamRepository: Send + Sync {
    fn create(&self, t: &Team) -> Result<(), StoreError>;
    fn get(&self, id: &TeamId) -> Result<Option<Team>, StoreError>;
    fn update(&self, t: &Team) -> Result<(), StoreError>;
    fn delete(&self, id: &TeamId) -> Result<(), StoreError>;
    fn list_by_org(&self, org_id: &OrgId) -> Result<Vec<Team>, StoreError>;
}

pub struct SqliteTeamRepository {
    store: Store,
}

impl SqliteTeamRepository {
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

fn row_to_team(row: &crate::sql::Row<'_>) -> crate::sql::Result<Team> {
    let id = TeamId::from_str_strict(&row.get::<_, String>(0)?).map_err(invalid)?;
    let org_id = OrgId::from_str_strict(&row.get::<_, String>(1)?).map_err(invalid)?;
    Ok(Team {
        id,
        org_id,
        name: row.get(2)?,
        created_at_ms: row.get(3)?,
        updated_at_ms: row.get(4)?,
    })
}

impl TeamRepository for SqliteTeamRepository {
    fn create(&self, t: &Team) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "INSERT INTO teams (id, org_id, name, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                t.id.as_str(),
                t.org_id.as_str(),
                t.name,
                t.created_at_ms,
                t.updated_at_ms,
            ],
        )?;
        Ok(())
    }

    fn get(&self, id: &TeamId) -> Result<Option<Team>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, org_id, name, created_at_ms, updated_at_ms FROM teams WHERE id = ?1",
        )?;
        Ok(stmt
            .query_row(params![id.as_str()], row_to_team)
            .optional()?)
    }

    fn update(&self, t: &Team) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        let n = conn.execute(
            "UPDATE teams SET name = ?2, updated_at_ms = ?3 WHERE id = ?1",
            params![t.id.as_str(), t.name, t.updated_at_ms],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    fn delete(&self, id: &TeamId) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute("DELETE FROM teams WHERE id = ?1", params![id.as_str()])?;
        Ok(())
    }

    fn list_by_org(&self, org_id: &OrgId) -> Result<Vec<Team>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, org_id, name, created_at_ms, updated_at_ms
             FROM teams WHERE org_id = ?1 ORDER BY name",
        )?;
        let rows = stmt.query_map(params![org_id.as_str()], row_to_team)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}
