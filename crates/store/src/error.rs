use std::fmt;

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    IdOutOfRange,
    Random,
    Vault,
    Duplicate,
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(e) => write!(f, "sqlite error: {e}"),
            Self::IdOutOfRange => f.write_str("identifier does not fit in a signed 64-bit integer"),
            Self::Random => f.write_str("system random generator unavailable"),
            Self::Vault => f.write_str("a stored secret cannot be sealed or opened"),
            Self::Duplicate => f.write_str("this Discord application is already registered"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_name_the_problem() {
        assert!(StoreError::IdOutOfRange.to_string().contains("64-bit"));
        assert!(StoreError::Random.to_string().contains("random"));
        assert!(StoreError::Vault.to_string().contains("secret"));
        assert!(StoreError::Duplicate.to_string().contains("already"));
        let sqlite = StoreError::from(rusqlite::Error::InvalidQuery);
        assert!(sqlite.to_string().starts_with("sqlite error"));
    }
}
