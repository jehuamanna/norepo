use crate::sql::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::StoreError;
use crate::ids::{DepartmentId, OrgId};
use crate::store::Store;
use crate::time::now_ms;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Department {
    pub id: DepartmentId,
    pub org_id: OrgId,
    pub name: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl Department {
    pub fn new(org_id: OrgId, name: impl Into<String>) -> Self {
        let now = now_ms();
        Self {
            id: DepartmentId::new(),
            org_id,
            name: name.into(),
            created_at_ms: now,
            updated_at_ms: now,
        }
    }
}

pub trait DepartmentRepository: Send + Sync {
    fn create(&self, d: &Department) -> Result<(), StoreError>;
    fn get(&self, id: &DepartmentId) -> Result<Option<Department>, StoreError>;
    fn update(&self, d: &Department) -> Result<(), StoreError>;
    fn delete(&self, id: &DepartmentId) -> Result<(), StoreError>;
    fn list_by_org(&self, org_id: &OrgId) -> Result<Vec<Department>, StoreError>;
}

pub struct SqliteDepartmentRepository {
    store: Store,
}

impl SqliteDepartmentRepository {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

fn row_to_department(row: &crate::sql::Row<'_>) -> crate::sql::Result<Department> {
    let id = DepartmentId::from_str_strict(&row.get::<_, String>(0)?).map_err(invalid)?;
    let org_id = OrgId::from_str_strict(&row.get::<_, String>(1)?).map_err(invalid)?;
    Ok(Department {
        id,
        org_id,
        name: row.get(2)?,
        created_at_ms: row.get(3)?,
        updated_at_ms: row.get(4)?,
    })
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

impl DepartmentRepository for SqliteDepartmentRepository {
    fn create(&self, d: &Department) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "INSERT INTO departments (id, org_id, name, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                d.id.as_str(),
                d.org_id.as_str(),
                d.name,
                d.created_at_ms,
                d.updated_at_ms,
            ],
        )?;
        Ok(())
    }

    fn get(&self, id: &DepartmentId) -> Result<Option<Department>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, org_id, name, created_at_ms, updated_at_ms FROM departments WHERE id = ?1",
        )?;
        Ok(stmt
            .query_row(params![id.as_str()], row_to_department)
            .optional()?)
    }

    fn update(&self, d: &Department) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        let n = conn.execute(
            "UPDATE departments SET name = ?2, updated_at_ms = ?3 WHERE id = ?1",
            params![d.id.as_str(), d.name, d.updated_at_ms],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    fn delete(&self, id: &DepartmentId) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "DELETE FROM departments WHERE id = ?1",
            params![id.as_str()],
        )?;
        Ok(())
    }

    fn list_by_org(&self, org_id: &OrgId) -> Result<Vec<Department>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, org_id, name, created_at_ms, updated_at_ms
             FROM departments WHERE org_id = ?1 ORDER BY name",
        )?;
        let rows = stmt.query_map(params![org_id.as_str()], row_to_department)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}
