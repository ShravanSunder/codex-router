//! Shared typed message and thread record access.
use crate::storage_support::{
    BoardTransaction, StoredIdentityRow, attribute_invalid_record, decode_identity,
    ensure_project_reader_state, invalid_record, resource_not_found, storage_error,
};
use project_board::*;

pub(crate) struct ThreadLocation {
    pub project_id: ProjectId,
    pub board_id: BoardId,
    pub topic_id: TopicId,
    pub root_message_id: MessageId,
    pub state: ThreadState,
}

struct ThreadLocationRow {
    project_id: String,
    board_id: String,
    topic_id: String,
    message_id: String,
    state: String,
    root_id: Option<String>,
}

async fn require_thread_unattributed(
    transaction: &mut BoardTransaction<'_>,
    root_message_id: &MessageId,
) -> Result<ThreadLocation, BoardError> {
    let row = sqlx::query_as!(
        ThreadLocationRow,
        "SELECT b.project_id,m.board_id,m.topic_id,m.message_id,t.state,m.root_id \
         FROM board_threads t \
         JOIN board_messages m ON m.message_id=t.root_id \
         JOIN project_boards b ON b.board_id=m.board_id \
         WHERE t.root_id=?",
        root_message_id.as_str(),
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage_error)?
    .ok_or_else(|| {
        resource_not_found(ResourceIdentity::Thread {
            root_message_id: root_message_id.clone(),
        })
    })?;
    if row.root_id.is_some() {
        return Err(invalid_record());
    }
    Ok(ThreadLocation {
        project_id: ProjectId::try_from(row.project_id).map_err(|_| invalid_record())?,
        board_id: BoardId::try_from(row.board_id).map_err(|_| invalid_record())?,
        topic_id: TopicId::try_from(row.topic_id).map_err(|_| invalid_record())?,
        root_message_id: MessageId::try_from(row.message_id).map_err(|_| invalid_record())?,
        state: decode_thread_state(row.state)?,
    })
}

pub(crate) async fn require_thread(
    transaction: &mut BoardTransaction<'_>,
    root_message_id: &MessageId,
) -> Result<ThreadLocation, BoardError> {
    require_thread_unattributed(transaction, root_message_id)
        .await
        .map_err(|error| {
            attribute_invalid_record(
                error,
                ResourceIdentity::Thread {
                    root_message_id: root_message_id.clone(),
                },
            )
        })
}

pub(crate) async fn load_message(
    transaction: &mut BoardTransaction<'_>,
    message_id: &MessageId,
) -> Result<Message, BoardError> {
    crate::message_row_decoding::load_message(transaction, message_id).await
}

pub(crate) async fn load_identity(
    transaction: &mut BoardTransaction<'_>,
    identity_key: &str,
) -> Result<Identity, BoardError> {
    let row = sqlx::query_as!(
        StoredIdentityRow,
        "SELECT identity_key,kind,service_id,endpoint_id,session_id,human_id \
         FROM board_identities WHERE identity_key=?",
        identity_key,
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(storage_error)?;
    decode_identity(&row)
}

pub(crate) async fn activate_watch(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    project_id: &ProjectId,
    root_message_id: &MessageId,
    boundary: i64,
) -> Result<(), BoardError> {
    ensure_project_reader_state(transaction, reader_key, project_id.as_str()).await?;
    sqlx::query!(
        "INSERT INTO thread_watches(reader_key,root_id,active,starts_after_activity) \
         VALUES(?,?,1,?) \
         ON CONFLICT(reader_key,root_id) DO UPDATE SET \
           active=1, \
           starts_after_activity=CASE WHEN thread_watches.active=1 \
             THEN thread_watches.starts_after_activity ELSE excluded.starts_after_activity END",
        reader_key,
        root_message_id.as_str(),
        boundary,
    )
    .execute(&mut **transaction)
    .await
    .map_err(storage_error)?;
    Ok(())
}

pub(crate) async fn load_watch_status(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    root_message_id: &MessageId,
) -> Result<WatchStatus, BoardError> {
    let row = sqlx::query!(
        "SELECT active,starts_after_activity \
         FROM thread_watches WHERE reader_key=? AND root_id=?",
        reader_key,
        root_message_id.as_str(),
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage_error)?;
    let Some(row) = row else {
        return Ok(WatchStatus {
            watching: false,
            starts_after_activity_sequence: None,
            earlier_unwatched_range: None,
            message: "This thread is not watched. Watch it to receive future activity.".to_owned(),
        });
    };
    if row.active != 0 && row.active != 1 {
        return Err(invalid_record());
    }
    let boundary_sequence = activity_sequence(row.starts_after_activity)?;
    let earlier_unwatched_range = (row.starts_after_activity > 0).then(|| HistoryRange {
        scope: MessageListScope::Thread {
            root_message_id: root_message_id.clone(),
        },
        selection: MessageSelection::Range {
            from_activity_sequence: ActivitySequence::ZERO,
            to_activity_sequence: boundary_sequence,
        },
    });
    Ok(WatchStatus {
        watching: row.active == 1,
        starts_after_activity_sequence: Some(boundary_sequence),
        earlier_unwatched_range,
        message: if row.active == 1 {
            "Watching future thread activity."
        } else {
            "This thread is not watched. Watch it to receive future activity."
        }
        .to_owned(),
    })
}

pub(crate) fn activity_sequence(value: i64) -> Result<ActivitySequence, BoardError> {
    u64::try_from(value)
        .ok()
        .and_then(|value| ActivitySequence::try_from(value).ok())
        .ok_or_else(invalid_record)
}

pub(crate) fn decode_thread_state(value: String) -> Result<ThreadState, BoardError> {
    match value.as_str() {
        "unresolved" => Ok(ThreadState::Unresolved),
        "resolved" => Ok(ThreadState::Resolved),
        _ => Err(invalid_record()),
    }
}
