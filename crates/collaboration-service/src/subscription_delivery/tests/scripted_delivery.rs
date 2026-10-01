//! Scripted client effect behind the real Layer-0 router.
use super::*;

pub(super) struct PreparedRecordingRoute {
    pub(super) delivery: Arc<ScriptedDelivery>,
    pub(super) reachability: collaboration_protocol::SessionReachability,
}
impl crate::SessionDeliveryRoute for PreparedRecordingRoute {
    fn reachability(&self) -> collaboration_protocol::SessionReachability {
        self.reachability
    }
    fn claim(&self, _: &SessionRef) -> DeliveryFuture<'_, crate::RouteClaim> {
        Box::pin(async { Ok(crate::RouteClaim::Holds) })
    }
    fn presence(&self, _: &SessionRef) -> DeliveryFuture<'_, crate::RoutePresence> {
        Box::pin(async { Ok(crate::RoutePresence::Running) })
    }
    fn deliver<'a>(
        &'a self,
        _: DeliveryRequest,
        _: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, DeliveryReceipt> {
        Box::pin(async { Err(DeliveryContractError::InvalidEvidence) })
    }
    fn deliver_prepared<'a>(
        &'a self,
        request: crate::layer_zero::DeliveryRequest,
        sink: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, DeliveryReceipt> {
        self.delivery.deliver_prepared(request, sink)
    }
    fn reconcile_attempt(
        &self,
        context: AttemptReconciliationContext,
    ) -> DeliveryFuture<'_, AttemptReconciliation> {
        self.delivery.reconcile_attempt(context)
    }
}

pub(super) struct ScriptedDelivery {
    pub(super) requests: mpsc::UnboundedSender<crate::layer_zero::DeliveryRequest>,
    pub(super) completions: Mutex<mpsc::UnboundedReceiver<DeliveryOutcome>>,
    pub(super) reconciliations: mpsc::UnboundedSender<(
        AttemptReconciliationContext,
        oneshot::Sender<AttemptReconciliation>,
    )>,
}
impl SessionMessageDelivery for ScriptedDelivery {
    fn deliver<'a>(
        &'a self,
        _: DeliveryRequest,
        _: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, DeliveryReceipt> {
        Box::pin(async { Err(DeliveryContractError::InvalidEvidence) })
    }
    fn deliver_prepared<'a>(
        &'a self,
        request: crate::layer_zero::DeliveryRequest,
        sink: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, DeliveryReceipt> {
        Box::pin(async move {
            let target = request.target.clone();
            let attempt_id = request.attempt.clone();
            self.requests.send(request).unwrap();
            let outcome = self.completions.lock().await.recv().await.unwrap();
            let reachability = if outcome == DeliveryOutcome::Queued {
                sink.record(agent_automation::RouteEffectEvidence::ProviderAcp(
                    agent_automation::ProviderAcpEffectEvidence {
                        target,
                        generation: collaboration_protocol::CodexGeneration {
                            service_epoch: collaboration_protocol::UuidIdentity::try_from(
                                SERVICE_ID.to_owned(),
                            )
                            .unwrap(),
                            generation: collaboration_protocol::GenerationNumber::try_from(1)
                                .unwrap(),
                        },
                        binding: agent_automation::ProviderBindingReference::try_from(
                            "owner-queued-binding".to_owned(),
                        )
                        .unwrap(),
                        attempt_id,
                        submission: agent_automation::SubmissionEffect::RouterQueued,
                        settlement: agent_automation::ProviderSettlementEffect::NotObserved,
                    },
                ))
                .await
                .unwrap();
                collaboration_protocol::SessionReachability::ProviderAcp
            } else {
                collaboration_protocol::SessionReachability::CodexAppServer
            };
            Ok(DeliveryReceipt {
                outcome,
                reachability: Some(reachability),
                client: None,
            })
        })
    }
    fn reconcile_attempt(
        &self,
        context: AttemptReconciliationContext,
    ) -> DeliveryFuture<'_, AttemptReconciliation> {
        Box::pin(async move {
            let (reply, response) = oneshot::channel();
            self.reconciliations.send((context, reply)).unwrap();
            Ok(response.await.unwrap())
        })
    }
}
