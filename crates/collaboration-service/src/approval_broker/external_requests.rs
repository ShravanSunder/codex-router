//! External ACP permission requests and offered-option mapping.
use super::*;

impl ServiceApprovalBroker {
    pub async fn request_external(
        &self,
        request: ExternalApprovalRequest,
    ) -> Result<BrokeredApprovalOutcome, ApprovalBrokerError> {
        self.request_external_with_timeout(request, APPROVAL_TIMEOUT)
            .await
    }

    pub async fn record_external_refusal(
        &self,
        refusal: ExternalApprovalRefusal,
    ) -> Result<(), ApprovalBrokerError> {
        let ExternalApprovalRefusal {
            requester,
            approver,
            generation,
            operation_metadata,
            offered_options,
            presentation,
            reason,
        } = refusal;
        let request_id = format!(
            "approval-{}",
            String::from(crate::new_service_uuid().map_err(|_| ApprovalBrokerError::Unavailable)?)
        );
        self.record(ApprovalRequestRecord {
            request_id,
            requester,
            approver,
            generation,
            state: ApprovalState::Cancelled,
            reason: Some(reason),
            offered_options: offered_options.iter().map(external_option_record).collect(),
            presentation,
            decision: None,
            operation: serde_json::to_value(operation_metadata)
                .map_err(|_| ApprovalBrokerError::Unavailable)?,
            expires_at: chrono::Utc::now().to_rfc3339(),
        })
        .await
    }

    pub(super) async fn request_external_with_timeout(
        &self,
        request: ExternalApprovalRequest,
        timeout: Duration,
    ) -> Result<BrokeredApprovalOutcome, ApprovalBrokerError> {
        if request.requester.endpoint.service_id != self.service_id
            || request.approver.endpoint.service_id != self.service_id
            || request.operation_metadata.target.endpoint.service_id != self.service_id
        {
            self.record_external_refusal(ExternalApprovalRefusal {
                requester: request.requester,
                approver: request.approver,
                generation: request.generation,
                operation_metadata: request.operation_metadata,
                offered_options: request.options,
                presentation: request.presentation,
                reason: "requester and approver must belong to this Router service".to_owned(),
            })
            .await?;
            return Ok(BrokeredApprovalOutcome::Cancelled);
        }
        if request.approver == request.operation_metadata.target {
            self.record_external_refusal(ExternalApprovalRefusal {
                requester: request.requester,
                approver: request.approver,
                generation: request.generation,
                operation_metadata: request.operation_metadata,
                offered_options: request.options.clone(),
                presentation: request.presentation,
                reason:
                    "set a different approver; the approver cannot be the blocked provider session"
                        .to_owned(),
            })
            .await?;
            return Ok(BrokeredApprovalOutcome::Cancelled);
        }
        if request.retirement.is_cancelled() || request.cancellation.is_cancelled() {
            self.record_external_refusal(ExternalApprovalRefusal {
                requester: request.requester,
                approver: request.approver,
                generation: request.generation,
                operation_metadata: request.operation_metadata,
                offered_options: request.options.clone(),
                presentation: request.presentation,
                reason: "permission request was cancelled before it could be presented".to_owned(),
            })
            .await?;
            return Ok(BrokeredApprovalOutcome::Cancelled);
        }
        let offered_options = request.options.iter().map(external_option_record).collect();
        let offered = match map_external_options(request.options.clone()) {
            Ok(offered) => offered,
            Err(reason) => {
                self.record_external_refusal(ExternalApprovalRefusal {
                    requester: request.requester,
                    approver: request.approver,
                    generation: request.generation,
                    operation_metadata: request.operation_metadata,
                    offered_options: request.options,
                    presentation: request.presentation,
                    reason: reason.to_owned(),
                })
                .await?;
                return Ok(BrokeredApprovalOutcome::Cancelled);
            }
        };
        let request_id = format!(
            "approval-{}",
            String::from(crate::new_service_uuid().map_err(|_| ApprovalBrokerError::Unavailable)?)
        );
        let record = ApprovalRequestRecord {
            request_id: request_id.clone(),
            requester: request.requester,
            approver: request.approver,
            generation: request.generation,
            state: ApprovalState::PendingClientDecision,
            reason: None,
            offered_options,
            presentation: request.presentation,
            decision: None,
            operation: serde_json::to_value(request.operation_metadata)
                .map_err(|_| ApprovalBrokerError::Unavailable)?,
            expires_at: (chrono::Utc::now()
                + chrono::Duration::from_std(timeout).unwrap_or_else(|_| {
                    chrono::Duration::seconds(APPROVAL_TIMEOUT.as_secs() as i64)
                }))
            .to_rfc3339(),
        };
        self.record(record.clone()).await?;
        let (completion, receiver) = oneshot::channel();
        self.pending.lock().await.insert(
            request_id.clone(),
            PendingApproval {
                record: record.clone(),
                offered,
                completion,
                generation_authority: ApprovalGenerationAuthority::External {
                    generation: record.generation.clone(),
                    retirement: request.retirement.clone(),
                },
            },
        );
        let mut cancellation = CancellationMarker {
            request_id: request_id.clone(),
            pending: Arc::clone(&self.pending),
            history: Arc::clone(&self.history),
            history_path: self.history_path.clone(),
            armed: true,
        };
        let deadline = tokio::time::Instant::now() + timeout;
        let mut receiver = receiver;
        let delivery = self.deliver(&record);
        tokio::pin!(delivery);
        let delivery_result = tokio::select! {
            biased;
            () = request.retirement.cancelled() => {
                let finished = self.finish_pending(
                    &request_id,
                    ApprovalState::Cancelled,
                    Some("provider retired before approval completed"),
                ).await?;
                cancellation.armed = false;
                return Ok(if finished {
                    BrokeredApprovalOutcome::Cancelled
                } else {
                    receiver.await.unwrap_or(BrokeredApprovalOutcome::Cancelled)
                });
            }
            () = request.cancellation.cancelled() => {
                let finished = self.finish_pending(
                    &request_id,
                    ApprovalState::Cancelled,
                    Some("permission request was cancelled by the provider"),
                ).await?;
                cancellation.armed = false;
                return Ok(if finished {
                    BrokeredApprovalOutcome::Cancelled
                } else {
                    receiver.await.unwrap_or(BrokeredApprovalOutcome::Cancelled)
                });
            }
            () = tokio::time::sleep_until(deadline) => {
                let finished = self.finish_pending(
                    &request_id,
                    ApprovalState::TimedOut,
                    Some("approval timed out before the approver decided"),
                ).await?;
                cancellation.armed = false;
                if !finished {
                    return Ok(receiver.await.unwrap_or(BrokeredApprovalOutcome::Cancelled));
                }
                return Ok(BrokeredApprovalOutcome::Cancelled);
            }
            result = &mut receiver => {
                cancellation.armed = false;
                return Ok(result.unwrap_or(BrokeredApprovalOutcome::Cancelled));
            }
            result = &mut delivery => result,
        };
        if delivery_result.is_err() {
            let finished = self
                .finish_pending(
                    &request_id,
                    ApprovalState::ApproverUnreachable,
                    Some("approval notice could not be delivered to the configured approver"),
                )
                .await?;
            cancellation.armed = false;
            return Ok(if finished {
                BrokeredApprovalOutcome::Cancelled
            } else {
                receiver.await.unwrap_or(BrokeredApprovalOutcome::Cancelled)
            });
        }
        tokio::select! {
            biased;
            () = request.retirement.cancelled() => {
                let finished = self.finish_pending(
                    &request_id,
                    ApprovalState::Cancelled,
                    Some("provider retired before approval completed"),
                ).await?;
                cancellation.armed = false;
                if finished {
                    Ok(BrokeredApprovalOutcome::Cancelled)
                } else {
                    Ok(receiver.await.unwrap_or(BrokeredApprovalOutcome::Cancelled))
                }
            }
            () = request.cancellation.cancelled() => {
                let finished = self.finish_pending(
                    &request_id,
                    ApprovalState::Cancelled,
                    Some("permission request was cancelled by the provider"),
                ).await?;
                cancellation.armed = false;
                if finished {
                    Ok(BrokeredApprovalOutcome::Cancelled)
                } else {
                    Ok(receiver.await.unwrap_or(BrokeredApprovalOutcome::Cancelled))
                }
            }
            () = tokio::time::sleep_until(deadline) => {
                let finished = self.finish_pending(
                    &request_id,
                    ApprovalState::TimedOut,
                    Some("approval timed out before the approver decided"),
                ).await?;
                cancellation.armed = false;
                if finished {
                    Ok(BrokeredApprovalOutcome::Cancelled)
                } else {
                    Ok(receiver.await.unwrap_or(BrokeredApprovalOutcome::Cancelled))
                }
            }
            result = &mut receiver => match result {
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
    }
}

pub(super) fn map_external_options(
    options: Vec<ExternalApprovalOption>,
) -> Result<BTreeMap<ApprovalDecision, String>, &'static str> {
    let mut identifiers = std::collections::BTreeSet::new();
    let mut offered = BTreeMap::new();
    let mut has_supported_allow = false;
    for option in options {
        if !identifiers.insert(option.option_id.clone()) {
            return Err("external permission options contain a duplicate optionId");
        }
        let decision = match option.scope {
            ExternalApprovalOptionScope::AllowOnce => {
                has_supported_allow = true;
                Some(ApprovalDecision::Allow)
            }
            ExternalApprovalOptionScope::RejectOnce => Some(ApprovalDecision::Deny),
            ExternalApprovalOptionScope::AllowAlways
            | ExternalApprovalOptionScope::RejectAlways
            | ExternalApprovalOptionScope::Unsupported { .. } => None,
        };
        if let Some(decision) = decision
            && offered.insert(decision, option.option_id).is_some()
        {
            return Err("external permission options contain duplicate supported decisions");
        }
    }
    if !has_supported_allow {
        return Err("no supported one-time allow option remains");
    }
    Ok(offered)
}

pub(super) fn external_option_record(option: &ExternalApprovalOption) -> ApprovalOfferedOption {
    let scope = match &option.scope {
        ExternalApprovalOptionScope::AllowOnce => ApprovalOptionScope::AllowOnce,
        ExternalApprovalOptionScope::AllowAlways => ApprovalOptionScope::AllowAlways,
        ExternalApprovalOptionScope::RejectOnce => ApprovalOptionScope::RejectOnce,
        ExternalApprovalOptionScope::RejectAlways => ApprovalOptionScope::RejectAlways,
        ExternalApprovalOptionScope::Unsupported { provider_kind } => {
            ApprovalOptionScope::Unsupported {
                provider_kind: provider_kind.clone(),
            }
        }
    };
    ApprovalOfferedOption {
        option_id: option.option_id.clone(),
        label: option.label.clone(),
        scope,
    }
}
