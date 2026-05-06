use crate::sql::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::StoreError;
use crate::ids::OrgId;
use crate::store::Store;
use crate::time::now_ms;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrgFlavour {
    Local,
    NonLocal,
    System,
}

impl OrgFlavour {
    pub fn as_str(&self) -> &'static str {
        match self {
            OrgFlavour::Local => "local",
            OrgFlavour::NonLocal => "non_local",
            OrgFlavour::System => "system",
        }
    }

    pub fn from_str(s: &str) -> Result<Self, StoreError> {
        match s {
            "local" => Ok(OrgFlavour::Local),
            "non_local" => Ok(OrgFlavour::NonLocal),
            "system" => Ok(OrgFlavour::System),
            other => Err(StoreError::InvalidInput(format!(
                "unknown org flavour {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Org {
    pub id: OrgId,
    pub name: String,
    pub flavour: OrgFlavour,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl Org {
    pub fn new(name: impl Into<String>, flavour: OrgFlavour) -> Self {
        let now = now_ms();
        Self {
            id: OrgId::new(),
            name: name.into(),
            flavour,
            created_at_ms: now,
            updated_at_ms: now,
        }
    }
}

pub trait OrgRepository: Send + Sync {
    fn create(&self, org: &Org) -> Result<(), StoreError>;
    fn get(&self, id: &OrgId) -> Result<Option<Org>, StoreError>;
    fn update(&self, org: &Org) -> Result<(), StoreError>;
    fn delete(&self, id: &OrgId) -> Result<(), StoreError>;
    fn list(&self, limit: usize, after: Option<OrgId>) -> Result<Vec<Org>, StoreError>;
}

pub struct SqliteOrgRepository {
    store: Store,
}

impl SqliteOrgRepository {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

fn row_to_org(row: &crate::sql::Row<'_>) -> crate::sql::Result<Org> {
    let id_str: String = row.get(0)?;
    let id = OrgId::from_str_strict(&id_str).map_err(|e| {
        crate::sql::Error::FromSqlConversionFailure(
            0,
            crate::sql::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                e.to_string(),
            )),
        )
    })?;
    let flavour_str: String = row.get(2)?;
    let flavour = OrgFlavour::from_str(&flavour_str).map_err(|e| {
        crate::sql::Error::FromSqlConversionFailure(
            2,
            crate::sql::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                e.to_string(),
            )),
        )
    })?;
    Ok(Org {
        id,
        name: row.get(1)?,
        flavour,
        created_at_ms: row.get(3)?,
        updated_at_ms: row.get(4)?,
    })
}

impl OrgRepository for SqliteOrgRepository {
    fn create(&self, org: &Org) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "INSERT INTO orgs (id, name, flavour, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                org.id.as_str(),
                org.name,
                org.flavour.as_str(),
                org.created_at_ms,
                org.updated_at_ms,
            ],
        )?;
        Ok(())
    }

    fn get(&self, id: &OrgId) -> Result<Option<Org>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, flavour, created_at_ms, updated_at_ms FROM orgs WHERE id = ?1",
        )?;
        Ok(stmt.query_row(params![id.as_str()], row_to_org).optional()?)
    }

    fn update(&self, org: &Org) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        let n = conn.execute(
            "UPDATE orgs SET name = ?2, flavour = ?3, updated_at_ms = ?4 WHERE id = ?1",
            params![
                org.id.as_str(),
                org.name,
                org.flavour.as_str(),
                org.updated_at_ms,
            ],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    fn delete(&self, id: &OrgId) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute("DELETE FROM orgs WHERE id = ?1", params![id.as_str()])?;
        Ok(())
    }

    fn list(&self, limit: usize, after: Option<OrgId>) -> Result<Vec<Org>, StoreError> {
        let conn = self.store.conn()?;
        let after_str = after.as_ref().map(|i| i.as_str());
        let mut stmt = conn.prepare(
            "SELECT id, name, flavour, created_at_ms, updated_at_ms
             FROM orgs WHERE (?1 IS NULL OR id > ?1) ORDER BY id LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![after_str, limit as i64], row_to_org)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}
