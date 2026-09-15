//! Thread lifecycle, watch membership, and thread discovery.
use crate::BoardStore;
use crate::board_topic_records::require_board;
use crate::message_records::{
    activate_watch, activity_sequence, load_watch_status, require_thread,
};
use crate::participant_records::resolve_in_transaction;
use crate::participant_row_decoding::{load_orchestrator, load_participant};
use crate::storage_support::{
    allocate_activity_sequence, archived_board, current_activity_sequence,
    decode_cursor as decode_signed_cursor, encode_cursor, ensure_identity, invalid_cursor,
    invalid_record, recompute_project_unread, storage_error,
};
use message_board::*;
use serde::{Deserialize, Serialize};
use sqlx::Connection;

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
    orchestrator_key: Option<String>,
    orchestrator_last_seen: Option<i64>,
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
            "SELECT t.root_id,t.state,p.reader_key AS orchestrator_key,p.last_seen_activity AS orchestrator_last_seen \
             FROM board_threads t \
             JOIN board_messages m ON m.message_id=t.root_id \
             JOIN project_boards b ON b.board_id=m.board_id \
             LEFT JOIN thread_watches w ON w.root_id=t.root_id AND w.reader_key=? \
             LEFT JOIN thread_participants p ON p.root_id=t.root_id AND p.role='orchestrator' AND p.closed_at_activity IS NULL \
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
        let has_more = rows.len() > request.page.limit.get() as usize;
        let rows = rows
            .into_iter()
            .take(request.page.limit.get() as usize)
            .collect::<Vec<_>>();
        let next_cursor = if has_more {
            let last = rows.last().ok_or_else(invalid_record)?.root_id.clone();
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
        let mut records = Vec::with_capacity(rows.len());
        for row in rows {
            records.push(decode_thread(&mut transaction, row).await?);
        }
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
    recompute_project_unread(&mut transaction, &actor_key, location.project_id.as_str()).await?;
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

async fn decode_thread(
    transaction: &mut crate::storage_support::BoardTransaction<'_>,
    row: StoredThreadRow,
) -> Result<Thread, BoardError> {
    let orchestrator = match (row.orchestrator_key, row.orchestrator_last_seen) {
        (None, None) => None,
        (Some(key), Some(sequence)) => Some(OrchestratorHolder {
            identity: crate::message_records::load_identity(transaction, &key).await?,
            last_seen_activity: activity_sequence(sequence)?,
        }),
        _ => return Err(invalid_record()),
    };
    Ok(Thread {
        root_message_id: MessageId::try_from(row.root_id).map_err(|_| invalid_record())?,
        state: match row.state.as_str() {
            "unresolved" => ThreadState::Unresolved,
            "resolved" => ThreadState::Resolved,
            _ => return Err(invalid_record()),
        },
        orchestrator,
    })
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
