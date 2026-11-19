use axum::extract::{Form, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use platform_core::{Entitlement, GuildId, PaymentEvent, PremiumStatus, ProductId};
use platform_store::{SqliteStore, StoreError};
use platform_stripe::CheckoutParams;
use serde::Deserialize;

use crate::render::{message, not_found, redirect, require, require_csrf, server_error};
use crate::state::WebState;

// Délai minimal entre deux tentatives de checkout par le même utilisateur.
const CHECKOUT_COOLDOWN_SECS: u32 = 10;

#[derive(Deserialize)]
pub(crate) struct SubscriptionForm {
    product: u64,
    guild: String,
    csrf: String,
}

pub(crate) fn load_entitlement(
    store: &SqliteStore,
    guild: GuildId,
    product: ProductId,
) -> Result<Entitlement, StoreError> {
    Ok(store
        .entitlement(guild, product)?
        .unwrap_or_else(|| Entitlement::new(guild, product)))
}

pub(crate) fn status_of(state: &WebState, e: &Entitlement) -> PremiumStatus {
    e.status((state.clock)(), state.config.grace_secs)
}

// Compte comme abonné actif : actif, ou en grâce (renouvellement payé en retard).
// `now`/`grace_secs` restent des paramètres (pas `status_of`) : un appelant qui filtre
// beaucoup d'entitlements lit l'horloge une seule fois avant la boucle, pas à chaque entitlement.
pub(crate) fn counts_as_active_subscriber(
    e: &Entitlement,
    now: platform_core::Timestamp,
    grace_secs: u32,
) -> bool {
    matches!(
        e.status(now, grace_secs),
        PremiumStatus::Active | PremiumStatus::Grace
    )
}

// Comme `api::servers::subscribe` : ouvre une session Stripe Checkout (mode test), rien
// n'est accordé ici. Seul le webhook `checkout.session.completed` applique le paiement.
pub(crate) async fn subscribe(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<SubscriptionForm>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    if let Err(response) = require_csrf(&state, &auth, &form.csrf) {
        return response;
    }
    let Some(guild) = crate::discord::decimal_id(&form.guild) else {
        return message(
            StatusCode::BAD_REQUEST,
            &state,
            Some(&auth),
            "Invalid server",
            "The server identifier is not valid.",
        );
    };
    if !auth.guilds.iter().any(|g| g.id == form.guild) {
        return message(
            StatusCode::FORBIDDEN,
            &state,
            Some(&auth),
            "Forbidden",
            "You can only subscribe for a server you manage.",
        );
    }
    let product = ProductId(form.product);
    let (bot_name, plans) = {
        let Ok(store) = state.store.lock() else {
            return server_error(&state, Some(&auth));
        };
        let bot = match store.product(product) {
            Ok(Some(p)) => p,
            Ok(None) => return not_found(&state, &auth),
            Err(_) => return server_error(&state, Some(&auth)),
        };
        let Ok(plans) = store.plans_of(product) else {
            return server_error(&state, Some(&auth));
        };
        (bot.name, plans)
    };
    let Some(chosen) = plans.first() else {
        return message(
            StatusCode::CONFLICT,
            &state,
            Some(&auth),
            "No priced plan yet",
            "This bot has no priced plan yet. Its creator must add one before anyone can subscribe.",
        );
    };
    if chosen.price_cents == 0 {
        let now = (state.clock)();
        let days = chosen.days;
        let guild_id = GuildId(guild);
        // Ok(true) : essai accordé. Ok(false) : déjà utilisé. Err : erreur DB.
        let outcome: Result<bool, ()> = state.store.lock().map_err(|_| ()).and_then(|store| {
            let current = load_entitlement(&store, guild_id, product).map_err(|_| ())?;
            if current.paid_until != i64::MIN {
                return Ok(false);
            }
            let event = current.payment_for_days(now, days);
            store
                .save_entitlement(&current.apply(event))
                .map_err(|_| ())?;
            store
                .record_subscription(guild_id, product, auth.user_id)
                .map_err(|_| ())?;
            Ok(true)
        });
        return match outcome {
            Ok(true) => redirect("/servers"),
            Ok(false) => message(
                StatusCode::CONFLICT,
                &state,
                Some(&auth),
                "Free trial already used",
                "This server has already claimed the free trial for this bot. Subscribe to a paid plan to continue.",
            ),
            Err(()) => server_error(&state, Some(&auth)),
        };
    }
    // Rate limit : un checkout toutes les 10 s par utilisateur.
    let checkout_key = format!("checkout:{}", auth.user_id);
    let now = (state.clock)();
    let rate_ok = state.store.lock().map_err(|_| ()).and_then(|s| {
        s.check_and_record_deploy(&checkout_key, now, CHECKOUT_COOLDOWN_SECS)
            .map_err(|_| ())
    });
    match rate_ok {
        Ok(Err(_)) => {
            return message(
                StatusCode::TOO_MANY_REQUESTS,
                &state,
                Some(&auth),
                "Too many requests",
                "Please wait a few seconds before trying again.",
            )
        }
        Err(_) => return server_error(&state, Some(&auth)),
        Ok(Ok(())) => {}
    }
    let base = state
        .config
        .frontend_url
        .clone()
        .unwrap_or_else(|| state.config.public_url.clone());
    let success_url = format!("{base}/dashboard?stripe=success");
    let cancel_url = format!("{base}/dashboard?stripe=cancel");
    let guild_text = guild.to_string();
    let product_text = product.0.to_string();
    let user_text = auth.user_id.to_string();
    let days_text = chosen.days.to_string();
    let metadata = [
        ("guild_id", guild_text.as_str()),
        ("product_id", product_text.as_str()),
        ("user_id", user_text.as_str()),
        ("days", days_text.as_str()),
        ("plan_name", chosen.name.as_str()),
    ];
    match state.stripe.create_checkout_session(&CheckoutParams {
        success_url: &success_url,
        cancel_url: &cancel_url,
        currency: &state.config.stripe_currency,
        product_name: &format!("{bot_name}: {}", chosen.name),
        unit_amount_cents: chosen.price_cents,
        metadata: &metadata,
    }) {
        Ok(session) => redirect(&session.url),
        Err(platform_stripe::StripeError::NotConfigured) => message(
            StatusCode::SERVICE_UNAVAILABLE,
            &state,
            Some(&auth),
            "Payments are not configured",
            "This server has not set up Stripe yet. Ask its administrator.",
        ),
        Err(_) => message(
            StatusCode::BAD_GATEWAY,
            &state,
            Some(&auth),
            "Stripe did not answer",
            "Try again in a minute.",
        ),
    }
}

pub(crate) async fn cancel(
    State(state): State<WebState>,
    headers: HeaderMap,
    Form(form): Form<SubscriptionForm>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    if let Err(response) = require_csrf(&state, &auth, &form.csrf) {
        return response;
    }
    let Some(guild) = crate::discord::decimal_id(&form.guild) else {
        return message(
            StatusCode::BAD_REQUEST,
            &state,
            Some(&auth),
            "Invalid server",
            "The server identifier is not valid.",
        );
    };
    let (guild, product) = (GuildId(guild), ProductId(form.product));
    let now = (state.clock)();
    let Ok(store) = state.store.lock() else {
        return server_error(&state, Some(&auth));
    };
    let owns = store.subscriptions_of(auth.user_id).map(|subs| {
        subs.iter()
            .any(|s| s.guild == guild && s.product == product)
    });
    match owns {
        Ok(true) => {}
        Ok(false) => {
            return message(
                StatusCode::FORBIDDEN,
                &state,
                Some(&auth),
                "Forbidden",
                "This subscription is not yours.",
            )
        }
        Err(_) => return server_error(&state, Some(&auth)),
    }
    let result = load_entitlement(&store, guild, product).and_then(|current| {
        store.save_entitlement(&current.apply(PaymentEvent::Canceled { effective: now }))
    });
    match result {
        Ok(()) => redirect("/dashboard"),
        Err(_) => server_error(&state, Some(&auth)),
    }
}
