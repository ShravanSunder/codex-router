//! Read-only collection commands use the same typed SDK and never open automation storage.
use communication_client::{
    AutomationInspectionClientError, ClientError, ControlClient, WakeClientError,
};
use communication_protocol::{
    AutomationPageRequest, DeliveryListRequest, DeliveryShowRequest, RevisionListRequest,
    RunListRequest,
};
use serde_json::{Value, json};
use std::{
    io::{self, Write},
    path::PathBuf,
};

#[derive(clap::Args)]
pub(crate) struct PageOptions {
    /// Opaque cursor from the previous page. Start without it for newly created records.
    #[arg(long)]
    pub cursor: Option<String>,
    #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=100))]
    pub limit: u32,
}
impl PageOptions {
    fn request(self) -> Result<AutomationPageRequest, String> {
        Ok(AutomationPageRequest {
            cursor: self.cursor,
            limit: self
                .limit
                .try_into()
                .map_err(|_| "--limit must be 1..100")?,
        })
    }
}
#[derive(clap::Args)]
pub(crate) struct RunListOptions {
    #[arg(long)]
    pub schedule_id: String,
    #[command(flatten)]
    pub page: PageOptions,
}
#[derive(clap::Args)]
pub(crate) struct RevisionListOptions {
    #[arg(long)]
    pub instruction_id: String,
    #[command(flatten)]
    pub page: PageOptions,
}
#[derive(clap::Args)]
pub(crate) struct DeliveryListOptions {
    #[arg(long)]
    pub wakeup_id: Option<String>,
    #[command(flatten)]
    pub page: PageOptions,
}
pub(crate) enum CollectionCommand {
    Instructions(PageOptions),
    Schedules(PageOptions),
    Runs(RunListOptions),
    Revisions(RevisionListOptions),
    Deliveries(DeliveryListOptions),
    DeliveryShow(String),
}
pub(crate) struct CollectionContext {
    pub service_directory: Option<PathBuf>,
    pub json: bool,
}
enum PreparedCollection {
    Instructions(AutomationPageRequest),
    Schedules(AutomationPageRequest),
    Runs(RunListRequest),
    Revisions(RevisionListRequest),
    Deliveries(DeliveryListRequest),
    DeliveryShow(DeliveryShowRequest),
}
enum ReadCommandError {
    Inspection(Box<communication_protocol::AutomationInspectionFailure>),
    Wake(Box<communication_protocol::WakeFailure>),
    Client(ClientError),
}
impl From<AutomationInspectionClientError> for ReadCommandError {
    fn from(error: AutomationInspectionClientError) -> Self {
        match error {
            AutomationInspectionClientError::Rejected(error) => Self::Inspection(error),
            AutomationInspectionClientError::Connection(error) => Self::Client(error),
        }
    }
}
impl From<WakeClientError> for ReadCommandError {
    fn from(error: WakeClientError) -> Self {
        match error {
            WakeClientError::Rejected(error) => Self::Wake(error),
            WakeClientError::Connection(error) => Self::Client(error),
        }
    }
}
impl From<ClientError> for ReadCommandError {
    fn from(error: ClientError) -> Self {
        Self::Client(error)
    }
}

pub(crate) fn run_collection_command(
    command: CollectionCommand,
    context: CollectionContext,
) -> i32 {
    let request = match prepare(command) {
        Ok(request) => request,
        Err(message) => {
            return crate::endpoint_commands::report_failure(
                "invalidField",
                &message,
                2,
                context.json,
            );
        }
    };
    let directory = match crate::endpoint_commands::resolve_directory(context.service_directory) {
        Ok(directory) => directory,
        Err(message) => {
            return crate::endpoint_commands::report_failure(
                "invalidUsage",
                &message,
                2,
                context.json,
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
                context.json,
            );
        }
    };
    let result: Result<Value, ReadCommandError> = runtime.block_on(async {
        let mut client = ControlClient::connect(
            &directory,
            "agent-sessions-inspection",
            env!("CARGO_PKG_VERSION"),
        )
        .await?;
        let result = match request {
            PreparedCollection::Instructions(request) => {
                read_value(client.list_instructions(request)).await
            }
            PreparedCollection::Schedules(request) => {
                read_value(client.list_schedules(request)).await
            }
            PreparedCollection::Runs(request) => read_value(client.list_runs(request)).await,
            PreparedCollection::Revisions(request) => {
                read_value(client.list_instruction_revisions(request)).await
            }
            PreparedCollection::Deliveries(request) => {
                read_value(client.list_deliveries(request)).await
            }
            PreparedCollection::DeliveryShow(request) => {
                read_value(client.read_delivery(request)).await
            }
        };
        let _ = client.close().await;
        result
    });
    let (record, code) = match result {
        Ok(result) => (json!({"kind":"result","result":result}), 0),
        Err(ReadCommandError::Inspection(error)) => (json!({"kind":"error","error":error}), 4),
        Err(ReadCommandError::Wake(error)) => (json!({"kind":"error","error":error}), 4),
        Err(ReadCommandError::Client(error)) => {
            let kind = if matches!(error, ClientError::UnsupportedCapability(_)) {
                "unsupportedCapability"
            } else {
                "unavailable"
            };
            (
                json!({"kind":"error","error":{"kind":kind,"stage":"inspection","message":"Inspection connection unavailable; reconnect without resubmitting work.","nextAction":"retryLater"}}),
                3,
            )
        }
    };
    let encoded = if context.json {
        serde_json::to_string(&record)
    } else {
        serde_json::to_string_pretty(&record)
    };
    match encoded {
        Ok(text) if writeln!(io::stdout(), "{text}").is_ok() => code,
        _ => 3,
    }
}
async fn read_value<TResult: serde::Serialize, TError: Into<ReadCommandError>>(
    operation: impl std::future::Future<Output = Result<TResult, TError>>,
) -> Result<Value, ReadCommandError> {
    let result = operation.await.map_err(Into::into)?;
    serde_json::to_value(result).map_err(|_| {
        ReadCommandError::Client(ClientError::Protocol("inspection output encoding failed"))
    })
}
fn prepare(command: CollectionCommand) -> Result<PreparedCollection, String> {
    match command {
        CollectionCommand::Instructions(page) => {
            Ok(PreparedCollection::Instructions(page.request()?))
        }
        CollectionCommand::Schedules(page) => Ok(PreparedCollection::Schedules(page.request()?)),
        CollectionCommand::Runs(options) => {
            let page = options.page.request()?;
            Ok(PreparedCollection::Runs(RunListRequest {
                schedule_id: options
                    .schedule_id
                    .try_into()
                    .map_err(|_| "--schedule-id requires UUIDv7")?,
                cursor: page.cursor,
                limit: page.limit,
            }))
        }
        CollectionCommand::Revisions(options) => {
            let page = options.page.request()?;
            Ok(PreparedCollection::Revisions(RevisionListRequest {
                instruction_id: options
                    .instruction_id
                    .try_into()
                    .map_err(|_| "--instruction-id requires UUIDv7")?,
                cursor: page.cursor,
                limit: page.limit,
            }))
        }
        CollectionCommand::Deliveries(options) => {
            let page = options.page.request()?;
            Ok(PreparedCollection::Deliveries(DeliveryListRequest {
                wakeup_id: options
                    .wakeup_id
                    .map(|id| id.try_into().map_err(|_| "--wakeup-id requires UUIDv7"))
                    .transpose()?,
                cursor: page.cursor,
                limit: page.limit,
            }))
        }
        CollectionCommand::DeliveryShow(id) => {
            Ok(PreparedCollection::DeliveryShow(DeliveryShowRequest {
                delivery_id: id.try_into().map_err(|_| "--delivery-id requires UUIDv7")?,
            }))
        }
    }
}
