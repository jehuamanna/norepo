use crate::sql::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::StoreError;
use crate::ids::{OrgId, ProjectId};
use crate::sqlite::Store;
use crate::time::now_ms;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Project {
    pub id: ProjectId,
    pub org_id: OrgId,
    pub name: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl Project {
    pub fn new(org_id: OrgId, name: impl Into<String>) -> Self {
        let now = now_ms();
        Self {
            id: ProjectId::new(),
            org_id,
            name: name.into(),
            created_at_ms: now,
            updated_at_ms: now,
        }
    }
}

pub trait ProjectRepository: Send + Sync {
    fn create(&self, p: &Project) -> Result<(), StoreError>;
    fn get(&self, id: &ProjectId) -> Result<Option<Project>, StoreError>;
    fn update(&self, p: &Project) -> Result<(), StoreError>;
    fn delete(&self, id: &ProjectId) -> Result<(), StoreError>;
    fn list_by_org(&self, org_id: &OrgId) -> Result<Vec<Project>, StoreError>;
}

pub struct SqliteProjectRepository {
    store: Store,
}

impl SqliteProjectRepository {
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

fn row_to_project(row: &crate::sql::Row<'_>) -> crate::sql::Result<Project> {
    let id = ProjectId::from_str_strict(&row.get::<_, String>(0)?).map_err(invalid)?;
    let org_id = OrgId::from_str_strict(&row.get::<_, String>(1)?).map_err(invalid)?;
    Ok(Project {
        id,
        org_id,
        name: row.get(2)?,
        created_at_ms: row.get(3)?,
        updated_at_ms: row.get(4)?,
    })
}

impl ProjectRepository for SqliteProjectRepository {
    fn create(&self, p: &Project) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "INSERT INTO projects (id, org_id, name, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                p.id.as_str(),
                p.org_id.as_str(),
                p.name,
                p.created_at_ms,
                p.updated_at_ms,
            ],
        )?;
        Ok(())
    }

    fn get(&self, id: &ProjectId) -> Result<Option<Project>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, org_id, name, created_at_ms, updated_at_ms FROM projects WHERE id = ?1",
        )?;
        Ok(stmt
            .query_row(params![id.as_str()], row_to_project)
            .optional()?)
    }

    fn update(&self, p: &Project) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        let n = conn.execute(
            "UPDATE projects SET name = ?2, updated_at_ms = ?3 WHERE id = ?1",
            params![p.id.as_str(), p.name, p.updated_at_ms],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    fn delete(&self, id: &ProjectId) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute("DELETE FROM projects WHERE id = ?1", params![id.as_str()])?;
        Ok(())
    }

    fn list_by_org(&self, org_id: &OrgId) -> Result<Vec<Project>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, org_id, name, created_at_ms, updated_at_ms
             FROM projects WHERE org_id = ?1 ORDER BY name",
        )?;
        let rows = stmt.query_map(params![org_id.as_str()], row_to_project)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}
