//! Explicit listing and single-use decisions for client-exposed native approvals.
use clap::{Args, Parser, Subcommand};
use collaboration_client::protocol::{ApprovalDecideParams, ApprovalDecision, SessionRef};
use collaboration_client::{ClientError, ControlClient};
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
                .await?;
        let value = match request {
            Some(request) => serde_json::to_value(client.decide_approval(request).await?),
            None => serde_json::to_value(client.list_pending_approvals(pending_only).await?),
        }
        .map_err(|_| ClientError::Protocol("approval output encoding failed"))?;
        let _ = client.close().await;
        Ok::<_, ClientError>(value)
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
        Err(ClientError::Rejected { data, .. }) => crate::endpoint_commands::report_failure(
            data.as_ref()
                .and_then(|v| v.get("kind"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("rejected"),
            "Approval operation rejected",
            4,
            output.json,
        ),
        Err(error) => crate::permission_diagnostic_reporting::report_permission_error(
            &error,
            crate::permission_diagnostic_reporting::PermissionDiagnosticRendering::Command,
            output.json,
        )
        .unwrap_or_else(|| {
            crate::endpoint_commands::report_failure(
                "unavailable",
                "Approval service unavailable",
                3,
                output.json,
            )
        }),
    }
}

fn parse_actor(value: &str) -> Result<SessionRef, ()> {
    let parsed: serde_json::Value = serde_json::from_str(value).map_err(|_| ())?;
    if parsed.get("kind").and_then(serde_json::Value::as_str) == Some("session") {
        return serde_json::from_value(parsed.get("session").cloned().ok_or(())?).map_err(|_| ());
    }
    serde_json::from_value(parsed).map_err(|_| ())
}
