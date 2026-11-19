use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};

use crate::error::StoreError;

const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

// Chiffre les jetons des bots des créateurs avant leur écriture en base. Une base
// volée sans la clé maître ne donne aucun jeton ; la clé n'est jamais écrite dans la base.
pub struct Vault {
    cipher: ChaCha20Poly1305,
}

impl Vault {
    pub fn new(key: &[u8; KEY_LEN]) -> Self {
        Self {
            cipher: ChaCha20Poly1305::new(key.into()),
        }
    }

    // `context` lie le message à son emplacement (produit, version) : un blob recopié dans
    // la ligne d'un autre bot ne se déchiffre pas.
    pub fn seal(&self, plaintext: &str, context: &str) -> Result<Vec<u8>, StoreError> {
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).map_err(|_| StoreError::Random)?;
        let payload = Payload {
            msg: plaintext.as_bytes(),
            aad: context.as_bytes(),
        };
        let sealed = self
            .cipher
            .encrypt(&Nonce::from(nonce), payload)
            .map_err(|_| StoreError::Vault)?;
        let mut blob = nonce.to_vec();
        blob.extend_from_slice(&sealed);
        Ok(blob)
    }

    pub fn open(&self, blob: &[u8], context: &str) -> Result<String, StoreError> {
        let (nonce, sealed) = blob.split_at_checked(NONCE_LEN).ok_or(StoreError::Vault)?;
        let nonce: [u8; NONCE_LEN] = nonce.try_into().map_err(|_| StoreError::Vault)?;
        let payload = Payload {
            msg: sealed,
            aad: context.as_bytes(),
        };
        let plain = self
            .cipher
            .decrypt(&Nonce::from(nonce), payload)
            .map_err(|_| StoreError::Vault)?;
        String::from_utf8(plain).map_err(|_| StoreError::Vault)
    }
}

// 64 chiffres hexadécimaux exactement : 256 bits.
pub fn parse_key(hex: &str) -> Option<[u8; KEY_LEN]> {
    if hex.len() != KEY_LEN * 2 {
        return None;
    }
    let digit = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    };
    let bytes = hex.as_bytes();
    let mut key = [0u8; KEY_LEN];
    for (i, slot) in key.iter_mut().enumerate() {
        let high = digit(*bytes.get(i * 2)?)?;
        let low = digit(*bytes.get(i * 2 + 1)?)?;
        *slot = high * 16 + low;
    }
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault(byte: u8) -> Vault {
        Vault::new(&[byte; KEY_LEN])
    }

    fn sealed(v: &Vault, text: &str, context: &str) -> Vec<u8> {
        match v.seal(text, context) {
            Ok(b) => b,
            Err(e) => panic!("seal failed: {e}"),
        }
    }

    #[test]
    fn a_sealed_secret_opens_back_to_the_same_text() {
        let v = vault(7);
        let blob = sealed(&v, "tok.en.value", "1:free");
        assert_eq!(
            v.open(&blob, "1:free").ok().as_deref(),
            Some("tok.en.value")
        );
    }

    #[test]
    fn the_blob_does_not_contain_the_secret() {
        let blob = sealed(&vault(7), "super-secret-token", "1:free");
        assert!(!blob.windows(6).any(|w| w == b"secret"));
        assert_eq!(blob.len(), NONCE_LEN + "super-secret-token".len() + 16);
    }

    #[test]
    fn sealing_twice_gives_different_blobs() {
        let v = vault(7);
        assert_ne!(sealed(&v, "same", "c"), sealed(&v, "same", "c"));
    }

    #[test]
    fn another_key_cannot_open_it() {
        let blob = sealed(&vault(1), "secret", "c");
        assert!(vault(2).open(&blob, "c").is_err());
    }

    #[test]
    fn another_context_cannot_open_it() {
        let v = vault(1);
        let blob = sealed(&v, "secret", "1:free");
        assert!(v.open(&blob, "1:premium").is_err());
        assert!(v.open(&blob, "2:free").is_err());
    }

    #[test]
    fn a_modified_blob_is_refused() {
        let v = vault(1);
        let blob = sealed(&v, "secret", "c");
        for index in [0, NONCE_LEN, blob.len() - 1] {
            let mut bad = blob.clone();
            bad[index] ^= 1;
            assert!(v.open(&bad, "c").is_err(), "byte {index}");
        }
    }

    #[test]
    fn truncated_blobs_are_refused_not_a_panic() {
        let v = vault(1);
        assert!(v.open(&[], "c").is_err());
        assert!(v.open(&[0; NONCE_LEN], "c").is_err());
        assert!(v.open(&[0; NONCE_LEN + 15], "c").is_err());
    }

    #[test]
    fn an_empty_secret_round_trips() {
        let v = vault(3);
        let blob = sealed(&v, "", "c");
        assert_eq!(v.open(&blob, "c").ok().as_deref(), Some(""));
    }

    #[test]
    fn keys_are_exactly_64_hex_digits() {
        assert_eq!(parse_key(&"ab".repeat(32)), Some([0xab; 32]));
        assert_eq!(parse_key(&"AB".repeat(32)), Some([0xab; 32]));
        let mut mixed = "00".repeat(31);
        mixed.push_str("1f");
        assert_eq!(parse_key(&mixed).map(|k| (k[0], k[31])), Some((0, 0x1f)));
        assert_eq!(parse_key(&"ab".repeat(31)), None);
        assert_eq!(parse_key(&"ab".repeat(33)), None);
        assert_eq!(parse_key(&format!("{}g0", "ab".repeat(31))), None);
        assert_eq!(parse_key(&format!("+{}", "a".repeat(63))), None);
        assert_eq!(parse_key(""), None);
    }
}
