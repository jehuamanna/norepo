use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format_version: u32,
    pub schema_version: u32,
    pub source_org_id: String,
    pub source_org_name: String,
    pub source_flavour: String,
    pub exported_at_ms: i64,
    pub exporter_user_id: Option<String>,
    pub entity_counts: BTreeMap<String, u32>,
}
