//! Smooth weighted deficit account selection.

use std::collections::HashMap;

use codex_router_core::ids::AccountId;

use crate::reservation::ReservationHandle;

/// Selection decision returned to proxy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectionDecision {
    account_id: AccountId,
    reservation_handle: ReservationHandle,
    affinity_reason: String,
    audit_reason: String,
}

impl SelectionDecision {
    /// Creates a selection decision.
    #[must_use]
    pub fn new(
        account_id: AccountId,
        reservation_handle: ReservationHandle,
        affinity_reason: impl Into<String>,
        audit_reason: impl Into<String>,
    ) -> Self {
        Self {
            account_id,
            reservation_handle,
            affinity_reason: affinity_reason.into(),
            audit_reason: audit_reason.into(),
        }
    }

    /// Returns selected account id.
    #[must_use]
    pub const fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// Returns reservation handle.
    #[must_use]
    pub const fn reservation_handle(&self) -> &ReservationHandle {
        &self.reservation_handle
    }

    /// Returns affinity reason.
    #[must_use]
    pub fn affinity_reason(&self) -> &str {
        &self.affinity_reason
    }

    /// Returns audit reason.
    #[must_use]
    pub fn audit_reason(&self) -> &str {
        &self.audit_reason
    }
}

/// Weighted selector state.
#[derive(Clone, Debug, Default)]
pub struct WeightedDeficitSelector {
    current_weights: HashMap<AccountId, i64>,
}

impl WeightedDeficitSelector {
    /// Selects one account from eligible accounts and advances selector state.
    pub fn select(
        &mut self,
        accounts: &[(AccountId, u32)],
        _request_cost: u32,
    ) -> Option<AccountId> {
        if accounts.is_empty() {
            return None;
        }

        let total_weight = accounts.iter().fold(0_i64, |total, (_account_id, weight)| {
            total + i64::from(*weight)
        });
        let mut selected: Option<AccountId> = None;
        let mut selected_weight = i64::MIN;

        for (account_id, weight) in accounts {
            let current_weight = self.current_weights.entry(account_id.clone()).or_insert(0);
            *current_weight += i64::from(*weight);
            if *current_weight > selected_weight {
                selected = Some(account_id.clone());
                selected_weight = *current_weight;
            }
        }

        let selected = selected?;
        if let Some(current_weight) = self.current_weights.get_mut(&selected) {
            *current_weight -= total_weight;
        }

        Some(selected)
    }

    /// Advances selector state as if `selected_account` won this round.
    ///
    /// This is used when a higher-level routing policy pins or temporarily
    /// holds an account, while weighted deficit still needs to account for
    /// that reuse in future fairness decisions.
    pub fn record_selection(
        &mut self,
        accounts: &[(AccountId, u32)],
        selected_account: &AccountId,
    ) -> bool {
        if accounts.is_empty() {
            return false;
        }
        if !accounts
            .iter()
            .any(|(account_id, _weight)| account_id == selected_account)
        {
            return false;
        }

        let total_weight = accounts.iter().fold(0_i64, |total, (_account_id, weight)| {
            total + i64::from(*weight)
        });

        for (account_id, weight) in accounts {
            let current_weight = self.current_weights.entry(account_id.clone()).or_insert(0);
            *current_weight += i64::from(*weight);
        }

        if let Some(current_weight) = self.current_weights.get_mut(selected_account) {
            *current_weight -= total_weight;
        }

        true
    }
}
