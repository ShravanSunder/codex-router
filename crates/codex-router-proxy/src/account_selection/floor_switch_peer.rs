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
            let peer_inputs = projection
                .accounts()
                .iter()
                .filter(|account| account.account_id() != source_account_id)
                .filter(|account| {
                    !runtime_exhaustions.iter().any(|exhaustion| {
                        exhaustion.account_id == *account.account_id()
                            && now_unix_seconds < exhaustion.expires_unix_seconds
                    })
                })
                .cloned()
                .collect::<Vec<_>>();
            let assessment = assess_route_band(BurnDownRouteBandAssessmentInput::new(
                route_band,
                now_unix_seconds,
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
