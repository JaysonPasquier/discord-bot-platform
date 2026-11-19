// Stripe prévient des paiements et des litiges. Sans secret configuré la route n'existe pas ;
// sans signature valide elle refuse. Toujours 200 une fois l'événement accepté (Stripe rejoue
// sinon), même si l'application interne échoue : l'erreur reste dans le journal du processus.
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use platform_core::{GuildId, PaymentEvent, ProductId};
use platform_store::SqliteStore;
use platform_stripe::{parse_event, verify_signature, StripeEvent};

use crate::billing::load_entitlement;
use crate::state::WebState;

pub(crate) async fn webhook(
    State(state): State<WebState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(secret) = state.config.stripe_webhook_secret.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let signature = headers
        .get("stripe-signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let now = (state.clock)();
    if !verify_signature(secret.expose(), &body, signature, now) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(event) = parse_event(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    // Le lock est relâché avant un éventuel instantané d'abonnés (`stats::snapshot` le reprend :
    // `Mutex` n'est pas réentrant, un verrou encore tenu ici ferait un interblocage).
    let touched = {
        let Ok(store) = state.store.lock() else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        // Un webhook rejoué (retry de Stripe ou livraison interceptée et renvoyée) ne doit jamais
        // s'appliquer deux fois.
        match store.mark_stripe_event_seen(&event.id, now) {
            Ok(false) => return StatusCode::OK.into_response(),
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            Ok(true) => {}
        }
        match event.kind.as_str() {
            // Un moyen de paiement asynchrone (virement...) termine d'abord la session sans être
            // payé (`checkout.session.completed` avec `payment_status: "unpaid"`, ignoré plus
            // bas) ; l'accès n'est accordé qu'ici, quand Stripe confirme le paiement reçu.
            "checkout.session.completed" | "checkout.session.async_payment_succeeded" => {
                apply_checkout_completed(&store, &event, now)
            }
            "charge.dispute.created" => apply_dispute(&store, &event, PaymentEvent::Disputed),
            "charge.dispute.closed"
                if event.data.object.get("status").and_then(|v| v.as_str()) == Some("won") =>
            {
                apply_dispute(&store, &event, PaymentEvent::DisputeWon)
            }
            _ => None,
        }
    };
    if let Some(product) = touched {
        crate::stats::snapshot(&state, product);
    }
    StatusCode::OK.into_response()
}

fn meta<'a>(object: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    object.get("metadata")?.get(key)?.as_str()
}

// Appelé pour `checkout.session.completed` ET `checkout.session.async_payment_succeeded`
// seul un `payment_status: "paid"` accorde l'accès. Un moyen de
// paiement asynchrone (virement...) complète d'abord la session sans être payé ; le deuxième
// événement, avec la même session et la même métadonnée, confirme le paiement plus tard.
fn apply_checkout_completed(
    store: &SqliteStore,
    event: &StripeEvent,
    now: i64,
) -> Option<ProductId> {
    let object = &event.data.object;
    if object.get("payment_status").and_then(|v| v.as_str()) != Some("paid") {
        return None;
    }
    let guild = meta(object, "guild_id").and_then(|v| v.parse::<u64>().ok())?;
    let product = meta(object, "product_id").and_then(|v| v.parse::<u64>().ok())?;
    let days = meta(object, "days").and_then(|v| v.parse::<u32>().ok())?;
    let user_id = meta(object, "user_id").and_then(|v| v.parse::<u64>().ok())?;
    let plan_name = meta(object, "plan_name").unwrap_or("Plan");
    let price_cents = u32::try_from(
        object
            .get("amount_total")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
    )
    .unwrap_or(0);
    let (guild, product) = (GuildId(guild), ProductId(product));
    let current = load_entitlement(store, guild, product).ok()?;
    let updated = current.apply(current.payment_for_days(now, days));
    store.save_entitlement(&updated).ok()?;
    let _ = store.record_subscription(guild, product, user_id);
    let _ = store.record_payment(product, guild.0, plan_name, price_cents, days, now);
    if let Some(intent) = object.get("payment_intent").and_then(|v| v.as_str()) {
        let _ = store.remember_stripe_intent(intent, guild, product, now);
    }
    Some(product)
}

// Un litige ne porte que l'identifiant du PaymentIntent : la correspondance avec guild/produit a
// été retenue à la création de la session (table `stripe_intents`).
fn apply_dispute(
    store: &SqliteStore,
    event: &StripeEvent,
    kind: PaymentEvent,
) -> Option<ProductId> {
    let intent = event
        .data
        .object
        .get("payment_intent")
        .and_then(|v| v.as_str())?;
    let (guild, product) = store.stripe_intent(intent).ok().flatten()?;
    let current = load_entitlement(store, guild, product).ok()?;
    store.save_entitlement(&current.apply(kind)).ok()?;
    Some(product)
}
