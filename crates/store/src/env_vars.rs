use rusqlite::{params, OptionalExtension};

use crate::error::StoreError;
use crate::sqlite::{to_db, SqliteStore};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS env_vars (
    product_id INTEGER NOT NULL,
    tier TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (product_id, tier, key)
) STRICT;";

pub const MAX_ENV_VARS: usize = 32;
pub const MAX_KEY_LEN: usize = 64;
pub const MAX_VALUE_LEN: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

pub fn key_ok(key: &str) -> bool {
    if key.is_empty() || key.len() > MAX_KEY_LEN {
        return false;
    }
    let mut chars = key.chars();
    let first = chars.next().unwrap_or('\0');
    // Variable d'environnement : lettre ou underscore en premier, puis lettres, chiffres, underscores.
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl SqliteStore {
    pub fn env_vars(&self, product: u64, tier: &str) -> Result<Vec<EnvVar>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT key, value FROM env_vars WHERE product_id = ?1 AND tier = ?2 ORDER BY key",
        )?;
        let rows = stmt.query_map(params![to_db(product)?, tier], |r| {
            Ok(EnvVar {
                key: r.get(0)?,
                value: r.get(1)?,
            })
        })?;
        rows.map(|r| Ok(r?)).collect()
    }

    pub fn set_env_var(
        &self,
        product: u64,
        tier: &str,
        key: &str,
        value: &str,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO env_vars (product_id, tier, key, value) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (product_id, tier, key) DO UPDATE SET value = excluded.value",
            params![to_db(product)?, tier, key, value],
        )?;
        Ok(())
    }

    pub fn delete_env_var(&self, product: u64, tier: &str, key: &str) -> Result<bool, StoreError> {
        let n = self.conn.execute(
            "DELETE FROM env_vars WHERE product_id = ?1 AND tier = ?2 AND key = ?3",
            params![to_db(product)?, tier, key],
        )?;
        Ok(n > 0)
    }

    pub fn count_env_vars(&self, product: u64, tier: &str) -> Result<usize, StoreError> {
        let n: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM env_vars WHERE product_id = ?1 AND tier = ?2",
                params![to_db(product)?, tier],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        Ok(usize::try_from(n).unwrap_or(0))
    }

    // Pour le moteur de déploiement : charge les variables d'une version.
    pub fn env_vars_for_launch(
        &self,
        product: u64,
        tier: &str,
    ) -> Result<Vec<(String, String)>, StoreError> {
        Ok(self
            .env_vars(product, tier)?
            .into_iter()
            .map(|e| (e.key, e.value))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> SqliteStore {
        match SqliteStore::in_memory() {
            Ok(s) => s,
            Err(e) => panic!("cannot open in-memory store: {e}"),
        }
    }

    fn vars(s: &SqliteStore, product: u64, tier: &str) -> Vec<EnvVar> {
        match s.env_vars(product, tier) {
            Ok(v) => v,
            Err(e) => panic!("env_vars failed: {e}"),
        }
    }

    fn set(s: &SqliteStore, product: u64, tier: &str, key: &str, value: &str) {
        if let Err(e) = s.set_env_var(product, tier, key, value) {
            panic!("set_env_var failed: {e}");
        }
    }

    fn delete(s: &SqliteStore, product: u64, tier: &str, key: &str) -> bool {
        match s.delete_env_var(product, tier, key) {
            Ok(deleted) => deleted,
            Err(e) => panic!("delete_env_var failed: {e}"),
        }
    }

    #[test]
    fn key_validation_accepts_valid_names() {
        for k in ["FOO", "foo_bar", "_PRIVATE", "X1", "a"] {
            assert!(key_ok(k), "{k}");
        }
    }

    #[test]
    fn key_validation_rejects_invalid_names() {
        for k in ["", "1START", "has space", "has-dash", "has.dot"] {
            assert!(!key_ok(k), "{k}");
        }
    }

    #[test]
    fn set_get_delete_round_trips() {
        let s = store();
        set(&s, 1, "free", "KEY", "value");
        assert_eq!(
            vars(&s, 1, "free"),
            vec![EnvVar {
                key: "KEY".into(),
                value: "value".into()
            }]
        );
        assert!(delete(&s, 1, "free", "KEY"));
        assert!(vars(&s, 1, "free").is_empty());
    }

    #[test]
    fn set_overwrites_the_same_key() {
        let s = store();
        set(&s, 1, "free", "K", "v1");
        set(&s, 1, "free", "K", "v2");
        assert_eq!(vars(&s, 1, "free")[0].value, "v2");
    }

    #[test]
    fn delete_missing_key_returns_false() {
        let s = store();
        assert!(!delete(&s, 1, "free", "NOPE"));
    }

    #[test]
    fn vars_are_scoped_by_product_and_tier() {
        let s = store();
        set(&s, 1, "free", "K", "a");
        set(&s, 1, "premium", "K", "b");
        set(&s, 2, "free", "K", "c");
        assert_eq!(vars(&s, 1, "free")[0].value, "a");
        assert_eq!(vars(&s, 1, "premium")[0].value, "b");
        assert_eq!(vars(&s, 2, "free")[0].value, "c");
    }
}
