//! Global MCP servers modal.
//!
//! Opened from the global Settings dialog. Wraps
//! [`crate::shell::mcp_settings::McpSettingsPanel`] in
//! [`crate::shell::mcp_settings::McpPanelMode::Global`] mode so only
//! the user-scope MCP servers (the ones available to every project)
//! are listed; the scope picker on the add form is hidden, and
//! `claude mcp add` writes the global user config (no project cwd).

#![cfg(not(target_arch = "wasm32"))]

use dioxus::prelude::*;

use crate::shell::mcp_settings::{McpPanelMode, McpSettingsPanel};

/// App-scope visibility signal for the panel. Provided in `App`,
/// flipped by the Settings dialog button. The panel owns the close
/// write (Esc / scrim click / Close button).
#[derive(Clone, Copy)]
pub struct GlobalMcpSettingsOpen(pub Signal<bool>);

#[component]
pub fn GlobalMcpSettingsPanel() -> Element {
    let GlobalMcpSettingsOpen(open) = use_context();
    rsx! {
        McpSettingsPanel {
            open,
            mode: McpPanelMode::Global,
        }
    }
}
