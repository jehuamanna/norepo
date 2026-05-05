//! `JsonFormatPlugin` — `format_id = "json"`.
//!
//! Module is named `json_format` to avoid shadowing `serde_json::json!`. View pretty-prints
//! valid JSON (2-space indent) and falls back to raw text with an `data-error="parse"`
//! attribute on parse failure. Edit mounts MonacoBackend with the `json` language
//! descriptor; JSON Schema validation is deferred to v2.

use dioxus::prelude::*;

use crate::editor::LanguageDescriptor;
use crate::plugin::{FormatCaps, FormatPlugin, PluginManifest, PluginSurface};

mod view;

pub struct JsonFormatPlugin {
    manifest: PluginManifest,
}

impl JsonFormatPlugin {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: "json-note".into(),
                display_name: "JSON".into(),
                version: "0.1.0".into(),
                format_id: Some("json"),
                extensions: &["json"],
                surfaces: vec![PluginSurface::MainAreaTabContent],
            },
        }
    }
}

impl Default for JsonFormatPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for JsonFormatPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn capabilities(&self) -> FormatCaps {
        FormatCaps::VIEW | FormatCaps::EDIT
    }

    fn render(&self, _note_id: &str, content: &str) -> Element {
        let content = content.to_string();
        rsx! { view::JsonView { content } }
    }

    fn render_edit(
        &self,
        note_id: &str,
        content: &str,
        on_change: EventHandler<String>,
    ) -> Element {
        let note_id = note_id.to_string();
        let content = content.to_string();
        rsx! {
            view::JsonEditor {
                note_id,
                content,
                language: LanguageDescriptor::json(),
                on_change,
            }
        }
    }
}

/// Pretty-print a JSON string with 2-space indentation. Returns `None` on parse failure.
/// Public for the View component — also tested independently here.
pub(crate) fn pretty_print(input: &str) -> Option<String> {
    let parsed = simple_json_parse(input)?;
    Some(simple_json_format(&parsed, 0))
}

/// A tiny self-contained JSON parser sufficient for the View pretty-print path. Avoids a
/// `serde_json` dep for this single use; the bigger format-validation story is JSON Schema
/// inside Monaco, which is wholly JS-side.
#[derive(Clone, Debug)]
enum JsonValue {
    Null,
    Bool(bool),
    Number(String), // preserved as-is to avoid float roundtrip drift
    String(String),
    Array(Vec<JsonValue>),
    Object(Vec<(String, JsonValue)>),
}

fn simple_json_parse(s: &str) -> Option<JsonValue> {
    let bytes = s.as_bytes();
    let mut i = 0;
    let v = parse_value(bytes, &mut i)?;
    skip_ws(bytes, &mut i);
    if i == bytes.len() {
        Some(v)
    } else {
        None
    }
}

fn skip_ws(bytes: &[u8], i: &mut usize) {
    while *i < bytes.len() && matches!(bytes[*i], b' ' | b'\t' | b'\n' | b'\r') {
        *i += 1;
    }
}

fn parse_value(bytes: &[u8], i: &mut usize) -> Option<JsonValue> {
    skip_ws(bytes, i);
    if *i >= bytes.len() {
        return None;
    }
    match bytes[*i] {
        b'"' => parse_string(bytes, i).map(JsonValue::String),
        b'{' => parse_object(bytes, i),
        b'[' => parse_array(bytes, i),
        b't' | b'f' => parse_bool(bytes, i),
        b'n' => parse_null(bytes, i),
        b'-' | b'0'..=b'9' => parse_number(bytes, i),
        _ => None,
    }
}

fn parse_string(bytes: &[u8], i: &mut usize) -> Option<String> {
    if bytes.get(*i)? != &b'"' {
        return None;
    }
    *i += 1;
    let start = *i;
    while *i < bytes.len() {
        match bytes[*i] {
            b'"' => {
                let s = std::str::from_utf8(&bytes[start..*i]).ok()?.to_string();
                *i += 1;
                return Some(s);
            }
            b'\\' => *i += 2, // skip escape; sufficient for round-trip
            _ => *i += 1,
        }
    }
    None
}

fn parse_object(bytes: &[u8], i: &mut usize) -> Option<JsonValue> {
    *i += 1; // consume '{'
    let mut entries = Vec::new();
    skip_ws(bytes, i);
    if bytes.get(*i) == Some(&b'}') {
        *i += 1;
        return Some(JsonValue::Object(entries));
    }
    loop {
        skip_ws(bytes, i);
        let key = parse_string(bytes, i)?;
        skip_ws(bytes, i);
        if bytes.get(*i) != Some(&b':') {
            return None;
        }
        *i += 1;
        let v = parse_value(bytes, i)?;
        entries.push((key, v));
        skip_ws(bytes, i);
        match bytes.get(*i) {
            Some(b',') => {
                *i += 1;
                continue;
            }
            Some(b'}') => {
                *i += 1;
                return Some(JsonValue::Object(entries));
            }
            _ => return None,
        }
    }
}

fn parse_array(bytes: &[u8], i: &mut usize) -> Option<JsonValue> {
    *i += 1;
    let mut items = Vec::new();
    skip_ws(bytes, i);
    if bytes.get(*i) == Some(&b']') {
        *i += 1;
        return Some(JsonValue::Array(items));
    }
    loop {
        let v = parse_value(bytes, i)?;
        items.push(v);
        skip_ws(bytes, i);
        match bytes.get(*i) {
            Some(b',') => {
                *i += 1;
                continue;
            }
            Some(b']') => {
                *i += 1;
                return Some(JsonValue::Array(items));
            }
            _ => return None,
        }
    }
}

fn parse_bool(bytes: &[u8], i: &mut usize) -> Option<JsonValue> {
    if bytes[*i..].starts_with(b"true") {
        *i += 4;
        Some(JsonValue::Bool(true))
    } else if bytes[*i..].starts_with(b"false") {
        *i += 5;
        Some(JsonValue::Bool(false))
    } else {
        None
    }
}

fn parse_null(bytes: &[u8], i: &mut usize) -> Option<JsonValue> {
    if bytes[*i..].starts_with(b"null") {
        *i += 4;
        Some(JsonValue::Null)
    } else {
        None
    }
}

fn parse_number(bytes: &[u8], i: &mut usize) -> Option<JsonValue> {
    let start = *i;
    if bytes[*i] == b'-' {
        *i += 1;
    }
    while *i < bytes.len() && (bytes[*i].is_ascii_digit() || matches!(bytes[*i], b'.' | b'e' | b'E' | b'+' | b'-')) {
        *i += 1;
    }
    let s = std::str::from_utf8(&bytes[start..*i]).ok()?.to_string();
    Some(JsonValue::Number(s))
}

fn simple_json_format(v: &JsonValue, depth: usize) -> String {
    let indent = "  ".repeat(depth);
    let inner = "  ".repeat(depth + 1);
    match v {
        JsonValue::Null => "null".into(),
        JsonValue::Bool(true) => "true".into(),
        JsonValue::Bool(false) => "false".into(),
        JsonValue::Number(n) => n.clone(),
        JsonValue::String(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
        JsonValue::Array(items) if items.is_empty() => "[]".into(),
        JsonValue::Array(items) => {
            let body: Vec<String> = items
                .iter()
                .map(|i| format!("{inner}{}", simple_json_format(i, depth + 1)))
                .collect();
            format!("[\n{}\n{indent}]", body.join(",\n"))
        }
        JsonValue::Object(entries) if entries.is_empty() => "{}".into(),
        JsonValue::Object(entries) => {
            let body: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{inner}\"{}\": {}",
                        k.replace('\\', "\\\\").replace('"', "\\\""),
                        simple_json_format(v, depth + 1),
                    )
                })
                .collect();
            format!("{{\n{}\n{indent}}}", body.join(",\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_format_id_and_extensions() {
        let p = JsonFormatPlugin::new();
        assert_eq!(p.manifest().format_id, Some("json"));
        assert_eq!(p.manifest().extensions, &["json"]);
    }

    #[test]
    fn capabilities_are_view_and_edit() {
        let p = JsonFormatPlugin::new();
        let caps = p.capabilities();
        assert!(caps.contains(FormatCaps::VIEW));
        assert!(caps.contains(FormatCaps::EDIT));
    }

    #[test]
    fn pretty_prints_simple_object() {
        let out = pretty_print(r#"{"a":1,"b":"x"}"#).unwrap();
        assert_eq!(out, "{\n  \"a\": 1,\n  \"b\": \"x\"\n}");
    }

    #[test]
    fn pretty_prints_nested_array() {
        let out = pretty_print(r#"[1,[2,3]]"#).unwrap();
        assert_eq!(out, "[\n  1,\n  [\n    2,\n    3\n  ]\n]");
    }

    #[test]
    fn pretty_prints_empty_collections() {
        assert_eq!(pretty_print("{}"), Some("{}".into()));
        assert_eq!(pretty_print("[]"), Some("[]".into()));
    }

    #[test]
    fn invalid_json_returns_none() {
        assert!(pretty_print("{invalid").is_none());
        assert!(pretty_print("").is_none());
    }

    #[test]
    fn pretty_print_handles_null_and_bool() {
        assert_eq!(pretty_print("null"), Some("null".into()));
        assert_eq!(pretty_print("true"), Some("true".into()));
        assert_eq!(pretty_print("false"), Some("false".into()));
    }
}
