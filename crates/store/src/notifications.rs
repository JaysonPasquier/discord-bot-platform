use rusqlite::{params, OptionalExtension};

use crate::error::StoreError;
use crate::sqlite::{to_db, SqliteStore};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS notification_webhooks (
    product_id INTEGER PRIMARY KEY,
    webhook_url TEXT NOT NULL
) STRICT;";

impl SqliteStore {
    pub fn notification_webhook(&self, product: u64) -> Result<Option<String>, StoreError> {
        self.conn
            .query_row(
                "SELECT webhook_url FROM notification_webhooks WHERE product_id = ?1",
                params![to_db(product)?],
                |r| r.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn set_notification_webhook(&self, product: u64, url: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO notification_webhooks (product_id, webhook_url) VALUES (?1, ?2)
             ON CONFLICT (product_id) DO UPDATE SET webhook_url = excluded.webhook_url",
            params![to_db(product)?, url],
        )?;
        Ok(())
    }

    pub fn delete_notification_webhook(&self, product: u64) -> Result<(), StoreError> {
        self.conn.execute(
            "DELETE FROM notification_webhooks WHERE product_id = ?1",
            params![to_db(product)?],
        )?;
        Ok(())
    }
}
