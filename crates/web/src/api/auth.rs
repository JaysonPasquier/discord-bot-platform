// Qui appelle l'API : la session par cookie (HttpOnly) et, pour toute écriture, le jeton CSRF en
// en-tête. Un en-tête personnalisé ne peut pas être envoyé par un autre site sans autorisation
// préalable du navigateur : c'est ce qui rend une écriture forgée impossible.
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::Method;

use super::error::ApiError;
use crate::session::{authenticate, tokens_match, Auth};
use crate::state::WebState;

pub(crate) const CSRF_HEADER: &str = "x-csrf-token";

// Un utilisateur connecté (lecture ou écriture).
pub(crate) struct User(pub(crate) Auth);

impl FromRequestParts<WebState> for User {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &WebState,
    ) -> Result<Self, Self::Rejection> {
        let auth = authenticate(state, &parts.headers).ok_or_else(ApiError::unauthorized)?;
        let reading = matches!(parts.method, Method::GET | Method::HEAD);
        if !reading {
            let sent = parts
                .headers
                .get(CSRF_HEADER)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            if !tokens_match(&auth.csrf, sent) {
                return Err(ApiError::forbidden(
                    "csrf",
                    "Invalid form token. Reload the page and try again.",
                ));
            }
        }
        Ok(Self(auth))
    }
}

// L'administrateur de la plateforme : PLATFORM_ADMIN_USER_ID doit correspondre.
#[allow(dead_code)]
pub(crate) struct Admin(pub(crate) Auth);

impl FromRequestParts<WebState> for Admin {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &WebState,
    ) -> Result<Self, Self::Rejection> {
        let auth = authenticate(state, &parts.headers).ok_or_else(ApiError::unauthorized)?;
        let admin_id = state
            .config
            .admin_user_id
            .as_deref()
            .ok_or_else(ApiError::not_found)?;
        if auth.user_id.to_string() != admin_id {
            return Err(ApiError::forbidden("not_admin", "Admin access required."));
        }
        let reading = matches!(parts.method, Method::GET | Method::HEAD);
        if !reading {
            let sent = parts
                .headers
                .get(CSRF_HEADER)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            if !tokens_match(&auth.csrf, sent) {
                return Err(ApiError::forbidden(
                    "csrf",
                    "Invalid form token. Reload the page and try again.",
                ));
            }
        }
        Ok(Self(auth))
    }
}

// Un appel via Bearer token (upl_...) : pour les routes API sans cookie.
pub(crate) struct TokenUser(pub(crate) u64);

impl FromRequestParts<WebState> for TokenUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &WebState,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(ApiError::unauthorized)?;
        let token = header
            .strip_prefix("Bearer ")
            .ok_or_else(ApiError::unauthorized)?;
        let user_id = state
            .store
            .lock()
            .map_err(|_| ApiError::internal())?
            .validate_api_token(token)
            .map_err(|_| ApiError::internal())?
            .ok_or_else(ApiError::unauthorized)?;
        Ok(Self(user_id))
    }
}

// Un visiteur, connecté ou non : pour les lectures publiques.
pub(crate) struct Visitor(pub(crate) Option<Auth>);

impl FromRequestParts<WebState> for Visitor {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &WebState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(authenticate(state, &parts.headers)))
    }
}
