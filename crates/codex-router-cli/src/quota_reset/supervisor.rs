//! Command-level ownership for reset workflow effects and authority.

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::task::JoinSet;

use super::domain::ActiveCredentialGeneration;
use super::domain::ConsumePortResult;
use super::domain::ConsumeUnknownReason;
use super::domain::CreditInventoryPortResult;
use super::domain::LiveUsagePortResult;
use super::domain::LiveWeeklyUsage;
use super::domain::RenderSafeFailure;
use super::domain::ValidatedCreditInventory;
use super::service::ConfirmationAuthority;
use super::service::InspectionAuthority;
use super::service::ResetAuthorityReader;
use super::service::ResetServiceProvider;
use super::service::ResetWorkflowService;
use super::service::RevalidationReceipt;
use super::workflow::CorrelatedOutcome;
use super::workflow::InspectionStart;
use super::workflow::OperationCorrelation;
use super::workflow::ResetWorkflow;
use super::workflow::WorkflowIntent;

mod effects;
mod protocol;
mod session_state;

#[cfg(test)]
pub(crate) use super::domain::ConsumeUnknownReason as TestConsumeUnknownReason;
pub(crate) use super::domain::KnownConsumeOutcome;
#[cfg(test)]
pub(crate) use super::domain::RenderSafeFailure as TestRenderSafeFailure;
pub(crate) use super::workflow::ConfirmationSelection;
pub(crate) use super::workflow::OperationActivity;
pub(crate) use super::workflow::OperationSuccess;
pub(crate) use super::workflow::WorkflowPhase;
pub(crate) use super::workflow::WorkflowResult;
use effects::GenerationAllocator;
pub(in crate::quota_reset) use effects::ProductionRedeemRequestIdFactory;
use effects::RedeemRequestIdFactory;
use effects::SessionTaskOutput;
#[cfg(test)]
pub(crate) use protocol::LiveWeeklyDisplayFacts;
use protocol::PinnedResetTarget;
pub(crate) use protocol::PinnedTargetInvalidationReason;
#[cfg(test)]
pub(crate) use protocol::ResetCreditDisplayRecord;
pub(crate) use protocol::ResetCreditDisplayStatusDto;
pub(crate) use protocol::ResetEligibilityDisabledReason;
pub(crate) use protocol::ResetSessionIntent;
pub(in crate::quota_reset) use protocol::ResetSessionOutcome;
pub(crate) use protocol::ResetSessionPorts;
pub(crate) use protocol::ResetValueProvenance;
pub(crate) use protocol::ResetWorkflowSnapshot;

const MINIMUM_PORT_CAPACITY: usize = 1;

#[cfg(test)]
pub(crate) fn test_live_usage_success(remaining_percent: u32) -> OperationSuccess {
    OperationSuccess::LiveUsage(LiveWeeklyUsage::new(remaining_percent))
}

/// Sole reducer, authority, and reset-effect task owner for one quota command.
pub(in crate::quota_reset) struct QuotaInteractiveSession<
    TAuthorityReader,
    TProvider,
    TRedeemRequestIdFactory,
> where
    TAuthorityReader: ResetAuthorityReader + 'static,
    TProvider: ResetServiceProvider + 'static,
    TRedeemRequestIdFactory: RedeemRequestIdFactory,
{
    service: Arc<ResetWorkflowService<TAuthorityReader, TProvider>>,
    redeem_request_id_factory: TRedeemRequestIdFactory,
    workflow: ResetWorkflow,
    intent_receiver: mpsc::Receiver<ResetSessionIntent>,
    snapshot_sender: watch::Sender<ResetWorkflowSnapshot>,
    tasks: JoinSet<SessionTaskOutput<TAuthorityReader, TProvider>>,
    generations: GenerationAllocator,
    inspection_authority: Option<InspectionAuthority<TAuthorityReader::Authority>>,
    confirmation_authority: Option<ConfirmationAuthority<TAuthorityReader::Authority>>,
    inspection_usage: Option<LiveWeeklyUsage>,
    inspection_inventory: Option<ValidatedCreditInventory>,
    current_target: Option<PinnedResetTarget>,
    invalidation_reason: Option<PinnedTargetInvalidationReason>,
    terminal_outcome: Option<WorkflowResult>,
    presentation_connected: bool,
}

impl<TAuthorityReader, TProvider, TRedeemRequestIdFactory>
    QuotaInteractiveSession<TAuthorityReader, TProvider, TRedeemRequestIdFactory>
where
    TAuthorityReader: ResetAuthorityReader + 'static,
    TProvider: ResetServiceProvider + 'static,
    TRedeemRequestIdFactory: RedeemRequestIdFactory,
{
    pub(in crate::quota_reset) fn new(
        service: ResetWorkflowService<TAuthorityReader, TProvider>,
        redeem_request_id_factory: TRedeemRequestIdFactory,
        port_capacity: usize,
    ) -> (Self, ResetSessionPorts) {
        let capacity = port_capacity.max(MINIMUM_PORT_CAPACITY);
        let (intent_sender, intent_receiver) = mpsc::channel(capacity);
        let initial_snapshot =
            ResetWorkflowSnapshot::from_workflow(&ResetWorkflow::default(), None, None);
        let (snapshot_sender, snapshot_receiver) = watch::channel(initial_snapshot);
        (
            Self {
                service: Arc::new(service),
                redeem_request_id_factory,
                workflow: ResetWorkflow::default(),
                intent_receiver,
                snapshot_sender,
                tasks: JoinSet::new(),
                generations: GenerationAllocator::default(),
                inspection_authority: None,
                confirmation_authority: None,
                inspection_usage: None,
                inspection_inventory: None,
                current_target: None,
                invalidation_reason: None,
                terminal_outcome: None,
                presentation_connected: true,
            },
            ResetSessionPorts {
                intent_sender,
                snapshot_receiver,
            },
        )
    }

    pub(in crate::quota_reset) async fn run(mut self) -> ResetSessionOutcome {
        loop {
            tokio::select! {
                intent = self.intent_receiver.recv(), if self.presentation_connected => {
                    let Some(intent) = intent else {
                        if self.workflow.phase() == WorkflowPhase::Committing {
                            self.presentation_connected = false;
                            continue;
                        }
                        self.reap_precommit_for_shutdown().await;
                        return self.sanitized_outcome();
                    };
                    if let Some(outcome) = self.apply_intent(intent).await {
                        return outcome;
                    }
                }
                completed = self.tasks.join_next(), if !self.tasks.is_empty() => {
                    let Some(completed) = completed else {
                        continue;
                    };
                    if let Some(outcome) = self.apply_task_completion(completed).await {
                        return outcome;
                    }
                }
            }
        }
    }

    async fn apply_intent(&mut self, intent: ResetSessionIntent) -> Option<ResetSessionOutcome> {
        match intent {
            ResetSessionIntent::BeginInspection {
                account_id,
                active_credential_generation,
                now_unix_seconds,
            } if self.workflow.phase() == WorkflowPhase::Browse => {
                self.current_target = Some(PinnedResetTarget {
                    account_id: account_id.clone(),
                    active_credential_generation,
                });
                self.invalidation_reason = None;
                self.terminal_outcome = None;
                let attempt_generation = self.generations.allocate_attempt();
                let start = InspectionStart::new(
                    account_id,
                    ActiveCredentialGeneration::new(active_credential_generation),
                    attempt_generation,
                    self.generations.allocate_operation(),
                    self.generations.allocate_operation(),
                );
                let requests = self
                    .workflow
                    .reduce(WorkflowIntent::BeginInspection(start.clone()));
                if requests.len() == 2 {
                    let service = Arc::clone(&self.service);
                    self.tasks.spawn(async move {
                        let authority = service
                            .resolve_inspection_authority(
                                start.live_usage_correlation().account_id(),
                                start
                                    .live_usage_correlation()
                                    .active_credential_generation(),
                                now_unix_seconds,
                            )
                            .await;
                        SessionTaskOutput::InspectionAuthorityResolved {
                            start,
                            now_unix_seconds,
                            authority,
                        }
                    });
                }
            }
            ResetSessionIntent::OpenConfirmation => self.open_confirmation(),
            ResetSessionIntent::SelectNo => {
                self.workflow.reduce(WorkflowIntent::SelectNo);
            }
            ResetSessionIntent::SelectYes => {
                self.workflow.reduce(WorkflowIntent::SelectYes);
            }
            ResetSessionIntent::Confirm { now_unix_seconds } => {
                self.begin_revalidation(now_unix_seconds);
            }
            ResetSessionIntent::Cancel if self.workflow.phase() != WorkflowPhase::Committing => {
                self.cancel_precommit().await;
            }
            ResetSessionIntent::DismissResult if self.workflow.phase() == WorkflowPhase::Result => {
                self.cancel_precommit().await;
            }
            ResetSessionIntent::PinnedTargetInvalidated {
                account_id,
                active_credential_generation,
                reason,
            } if self.target_matches(&account_id, active_credential_generation)
                && self.workflow.phase() != WorkflowPhase::Committing =>
            {
                self.invalidate_pinned_target(reason).await;
            }
            ResetSessionIntent::Shutdown if self.workflow.phase() == WorkflowPhase::Committing => {
                self.presentation_connected = false;
            }
            ResetSessionIntent::Shutdown => {
                self.reap_precommit_for_shutdown().await;
                return Some(self.sanitized_outcome());
            }
            ResetSessionIntent::Cancel
            | ResetSessionIntent::DismissResult
            | ResetSessionIntent::PinnedTargetInvalidated { .. } => {}
            ResetSessionIntent::BeginInspection { .. } => {}
        }
        self.publish_snapshot();
        None
    }

    fn open_confirmation(&mut self) {
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

    fn begin_revalidation(&mut self, now_unix_seconds: u64) {
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
        let redeem_request_id = match self.redeem_request_id_factory.mint() {
            Ok(identity) => identity,
            Err(failure) => {
                self.workflow.reduce(WorkflowIntent::AuthorityLost(failure));
                return;
            }
        };
        let usage_correlation = usage_request.correlation().clone();
        let inventory_correlation = inventory_request.correlation().clone();
        let service = Arc::clone(&self.service);
        self.tasks.spawn(async move {
            let receipt = service
                .revalidate(confirmation_authority, now_unix_seconds, redeem_request_id)
                .await;
            SessionTaskOutput::RevalidationCompleted {
                usage_correlation,
                inventory_correlation,
                receipt,
            }
        });
    }

    async fn apply_task_completion(
        &mut self,
        completed: Result<SessionTaskOutput<TAuthorityReader, TProvider>, tokio::task::JoinError>,
    ) -> Option<ResetSessionOutcome> {
        let task_output = match completed {
            Ok(task_output) => task_output,
            Err(_) if self.workflow.phase() == WorkflowPhase::Committing => {
                let correlation = self.workflow.consume_correlation()?;
                SessionTaskOutput::ConsumeCompleted {
                    correlation,
                    terminal: ConsumePortResult::OutcomeUnknown(
                        ConsumeUnknownReason::InvalidResponse,
                    ),
                }
            }
            Err(_) => {
                self.workflow.reduce(WorkflowIntent::AuthorityLost(
                    RenderSafeFailure::InvalidResponse,
                ));
                self.publish_snapshot();
                return None;
            }
        };
        match task_output {
            SessionTaskOutput::InspectionAuthorityResolved {
                start,
                now_unix_seconds,
                authority,
            } => {
                self.handle_inspection_authority(start, now_unix_seconds, authority);
            }
            SessionTaskOutput::InspectionUsageCompleted {
                correlation,
                terminal,
            } => {
                if self.correlation_is_current(&correlation) {
                    if let LiveUsagePortResult::Known(usage) = terminal {
                        self.inspection_usage = Some(usage);
                        self.workflow.reduce(WorkflowIntent::OperationCompleted(
                            CorrelatedOutcome::inspection_live_usage(
                                correlation,
                                LiveUsagePortResult::Known(usage),
                            ),
                        ));
                    } else {
                        self.workflow.reduce(WorkflowIntent::OperationCompleted(
                            CorrelatedOutcome::inspection_live_usage(correlation, terminal),
                        ));
                    }
                }
            }
            SessionTaskOutput::InspectionInventoryCompleted {
                correlation,
                terminal,
            } => {
                if self.correlation_is_current(&correlation) {
                    if let CreditInventoryPortResult::Validated(inventory) = terminal {
                        self.inspection_inventory = Some(inventory.clone());
                        self.workflow.reduce(WorkflowIntent::OperationCompleted(
                            CorrelatedOutcome::inspection_credit_inventory(
                                correlation,
                                CreditInventoryPortResult::Validated(inventory),
                            ),
                        ));
                    } else {
                        self.workflow.reduce(WorkflowIntent::OperationCompleted(
                            CorrelatedOutcome::inspection_credit_inventory(correlation, terminal),
                        ));
                    }
                }
            }
            SessionTaskOutput::RevalidationCompleted {
                usage_correlation,
                inventory_correlation,
                receipt,
            } => self.finish_revalidation(usage_correlation, inventory_correlation, receipt),
            SessionTaskOutput::ConsumeCompleted {
                correlation,
                terminal,
            } => {
                self.workflow.reduce(WorkflowIntent::OperationCompleted(
                    CorrelatedOutcome::consume_credit(correlation, terminal),
                ));
                self.publish_snapshot();
                let result = self
                    .workflow
                    .result()
                    .cloned()
                    .unwrap_or(WorkflowResult::Refused(RenderSafeFailure::InvalidResponse));
                self.terminal_outcome = Some(result.clone());
                self.reap_all_tasks().await;
                if !self.presentation_connected {
                    return Some(ResetSessionOutcome::Finished(result));
                }
            }
        }
        self.publish_snapshot();
        None
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

    fn finish_revalidation(
        &mut self,
        usage_correlation: OperationCorrelation,
        inventory_correlation: OperationCorrelation,
        receipt: RevalidationReceipt<TAuthorityReader::Authority, TProvider::PreparedConsume>,
    ) {
        if self.workflow.phase() != WorkflowPhase::Revalidating {
            return;
        }
        let RevalidationReceipt {
            live_usage,
            credit_inventory,
            authorization,
        } = receipt;
        let refusal = authorization.as_ref().err().copied();
        let usage = live_usage.unwrap_or_else(|| {
            LiveUsagePortResult::Failed(refusal.unwrap_or(RenderSafeFailure::InvalidResponse))
        });
        let inventory = credit_inventory.unwrap_or_else(|| {
            CreditInventoryPortResult::Failed(refusal.unwrap_or(RenderSafeFailure::InvalidResponse))
        });
        if let LiveUsagePortResult::Known(fresh_usage) = &usage {
            self.inspection_usage = Some(*fresh_usage);
        }
        if let CreditInventoryPortResult::Validated(fresh_inventory) = &inventory {
            self.inspection_inventory = Some(fresh_inventory.clone());
        }
        self.workflow.reduce(WorkflowIntent::OperationCompleted(
            CorrelatedOutcome::revalidation_live_usage(usage_correlation, usage),
        ));
        self.workflow.reduce(WorkflowIntent::OperationCompleted(
            CorrelatedOutcome::revalidation_credit_inventory(inventory_correlation, inventory),
        ));
        let Ok(capability) = authorization else {
            return;
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

    async fn cancel_precommit(&mut self) {
        self.workflow.reduce(WorkflowIntent::Cancel);
        self.generations.allocate_attempt();
        self.tasks.abort_all();
        self.reap_all_tasks().await;
        self.inspection_authority = None;
        self.confirmation_authority = None;
        self.inspection_usage = None;
        self.inspection_inventory = None;
        self.invalidation_reason = None;
        self.terminal_outcome = None;
        self.publish_snapshot();
    }

    async fn invalidate_pinned_target(&mut self, reason: PinnedTargetInvalidationReason) {
        let failure = match reason {
            PinnedTargetInvalidationReason::AccountRemoved => RenderSafeFailure::AccountUnavailable,
            PinnedTargetInvalidationReason::CredentialGenerationChanged => {
                RenderSafeFailure::CredentialGenerationChanged
            }
        };
        self.tasks.abort_all();
        self.reap_all_tasks().await;
        self.inspection_authority = None;
        self.confirmation_authority = None;
        self.inspection_usage = None;
        self.inspection_inventory = None;
        self.invalidation_reason = Some(reason);
        self.terminal_outcome = None;
        self.workflow
            .reduce(WorkflowIntent::PinnedTargetInvalidated(failure));
        self.publish_snapshot();
    }

    async fn reap_precommit_for_shutdown(&mut self) {
        if self.workflow.phase() != WorkflowPhase::Result {
            self.workflow.reduce(WorkflowIntent::Cancel);
        }
        self.tasks.abort_all();
        self.reap_all_tasks().await;
        self.publish_snapshot();
    }

    async fn reap_all_tasks(&mut self) {
        while self.tasks.join_next().await.is_some() {}
    }
}

#[cfg(test)]
mod tests;
