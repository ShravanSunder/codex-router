//! Joining and replacing unique Participant seats.
use super::*;

impl BoardStore {
    pub async fn join_thread(
        &mut self,
        request: ThreadJoinRequest,
        now: DateTime<Utc>,
    ) -> Result<ThreadJoinResult, BoardError> {
        if request.replace.as_ref() == Some(&request.actor) {
            return Err(BoardError::participant_refusal(ParticipantRefusal {
                kind: BoardFailureKind::SelfReplace,
                message: "Replace must name a different current seat holder.".to_owned(),
                next_action: BoardNextAction::RepeatJoinWithoutReplace,
                root_message_id: request.root_message_id,
                actor: request.actor,
                holder: None,
                holder_last_seen_activity: None,
                target: None,
                named_holder: request.replace,
            }));
        }
        if request.replace.is_some()
            && !matches!(
                request.role,
                ParticipantRole::Orchestrator | ParticipantRole::Implementer
            )
        {
            return Err(BoardError::invalid_field(
                "replace",
                "may be used only with Role orchestrator or implementer",
            ));
        }
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let location = require_thread(&mut transaction, &request.root_message_id).await?;
        if location.state == ThreadState::Resolved {
            return Err(BoardError::thread_resolved());
        }
        let actor_key = ensure_identity(&mut transaction, &request.actor).await?;
        let existing =
            load_participant(&mut transaction, &request.actor, &request.root_message_id).await?;
        if existing.as_ref().is_some_and(|participant| {
            participant.is_open()
                && participant.role == ParticipantRole::Orchestrator
                && request.role != ParticipantRole::Orchestrator
        }) {
            return Err(BoardError::participant_refusal(ParticipantRefusal {
                kind: BoardFailureKind::OrchestratorRoleChange,
                message: "The current Orchestrator must Leave with handover or resolution before changing Role.".to_owned(),
                next_action: BoardNextAction::LeaveWithHandoverOrResolve,
                root_message_id: request.root_message_id,
                actor: request.actor,
                holder: existing.map(|participant| participant.identity),
                holder_last_seen_activity: None,
                target: None,
                named_holder: None,
            }));
        }
        let holder = match request.role {
            ParticipantRole::Orchestrator => {
                load_orchestrator(&mut transaction, &request.root_message_id)
                    .await?
                    .map(|holder| (holder.identity, holder.last_seen_activity))
            }
            ParticipantRole::Implementer => {
                load_implementer(&mut transaction, &request.root_message_id)
                    .await?
                    .map(|holder| (holder.identity, holder.last_seen_activity))
            }
            _ => None,
        };
        if matches!(
            request.role,
            ParticipantRole::Orchestrator | ParticipantRole::Implementer
        ) {
            match (&holder, &request.replace) {
                (Some((holder, last_seen)), None) if *holder != request.actor => {
                    return Err(match request.role {
                        ParticipantRole::Orchestrator => BoardError::orchestrator_already_exists(
                            request.root_message_id,
                            request.actor,
                            holder.clone(),
                            *last_seen,
                        ),
                        ParticipantRole::Implementer => BoardError::implementer_already_exists(
                            request.root_message_id,
                            request.actor,
                            holder.clone(),
                            *last_seen,
                        ),
                        _ => BoardError::invalid_field("role", "requires a unique seat"),
                    });
                }
                (Some((holder, last_seen)), Some(named)) if *holder != *named => {
                    return Err(BoardError::participant_refusal(ParticipantRefusal {
                        kind: if request.role == ParticipantRole::Orchestrator { BoardFailureKind::StaleOrchestrator } else { BoardFailureKind::StaleImplementer },
                        message: "The named seat holder is stale. Inspect Participants and retry with the current holder.".to_owned(),
                        next_action: BoardNextAction::InspectParticipants,
                        root_message_id: request.root_message_id,
                        actor: request.actor,
                        holder: Some(holder.clone()),
                        holder_last_seen_activity: Some(*last_seen),
                        target: None,
                        named_holder: request.replace,
                    }));
                }
                (None, Some(_)) => {
                    return Err(BoardError::participant_refusal(ParticipantRefusal {
                        kind: if request.role == ParticipantRole::Orchestrator { BoardFailureKind::StaleOrchestrator } else { BoardFailureKind::StaleImplementer },
                        message: "The named seat holder is stale because the Thread has no current holder.".to_owned(),
                        next_action: BoardNextAction::InspectParticipants,
                        root_message_id: request.root_message_id,
                        actor: request.actor,
                        holder: None,
                        holder_last_seen_activity: None,
                        target: None,
                        named_holder: request.replace,
                    }));
                }
                _ => {}
            }
        }
        let kind = if request.replace.is_some() {
            if request.role == ParticipantRole::Orchestrator {
                "orchestratorReplaced"
            } else {
                "implementerReplaced"
            }
        } else {
            "participantJoined"
        };
        let replaced_key = request.replace.as_ref().map(identity_key);
        let sequence = insert_lifecycle_activity(
            &mut transaction,
            &location,
            kind,
            &actor_key,
            Some(ParticipantChange {
                participant_key: &actor_key,
                participant_role: Some(request.role),
                replaced_participant_key: replaced_key.as_deref(),
            }),
        )
        .await?;
        if let Some((holder, _)) = &holder
            && request.replace.as_ref() == Some(holder)
        {
            let holder_key = identity_key(holder);
            match request.role {
                ParticipantRole::Orchestrator => sqlx::query!(
                    "UPDATE thread_participants SET last_seen_activity=?,closed_at_activity=?,closed_reason='replaced',replaced_by=? WHERE reader_key=? AND root_id=? AND role='orchestrator' AND closed_at_activity IS NULL",
                    sequence, sequence, actor_key, holder_key, request.root_message_id.as_str(),
                ).execute(&mut *transaction).await.map_err(storage_error)?,
                ParticipantRole::Implementer => sqlx::query!(
                    "UPDATE thread_participants SET last_seen_activity=?,closed_at_activity=?,closed_reason='replaced',replaced_by=? WHERE reader_key=? AND root_id=? AND role='implementer' AND closed_at_activity IS NULL",
                    sequence, sequence, actor_key, holder_key, request.root_message_id.as_str(),
                ).execute(&mut *transaction).await.map_err(storage_error)?,
                _ => return Err(BoardError::invalid_field("replace", "requires a unique seat")),
            };
            end_thread_subscription_for_leave(
                &mut transaction,
                &holder_key,
                &request.root_message_id,
                EndReason::Replaced,
                now,
            )
            .await?;
        }
        upsert_joined_participant(
            &mut transaction,
            &actor_key,
            &request.root_message_id,
            &request.actor,
            request.role,
            request.note.as_ref(),
            sequence,
        )
        .await?;
        apply_watch_choice(
            &mut transaction,
            &actor_key,
            &location.project_id,
            &request.root_message_id,
            sequence,
            request.watch,
        )
        .await?;
        if request.watch {
            let policy_patch = SubscriptionPolicyPatch {
                mode: request.mode,
                when_idle: request.when_idle,
                ..SubscriptionPolicyPatch::default()
            };
            let subscription_started = upsert_join_subscription(
                &mut transaction,
                &actor_key,
                &request.actor,
                &request.root_message_id,
                &policy_patch,
                now,
            )
            .await?;
            if subscription_started
                && let Some(latest_message_activity) =
                    latest_message_activity_for_root(&mut transaction, &request.root_message_id)
                        .await?
            {
                let _ = write_delivered_position_if_valid(
                    &mut transaction,
                    &actor_key,
                    &request.root_message_id,
                    latest_message_activity,
                )
                .await?;
            }
        } else {
            end_thread_subscription_for_join_without_watch(
                &mut transaction,
                &actor_key,
                &request.actor,
                &request.root_message_id,
                now,
            )
            .await?;
        }
        let participant =
            load_participant(&mut transaction, &request.actor, &request.root_message_id)
                .await?
                .ok_or_else(invalid_record)?;
        let orchestrator = load_orchestrator(&mut transaction, &request.root_message_id).await?;
        let implementer = load_implementer(&mut transaction, &request.root_message_id).await?;
        let watch_status =
            load_watch_status(&mut transaction, &actor_key, &request.root_message_id).await?;
        transaction.commit().await.map_err(storage_error)?;
        self.notify_activity();
        Ok(ThreadJoinResult {
            participant,
            orchestrator,
            implementer,
            watch_status,
            outcome: "Participant joined the Thread.".to_owned(),
        })
    }
}
