//! Atomic immutable message creation and unread publication.
use crate::BoardStore;
use crate::board_topic_records::{require_board, require_topic};
use crate::message_records::{activate_watch, load_message, load_watch_status, require_thread};
use crate::storage_support::{
    BoardTransaction, allocate_activity_sequence, archived_board, attribute_invalid_record,
    current_activity_sequence, ensure_acting_for_identity, ensure_identity, invalid_record,
    recompute_project_unread, resource_already_exists, storage_error,
};
use project_board::*;
use sqlx::Connection;
use std::time::{SystemTime, UNIX_EPOCH};

const TOP_LEVEL_COOLDOWN_MILLIS: i64 = 30_000;

impl BoardStore {
    pub async fn post_message(
        &mut self,
        request: MessagePostRequest,
    ) -> Result<MessagePostResult, BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        if message_exists(&mut transaction, &request.message_id).await? {
            return Err(resource_already_exists(ResourceIdentity::Message {
                message_id: request.message_id,
            }));
        }
        let (project_id, board_id, topic_id, root_id) = match &request.placement {
            Placement::Topic { topic_id } => {
                let topic = require_topic(&mut transaction, topic_id).await?;
                let board = require_board(&mut transaction, &topic.board_id).await?;
                if board.state == BoardState::Archived {
                    return Err(archived_board());
                }
                (board.project_id, board.board_id, topic.topic_id, None)
            }
            Placement::Thread { root_message_id } => {
                let location = require_thread(&mut transaction, root_message_id).await?;
                if require_board(&mut transaction, &location.board_id)
                    .await?
                    .state
                    == BoardState::Archived
                {
                    return Err(archived_board());
                }
                if location.state == ThreadState::Resolved {
                    return Err(BoardError::thread_resolved());
                }
                (
                    location.project_id,
                    location.board_id,
                    location.topic_id,
                    Some(location.root_message_id),
                )
            }
        };
        validate_reference_targets(&mut transaction, request.references.as_slice()).await?;
        let actor_key = ensure_identity(&mut transaction, &request.actor).await?;
        let acting_for_key =
            ensure_acting_for_identity(&mut transaction, request.acting_for.as_ref()).await?;
        let activity_before_post = current_activity_sequence(&mut transaction).await?;
        if root_id.is_none() {
            enforce_and_record_cooldown(&mut transaction, &actor_key, &board_id).await?;
        }
        let root_id_text = root_id.as_ref().map(MessageId::as_str);
        sqlx::query!(
            "INSERT INTO board_messages(message_id,topic_id,board_id,root_id,actor_key,acting_for_key,text) \
             VALUES(?,?,?,?,?,?,?)",
            request.message_id.as_str(),
            topic_id.as_str(),
            board_id.as_str(),
            root_id_text,
            actor_key,
            acting_for_key,
            request.text.as_str(),
        )
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        if root_id.is_none() {
            sqlx::query!(
                "INSERT INTO board_threads(root_id,state) VALUES(?,'unresolved')",
                request.message_id.as_str(),
            )
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?;
        }
        for (ordinal, reference) in request.references.iter().enumerate() {
            let ordinal = i64::try_from(ordinal).map_err(|_| invalid_record())?;
            let (kind, target_message_id, target_root_id) = match reference {
                ReferenceTarget::Message { message_id } => {
                    ("message", Some(message_id.as_str()), None)
                }
                ReferenceTarget::Thread { root_message_id } => {
                    ("thread", None, Some(root_message_id.as_str()))
                }
            };
            sqlx::query!(
                "INSERT INTO message_references(source_id,ordinal,kind,target_message_id,target_root_id) \
                 VALUES(?,?,?,?,?)",
                request.message_id.as_str(),
                ordinal,
                kind,
                target_message_id,
                target_root_id,
            )
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?;
        }
        let activity_sequence = allocate_activity_sequence(&mut transaction).await?;
        let activity_kind = if root_id.is_some() {
            "threadMessageCreated"
        } else {
            "mainMessageCreated"
        };
        sqlx::query!(
            "INSERT INTO board_activity(activity_sequence,project_id,board_id,topic_id,root_id,kind,actor_key,message_id) \
             VALUES(?,?,?,?,?,?,?,?)",
            activity_sequence,
            project_id.as_str(),
            board_id.as_str(),
            topic_id.as_str(),
            root_id_text,
            activity_kind,
            actor_key,
            request.message_id.as_str(),
        )
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let watched_root = root_id.as_ref().unwrap_or(&request.message_id);
        activate_watch(
            &mut transaction,
            &actor_key,
            &project_id,
            watched_root,
            activity_before_post,
        )
        .await?;
        publish_message_unread(&mut transaction, &project_id, root_id.as_ref(), &actor_key).await?;
        recompute_project_unread(&mut transaction, &actor_key, project_id.as_str()).await?;
        let message = load_message(&mut transaction, &request.message_id).await?;
        let watch_status = load_watch_status(&mut transaction, &actor_key, watched_root).await?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(MessagePostResult {
            message,
            watch_status,
            outcome: "Message posted and its thread is watched.".to_owned(),
        })
    }
}

async fn message_exists(
    transaction: &mut BoardTransaction<'_>,
    message_id: &MessageId,
) -> Result<bool, BoardError> {
    let exists = sqlx::query_scalar!(
        "SELECT EXISTS(SELECT 1 FROM board_messages WHERE message_id=?)",
        message_id.as_str(),
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(storage_error)?;
    Ok(exists != 0)
}

async fn validate_reference_targets(
    transaction: &mut BoardTransaction<'_>,
    references: &[ReferenceTarget],
) -> Result<(), BoardError> {
    for reference in references {
        let exists = match reference {
            ReferenceTarget::Message { message_id } => sqlx::query_scalar!(
                "SELECT EXISTS(SELECT 1 FROM board_messages WHERE message_id=?)",
                message_id.as_str(),
            )
            .fetch_one(&mut **transaction)
            .await
            .map_err(storage_error)?,
            ReferenceTarget::Thread { root_message_id } => sqlx::query_scalar!(
                "SELECT EXISTS(SELECT 1 FROM board_threads WHERE root_id=?)",
                root_message_id.as_str(),
            )
            .fetch_one(&mut **transaction)
            .await
            .map_err(storage_error)?,
        };
        if exists == 0 {
            return Err(BoardError {
                kind: BoardFailureKind::ReferenceTargetNotFound,
                stage: BoardFailureStage::Validation,
                message: "The referenced message or thread does not exist.".to_owned(),
                next_action: BoardNextAction::CorrectRequest,
                details: BoardErrorDetails::None,
            });
        }
    }
    Ok(())
}

async fn enforce_and_record_cooldown(
    transaction: &mut BoardTransaction<'_>,
    actor_key: &str,
    board_id: &BoardId,
) -> Result<(), BoardError> {
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| BoardError::board_unavailable())?
            .as_millis(),
    )
    .map_err(|_| BoardError::board_unavailable())?;
    let previous = sqlx::query_scalar!(
        "SELECT last_post_at_ms FROM actor_board_cooldowns WHERE actor_key=? AND board_id=?",
        actor_key,
        board_id.as_str(),
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage_error)?;
    if let Some(previous) = previous {
        let retry_after_seconds =
            calculate_cooldown_retry_after_seconds(now, previous).map_err(|error| {
                attribute_invalid_record(
                    error,
                    ResourceIdentity::Board {
                        board_id: board_id.clone(),
                    },
                )
            })?;
        if let Some(retry_after_seconds) = retry_after_seconds {
            return Err(BoardError::top_level_message_cooldown(retry_after_seconds));
        }
    }
    sqlx::query!(
        "INSERT INTO actor_board_cooldowns(actor_key,board_id,last_post_at_ms) VALUES(?,?,?) \
         ON CONFLICT(actor_key,board_id) DO UPDATE SET last_post_at_ms=excluded.last_post_at_ms",
        actor_key,
        board_id.as_str(),
        now,
    )
    .execute(&mut **transaction)
    .await
    .map_err(storage_error)?;
    Ok(())
}

fn calculate_cooldown_retry_after_seconds(
    current_time_millis: i64,
    previous_post_time_millis: i64,
) -> Result<Option<u64>, BoardError> {
    if previous_post_time_millis < 0 || previous_post_time_millis > current_time_millis {
        return Err(invalid_record());
    }
    let elapsed_millis = current_time_millis
        .checked_sub(previous_post_time_millis)
        .ok_or_else(invalid_record)?;
    let remaining_millis = TOP_LEVEL_COOLDOWN_MILLIS
        .checked_sub(elapsed_millis)
        .ok_or_else(invalid_record)?;
    if remaining_millis <= 0 {
        return Ok(None);
    }
    let rounded_millis = remaining_millis
        .checked_add(999)
        .ok_or_else(invalid_record)?;
    let retry_after_seconds =
        u64::try_from(rounded_millis / 1_000).map_err(|_| invalid_record())?;
    Ok(Some(retry_after_seconds))
}

async fn publish_message_unread(
    transaction: &mut BoardTransaction<'_>,
    project_id: &ProjectId,
    root_id: Option<&MessageId>,
    actor_key: &str,
) -> Result<(), BoardError> {
    if let Some(root_message_id) = root_id {
        sqlx::query!(
            "UPDATE project_reader_state SET has_unread=1 \
             WHERE project_id=? AND reader_key<>? AND reader_key IN ( \
               SELECT reader_key FROM thread_watches \
               WHERE root_id=? AND active=1 \
                 AND starts_after_activity<(SELECT last_sequence FROM activity_checkpoint WHERE singleton=1))",
            project_id.as_str(),
            actor_key,
            root_message_id.as_str(),
        )
        .execute(&mut **transaction)
        .await
        .map_err(storage_error)?;
    } else {
        sqlx::query!(
            "UPDATE project_reader_state SET has_unread=1 \
             WHERE project_id=? AND reader_key<>? AND main_start IS NOT NULL \
               AND main_start<(SELECT last_sequence FROM activity_checkpoint WHERE singleton=1)",
            project_id.as_str(),
            actor_key,
        )
        .execute(&mut **transaction)
        .await
        .map_err(storage_error)?;
    }
    Ok(())
}
