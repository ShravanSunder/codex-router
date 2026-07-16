//! Inspection authority startup, independent live reads, and confirmation binding.

use std::sync::Arc;

use super::*;

impl<TAuthorityReader, TProvider, TRedeemRequestIdFactory>
    QuotaInteractiveSession<TAuthorityReader, TProvider, TRedeemRequestIdFactory>
where
    TAuthorityReader: ResetAuthorityReader + 'static,
    TProvider: ResetServiceProvider + 'static,
    TRedeemRequestIdFactory: RedeemRequestIdFactory,
{
    pub(super) fn open_confirmation(&mut self) {
        self.workflow.reduce(WorkflowIntent::OpenConfirmation);
        if self.workflow.phase() != WorkflowPhase::Confirming {
            return;
        }
        let Some(inspection_authority) = self.inspection_authority.take() else {
            self.workflow.reduce(WorkflowIntent::AuthorityLost(
                RenderSafeFailure::CredentialUnavailable,
            ));
            return;
        };
        let (Some(usage), Some(inventory)) =
            (self.inspection_usage, self.inspection_inventory.as_ref())
        else {
            self.workflow.reduce(WorkflowIntent::AuthorityLost(
                RenderSafeFailure::InvalidResponse,
            ));
            return;
        };
        let Some(attempt_generation) = self.current_attempt_generation() else {
            self.workflow.reduce(WorkflowIntent::AuthorityLost(
                RenderSafeFailure::InvalidResponse,
            ));
            return;
        };
        match ResetWorkflowService::<TAuthorityReader, TProvider>::bind_confirmation(
            inspection_authority,
            attempt_generation,
            usage,
            inventory,
        ) {
            Ok(authority) => self.confirmation_authority = Some(authority),
            Err(failure) => {
                self.workflow.reduce(WorkflowIntent::AuthorityLost(failure));
            }
        }
    }

    fn handle_inspection_authority(
        &mut self,
        start: InspectionStart,
        now_unix_seconds: u64,
        authority: Result<InspectionAuthority<TAuthorityReader::Authority>, RenderSafeFailure>,
    ) {
        if self.workflow.phase() != WorkflowPhase::Inspecting {
            return;
        }
        let authority = match authority {
            Ok(authority) => authority,
            Err(failure) => {
                self.workflow.reduce(WorkflowIntent::AuthorityLost(failure));
                self.workflow.reduce(WorkflowIntent::OperationCompleted(
                    CorrelatedOutcome::inspection_live_usage(
                        start.live_usage_correlation(),
                        LiveUsagePortResult::Failed(failure),
                    ),
                ));
                self.workflow.reduce(WorkflowIntent::OperationCompleted(
                    CorrelatedOutcome::inspection_credit_inventory(
                        start.credit_inventory_correlation(),
                        CreditInventoryPortResult::Failed(failure),
                    ),
                ));
                return;
            }
        };
        self.inspection_authority = Some(authority.clone());
        let usage_service = Arc::clone(&self.service);
        let usage_authority = authority.clone();
        let usage_correlation = start.live_usage_correlation();
        self.tasks.spawn(async move {
            let terminal = usage_service.inspect_usage(usage_authority).await;
            SessionTaskOutput::InspectionUsageCompleted {
                correlation: usage_correlation,
                terminal,
            }
        });
        let inventory_service = Arc::clone(&self.service);
        let inventory_correlation = start.credit_inventory_correlation();
        self.tasks.spawn(async move {
            let terminal = match i64::try_from(now_unix_seconds) {
                Ok(now_unix_seconds) => {
                    inventory_service
                        .inspect_inventory(authority, now_unix_seconds)
                        .await
                }
                Err(_) => CreditInventoryPortResult::Failed(RenderSafeFailure::InvalidResponse),
            };
            SessionTaskOutput::InspectionInventoryCompleted {
                correlation: inventory_correlation,
                terminal,
            }
        });
    }

    pub(super) fn start_inspection_authority_read(
        &mut self,
        start: InspectionStart,
        now_unix_seconds: u64,
        prepared: Result<TAuthorityReader::PreparedRead, RenderSafeFailure>,
    ) {
        if self.workflow.phase() != WorkflowPhase::Inspecting
            || !self.correlation_is_current(&start.live_usage_correlation())
        {
            return;
        }
        match prepared {
            Ok(prepared) => {
                self.inspection_authority_read = Some(PendingInspectionAuthorityRead {
                    start,
                    now_unix_seconds,
                    operation:
                        ResetWorkflowService::<TAuthorityReader, TProvider>::start_authority_read(
                            prepared,
                        ),
                });
            }
            Err(failure) => {
                self.handle_inspection_authority(start, now_unix_seconds, Err(failure));
            }
        }
    }

    pub(super) fn finish_inspection_authority_read(
        &mut self,
        authority: Result<TAuthorityReader::Authority, RenderSafeFailure>,
    ) {
        let Some(pending) = self.inspection_authority_read.take() else {
            return;
        };
        if self.workflow.phase() != WorkflowPhase::Inspecting
            || !self.correlation_is_current(&pending.start.live_usage_correlation())
        {
            return;
        }
        self.handle_inspection_authority(
            pending.start,
            pending.now_unix_seconds,
            authority.map(
                ResetWorkflowService::<TAuthorityReader, TProvider>::bind_inspection_authority,
            ),
        );
    }
}
