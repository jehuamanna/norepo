/// Critical chrome CSS embedded into the desktop webview's <head> via
/// `dioxus::desktop::Config::with_custom_head`. The webview parses this
/// synchronously before the body, so the very first paint already has the
/// VS-Code grid + theme tokens applied — no flash of unstyled content while
/// `document::Stylesheet` round-trips through wry's asset protocol on the
/// first VDOM commit.
///
/// We inline all five chrome stylesheets at compile time. They re-load via
/// `document::Stylesheet` from inside `App` for hot-reload + cache busting,
/// but the inline copy here paints first.
#[cfg(feature = "desktop")]
const CRITICAL_HEAD: &str = concat!(
    "<style>",
    include_str!("../assets/main.css"),
    "</style>\n",
    "<style>",
    include_str!("../assets/tailwind.css"),
    "</style>\n",
    "<style>",
    include_str!("../assets/theme.css"),
    "</style>\n",
    "<style>",
    include_str!("../assets/shell.css"),
    "</style>\n",
    "<style>",
    include_str!("../assets/markdown.css"),
    "</style>\n",
);

fn main() {
    operon_dioxus::agent::tracing_init::init(None);

    #[cfg(feature = "desktop")]
    {
        let cfg = dioxus::desktop::Config::new().with_custom_head(CRITICAL_HEAD.to_string());
        dioxus::LaunchBuilder::new()
            .with_cfg(cfg)
            .launch(operon_dioxus::app::App);
    }

    #[cfg(not(feature = "desktop"))]
    {
        dioxus::launch(operon_dioxus::app::App);
    }
}
