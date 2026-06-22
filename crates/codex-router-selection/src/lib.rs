//! Account selection state machine for codex-router.

pub mod affinity;
pub mod eligibility;
pub mod precommit;
pub mod reservation;
pub mod turn_state;
pub mod weighted_deficit;

/// Returns this crate's package name.
#[must_use]
pub const fn package_name() -> &'static str {
    "codex-router-selection"
}

#[cfg(test)]
mod tests {
    use codex_router_core::ids::AccountId;
    use codex_router_core::ids::AffinityKey;
    use codex_router_core::ids::ReservationId;
    use codex_router_core::redaction::SecretString;
    use codex_router_quota::snapshot::SnapshotFreshness;

    use super::package_name;
    use crate::affinity::AffinityTable;
    use crate::eligibility::Eligibility;
    use crate::eligibility::SelectionCandidate;
    use crate::precommit::PrecommitFailure;
    use crate::precommit::PrecommitFailureClassifier;
    use crate::precommit::PrecommitRotationDecision;
    use crate::precommit::classify_precommit_failure;
    use crate::reservation::ReservationBook;
    use crate::reservation::ReservationHandle;
    use crate::turn_state::TurnStateEnvelopeCodec;
    use crate::weighted_deficit::SelectionDecision;
    use crate::weighted_deficit::WeightedDeficitSelector;

    #[test]
    fn reports_package_name() {
        assert_eq!(package_name(), "codex-router-selection");
    }

    #[test]
    fn eligibility_penalizes_unknown_or_stale_when_fresh_accounts_exist() {
        let fresh = candidate(
            "acct_fresh",
            80,
            SnapshotFreshness::Fresh { age_seconds: 10 },
        );
        let stale = candidate(
            "acct_stale",
            80,
            SnapshotFreshness::StaleWithPenalty { age_seconds: 600 },
        );
        let unknown = candidate("acct_unknown", 80, SnapshotFreshness::Unknown);

        assert_eq!(
            fresh.eligibility(true),
            Eligibility::Eligible { headroom: 80 }
        );
        assert_eq!(
            stale.eligibility(true),
            Eligibility::Penalized {
                headroom: 20,
                reason: "stale_quota"
            }
        );
        assert_eq!(
            unknown.eligibility(true),
            Eligibility::Penalized {
                headroom: 10,
                reason: "unknown_quota"
            }
        );
        assert_eq!(
            unknown.eligibility(false),
            Eligibility::Eligible { headroom: 80 }
        );
    }

    #[test]
    fn weighted_deficit_round_robin_balances_eligible_accounts() {
        let mut selector = WeightedDeficitSelector::default();
        let account_a = account_id("acct_a");
        let account_b = account_id("acct_b");
        let accounts = [(account_a.clone(), 10_u32), (account_b.clone(), 20_u32)];

        assert_eq!(selector.select(&accounts, 10), Some(account_b.clone()));
        assert_eq!(selector.select(&accounts, 10), Some(account_a));
        assert_eq!(selector.select(&accounts, 10), Some(account_b));
    }

    #[test]
    fn reservations_reduce_immediate_headroom_until_released() {
        let mut reservations = ReservationBook::default();
        let account = account_id("acct_primary");
        let reservation = ReservationId::new("reservation_1");

        assert_eq!(reservations.available_headroom(&account, 50), 50);
        reservations.reserve(reservation.clone(), account.clone(), 35);
        assert_eq!(reservations.available_headroom(&account, 50), 15);
        reservations.release(&reservation);
        assert_eq!(reservations.available_headroom(&account, 50), 50);
    }

    #[test]
    fn affinity_overrides_balance_only_when_pinned_account_is_eligible() {
        let mut affinity = AffinityTable::default();
        let key = AffinityKey::new("previous_response_1");
        let account = account_id("acct_pinned");
        affinity.pin(key.clone(), account.clone());

        assert_eq!(
            affinity.resolve(&key, |candidate| candidate == &account),
            Some(account)
        );
        assert_eq!(affinity.resolve(&key, |_candidate| false), None);
    }

    #[test]
    fn turn_state_envelope_roundtrips_and_rejects_tampering() {
        let codec = TurnStateEnvelopeCodec::new(SecretString::new("signing-key-canary"));
        let account = account_id("acct_primary");
        let envelope = match codec.encode(&account, Some(SecretString::new("upstream-token"))) {
            Ok(envelope) => envelope,
            Err(error) => panic!("turn state should encode: {error}"),
        };

        assert!(!format!("{envelope:?}").contains("upstream-token"));
        let decoded = match codec.decode(&envelope) {
            Ok(decoded) => decoded,
            Err(error) => panic!("turn state should decode: {error}"),
        };
        assert_eq!(decoded.account_id(), &account);
        assert_eq!(
            decoded.upstream_token().map(SecretString::expose_secret),
            Some("upstream-token")
        );

        let tampered = envelope.as_str().replace('a', "b");
        assert!(codec.decode_str(&tampered).is_err());
    }

    #[test]
    fn precommit_rotation_is_narrow_and_does_not_gate_timeouts() {
        assert_eq!(
            classify_precommit_failure(PrecommitFailure::AuthenticationRejected),
            PrecommitRotationDecision::RotateAccount
        );
        assert_eq!(
            classify_precommit_failure(PrecommitFailure::QuotaExhausted),
            PrecommitRotationDecision::RotateAccount
        );
        assert_eq!(
            classify_precommit_failure(PrecommitFailure::Timeout),
            PrecommitRotationDecision::ReturnToCodex
        );
        assert_eq!(
            classify_precommit_failure(PrecommitFailure::MalformedResponse),
            PrecommitRotationDecision::ReturnToCodex
        );
    }

    #[test]
    fn selection_decision_and_reservation_handle_are_proxy_contracts() {
        let account = account_id("acct_selected");
        let reservation_id = ReservationId::new("reservation_proxy");
        let handle = ReservationHandle::new(reservation_id.clone(), account.clone(), 11);
        let decision = SelectionDecision::new(
            account.clone(),
            handle.clone(),
            "weighted_deficit",
            "fresh_quota",
        );

        assert_eq!(decision.account_id(), &account);
        assert_eq!(decision.reservation_handle(), &handle);
        assert_eq!(decision.affinity_reason(), "weighted_deficit");
        assert_eq!(decision.audit_reason(), "fresh_quota");
        assert_eq!(handle.reservation_id(), &reservation_id);
        assert_eq!(handle.headroom_cost(), 11);
    }

    #[test]
    fn named_precommit_classifier_matches_free_function() {
        let classifier = PrecommitFailureClassifier;

        assert_eq!(
            classifier.classify(PrecommitFailure::QuotaExhausted),
            PrecommitRotationDecision::RotateAccount
        );
        assert_eq!(
            classifier.classify(PrecommitFailure::Timeout),
            PrecommitRotationDecision::ReturnToCodex
        );
    }

    fn candidate(
        account_id_value: &str,
        remaining_headroom: u32,
        freshness: SnapshotFreshness,
    ) -> SelectionCandidate {
        SelectionCandidate::new(account_id(account_id_value), remaining_headroom, freshness)
    }

    fn account_id(value: &str) -> AccountId {
        match AccountId::new(value) {
            Ok(account_id) => account_id,
            Err(error) => panic!("account id should parse: {error}"),
        }
    }
}
