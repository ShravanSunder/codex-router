//! Read-only live peer assessment for an established socket's graceful floor switch.

use super::*;
use codex_router_state::sqlite::AsyncSqliteStateStore;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FloorSwitchPeerAssessment {
    SelectablePeer,
    NoPeer,
    AuthorityUnavailable,
}

pub(crate) trait LiveFloorSwitchPeerAssessor: Send + Sync {
    fn assess_peer<'a>(
        &'a self,
        source_account_id: &'a AccountId,
        route_band: RouteBand,
    ) -> BoxFuture<'a, FloorSwitchPeerAssessment>;
}

#[derive(Clone)]
pub(crate) struct RuntimeFloorSwitchPeerAssessor {
    state_repository: AsyncSqliteStateStore,
    active_reservations: RouteBandReservationBooks,
    runtime_exhaustions: RouteBandRuntimeExhaustions,
    route_band_queue_health: RouteBandQueueHealth,
    selection_reservation_lock: SelectionReservationLock,
    clock: UnixClock,
}

impl RuntimeFloorSwitchPeerAssessor {
    pub(crate) fn new(
        state_repository: AsyncSqliteStateStore,
        runtime_state: &AsyncAccountSelectorRuntimeState,
        clock: UnixClock,
    ) -> Self {
        Self {
            state_repository,
            active_reservations: Arc::clone(&runtime_state.active_reservations),
            runtime_exhaustions: Arc::clone(&runtime_state.runtime_exhaustions),
            route_band_queue_health: Arc::clone(&runtime_state.route_band_queue_health),
            selection_reservation_lock: Arc::clone(&runtime_state.selection_reservation_lock),
            clock,
        }
    }
}

impl LiveFloorSwitchPeerAssessor for RuntimeFloorSwitchPeerAssessor {
    fn assess_peer<'a>(
        &'a self,
        source_account_id: &'a AccountId,
        route_band: RouteBand,
    ) -> BoxFuture<'a, FloorSwitchPeerAssessment> {
        Box::pin(async move {
            let _selection_guard = self.selection_reservation_lock.lock().await;
            let now_unix_seconds = (self.clock)();
            if route_band_queue_health_allows_selection(&self.route_band_queue_health, route_band)
                .is_err()
            {
                return FloorSwitchPeerAssessment::AuthorityUnavailable;
            }

            let active_session_overrides = {
                let reservations = match self.active_reservations.lock() {
                    Ok(reservations) => reservations,
                    Err(_) => return FloorSwitchPeerAssessment::AuthorityUnavailable,
                };
                reservations
                    .get(route_band.as_str())
                    .cloned()
                    .map(|mut book| {
                        book.purge_stale(now_unix_seconds, ACTIVE_RESERVATION_MAX_AGE_SECONDS);
                        active_session_counts_by_account(&book)
                    })
            };
            let projection = match project_route_band_selection_inputs_with_active_counts_read_only(
                &self.state_repository,
                route_band.as_str(),
                now_unix_seconds,
                ACTIVE_RESERVATION_MAX_AGE_SECONDS,
                active_session_overrides.as_ref(),
            )
            .await
            {
                Ok(projection) => projection,
                Err(_) => return FloorSwitchPeerAssessment::AuthorityUnavailable,
            };
            let runtime_exhaustions = {
                let runtime_exhaustions = match self.runtime_exhaustions.lock() {
                    Ok(exhaustions) => exhaustions,
                    Err(_) => return FloorSwitchPeerAssessment::AuthorityUnavailable,
                };
                runtime_exhaustions
                    .get(route_band.as_str())
                    .cloned()
                    .unwrap_or_default()
            };
            let peer_inputs = floor_switch_peer_inputs(
                projection.accounts(),
                source_account_id,
                &runtime_exhaustions,
                now_unix_seconds,
            );
            let assessment = assess_route_band(BurnDownRouteBandAssessmentInput::new(
                route_band,
                now_unix_seconds,
                RESPONSES_WEBSOCKET.clone(),
                peer_inputs,
            ));
            if assessment
                .accounts()
                .iter()
                .any(BurnDownAccountAssessment::is_healthy_floor_switch_peer)
            {
                FloorSwitchPeerAssessment::SelectablePeer
            } else {
                FloorSwitchPeerAssessment::NoPeer
            }
        })
    }
}

fn floor_switch_peer_inputs(
    projected_accounts: &[BurnDownAccountInput],
    source_account_id: &AccountId,
    runtime_exhaustions: &[RuntimeQuotaExhaustion],
    now_unix_seconds: u64,
) -> Vec<BurnDownAccountInput> {
    projected_accounts
        .iter()
        .filter(|account| account.provider() == RESPONSES_WEBSOCKET.provider)
        .filter(|account| account.account_id() != source_account_id)
        .filter(|account| {
            !runtime_exhaustions.iter().any(|exhaustion| {
                exhaustion.account_id == *account.account_id()
                    && now_unix_seconds < exhaustion.expires_unix_seconds
            })
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::RuntimeQuotaExhaustion;
    use super::floor_switch_peer_inputs;
    use codex_router_core::ids::AccountId;
    use codex_router_core::provider::Provider;
    use codex_router_selection::burn_down::BurnDownAccountInput;

    fn account(label: &str, provider: Provider) -> BurnDownAccountInput {
        BurnDownAccountInput::new(
            AccountId::new(format!("acct_{label}"))
                .unwrap_or_else(|error| panic!("test account id should parse: {error}")),
            label,
            provider,
            Vec::new(),
        )
    }

    #[test]
    fn floor_switch_peer_candidates_are_filtered_by_openai_profile_before_exhaustion() {
        let source = account("source", Provider::Openai);
        let source_id = source.account_id().clone();
        let openai_peer = account("openai_peer", Provider::Openai);
        let openai_peer_id = openai_peer.account_id().clone();
        let claude_peer = account("claude_peer", Provider::Claude);
        let claude_peer_id = claude_peer.account_id().clone();
        let exhausted_openai_peer = account("exhausted_openai_peer", Provider::Openai);
        let exhausted_peer_id = exhausted_openai_peer.account_id().clone();
        let accounts = vec![source, openai_peer, claude_peer, exhausted_openai_peer];
        let runtime_exhaustions = vec![RuntimeQuotaExhaustion::new(exhausted_peer_id.clone(), 900)];

        let peer_inputs =
            floor_switch_peer_inputs(&accounts, &source_id, &runtime_exhaustions, 1_000);

        assert_eq!(peer_inputs.len(), 1);
        assert_eq!(peer_inputs[0].account_id(), &openai_peer_id);
        assert!(
            peer_inputs
                .iter()
                .all(|account| account.provider() == Provider::Openai)
        );
        assert!(!peer_inputs.iter().any(|account| {
            account.account_id() == &claude_peer_id || account.account_id() == &exhausted_peer_id
        }));
    }
}
