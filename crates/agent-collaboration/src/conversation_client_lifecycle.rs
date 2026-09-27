//! Provider-only lifecycle commands with durable operation IDs.

use super::*;
use collaboration_client::protocol::{
    ConversationCloseRequest, ConversationOperationWaitRequest, ConversationResumeRequest,
    PositiveSeconds, ProviderRequestedPolicy, ProviderWorkingDirectory,
};

pub(crate) fn run_resume(args: LoadArguments) -> i32 {
    if !args.cwd.is_absolute() {
        return crate::endpoint_commands::report_failure(
            "invalidField",
            "--cwd must be absolute",
            2,
            args.json,
        );
    }
    let directory = match crate::endpoint_commands::resolve_directory(args.service_directory) {
        Ok(value) => value,
        Err(message) => {
            return crate::endpoint_commands::report_failure(
                "invalidField",
                &message,
                2,
                args.json,
            );
        }
    };
    let operation_id = match parse_cli_operation_id(args.operation_id.as_deref(), args.json) {
        Ok(value) => value,
        Err(exit) => return exit,
    };
    let generation = match parse_cli_generation(args.generation.as_deref(), args.json) {
        Ok(value) => value,
        Err(exit) => return exit,
    };
    let target = match parse_session_ref(&args.target, "--target", args.json) {
        Ok(value) => value,
        Err(exit) => return exit,
    };
    let working_directory = match ProviderWorkingDirectory::try_from(args.cwd.display().to_string())
    {
        Ok(value) => value,
        Err(_) => {
            return crate::endpoint_commands::report_failure(
                "invalidField",
                "invalid --cwd",
                2,
                args.json,
            );
        }
    };
    let timeout = match u32::try_from(args.timeout_seconds)
        .ok()
        .and_then(|value| PositiveSeconds::try_from(value).ok())
    {
        Some(value) => value,
        None => {
            return crate::endpoint_commands::report_failure(
                "invalidField",
                "invalid timeout",
                2,
                args.json,
            );
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(value) => value,
        Err(_) => return 3,
    };
    runtime.block_on(async move {
        let requested_by =
            match current_session_ref(&target.endpoint.service_id, args.from.as_deref()) {
                Ok(value) => value,
                Err(message) => {
                    return crate::endpoint_commands::report_failure(
                        "invalidField",
                        &message,
                        2,
                        args.json,
                    );
                }
            };
        let approver =
            match parse_optional_session_ref(args.approver.as_deref(), "--approver", args.json) {
                Ok(value) => value.unwrap_or_else(|| requested_by.clone()),
                Err(exit) => return exit,
            };
        let mut client = match ControlClient::connect(
            &directory,
            "agent-collaboration",
            env!("CARGO_PKG_VERSION"),
        )
        .await
        {
            Ok(value) => value,
            Err(error) => {
                return report_create_client_error(
                    ConversationClientError::Client(error),
                    &operation_id,
                    args.json,
                );
            }
        };
        if emit_operation_start(&operation_id, args.json).is_err() {
            return 3;
        }
        let result = client
            .resume_provider_conversation(ConversationResumeRequest {
                operation_id: operation_id.clone(),
                target,
                generation,
                working_directory,
                requested_by,
                approver,
                requested_policy: ProviderRequestedPolicy {
                    access: match args.access {
                        ConversationAccess::WriteRestricted => RouterAccess::WriteRestricted,
                        ConversationAccess::WorkspaceWrite => RouterAccess::WorkspaceWrite,
                    },
                },
            })
            .await;
        let exit = match result {
            Ok(_) => report_wait(&mut client, operation_id, timeout, args.json).await,
            Err(error) => report_create_client_error(
                ConversationClientError::Client(error),
                &operation_id,
                args.json,
            ),
        };
        let _ = client.close().await;
        exit
    })
}

pub(crate) fn run_close(args: CloseArguments) -> i32 {
    let directory = match crate::endpoint_commands::resolve_directory(args.service_directory) {
        Ok(value) => value,
        Err(message) => {
            return crate::endpoint_commands::report_failure(
                "invalidField",
                &message,
                2,
                args.json,
            );
        }
    };
    let operation_id = match parse_cli_operation_id(args.operation_id.as_deref(), args.json) {
        Ok(value) => value,
        Err(exit) => return exit,
    };
    let generation = match parse_cli_generation(args.generation.as_deref(), args.json) {
        Ok(value) => value,
        Err(exit) => return exit,
    };
    let target = match parse_session_ref(&args.target, "--target", args.json) {
        Ok(value) => value,
        Err(exit) => return exit,
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(value) => value,
        Err(_) => return 3,
    };
    runtime.block_on(async move {
        let requested_by =
            match current_session_ref(&target.endpoint.service_id, args.from.as_deref()) {
                Ok(value) => value,
                Err(message) => {
                    return crate::endpoint_commands::report_failure(
                        "invalidField",
                        &message,
                        2,
                        args.json,
                    );
                }
            };
        let approver =
            match parse_optional_session_ref(args.approver.as_deref(), "--approver", args.json) {
                Ok(value) => value.unwrap_or_else(|| requested_by.clone()),
                Err(exit) => return exit,
            };
        let mut client = match ControlClient::connect(
            &directory,
            "agent-collaboration",
            env!("CARGO_PKG_VERSION"),
        )
        .await
        {
            Ok(value) => value,
            Err(error) => {
                return report_create_client_error(
                    ConversationClientError::Client(error),
                    &operation_id,
                    args.json,
                );
            }
        };
        if emit_operation_start(&operation_id, args.json).is_err() {
            return 3;
        }
        let result = client
            .close_provider_conversation(ConversationCloseRequest {
                operation_id: operation_id.clone(),
                target,
                generation,
                requested_by,
                approver,
            })
            .await;
        let exit = match result {
            Ok(_) => {
                report_wait(
                    &mut client,
                    operation_id,
                    PositiveSeconds::DEFAULT_SUMMARY_TIMEOUT,
                    args.json,
                )
                .await
            }
            Err(error) => report_create_client_error(
                ConversationClientError::Client(error),
                &operation_id,
                args.json,
            ),
        };
        let _ = client.close().await;
        exit
    })
}

async fn report_wait(
    client: &mut ControlClient,
    operation_id: OperationId,
    timeout_seconds: PositiveSeconds,
    json_output: bool,
) -> i32 {
    let result = client
        .wait_for_provider_conversation_operation(ConversationOperationWaitRequest {
            operation_id: operation_id.clone(),
            timeout_seconds,
        })
        .await;
    match result {
        Ok(result) => {
            let encoded = if json_output {
                serde_json::to_string(&result)
            } else {
                serde_json::to_string_pretty(&result)
            };
            match encoded {
                Ok(encoded) if writeln!(io::stdout(), "{encoded}").is_ok() => 0,
                _ => 3,
            }
        }
        Err(error) => report_create_client_error(
            ConversationClientError::Client(error),
            &operation_id,
            json_output,
        ),
    }
}
