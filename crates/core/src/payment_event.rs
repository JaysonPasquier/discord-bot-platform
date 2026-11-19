use crate::entitlement::{Entitlement, Timestamp};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentEvent {
    Paid { period_end: Timestamp },
    Canceled { effective: Timestamp },
    Disputed,
    DisputeWon,
}

const DAY_SECS: i64 = 86_400;

impl Entitlement {
    // Un renouvellement prolonge la période en cours ; sinon elle démarre maintenant.
    pub fn payment_for_days(&self, now: Timestamp, days: u32) -> PaymentEvent {
        let start = self.paid_until.max(now);
        PaymentEvent::Paid {
            period_end: start.saturating_add(i64::from(days) * DAY_SECS),
        }
    }

    // Un paiement ne compte que s'il allonge la période : un événement rejoué ou plus ancien
    // ne change rien, et deux paiements dans le désordre donnent le même résultat. Un paiement
    // plus long met fin à l'annulation (réabonnement) mais ne lève jamais une révocation pour
    // litige : seul `DisputeWon` le fait.
    pub fn apply(self, event: PaymentEvent) -> Self {
        match event {
            PaymentEvent::Paid { period_end } if period_end > self.paid_until => Self {
                paid_until: period_end,
                canceled: false,
                ..self
            },
            PaymentEvent::Paid { .. } => self,
            PaymentEvent::Canceled { effective } => Self {
                paid_until: self.paid_until.min(effective),
                canceled: true,
                ..self
            },
            PaymentEvent::Disputed => Self {
                revoked: true,
                ..self
            },
            PaymentEvent::DisputeWon => Self {
                revoked: false,
                ..self
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entitlement::PremiumStatus;
    use crate::ids::{GuildId, ProductId};

    fn fresh() -> Entitlement {
        Entitlement::new(GuildId(1), ProductId(2))
    }

    fn paid(period_end: Timestamp) -> PaymentEvent {
        PaymentEvent::Paid { period_end }
    }

    #[test]
    fn a_first_payment_starts_now() {
        assert_eq!(
            fresh().payment_for_days(1_000, 2),
            PaymentEvent::Paid {
                period_end: 1_000 + 2 * 86_400
            }
        );
    }

    #[test]
    fn a_renewal_starts_at_the_end_of_the_current_period() {
        let e = fresh().apply(paid(5_000_000));
        assert_eq!(
            e.payment_for_days(1_000, 1),
            PaymentEvent::Paid {
                period_end: 5_000_000 + 86_400
            }
        );
    }

    #[test]
    fn a_payment_after_expiry_starts_now() {
        let e = fresh().apply(paid(500));
        assert_eq!(
            e.payment_for_days(1_000, 1),
            PaymentEvent::Paid {
                period_end: 1_000 + 86_400
            }
        );
    }

    #[test]
    fn a_huge_number_of_days_cannot_overflow() {
        assert_eq!(
            fresh().payment_for_days(i64::MAX - 1, u32::MAX),
            PaymentEvent::Paid {
                period_end: i64::MAX
            }
        );
    }

    #[test]
    fn a_new_entitlement_never_grants_access() {
        assert_eq!(fresh().status(0, u32::MAX), PremiumStatus::Expired);
    }

    #[test]
    fn a_payment_activates_premium_until_the_period_end() {
        let e = fresh().apply(paid(1_000));
        assert_eq!(e.status(999, 0), PremiumStatus::Active);
        assert_eq!(e.status(1_000, 0), PremiumStatus::Expired);
    }

    #[test]
    fn a_renewal_extends_the_period() {
        assert_eq!(
            fresh().apply(paid(1_000)).apply(paid(2_000)).paid_until,
            2_000
        );
    }

    #[test]
    fn an_older_payment_never_shortens_the_period() {
        assert_eq!(
            fresh().apply(paid(2_000)).apply(paid(1_000)).paid_until,
            2_000
        );
    }

    #[test]
    fn replaying_a_payment_changes_nothing() {
        let once = fresh().apply(paid(1_000));
        assert_eq!(once.apply(paid(1_000)), once);
    }

    #[test]
    fn payments_arriving_out_of_order_give_the_same_result() {
        let a = fresh().apply(paid(1_000)).apply(paid(2_000));
        let b = fresh().apply(paid(2_000)).apply(paid(1_000));
        assert_eq!(a, b);
    }

    #[test]
    fn cancel_ends_premium_at_the_effective_time() {
        let e = fresh()
            .apply(paid(2_000))
            .apply(PaymentEvent::Canceled { effective: 1_500 });
        assert_eq!(e.paid_until, 1_500);
    }

    #[test]
    fn cancel_never_extends_the_period() {
        let e = fresh()
            .apply(paid(1_000))
            .apply(PaymentEvent::Canceled { effective: 5_000 });
        assert_eq!(e.paid_until, 1_000);
    }

    #[test]
    fn an_immediate_cancel_ends_access_even_with_a_grace_period() {
        let e = fresh()
            .apply(paid(2_000))
            .apply(PaymentEvent::Canceled { effective: 1_500 });
        assert!(e.canceled);
        assert_eq!(e.status(1_499, 600), PremiumStatus::Active);
        assert_eq!(e.status(1_500, 600), PremiumStatus::Expired);
    }

    #[test]
    fn a_cancel_at_period_end_removes_the_grace_but_keeps_the_period() {
        let e = fresh()
            .apply(paid(2_000))
            .apply(PaymentEvent::Canceled { effective: 9_000 });
        assert_eq!(e.paid_until, 2_000);
        assert_eq!(e.status(1_999, 600), PremiumStatus::Active);
        assert_eq!(e.status(2_000, 600), PremiumStatus::Expired);
    }

    #[test]
    fn paying_again_after_a_cancel_restores_the_grace() {
        let e = fresh()
            .apply(paid(2_000))
            .apply(PaymentEvent::Canceled { effective: 1_500 })
            .apply(paid(3_000));
        assert!(!e.canceled);
        assert_eq!(e.status(3_000, 600), PremiumStatus::Grace);
    }

    #[test]
    fn an_older_payment_arriving_late_does_not_undo_a_cancel() {
        let e = fresh()
            .apply(paid(2_000))
            .apply(PaymentEvent::Canceled { effective: 1_500 })
            .apply(paid(1_200));
        assert!(e.canceled);
        assert_eq!(e.paid_until, 1_500);
    }

    #[test]
    fn replaying_a_payment_keeps_a_cancel() {
        let e = fresh()
            .apply(paid(2_000))
            .apply(PaymentEvent::Canceled { effective: 2_000 })
            .apply(paid(2_000));
        assert!(e.canceled);
    }

    #[test]
    fn a_dispute_revokes_at_once() {
        let e = fresh().apply(paid(2_000)).apply(PaymentEvent::Disputed);
        assert!(e.revoked);
        assert_eq!(e.status(0, 60), PremiumStatus::Expired);
    }

    #[test]
    fn a_payment_does_not_lift_a_revocation() {
        let e = fresh().apply(PaymentEvent::Disputed).apply(paid(2_000));
        assert!(e.revoked);
        assert_eq!(e.status(0, 60), PremiumStatus::Expired);
    }

    #[test]
    fn winning_the_dispute_restores_access() {
        let e = fresh()
            .apply(paid(2_000))
            .apply(PaymentEvent::Disputed)
            .apply(PaymentEvent::DisputeWon);
        assert!(!e.revoked);
        assert_eq!(e.status(1_000, 0), PremiumStatus::Active);
    }

    #[test]
    fn events_keep_the_guild_and_product() {
        let e = fresh().apply(paid(1)).apply(PaymentEvent::Disputed);
        assert_eq!((e.guild, e.product), (GuildId(1), ProductId(2)));
    }
}
