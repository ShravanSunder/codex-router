//! Configure future worker/summary budgets and inspect automation readiness through the Rust SDK.
use clap::{Parser, Subcommand};
use communication_client::{ConfigurationClientError, ControlClient};
use communication_protocol::{AutomationConfigureRequest, OperationId};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
};
#[derive(Parser)]
#[command(
    name = "agent-sessions automation",
    bin_name = "agent-sessions automation"
)]
struct AutomationArguments {
    #[arg(long, global = true)]
    service_directory: Option<PathBuf>,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: AutomationCommand,
}
#[derive(Subcommand)]
enum AutomationCommand {
    /// Inspect durable automation storage and effective default budgets.
    Status,
    /// Replace defaults for future attempts; active attempts retain captured budgets.
    Configure {
        #[arg(long)]
        operation_id: Option<String>,
        #[arg(long)]
        execution_timeout_seconds: u32,
        #[arg(long)]
        summary_timeout_seconds: u32,
    },
}
pub fn run_automation_command(arguments: Vec<OsString>) -> i32 {
    let args = match AutomationArguments::try_parse_from(arguments) {
        Ok(args) => args,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _ = error.print();
            return code;
        }
    };
    let request = match args.command {
        AutomationCommand::Status => None,
        AutomationCommand::Configure {
            operation_id,
            execution_timeout_seconds,
            summary_timeout_seconds,
        } => {
            let parsed = (|| {
                Ok::<_, String>(AutomationConfigureRequest {
                    operation_id: operation_id.map_or_else(
                        || Ok(OperationId::generate()),
                        |id| {
                            id.try_into()
                                .map_err(|_| "--operation-id requires UUIDv7".to_owned())
                        },
                    )?,
                    execution_timeout_seconds: execution_timeout_seconds
                        .try_into()
                        .map_err(|_| "--execution-timeout-seconds must be1..31536000")?,
                    summary_timeout_seconds: summary_timeout_seconds
                        .try_into()
                        .map_err(|_| "--summary-timeout-seconds must be1..31536000")?,
                })
            })();
            match parsed {
                Ok(request) => Some(request),
                Err(message) => {
                    return crate::endpoint_commands::report_failure(
                        "invalidField",
                        &message,
                        2,
                        args.json,
                    );
                }
            }
        }
    };
    let operation_id = request.as_ref().map(|request| request.operation_id.clone());
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
    let result: Result<Value, ConfigurationClientError> = runtime.block_on(async {
        let mut client = ControlClient::connect(
            &directory,
            "agent-sessions-automation",
            env!("CARGO_PKG_VERSION"),
        )
        .await?;
        dispatched = true;
        let result = match request {
            Some(request) => client.configure_automation(request).await.and_then(encode),
            None => client.automation_status().await.and_then(encode),
        };
        let _ = client.close().await;
        result
    });
    let (record, code) = match result {
        Ok(result) => (
            json!({"kind":"result","operationId":operation_id,"result":result}),
            0,
        ),
        Err(ConfigurationClientError::Rejected(error)) => {
            let uncertain = !matches!(
                error.file_state,
                communication_protocol::ConfigurationFileState::NotReplaced
            );
            (
                json!({"kind":"error","operationId":operation_id,"error":error}),
                if uncertain { 5 } else { 4 },
            )
        }
        Err(ConfigurationClientError::Connection(_)) => {
            let uncertain = dispatched && operation_id.is_some();
            (
                json!({"kind":"error","operationId":operation_id,"error":{"kind":if uncertain{"outcomeUnknown"}else{"unavailable"},"message":if uncertain{"Configuration may have changed. Inspect or replay the same operation ID; no active attempt budget is silently reset."}else{"Automation service unavailable; no configuration change dispatched."},"nextAction":if uncertain{"inspectOperation"}else{"retryLater"}}}),
                if uncertain { 5 } else { 3 },
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
fn encode<TResult: serde::Serialize>(result: TResult) -> Result<Value, ConfigurationClientError> {
    serde_json::to_value(result).map_err(|_| {
        communication_client::ClientError::Protocol("invalid configuration output").into()
    })
}
