//! Human/agent instruction management through the same public Rust client.
use clap::{Args, Parser, Subcommand};
use communication_client::{ControlClient, InstructionClientError};
use communication_protocol::{
    InstructionCreateParams, InstructionShowParams, InstructionSnapshot, InstructionText,
    InstructionUpdateParams, OperationId,
};
use serde_json::json;
use std::{
    ffi::OsString,
    io::{self, Read, Write},
    path::PathBuf,
};

#[derive(Parser)]
#[command(
    name = "agent-sessions instruction",
    bin_name = "agent-sessions instruction"
)]
struct InstructionArguments {
    #[arg(long, global = true)]
    service_directory: Option<PathBuf>,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: InstructionCommand,
}
#[derive(Subcommand)]
enum InstructionCommand {
    /// Create reusable instruction text. Reuse operation-id to replay a lost response safely.
    Create {
        #[arg(long)]
        operation_id: Option<String>,
        #[command(flatten)]
        content: TextInput,
    },
    /// Edit only the expected current revision; historical instruction text is retained.
    Update {
        #[arg(long)]
        operation_id: Option<String>,
        #[arg(long)]
        instruction_id: String,
        #[arg(long)]
        expected_revision_id: String,
        #[command(flatten)]
        content: TextInput,
    },
    /// Read current instruction text without loading or prompting a native thread.
    Show {
        #[arg(long)]
        instruction_id: String,
    },
}
#[derive(Args)]
struct TextInput {
    #[arg(
        long,
        required_unless_present = "text_file",
        conflicts_with = "text_file"
    )]
    text: Option<String>,
    /// Read UTF-8 text from a file; '-' reads stdin. Content is captured now.
    #[arg(long)]
    text_file: Option<PathBuf>,
}
enum PreparedInstruction {
    Create(InstructionCreateParams),
    Update(InstructionUpdateParams),
    Show(InstructionShowParams),
}
impl PreparedInstruction {
    fn operation_id(&self) -> Option<OperationId> {
        match self {
            Self::Create(p) => Some(p.operation_id.clone()),
            Self::Update(p) => Some(p.operation_id.clone()),
            Self::Show(_) => None,
        }
    }
}

pub fn run_instruction_command(arguments: Vec<OsString>) -> i32 {
    let args = match InstructionArguments::try_parse_from(arguments) {
        Ok(args) => args,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _ = error.print();
            return code;
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
    let result: Result<InstructionSnapshot, InstructionClientError> = runtime.block_on(async {
        let mut client = ControlClient::connect(
            &directory,
            "agent-sessions-instructions",
            env!("CARGO_PKG_VERSION"),
        )
        .await?;
        dispatched = true;
        let result = match prepared {
            PreparedInstruction::Create(p) => client.create_instruction(p).await,
            PreparedInstruction::Update(p) => client.update_instruction(p).await,
            PreparedInstruction::Show(p) => client.read_instruction(p).await,
        };
        let _ = client.close().await;
        result
    });
    let (record, exit) = match result {
        Ok(result) => (
            json!({"kind":"result","operationId":operation_id,"result":result}),
            0,
        ),
        Err(InstructionClientError::Rejected(error)) => (
            json!({"kind":"error","operationId":operation_id,"error":error}),
            4,
        ),
        Err(InstructionClientError::Connection(_)) => {
            let uncertain = dispatched && operation_id.is_some();
            (
                json!({"kind":"error","operationId":operation_id,"error":{"kind":if uncertain{"outcomeUnknown"}else{"unavailable"},"message":if uncertain{"Connection failed; the instruction mutation may have committed. Inspect or replay the same operation ID."}else{"Instruction service unavailable; no mutation was dispatched."},"nextAction":if uncertain{"inspectOperation"}else{"retryLater"}}}),
                if uncertain { 5 } else { 3 },
            )
        }
    };
    let rendered = if args.json {
        serde_json::to_string(&record)
    } else {
        serde_json::to_string_pretty(&record)
    };
    match rendered {
        Ok(text) => {
            if writeln!(io::stdout(), "{text}").is_ok() {
                exit
            } else {
                5
            }
        }
        Err(_) => 5,
    }
}
fn operation(value: Option<String>) -> Result<OperationId, String> {
    value.map_or_else(
        || Ok(OperationId::generate()),
        |v| {
            v.try_into()
                .map_err(|_| "--operation-id must be a canonical lowercase UUIDv7".into())
        },
    )
}
fn prepare(command: InstructionCommand) -> Result<PreparedInstruction, String> {
    Ok(match command {
        InstructionCommand::Create {
            operation_id,
            content,
        } => PreparedInstruction::Create(InstructionCreateParams {
            operation_id: operation(operation_id)?,
            text: read_text(content)?,
        }),
        InstructionCommand::Update {
            operation_id,
            instruction_id,
            expected_revision_id,
            content,
        } => PreparedInstruction::Update(InstructionUpdateParams {
            operation_id: operation(operation_id)?,
            instruction_id: instruction_id
                .try_into()
                .map_err(|_| "--instruction-id must be UUIDv7")?,
            expected_revision_id: expected_revision_id
                .try_into()
                .map_err(|_| "--expected-revision-id must be UUIDv7")?,
            text: read_text(content)?,
        }),
        InstructionCommand::Show { instruction_id } => {
            PreparedInstruction::Show(InstructionShowParams {
                instruction_id: instruction_id
                    .try_into()
                    .map_err(|_| "--instruction-id must be UUIDv7")?,
            })
        }
    })
}
fn read_text(input: TextInput) -> Result<InstructionText, String> {
    let text = if let Some(text) = input.text {
        text
    } else {
        let path = input.text_file.ok_or("Provide --text or --text-file")?;
        let mut reader: Box<dyn Read> = if path.as_os_str() == "-" {
            Box::new(io::stdin())
        } else {
            Box::new(std::fs::File::open(path).map_err(|_| "Instruction text file unavailable")?)
        };
        let mut text = String::new();
        reader
            .by_ref()
            .take(1_048_577)
            .read_to_string(&mut text)
            .map_err(|_| "Instruction content must be readable UTF-8")?;
        text
    };
    InstructionText::try_from(text).map_err(|error| error.to_string())
}
