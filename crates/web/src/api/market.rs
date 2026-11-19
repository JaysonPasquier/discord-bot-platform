// Le marché et la fiche d'un bot : public. Un bot n'y figure que lorsque son créateur a connecté au
// moins une version ; aucun chiffre n'est inventé (les serveurs installés ne sont pas encore mesurés).
use axum::extract::{Path, State};
use axum::Json;
use platform_core::ProductId;
use platform_store::Product;
use serde::Serialize;

use super::auth::Visitor;
use super::error::ApiError;
use super::listing::{view, ListingView};
use crate::discord::invite_url;
use crate::icons::bot_slot;
use crate::state::WebState;

#[derive(Serialize)]
pub(crate) struct Listed {
    id: u64,
    name: String,
    slot: &'static str,
    owner: String,
    has_free: bool,
    has_premium: bool,
    description: String,
    category: String,
    has_avatar: bool,
    version: i64,
}

struct Loaded {
    product: Product,
    owner: String,
    free: Option<String>,
    premium: Option<String>,
}

fn load(state: &WebState, product: Product) -> Result<Loaded, ApiError> {
    let store = state.store.lock().map_err(|_| ApiError::internal())?;
    let client = |tier: &str| {
        store
            .credential(product.id, tier)
            .map(|c| c.map(|c| c.client_id))
            .map_err(|_| ApiError::internal())
    };
    let (free, premium) = (client("free")?, client("premium")?);
    let owner = store
        .username(product.owner)
        .ok()
        .flatten()
        .unwrap_or_else(|| "a creator".to_string());
    Ok(Loaded {
        product,
        owner,
        free,
        premium,
    })
}

#[derive(Serialize)]
pub(crate) struct Market {
    bots: Vec<Listed>,
}

pub(crate) async fn market(
    State(state): State<WebState>,
    _visitor: Visitor,
) -> Result<Json<Market>, ApiError> {
    let products = state
        .store
        .lock()
        .map_err(|_| ApiError::internal())?
        .all_products()
        .map_err(|_| ApiError::internal())?;
    let mut bots = Vec::new();
    for product in products {
        let l = load(&state, product)?;
        if l.free.is_some() || l.premium.is_some() {
            let page = view(&state, l.product.id.0)?;
            bots.push(Listed {
                description: page.description,
                category: page.category,
                has_avatar: page.has_avatar,
                version: page.version,
                id: l.product.id.0,
                slot: bot_slot(l.product.id.0),
                name: l.product.name,
                owner: l.owner,
                has_free: l.free.is_some(),
                has_premium: l.premium.is_some(),
            });
        }
    }
    Ok(Json(Market { bots }))
}

#[derive(Serialize)]
pub(crate) struct Listing {
    now: i64,
    id: u64,
    name: String,
    slot: &'static str,
    owner: String,
    free_invite: Option<String>,
    premium_invite: Option<String>,
    premium_servers: usize,
    // Le dernier nombre de serveurs que Discord a donné (créateur à jour), ou rien.
    servers: Option<u64>,
    last_update: Option<i64>,
    subscription_days: u32,
    // Les serveurs que le visiteur gère : de quoi choisir où activer le premium.
    signed_in: bool,
    guilds: Vec<GuildDto>,
    #[serde(flatten)]
    page: ListingView,
}

#[derive(Serialize)]
pub(crate) struct GuildDto {
    id: String,
    name: String,
}

pub(crate) async fn listing(
    State(state): State<WebState>,
    Visitor(auth): Visitor,
    Path(id): Path<u64>,
) -> Result<Json<Listing>, ApiError> {
    let found = state
        .store
        .lock()
        .map_err(|_| ApiError::internal())?
        .product(ProductId(id))
        .map_err(|_| ApiError::internal())?
        .ok_or_else(ApiError::not_found)?;
    let l = load(&state, found)?;
    let mine = auth.as_ref().is_some_and(|a| a.user_id == l.product.owner);
    if l.free.is_none() && l.premium.is_none() && !mine {
        return Err(ApiError::not_found());
    }
    let now = (state.clock)();
    let (premium_servers, last_update, servers) = {
        let store = state.store.lock().map_err(|_| ApiError::internal())?;
        let servers = store
            .entitlements_of(l.product.id)
            .unwrap_or_default()
            .iter()
            .filter(|e| {
                crate::billing::counts_as_active_subscriber(e, now, state.config.grace_secs)
            })
            .count();
        let last = store
            .deployments_of(l.product.id, 1)
            .ok()
            .and_then(|d| d.first().map(|d| d.created_at));
        let known: Vec<u64> = store
            .guild_counts(l.product.id)
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, known)| *known)
            .map(|(c, _)| c.guilds)
            .collect();
        (
            servers,
            last,
            (!known.is_empty()).then(|| known.iter().sum()),
        )
    };
    let page = view(&state, id)?;
    Ok(Json(Listing {
        now,
        page,
        id,
        slot: bot_slot(id),
        name: l.product.name,
        owner: l.owner,
        free_invite: l.free.as_deref().map(invite_url),
        premium_invite: l.premium.as_deref().map(invite_url),
        premium_servers,
        servers,
        last_update,
        subscription_days: state.config.subscription_days,
        signed_in: auth.is_some(),
        guilds: auth
            .map(|a| {
                a.guilds
                    .iter()
                    .map(|g| GuildDto {
                        id: g.id.clone(),
                        name: g.name.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }))
}
