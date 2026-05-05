//! RBAG/ODU/TPN frontend — Plans-Phase-6.
//!
//! This module hosts the auth screens, org switcher, admin tooling, and
//! project/note views that consume the operon-api-server endpoints introduced
//! in Plans-Phase-2 through Plans-Phase-5. It is structurally independent
//! from the existing workspace-shell tree (`crate::shell`) — both can coexist
//! and the host app chooses which to mount via routing.

pub mod api;
pub mod screens;
pub mod state;
pub mod types;

pub use state::{AppState, AppStateProvider};
pub use types::{LoginResponse, MePayload, MembershipBrief};
