//! LSP wire codec — `Content-Length: N\r\n\r\n<json>` framing over a byte stream.

use bytes::{Bytes, BytesMut};
use std::io;

/// Encode a JSON value as an LSP frame.
pub fn encode(payload: &serde_json::Value) -> Bytes {
    let body = serde_json::to_vec(payload).expect("serde_json on Value never fails");
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(&body);
    Bytes::from(out)
}

/// Try to decode a single LSP frame from `buf`. Consumes the frame on success.
/// Returns `Ok(None)` if more bytes are needed, `Ok(Some(_))` if a frame was
/// extracted, `Err(_)` on malformed input.
pub fn try_decode(buf: &mut BytesMut) -> io::Result<Option<serde_json::Value>> {
    // Header termination: \r\n\r\n
    let header_end = match find_header_end(buf) {
        Some(p) => p,
        None => return Ok(None),
    };
    let header_bytes = &buf[..header_end];
    let header_str = std::str::from_utf8(header_bytes)
        .map_err(|_| io::Error::other("invalid utf-8 in LSP header"))?;
    let mut content_length: Option<usize> = None;
    for line in header_str.split("\r\n") {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| io::Error::other("malformed LSP header line"))?;
        if name.trim().eq_ignore_ascii_case("Content-Length") {
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| io::Error::other("invalid Content-Length"))?,
            );
        }
    }
    let cl = content_length.ok_or_else(|| io::Error::other("missing Content-Length"))?;
    let total = header_end + 4 + cl; // 4 = "\r\n\r\n"
    if buf.len() < total {
        return Ok(None);
    }
    // Consume the header + body.
    let _ = buf.split_to(header_end + 4);
    let body_bytes = buf.split_to(cl);
    let v: serde_json::Value = serde_json::from_slice(&body_bytes)
        .map_err(|e| io::Error::other(format!("invalid LSP body json: {e}")))?;
    Ok(Some(v))
}

fn find_header_end(buf: &BytesMut) -> Option<usize> {
    if buf.len() < 4 {
        return None;
    }
    for i in 0..=buf.len() - 4 {
        if &buf[i..i + 4] == b"\r\n\r\n" {
            return Some(i);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_writes_content_length_header() {
        let v = serde_json::json!({ "jsonrpc": "2.0", "method": "initialize" });
        let frame = encode(&v);
        let s = std::str::from_utf8(&frame).unwrap();
        assert!(s.starts_with("Content-Length: "));
        assert!(s.contains("\r\n\r\n"));
        assert!(s.contains("\"method\":\"initialize\""));
    }

    #[test]
    fn round_trip_single_message() {
        let v = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "test" });
        let frame = encode(&v);
        let mut buf = BytesMut::from(&frame[..]);
        let got = try_decode(&mut buf).unwrap().unwrap();
        assert_eq!(got, v);
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn decode_returns_none_when_header_incomplete() {
        let mut buf = BytesMut::from(&b"Content-Length: 10\r\n"[..]);
        assert!(try_decode(&mut buf).unwrap().is_none());
    }

    #[test]
    fn decode_returns_none_when_body_incomplete() {
        let mut buf = BytesMut::from(&b"Content-Length: 100\r\n\r\n{"[..]);
        assert!(try_decode(&mut buf).unwrap().is_none());
    }

    #[test]
    fn decode_two_concatenated_frames() {
        let a = serde_json::json!({ "id": 1 });
        let b = serde_json::json!({ "id": 2 });
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&encode(&a));
        buf.extend_from_slice(&encode(&b));
        let got_a = try_decode(&mut buf).unwrap().unwrap();
        let got_b = try_decode(&mut buf).unwrap().unwrap();
        assert_eq!(got_a, a);
        assert_eq!(got_b, b);
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn missing_content_length_errors() {
        let mut buf = BytesMut::from(&b"Other-Header: 5\r\n\r\n"[..]);
        assert!(try_decode(&mut buf).is_err());
    }

    #[test]
    fn case_insensitive_content_length() {
        let mut buf = BytesMut::from(&b"content-length: 2\r\n\r\n{}"[..]);
        let v = try_decode(&mut buf).unwrap().unwrap();
        assert_eq!(v, serde_json::json!({}));
    }
}
