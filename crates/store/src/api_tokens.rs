// Tokens API personnels : un utilisateur crée un jeton pour accéder à l'API REST.
// Seul le hash SHA-256 est stocké ; le jeton en clair n'est montré qu'une fois.
use rusqlite::params;

use crate::error::StoreError;
use crate::sqlite::SqliteStore;
use crate::token::{generate_token, hash_token, to_hex};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS api_tokens (
    id      INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL,
    name    TEXT    NOT NULL,
    hash    TEXT    NOT NULL UNIQUE,
    prefix  TEXT    NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;";

pub struct ApiToken {
    pub id: i64,
    pub name: String,
    pub prefix: String,
    pub created_at: i64,
}

impl SqliteStore {
    // Crée un jeton, retourne le jeton en clair (ne sera plus accessible ensuite).
    pub fn create_api_token(
        &self,
        user_id: u64,
        name: &str,
        now: i64,
    ) -> Result<String, StoreError> {
        let raw = generate_token()?;
        let full = format!("upl_{raw}");
        let hash = to_hex(&hash_token(&full));
        let prefix = &full[..7]; // "upl_" + 3 hex chars
        self.conn.execute(
            "INSERT INTO api_tokens (user_id, name, hash, prefix, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![user_id as i64, name, hash, prefix, now],
        )?;
        Ok(full)
    }

    pub fn list_api_tokens(&self, user_id: u64) -> Result<Vec<ApiToken>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, prefix, created_at FROM api_tokens WHERE user_id = ?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![user_id as i64], |r| {
            Ok(ApiToken {
                id: r.get(0)?,
                name: r.get(1)?,
                prefix: r.get(2)?,
                created_at: r.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn delete_api_token(&self, id: i64, user_id: u64) -> Result<(), StoreError> {
        self.conn.execute(
            "DELETE FROM api_tokens WHERE id = ?1 AND user_id = ?2",
            params![id, user_id as i64],
        )?;
        Ok(())
    }

    // Valide un Bearer token ; retourne le user_id si valide.
    pub fn validate_api_token(&self, token: &str) -> Result<Option<u64>, StoreError> {
        let hash = to_hex(&hash_token(token));
        let result = self.conn.query_row(
            "SELECT user_id FROM api_tokens WHERE hash = ?1",
            params![hash],
            |r| r.get::<_, i64>(0),
        );
        match result {
            Ok(uid) => Ok(Some(uid as u64)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(StoreError::from(e)),
        }
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
    fn create_and_validate_token() {
        let s = store();
        let token = ok(s.create_api_token(1, "test", 0), "create");
        assert!(token.starts_with("upl_"));
        let uid = ok(s.validate_api_token(&token), "validate");
        assert_eq!(uid, Some(1));
    }

    #[test]
    fn wrong_token_returns_none() {
        let s = store();
        let uid = ok(s.validate_api_token("upl_notavalidtoken"), "validate");
        assert_eq!(uid, None);
    }

    #[test]
    fn list_returns_tokens_for_user() {
        let s = store();
        ok(s.create_api_token(1, "ci", 100), "create");
        ok(s.create_api_token(1, "local", 200), "create");
        ok(s.create_api_token(2, "other", 300), "create");
        let tokens = ok(s.list_api_tokens(1), "list");
        assert_eq!(tokens.len(), 2);
    }

    #[test]
    fn delete_removes_only_owners_token() {
        let s = store();
        ok(s.create_api_token(1, "ci", 0), "create");
        let tokens = ok(s.list_api_tokens(1), "list");
        let id = tokens[0].id;
        ok(s.delete_api_token(id, 2), "delete by other user");
        // Token still there: wrong owner
        assert_eq!(ok(s.list_api_tokens(1), "list").len(), 1);
        ok(s.delete_api_token(id, 1), "delete by owner");
        assert_eq!(ok(s.list_api_tokens(1), "list").len(), 0);
    }
}
