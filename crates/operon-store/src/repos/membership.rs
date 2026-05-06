use crate::sql::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::StoreError;
use crate::ids::{DepartmentId, MembershipId, OrgId, UserId};
use crate::sqlite::Store;
use crate::time::now_ms;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    MasterAdmin,
    OrgAdmin,
    User,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::MasterAdmin => "master_admin",
            Role::OrgAdmin => "org_admin",
            Role::User => "user",
        }
    }

    pub fn from_str(s: &str) -> Result<Self, StoreError> {
        match s {
            "master_admin" => Ok(Role::MasterAdmin),
            "org_admin" => Ok(Role::OrgAdmin),
            "user" => Ok(Role::User),
            other => Err(StoreError::InvalidInput(format!("unknown role {other}"))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Membership {
    pub id: MembershipId,
    pub user_id: UserId,
    pub org_id: OrgId,
    pub role: Role,
    pub department_id: Option<DepartmentId>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl Membership {
    pub fn new(
        user_id: UserId,
        org_id: OrgId,
        role: Role,
        department_id: Option<DepartmentId>,
    ) -> Result<Self, StoreError> {
        if role != Role::MasterAdmin && department_id.is_none() {
            return Err(StoreError::InvalidInput(
                "department_id required when role is not master_admin".into(),
            ));
        }
        let now = now_ms();
        Ok(Self {
            id: MembershipId::new(),
            user_id,
            org_id,
            role,
            department_id,
            created_at_ms: now,
            updated_at_ms: now,
        })
    }
}

pub trait MembershipRepository: Send + Sync {
    fn create(&self, m: &Membership) -> Result<(), StoreError>;
    fn get(&self, id: &MembershipId) -> Result<Option<Membership>, StoreError>;
    fn by_user_org(&self, user: &UserId, org: &OrgId) -> Result<Option<Membership>, StoreError>;
    fn by_user(&self, user: &UserId) -> Result<Vec<Membership>, StoreError>;
    fn by_org(&self, org: &OrgId) -> Result<Vec<Membership>, StoreError>;
    fn update(&self, m: &Membership) -> Result<(), StoreError>;
    fn delete(&self, id: &MembershipId) -> Result<(), StoreError>;
    fn count_master_admins(&self) -> Result<u32, StoreError>;
}

pub struct SqliteMembershipRepository {
    store: Store,
}

impl SqliteMembershipRepository {
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

fn row_to_membership(row: &crate::sql::Row<'_>) -> crate::sql::Result<Membership> {
    let id = MembershipId::from_str_strict(&row.get::<_, String>(0)?).map_err(invalid)?;
    let user_id = UserId::from_str_strict(&row.get::<_, String>(1)?).map_err(invalid)?;
    let org_id = OrgId::from_str_strict(&row.get::<_, String>(2)?).map_err(invalid)?;
    let role = Role::from_str(&row.get::<_, String>(3)?).map_err(invalid)?;
    let dept_opt: Option<String> = row.get(4)?;
    let department_id = match dept_opt {
        Some(s) => Some(DepartmentId::from_str_strict(&s).map_err(invalid)?),
        None => None,
    };
    Ok(Membership {
        id,
        user_id,
        org_id,
        role,
        department_id,
        created_at_ms: row.get(5)?,
        updated_at_ms: row.get(6)?,
    })
}

const SELECT_COLS: &str =
    "id, user_id, org_id, role, department_id, created_at_ms, updated_at_ms";

impl MembershipRepository for SqliteMembershipRepository {
    fn create(&self, m: &Membership) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "INSERT INTO memberships (id, user_id, org_id, role, department_id, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                m.id.as_str(),
                m.user_id.as_str(),
                m.org_id.as_str(),
                m.role.as_str(),
                m.department_id.as_ref().map(|d| d.as_str()),
                m.created_at_ms,
                m.updated_at_ms,
            ],
        )?;
        Ok(())
    }

    fn get(&self, id: &MembershipId) -> Result<Option<Membership>, StoreError> {
        let conn = self.store.conn()?;
        let sql = format!("SELECT {SELECT_COLS} FROM memberships WHERE id = ?1");
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt
            .query_row(params![id.as_str()], row_to_membership)
            .optional()?)
    }

    fn by_user_org(
        &self,
        user: &UserId,
        org: &OrgId,
    ) -> Result<Option<Membership>, StoreError> {
        let conn = self.store.conn()?;
        let sql =
            format!("SELECT {SELECT_COLS} FROM memberships WHERE user_id = ?1 AND org_id = ?2");
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt
            .query_row(params![user.as_str(), org.as_str()], row_to_membership)
            .optional()?)
    }

    fn by_user(&self, user: &UserId) -> Result<Vec<Membership>, StoreError> {
        let conn = self.store.conn()?;
        let sql = format!("SELECT {SELECT_COLS} FROM memberships WHERE user_id = ?1");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![user.as_str()], row_to_membership)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    fn by_org(&self, org: &OrgId) -> Result<Vec<Membership>, StoreError> {
        let conn = self.store.conn()?;
        let sql = format!("SELECT {SELECT_COLS} FROM memberships WHERE org_id = ?1");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![org.as_str()], row_to_membership)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    fn update(&self, m: &Membership) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        let n = conn.execute(
            "UPDATE memberships SET role = ?2, department_id = ?3, updated_at_ms = ?4 WHERE id = ?1",
            params![
                m.id.as_str(),
                m.role.as_str(),
                m.department_id.as_ref().map(|d| d.as_str()),
                m.updated_at_ms,
            ],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    fn delete(&self, id: &MembershipId) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "DELETE FROM memberships WHERE id = ?1",
            params![id.as_str()],
        )?;
        Ok(())
    }

    fn count_master_admins(&self) -> Result<u32, StoreError> {
        let conn = self.store.conn()?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM memberships WHERE role = 'master_admin'",
            [],
            |row| row.get(0),
        )?;
        Ok(count as u32)
    }
}
