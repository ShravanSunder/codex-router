use super::board_arguments::*;
use super::board_value_parsing::*;
use project_board::*;
use std::path::PathBuf;

pub(super) struct CommandContext {
    pub service_directory: Option<PathBuf>,
    pub json: bool,
}

pub(super) enum PreparedRepository {
    Origin(NormalizedOrigin),
    Local(CommonDirectory),
}

pub(super) enum PreparedBoardCommand {
    ProjectCreate(ProjectCreateRequest),
    ProjectUpdate(ProjectUpdateRequest),
    ProjectShow(ProjectShowRequest),
    ProjectList {
        repository: Option<PreparedRepository>,
        page: PageRequest,
    },
    RepositoryAttach {
        project_id: ProjectId,
        repository: PreparedRepository,
        actor: Identity,
        acting_for: Option<ActingForIdentity>,
    },
    RepositoryDetach {
        project_id: ProjectId,
        repository: PreparedRepository,
        actor: Identity,
        acting_for: Option<ActingForIdentity>,
    },
    RepositoryList(RepositoryListRequest),
    BoardCreate(BoardCreateRequest),
    BoardUpdate(BoardUpdateRequest),
    BoardShow(BoardShowRequest),
    BoardList(BoardListRequest),
    BoardArchive(BoardArchiveRequest),
    TopicCreate(TopicCreateRequest),
    TopicUpdate(TopicUpdateRequest),
    TopicList(TopicListRequest),
    MessagePost(MessagePostRequest),
    MessageShow(MessageShowRequest),
    MessageList(MessageListRequest),
    ThreadShow(ThreadShowRequest),
    ThreadResolve(ThreadResolveRequest),
    ThreadUnresolve(ThreadUnresolveRequest),
    ThreadWatch(ThreadWatchRequest),
    ThreadUnwatch(ThreadUnwatchRequest),
    ThreadList(ThreadListRequest),
    InboxFetch(InboxFetchRequest),
    InboxAcknowledge(InboxAcknowledgeRequest),
    InboxProjects(InboxProjectsRequest),
}

impl PreparedBoardCommand {
    pub(super) fn is_mutation(&self) -> bool {
        !matches!(
            self,
            Self::ProjectShow(_)
                | Self::ProjectList { .. }
                | Self::RepositoryList(_)
                | Self::BoardShow(_)
                | Self::BoardList(_)
                | Self::TopicList(_)
                | Self::MessageShow(_)
                | Self::MessageList(_)
                | Self::ThreadShow(_)
                | Self::ThreadList(_)
                | Self::InboxFetch(_)
                | Self::InboxProjects(_)
        )
    }

    pub(super) fn uncertain_resource(&self) -> Option<ResourceIdentity> {
        match self {
            Self::ProjectCreate(request) => Some(ResourceIdentity::Project {
                project_id: request.project_id.clone(),
            }),
            Self::ProjectUpdate(request) => Some(ResourceIdentity::Project {
                project_id: request.project_id.clone(),
            }),
            Self::RepositoryAttach { project_id, .. }
            | Self::RepositoryDetach { project_id, .. } => Some(ResourceIdentity::Project {
                project_id: project_id.clone(),
            }),
            Self::BoardCreate(request) => Some(ResourceIdentity::Board {
                board_id: request.board_id.clone(),
            }),
            Self::BoardUpdate(request) => Some(ResourceIdentity::Board {
                board_id: request.board_id.clone(),
            }),
            Self::BoardArchive(request) => Some(ResourceIdentity::Board {
                board_id: request.board_id.clone(),
            }),
            Self::TopicCreate(request) => Some(ResourceIdentity::Topic {
                topic_id: request.topic_id.clone(),
            }),
            Self::TopicUpdate(request) => Some(ResourceIdentity::Topic {
                topic_id: request.topic_id.clone(),
            }),
            Self::MessagePost(request) => Some(ResourceIdentity::Message {
                message_id: request.message_id.clone(),
            }),
            Self::ThreadResolve(request) => Some(ResourceIdentity::Thread {
                root_message_id: request.root_message_id.clone(),
            }),
            Self::ThreadUnresolve(request) => Some(ResourceIdentity::Thread {
                root_message_id: request.root_message_id.clone(),
            }),
            Self::ThreadWatch(request) => Some(ResourceIdentity::Thread {
                root_message_id: request.root_message_id.clone(),
            }),
            Self::ThreadUnwatch(request) => Some(ResourceIdentity::Thread {
                root_message_id: request.root_message_id.clone(),
            }),
            Self::InboxAcknowledge(request) => match &request.scope {
                ReadScope::Topic { topic_id } => Some(ResourceIdentity::Topic {
                    topic_id: topic_id.clone(),
                }),
                ReadScope::Thread { root_message_id } => Some(ResourceIdentity::Thread {
                    root_message_id: root_message_id.clone(),
                }),
            },
            _ => None,
        }
    }
}

pub(super) fn prepare(
    command: BoardCommand,
) -> Result<(PreparedBoardCommand, CommandContext), String> {
    match command {
        BoardCommand::Project { command } => prepare_project(command),
        BoardCommand::Repository { command } => prepare_repository_command(command),
        BoardCommand::Create(arguments) => {
            let (actor, acting_for) = parse_mutation_identity(&arguments.identity)?;
            let request = BoardCreateRequest {
                board_id: parse_or_generate_uuid_v7(arguments.board_id, "--board-id")?,
                project_id: parse_uuid_v7(arguments.project_id, "--project-id")?,
                name: parse_bounded_text(arguments.name, "--name")?,
                description: parse_bounded_text(arguments.description, "--description")?,
                actor,
                acting_for,
            };
            Ok((
                PreparedBoardCommand::BoardCreate(request),
                command_context(arguments.common),
            ))
        }
        BoardCommand::Update(arguments) => {
            let (actor, acting_for) = parse_mutation_identity(&arguments.identity)?;
            let request = BoardUpdateRequest {
                board_id: parse_uuid_v7(arguments.board_id, "--board-id")?,
                name: parse_bounded_text(arguments.name, "--name")?,
                description: parse_bounded_text(arguments.description, "--description")?,
                actor,
                acting_for,
            };
            Ok((
                PreparedBoardCommand::BoardUpdate(request),
                command_context(arguments.common),
            ))
        }
        BoardCommand::Show(arguments) => Ok((
            PreparedBoardCommand::BoardShow(BoardShowRequest {
                board_id: parse_uuid_v7(arguments.board_id, "--board-id")?,
            }),
            command_context(arguments.common),
        )),
        BoardCommand::List(arguments) => Ok((
            PreparedBoardCommand::BoardList(BoardListRequest {
                project_id: arguments
                    .project_id
                    .map(|value| parse_uuid_v7(value, "--project-id"))
                    .transpose()?,
                include_archived: arguments.include_archived,
                page: prepare_page_request(arguments.page)?,
            }),
            command_context(arguments.common),
        )),
        BoardCommand::Archive(arguments) => {
            let (actor, acting_for) = parse_mutation_identity(&arguments.identity)?;
            Ok((
                PreparedBoardCommand::BoardArchive(BoardArchiveRequest {
                    board_id: parse_uuid_v7(arguments.board_id, "--board-id")?,
                    actor,
                    acting_for,
                }),
                command_context(arguments.common),
            ))
        }
        BoardCommand::Topic { command } => prepare_topic(command),
        BoardCommand::Message { command } => prepare_message(command),
        BoardCommand::Thread { command } => prepare_thread(command),
        BoardCommand::Inbox { command } => prepare_inbox(command),
    }
}

fn prepare_project(
    command: ProjectCommand,
) -> Result<(PreparedBoardCommand, CommandContext), String> {
    match command {
        ProjectCommand::Create(arguments) => {
            let (actor, acting_for) = parse_mutation_identity(&arguments.identity)?;
            Ok((
                PreparedBoardCommand::ProjectCreate(ProjectCreateRequest {
                    project_id: parse_or_generate_uuid_v7(arguments.project_id, "--project-id")?,
                    name: parse_bounded_text(arguments.name, "--name")?,
                    description: parse_bounded_text(arguments.description, "--description")?,
                    actor,
                    acting_for,
                }),
                command_context(arguments.common),
            ))
        }
        ProjectCommand::Update(arguments) => {
            let (actor, acting_for) = parse_mutation_identity(&arguments.identity)?;
            Ok((
                PreparedBoardCommand::ProjectUpdate(ProjectUpdateRequest {
                    project_id: parse_uuid_v7(arguments.project_id, "--project-id")?,
                    name: parse_bounded_text(arguments.name, "--name")?,
                    description: parse_bounded_text(arguments.description, "--description")?,
                    actor,
                    acting_for,
                }),
                command_context(arguments.common),
            ))
        }
        ProjectCommand::Show(arguments) => Ok((
            PreparedBoardCommand::ProjectShow(ProjectShowRequest {
                project_id: parse_uuid_v7(arguments.project_id, "--project-id")?,
            }),
            command_context(arguments.common),
        )),
        ProjectCommand::List(arguments) => Ok((
            PreparedBoardCommand::ProjectList {
                repository: optional_repository(arguments.repository)?,
                page: prepare_page_request(arguments.page)?,
            },
            command_context(arguments.common),
        )),
    }
}

fn prepare_repository_command(
    command: RepositoryCommand,
) -> Result<(PreparedBoardCommand, CommandContext), String> {
    match command {
        RepositoryCommand::Attach(arguments) => {
            let (actor, acting_for) = parse_mutation_identity(&arguments.identity)?;
            let project_id = parse_uuid_v7(arguments.project_id, "--project-id")?;
            let repository = required_repository(arguments.repository)?;
            Ok((
                PreparedBoardCommand::RepositoryAttach {
                    project_id,
                    repository,
                    actor,
                    acting_for,
                },
                command_context(arguments.common),
            ))
        }
        RepositoryCommand::Detach(arguments) => {
            let (actor, acting_for) = parse_mutation_identity(&arguments.identity)?;
            let project_id = parse_uuid_v7(arguments.project_id, "--project-id")?;
            let repository = required_repository(arguments.repository)?;
            Ok((
                PreparedBoardCommand::RepositoryDetach {
                    project_id,
                    repository,
                    actor,
                    acting_for,
                },
                command_context(arguments.common),
            ))
        }
        RepositoryCommand::List(arguments) => Ok((
            PreparedBoardCommand::RepositoryList(RepositoryListRequest {
                project_id: parse_uuid_v7(arguments.project_id, "--project-id")?,
                page: prepare_page_request(arguments.page)?,
            }),
            command_context(arguments.common),
        )),
    }
}

fn prepare_topic(command: TopicCommand) -> Result<(PreparedBoardCommand, CommandContext), String> {
    match command {
        TopicCommand::Create(arguments) => {
            let (actor, acting_for) = parse_mutation_identity(&arguments.identity)?;
            Ok((
                PreparedBoardCommand::TopicCreate(TopicCreateRequest {
                    topic_id: parse_or_generate_uuid_v7(arguments.topic_id, "--topic-id")?,
                    board_id: parse_uuid_v7(arguments.board_id, "--board-id")?,
                    name: parse_bounded_text(arguments.name, "--name")?,
                    description: parse_bounded_text(arguments.description, "--description")?,
                    actor,
                    acting_for,
                }),
                command_context(arguments.common),
            ))
        }
        TopicCommand::Update(arguments) => {
            let (actor, acting_for) = parse_mutation_identity(&arguments.identity)?;
            Ok((
                PreparedBoardCommand::TopicUpdate(TopicUpdateRequest {
                    topic_id: parse_uuid_v7(arguments.topic_id, "--topic-id")?,
                    name: parse_bounded_text(arguments.name, "--name")?,
                    description: parse_bounded_text(arguments.description, "--description")?,
                    actor,
                    acting_for,
                }),
                command_context(arguments.common),
            ))
        }
        TopicCommand::List(arguments) => Ok((
            PreparedBoardCommand::TopicList(TopicListRequest {
                board_id: parse_uuid_v7(arguments.board_id, "--board-id")?,
                page: prepare_page_request(arguments.page)?,
            }),
            command_context(arguments.common),
        )),
    }
}

fn prepare_message(
    command: MessageCommand,
) -> Result<(PreparedBoardCommand, CommandContext), String> {
    match command {
        MessageCommand::Post(arguments) => {
            let (actor, acting_for) = parse_mutation_identity(&arguments.identity)?;
            let placement = match arguments.placement {
                PlacementKind::Topic => Placement::Topic {
                    topic_id: parse_uuid_v7(
                        require_only(
                            arguments.topic_id,
                            arguments.root_message_id,
                            "--topic-id",
                            "--root-message-id",
                        )?,
                        "--topic-id",
                    )?,
                },
                PlacementKind::Thread => Placement::Thread {
                    root_message_id: parse_uuid_v7(
                        require_only(
                            arguments.root_message_id,
                            arguments.topic_id,
                            "--root-message-id",
                            "--topic-id",
                        )?,
                        "--root-message-id",
                    )?,
                },
            };
            let message_text = match (arguments.text, arguments.text_file) {
                (Some(value), None) => value,
                (None, Some(path)) => read_message_file(&path)?,
                _ => return Err("exactly one of --text or --text-file is required".into()),
            };
            let mut references = Vec::with_capacity(
                arguments.reference_message.len() + arguments.reference_thread.len(),
            );
            for value in arguments.reference_message {
                references.push(ReferenceTarget::Message {
                    message_id: parse_uuid_v7(value, "--reference-message")?,
                });
            }
            for value in arguments.reference_thread {
                references.push(ReferenceTarget::Thread {
                    root_message_id: parse_uuid_v7(value, "--reference-thread")?,
                });
            }
            Ok((
                PreparedBoardCommand::MessagePost(MessagePostRequest {
                    message_id: parse_or_generate_uuid_v7(arguments.message_id, "--message-id")?,
                    placement,
                    actor,
                    acting_for,
                    text: parse_bounded_text(message_text, "--text/--text-file")?,
                    references: references
                        .try_into()
                        .map_err(|error: InvalidMessageReferences| error.to_string())?,
                }),
                command_context(arguments.common),
            ))
        }
        MessageCommand::Show(arguments) => Ok((
            PreparedBoardCommand::MessageShow(MessageShowRequest {
                message_id: parse_uuid_v7(arguments.message_id, "--message-id")?,
            }),
            command_context(arguments.common),
        )),
        MessageCommand::List(arguments) => {
            let scope = message_scope(&arguments)?;
            let selection = message_selection(&arguments)?;
            selection
                .validate()
                .map_err(|error| format!("--selection range is invalid: {error}"))?;
            Ok((
                PreparedBoardCommand::MessageList(MessageListRequest {
                    scope,
                    selection,
                    page: prepare_page_request(arguments.page)?,
                }),
                command_context(arguments.common),
            ))
        }
    }
}

fn prepare_thread(
    command: ThreadCommand,
) -> Result<(PreparedBoardCommand, CommandContext), String> {
    match command {
        ThreadCommand::Show(arguments) => Ok((
            PreparedBoardCommand::ThreadShow(ThreadShowRequest {
                root_message_id: parse_uuid_v7(arguments.root_message_id, "--root-message-id")?,
                reader: arguments
                    .reader
                    .map(|value| parse_identity(&value, "--reader"))
                    .transpose()?,
            }),
            command_context(arguments.common),
        )),
        ThreadCommand::Resolve(arguments) => thread_mutation(
            arguments,
            PreparedBoardCommand::ThreadResolve,
            |root_message_id, actor, acting_for| ThreadResolveRequest {
                root_message_id,
                actor,
                acting_for,
            },
        ),
        ThreadCommand::Unresolve(arguments) => thread_mutation(
            arguments,
            PreparedBoardCommand::ThreadUnresolve,
            |root_message_id, actor, acting_for| ThreadUnresolveRequest {
                root_message_id,
                actor,
                acting_for,
            },
        ),
        ThreadCommand::Watch(arguments) => thread_mutation(
            arguments,
            PreparedBoardCommand::ThreadWatch,
            |root_message_id, actor, acting_for| ThreadWatchRequest {
                root_message_id,
                actor,
                acting_for,
            },
        ),
        ThreadCommand::Unwatch(arguments) => thread_mutation(
            arguments,
            PreparedBoardCommand::ThreadUnwatch,
            |root_message_id, actor, acting_for| ThreadUnwatchRequest {
                root_message_id,
                actor,
                acting_for,
            },
        ),
        ThreadCommand::List(arguments) => Ok((
            PreparedBoardCommand::ThreadList(ThreadListRequest {
                project_id: parse_uuid_v7(arguments.project_id, "--project-id")?,
                reader: parse_identity(&arguments.reader, "--reader")?,
                watched_only: arguments.watched_only,
                page: prepare_page_request(arguments.page)?,
            }),
            command_context(arguments.common),
        )),
    }
}

fn thread_mutation<TRequest>(
    arguments: ThreadMutationArguments,
    wrap: impl FnOnce(TRequest) -> PreparedBoardCommand,
    build: impl FnOnce(MessageId, Identity, Option<ActingForIdentity>) -> TRequest,
) -> Result<(PreparedBoardCommand, CommandContext), String> {
    let (actor, acting_for) = parse_mutation_identity(&arguments.identity)?;
    let request = build(
        parse_uuid_v7(arguments.root_message_id, "--root-message-id")?,
        actor,
        acting_for,
    );
    Ok((wrap(request), command_context(arguments.common)))
}

fn prepare_inbox(command: InboxCommand) -> Result<(PreparedBoardCommand, CommandContext), String> {
    match command {
        InboxCommand::Fetch(arguments) => Ok((
            PreparedBoardCommand::InboxFetch(InboxFetchRequest {
                project_id: parse_uuid_v7(arguments.project_id, "--project-id")?,
                reader: parse_identity(&arguments.reader, "--reader")?,
                page: prepare_page_request(arguments.page)?,
            }),
            command_context(arguments.common),
        )),
        InboxCommand::Acknowledge(arguments) => {
            let (actor, acting_for) = parse_mutation_identity(&arguments.identity)?;
            let scope = match arguments.scope {
                ReadScopeKind::Topic => ReadScope::Topic {
                    topic_id: parse_uuid_v7(
                        require_only(
                            arguments.topic_id,
                            arguments.root_message_id,
                            "--topic-id",
                            "--root-message-id",
                        )?,
                        "--topic-id",
                    )?,
                },
                ReadScopeKind::Thread => ReadScope::Thread {
                    root_message_id: parse_uuid_v7(
                        require_only(
                            arguments.root_message_id,
                            arguments.topic_id,
                            "--root-message-id",
                            "--topic-id",
                        )?,
                        "--root-message-id",
                    )?,
                },
            };
            Ok((
                PreparedBoardCommand::InboxAcknowledge(InboxAcknowledgeRequest {
                    actor,
                    acting_for,
                    scope,
                    through_activity_sequence: activity_sequence(
                        arguments.through_activity_sequence,
                        "--through-activity-sequence",
                    )?,
                }),
                command_context(arguments.common),
            ))
        }
        InboxCommand::Projects(arguments) => Ok((
            PreparedBoardCommand::InboxProjects(InboxProjectsRequest {
                reader: parse_identity(&arguments.reader, "--reader")?,
                unread_only: arguments.unread_only,
                page: prepare_page_request(arguments.page)?,
            }),
            command_context(arguments.common),
        )),
    }
}
