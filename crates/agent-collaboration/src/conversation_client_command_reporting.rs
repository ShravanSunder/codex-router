//! Endpoint resolution, operation identity, output, and failure rendering for conversation CLI.
use super::*;

pub(super) async fn resolve_create_endpoint(
    directory: &std::path::Path,
    endpoint: &str,
) -> Result<EndpointRef, ConversationClientError> {
    if endpoint.starts_with('{') {
        return serde_json::from_str(endpoint).map_err(|_| {
            ConversationClientError::InvalidInput("invalid --endpoint EndpointRef JSON")
        });
    }
    let endpoint_id = endpoint
        .to_owned()
        .try_into()
        .map_err(|_| ConversationClientError::InvalidInput("invalid endpoint ID"))?;
    let control =
        ControlClient::connect(directory, "agent-collaboration", env!("CARGO_PKG_VERSION")).await?;
    let resolved = EndpointRef {
        service_id: control.identity().service_id.clone(),
        endpoint_id,
    };
    control.close().await?;
    Ok(resolved)
}

pub(super) async fn endpoint_has_provider_channel(
    directory: &std::path::Path,
    endpoint: &EndpointRef,
) -> Result<bool, ConversationClientError> {
    let mut control =
        ControlClient::connect(directory, "agent-collaboration", env!("CARGO_PKG_VERSION")).await?;
    if control.identity().service_id != endpoint.service_id {
        return Err(
            ClientError::Protocol("conversation endpoint belongs to another service").into(),
        );
    }
    let description = control
        .list_endpoints()
        .await?
        .endpoints
        .into_iter()
        .find(|description| description.endpoint == *endpoint)
        .ok_or(ClientError::Protocol("conversation endpoint not found"))?;
    let provider = description.channels.iter().any(|channel| {
        matches!(
            channel,
            collaboration_client::protocol::ChannelDescription::ExternalProvider { .. }
        )
    });
    let _closed = control.close().await;
    Ok(provider)
}

pub(super) fn emit_create_start(operation_id: &OperationId, json_output: bool) -> io::Result<()> {
    if json_output {
        let record = ConversationRecord::ConversationCreateStarted {
            operation_id: operation_id.clone(),
        };
        let encoded = serde_json::to_string(&record).map_err(io::Error::other)?;
        writeln!(io::stdout(), "{encoded}")
    } else {
        writeln!(
            io::stdout(),
            "operation ID: {}",
            String::from(operation_id.clone())
        )
    }
}

pub(super) fn emit_create_outcome(
    outcome: &ConversationCreateOutcome,
    json_output: bool,
) -> io::Result<()> {
    if json_output {
        let encoded = serde_json::to_string(outcome).map_err(io::Error::other)?;
        writeln!(io::stdout(), "{encoded}")
    } else {
        match outcome {
            ConversationCreateOutcome::Created { target, .. } => writeln!(
                io::stdout(),
                "created: {}",
                serde_json::to_string(target).map_err(io::Error::other)?
            ),
            ConversationCreateOutcome::Pending { .. } => writeln!(io::stdout(), "pending"),
        }
    }
}

pub(super) fn report_create_client_error(
    error: ConversationClientError,
    operation_id: &OperationId,
    json_output: bool,
) -> i32 {
    match error {
        ConversationClientError::Codex(error) => report_create_failure(*error, json_output),
        ConversationClientError::Client(error) => {
            if let Some(exit) = crate::permission_diagnostic_reporting::report_permission_error(
                &error,
                crate::permission_diagnostic_reporting::PermissionDiagnosticRendering::Operation(
                    Some(operation_id),
                ),
                json_output,
            ) {
                return exit;
            }
            if let ClientError::Rejected {
                data: Some(data), ..
            } = &error
                && let Ok(failure) =
                    serde_json::from_value::<ConversationOperationFailure>(data.clone())
            {
                return report_create_operation_failure(&failure, json_output);
            }
            report_conversation_failure(
                operation_failure_from_client_error(error, OperationEffect::None),
                None,
                json_output,
            )
        }
        ConversationClientError::UnsupportedInput {
            endpoint,
            field,
            fix,
        } => crate::endpoint_commands::report_failure(
            "unsupportedCapability",
            &format!(
                "{field} is unsupported by {}; {fix}",
                String::from(endpoint.endpoint_id)
            ),
            4,
            json_output,
        ),
        ConversationClientError::UnavailableEndpoint {
            endpoint,
            reason,
            fix,
        } => crate::endpoint_commands::report_failure(
            "unavailable",
            &format!(
                "{} is unavailable: {reason}; fix: {fix}",
                String::from(endpoint.endpoint_id)
            ),
            4,
            json_output,
        ),
        ConversationClientError::InvalidInput(message) => {
            crate::endpoint_commands::report_failure("invalidField", message, 2, json_output)
        }
        ConversationClientError::MissingOperationId {
            endpoint,
            operation,
        } => crate::endpoint_commands::report_failure(
            "invalidField",
            &format!(
                "{operation} on {} requires --operation-id UUIDv7",
                String::from(endpoint.endpoint_id)
            ),
            2,
            json_output,
        ),
        ConversationClientError::OperationFailure(failure) => {
            report_create_operation_failure(&failure, json_output)
        }
        ConversationClientError::CallerCancelled {
            operation_id: cancelled_id,
        } => {
            let message = "caller cancelled; inspect the operation ID before retrying";
            if json_output {
                let _written = writeln!(
                    io::stdout(),
                    "{}",
                    serde_json::json!({
                        "kind":"error","error":{"kind":"callerCancelled","operationId":cancelled_id,"message":message}
                    })
                );
            } else {
                let _written = writeln!(
                    io::stderr(),
                    "{message}: {}",
                    String::from(operation_id.clone())
                );
            }
            130
        }
        ConversationClientError::AfterCreate {
            create_operation_id,
            target,
            source,
        } => {
            let message = "conversation was created, but its prompt did not complete; inspect both operation IDs before retrying";
            let (source_detail, exit_code) = match *source {
                ConversationClientError::Codex(error) => {
                    let (failure, _, _) = error.into_parts();
                    let exit_code = operation_failure_exit(&failure);
                    (serde_json::json!(failure), exit_code)
                }
                ConversationClientError::Client(ClientError::Rejected {
                    data: Some(data), ..
                }) => (data, 4),
                ConversationClientError::OperationFailure(failure) => {
                    (serde_json::json!(failure), 4)
                }
                ConversationClientError::CallerCancelled { operation_id } => (
                    serde_json::json!({"kind":"callerCancelled","operationId":operation_id}),
                    130,
                ),
                other => (serde_json::json!({"message":other.to_string()}), 5),
            };
            if json_output {
                let _written = writeln!(
                    io::stdout(),
                    "{}",
                    serde_json::json!({
                        "kind":"error","error":{"kind":"afterCreate","createOperationId":create_operation_id,
                            "target":target,"message":message,"source":source_detail}
                    })
                );
            } else {
                let _written = writeln!(
                    io::stderr(),
                    "{message}: {} ({source_detail})",
                    String::from(create_operation_id)
                );
            }
            exit_code
        }
    }
}

pub(super) fn report_create_operation_failure(
    failure: &ConversationOperationFailure,
    json_output: bool,
) -> i32 {
    let exit_code = match failure.kind {
        ConversationOperationFailureKind::InvalidRequest
        | ConversationOperationFailureKind::UnsupportedCapability
        | ConversationOperationFailureKind::ProtocolViolation => 2,
        ConversationOperationFailureKind::AuthenticationRequired
        | ConversationOperationFailureKind::Unavailable => 3,
        ConversationOperationFailureKind::PermissionRejected
        | ConversationOperationFailureKind::Busy
        | ConversationOperationFailureKind::NotFound
        | ConversationOperationFailureKind::StaleGeneration
        | ConversationOperationFailureKind::ProviderRejected => 4,
        ConversationOperationFailureKind::OutcomeUnknown => 5,
    };
    if json_output {
        let _written = writeln!(
            io::stdout(),
            "{}",
            serde_json::json!({"kind":"error","error":failure})
        );
    } else {
        let _written = writeln!(io::stderr(), "{}", String::from(failure.message.clone()));
    }
    exit_code
}

pub(super) fn report_create_failure(
    error: collaboration_client::OperationError,
    json_output: bool,
) -> i32 {
    let (failure, target, _turn_id) = error.into_parts();
    report_conversation_failure(failure, target, json_output)
}

pub(super) fn report_conversation_failure(
    failure: OperationFailure,
    target: Option<collaboration_client::protocol::SessionRef>,
    json_output: bool,
) -> i32 {
    let record = ConversationRecord::ConversationError {
        target,
        error: failure.clone(),
    };
    if json_output {
        if let Ok(encoded) = serde_json::to_string(&record) {
            let _printed = writeln!(io::stdout(), "{encoded}");
        }
    } else {
        let _printed = writeln!(io::stderr(), "{}", failure.message);
    }
    operation_failure_exit(&failure)
}

pub(super) fn parse_cli_operation_id(
    value: Option<&str>,
    json_output: bool,
) -> Result<OperationId, i32> {
    match value {
        Some(value) => OperationId::try_from(value.to_owned()).map_err(|_| {
            crate::endpoint_commands::report_failure(
                "invalidField",
                "operation ID must be a canonical lowercase RFC UUIDv7",
                2,
                json_output,
            )
        }),
        None => Ok(OperationId::generate()),
    }
}

pub(super) fn report_uninspectable_codex_operation_id(json_output: bool) -> i32 {
    crate::endpoint_commands::report_failure(
        "unsupportedCapability",
        "omit the operation ID for Codex prompts; it is not inspectable",
        2,
        json_output,
    )
}

pub(super) fn parse_cli_generation(
    value: Option<&str>,
    json_output: bool,
) -> Result<Option<CodexGeneration>, i32> {
    value.map(serde_json::from_str).transpose().map_err(|_| {
        crate::endpoint_commands::report_failure(
            "invalidField",
            "invalid --generation JSON",
            2,
            json_output,
        )
    })
}

pub(super) fn parse_session_ref(
    value: &str,
    flag: &str,
    json_output: bool,
) -> Result<SessionRef, i32> {
    serde_json::from_str(value).map_err(|_| {
        crate::endpoint_commands::report_failure(
            "invalidField",
            &format!("invalid {flag} SessionRef"),
            2,
            json_output,
        )
    })
}

pub(super) fn parse_optional_session_ref(
    value: Option<&str>,
    flag: &str,
    json_output: bool,
) -> Result<Option<SessionRef>, i32> {
    value
        .map(|value| parse_session_ref(value, flag, json_output))
        .transpose()
}

pub(super) fn emit_operation_start(
    operation_id: &OperationId,
    json_output: bool,
) -> io::Result<()> {
    if json_output {
        let record = ConversationRecord::ConversationOperationStarted {
            operation_id: operation_id.clone(),
        };
        writeln!(
            io::stdout(),
            "{}",
            serde_json::to_string(&record).map_err(io::Error::other)?
        )
    } else {
        writeln!(
            io::stdout(),
            "operation ID: {}",
            String::from(operation_id.clone())
        )
    }
}

pub(super) fn emit_operation_outcome(
    outcome: &ConversationOperationResult,
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

pub(super) fn operation_result_exit(outcome: &ConversationOperationResult) -> i32 {
    match outcome {
        ConversationOperationResult::Completed { settlement, .. } => match settlement.stop_reason {
            Some(ConversationStopReason::TimedOut) => 124,
            Some(ConversationStopReason::Cancelled) => 130,
            _ => 0,
        },
        ConversationOperationResult::Pending { .. } => 0,
    }
}

pub(super) fn emit_cancel_submission(
    submission: &collaboration_client::protocol::ConversationOperationSubmission,
    json_output: bool,
) -> io::Result<()> {
    let encoded = if json_output {
        serde_json::to_string(submission)
    } else {
        serde_json::to_string_pretty(submission)
    }
    .map_err(io::Error::other)?;
    writeln!(io::stdout(), "{encoded}")
}
