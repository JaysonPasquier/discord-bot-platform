use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitHubError {
    NotConfigured,
    BadKey,
    Unauthorized,
    NotFound,
    RateLimited,
    TooLarge,
    InvalidName,
    Parse,
    Http(String),
}

impl fmt::Display for GitHubError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConfigured => f.write_str("GitHub is not configured"),
            Self::BadKey => f.write_str("the GitHub App private key cannot be read"),
            Self::Unauthorized => f.write_str("GitHub refused the credentials"),
            Self::NotFound => {
                f.write_str("GitHub does not know this resource, or the app cannot see it")
            }
            Self::RateLimited => f.write_str("GitHub rate limit reached"),
            Self::TooLarge => f.write_str("the repository archive is too large"),
            Self::InvalidName => f.write_str("invalid repository or branch name"),
            Self::Parse => f.write_str("unexpected answer from GitHub"),
            Self::Http(e) => write!(f, "cannot reach GitHub: {e}"),
        }
    }
}

impl std::error::Error for GitHubError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_names_the_problem() {
        for (error, word) in [
            (GitHubError::NotConfigured, "not configured"),
            (GitHubError::BadKey, "private key"),
            (GitHubError::Unauthorized, "refused"),
            (GitHubError::NotFound, "cannot see"),
            (GitHubError::RateLimited, "rate limit"),
            (GitHubError::TooLarge, "too large"),
            (GitHubError::InvalidName, "invalid"),
            (GitHubError::Parse, "unexpected"),
            (GitHubError::Http("boom".to_string()), "boom"),
        ] {
            assert!(error.to_string().contains(word), "{error:?}");
        }
    }
}
