//! Read-only live peer assessment for an established socket's graceful floor switch.

use super::*;
use codex_router_selection::burn_down::QuotaEvidenceReason;
use codex_router_state::account::AccountStatus;
use codex_router_state::quota_snapshot::SelectorQuotaInput;
use codex_router_state::sqlite::AsyncSqliteStateStore;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FloorSwitchPeerAssessment {
    SelectablePeer,
    NoPeer,
    AuthorityUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AccountSourceAdmission {
    Permitted,
    PermittedByCreditBackedQuota,
    ReconnectRequired,
}

pub(crate) trait LiveAccountAdmissionAssessor: Send + Sync {
    fn assess_peer<'a>(
        &'a self,
        source_account_id: &'a AccountId,
        route_band: RouteBand,
    ) -> BoxFuture<'a, FloorSwitchPeerAssessment>;

    fn assess_source_account<'a>(
        &'a self,
        source_account_id: &'a AccountId,
        pinned_credential_generation: u64,
        route_band: RouteBand,
        credit_backed_admission_seen: bool,
    ) -> BoxFuture<'a, AccountSourceAdmission>;
}

#[derive(Clone)]
pub(crate) struct RuntimeAccountAdmissionAssessor {
    state_repository: AsyncSqliteStateStore,
    active_reservations: RouteBandReservationBooks,
    runtime_exhaustions: RouteBandRuntimeExhaustions,
    route_band_queue_health: RouteBandQueueHealth,
    selection_reservation_lock: SelectionReservationLock,
    clock: UnixClock,
}

impl RuntimeAccountAdmissionAssessor {
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

impl LiveAccountAdmissionAssessor for RuntimeAccountAdmissionAssessor {
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
            let peer_inputs = account_admission_peer_inputs(
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

    fn assess_source_account<'a>(
        &'a self,
        source_account_id: &'a AccountId,
        pinned_credential_generation: u64,
        route_band: RouteBand,
        credit_backed_admission_seen: bool,
    ) -> BoxFuture<'a, AccountSourceAdmission> {
        Box::pin(async move {
            if route_band != RouteBand::Responses {
                return AccountSourceAdmission::ReconnectRequired;
            }
            let now_unix_seconds = (self.clock)();
            if route_band_queue_health_allows_selection(&self.route_band_queue_health, route_band)
                .is_err()
            {
                return AccountSourceAdmission::ReconnectRequired;
            }
            let selector_inputs = match self
                .state_repository
                .selector_inputs_for_route_band(route_band.as_str(), now_unix_seconds)
                .await
            {
                Ok(inputs) => inputs,
                Err(_) => return AccountSourceAdmission::ReconnectRequired,
            };
            let policies = match self.state_repository.list_account_routing_policies().await {
                Ok(policies) => policies,
                Err(_) => return AccountSourceAdmission::ReconnectRequired,
            };
            let runtime_exhaustions = match self.runtime_exhaustions.lock() {
                Ok(exhaustions) => exhaustions
                    .get(route_band.as_str())
                    .cloned()
                    .unwrap_or_default(),
                Err(_) => return AccountSourceAdmission::ReconnectRequired,
            };
            if runtime_exhaustions.iter().any(|exhaustion| {
                exhaustion.account_id == *source_account_id
                    && now_unix_seconds < exhaustion.expires_unix_seconds
            }) {
                return AccountSourceAdmission::ReconnectRequired;
            }
            let Some(selector_input) = selector_inputs
                .iter()
                .find(|input| input.account_id() == source_account_id)
            else {
                return AccountSourceAdmission::ReconnectRequired;
            };
            let pinned_credential_generation_matches =
                selector_input.active_credential_generation() == Some(pinned_credential_generation);
            if credit_backed_admission_seen && !pinned_credential_generation_matches {
                return AccountSourceAdmission::ReconnectRequired;
            }
            let account_inputs = selector_inputs
                .iter()
                .filter(|input| input.provider() == Provider::Openai)
                .filter(|input| {
                    !runtime_exhaustions.iter().any(|exhaustion| {
                        exhaustion.account_id == *input.account_id()
                            && now_unix_seconds < exhaustion.expires_unix_seconds
                    })
                })
                .map(|input| {
                    let floor_basis_points = policies
                        .iter()
                        .find(|policy| policy.account_id() == input.account_id())
                        .map(|policy| {
                            u32::from(policy.weekly_quota_floor_basis_points().basis_points())
                        });
                    account_turn_input_from_selector_input(
                        input,
                        floor_basis_points,
                        now_unix_seconds,
                    )
                })
                .collect();
            let assessment = assess_route_band(BurnDownRouteBandAssessmentInput::new(
                route_band,
                now_unix_seconds,
                RESPONSES_WEBSOCKET.clone(),
                account_inputs,
            ));
            let Some(account) = assessment
                .accounts()
                .iter()
                .find(|account| account.account_id() == source_account_id)
            else {
                return AccountSourceAdmission::ReconnectRequired;
            };
            if account.routing_exclusion() == RoutingExclusion::WeeklyQuotaFloor
                && !credit_backed_admission_seen
            {
                // Existing ordinary sockets rely on the established floor notifier to
                // switch after the current turn; credit-backed continuation stays strict.
                return AccountSourceAdmission::Permitted;
            }
            if account.routing_exclusion() != RoutingExclusion::None {
                return AccountSourceAdmission::ReconnectRequired;
            }
            match account.availability() {
                AccountAvailability::Usable => AccountSourceAdmission::Permitted,
                AccountAvailability::Reserve
                    if account.quota_evidence_reason() == QuotaEvidenceReason::CreditBacked =>
                {
                    if pinned_credential_generation_matches
                        && assessment
                            .weighted_candidates()
                            .iter()
                            .any(|(account_id, _)| account_id == source_account_id)
                    {
                        AccountSourceAdmission::PermittedByCreditBackedQuota
                    } else {
                        AccountSourceAdmission::ReconnectRequired
                    }
                }
                AccountAvailability::Reserve => AccountSourceAdmission::Permitted,
                // Preserve established non-credit fallback sessions. They cannot use this
                // branch after the socket has admitted credit-backed work.
                AccountAvailability::Unknown if !credit_backed_admission_seen => {
                    AccountSourceAdmission::Permitted
                }
                AccountAvailability::Unknown
                | AccountAvailability::Blocked
                | AccountAvailability::Excluded => AccountSourceAdmission::ReconnectRequired,
            }
        })
    }
}

fn account_turn_input_from_selector_input(
    selector_input: &SelectorQuotaInput,
    weekly_floor_basis_points: Option<u32>,
    now_unix_seconds: u64,
) -> BurnDownAccountInput {
    let windows = selector_input
        .windows()
        .iter()
        .map(|window| {
            let status = match window.status() {
                codex_router_state::quota_snapshot::SelectorQuotaWindowStatus::Eligible => {
                    QuotaWindowStatus::Eligible
                }
                codex_router_state::quota_snapshot::SelectorQuotaWindowStatus::Stale => {
                    QuotaWindowStatus::Stale
                }
                codex_router_state::quota_snapshot::SelectorQuotaWindowStatus::Unknown => {
                    QuotaWindowStatus::Unknown
                }
                codex_router_state::quota_snapshot::SelectorQuotaWindowStatus::Ineligible => {
                    QuotaWindowStatus::Ineligible
                }
            };
            let mut fact = QuotaWindowFact::new(window.limit_window_seconds(), status)
                .with_remaining_headroom(window.remaining_headroom())
                .with_observed_unix_seconds(window.observed_unix_seconds())
                .with_effective(window.effective());
            if let Some(reset_unix_seconds) = window.reset_unix_seconds() {
                fact = fact.with_reset_unix_seconds(reset_unix_seconds);
            }
            fact
        })
        .collect();
    let eligibility = if selector_input.provider() == Provider::Openai
        && selector_input.route_band() == RouteBand::Responses.as_str()
        && weekly_floor_basis_points.is_none()
        && selector_input.has_current_credit_authority(now_unix_seconds)
    {
        CreditBackedEligibility::Eligible
    } else {
        CreditBackedEligibility::Ineligible
    };
    let mut account_input = BurnDownAccountInput::new(
        selector_input.account_id().clone(),
        selector_input.account_label(),
        selector_input.provider(),
        windows,
    )
    .with_account_enabled(selector_input.account_status() == AccountStatus::Enabled)
    .with_active_credential(selector_input.active_credential_generation().is_some())
    .with_credit_backed_eligibility(eligibility);
    if let Some(floor_basis_points) = weekly_floor_basis_points {
        account_input = account_input.with_weekly_quota_floor_basis_points(floor_basis_points);
    }
    account_input
}

fn account_admission_peer_inputs(
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
    use super::account_admission_peer_inputs;
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
            account_admission_peer_inputs(&accounts, &source_id, &runtime_exhaustions, 1_000);

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
