// Routes API protégées par Bearer token (upl_...). Les bots et scripts externes les utilisent
// sans cookie ni navigateur. Elles ne retournent que les données de l'utilisateur propriétaire.
use axum::extract::State;
use axum::Json;
use serde::Serialize;

use super::auth::TokenUser;
use super::error::ApiError;
use crate::state::WebState;

#[derive(Serialize)]
pub(crate) struct MeResponse {
    pub user_id: u64,
    pub username: String,
    pub email: Option<String>,
    pub email_verified: bool,
}

pub(crate) async fn me(
    TokenUser(user_id): TokenUser,
    State(state): State<WebState>,
) -> Result<Json<MeResponse>, ApiError> {
    let store = state.store.lock().map_err(|_| ApiError::internal())?;
    let profile = store
        .user_profile(user_id)
        .map_err(|_| ApiError::internal())?
        .ok_or_else(ApiError::not_found)?;
    let email_status = store
        .email_status(user_id)
        .map_err(|_| ApiError::internal())?;
    Ok(Json(MeResponse {
        user_id,
        username: profile.username,
        email: email_status
            .as_ref()
            .filter(|s| !s.email.is_empty())
            .map(|s| s.email.clone()),
        email_verified: email_status.map(|s| s.verified).unwrap_or(false),
    }))
}

#[derive(Serialize)]
pub(crate) struct BotSummary {
    pub id: u64,
    pub name: String,
}

pub(crate) async fn bots(
    TokenUser(user_id): TokenUser,
    State(state): State<WebState>,
) -> Result<Json<Vec<BotSummary>>, ApiError> {
    let store = state.store.lock().map_err(|_| ApiError::internal())?;
    let products = store
        .products_of(user_id)
        .map_err(|_| ApiError::internal())?;
    Ok(Json(
        products
            .into_iter()
            .map(|p| BotSummary {
                id: p.id.0,
                name: p.name,
            })
            .collect(),
    ))
}
