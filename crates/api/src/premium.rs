use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use platform_core::{Entitlement, GuildId, Timestamp};
use serde::Serialize;

use crate::directory::Directory;
use crate::rate_limit::RateLimiter;

pub type Clock = Arc<dyn Fn() -> Timestamp + Send + Sync>;

#[derive(Clone)]
pub struct AppState {
    directory: Arc<dyn Directory>,
    clock: Clock,
    grace_secs: u32,
    rate_limiter: Arc<RateLimiter>,
}

impl AppState {
    pub fn new(directory: Arc<dyn Directory>, clock: Clock, grace_secs: u32) -> Self {
        // 600 requêtes par fenêtre de 60 s = 10 req/s par produit.
        Self::with_limiter(
            directory,
            clock,
            grace_secs,
            Arc::new(RateLimiter::new(60, 600)),
        )
    }

    pub fn with_limiter(
        directory: Arc<dyn Directory>,
        clock: Clock,
        grace_secs: u32,
        rate_limiter: Arc<RateLimiter>,
    ) -> Self {
        Self {
            directory,
            clock,
            grace_secs,
            rate_limiter,
        }
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/premium/{guild_id}", get(premium))
        .with_state(state)
}

// Les identifiants Discord dépassent 2^53 : en JSON ils doivent être des chaînes, sinon
// un bot JavaScript les arrondit.
#[derive(Serialize)]
struct PremiumResponse {
    guild_id: String,
    status: &'static str,
    access: bool,
}

async fn premium(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(guild_id): Path<String>,
) -> Result<Json<PremiumResponse>, StatusCode> {
    let token = bearer_token(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    // Le jeton désigne le produit : un bot ne lit jamais les droits d'un autre produit.
    let product = state
        .directory
        .product_for_token(token)
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let now = (state.clock)();
    if !state.rate_limiter.check(product, now) {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    let guild = parse_guild_id(&guild_id).ok_or(StatusCode::BAD_REQUEST)?;
    let entitlement = state
        .directory
        .entitlement(guild, product)
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .unwrap_or_else(|| Entitlement::new(guild, product));
    let status = entitlement.status(now, state.grace_secs);
    Ok(Json(PremiumResponse {
        guild_id: guild.0.to_string(),
        status: status.as_str(),
        access: status.grants_access(),
    }))
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    value.strip_prefix("Bearer ").filter(|t| !t.is_empty())
}

// `u64::from_str` accepte un « + » initial : on exige des chiffres seuls.
fn parse_guild_id(raw: &str) -> Option<GuildId> {
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    raw.parse().ok().map(GuildId)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use platform_core::ProductId;
    use tower::ServiceExt;

    use super::*;
    use crate::directory::{DirectoryError, SqliteDirectory};
    use platform_store::{hash_token, SqliteStore};

    #[derive(Default)]
    struct Fake {
        tokens: HashMap<String, ProductId>,
        entitlements: HashMap<(u64, u64), Entitlement>,
        fail_tokens: bool,
        fail_entitlements: bool,
    }

    impl Directory for Fake {
        fn product_for_token(&self, token: &str) -> Result<Option<ProductId>, DirectoryError> {
            if self.fail_tokens {
                return Err(DirectoryError);
            }
            Ok(self.tokens.get(token).copied())
        }

        fn entitlement(
            &self,
            guild: GuildId,
            product: ProductId,
        ) -> Result<Option<Entitlement>, DirectoryError> {
            if self.fail_entitlements {
                return Err(DirectoryError);
            }
            Ok(self.entitlements.get(&(guild.0, product.0)).copied())
        }
    }

    fn fake() -> Fake {
        let mut f = Fake::default();
        f.tokens.insert("tok1".to_string(), ProductId(1));
        f.tokens.insert("tok2".to_string(), ProductId(2));
        f
    }

    fn grant(f: &mut Fake, guild: u64, product: u64, paid_until: i64, revoked: bool) {
        f.entitlements.insert(
            (guild, product),
            Entitlement {
                guild: GuildId(guild),
                product: ProductId(product),
                paid_until,
                revoked,
                canceled: false,
            },
        );
    }

    fn app(directory: impl Directory) -> Router {
        router(AppState::new(Arc::new(directory), Arc::new(|| 1_000), 100))
    }

    fn app_limited(directory: impl Directory, max: usize) -> Router {
        router(AppState::with_limiter(
            Arc::new(directory),
            Arc::new(|| 1_000),
            100,
            Arc::new(crate::rate_limit::RateLimiter::new(60, max)),
        ))
    }

    async fn call(app: Router, path: &str, auth: Option<&str>) -> (StatusCode, String) {
        let mut request = Request::builder().uri(path);
        if let Some(value) = auth {
            request = request.header(AUTHORIZATION, value);
        }
        let Ok(request) = request.body(Body::empty()) else {
            panic!("bad request");
        };
        let Ok(response) = app.oneshot(request).await;
        let status = response.status();
        let Ok(bytes) = to_bytes(response.into_body(), 1 << 16).await else {
            panic!("cannot read body");
        };
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    fn body(guild: &str, status: &str, access: bool) -> String {
        format!("{{\"guild_id\":\"{guild}\",\"status\":\"{status}\",\"access\":{access}}}")
    }

    #[tokio::test]
    async fn an_active_subscription_grants_access() {
        let mut f = fake();
        grant(&mut f, 10, 1, 2_000, false);
        let (code, text) = call(app(f), "/v1/premium/10", Some("Bearer tok1")).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(text, body("10", "active", true));
    }

    #[tokio::test]
    async fn a_late_renewal_is_in_grace_and_still_grants_access() {
        let mut f = fake();
        grant(&mut f, 10, 1, 950, false);
        let (_, text) = call(app(f), "/v1/premium/10", Some("Bearer tok1")).await;
        assert_eq!(text, body("10", "grace", true));
    }

    #[tokio::test]
    async fn an_ended_subscription_is_expired() {
        let mut f = fake();
        grant(&mut f, 10, 1, 900, false);
        let (_, text) = call(app(f), "/v1/premium/10", Some("Bearer tok1")).await;
        assert_eq!(text, body("10", "expired", false));
    }

    #[tokio::test]
    async fn a_revoked_subscription_is_expired() {
        let mut f = fake();
        grant(&mut f, 10, 1, 2_000, true);
        let (_, text) = call(app(f), "/v1/premium/10", Some("Bearer tok1")).await;
        assert_eq!(text, body("10", "expired", false));
    }

    #[tokio::test]
    async fn an_unknown_guild_is_expired_not_an_error() {
        let (code, text) = call(app(fake()), "/v1/premium/77", Some("Bearer tok1")).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(text, body("77", "expired", false));
    }

    #[tokio::test]
    async fn a_bot_cannot_see_another_products_subscriptions() {
        let mut f = fake();
        grant(&mut f, 10, 2, 2_000, false);
        let (_, text) = call(app(f), "/v1/premium/10", Some("Bearer tok1")).await;
        assert_eq!(text, body("10", "expired", false));
    }

    #[tokio::test]
    async fn large_guild_ids_stay_exact() {
        let id = "1234567890123456789";
        let (_, text) = call(
            app(fake()),
            &format!("/v1/premium/{id}"),
            Some("Bearer tok1"),
        )
        .await;
        assert_eq!(text, body(id, "expired", false));
    }

    #[tokio::test]
    async fn leading_zeros_are_normalised() {
        let (_, text) = call(app(fake()), "/v1/premium/0010", Some("Bearer tok1")).await;
        assert_eq!(text, body("10", "expired", false));
    }

    #[tokio::test]
    async fn requests_without_a_valid_bearer_token_are_unauthorized() {
        for auth in [
            None,
            Some("Basic tok1"),
            Some("Bearer "),
            Some("Bearer"),
            Some("tok1"),
            Some("Bearer nope"),
        ] {
            let (code, _) = call(app(fake()), "/v1/premium/10", auth).await;
            assert_eq!(code, StatusCode::UNAUTHORIZED, "{auth:?}");
        }
    }

    #[tokio::test]
    async fn malformed_guild_ids_are_bad_requests() {
        for id in ["abc", "+5", "-1", "1.5", "18446744073709551616", "1e3"] {
            let (code, _) = call(
                app(fake()),
                &format!("/v1/premium/{id}"),
                Some("Bearer tok1"),
            )
            .await;
            assert_eq!(code, StatusCode::BAD_REQUEST, "{id}");
        }
    }

    #[tokio::test]
    async fn the_largest_u64_is_a_valid_guild_id() {
        let (code, _) = call(
            app(fake()),
            "/v1/premium/18446744073709551615",
            Some("Bearer tok1"),
        )
        .await;
        assert_eq!(code, StatusCode::OK);
    }

    #[tokio::test]
    async fn authentication_is_checked_before_the_guild_id() {
        let (code, _) = call(app(fake()), "/v1/premium/abc", None).await;
        assert_eq!(code, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_failing_token_lookup_is_service_unavailable() {
        let mut f = fake();
        f.fail_tokens = true;
        let (code, _) = call(app(f), "/v1/premium/10", Some("Bearer tok1")).await;
        assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn a_failing_entitlement_lookup_is_service_unavailable_not_expired() {
        let mut f = fake();
        f.fail_entitlements = true;
        let (code, _) = call(app(f), "/v1/premium/10", Some("Bearer tok1")).await;
        assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn unknown_routes_are_not_found() {
        let (code, _) = call(app(fake()), "/v1/other", Some("Bearer tok1")).await;
        assert_eq!(code, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn rate_limit_returns_429_when_exceeded() {
        let a = app_limited(fake(), 2);
        let (c1, _) = call(a.clone(), "/v1/premium/10", Some("Bearer tok1")).await;
        let (c2, _) = call(a.clone(), "/v1/premium/10", Some("Bearer tok1")).await;
        let (c3, _) = call(a, "/v1/premium/10", Some("Bearer tok1")).await;
        assert_eq!(c1, StatusCode::OK);
        assert_eq!(c2, StatusCode::OK);
        assert_eq!(c3, StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn rate_limit_is_per_product_not_global() {
        let a = app_limited(fake(), 1);
        let (c1, _) = call(a.clone(), "/v1/premium/10", Some("Bearer tok1")).await;
        let (c2, _) = call(a.clone(), "/v1/premium/10", Some("Bearer tok1")).await;
        // tok2 est un produit différent : pas bloqué.
        let (c3, _) = call(a, "/v1/premium/10", Some("Bearer tok2")).await;
        assert_eq!(c1, StatusCode::OK);
        assert_eq!(c2, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(c3, StatusCode::OK);
    }

    #[tokio::test]
    async fn works_end_to_end_on_top_of_sqlite() {
        let Ok(store) = SqliteStore::in_memory() else {
            panic!("cannot open in-memory store");
        };
        let e = Entitlement {
            guild: GuildId(10),
            product: ProductId(1),
            paid_until: 5_000,
            revoked: false,
            canceled: false,
        };
        assert!(store.save_entitlement(&e).is_ok());
        assert!(store.add_token(&hash_token("tok1"), ProductId(1)).is_ok());
        let directory = SqliteDirectory::new(store);
        let (code, text) = call(app(directory), "/v1/premium/10", Some("Bearer tok1")).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(text, body("10", "active", true));
    }
}
