//! Shared client routing for create, target prompt, load, and cancel CLI commands.
use super::*;

#[path = "conversation_client_command_reporting.rs"]
mod reporting;
use reporting::*;

pub(super) fn run_create(args: CreateArguments) -> i32 {
    let directory = match crate::endpoint_commands::resolve_directory(args.service_directory) {
        Ok(value) => value,
        Err(error) => {
            return crate::endpoint_commands::report_failure("invalidField", &error, 2, args.json);
        }
    };
    if !args.cwd.is_absolute()
        || args
            .model
            .as_deref()
            .is_some_and(|value| validate_choice_value(value, "--model").is_err())
        || args
            .effort
            .as_deref()
            .is_some_and(|value| validate_choice_value(value, "--effort").is_err())
    {
        return crate::endpoint_commands::report_failure(
            "invalidField",
            "Create requires an absolute --cwd and non-empty supplied --model/--effort",
            2,
            args.json,
        );
    }
    let operation_id = match args.operation_id.as_deref() {
        Some(value) => match OperationId::try_from(value.to_owned()) {
            Ok(value) => value,
            Err(_) => {
                return crate::endpoint_commands::report_failure(
                    "invalidField",
                    "operation ID must be a canonical lowercase RFC UUIDv7",
                    2,
                    args.json,
                );
            }
        },
        None => OperationId::generate(),
    };
    let generation: Option<CodexGeneration> = match args
        .generation
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
    {
        Ok(value) => value,
        Err(_) => {
            return crate::endpoint_commands::report_failure(
                "invalidField",
                "invalid --generation JSON",
                2,
                args.json,
            );
        }
    };
    let fork: Option<SessionId> = match args.fork.map(TryInto::try_into).transpose() {
        Ok(value) => value,
        Err(_) => {
            return crate::endpoint_commands::report_failure(
                "invalidField",
                "invalid --fork session ID",
                2,
                args.json,
            );
        }
    };
    let timeout = Duration::from_secs(args.timeout_seconds);
    if u32::try_from(args.timeout_seconds).is_err() {
        return crate::endpoint_commands::report_failure(
            "invalidField",
            "timeout seconds exceed the supported range",
            2,
            args.json,
        );
    }
    if emit_create_start(&operation_id, args.json).is_err() {
        return 3;
    }
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(value) => value,
        Err(_) => return 3,
    };
    runtime.block_on(async move {
        let endpoint = match resolve_create_endpoint(&directory, &args.endpoint).await {
            Ok(value) => value,
            Err(error) => return report_create_client_error(error, &operation_id, args.json),
        };
        let creator = match current_session_ref(&endpoint.service_id, args.from.as_deref()) {
            Ok(value) => value,
            Err(_) => {
                let message = if args.from.is_some() {
                    "invalid --from SessionRef"
                } else {
                    "current session identity unavailable; run agent-collaboration whoami --json or pass --from SessionRef JSON"
                };
                return report_conversation_failure(
                    operation_failure_from_client_error(
                        ClientError::Protocol(message),
                        OperationEffect::None,
                    ),
                    None,
                    args.json,
                );
            }
        };
        let approver = match args.approver.as_deref() {
            Some(value) => match serde_json::from_str(value) {
                Ok(value) => Some(value),
                Err(_) => {
                    return crate::endpoint_commands::report_failure(
                        "invalidField",
                        "invalid --approver SessionRef",
                        2,
                        args.json,
                    );
                }
            },
            None => Some(creator.clone()),
        };
        let root_message_id = match args.root_message_id.map(TryInto::try_into).transpose() {
            Ok(value) => value,
            Err(_) => {
                return crate::endpoint_commands::report_failure(
                    "invalidField",
                    "invalid root message ID",
                    2,
                    args.json,
                );
            }
        };
        let request = ConversationCreateInput {
            operation_id: operation_id.clone(),
            endpoint: endpoint.clone(),
            working_directory: args.cwd,
            fork,
            generation,
            model: args.model,
            effort: args.effort,
            access: match args.access {
                ConversationAccess::WriteRestricted => RouterAccess::WriteRestricted,
                ConversationAccess::WorkspaceWrite => RouterAccess::WorkspaceWrite,
            },
            created_by: creator,
            approver,
            root_message_id,
        };
        if let Err(error) = ConversationClient::validate_create_input(&request, timeout) {
            return report_create_client_error(error, &operation_id, args.json);
        }
        let client = match ConversationClient::connect(&directory, &endpoint).await {
            Ok(value) => value,
            Err(error) => {
                return report_create_client_error(error, &operation_id, args.json);
            }
        };
        match client.create(request, timeout).await {
            Ok(outcome) => emit_create_outcome(&outcome, args.json).map_or(3, |()| 0),
            Err(error) => report_create_client_error(error, &operation_id, args.json),
        }
    })
}

pub(super) fn run_new_prompt(args: PromptArguments) -> i32 {
    let (directory, text) = match prepare(&args) {
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
    let create_operation_id = match parse_cli_operation_id(args.operation_id.as_deref(), args.json)
    {
        Ok(value) => value,
        Err(exit) => return exit,
    };
    let prompt_operation_id =
        match parse_cli_operation_id(args.prompt_operation_id.as_deref(), args.json) {
            Ok(value) => value,
            Err(exit) => return exit,
        };
    let generation = match parse_cli_generation(args.generation.as_deref(), args.json) {
        Ok(value) => value,
        Err(exit) => return exit,
    };
    if emit_operation_start(&create_operation_id, args.json).is_err() {
        return 3;
    }
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(value) => value,
        Err(_) => return 3,
    };
    runtime.block_on(async move {
        let endpoint_id = match args.endpoint.as_deref() {
            Some(value) => value,
            None => {
                return crate::endpoint_commands::report_failure(
                    "invalidField",
                    "--endpoint is required with --new",
                    2,
                    args.json,
                );
            }
        };
        let endpoint = match resolve_create_endpoint(&directory, endpoint_id).await {
            Ok(value) => value,
            Err(error) => {
                return report_create_client_error(error, &create_operation_id, args.json);
            }
        };
        let provider = match endpoint_has_provider_channel(&directory, &endpoint).await {
            Ok(value) => value,
            Err(error) => {
                return report_create_client_error(error, &create_operation_id, args.json);
            }
        };
        if !provider && args.prompt_operation_id.is_some() {
            return report_uninspectable_codex_operation_id(args.json);
        }
        if provider && emit_operation_start(&prompt_operation_id, args.json).is_err() {
            return 3;
        }
        let creator = match current_session_ref(&endpoint.service_id, args.from.as_deref()) {
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
                Ok(value) => value.or_else(|| Some(creator.clone())),
                Err(exit) => return exit,
            };
        let root_message_id = match args.root_message_id.map(TryInto::try_into).transpose() {
            Ok(value) => value,
            Err(_) => {
                return crate::endpoint_commands::report_failure(
                    "invalidField",
                    "invalid root message ID",
                    2,
                    args.json,
                );
            }
        };
        let access = match args.access {
            Some(ConversationAccess::WriteRestricted) => RouterAccess::WriteRestricted,
            Some(ConversationAccess::WorkspaceWrite) => RouterAccess::WorkspaceWrite,
            None => {
                return crate::endpoint_commands::report_failure(
                    "invalidField",
                    "--access is required with --new",
                    2,
                    args.json,
                );
            }
        };
        let cwd = match args.cwd {
            Some(value) => value,
            None => {
                return crate::endpoint_commands::report_failure(
                    "invalidField",
                    "--cwd is required with --new",
                    2,
                    args.json,
                );
            }
        };
        let message = match text.try_into() {
            Ok(text) => PublicPromptContent::Agent {
                sender: creator.clone(),
                text,
            },
            Err(_) => {
                return crate::endpoint_commands::report_failure(
                    "invalidField",
                    "invalid conversation content",
                    2,
                    args.json,
                );
            }
        };
        let create = ConversationCreateInput {
            operation_id: create_operation_id.clone(),
            endpoint,
            working_directory: cwd,
            access,
            created_by: creator,
            approver,
            generation,
            model: args.model,
            effort: args.effort.clone(),
            fork: None,
            root_message_id,
        };
        let timeout = Duration::from_secs(args.timeout_seconds);
        if let Err(error) = ConversationClient::validate_create_input(&create, timeout) {
            return report_create_client_error(error, &create_operation_id, args.json);
        }
        let input = ConversationCreatePromptInput {
            create,
            prompt_operation_id: provider.then_some(prompt_operation_id),
            message,
            prompt_effort: args.effort,
        };
        let cancel = CancellationToken::new();
        let signal = cancel.clone();
        let signal_task = tokio::spawn(async move {
            let _signal = tokio::signal::ctrl_c().await;
            signal.cancel();
        });
        let result =
            ConversationClient::create_and_prompt(&directory, input, timeout, cancel).await;
        signal_task.abort();
        let _joined = signal_task.await;
        match result {
            Ok(outcome) => {
                emit_create_prompt_outcome(&outcome, args.json).map_or(3, |()| match &outcome {
                    ConversationCreatePromptOutcome::CreatePending { .. } => 0,
                    ConversationCreatePromptOutcome::Prompt { prompt, .. } => {
                        operation_result_exit(prompt)
                    }
                })
            }
            Err(error) => report_create_client_error(error, &create_operation_id, args.json),
        }
    })
}

fn emit_create_prompt_outcome(
    outcome: &ConversationCreatePromptOutcome,
    json_output: bool,
) -> io::Result<()> {
    let encoded = if json_output {
        serde_json::to_string(outcome)
    } else {
        serde_json::to_string_pretty(outcome)
    }
    .map_err(io::Error::other)?;
    writeln!(io::stdout(), "{encoded}")
}

pub(super) fn run_existing_prompt(args: PromptArguments) -> i32 {
    let (directory, text) = match prepare(&args) {
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
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(value) => value,
        Err(_) => return 3,
    };
    runtime.block_on(async move {
        let target = if let Some(address) = args.to.as_deref() {
            match parse_session_ref(address, "--to", args.json) {
                Ok(value) => value,
                Err(exit) => return exit,
            }
        } else {
            let selected = match conversation_target(&args) {
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
            let endpoint_id: String = selected.endpoint_id().into();
            let endpoint = match resolve_create_endpoint(&directory, &endpoint_id).await {
                Ok(value) => value,
                Err(error) => return report_create_client_error(error, &operation_id, args.json),
            };
            match selected.resolve(&endpoint.service_id) {
                Ok(value) => value,
                Err(_) => {
                    return crate::endpoint_commands::report_failure(
                        "invalidField",
                        "session target belongs to another service",
                        2,
                        args.json,
                    );
                }
            }
        };
        let sender = match current_session_ref(&target.endpoint.service_id, args.from.as_deref()) {
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
                Ok(value) => value,
                Err(exit) => return exit,
            };
        let message = match text.try_into() {
            Ok(text) => PublicPromptContent::Agent {
                sender: sender.clone(),
                text,
            },
            Err(_) => {
                return crate::endpoint_commands::report_failure(
                    "invalidField",
                    "invalid conversation content",
                    2,
                    args.json,
                );
            }
        };
        let client = match ConversationClient::connect(&directory, &target.endpoint).await {
            Ok(value) => value,
            Err(error) => return report_create_client_error(error, &operation_id, args.json),
        };
        if matches!(client, ConversationClient::CodexAcp(_)) && args.operation_id.is_some() {
            return report_uninspectable_codex_operation_id(args.json);
        }
        let provider = matches!(client, ConversationClient::ExternalProvider(_));
        if provider && emit_operation_start(&operation_id, args.json).is_err() {
            return 3;
        }
        let input = ConversationPromptInput {
            operation_id: provider.then_some(operation_id.clone()),
            target: target.clone(),
            working_directory: args.cwd,
            requested_by: sender,
            approver,
            message,
            effort: args.effort,
            generation,
        };
        let cancel = CancellationToken::new();
        let signal = cancel.clone();
        let signal_task = tokio::spawn(async move {
            let _signal = tokio::signal::ctrl_c().await;
            signal.cancel();
        });
        let result = client
            .prompt(input, Duration::from_secs(args.timeout_seconds), cancel)
            .await;
        signal_task.abort();
        let _joined = signal_task.await;
        match result {
            Ok(outcome) => emit_operation_outcome(&outcome, args.json)
                .map_or(3, |()| operation_result_exit(&outcome)),
            Err(error) => report_create_client_error(error, &operation_id, args.json),
        }
    })
}

pub(super) fn run_load(args: LoadArguments) -> i32 {
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
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(value) => value,
        Err(_) => return 3,
    };
    runtime.block_on(async move {
        let sender = match current_session_ref(&target.endpoint.service_id, args.from.as_deref()) {
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
                Ok(value) => value,
                Err(exit) => return exit,
            };
        let client = match ConversationClient::connect(&directory, &target.endpoint).await {
            Ok(value) => value,
            Err(error) => return report_create_client_error(error, &operation_id, args.json),
        };
        if matches!(client, ConversationClient::CodexAcp(_)) && args.operation_id.is_some() {
            return report_uninspectable_codex_operation_id(args.json);
        }
        let provider = matches!(client, ConversationClient::ExternalProvider(_));
        if provider && emit_operation_start(&operation_id, args.json).is_err() {
            return 3;
        }
        let input = ConversationLoadInput {
            operation_id: provider.then_some(operation_id.clone()),
            target: target.clone(),
            working_directory: args.cwd,
            requested_by: sender,
            approver,
            access: match args.access {
                ConversationAccess::WriteRestricted => RouterAccess::WriteRestricted,
                ConversationAccess::WorkspaceWrite => RouterAccess::WorkspaceWrite,
            },
            generation,
        };
        match client
            .load(input, Duration::from_secs(args.timeout_seconds))
            .await
        {
            Ok(outcome) => emit_operation_outcome(&outcome, args.json).map_or(3, |()| 0),
            Err(error) => report_create_client_error(error, &operation_id, args.json),
        }
    })
}

pub(super) fn run_cancel(args: CancelArguments) -> i32 {
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
    let target_operation_id = match OperationId::try_from(args.target_operation_id) {
        Ok(value) => value,
        Err(_) => {
            return crate::endpoint_commands::report_failure(
                "invalidField",
                "target operation ID must be a canonical lowercase RFC UUIDv7",
                2,
                args.json,
            );
        }
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
        let sender = match current_session_ref(&target.endpoint.service_id, args.from.as_deref()) {
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
                Ok(value) => value,
                Err(exit) => return exit,
            };
        let input = ConversationCancelInput {
            operation_id: operation_id.clone(),
            target_operation_id,
            target: target.clone(),
            requested_by: sender,
            approver,
            generation,
        };
        let client = match ConversationClient::connect(&directory, &target.endpoint).await {
            Ok(value) => value,
            Err(error) => return report_create_client_error(error, &operation_id, args.json),
        };
        if matches!(client, ConversationClient::ExternalProvider(_))
            && emit_operation_start(&operation_id, args.json).is_err()
        {
            return 3;
        }
        match client.cancel(input).await {
            Ok(submission) => emit_cancel_submission(&submission, args.json).map_or(3, |()| 0),
            Err(error) => report_create_client_error(error, &operation_id, args.json),
        }
    })
}
