// Les bots du créateur : liste, création, vue d'ensemble.
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use platform_core::ProductId;
use platform_runner::{container_name, tier_name, Tier};
use platform_store::Product;
use serde::{Deserialize, Serialize};

use super::auth::User;
use super::dto::{AppDto, BotSummaryDto, ContainerDto, DeploymentDto, VersionDto};
use super::error::ApiError;
use crate::discord::invite_url;
use crate::icons::bot_slot;
use crate::session::Auth;
use crate::state::WebState;
use crate::summary::gather_cards;

// Un bot qui n'appartient pas à l'utilisateur répond « introuvable », comme s'il n'existait pas.
pub(crate) fn owned(state: &WebState, auth: &Auth, id: u64) -> Result<Product, ApiError> {
    let found = state
        .store
        .lock()
        .map_err(|_| ApiError::internal())?
        .product(ProductId(id))
        .map_err(|_| ApiError::internal())?;
    found
        .filter(|p| p.owner == auth.user_id)
        .ok_or_else(ApiError::not_found)
}

#[derive(Serialize)]
pub(crate) struct BotList {
    now: i64,
    bots: Vec<BotSummaryDto>,
}

pub(crate) async fn list(
    State(state): State<WebState>,
    User(auth): User,
) -> Result<Json<BotList>, ApiError> {
    let now = (state.clock)();
    let mut cards = {
        let store = state.store.lock().map_err(|_| ApiError::internal())?;
        let products = store
            .products_of(auth.user_id)
            .map_err(|_| ApiError::internal())?;
        gather_cards(&store, products, now, state.config.grace_secs)
            .map_err(|()| ApiError::internal())?
    };
    let containers = state.containers.clone();
    let ids: Vec<(u64, Tier)> = cards
        .iter()
        .flat_map(|c| {
            [
                (c.product.id.0, Tier::Free, c.free.has_credential),
                (c.product.id.0, Tier::Premium, c.premium.has_credential),
            ]
        })
        .filter(|(_, _, has_credential)| *has_credential)
        .map(|(id, tier, _)| (id, tier))
        .collect();
    let crash_looping: Vec<(u64, Tier)> = tokio::task::spawn_blocking(move || {
        ids.into_iter()
            .filter(|(id, tier)| {
                matches!(
                    containers.state(&container_name(*id, *tier)),
                    Ok(Some(platform_runner::ContainerState::Restarting))
                )
            })
            .collect()
    })
    .await
    .map_err(|_| ApiError::internal())?;
    for card in &mut cards {
        card.free.crash_looping = crash_looping.contains(&(card.product.id.0, Tier::Free));
        card.premium.crash_looping = crash_looping.contains(&(card.product.id.0, Tier::Premium));
    }
    Ok(Json(BotList {
        now,
        bots: cards.iter().map(BotSummaryDto::from).collect(),
    }))
}

#[derive(Deserialize)]
pub(crate) struct NewBot {
    name: String,
}

#[derive(Serialize)]
pub(crate) struct Created {
    id: u64,
}

pub(crate) async fn create(
    State(state): State<WebState>,
    User(auth): User,
    Json(form): Json<NewBot>,
) -> Result<(StatusCode, Json<Created>), ApiError> {
    if !platform_config::valid_name(&form.name) {
        return Err(ApiError::bad_request(
            "invalid_name",
            "A bot name has 3 to 32 characters: letters, digits, spaces, - and _.",
        ));
    }
    let store = state.store.lock().map_err(|_| ApiError::internal())?;
    let count = store
        .products_of(auth.user_id)
        .map_err(|_| ApiError::internal())?
        .len();
    let max = state.config.max_products;
    if count >= usize::try_from(max).unwrap_or(usize::MAX) {
        return Err(ApiError::forbidden(
            "limit_reached",
            format!("You can have at most {max} bots for now."),
        ));
    }
    let id = store
        .create_product(auth.user_id, &form.name, (state.clock)())
        .map_err(|_| ApiError::internal())?;
    Ok((StatusCode::CREATED, Json(Created { id: id.0 })))
}

#[derive(Serialize)]
pub(crate) struct Overview {
    now: i64,
    id: u64,
    name: String,
    slot: &'static str,
    versions: Vec<VersionDto>,
    deployments: Vec<DeploymentDto>,
    premium_subscribers: usize,
}

pub(crate) const MAX_DEPLOYMENTS: u32 = 500;

pub(crate) async fn overview(
    State(state): State<WebState>,
    User(auth): User,
    Path(id): Path<u64>,
) -> Result<Json<Overview>, ApiError> {
    let product = owned(&state, &auth, id)?;
    let now = (state.clock)();
    let (sources, credentials, deployments, premium_subscribers) = {
        let store = state.store.lock().map_err(|_| ApiError::internal())?;
        let read = |tier: Tier| {
            let name = tier_name(tier);
            Ok::<_, ApiError>((
                store
                    .source(product.id, name)
                    .map_err(|_| ApiError::internal())?,
                store
                    .credential(product.id, name)
                    .map_err(|_| ApiError::internal())?,
            ))
        };
        let (free, premium) = (read(Tier::Free)?, read(Tier::Premium)?);
        let deployments = store
            .deployments_of(product.id, MAX_DEPLOYMENTS)
            .map_err(|_| ApiError::internal())?;
        let subscribers = store
            .entitlements_of(product.id)
            .map_err(|_| ApiError::internal())?
            .iter()
            .filter(|e| {
                crate::billing::counts_as_active_subscriber(e, now, state.config.grace_secs)
            })
            .count();
        (
            [free.0, premium.0],
            [free.1, premium.1],
            deployments,
            subscribers,
        )
    };
    let containers = state.containers.clone();
    let states = tokio::task::spawn_blocking(move || {
        [Tier::Free, Tier::Premium].map(|tier| {
            let name = container_name(id, tier);
            let state = containers.state(&name).ok();
            let logs = if matches!(state, Some(Some(_))) {
                containers.logs(&name, 30).unwrap_or_default()
            } else {
                String::new()
            };
            (state, logs)
        })
    })
    .await
    .map_err(|_| ApiError::internal())?;
    let versions = [Tier::Free, Tier::Premium]
        .into_iter()
        .zip(sources)
        .zip(credentials)
        .zip(states)
        .map(
            |(((tier, source), credential), (container, logs))| VersionDto {
                tier: tier_name(tier),
                source: source.as_ref().map(Into::into),
                app: credential.map(|c| AppDto {
                    invite_url: invite_url(&c.client_id),
                    name: c.bot_name,
                    client_id: c.client_id,
                }),
                container: ContainerDto::from_state(&container),
                logs,
            },
        )
        .collect();
    Ok(Json(Overview {
        now,
        id,
        name: product.name,
        slot: bot_slot(id),
        versions,
        deployments: deployments.iter().map(DeploymentDto::from).collect(),
        premium_subscribers,
    }))
}
