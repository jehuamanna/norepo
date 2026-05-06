use crate::sql::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::StoreError;
use crate::ids::{ProjectId, TeamId, TeamProjectId};
use crate::store::Store;
use crate::time::now_ms;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamProject {
    pub id: TeamProjectId,
    pub team_id: TeamId,
    pub project_id: ProjectId,
    pub created_at_ms: i64,
}

impl TeamProject {
    pub fn new(team_id: TeamId, project_id: ProjectId) -> Self {
        Self {
            id: TeamProjectId::new(),
            team_id,
            project_id,
            created_at_ms: now_ms(),
        }
    }
}

pub trait TeamProjectRepository: Send + Sync {
    fn create(&self, t: &TeamProject) -> Result<(), StoreError>;
    fn get(&self, id: &TeamProjectId) -> Result<Option<TeamProject>, StoreError>;
    fn delete(&self, id: &TeamProjectId) -> Result<(), StoreError>;
    fn list_by_team(&self, team: &TeamId) -> Result<Vec<TeamProject>, StoreError>;
    fn list_by_project(&self, project: &ProjectId) -> Result<Vec<TeamProject>, StoreError>;
}

pub struct SqliteTeamProjectRepository {
    store: Store,
}

impl SqliteTeamProjectRepository {
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

fn row(row: &crate::sql::Row<'_>) -> crate::sql::Result<TeamProject> {
    Ok(TeamProject {
        id: TeamProjectId::from_str_strict(&row.get::<_, String>(0)?).map_err(invalid)?,
        team_id: TeamId::from_str_strict(&row.get::<_, String>(1)?).map_err(invalid)?,
        project_id: ProjectId::from_str_strict(&row.get::<_, String>(2)?).map_err(invalid)?,
        created_at_ms: row.get(3)?,
    })
}

impl TeamProjectRepository for SqliteTeamProjectRepository {
    fn create(&self, t: &TeamProject) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "INSERT INTO team_projects (id, team_id, project_id, created_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                t.id.as_str(),
                t.team_id.as_str(),
                t.project_id.as_str(),
                t.created_at_ms,
            ],
        )?;
        Ok(())
    }

    fn get(&self, id: &TeamProjectId) -> Result<Option<TeamProject>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, team_id, project_id, created_at_ms FROM team_projects WHERE id = ?1",
        )?;
        Ok(stmt.query_row(params![id.as_str()], row).optional()?)
    }

    fn delete(&self, id: &TeamProjectId) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "DELETE FROM team_projects WHERE id = ?1",
            params![id.as_str()],
        )?;
        Ok(())
    }

    fn list_by_team(&self, team: &TeamId) -> Result<Vec<TeamProject>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, team_id, project_id, created_at_ms FROM team_projects WHERE team_id = ?1",
        )?;
        let rows = stmt.query_map(params![team.as_str()], row)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    fn list_by_project(&self, project: &ProjectId) -> Result<Vec<TeamProject>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, team_id, project_id, created_at_ms FROM team_projects WHERE project_id = ?1",
        )?;
        let rows = stmt.query_map(params![project.as_str()], row)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}
