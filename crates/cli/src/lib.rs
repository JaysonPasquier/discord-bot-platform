#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used)]

mod args;
mod commands;
mod deploy;
mod doctor;
mod error;
mod server;
mod webconfig;

pub use args::{parse, Command, Deploy, Invocation};
pub use commands::{execute, validate_file, validate_text};
pub use deploy::deploy_spec;
pub use doctor::{doctor_checks, has_failure, render, Check, Facts, Level};
pub use error::CliError;
pub use server::{serve, serve_all};
pub use webconfig::{web_settings, GitHubSettings, WebSettings};

use platform_core::Timestamp;

// Trois jours : valeur de départ, à affiner avec les prix.
pub const GRACE_SECS: u32 = 3 * 24 * 3600;

pub fn unix_now() -> Timestamp {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_is_a_plausible_unix_time_and_does_not_go_backwards() {
        let first = unix_now();
        assert!(first > 1_700_000_000, "{first}");
        assert!(unix_now() >= first);
    }

    #[test]
    fn grace_is_three_days() {
        assert_eq!(GRACE_SECS, 259_200);
    }
}
