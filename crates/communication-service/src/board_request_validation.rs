//! Safe board request classification before typed Serde admission.
use project_board::*;
use serde_json::{Map, Value};

const IDENTITY_REQUIREMENT: &str = "must be a human identity with a 1 to 4096 byte humanId without NUL, or a session identity with a canonical service UUID, 1 to 64 byte lowercase endpointId, and 1 to 4096 byte sessionId without NUL";

pub(crate) fn classify(method: &str, params: &Value) -> BoardError {
    classify_failure(method, params).unwrap_or_else(|| malformed_request(method))
}

fn classify_failure(method: &str, params: &Value) -> Option<BoardError> {
    let Some(fields) = params.as_object() else {
        return Some(invalid_field("request", "must be a JSON object"));
    };
    validate_required_identities(method, fields)
        .err()
        .or_else(|| validate_optional_identity(fields, "actor", false).err())
        .or_else(|| validate_optional_identity(fields, "reader", false).err())
        .or_else(|| validate_optional_identity(fields, "actingFor", true).err())
        .or_else(|| validate_known_uuid_v7_fields(fields).err())
        .or_else(|| validate_known_activity_positions(fields).err())
        .or_else(|| validate_metadata_fields(method, fields).err())
        .or_else(|| validate_page(fields).err())
        .or_else(|| validate_repository(fields).err())
        .or_else(|| validate_closed_variants(method, fields).err())
}

fn malformed_request(method: &str) -> BoardError {
    invalid_field(
        "request",
        format!(
            "must contain the documented camelCase fields and supported kind variants for {method}"
        ),
    )
}

fn validate_required_identities(
    method: &str,
    fields: &Map<String, Value>,
) -> Result<(), BoardError> {
    const ACTOR_METHODS: &[&str] = &[
        "board/projectCreate",
        "board/projectUpdate",
        "board/repositoryAttach",
        "board/repositoryDetach",
        "board/create",
        "board/update",
        "board/archive",
        "board/topicCreate",
        "board/topicUpdate",
        "board/messagePost",
        "board/threadResolve",
        "board/threadUnresolve",
        "board/threadWatch",
        "board/threadUnwatch",
        "board/inboxAcknowledge",
    ];
    const READER_METHODS: &[&str] = &[
        "board/threadList",
        "board/inboxFetch",
        "board/inboxProjects",
    ];
    if ACTOR_METHODS.contains(&method) && fields.get("actor").is_none_or(Value::is_null) {
        return Err(invalid_identity("actor", IDENTITY_REQUIREMENT));
    }
    if READER_METHODS.contains(&method) && fields.get("reader").is_none_or(Value::is_null) {
        return Err(invalid_identity("reader", IDENTITY_REQUIREMENT));
    }
    Ok(())
}

fn validate_optional_identity(
    fields: &Map<String, Value>,
    field: &'static str,
    acting_for: bool,
) -> Result<(), BoardError> {
    let Some(value) = fields.get(field) else {
        return Ok(());
    };
    if value.is_null() && field != "actor" {
        return Ok(());
    }
    let valid = if acting_for {
        serde_json::from_value::<ActingForIdentity>(value.clone()).is_ok()
    } else {
        serde_json::from_value::<Identity>(value.clone()).is_ok()
    };
    if !valid {
        return Err(invalid_identity(
            field,
            if acting_for {
                "must be null or a human identity with a 1 to 4096 byte humanId without NUL"
            } else {
                IDENTITY_REQUIREMENT
            },
        ));
    }
    Ok(())
}

fn validate_known_uuid_v7_fields(fields: &Map<String, Value>) -> Result<(), BoardError> {
    for field in [
        "projectId",
        "boardId",
        "topicId",
        "messageId",
        "rootMessageId",
    ] {
        validate_uuid_field(fields.get(field), field)?;
    }
    for (container_name, id_fields) in [
        ("placement", &["topicId", "rootMessageId"][..]),
        (
            "scope",
            &["topicId", "rootMessageId", "boardId", "projectId"][..],
        ),
    ] {
        if let Some(container) = fields.get(container_name).and_then(Value::as_object) {
            for field in id_fields {
                validate_uuid_field(container.get(*field), &format!("{container_name}.{field}"))?;
            }
        }
    }
    if let Some(references) = fields.get("references").and_then(Value::as_array) {
        for (index, reference) in references.iter().enumerate() {
            if let Some(reference) = reference.as_object() {
                for field in ["messageId", "rootMessageId"] {
                    validate_uuid_field(
                        reference.get(field),
                        &format!("references[{index}].{field}"),
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn validate_uuid_field(value: Option<&Value>, field: &str) -> Result<(), BoardError> {
    if let Some(value) = value
        && value
            .as_str()
            .and_then(|value| MessageId::try_from(value.to_owned()).ok())
            .is_none()
    {
        return Err(invalid_field(
            field,
            "must be a canonical lowercase RFC UUIDv7",
        ));
    }
    Ok(())
}

fn validate_known_activity_positions(fields: &Map<String, Value>) -> Result<(), BoardError> {
    validate_activity_field(
        fields.get("throughActivitySequence"),
        "throughActivitySequence",
    )?;
    if let Some(selection) = fields.get("selection").and_then(Value::as_object) {
        for field in [
            "afterActivitySequence",
            "fromActivitySequence",
            "toActivitySequence",
        ] {
            validate_activity_field(selection.get(field), &format!("selection.{field}"))?;
        }
    }
    Ok(())
}

fn validate_activity_field(value: Option<&Value>, field: &str) -> Result<(), BoardError> {
    if let Some(value) = value
        && value
            .as_u64()
            .and_then(|value| ActivitySequence::try_from(value).ok())
            .is_none()
    {
        return Err(invalid_field(
            field,
            "must be an integer from 0 through 9223372036854775807",
        ));
    }
    Ok(())
}

fn validate_metadata_fields(method: &str, fields: &Map<String, Value>) -> Result<(), BoardError> {
    if let Some(value) = fields.get("name") {
        let valid = value
            .as_str()
            .and_then(|value| ResourceName::try_from(value.to_owned()).ok())
            .is_some();
        if !valid {
            if matches!(method, "board/topicCreate" | "board/topicUpdate") {
                return Err(invalid_topic_name(
                    "must trim to 1 through 256 UTF-8 bytes and be unique within its board",
                ));
            }
            return Err(invalid_field(
                "name",
                "must trim to 1 through 256 UTF-8 bytes",
            ));
        }
    } else if matches!(method, "board/topicCreate" | "board/topicUpdate") {
        return Err(invalid_topic_name(
            "is required and must trim to 1 through 256 UTF-8 bytes",
        ));
    }
    if let Some(value) = fields.get("description")
        && value
            .as_str()
            .and_then(|value| Description::try_from(value.to_owned()).ok())
            .is_none()
    {
        return Err(invalid_field(
            "description",
            "must contain at most 16384 UTF-8 bytes",
        ));
    }
    if let Some(value) = fields.get("text")
        && value
            .as_str()
            .and_then(|value| MessageText::try_from(value.to_owned()).ok())
            .is_none()
    {
        return Err(invalid_field(
            "text",
            "must contain 1 through 65536 UTF-8 bytes",
        ));
    }
    Ok(())
}

fn validate_page(fields: &Map<String, Value>) -> Result<(), BoardError> {
    let Some(page) = fields.get("page") else {
        return Ok(());
    };
    if let Some(limit) = page.get("limit")
        && limit
            .as_u64()
            .and_then(|limit| u32::try_from(limit).ok())
            .and_then(|limit| PageLimit::try_from(limit).ok())
            .is_none()
    {
        return Err(invalid_field(
            "page.limit",
            "must be an integer from 1 through 100",
        ));
    }
    if serde_json::from_value::<PageRequest>(page.clone()).is_err() {
        return Err(invalid_field(
            "page",
            "must contain only a bounded limit and optional opaque cursor",
        ));
    }
    Ok(())
}

fn validate_repository(fields: &Map<String, Value>) -> Result<(), BoardError> {
    if let Some(repository) = fields.get("repository")
        && !repository.is_null()
        && serde_json::from_value::<RepositoryRef>(repository.clone()).is_err()
    {
        return Err(invalid_field(
            "repository",
            "must be a canonical origin or local repository reference",
        ));
    }
    Ok(())
}

fn validate_closed_variants(method: &str, fields: &Map<String, Value>) -> Result<(), BoardError> {
    validate_variant::<Placement>(
        fields,
        "placement",
        "must be topic or thread with its required UUIDv7 field",
    )?;
    if method == "board/messageList" {
        validate_variant::<MessageListScope>(
            fields,
            "scope",
            "must be topic, thread, board, project, or allProjects",
        )?;
        validate_variant::<MessageSelection>(
            fields,
            "selection",
            "must be latest, afterPosition, or an ordered inclusive range",
        )?;
    } else if method == "board/inboxAcknowledge" {
        validate_variant::<ReadScope>(
            fields,
            "scope",
            "must be topic or thread with its required UUIDv7 field",
        )?;
    }
    if let Some(references) = fields.get("references")
        && serde_json::from_value::<MessageReferences>(references.clone()).is_err()
    {
        return Err(invalid_field(
            "references",
            "must contain at most 64 distinct message or thread targets",
        ));
    }
    Ok(())
}

fn validate_variant<TValue: serde::de::DeserializeOwned>(
    fields: &Map<String, Value>,
    field: &'static str,
    requirement: &'static str,
) -> Result<(), BoardError> {
    if let Some(value) = fields.get(field)
        && serde_json::from_value::<TValue>(value.clone()).is_err()
    {
        return Err(invalid_field(field, requirement));
    }
    Ok(())
}

fn invalid_field(field: impl Into<String>, requirement: impl Into<String>) -> BoardError {
    BoardError::invalid_field(field, requirement)
}

fn invalid_identity(field: &'static str, requirement: &'static str) -> BoardError {
    BoardError {
        kind: BoardFailureKind::InvalidIdentity,
        stage: BoardFailureStage::Validation,
        message: format!("Correct {field}: {requirement}."),
        next_action: BoardNextAction::CorrectRequest,
        details: BoardErrorDetails::FieldConstraint {
            field: field.to_owned(),
            requirement: requirement.to_owned(),
        },
    }
}

fn invalid_topic_name(requirement: &'static str) -> BoardError {
    BoardError {
        kind: BoardFailureKind::InvalidTopicName,
        stage: BoardFailureStage::Validation,
        message: format!("Correct name: {requirement}."),
        next_action: BoardNextAction::SelectDifferentName,
        details: BoardErrorDetails::FieldConstraint {
            field: "name".to_owned(),
            requirement: requirement.to_owned(),
        },
    }
}
