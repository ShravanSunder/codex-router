//! Bounded immutable message-history reads and cursor validation.
use crate::BoardStore;
use crate::board_topic_records::{require_board, require_topic};
use crate::message_records::{load_message, require_thread};
use crate::project_records::require_project;
use crate::storage_support::{
    BoardTransaction, current_activity_sequence, decode_cursor, encode_cursor, invalid_cursor,
    invalid_record, storage_error,
};
use project_board::*;
use serde::{Deserialize, Serialize};
use sqlx::Connection;

const PAGE_RECORDS_BYTE_BUDGET: usize = 900 * 1024;

#[derive(Serialize, Deserialize)]
struct MessageCursor {
    operation: String,
    fingerprint: String,
    upper_sequence: i64,
    last_sequence: i64,
}

struct MessageHistoryRow {
    activity_sequence: i64,
    message_id: String,
}

impl BoardStore {
    pub async fn show_message(
        &mut self,
        request: MessageShowRequest,
    ) -> Result<MessageShowResult, BoardError> {
        let mut transaction = self.connection.begin().await.map_err(storage_error)?;
        let message = load_message(&mut transaction, &request.message_id).await?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(MessageShowResult { message })
    }

    pub async fn list_messages(
        &mut self,
        request: MessageListRequest,
    ) -> Result<MessageListResult, BoardError> {
        let fingerprint = serde_json::to_string(&(&request.scope, &request.selection))
            .map_err(|_| invalid_cursor())?;
        let mut transaction = self.connection.begin().await.map_err(storage_error)?;
        validate_message_scope(&mut transaction, &request.scope).await?;
        let latest = current_activity_sequence(&mut transaction).await?;
        validate_selection(&request.selection, latest)?;
        let (upper_sequence, last_sequence) = decode_message_cursor(
            &self.cursor_key,
            request.page.cursor.as_deref(),
            &fingerprint,
            &request.selection,
            latest,
        )?;
        let latest_mode = matches!(request.selection, MessageSelection::Latest);
        let ordering = if latest_mode {
            MessageOrdering::NewestFirst
        } else {
            MessageOrdering::OldestFirst
        };
        let (scope_kind, scope_id) = message_scope_parts(&request.scope);
        let query_limit = i64::from(request.page.limit.get()) + 1;
        let rows = sqlx::query_as!(
            MessageHistoryRow,
            "SELECT a.activity_sequence,m.message_id \
             FROM board_activity a JOIN board_messages m ON m.message_id=a.message_id \
             WHERE a.message_id IS NOT NULL AND a.activity_sequence<=? \
               AND ((?=1 AND a.activity_sequence<?) OR (?=0 AND a.activity_sequence>?)) \
               AND ((?='topic' AND a.topic_id=? AND a.root_id IS NULL) \
                 OR (?='thread' AND a.root_id=?) \
                 OR (?='board' AND a.board_id=?) \
                 OR (?='project' AND a.project_id=? AND a.root_id IS NULL) \
                 OR (?='all' AND a.root_id IS NULL)) \
             ORDER BY CASE WHEN ?=1 THEN a.activity_sequence END DESC, \
                      CASE WHEN ?=0 THEN a.activity_sequence END ASC LIMIT ?",
            upper_sequence,
            latest_mode,
            last_sequence,
            latest_mode,
            last_sequence,
            scope_kind,
            scope_id,
            scope_kind,
            scope_id,
            scope_kind,
            scope_id,
            scope_kind,
            scope_id,
            scope_kind,
            latest_mode,
            latest_mode,
            query_limit,
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let mut has_more = rows.len() > request.page.limit.get() as usize;
        let selected = rows.iter().take(request.page.limit.get() as usize);
        let mut records = Vec::with_capacity(request.page.limit.get() as usize);
        let mut encoded_record_bytes = 2_usize;
        let mut next_last_sequence = last_sequence;
        for row in selected {
            let message_id =
                MessageId::try_from(row.message_id.clone()).map_err(|_| invalid_record())?;
            let message = load_message(&mut transaction, &message_id).await?;
            let message_bytes = serde_json::to_vec(&message)
                .map_err(|_| invalid_record())?
                .len();
            let separator_bytes = usize::from(!records.is_empty());
            if encoded_record_bytes + separator_bytes + message_bytes > PAGE_RECORDS_BYTE_BUDGET {
                has_more = true;
                break;
            }
            encoded_record_bytes += separator_bytes + message_bytes;
            next_last_sequence = row.activity_sequence;
            records.push(message);
        }
        let next_cursor = has_more
            .then(|| {
                encode_cursor(
                    &self.cursor_key,
                    &MessageCursor {
                        operation: "messages".to_owned(),
                        fingerprint,
                        upper_sequence,
                        last_sequence: next_last_sequence,
                    },
                )
            })
            .transpose()?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(MessageListResult {
            page: MessagePage {
                scope: request.scope,
                selection: request.selection,
                ordering,
                records,
                next_cursor,
            },
        })
    }
}

fn validate_selection(selection: &MessageSelection, latest: i64) -> Result<(), BoardError> {
    let beyond = |sequence: ActivitySequence| {
        i64::try_from(sequence.get()).map_or(true, |value| value > latest)
    };
    let invalid = match selection {
        MessageSelection::Latest => false,
        MessageSelection::AfterPosition {
            after_activity_sequence,
        } => beyond(*after_activity_sequence),
        MessageSelection::Range {
            from_activity_sequence,
            to_activity_sequence,
        } => {
            beyond(*from_activity_sequence)
                || beyond(*to_activity_sequence)
                || from_activity_sequence > to_activity_sequence
        }
    };
    if invalid {
        return Err(BoardError {
            kind: BoardFailureKind::PositionBeyondLatest,
            stage: BoardFailureStage::Validation,
            message: "The requested activity position is beyond the current board activity."
                .to_owned(),
            next_action: BoardNextAction::CorrectRequest,
            details: BoardErrorDetails::None,
        });
    }
    Ok(())
}

fn decode_message_cursor(
    cursor_key: &[u8; 32],
    cursor: Option<&str>,
    fingerprint: &str,
    selection: &MessageSelection,
    latest: i64,
) -> Result<(i64, i64), BoardError> {
    if let Some(cursor) = cursor {
        let cursor: MessageCursor = decode_cursor(cursor_key, cursor)?;
        if cursor.operation != "messages"
            || cursor.fingerprint != fingerprint
            || cursor.upper_sequence > latest
        {
            return Err(invalid_cursor());
        }
        return Ok((cursor.upper_sequence, cursor.last_sequence));
    }
    Ok(match selection {
        MessageSelection::Latest => (latest, latest.saturating_add(1)),
        MessageSelection::AfterPosition {
            after_activity_sequence,
        } => (
            latest,
            i64::try_from(after_activity_sequence.get()).map_err(|_| invalid_cursor())?,
        ),
        MessageSelection::Range {
            from_activity_sequence,
            to_activity_sequence,
        } => (
            i64::try_from(to_activity_sequence.get()).map_err(|_| invalid_cursor())?,
            i64::try_from(from_activity_sequence.get())
                .map_err(|_| invalid_cursor())?
                .saturating_sub(1),
        ),
    })
}

fn message_scope_parts(scope: &MessageListScope) -> (&'static str, &str) {
    match scope {
        MessageListScope::Topic { topic_id } => ("topic", topic_id.as_str()),
        MessageListScope::Thread { root_message_id } => ("thread", root_message_id.as_str()),
        MessageListScope::Board { board_id } => ("board", board_id.as_str()),
        MessageListScope::Project { project_id } => ("project", project_id.as_str()),
        MessageListScope::AllProjects => ("all", ""),
    }
}

async fn validate_message_scope(
    transaction: &mut BoardTransaction<'_>,
    scope: &MessageListScope,
) -> Result<(), BoardError> {
    match scope {
        MessageListScope::Topic { topic_id } => {
            require_topic(transaction, topic_id).await?;
        }
        MessageListScope::Thread { root_message_id } => {
            require_thread(transaction, root_message_id).await?;
        }
        MessageListScope::Board { board_id } => {
            require_board(transaction, board_id).await?;
        }
        MessageListScope::Project { project_id } => {
            require_project(transaction, project_id).await?;
        }
        MessageListScope::AllProjects => {}
    }
    Ok(())
}
