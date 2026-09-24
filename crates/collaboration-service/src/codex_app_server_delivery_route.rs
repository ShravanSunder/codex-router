//! Codex app-server delivery owns native admission and its recorded effects.
use crate::{
    AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext,
    DeliveryClientReceipt, DeliveryContractError, DeliveryFuture, DeliveryPrecondition,
    DeliveryReceipt, DeliveryRequest, EndpointDirectory, NativeControlBackend, RouteClaim,
    RouteUnavailableReason, SessionDeliveryRoute,
};
use agent_automation::{
    CessationEvidence, NativeEffectEvidence, PreparationEffect, RouteEffectEvidence,
    SubmissionEffect,
};
use codex_acp_adapter::{HeldBindingCheckout, UnmaterializedBindingStore};
use collaboration_protocol::{
    AcceptedResumeEffect, CodexGeneration, DeliveryNextAction, DeliveryOutcome, DeliveryRejection,
    DeliveryRejectionReason, MessageDelivery, NativeSendAcceptance, NativeSendParams,
    SessionReachability, SessionRef, UuidIdentity,
};
use serde_json::{Value, json};

pub struct CodexAppServerDeliveryRoute {
    service_id: UuidIdentity,
    endpoints: EndpointDirectory,
    backend: NativeControlBackend,
    holder: std::sync::Arc<crate::UnmaterializedThreadHolder>,
}

impl CodexAppServerDeliveryRoute {
    #[must_use]
    pub fn new(
        service_id: UuidIdentity,
        endpoints: EndpointDirectory,
        backend: NativeControlBackend,
        holder: std::sync::Arc<crate::UnmaterializedThreadHolder>,
    ) -> Self {
        Self {
            service_id,
            endpoints,
            backend,
            holder,
        }
    }

    async fn deliver_native(
        &self,
        request: DeliveryRequest,
        sink: &dyn AttemptEvidenceSink,
    ) -> Result<DeliveryReceipt, DeliveryContractError> {
        let Ok(admission) = self.backend.gate.acquire() else {
            return Ok(not_submitted("Native backend unavailable", true));
        };
        if let DeliveryPrecondition::EndpointGeneration { expected } = &request.precondition
            && expected != admission.generation()
        {
            return Ok(not_submitted("staleGeneration", false));
        }
        let generation = admission.generation().clone();
        let mut effects = NativeEffectEvidence {
            target: Some(request.target.clone()),
            generation: Some(generation.clone()),
            client_user_message_id: Some(request.correlation.as_str().to_owned()),
            native_turn_id: None,
            native_submission_id: None,
            allocation: PreparationEffect::NotRequested,
            resume: if request.mode == MessageDelivery::Auto {
                PreparationEffect::Unknown
            } else {
                PreparationEffect::NotRequested
            },
            submission: SubmissionEffect::Dispatching,
            cessation: CessationEvidence::NotApplicable,
        };
        sink.record(RouteEffectEvidence::CodexAppServer(effects.clone()))
            .await
            .map_err(|_| DeliveryContractError::EvidencePersistence)?;
        let endpoints = self
            .endpoints
            .subscribe()
            .and_then(|subscription| subscription.snapshot())
            .map_err(|_| DeliveryContractError::ClientOperation)?
            .endpoints;
        let params = NativeSendParams {
            target: request.target.clone(),
            generation: generation.clone(),
            message: request.message,
            delivery: request.mode,
            client_user_message_id: Some(
                request
                    .correlation
                    .as_str()
                    .to_owned()
                    .try_into()
                    .map_err(|_| DeliveryContractError::InvalidEvidence)?,
            ),
        };
        let checked_out = self
            .holder
            .checkout(&String::from(request.target.session_id.clone()));
        let response = match checked_out {
            HeldBindingCheckout::Ready(mut binding) => {
                let mut checkout = HeldBindingCleanup::new(&self.holder, binding.session_id());
                if binding.generation() != &generation {
                    checkout.finish();
                    effects.submission = SubmissionEffect::NotDispatched;
                    effects.resume = PreparationEffect::NotRequested;
                    sink.record(RouteEffectEvidence::CodexAppServer(effects))
                        .await?;
                    return Ok(not_submitted("staleGeneration", false));
                }
                let response = crate::native_message_dispatch::dispatch_message(
                    crate::native_message_dispatch::NativeMessageRequest {
                        params,
                        id: json!(request.attempt.as_str()),
                        service_id: &self.service_id,
                        backend: &self.backend,
                        endpoints: &endpoints,
                        held_connection: Some(binding.connection_mut()),
                    },
                )
                .await;
                match &response {
                    crate::native_message_dispatch::NativeMessageOutcome::Accepted(receipt)
                        if matches!(
                            receipt.acceptance,
                            NativeSendAcceptance::QueueAccepted { .. }
                        ) =>
                    {
                        self.holder.restore(*binding);
                        checkout.disarm();
                    }
                    crate::native_message_dispatch::NativeMessageOutcome::Failed(failure)
                        if matches!(
                            failure
                                .pointer("/error/data/effects/submission")
                                .and_then(Value::as_str),
                            Some("notDispatched" | "rejected")
                        ) =>
                    {
                        self.holder.restore(*binding);
                        checkout.disarm();
                    }
                    _ => checkout.finish(),
                }
                response
            }
            HeldBindingCheckout::Busy => {
                effects.submission = SubmissionEffect::NotDispatched;
                effects.resume = PreparationEffect::NotRequested;
                sink.record(RouteEffectEvidence::CodexAppServer(effects))
                    .await?;
                return Ok(not_submitted("Held session is busy", true));
            }
            HeldBindingCheckout::Missing => {
                crate::native_message_dispatch::dispatch_message(
                    crate::native_message_dispatch::NativeMessageRequest {
                        params,
                        id: json!(request.attempt.as_str()),
                        service_id: &self.service_id,
                        backend: &self.backend,
                        endpoints: &endpoints,
                        held_connection: None,
                    },
                )
                .await
            }
        };
        let receipt = interpret_native_response(&request.target, response, &mut effects);
        if sink
            .record(RouteEffectEvidence::CodexAppServer(effects))
            .await
            .is_err()
        {
            return Ok(DeliveryReceipt {
                outcome: DeliveryOutcome::Unknown,
                reachability: Some(SessionReachability::CodexAppServer),
                client: None,
            });
        }
        Ok(receipt)
    }
}

struct HeldBindingCleanup<'a> {
    holder: &'a crate::UnmaterializedThreadHolder,
    session_id: String,
    active: bool,
}
impl<'a> HeldBindingCleanup<'a> {
    fn new(holder: &'a crate::UnmaterializedThreadHolder, session_id: &str) -> Self {
        Self {
            holder,
            session_id: session_id.into(),
            active: true,
        }
    }
    fn disarm(&mut self) {
        self.active = false;
    }
    fn finish(&mut self) {
        self.holder.finish(&self.session_id);
        self.disarm();
    }
}
impl Drop for HeldBindingCleanup<'_> {
    fn drop(&mut self) {
        if self.active {
            self.holder.finish(&self.session_id);
        }
    }
}

impl SessionDeliveryRoute for CodexAppServerDeliveryRoute {
    fn scheduled_runs(&self) -> Option<std::sync::Arc<dyn crate::ScheduledRunRoute>> {
        Some(std::sync::Arc::new(
            crate::CodexAppServerScheduledRuns::new(self.backend.clone()),
        ))
    }

    fn reachability(&self) -> SessionReachability {
        SessionReachability::CodexAppServer
    }

    fn claim(&self, target: &SessionRef) -> DeliveryFuture<'_, RouteClaim> {
        let claim = if target.endpoint != self.backend.endpoint {
            RouteClaim::NotMine
        } else if self.backend.gate.acquire().is_ok() {
            RouteClaim::Holds
        } else {
            RouteClaim::Unavailable {
                reason: RouteUnavailableReason {
                    reason: "Native backend unavailable".into(),
                    fix: "retry when the app-server is available".into(),
                },
                retryable: true,
            }
        };
        Box::pin(async move { Ok(claim) })
    }

    fn deliver<'a>(
        &'a self,
        request: DeliveryRequest,
        sink: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, DeliveryReceipt> {
        Box::pin(async move { self.deliver_native(request, sink).await })
    }

    fn reconcile_attempt(
        &self,
        context: AttemptReconciliationContext,
    ) -> DeliveryFuture<'_, AttemptReconciliation> {
        Box::pin(async move {
            Ok(crate::codex_queue_reconciliation::reconcile(&self.backend, context).await)
        })
    }
}

fn not_submitted(reason: &str, retryable: bool) -> DeliveryReceipt {
    DeliveryReceipt {
        outcome: DeliveryOutcome::NotSubmitted {
            retryable,
            reason: reason.into(),
        },
        reachability: Some(SessionReachability::CodexAppServer),
        client: None,
    }
}

fn interpret_native_response(
    target: &SessionRef,
    response: crate::native_message_dispatch::NativeMessageOutcome,
    effects: &mut NativeEffectEvidence<SessionRef, CodexGeneration>,
) -> DeliveryReceipt {
    let (outcome, client) = match response {
        crate::native_message_dispatch::NativeMessageOutcome::Accepted(receipt) => match receipt {
            receipt
                if receipt.target == *target
                    && effects.generation.as_ref() == Some(&receipt.generation)
                    && effects.client_user_message_id.as_deref()
                        == Some(String::from(receipt.client_user_message_id.clone()).as_str()) =>
            {
                effects.resume = match receipt.resume_effect {
                    AcceptedResumeEffect::Accepted => PreparationEffect::Accepted,
                    AcceptedResumeEffect::NotRequested => PreparationEffect::NotRequested,
                };
                effects.submission = SubmissionEffect::Accepted;
                let client = Some(DeliveryClientReceipt::CodexAppServer(receipt.clone()));
                match receipt.acceptance {
                    NativeSendAcceptance::QueueAccepted { submission_id } => {
                        effects.native_submission_id = Some(String::from(submission_id));
                        (DeliveryOutcome::Queued, client)
                    }
                    NativeSendAcceptance::NativeInputAccepted { turn_id, .. } => {
                        effects.native_turn_id = Some(String::from(turn_id));
                        (DeliveryOutcome::StartedOrSteered, client)
                    }
                    NativeSendAcceptance::SteerAccepted {
                        turn_id,
                        submission_id,
                    } => {
                        effects.native_turn_id = Some(String::from(turn_id));
                        effects.native_submission_id = submission_id.map(String::from);
                        (DeliveryOutcome::Steered, client)
                    }
                }
            }
            _ => {
                effects.submission = SubmissionEffect::Unknown;
                (DeliveryOutcome::Unknown, None)
            }
        },
        crate::native_message_dispatch::NativeMessageOutcome::Failed(response) => {
            let data = response.pointer("/error/data");
            effects.resume = match data
                .and_then(|value| value.pointer("/effects/resume"))
                .and_then(Value::as_str)
            {
                Some("notRequested") => PreparationEffect::NotRequested,
                Some("accepted") => PreparationEffect::Accepted,
                Some("rejected") => PreparationEffect::Rejected,
                _ => PreparationEffect::Unknown,
            };
            effects.submission = match data
                .and_then(|value| value.pointer("/effects/submission"))
                .and_then(Value::as_str)
            {
                Some("notDispatched") => SubmissionEffect::NotDispatched,
                Some("rejected") => SubmissionEffect::Rejected,
                _ => SubmissionEffect::Unknown,
            };
            let kind = data
                .and_then(|value| value.get("kind"))
                .and_then(Value::as_str)
                .unwrap_or("outcomeUnknown");
            if effects.resume == PreparationEffect::Unknown
                || effects.submission == SubmissionEffect::Unknown
            {
                (DeliveryOutcome::Unknown, None)
            } else if kind == "threadMissingOrUnmaterialized" {
                (
                    DeliveryOutcome::NotSubmitted {
                        retryable: false,
                        reason: "thread not found or never started; if it was created without a first message, it was lost when the Host restarted — create it again".into(),
                    },
                    None,
                )
            } else if kind == "nativeRejected" || kind == "unsupportedCapability" {
                let reason = data
                    .and_then(|value| value.get("reason"))
                    .cloned()
                    .and_then(|value| serde_json::from_value::<DeliveryRejectionReason>(value).ok())
                    .unwrap_or(DeliveryRejectionReason::UnsupportedCapability);
                let next_action = data
                    .and_then(|value| value.get("nextAction"))
                    .cloned()
                    .and_then(|value| serde_json::from_value::<DeliveryNextAction>(value).ok())
                    .unwrap_or(DeliveryNextAction::CorrectRequest);
                (
                    DeliveryOutcome::Rejected(DeliveryRejection {
                        reason,
                        next_action,
                        client_code: data
                            .and_then(|value| value.get("clientCode"))
                            .and_then(Value::as_i64),
                        detail: None,
                    }),
                    None,
                )
            } else {
                (
                    DeliveryOutcome::NotSubmitted {
                        retryable: matches!(kind, "unavailable" | "overloaded"),
                        reason: kind.into(),
                    },
                    None,
                )
            }
        }
    };
    DeliveryReceipt {
        outcome,
        reachability: Some(SessionReachability::CodexAppServer),
        client,
    }
}
