//! Built-in plugin implementations bundled with the Shell.

pub mod artifact;
pub mod canvas;
pub mod cleanup;
pub mod code;
pub mod excalidraw;
pub mod image;
pub mod json_format;
pub mod kanban;
pub mod local_projects_explorer;
pub mod local_search;
pub mod markdown;
pub mod mdx;
pub mod notes_explorer;
pub mod phase;
pub mod plaintext;
#[cfg(not(target_arch = "wasm32"))]
pub mod revise_flow;
pub mod richtext_tiptap;
pub mod skill;
pub mod toc;
pub mod workflow;
