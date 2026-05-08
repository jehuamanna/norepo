//! Workflow notes — visual DAG of skill-node references with stale-
//! tracking for cascade re-execution.
//!
//! M3a (this milestone): pure Rust engine + data model. Hashes skill +
//! node config + upstream outputs, marks downstream nodes Dirty when
//! anything drifts, topo-sorts the dirty subset for cascade execution.
//! No UI; the React Flow canvas + executor wiring land in M3b/M3c.

pub mod engine;
pub mod state;

pub use engine::{
    compute_input_hash, hash_body, propagate_dirty, topo_order_dirty, CycleError, EngineError,
    SkillBag, SkillSnapshot,
};
pub use state::{Edge, EdgeId, Node, NodeId, NodeStatus, WorkflowGraph};