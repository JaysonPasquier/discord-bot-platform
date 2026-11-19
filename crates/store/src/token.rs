use sha2::{Digest, Sha256};

use crate::error::StoreError;

pub type TokenHash = [u8; 32];

pub fn generate_token() -> Result<String, StoreError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| StoreError::Random)?;
    Ok(to_hex(&bytes))
}

// Seul le hash est stocké : une base volée ne donne pas de jeton utilisable.
pub fn hash_token(token: &str) -> TokenHash {
    Sha256::digest(token.as_bytes()).into()
}

pub fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

// L'inverse de `to_hex` : utilisé pour retrouver le hash d'une session depuis l'identifiant que
// l'API a renvoyé à l'utilisateur. `None` si ce n'est pas de l'hexadécimal propre.
pub fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_uses_two_lowercase_digits_per_byte() {
        assert_eq!(to_hex(&[0x00, 0xff, 0x1a, 0xa1]), "00ff1aa1");
        assert_eq!(to_hex(&[]), "");
    }

    #[test]
    fn hex_decode_round_trips_through_to_hex() {
        assert_eq!(
            hex_decode(&to_hex(&[0x00, 0xff, 0x1a])),
            Some(vec![0x00, 0xff, 0x1a])
        );
    }

    #[test]
    fn hex_decode_refuses_odd_length_even_with_only_hex_digits() {
        assert_eq!(hex_decode("abc"), None);
    }

    #[test]
    fn hex_decode_refuses_non_hex_characters_even_at_even_length() {
        assert_eq!(hex_decode("zz"), None);
        assert_eq!(hex_decode("gg"), None);
    }

    #[test]
    fn hash_matches_the_sha256_test_vector() {
        assert_eq!(
            to_hex(&hash_token("abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn different_tokens_have_different_hashes() {
        assert_ne!(hash_token("a"), hash_token("b"));
        assert_eq!(hash_token("a"), hash_token("a"));
    }

    #[test]
    fn generated_tokens_are_64_hex_characters_and_unique() {
        let (Ok(a), Ok(b)) = (generate_token(), generate_token()) else {
            panic!("token generation failed");
        };
        assert_eq!(a.len(), 64);
        assert!(a
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)));
        assert_ne!(a, b);
    }
}
