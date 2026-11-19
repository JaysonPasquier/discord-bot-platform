// SDK BotPlatform pour Rust — vérifie si un serveur Discord a accès au premium d'un bot.
//
// Pour les bots async (Serenity, Poise), appelez `is_premium` via
// `tokio::task::spawn_blocking` ou `tokio::task::block_in_place`.
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

pub type Fetcher = Box<dyn Fn(&str, &str) -> Result<FetchResult, ()> + Send + Sync>;
pub type Clock = Box<dyn Fn() -> u64 + Send + Sync>;

pub enum FetchResult {
    Access(bool),
    ServerError,
    AuthError,
    RequestError(u16),
}

#[derive(Debug)]
pub enum PlatformError {
    Auth,
    Request(u16),
}

impl std::fmt::Display for PlatformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlatformError::Auth => f.write_str("API token rejected (401)"),
            PlatformError::Request(s) => write!(f, "unexpected status {s}"),
        }
    }
}

impl std::error::Error for PlatformError {}

struct CacheEntry {
    access: bool,
    at_ms: u64,
}

pub struct PlatformClient {
    fetcher: Fetcher,
    clock: Clock,
    jitter_ms: u64,
    refresh_ms: u64,
    cache_ttl_ms: u64,
    default_on_failure: bool,
    cache: Mutex<HashMap<String, CacheEntry>>,
}

pub struct Config {
    pub base_url: String,
    pub token: String,
    pub refresh_ms: u64,
    pub cache_ttl_ms: u64,
    pub timeout_ms: u64,
    pub default_on_failure: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            token: String::new(),
            refresh_ms: 60_000,
            cache_ttl_ms: 24 * 60 * 60 * 1000,
            timeout_ms: 3_000,
            default_on_failure: false,
        }
    }
}

impl PlatformClient {
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        let base = base_url.into().trim_end_matches('/').to_string();
        let tok = token.into();
        Self::with_config_and_injectors(
            Config {
                base_url: base.clone(),
                token: tok.clone(),
                ..Default::default()
            },
            Self::make_ureq_fetcher(base, tok, 3_000),
            Box::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or(Duration::ZERO)
                    .as_millis() as u64
            }),
        )
    }

    fn make_ureq_fetcher(base_url: String, token: String, timeout_ms: u64) -> Fetcher {
        Box::new(move |guild_id: &str, _tok: &str| {
            let url = format!("{base_url}/v1/premium/{guild_id}");
            let agent = ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_millis(timeout_ms)))
                .http_status_as_error(false)
                .build()
                .into();
            let mut response = agent
                .get(&url)
                .header("Authorization", format!("Bearer {token}"))
                .call()
                .map_err(|_| ())?;
            let status = response.status().as_u16();
            if status == 401 {
                return Ok(FetchResult::AuthError);
            }
            if status >= 500 {
                return Ok(FetchResult::ServerError);
            }
            if status != 200 {
                return Ok(FetchResult::RequestError(status));
            }
            #[derive(Deserialize)]
            struct Body {
                access: bool,
            }
            let body: Body = response.body_mut().read_json().map_err(|_| ())?;
            Ok(FetchResult::Access(body.access))
        })
    }

    pub fn with_config_and_injectors(config: Config, fetcher: Fetcher, clock: Clock) -> Self {
        let jitter = (clock)() % config.refresh_ms.min(15_000).max(1);
        Self {
            fetcher,
            clock,
            jitter_ms: jitter,
            refresh_ms: config.refresh_ms,
            cache_ttl_ms: config.cache_ttl_ms,
            default_on_failure: config.default_on_failure,
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn is_premium(&self, guild_id: u64) -> Result<bool, PlatformError> {
        let id = guild_id.to_string();
        let now = (self.clock)();
        let cached = self
            .cache
            .lock()
            .ok()
            .and_then(|c| c.get(&id).map(|e| (e.access, e.at_ms)));
        if let Some((access, at)) = cached {
            if now - at < self.refresh_ms + self.jitter_ms {
                return Ok(access);
            }
        }
        match (self.fetcher)(&id, "") {
            Ok(FetchResult::Access(access)) => {
                if let Ok(mut cache) = self.cache.lock() {
                    cache.insert(id, CacheEntry { access, at_ms: now });
                }
                Ok(access)
            }
            Ok(FetchResult::AuthError) => Err(PlatformError::Auth),
            Ok(FetchResult::RequestError(s)) => Err(PlatformError::Request(s)),
            Ok(FetchResult::ServerError) | Err(()) => Ok(self.fallback(cached, now)),
        }
    }

    fn fallback(&self, cached: Option<(bool, u64)>, now: u64) -> bool {
        if let Some((access, at)) = cached {
            if now.saturating_sub(at) <= self.cache_ttl_ms {
                return access;
            }
        }
        self.default_on_failure
    }
}
