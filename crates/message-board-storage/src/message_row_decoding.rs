//! Typed SQL rows and domain validation for immutable messages.
use crate::message_records::{activity_sequence, load_identity, require_thread};
use crate::storage_support::{
    BoardTransaction, attribute_invalid_record, invalid_record, resource_not_found, storage_error,
};
use message_board::*;

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
    posted_from_activity: Option<i64>,
    participant_activity_sequence: Option<i64>,
    participant_root_id: Option<String>,
    participant_kind: Option<String>,
    participant_key: Option<String>,
    participant_role: Option<String>,
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
                a.root_id AS activity_root_id, m.posted_from_activity, \
                pa.activity_sequence AS \"participant_activity_sequence?\", pa.root_id AS participant_root_id, \
                pa.kind AS \"participant_kind?\", pa.participant_key, pa.participant_role \
         FROM board_messages m \
         JOIN board_activity a ON a.message_id=m.message_id \
         LEFT JOIN board_activity pa ON pa.activity_sequence=m.posted_from_activity \
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
    let posted_as_role = decode_posted_as_role(&row, &actor)?;
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
        posted_as_role,
        text: MessageText::try_from(escape_stored_controls(row.text))
            .map_err(|_| invalid_record())?,
        references: MessageReferences::try_from(references).map_err(|_| invalid_record())?,
        activity_sequence: activity_sequence(row.activity_sequence)?,
    })
}

/// Escapes C0 controls in stored text so rows written before the write-boundary
/// rejection still decode and still read back as valid JSON.
///
/// Newline and tab are layout and stay literal; everything else becomes its
/// `\u00XX` escape rather than a raw byte in the reader's output.
fn escape_stored_controls(text: String) -> String {
    if !text
        .chars()
        .any(|character| character <= '\u{001f}' && !matches!(character, '\n' | '\t'))
    {
        return text;
    }
    text.chars()
        .map(|character| {
            if character <= '\u{001f}' && !matches!(character, '\n' | '\t') {
                format!("\\u{:04x}", character as u32)
            } else {
                character.to_string()
            }
        })
        .collect()
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

fn decode_posted_as_role(
    row: &StoredMessageRow,
    actor: &Identity,
) -> Result<Option<ParticipantRole>, BoardError> {
    let Some(sequence) = row.posted_from_activity else {
        return Ok(None);
    };
    let expected_root = row.root_id.as_deref().unwrap_or(&row.message_id);
    if !matches!(actor, Identity::Session { .. })
        || row.participant_activity_sequence != Some(sequence)
        || row.participant_key.as_deref() != Some(row.actor_key.as_str())
        || row.participant_root_id.as_deref() != Some(expected_root)
        || (row.root_id.is_some() && sequence >= row.activity_sequence)
        || (row.root_id.is_none() && row.participant_kind.as_deref() != Some("participantJoined"))
    {
        return Err(invalid_record());
    }
    let role = row.participant_role.as_deref().ok_or_else(invalid_record)?;
    crate::participant_row_decoding::decode_role(role).map(Some)
}
