// Vérification d'email et stockage sécurisé de l'adresse par utilisateur.
// Le code de vérification est un jeton aléatoire à usage unique, valable 24h.
// En production, le code est envoyé par email ; en développement, il est affiché directement.
use rusqlite::{params, OptionalExtension};

use crate::error::StoreError;
use crate::sqlite::SqliteStore;
use crate::token::{generate_token, hash_token, to_hex};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS email_verifications (
    user_id    INTEGER PRIMARY KEY,
    email      TEXT    NOT NULL,
    code_hash  TEXT    NOT NULL,
    expires_at INTEGER NOT NULL,
    verified   INTEGER NOT NULL DEFAULT 0
) STRICT;";

// Migration : colonne email_verified sur users (ajoutée si absente).
pub(crate) fn migrate(conn: &rusqlite::Connection) -> Result<(), StoreError> {
    let has: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('users') WHERE name = 'email'")?
        .exists([])?;
    if !has {
        conn.execute_batch(
            "ALTER TABLE users ADD COLUMN email TEXT;
             ALTER TABLE users ADD COLUMN email_verified INTEGER NOT NULL DEFAULT 0;",
        )?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailVerification {
    pub email: String,
    pub verified: bool,
}

impl SqliteStore {
    // Crée un code de vérification pour `email`, valable `ttl_secs` secondes.
    // Retourne le code en clair (à envoyer par email ou à afficher en dev).
    pub fn start_email_verification(
        &self,
        user_id: u64,
        email: &str,
        now: i64,
        ttl_secs: i64,
    ) -> Result<String, StoreError> {
        let raw = generate_token()?;
        let code = raw[..16].to_string(); // 16 hex chars suffisent pour un code à usage unique
        let hash = to_hex(&hash_token(&code));
        let expires = now + ttl_secs;
        self.conn.execute(
            "INSERT INTO email_verifications (user_id, email, code_hash, expires_at, verified)
             VALUES (?1, ?2, ?3, ?4, 0)
             ON CONFLICT (user_id) DO UPDATE SET email = excluded.email,
                code_hash = excluded.code_hash, expires_at = excluded.expires_at,
                verified = 0",
            params![user_id as i64, email, hash, expires],
        )?;
        Ok(code)
    }

    // Valide le code ; si correct et non expiré, marque l'email comme vérifié sur le compte.
    pub fn confirm_email_verification(
        &self,
        user_id: u64,
        code: &str,
        now: i64,
    ) -> Result<bool, StoreError> {
        let hash = to_hex(&hash_token(code));
        let row = self
            .conn
            .query_row(
                "SELECT email, expires_at FROM email_verifications
                 WHERE user_id = ?1 AND code_hash = ?2 AND verified = 0",
                params![user_id as i64, hash],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?;
        let Some((email, expires_at)) = row else {
            return Ok(false);
        };
        if now > expires_at {
            return Ok(false);
        }
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE email_verifications SET verified = 1 WHERE user_id = ?1",
            params![user_id as i64],
        )?;
        tx.execute(
            "UPDATE users SET email = ?1, email_verified = 1 WHERE discord_id = ?2",
            params![email, user_id as i64],
        )?;
        tx.commit()?;
        Ok(true)
    }

    // Récupère le statut email de l'utilisateur.
    pub fn email_status(&self, user_id: u64) -> Result<Option<EmailVerification>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT email, email_verified FROM users WHERE discord_id = ?1",
                params![user_id as i64],
                |r| {
                    Ok(EmailVerification {
                        email: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                        verified: r.get::<_, i64>(1).map(|v| v != 0).unwrap_or(false),
                    })
                },
            )
            .optional()?)
    }

    // En développement : retourne le code en attente pour cet utilisateur (pour l'afficher à l'écran).
    pub fn pending_verification_code(
        &self,
        user_id: u64,
        now: i64,
    ) -> Result<Option<String>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT code_hash FROM email_verifications
                 WHERE user_id = ?1 AND verified = 0 AND expires_at > ?2",
                params![user_id as i64, now],
                |r| r.get(0),
            )
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ok;

    fn store() -> SqliteStore {
        ok(SqliteStore::in_memory(), "in-memory store")
    }

    #[test]
    fn verification_flow_marks_email_verified() {
        let s = store();
        ok(
            s.conn.execute(
                "INSERT INTO users (discord_id, username) VALUES (1, 'alice')",
                [],
            ),
            "insert user",
        );
        let code = ok(
            s.start_email_verification(1, "alice@example.com", 1000, 86400),
            "start",
        );
        assert!(code.len() == 16);
        // Mauvais code
        assert!(!ok(
            s.confirm_email_verification(1, "badcode12345678", 1001),
            "bad code"
        ));
        // Code expiré
        assert!(!ok(
            s.confirm_email_verification(1, &code, 1_000_000),
            "expired"
        ));
        // Bon code
        assert!(ok(
            s.confirm_email_verification(1, &code, 1001),
            "good code"
        ));
        let status = match ok(s.email_status(1), "status") {
            Some(status) => status,
            None => panic!("expected a status"),
        };
        assert_eq!(status.email, "alice@example.com");
        assert!(status.verified);
    }

    #[test]
    fn restarting_verification_invalidates_old_code() {
        let s = store();
        ok(
            s.conn.execute(
                "INSERT INTO users (discord_id, username) VALUES (2, 'bob')",
                [],
            ),
            "insert user",
        );
        let old = ok(
            s.start_email_verification(2, "bob@example.com", 0, 86400),
            "first",
        );
        let new = ok(
            s.start_email_verification(2, "bob2@example.com", 0, 86400),
            "second",
        );
        assert_ne!(old, new);
        assert!(!ok(
            s.confirm_email_verification(2, &old, 100),
            "old code rejected"
        ));
        assert!(ok(
            s.confirm_email_verification(2, &new, 100),
            "new code accepted"
        ));
    }
}
