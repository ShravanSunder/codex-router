//! Schedule configuration through typed SDK calls; thread preparation remains a separate command.
use crate::schedule_preparation_arguments::{PreparationArguments, PreparedDestination};
use clap::{Parser, Subcommand};
use communication_client::{ControlClient, ScheduleClientError};
use communication_protocol::{
    LocalMutationState, OperationId, ScheduleCreateRequest, ScheduleDefinition, ScheduleEffects,
    ScheduleEnableRequest, ScheduleShowRequest, ScheduleSnapshot, ScheduleUpdateRequest,
};
use serde_json::json;
use std::{
    ffi::OsString,
    io::{self, Read, Write},
    path::PathBuf,
};
#[derive(Parser)]
#[command(name = "agent-sessions schedule", bin_name = "agent-sessions schedule")]
struct ScheduleArguments {
    #[arg(long, global = true)]
    service_directory: Option<PathBuf>,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: ScheduleCommand,
}
#[derive(Subcommand)]
enum ScheduleCommand {
    /// Prepare a fresh, forked or explicitly adopted native thread without enabling its schedule.
    Prepare(PreparationArguments),
    /// Save schedule configuration. An unprepared destination must be disabled.
    Create {
        /// UTF-8 ScheduleDefinition JSON; '-' reads stdin. See the published Control schema.
        #[arg(long)]
        definition_file: PathBuf,
        #[arg(long)]
        operation_id: Option<String>,
    },
    /// Replace future configuration using the current edit token; admitted runs retain their inputs.
    Update {
        #[arg(long)]
        schedule_id: String,
        #[arg(long)]
        expected_change_id: String,
        #[arg(long)]
        definition_file: PathBuf,
        #[arg(long)]
        operation_id: Option<String>,
    },
    /// Inspect configuration, timing and derived active/waiting run identities.
    Show {
        #[arg(long)]
        schedule_id: String,
    },
    /// Enable future triggers after destination preparation and capability checks.
    Enable {
        #[arg(long)]
        schedule_id: String,
        #[arg(long)]
        operation_id: Option<String>,
    },
    /// Stop future triggers. Existing waiting and active runs remain intact.
    Disable {
        #[arg(long)]
        schedule_id: String,
        #[arg(long)]
        operation_id: Option<String>,
    },
}
enum PreparedSchedule {
    Prepare {
        operation_id: OperationId,
        schedule_id: communication_protocol::ScheduleId,
        destination: PreparedDestination,
    },
    Create(Box<ScheduleCreateRequest>),
    Update(Box<ScheduleUpdateRequest>),
    Show(ScheduleShowRequest),
    Enable(ScheduleEnableRequest),
    Disable(ScheduleEnableRequest),
}
impl PreparedSchedule {
    fn operation_id(&self) -> Option<OperationId> {
        match self {
            Self::Prepare { operation_id, .. } => Some(operation_id.clone()),
            Self::Create(request) => Some(request.operation_id.clone()),
            Self::Update(request) => Some(request.operation_id.clone()),
            Self::Show(_) => None,
            Self::Enable(request) | Self::Disable(request) => Some(request.operation_id.clone()),
        }
    }
}
pub fn run_schedule_command(arguments: Vec<OsString>) -> i32 {
    let args = match ScheduleArguments::try_parse_from(arguments) {
        Ok(args) => args,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _ = error.print();
            return code;
        }
    };
    let directory = match crate::endpoint_commands::resolve_directory(args.service_directory) {
        Ok(directory) => directory,
        Err(message) => {
            return crate::endpoint_commands::report_failure(
                "invalidUsage",
                &message,
                2,
                args.json,
            );
        }
    };
    let prepared = match prepare(args.command) {
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
    let operation_id = prepared.operation_id();
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
    let result: Result<ScheduleSnapshot, ScheduleClientError> = runtime.block_on(async {
        let mut client = ControlClient::connect(
            &directory,
            "agent-sessions-schedule",
            env!("CARGO_PKG_VERSION"),
        )
        .await?;
        dispatched = true;
        let result = match prepared {
            PreparedSchedule::Prepare {
                operation_id,
                schedule_id,
                destination,
            } => {
                client
                    .prepare_schedule(communication_protocol::SchedulePrepareRequest {
                        operation_id,
                        schedule_id,
                        destination: destination.resolve(client.identity().service_id.clone()),
                    })
                    .await
            }
            PreparedSchedule::Create(request) => client.create_schedule(*request).await,
            PreparedSchedule::Update(request) => client.update_schedule(*request).await,
            PreparedSchedule::Show(request) => client.read_schedule(request).await,
            PreparedSchedule::Enable(request) => client.enable_schedule(request).await,
            PreparedSchedule::Disable(request) => client.disable_schedule(request).await,
        };
        let _ = client.close().await;
        result
    });
    let (record, code) = match result {
        Ok(result) => (
            json!({"kind":"result","operationId":operation_id,"result":result}),
            0,
        ),
        Err(ScheduleClientError::Rejected(error)) => {
            let uncertain = matches!(
                error.effects,
                ScheduleEffects::Local {
                    mutation: LocalMutationState::Unknown | LocalMutationState::Committed
                }
            ) || matches!(&error.effects,ScheduleEffects::Native{evidence} if matches!(evidence.allocation,communication_protocol::PreparationEffect::Accepted|communication_protocol::PreparationEffect::Unknown) || matches!(evidence.resume,communication_protocol::PreparationEffect::Accepted|communication_protocol::PreparationEffect::Unknown) || matches!(evidence.submission,communication_protocol::SubmissionEffect::Accepted|communication_protocol::SubmissionEffect::Dispatching|communication_protocol::SubmissionEffect::Unknown));
            (
                json!({"kind":"error","operationId":operation_id,"error":error}),
                if uncertain { 5 } else { 4 },
            )
        }
        Err(ScheduleClientError::Connection(_)) => {
            let uncertain = dispatched && operation_id.is_some();
            (
                json!({"kind":"error","operationId":operation_id,"error":{"kind":if uncertain{"outcomeUnknown"}else{"unavailable"},"message":if uncertain{"Schedule mutation may have committed. Inspect or replay the same operation ID before creating another request."}else{"Schedule service unavailable; no mutation was dispatched."},"nextAction":if uncertain{"inspectOperation"}else{"retryLater"}}}),
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
fn operation(value: Option<String>) -> Result<OperationId, String> {
    value.map_or_else(
        || Ok(OperationId::generate()),
        |value| {
            value
                .try_into()
                .map_err(|_| "--operation-id requires a canonical UUIDv7".into())
        },
    )
}
fn prepare(command: ScheduleCommand) -> Result<PreparedSchedule, String> {
    match command {
        ScheduleCommand::Prepare(args) => {
            let destination = args.destination()?;
            Ok(PreparedSchedule::Prepare {
                operation_id: operation(args.operation_id)?,
                schedule_id: args
                    .schedule_id
                    .try_into()
                    .map_err(|_| "--schedule-id requires UUIDv7")?,
                destination,
            })
        }
        ScheduleCommand::Create {
            definition_file,
            operation_id,
        } => Ok(PreparedSchedule::Create(Box::new(ScheduleCreateRequest {
            operation_id: operation(operation_id)?,
            definition: read_definition(definition_file)?,
        }))),
        ScheduleCommand::Update {
            schedule_id,
            expected_change_id,
            definition_file,
            operation_id,
        } => Ok(PreparedSchedule::Update(Box::new(ScheduleUpdateRequest {
            operation_id: operation(operation_id)?,
            schedule_id: schedule_id
                .try_into()
                .map_err(|_| "--schedule-id requires UUIDv7")?,
            expected_change_id: expected_change_id
                .try_into()
                .map_err(|_| "--expected-change-id requires UUIDv7")?,
            definition: read_definition(definition_file)?,
        }))),
        ScheduleCommand::Show { schedule_id } => Ok(PreparedSchedule::Show(ScheduleShowRequest {
            schedule_id: schedule_id
                .try_into()
                .map_err(|_| "--schedule-id requires UUIDv7")?,
        })),
        ScheduleCommand::Enable {
            schedule_id,
            operation_id,
        } => Ok(PreparedSchedule::Enable(ScheduleEnableRequest {
            schedule_id: schedule_id
                .try_into()
                .map_err(|_| "--schedule-id requires UUIDv7")?,
            operation_id: operation(operation_id)?,
        })),
        ScheduleCommand::Disable {
            schedule_id,
            operation_id,
        } => Ok(PreparedSchedule::Disable(ScheduleEnableRequest {
            schedule_id: schedule_id
                .try_into()
                .map_err(|_| "--schedule-id requires UUIDv7")?,
            operation_id: operation(operation_id)?,
        })),
    }
}
fn read_definition(path: PathBuf) -> Result<ScheduleDefinition, String> {
    let mut reader: Box<dyn Read> = if path.as_os_str() == "-" {
        Box::new(io::stdin())
    } else {
        Box::new(std::fs::File::open(path).map_err(|_| "Schedule definition file unavailable")?)
    };
    let mut text = String::new();
    reader
        .by_ref()
        .take(1_048_577)
        .read_to_string(&mut text)
        .map_err(|_| "Schedule definition must be readable UTF-8")?;
    if text.len() > 1_048_576 {
        return Err("Schedule definition exceeds Control frame limit".into());
    }
    serde_json::from_str(&text).map_err(|_|"Definition must be closed ScheduleDefinition JSON with instructionId, timing, enabled, destination and nullable executionTimeoutSeconds; inspect the Control schema.".into())
}
