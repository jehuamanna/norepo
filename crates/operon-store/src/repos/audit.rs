use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::error::StoreError;
use crate::ids::{OrgId, UserId};
use crate::sqlite::Store;
use crate::time::now_ms;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Allowed,
    Denied,
    Error,
}

impl AuditOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuditOutcome::Allowed => "allowed",
            AuditOutcome::Denied => "denied",
            AuditOutcome::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub user_id: Option<UserId>,
    pub org_id: Option<OrgId>,
    pub role: Option<String>,
    pub action: String,
    pub scope_type: String,
    pub scope_id: Option<String>,
    pub outcome: AuditOutcome,
    pub payload_json: Option<String>,
}

pub trait AuditLogRepository: Send + Sync {
    fn record(&self, entry: &AuditEntry) -> Result<(), StoreError>;
    fn count_by_outcome(&self, outcome: AuditOutcome) -> Result<u32, StoreError>;
    fn by_action_outcome(
        &self,
        action: &str,
        outcome: AuditOutcome,
    ) -> Result<Vec<AuditEntry>, StoreError>;
}

pub struct SqliteAuditLogRepository {
    store: Store,
}

impl SqliteAuditLogRepository {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

impl AuditLogRepository for SqliteAuditLogRepository {
    fn record(&self, e: &AuditEntry) -> Result<(), StoreError> {
        let conn = self.store.conn()?;
        conn.execute(
            "INSERT INTO audit_log (id, user_id, org_id, role, action, scope_type, scope_id, outcome, payload_json, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                Uuid::new_v4().to_string(),
                e.user_id.as_ref().map(|u| u.as_str()),
                e.org_id.as_ref().map(|o| o.as_str()),
                e.role,
                e.action,
                e.scope_type,
                e.scope_id,
                e.outcome.as_str(),
                e.payload_json,
                now_ms(),
            ],
        )?;
        Ok(())
    }

    fn count_by_outcome(&self, outcome: AuditOutcome) -> Result<u32, StoreError> {
        let conn = self.store.conn()?;
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM audit_log WHERE outcome = ?1",
            params![outcome.as_str()],
            |r| r.get(0),
        )?;
        Ok(n as u32)
    }

    fn by_action_outcome(
        &self,
        action: &str,
        outcome: AuditOutcome,
    ) -> Result<Vec<AuditEntry>, StoreError> {
        let conn = self.store.conn()?;
        let mut stmt = conn.prepare(
            "SELECT user_id, org_id, role, action, scope_type, scope_id, outcome, payload_json
             FROM audit_log WHERE action = ?1 AND outcome = ?2",
        )?;
        let rows = stmt.query_map(params![action, outcome.as_str()], |row| {
            let user_id_opt: Option<String> = row.get(0)?;
            let org_id_opt: Option<String> = row.get(1)?;
            let outcome_str: String = row.get(6)?;
            Ok(AuditEntry {
                user_id: user_id_opt
                    .and_then(|s| UserId::from_str_strict(&s).ok()),
                org_id: org_id_opt.and_then(|s| OrgId::from_str_strict(&s).ok()),
                role: row.get(2)?,
                action: row.get(3)?,
                scope_type: row.get(4)?,
                scope_id: row.get(5)?,
                outcome: match outcome_str.as_str() {
                    "allowed" => AuditOutcome::Allowed,
                    "denied" => AuditOutcome::Denied,
                    _ => AuditOutcome::Error,
                },
                payload_json: row.get(7)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}
