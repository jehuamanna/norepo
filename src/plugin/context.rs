//! Plugin lifecycle context.
//!
//! Handed to plugins on registration. Phase 2 exposed only the theme signal; Phase 3 added
//! the optional tabs handle. Phase 5 will add the optional command-registry handle.

use dioxus::prelude::*;

use crate::tabs::TabManager;
use crate::theme::Theme;

#[derive(Clone)]
pub struct PluginContext {
    /// The active theme. Plugins may read it to style their contributions.
    pub theme: Signal<Theme>,
    /// `Some(...)` once the tab manager has been provided; `None` while the registry is built
    /// before [`crate::tabs::TabManager`] is initialized (kept Optional for the
    /// architecturally-pristine path even though current callers always supply it).
    pub tabs: Option<Signal<TabManager>>,
    // Forward-looking field — to be added by Phase 5:
    //   pub commands: Option<std::rc::Rc<crate::commands::CommandRegistry>>,
}
