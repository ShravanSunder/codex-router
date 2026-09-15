//! Checked Participant row loading and Rust validation of closed persisted sets.
use crate::message_records::{activity_sequence, load_identity};
use crate::storage_support::{BoardTransaction, identity_key, invalid_record, storage_error};
use message_board::*;

pub(crate) struct StoredParticipantRow {
    pub(crate) reader_key: String,
    pub(crate) root_id: String,
    pub(crate) role: String,
    pub(crate) note: Option<String>,
    pub(crate) joined_at_activity: i64,
    pub(crate) last_seen_activity: i64,
    pub(crate) closed_at_activity: Option<i64>,
    pub(crate) closed_reason: Option<String>,
    pub(crate) replaced_by: Option<String>,
}

pub(crate) fn role_name(role: ParticipantRole) -> &'static str {
    match role {
        ParticipantRole::Orchestrator => "orchestrator",
        ParticipantRole::Advisor => "advisor",
        ParticipantRole::Reviewer => "reviewer",
        ParticipantRole::Participant => "participant",
    }
}

fn decode_role(role: &str) -> Result<ParticipantRole, BoardError> {
    match role {
        "orchestrator" => Ok(ParticipantRole::Orchestrator),
        "advisor" => Ok(ParticipantRole::Advisor),
        "reviewer" => Ok(ParticipantRole::Reviewer),
        "participant" => Ok(ParticipantRole::Participant),
        _ => Err(invalid_record()),
    }
}

fn decode_closed_reason(
    reason: Option<&str>,
) -> Result<Option<ParticipantClosedReason>, BoardError> {
    match reason {
        None => Ok(None),
        Some("left") => Ok(Some(ParticipantClosedReason::Left)),
        Some("replaced") => Ok(Some(ParticipantClosedReason::Replaced)),
        Some("resolved") => Ok(Some(ParticipantClosedReason::Resolved)),
        Some(_) => Err(invalid_record()),
    }
}

async fn stored_participant(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    root_message_id: &MessageId,
) -> Result<Option<StoredParticipantRow>, BoardError> {
    sqlx::query_as!(
        StoredParticipantRow,
        "SELECT reader_key,root_id,role,note,joined_at_activity,last_seen_activity,closed_at_activity,closed_reason,replaced_by \
         FROM thread_participants WHERE reader_key=? AND root_id=?",
        reader_key,
        root_message_id.as_str(),
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage_error)
}

async fn activity_is_on_thread(
    transaction: &mut BoardTransaction<'_>,
    sequence: i64,
    root_message_id: &MessageId,
) -> Result<bool, BoardError> {
    sqlx::query_scalar!(
        "SELECT EXISTS(SELECT 1 FROM board_activity WHERE activity_sequence=? AND root_id=?)",
        sequence,
        root_message_id.as_str(),
    )
    .fetch_one(&mut **transaction)
    .await
    .map(|exists| exists != 0)
    .map_err(storage_error)
}

pub(crate) async fn decode_participant(
    transaction: &mut BoardTransaction<'_>,
    row: StoredParticipantRow,
) -> Result<Participant, BoardError> {
    let root_message_id = MessageId::try_from(row.root_id.clone()).map_err(|_| invalid_record())?;
    for sequence in [
        Some(row.joined_at_activity),
        Some(row.last_seen_activity),
        row.closed_at_activity,
    ]
    .into_iter()
    .flatten()
    {
        if !activity_is_on_thread(transaction, sequence, &root_message_id).await? {
            return Err(BoardError::invalid_record(ResourceIdentity::Thread {
                root_message_id,
            }));
        }
    }
    let identity = load_identity(transaction, &row.reader_key).await?;
    let replaced_by = match row.replaced_by.clone() {
        Some(key) => Some(load_identity(transaction, &key).await?),
        None => None,
    };
    decode_participant_with_identity(row, identity, replaced_by)
}

pub(crate) fn decode_projected_participant(
    row: StoredParticipantRow,
    identity: Identity,
    joined_activity_on_thread: bool,
    last_seen_activity_on_thread: bool,
    closed_activity_on_thread: bool,
) -> Result<Participant, BoardError> {
    let root_message_id = MessageId::try_from(row.root_id.clone()).map_err(|_| invalid_record())?;
    if !joined_activity_on_thread
        || !last_seen_activity_on_thread
        || (row.closed_at_activity.is_some() && !closed_activity_on_thread)
    {
        return Err(BoardError::invalid_record(ResourceIdentity::Thread {
            root_message_id,
        }));
    }
    decode_participant_with_identity(row, identity, None)
}

fn decode_participant_with_identity(
    row: StoredParticipantRow,
    identity: Identity,
    replaced_by: Option<Identity>,
) -> Result<Participant, BoardError> {
    let root_message_id = MessageId::try_from(row.root_id).map_err(|_| invalid_record())?;
    if identity_key(&identity) != row.reader_key
        || row.replaced_by.is_some() != replaced_by.is_some()
    {
        return Err(BoardError::invalid_record(ResourceIdentity::Thread {
            root_message_id,
        }));
    }
    Participant::new(
        identity,
        decode_role(&row.role)?,
        row.note
            .map(ParticipantNote::try_from)
            .transpose()
            .map_err(|_| invalid_record())?,
        activity_sequence(row.joined_at_activity)?,
        activity_sequence(row.last_seen_activity)?,
        row.closed_at_activity.map(activity_sequence).transpose()?,
        decode_closed_reason(row.closed_reason.as_deref())?,
        replaced_by,
    )
    .map_err(|_| BoardError::invalid_record(ResourceIdentity::Thread { root_message_id }))
}

pub(crate) async fn load_participant(
    transaction: &mut BoardTransaction<'_>,
    identity: &Identity,
    root_message_id: &MessageId,
) -> Result<Option<Participant>, BoardError> {
    let key = identity_key(identity);
    match stored_participant(transaction, &key, root_message_id).await? {
        Some(row) => decode_participant(transaction, row).await.map(Some),
        None => Ok(None),
    }
}

pub(crate) async fn require_open_participant(
    transaction: &mut BoardTransaction<'_>,
    identity: &Identity,
    root_message_id: &MessageId,
) -> Result<Participant, BoardError> {
    match load_participant(transaction, identity, root_message_id).await? {
        Some(participant) if participant.is_open() => Ok(participant),
        _ => Err(BoardError::participant_required(
            root_message_id.clone(),
            identity.clone(),
        )),
    }
}

pub(crate) async fn load_orchestrator(
    transaction: &mut BoardTransaction<'_>,
    root_message_id: &MessageId,
) -> Result<Option<OrchestratorHolder>, BoardError> {
    let row = sqlx::query_as!(
        StoredParticipantRow,
        "SELECT reader_key,root_id,role,note,joined_at_activity,last_seen_activity,closed_at_activity,closed_reason,replaced_by \
         FROM thread_participants WHERE root_id=? AND role='orchestrator' AND closed_at_activity IS NULL",
        root_message_id.as_str(),
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage_error)?;
    match row {
        Some(row) => {
            let participant = decode_participant(transaction, row).await?;
            Ok(Some(OrchestratorHolder {
                identity: participant.identity,
                last_seen_activity: participant.last_seen_activity,
            }))
        }
        None => Ok(None),
    }
}
