use platform_core::{GuildId, ProductId, Timestamp};
use rusqlite::params;

use crate::error::StoreError;
use crate::sqlite::{from_db, to_db, SqliteStore};
use crate::token::TokenHash;

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS products (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    owner_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS subscriptions (
    guild_id INTEGER NOT NULL,
    product_id INTEGER NOT NULL,
    subscriber_id INTEGER NOT NULL,
    PRIMARY KEY (guild_id, product_id)
) STRICT;";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Product {
    pub id: ProductId,
    pub owner: u64,
    pub name: String,
    pub ceded: bool,
    pub suspended: bool,
}

// `ceded` et `suspended` ont été ajoutés après le premier schéma : migrations idempotentes.
pub(crate) fn migrate(conn: &rusqlite::Connection) -> Result<(), StoreError> {
    let has_ceded: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('products') WHERE name = 'ceded'")?
        .exists([])?;
    if !has_ceded {
        conn.execute_batch("ALTER TABLE products ADD COLUMN ceded INTEGER NOT NULL DEFAULT 0")?;
    }
    let has_suspended: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('products') WHERE name = 'suspended'")?
        .exists([])?;
    if !has_suspended {
        conn.execute_batch("ALTER TABLE products ADD COLUMN suspended INTEGER NOT NULL DEFAULT 0")?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subscription {
    pub guild: GuildId,
    pub product: ProductId,
}

fn product_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<(i64, i64, String, i64, i64)> {
    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
}

fn to_product(
    (id, owner, name, ceded, suspended): (i64, i64, String, i64, i64),
) -> Result<Product, StoreError> {
    Ok(Product {
        id: ProductId(from_db(id)?),
        owner: from_db(owner)?,
        name,
        ceded: ceded != 0,
        suspended: suspended != 0,
    })
}

impl SqliteStore {
    pub fn server_count_of(&self, owner: u64) -> Result<u64, StoreError> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT s.guild_id) FROM subscriptions s
             JOIN products p ON p.id = s.product_id
             WHERE p.owner_id = ?1",
            params![to_db(owner)?],
            |r| r.get(0),
        )?;
        from_db(count)
    }

    pub fn suspend_product(&self, id: ProductId) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE products SET suspended = 1 WHERE id = ?1",
            params![to_db(id.0)?],
        )?;
        Ok(())
    }

    pub fn unsuspend_product(&self, id: ProductId) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE products SET suspended = 0 WHERE id = ?1",
            params![to_db(id.0)?],
        )?;
        Ok(())
    }

    pub fn create_product(
        &self,
        owner: u64,
        name: &str,
        now: Timestamp,
    ) -> Result<ProductId, StoreError> {
        self.conn.execute(
            "INSERT INTO products (owner_id, name, created_at) VALUES (?1, ?2, ?3)",
            params![to_db(owner)?, name, now],
        )?;
        Ok(ProductId(from_db(self.conn.last_insert_rowid())?))
    }

    pub fn product(&self, id: ProductId) -> Result<Option<Product>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT id, owner_id, name, ceded, COALESCE(suspended, 0) FROM products WHERE id = ?1",
        )?;
        let mut rows = statement.query_map(params![to_db(id.0)?], product_from_row)?;
        rows.next().transpose()?.map(to_product).transpose()
    }

    pub fn products_of(&self, owner: u64) -> Result<Vec<Product>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT id, owner_id, name, ceded, COALESCE(suspended, 0) FROM products WHERE owner_id = ?1 ORDER BY id",
        )?;
        let rows = statement.query_map(params![to_db(owner)?], product_from_row)?;
        rows.map(|row| to_product(row?)).collect()
    }

    pub fn cede_product(&self, id: ProductId, owner: u64) -> Result<bool, StoreError> {
        let updated = self.conn.execute(
            "UPDATE products SET ceded = 1, owner_id = 0 WHERE id = ?1 AND owner_id = ?2 AND ceded = 0",
            params![to_db(id.0)?, to_db(owner)?],
        )?;
        Ok(updated > 0)
    }

    pub fn rename_product(&self, id: ProductId, name: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE products SET name = ?2 WHERE id = ?1",
            params![to_db(id.0)?, name],
        )?;
        Ok(())
    }

    // Efface le bot et tout ce qui s'y rattache, d'un seul bloc : jamais un bot à moitié supprimé.
    pub fn delete_product(&self, id: ProductId) -> Result<(), StoreError> {
        let product = to_db(id.0)?;
        let transaction = self.conn.unchecked_transaction()?;
        for table in [
            "deployments",
            "product_sources",
            "bot_credentials",
            "guild_profiles",
            "listings",
            "listing_commands",
            "listing_plans",
            "payments",
            "subscriber_snapshots",
            "guild_counts",
            "entitlements",
            "subscriptions",
            "container_tokens",
            "env_vars",
            "notification_webhooks",
            "reports",
        ] {
            transaction.execute(
                &format!("DELETE FROM {table} WHERE product_id = ?1"),
                params![product],
            )?;
        }
        transaction.execute("DELETE FROM products WHERE id = ?1", params![product])?;
        Ok(transaction.commit()?)
    }

    pub fn all_products(&self) -> Result<Vec<Product>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT id, owner_id, name, ceded, COALESCE(suspended, 0) FROM products ORDER BY id",
        )?;
        let rows = statement.query_map([], product_from_row)?;
        rows.map(|row| to_product(row?)).collect()
    }

    pub fn record_subscription(
        &self,
        guild: GuildId,
        product: ProductId,
        subscriber: u64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO subscriptions (guild_id, product_id, subscriber_id) VALUES (?1, ?2, ?3)
             ON CONFLICT (guild_id, product_id) DO UPDATE SET subscriber_id = excluded.subscriber_id",
            params![to_db(guild.0)?, to_db(product.0)?, to_db(subscriber)?],
        )?;
        Ok(())
    }

    pub fn subscriptions_of(&self, subscriber: u64) -> Result<Vec<Subscription>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT guild_id, product_id FROM subscriptions WHERE subscriber_id = ?1
             ORDER BY product_id, guild_id",
        )?;
        let rows = statement.query_map(params![to_db(subscriber)?], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })?;
        rows.map(|row| {
            let (guild, product) = row?;
            Ok(Subscription {
                guild: GuildId(from_db(guild)?),
                product: ProductId(from_db(product)?),
            })
        })
        .collect()
    }

    // Un déploiement remplace le jeton du container : l'ancien cesse de fonctionner et le
    // jeton en clair n'est jamais conservé, seulement injecté dans le container.
    pub fn replace_container_token(
        &self,
        container: &str,
        hash: &TokenHash,
        product: ProductId,
    ) -> Result<(), StoreError> {
        let transaction = self.conn.unchecked_transaction()?;
        transaction.execute(
            "DELETE FROM container_tokens WHERE container = ?1",
            params![container],
        )?;
        transaction.execute(
            "INSERT INTO container_tokens (token_hash, product_id, container) VALUES (?1, ?2, ?3)",
            params![hash.as_slice(), to_db(product.0)?, container],
        )?;
        Ok(transaction.commit()?)
    }
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

    fn create(s: &SqliteStore, owner: u64, name: &str) -> ProductId {
        match s.create_product(owner, name, 100) {
            Ok(id) => id,
            Err(e) => panic!("create failed: {e}"),
        }
    }

    fn names(products: Result<Vec<Product>, StoreError>) -> Vec<String> {
        match products {
            Ok(list) => list.into_iter().map(|p| p.name).collect(),
            Err(e) => panic!("read failed: {e}"),
        }
    }

    #[test]
    fn products_get_increasing_ids_and_are_readable() {
        let s = store();
        let a = create(&s, 1, "alpha");
        let b = create(&s, 1, "beta");
        assert!(b.0 > a.0);
        assert_eq!(
            s.product(a).ok().flatten(),
            Some(Product {
                id: a,
                owner: 1,
                name: "alpha".to_string(),
                ceded: false,
                suspended: false,
            })
        );
        assert!(matches!(s.product(ProductId(999)), Ok(None)));
    }

    #[test]
    fn renaming_changes_only_that_product() {
        let s = store();
        let a = create(&s, 1, "alpha");
        create(&s, 1, "beta");
        assert!(s.rename_product(a, "gamma").is_ok());
        assert_eq!(names(s.products_of(1)), ["gamma", "beta"]);
        assert_eq!(s.product(a).ok().flatten().map(|p| p.owner), Some(1));
    }

    #[test]
    fn deleting_a_product_removes_everything_that_belongs_to_it_and_nothing_else() {
        use platform_core::Entitlement;
        let s = store();
        let (a, b) = (create(&s, 1, "alpha"), create(&s, 1, "beta"));
        for p in [a, b] {
            assert!(s.record_subscription(GuildId(9), p, 5).is_ok());
            assert!(s.save_entitlement(&Entitlement::new(GuildId(9), p)).is_ok());
            assert!(s.add_token(&hash_token(&format!("t{}", p.0)), p).is_ok());
            assert!(s
                .create_deployment(p, "free", "abc", "main", "manual", 1)
                .is_ok());
        }
        assert!(s.delete_product(a).is_ok());
        assert!(matches!(s.product(a), Ok(None)));
        assert!(matches!(s.entitlement(GuildId(9), a), Ok(None)));
        assert!(matches!(s.deployments_of(a, 10), Ok(d) if d.is_empty()));
        assert!(matches!(s.product_for_token(&hash_token("t1")), Ok(None)));
        assert_eq!(names(s.products_of(1)), ["beta"]);
        assert!(matches!(s.entitlement(GuildId(9), b), Ok(Some(_))));
        assert!(matches!(s.deployments_of(b, 10), Ok(d) if d.len() == 1));
        assert!(matches!(
            s.product_for_token(&hash_token("t2")),
            Ok(Some(_))
        ));
        assert!(matches!(s.subscriptions_of(5), Ok(v) if v.len() == 1));
    }

    #[test]
    fn products_are_listed_per_owner_in_creation_order() {
        let s = store();
        create(&s, 1, "alpha");
        create(&s, 2, "gamma");
        create(&s, 1, "beta");
        assert_eq!(names(s.products_of(1)), ["alpha", "beta"]);
        assert_eq!(names(s.products_of(2)), ["gamma"]);
        assert!(names(s.products_of(3)).is_empty());
        assert_eq!(names(s.all_products()), ["alpha", "gamma", "beta"]);
    }

    #[test]
    fn subscriptions_are_listed_per_subscriber() {
        let s = store();
        assert!(s.record_subscription(GuildId(10), ProductId(1), 7).is_ok());
        assert!(s.record_subscription(GuildId(11), ProductId(1), 7).is_ok());
        assert!(s.record_subscription(GuildId(12), ProductId(2), 8).is_ok());
        let mine = s.subscriptions_of(7).unwrap_or_default();
        assert_eq!(
            mine,
            vec![
                Subscription {
                    guild: GuildId(10),
                    product: ProductId(1)
                },
                Subscription {
                    guild: GuildId(11),
                    product: ProductId(1)
                },
            ]
        );
        assert_eq!(s.subscriptions_of(8).map(|v| v.len()).ok(), Some(1));
        assert_eq!(s.subscriptions_of(9).map(|v| v.len()).ok(), Some(0));
    }

    #[test]
    fn a_new_subscriber_takes_over_the_guild_subscription() {
        let s = store();
        assert!(s.record_subscription(GuildId(10), ProductId(1), 7).is_ok());
        assert!(s.record_subscription(GuildId(10), ProductId(1), 8).is_ok());
        assert_eq!(s.subscriptions_of(7).map(|v| v.len()).ok(), Some(0));
        assert_eq!(s.subscriptions_of(8).map(|v| v.len()).ok(), Some(1));
    }

    #[test]
    fn replacing_a_container_token_invalidates_the_previous_one() {
        let s = store();
        let (old, new) = (hash_token("old"), hash_token("new"));
        assert!(s
            .replace_container_token("bot-1-free", &old, ProductId(1))
            .is_ok());
        assert!(matches!(s.product_for_token(&old), Ok(Some(ProductId(1)))));
        assert!(s
            .replace_container_token("bot-1-free", &new, ProductId(1))
            .is_ok());
        assert!(matches!(s.product_for_token(&old), Ok(None)));
        assert!(matches!(s.product_for_token(&new), Ok(Some(ProductId(1)))));
    }

    #[test]
    fn replacing_one_container_leaves_the_others_alone() {
        let s = store();
        let (free, premium) = (hash_token("free"), hash_token("premium"));
        assert!(s
            .replace_container_token("bot-1-free", &free, ProductId(1))
            .is_ok());
        assert!(s
            .replace_container_token("bot-1-premium", &premium, ProductId(1))
            .is_ok());
        assert!(s
            .replace_container_token("bot-1-free", &hash_token("free2"), ProductId(1))
            .is_ok());
        assert!(matches!(
            s.product_for_token(&premium),
            Ok(Some(ProductId(1)))
        ));
        assert!(matches!(s.product_for_token(&free), Ok(None)));
    }

    #[test]
    fn identifiers_above_i64_max_are_rejected() {
        let s = store();
        assert!(matches!(
            s.create_product(u64::MAX, "x", 0),
            Err(StoreError::IdOutOfRange)
        ));
        assert!(matches!(
            s.record_subscription(GuildId(1), ProductId(1), u64::MAX),
            Err(StoreError::IdOutOfRange)
        ));
    }
}
