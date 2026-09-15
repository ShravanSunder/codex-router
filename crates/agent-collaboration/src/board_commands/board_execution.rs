use super::board_preparation::{self, CommandContext, PreparedBoardCommand};
use collaboration_client::board::*;
use collaboration_client::{BoardClientError, ClientError, ControlClient};
use serde::Serialize;
use serde_json::{Value, json};
use std::io::{self, Write};

enum CommandExecutionError {
    ConnectionBeforeRequest(ClientError),
    Request {
        error: BoardClientError,
        thread_create_text_file: Option<std::path::PathBuf>,
    },
    InvalidUsage(String),
}

enum CommandExecutionResult {
    Value(Value),
    Exit(i32),
}

pub(super) fn execute(command: PreparedBoardCommand, context: CommandContext) -> i32 {
    let directory = match crate::endpoint_commands::resolve_directory(context.service_directory) {
        Ok(directory) => directory,
        Err(message) => {
            return crate::endpoint_commands::report_failure(
                "invalidUsage",
                &message,
                2,
                context.json,
            );
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            return report_connection_failure(context.json);
        }
    };
    let result = runtime.block_on(async {
        let mut client = ControlClient::connect(
            &directory,
            "agent-collaboration-board",
            env!("CARGO_PKG_VERSION"),
        )
        .await
        .map_err(CommandExecutionError::ConnectionBeforeRequest)?;
        let mut command = command;
        board_preparation::finalize_command(&mut command, &client)
            .map_err(CommandExecutionError::InvalidUsage)?;
        let result = match command {
            PreparedBoardCommand::ThreadJoin(pending) => {
                let board_preparation::PendingThreadJoin {
                    request,
                    actor,
                    listen,
                } = *pending;
                if let Some(listen) = listen {
                    let joined = client.board_thread_join(request).await.map_err(|error| {
                        CommandExecutionError::Request {
                            error,
                            thread_create_text_file: None,
                        }
                    })?;
                    let joined = serialize_result(joined).map_err(|error| {
                        CommandExecutionError::Request {
                            error,
                            thread_create_text_file: None,
                        }
                    })?;
                    if write_machine_result_and_flush(joined) != 0 {
                        Ok(CommandExecutionResult::Exit(1))
                    } else {
                        Ok(CommandExecutionResult::Exit(
                            super::board_thread_listen_execution::run_with_client(
                                listen.request,
                                &mut client,
                            )
                            .await,
                        ))
                    }
                } else {
                    dispatch(
                        &mut client,
                        PreparedBoardCommand::ThreadJoin(Box::new(
                            board_preparation::PendingThreadJoin {
                                request,
                                actor,
                                listen: None,
                            },
                        )),
                    )
                    .await
                    .map(CommandExecutionResult::Value)
                    .map_err(|error| CommandExecutionError::Request {
                        error,
                        thread_create_text_file: None,
                    })
                }
            }
            command => {
                let thread_create_text_file = match &command {
                    PreparedBoardCommand::MessagePost(pending) => pending.text_file.clone(),
                    _ => None,
                };
                dispatch(&mut client, command)
                    .await
                    .map(CommandExecutionResult::Value)
                    .map_err(|error| CommandExecutionError::Request {
                        error,
                        thread_create_text_file,
                    })
            }
        };
        let _closed = client.close().await;
        result
    });
    report(result, context.json)
}

async fn dispatch(
    client: &mut ControlClient,
    command: PreparedBoardCommand,
) -> Result<Value, BoardClientError> {
    match command {
        PreparedBoardCommand::DiscoverySearch(request) => {
            serialize_result(client.board_discovery_search(request).await?)
        }
        PreparedBoardCommand::MessageSearch(request) => {
            serialize_result(client.board_message_search(request).await?)
        }
        PreparedBoardCommand::ProjectCreate(request) => {
            serialize_result(client.board_project_create(request).await?)
        }
        PreparedBoardCommand::ProjectUpdate(request) => {
            serialize_result(client.board_project_update(request).await?)
        }
        PreparedBoardCommand::ProjectShow(request) => {
            serialize_result(client.board_project_show(request).await?)
        }
        PreparedBoardCommand::ProjectList { repository, page } => {
            let repository = repository
                .map(|repository| repository.for_client(client))
                .transpose()?;
            serialize_result(
                client
                    .board_project_list(ProjectListRequest { repository, page })
                    .await?,
            )
        }
        PreparedBoardCommand::RepositoryAttach {
            project_id,
            repository,
            actor,
            acting_for,
        } => {
            let repository = repository.for_client(client)?;
            serialize_result(
                client
                    .board_repository_attach(RepositoryAttachRequest {
                        project_id,
                        repository,
                        actor,
                        acting_for,
                    })
                    .await?,
            )
        }
        PreparedBoardCommand::RepositoryDetach {
            project_id,
            repository,
            actor,
            acting_for,
        } => {
            let repository = repository.for_client(client)?;
            serialize_result(
                client
                    .board_repository_detach(RepositoryDetachRequest {
                        project_id,
                        repository,
                        actor,
                        acting_for,
                    })
                    .await?,
            )
        }
        PreparedBoardCommand::RepositoryList(request) => {
            serialize_result(client.board_repository_list(request).await?)
        }
        PreparedBoardCommand::BoardCreate(request) => {
            serialize_result(client.board_create(request).await?)
        }
        PreparedBoardCommand::BoardUpdate(request) => {
            serialize_result(client.board_update(request).await?)
        }
        PreparedBoardCommand::BoardShow(request) => {
            serialize_result(client.board_show(request).await?)
        }
        PreparedBoardCommand::BoardList(request) => {
            serialize_result(client.board_list(request).await?)
        }
        PreparedBoardCommand::BoardArchive(request) => {
            serialize_result(client.board_archive(request).await?)
        }
        PreparedBoardCommand::TopicCreate(request) => {
            serialize_result(client.board_topic_create(request).await?)
        }
        PreparedBoardCommand::TopicUpdate(request) => {
            serialize_result(client.board_topic_update(request).await?)
        }
        PreparedBoardCommand::TopicList(request) => {
            serialize_result(client.board_topic_list(request).await?)
        }
        PreparedBoardCommand::MessagePost(pending) => {
            serialize_result(client.board_message_post(pending.request).await?)
        }
        PreparedBoardCommand::MessageShow(request) => {
            serialize_result(client.board_message_show(request).await?)
        }
        PreparedBoardCommand::MessageList(request) => {
            serialize_result(client.board_message_list(request).await?)
        }
        PreparedBoardCommand::ThreadShow(request) => {
            serialize_result(client.board_thread_show(request).await?)
        }
        PreparedBoardCommand::ThreadResolve(request) => {
            serialize_result(client.board_thread_resolve(request).await?)
        }
        PreparedBoardCommand::ThreadUnresolve(request) => {
            serialize_result(client.board_thread_unresolve(request).await?)
        }
        PreparedBoardCommand::ThreadWatch(request) => {
            serialize_result(client.board_thread_watch(request).await?)
        }
        PreparedBoardCommand::ThreadUnwatch(request) => {
            serialize_result(client.board_thread_unwatch(request).await?)
        }
        PreparedBoardCommand::ThreadList(request) => {
            serialize_result(client.board_thread_list(request).await?)
        }
        PreparedBoardCommand::ThreadCreate(pending) => {
            serialize_result(client.board_thread_create(pending.request).await?)
        }
        PreparedBoardCommand::ThreadJoin(pending) => {
            debug_assert!(pending.listen.is_none());
            serialize_result(client.board_thread_join(pending.request).await?)
        }
        PreparedBoardCommand::ThreadLeave(pending) => {
            serialize_result(client.board_thread_leave(pending.request).await?)
        }
        PreparedBoardCommand::ThreadParticipantList(request) => {
            serialize_result(client.board_thread_participant_list(request).await?)
        }
        PreparedBoardCommand::ThreadListen(_) => {
            Err(ClientError::Protocol("Thread Listen must use streaming stdout execution").into())
        }
        PreparedBoardCommand::ThreadListenShow(request) => {
            serialize_result(client.board_thread_listen_show(request).await?)
        }
        PreparedBoardCommand::ThreadListenCancel(request) => {
            serialize_result(client.board_thread_listen_cancel(request).await?)
        }
        PreparedBoardCommand::InboxFetch(request) => {
            serialize_result(client.board_inbox_fetch(request).await?)
        }
        PreparedBoardCommand::InboxAcknowledge(request) => {
            serialize_result(client.board_inbox_acknowledge(request).await?)
        }
        PreparedBoardCommand::InboxProjects(request) => {
            serialize_result(client.board_inbox_projects(request).await?)
        }
    }
}

fn serialize_result<TValue: Serialize>(value: TValue) -> Result<Value, BoardClientError> {
    serde_json::to_value(value)
        .map_err(|_| ClientError::Protocol("board result could not be rendered").into())
}

fn report(result: Result<CommandExecutionResult, CommandExecutionError>, machine: bool) -> i32 {
    match result {
        Ok(CommandExecutionResult::Value(result)) => write_result(result, machine),
        Ok(CommandExecutionResult::Exit(code)) => code,
        Err(CommandExecutionError::Request {
            error: BoardClientError::Rejected(error),
            thread_create_text_file,
        }) => {
            if machine {
                write_json(&json!({"kind":"error","error":error}), 4)
            } else {
                let mut stderr = io::stderr().lock();
                let kind = wire_name(&error.kind);
                let next_action = refusal_command(&error, thread_create_text_file.as_deref());
                let _written = writeln!(
                    stderr,
                    "Error: {kind}\n{}\nNext action: {next_action}",
                    error.message
                );
                4
            }
        }
        Err(CommandExecutionError::Request {
            error:
                BoardClientError::OutcomeUnknown {
                    resource,
                    message,
                    next_action,
                },
            ..
        }) => report_uncertain_outcome(machine, &resource, message, next_action),
        Err(CommandExecutionError::InvalidUsage(message)) => {
            crate::endpoint_commands::report_failure("invalidField", &message, 2, machine)
        }
        Err(CommandExecutionError::ConnectionBeforeRequest(error)) => {
            crate::permission_diagnostic_reporting::report_permission_error(
                &error,
                crate::permission_diagnostic_reporting::PermissionDiagnosticRendering::Command,
                machine,
            )
            .unwrap_or_else(|| report_connection_failure(machine))
        }
        Err(CommandExecutionError::Request {
            error: BoardClientError::Connection(_),
            ..
        }) => report_connection_failure(machine),
    }
}

fn refusal_command(
    error: &BoardError,
    thread_create_text_file: Option<&std::path::Path>,
) -> String {
    let actor = |identity: &Identity| {
        serde_json::to_string(identity)
            .map(|value| format!("'{value}'"))
            .unwrap_or_else(|_| "<actor>".to_owned())
    };
    match (&error.next_action, &error.details) {
        (BoardNextAction::CreateThread, BoardErrorDetails::ThreadCreateRefusal { refusal }) => {
            let text_file = thread_create_text_file
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<path>".to_owned());
            format!(
                "agent-collaboration board thread create --topic-id {} --actor {} --role <role> (--watch | --no-watch) --text-file '{}' --json",
                refusal.topic_id.as_str(),
                actor(&refusal.actor),
                text_file.replace('\'', "'\\''")
            )
        }
        (BoardNextAction::JoinThread, BoardErrorDetails::ParticipantRefusal { refusal }) => {
            format!(
                "agent-collaboration board thread join --root-message-id {} --actor {} --role <role> (--watch | --no-watch) --json",
                refusal.root_message_id.as_str(),
                actor(&refusal.actor)
            )
        }
        (
            BoardNextAction::ReplaceOrchestrator,
            BoardErrorDetails::ParticipantRefusal { refusal },
        ) => format!(
            "agent-collaboration board thread join --root-message-id {} --actor {} --role orchestrator --replace {} (--watch | --no-watch) --json",
            refusal.root_message_id.as_str(),
            actor(&refusal.actor),
            refusal
                .holder
                .as_ref()
                .map(actor)
                .unwrap_or_else(|| "<current-orchestrator>".to_owned()),
        ),
        (
            BoardNextAction::LeaveWithHandoverOrResolve,
            BoardErrorDetails::ParticipantRefusal { refusal },
        ) => format!(
            "agent-collaboration board thread leave --root-message-id {} --actor {} (--to <joined-participant> | --resolve) --json",
            refusal.root_message_id.as_str(),
            actor(&refusal.actor)
        ),
        (
            BoardNextAction::JoinHandoverTarget,
            BoardErrorDetails::ParticipantRefusal { refusal },
        ) => format!(
            "agent-collaboration board thread join --root-message-id {} --actor {} --role <role> (--watch | --no-watch) --json",
            refusal.root_message_id.as_str(),
            refusal
                .target
                .as_ref()
                .map(actor)
                .unwrap_or_else(|| "<handover-target>".to_owned()),
        ),
        (
            BoardNextAction::InspectParticipants,
            BoardErrorDetails::ParticipantRefusal { refusal },
        ) => format!(
            "agent-collaboration board thread participant list --root-message-id {} --json",
            refusal.root_message_id.as_str()
        ),
        (
            BoardNextAction::RepeatJoinWithoutReplace,
            BoardErrorDetails::ParticipantRefusal { refusal },
        ) => format!(
            "agent-collaboration board thread join --root-message-id {} --actor {} --role orchestrator (--watch | --no-watch) --json",
            refusal.root_message_id.as_str(),
            actor(&refusal.actor)
        ),
        _ => wire_name(&error.next_action),
    }
}

fn report_uncertain_outcome(
    machine: bool,
    resource: &ResourceIdentity,
    message: &str,
    next_action: BoardNextAction,
) -> i32 {
    if machine {
        write_json(
            &json!({"kind":"error","error":{"kind":"outcomeUnknown","stage":"inspection","message":message,"nextAction":next_action,"details":{"kind":"resource","resource":resource}}}),
            5,
        )
    } else {
        let mut stderr = io::stderr().lock();
        let resource = serde_json::to_string(resource)
            .unwrap_or_else(|_| "affected resource unavailable".into());
        let next_action = wire_name(&next_action);
        let _written = writeln!(
            stderr,
            "Error: outcomeUnknown\n{message}\nNext action: {next_action}\nAffected resource: {resource}"
        );
        5
    }
}

fn wire_name<TValue: Serialize>(value: &TValue) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unavailable".to_owned())
}

fn write_result(result: Value, machine: bool) -> i32 {
    if machine {
        write_json(&json!({"kind":"result","result":result}), 0)
    } else {
        let rendered =
            serde_json::to_string_pretty(&result).unwrap_or_else(|_| "Output unavailable".into());
        if writeln!(io::stdout().lock(), "{rendered}").is_ok() {
            0
        } else {
            3
        }
    }
}

fn write_machine_result_and_flush(result: Value) -> i32 {
    let value = json!({"kind":"result","result":result});
    let mut stdout = io::stdout().lock();
    if writeln!(stdout, "{value}").is_ok() && stdout.flush().is_ok() {
        0
    } else {
        3
    }
}

fn report_connection_failure(machine: bool) -> i32 {
    let message =
        "The board service is unavailable. Check the selected service directory and retry.";
    if machine {
        write_json(
            &json!({"kind":"error","error":{"kind":"boardUnavailable","stage":"storage","message":message,"nextAction":"retryLater","details":{"kind":"none"}}}),
            3,
        )
    } else {
        let _written = writeln!(
            io::stderr().lock(),
            "Error: boardUnavailable\n{message}\nNext action: retryLater"
        );
        3
    }
}

fn write_json(value: &Value, success_code: i32) -> i32 {
    if writeln!(io::stdout().lock(), "{value}").is_ok() {
        success_code
    } else {
        3
    }
}

#[cfg(test)]
mod tests {
    use super::refusal_command;
    use collaboration_client::board::*;
    use std::path::Path;

    #[test]
    fn topic_post_refusal_preserves_the_supplied_text_file_path() {
        let topic_id =
            TopicId::try_from("018f6f67-64d2-7a21-bf9a-8f193f987091".to_owned()).expect("topic ID");
        let actor = Identity::Human {
            human_id: HumanId::try_from("owner".to_owned()).expect("human ID"),
        };
        let text = MessageText::try_from("root text".to_owned()).expect("message text");
        let refusal = BoardError::session_topic_post(topic_id, actor, text);
        let command = refusal_command(&refusal, Some(Path::new("/tmp/root message.txt")));

        assert!(command.contains("--text-file '/tmp/root message.txt'"));
        assert!(!command.contains("--text-file '<path>'"));
    }
}
