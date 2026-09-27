//! Legacy native approval broker integration.

use super::*;

impl ApprovalBroker for ServiceInteractionBroker {
    fn register_route(
        &self,
        route: ApprovalRoute,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), ApprovalBrokerError>> + Send + '_>,
    > {
        Box::pin(async move {
            if route.created_by.endpoint.service_id != self.service_id
                || route.approver.endpoint.service_id != self.service_id
            {
                return Err(ApprovalBrokerError::RouteUnavailable);
            }
            self.routes
                .lock()
                .await
                .insert(route.thread_id.clone(), route);
            self.persist_routes().await
        })
    }

    fn request(
        &self,
        request: BrokeredApprovalRequest,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<BrokeredApprovalOutcome, ApprovalBrokerError>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            let requester = SessionRef {
                endpoint: self.backend.endpoint.clone(),
                session_id: request
                    .thread_id
                    .clone()
                    .try_into()
                    .map_err(|_| ApprovalBrokerError::Unavailable)?,
            };
            let route = self.routes.lock().await.get(&request.thread_id).cloned();
            let Some(route) = route else {
                let diagnostic = native_approval_refusal_diagnostic(
                    &self.backend.endpoint,
                    &request.thread_id,
                    &request.request,
                    NativeApprovalRefusalReason::MissingRoute,
                );
                tracing::warn!(
                    endpoint = %diagnostic.endpoint,
                    provider_session_id = %diagnostic.provider_session_id,
                    method = diagnostic.method,
                    reason_code = diagnostic.reason_code,
                    "native approval request refused before route lookup",
                );
                return Err(ApprovalBrokerError::RouteUnavailable);
            };
            let native_options = request
                .request
                .pointer("/params/options")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let offered_options = native_option_records(native_options);
            if requester == route.approver {
                let request_id = format!(
                    "approval-{}",
                    String::from(
                        crate::new_service_uuid().map_err(|_| ApprovalBrokerError::Unavailable)?
                    )
                );
                self.record(ApprovalRequestRecord {
                    request_id,
                    requester,
                    approver: route.approver,
                    generation: request.generation,
                    state: ApprovalState::ApproverIsRequester,
                    reason: Some("set a different approver".to_owned()),
                    offered_options,
                    presentation: None,
                    decision: None,
                    operation: request.request,
                    expires_at: chrono::Utc::now().to_rfc3339(),
                })
                .await?;
                return Ok(BrokeredApprovalOutcome::Cancelled);
            }
            let offered = match map_native_options(native_options) {
                Ok(offered) => offered,
                Err(reason) => {
                    let request_id = format!(
                        "approval-{}",
                        String::from(
                            crate::new_service_uuid()
                                .map_err(|_| { ApprovalBrokerError::Unavailable })?
                        )
                    );
                    self.record(ApprovalRequestRecord {
                        request_id,
                        requester,
                        approver: route.approver,
                        generation: request.generation,
                        state: ApprovalState::Cancelled,
                        reason: Some(reason.to_owned()),
                        offered_options,
                        presentation: None,
                        decision: None,
                        operation: request.request,
                        expires_at: chrono::Utc::now().to_rfc3339(),
                    })
                    .await?;
                    return Ok(BrokeredApprovalOutcome::Cancelled);
                }
            };
            let request_id = format!(
                "approval-{}",
                String::from(
                    crate::new_service_uuid().map_err(|_| ApprovalBrokerError::Unavailable)?
                )
            );
            let expires_at = (chrono::Utc::now()
                + chrono::Duration::seconds(APPROVAL_TIMEOUT.as_secs() as i64))
            .to_rfc3339();
            let record = ApprovalRequestRecord {
                request_id: request_id.clone(),
                requester,
                approver: route.approver,
                generation: request.generation,
                state: ApprovalState::PendingClientDecision,
                reason: None,
                offered_options,
                presentation: None,
                operation: request.request,
                expires_at,
                decision: None,
            };
            self.record(record.clone()).await?;
            let (completion, receiver) = oneshot::channel();
            self.pending.lock().await.insert(
                request_id.clone(),
                PendingApproval {
                    record: record.clone(),
                    offered,
                    completion,
                },
            );
            let mut cancellation = CancellationMarker {
                request_id: request_id.clone(),
                pending: Arc::clone(&self.pending),
                history: Arc::clone(&self.history),
                history_path: self.history_path.clone(),
                armed: true,
            };
            let deadline = tokio::time::Instant::now() + APPROVAL_TIMEOUT;
            let delivery = self.deliver(&record);
            tokio::pin!(delivery);
            let delivery_result = tokio::select! {
                biased;
                () = tokio::time::sleep_until(deadline) => {
                    self.finish_pending(
                        &request_id,
                        ApprovalState::TimedOut,
                        Some("approval timed out before the notice reached its approver"),
                    ).await?;
                    cancellation.armed = false;
                    return Ok(BrokeredApprovalOutcome::Cancelled);
                }
                result = &mut delivery => result,
            };
            if delivery_result.is_err() {
                self.finish_pending(
                    &request_id,
                    ApprovalState::ApproverUnreachable,
                    Some("approval notice could not be delivered to the configured approver"),
                )
                .await?;
                cancellation.armed = false;
                return Ok(BrokeredApprovalOutcome::Cancelled);
            }
            tokio::select! {
                biased;
                () = tokio::time::sleep_until(deadline) => {
                    self.finish_pending(
                        &request_id,
                        ApprovalState::TimedOut,
                        Some("approval timed out before an approver decided"),
                    ).await?;
                    cancellation.armed = false;
                    Ok(BrokeredApprovalOutcome::Cancelled)
                }
                result = receiver => match result {
                Ok(outcome) => {
                    cancellation.armed = false;
                    Ok(outcome)
                }
                Err(_) => {
                    cancellation.armed = false;
                    Ok(BrokeredApprovalOutcome::Cancelled)
                }
            }
            }
        })
    }

    fn route(
        &self,
        thread_id: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Option<ApprovalRoute>, ApprovalBrokerError>>
                + Send
                + '_,
        >,
    > {
        let thread_id = thread_id.to_owned();
        Box::pin(async move { Ok(self.routes.lock().await.get(&thread_id).cloned()) })
    }
}
