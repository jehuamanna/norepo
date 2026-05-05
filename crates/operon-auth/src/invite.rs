//! Invite tokens use the same generate+hash machinery as session tokens.
//! Re-exported for clarity at call sites.

pub use crate::session::{generate_token as generate, hash_token as hash};
