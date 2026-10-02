//! Claude Code peer reachability and write-only message delivery.
use crate::live_session_ownership_check::{LiveSessionOwnership, LiveSessionOwnershipCheck};
use agent_automation::{
    ClaudeCodePeerEffectEvidence, PeerSessionReference, PeerWriteEffect, RouteEffectEvidence,
};
use claude_code_peer_messaging::{
    ClaudeCodePeerSocket, ClaudeCodeSessionRegistry, PeerSessionLookup, PeerSessionRecord,
    PeerSocketWriteOutcome,
};
use collaboration_protocol::{
    CodexGeneration, DeliveryClientReceipt, DeliveryNextAction, DeliveryOutcome, DeliveryPeerClaim,
    DeliveryReceipt, DeliveryRejection, DeliveryRejectionReason, EndpointRef, MessageDelivery,
    SessionReachability, SessionRef,
};
use collaboration_service::{
    AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext,
    DeliveryContractError, DeliveryFuture, DeliveryPrecondition, RouteClaim, RoutePresence,
    SessionDeliveryRoute,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct ClaudeCodePeerDeliveryRoute {
    endpoint: EndpointRef,
    registry: Arc<ClaudeCodeSessionRegistry>,
    socket: Arc<ClaudeCodePeerSocket>,
}

enum PeerDeliveryPreparation {
    Ready(PeerSessionRecord),
    Finished(DeliveryReceipt),
}

const AMBIGUOUS_PEER_CLAIM_REASON: &str =
    "multiple live Claude Code registry records claim this session";

impl ClaudeCodePeerDeliveryRoute {
    #[must_use]
    pub fn new(
        endpoint: EndpointRef,
        registry: Arc<ClaudeCodeSessionRegistry>,
        socket: Arc<ClaudeCodePeerSocket>,
    ) -> Self {
        Self {
            endpoint,
            registry,
            socket,
        }
    }

    pub(crate) fn serves(&self, target: &SessionRef) -> bool {
        target.endpoint == self.endpoint
    }

    pub(crate) async fn lookup(&self, target: &SessionRef) -> PeerSessionLookup {
        let registry = Arc::clone(&self.registry);
        let session_id = target.session_id.clone();
        match tokio::task::spawn_blocking(move || registry.lookup(&session_id)).await {
            Ok(Ok(lookup)) => lookup,
            Ok(Err(error)) => PeerSessionLookup::LiveUnsupported {
                reason: error.to_string(),
            },
            Err(error) => PeerSessionLookup::LiveUnsupported {
                reason: format!("Claude Code registry lookup task failed: {error}"),
            },
        }
    }

    pub(crate) fn evidence(
        peer: &PeerSessionRecord,
        write: PeerWriteEffect,
    ) -> Result<RouteEffectEvidence<SessionRef, CodexGeneration>, DeliveryContractError> {
        let session_id = PeerSessionReference::try_from(String::from(peer.session_id.clone()))
            .map_err(|_| DeliveryContractError::InvalidEvidence)?;
        Ok(RouteEffectEvidence::ClaudeCodePeer(
            ClaudeCodePeerEffectEvidence {
                session_id,
                process_id: peer.process_id,
                write,
            },
        ))
    }

    pub(crate) async fn write_peer_message(
        &self,
        peer: &PeerSessionRecord,
        prepared_line: &str,
    ) -> PeerSocketWriteOutcome {
        self.socket.write_user_message(peer, prepared_line).await
    }

    async fn prepare_peer_delivery(
        &self,
        target: &SessionRef,
        precondition: &DeliveryPrecondition,
        mode: MessageDelivery,
    ) -> Result<PeerDeliveryPreparation, DeliveryContractError> {
        if !self.serves(target) {
            return Ok(PeerDeliveryPreparation::Finished(peer_rejection(
                DeliveryRejectionReason::NoRoute,
                "Claude Code peer route does not serve this endpoint",
            )));
        }
        if matches!(
            precondition,
            DeliveryPrecondition::EndpointGeneration { .. }
        ) {
            return Ok(PeerDeliveryPreparation::Finished(peer_rejection(
                DeliveryRejectionReason::StaleGeneration,
                "Claude Code peer sessions have no endpoint generation",
            )));
        }
        if mode == MessageDelivery::Queue {
            return Ok(PeerDeliveryPreparation::Finished(peer_rejection(
                DeliveryRejectionReason::QueueUnsupported,
                "Queue delivery isn't supported for Claude Code terminals",
            )));
        }
        Ok(match self.lookup(target).await {
            PeerSessionLookup::Absent => PeerDeliveryPreparation::Finished(peer_receipt(
                DeliveryOutcome::NotSubmitted {
                    retryable: true,
                    reason: "Claude Code session is no longer live".to_owned(),
                },
                None,
            )),
            PeerSessionLookup::LiveUnsupported { reason } => PeerDeliveryPreparation::Finished(
                peer_rejection(DeliveryRejectionReason::LiveElsewhere, &reason),
            ),
            PeerSessionLookup::Ambiguous { claims } => {
                PeerDeliveryPreparation::Finished(peer_receipt(
                    DeliveryOutcome::Rejected(peer_claim_rejection(claims)),
                    None,
                ))
            }
            PeerSessionLookup::Writable(peer) => PeerDeliveryPreparation::Ready(peer),
        })
    }

    async fn write_peer_delivery(
        &self,
        peer: &PeerSessionRecord,
        text: &str,
        evidence: &dyn AttemptEvidenceSink,
    ) -> Result<DeliveryReceipt, DeliveryContractError> {
        evidence
            .record(Self::evidence(peer, PeerWriteEffect::Dispatching)?)
            .await?;
        match self.socket.write_user_message(peer, text).await {
            PeerSocketWriteOutcome::Written => {
                if evidence
                    .record(Self::evidence(peer, PeerWriteEffect::Written)?)
                    .await
                    .is_err()
                {
                    return Ok(peer_receipt(DeliveryOutcome::Unknown, None));
                }
                Ok(peer_receipt(
                    DeliveryOutcome::PeerMessageWritten,
                    Some(DeliveryClientReceipt::ClaudeCodePeer),
                ))
            }
            PeerSocketWriteOutcome::NotSubmitted { reason } => {
                if evidence
                    .record(Self::evidence(peer, PeerWriteEffect::NotDispatched)?)
                    .await
                    .is_err()
                {
                    return Ok(peer_receipt(DeliveryOutcome::Unknown, None));
                }
                Ok(peer_receipt(
                    DeliveryOutcome::NotSubmitted {
                        retryable: true,
                        reason: reason.to_owned(),
                    },
                    None,
                ))
            }
            PeerSocketWriteOutcome::Unknown { .. } => {
                if evidence
                    .record(Self::evidence(peer, PeerWriteEffect::Unknown)?)
                    .await
                    .is_err()
                {
                    return Ok(peer_receipt(DeliveryOutcome::Unknown, None));
                }
                Ok(peer_receipt(DeliveryOutcome::Unknown, None))
            }
        }
    }
}

fn peer_receipt(
    outcome: DeliveryOutcome,
    client: Option<DeliveryClientReceipt>,
) -> DeliveryReceipt {
    DeliveryReceipt {
        outcome,
        reachability: Some(SessionReachability::ClaudeCodePeer),
        client,
    }
}

fn peer_rejection(reason: DeliveryRejectionReason, detail: &str) -> DeliveryReceipt {
    let next_action = match reason {
        DeliveryRejectionReason::LiveElsewhere => DeliveryNextAction::InspectTarget,
        _ => DeliveryNextAction::CorrectRequest,
    };
    peer_receipt(
        DeliveryOutcome::Rejected(DeliveryRejection {
            reason,
            next_action,
            client_code: None,
            detail: Some(detail.to_owned()),
            claims: None,
        }),
        None,
    )
}

fn peer_claim_rejection(claims: Vec<claude_code_peer_messaging::PeerClaim>) -> DeliveryRejection {
    let claim_count = claims.len();
    let claims = claims
        .into_iter()
        .map(|claim| DeliveryPeerClaim {
            pid: claim.pid,
            name: claim.name,
            cwd: claim.cwd.map(|cwd| cwd.to_string_lossy().into_owned()),
        })
        .collect();
    DeliveryRejection {
        reason: DeliveryRejectionReason::LiveElsewhere,
        next_action: DeliveryNextAction::InspectTarget,
        client_code: None,
        detail: Some(format!(
            "this Claude session is claimed by {claim_count} live terminals"
        )),
        claims: Some(claims),
    }
}

impl LiveSessionOwnershipCheck for ClaudeCodePeerDeliveryRoute {
    fn check<'a>(&'a self, target: &'a SessionRef) -> DeliveryFuture<'a, LiveSessionOwnership> {
        Box::pin(async move {
            if !self.serves(target) {
                return Ok(LiveSessionOwnership::NotLive);
            }
            Ok(match self.lookup(target).await {
                PeerSessionLookup::Absent => LiveSessionOwnership::NotLive,
                PeerSessionLookup::Writable(_) => LiveSessionOwnership::LiveWritable,
                PeerSessionLookup::LiveUnsupported { .. } | PeerSessionLookup::Ambiguous { .. } => {
                    LiveSessionOwnership::LiveUnsupported
                }
            })
        })
    }
}

impl SessionDeliveryRoute for ClaudeCodePeerDeliveryRoute {
    fn scheduled_runs(&self) -> Option<Arc<dyn collaboration_service::ScheduledRunRoute>> {
        Some(Arc::new(self.clone()))
    }
    fn reachability(&self) -> SessionReachability {
        SessionReachability::ClaudeCodePeer
    }

    fn supports_delivery_mode(&self, target: &SessionRef, mode: MessageDelivery) -> bool {
        !self.serves(target) || mode != MessageDelivery::Queue
    }

    fn claim(&self, target: &SessionRef) -> DeliveryFuture<'_, RouteClaim> {
        let target = target.clone();
        Box::pin(async move {
            if !self.serves(&target) {
                return Ok(RouteClaim::NotMine);
            }
            Ok(match self.lookup(&target).await {
                PeerSessionLookup::Absent => RouteClaim::NotMine,
                PeerSessionLookup::Writable(_) => RouteClaim::Holds,
                PeerSessionLookup::LiveUnsupported { reason } => RouteClaim::LiveElsewhere {
                    writable: false,
                    detail: Some(reason),
                },
                PeerSessionLookup::Ambiguous { claims } => RouteClaim::Rejected {
                    rejection: peer_claim_rejection(claims),
                },
            })
        })
    }

    fn presence(&self, target: &SessionRef) -> DeliveryFuture<'_, RoutePresence> {
        let target = target.clone();
        Box::pin(async move {
            if !self.serves(&target) {
                return Ok(RoutePresence::NotMine);
            }
            Ok(match self.lookup(&target).await {
                PeerSessionLookup::Absent => RoutePresence::NotMine,
                PeerSessionLookup::Writable(_) => RoutePresence::Running,
                PeerSessionLookup::LiveUnsupported { reason } => RoutePresence::LiveElsewhere {
                    detail: Some(reason),
                },
                PeerSessionLookup::Ambiguous { .. } => RoutePresence::LiveElsewhere {
                    detail: Some(AMBIGUOUS_PEER_CLAIM_REASON.to_owned()),
                },
            })
        })
    }

    fn deliver<'a>(
        &'a self,
        request: collaboration_service::layer_zero::DeliveryRequest,
        evidence: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, DeliveryReceipt> {
        Box::pin(async move {
            if request.correlation.as_str() != request.payload.push_id.as_str() {
                return Err(DeliveryContractError::InvalidEvidence);
            }
            let peer = match self
                .prepare_peer_delivery(&request.target, &request.precondition, request.mode)
                .await?
            {
                PeerDeliveryPreparation::Ready(peer) => peer,
                PeerDeliveryPreparation::Finished(receipt) => return Ok(receipt),
            };
            self.write_peer_delivery(&peer, request.payload.line.as_str(), evidence)
                .await
        })
    }

    fn reconcile_attempt(
        &self,
        context: AttemptReconciliationContext,
    ) -> DeliveryFuture<'_, AttemptReconciliation> {
        Box::pin(async move {
            let RouteEffectEvidence::ClaudeCodePeer(peer) = context.recorded else {
                return Err(DeliveryContractError::InvalidEvidence);
            };
            if !self.serves(&context.target)
                || peer.session_id.as_str() != String::from(context.target.session_id).as_str()
            {
                return Err(DeliveryContractError::InvalidEvidence);
            }
            Ok(match peer.write {
                PeerWriteEffect::NotDispatched => AttemptReconciliation::KnownNotSubmitted,
                PeerWriteEffect::Written => {
                    AttemptReconciliation::Accepted(Box::new(peer_receipt(
                        DeliveryOutcome::PeerMessageWritten,
                        Some(DeliveryClientReceipt::ClaudeCodePeer),
                    )))
                }
                PeerWriteEffect::Dispatching | PeerWriteEffect::Unknown => {
                    AttemptReconciliation::StillUnknown
                }
            })
        })
    }
}
