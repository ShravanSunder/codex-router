use super::{board_arguments::*, board_preparation::CommandContext};
use collaboration_client::board::*;
use collaboration_client::{BoardRepositoryError, BoardRepositoryLocation};
use std::{fs::File, io::Read, path::Path};

pub(super) fn message_scope(arguments: &MessageListArguments) -> Result<MessageListScope, String> {
    let supplied = [
        arguments.topic_id.is_some(),
        arguments.root_message_id.is_some(),
        arguments.board_id.is_some(),
        arguments.project_id.is_some(),
    ]
    .into_iter()
    .filter(|value| *value)
    .count();
    match arguments.scope {
        MessageScopeKind::Topic if supplied == 1 => arguments
            .topic_id
            .clone()
            .map(|value| parse_uuid_v7(value, "--topic-id"))
            .transpose()?
            .map(|topic_id| MessageListScope::Topic { topic_id }),
        MessageScopeKind::Thread if supplied == 1 => arguments
            .root_message_id
            .clone()
            .map(|value| parse_uuid_v7(value, "--root-message-id"))
            .transpose()?
            .map(|root_message_id| MessageListScope::Thread { root_message_id }),
        MessageScopeKind::Board if supplied == 1 => arguments
            .board_id
            .clone()
            .map(|value| parse_uuid_v7(value, "--board-id"))
            .transpose()?
            .map(|board_id| MessageListScope::Board { board_id }),
        MessageScopeKind::Project if supplied == 1 => arguments
            .project_id
            .clone()
            .map(|value| parse_uuid_v7(value, "--project-id"))
            .transpose()?
            .map(|project_id| MessageListScope::Project { project_id }),
        MessageScopeKind::AllProjects if supplied == 0 => Some(MessageListScope::AllProjects),
        _ => None,
    }
    .ok_or_else(|| {
        "--scope requires exactly its matching ID flag (or no ID for all-projects)".into()
    })
}

pub(super) fn message_selection(
    arguments: &MessageListArguments,
) -> Result<MessageSelection, String> {
    match arguments.selection {
        MessageSelectionKind::Latest
            if arguments.after_activity_sequence.is_none()
                && arguments.from_activity_sequence.is_none()
                && arguments.to_activity_sequence.is_none() =>
        {
            Ok(MessageSelection::Latest)
        }
        MessageSelectionKind::AfterPosition
            if arguments.after_activity_sequence.is_some()
                && arguments.from_activity_sequence.is_none()
                && arguments.to_activity_sequence.is_none() =>
        {
            Ok(MessageSelection::AfterPosition {
                after_activity_sequence: activity_sequence(
                    arguments.after_activity_sequence.unwrap_or_default(),
                    "--after-activity-sequence",
                )?,
            })
        }
        MessageSelectionKind::Range
            if arguments.after_activity_sequence.is_none()
                && arguments.from_activity_sequence.is_some()
                && arguments.to_activity_sequence.is_some() =>
        {
            Ok(MessageSelection::Range {
                from_activity_sequence: activity_sequence(
                    arguments.from_activity_sequence.unwrap_or_default(),
                    "--from-activity-sequence",
                )?,
                to_activity_sequence: activity_sequence(
                    arguments.to_activity_sequence.unwrap_or_default(),
                    "--to-activity-sequence",
                )?,
            })
        }
        _ => Err("--selection latest takes no bounds; after-position requires --after-activity-sequence; range requires --from-activity-sequence and --to-activity-sequence".into()),
    }
}

pub(super) fn activity_sequence(value: u64, flag: &str) -> Result<ActivitySequence, String> {
    value
        .try_into()
        .map_err(|_| format!("{flag} must not exceed the maximum SQLite activity position"))
}

pub(super) fn parse_duration_seconds(value: &str, flag: &str) -> Result<u64, String> {
    let error = || format!("{flag} requires a positive integer followed by s, m, h, or d");
    let (number, multiplier) = if let Some(number) = value.strip_suffix('s') {
        (number, 1_u64)
    } else if let Some(number) = value.strip_suffix('m') {
        (number, 60)
    } else if let Some(number) = value.strip_suffix('h') {
        (number, 60 * 60)
    } else if let Some(number) = value.strip_suffix('d') {
        (number, 24 * 60 * 60)
    } else {
        return Err(error());
    };
    let seconds = number
        .parse::<u64>()
        .map_err(|_| error())?
        .checked_mul(multiplier)
        .ok_or_else(error)?;
    if seconds == 0 {
        return Err(error());
    }
    Ok(seconds)
}

pub(super) fn parse_mutation_identity(
    arguments: &MutationIdentityArguments,
) -> Result<(Identity, Option<ActingForIdentity>), String> {
    Ok((
        parse_identity(&arguments.actor, "--actor")?,
        arguments
            .acting_for
            .as_deref()
            .map(parse_acting_for)
            .transpose()?,
    ))
}

pub(super) fn parse_identity(value: &str, flag: &str) -> Result<Identity, String> {
    if value == "self" {
        return serde_json::from_value(
            serde_json::json!({"kind":"human","humanId":"__agent_collaboration_self__"}),
        )
        .map_err(|_| format!("{flag} self could not be represented"));
    }
    serde_json::from_str(value)
        .map_err(|_| format!("{flag} must be valid typed Identity JSON with kind session or human"))
}

pub(super) fn parse_acting_for(value: &str) -> Result<ActingForIdentity, String> {
    serde_json::from_str(value).map_err(|_| "--acting-for must be valid human identity JSON".into())
}

pub(super) fn parse_uuid_v7<TIdentity>(value: String, flag: &str) -> Result<TIdentity, String>
where
    TIdentity: TryFrom<String>,
{
    TIdentity::try_from(value).map_err(|_| format!("{flag} must be a canonical lowercase UUIDv7"))
}

pub(super) fn parse_or_generate_uuid_v7<TIdentity>(
    value: Option<String>,
    flag: &str,
) -> Result<TIdentity, String>
where
    TIdentity: TryFrom<String> + GeneratedIdentity,
{
    value
        .map(|value| parse_uuid_v7(value, flag))
        .transpose()
        .map(|value| value.unwrap_or_else(TIdentity::generate))
}

pub(super) trait GeneratedIdentity {
    fn generate() -> Self;
}
impl GeneratedIdentity for ProjectId {
    fn generate() -> Self {
        ProjectId::generate()
    }
}
impl GeneratedIdentity for BoardId {
    fn generate() -> Self {
        BoardId::generate()
    }
}
impl GeneratedIdentity for TopicId {
    fn generate() -> Self {
        TopicId::generate()
    }
}
impl GeneratedIdentity for MessageId {
    fn generate() -> Self {
        MessageId::generate()
    }
}

pub(super) fn parse_bounded_text<TText>(value: String, flag: &str) -> Result<TText, String>
where
    TText: TryFrom<String>,
    TText::Error: std::fmt::Display,
{
    TText::try_from(value).map_err(|domain_error| {
        let requirement = match flag {
            "--query" => "must contain 1 to 256 UTF-8 bytes after trimming",
            "--name" => "must contain 1 to 256 UTF-8 bytes after trimming",
            "--description" => "must contain 0 to 16384 UTF-8 bytes",
            "--text/--text-file" => "must contain 1 to 65536 UTF-8 bytes",
            _ => return format!("{flag} is invalid: {domain_error}"),
        };
        format!("{flag} {requirement}")
    })
}

pub(super) fn prepare_page_request(arguments: PageArguments) -> Result<PageRequest, String> {
    Ok(PageRequest {
        limit: arguments
            .limit
            .try_into()
            .map_err(|_| "--limit must be between 1 and 100")?,
        cursor: arguments.cursor,
    })
}

pub(super) fn command_context(arguments: CommonArguments) -> CommandContext {
    CommandContext {
        service_directory: arguments.service_directory,
        json: arguments.json,
    }
}

pub(super) fn require_only<TValue>(
    required: Option<TValue>,
    conflicting: Option<TValue>,
    required_flag: &str,
    conflicting_flag: &str,
) -> Result<TValue, String> {
    match (required, conflicting) {
        (Some(value), None) => Ok(value),
        _ => Err(format!(
            "{required_flag} is required and {conflicting_flag} must be omitted for this variant"
        )),
    }
}

pub(super) fn optional_repository(
    arguments: RepositorySelectorArguments,
) -> Result<Option<BoardRepositoryLocation>, String> {
    match (arguments.repository_origin, arguments.repository_path) {
        (None, None) => Ok(None),
        (Some(origin), None) => Ok(Some(
            BoardRepositoryLocation::from_origin(&origin).map_err(repository_error)?,
        )),
        (None, Some(path)) => Ok(Some(
            BoardRepositoryLocation::discover(&path).map_err(repository_error)?,
        )),
        (Some(_), Some(_)) => {
            Err("use only one of --repository-origin or --repository-path".into())
        }
    }
}

pub(super) fn required_repository(
    arguments: RepositorySelectorArguments,
) -> Result<BoardRepositoryLocation, String> {
    optional_repository(arguments)?
        .ok_or_else(|| "one of --repository-origin or --repository-path is required".into())
}

fn repository_error(error: BoardRepositoryError) -> String {
    match error {
        BoardRepositoryError::InvalidOrigin => {
            "--repository-origin must identify a host and repository path".into()
        }
        BoardRepositoryError::InvalidNormalizedOrigin => {
            "--repository-origin could not be canonicalized".into()
        }
        BoardRepositoryError::UnreadablePath => {
            "--repository-path must resolve to a readable local path".into()
        }
        BoardRepositoryError::MissingRepository => {
            "--repository-path is not inside a Git repository with an origin or common directory"
                .into()
        }
        other => other.to_string(),
    }
}

pub(super) fn read_message_file(path: &Path) -> Result<String, String> {
    let file = File::open(path).map_err(|_| "--text-file must be a readable file")?;
    let mut bytes = Vec::new();
    file.take((MAX_MESSAGE_TEXT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "--text-file could not be read")?;
    if bytes.len() > MAX_MESSAGE_TEXT_BYTES {
        return Err("--text-file exceeds the 65536-byte message limit".into());
    }
    String::from_utf8(bytes).map_err(|_| "--text-file must contain valid UTF-8".into())
}

pub(super) fn inbox_scope(arguments: &InboxFetchArguments) -> Result<InboxScope, String> {
    let supplied = [
        arguments.project_id.is_some(),
        arguments.board_id.is_some(),
        arguments.topic_id.is_some(),
    ]
    .into_iter()
    .filter(|value| *value)
    .count();
    if supplied != 1 {
        return Err("--scope requires exactly its matching project/board/topic ID".into());
    }
    match arguments.scope {
        InboxScopeKind::Project => arguments
            .project_id
            .clone()
            .map(|value| parse_uuid_v7(value, "--project-id"))
            .transpose()?
            .map(|project_id| InboxScope::Project { project_id }),
        InboxScopeKind::Board => arguments
            .board_id
            .clone()
            .map(|value| parse_uuid_v7(value, "--board-id"))
            .transpose()?
            .map(|board_id| InboxScope::Board { board_id }),
        InboxScopeKind::Topic => arguments
            .topic_id
            .clone()
            .map(|value| parse_uuid_v7(value, "--topic-id"))
            .transpose()?
            .map(|topic_id| InboxScope::Topic { topic_id }),
    }
    .ok_or_else(|| "--scope requires exactly its matching project/board/topic ID".into())
}
