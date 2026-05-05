use operon_auth::password::{hash, verify};

#[test]
fn hash_then_verify_succeeds() {
    let h = hash("correct-horse-battery-staple").unwrap();
    verify("correct-horse-battery-staple", &h).unwrap();
}

#[test]
fn verify_with_wrong_password_fails() {
    let h = hash("right").unwrap();
    let err = verify("wrong", &h).unwrap_err();
    matches!(err, operon_auth::AuthError::InvalidCredentials);
}

#[test]
fn two_hashes_of_same_password_differ() {
    let a = hash("same").unwrap();
    let b = hash("same").unwrap();
    assert_ne!(a, b);
}

#[test]
fn hash_format_is_phc() {
    let h = hash("hi").unwrap();
    assert!(h.starts_with("$argon2id$"));
}
