// Panneau d'administration : vue globale de la plateforme réservée à PLATFORM_ADMIN_USER_ID.
use axum::extract::{Path, State};
use axum::Json;
use platform_core::{Entitlement, GuildId, ProductId};
use platform_runner::container_name;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use super::auth::Admin;
use super::error::ApiError;
use crate::engine::parse_tier;
use crate::state::WebState;

#[derive(Serialize)]
pub(crate) struct AdminOverview {
    users: usize,
    bots: usize,
    revenue_cents: u64,
    active_subscriptions: usize,
    bots_list: Vec<AdminBot>,
    users_list: Vec<AdminUser>,
    recent_deployments: Vec<AdminDeploy>,
    subscriptions: Vec<AdminSubscription>,
}

#[derive(Serialize)]
pub(crate) struct AdminBot {
    id: u64,
    owner_id: String,
    owner_name: String,
    name: String,
}

#[derive(Serialize)]
pub(crate) struct AdminUser {
    id: String,
    name: String,
}

#[derive(Serialize)]
pub(crate) struct AdminDeploy {
    id: u64,
    product_id: u64,
    product_name: String,
    tier: String,
    status: String,
    trigger: String,
    branch: String,
    created_at: i64,
}

#[derive(Serialize)]
pub(crate) struct AdminSubscription {
    guild_id: String,
    product_id: u64,
    product_name: String,
    paid_until: i64,
    status: String,
}

fn entitlement_status(e: &Entitlement, now: i64) -> &'static str {
    if e.revoked {
        "revoked"
    } else if e.canceled {
        "canceled"
    } else if e.paid_until <= now {
        "expired"
    } else {
        "active"
    }
}

pub(crate) async fn overview(
    State(state): State<WebState>,
    Admin(_): Admin,
) -> Result<Json<AdminOverview>, ApiError> {
    let store = state.store.lock().map_err(|_| ApiError::internal())?;

    let users = store.all_users().map_err(|_| ApiError::internal())?;
    let bots = store.all_products().map_err(|_| ApiError::internal())?;
    let entitlements = store.all_entitlements().map_err(|_| ApiError::internal())?;
    let deployments = store
        .recent_deployments_all(20)
        .map_err(|_| ApiError::internal())?;
    let revenue_cents = store
        .total_revenue_all()
        .map_err(|_| ApiError::internal())?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let user_map: HashMap<u64, String> = users.iter().cloned().collect();
    let product_map: HashMap<u64, String> = bots.iter().map(|p| (p.id.0, p.name.clone())).collect();

    let users_list: Vec<AdminUser> = users
        .iter()
        .map(|(id, name)| AdminUser {
            id: id.to_string(),
            name: name.clone(),
        })
        .collect();

    let bots_list: Vec<AdminBot> = bots
        .iter()
        .map(|p| AdminBot {
            id: p.id.0,
            owner_id: p.owner.to_string(),
            owner_name: user_map.get(&p.owner).cloned().unwrap_or_default(),
            name: p.name.clone(),
        })
        .collect();

    let subscriptions: Vec<AdminSubscription> = entitlements
        .iter()
        .map(|e| AdminSubscription {
            guild_id: e.guild.0.to_string(),
            product_id: e.product.0,
            product_name: product_map.get(&e.product.0).cloned().unwrap_or_default(),
            paid_until: e.paid_until,
            status: entitlement_status(e, now).to_owned(),
        })
        .collect();

    let active_subscriptions = subscriptions
        .iter()
        .filter(|s| s.status == "active")
        .count();

    let recent_deployments: Vec<AdminDeploy> = deployments
        .iter()
        .map(|d| AdminDeploy {
            id: d.id,
            product_id: d.product.0,
            product_name: product_map.get(&d.product.0).cloned().unwrap_or_default(),
            tier: d.tier.clone(),
            status: d.status.as_str().to_owned(),
            trigger: d.trigger.clone(),
            branch: d.branch.clone(),
            created_at: d.created_at,
        })
        .collect();

    Ok(Json(AdminOverview {
        users: users_list.len(),
        bots: bots_list.len(),
        revenue_cents,
        active_subscriptions,
        bots_list,
        users_list,
        recent_deployments,
        subscriptions,
    }))
}

#[derive(Deserialize)]
pub(crate) struct SubscriptionTarget {
    guild_id: String,
    product_id: u64,
}

pub(crate) async fn revoke_subscription(
    State(state): State<WebState>,
    Admin(_): Admin,
    Json(body): Json<SubscriptionTarget>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let guild: u64 = body
        .guild_id
        .parse()
        .map_err(|_| ApiError::bad_request("invalid_guild", "Guild ID must be a number."))?;
    let store = state.store.lock().map_err(|_| ApiError::internal())?;
    let mut e = store
        .entitlement(GuildId(guild), ProductId(body.product_id))
        .map_err(|_| ApiError::internal())?
        .ok_or(ApiError::not_found())?;
    e.revoked = true;
    store
        .save_entitlement(&e)
        .map_err(|_| ApiError::internal())?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub(crate) async fn restore_subscription(
    State(state): State<WebState>,
    Admin(_): Admin,
    Json(body): Json<SubscriptionTarget>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let guild: u64 = body
        .guild_id
        .parse()
        .map_err(|_| ApiError::bad_request("invalid_guild", "Guild ID must be a number."))?;
    let store = state.store.lock().map_err(|_| ApiError::internal())?;
    let mut e = store
        .entitlement(GuildId(guild), ProductId(body.product_id))
        .map_err(|_| ApiError::internal())?
        .ok_or(ApiError::not_found())?;
    e.revoked = false;
    e.canceled = false;
    store
        .save_entitlement(&e)
        .map_err(|_| ApiError::internal())?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub(crate) async fn stop_bot(
    State(state): State<WebState>,
    Admin(_): Admin,
    Path((id, tier_raw)): Path<(u64, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Some(tier) = parse_tier(&tier_raw) else {
        return Err(ApiError::bad_request(
            "unknown_tier",
            "Tier must be 'free' or 'premium'.",
        ));
    };
    let name = container_name(id, tier);
    state
        .containers
        .stop(&name)
        .map_err(|_| ApiError::internal())?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub(crate) async fn delete_bot(
    State(state): State<WebState>,
    Admin(_): Admin,
    Path(id): Path<u64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let store = state.store.lock().map_err(|_| ApiError::internal())?;
    store
        .delete_product(ProductId(id))
        .map_err(|_| ApiError::internal())?;
    Ok(Json(serde_json::json!({ "ok": true })))
}
