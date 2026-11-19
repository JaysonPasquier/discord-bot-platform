#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used)]

mod client;
mod encoding;
mod error;
mod jwt;
mod types;
mod webhook;

pub use jwt::check_private_key;

pub use client::{GitHub, GitHubApp, HttpGitHub, NoGitHub};
pub use error::GitHubError;
pub use types::{valid_branch, valid_repo, valid_sha, Branch, Installation, Repo};
pub use webhook::{parse_push, verify_signature, PushEvent};
