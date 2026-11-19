// Les statistiques d'un bot : tout vient de ce que la plateforme a vraiment enregistré ou demandé à Discord.
use platform_core::ProductId;
use platform_runner::{tier_name, Tier};

use crate::bots::{open_token, stored_credential};
use crate::state::WebState;

pub(crate) const DAY: i64 = 86_400;
// Discord n'est pas interrogé plus d'une fois par ce délai pour un même bot.
const GUILD_COUNT_TTL: i64 = 600;

pub(crate) fn day_of(timestamp: i64) -> i64 {
    timestamp.div_euclid(DAY)
}

// Combien d'éléments par jour sur les `days` derniers jours (aujourd'hui compris), du plus ancien au plus récent.
pub(crate) fn per_day(timestamps: &[i64], now: i64, days: i64) -> Vec<(i64, u32)> {
    let today = day_of(now);
    (today - days + 1..=today)
        .map(|day| {
            let count = timestamps.iter().filter(|t| day_of(**t) == day).count();
            (day, u32::try_from(count).unwrap_or(u32::MAX))
        })
        .collect()
}

// Le dernier instantané connu vaut pour les jours suivants sans instantané ; avant le premier : inconnu.
pub(crate) fn carry_forward(
    snapshots: &[(i64, u64)],
    now: i64,
    days: i64,
) -> Vec<(i64, Option<u64>)> {
    let today = day_of(now);
    let mut last = snapshots
        .iter()
        .rev()
        .find(|(d, _)| *d < today - days + 1)
        .map(|(_, c)| *c);
    (today - days + 1..=today)
        .map(|day| {
            if let Some((_, c)) = snapshots.iter().find(|(d, _)| *d == day) {
                last = Some(*c);
            }
            (day, last)
        })
        .collect()
}

pub(crate) fn active_subscribers(state: &WebState, product: ProductId) -> Option<u64> {
    let now = (state.clock)();
    let store = state.store.lock().ok()?;
    let count = store
        .entitlements_of(product)
        .ok()?
        .iter()
        .filter(|e| crate::billing::counts_as_active_subscriber(e, now, state.config.grace_secs))
        .count();
    u64::try_from(count).ok()
}

// Écrit le nombre d'abonnés d'aujourd'hui : appelé à chaque abonnement, annulation, affichage et heure.
pub(crate) fn snapshot(state: &WebState, product: ProductId) {
    let Some(count) = active_subscribers(state, product) else {
        return;
    };
    if let Ok(store) = state.store.lock() {
        let _ = store.record_snapshot(product, day_of((state.clock)()), count);
    }
}

pub fn snapshot_all(state: &WebState) {
    let products = state
        .store
        .lock()
        .ok()
        .and_then(|s| s.all_products().ok())
        .unwrap_or_default();
    for p in products {
        snapshot(state, p.id);
    }
}

// Redemande à Discord le nombre de serveurs d'une version, si le dernier essai est trop ancien.
pub(crate) async fn refresh_guild_counts(state: &WebState, product: u64) {
    for tier in [Tier::Free, Tier::Premium] {
        let now = (state.clock)();
        let fresh = state.store.lock().is_ok_and(|s| {
            s.guild_counts(ProductId(product))
                .unwrap_or_default()
                .iter()
                .any(|(c, _)| c.tier == tier_name(tier) && now - c.checked_at < GUILD_COUNT_TTL)
        });
        if fresh {
            continue;
        }
        let Ok(Some(credential)) = stored_credential(state, product, tier) else {
            continue;
        };
        let Some(token) = open_token(state, product, tier, &credential) else {
            continue;
        };
        let bots = state.bots.clone();
        let found = tokio::task::spawn_blocking(move || bots.guild_count(&token))
            .await
            .ok()
            .flatten();
        if let Ok(store) = state.store.lock() {
            let _ = match found {
                Some(n) => store.save_guild_count(ProductId(product), tier_name(tier), n, now),
                None => store.touch_guild_count(ProductId(product), tier_name(tier), now),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_are_utc_and_negative_times_do_not_wrap() {
        assert_eq!((day_of(0), day_of(DAY - 1), day_of(DAY)), (0, 0, 1));
        assert_eq!(day_of(-1), -1);
    }

    #[test]
    fn per_day_covers_exactly_the_window_oldest_first() {
        let now = 10 * DAY + 5;
        let rows = per_day(
            &[10 * DAY, 10 * DAY + 9, 9 * DAY, 3 * DAY, 11 * DAY],
            now,
            3,
        );
        assert_eq!(rows, [(8, 0), (9, 1), (10, 2)]);
    }

    #[test]
    fn a_single_day_window_is_today_only() {
        assert_eq!(per_day(&[5 * DAY], 5 * DAY, 1), [(5, 1)]);
        assert!(per_day(&[], 5 * DAY, 0).is_empty());
    }

    #[test]
    fn history_is_unknown_before_the_first_snapshot_then_carried_forward() {
        let now = 10 * DAY;
        let rows = carry_forward(&[(8, 2), (10, 5)], now, 4);
        assert_eq!(rows, [(7, None), (8, Some(2)), (9, Some(2)), (10, Some(5))]);
    }

    #[test]
    fn a_snapshot_older_than_the_window_still_carries_into_it() {
        let rows = carry_forward(&[(1, 4)], 10 * DAY, 2);
        assert_eq!(rows, [(9, Some(4)), (10, Some(4))]);
    }
}
