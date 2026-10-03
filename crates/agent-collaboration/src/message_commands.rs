//! CLI commands for direct messages and stored Router push records.
use crate::failure_line::render_failure_line;
use crate::message_input_arguments::MessageSendArguments;
use clap::{Parser, Subcommand};
use collaboration_client::protocol::{PushId, RouterLink, SessionRef};
use collaboration_client::{OperationEffect, OperationFailure, OperationFailureKind};
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
};

mod message_send_reply;
mod push_record_queries;

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

pub fn run_message_command(arguments: Vec<OsString>) -> i32 {
    let parsed =
        match crate::automation_argument_feedback::parse_arguments::<MessageArguments>(arguments) {
            Ok(value) => value,
            Err(code) => return code,
        };
    match parsed.command {
        MessageCommand::Send(args) => message_send_reply::run_message_send(args),
        MessageCommand::Inbox(args) => push_record_queries::run_message_inbox(args),
        MessageCommand::History(args) => push_record_queries::run_message_history(args),
        MessageCommand::Reply(args) => message_send_reply::run_message_reply(args),
    }
}

pub fn run_push_record_show_command(arguments: Vec<OsString>) -> i32 {
    let args = match crate::automation_argument_feedback::parse_arguments::<PushRecordShowArguments>(
        arguments,
    ) {
        Ok(value) => value,
        Err(code) => return code,
    };
    push_record_queries::run_push_record_show(args)
}

fn validate_push_reference(reference: &str) -> Result<(), String> {
    if PushId::try_from(reference.to_owned()).is_ok() || RouterLink::parse(reference).is_ok() {
        return Ok(());
    }
    Err("PUSH_ID_OR_LINK must be a UUIDv7 push id or router://<machine-id>/push/<push-id>; for example 019f0000-0000-7000-8000-000000000101 or router://00000000-0000-4000-8000-000000000001/push/019f0000-0000-7000-8000-000000000101".to_owned())
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
