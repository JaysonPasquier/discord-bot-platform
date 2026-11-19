// Les chiffres d'un bot, tels que la plateforme les a vraiment : rien n'est inventé. Un chiffre que
// la plateforme ne peut pas connaître est `null`, et l'interface écrit « Not tracked yet ».
use axum::extract::{Path, State};
use axum::Json;
use platform_core::ProductId;
use serde::Serialize;

use super::auth::User;
use super::bots::owned;
use super::error::ApiError;
use crate::state::WebState;
use crate::stats::{
    active_subscribers, carry_forward, per_day, refresh_guild_counts, snapshot, DAY,
};

const DEPLOY_DAYS: i64 = 7;
const HISTORY_DAYS: i64 = 30;
const MONTH: i64 = 30 * DAY;

#[derive(Serialize)]
pub(crate) struct Servers {
    // Les serveurs où chaque version a été ajoutée (nombre approximatif de Discord, À VÉRIFIER).
    free: Option<u64>,
    premium: Option<u64>,
    // Un serveur qui a ajouté les deux versions compte deux fois.
    total: Option<u64>,
    checked_at: Option<i64>,
}

#[derive(Serialize)]
pub(crate) struct Revenue {
    simulated: bool,
    last_30_days_cents: u64,
    previous_30_days_cents: u64,
}

#[derive(Serialize)]
pub(crate) struct Point {
    day_start: i64,
    count: Option<u64>,
}

#[derive(Serialize)]
pub(crate) struct Stats {
    now: i64,
    servers: Servers,
    revenue: Revenue,
    deployments_per_day: Vec<Point>,
    subscribers_now: u64,
    subscribers_history: Vec<Point>,
}

pub(crate) async fn read(
    State(state): State<WebState>,
    User(auth): User,
    Path(id): Path<u64>,
) -> Result<Json<Stats>, ApiError> {
    owned(&state, &auth, id)?;
    refresh_guild_counts(&state, id).await;
    let product = ProductId(id);
    snapshot(&state, product);
    let now = (state.clock)();
    let store = state.store.lock().map_err(|_| ApiError::internal())?;
    let counts = store
        .guild_counts(product)
        .map_err(|_| ApiError::internal())?;
    let count_of = |tier: &str| {
        counts
            .iter()
            .find(|(c, known)| c.tier == tier && *known)
            .map(|(c, _)| c.guilds)
    };
    let (free, premium) = (count_of("free"), count_of("premium"));
    let servers = Servers {
        free,
        premium,
        total: match (free, premium) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
        },
        checked_at: counts.iter().map(|(c, _)| c.checked_at).max(),
    };
    let revenue = Revenue {
        simulated: true,
        last_30_days_cents: store
            .revenue_between(product, now - MONTH, now + 1)
            .map_err(|_| ApiError::internal())?,
        previous_30_days_cents: store
            .revenue_between(product, now - 2 * MONTH, now - MONTH)
            .map_err(|_| ApiError::internal())?,
    };
    let deployed: Vec<i64> = store
        .deployments_of(product, super::bots::MAX_DEPLOYMENTS)
        .map_err(|_| ApiError::internal())?
        .iter()
        .map(|d| d.created_at)
        .collect();
    let snapshots = store
        .snapshots_since(product, 0)
        .map_err(|_| ApiError::internal())?;
    drop(store);
    let point = |(day, count): (i64, Option<u64>)| Point {
        day_start: day * DAY,
        count,
    };
    Ok(Json(Stats {
        now,
        servers,
        revenue,
        deployments_per_day: per_day(&deployed, now, DEPLOY_DAYS)
            .into_iter()
            .map(|(d, n)| point((d, Some(u64::from(n)))))
            .collect(),
        subscribers_now: active_subscribers(&state, product).unwrap_or(0),
        subscribers_history: carry_forward(&snapshots, now, HISTORY_DAYS)
            .into_iter()
            .map(point)
            .collect(),
    }))
}
