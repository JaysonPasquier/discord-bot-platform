use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use platform_core::ProductId;
use platform_runner::{container_name, tier_name, Tier};
use platform_store::{credential_context, BotCredential, StoreError};
use serde::Deserialize;

use crate::bot_app::BotApp;
use crate::discord::DiscordError;
use crate::engine::parse_tier;
use crate::products::owned_product;
use crate::render::{message, message_back, require, require_csrf, server_error};
use crate::state::WebState;

const MAX_TOKEN_LEN: usize = 200;

// Pas de `Debug` : le jeton ne doit jamais finir dans un journal.
#[derive(Deserialize)]
pub(crate) struct BotForm {
    tier: String,
    token: String,
    csrf: String,
}

// Un jeton de bot est un texte ASCII visible, sans espace : tout le reste est une erreur de collage.
pub(crate) fn clean_token(raw: &str) -> Option<String> {
    let token = raw.trim();
    let valid = !token.is_empty()
        && token.len() <= MAX_TOKEN_LEN
        && token.bytes().all(|b| b.is_ascii_graphic());
    valid.then(|| token.to_string())
}

pub(crate) fn stored_credential(
    state: &WebState,
    product: u64,
    tier: Tier,
) -> Result<Option<BotCredential>, StoreError> {
    let store = state.store.lock().map_err(|_| StoreError::Vault)?;
    store.credential(ProductId(product), tier_name(tier))
}

// Le jeton n'est déchiffré qu'ici, pour un déploiement, et jamais renvoyé dans une page.
pub(crate) fn open_token(
    state: &WebState,
    product: u64,
    tier: Tier,
    credential: &BotCredential,
) -> Option<String> {
    let context = credential_context(ProductId(product), tier_name(tier));
    state.vault.open(&credential.token_sealed, &context).ok()
}

fn verify(
    state: &WebState,
    token: String,
) -> impl std::future::Future<Output = Option<Result<BotApp, DiscordError>>> {
    let bots = state.bots.clone();
    let task = tokio::task::spawn_blocking(move || bots.verify(&token));
    async move { task.await.ok() }
}

pub(crate) async fn save(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
    Form(form): Form<BotForm>,
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
    let back = format!("/products/{id}");
    let Some(token) = clean_token(&form.token) else {
        return message_back(
            StatusCode::BAD_REQUEST,
            &state,
            Some(&auth),
            "Invalid token",
            "Paste the bot token exactly as the Discord portal shows it (Bot tab, Reset Token).",
            &back,
        );
    };
    // Chaque essai interroge Discord : même délai minimal que pour un déploiement.
    let key = format!("verify-{}", container_name(id, tier));
    let wait =
        state
            .limiter
            .check_and_record(&key, (state.clock)(), state.config.deploy_cooldown_secs);
    if let Err(seconds) = wait {
        return message_back(
            StatusCode::TOO_MANY_REQUESTS,
            &state,
            Some(&auth),
            "Slow down",
            &format!("Wait {seconds} more second(s) before checking a token again."),
            &back,
        );
    }
    let app = match verify(&state, token.clone()).await {
        Some(Ok(app)) => app,
        Some(Err(DiscordError::Rejected)) => {
            return message_back(
                StatusCode::BAD_REQUEST,
                &state,
                Some(&auth),
                "Discord refused this token",
                "It may be wrong, or already reset. Copy a fresh one from the Bot tab of your application.",
                &back,
            )
        }
        _ => {
            return message_back(
                StatusCode::BAD_GATEWAY,
                &state,
                Some(&auth),
                "Discord did not answer",
                "The token could not be checked right now. Try again in a minute.",
                &back,
            )
        }
    };
    let context = credential_context(ProductId(id), tier_name(tier));
    let Ok(sealed) = state.vault.seal(&token, &context) else {
        return server_error(&state, Some(&auth));
    };
    let credential = BotCredential {
        client_id: app.id,
        bot_name: app.name,
        token_sealed: sealed,
    };
    let saved = state
        .store
        .lock()
        .map(|s| s.save_credential(ProductId(id), tier_name(tier), &credential));
    match saved {
        Ok(Ok(())) => message_back(
            StatusCode::OK,
            &state,
            Some(&auth),
            "Bot connected",
            "Discord accepted the token. The token is stored encrypted and will not be shown again. Deploy this version to start it; a version that is already running keeps its old token until you deploy again.",
            &back,
        ),
        Ok(Err(StoreError::Duplicate)) => message_back(
            StatusCode::CONFLICT,
            &state,
            Some(&auth),
            "Application already used",
            "This Discord application is already registered on another bot or version. Each version needs its own Discord application.",
            &back,
        ),
        _ => server_error(&state, Some(&auth)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_trimmed_visible_ascii_without_spaces() {
        assert_eq!(clean_token("  abc.DEF-_1  ").as_deref(), Some("abc.DEF-_1"));
        for bad in ["", "   ", "a b", "a\nb", "tok\u{e9}n", "a\u{0}b"] {
            assert_eq!(clean_token(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_length_limit_is_exact() {
        assert!(clean_token(&"a".repeat(MAX_TOKEN_LEN)).is_some());
        assert!(clean_token(&"a".repeat(MAX_TOKEN_LEN + 1)).is_none());
    }
}
