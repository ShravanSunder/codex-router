//! Client-neutral inspection of a caller-owned conversation operation.
use clap::{Args, Subcommand};
use collaboration_client::protocol::{
    ConversationOperationReconcileRequest, ConversationOperationShowRequest,
    ConversationOperationWaitRequest, OperationId, PositiveSeconds,
};
use collaboration_client::{ClientError, ControlClient};
use std::path::PathBuf;

#[derive(Subcommand)]
pub(crate) enum ConversationOperationCommand {
    /// Inspect durable metadata without replaying client work.
    Show(OperationIdentityArguments),
    /// Wait for an admitted operation; detaching does not cancel it.
    Wait(OperationWaitArguments),
    /// Reconcile only from the exact recorded client evidence.
    Reconcile(OperationIdentityArguments),
}

#[derive(Args)]
pub(crate) struct OperationIdentityArguments {
    #[command(flatten)]
    connection: OperationConnectionArguments,
    #[arg(long)]
    operation_id: String,
}

#[derive(Args)]
pub(crate) struct OperationWaitArguments {
    #[command(flatten)]
    connection: OperationConnectionArguments,
    #[arg(long)]
    operation_id: String,
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..))]
    timeout_seconds: u64,
}

#[derive(Args)]
struct OperationConnectionArguments {
    #[arg(long)]
    service_directory: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

enum OperationRead {
    Show(ConversationOperationShowRequest),
    Wait(ConversationOperationWaitRequest),
    Reconcile(ConversationOperationReconcileRequest),
}

impl OperationRead {
    async fn execute(self, client: &mut ControlClient) -> Result<serde_json::Value, ClientError> {
        let response = match self {
            Self::Show(request) => {
                serde_json::to_value(client.show_provider_conversation_operation(request).await?)
            }
            Self::Wait(request) => serde_json::to_value(
                client
                    .wait_for_provider_conversation_operation(request)
                    .await?,
            ),
            Self::Reconcile(request) => serde_json::to_value(
                client
                    .reconcile_provider_conversation_operation(request)
                    .await?,
            ),
        };
        response.map_err(|_| ClientError::Protocol("conversation operation output encoding failed"))
    }
}

pub(crate) fn run_conversation_operation_command(command: ConversationOperationCommand) -> i32 {
    let (connection, operation_id, request) = match prepare(command) {
        Ok(prepared) => prepared,
        Err((message, json)) => {
            return crate::endpoint_commands::report_failure("invalidField", &message, 2, json);
        }
    };
    let directory = match crate::endpoint_commands::resolve_directory(connection.service_directory)
    {
        Ok(directory) => directory,
        Err(message) => {
            return crate::endpoint_commands::report_failure(
                "invalidField",
                &message,
                2,
                connection.json,
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
    runtime.block_on(async move {
        let mut client = match ControlClient::connect(
            &directory,
            "agent-collaboration",
            env!("CARGO_PKG_VERSION"),
        )
        .await
        {
            Ok(client) => client,
            Err(error) => {
                return crate::provider_conversation_commands::report_client_failure(
                    error,
                    &operation_id,
                    false,
                    connection.json,
                );
            }
        };
        let result = request.execute(&mut client).await;
        let _closed = client.close().await;
        match result {
            Ok(value) => {
                crate::provider_conversation_commands::report_success(value, connection.json)
            }
            Err(error) => crate::provider_conversation_commands::report_client_failure(
                error,
                &operation_id,
                false,
                connection.json,
            ),
        }
    })
}

fn prepare(
    command: ConversationOperationCommand,
) -> Result<(OperationConnectionArguments, OperationId, OperationRead), (String, bool)> {
    match command {
        ConversationOperationCommand::Show(args) => {
            let operation_id = parse_operation_id(&args.operation_id, args.connection.json)?;
            Ok((
                args.connection,
                operation_id.clone(),
                OperationRead::Show(ConversationOperationShowRequest { operation_id }),
            ))
        }
        ConversationOperationCommand::Reconcile(args) => {
            let operation_id = parse_operation_id(&args.operation_id, args.connection.json)?;
            Ok((
                args.connection,
                operation_id.clone(),
                OperationRead::Reconcile(ConversationOperationReconcileRequest { operation_id }),
            ))
        }
        ConversationOperationCommand::Wait(args) => {
            let operation_id = parse_operation_id(&args.operation_id, args.connection.json)?;
            let timeout_seconds = u32::try_from(args.timeout_seconds)
                .ok()
                .and_then(|seconds| PositiveSeconds::try_from(seconds).ok())
                .ok_or_else(|| {
                    (
                        "duration must be an integer between 1 and 31536000 seconds".to_owned(),
                        args.connection.json,
                    )
                })?;
            Ok((
                args.connection,
                operation_id.clone(),
                OperationRead::Wait(ConversationOperationWaitRequest {
                    operation_id,
                    timeout_seconds,
                }),
            ))
        }
    }
}

fn parse_operation_id(value: &str, json: bool) -> Result<OperationId, (String, bool)> {
    OperationId::try_from(value.to_owned()).map_err(|_| {
        (
            "operation ID must be a canonical lowercase RFC UUIDv7".to_owned(),
            json,
        )
    })
}
