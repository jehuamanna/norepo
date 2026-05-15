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
/// Plans-Phase-9-monaco-desktop (rev 15): assets are embedded into
/// the binary via `include_dir!` so installed bundles (.deb /
/// .AppImage / .app / .msi) work without any sibling asset folder.
/// `dx bundle` only copies `asset!()`-tagged files into the package,
/// and the bridge ships ~120 chunks (monaco languages) that we can't
/// realistically tag one-by-one. Embedding sidesteps the OS-specific
/// "where do resources end up" problem entirely.
///
/// Dev override: setting `OPERON_BRIDGE_DIR` to an on-disk copy of
/// the dist folder makes the handler read from there first, so
/// editor-bridge iteration doesn't require a Rust rebuild. The
/// embedded copy is the fallback path.
#[cfg(feature = "desktop")]
static BRIDGE_DIST: include_dir::Dir<'_> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/assets/editor-bridge/dist");

#[cfg(feature = "desktop")]
fn bridge_mime_for(path: &str) -> &'static str {
    if path.ends_with(".js") || path.ends_with(".mjs") {
        "application/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".json") {
        "application/json; charset=utf-8"
    } else if path.ends_with(".ttf") {
        "font/ttf"
    } else if path.ends_with(".woff") {
        "font/woff"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else {
        "application/octet-stream"
    }
}

#[cfg(feature = "desktop")]
fn bridge_protocol_handler(
    _webview_id: dioxus::desktop::wry::WebViewId,
    req: dioxus::desktop::wry::http::Request<Vec<u8>>,
) -> dioxus::desktop::wry::http::Response<std::borrow::Cow<'static, [u8]>> {
    use std::borrow::Cow;
    use dioxus::desktop::wry::http::Response;

    let raw_path = req.uri().path().trim_start_matches('/');
    // Defense in depth: reject `..` / `.` segments to prevent
    // directory traversal outside the bridge dist folder. Empty
    // segments (`//`) are also rejected for safety.
    if raw_path.is_empty()
        || raw_path.split('/').any(|seg| {
            seg.is_empty()
                || seg == ".."
                || seg == "."
                || seg.starts_with("..")
        })
    {
        return Response::builder()
            .status(400)
            .body(Cow::Borrowed(b"" as &[u8]))
            .unwrap();
    }

    let ok = |bytes: Vec<u8>| {
        Response::builder()
            .status(200)
            .header("Content-Type", bridge_mime_for(raw_path))
            .header("Access-Control-Allow-Origin", "*")
            .body(Cow::Owned(bytes))
            .unwrap()
    };

    // 1) `OPERON_BRIDGE_DIR` override — point at any on-disk dist
    //    copy. Used during editor-bridge dev iteration so JS edits
    //    don't require a Rust rebuild.
    if let Ok(env_root) = std::env::var("OPERON_BRIDGE_DIR") {
        let p = std::path::PathBuf::from(env_root).join(raw_path);
        if let Ok(bytes) = std::fs::read(&p) {
            return ok(bytes);
        }
    }

    // 2) Embedded copy — the canonical path for installed bundles
    //    AND for `cargo run` / `dx serve` (every run picks up the
    //    dist baked into the build).
    if let Some(file) = BRIDGE_DIST.get_file(raw_path) {
        return ok(file.contents().to_vec());
    }

    Response::builder()
        .status(404)
        .body(Cow::Borrowed(b"" as &[u8]))
        .unwrap()
}

fn main() {
    operon_dioxus::agent::tracing_init::init(None);

    #[cfg(feature = "desktop")]
    {
        // `with_menu(None)` suppresses dioxus-desktop's default native
        // Window/Edit/Help menubar — Operon ships its own in-app Menubar
        // (`src/shell/menubar.rs`) and the OS-level strip is redundant.
        //
        // `with_disable_context_menu(false)` keeps the webview's native
        // right-click menu (with Copy/Paste/Select All) in release builds.
        // Dioxus-desktop's default is to inject a contextmenu-preventDefault
        // script when `!cfg!(debug_assertions)`, which otherwise breaks copy
        // from chat / transcript text in shipped builds.
        let cfg = dioxus::desktop::Config::new()
            .with_custom_head(CRITICAL_HEAD.to_string())
            .with_custom_protocol("bridge", bridge_protocol_handler)
            .with_menu(None)
            .with_disable_context_menu(false);
        dioxus::LaunchBuilder::new()
            .with_cfg(cfg)
            .launch(operon_dioxus::app::App);
    }

    #[cfg(not(feature = "desktop"))]
    {
        dioxus::launch(operon_dioxus::app::App);
    }
}
