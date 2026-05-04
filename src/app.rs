//! Application root: provides theme, tab manager, plugin registry, activity-state, command
//! registry, and palette-state contexts; loads stylesheets; mounts the [`Shell`].

use std::rc::Rc;

use dioxus::prelude::*;

use crate::commands::{register_builtin_commands, CommandRegistry, PaletteState};
use crate::plugin::{register_builtins, PluginContext, PluginRegistry};
use crate::shell::state::{ActiveActivity, ActivityItemId, LastActiveActivity};
use crate::shell::Shell;
use crate::tabs::TabManager;
use crate::theme::{self, Theme};

const FAVICON: Asset = asset!("/assets/favicon.ico");
const MAIN_CSS: Asset = asset!("/assets/main.css");
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");
const THEME_CSS: Asset = asset!("/assets/theme.css");
const SHELL_CSS: Asset = asset!("/assets/shell.css");

#[component]
pub fn App() -> Element {
    let theme: Signal<Theme> = use_signal(theme::defaults::dark);
    use_context_provider(|| theme);

    let tabs: Signal<TabManager> = use_signal(TabManager::new);
    use_context_provider(|| tabs);

    let active: Signal<Option<ActivityItemId>> = use_signal(|| None);
    use_context_provider(|| ActiveActivity(active));

    let last_active: Signal<Option<ActivityItemId>> = use_signal(|| None);
    use_context_provider(|| LastActiveActivity(last_active));

    let palette: Signal<PaletteState> = use_signal(PaletteState::default);
    use_context_provider(|| palette);

    use_context_provider(|| {
        let mut registry = PluginRegistry::new();
        let ctx = PluginContext {
            theme,
            tabs: Some(tabs),
        };
        if let Err(err) = register_builtins(&mut registry, &ctx) {
            eprintln!("operon: register_builtins failed: {err}");
        }
        Rc::new(registry)
    });

    use_context_provider(|| {
        let mut reg = CommandRegistry::new();
        if let Err(err) = register_builtin_commands(&mut reg) {
            eprintln!("operon: register_builtin_commands failed: {err}");
        }
        Rc::new(reg)
    });

    let snapshot = theme.read();
    let data = snapshot.data_attr();
    let style = snapshot.css_variables();
    drop(snapshot);

    rsx! {
        document::Link { rel: "icon", href: FAVICON }
        document::Stylesheet { href: MAIN_CSS }
        document::Stylesheet { href: TAILWIND_CSS }
        document::Stylesheet { href: THEME_CSS }
        document::Stylesheet { href: SHELL_CSS }
        div {
            id: "operon-root",
            "data-theme": "{data}",
            style: "{style}",
            Shell {}
        }
    }
}
