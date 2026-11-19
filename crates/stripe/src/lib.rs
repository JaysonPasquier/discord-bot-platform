#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used)]

mod checkout;
mod error;
mod key;
mod webhook;

pub use checkout::{CheckoutParams, CheckoutSession, HttpStripe, NoStripe, Stripe};
pub use error::StripeError;
pub use key::{is_live_secret_key, is_test_secret_key, is_webhook_secret};
pub use webhook::{parse_event, verify_signature, EventData, StripeEvent};
