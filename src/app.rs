//! Application root: provides the theme + plugin registry contexts, loads stylesheets, and
//! mounts the [`Shell`].

use std::rc::Rc;

use dioxus::prelude::*;

use crate::plugin::{register_builtins, PluginContext, PluginRegistry};
use crate::shell::Shell;
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

    use_context_provider(|| {
        let mut registry = PluginRegistry::new();
        let ctx = PluginContext { theme };
        if let Err(err) = register_builtins(&mut registry, &ctx) {
            eprintln!("operon: register_builtins failed: {err}");
        }
        Rc::new(registry)
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
