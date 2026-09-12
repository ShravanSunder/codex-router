//! Shared typed message and thread record access.
use crate::storage_support::{
    BoardTransaction, StoredIdentityRow, attribute_invalid_record, current_activity_sequence,
    decode_identity, ensure_project_reader_state, invalid_record, storage_error,
    validate_stored_boundary,
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
    .ok_or_else(|| invalid_root_message(root_message_id))?;
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
        "SELECT watch.active,watch.starts_after_activity,board.state AS board_state \
         FROM board_threads thread \
         JOIN board_messages message ON message.message_id=thread.root_id \
         JOIN project_boards board ON board.board_id=message.board_id \
         LEFT JOIN thread_watches watch \
           ON watch.root_id=thread.root_id AND watch.reader_key=? \
         WHERE thread.root_id=?",
        reader_key,
        root_message_id.as_str(),
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage_error)?;
    let row = row.ok_or_else(|| invalid_root_message(root_message_id))?;
    let board_lifecycle = match row.board_state.as_str() {
        "active" => "The board is active.",
        "archived" => "The board is archived and read-only.",
        _ => return Err(invalid_record()),
    };
    let latest = current_activity_sequence(transaction).await?;
    let (Some(active), Some(starts_after_activity)) = (row.active, row.starts_after_activity)
    else {
        if row.active.is_some() || row.starts_after_activity.is_some() {
            return Err(invalid_record());
        }
        let earlier_unwatched_range =
            thread_message_history_range(transaction, root_message_id, latest).await?;
        let history_guidance = if earlier_unwatched_range.is_some() {
            " Use earlierUnwatchedRange with a message list range request to fetch earlier history."
        } else {
            " No earlier thread-message history is available."
        };
        return Ok(WatchStatus {
            watching: false,
            starts_after_activity_sequence: None,
            earlier_unwatched_range,
            message: format!(
                "This thread is not watched. Use thread watch to receive future activity.{history_guidance} {board_lifecycle}"
            ),
        });
    };
    if active != 0 && active != 1 {
        return Err(invalid_record());
    }
    let thread_resource = ResourceIdentity::Thread {
        root_message_id: root_message_id.clone(),
    };
    validate_stored_boundary(starts_after_activity, 0, latest, thread_resource)?;
    let boundary_sequence = activity_sequence(starts_after_activity)?;
    let history_upper_sequence = if active == 1 {
        starts_after_activity
    } else {
        latest
    };
    let earlier_unwatched_range =
        thread_message_history_range(transaction, root_message_id, history_upper_sequence).await?;
    let history_guidance = if earlier_unwatched_range.is_some() {
        " Use earlierUnwatchedRange with a message list range request to fetch earlier history."
    } else {
        " No earlier thread-message history is available."
    };
    Ok(WatchStatus {
        watching: active == 1,
        starts_after_activity_sequence: (active == 1).then_some(boundary_sequence),
        earlier_unwatched_range,
        message: if active == 1 {
            format!("Watching future thread activity.{history_guidance} {board_lifecycle}")
        } else {
            format!(
                "This thread is not watched. Use thread watch to receive future activity.{history_guidance} {board_lifecycle}"
            )
        },
    })
}

async fn thread_message_history_range(
    transaction: &mut BoardTransaction<'_>,
    root_message_id: &MessageId,
    upper_sequence: i64,
) -> Result<Option<HistoryRange>, BoardError> {
    let exists = sqlx::query_scalar!(
        "SELECT EXISTS(SELECT 1 FROM board_activity \
         WHERE root_id=? AND kind='threadMessageCreated' AND activity_sequence<=?)",
        root_message_id.as_str(),
        upper_sequence,
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(storage_error)?;
    if exists != 0 {
        Ok(Some(HistoryRange {
            scope: MessageListScope::Thread {
                root_message_id: root_message_id.clone(),
            },
            selection: MessageSelection::Range {
                from_activity_sequence: ActivitySequence::ZERO,
                to_activity_sequence: activity_sequence(upper_sequence)?,
            },
        }))
    } else {
        Ok(None)
    }
}

fn invalid_root_message(root_message_id: &MessageId) -> BoardError {
    BoardError {
        kind: BoardFailureKind::InvalidRootMessage,
        stage: BoardFailureStage::Validation,
        message: "The thread root does not exist or is not a top-level message. Choose a top-level message ID.".to_owned(),
        next_action: BoardNextAction::InspectResource,
        details: BoardErrorDetails::Resource {
            resource: ResourceIdentity::Thread {
                root_message_id: root_message_id.clone(),
            },
        },
    }
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
