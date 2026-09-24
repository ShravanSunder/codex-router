//! Write-only scheduled runs to a live Claude Code peer.
use crate::claude_code_peer_delivery_route::ClaudeCodePeerDeliveryRoute;
use agent_automation::{PeerWriteEffect, RouteEffectEvidence};
use claude_code_peer_messaging::{PeerSessionLookup, PeerSocketWriteOutcome};
use collaboration_protocol::{
    CodexGeneration, DeliveryClientReceipt, DeliveryNextAction, DeliveryOutcome, DeliveryReceipt,
    DeliveryRejection, DeliveryRejectionReason, MessageContent, RunExecution, ScheduleFailureKind,
    SessionRef,
};
use collaboration_service::{
    DeliveryContractError, DeliveryFuture, DeliveryPrecondition, FreshSessionRequest,
    PreparationEvidenceSink, PreparedTarget, RunAcceptance, RunEvidenceDisposition,
    RunEvidenceSink, RunObservationContext, RunReconciliation, RunSettlement, RunSubmission,
    RunSummarySource, ScheduleCapability, ScheduleDestination, SchedulePreparationFailure,
    SchedulePreparationOutcome, SchedulePreparationRequest, ScheduleSupport, ScheduledRunExecution,
    ScheduledRunRoute, ScheduledRunSubmission, SettlementEvidence, StopRequestOutcome,
};

impl ClaudeCodePeerDeliveryRoute {
    async fn peer_run_evidence(
        &self,
        target: &SessionRef,
    ) -> Result<RouteEffectEvidence<SessionRef, CodexGeneration>, DeliveryContractError> {
        if !self.serves(target) {
            return Err(DeliveryContractError::ClientOperation);
        }
        let PeerSessionLookup::Writable(peer) = self.lookup(target).await else {
            return Err(DeliveryContractError::ClientOperation);
        };
        Self::evidence(&peer, PeerWriteEffect::NotDispatched)
    }
}

impl ScheduledRunRoute for ClaudeCodePeerDeliveryRoute {
    fn supports_endpoint(&self, _endpoint: &collaboration_protocol::EndpointRef) -> bool {
        // A peer can receive an existing target, but cannot create a fresh session.
        false
    }
}

impl ScheduledRunExecution for ClaudeCodePeerDeliveryRoute {
    fn support(&self, destination: &ScheduleDestination) -> DeliveryFuture<'_, ScheduleSupport> {
        let destination = destination.clone();
        Box::pin(async move {
            let missing = match destination {
                ScheduleDestination::Existing { target }
                    if self.peer_run_evidence(&target).await.is_ok() =>
                {
                    return Ok(ScheduleSupport::Supported {
                        settlement: SettlementEvidence::WriteOnly,
                    });
                }
                ScheduleDestination::Existing { .. } => {
                    vec![ScheduleCapability::ReadTarget, ScheduleCapability::StartRun]
                }
                ScheduleDestination::Fresh { .. } => vec![ScheduleCapability::CreateSession],
                ScheduleDestination::Fork { .. } => vec![ScheduleCapability::ForkSession],
            };
            Ok(ScheduleSupport::Unsupported { missing })
        })
    }

    fn initial_evidence(
        &self,
        destination: &ScheduleDestination,
    ) -> DeliveryFuture<'_, RouteEffectEvidence<SessionRef, CodexGeneration>> {
        let destination = destination.clone();
        Box::pin(async move {
            match destination {
                ScheduleDestination::Existing { target } => self.peer_run_evidence(&target).await,
                ScheduleDestination::Fork { source, .. } => self.peer_run_evidence(&source).await,
                ScheduleDestination::Fresh { .. } => Err(DeliveryContractError::ClientOperation),
            }
        })
    }

    fn prepare_destination<'a>(
        &'a self,
        request: SchedulePreparationRequest,
        sink: &'a dyn PreparationEvidenceSink,
    ) -> DeliveryFuture<'a, SchedulePreparationOutcome> {
        Box::pin(async move {
            let target = match request.destination {
                collaboration_protocol::DestinationPreparation::Existing { target, .. } => target,
                collaboration_protocol::DestinationPreparation::Fork { source, .. } => {
                    let evidence = self.peer_run_evidence(&source).await?;
                    return Ok(SchedulePreparationOutcome::Failed(
                        SchedulePreparationFailure {
                            kind: ScheduleFailureKind::UnsupportedCapability,
                            explanation: "Claude Code peer sessions cannot be forked".into(),
                            evidence,
                            uncertain: false,
                        },
                    ));
                }
                collaboration_protocol::DestinationPreparation::Fresh { .. } => {
                    return Err(DeliveryContractError::ClientOperation);
                }
            };
            let evidence = self.peer_run_evidence(&target).await?;
            match sink.record_intent(evidence.clone()).await {
                Ok(true) => Ok(SchedulePreparationOutcome::Prepared(PreparedTarget {
                    target,
                    evidence,
                })),
                Ok(false) => Ok(SchedulePreparationOutcome::Failed(
                    SchedulePreparationFailure {
                        kind: ScheduleFailureKind::OutcomeUnknown,
                        explanation: "Peer preparation intent could not be established".into(),
                        evidence,
                        uncertain: true,
                    },
                )),
                Err(DeliveryContractError::ClientOperation) => Ok(
                    SchedulePreparationOutcome::Failed(SchedulePreparationFailure {
                        kind: ScheduleFailureKind::AutomationUnavailable,
                        explanation: "Schedule configuration is unavailable".into(),
                        evidence,
                        uncertain: false,
                    }),
                ),
                Err(error) => Err(error),
            }
        })
    }

    fn prepare_existing_target<'a>(
        &'a self,
        target: &SessionRef,
        sink: &'a dyn RunEvidenceSink,
    ) -> DeliveryFuture<'a, PreparedTarget> {
        let target = target.clone();
        Box::pin(async move {
            let evidence = self.peer_run_evidence(&target).await?;
            if matches!(
                sink.record(evidence.clone()).await?,
                RunEvidenceDisposition::AdmissionRefused
            ) {
                return Err(DeliveryContractError::ClientOperation);
            }
            Ok(PreparedTarget { target, evidence })
        })
    }

    fn prepare_fresh_session<'a>(
        &'a self,
        _request: FreshSessionRequest,
        _sink: &'a dyn RunEvidenceSink,
    ) -> DeliveryFuture<'a, PreparedTarget> {
        Box::pin(async { Err(DeliveryContractError::ClientOperation) })
    }

    fn submit_run<'a>(
        &'a self,
        run: ScheduledRunSubmission,
        sink: &'a dyn RunEvidenceSink,
    ) -> DeliveryFuture<'a, RunSubmission> {
        Box::pin(async move {
            let RouteEffectEvidence::ClaudeCodePeer(recorded) = &run.recorded else {
                return Err(DeliveryContractError::InvalidEvidence);
            };
            if recorded.session_id.as_str() != String::from(run.target.session_id.clone()) {
                return Err(DeliveryContractError::InvalidEvidence);
            }
            if !matches!(run.precondition, DeliveryPrecondition::Unpinned) {
                return Ok(rejected(
                    DeliveryRejectionReason::StaleGeneration,
                    DeliveryNextAction::InspectTarget,
                    "Claude Code peer sessions have no provider generation",
                ));
            }
            let peer = match self.lookup(&run.target).await {
                PeerSessionLookup::Writable(peer) if peer.process_id == recorded.process_id => peer,
                PeerSessionLookup::LiveUnsupported { .. } => {
                    return Ok(rejected(
                        DeliveryRejectionReason::LiveElsewhere,
                        DeliveryNextAction::InspectTarget,
                        "Claude Code session is live but not writable",
                    ));
                }
                PeerSessionLookup::Absent | PeerSessionLookup::Writable(_) => {
                    return Ok(rejected(
                        DeliveryRejectionReason::EndpointUnavailable,
                        DeliveryNextAction::RetryLater,
                        "Claude Code peer session is no longer writable",
                    ));
                }
            };
            let rendered = Self::render_peer_message(
                &run.target,
                &MessageContent::Router { text: run.message },
            )?;
            let dispatch = Self::evidence(&peer, PeerWriteEffect::Dispatching)?;
            if matches!(
                sink.record(dispatch).await?,
                RunEvidenceDisposition::AdmissionRefused
            ) {
                return Ok(RunSubmission::NotStartedBusy);
            }
            let outcome = self.write_peer_message(&peer, &rendered).await;
            let (write, result) = match outcome {
                PeerSocketWriteOutcome::Written => {
                    let written_at = chrono::Utc::now()
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                        .try_into()
                        .map_err(|_| DeliveryContractError::InvalidEvidence)?;
                    (
                        PeerWriteEffect::Written,
                        RunSubmission::Started(Box::new(RunAcceptance {
                            execution: RunExecution::ClaudeCodePeer {
                                target: run.target,
                                written_at,
                            },
                            receipt: DeliveryReceipt {
                                outcome: DeliveryOutcome::PeerMessageWritten,
                                reachability: Some(
                                    collaboration_protocol::SessionReachability::ClaudeCodePeer,
                                ),
                                client: Some(DeliveryClientReceipt::ClaudeCodePeer),
                            },
                        })),
                    )
                }
                PeerSocketWriteOutcome::NotSubmitted { reason } => (
                    PeerWriteEffect::NotDispatched,
                    rejected(
                        DeliveryRejectionReason::EndpointUnavailable,
                        DeliveryNextAction::RetryLater,
                        reason,
                    ),
                ),
                PeerSocketWriteOutcome::Unknown { .. } => {
                    (PeerWriteEffect::Unknown, RunSubmission::Unknown)
                }
            };
            if !matches!(
                sink.record(Self::evidence(&peer, write)?).await,
                Ok(RunEvidenceDisposition::Recorded { .. })
            ) {
                return Ok(RunSubmission::Unknown);
            }
            Ok(result)
        })
    }

    fn observe_settlement(
        &self,
        context: RunObservationContext,
    ) -> DeliveryFuture<'_, RunSettlement> {
        Box::pin(async move {
            match context.recorded {
                RouteEffectEvidence::ClaudeCodePeer(peer)
                    if peer.write == PeerWriteEffect::Written =>
                {
                    Ok(RunSettlement::WrittenWithoutCompletion)
                }
                RouteEffectEvidence::ClaudeCodePeer(_) => Ok(RunSettlement::Pending),
                _ => Err(DeliveryContractError::InvalidEvidence),
            }
        })
    }

    fn summary_source(
        &self,
        _context: RunObservationContext,
    ) -> DeliveryFuture<'_, RunSummarySource> {
        Box::pin(async { Err(DeliveryContractError::InvalidEvidence) })
    }

    fn request_stop<'a>(
        &'a self,
        _context: RunObservationContext,
        _sink: &'a dyn RunEvidenceSink,
    ) -> DeliveryFuture<'a, StopRequestOutcome> {
        Box::pin(async { Ok(StopRequestOutcome::Unsupported) })
    }

    fn reconcile_run(
        &self,
        context: RunObservationContext,
    ) -> DeliveryFuture<'_, RunReconciliation> {
        Box::pin(async move {
            match context.recorded {
                RouteEffectEvidence::ClaudeCodePeer(peer) => Ok(match peer.write {
                    PeerWriteEffect::Written => RunReconciliation::Settled {
                        settlement: RunSettlement::WrittenWithoutCompletion,
                    },
                    PeerWriteEffect::NotDispatched => RunReconciliation::KnownNotSubmitted,
                    PeerWriteEffect::Dispatching | PeerWriteEffect::Unknown => {
                        RunReconciliation::StillUnknown
                    }
                }),
                _ => Err(DeliveryContractError::InvalidEvidence),
            }
        })
    }
}

fn rejected(
    reason: DeliveryRejectionReason,
    next_action: DeliveryNextAction,
    detail: &str,
) -> RunSubmission {
    RunSubmission::Rejected(DeliveryRejection {
        reason,
        next_action,
        client_code: None,
        detail: Some(detail.to_owned()),
    })
}
