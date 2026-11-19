// Ce qu'il faut garder pour les statistiques d'un bot : paiements simulés, un instantané du nombre
// d'abonnés par jour, et le dernier nombre de serveurs connu pour chaque version.
use platform_core::{ProductId, Timestamp};
use rusqlite::params;

use crate::error::StoreError;
use crate::sqlite::{to_db, SqliteStore};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS payments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    product_id INTEGER NOT NULL,
    guild_id INTEGER NOT NULL,
    plan_name TEXT NOT NULL,
    price_cents INTEGER NOT NULL,
    days INTEGER NOT NULL,
    paid_at INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS payments_by_product ON payments (product_id, paid_at);
CREATE TABLE IF NOT EXISTS subscriber_snapshots (
    product_id INTEGER NOT NULL,
    day INTEGER NOT NULL,
    subscribers INTEGER NOT NULL,
    PRIMARY KEY (product_id, day)
) STRICT;
CREATE TABLE IF NOT EXISTS guild_counts (
    product_id INTEGER NOT NULL,
    tier TEXT NOT NULL,
    guilds INTEGER NOT NULL,
    checked_at INTEGER NOT NULL,
    PRIMARY KEY (product_id, tier)
) STRICT;";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuildCount {
    pub tier: String,
    pub guilds: u64,
    pub checked_at: Timestamp,
}

impl SqliteStore {
    // Paiement simulé : aucun argent ne bouge, on garde seulement ce que la formule affichait.
    pub fn record_payment(
        &self,
        product: ProductId,
        guild: u64,
        plan_name: &str,
        price_cents: u32,
        days: u32,
        at: Timestamp,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO payments (product_id, guild_id, plan_name, price_cents, days, paid_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                to_db(product.0)?,
                to_db(guild)?,
                plan_name,
                price_cents,
                days,
                at
            ],
        )?;
        Ok(())
    }

    // Somme des paiements dans l'intervalle [from, to[.
    pub fn revenue_between(
        &self,
        product: ProductId,
        from: Timestamp,
        to: Timestamp,
    ) -> Result<u64, StoreError> {
        let cents: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(price_cents), 0) FROM payments
             WHERE product_id = ?1 AND paid_at >= ?2 AND paid_at < ?3",
            params![to_db(product.0)?, from, to],
            |r| r.get(0),
        )?;
        crate::sqlite::from_db(cents)
    }

    pub fn total_revenue_all(&self) -> Result<u64, StoreError> {
        let cents: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(price_cents), 0) FROM payments",
            [],
            |r| r.get(0),
        )?;
        crate::sqlite::from_db(cents)
    }

    pub fn revenue_all_since(&self, from: Timestamp) -> Result<u64, StoreError> {
        let cents: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(price_cents), 0) FROM payments WHERE paid_at >= ?1",
            params![from],
            |r| r.get(0),
        )?;
        crate::sqlite::from_db(cents)
    }

    pub fn record_snapshot(
        &self,
        product: ProductId,
        day: i64,
        subscribers: u64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO subscriber_snapshots (product_id, day, subscribers) VALUES (?1, ?2, ?3)
             ON CONFLICT (product_id, day) DO UPDATE SET subscribers = excluded.subscribers",
            params![to_db(product.0)?, day, to_db(subscribers)?],
        )?;
        Ok(())
    }

    pub fn snapshots_since(
        &self,
        product: ProductId,
        first_day: i64,
    ) -> Result<Vec<(i64, u64)>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT day, subscribers FROM subscriber_snapshots
             WHERE product_id = ?1 AND day >= ?2 ORDER BY day",
        )?;
        let rows = statement.query_map(params![to_db(product.0)?, first_day], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })?;
        rows.map(|row| {
            let (day, count) = row?;
            Ok((day, crate::sqlite::from_db(count)?))
        })
        .collect()
    }

    pub fn save_guild_count(
        &self,
        product: ProductId,
        tier: &str,
        guilds: u64,
        at: Timestamp,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO guild_counts (product_id, tier, guilds, checked_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (product_id, tier) DO UPDATE SET guilds = excluded.guilds,
                checked_at = excluded.checked_at",
            params![to_db(product.0)?, tier, to_db(guilds)?, at],
        )?;
        Ok(())
    }

    // Le moment de la dernière tentative est gardé même quand Discord n'a pas donné de nombre :
    // on ne le redemande pas à chaque affichage.
    pub fn touch_guild_count(
        &self,
        product: ProductId,
        tier: &str,
        at: Timestamp,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO guild_counts (product_id, tier, guilds, checked_at) VALUES (?1, ?2, -1, ?3)
             ON CONFLICT (product_id, tier) DO UPDATE SET checked_at = excluded.checked_at",
            params![to_db(product.0)?, tier, at],
        )?;
        Ok(())
    }

    // `guilds` vaut -1 en base tant que Discord n'a jamais donné de nombre : ces lignes sont ignorées ici,
    // sauf pour la date de vérification.
    pub fn guild_counts(&self, product: ProductId) -> Result<Vec<(GuildCount, bool)>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT tier, guilds, checked_at FROM guild_counts WHERE product_id = ?1 ORDER BY tier",
        )?;
        let rows = statement.query_map(params![to_db(product.0)?], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (tier, guilds, checked_at) = row?;
            let known = guilds >= 0;
            Ok((
                GuildCount {
                    tier,
                    guilds: u64::try_from(guilds).unwrap_or(0),
                    checked_at,
                },
                known,
            ))
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: ProductId = ProductId(1);

    fn store() -> SqliteStore {
        match SqliteStore::in_memory() {
            Ok(s) => s,
            Err(e) => panic!("cannot open in-memory store: {e}"),
        }
    }

    #[test]
    fn revenue_counts_one_product_and_a_half_open_interval() {
        let s = store();
        for (product, cents, at) in [(1, 500, 100), (1, 700, 200), (1, 900, 300), (2, 5000, 150)] {
            assert!(s
                .record_payment(ProductId(product), 9, "Pro", cents, 30, at)
                .is_ok());
        }
        assert_eq!(s.revenue_between(P, 100, 300).unwrap_or(0), 1200);
        assert_eq!(s.revenue_between(P, 0, 1000).unwrap_or(0), 2100);
        assert_eq!(s.revenue_between(P, 301, 1000).unwrap_or(9), 0);
    }

    #[test]
    fn a_day_has_one_snapshot_and_the_last_write_wins() {
        let s = store();
        assert!(s.record_snapshot(P, 10, 1).is_ok());
        assert!(s.record_snapshot(P, 10, 3).is_ok());
        assert!(s.record_snapshot(P, 11, 2).is_ok());
        assert!(s.record_snapshot(ProductId(2), 11, 8).is_ok());
        assert_eq!(
            s.snapshots_since(P, 0).unwrap_or_default(),
            [(10, 3), (11, 2)]
        );
        assert_eq!(s.snapshots_since(P, 11).unwrap_or_default(), [(11, 2)]);
    }

    #[test]
    fn a_failed_check_keeps_the_last_known_count_and_moves_the_date() {
        let s = store();
        assert!(s.touch_guild_count(P, "free", 5).is_ok());
        assert!(matches!(s.guild_counts(P).as_deref(), Ok([(c, false)]) if c.checked_at == 5));
        assert!(s.save_guild_count(P, "free", 42, 10).is_ok());
        assert!(s.touch_guild_count(P, "free", 20).is_ok());
        let counts = s.guild_counts(P).unwrap_or_default();
        assert_eq!(counts.len(), 1);
        assert_eq!(
            (counts[0].0.guilds, counts[0].0.checked_at, counts[0].1),
            (42, 20, true)
        );
    }
}
