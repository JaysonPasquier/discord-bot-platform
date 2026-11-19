use platform_core::ProductId;
use rusqlite::{params, OptionalExtension};

use crate::error::StoreError;
use crate::sqlite::{from_db, to_db, SqliteStore};
use crate::vault::Vault;

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS bot_credentials (
    product_id INTEGER NOT NULL,
    tier TEXT NOT NULL,
    client_id TEXT NOT NULL,
    bot_name TEXT NOT NULL,
    token_sealed BLOB NOT NULL,
    PRIMARY KEY (product_id, tier)
) STRICT;
CREATE UNIQUE INDEX IF NOT EXISTS bot_credentials_client ON bot_credentials (client_id);";

// Le jeton reste chiffré ici : seul `Vault` le déchiffre, au moment du déploiement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotCredential {
    pub client_id: String,
    pub bot_name: String,
    pub token_sealed: Vec<u8>,
}

// Texte lié au chiffrement : le blob ne s'ouvre que pour ce produit et cette version.
pub fn credential_context(product: ProductId, tier: &str) -> String {
    format!("bot-credential:{}:{tier}", product.0)
}

impl SqliteStore {
    pub fn save_credential(
        &self,
        product: ProductId,
        tier: &str,
        credential: &BotCredential,
    ) -> Result<(), StoreError> {
        self.conn
            .execute(
                "INSERT INTO bot_credentials (product_id, tier, client_id, bot_name, token_sealed)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (product_id, tier) DO UPDATE SET client_id = excluded.client_id,
                bot_name = excluded.bot_name, token_sealed = excluded.token_sealed",
                params![
                    to_db(product.0)?,
                    tier,
                    credential.client_id,
                    credential.bot_name,
                    credential.token_sealed
                ],
            )
            .map_err(|e| match e {
                rusqlite::Error::SqliteFailure(f, _)
                    if f.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    StoreError::Duplicate
                }
                other => other.into(),
            })?;
        Ok(())
    }

    // Re-chiffre tous les jetons de bots avec une nouvelle clé vault.
    // Atomique : commit global ou rollback complet.
    pub fn rotate_vault_key(&self, old: &Vault, new: &Vault) -> Result<usize, StoreError> {
        struct Row {
            product_id: i64,
            tier: String,
            token_sealed: Vec<u8>,
        }
        let rows: Vec<Row> = {
            let mut stmt = self
                .conn
                .prepare("SELECT product_id, tier, token_sealed FROM bot_credentials")?;
            let collected = stmt
                .query_map([], |r| {
                    Ok(Row {
                        product_id: r.get(0)?,
                        tier: r.get(1)?,
                        token_sealed: r.get(2)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            collected
        };
        let tx = self.conn.unchecked_transaction()?;
        let mut count = 0usize;
        for row in &rows {
            let product = ProductId(from_db(row.product_id)?);
            let ctx = credential_context(product, &row.tier);
            let plaintext = old.open(&row.token_sealed, &ctx)?;
            let new_sealed = new.seal(&plaintext, &ctx)?;
            tx.execute(
                "UPDATE bot_credentials SET token_sealed = ?1
                 WHERE product_id = ?2 AND tier = ?3",
                params![new_sealed, row.product_id, row.tier],
            )?;
            count += 1;
        }
        tx.commit()?;
        Ok(count)
    }

    pub fn credential(
        &self,
        product: ProductId,
        tier: &str,
    ) -> Result<Option<BotCredential>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT client_id, bot_name, token_sealed FROM bot_credentials
                 WHERE product_id = ?1 AND tier = ?2",
                params![to_db(product.0)?, tier],
                |r| {
                    Ok(BotCredential {
                        client_id: r.get(0)?,
                        bot_name: r.get(1)?,
                        token_sealed: r.get(2)?,
                    })
                },
            )
            .optional()?)
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

    fn credential(id: &str, blob: &[u8]) -> BotCredential {
        BotCredential {
            client_id: id.to_string(),
            bot_name: format!("bot {id}"),
            token_sealed: blob.to_vec(),
        }
    }

    #[test]
    fn a_credential_is_stored_per_product_and_tier() {
        let s = store();
        assert!(s
            .save_credential(ProductId(1), "free", &credential("11", b"a"))
            .is_ok());
        assert!(s
            .save_credential(ProductId(1), "premium", &credential("12", b"b"))
            .is_ok());
        assert!(s
            .save_credential(ProductId(2), "free", &credential("21", b"c"))
            .is_ok());
        assert_eq!(
            s.credential(ProductId(1), "free").ok().flatten(),
            Some(credential("11", b"a"))
        );
        assert_eq!(
            s.credential(ProductId(1), "premium").ok().flatten(),
            Some(credential("12", b"b"))
        );
        assert_eq!(
            s.credential(ProductId(2), "free").ok().flatten(),
            Some(credential("21", b"c"))
        );
        assert!(matches!(s.credential(ProductId(2), "premium"), Ok(None)));
        assert!(matches!(s.credential(ProductId(3), "free"), Ok(None)));
    }

    #[test]
    fn saving_again_replaces_every_field() {
        let s = store();
        assert!(s
            .save_credential(ProductId(1), "free", &credential("11", b"old"))
            .is_ok());
        assert!(s
            .save_credential(ProductId(1), "free", &credential("99", b"new"))
            .is_ok());
        assert_eq!(
            s.credential(ProductId(1), "free").ok().flatten(),
            Some(credential("99", b"new"))
        );
    }

    #[test]
    fn the_context_names_the_product_and_the_tier() {
        assert_eq!(
            credential_context(ProductId(7), "free"),
            "bot-credential:7:free"
        );
        assert_ne!(
            credential_context(ProductId(7), "free"),
            credential_context(ProductId(7), "premium")
        );
    }

    #[test]
    fn one_discord_application_cannot_serve_two_bots_or_two_versions() {
        let s = store();
        assert!(s
            .save_credential(ProductId(1), "free", &credential("11", b"a"))
            .is_ok());
        for (product, tier) in [(2, "free"), (1, "premium"), (2, "premium")] {
            assert!(matches!(
                s.save_credential(ProductId(product), tier, &credential("11", b"b")),
                Err(StoreError::Duplicate)
            ));
        }
        assert!(s
            .save_credential(ProductId(1), "free", &credential("11", b"again"))
            .is_ok());
        assert_eq!(
            s.credential(ProductId(1), "free").ok().flatten(),
            Some(credential("11", b"again"))
        );
        assert!(matches!(s.credential(ProductId(2), "free"), Ok(None)));
    }

    #[test]
    fn other_database_failures_are_not_reported_as_a_duplicate() {
        let s = store();
        assert!(s.conn.execute_batch("DROP TABLE bot_credentials").is_ok());
        assert!(matches!(
            s.save_credential(ProductId(1), "free", &credential("11", b"a")),
            Err(StoreError::Sqlite(_))
        ));
    }

    #[test]
    fn identifiers_above_i64_max_are_rejected() {
        let s = store();
        assert!(matches!(
            s.save_credential(ProductId(u64::MAX), "free", &credential("1", b"x")),
            Err(StoreError::IdOutOfRange)
        ));
    }
}
