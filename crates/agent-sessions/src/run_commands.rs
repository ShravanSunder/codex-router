//! Run inspection and explicit summary recovery through the same public SDK agents use.
use clap::{Parser, Subcommand};
use communication_client::{ControlClient, RunClientError};
use communication_protocol::{
    LocalMutationEvidence, LocalMutationState, OperationId, RunRecoveryRequest, RunShowRequest,
    RunSnapshot,
};
use serde_json::json;
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
};
#[derive(Parser)]
#[command(name = "agent-sessions run", bin_name = "agent-sessions run")]
struct RunArguments {
    #[arg(long, global = true)]
    service_directory: Option<PathBuf>,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: RunCommand,
}
#[derive(Subcommand)]
enum RunCommand {
    /// Observe exact worker/summary evidence without starting or interrupting work; uncertainty keeps ownership.
    Reconcile {
        #[arg(long)]
        run_id: String,
    },
    /// Inspect latest and retained prior summary attempts with explicit history coverage.
    Summaries(crate::automation_collection_commands::RunSummaryOptions),
    /// List current records using a service-scoped pagination cursor.
    List(crate::automation_collection_commands::RunListOptions),
    #[command(flatten)]
    Action(RunAction),
}
#[derive(Subcommand)]
enum RunAction {
    /// Inspect exact worker state, native effects and the current continuity summary.
    Show {
        #[arg(long)]
        run_id: String,
    },
    /// Retry summary only after its previous attempt is confirmed stopped; keeps the same Run.
    SummaryRetry {
        #[arg(long)]
        run_id: String,
        #[arg(long)]
        operation_id: Option<String>,
    },
    /// Explicitly proceed without summary; cannot release uncertain or active summary work.
    SummarySkip {
        #[arg(long)]
        run_id: String,
        #[arg(long)]
        operation_id: Option<String>,
    },
}
enum PreparedRun {
    Show(RunShowRequest),
    Retry(RunRecoveryRequest),
    Skip(RunRecoveryRequest),
}
pub fn run_workflow_command(arguments: Vec<OsString>) -> i32 {
    let args = match RunArguments::try_parse_from(arguments) {
        Ok(args) => args,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _ = error.print();
            return code;
        }
    };
    let command = match args.command {
        RunCommand::Reconcile { run_id } => {
            return crate::automation_collection_commands::run_collection_command(
                crate::automation_collection_commands::CollectionCommand::RunReconcile(run_id),
                crate::automation_collection_commands::CollectionContext {
                    service_directory: args.service_directory,
                    json: args.json,
                },
            );
        }
        RunCommand::Summaries(options) => {
            return crate::automation_collection_commands::run_collection_command(
                crate::automation_collection_commands::CollectionCommand::RunSummaries(options),
                crate::automation_collection_commands::CollectionContext {
                    service_directory: args.service_directory,
                    json: args.json,
                },
            );
        }
        RunCommand::List(options) => {
            return crate::automation_collection_commands::run_collection_command(
                crate::automation_collection_commands::CollectionCommand::Runs(options),
                crate::automation_collection_commands::CollectionContext {
                    service_directory: args.service_directory,
                    json: args.json,
                },
            );
        }
        RunCommand::Action(command) => command,
    };
    let prepared = match prepare(command) {
        Ok(request) => request,
        Err(message) => {
            return crate::endpoint_commands::report_failure(
                "invalidField",
                &message,
                2,
                args.json,
            );
        }
    };
    let operation_id = match &prepared {
        PreparedRun::Show(_) => None,
        PreparedRun::Retry(request) | PreparedRun::Skip(request) => {
            Some(request.operation_id.clone())
        }
    };
    let directory = match crate::endpoint_commands::resolve_directory(args.service_directory) {
        Ok(path) => path,
        Err(message) => {
            return crate::endpoint_commands::report_failure(
                "invalidUsage",
                &message,
                2,
                args.json,
            );
        }
    };
    if !args.json
        && let Some(id) = &operation_id
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
                args.json,
            );
        }
    };
    let mut dispatched = false;
    let result: Result<RunSnapshot, RunClientError> = runtime.block_on(async {
        let mut client =
            ControlClient::connect(&directory, "agent-sessions-run", env!("CARGO_PKG_VERSION"))
                .await?;
        dispatched = true;
        let result = match prepared {
            PreparedRun::Show(request) => client.read_run(request).await,
            PreparedRun::Retry(request) => client.retry_summary(request).await,
            PreparedRun::Skip(request) => client.skip_summary(request).await,
        };
        let _ = client.close().await;
        result
    });
    let (record, code) = match result {
        Ok(result) => (
            json!({"kind":"result","operationId":operation_id,"result":result}),
            0,
        ),
        Err(RunClientError::Rejected(error)) => {
            let unknown = matches!(
                error.effects,
                LocalMutationEvidence::Local {
                    mutation: LocalMutationState::Unknown | LocalMutationState::Committed
                }
            );
            (
                json!({"kind":"error","operationId":operation_id,"error":error}),
                if unknown { 5 } else { 4 },
            )
        }
        Err(RunClientError::Connection(_)) => {
            let unknown = dispatched && operation_id.is_some();
            (
                json!({"kind":"error","operationId":operation_id,"error":{"kind":if unknown{"outcomeUnknown"}else{"unavailable"},"message":if unknown{"Run recovery may have committed. Inspect or replay the same operation ID; do not blindly create another attempt."}else{"Run service unavailable; no recovery mutation dispatched."},"nextAction":if unknown{"inspectOperation"}else{"retryLater"}}}),
                if unknown { 5 } else { 3 },
            )
        }
    };
    let text = if args.json {
        serde_json::to_string(&record)
    } else {
        serde_json::to_string_pretty(&record)
    };
    match text {
        Ok(text) if writeln!(io::stdout(), "{text}").is_ok() => code,
        _ => 5,
    }
}
fn operation(value: Option<String>) -> Result<OperationId, String> {
    value.map_or_else(
        || Ok(OperationId::generate()),
        |value| {
            value
                .try_into()
                .map_err(|_| "--operation-id requires UUIDv7".into())
        },
    )
}
fn prepare(command: RunAction) -> Result<PreparedRun, String> {
    Ok(match command {
        RunAction::Show { run_id } => PreparedRun::Show(RunShowRequest {
            run_id: run_id.try_into().map_err(|_| "--run-id requires UUIDv7")?,
        }),
        RunAction::SummaryRetry {
            run_id,
            operation_id,
        } => PreparedRun::Retry(RunRecoveryRequest {
            run_id: run_id.try_into().map_err(|_| "--run-id requires UUIDv7")?,
            operation_id: operation(operation_id)?,
        }),
        RunAction::SummarySkip {
            run_id,
            operation_id,
        } => PreparedRun::Skip(RunRecoveryRequest {
            run_id: run_id.try_into().map_err(|_| "--run-id requires UUIDv7")?,
            operation_id: operation(operation_id)?,
        }),
    })
}
