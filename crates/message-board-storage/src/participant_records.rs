//! Participant lifecycle, Orchestrator ownership, and Participant read models.
use crate::BoardStore;
use crate::board_topic_records::require_board;
use crate::message_records::{activate_watch, load_watch_status, require_thread};
use crate::participant_row_decoding::{
    StoredParticipantRow, decode_participant, load_orchestrator, load_participant,
    require_open_participant, role_name,
};
use crate::storage_support::{
    BoardTransaction, allocate_activity_sequence, archived_board,
    decode_cursor as decode_signed_cursor, encode_cursor, ensure_identity, identity_key,
    invalid_cursor, invalid_record, recompute_project_unread, storage_error,
};
use message_board::*;
use serde::{Deserialize, Serialize};
use sqlx::Connection;

const PARTICIPANT_PAGE_RECORDS_BYTE_BUDGET: usize = 900 * 1024;

#[derive(Serialize, Deserialize)]
struct ParticipantCursor {
    operation: String,
    root_id: String,
    last_reader_key: String,
}

pub(crate) async fn apply_watch_choice(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    project_id: &ProjectId,
    root_message_id: &MessageId,
    boundary: i64,
    watch: bool,
) -> Result<(), BoardError> {
    if watch {
        activate_watch(
            transaction,
            reader_key,
            project_id,
            root_message_id,
            boundary,
        )
        .await?;
    } else {
        sqlx::query!(
            "UPDATE thread_watches SET active=0 WHERE reader_key=? AND root_id=?",
            reader_key,
            root_message_id.as_str(),
        )
        .execute(&mut **transaction)
        .await
        .map_err(storage_error)?;
    }
    recompute_project_unread(transaction, reader_key, project_id.as_str()).await?;
    Ok(())
}

async fn insert_lifecycle_activity(
    transaction: &mut BoardTransaction<'_>,
    location: &crate::message_records::ThreadLocation,
    kind: &str,
    actor_key: &str,
) -> Result<i64, BoardError> {
    let sequence = allocate_activity_sequence(transaction).await?;
    sqlx::query!(
        "INSERT INTO board_activity(activity_sequence,project_id,board_id,topic_id,root_id,kind,actor_key,message_id) \
         VALUES(?,?,?,?,?,?,?,NULL)",
        sequence,
        location.project_id.as_str(),
        location.board_id.as_str(),
        location.topic_id.as_str(),
        location.root_message_id.as_str(),
        kind,
        actor_key,
    )
    .execute(&mut **transaction)
    .await
    .map_err(storage_error)?;
    Ok(sequence)
}

async fn upsert_joined_participant(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    root_message_id: &MessageId,
    role: ParticipantRole,
    note: Option<&ParticipantNote>,
    sequence: i64,
) -> Result<(), BoardError> {
    let role = role_name(role);
    let note = note.map(ParticipantNote::as_str);
    sqlx::query!(
        "INSERT INTO thread_participants(reader_key,root_id,role,note,joined_at_activity,last_seen_activity,closed_at_activity,closed_reason,replaced_by) \
         VALUES(?,?,?,?,?,?,NULL,NULL,NULL) \
         ON CONFLICT(reader_key,root_id) DO UPDATE SET role=excluded.role, \
           note=COALESCE(excluded.note,thread_participants.note), \
           joined_at_activity=excluded.joined_at_activity,last_seen_activity=excluded.last_seen_activity, \
           closed_at_activity=NULL,closed_reason=NULL,replaced_by=NULL",
        reader_key,
        root_message_id.as_str(),
        role,
        note,
        sequence,
        sequence,
    )
    .execute(&mut **transaction)
    .await
    .map_err(storage_error)?;
    Ok(())
}

impl BoardStore {
    pub async fn join_thread(
        &mut self,
        request: ThreadJoinRequest,
    ) -> Result<ThreadJoinResult, BoardError> {
        if request.replace.as_ref() == Some(&request.actor) {
            return Err(BoardError::participant_refusal(ParticipantRefusal {
                kind: BoardFailureKind::SelfReplace,
                message: "Replace must name a different current Orchestrator.".to_owned(),
                next_action: BoardNextAction::RepeatJoinWithoutReplace,
                root_message_id: request.root_message_id,
                actor: request.actor,
                holder: None,
                holder_last_seen_activity: None,
                target: None,
                named_holder: request.replace,
            }));
        }
        if request.replace.is_some() && request.role != ParticipantRole::Orchestrator {
            return Err(BoardError::invalid_field(
                "replace",
                "may be used only with Role orchestrator",
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
        let holder = load_orchestrator(&mut transaction, &request.root_message_id).await?;
        if request.role == ParticipantRole::Orchestrator {
            match (&holder, &request.replace) {
                (Some(holder), None) if holder.identity != request.actor => {
                    return Err(BoardError::orchestrator_already_exists(
                        request.root_message_id,
                        request.actor,
                        holder.identity.clone(),
                        holder.last_seen_activity,
                    ));
                }
                (Some(holder), Some(named)) if holder.identity != *named => {
                    return Err(BoardError::participant_refusal(ParticipantRefusal {
                        kind: BoardFailureKind::StaleOrchestrator,
                        message: "The named Orchestrator is stale. Inspect Participants and retry with the current holder.".to_owned(),
                        next_action: BoardNextAction::InspectParticipants,
                        root_message_id: request.root_message_id,
                        actor: request.actor,
                        holder: Some(holder.identity.clone()),
                        holder_last_seen_activity: Some(holder.last_seen_activity),
                        target: None,
                        named_holder: request.replace,
                    }));
                }
                (None, Some(_)) => {
                    return Err(BoardError::participant_refusal(ParticipantRefusal {
                        kind: BoardFailureKind::StaleOrchestrator,
                        message: "The named Orchestrator is stale because the Thread has no current Orchestrator.".to_owned(),
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
            "orchestratorReplaced"
        } else {
            "participantJoined"
        };
        let sequence =
            insert_lifecycle_activity(&mut transaction, &location, kind, &actor_key).await?;
        if let Some(holder) = &holder
            && request.replace.as_ref() == Some(&holder.identity)
        {
            let holder_key = identity_key(&holder.identity);
            sqlx::query!(
                "UPDATE thread_participants SET last_seen_activity=?,closed_at_activity=?,closed_reason='replaced',replaced_by=? \
                 WHERE reader_key=? AND root_id=? AND role='orchestrator' AND closed_at_activity IS NULL",
                sequence,
                sequence,
                actor_key,
                holder_key,
                request.root_message_id.as_str(),
            )
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?;
        }
        upsert_joined_participant(
            &mut transaction,
            &actor_key,
            &request.root_message_id,
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
        let participant =
            load_participant(&mut transaction, &request.actor, &request.root_message_id)
                .await?
                .ok_or_else(invalid_record)?;
        let orchestrator = load_orchestrator(&mut transaction, &request.root_message_id).await?;
        let watch_status =
            load_watch_status(&mut transaction, &actor_key, &request.root_message_id).await?;
        transaction.commit().await.map_err(storage_error)?;
        self.notify_activity();
        Ok(ThreadJoinResult {
            participant,
            orchestrator,
            watch_status,
            outcome: "Participant joined the Thread.".to_owned(),
        })
    }

    pub async fn leave_thread(
        &mut self,
        request: ThreadLeaveRequest,
    ) -> Result<ThreadLeaveResult, BoardError> {
        if request.resolve && request.to.is_some() {
            return Err(BoardError::invalid_field(
                "leave",
                "must state only one of to or resolve",
            ));
        }
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let location = require_thread(&mut transaction, &request.root_message_id).await?;
        if request.resolve
            && require_board(&mut transaction, &location.board_id)
                .await?
                .state
                == BoardState::Archived
        {
            return Err(archived_board());
        }
        let actor_key = ensure_identity(&mut transaction, &request.actor).await?;
        let participant =
            require_open_participant(&mut transaction, &request.actor, &request.root_message_id)
                .await?;
        if participant.role == ParticipantRole::Orchestrator
            && !request.resolve
            && request.to.is_none()
        {
            return Err(BoardError::participant_refusal(ParticipantRefusal {
                kind: BoardFailureKind::OrchestratorHandoverRequired,
                message: "An Orchestrator must Leave with a handover target or resolve the Thread."
                    .to_owned(),
                next_action: BoardNextAction::LeaveWithHandoverOrResolve,
                root_message_id: request.root_message_id,
                actor: request.actor,
                holder: Some(participant.identity),
                holder_last_seen_activity: Some(participant.last_seen_activity),
                target: None,
                named_holder: None,
            }));
        }
        if participant.role != ParticipantRole::Orchestrator
            && (request.resolve || request.to.is_some())
        {
            return Err(BoardError::invalid_field(
                "leave",
                "a non-Orchestrator must omit to and resolve",
            ));
        }
        if request.resolve {
            sqlx::query!(
                "UPDATE thread_watches SET active=0 WHERE reader_key=? AND root_id=?",
                actor_key,
                request.root_message_id.as_str(),
            )
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?;
        }
        let sequence = if request.resolve {
            resolve_in_transaction(&mut transaction, &location, &actor_key).await?
        } else if let Some(target) = &request.to {
            if *target == request.actor {
                return Err(BoardError::invalid_field(
                    "to",
                    "must name another Participant",
                ));
            }
            let target_participant =
                load_participant(&mut transaction, target, &request.root_message_id).await?;
            let Some(target_participant) = target_participant.filter(Participant::is_open) else {
                return Err(BoardError::participant_refusal(ParticipantRefusal {
                    kind: BoardFailureKind::HandoverTargetNotParticipant,
                    message: "The handover target must Join the Thread first.".to_owned(),
                    next_action: BoardNextAction::JoinHandoverTarget,
                    root_message_id: request.root_message_id,
                    actor: request.actor,
                    holder: Some(participant.identity),
                    holder_last_seen_activity: Some(participant.last_seen_activity),
                    target: Some(target.clone()),
                    named_holder: None,
                }));
            };
            let sequence = insert_lifecycle_activity(
                &mut transaction,
                &location,
                "orchestratorReplaced",
                &actor_key,
            )
            .await?;
            let target_key = identity_key(&target_participant.identity);
            sqlx::query!(
                "UPDATE thread_participants SET last_seen_activity=?,closed_at_activity=?,closed_reason='replaced',replaced_by=? WHERE reader_key=? AND root_id=? AND closed_at_activity IS NULL",
                sequence,
                sequence,
                target_key,
                actor_key,
                request.root_message_id.as_str(),
            )
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?;
            sqlx::query!(
                "UPDATE thread_participants SET role='orchestrator',last_seen_activity=? WHERE reader_key=? AND root_id=? AND closed_at_activity IS NULL",
                sequence,
                target_key,
                request.root_message_id.as_str(),
            )
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?;
            sequence
        } else {
            let sequence = insert_lifecycle_activity(
                &mut transaction,
                &location,
                "participantLeft",
                &actor_key,
            )
            .await?;
            sqlx::query!(
                "UPDATE thread_participants SET last_seen_activity=?,closed_at_activity=?,closed_reason='left',replaced_by=NULL WHERE reader_key=? AND root_id=? AND closed_at_activity IS NULL",
                sequence,
                sequence,
                actor_key,
                request.root_message_id.as_str(),
            )
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?;
            sequence
        };
        if !request.resolve {
            sqlx::query!(
                "UPDATE thread_watches SET active=0 WHERE reader_key=? AND root_id=?",
                actor_key,
                request.root_message_id.as_str(),
            )
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?;
            recompute_project_unread(&mut transaction, &actor_key, location.project_id.as_str())
                .await?;
        }
        let participant =
            load_participant(&mut transaction, &request.actor, &request.root_message_id)
                .await?
                .ok_or_else(invalid_record)?;
        let orchestrator = load_orchestrator(&mut transaction, &request.root_message_id).await?;
        let watch_status =
            load_watch_status(&mut transaction, &actor_key, &request.root_message_id).await?;
        let state = if request.resolve {
            ThreadState::Resolved
        } else {
            location.state
        };
        transaction.commit().await.map_err(storage_error)?;
        self.notify_activity();
        Ok(ThreadLeaveResult {
            participant,
            thread: Thread {
                root_message_id: request.root_message_id,
                state,
                orchestrator: orchestrator.clone(),
            },
            orchestrator,
            watch_status,
            outcome: format!("Participant left the Thread at Activity {sequence}."),
        })
    }

    pub async fn list_thread_participants(
        &mut self,
        request: ThreadParticipantListRequest,
    ) -> Result<ThreadParticipantListResult, BoardError> {
        let last_reader_key = decode_participant_cursor(
            &self.cursor_key,
            request.page.cursor.as_deref(),
            &request.root_message_id,
        )?;
        let mut transaction = self.connection.begin().await.map_err(storage_error)?;
        require_thread(&mut transaction, &request.root_message_id).await?;
        let query_limit = i64::from(request.page.limit.get()) + 1;
        let rows = sqlx::query_as!(
            StoredParticipantRow,
            "SELECT reader_key,root_id,role,note,joined_at_activity,last_seen_activity,closed_at_activity,closed_reason,replaced_by \
             FROM thread_participants WHERE root_id=? AND reader_key>? ORDER BY reader_key LIMIT ?",
            request.root_message_id.as_str(),
            last_reader_key,
            query_limit,
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let mut has_more = rows.len() > request.page.limit.get() as usize;
        let mut records = Vec::with_capacity(request.page.limit.get() as usize);
        let mut encoded_bytes = 2_usize;
        let mut next_reader_key = last_reader_key;
        for row in rows.into_iter().take(request.page.limit.get() as usize) {
            let reader_key = row.reader_key.clone();
            let participant = decode_participant(&mut transaction, row).await?;
            let record_bytes = serde_json::to_vec(&participant)
                .map_err(|_| invalid_record())?
                .len();
            let separator_bytes = usize::from(!records.is_empty());
            if encoded_bytes + separator_bytes + record_bytes > PARTICIPANT_PAGE_RECORDS_BYTE_BUDGET
            {
                if records.is_empty() {
                    return Err(BoardError::invalid_record(ResourceIdentity::Thread {
                        root_message_id: request.root_message_id,
                    }));
                }
                has_more = true;
                break;
            }
            encoded_bytes += separator_bytes + record_bytes;
            next_reader_key = reader_key;
            records.push(participant);
        }
        let next_cursor = has_more
            .then(|| {
                encode_cursor(
                    &self.cursor_key,
                    &ParticipantCursor {
                        operation: "threadParticipants".to_owned(),
                        root_id: request.root_message_id.as_str().to_owned(),
                        last_reader_key: next_reader_key,
                    },
                )
            })
            .transpose()?;
        let orchestrator = load_orchestrator(&mut transaction, &request.root_message_id).await?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(ThreadParticipantListResult {
            page: Page {
                records,
                next_cursor,
            },
            orchestrator,
        })
    }
}

pub(crate) async fn resolve_in_transaction(
    transaction: &mut BoardTransaction<'_>,
    location: &crate::message_records::ThreadLocation,
    actor_key: &str,
) -> Result<i64, BoardError> {
    sqlx::query!(
        "UPDATE board_threads SET state='resolved' WHERE root_id=?",
        location.root_message_id.as_str(),
    )
    .execute(&mut **transaction)
    .await
    .map_err(storage_error)?;
    let sequence =
        insert_lifecycle_activity(transaction, location, "threadResolved", actor_key).await?;
    sqlx::query!(
        "UPDATE thread_participants SET last_seen_activity=?,closed_at_activity=?,closed_reason='resolved',replaced_by=NULL \
         WHERE root_id=? AND closed_at_activity IS NULL",
        sequence,
        sequence,
        location.root_message_id.as_str(),
    )
    .execute(&mut **transaction)
    .await
    .map_err(storage_error)?;
    sqlx::query!(
        "UPDATE project_reader_state SET has_unread=1 \
         WHERE project_id=? AND reader_key<>? AND reader_key IN ( \
           SELECT reader_key FROM thread_watches \
           WHERE root_id=? AND active=1 AND starts_after_activity<?)",
        location.project_id.as_str(),
        actor_key,
        location.root_message_id.as_str(),
        sequence,
    )
    .execute(&mut **transaction)
    .await
    .map_err(storage_error)?;
    recompute_project_unread(transaction, actor_key, location.project_id.as_str()).await?;
    Ok(sequence)
}

pub(crate) async fn advance_participant_last_seen(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    root_message_id: &MessageId,
    sequence: i64,
) -> Result<(), BoardError> {
    sqlx::query!(
        "UPDATE thread_participants SET last_seen_activity=MAX(last_seen_activity,?) \
         WHERE reader_key=? AND root_id=? AND closed_at_activity IS NULL",
        sequence,
        reader_key,
        root_message_id.as_str(),
    )
    .execute(&mut **transaction)
    .await
    .map_err(storage_error)?;
    Ok(())
}

fn decode_participant_cursor(
    cursor_key: &[u8; 32],
    cursor: Option<&str>,
    root_message_id: &MessageId,
) -> Result<String, BoardError> {
    let Some(cursor) = cursor else {
        return Ok(String::new());
    };
    let cursor: ParticipantCursor = decode_signed_cursor(cursor_key, cursor)?;
    if cursor.operation != "threadParticipants" || cursor.root_id != root_message_id.as_str() {
        return Err(invalid_cursor());
    }
    Ok(cursor.last_reader_key)
}
