#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used)]

mod domain;
mod model;
mod validate;

pub use domain::{check_domain, DomainError};
pub use model::{BotConfig, Entrypoint, Interval, Language, Plan, Premium};
pub use validate::{parse, valid_name, validate, ConfigError, Issue, Problem};
