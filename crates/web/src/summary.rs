// Le résumé d'un bot (ses deux versions, son dernier déploiement, ses abonnés premium), lu dans la
// base : une seule source pour le tableau de bord HTML et pour l'API JSON.
use platform_core::ProductId;
use platform_runner::{tier_name, Tier};
use platform_store::{Deployment, Product, SqliteStore};

pub(crate) const RECENT_DEPLOYMENTS: u32 = 30;

// Un version d'un bot, résumée pour la carte : juste de quoi dire ce qu'il reste à faire.
pub(crate) struct VersionSummary {
    pub(crate) tier: Tier,
    pub(crate) has_source: bool,
    pub(crate) has_credential: bool,
    pub(crate) app_name: Option<String>,
    pub(crate) latest: Option<Deployment>,
    // Mis à jour après coup par l'appelant (nécessite `containers`, hors de ce module volontairement
    // pur) : le container boucle en redémarrage, souvent le signe d'un jeton Discord invalide ou
    // réinitialisé (approximation : la plateforme ne voit pas le 401 de la Gateway
    // elle-même, seulement que le processus du créateur quitte et recommence).
    pub(crate) crash_looping: bool,
}

pub(crate) struct BotCard {
    pub(crate) product: Product,
    pub(crate) free: VersionSummary,
    pub(crate) premium: VersionSummary,
    pub(crate) last_deploy: Option<Deployment>,
    // Vrai : compté depuis les adhésions, pas une estimation.
    pub(crate) premium_subscribers: usize,
}

fn version_summary(
    store: &SqliteStore,
    product: ProductId,
    tier: Tier,
) -> Result<VersionSummary, ()> {
    let name = tier_name(tier);
    let credential = store.credential(product, name).map_err(|_| ())?;
    let has_source = store.source(product, name).map_err(|_| ())?.is_some();
    let latest = store
        .deployments_of(product, RECENT_DEPLOYMENTS)
        .map_err(|_| ())?
        .into_iter()
        .find(|d| d.tier == name);
    Ok(VersionSummary {
        tier,
        has_source,
        has_credential: credential.is_some(),
        app_name: credential.map(|c| c.bot_name),
        latest,
        crash_looping: false,
    })
}

pub(crate) fn gather_cards(
    store: &SqliteStore,
    products: Vec<Product>,
    now: i64,
    grace_secs: u32,
) -> Result<Vec<BotCard>, ()> {
    let mut cards = Vec::new();
    for product in products {
        let free = version_summary(store, product.id, Tier::Free)?;
        let premium = version_summary(store, product.id, Tier::Premium)?;
        let last_deploy = store
            .deployments_of(product.id, RECENT_DEPLOYMENTS)
            .map_err(|_| ())?
            .into_iter()
            .max_by_key(|d| d.created_at);
        let premium_subscribers = store
            .entitlements_of(product.id)
            .map_err(|_| ())?
            .iter()
            .filter(|e| crate::billing::counts_as_active_subscriber(e, now, grace_secs))
            .count();
        cards.push(BotCard {
            product,
            free,
            premium,
            last_deploy,
            premium_subscribers,
        });
    }
    Ok(cards)
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_store::SqliteStore;

    fn store() -> SqliteStore {
        match SqliteStore::in_memory() {
            Ok(s) => s,
            Err(e) => panic!("cannot open in-memory store: {e}"),
        }
    }

    // Deux versions du même produit peuvent chacune avoir leurs propres déploiements : celle
    // d'une version ne doit jamais récupérer le dernier déploiement de l'autre.
    #[test]
    fn each_version_shows_only_its_own_latest_deployment() {
        let s = store();
        let product = s
            .create_product(1, "Bot", 0)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(s
            .create_deployment(
                product,
                "free",
                "a".repeat(40).as_str(),
                "main",
                "manual",
                100
            )
            .is_ok());
        assert!(s
            .create_deployment(
                product,
                "premium",
                "b".repeat(40).as_str(),
                "main",
                "manual",
                200
            )
            .is_ok());
        let free = version_summary(&s, product, Tier::Free).unwrap_or_else(|()| panic!("free"));
        let premium =
            version_summary(&s, product, Tier::Premium).unwrap_or_else(|()| panic!("premium"));
        assert_eq!(free.latest.map(|d| d.commit), Some("a".repeat(40)));
        assert_eq!(premium.latest.map(|d| d.commit), Some("b".repeat(40)));
    }
}
