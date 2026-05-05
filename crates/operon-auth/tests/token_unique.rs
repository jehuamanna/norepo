use operon_auth::session::{generate_token, hash_token};

#[test]
fn generate_token_is_43_chars_base64url() {
    let t = generate_token();
    assert_eq!(t.len(), 43);
    assert!(t
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
}

#[test]
fn two_tokens_differ() {
    assert_ne!(generate_token(), generate_token());
}

#[test]
fn hash_token_is_deterministic_64_hex() {
    let a = hash_token("abc");
    let b = hash_token("abc");
    assert_eq!(a, b);
    assert_eq!(a.len(), 64);
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
}
