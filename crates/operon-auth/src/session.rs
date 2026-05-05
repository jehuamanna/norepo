use base64::Engine;
use rand::RngCore;
use sha2::{Digest, Sha256};

/// Generate a fresh 32-byte random token, base64url-encoded (43 chars, no padding).
pub fn generate_token() -> String {
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

/// SHA-256 the token bytes, return hex (64 chars) for storage.
pub fn hash_token(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex::encode(h.finalize())
}
