// L'API JSON du frontend (`/api/*`). Elle ne rend aucune page : le frontend vit à part et peut
// démarrer sans le serveur (il affiche alors que le serveur est injoignable), comme le serveur peut
// démarrer sans lui. Les bots, eux, parlent à `/v1/premium` (crate `platform-api`), pas à celle-ci.
mod admin;
mod auth;
mod bot_actions;
mod bots;
mod docs;
mod dto;
mod env_vars;
mod error;
mod listing;
mod market;
mod profile;
mod servers;
mod session;
mod source;
mod stats;
mod token_routes;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;

use crate::state::WebState;

// Plafond technique du corps ; la limite réglable (`max_image_bytes`) est vérifiée par la route.
const UPLOAD_CEILING: usize = 16 * 1024 * 1024;

pub(crate) fn routes() -> Router<WebState> {
    Router::new()
        .route("/session", get(session::read))
        .route("/dev-login", post(session::dev_login))
        .route("/logout", post(session::logout))
        .route("/sessions", get(session::list_sessions))
        .route("/sessions/{id}/revoke", post(session::revoke_session))
        .route("/bots", get(bots::list).post(bots::create))
        .route("/bots/{id}", get(bots::overview))
        .route("/bots/{id}/stats", get(stats::read))
        .route("/bots/{id}/rename", post(bot_actions::rename))
        .route("/bots/{id}/stop", post(bot_actions::stop))
        .route("/bots/{id}/delete", post(bot_actions::delete))
        .route("/bots/{id}/token", post(bot_actions::save_token))
        .route("/bots/{id}/run", post(bot_actions::run_sample))
        .route("/bots/{id}/deployments", post(source::deploy_branch))
        .route(
            "/bots/{id}/deployments/{deployment}",
            get(bot_actions::deployment),
        )
        .route(
            "/bots/{id}/deployments/{deployment}/redeploy",
            post(source::redeploy),
        )
        .route(
            "/bots/{id}/deployments/{deployment}/rebuild",
            post(source::rebuild),
        )
        .route("/bots/{id}/source/{tier}", post(source::save_source))
        .route("/bots/{id}/source/{tier}/auto", post(source::set_auto))
        .route(
            "/bots/{id}/source/{tier}/remove",
            post(source::remove_source),
        )
        .route("/bots/{id}/listing", get(listing::read).post(listing::save))
        .route(
            "/bots/{id}/listing/image/{kind}",
            post(listing::upload).layer(DefaultBodyLimit::max(UPLOAD_CEILING)),
        )
        .route(
            "/bots/{id}/listing/image/{kind}/remove",
            post(listing::remove_image),
        )
        .route("/market/{id}/image/{kind}", get(listing::image))
        .route("/docs", get(docs::read))
        .route("/market", get(market::market))
        .route("/market/{id}", get(market::listing))
        .route("/subscriptions", get(servers::list))
        .route("/subscribe", post(servers::subscribe))
        .route("/subscriptions/cancel", post(servers::cancel))
        .route("/subscriptions/{product}/{guild}", get(profile::get))
        .route(
            "/subscriptions/{product}/{guild}/profile",
            post(profile::apply).layer(DefaultBodyLimit::max(UPLOAD_CEILING)),
        )
        .route("/github", get(source::github))
        .route("/github/installations/{id}/repos", get(source::repos))
        .route("/github/branches", get(source::branches))
        .route(
            "/bots/{id}/env/{tier}",
            get(env_vars::list).post(env_vars::set),
        )
        .route("/bots/{id}/env/{tier}/delete", post(env_vars::delete))
        .route("/v1/me", get(token_routes::me))
        .route("/v1/bots", get(token_routes::bots))
        .route("/admin", get(admin::overview))
        .route(
            "/admin/subscriptions/revoke",
            post(admin::revoke_subscription),
        )
        .route(
            "/admin/subscriptions/restore",
            post(admin::restore_subscription),
        )
        .route("/admin/bots/{id}/stop/{tier}", post(admin::stop_bot))
        .route("/admin/bots/{id}/delete", post(admin::delete_bot))
}
