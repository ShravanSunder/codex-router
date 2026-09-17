//! Human and agent entrypoints for public native inspection and exact interruption.
use clap::{Args, Parser, Subcommand};
use collaboration_client::protocol::{ChannelDescription, EndpointAvailability};
use collaboration_client::{ClientError, ControlClient};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
};

#[derive(Parser)]
#[command(name = "agent-collaboration")]
struct NativeControlArguments {
    #[command(subcommand)]
    command: NativeControlCommand,
}
#[derive(Subcommand)]
enum NativeControlCommand {
    Session {
        #[command(subcommand)]
        command: SessionOperation,
    },
    Turn {
        #[command(subcommand)]
        command: TurnOperation,
    },
}
#[derive(Subcommand)]
enum SessionOperation {
    /// Read native metadata without loading or resuming the thread.
    Inspect(TargetArguments),
    /// Set the explicit persisted Codex thread name without resuming it.
    Rename {
        #[command(flatten)]
        target: TargetArguments,
        #[arg(long)]
        name: String,
    },
}
#[derive(Subcommand)]
enum TurnOperation {
    /// Interrupt exactly this turn; never stop the app-server or delete its thread.
    Interrupt {
        #[command(flatten)]
        target: TargetArguments,
        #[arg(long)]
        turn: String,
    },
}
#[derive(Args)]
struct TargetArguments {
    #[command(flatten)]
    target: crate::session_target_arguments::SessionTargetArguments,
    #[arg(long)]
    service_directory: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}
pub fn run_native_session_command(arguments: Vec<OsString>) -> i32 {
    let arguments = std::iter::once(OsString::from("agent-collaboration"))
        .chain(arguments)
        .collect();
    let parsed = match crate::automation_argument_feedback::parse_arguments::<NativeControlArguments>(
        arguments,
    ) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    enum RequestedOperation {
        Inspect,
        Rename(String),
        Interrupt(String),
    }
    let (target, operation) = match parsed.command {
        NativeControlCommand::Session {
            command: SessionOperation::Inspect(target),
        } => (target, RequestedOperation::Inspect),
        NativeControlCommand::Session {
            command: SessionOperation::Rename { target, name },
        } => (target, RequestedOperation::Rename(name)),
        NativeControlCommand::Turn {
            command: TurnOperation::Interrupt { target, turn },
        } => (target, RequestedOperation::Interrupt(turn)),
    };
    let machine_output = target.json;
    let resolved = (|| {
        let directory = crate::endpoint_commands::resolve_directory(target.service_directory)?;
        let target = target.target.parse()?;
        if let RequestedOperation::Interrupt(id) = &operation
            && collaboration_client::protocol::NonEmptyText::try_from(id.clone()).is_err()
        {
            return Err("A nonempty exact turn ID is required".to_owned());
        }
        if let RequestedOperation::Rename(name) = &operation
            && (name.trim() != name
                || !(1..=120).contains(&name.chars().count())
                || name.chars().any(char::is_control))
        {
            return Err("--name requires 1 to 120 Unicode scalar values without surrounding whitespace or control characters".to_owned());
        }
        Ok::<_, String>((directory, target))
    })();
    let (directory, parsed_target) = match resolved {
        Ok(resolved) => resolved,
        Err(message) => {
            return crate::endpoint_commands::report_failure(
                "invalidField",
                &message,
                2,
                machine_output,
            );
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
                machine_output,
            );
        }
    };
    // The envelope kind is the command's own, not a guess from the result's fields.
    let operation_kind = match &operation {
        RequestedOperation::Inspect => "inspect",
        RequestedOperation::Rename(_) => "rename",
        RequestedOperation::Interrupt(_) => "interrupt",
    };
    let mut mutation_started = false;
    let result = runtime.block_on(async {
        let mut client =
            ControlClient::connect(&directory, "agent-collaboration", env!("CARGO_PKG_VERSION"))
                .await?;
        let target = parsed_target
            .resolve(&client.identity().service_id)
            .map_err(|_| ClientError::Protocol("invalid session target"))?;
        let result = match operation {
            RequestedOperation::Inspect => json!(client.inspect_session(&target).await?),
            RequestedOperation::Rename(name) => {
                mutation_started = true;
                json!(
                    client
                        .rename_session(collaboration_client::protocol::NativeRenameParams {
                            target,
                            name
                        })
                        .await?
                )
            }
            RequestedOperation::Interrupt(turn) => {
                let inventory = client.list_endpoints().await?;
                let generation = inventory
                    .endpoints
                    .iter()
                    .find(|entry| {
                        entry.endpoint == target.endpoint
                            && matches!(entry.availability, EndpointAvailability::Available { .. })
                    })
                    .and_then(|entry| {
                        entry.channels.iter().find_map(|channel| match channel {
                            ChannelDescription::NativeCodex { generation, .. } => {
                                generation.clone()
                            }
                            _ => None,
                        })
                    })
                    .ok_or(ClientError::Rejected {
                        code: -32050,
                        data: Some(json!({"kind":"unavailable"})),
                    })?;
                mutation_started = true;
                json!(client.interrupt_turn(&target, &generation, &turn).await?)
            }
        };
        let _closed = client.close().await;
        Ok::<_, ClientError>(result)
    });
    match result {
        Ok(result) => {
            let result = if machine_output {
                match operation_kind {
                    "rename" => crate::endpoint_commands::mutation_envelope(
                        json!(result),
                        json!({"previousName": result.get("previousName")}),
                    ),
                    "interrupt" => crate::endpoint_commands::mutation_envelope(
                        json!(result),
                        json!({"kind": result.get("kind")}),
                    ),
                    _ => crate::endpoint_commands::result_envelope(json!(result)),
                }
                .to_string()
            } else {
                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| "Result encoding failed".into())
            };
            if writeln!(io::stdout(), "{result}").is_ok() {
                0
            } else {
                3
            }
        }
        Err(ClientError::Rejected { data, .. }) => {
            let kind = data
                .as_ref()
                .and_then(|data| data.get("kind"))
                .and_then(Value::as_str);
            let code = match kind {
                Some("outcomeUnknown") => 5,
                Some("unavailable") => 3,
                Some("unsupportedCapability") => 2,
                _ => 4,
            };
            crate::endpoint_commands::report_failure(
                kind.unwrap_or("rejected"),
                "Native control operation rejected",
                code,
                machine_output,
            )
        }
        Err(_) if mutation_started => crate::endpoint_commands::report_failure(
            "outcomeUnknown",
            "Interruption outcome unknown; no request was replayed",
            5,
            machine_output,
        ),
        Err(error) => crate::permission_diagnostic_reporting::report_permission_error(
            &error,
            crate::permission_diagnostic_reporting::PermissionDiagnosticRendering::Command,
            machine_output,
        )
        .unwrap_or_else(|| {
            crate::endpoint_commands::report_failure(
                "unavailable",
                "Native control connection unavailable",
                3,
                machine_output,
            )
        }),
    }
}
