//! Project inbox initialization, activity reads, scoped acknowledgements, and summaries.
use crate::BoardStore;
use crate::board_topic_records::{require_board, require_topic};
use crate::message_records::{activity_sequence, load_identity, load_message, require_thread};
use crate::project_records::require_project;
use crate::storage_support::{
    BoardTransaction, current_activity_sequence, decode_cursor, encode_cursor, ensure_identity,
    ensure_project_reader_state, invalid_cursor, invalid_record, project_for_scope,
    recompute_project_unread, storage_error, validate_reader_activity_boundaries,
    validate_stored_boundary,
};
use message_board::*;
use serde::{Deserialize, Serialize};
use sqlx::Connection;

const PAGE_RECORDS_BYTE_BUDGET: usize = 900 * 1024;

#[derive(Serialize, Deserialize)]
struct InboxCursor {
    operation: String,
    scope: InboxScope,
    read_mode: InboxReadMode,
    reader_key: String,
    upper_sequence: i64,
    last_sequence: i64,
}
#[derive(Serialize, Deserialize)]
struct SummaryCursor {
    operation: String,
    reader_key: String,
    unread_only: bool,
    last_project: String,
}

#[derive(Debug)]
struct StoredInboxActivityRow {
    project_id: String,
    board_id: String,
    activity_sequence: i64,
    kind: String,
    message_id: Option<String>,
    root_id: Option<String>,
    topic_id: String,
    actor_key: String,
}

#[derive(Debug)]
struct StoredUnreadSummaryRow {
    project_id: String,
    has_unread: i64,
    main_start: Option<i64>,
}

impl BoardStore {
    pub async fn fetch_inbox(
        &mut self,
        request: InboxFetchRequest,
    ) -> Result<InboxFetchResult, BoardError> {
        let mut transaction = self
            .connection
            .begin_with(if request.read_mode == InboxReadMode::Unread {
                "BEGIN IMMEDIATE"
            } else {
                "BEGIN"
            })
            .await
            .map_err(storage_error)?;
        let project_id = inbox_project(&mut transaction, &request.scope).await?;
        let unread_mode = request.read_mode == InboxReadMode::Unread;
        let reader_key = if unread_mode {
            ensure_identity(&mut transaction, &request.reader).await?
        } else {
            crate::storage_support::identity_key(&request.reader)
        };
        let latest = current_activity_sequence(&mut transaction).await?;
        let existing_start: Option<Option<i64>> = sqlx::query_scalar!(
            "SELECT main_start FROM project_reader_state WHERE reader_key=? AND project_id=?",
            reader_key,
            project_id.as_str(),
        )
        .fetch_optional(&mut *transaction)
        .await
        .map_err(storage_error)?;
        if unread_mode {
            ensure_project_reader_state(&mut transaction, &reader_key, project_id.as_str()).await?;
        }
        validate_reader_activity_boundaries(
            &mut transaction,
            &reader_key,
            project_id.as_str(),
            latest,
        )
        .await?;
        let initialized_now = unread_mode && existing_start.flatten().is_none();
        if initialized_now {
            sqlx::query!("UPDATE project_reader_state SET main_start=? WHERE reader_key=? AND project_id=? AND main_start IS NULL", latest, reader_key, project_id.as_str())
                .execute(&mut *transaction).await.map_err(storage_error)?;
        }
        let (upper, last) = decode_inbox_cursor(
            &self.cursor_key,
            request.page.cursor.as_deref(),
            &request.scope,
            request.read_mode,
            &reader_key,
            latest,
        )?;
        let row_limit = i64::from(request.page.limit.get()) + 1;
        validate_watched_project_boundaries(&mut transaction, &reader_key, latest).await?;
        let (scope_kind, scope_id) = match &request.scope {
            InboxScope::Project { project_id } => ("project", project_id.as_str()),
            InboxScope::Board { board_id } => ("board", board_id.as_str()),
            InboxScope::Topic { topic_id } => ("topic", topic_id.as_str()),
        };
        let latest_mode = !unread_mode;
        let rows = sqlx::query_as!(StoredInboxActivityRow,
            "SELECT a.activity_sequence,a.project_id,a.board_id,a.kind,a.message_id,a.root_id,a.topic_id,a.actor_key \
             FROM board_activity a \
             LEFT JOIN project_reader_state prs ON prs.reader_key=? AND prs.project_id=a.project_id \
             LEFT JOIN topic_read_bookmarks tb ON tb.reader_key=? AND tb.topic_id=a.topic_id \
             LEFT JOIN thread_watches w ON w.reader_key=? AND w.root_id=a.root_id \
             LEFT JOIN thread_read_bookmarks rb ON rb.reader_key=? AND rb.root_id=a.root_id \
             WHERE a.activity_sequence<=? \
               AND ((?=1 AND a.activity_sequence<?) OR (?=0 AND a.activity_sequence>?)) \
               AND (?=1 OR a.actor_key<>?) \
               AND ((a.kind='mainMessageCreated' \
                 AND ((?='project' AND a.project_id=?) OR (?='board' AND a.board_id=?) OR (?='topic' AND a.topic_id=?)) \
                 AND (?=1 OR (prs.main_start IS NOT NULL AND a.activity_sequence>prs.main_start AND a.activity_sequence>COALESCE(tb.through_activity,0)))) \
               OR (a.kind IN ('threadMessageCreated','threadResolved','threadUnresolved') AND w.active=1 \
                 AND ((?=1 AND a.kind='threadMessageCreated') \
                   OR (?=0 AND a.activity_sequence>w.starts_after_activity AND a.activity_sequence>COALESCE(rb.through_activity,0))))) \
             ORDER BY CASE WHEN ?=1 THEN a.activity_sequence END DESC, CASE WHEN ?=0 THEN a.activity_sequence END ASC LIMIT ?",
            reader_key, reader_key, reader_key, reader_key, upper,
            latest_mode, last, latest_mode, last, latest_mode, reader_key,
            scope_kind, scope_id, scope_kind, scope_id, scope_kind, scope_id,
            latest_mode, latest_mode, latest_mode, latest_mode, latest_mode, row_limit)
            .fetch_all(&mut *transaction).await.map_err(storage_error)?;
        let mut has_more = rows.len() > request.page.limit.get() as usize;
        let rows = rows
            .into_iter()
            .take(request.page.limit.get() as usize)
            .collect::<Vec<_>>();
        let mut records = Vec::with_capacity(rows.len());
        let mut encoded_record_bytes = 2_usize;
        let mut next_last = last;
        for row in &rows {
            let activity = decode_inbox_activity(&mut transaction, row).await?;
            let activity_bytes = serde_json::to_vec(&activity)
                .map_err(|_| invalid_record())?
                .len();
            let separator_bytes = usize::from(!records.is_empty());
            if encoded_record_bytes + separator_bytes + activity_bytes > PAGE_RECORDS_BYTE_BUDGET {
                has_more = true;
                break;
            }
            encoded_record_bytes += separator_bytes + activity_bytes;
            next_last = row.activity_sequence;
            records.push(activity);
        }
        if unread_mode {
            recompute_project_unread(&mut transaction, &reader_key, project_id.as_str()).await?;
        }
        let next_cursor = has_more
            .then(|| {
                encode_cursor(
                    &self.cursor_key,
                    &InboxCursor {
                        operation: "inbox".to_owned(),
                        scope: request.scope.clone(),
                        read_mode: request.read_mode,
                        reader_key: reader_key.clone(),
                        upper_sequence: upper,
                        last_sequence: next_last,
                    },
                )
            })
            .transpose()?;
        let initialization_boundary = activity_sequence(latest)?;
        let initialization = if initialized_now {
            let earlier_history_range =
                project_main_history_range(&mut transaction, &project_id, latest).await?;
            let message = if earlier_history_range.is_some() {
                "Main-message tracking starts now. Earlier project main messages remain available through earlierHistoryRange with a message list range request."
            } else {
                "Main-message tracking starts now. No earlier project main-message history is available."
            };
            InboxInitializationStatus::Initialized {
                starts_after_activity_sequence: initialization_boundary,
                earlier_history_range,
                message: message.to_owned(),
            }
        } else if unread_mode {
            InboxInitializationStatus::Existing
        } else {
            InboxInitializationStatus::NotApplicable
        };
        transaction.commit().await.map_err(storage_error)?;
        Ok(InboxFetchResult {
            page: InboxPage {
                project_id,
                scope: request.scope,
                ordering: if unread_mode {
                    MessageOrdering::OldestFirst
                } else {
                    MessageOrdering::NewestFirst
                },
                upper_activity_sequence: activity_sequence(upper)?,
                reader: request.reader,
                read_mode: request.read_mode,
                records,
                next_cursor,
                initialization,
            },
        })
    }

    pub async fn acknowledge_inbox(
        &mut self,
        request: InboxAcknowledgeRequest,
    ) -> Result<InboxAcknowledgeResult, BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let reader_key = ensure_identity(&mut transaction, &request.actor).await?;
        let through = i64::try_from(request.through_activity_sequence.get())
            .map_err(|_| invalid_acknowledgement())?;
        let latest = current_activity_sequence(&mut transaction).await?;
        if through == 0
            || through > latest
            || !activity_belongs_to_scope(&mut transaction, &request.scope, through).await?
        {
            return Err(invalid_acknowledgement());
        }
        let project_id = project_for_scope(&mut transaction, &request.scope).await?;
        validate_reader_activity_boundaries(&mut transaction, &reader_key, &project_id, latest)
            .await?;
        let previous: Option<i64> = match &request.scope {
            ReadScope::Topic { topic_id } => sqlx::query_scalar!("SELECT through_activity FROM topic_read_bookmarks WHERE reader_key=? AND topic_id=?", reader_key, topic_id.as_str()).fetch_optional(&mut *transaction).await.map_err(storage_error)?,
            ReadScope::Thread { root_message_id } => sqlx::query_scalar!("SELECT through_activity FROM thread_read_bookmarks WHERE reader_key=? AND root_id=?", reader_key, root_message_id.as_str()).fetch_optional(&mut *transaction).await.map_err(storage_error)?,
        };
        if previous.is_some_and(|previous| through < previous) {
            return Err(invalid_acknowledgement());
        }
        match &request.scope {
            ReadScope::Topic { topic_id } => {
                sqlx::query!("INSERT INTO topic_read_bookmarks(reader_key,topic_id,through_activity) VALUES(?,?,?) ON CONFLICT(reader_key,topic_id) DO UPDATE SET through_activity=excluded.through_activity", reader_key, topic_id.as_str(), through).execute(&mut *transaction).await.map_err(storage_error)?;
            }
            ReadScope::Thread { root_message_id } => {
                sqlx::query!("INSERT INTO thread_read_bookmarks(reader_key,root_id,through_activity) VALUES(?,?,?) ON CONFLICT(reader_key,root_id) DO UPDATE SET through_activity=excluded.through_activity", reader_key, root_message_id.as_str(), through).execute(&mut *transaction).await.map_err(storage_error)?;
            }
        }
        let has_unread =
            recompute_project_unread(&mut transaction, &reader_key, &project_id).await?;
        let main_tracking_initialized = sqlx::query_scalar!("SELECT main_start IS NOT NULL FROM project_reader_state WHERE reader_key=? AND project_id=?", reader_key, project_id)
            .fetch_optional(&mut *transaction).await.map_err(storage_error)?.unwrap_or(0) != 0;
        transaction.commit().await.map_err(storage_error)?;
        let project_id = ProjectId::try_from(project_id).map_err(|_| invalid_record())?;
        Ok(InboxAcknowledgeResult {
            bookmark: Bookmark {
                reader: request.actor,
                scope: request.scope,
                through_activity_sequence: request.through_activity_sequence,
            },
            project_unread_summary: ProjectUnreadSummary {
                project_id,
                has_unread,
                main_tracking_initialized,
            },
            outcome: if previous == Some(through) {
                "Inbox acknowledgement was already current."
            } else {
                "Inbox acknowledgement advanced for this scope."
            }
            .to_owned(),
        })
    }

    pub async fn list_inbox_projects(
        &mut self,
        request: InboxProjectsRequest,
    ) -> Result<InboxProjectsResult, BoardError> {
        let reader_key = crate::storage_support::identity_key(&request.reader);
        let last_project = decode_summary_cursor(
            &self.cursor_key,
            request.page.cursor.as_deref(),
            &reader_key,
            request.unread_only,
        )?;
        let mut transaction = self.connection.begin().await.map_err(storage_error)?;
        let latest = current_activity_sequence(&mut transaction).await?;
        let row_limit = i64::from(request.page.limit.get()) + 1;
        let rows = sqlx::query_as!(StoredUnreadSummaryRow, "SELECT project_id,has_unread,main_start FROM project_reader_state WHERE reader_key=? AND project_id>? AND (?=0 OR has_unread=1) ORDER BY project_id LIMIT ?", reader_key, last_project, request.unread_only, row_limit)
            .fetch_all(&mut *transaction).await.map_err(storage_error)?;
        let has_more = rows.len() > request.page.limit.get() as usize;
        let rows = rows
            .into_iter()
            .take(request.page.limit.get() as usize)
            .collect::<Vec<_>>();
        let next_cursor = if has_more {
            let last_project = rows.last().ok_or_else(invalid_record)?.project_id.clone();
            Some(encode_cursor(
                &self.cursor_key,
                &SummaryCursor {
                    operation: "summaries".to_owned(),
                    reader_key: reader_key.clone(),
                    unread_only: request.unread_only,
                    last_project,
                },
            )?)
        } else {
            None
        };
        let mut records = Vec::with_capacity(rows.len());
        for row in &rows {
            records.push(decode_summary(row, latest)?);
        }
        transaction.commit().await.map_err(storage_error)?;
        Ok(InboxProjectsResult {
            page: Page {
                records,
                next_cursor,
            },
        })
    }
}

async fn project_main_history_range(
    transaction: &mut BoardTransaction<'_>,
    project_id: &ProjectId,
    upper_sequence: i64,
) -> Result<Option<HistoryRange>, BoardError> {
    let exists = sqlx::query_scalar!(
        "SELECT EXISTS(SELECT 1 FROM board_activity \
         WHERE project_id=? AND kind='mainMessageCreated' AND activity_sequence<=?)",
        project_id.as_str(),
        upper_sequence,
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(storage_error)?;
    if exists != 0 {
        Ok(Some(HistoryRange {
            scope: MessageListScope::Project {
                project_id: project_id.clone(),
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

async fn decode_inbox_activity(
    transaction: &mut BoardTransaction<'_>,
    row: &StoredInboxActivityRow,
) -> Result<InboxActivity, BoardError> {
    let sequence_value = row.activity_sequence;
    let sequence = activity_sequence(sequence_value)?;
    let project_id = ProjectId::try_from(row.project_id.clone()).map_err(|_| invalid_record())?;
    let board_id = BoardId::try_from(row.board_id.clone()).map_err(|_| invalid_record())?;
    if require_board(transaction, &board_id).await?.project_id != project_id {
        return Err(invalid_record());
    }
    let kind = &row.kind;
    let topic_id = TopicId::try_from(row.topic_id.clone()).map_err(|_| invalid_record())?;
    match kind.as_str() {
        "mainMessageCreated" | "threadMessageCreated" => {
            let message_id =
                MessageId::try_from(row.message_id.clone().ok_or_else(invalid_record)?)
                    .map_err(|_| invalid_record())?;
            let root = row
                .root_id
                .clone()
                .map(MessageId::try_from)
                .transpose()
                .map_err(|_| invalid_record())?;
            if (kind == "mainMessageCreated" && root.is_some())
                || (kind == "threadMessageCreated" && root.is_none())
            {
                return Err(invalid_record());
            }
            let acknowledgement_scope = match root {
                Some(root_message_id) => ReadScope::Thread { root_message_id },
                None => ReadScope::Topic { topic_id },
            };
            let message = load_message(transaction, &message_id).await?;
            if message.board_id != board_id || message.topic_id.as_str() != row.topic_id {
                return Err(invalid_record());
            }
            Ok(InboxActivity::MessageCreated {
                project_id,
                activity_sequence: sequence,
                acknowledgement_scope,
                message,
            })
        }
        "threadResolved" | "threadUnresolved" => {
            if row.message_id.is_some() {
                return Err(invalid_record());
            }
            let root_message_id =
                MessageId::try_from(row.root_id.clone().ok_or_else(invalid_record)?)
                    .map_err(|_| invalid_record())?;
            let location = require_thread(transaction, &root_message_id).await?;
            if location.topic_id != topic_id
                || location.board_id != board_id
                || location.project_id != project_id
            {
                return Err(invalid_record());
            }
            let actor_key = &row.actor_key;
            Ok(InboxActivity::ThreadStateChanged {
                project_id,
                board_id,
                activity_sequence: sequence,
                acknowledgement_scope: ReadScope::Thread {
                    root_message_id: root_message_id.clone(),
                },
                root_message_id,
                topic_id,
                actor: load_identity(transaction, actor_key).await?,
                state: if kind == "threadResolved" {
                    ThreadState::Resolved
                } else {
                    ThreadState::Unresolved
                },
            })
        }
        _ => Err(invalid_record()),
    }
}

async fn activity_belongs_to_scope(
    transaction: &mut BoardTransaction<'_>,
    scope: &ReadScope,
    sequence: i64,
) -> Result<bool, BoardError> {
    match scope {
        ReadScope::Topic { topic_id } => sqlx::query_scalar!("SELECT EXISTS(SELECT 1 FROM board_activity WHERE activity_sequence=? AND topic_id=? AND kind='mainMessageCreated')", sequence, topic_id.as_str()).fetch_one(&mut **transaction).await.map(|value| value != 0).map_err(storage_error),
        ReadScope::Thread { root_message_id } => sqlx::query_scalar!("SELECT EXISTS(SELECT 1 FROM board_activity WHERE activity_sequence=? AND root_id=? AND kind IN ('threadMessageCreated','threadResolved','threadUnresolved'))", sequence, root_message_id.as_str()).fetch_one(&mut **transaction).await.map(|value| value != 0).map_err(storage_error),
    }
}
fn invalid_acknowledgement() -> BoardError {
    BoardError {
        kind: BoardFailureKind::InvalidAcknowledgement,
        stage: BoardFailureStage::Validation,
        message:
            "The acknowledgement is future, backwards, malformed, or outside the selected scope."
                .to_owned(),
        next_action: BoardNextAction::CorrectRequest,
        details: BoardErrorDetails::None,
    }
}
fn decode_inbox_cursor(
    cursor_key: &[u8; 32],
    cursor: Option<&str>,
    scope: &InboxScope,
    read_mode: InboxReadMode,
    reader_key: &str,
    latest: i64,
) -> Result<(i64, i64), BoardError> {
    let Some(cursor) = cursor else {
        return Ok((
            latest,
            if read_mode == InboxReadMode::Latest {
                latest.saturating_add(1)
            } else {
                0
            },
        ));
    };
    let cursor: InboxCursor = decode_cursor(cursor_key, cursor)?;
    if cursor.operation != "inbox"
        || cursor.scope != *scope
        || cursor.read_mode != read_mode
        || cursor.reader_key != reader_key
        || cursor.upper_sequence > latest
    {
        return Err(invalid_cursor());
    }
    Ok((cursor.upper_sequence, cursor.last_sequence))
}
fn decode_summary_cursor(
    cursor_key: &[u8; 32],
    cursor: Option<&str>,
    reader_key: &str,
    unread_only: bool,
) -> Result<String, BoardError> {
    let Some(cursor) = cursor else {
        return Ok(String::new());
    };
    let cursor: SummaryCursor = decode_cursor(cursor_key, cursor)?;
    if cursor.operation != "summaries"
        || cursor.reader_key != reader_key
        || cursor.unread_only != unread_only
    {
        return Err(invalid_cursor());
    }
    Ok(cursor.last_project)
}
fn decode_summary(
    row: &StoredUnreadSummaryRow,
    latest: i64,
) -> Result<ProjectUnreadSummary, BoardError> {
    let has_unread = row.has_unread;
    if has_unread != 0 && has_unread != 1 {
        return Err(invalid_record());
    }
    let project_id = ProjectId::try_from(row.project_id.clone()).map_err(|_| invalid_record())?;
    if let Some(main_start) = row.main_start {
        validate_stored_boundary(
            main_start,
            0,
            latest,
            ResourceIdentity::Project {
                project_id: project_id.clone(),
            },
        )?;
    }
    Ok(ProjectUnreadSummary {
        project_id,
        has_unread: has_unread == 1,
        main_tracking_initialized: row.main_start.is_some(),
    })
}

async fn inbox_project(
    transaction: &mut BoardTransaction<'_>,
    scope: &InboxScope,
) -> Result<ProjectId, BoardError> {
    match scope {
        InboxScope::Project { project_id } => {
            require_project(transaction, project_id).await?;
            Ok(project_id.clone())
        }
        InboxScope::Board { board_id } => {
            Ok(require_board(transaction, board_id).await?.project_id)
        }
        InboxScope::Topic { topic_id } => {
            let topic = require_topic(transaction, topic_id).await?;
            Ok(require_board(transaction, &topic.board_id)
                .await?
                .project_id)
        }
    }
}

async fn validate_watched_project_boundaries(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    latest: i64,
) -> Result<(), BoardError> {
    let projects = sqlx::query_scalar!("SELECT DISTINCT b.project_id FROM thread_watches w JOIN board_messages m ON m.message_id=w.root_id JOIN project_boards b ON b.board_id=m.board_id WHERE w.reader_key=? AND w.active=1", reader_key)
        .fetch_all(&mut **transaction).await.map_err(storage_error)?;
    for project_id in projects {
        validate_reader_activity_boundaries(transaction, reader_key, &project_id, latest).await?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "inbox_cursor_contract_tests.rs"]
mod inbox_cursor_contract_tests;
