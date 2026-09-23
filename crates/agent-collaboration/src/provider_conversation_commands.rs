//! External ACP provider conversation commands over the typed Control client.
use clap::{Args, Subcommand, ValueEnum};
use collaboration_client::protocol::{
    ConversationCancelRequest, ConversationCreateRequest, ConversationLoadRequest,
    ConversationOperationFailure, ConversationOperationFailureKind,
    ConversationOperationReconcileRequest, ConversationOperationShowRequest,
    ConversationOperationWaitRequest, ConversationPromptRequest, MessageContent, MessageText,
    OperationId, PositiveSeconds, ProviderRequestedPolicy, ProviderWorkingDirectory, RouterAccess,
};
use collaboration_client::{ClientError, ControlClient};
use serde::{Serialize, de::DeserializeOwned};
use std::{io::Write, path::PathBuf};

#[derive(Subcommand)]
pub(crate) enum ProviderConversationCommand {
    /// Admit creation of a provider-owned conversation under a caller-supplied operation ID.
    Create(ProviderCreateArguments),
    /// Load an existing provider-owned conversation when the binding supports load.
    Load(ProviderLoadArguments),
    /// Submit one prompt to an existing provider-owned conversation.
    Prompt(ProviderPromptArguments),
    /// Cooperatively cancel one exact active operation and binding generation.
    Cancel(ProviderCancelArguments),
    /// Inspect metadata for an operation without replaying provider work.
    OperationShow(OperationIdentityArguments),
    /// Wait for already-owned provider work; caller cancellation only detaches this waiter.
    Wait(ProviderWaitArguments),
    /// Query supported exact evidence for an operation without replaying it.
    Reconcile(OperationIdentityArguments),
}

#[derive(Args)]
pub(crate) struct ProviderCreateArguments {
    #[command(flatten)]
    connection: ProviderConnectionArguments,
    #[arg(long)]
    operation_id: String,
    #[arg(long)]
    endpoint: String,
    #[arg(long)]
    generation: String,
    #[arg(long)]
    created_by: String,
    #[arg(long)]
    approver: String,
    #[arg(long)]
    cwd: PathBuf,
    #[arg(long)]
    access: ProviderAccess,
}

#[derive(Args)]
pub(crate) struct ProviderLoadArguments {
    #[command(flatten)]
    connection: ProviderConnectionArguments,
    #[arg(long)]
    operation_id: String,
    #[arg(long)]
    target: String,
    #[arg(long)]
    generation: String,
    #[arg(long)]
    requested_by: String,
    #[arg(long)]
    approver: String,
    #[arg(long)]
    cwd: PathBuf,
    #[arg(long)]
    access: ProviderAccess,
}

#[derive(Args)]
pub(crate) struct ProviderPromptArguments {
    #[command(flatten)]
    connection: ProviderConnectionArguments,
    #[arg(long)]
    operation_id: String,
    #[arg(long)]
    target: String,
    #[arg(long)]
    generation: String,
    #[arg(long)]
    requested_by: String,
    #[arg(long)]
    approver: String,
    #[arg(
        long,
        required_unless_present = "text_file",
        conflicts_with = "text_file"
    )]
    text: Option<String>,
    #[arg(long)]
    text_file: Option<PathBuf>,
}

#[derive(Args)]
pub(crate) struct ProviderCancelArguments {
    #[command(flatten)]
    connection: ProviderConnectionArguments,
    #[arg(long)]
    operation_id: String,
    #[arg(long)]
    target_operation_id: String,
    #[arg(long)]
    target: String,
    #[arg(long)]
    generation: String,
    #[arg(long)]
    requested_by: String,
    #[arg(long)]
    approver: String,
}

#[derive(Args)]
pub(crate) struct OperationIdentityArguments {
    #[command(flatten)]
    connection: ProviderConnectionArguments,
    #[arg(long)]
    operation_id: String,
}

#[derive(Args)]
pub(crate) struct ProviderWaitArguments {
    #[command(flatten)]
    connection: ProviderConnectionArguments,
    #[arg(long)]
    operation_id: String,
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..))]
    timeout_seconds: u64,
}

#[derive(Args)]
struct ProviderConnectionArguments {
    #[arg(long)]
    service_directory: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum ProviderAccess {
    WriteRestricted,
    WorkspaceWrite,
}

impl From<ProviderAccess> for RouterAccess {
    fn from(value: ProviderAccess) -> Self {
        match value {
            ProviderAccess::WriteRestricted => Self::WriteRestricted,
            ProviderAccess::WorkspaceWrite => Self::WorkspaceWrite,
        }
    }
}

pub(crate) fn run_provider_conversation_command(command: ProviderConversationCommand) -> i32 {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return 3,
    };
    runtime.block_on(run_command(command))
}

async fn run_command(command: ProviderConversationCommand) -> i32 {
    let prepared = match prepare_command(command) {
        Ok(prepared) => prepared,
        Err(error) => return report_validation_failure(&error.message, error.json_output),
    };
    let (connection, operation_id, request) = prepared.into_parts();
    let may_have_dispatched = request.is_mutation();
    let directory = match crate::endpoint_commands::resolve_directory(connection.service_directory)
    {
        Ok(directory) => directory,
        Err(message) => return report_validation_failure(&message, connection.json),
    };
    let mut client =
        match ControlClient::connect(&directory, "agent-collaboration", env!("CARGO_PKG_VERSION"))
            .await
        {
            Ok(client) => client,
            Err(error) => {
                return report_client_failure(error, &operation_id, false, connection.json);
            }
        };
    let result = request.execute(&mut client).await;
    let _closed = client.close().await;
    match result {
        Ok(value) => report_success(value, connection.json),
        Err(error) => {
            report_client_failure(error, &operation_id, may_have_dispatched, connection.json)
        }
    }
}

struct PreparedCommand {
    connection: ProviderConnectionArguments,
    operation_id: OperationId,
    request: PreparedRequest,
}

impl PreparedCommand {
    fn into_parts(self) -> (ProviderConnectionArguments, OperationId, PreparedRequest) {
        (self.connection, self.operation_id, self.request)
    }
}

enum PreparedRequest {
    Create(ConversationCreateRequest),
    Load(ConversationLoadRequest),
    Prompt(ConversationPromptRequest),
    Cancel(ConversationCancelRequest),
    Show(ConversationOperationShowRequest),
    Wait(ConversationOperationWaitRequest),
    Reconcile(ConversationOperationReconcileRequest),
}

impl PreparedRequest {
    const fn is_mutation(&self) -> bool {
        matches!(
            self,
            Self::Create(_) | Self::Load(_) | Self::Prompt(_) | Self::Cancel(_)
        )
    }
    async fn execute(self, client: &mut ControlClient) -> Result<serde_json::Value, ClientError> {
        match self {
            Self::Create(request) => encode(client.create_provider_conversation(request).await?),
            Self::Load(request) => encode(client.load_provider_conversation(request).await?),
            Self::Prompt(request) => encode(client.prompt_provider_conversation(request).await?),
            Self::Cancel(request) => encode(
                client
                    .cancel_provider_conversation_operation(request)
                    .await?,
            ),
            Self::Show(request) => {
                encode(client.show_provider_conversation_operation(request).await?)
            }
            Self::Wait(request) => encode(
                client
                    .wait_for_provider_conversation_operation(request)
                    .await?,
            ),
            Self::Reconcile(request) => encode(
                client
                    .reconcile_provider_conversation_operation(request)
                    .await?,
            ),
        }
    }
}

fn encode(value: impl Serialize) -> Result<serde_json::Value, ClientError> {
    serde_json::to_value(value)
        .map_err(|_| ClientError::Protocol("provider conversation output encoding failed"))
}

struct PreparationError {
    message: String,
    json_output: bool,
}

fn prepare_command(
    command: ProviderConversationCommand,
) -> Result<PreparedCommand, PreparationError> {
    match command {
        ProviderConversationCommand::Create(args) => {
            let operation_id = parse_operation_id(&args.operation_id, args.connection.json)?;
            let request = ConversationCreateRequest {
                operation_id: operation_id.clone(),
                endpoint: parse_json("--endpoint", &args.endpoint, args.connection.json)?,
                generation: parse_json("--generation", &args.generation, args.connection.json)?,
                working_directory: parse_working_directory(&args.cwd, args.connection.json)?,
                created_by: parse_json("--created-by", &args.created_by, args.connection.json)?,
                approver: parse_json("--approver", &args.approver, args.connection.json)?,
                requested_policy: ProviderRequestedPolicy {
                    access: args.access.into(),
                },
            };
            Ok(PreparedCommand {
                connection: args.connection,
                operation_id,
                request: PreparedRequest::Create(request),
            })
        }
        ProviderConversationCommand::Load(args) => {
            let operation_id = parse_operation_id(&args.operation_id, args.connection.json)?;
            let request = ConversationLoadRequest {
                operation_id: operation_id.clone(),
                target: parse_json("--target", &args.target, args.connection.json)?,
                generation: parse_json("--generation", &args.generation, args.connection.json)?,
                working_directory: parse_working_directory(&args.cwd, args.connection.json)?,
                requested_by: parse_json(
                    "--requested-by",
                    &args.requested_by,
                    args.connection.json,
                )?,
                approver: parse_json("--approver", &args.approver, args.connection.json)?,
                requested_policy: ProviderRequestedPolicy {
                    access: args.access.into(),
                },
            };
            Ok(PreparedCommand {
                connection: args.connection,
                operation_id,
                request: PreparedRequest::Load(request),
            })
        }
        ProviderConversationCommand::Prompt(args) => {
            let operation_id = parse_operation_id(&args.operation_id, args.connection.json)?;
            let text = match (args.text, args.text_file) {
                (Some(text), None) => text,
                (None, Some(path)) => {
                    std::fs::read_to_string(path).map_err(|error| PreparationError {
                        message: format!("failed to read --text-file: {error}"),
                        json_output: args.connection.json,
                    })?
                }
                _ => {
                    return Err(PreparationError {
                        message: "exactly one of --text or --text-file is required".to_owned(),
                        json_output: args.connection.json,
                    });
                }
            };
            let request = ConversationPromptRequest {
                operation_id: operation_id.clone(),
                target: parse_json("--target", &args.target, args.connection.json)?,
                generation: parse_json("--generation", &args.generation, args.connection.json)?,
                requested_by: parse_json(
                    "--requested-by",
                    &args.requested_by,
                    args.connection.json,
                )?,
                approver: parse_json("--approver", &args.approver, args.connection.json)?,
                prompt: MessageContent::Agent {
                    sender: parse_json("--requested-by", &args.requested_by, args.connection.json)?,
                    text: MessageText::try_from(text).map_err(|error| PreparationError {
                        message: error.to_string(),
                        json_output: args.connection.json,
                    })?,
                },
            };
            Ok(PreparedCommand {
                connection: args.connection,
                operation_id,
                request: PreparedRequest::Prompt(request),
            })
        }
        ProviderConversationCommand::Cancel(args) => {
            let operation_id = parse_operation_id(&args.operation_id, args.connection.json)?;
            let request = ConversationCancelRequest {
                operation_id: operation_id.clone(),
                target_operation_id: parse_operation_id(
                    &args.target_operation_id,
                    args.connection.json,
                )?,
                target: parse_json("--target", &args.target, args.connection.json)?,
                generation: parse_json("--generation", &args.generation, args.connection.json)?,
                requested_by: parse_json(
                    "--requested-by",
                    &args.requested_by,
                    args.connection.json,
                )?,
                approver: parse_json("--approver", &args.approver, args.connection.json)?,
            };
            Ok(PreparedCommand {
                connection: args.connection,
                operation_id,
                request: PreparedRequest::Cancel(request),
            })
        }
        ProviderConversationCommand::OperationShow(args) => {
            prepare_identity(args, PreparedRequest::Show)
        }
        ProviderConversationCommand::Reconcile(args) => {
            let operation_id = parse_operation_id(&args.operation_id, args.connection.json)?;
            Ok(PreparedCommand {
                connection: args.connection,
                operation_id: operation_id.clone(),
                request: PreparedRequest::Reconcile(ConversationOperationReconcileRequest {
                    operation_id,
                }),
            })
        }
        ProviderConversationCommand::Wait(args) => {
            let operation_id = parse_operation_id(&args.operation_id, args.connection.json)?;
            let timeout_seconds = u32::try_from(args.timeout_seconds)
                .ok()
                .and_then(|seconds| PositiveSeconds::try_from(seconds).ok())
                .ok_or_else(|| PreparationError {
                    message: "duration must be an integer between 1 and 31536000 seconds"
                        .to_owned(),
                    json_output: args.connection.json,
                })?;
            Ok(PreparedCommand {
                connection: args.connection,
                operation_id: operation_id.clone(),
                request: PreparedRequest::Wait(ConversationOperationWaitRequest {
                    operation_id,
                    timeout_seconds,
                }),
            })
        }
    }
}

fn prepare_identity(
    args: OperationIdentityArguments,
    constructor: impl FnOnce(ConversationOperationShowRequest) -> PreparedRequest,
) -> Result<PreparedCommand, PreparationError> {
    let operation_id = parse_operation_id(&args.operation_id, args.connection.json)?;
    Ok(PreparedCommand {
        connection: args.connection,
        operation_id: operation_id.clone(),
        request: constructor(ConversationOperationShowRequest { operation_id }),
    })
}

fn parse_operation_id(value: &str, json_output: bool) -> Result<OperationId, PreparationError> {
    value.to_owned().try_into().map_err(|_| PreparationError {
        message: "operation ID must be a canonical lowercase RFC UUIDv7".to_owned(),
        json_output,
    })
}

fn parse_json<TValue: DeserializeOwned>(
    name: &str,
    value: &str,
    json_output: bool,
) -> Result<TValue, PreparationError> {
    serde_json::from_str(value).map_err(|_| PreparationError {
        message: format!("invalid {name} JSON"),
        json_output,
    })
}

fn parse_working_directory(
    path: &std::path::Path,
    json_output: bool,
) -> Result<ProviderWorkingDirectory, PreparationError> {
    path.to_str()
        .ok_or_else(|| PreparationError {
            message: "--cwd must be UTF-8".to_owned(),
            json_output,
        })?
        .to_owned()
        .try_into()
        .map_err(|error: &'static str| PreparationError {
            message: error.to_owned(),
            json_output,
        })
}

fn report_success(value: serde_json::Value, json_output: bool) -> i32 {
    let value = crate::endpoint_commands::result_envelope(value);
    let rendered = if json_output {
        value.to_string()
    } else {
        serde_json::to_string_pretty(&value).unwrap_or_default()
    };
    if writeln!(std::io::stdout(), "{rendered}").is_ok() {
        0
    } else {
        3
    }
}

fn report_validation_failure(message: &str, json_output: bool) -> i32 {
    crate::endpoint_commands::report_failure("invalidField", message, 2, json_output)
}

fn report_client_failure(
    error: ClientError,
    operation_id: &OperationId,
    may_have_dispatched: bool,
    json_output: bool,
) -> i32 {
    if let ClientError::Rejected {
        data: Some(data), ..
    } = &error
        && let Ok(failure) = serde_json::from_value::<ConversationOperationFailure>(data.clone())
    {
        return report_typed_failure(&failure, json_output);
    }
    let (kind, stage, effect, exit) = if may_have_dispatched {
        ("outcomeUnknown", "settlement", "unknown", 5)
    } else {
        ("unavailable", "binding", "none", 3)
    };
    let failure = serde_json::json!({
        "kind":kind,"stage":stage,"effect":effect,"message":error.to_string(),
        "operationId":operation_id
    });
    let rendered = if json_output {
        serde_json::json!({"kind":"error","error":failure}).to_string()
    } else {
        format!("Error: {kind}\nOperation ID: {operation_id:?}\n{error}")
    };
    let _written = if json_output {
        writeln!(std::io::stdout(), "{rendered}")
    } else {
        writeln!(std::io::stderr(), "{rendered}")
    };
    exit
}

fn report_typed_failure(failure: &ConversationOperationFailure, json_output: bool) -> i32 {
    let exit = match failure.kind {
        ConversationOperationFailureKind::InvalidRequest
        | ConversationOperationFailureKind::UnsupportedCapability
        | ConversationOperationFailureKind::ProtocolViolation => 2,
        ConversationOperationFailureKind::AuthenticationRequired
        | ConversationOperationFailureKind::Unavailable => 3,
        ConversationOperationFailureKind::PermissionRejected
        | ConversationOperationFailureKind::Busy
        | ConversationOperationFailureKind::NotFound
        | ConversationOperationFailureKind::StaleGeneration
        | ConversationOperationFailureKind::ProviderRejected => 4,
        ConversationOperationFailureKind::OutcomeUnknown => 5,
    };
    let rendered = if json_output {
        serde_json::json!({"kind":"error","error":failure}).to_string()
    } else {
        format!("Error: {:?}\n{:?}", failure.kind, failure.message)
    };
    let _written = if json_output {
        writeln!(std::io::stdout(), "{rendered}")
    } else {
        writeln!(std::io::stderr(), "{rendered}")
    };
    exit
}
