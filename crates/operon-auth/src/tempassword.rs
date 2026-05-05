use rand::Rng;

const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghjkmnpqrstuvwxyz23456789";

/// Generate a 12-char human-friendly temporary password (no easily-confused
/// characters: 0/O, 1/l/I removed).
pub fn generate() -> String {
    let mut rng = rand::thread_rng();
    (0..12)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}
