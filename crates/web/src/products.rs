use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use platform_core::ProductId;
use platform_runner::container_name;
use platform_store::Product;
use serde::Deserialize;

use crate::bots::{open_token, stored_credential};
use crate::engine::{launch, parse_tier};
use crate::render::{
    message, message_back, not_found, redirect, require, require_csrf, server_error,
};
use crate::session::Auth;
use crate::state::WebState;

#[derive(Deserialize)]
pub(crate) struct CreateForm {
    name: String,
    csrf: String,
}

#[derive(Deserialize)]
pub(crate) struct TierForm {
    tier: String,
    csrf: String,
}

// Un produit qui n'appartient pas à l'utilisateur répond « introuvable », comme s'il n'existait pas.
pub(crate) fn owned_product(state: &WebState, auth: &Auth, id: u64) -> Result<Product, Response> {
    let found = state
        .store
        .lock()
        .map_err(|_| server_error(state, Some(auth)))?
        .product(ProductId(id))
        .map_err(|_| server_error(state, Some(auth)))?;
    found
        .filter(|p| p.owner == auth.user_id)
        .ok_or_else(|| not_found(state, auth))
}

pub(crate) async fn create(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<CreateForm>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    if let Err(response) = require_csrf(&state, &auth, &form.csrf) {
        return response;
    }
    if !platform_config::valid_name(&form.name) {
        return message(
            StatusCode::BAD_REQUEST,
            &state,
            Some(&auth),
            "Invalid name",
            "A bot name has 3 to 32 characters: letters, digits, spaces, - and _.",
        );
    }
    let owned = state
        .store
        .lock()
        .map(|s| s.products_of(auth.user_id).map(|mine| mine.len()));
    let Ok(Ok(count)) = owned else {
        return server_error(&state, Some(&auth));
    };
    let max = state.config.max_products;
    if count >= usize::try_from(max).unwrap_or(usize::MAX) {
        return message(
            StatusCode::FORBIDDEN,
            &state,
            Some(&auth),
            "Limit reached",
            &format!("You can have at most {max} bots for now."),
        );
    }
    let created = state
        .store
        .lock()
        .map(|s| s.create_product(auth.user_id, &form.name, (state.clock)()));
    match created {
        Ok(Ok(id)) => redirect(&format!("/products/{}", id.0)),
        _ => server_error(&state, Some(&auth)),
    }
}

pub(crate) async fn deploy(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
    Form(form): Form<TierForm>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    if let Err(response) = require_csrf(&state, &auth, &form.csrf) {
        return response;
    }
    let product = match owned_product(&state, &auth, id) {
        Ok(p) => p,
        Err(response) => return response,
    };
    let Some(tier) = parse_tier(&form.tier) else {
        return message(
            StatusCode::BAD_REQUEST,
            &state,
            Some(&auth),
            "Unknown version",
            "Choose free or premium.",
        );
    };
    let name = container_name(id, tier);
    let credential = match stored_credential(&state, id, tier) {
        Ok(Some(c)) => c,
        Ok(None) => {
            return message_back(
                StatusCode::BAD_REQUEST,
                &state,
                Some(&auth),
                "No bot connected",
                "Connect this version's Discord bot (paste its token) before deploying.",
                &format!("/products/{id}"),
            )
        }
        Err(_) => return server_error(&state, Some(&auth)),
    };
    let Some(discord_token) = open_token(&state, id, tier, &credential) else {
        return message_back(
            StatusCode::INTERNAL_SERVER_ERROR,
            &state,
            Some(&auth),
            "Stored token unreadable",
            "The stored token cannot be decrypted (was the master key changed?). Save the token again.",
            &format!("/products/{id}"),
        );
    };
    // Avant de toucher au jeton : un déploiement refusé ne doit pas invalider celui du container en marche.
    let wait =
        state
            .limiter
            .check_and_record(&name, (state.clock)(), state.config.deploy_cooldown_secs);
    if let Err(seconds) = wait {
        return message_back(
            StatusCode::TOO_MANY_REQUESTS,
            &state,
            Some(&auth),
            "Slow down",
            &format!("Wait {seconds} more second(s) before deploying this version again."),
            &format!("/products/{id}"),
        );
    }
    // Le jeton d'API n'existe que dans `launch` : haché en base, injecté dans le container, jamais affiché.
    let image = state.config.bot_image.clone();
    let launched = {
        let state = state.clone();
        tokio::task::spawn_blocking(move || launch(&state, id, tier, &image, &discord_token)).await
    };
    let (status, text) = match launched {
        Ok(Ok(())) => (StatusCode::OK, "The container is starting. Reload the bot page in a few seconds."),
        _ => (StatusCode::BAD_GATEWAY, "Docker did not start the container. Check that Docker Desktop is running and the image exists."),
    };
    message_back(
        status,
        &state,
        Some(&auth),
        &format!("Deploy {}", product.name),
        text,
        &format!("/products/{id}"),
    )
}

pub(crate) async fn stop(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
    Form(form): Form<TierForm>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    if let Err(response) = require_csrf(&state, &auth, &form.csrf) {
        return response;
    }
    if let Err(response) = owned_product(&state, &auth, id) {
        return response;
    }
    let Some(tier) = parse_tier(&form.tier) else {
        return message(
            StatusCode::BAD_REQUEST,
            &state,
            Some(&auth),
            "Unknown version",
            "Choose free or premium.",
        );
    };
    let name = container_name(id, tier);
    let containers = state.containers.clone();
    let _ = tokio::task::spawn_blocking(move || containers.stop(&name)).await;
    redirect(&format!("/products/{id}"))
}
