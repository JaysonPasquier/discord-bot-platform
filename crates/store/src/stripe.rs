// Ce que le webhook Stripe a besoin de retenir : un événement déjà traité ne doit jamais
// s'appliquer deux fois (webhook rejoué), et un litige ne porte que l'identifiant du PaymentIntent,
// jamais guild/produit directement : on garde la correspondance faite à la session de paiement.
use platform_core::{GuildId, ProductId, Timestamp};
use rusqlite::{params, OptionalExtension};

use crate::error::StoreError;
use crate::sqlite::{from_db, to_db, SqliteStore};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS stripe_events (
    event_id TEXT PRIMARY KEY,
    received_at INTEGER NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS stripe_intents (
    payment_intent_id TEXT PRIMARY KEY,
    guild_id INTEGER NOT NULL,
    product_id INTEGER NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;";

impl SqliteStore {
    // `true` si l'événement est nouveau (et doit être traité) ; `false` s'il a déjà été vu.
    pub fn mark_stripe_event_seen(
        &self,
        event_id: &str,
        now: Timestamp,
    ) -> Result<bool, StoreError> {
        let changed = self.conn.execute(
            "INSERT OR IGNORE INTO stripe_events (event_id, received_at) VALUES (?1, ?2)",
            params![event_id, now],
        )?;
        Ok(changed > 0)
    }

    pub fn remember_stripe_intent(
        &self,
        payment_intent_id: &str,
        guild: GuildId,
        product: ProductId,
        now: Timestamp,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO stripe_intents (payment_intent_id, guild_id, product_id, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (payment_intent_id) DO NOTHING",
            params![payment_intent_id, to_db(guild.0)?, to_db(product.0)?, now],
        )?;
        Ok(())
    }

    pub fn stripe_intent(
        &self,
        payment_intent_id: &str,
    ) -> Result<Option<(GuildId, ProductId)>, StoreError> {
        let row = self
            .conn
            .query_row(
                "SELECT guild_id, product_id FROM stripe_intents WHERE payment_intent_id = ?1",
                params![payment_intent_id],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?;
        row.map(|(guild, product)| Ok((GuildId(from_db(guild)?), ProductId(from_db(product)?))))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: GuildId = GuildId(9);
    const P: ProductId = ProductId(1);

    fn store() -> SqliteStore {
        match SqliteStore::in_memory() {
            Ok(s) => s,
            Err(e) => panic!("cannot open in-memory store: {e}"),
        }
    }

    #[test]
    fn an_event_is_seen_only_once() {
        let s = store();
        assert!(s.mark_stripe_event_seen("evt_1", 100).unwrap_or(false));
        assert!(!s.mark_stripe_event_seen("evt_1", 200).unwrap_or(true));
        assert!(s.mark_stripe_event_seen("evt_2", 100).unwrap_or(false));
    }

    #[test]
    fn a_payment_intent_resolves_back_to_its_guild_and_product() {
        let s = store();
        assert!(s.remember_stripe_intent("pi_1", G, P, 100).is_ok());
        assert_eq!(s.stripe_intent("pi_1").unwrap_or(None), Some((G, P)));
        assert_eq!(s.stripe_intent("pi_unknown").unwrap_or(Some((G, P))), None);
        // Un deuxième enregistrement du même intent ne change rien (idempotent).
        assert!(s
            .remember_stripe_intent("pi_1", GuildId(999), ProductId(2), 200)
            .is_ok());
        assert_eq!(s.stripe_intent("pi_1").unwrap_or(None), Some((G, P)));
    }
}
