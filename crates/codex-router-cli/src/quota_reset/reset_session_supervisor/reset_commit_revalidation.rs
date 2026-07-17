//! Revalidation authority, fresh-fact arbitration, and commit authorization.

use std::sync::Arc;

use super::*;

impl<TAuthorityReader, TProvider, TRedeemRequestIdFactory>
    QuotaInteractiveSession<TAuthorityReader, TProvider, TRedeemRequestIdFactory>
where
    TAuthorityReader: ResetAuthorityReader + 'static,
    TProvider: ResetServiceProvider + 'static,
    TRedeemRequestIdFactory: RedeemRequestIdFactory,
{
    pub(super) fn begin_revalidation(&mut self) {
        let usage_generation = self.generations.allocate_operation();
        let inventory_generation = self.generations.allocate_operation();
        let requests = self.workflow.reduce(WorkflowIntent::Confirm {
            live_usage_operation_generation: usage_generation,
            credit_inventory_operation_generation: inventory_generation,
        });
        let [usage_request, inventory_request] = requests.as_slice() else {
            return;
        };
        let Some(confirmation_authority) = self.confirmation_authority.take() else {
            self.workflow.reduce(WorkflowIntent::AuthorityLost(
                RenderSafeFailure::CredentialUnavailable,
            ));
            return;
        };
        let now_unix_seconds = match self.clock.now_unix_seconds() {
            Ok(now_unix_seconds) => now_unix_seconds,
            Err(failure) => {
                self.refuse_revalidation(
                    usage_request.correlation().clone(),
                    inventory_request.correlation().clone(),
                    failure,
                );
                return;
            }
        };
        let usage_correlation = usage_request.correlation().clone();
        let inventory_correlation = inventory_request.correlation().clone();
        let service = Arc::clone(&self.service);
        let account_id = confirmation_authority.account_id().clone();
        let generation = confirmation_authority.active_credential_generation();
        self.tasks.spawn(async move {
            let prepared = service
                .prepare_authority_read(&account_id, generation, now_unix_seconds)
                .await;
            SessionTaskOutput::RevalidationAuthorityPrepared {
                usage_correlation,
                inventory_correlation,
                now_unix_seconds,
                confirmation: confirmation_authority,
                prepared,
            }
        });
    }

    pub(super) fn start_revalidation_authority_read(
        &mut self,
        usage_correlation: OperationCorrelation,
        inventory_correlation: OperationCorrelation,
        now_unix_seconds: u64,
        confirmation: ConfirmationAuthority<TAuthorityReader::Authority>,
        prepared: Result<TAuthorityReader::PreparedRead, RenderSafeFailure>,
    ) {
        if self.workflow.phase() != WorkflowPhase::Revalidating
            || !self.correlation_is_current(&usage_correlation)
            || !self.correlation_is_current(&inventory_correlation)
        {
            return;
        }
        match prepared {
            Ok(prepared) => {
                self.revalidation_authority_read = Some(PendingRevalidationAuthorityRead {
                    usage_correlation,
                    inventory_correlation,
                    now_unix_seconds,
                    confirmation,
                    operation:
                        ResetWorkflowService::<TAuthorityReader, TProvider>::start_authority_read(
                            prepared,
                        ),
                });
            }
            Err(failure) => {
                self.refuse_revalidation(usage_correlation, inventory_correlation, failure);
            }
        }
    }

    pub(super) fn finish_revalidation_authority_read(
        &mut self,
        authority: Result<TAuthorityReader::Authority, RenderSafeFailure>,
    ) {
        let Some(pending) = self.revalidation_authority_read.take() else {
            return;
        };
        if self.workflow.phase() != WorkflowPhase::Revalidating
            || !self.correlation_is_current(&pending.usage_correlation)
            || !self.correlation_is_current(&pending.inventory_correlation)
        {
            return;
        }
        let context = authority.and_then(|authority| {
            ResetWorkflowService::<TAuthorityReader, TProvider>::bind_revalidation_context(
                pending.confirmation,
                authority,
                pending.now_unix_seconds,
            )
        });
        let context = match context {
            Ok(context) => context,
            Err(failure) => {
                self.refuse_revalidation(
                    pending.usage_correlation,
                    pending.inventory_correlation,
                    failure,
                );
                return;
            }
        };
        self.revalidation_context = Some(context.clone());
        let usage_service = Arc::clone(&self.service);
        let usage_context = context.clone();
        let usage_correlation = pending.usage_correlation;
        self.tasks.spawn(async move {
            let terminal = usage_service.revalidate_usage(usage_context).await;
            SessionTaskOutput::RevalidationUsageCompleted {
                correlation: usage_correlation,
                terminal,
            }
        });
        let inventory_service = Arc::clone(&self.service);
        let inventory_correlation = pending.inventory_correlation;
        let now_unix_seconds = pending.now_unix_seconds;
        self.tasks.spawn(async move {
            let terminal = match i64::try_from(now_unix_seconds) {
                Ok(now_unix_seconds) => {
                    inventory_service
                        .revalidate_inventory(context, now_unix_seconds)
                        .await
                }
                Err(_) => CreditInventoryPortResult::Failed(RenderSafeFailure::InvalidResponse),
            };
            SessionTaskOutput::RevalidationInventoryCompleted {
                correlation: inventory_correlation,
                terminal,
            }
        });
    }

    pub(super) fn finish_revalidation_if_ready(&mut self) {
        if self.workflow.phase() != WorkflowPhase::Revalidating
            || self.revalidation_context.is_none()
            || self.revalidation_usage.is_none()
            || self.revalidation_inventory.is_none()
        {
            return;
        }
        let (
            Some(context),
            Some((usage_correlation, usage)),
            Some((inventory_correlation, inventory)),
        ) = (
            self.revalidation_context.take(),
            self.revalidation_usage.take(),
            self.revalidation_inventory.take(),
        )
        else {
            return;
        };
        if let LiveUsagePortResult::Known(fresh_usage) = &usage {
            self.inspection_usage = Some(*fresh_usage);
        }
        if let CreditInventoryPortResult::Validated(fresh_inventory) = &inventory {
            self.inspection_inventory = Some(fresh_inventory.clone());
        }
        let validated =
            match ResetWorkflowService::<TAuthorityReader, TProvider>::validate_revalidation(
                context, &usage, &inventory,
            ) {
                Ok(validated) => validated,
                Err(failure) => {
                    self.refuse_revalidation(usage_correlation, inventory_correlation, failure);
                    return;
                }
            };
        let commit_unix_seconds = match self.clock.now_unix_seconds() {
            Ok(commit_unix_seconds) => commit_unix_seconds,
            Err(failure) => {
                self.refuse_revalidation(usage_correlation, inventory_correlation, failure);
                return;
            }
        };
        if let Err(failure) =
            ResetWorkflowService::<TAuthorityReader, TProvider>::validate_commit_time(
                &validated,
                commit_unix_seconds,
            )
        {
            self.refuse_revalidation(usage_correlation, inventory_correlation, failure);
            return;
        }
        let redeem_request_id = match self.redeem_request_id_factory.mint() {
            Ok(redeem_request_id) => redeem_request_id,
            Err(failure) => {
                self.refuse_revalidation(usage_correlation, inventory_correlation, failure);
                return;
            }
        };
        let capability =
            match self
                .service
                .authorize_commit(validated, commit_unix_seconds, redeem_request_id)
            {
                Ok(capability) => capability,
                Err(failure) => {
                    self.refuse_revalidation(usage_correlation, inventory_correlation, failure);
                    return;
                }
            };
        let consume_generation = self.generations.allocate_operation();
        let requests = self.workflow.reduce(WorkflowIntent::CommitAuthorized {
            consume_operation_generation: consume_generation,
        });
        let Some(correlation) = requests
            .first()
            .map(|request| request.correlation().clone())
        else {
            return;
        };
        let service = Arc::clone(&self.service);
        self.tasks.spawn(async move {
            let terminal = service.consume(capability).await;
            SessionTaskOutput::ConsumeCompleted {
                correlation,
                terminal,
            }
        });
    }

    fn refuse_revalidation(
        &mut self,
        usage_correlation: OperationCorrelation,
        inventory_correlation: OperationCorrelation,
        failure: RenderSafeFailure,
    ) {
        self.revalidation_context = None;
        self.revalidation_usage = None;
        self.revalidation_inventory = None;
        self.workflow.reduce(WorkflowIntent::RevalidationRefused {
            live_usage_correlation: usage_correlation,
            credit_inventory_correlation: inventory_correlation,
            failure,
        });
    }
}
