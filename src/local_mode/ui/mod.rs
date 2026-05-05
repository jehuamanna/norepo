//! Reusable UI primitives for the Local-Mode shell. Phase-2 introduces the
//! context menu, inline rename input, and confirm dialog used by the explorer
//! panel; later phases (notes, search) reuse the same building blocks.

pub mod confirm;
pub mod context_menu;
pub mod inline_rename;

pub use confirm::ConfirmDialog;
pub use context_menu::{ContextMenu, ContextMenuItem};
pub use inline_rename::InlineRename;
