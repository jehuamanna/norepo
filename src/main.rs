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

/// Plans-Phase-9-monaco-desktop (rev 2): serve the editor-bridge dist
/// folder over a custom Wry protocol so the desktop bootstrap script
/// can `import('bridge://localhost/index.js')`. Wry doesn't auto-serve
/// arbitrary `/assets/` URLs the way `dx serve --target web` does;
/// without this protocol the dynamic-import 404s and Monaco never
/// mounts.
///
/// Source dir is resolved at compile time via `CARGO_MANIFEST_DIR`,
/// which works for `dx serve` / `cargo run`. For `dx bundle` builds
/// the assets need to be relocated alongside the binary; that's a
/// follow-up — this MVP gets us functional desktop Monaco in dev.
#[cfg(feature = "desktop")]
fn bridge_protocol_handler(
    _webview_id: dioxus::desktop::wry::WebViewId,
    req: dioxus::desktop::wry::http::Request<Vec<u8>>,
) -> dioxus::desktop::wry::http::Response<std::borrow::Cow<'static, [u8]>> {
    use std::borrow::Cow;
    use dioxus::desktop::wry::http::Response;

    let raw_path = req.uri().path().trim_start_matches('/');
    // Defense in depth: reject `..` segments to prevent directory
    // traversal outside the bridge dist folder.
    if raw_path
        .split('/')
        .any(|seg| seg == ".." || seg == "." || seg.starts_with(".."))
    {
        return Response::builder()
            .status(400)
            .body(Cow::Borrowed(b"" as &[u8]))
            .unwrap();
    }
    let project_root = env!("CARGO_MANIFEST_DIR");
    let file_path = format!(
        "{}/assets/editor-bridge/dist/{}",
        project_root, raw_path
    );
    match std::fs::read(&file_path) {
        Ok(bytes) => {
            let mime = if raw_path.ends_with(".js") || raw_path.ends_with(".mjs") {
                "application/javascript; charset=utf-8"
            } else if raw_path.ends_with(".css") {
                "text/css; charset=utf-8"
            } else if raw_path.ends_with(".json") {
                "application/json; charset=utf-8"
            } else if raw_path.ends_with(".ttf") {
                "font/ttf"
            } else if raw_path.ends_with(".woff") {
                "font/woff"
            } else if raw_path.ends_with(".woff2") {
                "font/woff2"
            } else if raw_path.ends_with(".svg") {
                "image/svg+xml"
            } else {
                "application/octet-stream"
            };
            Response::builder()
                .status(200)
                .header("Content-Type", mime)
                .header("Access-Control-Allow-Origin", "*")
                .body(Cow::Owned(bytes))
                .unwrap()
        }
        Err(_) => Response::builder()
            .status(404)
            .body(Cow::Borrowed(b"" as &[u8]))
            .unwrap(),
    }
}

fn main() {
    operon_dioxus::agent::tracing_init::init(None);

    #[cfg(feature = "desktop")]
    {
        let cfg = dioxus::desktop::Config::new()
            .with_custom_head(CRITICAL_HEAD.to_string())
            .with_custom_protocol("bridge", bridge_protocol_handler);
        dioxus::LaunchBuilder::new()
            .with_cfg(cfg)
            .launch(operon_dioxus::app::App);
    }

    #[cfg(not(feature = "desktop"))]
    {
        dioxus::launch(operon_dioxus::app::App);
    }
}
