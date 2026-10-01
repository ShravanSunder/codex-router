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
    CodexGeneration, DeliveryClientReceipt, DeliveryNextAction, DeliveryOutcome, DeliveryReceipt,
    DeliveryRejection, DeliveryRejectionReason, EndpointRef, MessageContent, MessageDelivery,
    MessageHeaderContext, SessionReachability, SessionRef, render_message_with_context,
};
use collaboration_service::{
    AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext,
    DeliveryContractError, DeliveryFuture, DeliveryPrecondition, DeliveryRequest, RouteClaim,
    RoutePresence, SessionDeliveryRoute,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct ClaudeCodePeerDeliveryRoute {
    endpoint: EndpointRef,
    registry: Arc<ClaudeCodeSessionRegistry>,
    socket: Arc<ClaudeCodePeerSocket>,
    display_names: collaboration_service::SessionDisplayNameCache,
}

enum PeerDeliveryPreparation {
    Ready(PeerSessionRecord),
    Finished(DeliveryReceipt),
}

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
            display_names: collaboration_service::SessionDisplayNameCache::default(),
        }
    }

    pub(crate) fn with_display_names(
        mut self,
        display_names: collaboration_service::SessionDisplayNameCache,
    ) -> Self {
        self.display_names = display_names;
        self
    }

    pub(crate) fn serves(&self, target: &SessionRef) -> bool {
        target.endpoint == self.endpoint
    }

    pub(crate) async fn lookup(&self, target: &SessionRef) -> PeerSessionLookup {
        let registry = Arc::clone(&self.registry);
        let session_id = target.session_id.clone();
        let lookup = match tokio::task::spawn_blocking(move || registry.lookup(&session_id)).await {
            Ok(Ok(lookup)) => lookup,
            Ok(Err(error)) => PeerSessionLookup::LiveUnsupported {
                reason: error.to_string(),
            },
            Err(error) => PeerSessionLookup::LiveUnsupported {
                reason: format!("Claude Code registry lookup task failed: {error}"),
            },
        };
        if let PeerSessionLookup::Writable(peer) = &lookup {
            if let Some(name) = peer.name.as_deref() {
                self.display_names.remember(target.clone(), name);
            } else {
                self.display_names.forget(target.clone());
            }
        }
        lookup
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
        header_context: &MessageHeaderContext,
    ) -> Result<String, DeliveryContractError> {
        let rendered = render_message_with_context(target, message, header_context)
            .map_err(|_| DeliveryContractError::ClientOperation)?;
        match message {
            MessageContent::Agent { .. } => Ok(format!(
                "{}\n\nReply with `agent-collaboration message reply --text <TEXT>` as this Claude session.",
                rendered.text,
            )),
            MessageContent::HumanUser { .. } => Ok(format!(
                "Origin: human user\n\n{}\n\nFor follow-up messages, use `agent-collaboration message send --human-user --to <SessionRef> --text <TEXT>` as this Claude session.",
                rendered.text,
            )),
            MessageContent::Router { .. } => Ok(format!(
                "{}\n\nFor follow-up messages, use `agent-collaboration message send --to <SessionRef> --from <SessionRef> --text <TEXT>` as this Claude session.",
                rendered.text,
            )),
        }
    }

    pub(crate) fn header_context_for_delivery(
        &self,
        target: &SessionRef,
        message: &MessageContent,
        current: &MessageHeaderContext,
    ) -> MessageHeaderContext {
        let cached =
            MessageHeaderContext::resolve(target, message, &self.display_names, current.origin);
        MessageHeaderContext {
            sender_display_name: cached
                .sender_display_name
                .or_else(|| current.sender_display_name.clone()),
            recipient_display_name: cached
                .recipient_display_name
                .or_else(|| current.recipient_display_name.clone()),
            origin: current.origin,
        }
    }

    pub(crate) async fn write_peer_message(
        &self,
        peer: &PeerSessionRecord,
        message: &str,
    ) -> PeerSocketWriteOutcome {
        self.socket.write_user_message(peer, message).await
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
                "queue unsupported for Claude Code sessions",
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
        match self.write_peer_message(peer, text).await {
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
                PeerSessionLookup::LiveUnsupported { reason } => RouteClaim::LiveElsewhere {
                    writable: false,
                    detail: Some(reason),
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
            })
        })
    }

    fn deliver<'a>(
        &'a self,
        request: DeliveryRequest,
        evidence: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, DeliveryReceipt> {
        Box::pin(async move {
            let peer = match self
                .prepare_peer_delivery(&request.target, &request.precondition, request.mode)
                .await?
            {
                PeerDeliveryPreparation::Ready(peer) => peer,
                PeerDeliveryPreparation::Finished(receipt) => return Ok(receipt),
            };
            let header_context = self.header_context_for_delivery(
                &request.target,
                &request.message,
                &request.header_context,
            );
            let text =
                Self::render_peer_message(&request.target, &request.message, &header_context)?;
            self.write_peer_delivery(&peer, &text, evidence).await
        })
    }

    fn deliver_prepared<'a>(
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

#[cfg(test)]
mod peer_reply_guidance_tests {
    use super::ClaudeCodePeerDeliveryRoute;
    use collaboration_protocol::{
        EndpointId, EndpointRef, MessageContent, MessageHeaderContext, MessageText, SessionId,
        SessionRef, UuidIdentity,
    };

    fn target() -> SessionRef {
        SessionRef {
            endpoint: EndpointRef {
                service_id: UuidIdentity::try_from(
                    "018f47d2-24d5-7a68-b9ec-6f759c39458f".to_owned(),
                )
                .expect("service id"),
                endpoint_id: EndpointId::try_from("claude-local".to_owned()).expect("endpoint id"),
            },
            session_id: SessionId::try_from("peer-session".to_owned()).expect("session id"),
        }
    }

    #[test]
    fn reply_shortcut_is_suggested_only_for_agent_messages() {
        let target = target();
        let agent = MessageContent::Agent {
            sender: target.clone(),
            text: MessageText::try_from("Agent message".to_owned()).expect("message text"),
        };
        let human = MessageContent::HumanUser {
            text: MessageText::try_from("Human message".to_owned()).expect("message text"),
        };
        let router = MessageContent::Router {
            text: MessageText::try_from("Router notice".to_owned()).expect("message text"),
        };
        let context = MessageHeaderContext::default();

        let agent_text =
            ClaudeCodePeerDeliveryRoute::render_peer_message(&target, &agent, &context)
                .expect("Agent peer message");
        let human_text =
            ClaudeCodePeerDeliveryRoute::render_peer_message(&target, &human, &context)
                .expect("human peer message");
        let router_text =
            ClaudeCodePeerDeliveryRoute::render_peer_message(&target, &router, &context)
                .expect("Router peer message");

        assert!(agent_text.contains("message reply --text <TEXT>"));
        assert!(human_text.contains("message send --human-user"));
        assert!(!human_text.contains("message reply"));
        assert!(router_text.contains("message send --to <SessionRef>"));
        assert!(!router_text.contains("message reply"));
    }
}
