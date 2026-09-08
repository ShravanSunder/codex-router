//! Agent-friendly wake creation and inspection through the public Rust SDK.
use crate::message_input_arguments::{SendArguments, saved_message};
use crate::wakeup_timing_arguments::WakeTimingArguments;
use clap::{Parser, Subcommand};
use communication_client::{ControlClient, WakeClientError};
use communication_protocol::{
    LocalMutationEvidence, LocalMutationState, OperationId, WakeSendRequest, WakeShowRequest,
    WakeSnapshot,
};
use serde_json::json;
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
};
#[derive(Parser)]
#[command(name = "agent-sessions wake", bin_name = "agent-sessions wake")]
struct WakeArguments {
    #[command(subcommand)]
    command: WakeCommand,
}
#[derive(Subcommand)]
enum WakeCommand {
    /// Save a timed message. Returns after durable creation; this is not native acceptance.
    Send {
        #[command(flatten)]
        message: Box<SendArguments>,
        #[command(flatten)]
        timing: WakeTimingArguments,
        /// Reuse this UUIDv7 to recover a lost creation response safely.
        #[arg(long)]
        operation_id: Option<String>,
    },
    /// Inspect timing and first firing without prompting a native thread.
    Show {
        #[arg(long)]
        wakeup_id: String,
        #[arg(long)]
        service_directory: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
}
enum PreparedWake {
    Send(Box<WakeSendRequest>),
    Show(WakeShowRequest),
}
struct WakeInvocation {
    directory: PathBuf,
    json: bool,
    operation_id: Option<OperationId>,
    request: PreparedWake,
}
pub fn run_wakeup_command(arguments: Vec<OsString>) -> i32 {
    let args = match WakeArguments::try_parse_from(arguments) {
        Ok(args) => args,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _ = error.print();
            return code;
        }
    };
    let machine = match &args.command {
        WakeCommand::Send { message, .. } => message.json,
        WakeCommand::Show { json, .. } => *json,
    };
    let invocation = match prepare(args.command) {
        Ok(value) => value,
        Err(message) => {
            return crate::endpoint_commands::report_failure("invalidField", &message, 2, machine);
        }
    };
    if !invocation.json
        && let Some(id) = &invocation.operation_id
    {
        let _ = writeln!(io::stderr(), "Operation ID: {}", id.as_str());
    }
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
    let mut dispatched = false;
    let result: Result<WakeSnapshot, WakeClientError> = runtime.block_on(async {
        let mut client = ControlClient::connect(
            &invocation.directory,
            "agent-sessions-wake",
            env!("CARGO_PKG_VERSION"),
        )
        .await?;
        dispatched = true;
        let result = match invocation.request {
            PreparedWake::Send(request) => client.send_wakeup(*request).await,
            PreparedWake::Show(request) => client.read_wakeup(request).await,
        };
        let _ = client.close().await;
        result
    });
    let (record, code) = match result {
        Ok(snapshot) => (
            json!({"kind":"result","operationId":invocation.operation_id,"result":snapshot}),
            0,
        ),
        Err(WakeClientError::Rejected(error)) => {
            let uncertain = matches!(
                &error.effects,
                LocalMutationEvidence::Local {
                    mutation: LocalMutationState::Unknown | LocalMutationState::Committed
                }
            );
            (
                json!({"kind":"error","operationId":invocation.operation_id,"error":error}),
                if uncertain { 5 } else { 4 },
            )
        }
        Err(WakeClientError::Connection(_)) => {
            let uncertain = dispatched && invocation.operation_id.is_some();
            (
                json!({"kind":"error","operationId":invocation.operation_id,"error":{"kind":if uncertain{"outcomeUnknown"}else{"unavailable"},"message":if uncertain{"Wake creation may have committed. Inspect or replay the same operation ID; do not blindly create another wake."}else{"Wake service unavailable; no mutation dispatched."},"nextAction":if uncertain{"inspectOperation"}else{"retryLater"}}}),
                if uncertain { 5 } else { 3 },
            )
        }
    };
    let text = if machine {
        serde_json::to_string(&record)
    } else {
        serde_json::to_string_pretty(&record)
    };
    match text {
        Ok(text) if writeln!(io::stdout(), "{text}").is_ok() => code,
        _ => 5,
    }
}
fn prepare(command: WakeCommand) -> Result<WakeInvocation, String> {
    match command {
        WakeCommand::Send {
            message,
            timing,
            operation_id,
        } => {
            let (directory, saved) = saved_message(&message)?;
            let (timing, expiry) = timing.prepare()?;
            let id = operation_id.map_or_else(
                || Ok(OperationId::generate()),
                |id| {
                    id.try_into()
                        .map_err(|_| "--operation-id requires a canonical UUIDv7")
                },
            )?;
            Ok(WakeInvocation {
                directory,
                json: message.json,
                operation_id: Some(id.clone()),
                request: PreparedWake::Send(Box::new(WakeSendRequest {
                    operation_id: id,
                    message: saved,
                    timing,
                    expiry,
                })),
            })
        }
        WakeCommand::Show {
            wakeup_id,
            service_directory,
            json,
        } => Ok(WakeInvocation {
            directory: crate::endpoint_commands::resolve_directory(service_directory)?,
            json,
            operation_id: None,
            request: PreparedWake::Show(WakeShowRequest {
                wakeup_id: wakeup_id
                    .try_into()
                    .map_err(|_| "--wakeup-id requires a canonical UUIDv7")?,
            }),
        }),
    }
}
