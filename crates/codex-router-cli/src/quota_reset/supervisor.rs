//! Command-level ownership for reset workflow effects and authority.

use std::sync::Arc;

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
use super::service::RevalidationContext;
use super::service::StartedAuthorityRead;
use super::workflow::CorrelatedOutcome;
use super::workflow::InspectionStart;
use super::workflow::OperationCorrelation;
use super::workflow::ResetWorkflow;
use super::workflow::WorkflowIntent;

mod effects;
mod inspection;
mod protocol;
mod revalidation;
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
pub(in crate::quota_reset) use effects::ProductionResetClock;
use effects::RedeemRequestIdFactory;
use effects::ResetClock;
use effects::SessionTaskOutput;
#[cfg(test)]
pub(crate) use protocol::LiveWeeklyDisplayFacts;
use protocol::PinnedResetTarget;
pub(crate) use protocol::PinnedTargetInvalidationReason;
#[cfg(test)]
pub(crate) use protocol::ResetCreditDisplayRecord;
pub(crate) use protocol::ResetCreditDisplayStatusDto;
pub(crate) use protocol::ResetEligibilityDisabledReason;
pub(crate) use protocol::ResetIntentSender;
pub(crate) use protocol::ResetSessionIntent;
pub(crate) use protocol::ResetSessionOutcome;
pub(crate) use protocol::ResetSessionPorts;
pub(crate) use protocol::ResetValueProvenance;
pub(crate) use protocol::ResetWorkflowSnapshot;

#[cfg(test)]
pub(crate) fn test_live_usage_success(remaining_percent: u32) -> OperationSuccess {
    OperationSuccess::LiveUsage(LiveWeeklyUsage::new(remaining_percent))
}

struct PendingInspectionAuthorityRead<TAuthority: super::service::ResetAuthority> {
    start: InspectionStart,
    now_unix_seconds: u64,
    operation: StartedAuthorityRead<TAuthority>,
}

struct PendingRevalidationAuthorityRead<TAuthority: super::service::ResetAuthority> {
    usage_correlation: OperationCorrelation,
    inventory_correlation: OperationCorrelation,
    now_unix_seconds: u64,
    confirmation: ConfirmationAuthority<TAuthority>,
    operation: StartedAuthorityRead<TAuthority>,
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
    clock: Arc<dyn ResetClock>,
    workflow: ResetWorkflow,
    intent_receiver: tokio::sync::mpsc::UnboundedReceiver<ResetSessionIntent>,
    snapshot_sender: watch::Sender<ResetWorkflowSnapshot>,
    tasks: JoinSet<SessionTaskOutput<TAuthorityReader>>,
    inspection_authority_read: Option<PendingInspectionAuthorityRead<TAuthorityReader::Authority>>,
    revalidation_authority_read:
        Option<PendingRevalidationAuthorityRead<TAuthorityReader::Authority>>,
    generations: GenerationAllocator,
    inspection_authority: Option<InspectionAuthority<TAuthorityReader::Authority>>,
    confirmation_authority: Option<ConfirmationAuthority<TAuthorityReader::Authority>>,
    inspection_usage: Option<LiveWeeklyUsage>,
    inspection_inventory: Option<ValidatedCreditInventory>,
    revalidation_context: Option<RevalidationContext<TAuthorityReader::Authority>>,
    revalidation_usage: Option<(OperationCorrelation, LiveUsagePortResult)>,
    revalidation_inventory: Option<(OperationCorrelation, CreditInventoryPortResult)>,
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
        clock: Arc<dyn ResetClock>,
    ) -> (Self, ResetSessionPorts) {
        let (intent_sender, intent_receiver) = tokio::sync::mpsc::unbounded_channel();
        let initial_snapshot =
            ResetWorkflowSnapshot::from_workflow(&ResetWorkflow::default(), None, None);
        let (snapshot_sender, snapshot_receiver) = watch::channel(initial_snapshot);
        (
            Self {
                service: Arc::new(service),
                redeem_request_id_factory,
                clock,
                workflow: ResetWorkflow::default(),
                intent_receiver,
                snapshot_sender,
                tasks: JoinSet::new(),
                inspection_authority_read: None,
                revalidation_authority_read: None,
                generations: GenerationAllocator::default(),
                inspection_authority: None,
                confirmation_authority: None,
                inspection_usage: None,
                inspection_inventory: None,
                revalidation_context: None,
                revalidation_usage: None,
                revalidation_inventory: None,
                current_target: None,
                invalidation_reason: None,
                terminal_outcome: None,
                presentation_connected: true,
            },
            ResetSessionPorts {
                intent_sender: ResetIntentSender::new(intent_sender),
                snapshot_receiver,
            },
        )
    }

    pub(in crate::quota_reset) async fn run(mut self) -> ResetSessionOutcome {
        loop {
            tokio::select! {
                biased;
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
                authority = async {
                    match self.inspection_authority_read.as_mut() {
                        Some(pending) => (&mut pending.operation).await,
                        None => std::future::pending::<Result<
                            TAuthorityReader::Authority,
                            RenderSafeFailure,
                        >>()
                        .await,
                    }
                }, if self.inspection_authority_read.is_some() => {
                    self.finish_inspection_authority_read(authority);
                    self.publish_snapshot();
                }
                authority = async {
                    match self.revalidation_authority_read.as_mut() {
                        Some(pending) => (&mut pending.operation).await,
                        None => std::future::pending::<Result<
                            TAuthorityReader::Authority,
                            RenderSafeFailure,
                        >>()
                        .await,
                    }
                }, if self.revalidation_authority_read.is_some() => {
                    self.finish_revalidation_authority_read(authority);
                    self.publish_snapshot();
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
                        let prepared = service
                            .prepare_authority_read(
                                start.live_usage_correlation().account_id(),
                                start
                                    .live_usage_correlation()
                                    .active_credential_generation(),
                                now_unix_seconds,
                            )
                            .await;
                        SessionTaskOutput::InspectionAuthorityPrepared {
                            start,
                            now_unix_seconds,
                            prepared,
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
            ResetSessionIntent::Confirm { .. } => {
                self.begin_revalidation();
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

    async fn apply_task_completion(
        &mut self,
        completed: Result<SessionTaskOutput<TAuthorityReader>, tokio::task::JoinError>,
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
            SessionTaskOutput::InspectionAuthorityPrepared {
                start,
                now_unix_seconds,
                prepared,
            } => {
                self.start_inspection_authority_read(start, now_unix_seconds, prepared);
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
            SessionTaskOutput::RevalidationAuthorityPrepared {
                usage_correlation,
                inventory_correlation,
                now_unix_seconds,
                confirmation,
                prepared,
            } => self.start_revalidation_authority_read(
                usage_correlation,
                inventory_correlation,
                now_unix_seconds,
                confirmation,
                prepared,
            ),
            SessionTaskOutput::RevalidationUsageCompleted {
                correlation,
                terminal,
            } => {
                if self.workflow.phase() != WorkflowPhase::Revalidating
                    || !self.correlation_is_current(&correlation)
                {
                    return None;
                }
                self.workflow.reduce(WorkflowIntent::OperationCompleted(
                    CorrelatedOutcome::revalidation_live_usage(
                        correlation.clone(),
                        terminal.clone(),
                    ),
                ));
                self.revalidation_usage = Some((correlation, terminal));
                self.finish_revalidation_if_ready();
            }
            SessionTaskOutput::RevalidationInventoryCompleted {
                correlation,
                terminal,
            } => {
                if self.workflow.phase() != WorkflowPhase::Revalidating
                    || !self.correlation_is_current(&correlation)
                {
                    return None;
                }
                self.workflow.reduce(WorkflowIntent::OperationCompleted(
                    CorrelatedOutcome::revalidation_credit_inventory(
                        correlation.clone(),
                        terminal.clone(),
                    ),
                ));
                self.revalidation_inventory = Some((correlation, terminal));
                self.finish_revalidation_if_ready();
            }
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

    async fn cancel_precommit(&mut self) {
        self.workflow.reduce(WorkflowIntent::Cancel);
        self.generations.allocate_attempt();
        self.tasks.abort_all();
        self.reap_all_tasks().await;
        self.inspection_authority = None;
        self.confirmation_authority = None;
        self.inspection_usage = None;
        self.inspection_inventory = None;
        self.revalidation_context = None;
        self.revalidation_usage = None;
        self.revalidation_inventory = None;
        self.invalidation_reason = None;
        self.terminal_outcome = None;
        self.publish_snapshot();
    }

    async fn invalidate_pinned_target(&mut self, reason: PinnedTargetInvalidationReason) {
        let failure = match reason {
            PinnedTargetInvalidationReason::AccountRemoved => RenderSafeFailure::AccountUnavailable,
            PinnedTargetInvalidationReason::AccountDisabled => {
                RenderSafeFailure::AccountUnavailable
            }
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
        self.revalidation_context = None;
        self.revalidation_usage = None;
        self.revalidation_inventory = None;
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
        if let Some(mut pending) = self.inspection_authority_read.take() {
            let _ = (&mut pending.operation).await;
        }
        if let Some(mut pending) = self.revalidation_authority_read.take() {
            let _ = (&mut pending.operation).await;
        }
    }
}

#[cfg(test)]
mod tests;
