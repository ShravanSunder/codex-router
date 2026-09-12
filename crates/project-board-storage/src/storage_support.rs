use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use project_board::{
    ActingForIdentity, BoardError, BoardErrorDetails, BoardFailureKind, BoardFailureStage,
    BoardNextAction, Identity, ReadScope, ResourceIdentity,
};
use serde::{Serialize, de::DeserializeOwned};
use sha2::Sha256;
use sqlx::{Sqlite, Transaction};

pub(crate) type BoardTransaction<'connection> = Transaction<'connection, Sqlite>;

#[derive(Debug)]
pub(crate) struct StoredIdentityRow {
    pub identity_key: String,
    pub kind: String,
    pub service_id: Option<String>,
    pub endpoint_id: Option<String>,
    pub session_id: Option<String>,
    pub human_id: Option<String>,
}

pub(crate) fn encode_cursor<TValue: Serialize>(
    cursor_key: &[u8; 32],
    value: &TValue,
) -> Result<String, BoardError> {
    let payload = serde_json::to_vec(value).map_err(|_| invalid_cursor())?;
    let mut signer = Hmac::<Sha256>::new_from_slice(cursor_key).map_err(|_| invalid_cursor())?;
    signer.update(&payload);
    let signature = signer.finalize().into_bytes();
    Ok(format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(payload),
        URL_SAFE_NO_PAD.encode(signature)
    ))
}

pub(crate) fn decode_cursor<TValue: DeserializeOwned>(
    cursor_key: &[u8; 32],
    cursor: &str,
) -> Result<TValue, BoardError> {
    let (payload, signature) = cursor.split_once('.').ok_or_else(invalid_cursor)?;
    let payload = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| invalid_cursor())?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| invalid_cursor())?;
    let mut verifier = Hmac::<Sha256>::new_from_slice(cursor_key).map_err(|_| invalid_cursor())?;
    verifier.update(&payload);
    verifier
        .verify_slice(&signature)
        .map_err(|_| invalid_cursor())?;
    serde_json::from_slice(&payload).map_err(|_| invalid_cursor())
}

pub(crate) fn storage_error(_error: sqlx::Error) -> BoardError {
    BoardError {
        kind: BoardFailureKind::BoardUnavailable,
        stage: BoardFailureStage::Storage,
        message: "Board storage is unavailable. Retry later.".to_owned(),
        next_action: BoardNextAction::RetryLater,
        details: BoardErrorDetails::None,
    }
}

pub(crate) fn invalid_record() -> BoardError {
    BoardError {
        kind: BoardFailureKind::InvalidRecord,
        stage: BoardFailureStage::Inspection,
        message: "A stored board record is invalid.".to_owned(),
        next_action: BoardNextAction::InspectResource,
        details: BoardErrorDetails::None,
    }
}

pub(crate) fn invalid_stored_identifier(field: &str, selected_scope: &str) -> BoardError {
    BoardError {
        kind: BoardFailureKind::InvalidRecord,
        stage: BoardFailureStage::Inspection,
        message: "A stored resource identifier is invalid.".to_owned(),
        next_action: BoardNextAction::InspectResource,
        details: BoardErrorDetails::FieldConstraint {
            field: field.to_owned(),
            requirement: format!(
                "Stored identifier is invalid; inspect the selected {selected_scope} scope."
            ),
        },
    }
}

pub(crate) fn attribute_invalid_record(
    error: BoardError,
    resource: ResourceIdentity,
) -> BoardError {
    if error.kind == BoardFailureKind::InvalidRecord {
        BoardError::invalid_record(resource)
    } else {
        error
    }
}

pub(crate) fn invalid_cursor() -> BoardError {
    BoardError {
        kind: BoardFailureKind::InvalidCursor,
        stage: BoardFailureStage::Validation,
        message: "The pagination cursor is malformed or does not match this query.".to_owned(),
        next_action: BoardNextAction::CorrectRequest,
        details: BoardErrorDetails::None,
    }
}

pub(crate) fn resource_not_found(resource: ResourceIdentity) -> BoardError {
    BoardError {
        kind: BoardFailureKind::ResourceNotFound,
        stage: BoardFailureStage::Storage,
        message: "The requested board resource does not exist.".to_owned(),
        next_action: BoardNextAction::CorrectRequest,
        details: BoardErrorDetails::Resource { resource },
    }
}

pub(crate) fn resource_already_exists(resource: ResourceIdentity) -> BoardError {
    BoardError {
        kind: BoardFailureKind::ResourceAlreadyExists,
        stage: BoardFailureStage::Storage,
        message: "That resource ID already exists. Inspect the existing resource before retrying."
            .to_owned(),
        next_action: BoardNextAction::InspectResource,
        details: BoardErrorDetails::Resource { resource },
    }
}

pub(crate) fn name_conflict(message: &str) -> BoardError {
    BoardError {
        kind: BoardFailureKind::NameConflict,
        stage: BoardFailureStage::Storage,
        message: message.to_owned(),
        next_action: BoardNextAction::SelectDifferentName,
        details: BoardErrorDetails::None,
    }
}

pub(crate) fn archived_board() -> BoardError {
    BoardError {
        kind: BoardFailureKind::ArchivedBoard,
        stage: BoardFailureStage::Storage,
        message: "This board is archived and read-only.".to_owned(),
        next_action: BoardNextAction::InspectResource,
        details: BoardErrorDetails::None,
    }
}

pub(crate) fn identity_key(identity: &Identity) -> String {
    match identity {
        Identity::Session { session } => format!(
            "session:{}:{}:{}",
            session.endpoint.service_id.as_str(),
            session.endpoint.endpoint_id.as_str(),
            session.session_id.as_str()
        ),
        Identity::Human { human_id } => format!("human:{}", human_id.as_str()),
    }
}

pub(crate) async fn ensure_identity(
    transaction: &mut BoardTransaction<'_>,
    identity: &Identity,
) -> Result<String, BoardError> {
    let key = identity_key(identity);
    let (kind, service_id, endpoint_id, session_id, human_id) = match identity {
        Identity::Session { session } => (
            "session",
            Some(session.endpoint.service_id.as_str()),
            Some(session.endpoint.endpoint_id.as_str()),
            Some(session.session_id.as_str()),
            None,
        ),
        Identity::Human { human_id } => ("human", None, None, None, Some(human_id.as_str())),
    };
    sqlx::query!(
        "INSERT INTO board_identities(identity_key,kind,service_id,endpoint_id,session_id,human_id) \
         VALUES(?,?,?,?,?,?) ON CONFLICT(identity_key) DO NOTHING",
        key,
        kind,
        service_id,
        endpoint_id,
        session_id,
        human_id,
    )
    .execute(&mut **transaction)
    .await
    .map_err(storage_error)?;

    let row = sqlx::query_as!(
        StoredIdentityRow,
        "SELECT identity_key,kind,service_id,endpoint_id,session_id,human_id \
         FROM board_identities WHERE identity_key=?",
        key,
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(storage_error)?;
    if decode_identity(&row)? != *identity {
        return Err(invalid_record());
    }
    Ok(key)
}

pub(crate) async fn ensure_acting_for_identity(
    transaction: &mut BoardTransaction<'_>,
    acting_for: Option<&ActingForIdentity>,
) -> Result<Option<String>, BoardError> {
    match acting_for {
        None => Ok(None),
        Some(ActingForIdentity::Human { human_id }) => ensure_identity(
            transaction,
            &Identity::Human {
                human_id: human_id.clone(),
            },
        )
        .await
        .map(Some),
    }
}

pub(crate) fn decode_identity(row: &StoredIdentityRow) -> Result<Identity, BoardError> {
    use project_board::{
        EndpointId, HumanId, ServiceId, SessionEndpointRef, SessionId, SessionRef,
    };
    let identity = match row.kind.as_str() {
        "session" if row.human_id.is_none() => Identity::Session {
            session: SessionRef {
                endpoint: SessionEndpointRef {
                    service_id: ServiceId::try_from(
                        row.service_id.clone().ok_or_else(invalid_record)?,
                    )
                    .map_err(|_| invalid_record())?,
                    endpoint_id: EndpointId::try_from(
                        row.endpoint_id.clone().ok_or_else(invalid_record)?,
                    )
                    .map_err(|_| invalid_record())?,
                },
                session_id: SessionId::try_from(row.session_id.clone().ok_or_else(invalid_record)?)
                    .map_err(|_| invalid_record())?,
            },
        },
        "human"
            if row.service_id.is_none()
                && row.endpoint_id.is_none()
                && row.session_id.is_none() =>
        {
            Identity::Human {
                human_id: HumanId::try_from(row.human_id.clone().ok_or_else(invalid_record)?)
                    .map_err(|_| invalid_record())?,
            }
        }
        _ => return Err(invalid_record()),
    };
    if identity_key(&identity) != row.identity_key {
        return Err(invalid_record());
    }
    Ok(identity)
}

pub(crate) async fn current_activity_sequence(
    transaction: &mut BoardTransaction<'_>,
) -> Result<i64, BoardError> {
    sqlx::query_scalar!("SELECT last_sequence FROM activity_checkpoint WHERE singleton=1")
        .fetch_one(&mut **transaction)
        .await
        .map_err(storage_error)
}

pub(crate) async fn allocate_activity_sequence(
    transaction: &mut BoardTransaction<'_>,
) -> Result<i64, BoardError> {
    sqlx::query_scalar!(
        "UPDATE activity_checkpoint SET last_sequence=last_sequence+1 WHERE singleton=1 \
         RETURNING last_sequence",
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(storage_error)
}

pub(crate) async fn ensure_project_reader_state(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    project_id: &str,
) -> Result<(), BoardError> {
    sqlx::query!(
        "INSERT INTO project_reader_state(reader_key,project_id,main_start,has_unread) \
         VALUES(?,?,NULL,0) ON CONFLICT(reader_key,project_id) DO NOTHING",
        reader_key,
        project_id,
    )
    .execute(&mut **transaction)
    .await
    .map_err(storage_error)?;
    Ok(())
}

pub(crate) async fn recompute_project_unread(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    project_id: &str,
) -> Result<bool, BoardError> {
    let has_unread = sqlx::query_scalar!(
        "SELECT EXISTS( \
           SELECT 1 FROM board_activity a \
           JOIN project_reader_state prs ON prs.reader_key=? AND prs.project_id=a.project_id \
           LEFT JOIN topic_read_bookmarks tb ON tb.reader_key=prs.reader_key AND tb.topic_id=a.topic_id \
           LEFT JOIN thread_watches w ON w.reader_key=prs.reader_key AND w.root_id=a.root_id \
           LEFT JOIN thread_read_bookmarks rb ON rb.reader_key=prs.reader_key AND rb.root_id=a.root_id \
           WHERE a.project_id=? AND a.actor_key<>prs.reader_key AND ( \
             (a.kind='mainMessageCreated' AND prs.main_start IS NOT NULL \
                AND a.activity_sequence>prs.main_start \
                AND a.activity_sequence>COALESCE(tb.through_activity,0)) \
             OR (a.kind<>'mainMessageCreated' AND w.active=1 \
                AND a.activity_sequence>w.starts_after_activity \
                AND a.activity_sequence>COALESCE(rb.through_activity,0)) \
           ) \
         )",
        reader_key,
        project_id,
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(storage_error)?;
    let has_unread = has_unread != 0;
    sqlx::query!(
        "UPDATE project_reader_state SET has_unread=? WHERE reader_key=? AND project_id=?",
        has_unread,
        reader_key,
        project_id
    )
    .execute(&mut **transaction)
    .await
    .map_err(storage_error)?;
    Ok(has_unread)
}

pub(crate) async fn project_for_scope(
    transaction: &mut BoardTransaction<'_>,
    scope: &ReadScope,
) -> Result<String, BoardError> {
    let project_id = match scope {
        ReadScope::Topic { topic_id } => sqlx::query_scalar!(
            "SELECT b.project_id FROM board_topics t JOIN project_boards b ON b.board_id=t.board_id \
             WHERE t.topic_id=?", topic_id.as_str(),
        )
        .fetch_optional(&mut **transaction)
        .await
        .map_err(storage_error)?,
        ReadScope::Thread { root_message_id } => sqlx::query_scalar!(
            "SELECT b.project_id FROM board_threads th JOIN board_messages m ON m.message_id=th.root_id \
             JOIN project_boards b ON b.board_id=m.board_id WHERE th.root_id=?", root_message_id.as_str(),
        )
        .fetch_optional(&mut **transaction)
        .await
        .map_err(storage_error)?,
    };
    project_id.ok_or_else(|| match scope {
        ReadScope::Topic { topic_id } => resource_not_found(ResourceIdentity::Topic {
            topic_id: topic_id.clone(),
        }),
        ReadScope::Thread { root_message_id } => resource_not_found(ResourceIdentity::Thread {
            root_message_id: root_message_id.clone(),
        }),
    })
}
