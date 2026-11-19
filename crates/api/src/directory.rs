use std::sync::Mutex;

use platform_core::{Entitlement, GuildId, ProductId};
use platform_store::{hash_token, SqliteStore, StoreError};

// Le détail d'une panne ne sort pas de l'API : l'appelant reçoit 503, jamais une erreur interne.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectoryError;

pub trait Directory: Send + Sync + 'static {
    fn product_for_token(&self, token: &str) -> Result<Option<ProductId>, DirectoryError>;

    fn entitlement(
        &self,
        guild: GuildId,
        product: ProductId,
    ) -> Result<Option<Entitlement>, DirectoryError>;
}

pub struct SqliteDirectory {
    store: Mutex<SqliteStore>,
}

impl SqliteDirectory {
    pub fn new(store: SqliteStore) -> Self {
        Self {
            store: Mutex::new(store),
        }
    }

    fn with_store<T>(
        &self,
        f: impl FnOnce(&SqliteStore) -> Result<T, StoreError>,
    ) -> Result<T, DirectoryError> {
        let guard = self.store.lock().map_err(|_| DirectoryError)?;
        f(&guard).map_err(|_| DirectoryError)
    }
}

impl Directory for SqliteDirectory {
    fn product_for_token(&self, token: &str) -> Result<Option<ProductId>, DirectoryError> {
        self.with_store(|s| s.product_for_token(&hash_token(token)))
    }

    fn entitlement(
        &self,
        guild: GuildId,
        product: ProductId,
    ) -> Result<Option<Entitlement>, DirectoryError> {
        self.with_store(|s| s.entitlement(guild, product))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory() -> (SqliteDirectory, SqliteStore) {
        let (Ok(a), Ok(b)) = (SqliteStore::in_memory(), SqliteStore::in_memory()) else {
            panic!("cannot open in-memory store");
        };
        (SqliteDirectory::new(a), b)
    }

    #[test]
    fn an_empty_store_knows_nothing() {
        let (d, _) = directory();
        assert_eq!(d.product_for_token("t"), Ok(None));
        assert_eq!(d.entitlement(GuildId(1), ProductId(2)), Ok(None));
    }

    #[test]
    fn data_written_to_the_store_is_visible_through_the_directory() {
        let Ok(store) = SqliteStore::in_memory() else {
            panic!("cannot open in-memory store");
        };
        let e = Entitlement {
            guild: GuildId(1),
            product: ProductId(2),
            paid_until: 500,
            revoked: false,
            canceled: false,
        };
        assert!(store.save_entitlement(&e).is_ok());
        assert!(store.add_token(&hash_token("secret"), ProductId(2)).is_ok());
        let d = SqliteDirectory::new(store);
        assert_eq!(d.product_for_token("secret"), Ok(Some(ProductId(2))));
        assert_eq!(d.product_for_token("other"), Ok(None));
        assert_eq!(d.entitlement(GuildId(1), ProductId(2)), Ok(Some(e)));
    }

    #[test]
    fn a_store_failure_becomes_a_directory_error() {
        let (d, _) = directory();
        assert_eq!(
            d.entitlement(GuildId(u64::MAX), ProductId(2)),
            Err(DirectoryError)
        );
    }
}
