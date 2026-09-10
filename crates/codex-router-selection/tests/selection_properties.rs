use codex_router_core::ids::AccountId;
use codex_router_quota::snapshot::SnapshotFreshness;
use codex_router_selection::eligibility::{Eligibility, SelectionCandidate};
use codex_router_selection::weighted_deficit::WeightedDeficitSelector;
use proptest::prelude::*;

fn boundary_headroom_strategy() -> impl Strategy<Value = u32> {
    prop_oneof![
        Just(0),
        Just(1),
        Just(3),
        Just(4),
        Just(7),
        Just(8),
        Just(u32::MAX),
        any::<u32>(),
    ]
}

fn freshness_strategy() -> impl Strategy<Value = SnapshotFreshness> {
    prop_oneof![
        prop_oneof![Just(0_u64), Just(1), Just(u64::MAX), any::<u64>()]
            .prop_map(|age_seconds| SnapshotFreshness::Fresh { age_seconds }),
        prop_oneof![Just(0_u64), Just(1), Just(u64::MAX), any::<u64>()]
            .prop_map(|age_seconds| SnapshotFreshness::StaleWithPenalty { age_seconds }),
        Just(SnapshotFreshness::Unknown),
    ]
}

fn candidate(remaining_headroom: u32, freshness: SnapshotFreshness) -> Option<SelectionCandidate> {
    AccountId::new("generated-account")
        .ok()
        .map(|account_id| SelectionCandidate::new(account_id, remaining_headroom, freshness))
}

fn effective_headroom(eligibility: Eligibility) -> Option<u32> {
    match eligibility {
        Eligibility::Eligible { headroom } | Eligibility::Penalized { headroom, .. } => {
            Some(headroom)
        }
        Eligibility::Ineligible { .. } => None,
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 4_096,
        .. ProptestConfig::default()
    })]

    #[test]
    fn zero_headroom_is_always_ineligible(
        freshness in freshness_strategy(),
        known_fresh_account_exists in any::<bool>(),
    ) {
        prop_assert_eq!(
            candidate(0, freshness)
                .map(|candidate| candidate.eligibility(known_fresh_account_exists)),
            Some(Eligibility::Ineligible { reason: "no_headroom" }),
        );
    }

    #[test]
    fn effective_headroom_never_exceeds_supplied_headroom(
        remaining_headroom in boundary_headroom_strategy(),
        freshness in freshness_strategy(),
        known_fresh_account_exists in any::<bool>(),
    ) {
        let generated_candidate = candidate(remaining_headroom, freshness);
        prop_assert!(generated_candidate.is_some());
        let eligibility = generated_candidate
            .map(|candidate| candidate.eligibility(known_fresh_account_exists));
        if let Some(effective_headroom) = eligibility.and_then(effective_headroom) {
            prop_assert!(effective_headroom <= remaining_headroom);
        }
    }

    #[test]
    fn fresh_candidates_preserve_positive_headroom(
        remaining_headroom in 1_u32..=u32::MAX,
        age_seconds in prop_oneof![Just(0_u64), Just(1), Just(u64::MAX), any::<u64>()],
        known_fresh_account_exists in any::<bool>(),
    ) {
        prop_assert_eq!(
            candidate(
                remaining_headroom,
                SnapshotFreshness::Fresh { age_seconds },
            ).map(|candidate| candidate.eligibility(known_fresh_account_exists)),
            Some(Eligibility::Eligible { headroom: remaining_headroom }),
        );
    }

    #[test]
    fn no_fresh_alternative_preserves_positive_headroom(
        remaining_headroom in 1_u32..=u32::MAX,
        freshness in freshness_strategy(),
    ) {
        prop_assert_eq!(
            candidate(remaining_headroom, freshness).map(|candidate| candidate.eligibility(false)),
            Some(Eligibility::Eligible { headroom: remaining_headroom }),
        );
    }

    #[test]
    fn weighted_selection_returns_only_a_supplied_candidate(
        generated_accounts in prop::collection::vec((1_u16..=u16::MAX, any::<u32>()), 1..32),
        request_cost in any::<u32>(),
    ) {
        let expected_candidate_count = generated_accounts.len();
        let mut candidates = Vec::with_capacity(expected_candidate_count);
        for (candidate_index, (account_suffix, weight)) in generated_accounts.into_iter().enumerate() {
            if let Ok(account_id) = AccountId::new(format!(
                "generated-{candidate_index}-{account_suffix}"
            )) {
                candidates.push((account_id, weight));
            }
        }
        prop_assert_eq!(candidates.len(), expected_candidate_count);
        let selected = WeightedDeficitSelector::default().select(&candidates, request_cost);
        let selected_belongs_to_candidates = selected.as_ref().is_some_and(|selected_account| {
            candidates.iter().any(|(account_id, _weight)| account_id == selected_account)
        });

        prop_assert!(selected_belongs_to_candidates);
    }
}

#[test]
fn weighted_selection_returns_none_for_an_empty_candidate_set() {
    let mut selector = WeightedDeficitSelector::default();
    assert_eq!(selector.select(&[], 1), None);
}
