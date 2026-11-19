#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used)]
// Une page refusée est une `Response` complète renvoyée tôt : le schéma habituel d'axum, hors chemin critique.
#![allow(clippy::result_large_err)]

mod account_pages;
mod admin_page;
mod api;
mod api_token_pages;
mod base64;
mod billing;
mod bot_app;
mod bot_http;
mod bot_pages;
mod bots;
mod containers;
mod dashboard;
mod deployments;
mod developers;
mod discord;
mod discord_http;
mod docs;
mod engine;
mod env_vars_pages;
mod github_pages;
mod guild_profile;
mod html;
mod icons;
mod image;
mod listing;
mod listing_pages;
mod login;
mod products;
mod profile_pages;
mod profiles;
mod project_page;
mod render;
mod reports;
mod repos;
mod security_pages;
mod session;
mod state;
mod stats;
mod stripe_webhook;
mod summary;
mod views;

use axum::extract::DefaultBodyLimit;
use axum::http::header::{HeaderName, HeaderValue, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{middleware, Router};

pub use bot_app::{BotApp, BotVerifier};
pub use bot_http::HttpBotVerifier;
pub use containers::{ContainerError, Containers, DockerContainers};
pub use discord::{Discord, DiscordError, DiscordUser, Guild, NoDiscord};
pub use discord_http::HttpDiscord;
pub use engine::{enqueue, recover_interrupted, Engine};
pub use github_pages::poll_once;
pub use guild_profile::{GuildProfiles, HttpGuildProfiles, ProfileError, ProfilePatch};
pub use state::{AuthLimiter, Clock, DeployLimiter, Secret, WebConfig, WebState};
pub use stats::snapshot_all;

// Deux images au plus, plus 1 Mo pour le reste du formulaire.
fn upload_limit(max_image_bytes: usize) -> usize {
    max_image_bytes
        .saturating_mul(2)
        .saturating_add(1024 * 1024)
}

const CSP: &str = "default-src 'none'; style-src 'self'; script-src 'self'; font-src 'self'; img-src 'self'; connect-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'";

async fn css() -> Response {
    ([(CONTENT_TYPE, "text/css; charset=utf-8")], html::CSS).into_response()
}

async fn script() -> Response {
    ([(CONTENT_TYPE, "text/javascript; charset=utf-8")], html::JS).into_response()
}

async fn logo() -> Response {
    ([(CONTENT_TYPE, "image/png")], html::LOGO_PNG).into_response()
}

async fn font_geist() -> Response {
    ([(CONTENT_TYPE, "font/woff2")], html::FONT_GEIST).into_response()
}

async fn font_geist_mono() -> Response {
    ([(CONTENT_TYPE, "font/woff2")], html::FONT_GEIST_MONO).into_response()
}

async fn security_headers(mut response: Response) -> Response {
    // Les ressources statiques (CSS, JS, polices, images) sont immuables versionnées : longue durée.
    // Tout le reste (HTML, JSON, réponses API) ne doit pas être mis en cache.
    let is_static = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| {
            ct.starts_with("text/css")
                || ct.starts_with("text/javascript")
                || ct.starts_with("application/javascript")
                || ct.starts_with("font/")
                || ct.starts_with("image/")
        })
        .unwrap_or(false);
    let headers = response.headers_mut();
    for (name, value) in [
        ("content-security-policy", CSP),
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        ("referrer-policy", "no-referrer"),
        (
            "permissions-policy",
            "camera=(), microphone=(), geolocation=(), payment=(), usb=()",
        ),
    ] {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    let cache_value = if is_static {
        HeaderValue::from_static("public, max-age=31536000, immutable")
    } else {
        HeaderValue::from_static("no-store")
    };
    headers.insert(HeaderName::from_static("cache-control"), cache_value);
    response
}

pub fn router(state: WebState) -> Router {
    let upload = upload_limit(state.config.max_image_bytes);
    Router::new()
        .nest("/api", api::routes())
        .route("/", get(login::home))
        .route("/login", get(login::login))
        .route("/auth/discord", get(login::discord_redirect))
        .route("/auth/callback", get(login::callback))
        .route("/dev-login", post(login::dev_login))
        .route("/logout", post(login::logout))
        .route("/dashboard", get(dashboard::dashboard))
        .route("/market", get(listing::market))
        .route("/bots/{id}", get(listing::bot))
        .route(
            "/accept-cgu",
            get(login::accept_cgu_get).post(login::accept_cgu_post),
        )
        .route("/docs", get(docs::index))
        .route("/docs/cgu", get(docs::cgu))
        .route("/docs/{slug}", get(docs::show))
        .route("/sdk", get(docs::sdk))
        .route("/products", post(products::create))
        .route("/new", get(account_pages::new_bot))
        .route("/servers", get(account_pages::servers))
        .route("/products/{id}", get(project_page::show))
        .route("/products/{id}/source", get(bot_pages::source))
        .route(
            "/products/{id}/source/{tier}/auto",
            post(bot_pages::toggle_auto),
        )
        .route(
            "/products/{id}/bot",
            get(bot_pages::discord).post(bots::save),
        )
        .route(
            "/products/{id}/settings",
            get(bot_pages::settings).post(bot_pages::rename),
        )
        .route("/products/{id}/stop-all", post(bot_pages::stop_all))
        .route("/products/{id}/delete", post(bot_pages::delete))
        .route("/products/{id}/cede", post(bot_pages::cede))
        .route("/products/{id}/webhook", post(bot_pages::save_webhook))
        .route(
            "/products/{id}/env",
            get(env_vars_pages::show).post(env_vars_pages::add),
        )
        .route("/products/{id}/env/delete", post(env_vars_pages::delete))
        .route(
            "/account/profile",
            get(profile_pages::show).post(profile_pages::save),
        )
        .route(
            "/account/delete",
            get(account_pages::delete_confirm).post(account_pages::delete_account),
        )
        .route(
            "/account/tokens",
            get(api_token_pages::list).post(api_token_pages::create),
        )
        .route("/account/tokens/{id}/delete", post(api_token_pages::delete))
        .route("/account/security", get(security_pages::show))
        .route("/account/security/email", post(security_pages::start_email))
        .route(
            "/account/security/email/verify",
            get(security_pages::verify_email),
        )
        .route(
            "/account/security/totp/setup",
            get(security_pages::totp_setup),
        )
        .route(
            "/account/security/totp/enable",
            post(security_pages::totp_enable),
        )
        .route(
            "/account/security/totp/disable",
            post(security_pages::totp_disable),
        )
        .route(
            "/products/{id}/listing",
            get(listing_pages::show).post(listing_pages::create),
        )
        .route(
            "/products/{id}/listing/{plan_id}/delete",
            post(listing_pages::delete_plan),
        )
        .route("/developers/{id}", get(developers::show))
        .route("/bots/{id}/reports", post(reports::submit_report))
        .route(
            "/products/{id}/reports/{report_id}/respond",
            post(reports::respond_to_report),
        )
        .route(
            "/products/{id}/source/{tier}",
            get(repos::wizard).post(repos::save),
        )
        .route("/products/{id}/source/{tier}/remove", post(repos::remove))
        .route("/products/{id}/deploy/{tier}", get(deployments::choose))
        .route(
            "/products/{id}/deployments",
            get(bot_pages::deployments).post(deployments::create),
        )
        .route(
            "/products/{id}/deployments/{deployment}",
            get(deployments::detail),
        )
        .route(
            "/products/{id}/deployments/{deployment}/redeploy",
            post(deployments::redeploy),
        )
        .route(
            "/products/{id}/deployments/{deployment}/rebuild",
            post(deployments::rebuild),
        )
        .route("/github", get(github_pages::show))
        .route("/github/install", get(github_pages::install))
        .route("/github/installed", get(github_pages::installed))
        .route("/github/connect", get(github_pages::connect))
        .route("/github/callback", get(github_pages::callback))
        .route("/github/webhook", post(github_pages::webhook))
        .route("/webhooks/stripe", post(stripe_webhook::webhook))
        .route("/products/{id}/deploy", post(products::deploy))
        .route("/products/{id}/stop", post(products::stop))
        .route("/admin", get(admin_page::show))
        .route("/admin/bots/{id}/suspend", post(admin_page::suspend))
        .route("/admin/bots/{id}/unsuspend", post(admin_page::unsuspend))
        .route("/subscribe", post(billing::subscribe))
        .route("/subscriptions/cancel", post(billing::cancel))
        .route("/subscriptions/{product}/{guild}", get(profiles::show))
        .route(
            "/subscriptions/{product}/{guild}/profile",
            post(profiles::apply).layer(DefaultBodyLimit::max(upload)),
        )
        .route("/static/app.css", get(css))
        .route("/static/app.js", get(script))
        .route("/logo.png", get(logo))
        .route("/static/fonts/Geist-Variable.woff2", get(font_geist))
        .route(
            "/static/fonts/GeistMono-Variable.woff2",
            get(font_geist_mono),
        )
        .layer(middleware::map_response(security_headers))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_upload_limit_is_two_images_plus_one_megabyte() {
        assert_eq!(upload_limit(2 * 1024 * 1024), 5 * 1024 * 1024);
        assert_eq!(upload_limit(10 * 1024 * 1024), 21 * 1024 * 1024);
        assert_eq!(upload_limit(usize::MAX), usize::MAX);
    }
}
