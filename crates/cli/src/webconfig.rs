use std::net::SocketAddr;
use std::path::PathBuf;

use platform_store::parse_key;
use platform_web::{Secret, WebConfig};

use crate::error::CliError;
use crate::GRACE_SECS;

// Aucun `Debug` : le secret client ne doit jamais s'afficher.
pub struct GitHubSettings {
    pub app_id: String,
    pub client_id: String,
    pub client_secret: Secret,
    pub slug: String,
    pub key_file: PathBuf,
}

pub struct WebSettings {
    pub config: WebConfig,
    pub github: Option<GitHubSettings>,
    // 0 : pas de sondage (l'auto-déploiement passe alors par les webhooks).
    pub poll_secs: u32,
    pub build_timeout_secs: u32,
    pub work_dir: PathBuf,
    pub sdk_dir: PathBuf,
    pub oauth: Option<(String, Secret)>,
    // Hors de `WebConfig` (qui peut être affichée) : la clé maître ne doit jamais l'être.
    pub master_key: Option<[u8; 32]>,
    // Hors de `WebConfig` pour la même raison que `master_key`.
    pub stripe_secret_key: Option<Secret>,
}

const MAX_POLL_SECS: u32 = 3600;
const MIN_POLL_SECS: u32 = 5;
const MIN_BUILD_SECS: u32 = 30;
const MAX_BUILD_SECS: u32 = 3600;
const MIN_WEBHOOK_SECRET: usize = 8;

fn slug_ok(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 100
        && !slug.starts_with('-')
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn token_ok(text: &str) -> bool {
    !text.is_empty() && text.len() <= 200 && text.bytes().all(|b| b.is_ascii_graphic())
}

// Au-delà, un seul envoi occuperait trop de mémoire du serveur.
const MAX_IMAGE_MB_LIMIT: u32 = 25;

fn usage(message: impl Into<String>) -> CliError {
    CliError::Usage(message.into())
}

fn digits(name: &str, value: String) -> Result<String, CliError> {
    if value.bytes().all(|b| b.is_ascii_digit()) {
        Ok(value)
    } else {
        Err(usage(format!("{name} must contain only digits")))
    }
}

// Les secrets et réglages viennent de l'environnement du processus, jamais de la ligne de commande.
pub fn web_settings(
    get: &dyn Fn(&str) -> Option<String>,
    addr: &SocketAddr,
) -> Result<WebSettings, CliError> {
    let var = |name: &str| get(name).filter(|v| !v.is_empty());
    let public_url = var("PLATFORM_PUBLIC_URL")
        .unwrap_or_else(|| format!("http://{addr}"))
        .trim_end_matches('/')
        .to_string();
    let dev_login = var("PLATFORM_DEV_LOGIN").as_deref() == Some("1");
    if dev_login && !addr.ip().is_loopback() {
        return Err(usage(
            "PLATFORM_DEV_LOGIN=1 is only allowed when listening on a loopback address",
        ));
    }
    let oauth = match (
        var("DISCORD_OAUTH_CLIENT_ID"),
        var("DISCORD_OAUTH_CLIENT_SECRET"),
    ) {
        (Some(id), Some(secret)) => {
            Some((digits("DISCORD_OAUTH_CLIENT_ID", id)?, Secret::new(secret)))
        }
        (None, None) => None,
        _ => {
            return Err(usage(
                "DISCORD_OAUTH_CLIENT_ID and DISCORD_OAUTH_CLIENT_SECRET must be set together",
            ))
        }
    };
    let master_key = var("PLATFORM_MASTER_KEY")
        .map(|hex| {
            parse_key(&hex)
                .ok_or_else(|| usage("PLATFORM_MASTER_KEY must be exactly 64 hexadecimal digits"))
        })
        .transpose()?;
    let number = |name: &str, default: u32| -> Result<u32, CliError> {
        match var(name) {
            None => Ok(default),
            Some(v) if v.bytes().all(|b| b.is_ascii_digit()) => {
                v.parse().map_err(|_| usage(format!("{name} is too large")))
            }
            Some(_) => Err(usage(format!("{name} must be a whole number"))),
        }
    };
    let max_products = number("PLATFORM_MAX_PRODUCTS", 2)?;
    if max_products == 0 {
        return Err(usage("PLATFORM_MAX_PRODUCTS must be at least 1"));
    }
    let deploy_cooldown_secs = number("PLATFORM_DEPLOY_COOLDOWN_SECS", 10)?;
    let max_image_mb = number("PLATFORM_MAX_IMAGE_MB", 10)?;
    if !(1..=MAX_IMAGE_MB_LIMIT).contains(&max_image_mb) {
        return Err(usage(format!(
            "PLATFORM_MAX_IMAGE_MB must be between 1 and {MAX_IMAGE_MB_LIMIT}"
        )));
    }
    let poll_secs = number("PLATFORM_POLL_SECS", 30)?;
    if poll_secs != 0 && !(MIN_POLL_SECS..=MAX_POLL_SECS).contains(&poll_secs) {
        return Err(usage(format!(
            "PLATFORM_POLL_SECS must be 0 (off) or between {MIN_POLL_SECS} and {MAX_POLL_SECS}"
        )));
    }
    let build_timeout_secs = number("PLATFORM_BUILD_TIMEOUT_SECS", 600)?;
    if !(MIN_BUILD_SECS..=MAX_BUILD_SECS).contains(&build_timeout_secs) {
        return Err(usage(format!(
            "PLATFORM_BUILD_TIMEOUT_SECS must be between {MIN_BUILD_SECS} and {MAX_BUILD_SECS}"
        )));
    }
    let github = match (
        var("GITHUB_APP_ID"),
        var("GITHUB_APP_CLIENT_ID"),
        var("GITHUB_APP_CLIENT_SECRET"),
        var("GITHUB_APP_SLUG"),
        var("GITHUB_APP_PRIVATE_KEY_FILE"),
    ) {
        (Some(app_id), Some(client_id), Some(secret), Some(slug), Some(key_file)) => {
            if !token_ok(&client_id) || !token_ok(&secret) {
                return Err(usage("GITHUB_APP_CLIENT_ID and GITHUB_APP_CLIENT_SECRET must be single words"));
            }
            if !slug_ok(&slug) {
                return Err(usage("GITHUB_APP_SLUG is the lowercase name in the app's GitHub address (letters, digits, hyphens)"));
            }
            Some(GitHubSettings {
                app_id: digits("GITHUB_APP_ID", app_id)?,
                client_id,
                client_secret: Secret::new(secret),
                slug,
                key_file: PathBuf::from(key_file),
            })
        }
        (None, None, None, None, None) => None,
        _ => {
            return Err(usage(
                "GITHUB_APP_ID, GITHUB_APP_CLIENT_ID, GITHUB_APP_CLIENT_SECRET, GITHUB_APP_SLUG and GITHUB_APP_PRIVATE_KEY_FILE must all be set together",
            ))
        }
    };
    let github_webhook_secret = match var("GITHUB_WEBHOOK_SECRET") {
        Some(secret) if secret.len() < MIN_WEBHOOK_SECRET => {
            return Err(usage(format!(
                "GITHUB_WEBHOOK_SECRET must have at least {MIN_WEBHOOK_SECRET} characters"
            )))
        }
        other => other.map(Secret::new),
    };
    // Une clé qui ressemble à une clé de production est refusée au démarrage plutôt que
    // d'être acceptée en silence. Pas de clé : Stripe reste non configuré (NoStripe), les
    // abonnements sont refusés avec un message clair au lieu de paiements simulés.
    let stripe_secret_key = match var("STRIPE_SECRET_KEY") {
        Some(key) if platform_stripe::is_live_secret_key(&key) => {
            return Err(usage(
                "STRIPE_SECRET_KEY looks like a live key (sk_live_/rk_live_). Only test keys (sk_test_) are allowed here.",
            ))
        }
        Some(key) if !platform_stripe::is_test_secret_key(&key) => {
            return Err(usage(
                "STRIPE_SECRET_KEY must be a Stripe test secret key (sk_test_...).",
            ))
        }
        other => other.map(Secret::new),
    };
    let stripe_webhook_secret = match (var("STRIPE_WEBHOOK_SECRET"), &stripe_secret_key) {
        (Some(secret), _) if !platform_stripe::is_webhook_secret(&secret) => {
            return Err(usage(
                "STRIPE_WEBHOOK_SECRET must start with whsec_ (copy it from the Stripe dashboard or CLI).",
            ))
        }
        (Some(secret), _) => Some(Secret::new(secret)),
        (None, Some(_)) => {
            return Err(usage(
                "STRIPE_WEBHOOK_SECRET is required when STRIPE_SECRET_KEY is set: without it, webhooks cannot be verified.",
            ))
        }
        (None, None) => None,
    };
    let stripe_currency = var("PLATFORM_STRIPE_CURRENCY").unwrap_or_else(|| "usd".to_string());
    let config = WebConfig {
        public_url,
        api_url_for_containers: var("PLATFORM_API_URL_FOR_CONTAINERS")
            .unwrap_or_else(|| format!("http://host.docker.internal:{}", addr.port())),
        dev_login,
        bot_image: var("PLATFORM_BOT_IMAGE")
            .unwrap_or_else(|| "platform-discord-bot:dev".to_string()),
        subscription_days: 30,
        grace_secs: GRACE_SECS,
        max_products,
        max_image_bytes: usize::try_from(max_image_mb)
            .map_or(usize::MAX, |mb| mb.saturating_mul(1024 * 1024)),
        deploy_cooldown_secs,
        github_webhook_secret,
        frontend_url: var("PLATFORM_FRONTEND_URL").map(|u| u.trim_end_matches('/').to_string()),
        admin_user_id: var("PLATFORM_ADMIN_USER_ID"),
        stripe_webhook_secret,
        stripe_currency,
        maintenance_banner: var("PLATFORM_MAINTENANCE_BANNER"),
    };
    Ok(WebSettings {
        config,
        github,
        poll_secs,
        build_timeout_secs,
        stripe_secret_key,
        work_dir: PathBuf::from(
            var("PLATFORM_WORK_DIR").unwrap_or_else(|| ".platform/builds".to_string()),
        ),
        sdk_dir: PathBuf::from(var("PLATFORM_SDK_DIR").unwrap_or_else(|| "sdk/node".to_string())),
        oauth,
        master_key,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn settings(vars: &[(&str, &str)], addr: &str) -> Result<WebSettings, CliError> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        let Ok(addr) = addr.parse::<SocketAddr>() else {
            panic!("bad address {addr}");
        };
        web_settings(&|name| map.get(name).cloned(), &addr)
    }

    fn ok(vars: &[(&str, &str)], addr: &str) -> WebSettings {
        match settings(vars, addr) {
            Ok(s) => s,
            Err(e) => panic!("rejected: {e}"),
        }
    }

    #[test]
    fn defaults_follow_the_listening_address() {
        let s = ok(&[], "127.0.0.1:8080");
        assert_eq!(s.config.public_url, "http://127.0.0.1:8080");
        assert_eq!(
            s.config.api_url_for_containers,
            "http://host.docker.internal:8080"
        );
        assert_eq!(s.config.bot_image, "platform-discord-bot:dev");
        assert!(!s.config.dev_login);
        assert!(s.oauth.is_none());
        assert!(s.master_key.is_none());
        assert_eq!(
            (s.config.subscription_days, s.config.grace_secs),
            (30, GRACE_SECS)
        );
        assert_eq!(
            ok(&[], "127.0.0.1:9").config.api_url_for_containers,
            "http://host.docker.internal:9"
        );
    }

    #[test]
    fn overrides_are_read_from_the_environment() {
        let s = ok(
            &[
                ("PLATFORM_PUBLIC_URL", "https://bots.example/"),
                ("PLATFORM_API_URL_FOR_CONTAINERS", "http://api:1"),
                ("PLATFORM_BOT_IMAGE", "mybot:2"),
            ],
            "0.0.0.0:8080",
        );
        assert_eq!(s.config.public_url, "https://bots.example");
        assert!(s.config.secure_cookies());
        assert_eq!(s.config.api_url_for_containers, "http://api:1");
        assert_eq!(s.config.bot_image, "mybot:2");
    }

    #[test]
    fn the_abuse_limits_have_defaults_and_can_be_changed() {
        let s = ok(&[], "127.0.0.1:8080");
        assert_eq!(
            (s.config.max_products, s.config.deploy_cooldown_secs),
            (2, 10)
        );
        let s = ok(
            &[
                ("PLATFORM_MAX_PRODUCTS", "5"),
                ("PLATFORM_DEPLOY_COOLDOWN_SECS", "0"),
            ],
            "127.0.0.1:8080",
        );
        assert_eq!(
            (s.config.max_products, s.config.deploy_cooldown_secs),
            (5, 0)
        );
        let s = ok(&[("PLATFORM_MAX_PRODUCTS", "1")], "127.0.0.1:8080");
        assert_eq!(s.config.max_products, 1);
    }

    #[test]
    fn the_image_size_limit_defaults_to_ten_megabytes_and_is_bounded() {
        let default = ok(&[], "127.0.0.1:8080");
        assert_eq!(default.config.max_image_bytes, 10 * 1024 * 1024);
        for (value, bytes) in [("1", 1_048_576), ("25", 26_214_400)] {
            let s = ok(&[("PLATFORM_MAX_IMAGE_MB", value)], "127.0.0.1:8080");
            assert_eq!(s.config.max_image_bytes, bytes, "{value}");
        }
        for bad in ["0", "26", "abc", "-1", "5.5"] {
            let r = settings(&[("PLATFORM_MAX_IMAGE_MB", bad)], "127.0.0.1:8080");
            assert!(
                matches!(r, Err(CliError::Usage(m)) if m.contains("PLATFORM_MAX_IMAGE_MB")),
                "{bad}"
            );
        }
    }

    #[test]
    fn invalid_limits_are_refused_with_the_variable_name() {
        for (name, value) in [
            ("PLATFORM_MAX_PRODUCTS", "abc"),
            ("PLATFORM_MAX_PRODUCTS", "-1"),
            ("PLATFORM_MAX_PRODUCTS", "+3"),
            ("PLATFORM_MAX_PRODUCTS", "0"),
            ("PLATFORM_MAX_PRODUCTS", "99999999999"),
            ("PLATFORM_DEPLOY_COOLDOWN_SECS", "1.5"),
            ("PLATFORM_DEPLOY_COOLDOWN_SECS", "99999999999"),
        ] {
            let r = settings(&[(name, value)], "127.0.0.1:8080");
            assert!(
                matches!(r, Err(CliError::Usage(m)) if m.contains(name)),
                "{name}={value}"
            );
        }
    }

    #[test]
    fn empty_variables_count_as_unset() {
        let s = ok(
            &[
                ("PLATFORM_MASTER_KEY", ""),
                ("PLATFORM_BOT_IMAGE", ""),
                ("PLATFORM_PUBLIC_URL", ""),
            ],
            "127.0.0.1:8080",
        );
        assert!(s.master_key.is_none());
        assert_eq!(s.config.bot_image, "platform-discord-bot:dev");
        assert_eq!(s.config.public_url, "http://127.0.0.1:8080");
    }

    #[test]
    fn dev_login_needs_the_exact_value_and_a_loopback_address() {
        assert!(
            ok(&[("PLATFORM_DEV_LOGIN", "1")], "127.0.0.1:8080")
                .config
                .dev_login
        );
        assert!(
            ok(&[("PLATFORM_DEV_LOGIN", "1")], "[::1]:8080")
                .config
                .dev_login
        );
        assert!(
            !ok(&[("PLATFORM_DEV_LOGIN", "true")], "127.0.0.1:8080")
                .config
                .dev_login
        );
        assert!(
            !ok(&[("PLATFORM_DEV_LOGIN", "0")], "0.0.0.0:8080")
                .config
                .dev_login
        );
        for addr in ["0.0.0.0:8080", "192.168.1.5:8080", "[::]:8080"] {
            let r = settings(&[("PLATFORM_DEV_LOGIN", "1")], addr);
            assert!(matches!(r, Err(CliError::Usage(_))), "{addr}");
        }
    }

    #[test]
    fn oauth_needs_both_the_id_and_the_secret() {
        let both = ok(
            &[
                ("DISCORD_OAUTH_CLIENT_ID", "123"),
                ("DISCORD_OAUTH_CLIENT_SECRET", "s"),
            ],
            "127.0.0.1:8080",
        );
        assert_eq!(
            both.oauth.as_ref().map(|(id, s)| (id.as_str(), s.expose())),
            Some(("123", "s"))
        );
        for vars in [
            [("DISCORD_OAUTH_CLIENT_ID", "123")],
            [("DISCORD_OAUTH_CLIENT_SECRET", "s")],
        ] {
            assert!(matches!(
                settings(&vars, "127.0.0.1:8080"),
                Err(CliError::Usage(_))
            ));
        }
    }

    #[test]
    fn the_master_key_is_64_hex_digits_and_never_in_the_config() {
        let key = "0f".repeat(32);
        let s = ok(&[("PLATFORM_MASTER_KEY", key.as_str())], "127.0.0.1:8080");
        assert_eq!(s.master_key, Some([0x0f; 32]));
        assert!(!format!("{:?}", s.config).contains("0f0f"));
        for bad in ["abc", &"0f".repeat(31), &"zz".repeat(32), &"0f".repeat(33)] {
            let r = settings(&[("PLATFORM_MASTER_KEY", bad)], "127.0.0.1:8080");
            assert!(
                matches!(r, Err(CliError::Usage(m)) if m.contains("PLATFORM_MASTER_KEY")),
                "{bad}"
            );
        }
    }

    #[test]
    fn client_ids_must_be_numeric() {
        let oauth = [
            ("DISCORD_OAUTH_CLIENT_ID", "12a"),
            ("DISCORD_OAUTH_CLIENT_SECRET", "s"),
        ];
        assert!(
            matches!(settings(&oauth, "127.0.0.1:8080"), Err(CliError::Usage(m)) if m.contains("DISCORD_OAUTH_CLIENT_ID"))
        );
    }

    const GITHUB: [(&str, &str); 5] = [
        ("GITHUB_APP_ID", "12345"),
        ("GITHUB_APP_CLIENT_ID", "Iv1.abc"),
        ("GITHUB_APP_CLIENT_SECRET", "s3cret"),
        ("GITHUB_APP_SLUG", "my-bot-platform"),
        ("GITHUB_APP_PRIVATE_KEY_FILE", "C:/keys/app.pem"),
    ];

    #[test]
    fn the_github_app_is_all_or_nothing() {
        assert!(ok(&[], "127.0.0.1:8080").github.is_none());
        let s = ok(&GITHUB, "127.0.0.1:8080");
        let g = s.github.unwrap_or_else(|| panic!("github missing"));
        assert_eq!(
            (
                g.app_id.as_str(),
                g.client_id.as_str(),
                g.client_secret.expose(),
                g.slug.as_str()
            ),
            ("12345", "Iv1.abc", "s3cret", "my-bot-platform")
        );
        assert_eq!(g.key_file, PathBuf::from("C:/keys/app.pem"));
        for (skip, (name, _)) in GITHUB.iter().enumerate() {
            let partial: Vec<_> = GITHUB
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != skip)
                .map(|(_, p)| *p)
                .collect();
            assert!(
                matches!(settings(&partial, "127.0.0.1:8080"), Err(CliError::Usage(m)) if m.contains("must all be set together")),
                "without {name}"
            );
        }
    }

    #[test]
    fn github_values_are_checked() {
        let with = |name: &str, value: &str| {
            let vars: Vec<_> = GITHUB
                .iter()
                .map(|(k, v)| if *k == name { (*k, value) } else { (*k, *v) })
                .collect();
            settings(&vars, "127.0.0.1:8080")
        };
        assert!(
            matches!(with("GITHUB_APP_ID", "12a"), Err(CliError::Usage(m)) if m.contains("GITHUB_APP_ID"))
        );
        for bad in ["My App", "UPPER", "-lead", "a/b", ""] {
            if bad.is_empty() {
                continue;
            }
            assert!(
                matches!(with("GITHUB_APP_SLUG", bad), Err(CliError::Usage(m)) if m.contains("GITHUB_APP_SLUG")),
                "{bad}"
            );
        }
        assert!(matches!(
            with("GITHUB_APP_CLIENT_SECRET", "has space"),
            Err(CliError::Usage(_))
        ));
        assert!(with("GITHUB_APP_SLUG", "a1-b2").is_ok());
    }

    #[test]
    fn the_webhook_secret_needs_enough_characters_and_stays_out_of_the_debug_output() {
        let s = ok(&[("GITHUB_WEBHOOK_SECRET", "12345678")], "127.0.0.1:8080");
        assert_eq!(
            s.config.github_webhook_secret.as_ref().map(Secret::expose),
            Some("12345678")
        );
        assert!(!format!("{:?}", s.config).contains("12345678"));
        assert!(matches!(
            settings(&[("GITHUB_WEBHOOK_SECRET", "1234567")], "127.0.0.1:8080"),
            Err(CliError::Usage(m)) if m.contains("GITHUB_WEBHOOK_SECRET")
        ));
        assert!(ok(&[], "127.0.0.1:8080")
            .config
            .github_webhook_secret
            .is_none());
    }

    #[test]
    fn without_any_stripe_variable_stripe_stays_unconfigured() {
        let s = ok(&[], "127.0.0.1:8080");
        assert!(s.stripe_secret_key.is_none());
        assert!(s.config.stripe_webhook_secret.is_none());
        assert_eq!(s.config.stripe_currency, "usd");
    }

    #[test]
    fn a_live_stripe_key_is_refused_even_with_a_valid_webhook_secret() {
        let r = settings(
            &[
                ("STRIPE_SECRET_KEY", "sk_live_abc123"),
                ("STRIPE_WEBHOOK_SECRET", "whsec_abc123"),
            ],
            "127.0.0.1:8080",
        );
        assert!(
            matches!(r, Err(CliError::Usage(m)) if m.contains("STRIPE_SECRET_KEY") && m.contains("live"))
        );
    }

    #[test]
    fn a_malformed_stripe_key_is_refused() {
        let r = settings(
            &[("STRIPE_SECRET_KEY", "not-a-stripe-key")],
            "127.0.0.1:8080",
        );
        assert!(matches!(r, Err(CliError::Usage(m)) if m.contains("STRIPE_SECRET_KEY")));
    }

    #[test]
    fn a_test_key_without_a_webhook_secret_is_refused() {
        let r = settings(&[("STRIPE_SECRET_KEY", "sk_test_abc123")], "127.0.0.1:8080");
        assert!(matches!(r, Err(CliError::Usage(m)) if m.contains("STRIPE_WEBHOOK_SECRET")));
    }

    #[test]
    fn a_malformed_webhook_secret_is_refused() {
        let r = settings(
            &[
                ("STRIPE_SECRET_KEY", "sk_test_abc123"),
                ("STRIPE_WEBHOOK_SECRET", "not-whsec"),
            ],
            "127.0.0.1:8080",
        );
        assert!(matches!(r, Err(CliError::Usage(m)) if m.contains("STRIPE_WEBHOOK_SECRET")));
    }

    #[test]
    fn a_valid_test_key_and_webhook_secret_are_accepted_and_hidden_from_debug() {
        let s = ok(
            &[
                ("STRIPE_SECRET_KEY", "sk_test_abc123"),
                ("STRIPE_WEBHOOK_SECRET", "whsec_abc123"),
                ("PLATFORM_STRIPE_CURRENCY", "eur"),
            ],
            "127.0.0.1:8080",
        );
        assert_eq!(
            s.stripe_secret_key.as_ref().map(Secret::expose),
            Some("sk_test_abc123")
        );
        assert_eq!(
            s.config.stripe_webhook_secret.as_ref().map(Secret::expose),
            Some("whsec_abc123")
        );
        assert_eq!(s.config.stripe_currency, "eur");
        assert!(!format!("{:?}", s.config).contains("whsec_abc123"));
    }

    #[test]
    fn polling_and_build_limits_have_defaults_and_bounds() {
        let d = ok(&[], "127.0.0.1:8080");
        assert_eq!((d.poll_secs, d.build_timeout_secs), (30, 600));
        assert_eq!(d.work_dir, PathBuf::from(".platform/builds"));
        assert_eq!(d.sdk_dir, PathBuf::from("sdk/node"));
        assert_eq!(
            ok(&[("PLATFORM_POLL_SECS", "0")], "127.0.0.1:8080").poll_secs,
            0
        );
        assert_eq!(
            ok(&[("PLATFORM_POLL_SECS", "5")], "127.0.0.1:8080").poll_secs,
            5
        );
        assert_eq!(
            ok(&[("PLATFORM_POLL_SECS", "3600")], "127.0.0.1:8080").poll_secs,
            3600
        );
        for (name, bad) in [
            ("PLATFORM_POLL_SECS", "4"),
            ("PLATFORM_POLL_SECS", "3601"),
            ("PLATFORM_BUILD_TIMEOUT_SECS", "29"),
            ("PLATFORM_BUILD_TIMEOUT_SECS", "3601"),
        ] {
            assert!(
                matches!(settings(&[(name, bad)], "127.0.0.1:8080"), Err(CliError::Usage(m)) if m.contains(name)),
                "{name}={bad}"
            );
        }
        assert_eq!(
            ok(&[("PLATFORM_BUILD_TIMEOUT_SECS", "30")], "127.0.0.1:8080").build_timeout_secs,
            30
        );
        assert_eq!(
            ok(&[("PLATFORM_BUILD_TIMEOUT_SECS", "3600")], "127.0.0.1:8080").build_timeout_secs,
            3600
        );
        let custom = ok(
            &[
                ("PLATFORM_WORK_DIR", "D:/w"),
                ("PLATFORM_SDK_DIR", "D:/sdk"),
            ],
            "127.0.0.1:8080",
        );
        assert_eq!(
            (custom.work_dir, custom.sdk_dir),
            (PathBuf::from("D:/w"), PathBuf::from("D:/sdk"))
        );
    }
}
