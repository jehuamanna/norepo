//! Repository traits + SQLite implementations, one module per aggregate.

pub mod user;

pub use user::{SqliteUserRepository, User, UserRepository};
