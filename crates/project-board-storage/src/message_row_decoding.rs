//! Typed SQL rows and domain validation for immutable messages.
use crate::message_records::{activity_sequence, load_identity, require_thread};
use crate::storage_support::{
    BoardTransaction, attribute_invalid_record, invalid_record, resource_not_found, storage_error,
};
use project_board::*;

struct StoredMessageRow {
    message_id: String,
    board_id: String,
    topic_id: String,
    root_id: Option<String>,
    actor_key: String,
    acting_for_key: Option<String>,
    text: String,
    activity_sequence: i64,
    activity_actor_key: String,
    activity_kind: String,
    activity_root_id: Option<String>,
}

struct StoredReferenceRow {
    kind: String,
    target_message_id: Option<String>,
    target_root_id: Option<String>,
}

async fn load_message_unattributed(
    transaction: &mut BoardTransaction<'_>,
    message_id: &MessageId,
) -> Result<Message, BoardError> {
    let row = sqlx::query_as!(
        StoredMessageRow,
        "SELECT m.message_id,m.board_id,m.topic_id,m.root_id,m.actor_key,m.acting_for_key,m.text, \
                a.activity_sequence,a.actor_key AS activity_actor_key,a.kind AS activity_kind, \
                a.root_id AS activity_root_id \
         FROM board_messages m \
         JOIN board_activity a ON a.message_id=m.message_id \
         WHERE m.message_id=?",
        message_id.as_str(),
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage_error)?
    .ok_or_else(|| {
        resource_not_found(ResourceIdentity::Message {
            message_id: message_id.clone(),
        })
    })?;
    if row.activity_actor_key != row.actor_key {
        return Err(invalid_record());
    }
    let actor = load_identity(transaction, &row.actor_key).await?;
    let acting_for = match row.acting_for_key {
        None => None,
        Some(identity_key) => match load_identity(transaction, &identity_key).await? {
            Identity::Human { human_id } => Some(ActingForIdentity::Human { human_id }),
            Identity::Session { .. } => return Err(invalid_record()),
        },
    };
    let root_id = row
        .root_id
        .map(MessageId::try_from)
        .transpose()
        .map_err(|_| invalid_record())?;
    let topic_id = TopicId::try_from(row.topic_id).map_err(|_| invalid_record())?;
    let placement = match root_id {
        Some(root_message_id) => {
            let root = require_thread(transaction, &root_message_id).await?;
            if root.topic_id != topic_id
                || row.activity_kind != "threadMessageCreated"
                || row.activity_root_id.as_deref() != Some(root_message_id.as_str())
            {
                return Err(invalid_record());
            }
            Placement::Thread { root_message_id }
        }
        None => {
            if row.activity_kind != "mainMessageCreated" || row.activity_root_id.is_some() {
                return Err(invalid_record());
            }
            Placement::Topic {
                topic_id: topic_id.clone(),
            }
        }
    };
    let reference_rows = sqlx::query_as!(
        StoredReferenceRow,
        "SELECT kind,target_message_id,target_root_id \
         FROM message_references WHERE source_id=? ORDER BY ordinal",
        message_id.as_str(),
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage_error)?;
    let references = reference_rows
        .into_iter()
        .map(decode_reference)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Message {
        message_id: MessageId::try_from(row.message_id).map_err(|_| invalid_record())?,
        board_id: BoardId::try_from(row.board_id).map_err(|_| invalid_record())?,
        topic_id,
        placement,
        actor,
        acting_for,
        text: MessageText::try_from(row.text).map_err(|_| invalid_record())?,
        references: MessageReferences::try_from(references).map_err(|_| invalid_record())?,
        activity_sequence: activity_sequence(row.activity_sequence)?,
    })
}

pub(crate) async fn load_message(
    transaction: &mut BoardTransaction<'_>,
    message_id: &MessageId,
) -> Result<Message, BoardError> {
    load_message_unattributed(transaction, message_id)
        .await
        .map_err(|error| {
            attribute_invalid_record(
                error,
                ResourceIdentity::Message {
                    message_id: message_id.clone(),
                },
            )
        })
}

fn decode_reference(row: StoredReferenceRow) -> Result<ReferenceTarget, BoardError> {
    match (row.kind.as_str(), row.target_message_id, row.target_root_id) {
        ("message", Some(message_id), None) => Ok(ReferenceTarget::Message {
            message_id: MessageId::try_from(message_id).map_err(|_| invalid_record())?,
        }),
        ("thread", None, Some(root_message_id)) => Ok(ReferenceTarget::Thread {
            root_message_id: MessageId::try_from(root_message_id).map_err(|_| invalid_record())?,
        }),
        _ => Err(invalid_record()),
    }
}
