use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StripeError {
    NotConfigured,
    Unauthorized,
    InvalidRequest(String),
    Parse,
    Http(String),
}

impl fmt::Display for StripeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConfigured => f.write_str("Stripe is not configured on this server"),
            Self::Unauthorized => f.write_str("Stripe refused the secret key"),
            Self::InvalidRequest(m) => write!(f, "Stripe rejected the request: {m}"),
            Self::Parse => f.write_str("unexpected answer from Stripe"),
            Self::Http(e) => write!(f, "cannot reach Stripe: {e}"),
        }
    }
}

impl std::error::Error for StripeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_names_the_problem() {
        for (error, word) in [
            (StripeError::NotConfigured, "not configured"),
            (StripeError::Unauthorized, "refused"),
            (StripeError::InvalidRequest("x".to_string()), "rejected"),
            (StripeError::Parse, "unexpected"),
            (StripeError::Http("boom".to_string()), "boom"),
        ] {
            assert!(error.to_string().contains(word), "{error:?}");
        }
    }
}
