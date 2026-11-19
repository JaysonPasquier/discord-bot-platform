// Ce que le créateur fait sur un bot : renommer, arrêter, supprimer, coller un jeton, déployer.
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use platform_core::ProductId;
use platform_runner::{container_name, tier_name, Tier};
use platform_store::{credential_context, BotCredential, StoreError};
use serde::{Deserialize, Serialize};

use super::auth::User;
use super::bots::owned;
use super::dto::DeploymentDto;
use super::error::ApiError;
use crate::bots::{clean_token, open_token, stored_credential};
use crate::discord::DiscordError;
use crate::engine::{launch, parse_tier};
use crate::state::WebState;

pub(crate) fn tier_of(raw: &str) -> Result<Tier, ApiError> {
    parse_tier(raw)
        .ok_or_else(|| ApiError::bad_request("unknown_version", "Choose free or premium."))
}

pub(crate) fn slow_down(state: &WebState, key: &str, what: &str) -> Result<(), ApiError> {
    // Vérification persistante en base : résiste aux redémarrages.
    let store = state.store.lock().map_err(|_| ApiError::internal())?;
    store
        .check_and_record_deploy(key, (state.clock)(), state.config.deploy_cooldown_secs)
        .map_err(|_| ApiError::internal())?
        .map_err(|seconds| {
            ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "slow_down",
                format!("Wait {seconds} more second(s) before {what} again."),
            )
        })
}

#[derive(Deserialize)]
pub(crate) struct Rename {
    name: String,
}

pub(crate) async fn rename(
    State(state): State<WebState>,
    User(auth): User,
    Path(id): Path<u64>,
    Json(form): Json<Rename>,
) -> Result<StatusCode, ApiError> {
    owned(&state, &auth, id)?;
    if !platform_config::valid_name(&form.name) {
        return Err(ApiError::bad_request(
            "invalid_name",
            "A bot name has 3 to 32 characters: letters, digits, spaces, - and _.",
        ));
    }
    let store = state.store.lock().map_err(|_| ApiError::internal())?;
    store
        .rename_product(ProductId(id), &form.name)
        .map_err(|_| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(crate) struct StopForm {
    // Absent : les deux versions.
    tier: Option<String>,
}

async fn stop_tiers(state: &WebState, id: u64, tiers: &[Tier]) {
    for tier in tiers {
        let name = container_name(id, *tier);
        let containers = state.containers.clone();
        let _ = tokio::task::spawn_blocking(move || containers.stop(&name)).await;
    }
}

pub(crate) async fn stop(
    State(state): State<WebState>,
    User(auth): User,
    Path(id): Path<u64>,
    Json(form): Json<StopForm>,
) -> Result<StatusCode, ApiError> {
    owned(&state, &auth, id)?;
    let tiers = match form.tier {
        Some(raw) => vec![tier_of(&raw)?],
        None => vec![Tier::Free, Tier::Premium],
    };
    stop_tiers(&state, id, &tiers).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(crate) struct DeleteForm {
    confirm: String,
}

// Irréversible : le nom du bot doit être retapé.
pub(crate) async fn delete(
    State(state): State<WebState>,
    User(auth): User,
    Path(id): Path<u64>,
    Json(form): Json<DeleteForm>,
) -> Result<StatusCode, ApiError> {
    let product = owned(&state, &auth, id)?;
    if form.confirm != product.name {
        return Err(ApiError::bad_request(
            "name_mismatch",
            "The name you typed does not match this bot. Nothing was deleted.",
        ));
    }
    stop_tiers(&state, id, &[Tier::Free, Tier::Premium]).await;
    let store = state.store.lock().map_err(|_| ApiError::internal())?;
    store
        .delete_product(ProductId(id))
        .map_err(|_| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}

// Pas de `Debug` : le jeton ne doit jamais finir dans un journal.
#[derive(Deserialize)]
pub(crate) struct TokenForm {
    tier: String,
    token: String,
}

#[derive(Serialize)]
pub(crate) struct Connected {
    name: String,
    client_id: String,
}

pub(crate) async fn save_token(
    State(state): State<WebState>,
    User(auth): User,
    Path(id): Path<u64>,
    Json(form): Json<TokenForm>,
) -> Result<Json<Connected>, ApiError> {
    owned(&state, &auth, id)?;
    let tier = tier_of(&form.tier)?;
    let token = clean_token(&form.token).ok_or_else(|| {
        ApiError::bad_request(
            "invalid_token",
            "Paste the bot token exactly as the Discord portal shows it (Bot tab, Reset Token).",
        )
    })?;
    // Chaque essai interroge Discord : même délai minimal que pour un déploiement.
    slow_down(
        &state,
        &format!("verify-{}", container_name(id, tier)),
        "checking a token",
    )?;
    let bots = state.bots.clone();
    let checked = {
        let token = token.clone();
        tokio::task::spawn_blocking(move || bots.verify(&token)).await
    };
    let app = match checked {
        Ok(Ok(app)) => app,
        Ok(Err(DiscordError::Rejected)) => {
            return Err(ApiError::bad_request(
                "token_refused",
                "Discord refused this token. It may be wrong, or already reset. Copy a fresh one from the Bot tab of your application.",
            ))
        }
        _ => {
            return Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                "discord_unreachable",
                "Discord did not answer. Try again in a minute.",
            ))
        }
    };
    let context = credential_context(ProductId(id), tier_name(tier));
    let sealed = state
        .vault
        .seal(&token, &context)
        .map_err(|_| ApiError::internal())?;
    let credential = BotCredential {
        client_id: app.id.clone(),
        bot_name: app.name.clone(),
        token_sealed: sealed,
    };
    let saved = state
        .store
        .lock()
        .map_err(|_| ApiError::internal())?
        .save_credential(ProductId(id), tier_name(tier), &credential);
    match saved {
        Ok(()) => Ok(Json(Connected {
            name: app.name,
            client_id: app.id,
        })),
        Err(StoreError::Duplicate) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "app_in_use",
            "This Discord application is already registered on another bot or version. Each version needs its own Discord application.",
        )),
        Err(_) => Err(ApiError::internal()),
    }
}

#[derive(Deserialize)]
pub(crate) struct TierForm {
    tier: String,
}

// Démarre l'image d'exemple avec le jeton enregistré (sans dépôt).
pub(crate) async fn run_sample(
    State(state): State<WebState>,
    User(auth): User,
    Path(id): Path<u64>,
    Json(form): Json<TierForm>,
) -> Result<StatusCode, ApiError> {
    owned(&state, &auth, id)?;
    let tier = tier_of(&form.tier)?;
    let credential = stored_credential(&state, id, tier)
        .map_err(|_| ApiError::internal())?
        .ok_or_else(|| {
            ApiError::bad_request(
                "no_bot",
                "Connect this version's Discord bot (paste its token) before deploying.",
            )
        })?;
    let discord_token = open_token(&state, id, tier, &credential).ok_or_else(|| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "token_unreadable",
            "The stored token cannot be decrypted. Save the token again.",
        )
    })?;
    slow_down(&state, &container_name(id, tier), "deploying this version")?;
    let image = state.config.bot_image.clone();
    let launched = {
        let state = state.clone();
        tokio::task::spawn_blocking(move || launch(&state, id, tier, &image, &discord_token)).await
    };
    match launched {
        Ok(Ok(())) => Ok(StatusCode::ACCEPTED),
        _ => Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "docker_failed",
            "Docker did not start the container. Check that Docker Desktop is running and the image exists.",
        )),
    }
}

#[derive(Serialize)]
pub(crate) struct DeploymentDetail {
    #[serde(flatten)]
    deployment: DeploymentDto,
    log: String,
}

#[derive(Serialize)]
pub(crate) struct DeploymentView {
    now: i64,
    deployment: DeploymentDetail,
}

pub(crate) fn find_deployment(
    state: &WebState,
    id: u64,
    deployment: u64,
) -> Result<platform_store::Deployment, ApiError> {
    state
        .store
        .lock()
        .map_err(|_| ApiError::internal())?
        .deployment(deployment)
        .map_err(|_| ApiError::internal())?
        .filter(|d| d.product.0 == id)
        .ok_or_else(ApiError::not_found)
}

pub(crate) async fn deployment(
    State(state): State<WebState>,
    User(auth): User,
    Path((id, deployment)): Path<(u64, u64)>,
) -> Result<Json<DeploymentView>, ApiError> {
    owned(&state, &auth, id)?;
    let found = find_deployment(&state, id, deployment)?;
    Ok(Json(DeploymentView {
        now: (state.clock)(),
        deployment: DeploymentDetail {
            deployment: DeploymentDto::from(&found),
            log: found.log,
        },
    }))
}
