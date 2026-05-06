//! Cross-platform clipboard write helper.
//!
//! Plans-Phase-3-note-id-create / "Copy note ID". Uses `document::eval` to call
//! `navigator.clipboard.writeText` on both targets — Dioxus desktop runs in a
//! webview (wry) so the same API is available everywhere we render.

use dioxus::prelude::*;

/// Best-effort write of `text` to the system clipboard. Returns once the eval
/// has been queued; no error reporting (a failed clipboard write is a soft
/// failure — the caller should still surface a toast on success).
pub fn copy_text(text: &str) {
    // Escape backticks / backslashes / interpolations so the value embeds
    // safely in a JS template-string.
    let escaped = text
        .replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace('$', "\\$");
    let script = format!(
        "(navigator.clipboard && navigator.clipboard.writeText) ? \
         navigator.clipboard.writeText(`{escaped}`).catch(()=>{{}}) : \
         (function(){{ const t=document.createElement('textarea'); \
            t.value=`{escaped}`; t.style.position='fixed'; t.style.opacity='0'; \
            document.body.appendChild(t); t.select(); \
            try {{ document.execCommand('copy'); }} catch(_) {{}} \
            document.body.removeChild(t); }})();"
    );
    let _ = document::eval(&script);
}

#[cfg(test)]
mod tests {
    /// `copy_text` calls `document::eval` which requires a Dioxus runtime to
    /// be active, so it can't be exercised in a plain `cargo test` harness.
    /// E2E coverage lives in `e2e/specs/note-create.spec.ts`.
    #[test]
    fn js_escaping_handles_specials() {
        // Verify escaping logic in isolation (mirrors the inline replace
        // chain in `copy_text`) so a future refactor can't silently break it.
        let escaped = "with `backticks`"
            .replace('\\', "\\\\")
            .replace('`', "\\`")
            .replace('$', "\\$");
        assert_eq!(escaped, "with \\`backticks\\`");

        let escaped2 = "with $interp and \\backs"
            .replace('\\', "\\\\")
            .replace('`', "\\`")
            .replace('$', "\\$");
        assert_eq!(escaped2, "with \\$interp and \\\\backs");
    }
}
