//! Reusable UI primitives for the Local-Mode shell. Phase-2 introduces the
//! context menu, inline rename input, and confirm dialog used by the explorer
//! panel; later phases (notes, search) reuse the same building blocks.

pub mod clipboard;
pub mod confirm;
pub mod context_menu;
pub mod drag_drop;
pub mod inline_rename;
pub mod toast;

pub use clipboard::{
    BulkClipboard, ClipKind, ClipPayload, Clipboard, LocalBulkClipboard, LocalClipboard,
};
pub use confirm::ConfirmDialog;
pub use context_menu::{ContextMenu, ContextMenuItem};
pub use drag_drop::{
    classify_drop_position, DragDescendants, DragKind, DragSession, DropPosition,
};
pub use inline_rename::InlineRename;
pub use toast::{Toast, ToastHost, ToastKind, ToastSlot};
