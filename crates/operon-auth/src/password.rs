use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};

use crate::error::AuthError;

const M_COST: u32 = 19_456; // KiB
const T_COST: u32 = 2;
const P_COST: u32 = 1;

fn argon2() -> Argon2<'static> {
    let params = Params::new(M_COST, T_COST, P_COST, None).expect("argon2 params valid");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// Hash a cleartext password with Argon2id (OWASP-2024 parameters). Returns the
/// PHC-encoded hash (`$argon2id$v=19$m=...$...`) that includes salt + params.
pub fn hash(password: &str) -> Result<String, AuthError> {
    let salt = SaltString::generate(&mut OsRng);
    argon2()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AuthError::Hash(e.to_string()))
}

/// Verify a cleartext password against a previously stored PHC hash.
pub fn verify(password: &str, encoded: &str) -> Result<(), AuthError> {
    let parsed = PasswordHash::new(encoded).map_err(|e| AuthError::Hash(e.to_string()))?;
    argon2()
        .verify_password(password.as_bytes(), &parsed)
        .map_err(|_| AuthError::InvalidCredentials)
}
