// Qui suis-je : lecture de la session, connexion de test, déconnexion, sessions actives.
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use platform_store::hash_token;
use serde::{Deserialize, Serialize};

use super::auth::{User, Visitor};
use super::error::ApiError;
use crate::discord::Guild;
use crate::login::dev_user;
use crate::render::with_cookies;
use crate::session::{
    clear_cookie, cookie_value, set_cookie, start_session, SESSION_COOKIE, SESSION_TTL_SECS,
};
use crate::state::WebState;

#[derive(Serialize)]
struct UserDto {
    id: String,
    name: String,
}

#[derive(Serialize)]
struct LoginDto {
    discord: bool,
    dev: bool,
}

#[derive(Serialize)]
pub(crate) struct SessionDto {
    user: Option<UserDto>,
    csrf: Option<String>,
    guilds: Vec<Guild>,
    login: LoginDto,
    is_admin: bool,
    // L'heure du serveur : le frontend s'en sert pour afficher « il y a 3 min » sans se fier à son horloge.
    now: i64,
}

pub(crate) async fn read(
    State(state): State<WebState>,
    Visitor(auth): Visitor,
) -> Json<SessionDto> {
    let login = LoginDto {
        discord: state.discord.authorize_url("probe").is_some(),
        dev: state.config.dev_login,
    };
    let now = (state.clock)();
    let admin_id = state.config.admin_user_id.as_deref();
    Json(match auth {
        Some(a) => {
            let is_admin = admin_id.is_some_and(|id| id == a.user_id.to_string());
            SessionDto {
                user: Some(UserDto {
                    id: a.user_id.to_string(),
                    name: a.name,
                }),
                csrf: Some(a.csrf),
                guilds: a.guilds,
                login,
                is_admin,
                now,
            }
        }
        None => SessionDto {
            user: None,
            csrf: None,
            guilds: Vec::new(),
            login,
            is_admin: false,
            now,
        },
    })
}

#[derive(Deserialize)]
pub(crate) struct DevLogin {
    who: String,
}

pub(crate) async fn dev_login(
    State(state): State<WebState>,
    Json(form): Json<DevLogin>,
) -> Result<Response, ApiError> {
    if !state.config.dev_login {
        return Err(ApiError::not_found());
    }
    let Some((user, guilds)) = dev_user(&form.who) else {
        return Err(ApiError::bad_request("unknown_user", "Use alice or bob."));
    };
    let cookie = start_session(&state, &user, &guilds).ok_or_else(ApiError::internal)?;
    let cookie = set_cookie(
        SESSION_COOKIE,
        &cookie,
        SESSION_TTL_SECS,
        state.config.secure_cookies(),
    );
    Ok(with_cookies(
        StatusCode::NO_CONTENT.into_response(),
        &[cookie],
    ))
}

pub(crate) async fn logout(
    State(state): State<WebState>,
    User(_): User,
    headers: axum::http::HeaderMap,
) -> Response {
    if let (Some(cookie), Ok(store)) = (cookie_value(&headers, SESSION_COOKIE), state.store.lock())
    {
        let _ = store.delete_session(&hash_token(&cookie));
    }
    with_cookies(
        StatusCode::NO_CONTENT.into_response(),
        &[clear_cookie(SESSION_COOKIE, state.config.secure_cookies())],
    )
}

#[derive(Serialize)]
pub(crate) struct ActiveSessionDto {
    id: String,
    created_at: i64,
    expires_at: i64,
    is_current: bool,
}

#[derive(Serialize)]
pub(crate) struct Sessions {
    now: i64,
    sessions: Vec<ActiveSessionDto>,
}

// La liste des sessions actives de l'utilisateur connecté : de quoi reconnaître les siennes et
// révoquer celles qu'il ne reconnaît pas.
pub(crate) async fn list_sessions(
    State(state): State<WebState>,
    User(auth): User,
    headers: axum::http::HeaderMap,
) -> Result<Json<Sessions>, ApiError> {
    let now = (state.clock)();
    let current =
        cookie_value(&headers, SESSION_COOKIE).map(|c| platform_store::to_hex(&hash_token(&c)));
    let store = state.store.lock().map_err(|_| ApiError::internal())?;
    let sessions = store
        .sessions_of(auth.user_id, now)
        .map_err(|_| ApiError::internal())?
        .into_iter()
        .map(|s| {
            let is_current = current.as_deref() == Some(s.id.as_str());
            ActiveSessionDto {
                id: s.id,
                created_at: s.created_at,
                expires_at: s.expires_at,
                is_current,
            }
        })
        .collect();
    Ok(Json(Sessions { now, sessions }))
}

pub(crate) async fn revoke_session(
    State(state): State<WebState>,
    User(auth): User,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let store = state.store.lock().map_err(|_| ApiError::internal())?;
    store
        .revoke_session(auth.user_id, &id)
        .map_err(|_| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}
