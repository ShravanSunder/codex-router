//! Claude Code peer reachability and write-only message delivery.
use crate::live_session_ownership_check::{LiveSessionOwnership, LiveSessionOwnershipCheck};
use agent_automation::{
    ClaudeCodePeerEffectEvidence, PeerSessionReference, PeerWriteEffect, RouteEffectEvidence,
};
use claude_code_peer_messaging::{
    ClaudeCodePeerSocket, ClaudeCodeSessionRegistry, PeerSessionLookup, PeerSessionRecord,
    PeerSessionStatus, PeerSocketWriteOutcome,
};
use collaboration_protocol::{
    CodexGeneration, DeliveryClientReceipt, DeliveryNextAction, DeliveryOutcome, DeliveryReceipt,
    DeliveryRejection, DeliveryRejectionReason, MessageContent, MessageDelivery,
    SessionReachability, SessionRef, UuidIdentity, render_message,
};
use collaboration_service::{
    AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext,
    DeliveryContractError, DeliveryFuture, DeliveryPrecondition, DeliveryRequest, RouteClaim,
    SessionDeliveryRoute,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct ClaudeCodePeerDeliveryRoute {
    service_id: UuidIdentity,
    registry: Arc<ClaudeCodeSessionRegistry>,
    socket: Arc<ClaudeCodePeerSocket>,
}

impl ClaudeCodePeerDeliveryRoute {
    #[must_use]
    pub fn new(
        service_id: UuidIdentity,
        registry: Arc<ClaudeCodeSessionRegistry>,
        socket: Arc<ClaudeCodePeerSocket>,
    ) -> Self {
        Self {
            service_id,
            registry,
            socket,
        }
    }

    pub(crate) fn serves(&self, target: &SessionRef) -> bool {
        target.endpoint.service_id == self.service_id
            && String::from(target.endpoint.endpoint_id.clone()) == "claude-local"
    }

    pub(crate) async fn lookup(&self, target: &SessionRef) -> PeerSessionLookup {
        let registry = Arc::clone(&self.registry);
        let session_id = target.session_id.clone();
        match tokio::task::spawn_blocking(move || registry.lookup(&session_id)).await {
            Ok(Ok(lookup)) => lookup,
            Ok(Err(_)) | Err(_) => PeerSessionLookup::LiveUnsupported {
                reason: "Claude Code live-session registry is unreadable".to_owned(),
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

    pub(crate) fn render_peer_message(
        target: &SessionRef,
        message: &MessageContent,
    ) -> Result<String, DeliveryContractError> {
        let rendered =
            render_message(target, message).map_err(|_| DeliveryContractError::ClientOperation)?;
        match message {
            MessageContent::Agent { sender, .. } => {
                let sender = serde_json::to_string(sender)
                    .map_err(|_| DeliveryContractError::ClientOperation)?;
                Ok(format!(
                    "{}\n\nReply to {sender} through Router's message_send as this Claude session.",
                    rendered.text,
                ))
            }
            MessageContent::HumanUser { .. } => Ok(format!(
                "Origin: human user\n\n{}\n\nFor replies, use Router's message_send as this Claude session.",
                rendered.text,
            )),
            MessageContent::Router { .. } => Ok(format!(
                "{}\n\nFor replies, use Router's message_send as this Claude session.",
                rendered.text,
            )),
        }
    }

    pub(crate) async fn write_peer_message(
        &self,
        peer: &PeerSessionRecord,
        message: &str,
    ) -> PeerSocketWriteOutcome {
        self.socket.write_user_message(peer, message).await
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
        }),
        None,
    )
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
                PeerSessionLookup::LiveUnsupported { .. } => LiveSessionOwnership::LiveUnsupported,
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

    fn claim(&self, target: &SessionRef) -> DeliveryFuture<'_, RouteClaim> {
        let target = target.clone();
        Box::pin(async move {
            if !self.serves(&target) {
                return Ok(RouteClaim::NotMine);
            }
            Ok(match self.lookup(&target).await {
                PeerSessionLookup::Absent => RouteClaim::NotMine,
                PeerSessionLookup::Writable(_) => RouteClaim::Holds,
                PeerSessionLookup::LiveUnsupported { .. } => {
                    RouteClaim::LiveElsewhere { writable: false }
                }
            })
        })
    }

    fn deliver<'a>(
        &'a self,
        request: DeliveryRequest,
        evidence: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, DeliveryReceipt> {
        Box::pin(async move {
            if !self.serves(&request.target) {
                return Ok(peer_rejection(
                    DeliveryRejectionReason::NoRoute,
                    "Claude Code peer route does not serve this endpoint",
                ));
            }
            if matches!(
                request.precondition,
                DeliveryPrecondition::EndpointGeneration { .. }
            ) {
                return Ok(peer_rejection(
                    DeliveryRejectionReason::StaleGeneration,
                    "Claude Code peer sessions have no endpoint generation",
                ));
            }
            if request.mode == MessageDelivery::Queue {
                return Ok(peer_rejection(
                    DeliveryRejectionReason::QueueUnsupported,
                    "queue unsupported for Claude Code sessions",
                ));
            }
            let peer = match self.lookup(&request.target).await {
                PeerSessionLookup::Absent => {
                    return Ok(peer_receipt(
                        DeliveryOutcome::NotSubmitted {
                            retryable: true,
                            reason: "Claude Code session is no longer live".to_owned(),
                        },
                        None,
                    ));
                }
                PeerSessionLookup::LiveUnsupported { reason } => {
                    return Ok(peer_rejection(
                        DeliveryRejectionReason::LiveElsewhere,
                        &reason,
                    ));
                }
                PeerSessionLookup::Writable(peer) => peer,
            };
            if request.mode == MessageDelivery::Steer && peer.status != PeerSessionStatus::Busy {
                return Ok(peer_receipt(
                    DeliveryOutcome::NotSubmitted {
                        retryable: false,
                        reason: "no running turn".to_owned(),
                    },
                    None,
                ));
            }
            let text = Self::render_peer_message(&request.target, &request.message)?;
            evidence
                .record(Self::evidence(&peer, PeerWriteEffect::Dispatching)?)
                .await?;
            let outcome = self.socket.write_user_message(&peer, &text).await;
            match outcome {
                PeerSocketWriteOutcome::Written => {
                    if evidence
                        .record(Self::evidence(&peer, PeerWriteEffect::Written)?)
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
                        .record(Self::evidence(&peer, PeerWriteEffect::NotDispatched)?)
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
                        .record(Self::evidence(&peer, PeerWriteEffect::Unknown)?)
                        .await
                        .is_err()
                    {
                        return Ok(peer_receipt(DeliveryOutcome::Unknown, None));
                    }
                    Ok(peer_receipt(DeliveryOutcome::Unknown, None))
                }
            }
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
