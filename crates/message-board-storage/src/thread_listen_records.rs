//! Transactional Thread listening storage; Delivered and Acknowledged positions remain separate.
use crate::BoardStore;
use crate::board_topic_records::{require_board, require_topic};
use crate::message_records::{activate_watch, activity_sequence, load_message, require_thread};
use crate::participant_records::advance_participant_last_seen;
use crate::participant_row_decoding::load_participant;
use crate::storage_support::{
    current_activity_sequence, ensure_identity, invalid_record, storage_error,
    validate_stored_boundary,
};
use message_board::*;
use sqlx::Connection;
use std::collections::{BTreeMap, HashSet};

#[path = "thread_listen_delivery_position.rs"]
mod delivery_position;

/// How a Thread entered a listen's selection. Only a Thread the Reader watches
/// directly carries the per-Thread Participant gate; a Topic Watch contributes
/// Threads on the Topic's own read terms.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RootOrigin {
    ThreadWatch,
    TopicWatch,
}

const THREAD_BATCH_MESSAGE_LIMIT: i64 = 100;

#[derive(Clone, Copy)]
enum ThreadListenBatchPositionPolicy {
    CommitOnSelection,
    CommitAfterDelivery,
}

struct ThreadListenBoundary {
    stored_delivered_position: Option<i64>,
    effective_delivered_position: i64,
}

async fn listening_threads(
    transaction: &mut crate::storage_support::BoardTransaction<'_>,
    context: &ThreadListenContext,
    reader_key: &str,
) -> Result<Vec<ThreadListenThread>, BoardError> {
    let mut threads = context.threads.clone();
    if context.topic_ids.is_empty() {
        return Ok(threads);
    }
    for topic_id in &context.topic_ids {
        let root_ids = sqlx::query_scalar!(
            "SELECT threads.root_id FROM board_messages roots JOIN board_threads threads ON threads.root_id=roots.message_id WHERE roots.topic_id=? AND roots.root_id IS NULL ORDER BY threads.root_id",
            topic_id.as_str(),
        )
        .fetch_all(&mut **transaction)
        .await
        .map_err(storage_error)?;
        for root_id in root_ids {
            let root_message_id = MessageId::try_from(root_id).map_err(|_| invalid_record())?;
            if threads
                .iter()
                .any(|thread| thread.root_message_id == root_message_id)
            {
                continue;
            }
            let location = require_thread(transaction, &root_message_id).await?;
            activate_watch(
                transaction,
                reader_key,
                &location.project_id,
                &root_message_id,
                i64::try_from(context.armed_after_sequence.get()).map_err(|_| invalid_record())?,
            )
            .await?;
            threads.push(ThreadListenThread {
                root_message_id,
                initial_delivered_position: context.armed_after_sequence,
            });
        }
    }
    Ok(threads)
}

async fn load_thread_listen_boundary(
    transaction: &mut crate::storage_support::BoardTransaction<'_>,
    reader_key: &str,
    thread: &ThreadListenThread,
    latest: i64,
) -> Result<Option<ThreadListenBoundary>, BoardError> {
    let resource = ResourceIdentity::Thread {
        root_message_id: thread.root_message_id.clone(),
    };
    let initial =
        i64::try_from(thread.initial_delivered_position.get()).map_err(|_| invalid_record())?;
    let row = sqlx::query!(
        "SELECT watch.starts_after_activity,position.delivered_through, \
           CASE WHEN position.delivered_through IS NULL THEN 1 ELSE EXISTS( \
             SELECT 1 FROM board_activity activity \
             WHERE activity.activity_sequence=position.delivered_through \
               AND ((activity.root_id=watch.root_id AND activity.kind='threadMessageCreated') \
                 OR (activity.message_id=watch.root_id AND activity.kind='mainMessageCreated'))) END AS valid_scope \
         FROM thread_watches watch \
         LEFT JOIN thread_delivery_positions position \
           ON position.reader_key=watch.reader_key AND position.root_id=watch.root_id \
         WHERE watch.reader_key=? AND watch.root_id=? AND watch.active=1",
        reader_key,
        thread.root_message_id.as_str(),
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    validate_stored_boundary(row.starts_after_activity, 0, latest, resource.clone())?;
    let effective_delivered_position = match row.delivered_through {
        Some(delivered_position) => {
            validate_stored_boundary(
                delivered_position,
                row.starts_after_activity,
                latest,
                resource.clone(),
            )?;
            if row.valid_scope != 1 {
                return Err(BoardError::invalid_record(resource));
            }
            delivered_position
        }
        None => {
            let effective = initial.max(row.starts_after_activity);
            validate_stored_boundary(effective, row.starts_after_activity, latest, resource)?;
            effective
        }
    };
    Ok(Some(ThreadListenBoundary {
        stored_delivered_position: row.delivered_through,
        effective_delivered_position,
    }))
}

impl BoardStore {
    pub async fn prepare_thread_listen(
        &mut self,
        request: &ThreadListenRequest,
    ) -> Result<ThreadListenContext, BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let reader_key = ensure_identity(&mut transaction, &request.reader).await?;
        let latest = current_activity_sequence(&mut transaction).await?;
        let mut selected_topic_ids = Vec::new();
        let roots = match &request.selection {
            ThreadListenSelection::Watched => {
                selected_topic_ids = sqlx::query_scalar!(
                    "SELECT topic_id FROM topic_watches WHERE reader_key=? AND active=1 ORDER BY topic_id",
                    reader_key,
                )
                .fetch_all(&mut *transaction)
                .await
                .map_err(storage_error)?
                .into_iter()
                .map(|topic| TopicId::try_from(topic).map_err(|_| invalid_record()))
                .collect::<Result<Vec<_>, _>>()?;
                // Origin decides the Participant gate, so the two sources stay
                // separate: a Thread reached through a Topic Watch is readable on
                // the Topic's terms, exactly as Topic selection treats it.
                let thread_watched = sqlx::query_scalar!(
                    "SELECT root_id FROM thread_watches WHERE reader_key=? AND active=1 ORDER BY root_id",
                    reader_key,
                )
                .fetch_all(&mut *transaction)
                .await
                .map_err(storage_error)?;
                let topic_watched = sqlx::query_scalar!(
                    "SELECT threads.root_id FROM topic_watches topic_watch \
                     JOIN board_messages roots ON roots.topic_id=topic_watch.topic_id AND roots.root_id IS NULL \
                     JOIN board_threads threads ON threads.root_id=roots.message_id \
                     WHERE topic_watch.reader_key=? AND topic_watch.active=1 ORDER BY threads.root_id",
                    reader_key,
                )
                .fetch_all(&mut *transaction)
                .await
                .map_err(storage_error)?;
                let mut watched: BTreeMap<MessageId, RootOrigin> = BTreeMap::new();
                for root in thread_watched {
                    watched.insert(
                        MessageId::try_from(root).map_err(|_| invalid_record())?,
                        RootOrigin::ThreadWatch,
                    );
                }
                // An active Topic Watch wins the overlap. Arming any listen
                // activates a Thread Watch for each delivered root, so a Thread
                // Watch row is not evidence that the Reader claimed that Thread
                // directly; an active Topic Watch is evidence that it may read it.
                for root in topic_watched {
                    watched.insert(
                        MessageId::try_from(root).map_err(|_| invalid_record())?,
                        RootOrigin::TopicWatch,
                    );
                }
                watched.into_iter().collect::<Vec<_>>()
            }
            ThreadListenSelection::Roots { root_message_ids } => {
                if root_message_ids.is_empty() {
                    return Err(BoardError::invalid_field(
                        "selection.rootMessageIds",
                        "must contain at least one Thread",
                    ));
                }
                let mut distinct = HashSet::new();
                for root_message_id in root_message_ids {
                    if !distinct.insert(root_message_id.clone()) {
                        return Err(BoardError::invalid_field(
                            "selection.rootMessageIds",
                            "must not contain duplicate Threads",
                        ));
                    }
                }
                root_message_ids
                    .iter()
                    .map(|root_message_id| (root_message_id.clone(), RootOrigin::ThreadWatch))
                    .collect::<Vec<_>>()
            }
            ThreadListenSelection::Topic { topic_id } => {
                selected_topic_ids.push(topic_id.clone());
                let topic = require_topic(&mut transaction, topic_id).await?;
                let board = require_board(&mut transaction, &topic.board_id).await?;
                sqlx::query!(
                    "INSERT INTO topic_watches(reader_key,topic_id,starts_after_activity,active) VALUES(?,?,?,1) \
                     ON CONFLICT(reader_key,topic_id) DO UPDATE SET starts_after_activity=CASE WHEN topic_watches.active=0 THEN excluded.starts_after_activity ELSE topic_watches.starts_after_activity END,active=1",
                    reader_key,
                    topic_id.as_str(),
                    latest,
                )
                .execute(&mut *transaction)
                .await
                .map_err(storage_error)?;
                let _project_id = board.project_id;
                sqlx::query_scalar!(
                    "SELECT threads.root_id FROM board_messages roots JOIN board_threads threads ON threads.root_id=roots.message_id WHERE roots.topic_id=? AND roots.root_id IS NULL ORDER BY threads.root_id",
                    topic_id.as_str(),
                )
                .fetch_all(&mut *transaction)
                .await
                .map_err(storage_error)?
                .into_iter()
                .map(|root| {
                    MessageId::try_from(root)
                        .map(|root| (root, RootOrigin::TopicWatch))
                        .map_err(|_| invalid_record())
                })
                .collect::<Result<Vec<_>, _>>()?
            }
        };

        let mut selected_project: Option<ProjectId> = None;
        let mut selected_threads = Vec::with_capacity(roots.len());
        let mut missing_root_message_ids = Vec::new();
        for (root_message_id, origin) in roots {
            let location = require_thread(&mut transaction, &root_message_id).await?;
            if matches!(request.reader, Identity::Session { .. })
                && origin == RootOrigin::ThreadWatch
            {
                let participant =
                    load_participant(&mut transaction, &request.reader, &root_message_id).await?;
                if participant
                    .as_ref()
                    .is_none_or(|participant| !participant.is_open())
                {
                    missing_root_message_ids.push(root_message_id.clone());
                }
            }
            if selected_project
                .as_ref()
                .is_some_and(|project_id| *project_id != location.project_id)
            {
                return Err(BoardError::invalid_field(
                    "selection",
                    "must contain Threads from one project",
                ));
            }
            selected_project.get_or_insert_with(|| location.project_id.clone());
            selected_threads.push((root_message_id, location));
        }
        if let Some(root_message_id) = missing_root_message_ids.first().cloned() {
            return Err(BoardError::participants_required(
                root_message_id,
                missing_root_message_ids,
                request.reader.clone(),
            ));
        }

        let mut threads = Vec::with_capacity(selected_threads.len());
        for (root_message_id, location) in selected_threads {
            activate_watch(
                &mut transaction,
                &reader_key,
                &location.project_id,
                &root_message_id,
                latest,
            )
            .await?;
            let watch_start: i64 = sqlx::query_scalar!(
                "SELECT starts_after_activity FROM thread_watches WHERE reader_key=? AND root_id=? AND active=1",
                reader_key,
                root_message_id.as_str(),
            )
            .fetch_one(&mut *transaction)
            .await
            .map_err(storage_error)?;
            let requested_initial = match request.from_activity_sequence {
                Some(from) => {
                    let from = i64::try_from(from.get()).map_err(|_| {
                        BoardError::invalid_field(
                            "fromActivitySequence",
                            "must be a valid Activity sequence",
                        )
                    })?;
                    if from < watch_start || from > latest {
                        return Err(BoardError::invalid_field(
                            "fromActivitySequence",
                            "must be between the active Watch start and latest Activity",
                        ));
                    }
                    from
                }
                None => watch_start,
            };
            let candidate = ThreadListenThread {
                root_message_id: root_message_id.clone(),
                initial_delivered_position: activity_sequence(requested_initial)?,
            };
            let boundary =
                load_thread_listen_boundary(&mut transaction, &reader_key, &candidate, latest)
                    .await?
                    .ok_or_else(|| {
                        BoardError::invalid_record(ResourceIdentity::Thread {
                            root_message_id: root_message_id.clone(),
                        })
                    })?;
            if boundary.stored_delivered_position.is_some()
                && request.from_activity_sequence.is_some()
            {
                return Err(BoardError::invalid_field(
                    "fromActivitySequence",
                    "can initialize a Delivered position only before this Reader has received a Batch",
                ));
            }
            threads.push(ThreadListenThread {
                root_message_id,
                initial_delivered_position: activity_sequence(
                    boundary.effective_delivered_position,
                )?,
            });
        }
        transaction.commit().await.map_err(storage_error)?;
        Ok(ThreadListenContext {
            reader: request.reader.clone(),
            threads,
            topic_ids: selected_topic_ids,
            armed_after_sequence: activity_sequence(latest)?,
        })
    }

    pub async fn thread_listen_has_activity(
        &mut self,
        context: &ThreadListenContext,
    ) -> Result<bool, BoardError> {
        Ok(self.thread_listen_latest_activity(context).await?.is_some())
    }

    pub async fn thread_listen_latest_activity(
        &mut self,
        context: &ThreadListenContext,
    ) -> Result<Option<ActivitySequence>, BoardError> {
        let reader_key = crate::storage_support::identity_key(&context.reader);
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let current_latest = current_activity_sequence(&mut transaction).await?;
        let mut latest_activity: Option<i64> = None;
        for thread in listening_threads(&mut transaction, context, &reader_key).await? {
            let Some(boundary) =
                load_thread_listen_boundary(&mut transaction, &reader_key, &thread, current_latest)
                    .await?
            else {
                continue;
            };
            let latest = sqlx::query_scalar!(
                "SELECT MAX(activity.activity_sequence) FROM board_activity activity \
                 WHERE ((activity.root_id=? AND activity.kind='threadMessageCreated') OR (activity.message_id=? AND activity.kind='mainMessageCreated')) \
                   AND activity.actor_key<>? \
                   AND activity.activity_sequence>?",
                thread.root_message_id.as_str(),
                thread.root_message_id.as_str(),
                reader_key,
                boundary.effective_delivered_position,
            )
            .fetch_one(&mut *transaction)
            .await
            .map_err(storage_error)?;
            if latest > latest_activity {
                latest_activity = latest;
            }
        }
        transaction.commit().await.map_err(storage_error)?;
        latest_activity.map(activity_sequence).transpose()
    }

    pub async fn select_thread_listen_batch_set(
        &mut self,
        listen_id: ListenId,
        context: &ThreadListenContext,
        maximum_batch_set_bytes: usize,
    ) -> Result<ThreadListenBatchSet, BoardError> {
        self.select_thread_listen_batch_set_with_policy(
            listen_id,
            context,
            maximum_batch_set_bytes,
            ThreadListenBatchPositionPolicy::CommitOnSelection,
        )
        .await
    }

    /// Select a batch for session delivery without consuming it before the route accepts it.
    pub async fn select_pending_thread_listen_batch_set(
        &mut self,
        listen_id: ListenId,
        context: &ThreadListenContext,
        maximum_batch_set_bytes: usize,
    ) -> Result<ThreadListenBatchSet, BoardError> {
        self.select_thread_listen_batch_set_with_policy(
            listen_id,
            context,
            maximum_batch_set_bytes,
            ThreadListenBatchPositionPolicy::CommitAfterDelivery,
        )
        .await
    }

    async fn select_thread_listen_batch_set_with_policy(
        &mut self,
        listen_id: ListenId,
        context: &ThreadListenContext,
        maximum_batch_set_bytes: usize,
        position_policy: ThreadListenBatchPositionPolicy,
    ) -> Result<ThreadListenBatchSet, BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let reader_key = crate::storage_support::identity_key(&context.reader);
        let latest = current_activity_sequence(&mut transaction).await?;
        let mut batch_set = ThreadListenBatchSet {
            kind: ThreadListenOutputKind::BatchSet,
            listen_id,
            batches: Vec::new(),
            catch_up: context.armed_after_sequence
                > context
                    .threads
                    .iter()
                    .map(|thread| thread.initial_delivered_position)
                    .min()
                    .unwrap_or(context.armed_after_sequence),
        };
        let mut remaining_message_limit = THREAD_BATCH_MESSAGE_LIMIT;
        let selected_threads = listening_threads(&mut transaction, context, &reader_key).await?;
        'threads: for thread in &selected_threads {
            let Some(boundary) =
                load_thread_listen_boundary(&mut transaction, &reader_key, thread, latest).await?
            else {
                continue;
            };
            let message_ids = sqlx::query_scalar!(
                "SELECT activity.message_id AS \"message_id!: String\" FROM board_activity activity \
                 WHERE ((activity.root_id=? AND activity.kind='threadMessageCreated') OR (activity.message_id=? AND activity.kind='mainMessageCreated')) \
                   AND activity.message_id IS NOT NULL AND activity.actor_key<>? \
                   AND activity.activity_sequence>? \
                 ORDER BY activity.activity_sequence ASC LIMIT ?",
                thread.root_message_id.as_str(),
                thread.root_message_id.as_str(),
                reader_key,
                boundary.effective_delivered_position,
                remaining_message_limit,
            )
            .fetch_all(&mut *transaction)
            .await
            .map_err(storage_error)?;
            if message_ids.is_empty() {
                continue;
            }
            let mut batch = ThreadBatch {
                root_message_id: thread.root_message_id.clone(),
                delivered_through: thread.initial_delivered_position,
                messages: Vec::with_capacity(message_ids.len()),
            };
            for message_id in message_ids {
                let message_id = MessageId::try_from(message_id).map_err(|_| invalid_record())?;
                let message = load_message(&mut transaction, &message_id).await?;
                batch.messages.push(ThreadBatchMessage {
                    activity_sequence: message.activity_sequence,
                    message_id: message.message_id,
                    actor: message.actor,
                    text: message.text,
                });
                batch.delivered_through = batch
                    .messages
                    .last()
                    .ok_or_else(invalid_record)?
                    .activity_sequence;
                let replacing_batch = batch_set
                    .batches
                    .last()
                    .is_some_and(|selected| selected.root_message_id == batch.root_message_id);
                if replacing_batch {
                    batch_set.batches.pop();
                }
                batch_set.batches.push(batch.clone());
                let encoded_bytes = serde_json::to_vec(&batch_set)
                    .map_err(|_| BoardError::board_unavailable())?
                    .len();
                if encoded_bytes > maximum_batch_set_bytes {
                    batch_set.batches.pop();
                    batch.messages.pop();
                    if !batch.messages.is_empty() {
                        batch.delivered_through = batch
                            .messages
                            .last()
                            .ok_or_else(invalid_record)?
                            .activity_sequence;
                        batch_set.batches.push(batch);
                    } else if batch_set.batches.is_empty() {
                        return Err(BoardError::board_unavailable());
                    }
                    break 'threads;
                }
                remaining_message_limit -= 1;
                if remaining_message_limit == 0 {
                    break 'threads;
                }
            }
        }
        if matches!(
            position_policy,
            ThreadListenBatchPositionPolicy::CommitOnSelection
        ) {
            for batch in &batch_set.batches {
                let delivered_value =
                    i64::try_from(batch.delivered_through.get()).map_err(|_| invalid_record())?;
                let watch_start: i64 = sqlx::query_scalar!(
                    "SELECT starts_after_activity FROM thread_watches WHERE reader_key=? AND root_id=? AND active=1",
                    reader_key,
                    batch.root_message_id.as_str(),
                )
                .fetch_one(&mut *transaction)
                .await
                .map_err(storage_error)?;
                validate_stored_boundary(
                    delivered_value,
                    watch_start,
                    latest,
                    ResourceIdentity::Thread {
                        root_message_id: batch.root_message_id.clone(),
                    },
                )?;
                sqlx::query!(
                    "INSERT INTO thread_delivery_positions(reader_key,root_id,delivered_through) VALUES(?,?,?) \
                     ON CONFLICT(reader_key,root_id) DO UPDATE SET delivered_through=excluded.delivered_through",
                    reader_key,
                    batch.root_message_id.as_str(),
                    delivered_value,
                )
                .execute(&mut *transaction)
                .await
                .map_err(storage_error)?;
                advance_participant_last_seen(
                    &mut transaction,
                    &reader_key,
                    &batch.root_message_id,
                    delivered_value,
                )
                .await?;
            }
        }
        transaction.commit().await.map_err(storage_error)?;
        Ok(batch_set)
    }
}
