// TOTP (RFC 6238) avec HMAC-SHA256. Compatible avec Aegis, Authy et tout client RFC 6238.
// Le secret est 20 octets aléatoires stockés en clair dans la DB (en mémoire ils sont en clair ;
// la base n'est pas chiffrée — le TOTP protège contre le vol de mot de passe, pas de DB).
use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::error::StoreError;
use crate::sqlite::SqliteStore;

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS totp_secrets (
    user_id    INTEGER PRIMARY KEY,
    secret     BLOB    NOT NULL,
    enabled    INTEGER NOT NULL DEFAULT 0
) STRICT;";

const SECRET_BYTES: usize = 20;
const STEP: i64 = 30;
const DIGITS: u32 = 6;

// HMAC-SHA256 implémenté manuellement (pas de dépendance externe nécessaire).
fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        let h: [u8; 32] = Sha256::digest(key).into();
        k[..32].copy_from_slice(&h);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let ipad: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    let opad: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    let mut inner_input = Vec::with_capacity(BLOCK + data.len());
    inner_input.extend_from_slice(&ipad);
    inner_input.extend_from_slice(data);
    let inner: [u8; 32] = Sha256::digest(&inner_input).into();
    let mut outer_input = Vec::with_capacity(BLOCK + 32);
    outer_input.extend_from_slice(&opad);
    outer_input.extend_from_slice(&inner);
    Sha256::digest(&outer_input).into()
}

// Calcule le code TOTP pour un compteur donné (généralement floor(now/30)).
fn hotp(secret: &[u8], counter: i64) -> u32 {
    let msg = counter.to_be_bytes();
    let mac = hmac_sha256(secret, &msg);
    // RFC 4226 dynamic truncation, adapté pour 32 octets.
    let offset = (mac[31] & 0x0f) as usize;
    let code = u32::from_be_bytes([
        mac[offset] & 0x7f,
        mac[offset + 1],
        mac[offset + 2],
        mac[offset + 3],
    ]);
    code % 10u32.pow(DIGITS)
}

pub fn totp_now(secret: &[u8], now: i64) -> u32 {
    hotp(secret, now / STEP)
}

// Vérifie le code en acceptant une fenêtre de ±1 pas (décalage d'horloge).
pub fn verify_totp(secret: &[u8], code: u32, now: i64) -> bool {
    let counter = now / STEP;
    for delta in [-1i64, 0, 1] {
        if hotp(secret, counter + delta) == code {
            return true;
        }
    }
    false
}

// Encode base32 (RFC 4648) sans padding, pour le format otpauth://.
pub fn base32_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::new();
    let mut buf: u64 = 0;
    let mut bits: u32 = 0;
    for &byte in bytes {
        buf = (buf << 8) | u64::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buf >> bits) & 0x1f) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buf << (5 - bits)) & 0x1f) as usize] as char);
    }
    out
}

impl SqliteStore {
    // Génère un secret TOTP pour l'utilisateur (non encore activé).
    pub fn create_totp_secret(&self, user_id: u64) -> Result<Vec<u8>, StoreError> {
        let mut secret = vec![0u8; SECRET_BYTES];
        getrandom::fill(&mut secret).map_err(|_| StoreError::Random)?;
        self.conn.execute(
            "INSERT INTO totp_secrets (user_id, secret, enabled) VALUES (?1, ?2, 0)
             ON CONFLICT (user_id) DO UPDATE SET secret = excluded.secret, enabled = 0",
            params![user_id as i64, secret],
        )?;
        Ok(secret)
    }

    // Active le TOTP si le code fourni correspond au secret en attente.
    pub fn enable_totp(&self, user_id: u64, code: u32, now: i64) -> Result<bool, StoreError> {
        let secret = self.totp_secret_pending(user_id)?;
        let Some(secret) = secret else {
            return Ok(false);
        };
        if !verify_totp(&secret, code, now) {
            return Ok(false);
        }
        self.conn.execute(
            "UPDATE totp_secrets SET enabled = 1 WHERE user_id = ?1",
            params![user_id as i64],
        )?;
        Ok(true)
    }

    // Désactive le TOTP (nécessite de re-vérifier le code courant).
    pub fn disable_totp(&self, user_id: u64, code: u32, now: i64) -> Result<bool, StoreError> {
        let Some((secret, enabled)) = self.totp_row(user_id)? else {
            return Ok(false);
        };
        if !enabled || !verify_totp(&secret, code, now) {
            return Ok(false);
        }
        self.conn.execute(
            "DELETE FROM totp_secrets WHERE user_id = ?1",
            params![user_id as i64],
        )?;
        Ok(true)
    }

    // Vérifie un code TOTP pour l'authentification (uniquement si TOTP activé).
    pub fn verify_totp_code(&self, user_id: u64, code: u32, now: i64) -> Result<bool, StoreError> {
        let Some((secret, enabled)) = self.totp_row(user_id)? else {
            return Ok(false);
        };
        if !enabled {
            return Ok(false);
        }
        Ok(verify_totp(&secret, code, now))
    }

    pub fn totp_enabled(&self, user_id: u64) -> Result<bool, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT enabled FROM totp_secrets WHERE user_id = ?1",
                params![user_id as i64],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .map(|v| v != 0)
            .unwrap_or(false))
    }

    fn totp_row(&self, user_id: u64) -> Result<Option<(Vec<u8>, bool)>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT secret, enabled FROM totp_secrets WHERE user_id = ?1",
                params![user_id as i64],
                |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1).map(|v| v != 0)?)),
            )
            .optional()?)
    }

    fn totp_secret_pending(&self, user_id: u64) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT secret FROM totp_secrets WHERE user_id = ?1 AND enabled = 0",
                params![user_id as i64],
                |r| r.get(0),
            )
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totp_generates_6_digit_codes() {
        let secret = b"testsecretsecret";
        for now in [0i64, 30, 60, 1_700_000_000] {
            let code = totp_now(secret, now);
            assert!(code < 1_000_000, "code={code} at now={now}");
        }
    }

    #[test]
    fn verify_accepts_current_and_adjacent_steps() {
        let secret = b"testsecretsecret12";
        let now = 1_700_000_015i64;
        let code = totp_now(secret, now);
        assert!(verify_totp(secret, code, now));
        // Fenêtre ±1 pas
        assert!(verify_totp(secret, code, now - 30));
        assert!(verify_totp(secret, code, now + 30));
        // Hors fenêtre
        assert!(!verify_totp(secret, code + 1, now));
        assert!(!verify_totp(secret, 999_999, now));
    }

    #[test]
    fn base32_encodes_known_values() {
        // RFC 4648 test vector: "foo" -> "MZXW6"
        assert_eq!(base32_encode(b"foo"), "MZXW6");
        // vide
        assert_eq!(base32_encode(b""), "");
    }

    use crate::ok;

    fn store() -> SqliteStore {
        ok(SqliteStore::in_memory(), "in-memory store")
    }

    #[test]
    fn totp_lifecycle_create_enable_disable() {
        let s = store();
        ok(
            s.conn.execute(
                "INSERT INTO users (discord_id, username) VALUES (1, 'alice')",
                [],
            ),
            "insert",
        );
        let secret = ok(s.create_totp_secret(1), "create");
        assert!(!ok(s.totp_enabled(1), "enabled?"));
        // Mauvais code lors de l'activation
        assert!(!ok(s.enable_totp(1, 999_998, 0), "wrong enable"));
        // Bon code
        let code = totp_now(&secret, 0);
        assert!(ok(s.enable_totp(1, code, 0), "enable ok"));
        assert!(ok(s.totp_enabled(1), "enabled?"));
        // Désactivation avec mauvais code
        assert!(!ok(s.disable_totp(1, code + 1, 0), "wrong disable"));
        // Désactivation avec bon code
        let code2 = totp_now(&secret, 100);
        assert!(ok(s.disable_totp(1, code2, 100), "disable ok"));
        assert!(!ok(s.totp_enabled(1), "disabled?"));
    }
}
