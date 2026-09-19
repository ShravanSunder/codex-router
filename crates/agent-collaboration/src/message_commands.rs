//! Descriptive message submission through the public Rust client.
use crate::message_input_arguments::{SendArguments, prepare};
use clap::{Parser, Subcommand};
use collaboration_client::protocol::NativeSendReceipt;
use collaboration_client::{
    ClientError, ControlClient, MessageSendError, MessageSendRequest, OperationEffect,
    OperationFailure, OperationFailureKind,
};
use serde_json::json;
use std::{
    ffi::OsString,
    io::{self, Write},
};

#[derive(Parser)]
#[command(
    name = "agent-collaboration message",
    bin_name = "agent-collaboration message"
)]
struct MessageArguments {
    #[command(subcommand)]
    command: MessageCommand,
}
#[derive(Subcommand)]
enum MessageCommand {
    /// Submit information. Acceptance is not completion or a peer reply.
    Send(SendArguments),
}
pub fn run_message_command(arguments: Vec<OsString>) -> i32 {
    let parsed =
        match crate::automation_argument_feedback::parse_arguments::<MessageArguments>(arguments) {
            Ok(value) => value,
            Err(code) => return code,
        };
    let MessageCommand::Send(args) = parsed.command;
    let machine = args.json;
    let prepared = prepare(&args);
    let (directory, prepared) = match prepared {
        Ok(value) => value,
        Err(message) => {
            return crate::endpoint_commands::report_failure("invalidField", &message, 2, machine);
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(value) => value,
        Err(_) => {
            return crate::endpoint_commands::report_failure(
                "unavailable",
                "Client runtime unavailable",
                3,
                machine,
            );
        }
    };
    let outcome = runtime.block_on(async {
        let mut client =
            ControlClient::connect(&directory, "agent-collaboration", env!("CARGO_PKG_VERSION"))
                .await
                .map_err(MessageSendError::Preparation)?;
        let saved = prepared
            .resolve(&client.identity().service_id)
            .map_err(|_| {
                MessageSendError::Preparation(ClientError::InvalidRequest("invalid session target"))
            })?;
        let request = MessageSendRequest {
            target: saved.target,
            message: saved
                .content
                .try_into()
                .map_err(MessageSendError::Preparation)?,
            delivery: saved.delivery,
            generation_guard: saved.generation_guard,
            client_user_message_id: None,
        };
        let result = client.send_message(request).await;
        let _closed = client.close().await;
        result
    });
    report(outcome, machine)
}

fn report(result: Result<NativeSendReceipt, MessageSendError>, machine: bool) -> i32 {
    if let Err(MessageSendError::Preparation(error)) = &result
        && let Some(code) = crate::permission_diagnostic_reporting::report_permission_error(
            error,
            crate::permission_diagnostic_reporting::PermissionDiagnosticRendering::Command,
            machine,
        )
    {
        return code;
    }
    let (record, code, write_failure_code) = match result {
        Ok(receipt) => (
            crate::endpoint_commands::result_envelope(json!(receipt)),
            0,
            5,
        ),
        Err(error) => {
            let failure = error.into_operation_failure();
            let exit = operation_failure_exit(&failure);
            let write_failure = if failure.effect == OperationEffect::Unknown {
                5
            } else {
                3
            };
            (json!({"kind":"error","error":failure}), exit, write_failure)
        }
    };
    let written = if machine {
        writeln!(io::stdout(), "{record}")
    } else {
        writeln!(
            io::stdout(),
            "{}",
            serde_json::to_string_pretty(&record)
                .unwrap_or_else(|_| "Output unavailable".to_owned())
        )
    };
    if written.is_err() {
        write_failure_code
    } else {
        code
    }
}

fn operation_failure_exit(failure: &OperationFailure) -> i32 {
    match failure.kind {
        OperationFailureKind::Rejected => 4,
        _ if failure.effect == OperationEffect::Unknown => 5,
        OperationFailureKind::UnsupportedCapability | OperationFailureKind::ProtocolViolation => 2,
        OperationFailureKind::Unavailable if failure.effect == OperationEffect::None => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::operation_failure_exit;
    use collaboration_client::{ClientError, MessageSendError, OperationEffect};

    #[test]
    fn adapter_distinguishes_preparation_loss_from_post_dispatch_loss() {
        let transport = || {
            ClientError::Transport(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "fixture",
            ))
        };
        let preparation = MessageSendError::Preparation(transport()).into_operation_failure();
        assert_eq!(preparation.effect, OperationEffect::None);
        assert_eq!(
            preparation.kind,
            collaboration_client::OperationFailureKind::Unavailable
        );

        let submission = MessageSendError::Submission(transport()).into_operation_failure();
        assert_eq!(submission.effect, OperationEffect::Unknown);
        assert_eq!(
            submission.kind,
            collaboration_client::OperationFailureKind::Unavailable
        );

        let malformed_after_dispatch =
            MessageSendError::Submission(ClientError::Protocol("malformed result"))
                .into_operation_failure();
        assert_eq!(malformed_after_dispatch.effect, OperationEffect::Unknown);
        assert_eq!(operation_failure_exit(&malformed_after_dispatch), 5);
    }
}
