//! Application root: provides the theme context and stylesheets.
//!
//! The Shell layout is mounted in a later commit of Phase 1.

use dioxus::prelude::*;

use crate::theme::{self, Theme};

const FAVICON: Asset = asset!("/assets/favicon.ico");
const MAIN_CSS: Asset = asset!("/assets/main.css");
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

#[component]
pub fn App() -> Element {
    let theme: Signal<Theme> = use_signal(theme::defaults::dark);
    use_context_provider(|| theme);

    let snapshot = theme.read();
    let data = snapshot.data_attr();
    let style = snapshot.css_variables();
    drop(snapshot);

    rsx! {
        document::Link { rel: "icon", href: FAVICON }
        document::Stylesheet { href: MAIN_CSS }
        document::Stylesheet { href: TAILWIND_CSS }
        div {
            id: "operon-root",
            "data-theme": "{data}",
            style: "{style}",
            "Operon Shell — initializing…"
        }
    }
}
