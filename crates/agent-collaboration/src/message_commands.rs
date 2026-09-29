//! Descriptive message submission through the public Rust client.
use crate::message_input_arguments::{SendArguments, prepare};
use clap::{Parser, Subcommand};
use collaboration_client::protocol::{DeliveryOutcome, DeliveryReceipt, MessageText};
use collaboration_client::{
    ClientError, ControlClient, MessageReplyError, MessageReplyRequest, MessageSendError,
    MessageSendRequest, OperationEffect, OperationFailure, OperationFailureKind,
};
use serde_json::json;
use std::{
    ffi::OsString,
    io::{self, Read, Write},
    path::PathBuf,
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
    /// Reply to the most recent Agent sender delivered to this session.
    Reply(ReplyArguments),
}

#[derive(clap::Args)]
struct ReplyArguments {
    #[arg(
        long,
        required_unless_present = "text_file",
        conflicts_with = "text_file"
    )]
    text: Option<String>,
    /// Read reply text from a file; '-' reads stdin. Content is never shell-interpolated.
    #[arg(long)]
    text_file: Option<PathBuf>,
    /// Refuse unless this SessionRef JSON still matches the latest Agent sender.
    #[arg(long)]
    expect_sender: Option<String>,
    #[arg(long)]
    service_directory: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}
pub fn run_message_command(arguments: Vec<OsString>) -> i32 {
    let parsed =
        match crate::automation_argument_feedback::parse_arguments::<MessageArguments>(arguments) {
            Ok(value) => value,
            Err(code) => return code,
        };
    let args = match parsed.command {
        MessageCommand::Send(args) => args,
        MessageCommand::Reply(args) => return run_message_reply(args),
    };
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
                .map_err(|error| MessageSendError::Preparation(Box::new(error)))?;
        let saved = prepared
            .resolve(&client.identity().service_id)
            .map_err(|_| {
                MessageSendError::Preparation(Box::new(ClientError::InvalidRequest(
                    "invalid session target",
                )))
            })?;
        let request = MessageSendRequest {
            target: saved.target,
            message: saved
                .content
                .try_into()
                .map_err(|error| MessageSendError::Preparation(Box::new(error)))?,
            delivery: saved.delivery,
            generation_guard: saved.generation_guard,
            correlation: None,
        };
        let result = client.send_message(request).await;
        let _closed = client.close().await;
        result
    });
    report(outcome, machine)
}

fn run_message_reply(args: ReplyArguments) -> i32 {
    let machine = args.json;
    let expect_sender = match args
        .expect_sender
        .as_deref()
        .map(serde_json::from_str::<collaboration_client::protocol::SessionRef>)
        .transpose()
    {
        Ok(value) => value,
        Err(_) => {
            return crate::endpoint_commands::report_failure(
                "invalidField",
                &crate::message_input_arguments::session_ref_guidance("--expect-sender"),
                2,
                machine,
            );
        }
    };
    let reply_text = match read_reply_text(&args) {
        Ok(text) => text,
        Err(message) => {
            return crate::endpoint_commands::report_failure("invalidField", &message, 2, machine);
        }
    };
    let harness_identity = match crate::current_session_identity::read_harness_session_identity() {
        Ok(identity) => identity,
        Err(error) => {
            return crate::endpoint_commands::report_failure(
                "currentSessionUnavailable",
                &error.to_string(),
                2,
                machine,
            );
        }
    };
    let directory = match crate::endpoint_commands::resolve_directory(args.service_directory) {
        Ok(directory) => directory,
        Err(message) => {
            return crate::endpoint_commands::report_failure("invalidField", &message, 2, machine);
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
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
                .map_err(|error| MessageReplyError::Preparation(Box::new(error)))?;
        let caller = harness_identity
            .session_ref(&client.identity().service_id)
            .map_err(|_| {
                MessageReplyError::Preparation(Box::new(ClientError::InvalidRequest(
                    "invalid caller session identity",
                )))
            })?;
        let result = client
            .reply_to_latest_agent_sender(MessageReplyRequest {
                caller,
                expect_sender,
                text: reply_text,
            })
            .await;
        let _closed = client.close().await;
        result
    });
    report_reply(outcome, machine)
}

fn read_reply_text(args: &ReplyArguments) -> Result<MessageText, String> {
    let text = if let Some(text) = &args.text {
        text.clone()
    } else {
        let path = args
            .text_file
            .as_ref()
            .ok_or_else(|| "Message content required".to_owned())?;
        let mut reader: Box<dyn Read> = if path.as_os_str() == "-" {
            Box::new(io::stdin())
        } else {
            Box::new(std::fs::File::open(path).map_err(|_| "Message file unavailable")?)
        };
        let mut text = String::new();
        reader
            .by_ref()
            .take((collaboration_client::protocol::MAX_CONTROL_FRAME_BYTES + 1) as u64)
            .read_to_string(&mut text)
            .map_err(|_| "Cannot read UTF-8 message")?;
        text
    };
    text.try_into()
        .map_err(|_| "Invalid or oversized message text".to_owned())
}

fn report_reply(
    result: Result<collaboration_client::protocol::SessionMessageReplyResult, MessageReplyError>,
    machine: bool,
) -> i32 {
    let (record, exit_code, confirmation) = match result {
        Ok(reply) => {
            let exit_code = receipt_exit_status(&reply.receipt.outcome);
            let target_line = if exit_code == 0 {
                reply_confirmation_line(&reply)
            } else {
                reply_target_line(&reply)
            };
            (
                crate::endpoint_commands::result_envelope(serde_json::json!(reply.clone())),
                exit_code,
                Some(target_line),
            )
        }
        Err(error) => {
            let (failure, caller) = error.into_operation_failure_and_caller();
            let exit_code = operation_failure_exit(&failure);
            (
                serde_json::json!({"kind":"error","caller":caller,"error":failure}),
                exit_code,
                None,
            )
        }
    };
    let written = if machine {
        writeln!(io::stdout(), "{record}")
    } else if let Some(confirmation) = confirmation {
        writeln!(io::stdout(), "{confirmation}")
    } else {
        writeln!(
            io::stdout(),
            "{}",
            serde_json::to_string_pretty(&record)
                .unwrap_or_else(|_| "Output unavailable".to_owned())
        )
    };
    if written.is_err() { 5 } else { exit_code }
}

fn reply_confirmation_line(
    reply: &collaboration_client::protocol::SessionMessageReplyResult,
) -> String {
    format!(
        "replied to {} {}",
        reply.target_identity,
        session_ref_text(reply)
    )
}

fn reply_target_line(reply: &collaboration_client::protocol::SessionMessageReplyResult) -> String {
    format!(
        "reply target: {} {}",
        reply.target_identity,
        session_ref_text(reply)
    )
}

fn session_ref_text(reply: &collaboration_client::protocol::SessionMessageReplyResult) -> String {
    let target = serde_json::to_string(&reply.target)
        .unwrap_or_else(|_| "SessionRef unavailable".to_owned());
    target
}

fn report(result: Result<DeliveryReceipt, MessageSendError>, machine: bool) -> i32 {
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
        Ok(receipt) => {
            let exit = receipt_exit_status(&receipt.outcome);
            (
                crate::endpoint_commands::result_envelope(json!(receipt)),
                exit,
                5,
            )
        }
        Err(error) => {
            let (failure, target) = error.into_operation_failure_and_target();
            let exit = operation_failure_exit(&failure);
            let write_failure = if failure.effect == OperationEffect::Unknown {
                5
            } else {
                3
            };
            (
                json!({"kind":"error","target":target,"error":failure}),
                exit,
                write_failure,
            )
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

fn receipt_exit_status(outcome: &DeliveryOutcome) -> i32 {
    match outcome {
        DeliveryOutcome::NotSubmitted {
            retryable: true, ..
        } => 3,
        DeliveryOutcome::NotSubmitted {
            retryable: false, ..
        }
        | DeliveryOutcome::Rejected(_) => 4,
        DeliveryOutcome::Unknown => 5,
        DeliveryOutcome::Started
        | DeliveryOutcome::Steered
        | DeliveryOutcome::StartedOrSteered
        | DeliveryOutcome::Queued
        | DeliveryOutcome::PeerMessageWritten => 0,
    }
}

fn operation_failure_exit(failure: &OperationFailure) -> i32 {
    match failure.kind {
        OperationFailureKind::Rejected
            if failure.service_kind.as_deref() == Some("outcomeUnknown") =>
        {
            5
        }
        OperationFailureKind::Rejected
            if failure.service_kind.as_deref() == Some("unsupportedCapability") =>
        {
            2
        }
        OperationFailureKind::Rejected
            if matches!(
                failure.service_kind.as_deref(),
                Some("unavailable" | "replyUnavailable")
            ) =>
        {
            3
        }
        OperationFailureKind::Rejected => 4,
        _ if failure.effect == OperationEffect::Unknown => 5,
        OperationFailureKind::UnsupportedCapability | OperationFailureKind::ProtocolViolation => 2,
        OperationFailureKind::Unavailable if failure.effect == OperationEffect::None => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod reply_argument_tests {
    use super::{MessageArguments, MessageCommand, ReplyArguments, reply_confirmation_line};
    use clap::Parser;

    #[test]
    fn message_reply_requires_text_or_a_text_file() {
        let text_request = MessageArguments::try_parse_from([
            "agent-collaboration message",
            "reply",
            "--text",
            "hello",
        ])
        .expect("text reply arguments parse");
        assert!(matches!(
            text_request.command,
            MessageCommand::Reply(ReplyArguments {
                text: Some(ref text),
                text_file: None,
                ..
            }) if text == "hello"
        ));

        let file_request = MessageArguments::try_parse_from([
            "agent-collaboration message",
            "reply",
            "--text-file",
            "reply.md",
        ])
        .expect("file reply arguments parse");
        assert!(matches!(
            file_request.command,
            MessageCommand::Reply(ReplyArguments {
                text: None,
                text_file: Some(_),
                ..
            })
        ));

        let guarded_request = MessageArguments::try_parse_from([
            "agent-collaboration message",
            "reply",
            "--text",
            "hello",
            "--expect-sender",
            "{\"endpoint\":{},\"sessionId\":\"sender\"}",
        ])
        .expect("guarded reply arguments parse");
        assert!(matches!(
            guarded_request.command,
            MessageCommand::Reply(ReplyArguments {
                expect_sender: Some(ref expected),
                ..
            }) if expected.contains("sender")
        ));

        assert!(
            MessageArguments::try_parse_from(["agent-collaboration message", "reply"]).is_err()
        );
    }

    #[test]
    fn reply_confirmation_prints_the_resolved_identity_and_session_ref() {
        let target: collaboration_client::protocol::SessionRef = serde_json::from_value(
            serde_json::json!({
                "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},
                "sessionId":"reply-target"
            }),
        )
        .expect("reply target");
        let reply = collaboration_client::protocol::SessionMessageReplyResult {
            target: target.clone(),
            target_identity: "✳️ Claude Main".to_owned(),
            receipt: collaboration_client::protocol::DeliveryReceipt {
                outcome: collaboration_client::protocol::DeliveryOutcome::PeerMessageWritten,
                reachability: Some(
                    collaboration_client::protocol::SessionReachability::ClaudeCodePeer,
                ),
                client: Some(collaboration_client::protocol::DeliveryClientReceipt::ClaudeCodePeer),
            },
        };

        assert_eq!(
            reply_confirmation_line(&reply),
            format!(
                "replied to ✳️ Claude Main {}",
                serde_json::to_string(&target).expect("target JSON")
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{operation_failure_exit, receipt_exit_status};
    use collaboration_client::{ClientError, MessageSendError, OperationEffect};

    #[test]
    fn cli_receipt_keeps_foreign_writer_reason_action_and_rejected_exit() {
        let receipt: collaboration_client::protocol::DeliveryReceipt =
            serde_json::from_value(serde_json::json!({
                "outcome":{
                    "kind":"rejected","reason":"heldByAnotherClient",
                    "nextAction":"messageFromHoldingCodexClient","clientCode":-32600,
                    "detail":"Message it from the Codex client that holds it."
                },
                "reachability":"codexAppServer","client":null
            }))
            .expect("typed receipt");
        assert_eq!(receipt_exit_status(&receipt.outcome), 4);
        let output = crate::endpoint_commands::result_envelope(serde_json::json!(receipt));
        assert_eq!(
            output["result"]["record"]["outcome"]["reason"],
            "heldByAnotherClient"
        );
        assert_eq!(
            output["result"]["record"]["outcome"]["nextAction"],
            "messageFromHoldingCodexClient"
        );
    }

    #[test]
    fn adapter_distinguishes_preparation_loss_from_post_dispatch_loss() {
        let transport = || {
            ClientError::Transport(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "fixture",
            ))
        };
        let (preparation, preparation_target) =
            MessageSendError::Preparation(Box::new(transport()))
                .into_operation_failure_and_target();
        assert!(preparation_target.is_none());
        assert_eq!(preparation.effect, OperationEffect::None);
        assert_eq!(
            preparation.kind,
            collaboration_client::OperationFailureKind::Unavailable
        );

        let (submission, submission_target) = MessageSendError::Submission {
            target: serde_json::from_value(serde_json::json!({
                "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
                "sessionId":"target"
            }))
            .expect("target"),
            source: Box::new(transport()),
        }
        .into_operation_failure_and_target();
        assert!(submission_target.is_some());
        assert_eq!(submission.effect, OperationEffect::Unknown);
        assert_eq!(
            submission.kind,
            collaboration_client::OperationFailureKind::Unavailable
        );

        let (malformed_after_dispatch, malformed_target) = MessageSendError::Submission {
            target: serde_json::from_value(serde_json::json!({
                "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
                "sessionId":"target"
            }))
            .expect("target"),
            source: Box::new(ClientError::Protocol("malformed result")),
        }
        .into_operation_failure_and_target();
        assert!(malformed_target.is_some());
        assert_eq!(malformed_after_dispatch.effect, OperationEffect::Unknown);
        assert_eq!(operation_failure_exit(&malformed_after_dispatch), 5);
    }

    #[test]
    fn adapter_preserves_established_service_rejection_exit_codes() {
        for (service_kind, expected) in [
            ("outcomeUnknown", 5),
            ("unsupportedCapability", 2),
            ("unavailable", 3),
            ("nativeRejected", 4),
        ] {
            let (failure, _) = MessageSendError::Preparation(Box::new(ClientError::Rejected {
                code: -32050,
                data: Some(serde_json::json!({"kind":service_kind})),
            }))
            .into_operation_failure_and_target();
            assert_eq!(operation_failure_exit(&failure), expected, "{service_kind}");
        }
    }
}
