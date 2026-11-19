use crate::ids::{GuildId, ProductId};

pub type Timestamp = i64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PremiumStatus {
    Active,
    Grace,
    Expired,
}

impl PremiumStatus {
    pub fn grants_access(self) -> bool {
        !matches!(self, Self::Expired)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Grace => "grace",
            Self::Expired => "expired",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entitlement {
    pub guild: GuildId,
    pub product: ProductId,
    pub paid_until: Timestamp,
    pub revoked: bool,
    pub canceled: bool,
}

impl Entitlement {
    // `i64::MIN` : jamais payé. Zéro serait une vraie date (1970) et ouvrirait une grâce.
    pub fn new(guild: GuildId, product: ProductId) -> Self {
        Self {
            guild,
            product,
            paid_until: i64::MIN,
            revoked: false,
            canceled: false,
        }
    }

    // Le premium s'arrête exactement à `paid_until` ; la grâce couvre un renouvellement payé en retard.
    // Un droit révoqué (litige) expire tout de suite, sans grâce. Un abonnement annulé
    // n'aura pas de renouvellement à attendre : pas de grâce non plus (annulation, remboursement).
    pub fn status(&self, now: Timestamp, grace_secs: u32) -> PremiumStatus {
        let grace_secs = if self.canceled { 0 } else { grace_secs };
        if self.revoked || now >= self.paid_until.saturating_add(i64::from(grace_secs)) {
            return PremiumStatus::Expired;
        }
        if now < self.paid_until {
            PremiumStatus::Active
        } else {
            PremiumStatus::Grace
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entitlement(paid_until: Timestamp, revoked: bool) -> Entitlement {
        Entitlement {
            guild: GuildId(1),
            product: ProductId(2),
            paid_until,
            revoked,
            canceled: false,
        }
    }

    #[test]
    fn a_canceled_subscription_gets_no_grace() {
        let e = Entitlement {
            canceled: true,
            ..entitlement(1_000, false)
        };
        assert_eq!(e.status(999, 60), PremiumStatus::Active);
        assert_eq!(e.status(1_000, 60), PremiumStatus::Expired);
    }

    #[test]
    fn active_until_the_last_second_before_paid_until() {
        assert_eq!(
            entitlement(1_000, false).status(999, 60),
            PremiumStatus::Active
        );
    }

    #[test]
    fn grace_starts_exactly_at_paid_until() {
        assert_eq!(
            entitlement(1_000, false).status(1_000, 60),
            PremiumStatus::Grace
        );
    }

    #[test]
    fn grace_lasts_until_the_last_second_before_its_end() {
        assert_eq!(
            entitlement(1_000, false).status(1_059, 60),
            PremiumStatus::Grace
        );
    }

    #[test]
    fn expired_exactly_at_the_end_of_grace() {
        assert_eq!(
            entitlement(1_000, false).status(1_060, 60),
            PremiumStatus::Expired
        );
    }

    #[test]
    fn zero_grace_expires_at_paid_until() {
        assert_eq!(
            entitlement(1_000, false).status(1_000, 0),
            PremiumStatus::Expired
        );
    }

    #[test]
    fn revoked_is_expired_even_when_paid() {
        assert_eq!(
            entitlement(1_000, true).status(0, 60),
            PremiumStatus::Expired
        );
    }

    #[test]
    fn statuses_have_stable_names() {
        assert_eq!(PremiumStatus::Active.as_str(), "active");
        assert_eq!(PremiumStatus::Grace.as_str(), "grace");
        assert_eq!(PremiumStatus::Expired.as_str(), "expired");
    }

    #[test]
    fn only_expired_denies_access() {
        assert!(PremiumStatus::Active.grants_access());
        assert!(PremiumStatus::Grace.grants_access());
        assert!(!PremiumStatus::Expired.grants_access());
    }

    #[test]
    fn far_future_end_date_does_not_overflow() {
        assert_eq!(
            entitlement(i64::MAX, false).status(0, u32::MAX),
            PremiumStatus::Active
        );
    }
}
