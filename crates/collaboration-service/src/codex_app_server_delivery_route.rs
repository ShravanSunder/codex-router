//! Codex app-server delivery owns native admission and its recorded effects.
use crate::native_message_dispatch::{
    NativeMessageBody, NativeMessageParams, NativeThreadStatus, NativeThreadStatusReadError,
    read_native_thread_status,
};
use crate::{
    AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext,
    DeliveryClientReceipt, DeliveryContractError, DeliveryFuture, DeliveryPrecondition,
    DeliveryReceipt, DeliveryRequest, EndpointDirectory, LoadPolicy, NOT_LOADED_REASON,
    NativeControlBackend, RouteClaim, RoutePresence, RouteUnavailableReason, SessionDeliveryRoute,
};
use agent_automation::{
    CessationEvidence, NativeEffectEvidence, PreparationEffect, RouteEffectEvidence,
    SubmissionEffect,
};
use codex_acp_adapter::{HeldBindingCheckout, UnmaterializedBindingStore};
use codex_native_integration::NativeProtocolConnection;
use collaboration_protocol::{
    AcceptedResumeEffect, ChannelDescription, CodexGeneration, DeliveryCorrelationId,
    DeliveryNextAction, DeliveryOutcome, DeliveryRejection, DeliveryRejectionReason,
    MessageDelivery, MessageHeaderContext, NativeSendAcceptance, SessionReachability, SessionRef,
    UuidIdentity,
};
use serde_json::{Value, json};
use std::time::Duration;

pub struct CodexAppServerDeliveryRoute {
    service_id: UuidIdentity,
    endpoints: EndpointDirectory,
    backend: NativeControlBackend,
    holder: std::sync::Arc<crate::UnmaterializedThreadHolder>,
    display_names: crate::SessionDisplayNameCache,
}

struct NativeRouteDeliveryRequest {
    target: SessionRef,
    body: NativeMessageBody,
    header_context: MessageHeaderContext,
    mode: MessageDelivery,
    load_policy: LoadPolicy,
    precondition: DeliveryPrecondition,
    correlation: DeliveryCorrelationId,
    attempt: agent_automation::AttemptId,
}

impl From<DeliveryRequest> for NativeRouteDeliveryRequest {
    fn from(request: DeliveryRequest) -> Self {
        Self {
            target: request.target,
            body: NativeMessageBody::Content(request.message),
            header_context: request.header_context,
            mode: request.mode,
            load_policy: request.load_policy,
            precondition: request.precondition,
            correlation: request.correlation,
            attempt: request.attempt,
        }
    }
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
            display_names: crate::SessionDisplayNameCache::default(),
        }
    }

    #[must_use]
    pub fn with_display_names(mut self, display_names: crate::SessionDisplayNameCache) -> Self {
        self.display_names = display_names;
        self
    }

    async fn deliver_native(
        &self,
        request: NativeRouteDeliveryRequest,
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
        let header_context = request.header_context.clone();
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
        let load_policy = request.load_policy;
        let params = NativeMessageParams {
            target: request.target.clone(),
            generation: generation.clone(),
            body: request.body,
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
        let (response, held_idle_submission) = match checked_out {
            HeldBindingCheckout::Ready(mut binding) => {
                let mut checkout = crate::unmaterialized_thread_holder::HeldBindingCleanup::new(
                    &self.holder,
                    binding.session_id(),
                );
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
                        header_context: header_context.clone(),
                        display_names: &self.display_names,
                        held_connection: Some(binding.connection_mut()),
                        load_policy,
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
                (response, true)
            }
            HeldBindingCheckout::Busy => {
                effects.submission = SubmissionEffect::NotDispatched;
                effects.resume = PreparationEffect::NotRequested;
                sink.record(RouteEffectEvidence::CodexAppServer(effects))
                    .await?;
                return Ok(not_submitted("Held session is busy", true));
            }
            HeldBindingCheckout::Missing => {
                let response = crate::native_message_dispatch::dispatch_message(
                    crate::native_message_dispatch::NativeMessageRequest {
                        params,
                        id: json!(request.attempt.as_str()),
                        service_id: &self.service_id,
                        backend: &self.backend,
                        endpoints: &endpoints,
                        header_context,
                        display_names: &self.display_names,
                        held_connection: None,
                        load_policy,
                    },
                )
                .await;
                (response, false)
            }
        };
        let receipt = interpret_native_response(
            &request.target,
            response,
            held_idle_submission,
            &mut effects,
        );
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

    async fn inspect_thread_presence(&self, target: &SessionRef) -> RoutePresence {
        if target.endpoint != self.backend.endpoint {
            return RoutePresence::NotMine;
        }
        let session_id = String::from(target.session_id.clone());
        if self.holder.contains(&session_id) {
            return RoutePresence::Running;
        }
        let Ok(admission) = self.backend.gate.acquire() else {
            return RoutePresence::Unreachable {
                reason: "native backend is unavailable".to_owned(),
            };
        };
        let Some(schemas) = admission.schemas() else {
            return RoutePresence::Unreachable {
                reason: "native thread/read is not supported".to_owned(),
            };
        };
        let endpoint_snapshot = self
            .endpoints
            .subscribe()
            .and_then(|subscription| subscription.snapshot());
        let Some(endpoint) = endpoint_snapshot.ok().and_then(|snapshot| {
            snapshot
                .endpoints
                .into_iter()
                .find(|entry| entry.endpoint == target.endpoint)
        }) else {
            return RoutePresence::Unreachable {
                reason: "Codex endpoint is not available".to_owned(),
            };
        };
        let schema_matches = endpoint.channels.iter().any(|channel| {
            matches!(
                channel,
                ChannelDescription::NativeCodex {
                    schema_digest: Some(digest),
                    generation: Some(generation),
                    ..
                } if generation == admission.generation()
                    && String::from(digest.clone()) == schemas.schema_digest()
            )
        });
        if !schema_matches {
            return RoutePresence::Unreachable {
                reason: "native thread/read schema is unavailable".to_owned(),
            };
        }

        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let retired = admission.retirement();
        let connection = tokio::time::timeout_at(deadline, async {
            tokio::select! {
                biased;
                _ = retired.cancelled() => Err(codex_native_integration::NativeConnectionError::Unavailable),
                result = NativeProtocolConnection::connect(admission.backend_path()) => result,
            }
        })
        .await
        .unwrap_or(Err(
            codex_native_integration::NativeConnectionError::Unavailable,
        ));
        let Ok(mut connection) = connection else {
            return RoutePresence::Unreachable {
                reason: "native backend is unavailable".to_owned(),
            };
        };
        let session_id = String::from(target.session_id.clone());
        match read_native_thread_status(&mut connection, &schemas, &session_id, deadline, &retired)
            .await
        {
            Ok(snapshot) if snapshot.status == NativeThreadStatus::NotLoaded => {
                RoutePresence::Wakeable
            }
            Ok(snapshot)
                if matches!(
                    snapshot.status,
                    NativeThreadStatus::Idle | NativeThreadStatus::Active
                ) =>
            {
                RoutePresence::Running
            }
            Ok(_) => RoutePresence::Unreachable {
                reason: "Codex thread status is unavailable".to_owned(),
            },
            Err(NativeThreadStatusReadError::Missing) => RoutePresence::Unreachable {
                reason: "Codex thread is missing".to_owned(),
            },
            Err(
                NativeThreadStatusReadError::Rejected { .. }
                | NativeThreadStatusReadError::Unsupported
                | NativeThreadStatusReadError::Unavailable,
            ) => RoutePresence::Unreachable {
                reason: "Codex thread could not be read".to_owned(),
            },
            Err(NativeThreadStatusReadError::InvalidResponse) => RoutePresence::Unreachable {
                reason: "Codex thread status is unavailable".to_owned(),
            },
        }
    }
}

impl SessionDeliveryRoute for CodexAppServerDeliveryRoute {
    fn scheduled_runs(&self) -> Option<std::sync::Arc<dyn crate::ScheduledRunRoute>> {
        Some(std::sync::Arc::new(
            crate::CodexAppServerScheduledRuns::new(
                self.backend.clone(),
                std::sync::Arc::clone(&self.holder),
            ),
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

    fn presence(&self, target: &SessionRef) -> DeliveryFuture<'_, RoutePresence> {
        let target = target.clone();
        Box::pin(async move { Ok(self.inspect_thread_presence(&target).await) })
    }

    fn deliver<'a>(
        &'a self,
        request: DeliveryRequest,
        sink: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, DeliveryReceipt> {
        Box::pin(async move { self.deliver_native(request.into(), sink).await })
    }

    fn deliver_prepared<'a>(
        &'a self,
        request: crate::layer_zero::DeliveryRequest,
        sink: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, DeliveryReceipt> {
        Box::pin(async move {
            let push_id = &request.payload.push_id;
            let line_push_id = crate::codex_queue_reconciliation::prepared_push_id_from_line(
                &request.payload.line,
            );
            if request.correlation.as_str() != push_id.as_str()
                || line_push_id.as_ref() != Some(push_id)
            {
                return Err(DeliveryContractError::InvalidEvidence);
            }
            self.deliver_native(
                NativeRouteDeliveryRequest {
                    target: request.target,
                    body: NativeMessageBody::PreparedPush(request.payload.line),
                    header_context: MessageHeaderContext::default(),
                    mode: request.mode,
                    load_policy: request.payload.load_policy,
                    precondition: request.precondition,
                    correlation: request.correlation,
                    attempt: request.attempt,
                },
                sink,
            )
            .await
        })
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
    held_idle_submission: bool,
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
                    NativeSendAcceptance::NativeInputAccepted {
                        operation, turn_id, ..
                    } => {
                        effects.native_turn_id = Some(String::from(turn_id));
                        let outcome = if held_idle_submission
                            && matches!(
                                operation,
                                collaboration_protocol::NativeInputOperation::TurnStart
                            ) {
                            DeliveryOutcome::Started
                        } else {
                            DeliveryOutcome::StartedOrSteered
                        };
                        (outcome, client)
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
            } else if kind == NOT_LOADED_REASON {
                (
                    DeliveryOutcome::NotSubmitted {
                        retryable: true,
                        reason: NOT_LOADED_REASON.into(),
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
                        detail: (reason == DeliveryRejectionReason::HeldByAnotherClient).then(
                            || {
                                crate::message_effect_state::HELD_BY_ANOTHER_CLIENT_GUIDANCE
                                    .to_owned()
                            },
                        ),
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
