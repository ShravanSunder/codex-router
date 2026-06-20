//! Previous-response affinity pins.

use std::collections::HashMap;

use codex_router_core::ids::AccountId;
use codex_router_core::ids::AffinityKey;

/// In-memory affinity table used by the selection state machine.
#[derive(Clone, Debug, Default)]
pub struct AffinityTable {
    pins: HashMap<AffinityKey, AccountId>,
}

impl AffinityTable {
    /// Pins an affinity key to an account.
    pub fn pin(&mut self, affinity_key: AffinityKey, account_id: AccountId) {
        self.pins.insert(affinity_key, account_id);
    }

    /// Resolves a pin only if the pinned account is currently eligible.
    pub fn resolve(
        &self,
        affinity_key: &AffinityKey,
        is_eligible: impl FnOnce(&AccountId) -> bool,
    ) -> Option<AccountId> {
        let account_id = self.pins.get(affinity_key)?;
        if is_eligible(account_id) {
            return Some(account_id.clone());
        }

        None
    }
}
