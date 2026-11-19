use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use platform_build::ImageBuilder;
use platform_core::Timestamp;
use platform_github::GitHub;
use platform_store::{SqliteStore, Vault};
use platform_stripe::Stripe;

use crate::bot_app::BotVerifier;
use crate::containers::Containers;
use crate::discord::Discord;
use crate::engine::Engine;
use crate::guild_profile::GuildProfiles;

pub type Clock = Arc<dyn Fn() -> Timestamp + Send + Sync>;

// Un secret ne s'affiche jamais, même par un `{:?}` oublié dans un journal.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

#[derive(Clone, Debug)]
pub struct WebConfig {
    pub public_url: String,
    pub api_url_for_containers: String,
    pub dev_login: bool,
    pub bot_image: String,
    pub subscription_days: u32,
    pub grace_secs: u32,
    pub max_products: u32,
    pub max_image_bytes: usize,
    // `None` : les livraisons de webhook GitHub sont refusées (le déploiement automatique passe alors par le sondage).
    pub github_webhook_secret: Option<Secret>,
    pub deploy_cooldown_secs: u32,
    // `Some(url)` : le callback OAuth redirige vers `{url}/dashboard` après connexion.
    pub frontend_url: Option<String>,
    // `Some(id)` : l'utilisateur Discord avec cet identifiant est l'administrateur de la plateforme.
    pub admin_user_id: Option<String>,
    // `None` : Stripe n'est pas configuré, les abonnements sont refusés.
    pub stripe_webhook_secret: Option<Secret>,
    pub stripe_currency: String,
    // `Some(msg)` : bannière affichée sur toutes les pages (maintenance, incident).
    pub maintenance_banner: Option<String>,
}

// Délai minimal entre deux déploiements du même container, gardé en mémoire.
// Utilisé aussi pour limiter les sessions Stripe Checkout par utilisateur.
#[derive(Default)]
pub struct DeployLimiter {
    last: Mutex<HashMap<String, Timestamp>>,
}

impl DeployLimiter {
    // `Ok` enregistre l'action ; `Err(n)` : attendre encore n secondes.
    pub fn check_and_record(
        &self,
        key: &str,
        now: Timestamp,
        cooldown_secs: u32,
    ) -> Result<(), u32> {
        let Ok(mut last) = self.last.lock() else {
            return Ok(());
        };
        // Purge des entrées dont la fenêtre est passée depuis plus d'une période.
        let cutoff = now.saturating_sub(i64::from(cooldown_secs).saturating_mul(2));
        last.retain(|_, at| *at >= cutoff);
        if let Some(&at) = last.get(key) {
            let elapsed = now.saturating_sub(at);
            if (0..i64::from(cooldown_secs)).contains(&elapsed) {
                let remaining = i64::from(cooldown_secs) - elapsed;
                return Err(u32::try_from(remaining).unwrap_or(cooldown_secs));
            }
        }
        last.insert(key.to_string(), now);
        Ok(())
    }
}

// Rate limit par IP sur les endpoints d'authentification.
// Fenêtre glissante de 60 s, 10 requêtes max par IP.
#[derive(Default)]
pub struct AuthLimiter {
    map: Mutex<HashMap<String, (u32, Timestamp)>>,
}

impl AuthLimiter {
    // `true` : requête acceptée. `false` : limite dépassée.
    pub fn check(&self, ip: &str, now: Timestamp) -> bool {
        let Ok(mut map) = self.map.lock() else {
            return true;
        };
        // Purge des entrées trop vieilles.
        map.retain(|_, (_, start)| now.saturating_sub(*start) < AUTH_WINDOW_SECS * 2);
        let entry = map.entry(ip.to_string()).or_insert((0, now));
        if now.saturating_sub(entry.1) >= AUTH_WINDOW_SECS {
            *entry = (1, now);
            return true;
        }
        entry.0 = entry.0.saturating_add(1);
        entry.0 <= AUTH_MAX_REQUESTS
    }
}

const AUTH_WINDOW_SECS: Timestamp = 60;
const AUTH_MAX_REQUESTS: u32 = 10;

impl WebConfig {
    pub fn secure_cookies(&self) -> bool {
        self.public_url.starts_with("https://")
    }
}

#[derive(Clone)]
pub struct WebState {
    pub store: Arc<Mutex<SqliteStore>>,
    pub discord: Arc<dyn Discord>,
    pub bots: Arc<dyn BotVerifier>,
    pub profiles: Arc<dyn GuildProfiles>,
    pub github: Arc<dyn GitHub>,
    pub builder: Arc<dyn ImageBuilder>,
    pub engine: Arc<Engine>,
    pub vault: Arc<Vault>,
    pub containers: Arc<dyn Containers>,
    pub stripe: Arc<dyn Stripe>,
    pub clock: Clock,
    pub config: Arc<WebConfig>,
    pub limiter: Arc<DeployLimiter>,
    pub auth_limiter: Arc<AuthLimiter>,
}

#[cfg(test)]
pub(crate) fn test_state() -> WebState {
    use platform_runner::{ContainerState, RunSpec};

    use crate::bot_app::BotApp;
    use crate::containers::ContainerError;
    use crate::discord::{DiscordError, NoDiscord};
    use crate::guild_profile::{ProfileError, ProfilePatch};

    struct NoBuilder;
    impl ImageBuilder for NoBuilder {
        fn build(
            &self,
            _: &platform_build::BuildRequest,
            _: &mut dyn FnMut(&str),
        ) -> Result<platform_build::BuildOutput, platform_build::BuildError> {
            Err(platform_build::BuildError::MissingConfig)
        }
    }

    struct NoProfiles;
    impl GuildProfiles for NoProfiles {
        fn apply(&self, _: &str, _: u64, _: &ProfilePatch) -> Result<(), ProfileError> {
            Err(ProfileError::Unavailable)
        }
    }

    struct NoBots;
    impl BotVerifier for NoBots {
        fn verify(&self, _: &str) -> Result<BotApp, DiscordError> {
            Err(DiscordError::Rejected)
        }
    }

    struct Idle;
    impl Containers for Idle {
        fn deploy(&self, _: &RunSpec) -> Result<(), ContainerError> {
            Ok(())
        }
        fn stop(&self, _: &str) -> Result<(), ContainerError> {
            Ok(())
        }
        fn state(&self, _: &str) -> Result<Option<ContainerState>, ContainerError> {
            Ok(None)
        }
        fn logs(&self, _: &str, _: u32) -> Result<String, ContainerError> {
            Ok(String::new())
        }
        fn image_exists(&self, _: &str) -> Result<bool, ContainerError> {
            Ok(false)
        }
    }

    let Ok(store) = SqliteStore::in_memory() else {
        panic!("cannot open in-memory store");
    };
    WebState {
        store: Arc::new(Mutex::new(store)),
        discord: Arc::new(NoDiscord),
        bots: Arc::new(NoBots),
        profiles: Arc::new(NoProfiles),
        github: Arc::new(platform_github::NoGitHub),
        builder: Arc::new(NoBuilder),
        engine: Arc::new(Engine::new(true)),
        vault: Arc::new(Vault::new(&[1; 32])),
        containers: Arc::new(Idle),
        stripe: Arc::new(platform_stripe::NoStripe),
        clock: Arc::new(|| 0),
        config: Arc::new(WebConfig {
            public_url: "http://127.0.0.1:8080".to_string(),
            api_url_for_containers: String::new(),
            dev_login: false,
            bot_image: String::new(),
            subscription_days: 30,
            grace_secs: 0,
            max_products: 2,
            max_image_bytes: 2 * 1024 * 1024,
            github_webhook_secret: None,
            deploy_cooldown_secs: 0,
            frontend_url: None,
            admin_user_id: None,
            stripe_webhook_secret: None,
            stripe_currency: "usd".to_string(),
            maintenance_banner: None,
        }),
        limiter: Arc::new(DeployLimiter::default()),
        auth_limiter: Arc::new(AuthLimiter::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_hidden_from_debug_output() {
        let secret = Secret::new("hunter2");
        assert_eq!(secret.expose(), "hunter2");
        assert_eq!(format!("{secret:?}"), "Secret(***)");
        let wrapped = Some(secret);
        assert!(!format!("{wrapped:?}").contains("hunter2"));
    }

    #[test]
    fn cookies_are_secure_only_over_https() {
        let mut config = WebConfig {
            public_url: "http://127.0.0.1:8080".to_string(),
            api_url_for_containers: String::new(),
            dev_login: false,
            bot_image: String::new(),
            subscription_days: 30,
            grace_secs: 0,
            max_products: 2,
            max_image_bytes: 2 * 1024 * 1024,
            github_webhook_secret: None,
            deploy_cooldown_secs: 0,
            frontend_url: None,
            admin_user_id: None,
            stripe_webhook_secret: None,
            stripe_currency: "usd".to_string(),
            maintenance_banner: None,
        };
        assert!(!config.secure_cookies());
        config.public_url = "https://example.com".to_string();
        assert!(config.secure_cookies());
    }

    #[test]
    fn a_second_deployment_inside_the_cooldown_must_wait() {
        let limiter = DeployLimiter::default();
        assert_eq!(limiter.check_and_record("bot-1-free", 100, 10), Ok(()));
        assert_eq!(limiter.check_and_record("bot-1-free", 100, 10), Err(10));
        assert_eq!(limiter.check_and_record("bot-1-free", 104, 10), Err(6));
        assert_eq!(limiter.check_and_record("bot-1-free", 109, 10), Err(1));
    }

    #[test]
    fn the_cooldown_ends_exactly_after_its_length() {
        let limiter = DeployLimiter::default();
        assert_eq!(limiter.check_and_record("k", 100, 10), Ok(()));
        assert_eq!(limiter.check_and_record("k", 110, 10), Ok(()));
        assert_eq!(limiter.check_and_record("k", 119, 10), Err(1));
        assert_eq!(limiter.check_and_record("k", 120, 10), Ok(()));
    }

    #[test]
    fn a_refused_attempt_does_not_extend_the_wait() {
        let limiter = DeployLimiter::default();
        assert_eq!(limiter.check_and_record("k", 100, 10), Ok(()));
        assert_eq!(limiter.check_and_record("k", 105, 10), Err(5));
        assert_eq!(limiter.check_and_record("k", 110, 10), Ok(()));
    }

    #[test]
    fn containers_are_limited_independently() {
        let limiter = DeployLimiter::default();
        assert_eq!(limiter.check_and_record("bot-1-free", 100, 10), Ok(()));
        assert_eq!(limiter.check_and_record("bot-1-premium", 100, 10), Ok(()));
        assert_eq!(limiter.check_and_record("bot-2-free", 100, 10), Ok(()));
    }

    #[test]
    fn a_zero_cooldown_never_blocks_and_a_clock_going_back_does_not_either() {
        let limiter = DeployLimiter::default();
        for _ in 0..3 {
            assert_eq!(limiter.check_and_record("k", 100, 0), Ok(()));
        }
        let limiter = DeployLimiter::default();
        assert_eq!(limiter.check_and_record("k", 100, 10), Ok(()));
        assert_eq!(limiter.check_and_record("k", 50, 10), Ok(()));
        assert_eq!(limiter.check_and_record("k", 51, 10), Err(9));
    }
}
