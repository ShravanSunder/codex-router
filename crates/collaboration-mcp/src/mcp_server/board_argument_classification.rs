//! Typed rejections for board tool arguments that do not decode.
//!
//! A board tool's arguments decode straight into the board request type. When they don't, the
//! decoder's own message can name supplied keys and variants, so the tool classifies the
//! arguments instead: the rejection names the field and its requirement and never repeats a
//! supplied value, key or variant.
use message_board::*;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, de::DeserializeOwned};
use serde_json::{Map, Value};
use std::{borrow::Cow, marker::PhantomData};

const IDENTITY_REQUIREMENT: &str = "must be a human identity with a 1 to 4096 byte humanId without NUL, or a session identity with a canonical service UUID, 1 to 64 byte lowercase endpointId, and 1 to 4096 byte sessionId without NUL";

/// Decodes a board tool's arguments, or classifies them into the board's typed rejection.
pub(super) fn decode_board_arguments<TRequest: DeserializeOwned>(
    tool: &str,
    arguments: Map<String, Value>,
) -> Result<TRequest, BoardError> {
    let arguments = Value::Object(arguments);
    TRequest::deserialize(&arguments).map_err(|_| classify_board_arguments(tool, &arguments))
}

/// A board tool's arguments left undecoded, for a tool whose handler decodes them itself.
/// It publishes `TRequest`'s schema as the tool's input schema.
pub(super) struct UndecodedBoardArguments<TRequest> {
    arguments: Map<String, Value>,
    request: PhantomData<fn() -> TRequest>,
}

impl<TRequest: DeserializeOwned> UndecodedBoardArguments<TRequest> {
    pub(super) fn decode(self, tool: &str) -> Result<TRequest, BoardError> {
        decode_board_arguments(tool, self.arguments)
    }
}

impl<'de, TRequest> Deserialize<'de> for UndecodedBoardArguments<TRequest> {
    fn deserialize<TDeserializer: Deserializer<'de>>(
        deserializer: TDeserializer,
    ) -> Result<Self, TDeserializer::Error> {
        Map::deserialize(deserializer).map(|arguments| Self {
            arguments,
            request: PhantomData,
        })
    }
}

impl<TRequest: JsonSchema> JsonSchema for UndecodedBoardArguments<TRequest> {
    fn inline_schema() -> bool {
        TRequest::inline_schema()
    }

    fn schema_name() -> Cow<'static, str> {
        TRequest::schema_name()
    }

    fn schema_id() -> Cow<'static, str> {
        TRequest::schema_id()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        TRequest::json_schema(generator)
    }
}

/// The typed rejection for `tool`'s `arguments`, which did not decode.
pub(super) fn classify_board_arguments(tool: &str, arguments: &Value) -> BoardError {
    classify_failure(tool, arguments).unwrap_or_else(|| malformed_request(tool))
}

fn classify_failure(tool: &str, arguments: &Value) -> Option<BoardError> {
    let Some(fields) = arguments.as_object() else {
        return Some(invalid_field("request", "must be a JSON object"));
    };
    validate_required_identities(tool, fields)
        .err()
        .or_else(|| validate_optional_identity(fields, "actor", false).err())
        .or_else(|| validate_optional_identity(fields, "reader", false).err())
        .or_else(|| validate_optional_identity(fields, "actingFor", true).err())
        .or_else(|| validate_known_uuid_v7_fields(fields).err())
        .or_else(|| validate_known_activity_positions(fields).err())
        .or_else(|| validate_metadata_fields(tool, fields).err())
        .or_else(|| validate_page(fields).err())
        .or_else(|| validate_repository(fields).err())
        .or_else(|| validate_closed_variants(tool, fields).err())
}

fn malformed_request(tool: &str) -> BoardError {
    invalid_field(
        "request",
        format!(
            "must contain the documented camelCase fields and supported kind variants for {tool}"
        ),
    )
}

fn validate_required_identities(tool: &str, fields: &Map<String, Value>) -> Result<(), BoardError> {
    const ACTOR_METHODS: &[&str] = &[
        "board_project_create",
        "board_project_update",
        "board_repository_attach",
        "board_repository_detach",
        "board_create",
        "board_update",
        "board_archive",
        "board_topic_create",
        "board_topic_update",
        "board_message_post",
        "board_thread_create",
        "board_thread_join",
        "board_thread_leave",
        "board_thread_resolve",
        "board_thread_unresolve",
        "board_thread_watch",
        "board_thread_unwatch",
        "board_topic_watch",
        "board_topic_unwatch",
        "board_inbox_acknowledge",
        "board_thread_subscribe",
        "board_thread_unsubscribe",
        "board_thread_subscriptions",
        "board_thread_wait",
    ];
    const READER_METHODS: &[&str] = &[
        "board_thread_list",
        "board_inbox_fetch",
        "board_inbox_projects",
    ];
    if ACTOR_METHODS.contains(&tool) && fields.get("actor").is_none_or(Value::is_null) {
        return Err(invalid_identity("actor", IDENTITY_REQUIREMENT));
    }
    if READER_METHODS.contains(&tool) && fields.get("reader").is_none_or(Value::is_null) {
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

fn validate_metadata_fields(tool: &str, fields: &Map<String, Value>) -> Result<(), BoardError> {
    if let Some(value) = fields.get("name") {
        let valid = value
            .as_str()
            .and_then(|value| ResourceName::try_from(value.to_owned()).ok())
            .is_some();
        if !valid {
            if matches!(tool, "board_topic_create" | "board_topic_update") {
                return Err(invalid_topic_name(
                    "must trim to 1 through 256 UTF-8 bytes and be unique within its board",
                ));
            }
            return Err(invalid_field(
                "name",
                "must trim to 1 through 256 UTF-8 bytes",
            ));
        }
    } else if matches!(tool, "board_topic_create" | "board_topic_update") {
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
    let Some(repository) = fields.get("repository") else {
        return Ok(());
    };
    if repository.is_null() {
        return Ok(());
    }
    if let Some(repository_fields) = repository.as_object() {
        for (field, constructor_valid) in [
            (
                "normalizedOrigin",
                repository_fields
                    .get("normalizedOrigin")
                    .and_then(Value::as_str)
                    .map(|value| NormalizedOrigin::try_from(value.to_owned()).is_ok()),
            ),
            (
                "commonDirectory",
                repository_fields
                    .get("commonDirectory")
                    .and_then(Value::as_str)
                    .map(|value| CommonDirectory::try_from(value.to_owned()).is_ok()),
            ),
        ] {
            if constructor_valid == Some(false) {
                return Err(invalid_field(
                    format!("repository.{field}"),
                    "must be canonical and contain at most 4096 UTF-8 bytes",
                ));
            }
        }
    }
    if serde_json::from_value::<RepositoryRef>(repository.clone()).is_err() {
        return Err(invalid_field(
            "repository",
            "must be a canonical origin or local repository reference",
        ));
    }
    Ok(())
}

fn validate_closed_variants(tool: &str, fields: &Map<String, Value>) -> Result<(), BoardError> {
    validate_variant::<Placement>(
        fields,
        "placement",
        "must be topic or thread with its required UUIDv7 field",
    )?;
    if tool == "board_discovery_search" || tool == "board_message_search" {
        if fields
            .get("query")
            .and_then(Value::as_str)
            .and_then(|value| SearchQuery::try_from(value.to_owned()).ok())
            .is_none()
        {
            return Err(invalid_field(
                "query",
                "must contain 1 to 256 UTF-8 bytes after trimming",
            ));
        }
        if !fields.contains_key("scope") {
            return Err(invalid_field("scope", "is required"));
        }
        if tool == "board_discovery_search" {
            validate_variant::<DiscoveryScope>(
                fields,
                "scope",
                "must select allProjects, project, or board",
            )?;
            validate_variant::<DiscoveryKind>(
                fields,
                "kind",
                "must be all, project, board, or topic",
            )?;
        } else {
            validate_variant::<MessageListScope>(
                fields,
                "scope",
                "must select allProjects, project, board, topic, or thread",
            )?;
            validate_variant::<SearchMessageKind>(
                fields,
                "kind",
                "must be both, topLevel, or thread",
            )?;
        }
    } else if tool == "board_message_list" {
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
    } else if tool == "board_inbox_fetch" {
        if !fields.contains_key("scope") {
            return Err(invalid_field(
                "scope",
                "is required and must select project, board, or topic",
            ));
        }
        validate_variant::<InboxScope>(
            fields,
            "scope",
            "must be project, board, or topic with its required UUIDv7 field",
        )?;
        validate_variant::<InboxReadMode>(fields, "readMode", "must be unread or latest")?;
    } else if tool == "board_inbox_acknowledge" {
        validate_variant::<ReadScope>(
            fields,
            "scope",
            "must be topic or thread with its required UUIDv7 field",
        )?;
    } else if tool == "board_thread_subscribe" {
        if !fields.contains_key("scope") {
            return Err(invalid_field("scope", "is required"));
        }
        validate_variant::<SubscriptionScope>(
            fields,
            "scope",
            "must select thread or topic with its required UUIDv7 field",
        )?;
        if !fields.contains_key("policy") {
            return Err(invalid_field("policy", "is required"));
        }
        validate_variant::<SubscriptionPolicyPatch>(
            fields,
            "policy",
            "must contain supported optional mode, idle, timing and lifetime fields",
        )?;
    } else if tool == "board_thread_unsubscribe" {
        if !fields.contains_key("scope") {
            return Err(invalid_field("scope", "is required"));
        }
        validate_variant::<SubscriptionScope>(
            fields,
            "scope",
            "must select thread or topic with its required UUIDv7 field",
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
