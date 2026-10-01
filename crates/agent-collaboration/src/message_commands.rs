//! CLI commands for direct messages and stored Router push records.
use crate::failure_line::{
    render_failure_line, render_held_line, render_rejection_line, render_unknown_line,
};
use crate::message_input_arguments::{MessageSendArguments, prepare_message_send};
use clap::{Parser, Subcommand};
use collaboration_client::protocol::{
    DeliveryOutcome, MessageText, PushDeliveryState, PushId, PushRecordHistoryParams,
    PushRecordListParams, PushRecordListResult, PushRecordShowParams, PushRecordShowResult,
    RouterLink, SessionRef,
};
use collaboration_client::{
    ClientError, ControlClient, MessageReplyError, MessageReplyRequest, MessageSendError,
    MessageSendRequest, OperationEffect, OperationFailure, OperationFailureKind,
    operation_failure_from_client_error,
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
    Send(MessageSendArguments),
    /// List unread direct-message notices for this session.
    Inbox(InboxArguments),
    /// List retained direct-message notices with one session.
    History(HistoryArguments),
    /// Reply to one stored direct message by push id or Router link.
    Reply(ReplyArguments),
}

#[derive(clap::Args)]
struct InboxArguments {
    #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=100))]
    limit: u32,
    #[arg(long)]
    service_directory: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args)]
struct HistoryArguments {
    /// Exact SessionRef JSON for the other participant.
    #[arg(long = "with")]
    other_session: String,
    #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=100))]
    limit: u32,
    #[arg(long)]
    service_directory: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args)]
struct ReplyArguments {
    #[arg(value_name = "PUSH_ID_OR_LINK")]
    reference: String,
    #[arg(
        value_name = "TEXT",
        required_unless_present = "text_file",
        conflicts_with = "text_file"
    )]
    text: Option<String>,
    /// Read reply text from a file; '-' reads stdin. Content is never shell-interpolated.
    #[arg(long)]
    text_file: Option<PathBuf>,
    #[arg(long)]
    service_directory: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
#[command(
    name = "agent-collaboration show",
    bin_name = "agent-collaboration show",
    about = "Fetch one stored Router push record"
)]
struct PushRecordShowArguments {
    #[arg(value_name = "PUSH_ID_OR_LINK")]
    reference: String,
    #[arg(long)]
    service_directory: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

enum PushRecordQuery {
    Show(String),
    Inbox { limit: u32 },
    History { with: SessionRef, limit: u32 },
}

enum PushRecordReadResult {
    Show(Box<PushRecordShowResult>),
    List(PushRecordListResult),
}

pub fn run_message_command(arguments: Vec<OsString>) -> i32 {
    let parsed =
        match crate::automation_argument_feedback::parse_arguments::<MessageArguments>(arguments) {
            Ok(value) => value,
            Err(code) => return code,
        };
    match parsed.command {
        MessageCommand::Send(args) => run_message_send(args),
        MessageCommand::Inbox(args) => run_message_inbox(args),
        MessageCommand::History(args) => run_message_history(args),
        MessageCommand::Reply(args) => run_message_reply(args),
    }
}

pub fn run_push_record_show_command(arguments: Vec<OsString>) -> i32 {
    let args = match crate::automation_argument_feedback::parse_arguments::<PushRecordShowArguments>(
        arguments,
    ) {
        Ok(value) => value,
        Err(code) => return code,
    };
    if let Err(message) = validate_push_reference(&args.reference) {
        return report_message_failure("invalidField", &message, 2, args.json);
    }
    run_push_record_query(
        args.service_directory,
        args.json,
        PushRecordQuery::Show(args.reference),
    )
}

fn run_message_send(args: MessageSendArguments) -> i32 {
    let machine = args.json;
    let prepared = prepare_message_send(&args);
    let (directory, prepared) = match prepared {
        Ok(value) => value,
        Err(message) => {
            return report_message_failure("invalidField", &message, 2, machine);
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(value) => value,
        Err(_) => {
            return report_message_failure("unavailable", "Client runtime unavailable", 3, machine);
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
        };
        let result = client.send_message(request).await;
        let _closed = client.close().await;
        result
    });
    report(outcome, machine)
}

fn run_message_inbox(args: InboxArguments) -> i32 {
    run_push_record_query(
        args.service_directory,
        args.json,
        PushRecordQuery::Inbox { limit: args.limit },
    )
}

fn run_message_history(args: HistoryArguments) -> i32 {
    let with = match serde_json::from_str::<SessionRef>(&args.other_session) {
        Ok(value) => value,
        Err(_) => {
            return report_message_failure(
                "invalidField",
                "--with must be compact SessionRef JSON with endpoint.serviceId, endpoint.endpointId, and sessionId; for example --with '{\"endpoint\":{\"serviceId\":\"00000000-0000-4000-8000-000000000001\",\"endpointId\":\"codex-local\"},\"sessionId\":\"target-session\"}'",
                2,
                args.json,
            );
        }
    };
    run_push_record_query(
        args.service_directory,
        args.json,
        PushRecordQuery::History {
            with,
            limit: args.limit,
        },
    )
}

fn run_push_record_query(
    service_directory: Option<PathBuf>,
    machine: bool,
    query: PushRecordQuery,
) -> i32 {
    let harness_identity = match crate::current_session_identity::read_harness_session_identity() {
        Ok(identity) => identity,
        Err(error) => {
            return report_message_failure(
                "currentSessionUnavailable",
                &format!("{error}; run agent-collaboration whoami --json"),
                2,
                machine,
            );
        }
    };
    let directory = match crate::endpoint_commands::resolve_directory(service_directory) {
        Ok(directory) => directory,
        Err(message) => {
            return report_message_failure("invalidField", &message, 2, machine);
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            return report_message_failure("unavailable", "Client runtime unavailable", 3, machine);
        }
    };
    let outcome = runtime.block_on(async {
        let mut client =
            ControlClient::connect(&directory, "agent-collaboration", env!("CARGO_PKG_VERSION"))
                .await?;
        let caller = harness_identity
            .session_ref(&client.identity().service_id)
            .map_err(|_| ClientError::InvalidRequest("invalid current session identity"))?;
        let result = match query {
            PushRecordQuery::Show(reference) => client
                .router_show(PushRecordShowParams { caller, reference })
                .await
                .map(Box::new)
                .map(PushRecordReadResult::Show),
            PushRecordQuery::Inbox { limit } => client
                .message_inbox(PushRecordListParams { caller, limit })
                .await
                .map(PushRecordReadResult::List),
            PushRecordQuery::History { with, limit } => client
                .message_history(PushRecordHistoryParams {
                    caller,
                    with,
                    limit,
                })
                .await
                .map(PushRecordReadResult::List),
        };
        let _closed = client.close().await;
        result
    });
    report_push_record_read(outcome, machine)
}

fn report_push_record_read(
    result: Result<PushRecordReadResult, ClientError>,
    machine: bool,
) -> i32 {
    if let Err(error) = &result
        && let Some(code) = crate::permission_diagnostic_reporting::report_permission_error(
            error,
            crate::permission_diagnostic_reporting::PermissionDiagnosticRendering::Command,
            machine,
        )
    {
        return code;
    }
    match result {
        Ok(PushRecordReadResult::Show(show)) => {
            let written = if machine {
                writeln!(io::stdout(), "{}", push_record_show_envelope(*show))
            } else {
                writeln!(
                    io::stdout(),
                    "{}",
                    serde_json::to_string_pretty(&show)
                        .unwrap_or_else(|_| "Output unavailable".to_owned())
                )
            };
            if written.is_ok() { 0 } else { 3 }
        }
        Ok(PushRecordReadResult::List(list)) => {
            if machine {
                if writeln!(
                    io::stdout(),
                    "{}",
                    crate::endpoint_commands::result_envelope(json!(list))
                )
                .is_ok()
                {
                    0
                } else {
                    3
                }
            } else {
                let mut output = io::stdout().lock();
                for record in list.records {
                    if writeln!(output, "{}", record.line).is_err() {
                        return 3;
                    }
                }
                0
            }
        }
        Err(error) => {
            let failure = operation_failure_from_client_error(error, OperationEffect::None);
            let exit_code = operation_failure_exit(&failure);
            let human_failure = operation_failure_line(&failure, None);
            let record = json!({"kind":"error","error":failure});
            let written = if machine {
                writeln!(io::stdout(), "{record}")
            } else {
                writeln!(io::stderr(), "{human_failure}")
            };
            if written.is_ok() { exit_code } else { 5 }
        }
    }
}

fn push_record_show_envelope(show: PushRecordShowResult) -> serde_json::Value {
    let link = show.link;
    let activity_ranges = show.activity_ranges;
    let mut envelope = crate::endpoint_commands::result_envelope(json!(show.record));
    if let Some(result) = envelope
        .get_mut("result")
        .and_then(serde_json::Value::as_object_mut)
    {
        result.insert("link".to_owned(), json!(link));
        result.insert("activityRanges".to_owned(), json!(activity_ranges));
    }
    envelope
}

fn run_message_reply(args: ReplyArguments) -> i32 {
    let machine = args.json;
    let reference = args.reference.clone();
    if let Err(message) = validate_push_reference(&reference) {
        return report_message_failure("invalidField", &message, 2, machine);
    }
    let reply_text = match read_reply_text(&args) {
        Ok(text) => text,
        Err(message) => {
            return report_message_failure("invalidField", &message, 2, machine);
        }
    };
    let harness_identity = match crate::current_session_identity::read_harness_session_identity() {
        Ok(identity) => identity,
        Err(error) => {
            return report_message_failure(
                "currentSessionUnavailable",
                &format!("{error}; run agent-collaboration whoami --json"),
                2,
                machine,
            );
        }
    };
    let directory = match crate::endpoint_commands::resolve_directory(args.service_directory) {
        Ok(directory) => directory,
        Err(message) => {
            return report_message_failure("invalidField", &message, 2, machine);
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            return report_message_failure("unavailable", "Client runtime unavailable", 3, machine);
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
            .reply_to_push(MessageReplyRequest {
                caller,
                reference,
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

fn validate_push_reference(reference: &str) -> Result<(), String> {
    if PushId::try_from(reference.to_owned()).is_ok() || RouterLink::parse(reference).is_ok() {
        return Ok(());
    }
    Err("PUSH_ID_OR_LINK must be a UUIDv7 push id or router://<machine-id>/push/<push-id>; for example 019f0000-0000-7000-8000-000000000101 or router://00000000-0000-4000-8000-000000000001/push/019f0000-0000-7000-8000-000000000101".to_owned())
}

fn report_reply(
    result: Result<collaboration_client::protocol::SessionMessageReplyResult, MessageReplyError>,
    machine: bool,
) -> i32 {
    let (record, exit_code, confirmation, failure_line, write_failure_code) = match result {
        Ok(reply) => {
            let exit_code = receipt_exit_status(reply.delivery_state);
            let failure_line = receipt_failure_line(
                reply.delivery_state,
                &reply.target_identity,
                &reply.link,
                &reply.receipt.outcome,
            );
            let confirmation = failure_line
                .is_none()
                .then(|| reply_confirmation_line(&reply));
            (
                crate::endpoint_commands::result_envelope(serde_json::json!(reply)),
                exit_code,
                confirmation,
                failure_line,
                5,
            )
        }
        Err(error) => {
            let (failure, caller) = error.into_operation_failure_and_caller();
            let exit_code = operation_failure_exit(&failure);
            let failure_line = operation_failure_line(&failure, None);
            (
                serde_json::json!({"kind":"error","caller":caller,"error":failure}),
                exit_code,
                None,
                Some(failure_line),
                if exit_code == 5 { 5 } else { 3 },
            )
        }
    };
    let written = if machine {
        writeln!(io::stdout(), "{record}")
    } else if let Some(failure_line) = failure_line {
        writeln!(io::stderr(), "{failure_line}")
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
    if written.is_err() {
        write_failure_code
    } else {
        exit_code
    }
}

fn reply_confirmation_line(
    reply: &collaboration_client::protocol::SessionMessageReplyResult,
) -> String {
    format!(
        "replied to {} {} (push {} {}): {}",
        reply.target_identity,
        session_ref_text(reply),
        reply.push_id.as_str(),
        reply.link,
        delivery_outcome_label(&reply.receipt.outcome)
    )
}

fn session_ref_text(reply: &collaboration_client::protocol::SessionMessageReplyResult) -> String {
    serde_json::to_string(&reply.target).unwrap_or_else(|_| "SessionRef unavailable".to_owned())
}

fn report(
    result: Result<collaboration_client::protocol::PushMessageSendResult, MessageSendError>,
    machine: bool,
) -> i32 {
    if let Err(MessageSendError::Preparation(error)) = &result
        && let Some(code) = crate::permission_diagnostic_reporting::report_permission_error(
            error,
            crate::permission_diagnostic_reporting::PermissionDiagnosticRendering::Command,
            machine,
        )
    {
        return code;
    }
    let (record, code, confirmation, failure_line, write_failure_code) = match result {
        Ok(push) => {
            let exit = receipt_exit_status(push.delivery_state);
            let failure_line = receipt_failure_line(
                push.delivery_state,
                &push.target_identity,
                &push.link,
                &push.receipt.outcome,
            );
            let confirmation = failure_line
                .is_none()
                .then(|| send_confirmation_line(&push));
            let record = crate::endpoint_commands::result_envelope(json!(push));
            (record, exit, confirmation, failure_line, 5)
        }
        Err(error) => {
            let (failure, target) = error.into_operation_failure_and_target();
            let exit = operation_failure_exit(&failure);
            let human_failure = operation_failure_line(&failure, target.as_ref());
            let write_failure = if failure.effect == OperationEffect::Unknown {
                5
            } else {
                3
            };
            (
                json!({"kind":"error","target":target,"error":failure}),
                exit,
                None,
                Some(human_failure),
                write_failure,
            )
        }
    };
    let written = if machine {
        writeln!(io::stdout(), "{record}")
    } else if let Some(failure_line) = failure_line {
        writeln!(io::stderr(), "{failure_line}")
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
    if written.is_err() {
        write_failure_code
    } else {
        code
    }
}

fn send_confirmation_line(push: &collaboration_client::protocol::PushMessageSendResult) -> String {
    format!(
        "push {} to {} {}: {}; {}",
        push.push_id.as_str(),
        push.target_identity,
        serde_json::to_string(&push.target).unwrap_or_else(|_| "SessionRef unavailable".to_owned()),
        delivery_outcome_label(&push.receipt.outcome),
        push.link
    )
}

fn delivery_outcome_label(outcome: &DeliveryOutcome) -> String {
    match outcome {
        DeliveryOutcome::Started => "started".to_owned(),
        DeliveryOutcome::Steered => "steered".to_owned(),
        DeliveryOutcome::StartedOrSteered => "started or steered".to_owned(),
        DeliveryOutcome::Queued => "queued".to_owned(),
        DeliveryOutcome::PeerMessageWritten => "peer message written".to_owned(),
        DeliveryOutcome::NotSubmitted { retryable, reason } => format!(
            "not submitted{}: {reason}",
            if *retryable { " (retryable)" } else { "" }
        ),
        DeliveryOutcome::Rejected(_) => "rejected".to_owned(),
        DeliveryOutcome::Unknown => "outcome unknown".to_owned(),
    }
}

fn receipt_exit_status(delivery_state: PushDeliveryState) -> i32 {
    match delivery_state {
        PushDeliveryState::Delivered | PushDeliveryState::Held => 0,
        PushDeliveryState::Rejected => 4,
        PushDeliveryState::OutcomeUnknown
        | PushDeliveryState::Pending
        | PushDeliveryState::Attempted => 5,
    }
}

fn receipt_failure_line(
    delivery_state: PushDeliveryState,
    target: &str,
    push_link: &str,
    outcome: &DeliveryOutcome,
) -> Option<String> {
    match delivery_state {
        PushDeliveryState::Held => Some(render_held_line(push_link, target)),
        PushDeliveryState::Rejected => match outcome {
            DeliveryOutcome::Rejected(rejection) => {
                Some(render_rejection_line(rejection, target, push_link))
            }
            DeliveryOutcome::NotSubmitted { reason, .. } => Some(render_failure_line(
                &format!("Delivery to {target} was rejected: {reason}"),
                "correct the message or target and try again",
            )),
            _ => Some(render_failure_line(
                &format!("Delivery to {target} was rejected"),
                "run agent-collaboration show <link> to inspect the stored outcome",
            )),
        },
        PushDeliveryState::OutcomeUnknown
        | PushDeliveryState::Pending
        | PushDeliveryState::Attempted => Some(render_unknown_line(target, push_link)),
        PushDeliveryState::Delivered => None,
    }
}

fn report_message_failure(kind: &str, message: &str, exit_code: i32, machine: bool) -> i32 {
    if machine {
        return crate::endpoint_commands::report_failure(kind, message, exit_code, true);
    }
    let next_step = match kind {
        "invalidField" | "invalidUsage" => "correct the named argument and try again",
        "currentSessionUnavailable" | "identityUnavailable" => {
            "run agent-collaboration whoami --json"
        }
        "unavailable" => "check Router availability and retry",
        _ => "run agent-collaboration message --help",
    };
    let line = render_failure_line(message, next_step);
    if writeln!(io::stderr(), "{line}").is_err() {
        5
    } else {
        exit_code
    }
}

fn operation_failure_line(failure: &OperationFailure, target: Option<&SessionRef>) -> String {
    let explanation = target.map_or_else(
        || failure.message.clone(),
        |target| {
            format!(
                "Delivery to {}: {}",
                String::from(target.session_id.clone()),
                failure.message
            )
        },
    );
    let next_step = if failure.effect == OperationEffect::Unknown
        || failure.service_kind.as_deref() == Some("outcomeUnknown")
    {
        "inspect the target before retrying"
    } else if failure.service_kind.as_deref() == Some("notFound") {
        "check whether the Router link is expired or belongs to another machine"
    } else if failure.service_kind.as_deref() == Some("notPermitted") {
        "run show as the push sender or target session"
    } else if failure.service_kind.as_deref() == Some("invalidField") {
        "use a UUIDv7 push id or a router:// machine/push/id link"
    } else {
        match failure.kind {
            OperationFailureKind::UnsupportedCapability => {
                "run agent-collaboration message send --help for supported delivery modes"
            }
            OperationFailureKind::Unavailable | OperationFailureKind::Timeout => {
                "check Router availability, then retry"
            }
            OperationFailureKind::ProtocolViolation | OperationFailureKind::Rejected => {
                "correct the message request and try again"
            }
        }
    };
    render_failure_line(&explanation, next_step)
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
    use super::{
        HistoryArguments, InboxArguments, MessageArguments, MessageCommand, ReplyArguments,
        reply_confirmation_line,
    };
    use clap::Parser;

    #[test]
    fn reply_requires_a_reference_and_keeps_text_file_input() {
        const PUSH_ID: &str = "019f0000-0000-7000-8000-000000000101";

        let text_request = MessageArguments::try_parse_from([
            "agent-collaboration message",
            "reply",
            PUSH_ID,
            "hello",
        ])
        .expect("positional reply arguments parse");
        assert!(matches!(
            text_request.command,
            MessageCommand::Reply(ReplyArguments {
                reference,
                text: Some(ref text),
                text_file: None,
                ..
            }) if reference == PUSH_ID && text == "hello"
        ));

        let file_request = MessageArguments::try_parse_from([
            "agent-collaboration message",
            "reply",
            PUSH_ID,
            "--text-file",
            "reply.md",
        ])
        .expect("file reply arguments parse");
        assert!(matches!(
            file_request.command,
            MessageCommand::Reply(ReplyArguments {
                reference,
                text: None,
                text_file: Some(_),
                ..
            }) if reference == PUSH_ID
        ));

        assert!(
            MessageArguments::try_parse_from([
                "agent-collaboration message",
                "reply",
                "--text",
                "hello",
            ])
            .is_err()
        );

        assert!(
            MessageArguments::try_parse_from([
                "agent-collaboration message",
                "reply",
                PUSH_ID,
                "hello",
                "--expect-sender",
                "{\"endpoint\":{},\"sessionId\":\"sender\"}",
            ])
            .is_err()
        );
    }

    #[test]
    fn inbox_and_history_are_message_subcommands() {
        assert!(matches!(
            MessageArguments::try_parse_from(["agent-collaboration message", "inbox"])
                .expect("inbox arguments parse")
                .command,
            MessageCommand::Inbox(InboxArguments { .. })
        ));
        assert!(matches!(
            MessageArguments::try_parse_from([
                "agent-collaboration message",
                "history",
                "--with",
                "{\"endpoint\":{},\"sessionId\":\"other\"}",
            ])
            .expect("history arguments parse")
            .command,
            MessageCommand::History(HistoryArguments { .. })
        ));
    }

    #[test]
    fn reply_confirmation_names_the_recipient_push_and_delivery_outcome() {
        const PUSH_ID: &str = "019f0000-0000-7000-8000-000000000101";
        const PUSH_LINK: &str = "router://00000000-0000-4000-8000-000000000001/push/019f0000-0000-7000-8000-000000000101";
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
            push_id: PUSH_ID.to_owned().try_into().expect("push id"),
            link: PUSH_LINK.to_owned(),
            delivery_state: collaboration_client::protocol::PushDeliveryState::Delivered,
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
                "replied to ✳️ Claude Main {} (push {PUSH_ID} {PUSH_LINK}): peer message written",
                serde_json::to_string(&target).expect("target JSON"),
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
        assert_eq!(
            receipt_exit_status(collaboration_client::protocol::PushDeliveryState::Rejected),
            4
        );
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
