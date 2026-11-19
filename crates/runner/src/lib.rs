#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used)]

mod args;
mod bot;
mod build;
mod exec;
mod spec;

pub use args::docker_run_args;
pub use bot::{bot_spec, container_name, tier_name, BotParams};
pub use build::{docker_build_args, BuildSpec};
pub use exec::{ContainerState, Docker, RunError};
pub use spec::{RunSpec, SpecError, Tier};
