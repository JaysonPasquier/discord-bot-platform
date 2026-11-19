use std::path::Path;

use platform_core::{Entitlement, GuildId, ProductId};
use rusqlite::{params, Connection, OptionalExtension};

use crate::error::StoreError;
use crate::token::TokenHash;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS entitlements (
    guild_id INTEGER NOT NULL,
    product_id INTEGER NOT NULL,
    paid_until INTEGER NOT NULL,
    revoked INTEGER NOT NULL,
    canceled INTEGER NOT NULL,
    PRIMARY KEY (guild_id, product_id)
) STRICT;
CREATE TABLE IF NOT EXISTS container_tokens (
    token_hash BLOB PRIMARY KEY,
    product_id INTEGER NOT NULL,
    container TEXT NOT NULL DEFAULT ''
) STRICT;
CREATE TABLE IF NOT EXISTS deploy_limits (
    key TEXT PRIMARY KEY,
    last_at INTEGER NOT NULL
) STRICT;";

pub struct SqliteStore {
    pub(crate) conn: Connection,
}

impl SqliteStore {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        Self::init(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self, StoreError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, StoreError> {
        conn.execute_batch(SCHEMA)?;
        conn.execute_batch(crate::accounts::SCHEMA)?;
        crate::accounts::migrate(&conn)?;
        conn.execute_batch(crate::catalog::SCHEMA)?;
        conn.execute_batch(crate::credentials::SCHEMA)?;
        conn.execute_batch(crate::env_vars::SCHEMA)?;
        conn.execute_batch(crate::guild_profiles::SCHEMA)?;
        conn.execute_batch(crate::listings::SCHEMA)?;
        conn.execute_batch(crate::sources::SCHEMA)?;
        conn.execute_batch(crate::notifications::SCHEMA)?;
        conn.execute_batch(crate::reports::SCHEMA)?;
        conn.execute_batch(crate::stats::SCHEMA)?;
        conn.execute_batch(crate::stripe::SCHEMA)?;
        conn.execute_batch(crate::api_tokens::SCHEMA)?;
        conn.execute_batch(crate::email_verification::SCHEMA)?;
        conn.execute_batch(crate::totp::SCHEMA)?;
        crate::catalog::migrate(&conn)?;
        crate::email_verification::migrate(&conn)?;
        Ok(Self { conn })
    }

    pub fn save_entitlement(&self, e: &Entitlement) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO entitlements (guild_id, product_id, paid_until, revoked, canceled)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (guild_id, product_id)
             DO UPDATE SET paid_until = excluded.paid_until, revoked = excluded.revoked,
                           canceled = excluded.canceled",
            params![
                to_db(e.guild.0)?,
                to_db(e.product.0)?,
                e.paid_until,
                e.revoked,
                e.canceled
            ],
        )?;
        Ok(())
    }

    pub fn entitlement(
        &self,
        guild: GuildId,
        product: ProductId,
    ) -> Result<Option<Entitlement>, StoreError> {
        let row = self
            .conn
            .query_row(
                "SELECT paid_until, revoked, canceled FROM entitlements
                 WHERE guild_id = ?1 AND product_id = ?2",
                params![to_db(guild.0)?, to_db(product.0)?],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, bool>(1)?,
                        r.get::<_, bool>(2)?,
                    ))
                },
            )
            .optional()?;
        Ok(row.map(|(paid_until, revoked, canceled)| Entitlement {
            guild,
            product,
            paid_until,
            revoked,
            canceled,
        }))
    }

    // Toutes les adhésions d'un produit, pour un total de serveurs premium réel (jamais une
    // estimation) sur le tableau de bord de son créateur.
    pub fn entitlements_of(&self, product: ProductId) -> Result<Vec<Entitlement>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT guild_id, paid_until, revoked, canceled FROM entitlements WHERE product_id = ?1",
        )?;
        let rows = stmt.query_map(params![to_db(product.0)?], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, bool>(2)?,
                r.get::<_, bool>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (guild, paid_until, revoked, canceled) = row?;
            out.push(Entitlement {
                guild: GuildId(from_db(guild)?),
                product,
                paid_until,
                revoked,
                canceled,
            });
        }
        Ok(out)
    }

    pub fn all_entitlements(&self) -> Result<Vec<Entitlement>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT guild_id, product_id, paid_until, revoked, canceled FROM entitlements ORDER BY product_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, bool>(3)?,
                r.get::<_, bool>(4)?,
            ))
        })?;
        rows.map(|row| {
            let (guild, product, paid_until, revoked, canceled) = row?;
            Ok(Entitlement {
                guild: GuildId(from_db(guild)?),
                product: ProductId(from_db(product)?),
                paid_until,
                revoked,
                canceled,
            })
        })
        .collect()
    }

    // Rate limit de déploiement persistant en base.
    // `Ok(Ok(()))` : autorisé. `Ok(Err(n))` : attendre n secondes. `Err` : erreur DB.
    pub fn check_and_record_deploy(
        &self,
        key: &str,
        now: i64,
        cooldown_secs: u32,
    ) -> Result<Result<(), u32>, StoreError> {
        // Purge des entrées trop anciennes pour libérer l'espace.
        let cutoff = now.saturating_sub(i64::from(cooldown_secs).saturating_mul(2));
        self.conn.execute(
            "DELETE FROM deploy_limits WHERE last_at < ?1",
            params![cutoff],
        )?;

        let last: Option<i64> = self
            .conn
            .query_row(
                "SELECT last_at FROM deploy_limits WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()?;

        if let Some(at) = last {
            let elapsed = now.saturating_sub(at);
            if elapsed >= 0 && elapsed < i64::from(cooldown_secs) {
                let remaining = i64::from(cooldown_secs) - elapsed;
                return Ok(Err(u32::try_from(remaining).unwrap_or(cooldown_secs)));
            }
        }

        self.conn.execute(
            "INSERT OR REPLACE INTO deploy_limits (key, last_at) VALUES (?1, ?2)",
            params![key, now],
        )?;
        Ok(Ok(()))
    }

    pub fn add_token(&self, hash: &TokenHash, product: ProductId) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO container_tokens (token_hash, product_id) VALUES (?1, ?2)",
            params![hash.as_slice(), to_db(product.0)?],
        )?;
        Ok(())
    }

    pub fn product_for_token(&self, hash: &TokenHash) -> Result<Option<ProductId>, StoreError> {
        let raw: Option<i64> = self
            .conn
            .query_row(
                "SELECT product_id FROM container_tokens WHERE token_hash = ?1",
                params![hash.as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        raw.map(|v| {
            u64::try_from(v)
                .map(ProductId)
                .map_err(|_| StoreError::IdOutOfRange)
        })
        .transpose()
    }
}

// SQLite n'a que des entiers signés 64 bits ; les identifiants Discord tiennent dans 63 bits.
pub(crate) fn to_db(id: u64) -> Result<i64, StoreError> {
    i64::try_from(id).map_err(|_| StoreError::IdOutOfRange)
}

pub(crate) fn from_db(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::IdOutOfRange)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::hash_token;

    fn store() -> SqliteStore {
        match SqliteStore::in_memory() {
            Ok(s) => s,
            Err(e) => panic!("cannot open in-memory store: {e}"),
        }
    }

    fn entitlement(guild: u64, product: u64, paid_until: i64, revoked: bool) -> Entitlement {
        Entitlement {
            guild: GuildId(guild),
            product: ProductId(product),
            paid_until,
            revoked,
            canceled: false,
        }
    }

    fn load(s: &SqliteStore, guild: u64, product: u64) -> Option<Entitlement> {
        match s.entitlement(GuildId(guild), ProductId(product)) {
            Ok(v) => v,
            Err(e) => panic!("read failed: {e}"),
        }
    }

    fn save(s: &SqliteStore, e: &Entitlement) {
        if let Err(err) = s.save_entitlement(e) {
            panic!("write failed: {err}");
        }
    }

    #[test]
    fn an_unknown_entitlement_is_none() {
        assert_eq!(load(&store(), 1, 2), None);
    }

    #[test]
    fn an_entitlement_round_trips() {
        let s = store();
        let e = entitlement(1, 2, 1_000, true);
        save(&s, &e);
        assert_eq!(load(&s, 1, 2), Some(e));
    }

    #[test]
    fn the_canceled_flag_round_trips_independently_of_revoked() {
        let s = store();
        let canceled_only = Entitlement {
            canceled: true,
            ..entitlement(1, 2, 1_000, false)
        };
        save(&s, &canceled_only);
        assert_eq!(load(&s, 1, 2), Some(canceled_only));
        let revoked_only = entitlement(3, 4, 1_000, true);
        save(&s, &revoked_only);
        assert_eq!(load(&s, 3, 4), Some(revoked_only));
        save(&s, &entitlement(1, 2, 1_000, false));
        assert_eq!(load(&s, 1, 2).map(|e| e.canceled), Some(false));
    }

    #[test]
    fn saving_again_updates_instead_of_duplicating() {
        let s = store();
        save(&s, &entitlement(1, 2, 1_000, false));
        save(&s, &entitlement(1, 2, 2_000, true));
        assert_eq!(load(&s, 1, 2), Some(entitlement(1, 2, 2_000, true)));
    }

    #[test]
    fn guild_and_product_are_independent_keys() {
        let s = store();
        save(&s, &entitlement(1, 2, 100, false));
        save(&s, &entitlement(1, 3, 200, false));
        save(&s, &entitlement(9, 2, 300, false));
        assert_eq!(load(&s, 1, 2).map(|e| e.paid_until), Some(100));
        assert_eq!(load(&s, 1, 3).map(|e| e.paid_until), Some(200));
        assert_eq!(load(&s, 9, 2).map(|e| e.paid_until), Some(300));
        assert_eq!(load(&s, 9, 3), None);
    }

    #[test]
    fn entitlements_of_a_product_lists_every_guild_and_only_that_product() {
        let s = store();
        save(&s, &entitlement(1, 2, 100, false));
        save(&s, &entitlement(9, 2, 300, false));
        save(&s, &entitlement(1, 3, 200, false));
        let Ok(mut rows) = s.entitlements_of(ProductId(2)) else {
            panic!("read failed");
        };
        rows.sort_by_key(|e| e.guild.0);
        assert_eq!(
            rows,
            vec![entitlement(1, 2, 100, false), entitlement(9, 2, 300, false)]
        );
    }

    #[test]
    fn entitlements_of_an_unknown_product_is_empty() {
        let s = store();
        save(&s, &entitlement(1, 2, 100, false));
        let Ok(rows) = s.entitlements_of(ProductId(99)) else {
            panic!("read failed");
        };
        assert!(rows.is_empty());
    }

    #[test]
    fn never_paid_survives_storage() {
        let s = store();
        let e = Entitlement::new(GuildId(1), ProductId(2));
        save(&s, &e);
        assert_eq!(load(&s, 1, 2), Some(e));
    }

    #[test]
    fn identifiers_above_i64_max_are_rejected() {
        let s = store();
        let too_big = entitlement(u64::MAX, 1, 0, false);
        assert!(matches!(
            s.save_entitlement(&too_big),
            Err(StoreError::IdOutOfRange)
        ));
        assert!(matches!(
            s.entitlement(GuildId(1), ProductId(u64::MAX)),
            Err(StoreError::IdOutOfRange)
        ));
        save(&s, &entitlement(i64::MAX as u64, 1, 5, false));
        assert_eq!(load(&s, i64::MAX as u64, 1).map(|e| e.paid_until), Some(5));
        assert!(matches!(
            s.save_entitlement(&entitlement(i64::MAX as u64 + 1, 1, 0, false)),
            Err(StoreError::IdOutOfRange)
        ));
    }

    #[test]
    fn a_token_resolves_to_its_product() {
        let s = store();
        let hash = hash_token("secret");
        assert!(s.add_token(&hash, ProductId(7)).is_ok());
        assert!(matches!(s.product_for_token(&hash), Ok(Some(ProductId(7)))));
    }

    #[test]
    fn an_unknown_token_resolves_to_nothing() {
        let s = store();
        assert!(s.add_token(&hash_token("secret"), ProductId(7)).is_ok());
        assert!(matches!(
            s.product_for_token(&hash_token("other")),
            Ok(None)
        ));
    }

    #[test]
    fn a_token_cannot_be_registered_twice() {
        let s = store();
        let hash = hash_token("secret");
        assert!(s.add_token(&hash, ProductId(7)).is_ok());
        assert!(s.add_token(&hash, ProductId(8)).is_err());
        assert!(matches!(s.product_for_token(&hash), Ok(Some(ProductId(7)))));
    }

    #[test]
    fn data_survives_reopening_the_file() {
        let path =
            std::env::temp_dir().join(format!("platform-store-test-{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let e = entitlement(1, 2, 1_000, false);
        {
            let Ok(s) = SqliteStore::open(&path) else {
                panic!("cannot create file store");
            };
            save(&s, &e);
            assert!(s.add_token(&hash_token("t"), ProductId(2)).is_ok());
        }
        let Ok(reopened) = SqliteStore::open(&path) else {
            panic!("cannot reopen file store");
        };
        assert_eq!(load(&reopened, 1, 2), Some(e));
        assert!(matches!(
            reopened.product_for_token(&hash_token("t")),
            Ok(Some(ProductId(2)))
        ));
        let _ = std::fs::remove_file(&path);
    }
}
