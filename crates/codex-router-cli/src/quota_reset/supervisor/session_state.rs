//! Session snapshot publication and current-attempt identity checks.

use codex_router_core::ids::AccountId;

use super::QuotaInteractiveSession;
use super::effects::RedeemRequestIdFactory;
use super::protocol::ResetSessionOutcome;
use super::protocol::ResetWorkflowSnapshot;
use crate::quota_reset::domain::AttemptGeneration;
use crate::quota_reset::service::ResetAuthorityReader;
use crate::quota_reset::service::ResetServiceProvider;
use crate::quota_reset::workflow::OperationCorrelation;

impl<TAuthorityReader, TProvider, TRedeemRequestIdFactory>
    QuotaInteractiveSession<TAuthorityReader, TProvider, TRedeemRequestIdFactory>
where
    TAuthorityReader: ResetAuthorityReader + 'static,
    TProvider: ResetServiceProvider + 'static,
    TRedeemRequestIdFactory: RedeemRequestIdFactory,
{
    pub(super) fn publish_snapshot(&self) {
        self.snapshot_sender
            .send_replace(ResetWorkflowSnapshot::from_workflow(
                &self.workflow,
                self.current_target.clone(),
                self.invalidation_reason,
            ));
    }

    pub(super) fn target_matches(&self, account_id: &AccountId, generation: u64) -> bool {
        self.current_target.as_ref().is_some_and(|target| {
            target.account_id == *account_id && target.active_credential_generation == generation
        })
    }

    pub(super) fn sanitized_outcome(&self) -> ResetSessionOutcome {
        self.terminal_outcome.clone().map_or(
            ResetSessionOutcome::Cancelled,
            ResetSessionOutcome::Finished,
        )
    }

    pub(super) fn correlation_is_current(&self, correlation: &OperationCorrelation) -> bool {
        self.current_attempt_generation()
            .is_some_and(|generation| generation == correlation.attempt_generation())
    }

    pub(super) fn current_attempt_generation(&self) -> Option<AttemptGeneration> {
        self.generations.current_attempt()
    }
}
