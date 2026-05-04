//! Plugin lifecycle context.
//!
//! Handed to plugins on registration. Phase 2 exposes only the theme signal; later phases
//! extend [`PluginContext`] with optional handles to the tab manager (Phase 3) and the
//! command registry (Phase 5).

use dioxus::prelude::*;

use crate::theme::Theme;

#[derive(Clone)]
pub struct PluginContext {
    /// The active theme. Plugins may read it to style their contributions.
    pub theme: Signal<Theme>,
    // Forward-looking fields — to be added by their respective phases:
    //   pub tabs: Option<Signal<crate::tabs::TabManager>>,        (Phase 3)
    //   pub commands: Option<std::rc::Rc<crate::commands::CommandRegistry>>,  (Phase 5)
}
