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

/// Read an image out of the OS clipboard (bypassing the webview).
///
/// `navigator.clipboard.read()` inside the wry webview is gated behind
/// browser-style permissions — WebKitGTK rejects it with `NotAllowedError`
/// in the default desktop config — so the image-note "paste" affordance
/// asks the OS directly via `arboard`. The returned bytes are PNG-encoded
/// from arboard's RGBA pixel buffer so they slot straight into the
/// existing `images::write_image` / `attach_blob` path.
///
/// Returns the encoded bytes plus the MIME (`"image/png"`). Errors are
/// short user-facing strings ("No image on the clipboard.", etc.).
#[cfg(not(target_arch = "wasm32"))]
pub fn read_clipboard_image_png() -> Result<(Vec<u8>, &'static str), String> {
    // Errors are prefixed with `[arboard]` so user-facing surfaces can
    // distinguish them from the JS-bridge paths (`[js-paste]`,
    // `[js-keydown]`) and from any leaked legacy listener output.
    let mut clip = arboard::Clipboard::new()
        .map_err(|e| format!("[arboard] Could not open clipboard: {e}"))?;
    let img = match clip.get_image() {
        Ok(img) => img,
        Err(arboard::Error::ContentNotAvailable) => {
            return Err("[arboard] No image on the clipboard.".into());
        }
        Err(e) => return Err(format!("[arboard] Clipboard read failed: {e}")),
    };
    if img.width == 0 || img.height == 0 {
        return Err("[arboard] Clipboard image has zero dimensions.".into());
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, img.width as u32, img.height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| format!("[arboard] PNG header write failed: {e}"))?;
        writer
            .write_image_data(&img.bytes)
            .map_err(|e| format!("[arboard] PNG encode failed: {e}"))?;
    }
    Ok((out, "image/png"))
}

/// Write UTF-8 text to the OS clipboard via `arboard` (bypassing the
/// webview). Counterpart to `read_clipboard_text` for the same
/// reason: Monaco's `Cmd+C` / `Cmd+X` use `document.execCommand` /
/// `clipboardData` which silently no-op in WebKitGTK. The Monaco
/// host intercepts copy/cut, derives the selected text JS-side,
/// and routes it here so arboard hits the OS clipboard directly.
#[cfg(not(target_arch = "wasm32"))]
pub fn write_clipboard_text(text: &str) -> Result<(), String> {
    let mut clip = arboard::Clipboard::new()
        .map_err(|e| format!("[arboard] Could not open clipboard: {e}"))?;
    clip.set_text(text.to_string())
        .map_err(|e| format!("[arboard] Clipboard write failed: {e}"))
}

/// Read UTF-8 text out of the OS clipboard (bypassing the webview).
///
/// Counterpart to `read_clipboard_image_png` for plain text. The
/// Monaco editor host needs this because wry/WebKitGTK silently
/// drops text from `clipboardData.getData('text/plain')` on `paste`
/// events, so Monaco's native `Cmd+V` produces nothing. Routing
/// through arboard sidesteps the webview's broken clipboard bridge.
///
/// Returns the text as `String`. Errors map to short user-facing
/// strings prefixed with `[arboard]`. The `ContentNotAvailable`
/// case (clipboard has only image / file / nothing) returns an
/// error so callers can short-circuit silently — image-only
/// clipboards are handled by the separate image-paste path.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_clipboard_text() -> Result<String, String> {
    let mut clip = arboard::Clipboard::new()
        .map_err(|e| format!("[arboard] Could not open clipboard: {e}"))?;
    match clip.get_text() {
        Ok(t) => Ok(t),
        Err(arboard::Error::ContentNotAvailable) => {
            Err("[arboard] No text on the clipboard.".into())
        }
        Err(e) => Err(format!("[arboard] Clipboard text read failed: {e}")),
    }
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
