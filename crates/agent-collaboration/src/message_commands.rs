//! Descriptive message submission through the public Rust client.
use crate::message_input_arguments::{SendArguments, prepare};
use clap::{Parser, Subcommand};
use collaboration_client::protocol::NativeSendReceipt;
use collaboration_client::{ClientError, ControlClient, MessageSendError, MessageSendRequest};
use serde_json::{Value, json};
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
    let (result, submitted) = classify_send_outcome(outcome);
    report(result, machine, submitted)
}

fn classify_send_outcome(
    outcome: Result<NativeSendReceipt, MessageSendError>,
) -> (Result<NativeSendReceipt, ClientError>, bool) {
    match outcome {
        Ok(receipt) => (Ok(receipt), true),
        Err(MessageSendError::Preparation(error)) => (Err(error), false),
        Err(MessageSendError::Submission(error)) => (Err(error), true),
    }
}

fn report(result: Result<NativeSendReceipt, ClientError>, machine: bool, submitted: bool) -> i32 {
    if !submitted
        && let Err(error) = &result
        && let Some(code) = crate::permission_diagnostic_reporting::report_permission_error(
            error,
            crate::permission_diagnostic_reporting::PermissionDiagnosticRendering::Command,
            machine,
        )
    {
        return code;
    }
    let (record, code) = match result {
        Ok(receipt) => (crate::endpoint_commands::result_envelope(json!(receipt)), 0),
        Err(ClientError::Rejected { code, data }) => {
            let kind = data
                .as_ref()
                .and_then(|d| d.get("kind"))
                .and_then(Value::as_str);
            let exit = match kind {
                Some("outcomeUnknown") => 5,
                Some("unsupportedCapability") => 2,
                Some("unavailable") => 3,
                _ => 4,
            };
            (
                json!({"kind":"error","error":{"code":code,"data":data}}),
                exit,
            )
        }
        Err(error) => (
            json!({"kind":"error","error":{"kind":if submitted {"outcomeUnknown"} else {"unavailable"},"message":safe_connection_failure(&error)}}),
            if submitted { 5 } else { 3 },
        ),
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
        if submitted { 5 } else { 3 }
    } else {
        code
    }
}

fn safe_connection_failure(error: &ClientError) -> String {
    let category = match error {
        ClientError::Discovery { stage, source } => format!(
            "{stage}: IO {:?} (OS code {:?})",
            source.kind(),
            source.raw_os_error()
        ),
        ClientError::Transport(error) => {
            format!("IO {:?} (OS code {:?})", error.kind(), error.raw_os_error())
        }
        ClientError::Timeout => "request deadline".to_owned(),
        ClientError::InvalidRequest(_) => "request validation".to_owned(),
        ClientError::Protocol(_) => "protocol validation".to_owned(),
        ClientError::UnsupportedCapability(_) => "unsupported capability".to_owned(),
        ClientError::Rejected { .. } => "server rejection".to_owned(),
    };
    format!("Connection failed: {category}; no message replayed")
}

#[cfg(test)]
mod tests {
    use super::classify_send_outcome;
    use collaboration_client::{ClientError, MessageSendError};

    #[test]
    fn adapter_distinguishes_preparation_loss_from_post_dispatch_loss() {
        let transport = || {
            ClientError::Transport(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "fixture",
            ))
        };
        let (preparation, submitted) =
            classify_send_outcome(Err(MessageSendError::Preparation(transport())));
        assert!(!submitted);
        assert!(matches!(preparation, Err(ClientError::Transport(_))));

        let (submission, submitted) =
            classify_send_outcome(Err(MessageSendError::Submission(transport())));
        assert!(submitted);
        assert!(matches!(submission, Err(ClientError::Transport(_))));
    }
}
