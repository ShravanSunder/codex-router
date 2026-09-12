//! Thread lifecycle, watch membership, and thread discovery.
use crate::BoardStore;
use crate::board_topic_records::require_board;
use crate::message_records::{
    activate_watch, activity_sequence, load_watch_status, require_thread,
};
use crate::storage_support::{
    allocate_activity_sequence, archived_board, current_activity_sequence,
    decode_cursor as decode_signed_cursor, encode_cursor, ensure_identity, invalid_cursor,
    invalid_record, recompute_project_unread, storage_error,
};
use project_board::*;
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
        transaction.commit().await.map_err(storage_error)?;
        Ok(ThreadShowResult {
            thread: Thread {
                root_message_id: location.root_message_id,
                state: location.state,
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
        transaction.commit().await.map_err(storage_error)?;
        Ok(ThreadWatchResult {
            thread: Thread {
                root_message_id: request.root_message_id,
                state: location.state,
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
        transaction.commit().await.map_err(storage_error)?;
        Ok(ThreadUnwatchResult {
            thread: Thread {
                root_message_id: request.root_message_id,
                state: location.state,
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
            "SELECT t.root_id,t.state \
             FROM board_threads t \
             JOIN board_messages m ON m.message_id=t.root_id \
             JOIN project_boards b ON b.board_id=m.board_id \
             LEFT JOIN thread_watches w ON w.root_id=t.root_id AND w.reader_key=? \
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
        let records = rows
            .into_iter()
            .map(decode_thread)
            .collect::<Result<Vec<_>, _>>()?;
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
    if location.state == requested {
        transaction.commit().await.map_err(storage_error)?;
        return Ok((
            Thread {
                root_message_id: root.clone(),
                state: requested,
            },
            None,
            false,
        ));
    }
    let state = match requested {
        ThreadState::Resolved => "resolved",
        ThreadState::Unresolved => "unresolved",
    };
    sqlx::query!(
        "UPDATE board_threads SET state=? WHERE root_id=?",
        state,
        root.as_str(),
    )
    .execute(&mut *transaction)
    .await
    .map_err(storage_error)?;
    let sequence = allocate_activity_sequence(&mut transaction).await?;
    let kind = match requested {
        ThreadState::Resolved => "threadResolved",
        ThreadState::Unresolved => "threadUnresolved",
    };
    sqlx::query!(
        "INSERT INTO board_activity(activity_sequence,project_id,board_id,topic_id,root_id,kind,actor_key,message_id) \
         VALUES(?,?,?,?,?,?,?,NULL)",
        sequence,
        location.project_id.as_str(),
        location.board_id.as_str(),
        location.topic_id.as_str(),
        root.as_str(),
        kind,
        actor_key,
    )
    .execute(&mut *transaction)
    .await
    .map_err(storage_error)?;
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
    transaction.commit().await.map_err(storage_error)?;
    Ok((
        Thread {
            root_message_id: root.clone(),
            state: requested,
        },
        Some(activity_sequence(sequence)?),
        true,
    ))
}

fn decode_thread(row: StoredThreadRow) -> Result<Thread, BoardError> {
    Ok(Thread {
        root_message_id: MessageId::try_from(row.root_id).map_err(|_| invalid_record())?,
        state: match row.state.as_str() {
            "unresolved" => ThreadState::Unresolved,
            "resolved" => ThreadState::Resolved,
            _ => return Err(invalid_record()),
        },
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
