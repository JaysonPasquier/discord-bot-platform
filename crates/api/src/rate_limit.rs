use std::collections::HashMap;
use std::sync::Mutex;

use platform_core::{ProductId, Timestamp};

// Fenêtre glissante : on compte les requêtes dans la dernière `window_secs` secondes.
// Limite par défaut : 600 requêtes / 60 s = 10 req/s par produit.
pub struct RateLimiter {
    window_secs: i64,
    max_requests: usize,
    // Vec<Timestamp> : horodatages des requêtes récentes pour ce produit.
    buckets: Mutex<HashMap<ProductId, Vec<Timestamp>>>,
}

impl RateLimiter {
    pub fn new(window_secs: i64, max_requests: usize) -> Self {
        Self {
            window_secs,
            max_requests,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Renvoie `true` si la requête est autorisée, `false` si la limite est atteinte.
    pub fn check(&self, product: ProductId, now: Timestamp) -> bool {
        let cutoff = now - self.window_secs;
        let Ok(mut buckets) = self.buckets.lock() else {
            // Si le verrou est empoisonné, on laisse passer pour ne pas bloquer les bots.
            return true;
        };
        let bucket = buckets.entry(product).or_default();
        // Expire les anciens horodatages.
        bucket.retain(|&t| t > cutoff);
        if bucket.len() >= self.max_requests {
            return false;
        }
        bucket.push(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter() -> RateLimiter {
        RateLimiter::new(60, 3)
    }

    #[test]
    fn allows_requests_under_the_limit() {
        let rl = limiter();
        let p = ProductId(1);
        assert!(rl.check(p, 1000));
        assert!(rl.check(p, 1001));
        assert!(rl.check(p, 1002));
    }

    #[test]
    fn blocks_when_limit_is_reached() {
        let rl = limiter();
        let p = ProductId(1);
        assert!(rl.check(p, 1000));
        assert!(rl.check(p, 1001));
        assert!(rl.check(p, 1002));
        assert!(!rl.check(p, 1003));
    }

    #[test]
    fn window_expiry_unblocks_after_enough_time() {
        let rl = limiter();
        let p = ProductId(1);
        assert!(rl.check(p, 1000));
        assert!(rl.check(p, 1001));
        assert!(rl.check(p, 1002));
        assert!(!rl.check(p, 1003));
        // Les 3 premières requêtes sortent de la fenêtre de 60s.
        assert!(rl.check(p, 1061));
        assert!(rl.check(p, 1062));
        assert!(rl.check(p, 1063));
        assert!(!rl.check(p, 1064));
    }

    #[test]
    fn products_are_isolated() {
        let rl = limiter();
        let p1 = ProductId(1);
        let p2 = ProductId(2);
        assert!(rl.check(p1, 1000));
        assert!(rl.check(p1, 1001));
        assert!(rl.check(p1, 1002));
        assert!(!rl.check(p1, 1003));
        // p2 n'est pas affectée par p1.
        assert!(rl.check(p2, 1003));
    }

    #[test]
    fn exact_window_boundary_is_excluded() {
        let rl = limiter();
        let p = ProductId(1);
        // t=1000, fenêtre 60s : cutoff = 940. t=1000 > 940, donc encore valide.
        assert!(rl.check(p, 1000));
        assert!(rl.check(p, 1001));
        assert!(rl.check(p, 1002));
        assert!(!rl.check(p, 1003));
        // À t=1061, cutoff=1001 → seuls t=1001 et t=1002 restent, le troisième est libre.
        assert!(rl.check(p, 1061));
    }
}
