use std::fmt;

use platform_runner::RunError;
use platform_store::StoreError;

#[derive(Debug)]
pub enum CliError {
    Usage(String),
    Config(String),
    Store(StoreError),
    Run(RunError),
    Io(std::io::Error),
    MissingSecret(&'static str),
    Diagnostic,
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(m) => write!(f, "usage: {m}"),
            Self::Config(m) => write!(f, "invalid configuration:\n{m}"),
            Self::Store(e) => write!(f, "{e}"),
            Self::Run(e) => write!(f, "docker: {e:?}"),
            Self::Io(e) => write!(f, "{e}"),
            Self::MissingSecret(name) => write!(f, "environment variable {name} is required"),
            Self::Diagnostic => f.write_str("the diagnostic found problems (see above)"),
        }
    }
}

impl std::error::Error for CliError {}

impl From<StoreError> for CliError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<RunError> for CliError {
    fn from(error: RunError) -> Self {
        Self::Run(error)
    }
}

impl From<std::io::Error> for CliError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_say_what_is_wrong() {
        assert_eq!(CliError::Usage("x".into()).to_string(), "usage: x");
        assert_eq!(
            CliError::Config("y".into()).to_string(),
            "invalid configuration:\ny"
        );
        assert_eq!(
            CliError::MissingSecret("TOKEN").to_string(),
            "environment variable TOKEN is required"
        );
        assert_eq!(
            CliError::Diagnostic.to_string(),
            "the diagnostic found problems (see above)"
        );
        assert!(CliError::Store(StoreError::Random)
            .to_string()
            .contains("random"));
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        assert_eq!(CliError::from(io).to_string(), "gone");
        let run = CliError::from(RunError::Spec(platform_runner::SpecError::InvalidName));
        assert!(run.to_string().starts_with("docker: "));
    }
}
