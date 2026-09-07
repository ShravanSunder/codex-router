//! Human and agent entrypoints for public native inspection and exact interruption.
use clap::{Args, Parser, Subcommand};
use communication_client::{ClientError, ControlClient};
use communication_protocol::{
    ChannelDescription, EndpointAvailability, EndpointId, EndpointRef, SessionId, SessionRef,
};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
};

#[derive(Parser)]
#[command(name = "agent-sessions")]
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
    #[arg(long)]
    endpoint: String,
    #[arg(long)]
    session: String,
    #[arg(long)]
    service_directory: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}
pub fn run_native_session_command(arguments: Vec<OsString>) -> i32 {
    let parsed = NativeControlArguments::try_parse_from(
        std::iter::once(OsString::from("agent-sessions")).chain(arguments),
    );
    let parsed = match parsed {
        Ok(parsed) => parsed,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _printed = error.print();
            return code;
        }
    };
    let (target, turn) = match parsed.command {
        NativeControlCommand::Session {
            command: SessionOperation::Inspect(target),
        } => (target, None),
        NativeControlCommand::Turn {
            command: TurnOperation::Interrupt { target, turn },
        } => (target, Some(turn)),
    };
    let machine_output = target.json;
    let resolved = (|| {
        let directory = crate::endpoint_commands::resolve_directory(target.service_directory)?;
        let endpoint =
            EndpointId::try_from(target.endpoint).map_err(|_| "Invalid endpoint identifier")?;
        let session =
            SessionId::try_from(target.session).map_err(|_| "Invalid session identifier")?;
        if turn
            .as_ref()
            .is_some_and(|id| communication_protocol::NonEmptyText::try_from(id.clone()).is_err())
        {
            return Err("A nonempty exact turn ID is required".to_owned());
        }
        Ok::<_, String>((directory, endpoint, session))
    })();
    let (directory, endpoint, session_id) = match resolved {
        Ok(resolved) => resolved,
        Err(message) => {
            return crate::endpoint_commands::report_failure(
                "invalidUsage",
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
    let mut mutation_started = false;
    let result = runtime.block_on(async {
        let mut client =
            ControlClient::connect(&directory, "agent-sessions", env!("CARGO_PKG_VERSION")).await?;
        let target = SessionRef {
            endpoint: EndpointRef {
                service_id: client.identity().service_id.clone(),
                endpoint_id: endpoint,
            },
            session_id,
        };
        let result = match turn {
            None => json!(client.inspect_session(&target).await?),
            Some(turn) => {
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
                json!({"kind":"result","result":result}).to_string()
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
        Err(_) => crate::endpoint_commands::report_failure(
            "unavailable",
            "Native control connection unavailable",
            3,
            machine_output,
        ),
    }
}
