#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Free,
    Premium,
}

impl Tier {
    // Valeurs de départ, à affiner avec l'usage réel.
    pub fn memory_mb(self) -> u32 {
        match self {
            Self::Free => 128,
            Self::Premium => 512,
        }
    }

    pub fn cpus(self) -> &'static str {
        match self {
            Self::Free => "0.1",
            Self::Premium => "1",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSpec {
    pub name: String,
    pub image: String,
    pub tier: Tier,
    pub network: String,
    pub env: Vec<(String, String)>,
    pub command: Vec<String>,
    // Un volume nommé par produit (pas par container) : passer de gratuit à premium garde les
    // données et la configuration du bot. `None` : pas de volume (démo, bot d'exemple).
    pub volume: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecError {
    InvalidName,
    InvalidImage,
    InvalidNetwork,
    InvalidVolume,
    InvalidEnvName(String),
    InvalidEnvValue(String),
    TooManyEnv,
    InvalidCommand,
    MalformedEnvPair(String),
    ReservedEnvName(String),
}

pub(crate) const MAX_ENV: usize = 32;

// Où le volume partagé du produit est monté dans le container. Même chemin pour
// gratuit et premium : le code du bot le trouve toujours au même endroit, quelle que soit la version.
pub const VOLUME_MOUNT_PATH: &str = "/data";

// Un nom commençant par « - » serait lu par docker comme une option (injection d'argument).
pub(crate) fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphanumeric());
    first_ok
        && name.len() <= 63
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

pub(crate) fn valid_image(image: &str) -> bool {
    image.len() <= 255
        && image.starts_with(|c: char| c.is_ascii_alphanumeric())
        && image.chars().all(|c| {
            c.is_ascii_lowercase()
                || c.is_ascii_digit()
                || matches!(c, '.' | '/' | ':' | '@' | '-' | '_')
        })
}

pub(crate) fn valid_env_name(name: &str) -> bool {
    name.len() <= 64
        && name.starts_with(|c: char| c.is_ascii_uppercase() || c == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_follow_the_quotas_of_dec_017() {
        assert_eq!(Tier::Free.memory_mb(), 128);
        assert_eq!(Tier::Free.cpus(), "0.1");
        assert_eq!(Tier::Premium.memory_mb(), 512);
        assert_eq!(Tier::Premium.cpus(), "1");
    }

    #[test]
    fn names_are_plain_and_cannot_look_like_options() {
        assert!(valid_name("bot-12_free.v1"));
        assert!(valid_name("a"));
        assert!(valid_name(&"a".repeat(63)));
        assert!(!valid_name(&"a".repeat(64)));
        for bad in [
            "",
            "-rm",
            "--privileged",
            "_x",
            ".x",
            "a b",
            "a/b",
            "a=b",
            "é",
            "a\nb",
        ] {
            assert!(!valid_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn images_are_lowercase_references() {
        for good in [
            "alpine:3",
            "registry.local/team/bot:1.2",
            "bot@sha256:abc123",
            "a",
        ] {
            assert!(valid_image(good), "{good}");
        }
        assert!(valid_image(&"a".repeat(255)));
        assert!(!valid_image(&"a".repeat(256)));
        for bad in [
            "",
            "-v",
            "--privileged",
            "Alpine",
            "a b",
            "a;b",
            "a$b",
            "a\nb",
            ".x",
            "/x",
        ] {
            assert!(!valid_image(bad), "{bad:?}");
        }
    }

    #[test]
    fn env_names_are_shell_style_constants() {
        for good in ["DISCORD_TOKEN", "_X", "A1", "PLATFORM_API_TOKEN"] {
            assert!(valid_env_name(good), "{good}");
        }
        assert!(valid_env_name(&"A".repeat(64)));
        assert!(!valid_env_name(&"A".repeat(65)));
        for bad in ["", "1A", "a", "A-B", "A=B", "A B", "-A"] {
            assert!(!valid_env_name(bad), "{bad:?}");
        }
    }
}
