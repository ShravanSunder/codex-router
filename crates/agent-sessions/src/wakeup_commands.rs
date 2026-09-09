//! Agent-friendly wake creation and inspection through the public Rust SDK.
use crate::message_input_arguments::{SendArguments, saved_message};
use crate::wakeup_timing_arguments::WakeTimingArguments;
use clap::{Args, Parser, Subcommand};
use communication_client::{ControlClient, WakeClientError};
use communication_protocol::{
    LocalMutationEvidence, LocalMutationState, OperationId, WakeMutationRequest, WakeSendRequest,
    WakeShowRequest,
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
        /// Block until the first firing is recorded; native acceptance remains separate.
        #[arg(long)]
        wait_until_first_fire: bool,
    },
    /// Stop future firings and discard undispatched reminders; does not recall native input.
    Pause(LifecycleArguments),
    /// Continue original timing without replaying paused ticks or extending expiry.
    Resume(LifecycleArguments),
    /// Permanently cancel future firings and discard undispatched reminders.
    Cancel(LifecycleArguments),
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
#[derive(Args)]
struct LifecycleArguments {
    #[arg(long)]
    wakeup_id: String,
    #[arg(long)]
    operation_id: Option<String>,
    #[arg(long)]
    service_directory: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}
#[derive(Clone, Copy)]
enum LifecycleAction {
    Pause,
    Resume,
    Cancel,
}
enum PreparedWake {
    Send(Box<WakeSendRequest>),
    Show(WakeShowRequest),
    Mutate(LifecycleAction, WakeMutationRequest),
}
struct WakeInvocation {
    directory: PathBuf,
    json: bool,
    operation_id: Option<OperationId>,
    request: PreparedWake,
    wait_until_first_fire: bool,
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
        WakeCommand::Pause(args) | WakeCommand::Resume(args) | WakeCommand::Cancel(args) => {
            args.json
        }
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
    let result: Result<serde_json::Value, WakeClientError> = runtime.block_on(async {
        let mut client = ControlClient::connect(
            &invocation.directory,
            "agent-sessions-wake",
            env!("CARGO_PKG_VERSION"),
        )
        .await?;
        dispatched = true;
        let result = match invocation.request {
            PreparedWake::Send(request) => {
                client.send_wakeup(*request).await.and_then(encode_result)
            }
            PreparedWake::Show(request) => {
                client.read_wakeup(request).await.and_then(encode_result)
            }
            PreparedWake::Mutate(action, request) => match action {
                LifecycleAction::Pause => client.pause_wakeup(request).await,
                LifecycleAction::Resume => client.resume_wakeup(request).await,
                LifecycleAction::Cancel => client.cancel_wakeup(request).await,
            }
            .and_then(encode_result),
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
                json!({"kind":"error","operationId":invocation.operation_id,"error":{"kind":if uncertain{"outcomeUnknown"}else{"unavailable"},"message":if uncertain{"Wake mutation may have committed. Inspect or replay the same operation ID; do not blindly create another wake."}else{"Wake service unavailable; no mutation dispatched."},"nextAction":if uncertain{"inspectOperation"}else{"retryLater"}}}),
                if uncertain { 5 } else { 3 },
            )
        }
    };
    let wait_id = if invocation.wait_until_first_fire && code == 0 {
        record
            .pointer("/result/definition/wakeupId")
            .cloned()
            .and_then(|id| serde_json::from_value::<communication_protocol::WakeupId>(id).ok())
    } else {
        None
    };
    let text = if machine {
        serde_json::to_string(&record)
    } else {
        serde_json::to_string_pretty(&record)
    };
    match text {
        Ok(text) if writeln!(io::stdout(), "{text}").is_ok() => {
            if let Some(id) = wait_id {
                runtime.block_on(crate::wakeup_wait_output::wait(
                    &invocation.directory,
                    id,
                    machine,
                ))
            } else if invocation.wait_until_first_fire && code == 0 {
                5
            } else {
                code
            }
        }
        _ => 5,
    }
}
fn prepare(command: WakeCommand) -> Result<WakeInvocation, String> {
    match command {
        WakeCommand::Pause(args) => prepare_lifecycle(args, LifecycleAction::Pause),
        WakeCommand::Resume(args) => prepare_lifecycle(args, LifecycleAction::Resume),
        WakeCommand::Cancel(args) => prepare_lifecycle(args, LifecycleAction::Cancel),
        WakeCommand::Send {
            message,
            timing,
            operation_id,
            wait_until_first_fire,
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
                wait_until_first_fire,
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
            wait_until_first_fire: false,
            request: PreparedWake::Show(WakeShowRequest {
                wakeup_id: wakeup_id
                    .try_into()
                    .map_err(|_| "--wakeup-id requires a canonical UUIDv7")?,
            }),
        }),
    }
}

fn encode_result<TResult: serde::Serialize>(
    result: TResult,
) -> Result<serde_json::Value, WakeClientError> {
    serde_json::to_value(result).map_err(|_| {
        communication_client::ClientError::Protocol("cannot encode wake result").into()
    })
}
fn prepare_lifecycle(
    args: LifecycleArguments,
    action: LifecycleAction,
) -> Result<WakeInvocation, String> {
    let id = args.operation_id.map_or_else(
        || Ok(OperationId::generate()),
        |id| {
            id.try_into()
                .map_err(|_| "--operation-id requires a canonical UUIDv7")
        },
    )?;
    Ok(WakeInvocation {
        directory: crate::endpoint_commands::resolve_directory(args.service_directory)?,
        json: args.json,
        operation_id: Some(id.clone()),
        wait_until_first_fire: false,
        request: PreparedWake::Mutate(
            action,
            WakeMutationRequest {
                operation_id: id,
                wakeup_id: args
                    .wakeup_id
                    .try_into()
                    .map_err(|_| "--wakeup-id requires a canonical UUIDv7")?,
            },
        ),
    })
}
