use crate::sql::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::StoreError;
use crate::ids::{DepartmentId, InviteId, OrgId, UserId};
use crate::repos::membership::Role;
use crate::store::Store;
use crate::time::now_ms;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Invite {
    pub id: InviteId,
    pub email: String,
    pub org_id: OrgId,
    pub role: Role,
    pub department_id: Option<DepartmentId>,
    pub invited_by: UserId,
    pub token_hash: String,
    pub expires_at_ms: i64,
    pub accepted_at_ms: Option<i64>,
    pub created_at_ms: i64,
}

pub trait InviteRepository: Send + Sync {
    fn create(&self, i: &Invite) -> Result<(), StoreError>;
    fn get(&self, id: &InviteId) -> Result<Option<Invite>, StoreError>;
    fn by_token_hash(&self, token_hash: &str) -> Result<Option<Invite>, StoreError>;
    fn by_email_pending(&self, email: &str) -> Result<Vec<Invite>, StoreError>;
    fn mark_accepted(&self, id: &InviteId) -> Result<(), StoreError>;
    fn delete(&self, id: &InviteId) -> Result<(), StoreError>;
}

pub struct SqliteInviteRepository {
    store: Store,
}

impl SqliteInviteRepository {
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

fn row_to_invite(row: &crate::sql::Row<'_>) -> crate::sql::Result<Invite> {
    let dept_opt: Option<String> = row.get(4)?;
    Ok(Invite {
        id: InviteId::from_str_strict(&row.get::<_, String>(0)?).map_err(invalid)?,
        email: row.get(1)?,
        org_id: OrgId::from_str_strict(&row.get::<_, String>(2)?).map_err(invalid)?,
        role: Role::from_str(&row.get::<_, String>(3)?).map_err(invalid)?,
        department_id: match dept_opt {
            Some(s) => Some(DepartmentId::from_str_strict(&s).map_err(invalid)?),
            None => None,
        },
        invited_by: UserId::from_str_strict(&row.get::<_, String>(5)?).map_err(invalid)?,
        token_hash: row.get(6)?,
        expires_at_ms: row.get(7)?,
        accepted_at_ms: row.get(8)?,
        created_at_ms: row.get(9)?,
    })
}

const SELECT_COLS: &str = "id, email, org_id, role, department_id, invited_by, token_hash, expires_at_ms, accepted_at_ms, created_at_ms";

impl InviteRepository for SqliteInviteRepository {
    fn create(&self, i: &Invite) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "INSERT INTO invites (id, email, org_id, role, department_id, invited_by, token_hash, expires_at_ms, accepted_at_ms, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                i.id.as_str(),
                i.email,
                i.org_id.as_str(),
                i.role.as_str(),
                i.department_id.as_ref().map(|d| d.as_str()),
                i.invited_by.as_str(),
                i.token_hash,
                i.expires_at_ms,
                i.accepted_at_ms,
                i.created_at_ms,
            ],
        )?;
        Ok(())
    }

    fn get(&self, id: &InviteId) -> Result<Option<Invite>, StoreError> {
        let conn = self.store.conn()?;
        let sql = format!("SELECT {SELECT_COLS} FROM invites WHERE id = ?1");
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt
            .query_row(params![id.as_str()], row_to_invite)
            .optional()?)
    }

    fn by_token_hash(&self, token_hash: &str) -> Result<Option<Invite>, StoreError> {
        let conn = self.store.conn()?;
        let sql = format!("SELECT {SELECT_COLS} FROM invites WHERE token_hash = ?1");
        let mut stmt = conn.prepare(&sql)?;
        Ok(stmt
            .query_row(params![token_hash], row_to_invite)
            .optional()?)
    }

    fn by_email_pending(&self, email: &str) -> Result<Vec<Invite>, StoreError> {
        let conn = self.store.conn()?;
        let sql = format!(
            "SELECT {SELECT_COLS} FROM invites WHERE email = ?1 COLLATE NOCASE \
             AND accepted_at_ms IS NULL AND expires_at_ms > ?2"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![email.trim(), now_ms()], row_to_invite)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    fn mark_accepted(&self, id: &InviteId) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        let n = conn.execute(
            "UPDATE invites SET accepted_at_ms = ?2 WHERE id = ?1 AND accepted_at_ms IS NULL",
            params![id.as_str(), now_ms()],
        )?;
        if n == 0 {
            return Err(StoreError::Conflict("invite already accepted".into()));
        }
        Ok(())
    }

    fn delete(&self, id: &InviteId) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute("DELETE FROM invites WHERE id = ?1", params![id.as_str()])?;
        Ok(())
    }
}
