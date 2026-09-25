use super::board_arguments::*;
use super::board_value_parsing::*;
use collaboration_client::BoardRepositoryLocation;
use collaboration_client::ControlClient;
use collaboration_client::board::*;
use std::path::PathBuf;

pub(super) struct CommandContext {
    pub service_directory: Option<PathBuf>,
    pub json: bool,
}

pub(super) enum PreparedBoardCommand {
    DiscoverySearch(DiscoverySearchRequest),
    MessageSearch(MessageSearchRequest),
    ProjectCreate(ProjectCreateRequest),
    ProjectUpdate(ProjectUpdateRequest),
    ProjectShow(ProjectShowRequest),
    ProjectList {
        repository: Option<BoardRepositoryLocation>,
        page: PageRequest,
    },
    RepositoryAttach {
        project_id: ProjectId,
        repository: BoardRepositoryLocation,
        actor: Identity,
        acting_for: Option<ActingForIdentity>,
    },
    RepositoryDetach {
        project_id: ProjectId,
        repository: BoardRepositoryLocation,
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
    MessagePost(PendingMessagePost),
    MessageShow(MessageShowRequest),
    MessageList(MessageListRequest),
    ThreadShow(ThreadShowRequest),
    ThreadResolve(ThreadResolveRequest),
    ThreadUnresolve(ThreadUnresolveRequest),
    ThreadWatch(ThreadWatchRequest),
    ThreadUnwatch(ThreadUnwatchRequest),
    TopicWatch(TopicWatchRequest),
    TopicUnwatch(TopicWatchRequest),
    ThreadList(ThreadListRequest),
    RepositoryThreadList {
        repository: BoardRepositoryLocation,
        reader: Option<Identity>,
        page: PageRequest,
        default_repository_path: Option<String>,
    },
    ThreadCreate(Box<PendingThreadCreate>),
    ThreadJoin(Box<PendingThreadJoin>),
    ThreadLeave(PendingThreadLeave),
    ThreadParticipantList(ThreadParticipantListRequest),
    ThreadListen(PendingThreadListen),
    ThreadListenShow(ThreadListenShowRequest),
    ThreadListenCancel(ThreadListenCancelRequest),
    InboxFetch(InboxFetchRequest),
    InboxAcknowledge(InboxAcknowledgeRequest),
    InboxProjects(InboxProjectsRequest),
}

#[derive(Clone)]
pub(super) enum ActorInput {
    Explicit(Identity),
    Self_,
}

pub(super) struct PendingThreadCreate {
    pub request: ThreadCreateRequest,
    pub actor: ActorInput,
}

pub(super) struct PendingMessagePost {
    pub request: MessagePostRequest,
    pub text_file: Option<PathBuf>,
}

pub(super) struct PendingThreadJoin {
    pub request: ThreadJoinRequest,
    pub actor: ActorInput,
    pub listen: Option<PendingThreadListen>,
}

pub(super) struct PendingThreadLeave {
    pub request: ThreadLeaveRequest,
    pub actor: ActorInput,
}

pub(super) struct PendingThreadListen {
    pub request: ThreadListenRequest,
    pub actor: ActorInput,
}

pub(super) fn finalize_actor(
    actor: &ActorInput,
    client: &ControlClient,
) -> Result<Identity, String> {
    match actor {
        ActorInput::Explicit(identity) => Ok(identity.clone()),
        ActorInput::Self_ => self_identity(client),
    }
}

fn self_identity(client: &ControlClient) -> Result<Identity, String> {
    let harness = crate::current_session_identity::read_harness_session_identity()
        .map_err(|error| format!("--actor self: {error}"))?;
    let service_id: ServiceId = String::from(client.identity().service_id.clone())
        .try_into()
        .map_err(|_| "--actor self could not use the verified service identity".to_owned())?;
    let session_id = SessionId::try_from(harness.session_id).map_err(|_| {
        format!(
            "--actor self requires {} to contain a non-empty session ID without NUL",
            harness.harness.environment_variable
        )
    })?;
    let endpoint_id = EndpointId::try_from(harness.harness.endpoint_id.to_owned())
        .map_err(|_| "--actor self could not construct the local session endpoint".to_owned())?;
    Ok(Identity::Session {
        session: SessionRef {
            endpoint: SessionEndpointRef {
                service_id,
                endpoint_id,
            },
            session_id,
        },
    })
}

fn finalize_identity(identity: &mut Identity, client: &ControlClient) -> Result<(), String> {
    if identity
        .as_human()
        .is_some_and(|id| id.as_str() == "__agent_collaboration_self__")
    {
        *identity = self_identity(client)?;
    }
    Ok(())
}

pub(super) fn finalize_command(
    command: &mut PreparedBoardCommand,
    client: &ControlClient,
) -> Result<(), String> {
    match command {
        PreparedBoardCommand::RepositoryAttach { actor, .. }
        | PreparedBoardCommand::RepositoryDetach { actor, .. } => finalize_identity(actor, client)?,
        PreparedBoardCommand::ProjectCreate(request) => {
            finalize_identity(&mut request.actor, client)?
        }
        PreparedBoardCommand::ProjectUpdate(request) => {
            finalize_identity(&mut request.actor, client)?
        }
        PreparedBoardCommand::BoardCreate(request) => {
            finalize_identity(&mut request.actor, client)?
        }
        PreparedBoardCommand::BoardUpdate(request) => {
            finalize_identity(&mut request.actor, client)?
        }
        PreparedBoardCommand::BoardArchive(request) => {
            finalize_identity(&mut request.actor, client)?
        }
        PreparedBoardCommand::TopicCreate(request) => {
            finalize_identity(&mut request.actor, client)?
        }
        PreparedBoardCommand::TopicUpdate(request) => {
            finalize_identity(&mut request.actor, client)?
        }
        PreparedBoardCommand::MessagePost(pending) => {
            finalize_identity(&mut pending.request.actor, client)?
        }
        PreparedBoardCommand::ThreadShow(request) => {
            if let Some(reader) = &mut request.reader {
                finalize_identity(reader, client)?;
            }
        }
        PreparedBoardCommand::ThreadResolve(request) => {
            finalize_identity(&mut request.actor, client)?
        }
        PreparedBoardCommand::ThreadUnresolve(request) => {
            finalize_identity(&mut request.actor, client)?
        }
        PreparedBoardCommand::ThreadWatch(request) => {
            finalize_identity(&mut request.actor, client)?
        }
        PreparedBoardCommand::ThreadUnwatch(request) => {
            finalize_identity(&mut request.actor, client)?
        }
        PreparedBoardCommand::TopicWatch(request) | PreparedBoardCommand::TopicUnwatch(request) => {
            finalize_identity(&mut request.actor, client)?
        }
        PreparedBoardCommand::ThreadList(request) => {
            finalize_identity(&mut request.reader, client)?
        }
        PreparedBoardCommand::RepositoryThreadList {
            reader: Some(reader),
            ..
        } => finalize_identity(reader, client)?,
        PreparedBoardCommand::InboxFetch(request) => {
            finalize_identity(&mut request.reader, client)?
        }
        PreparedBoardCommand::InboxAcknowledge(request) => {
            finalize_identity(&mut request.actor, client)?
        }
        PreparedBoardCommand::InboxProjects(request) => {
            finalize_identity(&mut request.reader, client)?
        }
        PreparedBoardCommand::ThreadCreate(pending) => {
            let actor = finalize_actor(&pending.actor, client)?;
            if matches!(actor, Identity::Session { .. }) && pending.request.role.is_none() {
                return Err("Thread Create requires --role for a session actor".into());
            }
            pending.request.actor = actor;
        }
        PreparedBoardCommand::ThreadJoin(pending) => {
            let actor = finalize_actor(&pending.actor, client)?;
            pending.request.actor = actor.clone();
            if let Some(listen) = &mut pending.listen {
                listen.request.reader = actor;
            }
            // --replace self names this session, not a human called "self".
            if let Some(replace) = &mut pending.request.replace {
                finalize_identity(replace, client)?;
            }
        }
        PreparedBoardCommand::ThreadLeave(pending) => {
            pending.request.actor = finalize_actor(&pending.actor, client)?;
            if let Some(handover) = &mut pending.request.to {
                finalize_identity(handover, client)?;
            }
        }
        PreparedBoardCommand::ThreadListen(pending) => {
            pending.request.reader = finalize_actor(&pending.actor, client)?;
            if pending.request.delivery == ThreadListenDelivery::Session {
                match &pending.request.reader {
                    Identity::Session { .. } => {}
                    _ => {
                        return Err(
                            "--deliver session requires the calling session identity".into()
                        );
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn prepare(
    command: BoardCommand,
) -> Result<(PreparedBoardCommand, CommandContext), String> {
    match command {
        BoardCommand::Search(arguments) => {
            super::board_search_commands::prepare_discovery_search(arguments)
        }
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
        MessageCommand::Search(arguments) => {
            super::board_search_commands::prepare_message_search(arguments)
        }
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
            let text_file = arguments.text_file.clone();
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
                PreparedBoardCommand::MessagePost(PendingMessagePost {
                    request: MessagePostRequest {
                        message_id: parse_or_generate_uuid_v7(
                            arguments.message_id,
                            "--message-id",
                        )?,
                        placement,
                        actor,
                        acting_for,
                        text: parse_bounded_text(message_text, "--text/--text-file")?,
                        references: references
                            .try_into()
                            .map_err(|error: InvalidMessageReferences| error.to_string())?,
                    },
                    text_file,
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
        ThreadCommand::Create(arguments) => prepare_thread_create(arguments),
        ThreadCommand::Join(arguments) => prepare_thread_join(arguments),
        ThreadCommand::Leave(arguments) => prepare_thread_leave(arguments),
        ThreadCommand::Participant { command } => prepare_thread_participant(command),
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
        ThreadCommand::Watch(arguments) => prepare_watch(arguments, true),
        ThreadCommand::Unwatch(arguments) => prepare_watch(arguments, false),
        ThreadCommand::List(arguments) => Ok((
            super::board_thread_list_preparation::prepare_thread_list(&arguments)?,
            command_context(arguments.common),
        )),
        ThreadCommand::Listen(arguments) => prepare_thread_listen(arguments),
        ThreadCommand::Wait(arguments) => prepare_thread_wait(arguments),
    }
}

fn prepare_watch(
    arguments: ThreadWatchArguments,
    watch: bool,
) -> Result<(PreparedBoardCommand, CommandContext), String> {
    let (actor, acting_for) = parse_mutation_identity(&arguments.identity)?;
    let command = match (arguments.root_message_id, arguments.topic_id, watch) {
        (Some(root), None, true) => PreparedBoardCommand::ThreadWatch(ThreadWatchRequest {
            root_message_id: parse_uuid_v7(root, "--root-message-id")?,
            actor,
            acting_for,
        }),
        (Some(root), None, false) => PreparedBoardCommand::ThreadUnwatch(ThreadUnwatchRequest {
            root_message_id: parse_uuid_v7(root, "--root-message-id")?,
            actor,
            acting_for,
        }),
        (None, Some(topic), true) => PreparedBoardCommand::TopicWatch(TopicWatchRequest {
            topic_id: parse_uuid_v7(topic, "--topic-id")?,
            actor,
            acting_for,
        }),
        (None, Some(topic), false) => PreparedBoardCommand::TopicUnwatch(TopicWatchRequest {
            topic_id: parse_uuid_v7(topic, "--topic-id")?,
            actor,
            acting_for,
        }),
        _ => return Err("Choose exactly one of --root-message-id or --topic-id".into()),
    };
    Ok((command, command_context(arguments.common)))
}

fn prepare_thread_participant(
    command: ThreadParticipantCommand,
) -> Result<(PreparedBoardCommand, CommandContext), String> {
    match command {
        ThreadParticipantCommand::List(arguments) => {
            require_thread_json(&arguments.common, "Thread Participant List")?;
            Ok((
                PreparedBoardCommand::ThreadParticipantList(ThreadParticipantListRequest {
                    root_message_id: parse_uuid_v7(arguments.root_message_id, "--root-message-id")?,
                    page: prepare_page_request(arguments.page)?,
                }),
                command_context(arguments.common),
            ))
        }
    }
}

fn parse_actor_input(value: &str) -> Result<ActorInput, String> {
    if value == "self" {
        Ok(ActorInput::Self_)
    } else {
        parse_identity(value, "--actor").map(ActorInput::Explicit)
    }
}

fn placeholder_identity() -> Result<Identity, String> {
    Ok(Identity::Human {
        human_id: HumanId::try_from("pending-actor-finalization".to_owned())
            .map_err(|_| "could not prepare actor identity".to_owned())?,
    })
}

fn watch_choice(watch: bool, no_watch: bool) -> Result<bool, String> {
    match (watch, no_watch) {
        (true, false) => Ok(true),
        (false, true) => Ok(false),
        _ => Err("Choose exactly one Watch choice: --watch or --no-watch".into()),
    }
}

fn participant_role(role: ParticipantRoleKind) -> ParticipantRole {
    match role {
        ParticipantRoleKind::Orchestrator => ParticipantRole::Orchestrator,
        ParticipantRoleKind::Implementer => ParticipantRole::Implementer,
        ParticipantRoleKind::Advisor => ParticipantRole::Advisor,
        ParticipantRoleKind::Reviewer => ParticipantRole::Reviewer,
        ParticipantRoleKind::Participant => ParticipantRole::Participant,
    }
}

fn thread_references(
    message_references: Vec<String>,
    thread_references: Vec<String>,
) -> Result<MessageReferences, String> {
    let mut references = Vec::with_capacity(message_references.len() + thread_references.len());
    for value in message_references {
        references.push(ReferenceTarget::Message {
            message_id: parse_uuid_v7(value, "--reference-message")?,
        });
    }
    for value in thread_references {
        references.push(ReferenceTarget::Thread {
            root_message_id: parse_uuid_v7(value, "--reference-thread")?,
        });
    }
    references
        .try_into()
        .map_err(|error: InvalidMessageReferences| error.to_string())
}

fn prepare_thread_create(
    arguments: ThreadCreateArguments,
) -> Result<(PreparedBoardCommand, CommandContext), String> {
    require_thread_json(&arguments.common, "Thread Create")?;
    let actor = parse_actor_input(&arguments.actor)?;
    let acting_for = arguments
        .acting_for
        .as_deref()
        .map(parse_acting_for)
        .transpose()?;
    let role = arguments.role.map(participant_role);
    let watch = watch_choice(arguments.watch, arguments.no_watch)?;
    if matches!(&actor, ActorInput::Explicit(Identity::Session { .. })) && role.is_none() {
        return Err("Thread Create requires --role for a session actor".into());
    }
    let request = ThreadCreateRequest {
        message_id: parse_or_generate_uuid_v7(arguments.message_id, "--message-id")?,
        topic_id: parse_uuid_v7(arguments.topic_id, "--topic-id")?,
        actor: match &actor {
            ActorInput::Explicit(identity) => identity.clone(),
            ActorInput::Self_ => placeholder_identity()?,
        },
        acting_for,
        text: parse_bounded_text(read_message_file(&arguments.text_file)?, "--text-file")?,
        references: thread_references(arguments.reference_message, arguments.reference_thread)?,
        role,
        watch,
    };
    Ok((
        PreparedBoardCommand::ThreadCreate(Box::new(PendingThreadCreate { request, actor })),
        command_context(arguments.common),
    ))
}

fn prepare_thread_join(
    arguments: ThreadJoinArguments,
) -> Result<(PreparedBoardCommand, CommandContext), String> {
    require_thread_json(&arguments.common, "Thread Join")?;
    let actor = parse_actor_input(&arguments.actor)?;
    let role = participant_role(arguments.role);
    let replace = arguments
        .replace
        .map(|value| parse_identity(&value, "--replace"))
        .transpose()?;
    if replace.is_some() && role != ParticipantRole::Orchestrator {
        return Err("--replace requires --role orchestrator".into());
    }
    let request = ThreadJoinRequest {
        root_message_id: parse_uuid_v7(arguments.root_message_id, "--root-message-id")?,
        actor: match &actor {
            ActorInput::Explicit(identity) => identity.clone(),
            ActorInput::Self_ => placeholder_identity()?,
        },
        role,
        watch: watch_choice(arguments.watch, arguments.no_watch)?,
        replace,
        note: arguments
            .note
            .map(|value| parse_bounded_text(value, "--note"))
            .transpose()?,
    };
    let listen = prepare_join_listen(
        arguments.listen,
        arguments.max_wait,
        arguments.acknowledge,
        arguments.no_acknowledge,
        request.root_message_id.clone(),
        actor.clone(),
    )?;
    Ok((
        PreparedBoardCommand::ThreadJoin(Box::new(PendingThreadJoin {
            request,
            actor,
            listen,
        })),
        command_context(arguments.common),
    ))
}

fn prepare_thread_leave(
    arguments: ThreadLeaveArguments,
) -> Result<(PreparedBoardCommand, CommandContext), String> {
    require_thread_json(&arguments.common, "Thread Leave")?;
    let actor = parse_actor_input(&arguments.actor)?;
    let request = ThreadLeaveRequest {
        root_message_id: parse_uuid_v7(arguments.root_message_id, "--root-message-id")?,
        actor: match &actor {
            ActorInput::Explicit(identity) => identity.clone(),
            ActorInput::Self_ => placeholder_identity()?,
        },
        to: arguments
            .to
            .map(|value| parse_identity(&value, "--to"))
            .transpose()?,
        resolve: arguments.resolve,
    };
    Ok((
        PreparedBoardCommand::ThreadLeave(PendingThreadLeave { request, actor }),
        command_context(arguments.common),
    ))
}

fn prepare_join_listen(
    listen: Option<Vec<String>>,
    max_wait: Option<String>,
    acknowledge: bool,
    no_acknowledge: bool,
    root_message_id: MessageId,
    actor: ActorInput,
) -> Result<Option<PendingThreadListen>, String> {
    let Some(listen) = listen else {
        if max_wait.is_some() || acknowledge || no_acknowledge {
            return Err("--max-wait and acknowledgement choices require --listen".into());
        }
        return Ok(None);
    };
    let mode = match listen.as_slice() {
        [kind] if kind == "once" => ThreadListenMode::Once {
            max_wait_seconds: max_wait
                .as_deref()
                .map(|value| parse_duration_seconds(value, "--max-wait"))
                .transpose()?
                .unwrap_or(ThreadListenLifetime::Short.seconds()),
        },
        [kind] if matches!(kind.as_str(), "short" | "long") => {
            if max_wait.is_some() {
                return Err("--listen short|long forbids --max-wait".into());
            }
            ThreadListenMode::Repeating {
                lifetime_seconds: if kind == "short" {
                    ThreadListenLifetime::Short.seconds()
                } else {
                    ThreadListenLifetime::Long.seconds()
                },
            }
        }
        _ => return Err("--listen must be once, short, or long".into()),
    };
    if matches!(&mode, ThreadListenMode::Once { max_wait_seconds } if *max_wait_seconds > ThreadListenLifetime::Short.seconds())
    {
        return Err("--max-wait may shorten but not exceed the 25 minute Once lifetime".into());
    }
    let acknowledge = match (acknowledge, no_acknowledge) {
        (true, false) => true,
        (false, true) => false,
        _ => {
            return Err(
                "Choose exactly one acknowledgement mode: --acknowledge or --no-acknowledge".into(),
            );
        }
    };
    let reader = match &actor {
        ActorInput::Explicit(identity) => identity.clone(),
        ActorInput::Self_ => placeholder_identity()?,
    };
    Ok(Some(PendingThreadListen {
        request: ThreadListenRequest {
            reader,
            selection: ThreadListenSelection::Roots {
                root_message_ids: vec![root_message_id],
            },
            mode,
            from_activity_sequence: None,
            acknowledge,
            delivery: ThreadListenDelivery::Stdout,
        },
        actor,
    }))
}

fn prepare_thread_listen(
    arguments: ThreadListenArguments,
) -> Result<(PreparedBoardCommand, CommandContext), String> {
    if let Some(control) = arguments.control {
        return match control {
            ThreadListenControlCommand::Show(control) => {
                require_thread_listen_json(&control.common)?;
                Ok((
                    PreparedBoardCommand::ThreadListenShow(ThreadListenShowRequest {
                        listen_id: parse_uuid_v7(control.listen_id, "--listen-id")?,
                    }),
                    command_context(control.common),
                ))
            }
            ThreadListenControlCommand::Cancel(control) => {
                require_thread_listen_json(&control.common)?;
                Ok((
                    PreparedBoardCommand::ThreadListenCancel(ThreadListenCancelRequest {
                        listen_id: parse_uuid_v7(control.listen_id, "--listen-id")?,
                    }),
                    command_context(control.common),
                ))
            }
        };
    }
    require_thread_listen_json(&arguments.common)?;
    let selection_count = usize::from(arguments.watched)
        + usize::from(!arguments.root_message_id.is_empty())
        + usize::from(arguments.topic_id.is_some());
    let selection = match selection_count {
        1 if arguments.watched => ThreadListenSelection::Watched,
        1 if !arguments.root_message_id.is_empty() => ThreadListenSelection::Roots {
            root_message_ids: arguments
                .root_message_id
                .into_iter()
                .map(|value| parse_uuid_v7(value, "--root-message-id"))
                .collect::<Result<Vec<_>, _>>()?,
        },
        1 => ThreadListenSelection::Topic {
            topic_id: parse_uuid_v7(
                arguments.topic_id.ok_or("--topic-id is required")?,
                "--topic-id",
            )?,
        },
        _ => {
            return Err(
                "Choose exactly one Thread selection: --watched, repeated --root-message-id, or --topic-id".into(),
            );
        }
    };
    let delivery = match arguments.deliver {
        ThreadListenDeliveryKind::Stdout => ThreadListenDelivery::Stdout,
        ThreadListenDeliveryKind::Session => ThreadListenDelivery::Session,
    };
    let mode = match (arguments.once, arguments.lifetime) {
        (true, None) => {
            if arguments.shorten_for.is_some() {
                return Err("--for applies only with --lifetime short|long".into());
            }
            if delivery == ThreadListenDelivery::Session && arguments.max_wait.is_some() {
                return Err("--deliver session with --once uses the fixed 25 minute lifetime and forbids --max-wait".into());
            }
            let seconds = arguments
                .max_wait
                .as_deref()
                .map(|value| parse_duration_seconds(value, "--max-wait"))
                .transpose()?
                .unwrap_or(ThreadListenLifetime::Short.seconds());
            if seconds > ThreadListenLifetime::Short.seconds() {
                return Err(
                    "--max-wait may shorten but not exceed the 25 minute Once lifetime".into(),
                );
            }
            ThreadListenMode::Once {
                max_wait_seconds: seconds,
            }
        }
        (false, Some(lifetime)) => {
            let lifetime = match lifetime {
                ThreadListenLifetimeKind::Short => ThreadListenLifetime::Short,
                ThreadListenLifetimeKind::Long => ThreadListenLifetime::Long,
            };
            if delivery == ThreadListenDelivery::Session && arguments.shorten_for.is_some() {
                return Err("--deliver session uses its fixed lifetime and forbids --for; choose --lifetime short (25 minutes) or --lifetime long (75 minutes)".into());
            }
            let seconds = arguments
                .shorten_for
                .as_deref()
                .map(|value| parse_duration_seconds(value, "--for"))
                .transpose()?
                .unwrap_or(lifetime.seconds());
            if seconds > lifetime.seconds() {
                return Err("--for may shorten but not extend the selected lifetime".into());
            }
            ThreadListenMode::Repeating {
                lifetime_seconds: seconds,
            }
        }
        _ => return Err("Choose exactly one Listen mode: --once (25 minutes) or --lifetime short (25 minutes) or --lifetime long (75 minutes)".into()),
    };
    let actor = arguments.actor.as_deref().ok_or_else(|| {
        "Thread Listen requires --actor; use --actor self or pass an explicit Reader Identity JSON".to_owned()
    }).and_then(parse_actor_input)?;
    if delivery == ThreadListenDelivery::Session
        && let ActorInput::Explicit(identity) = &actor
        && !is_session_identity(identity)
    {
        return Err("--deliver session requires the calling session identity".into());
    }
    let acknowledge = match (arguments.acknowledge, arguments.no_acknowledge) {
        (true, false) => true,
        (false, true) => false,
        _ => {
            return Err(
                "Choose exactly one acknowledgement mode: --acknowledge or --no-acknowledge"
                    .to_owned(),
            );
        }
    };
    let request = ThreadListenRequest {
        reader: match &actor {
            ActorInput::Explicit(identity) => identity.clone(),
            ActorInput::Self_ => placeholder_identity()?,
        },
        selection,
        mode,
        from_activity_sequence: arguments
            .from_activity_sequence
            .map(|value| activity_sequence(value, "--from"))
            .transpose()?,
        acknowledge,
        delivery,
    };
    Ok((
        PreparedBoardCommand::ThreadListen(PendingThreadListen { request, actor }),
        command_context(arguments.common),
    ))
}

pub(super) fn is_session_identity(identity: &Identity) -> bool {
    matches!(identity, Identity::Session { .. })
}

fn prepare_thread_wait(
    arguments: ThreadWaitArguments,
) -> Result<(PreparedBoardCommand, CommandContext), String> {
    require_thread_wait_json(&arguments.common)?;
    let selection = match (arguments.watched, arguments.root_message_id.is_empty()) {
        (true, true) => ThreadListenSelection::Watched,
        (false, false) => ThreadListenSelection::Roots {
            root_message_ids: arguments
                .root_message_id
                .into_iter()
                .map(|value| parse_uuid_v7(value, "--root-message-id"))
                .collect::<Result<Vec<_>, _>>()?,
        },
        _ => {
            return Err(
                "Choose exactly one Thread selection: --watched or repeated --root-message-id"
                    .into(),
            );
        }
    };
    let actor = arguments
        .actor
        .as_deref()
        .ok_or_else(|| "Thread Wait requires --actor".to_owned())
        .and_then(parse_actor_input)?;
    let acknowledge = match (arguments.acknowledge, arguments.no_acknowledge) {
        (true, false) => true,
        (false, true) => false,
        _ => {
            return Err(
                "Choose exactly one acknowledgement mode: --acknowledge or --no-acknowledge"
                    .to_owned(),
            );
        }
    };
    let request = ThreadListenRequest {
        reader: match &actor {
            ActorInput::Explicit(identity) => identity.clone(),
            ActorInput::Self_ => placeholder_identity()?,
        },
        selection,
        mode: ThreadListenMode::Once {
            max_wait_seconds: parse_duration_seconds(
                arguments
                    .max_wait
                    .as_deref()
                    .ok_or_else(|| "Thread Wait requires --max-wait <duration>".to_owned())?,
                "--max-wait",
            )?,
        },
        from_activity_sequence: arguments
            .from_activity_sequence
            .map(|value| activity_sequence(value, "--from"))
            .transpose()?,
        acknowledge,
        delivery: ThreadListenDelivery::Stdout,
    };
    Ok((
        PreparedBoardCommand::ThreadListen(PendingThreadListen { request, actor }),
        command_context(arguments.common),
    ))
}

fn require_thread_json(common: &CommonArguments, command: &str) -> Result<(), String> {
    if common.json {
        Ok(())
    } else {
        Err(format!("{command} requires --json"))
    }
}

fn require_thread_listen_json(common: &CommonArguments) -> Result<(), String> {
    if common.json {
        Ok(())
    } else {
        Err("Thread Listen requires --json because stdout is its Delivery target".into())
    }
}

fn require_thread_wait_json(common: &CommonArguments) -> Result<(), String> {
    if common.json {
        Ok(())
    } else {
        Err("Thread Wait requires --json".into())
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
                scope: inbox_scope(&arguments)?,
                read_mode: match arguments.read_mode {
                    InboxModeKind::Unread => InboxReadMode::Unread,
                    InboxModeKind::Latest => InboxReadMode::Latest,
                },
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
