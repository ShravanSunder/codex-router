use super::board_preparation::{CommandContext, PreparedBoardCommand, PreparedRepository};
use communication_client::{BoardClientError, ClientError, ControlClient};
use project_board::*;
use serde::Serialize;
use serde_json::{Value, json};
use std::io::{self, Write};

enum CommandExecutionError {
    ConnectionBeforeRequest,
    Request(BoardClientError),
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
    let mutation = command.is_mutation();
    let uncertain_resource = command.uncertain_resource();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            return report_connection_failure(context.json, false, None);
        }
    };
    let result = runtime.block_on(async {
        let mut client = ControlClient::connect(
            &directory,
            "agent-sessions-board",
            env!("CARGO_PKG_VERSION"),
        )
        .await
        .map_err(|_| CommandExecutionError::ConnectionBeforeRequest)?;
        let result = dispatch(&mut client, command)
            .await
            .map_err(CommandExecutionError::Request);
        let _closed = client.close().await;
        result
    });
    report(result, context.json, mutation, uncertain_resource.as_ref())
}

async fn dispatch(
    client: &mut ControlClient,
    command: PreparedBoardCommand,
) -> Result<Value, BoardClientError> {
    match command {
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
                .map(|repository| materialize_repository(client, repository))
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
            let repository = materialize_repository(client, repository)?;
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
            let repository = materialize_repository(client, repository)?;
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
        PreparedBoardCommand::MessagePost(request) => {
            serialize_result(client.board_message_post(request).await?)
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

fn materialize_repository(
    client: &ControlClient,
    repository: PreparedRepository,
) -> Result<RepositoryRef, BoardClientError> {
    match repository {
        PreparedRepository::Origin(normalized_origin) => {
            Ok(RepositoryRef::Origin { normalized_origin })
        }
        PreparedRepository::Local(common_directory) => {
            let service_id: String = client.identity().service_id.clone().into();
            let service_id = ServiceId::try_from(service_id)
                .map_err(|_| ClientError::Protocol("invalid selected service identity"))?;
            Ok(RepositoryRef::Local {
                service_id,
                common_directory,
            })
        }
    }
}

fn serialize_result<TValue: Serialize>(value: TValue) -> Result<Value, BoardClientError> {
    serde_json::to_value(value)
        .map_err(|_| ClientError::Protocol("board result could not be rendered").into())
}

fn report(
    result: Result<Value, CommandExecutionError>,
    machine: bool,
    mutation: bool,
    uncertain_resource: Option<&ResourceIdentity>,
) -> i32 {
    match result {
        Ok(result) => write_result(result, machine),
        Err(CommandExecutionError::Request(BoardClientError::Rejected(error))) => {
            if machine {
                write_json(&json!({"kind":"error","error":error}), 4)
            } else {
                let mut stderr = io::stderr().lock();
                let _written = writeln!(
                    stderr,
                    "{}\nNext action: {:?}",
                    error.message, error.next_action
                );
                4
            }
        }
        Err(CommandExecutionError::ConnectionBeforeRequest) => {
            report_connection_failure(machine, false, None)
        }
        Err(CommandExecutionError::Request(BoardClientError::Connection(_))) => {
            report_connection_failure(machine, mutation, uncertain_resource)
        }
    }
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

fn report_connection_failure(
    machine: bool,
    mutation: bool,
    uncertain_resource: Option<&ResourceIdentity>,
) -> i32 {
    let (kind, message, code) = if mutation {
        (
            "outcomeUnknown",
            "The board write connection failed. Inspect the supplied resource ID or current state before deciding whether to retry.",
            5,
        )
    } else {
        (
            "boardUnavailable",
            "The board service is unavailable. Check the selected service directory and retry the read.",
            3,
        )
    };
    if machine {
        let details = uncertain_resource.map_or_else(
            || json!({"kind":"none"}),
            |resource| json!({"kind":"resource","resource":resource}),
        );
        write_json(
            &json!({"kind":"error","error":{"kind":kind,"stage":if mutation {"inspection"} else {"storage"},"message":message,"nextAction":if mutation {"inspectResource"} else {"retryLater"},"details":details}}),
            code,
        )
    } else {
        let mut stderr = io::stderr().lock();
        let _written = writeln!(stderr, "{message}");
        if let Some(resource) = uncertain_resource {
            let rendered = serde_json::to_string(resource)
                .unwrap_or_else(|_| "affected resource unavailable".into());
            let _written = writeln!(stderr, "Affected resource: {rendered}");
        }
        code
    }
}

fn write_json(value: &Value, success_code: i32) -> i32 {
    if writeln!(io::stdout().lock(), "{value}").is_ok() {
        success_code
    } else {
        3
    }
}
