//! Typed approval and question routing for provider Sessions.

use super::*;

impl ServiceInteractionBroker {
    /// Register an offered approval and return the exact agent option ID when
    /// its typed Approver decides. Legacy approval history remains untouched.
    pub async fn request_typed_approval(
        self: &Arc<Self>,
        requester: message_board::SessionRef,
        approver: message_board::Identity,
        request: session_event_model::ApprovalRequest,
        turn_cancellation: tokio_util::sync::CancellationToken,
        retirement: tokio_util::sync::CancellationToken,
        legacy_context: Option<TypedApprovalLegacyContext>,
    ) -> Result<oneshot::Receiver<TypedApprovalResolution>, InteractionHistoryError> {
        if !self.participants_belong_to_service(&requester, &approver) {
            return Err(InteractionHistoryError::Unavailable);
        }
        let legacy_metadata = if matches!(&approver, message_board::Identity::Session { .. }) {
            legacy_context.map(|context| LegacyApprovalMetadata {
                operation_id: context.operation_id,
                target: context.target,
                generation: context.generation,
                expires_at: (chrono::Utc::now()
                    + chrono::Duration::seconds(APPROVAL_TIMEOUT.as_secs() as i64))
                .to_rfc3339(),
            })
        } else {
            None
        };
        let request_id = request.request_id.clone();
        let typed_operation = self.typed_operations.lock().await;
        if self
            .typed_pending_approvals
            .lock()
            .await
            .contains_key(&request_id)
        {
            return Err(InteractionHistoryError::AlreadyExists);
        }
        if turn_cancellation.is_cancelled() || retirement.is_cancelled() {
            let reason = if turn_cancellation.is_cancelled() {
                "turnCancelled"
            } else {
                "providerRetired"
            };
            self.interaction_history
                .record_cancelled_approval(requester, approver, request, reason, legacy_metadata)
                .await?;
            return Err(InteractionHistoryError::NotPending);
        }
        let requester_for_pending = requester.clone();
        let notice_requester = requester.clone();
        let notice_approver = approver.clone();
        let notice_request = request.clone();
        self.interaction_history
            .record(InteractionHistoryRecord::Approval {
                requester,
                approver,
                request,
                state: InteractionHistoryState::Pending,
                legacy_metadata,
            })
            .await?;
        #[cfg(test)]
        if let Some(pause) = self.typed_after_record.lock().await.take() {
            let _ = pause.recorded.send(());
            let _ = pause.resume.await;
        }
        if turn_cancellation.is_cancelled() || retirement.is_cancelled() {
            let reason = if turn_cancellation.is_cancelled() {
                "turnCancelled"
            } else {
                "providerRetired"
            };
            self.interaction_history
                .cancel_approval(&request_id, reason)
                .await?;
            return Err(InteractionHistoryError::NotPending);
        }
        let (sender, receiver) = oneshot::channel();
        self.typed_pending_approvals.lock().await.insert(
            request_id.clone(),
            TypedPendingApproval {
                completion: sender,
                turn_cancellation: turn_cancellation.clone(),
                retirement: retirement.clone(),
                requester: requester_for_pending,
                notice_task: NoticeTask::default(),
            },
        );
        if turn_cancellation.is_cancelled() || retirement.is_cancelled() {
            let reason = if turn_cancellation.is_cancelled() {
                "turnCancelled"
            } else {
                "providerRetired"
            };
            self.interaction_history
                .cancel_approval(&request_id, reason)
                .await?;
            self.typed_pending_approvals
                .lock()
                .await
                .remove(&request_id);
            return Err(InteractionHistoryError::NotPending);
        }
        drop(typed_operation);
        let broker = Arc::downgrade(self);
        let delivery = self.session_delivery.get().cloned();
        let request_id_for_notice = request_id.clone();
        let notice_task = tokio::spawn(async move {
            if typed_interaction_notice::deliver_approval_notice(
                delivery.as_ref(),
                &notice_requester,
                &notice_approver,
                &notice_request,
            )
            .await
            .is_err()
                && let Some(broker) = broker.upgrade()
            {
                let _ = broker
                    .cancel_typed_approval(&request_id_for_notice, "approverUnreachable")
                    .await;
            }
        });
        if let Some(pending) = self
            .typed_pending_approvals
            .lock()
            .await
            .get_mut(&request_id)
        {
            pending.notice_task.0 = Some(notice_task);
        } else {
            notice_task.abort();
        }
        Ok(receiver)
    }

    pub async fn list_typed_approvals(
        &self,
        pending_only: bool,
    ) -> Vec<session_event_model::ApprovalRequest> {
        self.interaction_history
            .list_approvals(pending_only)
            .await
            .into_iter()
            .filter_map(|record| record.approval_request().cloned())
            .collect()
    }

    pub async fn list_interactions(&self) -> Vec<InteractionHistoryRecord> {
        self.interaction_history.list_all().await
    }

    pub async fn record_typed_refusal(
        &self,
        requester: message_board::SessionRef,
        approver: message_board::Identity,
        refusal: RefusedTypedApproval,
    ) -> Result<(), InteractionHistoryError> {
        if !self.participants_belong_to_service(&requester, &approver) {
            return Err(InteractionHistoryError::Unavailable);
        }
        self.interaction_history
            .record_refused_approval(requester, approver, refusal)
            .await
    }

    pub async fn request_question(
        self: &Arc<Self>,
        requester: message_board::SessionRef,
        approver: message_board::Identity,
        request: session_event_model::QuestionRequest,
        retirement: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<oneshot::Receiver<QuestionResponse>, InteractionHistoryError> {
        if !self.participants_belong_to_service(&requester, &approver) {
            return Err(InteractionHistoryError::Unavailable);
        }
        let request_id = request.request_id.clone();
        let notice_requester = requester.clone();
        let notice_approver = approver.clone();
        let notice_request = request.clone();
        let mut pending = self.pending_questions.lock().await;
        if pending.contains_key(&request_id) {
            return Err(InteractionHistoryError::AlreadyExists);
        }
        self.interaction_history
            .record_question(requester.clone(), approver, request)
            .await?;
        let (sender, receiver) = oneshot::channel();
        pending.insert(
            request_id.clone(),
            PendingQuestion {
                completion: sender,
                requester,
                retirement,
                notice_task: NoticeTask::default(),
            },
        );
        drop(pending);
        let broker = Arc::downgrade(self);
        let delivery = self.session_delivery.get().cloned();
        let request_id_for_notice = request_id.clone();
        let notice_task = tokio::spawn(async move {
            if typed_interaction_notice::deliver_question_notice(
                delivery.as_ref(),
                &notice_requester,
                &notice_approver,
                &notice_request,
            )
            .await
            .is_err()
                && let Some(broker) = broker.upgrade()
            {
                let _ = broker
                    .cancel_question(&request_id_for_notice, "approverUnreachable")
                    .await;
            }
        });
        if let Some(pending) = self.pending_questions.lock().await.get_mut(&request_id) {
            pending.notice_task.0 = Some(notice_task);
        } else {
            notice_task.abort();
        }
        Ok(receiver)
    }

    pub async fn record_cancelled_question(
        &self,
        requester: message_board::SessionRef,
        approver: message_board::Identity,
        request: session_event_model::QuestionRequest,
        reason: &str,
    ) -> Result<(), InteractionHistoryError> {
        if !self.participants_belong_to_service(&requester, &approver) {
            return Err(InteractionHistoryError::Unavailable);
        }
        self.interaction_history
            .record_cancelled_question(requester, approver, request, reason)
            .await
    }

    pub async fn list_questions(&self, pending_only: bool) -> Vec<InteractionHistoryRecord> {
        self.interaction_history.list_questions(pending_only).await
    }

    pub async fn respond_question(
        &self,
        request_id: &str,
        actor: &message_board::Identity,
        response: QuestionResponse,
    ) -> Result<(), InteractionHistoryError> {
        let mut pending = self.pending_questions.lock().await;
        if !pending.contains_key(request_id) {
            if let Some(InteractionHistoryRecord::Question { state, .. }) =
                self.interaction_history.interaction(request_id).await
                && state != QuestionHistoryState::Pending
            {
                return Err(InteractionHistoryError::AlreadySettled);
            }
            return Err(InteractionHistoryError::NotPending);
        }
        self.interaction_history
            .validate_question_response(request_id, actor, &response)
            .await?;
        #[cfg(test)]
        if let Some(pause) = self.question_before_send.lock().await.take() {
            let _ = pause.recorded.send(());
            let _ = pause.resume.await;
        }
        let sender = pending
            .remove(request_id)
            .ok_or(InteractionHistoryError::NotPending)?;
        if sender.completion.send(response.clone()).is_err() {
            self.interaction_history
                .cancel_question(request_id, "requesterUnavailable")
                .await?;
            return Err(InteractionHistoryError::NotPending);
        }
        self.interaction_history
            .respond_question(request_id, actor, &response)
            .await
    }

    pub async fn cancel_questions(
        &self,
        requester: &message_board::SessionRef,
        reason: &str,
    ) -> Result<Vec<String>, InteractionHistoryError> {
        let mut pending = self.pending_questions.lock().await;
        let cancelled = self
            .interaction_history
            .cancel_questions(requester, reason)
            .await?;
        for request_id in &cancelled {
            if let Some(sender) = pending.remove(request_id) {
                let _ = sender.completion.send(QuestionResponse::Cancelled);
            }
        }
        Ok(cancelled)
    }

    pub async fn cancel_typed_approvals(
        &self,
        requester: &message_board::SessionRef,
        reason: &str,
    ) -> Result<Vec<String>, InteractionHistoryError> {
        let _typed_operation = self.typed_operations.lock().await;
        let cancelled = self
            .interaction_history
            .cancel_approvals(requester, reason)
            .await?;
        for request_id in &cancelled {
            self.typed_pending_approvals.lock().await.remove(request_id);
        }
        Ok(cancelled)
    }

    pub async fn cancel_typed_approval(
        &self,
        request_id: &str,
        reason: &str,
    ) -> Result<(), InteractionHistoryError> {
        let _typed_operation = self.typed_operations.lock().await;
        if !self
            .typed_pending_approvals
            .lock()
            .await
            .contains_key(request_id)
        {
            return Err(InteractionHistoryError::NotPending);
        }
        self.interaction_history
            .cancel_approval(request_id, reason)
            .await?;
        self.typed_pending_approvals.lock().await.remove(request_id);
        Ok(())
    }

    pub async fn cancel_retired_typed_approvals(
        &self,
    ) -> Result<Vec<(message_board::SessionRef, String)>, InteractionHistoryError> {
        let _typed_operation = self.typed_operations.lock().await;
        let retired = self
            .typed_pending_approvals
            .lock()
            .await
            .iter()
            .filter(|(_, approval)| approval.retirement.is_cancelled())
            .map(|(request_id, approval)| (approval.requester.clone(), request_id.clone()))
            .collect::<Vec<_>>();
        for (_, request_id) in &retired {
            self.interaction_history
                .cancel_approval(request_id, "providerRetired")
                .await?;
            self.typed_pending_approvals.lock().await.remove(request_id);
        }
        let retired_questions = self
            .pending_questions
            .lock()
            .await
            .iter()
            .filter(|(_, question)| {
                question
                    .retirement
                    .as_ref()
                    .is_some_and(|retirement| retirement.is_cancelled())
            })
            .map(|(request_id, question)| (question.requester.clone(), request_id.clone()))
            .collect::<Vec<_>>();
        let mut settled_questions = Vec::new();
        for (requester, request_id) in retired_questions {
            match self.cancel_question(&request_id, "providerRetired").await {
                Ok(()) => settled_questions.push((requester, request_id)),
                Err(
                    InteractionHistoryError::AlreadySettled | InteractionHistoryError::NotPending,
                ) => {}
                Err(error) => return Err(error),
            }
        }
        let mut retired = retired;
        retired.extend(settled_questions);
        Ok(retired)
    }

    pub async fn cancel_question(
        &self,
        request_id: &str,
        reason: &str,
    ) -> Result<(), InteractionHistoryError> {
        let mut pending = self.pending_questions.lock().await;
        if !pending.contains_key(request_id) {
            if let Some(InteractionHistoryRecord::Question { state, .. }) =
                self.interaction_history.interaction(request_id).await
                && state != QuestionHistoryState::Pending
            {
                return Err(InteractionHistoryError::AlreadySettled);
            }
            return Err(InteractionHistoryError::NotPending);
        }
        self.interaction_history
            .cancel_question(request_id, reason)
            .await?;
        let sender = pending
            .remove(request_id)
            .ok_or(InteractionHistoryError::NotPending)?;
        let _ = sender.completion.send(QuestionResponse::Cancelled);
        Ok(())
    }

    pub async fn decide_typed_interaction(
        &self,
        request_id: &str,
        actor: &message_board::Identity,
        decision: TypedInteractionDecision,
    ) -> Result<TypedInteractionDecisionOutcome, InteractionHistoryError> {
        if decision == TypedInteractionDecision::Cancel {
            let record = self
                .interaction_history
                .interaction(request_id)
                .await
                .ok_or(InteractionHistoryError::NotPending)?;
            return match record {
                InteractionHistoryRecord::Approval { .. } => {
                    let _typed_operation = self.typed_operations.lock().await;
                    let cancellation = self
                        .typed_pending_approvals
                        .lock()
                        .await
                        .get(request_id)
                        .map(|approval| {
                            (
                                approval.turn_cancellation.is_cancelled(),
                                approval.retirement.is_cancelled(),
                            )
                        });
                    let Some((turn_cancelled, retired)) = cancellation else {
                        return Err(InteractionHistoryError::AlreadySettled);
                    };
                    if turn_cancelled || retired {
                        let reason = if turn_cancelled {
                            "turnCancelled"
                        } else {
                            "providerRetired"
                        };
                        self.interaction_history
                            .cancel_approval(request_id, reason)
                            .await?;
                        self.typed_pending_approvals.lock().await.remove(request_id);
                        return Err(InteractionHistoryError::NotPending);
                    }
                    self.interaction_history
                        .cancel_approval_as_approver(request_id, actor)
                        .await?;
                    let approval = self
                        .typed_pending_approvals
                        .lock()
                        .await
                        .remove(request_id)
                        .ok_or(InteractionHistoryError::NotPending)?;
                    let _ = approval.completion.send(TypedApprovalResolution::Cancelled);
                    Ok(TypedInteractionDecisionOutcome::ApprovalCancelled)
                }
                InteractionHistoryRecord::Question { .. } => {
                    self.respond_question(request_id, actor, QuestionResponse::Cancelled)
                        .await?;
                    Ok(TypedInteractionDecisionOutcome::QuestionCancelled)
                }
                InteractionHistoryRecord::RefusedApproval { .. } => {
                    Err(InteractionHistoryError::AlreadySettled)
                }
            };
        }
        let TypedInteractionDecision::SelectApproval {
            option_id,
            acknowledge_persistent,
            note,
        } = decision
        else {
            return Err(InteractionHistoryError::NotPending);
        };
        let _typed_operation = self.typed_operations.lock().await;
        let cancelled = self
            .typed_pending_approvals
            .lock()
            .await
            .get(request_id)
            .map(|approval| {
                (
                    approval.turn_cancellation.is_cancelled(),
                    approval.retirement.is_cancelled(),
                )
            });
        if let Some((turn_cancelled, retired)) = cancelled
            && (turn_cancelled || retired)
        {
            let reason = if turn_cancelled {
                "turnCancelled"
            } else {
                "providerRetired"
            };
            self.interaction_history
                .cancel_approval(request_id, reason)
                .await?;
            self.typed_pending_approvals.lock().await.remove(request_id);
            return Err(InteractionHistoryError::NotPending);
        }
        if cancelled.is_none() {
            if let Some(InteractionHistoryRecord::Approval { state, .. }) =
                self.interaction_history.interaction(request_id).await
                && state != InteractionHistoryState::Pending
            {
                return Err(InteractionHistoryError::AlreadySettled);
            }
            return Err(InteractionHistoryError::NotPending);
        }
        let selected = self
            .interaction_history
            .decide(request_id, actor, &option_id, acknowledge_persistent)
            .await?;
        let sender = self
            .typed_pending_approvals
            .lock()
            .await
            .remove(request_id)
            .ok_or(InteractionHistoryError::NotPending)?;
        sender
            .completion
            .send(TypedApprovalResolution::Selected(TypedApprovalSelection {
                option_id: selected.clone(),
                note,
            }))
            .map_err(|_| InteractionHistoryError::Unavailable)?;
        Ok(TypedInteractionDecisionOutcome::ApprovalSelected {
            option_id: selected,
        })
    }
}
