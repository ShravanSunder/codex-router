//! Explicit listing and single-use decisions for client-exposed native approvals.
use clap::{Args, Parser, Subcommand};
use collaboration_client::protocol::{ApprovalDecideParams, ApprovalDecision};
use collaboration_client::{
    ClientError, CollaborationClient, OperationEffect, OperationFailure, OperationFailureKind,
    operation_failure_from_client_error,
};
use message_board::Identity;
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
        #[arg(long)]
        include_options: bool,
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
        #[arg(long, group = "decision")]
        option_id: Option<String>,
        /// Optional note sent to the provider with the selected option.
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        acknowledge_persistent: bool,
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

fn operation_failure_exit(failure: &OperationFailure) -> i32 {
    match failure.kind {
        OperationFailureKind::Rejected => 4,
        _ if failure.effect == OperationEffect::Unknown => 5,
        OperationFailureKind::Timeout => 124,
        OperationFailureKind::UnsupportedCapability | OperationFailureKind::ProtocolViolation => 2,
        _ => 3,
    }
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
    let (output, request, pending_only, include_options) = match parsed.command {
        ApprovalCommand::List {
            pending,
            include_options,
            output,
        } => (output, None, pending, include_options),
        ApprovalCommand::Decide {
            request_id,
            allow,
            allow_for_session,
            deny,
            option_id,
            note,
            acknowledge_persistent,
            actor,
            output,
        } => {
            let actor = match parse_actor(&actor) {
                Ok(actor) => actor,
                Err(_) => {
                    return crate::endpoint_commands::report_failure(
                        "invalidField",
                        "--actor must be typed Identity or exact SessionRef JSON",
                        2,
                        output.json,
                    );
                }
            };
            if usize::from(allow)
                + usize::from(allow_for_session)
                + usize::from(deny)
                + usize::from(option_id.is_some())
                != 1
            {
                return crate::endpoint_commands::report_failure(
                    "invalidField",
                    "Choose exactly one of --option-id, --allow, --allow-for-session, or --deny",
                    2,
                    output.json,
                );
            }
            let decision = if allow {
                Some(ApprovalDecision::Allow)
            } else if allow_for_session {
                Some(ApprovalDecision::AllowForSession)
            } else if deny {
                Some(ApprovalDecision::Deny)
            } else {
                None
            };
            (
                output,
                Some(ApprovalDecideParams {
                    request_id,
                    decision,
                    option_id,
                    note,
                    acknowledge_persistent,
                    actor,
                }),
                false,
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
        let client = CollaborationClient::connect(
            &directory,
            "agent-collaboration",
            env!("CARGO_PKG_VERSION"),
        )
        .await
        .map_err(ApprovalCommandError::Client)?;
        let value = match request {
            Some(request) => serde_json::to_value(
                client
                    .decide_approval(request)
                    .await
                    .map_err(|error| ApprovalCommandError::Operation(error.into_parts().0))?,
            ),
            None if include_options => serde_json::to_value(
                client
                    .list_approvals_with_options(pending_only)
                    .await
                    .map_err(ApprovalCommandError::Client)?,
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
            let exit = operation_failure_exit(&failure);
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
            crate::permission_diagnostic_reporting::report_actionable_client_error(
                &error,
                crate::permission_diagnostic_reporting::PermissionDiagnosticRendering::Command,
                output.json,
            )
            .unwrap_or_else(|| {
                let failure = operation_failure_from_client_error(error, OperationEffect::None);
                let exit = operation_failure_exit(&failure);
                let message = failure.message.clone();
                let record = serde_json::json!({"kind":"error","error":failure});
                let _printed = if output.json {
                    writeln!(io::stdout(), "{record}")
                } else {
                    writeln!(io::stderr(), "{message}")
                };
                exit
            })
        }
    }
}

pub(crate) fn parse_actor(value: &str) -> Result<Identity, ()> {
    let parsed: serde_json::Value = serde_json::from_str(value).map_err(|_| ())?;
    if parsed.get("kind").is_some() {
        return serde_json::from_value(parsed).map_err(|_| ());
    }
    let session = serde_json::from_value(parsed).map_err(|_| ())?;
    Ok(Identity::Session { session })
}

#[cfg(test)]
mod approval_list_argument_tests {
    use super::*;

    #[test]
    fn options_are_absent_by_default_and_opt_in_explicitly() {
        let default =
            ApprovalArguments::try_parse_from(["approval", "list"]).expect("default list");
        let ApprovalCommand::List {
            include_options, ..
        } = default.command
        else {
            panic!("list")
        };
        assert!(!include_options);
        let detailed = ApprovalArguments::try_parse_from(["approval", "list", "--include-options"])
            .expect("detailed list");
        let ApprovalCommand::List {
            include_options, ..
        } = detailed.command
        else {
            panic!("list")
        };
        assert!(include_options);
    }
}
