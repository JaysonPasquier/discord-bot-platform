#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used)]

mod accounts;
mod api_tokens;
mod catalog;
mod credentials;
mod email_verification;
mod env_vars;
mod error;
mod guild_profiles;
mod listings;
mod notifications;
mod reports;
mod sources;
mod sqlite;
mod stats;
mod stripe;
mod token;
pub mod totp;
mod vault;

pub use accounts::{ActiveSession, Session, UserProfile};
pub use api_tokens::ApiToken;
pub use catalog::{Product, Subscription};
pub use credentials::{credential_context, BotCredential};
pub use email_verification::EmailVerification;
pub use env_vars::{key_ok as env_key_ok, EnvVar, MAX_ENV_VARS, MAX_KEY_LEN, MAX_VALUE_LEN};
pub use error::StoreError;
pub use guild_profiles::GuildProfile;
pub use listings::{ImageKind, ListedCommand, Listing, Plan};
pub use reports::{NewReport, Report};
pub use sources::{DeployStatus, Deployment, Source, StoredInstallation, WatchedSource};
pub use sqlite::SqliteStore;
pub use stats::GuildCount;
pub use token::{generate_token, hash_token, hex_decode, to_hex, TokenHash};
pub use vault::{parse_key, Vault};

// Dépaquette un `Result` dans les tests, sans `unwrap()`/`expect()` (interdits par le deny ci-dessus).
#[cfg(test)]
pub(crate) fn ok<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(v) => v,
        Err(e) => panic!("{what}: {e}"),
    }
}
