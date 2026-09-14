//! Literal text search in top-level and thread messages, independent of reader state.
use crate::BoardStore;
use crate::message_history_reads::{message_scope_parts, validate_message_scope};
use crate::message_records::load_message;
use crate::storage_support::{
    current_activity_sequence, decode_cursor, encode_cursor, invalid_cursor, invalid_record,
    storage_error,
};
use message_board::*;
use serde::{Deserialize, Serialize};
use sqlx::Connection;

const PAGE_BYTE_BUDGET: usize = 900 * 1024;

#[derive(Serialize, Deserialize)]
struct SearchCursor {
    operation: String,
    fingerprint: String,
    upper_sequence: i64,
    last_sequence: i64,
}
struct MessageSearchRow {
    activity_sequence: i64,
    message_id: String,
    project_id: String,
}

impl BoardStore {
    pub async fn search_messages(
        &mut self,
        request: MessageSearchRequest,
    ) -> Result<MessageSearchResult, BoardError> {
        if matches!(request.scope, MessageListScope::Thread { .. })
            && request.kind == SearchMessageKind::TopLevel
        {
            return Err(BoardError::invalid_field(
                "kind",
                "a thread scope contains thread messages, not top-level messages",
            ));
        }
        let fingerprint = serde_json::to_string(&(
            &request.query,
            &request.scope,
            request.kind,
            request.include_archived,
        ))
        .map_err(|_| invalid_cursor())?;
        let mut transaction = self.connection.begin().await.map_err(storage_error)?;
        validate_message_scope(&mut transaction, &request.scope).await?;
        let latest = current_activity_sequence(&mut transaction).await?;
        let (upper, last) = match request.page.cursor.as_deref() {
            None => (latest, None),
            Some(cursor) => {
                let cursor: SearchCursor = decode_cursor(&self.cursor_key, cursor)?;
                if cursor.operation != "messageSearch"
                    || cursor.fingerprint != fingerprint
                    || cursor.upper_sequence > latest
                {
                    return Err(invalid_cursor());
                }
                (cursor.upper_sequence, Some(cursor.last_sequence))
            }
        };
        let (scope_kind, scope_id) = message_scope_parts(&request.scope);
        let kind = match request.kind {
            SearchMessageKind::Both => "both",
            SearchMessageKind::TopLevel => "main",
            SearchMessageKind::Thread => "thread",
        };
        let query = request.query.as_str().to_ascii_lowercase();
        let limit = i64::from(request.page.limit.get()) + 1;
        let rows = sqlx::query_as!(MessageSearchRow,
            "SELECT a.activity_sequence,m.message_id,b.project_id FROM board_activity a \
             JOIN board_messages m ON m.message_id=a.message_id JOIN project_boards b ON b.board_id=m.board_id \
             WHERE a.activity_sequence<=? AND (? IS NULL OR a.activity_sequence<?) \
               AND (?='all' OR (?='project' AND b.project_id=?) OR (?='board' AND m.board_id=?) OR (?='topic' AND m.topic_id=?) OR (?='thread' AND m.root_id=?)) \
               AND (?='both' OR (?='main' AND m.root_id IS NULL) OR (?='thread' AND m.root_id IS NOT NULL)) \
               AND (? OR b.state='active') AND instr(lower(m.text),?)>0 \
             ORDER BY a.activity_sequence DESC LIMIT ?",
            upper,last,last,scope_kind,scope_kind,scope_id,scope_kind,scope_id,scope_kind,scope_id,scope_kind,scope_id,
            kind,kind,kind,request.include_archived,query,limit)
            .fetch_all(&mut *transaction).await.map_err(storage_error)?;
        let mut has_more = rows.len() > request.page.limit.get() as usize;
        let mut records = Vec::new();
        let mut bytes = 2;
        let mut next_last = last.unwrap_or(upper);
        for row in rows.iter().take(request.page.limit.get() as usize) {
            let message = load_message(
                &mut transaction,
                &MessageId::try_from(row.message_id.clone()).map_err(|_| invalid_record())?,
            )
            .await?;
            let record = LocatedMessage {
                project_id: ProjectId::try_from(row.project_id.clone())
                    .map_err(|_| invalid_record())?,
                message,
            };
            let size = serde_json::to_vec(&record)
                .map_err(|_| invalid_record())?
                .len()
                + usize::from(!records.is_empty());
            if bytes + size > PAGE_BYTE_BUDGET {
                has_more = true;
                break;
            }
            bytes += size;
            records.push(record);
            next_last = row.activity_sequence;
        }
        let next_cursor = has_more
            .then(|| {
                encode_cursor(
                    &self.cursor_key,
                    &SearchCursor {
                        operation: "messageSearch".to_owned(),
                        fingerprint,
                        upper_sequence: upper,
                        last_sequence: next_last,
                    },
                )
            })
            .transpose()?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(MessageSearchResult {
            page: Page {
                records,
                next_cursor,
            },
        })
    }
}
