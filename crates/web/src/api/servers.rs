// Les abonnements premium de l'utilisateur : paiement réel via Stripe en mode test.
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use platform_core::{GuildId, PaymentEvent, PremiumStatus, ProductId};
use platform_stripe::{CheckoutParams, StripeError};
use serde::{Deserialize, Serialize};

use super::auth::User;
use super::error::ApiError;
use crate::billing::{load_entitlement, status_of};
use crate::icons::bot_slot;
use crate::state::WebState;

#[derive(Serialize)]
pub(crate) struct SubscriptionDto {
    guild_id: String,
    server: String,
    product_id: u64,
    bot: String,
    slot: &'static str,
    nick: Option<String>,
    // active | grace | expired
    status: &'static str,
    canceled: bool,
    // None : jamais payé.
    paid_until: Option<i64>,
    plan_days: u32,
}

#[derive(Serialize)]
pub(crate) struct Subscriptions {
    now: i64,
    subscriptions: Vec<SubscriptionDto>,
}

pub(crate) async fn list(
    State(state): State<WebState>,
    User(auth): User,
) -> Result<Json<Subscriptions>, ApiError> {
    let store = state.store.lock().map_err(|_| ApiError::internal())?;
    let subs = store
        .subscriptions_of(auth.user_id)
        .map_err(|_| ApiError::internal())?;
    let mut out = Vec::new();
    for sub in subs {
        let product = store
            .product(sub.product)
            .map_err(|_| ApiError::internal())?
            .ok_or_else(ApiError::internal)?;
        let entitlement =
            load_entitlement(&store, sub.guild, sub.product).map_err(|_| ApiError::internal())?;
        let id = sub.guild.0.to_string();
        out.push(SubscriptionDto {
            server: auth
                .guilds
                .iter()
                .find(|g| g.id == id)
                .map_or_else(|| format!("server {id}"), |g| g.name.clone()),
            guild_id: id,
            product_id: sub.product.0,
            slot: bot_slot(sub.product.0),
            bot: product.name,
            nick: store
                .guild_profile(sub.guild, sub.product)
                .ok()
                .flatten()
                .and_then(|p| p.nick),
            status: match status_of(&state, &entitlement) {
                PremiumStatus::Active => "active",
                PremiumStatus::Grace => "grace",
                PremiumStatus::Expired => "expired",
            },
            canceled: entitlement.canceled,
            paid_until: (entitlement.paid_until != i64::MIN).then_some(entitlement.paid_until),
            plan_days: state.config.subscription_days,
        });
    }
    Ok(Json(Subscriptions {
        now: (state.clock)(),
        subscriptions: out,
    }))
}

#[derive(Deserialize)]
pub(crate) struct SubscriptionForm {
    product: u64,
    guild: String,
    // La formule choisie ; sans elle, la durée par défaut de la plateforme.
    #[serde(default)]
    plan: Option<u64>,
}

fn parse_guild(raw: &str) -> Result<u64, ApiError> {
    crate::discord::decimal_id(raw).ok_or_else(|| {
        ApiError::bad_request("invalid_server", "The server identifier is not valid.")
    })
}

#[derive(Serialize)]
pub(crate) struct CheckoutResponse {
    checkout_url: String,
}

fn stripe_error(error: StripeError) -> ApiError {
    match error {
        StripeError::NotConfigured => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "stripe_not_configured",
            "Payments are not configured on this server yet.",
        ),
        StripeError::Unauthorized | StripeError::InvalidRequest(_) => ApiError::new(
            StatusCode::BAD_GATEWAY,
            "stripe_rejected",
            "Stripe rejected this request.",
        ),
        StripeError::Parse | StripeError::Http(_) => ApiError::new(
            StatusCode::BAD_GATEWAY,
            "stripe_down",
            "Stripe did not answer. Try again in a minute.",
        ),
    }
}

// Crée une session Stripe Checkout (mode test) : aucun accès n'est accordé ici, seulement au
// webhook `checkout.session.completed` une fois le paiement confirmé.
pub(crate) async fn subscribe(
    State(state): State<WebState>,
    User(auth): User,
    Json(form): Json<SubscriptionForm>,
) -> Result<Json<CheckoutResponse>, ApiError> {
    let guild = parse_guild(&form.guild)?;
    if !auth.guilds.iter().any(|g| g.id == form.guild) {
        return Err(ApiError::forbidden(
            "not_your_server",
            "You can only subscribe for a server you manage.",
        ));
    }
    let product = ProductId(form.product);
    let (bot_name, plans) = {
        let store = state.store.lock().map_err(|_| ApiError::internal())?;
        let bot = store
            .product(product)
            .map_err(|_| ApiError::internal())?
            .ok_or_else(ApiError::not_found)?;
        let plans = store.plans_of(product).map_err(|_| ApiError::internal())?;
        (bot.name, plans)
    };
    let chosen = match (form.plan, plans.first()) {
        (Some(wanted), _) => plans
            .iter()
            .find(|p| p.id == wanted)
            .ok_or_else(|| ApiError::bad_request("invalid_plan", "This plan does not exist for this bot."))?,
        (None, Some(first)) => first,
        (None, None) => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "no_plan",
                "This bot has no priced plan yet. The creator must add one before anyone can subscribe.",
            ))
        }
    };
    if chosen.price_cents == 0 {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "free_plan",
            "This plan has no price: nothing to charge. Ask the creator to set a price above zero.",
        ));
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
    let session = state
        .stripe
        .create_checkout_session(&CheckoutParams {
            success_url: &success_url,
            cancel_url: &cancel_url,
            currency: &state.config.stripe_currency,
            product_name: &format!("{bot_name}: {}", chosen.name),
            unit_amount_cents: chosen.price_cents,
            metadata: &metadata,
        })
        .map_err(stripe_error)?;
    Ok(Json(CheckoutResponse {
        checkout_url: session.url,
    }))
}

pub(crate) async fn cancel(
    State(state): State<WebState>,
    User(auth): User,
    Json(form): Json<SubscriptionForm>,
) -> Result<StatusCode, ApiError> {
    let (guild, product) = (GuildId(parse_guild(&form.guild)?), ProductId(form.product));
    let now = (state.clock)();
    let store = state.store.lock().map_err(|_| ApiError::internal())?;
    let owns = store
        .subscriptions_of(auth.user_id)
        .map_err(|_| ApiError::internal())?
        .iter()
        .any(|s| s.guild == guild && s.product == product);
    if !owns {
        return Err(ApiError::forbidden(
            "not_yours",
            "This subscription is not yours.",
        ));
    }
    load_entitlement(&store, guild, product)
        .and_then(|current| {
            store.save_entitlement(&current.apply(PaymentEvent::Canceled { effective: now }))
        })
        .map_err(|_| ApiError::internal())?;
    drop(store);
    crate::stats::snapshot(&state, product);
    Ok(StatusCode::NO_CONTENT)
}
