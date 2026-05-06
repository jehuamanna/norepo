//! Image-blob helpers for image notes.
//!
//! Plans-Phase-6-image-notes. Bytes are content-addressed by SHA-256 and
//! stored at `<vault>/.operon/images/<sha256>.<ext>`. Multiple image notes
//! can reference the same blob — refcount-based GC happens via the
//! `attachments` table when a note is deleted (follow-up).
//!
//! Web parity (OPFS-backed write) is part of Plans-Phase-2-saving's wasm
//! work and gated on the wasm Store landing.

#![cfg(not(target_arch = "wasm32"))]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::vault::VaultRoot;

/// Hard cap per image. Pasted screenshots routinely run a few MB; 25MB
/// covers most photo formats while still rejecting pathological inputs.
pub const MAX_IMAGE_BYTES: usize = 25 * 1024 * 1024;

#[derive(Debug)]
pub enum ImageErr {
    UnsupportedMime(String),
    TooLarge { actual: usize, cap: usize },
    Io(std::io::Error),
    NotFound(PathBuf),
}

impl std::fmt::Display for ImageErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedMime(m) => write!(f, "unsupported image mime: {m}"),
            Self::TooLarge { actual, cap } => write!(f, "image too large ({actual} bytes > {cap})"),
            Self::Io(e) => write!(f, "image filesystem I/O failed: {e}"),
            Self::NotFound(p) => write!(f, "image not found at {}", p.display()),
        }
    }
}

impl std::error::Error for ImageErr {}

impl From<std::io::Error> for ImageErr {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Map a MIME type to the canonical filename extension Operon uses on disk.
/// Returns `None` for unsupported types so callers can reject early.
pub fn extension_for_mime(mime: &str) -> Option<&'static str> {
    match mime.to_ascii_lowercase().as_str() {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/webp" => Some("webp"),
        "image/gif" => Some("gif"),
        "image/svg+xml" => Some("svg"),
        "image/avif" => Some("avif"),
        _ => None,
    }
}

/// Result of a successful write — points at the on-disk blob and carries
/// the metadata the `attachments` row needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageWrite {
    /// Path relative to the vault root (e.g. `.operon/images/<sha>.png`).
    pub relative_path: PathBuf,
    /// Lower-case hex SHA-256.
    pub sha256_hex: String,
    /// Re-echoed from the input MIME so callers don't have to re-derive.
    pub mime_type: String,
    /// Number of bytes written.
    pub size_bytes: u64,
}

/// Hash + write `bytes` content-addressed under the vault. If the file
/// already exists (sha collision = identical content), the write becomes a
/// no-op and the existing `ImageWrite` is returned.
pub fn write_image(
    vault: &VaultRoot,
    bytes: &[u8],
    mime: &str,
) -> Result<ImageWrite, ImageErr> {
    let ext = extension_for_mime(mime).ok_or_else(|| ImageErr::UnsupportedMime(mime.into()))?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(ImageErr::TooLarge {
            actual: bytes.len(),
            cap: MAX_IMAGE_BYTES,
        });
    }
    let sha = sha256_hex(bytes);
    let images_dir = vault.images_dir();
    fs::create_dir_all(&images_dir)?;
    let filename = format!("{sha}.{ext}");
    let abs_path = images_dir.join(&filename);
    let relative_path = Path::new(".operon/images").join(&filename);
    if !abs_path.exists() {
        // Atomic-ish write: tempfile in same dir, fsync, rename.
        let tmp_path = abs_path.with_extension(format!("{ext}.tmp"));
        {
            let mut f = fs::File::create(&tmp_path)?;
            f.write_all(bytes)?;
            f.sync_all().ok();
        }
        fs::rename(&tmp_path, &abs_path)?;
    }
    Ok(ImageWrite {
        relative_path,
        sha256_hex: sha,
        mime_type: mime.to_string(),
        size_bytes: bytes.len() as u64,
    })
}

/// Read bytes back for a given relative blob path.
pub fn read_image(vault: &VaultRoot, relative: &Path) -> Result<Vec<u8>, ImageErr> {
    let abs = vault.path().join(relative);
    fs::read(&abs).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => ImageErr::NotFound(abs),
        _ => ImageErr::Io(e),
    })
}

/// Read the blob at `relative` and return a `data:<mime>;base64,...` URL
/// suitable for inline `<img src="…">` rendering. Used by both the
/// image-note tab viewer and the wikilink-embed image resolver. Returns
/// `None` when the blob is missing or its extension isn't a recognised
/// image type — callers fall back to text rendering in that case.
pub fn data_url_for_blob(vault: &VaultRoot, relative: &Path) -> Option<String> {
    let mime = match relative.extension().and_then(|s| s.to_str()) {
        Some(e) => match e.to_ascii_lowercase().as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            "gif" => "image/gif",
            "svg" => "image/svg+xml",
            "avif" => "image/avif",
            _ => return None,
        },
        None => return None,
    };
    let bytes = read_image(vault, relative).ok()?;
    Some(format!("data:{mime};base64,{}", base64_encode(&bytes)))
}

/// Standard-alphabet base64 encoder, padded. Tiny enough to avoid a crate
/// dependency for the `data:` URL use case.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHA: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = if chunk.len() > 1 { chunk[1] } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] } else { 0 };
        let triple = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(ALPHA[((triple >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((triple >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHA[((triple >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHA[(triple & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for byte in digest {
        s.push_str(&format!("{byte:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fixture_vault() -> (tempfile::TempDir, VaultRoot) {
        let tmp = tempdir().unwrap();
        let root = VaultRoot {
            path: tmp.path().to_path_buf(),
        };
        (tmp, root)
    }

    #[test]
    fn extension_map_covers_supported_mimes() {
        assert_eq!(extension_for_mime("image/png"), Some("png"));
        assert_eq!(extension_for_mime("IMAGE/JPEG"), Some("jpg"));
        assert_eq!(extension_for_mime("image/webp"), Some("webp"));
        assert_eq!(extension_for_mime("image/heic"), None);
    }

    #[test]
    fn write_image_creates_blob_and_is_idempotent() {
        let (_tmp, vault) = fixture_vault();
        let bytes = b"\x89PNG\r\n\x1a\nfake-png-bytes-1";
        let first = write_image(&vault, bytes, "image/png").unwrap();
        assert_eq!(first.size_bytes as usize, bytes.len());
        assert!(first.sha256_hex.len() == 64);
        let written = vault
            .images_dir()
            .join(format!("{}.png", first.sha256_hex));
        assert!(written.exists());
        let mtime_before = fs::metadata(&written).unwrap().modified().unwrap();
        // Second call with same bytes is a no-op (existing file untouched).
        let second = write_image(&vault, bytes, "image/png").unwrap();
        assert_eq!(first, second);
        let mtime_after = fs::metadata(&written).unwrap().modified().unwrap();
        assert_eq!(mtime_before, mtime_after);
    }

    #[test]
    fn write_image_rejects_unknown_mime() {
        let (_tmp, vault) = fixture_vault();
        let err = write_image(&vault, b"x", "image/heic").unwrap_err();
        assert!(matches!(err, ImageErr::UnsupportedMime(_)));
    }

    #[test]
    fn write_image_rejects_too_large() {
        let (_tmp, vault) = fixture_vault();
        let bytes = vec![0u8; MAX_IMAGE_BYTES + 1];
        let err = write_image(&vault, &bytes, "image/png").unwrap_err();
        assert!(matches!(err, ImageErr::TooLarge { .. }));
    }

    #[test]
    fn read_image_round_trip() {
        let (_tmp, vault) = fixture_vault();
        let bytes = b"some bytes";
        let w = write_image(&vault, bytes, "image/webp").unwrap();
        let read = read_image(&vault, &w.relative_path).unwrap();
        assert_eq!(read, bytes);
    }

    #[test]
    fn read_image_missing_returns_not_found() {
        let (_tmp, vault) = fixture_vault();
        let err = read_image(&vault, Path::new(".operon/images/missing.png")).unwrap_err();
        assert!(matches!(err, ImageErr::NotFound(_)));
    }

    #[test]
    fn data_url_round_trips_png_bytes() {
        let (_tmp, vault) = fixture_vault();
        let bytes = b"\x89PNG\r\n\x1a\n";
        let w = write_image(&vault, bytes, "image/png").unwrap();
        let url = data_url_for_blob(&vault, &w.relative_path).expect("data url");
        assert!(url.starts_with("data:image/png;base64,"));
        // Round-trip the base64 portion via a known byte: 0x89 0x50 0x4e ...
        // -> "iVBORw0KGgo=" (canonical PNG header b64).
        assert!(url.ends_with("iVBORw0KGgo="));
    }

    #[test]
    fn data_url_returns_none_for_unknown_extension() {
        let (_tmp, vault) = fixture_vault();
        // Synthesise a path with a non-image extension to skip the
        // mime branch — the function returns None before touching disk.
        assert!(data_url_for_blob(&vault, Path::new(".operon/images/x.bin")).is_none());
    }
}
