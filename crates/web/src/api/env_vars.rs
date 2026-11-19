// Variables d'environnement supplémentaires injectées dans les containers d'un bot.
use axum::extract::{Path, State};
use axum::Json;
use platform_store::{env_key_ok, MAX_ENV_VARS, MAX_VALUE_LEN};
use serde::{Deserialize, Serialize};

use super::auth::User;
use super::error::ApiError;
use crate::engine::parse_tier;
use crate::state::WebState;

#[derive(Serialize)]
pub(crate) struct EnvVarDto {
    key: String,
    value: String,
}

#[derive(Serialize)]
pub(crate) struct EnvVarList {
    vars: Vec<EnvVarDto>,
    max: usize,
}

#[derive(Deserialize)]
pub(crate) struct SetBody {
    key: String,
    value: String,
}

#[derive(Deserialize)]
pub(crate) struct DeleteBody {
    key: String,
}

pub(crate) async fn list(
    State(state): State<WebState>,
    Path((id, tier)): Path<(u64, String)>,
    User(auth): User,
) -> Result<Json<EnvVarList>, ApiError> {
    let Some(_) = parse_tier(&tier) else {
        return Err(ApiError::not_found());
    };
    let store = state.store.lock().map_err(|_| ApiError::internal())?;
    let product = store
        .product(platform_core::ProductId(id))
        .map_err(|_| ApiError::internal())?
        .ok_or_else(ApiError::not_found)?;
    if product.owner != auth.user_id {
        return Err(ApiError::not_found());
    }
    let vars = store
        .env_vars(id, &tier)
        .map_err(|_| ApiError::internal())?
        .into_iter()
        .map(|e| EnvVarDto {
            key: e.key,
            value: e.value,
        })
        .collect();
    Ok(Json(EnvVarList {
        vars,
        max: MAX_ENV_VARS,
    }))
}

pub(crate) async fn set(
    State(state): State<WebState>,
    Path((id, tier)): Path<(u64, String)>,
    User(auth): User,
    Json(body): Json<SetBody>,
) -> Result<Json<EnvVarList>, ApiError> {
    let Some(_) = parse_tier(&tier) else {
        return Err(ApiError::not_found());
    };
    if !env_key_ok(&body.key) {
        return Err(ApiError::bad_request(
            "invalid_key",
            "Keys must start with a letter or underscore and contain only letters, digits and underscores.",
        ));
    }
    if body.value.len() > MAX_VALUE_LEN {
        return Err(ApiError::bad_request(
            "value_too_long",
            format!("Value must be at most {MAX_VALUE_LEN} characters."),
        ));
    }
    let store = state.store.lock().map_err(|_| ApiError::internal())?;
    let product = store
        .product(platform_core::ProductId(id))
        .map_err(|_| ApiError::internal())?
        .ok_or_else(ApiError::not_found)?;
    if product.owner != auth.user_id {
        return Err(ApiError::not_found());
    }
    let count = store
        .count_env_vars(id, &tier)
        .map_err(|_| ApiError::internal())?;
    let already_exists = store
        .env_vars(id, &tier)
        .map_err(|_| ApiError::internal())?
        .iter()
        .any(|e| e.key == body.key);
    if !already_exists && count >= MAX_ENV_VARS {
        return Err(ApiError::conflict(
            "too_many_vars",
            format!("At most {MAX_ENV_VARS} environment variables per version."),
        ));
    }
    store
        .set_env_var(id, &tier, &body.key, &body.value)
        .map_err(|_| ApiError::internal())?;
    let vars = store
        .env_vars(id, &tier)
        .map_err(|_| ApiError::internal())?
        .into_iter()
        .map(|e| EnvVarDto {
            key: e.key,
            value: e.value,
        })
        .collect();
    Ok(Json(EnvVarList {
        vars,
        max: MAX_ENV_VARS,
    }))
}

pub(crate) async fn delete(
    State(state): State<WebState>,
    Path((id, tier)): Path<(u64, String)>,
    User(auth): User,
    Json(body): Json<DeleteBody>,
) -> Result<Json<EnvVarList>, ApiError> {
    let Some(_) = parse_tier(&tier) else {
        return Err(ApiError::not_found());
    };
    let store = state.store.lock().map_err(|_| ApiError::internal())?;
    let product = store
        .product(platform_core::ProductId(id))
        .map_err(|_| ApiError::internal())?
        .ok_or_else(ApiError::not_found)?;
    if product.owner != auth.user_id {
        return Err(ApiError::not_found());
    }
    store
        .delete_env_var(id, &tier, &body.key)
        .map_err(|_| ApiError::internal())?;
    let vars = store
        .env_vars(id, &tier)
        .map_err(|_| ApiError::internal())?
        .into_iter()
        .map(|e| EnvVarDto {
            key: e.key,
            value: e.value,
        })
        .collect();
    Ok(Json(EnvVarList {
        vars,
        max: MAX_ENV_VARS,
    }))
}
