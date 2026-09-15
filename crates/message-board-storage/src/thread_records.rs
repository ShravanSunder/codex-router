//! Thread lifecycle, watch membership, and thread discovery.
use crate::BoardStore;
use crate::board_topic_records::require_board;
use crate::message_records::{
    activate_watch, activity_sequence, load_watch_status, require_thread,
};
use crate::participant_records::resolve_in_transaction;
use crate::participant_row_decoding::{
    StoredParticipantRow, decode_projected_participant, load_orchestrator, load_participant,
};
use crate::storage_support::{
    StoredIdentityRow, allocate_activity_sequence, archived_board, current_activity_sequence,
    decode_cursor as decode_signed_cursor, decode_identity, encode_cursor, ensure_identity,
    invalid_cursor, invalid_record, recompute_project_unread, storage_error,
};
use message_board::*;
use serde::{Deserialize, Serialize};
use sqlx::Connection;

const THREAD_PAGE_RECORDS_BYTE_BUDGET: usize = 900 * 1024;

#[derive(Serialize, Deserialize)]
struct ThreadCursor {
    operation: String,
    project_id: String,
    reader_key: String,
    watched_only: bool,
    last_root: String,
}

struct StoredThreadRow {
    root_id: String,
    state: String,
    orchestrator_reader_key: Option<String>,
    orchestrator_root_id: Option<String>,
    orchestrator_role: Option<String>,
    orchestrator_note: Option<String>,
    orchestrator_joined_at_activity: Option<i64>,
    orchestrator_last_seen_activity: Option<i64>,
    orchestrator_closed_at_activity: Option<i64>,
    orchestrator_closed_reason: Option<String>,
    orchestrator_replaced_by: Option<String>,
    orchestrator_identity_key: Option<String>,
    orchestrator_identity_kind: Option<String>,
    orchestrator_identity_service_id: Option<String>,
    orchestrator_identity_endpoint_id: Option<String>,
    orchestrator_identity_session_id: Option<String>,
    orchestrator_identity_human_id: Option<String>,
    orchestrator_joined_activity_on_thread: i64,
    orchestrator_last_seen_activity_on_thread: i64,
    orchestrator_closed_activity_on_thread: i64,
}

impl BoardStore {
    pub async fn show_thread(
        &mut self,
        request: ThreadShowRequest,
    ) -> Result<ThreadShowResult, BoardError> {
        let mut transaction = self.connection.begin().await.map_err(storage_error)?;
        let location = require_thread(&mut transaction, &request.root_message_id).await?;
        let watch_status = match request.reader {
            None => None,
            Some(reader) => {
                let key = crate::storage_support::identity_key(&reader);
                Some(load_watch_status(&mut transaction, &key, &request.root_message_id).await?)
            }
        };
        let orchestrator = load_orchestrator(&mut transaction, &request.root_message_id).await?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(ThreadShowResult {
            thread: Thread {
                root_message_id: location.root_message_id,
                state: location.state,
                orchestrator,
            },
            watch_status,
        })
    }

    pub async fn resolve_thread(
        &mut self,
        request: ThreadResolveRequest,
    ) -> Result<ThreadResolveResult, BoardError> {
        let (thread, sequence, changed) = set_thread_state(
            &mut self.connection,
            &request.root_message_id,
            &request.actor,
            ThreadState::Resolved,
        )
        .await?;
        if changed {
            self.notify_activity();
        }
        Ok(ThreadResolveResult {
            thread,
            activity_sequence: sequence,
            outcome: if changed {
                "Thread resolved."
            } else {
                "Thread was already resolved."
            }
            .to_owned(),
        })
    }

    pub async fn unresolve_thread(
        &mut self,
        request: ThreadUnresolveRequest,
    ) -> Result<ThreadUnresolveResult, BoardError> {
        let (thread, sequence, changed) = set_thread_state(
            &mut self.connection,
            &request.root_message_id,
            &request.actor,
            ThreadState::Unresolved,
        )
        .await?;
        if changed {
            self.notify_activity();
        }
        Ok(ThreadUnresolveResult {
            thread,
            activity_sequence: sequence,
            outcome: if changed {
                "Thread marked unresolved; thread messages can be added while the board is active."
            } else {
                "Thread was already unresolved; thread messages can be added while the board is active."
            }
            .to_owned(),
        })
    }

    pub async fn watch_thread(
        &mut self,
        request: ThreadWatchRequest,
    ) -> Result<ThreadWatchResult, BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let location = require_thread(&mut transaction, &request.root_message_id).await?;
        let reader_key = ensure_identity(&mut transaction, &request.actor).await?;
        let boundary = current_activity_sequence(&mut transaction).await?;
        activate_watch(
            &mut transaction,
            &reader_key,
            &location.project_id,
            &request.root_message_id,
            boundary,
        )
        .await?;
        recompute_project_unread(&mut transaction, &reader_key, location.project_id.as_str())
            .await?;
        let watch_status =
            load_watch_status(&mut transaction, &reader_key, &request.root_message_id).await?;
        let outcome = format!(
            "Thread watch is active for future activity. {}",
            watch_status.message
        );
        let orchestrator = load_orchestrator(&mut transaction, &request.root_message_id).await?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(ThreadWatchResult {
            thread: Thread {
                root_message_id: request.root_message_id,
                state: location.state,
                orchestrator,
            },
            watch_status,
            outcome,
        })
    }

    pub async fn unwatch_thread(
        &mut self,
        request: ThreadUnwatchRequest,
    ) -> Result<ThreadUnwatchResult, BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let location = require_thread(&mut transaction, &request.root_message_id).await?;
        let reader_key = ensure_identity(&mut transaction, &request.actor).await?;
        sqlx::query!(
            "UPDATE thread_watches SET active=0 WHERE reader_key=? AND root_id=?",
            reader_key,
            request.root_message_id.as_str(),
        )
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        recompute_project_unread(&mut transaction, &reader_key, location.project_id.as_str())
            .await?;
        let watch_status =
            load_watch_status(&mut transaction, &reader_key, &request.root_message_id).await?;
        let outcome = format!("Thread watch is inactive. {}", watch_status.message);
        let orchestrator = load_orchestrator(&mut transaction, &request.root_message_id).await?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(ThreadUnwatchResult {
            thread: Thread {
                root_message_id: request.root_message_id,
                state: location.state,
                orchestrator,
            },
            watch_status,
            outcome,
        })
    }

    pub async fn list_threads(
        &mut self,
        request: ThreadListRequest,
    ) -> Result<ThreadListResult, BoardError> {
        let reader_key = crate::storage_support::identity_key(&request.reader);
        let last_root = decode_cursor(
            &self.cursor_key,
            request.page.cursor.as_deref(),
            &request.project_id,
            &reader_key,
            request.watched_only,
        )?;
        let mut transaction = self.connection.begin().await.map_err(storage_error)?;
        crate::project_records::require_project(&mut transaction, &request.project_id).await?;
        let query_limit = i64::from(request.page.limit.get()) + 1;
        let rows = sqlx::query_as!(
            StoredThreadRow,
            "SELECT t.root_id,t.state, \
                    p.reader_key AS orchestrator_reader_key,p.root_id AS orchestrator_root_id, \
                    p.role AS orchestrator_role,p.note AS orchestrator_note, \
                    p.joined_at_activity AS orchestrator_joined_at_activity, \
                    p.last_seen_activity AS orchestrator_last_seen_activity, \
                    p.closed_at_activity AS orchestrator_closed_at_activity, \
                    p.closed_reason AS orchestrator_closed_reason,p.replaced_by AS orchestrator_replaced_by, \
                    i.identity_key AS orchestrator_identity_key,i.kind AS orchestrator_identity_kind, \
                    i.service_id AS orchestrator_identity_service_id,i.endpoint_id AS orchestrator_identity_endpoint_id, \
                    i.session_id AS orchestrator_identity_session_id,i.human_id AS orchestrator_identity_human_id, \
                    EXISTS(SELECT 1 FROM board_activity joined_activity \
                      WHERE joined_activity.activity_sequence=p.joined_at_activity AND joined_activity.root_id=p.root_id) \
                      AS orchestrator_joined_activity_on_thread, \
                    EXISTS(SELECT 1 FROM board_activity last_seen_activity \
                      WHERE last_seen_activity.activity_sequence=p.last_seen_activity AND last_seen_activity.root_id=p.root_id) \
                      AS orchestrator_last_seen_activity_on_thread, \
                    EXISTS(SELECT 1 FROM board_activity closed_activity \
                      WHERE closed_activity.activity_sequence=p.closed_at_activity AND closed_activity.root_id=p.root_id) \
                      AS orchestrator_closed_activity_on_thread \
             FROM board_threads t \
             JOIN board_messages m ON m.message_id=t.root_id \
             JOIN project_boards b ON b.board_id=m.board_id \
             LEFT JOIN thread_watches w ON w.root_id=t.root_id AND w.reader_key=? \
             LEFT JOIN thread_participants p ON p.root_id=t.root_id AND p.closed_at_activity IS NULL \
                 AND (p.role='orchestrator' OR p.role NOT IN ('advisor','reviewer','participant')) \
             LEFT JOIN board_identities i ON i.identity_key=p.reader_key \
             WHERE b.project_id=? AND t.root_id>? AND (?=0 OR w.active=1) \
             ORDER BY t.root_id LIMIT ?",
            reader_key,
            request.project_id.as_str(),
            last_root,
            request.watched_only,
            query_limit,
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let mut records = Vec::with_capacity(request.page.limit.get() as usize);
        let mut encoded_bytes = 2_usize;
        let mut has_more = false;
        for row in rows {
            if records.len() == request.page.limit.get() as usize {
                has_more = true;
                break;
            }
            let thread = decode_thread(row)?;
            let thread_bytes = serde_json::to_vec(&thread)
                .map_err(|_| invalid_record())?
                .len();
            let separator_bytes = usize::from(!records.is_empty());
            if encoded_bytes + separator_bytes + thread_bytes > THREAD_PAGE_RECORDS_BYTE_BUDGET {
                if records.is_empty() {
                    return Err(invalid_record());
                }
                has_more = true;
                break;
            }
            encoded_bytes += separator_bytes + thread_bytes;
            records.push(thread);
        }
        let next_cursor = if has_more {
            let last = records
                .last()
                .ok_or_else(invalid_record)?
                .root_message_id
                .as_str()
                .to_owned();
            Some(encode_cursor(
                &self.cursor_key,
                &ThreadCursor {
                    operation: "threads".to_owned(),
                    project_id: request.project_id.as_str().to_owned(),
                    reader_key: reader_key.clone(),
                    watched_only: request.watched_only,
                    last_root: last,
                },
            )?)
        } else {
            None
        };
        transaction.commit().await.map_err(storage_error)?;
        Ok(ThreadListResult {
            page: Page {
                records,
                next_cursor,
            },
        })
    }
}

async fn set_thread_state(
    connection: &mut sqlx::SqliteConnection,
    root: &MessageId,
    actor: &Identity,
    requested: ThreadState,
) -> Result<(Thread, Option<ActivitySequence>, bool), BoardError> {
    let mut transaction = connection
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(storage_error)?;
    let location = require_thread(&mut transaction, root).await?;
    if require_board(&mut transaction, &location.board_id)
        .await?
        .state
        == BoardState::Archived
    {
        return Err(archived_board());
    }
    let actor_key = ensure_identity(&mut transaction, actor).await?;
    if requested == ThreadState::Resolved && matches!(actor, Identity::Session { .. }) {
        let participant = load_participant(&mut transaction, actor, root).await?;
        if participant
            .as_ref()
            .is_none_or(|participant| !participant.is_open())
        {
            return Err(BoardError::participant_required(
                root.clone(),
                actor.clone(),
            ));
        }
        let holder = load_orchestrator(&mut transaction, root).await?;
        if holder
            .as_ref()
            .is_none_or(|holder| holder.identity != *actor)
        {
            return Err(BoardError::participant_refusal(ParticipantRefusal {
                kind: BoardFailureKind::OrchestratorRequired,
                message:
                    "Only the current Orchestrator may resolve this Thread for a session identity."
                        .to_owned(),
                next_action: BoardNextAction::InspectParticipants,
                root_message_id: root.clone(),
                actor: actor.clone(),
                holder: holder.as_ref().map(|holder| holder.identity.clone()),
                holder_last_seen_activity: holder.as_ref().map(|holder| holder.last_seen_activity),
                target: None,
                named_holder: None,
            }));
        }
    }
    if location.state == requested {
        let orchestrator = load_orchestrator(&mut transaction, root).await?;
        transaction.commit().await.map_err(storage_error)?;
        return Ok((
            Thread {
                root_message_id: root.clone(),
                state: requested,
                orchestrator,
            },
            None,
            false,
        ));
    }
    let sequence = match requested {
        ThreadState::Resolved => {
            resolve_in_transaction(&mut transaction, &location, &actor_key).await?
        }
        ThreadState::Unresolved => {
            sqlx::query!(
                "UPDATE board_threads SET state='unresolved' WHERE root_id=?",
                root.as_str(),
            )
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?;
            let sequence = allocate_activity_sequence(&mut transaction).await?;
            sqlx::query!(
                "INSERT INTO board_activity(activity_sequence,project_id,board_id,topic_id,root_id,kind,actor_key,message_id) \
                 VALUES(?,?,?,?,?,'threadUnresolved',?,NULL)",
                sequence,
                location.project_id.as_str(),
                location.board_id.as_str(),
                location.topic_id.as_str(),
                root.as_str(),
                actor_key,
            )
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?;
            sequence
        }
    };
    if requested == ThreadState::Unresolved {
        sqlx::query!(
            "UPDATE project_reader_state SET has_unread=1 \
             WHERE project_id=? AND reader_key<>? AND reader_key IN ( \
               SELECT reader_key FROM thread_watches \
               WHERE root_id=? AND active=1 AND starts_after_activity<?)",
            location.project_id.as_str(),
            actor_key,
            root.as_str(),
            sequence,
        )
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        recompute_project_unread(&mut transaction, &actor_key, location.project_id.as_str())
            .await?;
    }
    let orchestrator = load_orchestrator(&mut transaction, root).await?;
    transaction.commit().await.map_err(storage_error)?;
    Ok((
        Thread {
            root_message_id: root.clone(),
            state: requested,
            orchestrator,
        },
        Some(activity_sequence(sequence)?),
        true,
    ))
}

fn decode_thread(row: StoredThreadRow) -> Result<Thread, BoardError> {
    let root_message_id = MessageId::try_from(row.root_id.clone()).map_err(|_| invalid_record())?;
    let state = match row.state.as_str() {
        "unresolved" => ThreadState::Unresolved,
        "resolved" => ThreadState::Resolved,
        _ => return Err(invalid_record()),
    };
    let orchestrator = decode_projected_orchestrator(&root_message_id, row)?;
    Ok(Thread {
        root_message_id,
        state,
        orchestrator,
    })
}

fn decode_projected_orchestrator(
    thread_root_message_id: &MessageId,
    row: StoredThreadRow,
) -> Result<Option<OrchestratorHolder>, BoardError> {
    let Some(reader_key) = row.orchestrator_reader_key else {
        if row.orchestrator_root_id.is_some()
            || row.orchestrator_role.is_some()
            || row.orchestrator_note.is_some()
            || row.orchestrator_joined_at_activity.is_some()
            || row.orchestrator_last_seen_activity.is_some()
            || row.orchestrator_closed_at_activity.is_some()
            || row.orchestrator_closed_reason.is_some()
            || row.orchestrator_replaced_by.is_some()
            || row.orchestrator_identity_key.is_some()
            || row.orchestrator_identity_kind.is_some()
            || row.orchestrator_identity_service_id.is_some()
            || row.orchestrator_identity_endpoint_id.is_some()
            || row.orchestrator_identity_session_id.is_some()
            || row.orchestrator_identity_human_id.is_some()
        {
            return Err(invalid_record());
        }
        return Ok(None);
    };
    let participant_root_id = row.orchestrator_root_id.ok_or_else(invalid_record)?;
    let participant_root_message_id =
        MessageId::try_from(participant_root_id).map_err(|_| invalid_record())?;
    if participant_root_message_id != *thread_root_message_id {
        return Err(invalid_record());
    }
    let identity = decode_identity(&StoredIdentityRow {
        identity_key: row.orchestrator_identity_key.ok_or_else(invalid_record)?,
        kind: row.orchestrator_identity_kind.ok_or_else(invalid_record)?,
        service_id: row.orchestrator_identity_service_id,
        endpoint_id: row.orchestrator_identity_endpoint_id,
        session_id: row.orchestrator_identity_session_id,
        human_id: row.orchestrator_identity_human_id,
    })?;
    let participant = decode_projected_participant(
        StoredParticipantRow {
            reader_key,
            root_id: participant_root_message_id.as_str().to_owned(),
            role: row.orchestrator_role.ok_or_else(invalid_record)?,
            note: row.orchestrator_note,
            joined_at_activity: row
                .orchestrator_joined_at_activity
                .ok_or_else(invalid_record)?,
            last_seen_activity: row
                .orchestrator_last_seen_activity
                .ok_or_else(invalid_record)?,
            closed_at_activity: row.orchestrator_closed_at_activity,
            closed_reason: row.orchestrator_closed_reason,
            replaced_by: row.orchestrator_replaced_by,
        },
        identity,
        row.orchestrator_joined_activity_on_thread != 0,
        row.orchestrator_last_seen_activity_on_thread != 0,
        row.orchestrator_closed_activity_on_thread != 0,
    )?;
    if participant.role != ParticipantRole::Orchestrator || !participant.is_open() {
        return Err(invalid_record());
    }
    Ok(Some(OrchestratorHolder {
        identity: participant.identity,
        last_seen_activity: participant.last_seen_activity,
    }))
}
fn decode_cursor(
    cursor_key: &[u8; 32],
    cursor: Option<&str>,
    project: &ProjectId,
    reader_key: &str,
    watched_only: bool,
) -> Result<String, BoardError> {
    let Some(cursor) = cursor else {
        return Ok(String::new());
    };
    let cursor: ThreadCursor = decode_signed_cursor(cursor_key, cursor)?;
    if cursor.operation != "threads"
        || cursor.project_id != project.as_str()
        || cursor.reader_key != reader_key
        || cursor.watched_only != watched_only
    {
        return Err(invalid_cursor());
    }
    Ok(cursor.last_root)
}
