//! Explicit listing and single-use decisions for client-exposed native approvals.
use clap::{Args, Parser, Subcommand};
use collaboration_client::protocol::{ApprovalDecideParams, ApprovalDecision, SessionRef};
use collaboration_client::{
    ClientError, ControlClient, OperationEffect, OperationFailure, OperationFailureKind,
    operation_failure_from_client_error,
};
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
};

#[derive(Parser)]
#[command(name = "agent-collaboration approval")]
struct ApprovalArguments {
    #[command(subcommand)]
    command: ApprovalCommand,
}

#[derive(Subcommand)]
enum ApprovalCommand {
    /// List approval history, optionally restricted to pending requests.
    List {
        #[arg(long)]
        pending: bool,
        #[command(flatten)]
        output: OutputArguments,
    },
    /// Resolve one pending request as its configured approver.
    Decide {
        #[arg(long)]
        request_id: String,
        #[arg(long, group = "decision")]
        allow: bool,
        #[arg(long, group = "decision")]
        allow_for_session: bool,
        #[arg(long, group = "decision")]
        deny: bool,
        #[arg(long)]
        actor: String,
        #[command(flatten)]
        output: OutputArguments,
    },
}

enum ApprovalCommandError {
    Client(ClientError),
    Operation(OperationFailure),
}

#[derive(Args)]
struct OutputArguments {
    #[arg(long)]
    service_directory: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

pub fn run_approval_command(arguments: Vec<OsString>) -> i32 {
    let parsed = match crate::automation_argument_feedback::parse_arguments::<ApprovalArguments>(
        arguments,
    ) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    let (output, request, pending_only) = match parsed.command {
        ApprovalCommand::List { pending, output } => (output, None, pending),
        ApprovalCommand::Decide {
            request_id,
            allow,
            allow_for_session,
            deny,
            actor,
            output,
        } => {
            let actor = match parse_actor(&actor) {
                Ok(actor) => actor,
                Err(_) => {
                    return crate::endpoint_commands::report_failure(
                        "invalidField",
                        "--actor must be exact SessionRef JSON",
                        2,
                        output.json,
                    );
                }
            };
            if usize::from(allow) + usize::from(allow_for_session) + usize::from(deny) != 1 {
                return crate::endpoint_commands::report_failure(
                    "invalidField",
                    "Choose exactly one of --allow, --allow-for-session, or --deny",
                    2,
                    output.json,
                );
            }
            let decision = if allow {
                ApprovalDecision::Allow
            } else if allow_for_session {
                ApprovalDecision::AllowForSession
            } else {
                ApprovalDecision::Deny
            };
            (
                output,
                Some(ApprovalDecideParams {
                    request_id,
                    decision,
                    actor,
                }),
                false,
            )
        }
    };
    let directory = match crate::endpoint_commands::resolve_directory(output.service_directory) {
        Ok(directory) => directory,
        Err(message) => {
            return crate::endpoint_commands::report_failure(
                "invalidField",
                &message,
                2,
                output.json,
            );
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return 3,
    };
    let result = runtime.block_on(async {
        let mut client =
            ControlClient::connect(&directory, "agent-collaboration", env!("CARGO_PKG_VERSION"))
                .await
                .map_err(ApprovalCommandError::Client)?;
        let value = match request {
            Some(request) => serde_json::to_value(
                client
                    .decide_approval(request)
                    .await
                    .map_err(|error| ApprovalCommandError::Operation(error.into_parts().0))?,
            ),
            None => serde_json::to_value(
                client
                    .list_pending_approvals(pending_only)
                    .await
                    .map_err(ApprovalCommandError::Client)?,
            ),
        }
        .map_err(|_| {
            ApprovalCommandError::Client(ClientError::Protocol("approval output encoding failed"))
        })?;
        let _ = client.close().await;
        Ok::<_, ApprovalCommandError>(value)
    });
    match result {
        Ok(value) => {
            let rendered = if output.json {
                crate::endpoint_commands::result_envelope(value).to_string()
            } else {
                serde_json::to_string_pretty(&value).unwrap_or_default()
            };
            if writeln!(io::stdout(), "{rendered}").is_ok() {
                0
            } else {
                3
            }
        }
        Err(ApprovalCommandError::Operation(failure)) => {
            let exit = match failure.kind {
                OperationFailureKind::Rejected => 4,
                _ if failure.effect == OperationEffect::Unknown => 5,
                OperationFailureKind::Timeout => 124,
                OperationFailureKind::UnsupportedCapability
                | OperationFailureKind::ProtocolViolation => 2,
                _ => 3,
            };
            let message = failure.message.clone();
            let record = serde_json::json!({"kind":"error","error":failure});
            let _printed = if output.json {
                writeln!(io::stdout(), "{record}")
            } else {
                writeln!(io::stderr(), "{message}")
            };
            exit
        }
        Err(ApprovalCommandError::Client(error)) => {
            crate::permission_diagnostic_reporting::report_permission_error(
                &error,
                crate::permission_diagnostic_reporting::PermissionDiagnosticRendering::Command,
                output.json,
            )
            .unwrap_or_else(|| {
                let failure = operation_failure_from_client_error(error, OperationEffect::None);
                let message = failure.message.clone();
                let record = serde_json::json!({"kind":"error","error":failure});
                let _printed = if output.json {
                    writeln!(io::stdout(), "{record}")
                } else {
                    writeln!(io::stderr(), "{message}")
                };
                3
            })
        }
    }
}

fn parse_actor(value: &str) -> Result<SessionRef, ()> {
    let parsed: serde_json::Value = serde_json::from_str(value).map_err(|_| ())?;
    if parsed.get("kind").and_then(serde_json::Value::as_str) == Some("session") {
        return serde_json::from_value(parsed.get("session").cloned().ok_or(())?).map_err(|_| ());
    }
    serde_json::from_value(parsed).map_err(|_| ())
}
