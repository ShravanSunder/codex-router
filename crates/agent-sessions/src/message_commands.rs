//! Descriptive message submission through the public Rust client.
use clap::{Args, Parser, Subcommand, ValueEnum};
use communication_client::{ClientError, ControlClient};
use communication_protocol::{
    ChannelDescription, MessageContent, MessageDelivery, NativeSendParams, NativeSendReceipt,
    SessionRef,
};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::{self, Read, Write},
    path::PathBuf,
};

#[derive(Parser)]
#[command(name = "agent-sessions message", bin_name = "agent-sessions message")]
struct MessageArguments {
    #[command(subcommand)]
    command: MessageCommand,
}
#[derive(Subcommand)]
enum MessageCommand {
    /// Submit information. Acceptance is not completion or a peer reply.
    Send(SendArguments),
}
#[derive(Clone, Copy, ValueEnum)]
enum DeliveryChoice {
    Auto,
    Queue,
    Steer,
}
#[derive(Args)]
struct SendArguments {
    /// Exact recipient as compact SessionRef JSON from discovery.
    #[arg(long)]
    to: String,
    /// Self-declared sender SessionRef JSON; not authenticated identity.
    #[arg(
        long = "from",
        required_unless_present = "human_user",
        conflicts_with = "human_user"
    )]
    sender: Option<String>,
    /// Explicit human input; omit the agent declaration.
    #[arg(long)]
    human_user: bool,
    /// Auto steers active work or starts/resumes. Queue requires loaded; steer requires active.
    #[arg(long, value_enum, default_value = "auto")]
    delivery: DeliveryChoice,
    #[arg(
        long,
        required_unless_present = "text_file",
        conflicts_with = "text_file"
    )]
    text: Option<String>,
    /// Read content from a file; '-' reads stdin. Content is never shell-interpolated.
    #[arg(long)]
    text_file: Option<PathBuf>,
    #[arg(long)]
    service_directory: Option<PathBuf>,
    #[arg(long, requires = "expected_generation")]
    expected_service_epoch: Option<String>,
    #[arg(long, requires = "expected_service_epoch")]
    expected_generation: Option<u64>,
    #[arg(long)]
    json: bool,
}

pub fn run_message_command(arguments: Vec<OsString>) -> i32 {
    let parsed = match MessageArguments::try_parse_from(arguments) {
        Ok(value) => value,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _printed = error.print();
            return code;
        }
    };
    let MessageCommand::Send(args) = parsed.command;
    let machine = args.json;
    let prepared = prepare(&args);
    let (directory, target, content) = match prepared {
        Ok(value) => value,
        Err(message) => {
            return crate::endpoint_commands::report_failure("invalidUsage", &message, 2, machine);
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(value) => value,
        Err(_) => {
            return crate::endpoint_commands::report_failure(
                "unavailable",
                "Client runtime unavailable",
                3,
                machine,
            );
        }
    };
    let mut submitted = false;
    let result = runtime.block_on(async {
        let mut client =
            ControlClient::connect(&directory, "agent-sessions", env!("CARGO_PKG_VERSION")).await?;
        let inventory = client.list_endpoints().await?;
        let generation = inventory
            .endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint == target.endpoint)
            .and_then(|endpoint| {
                endpoint.channels.iter().find_map(|channel| match channel {
                    ChannelDescription::NativeCodex {
                        generation: Some(generation),
                        ..
                    } => Some(generation.clone()),
                    _ => None,
                })
            })
            .ok_or(ClientError::Rejected {
                code: -32050,
                data: Some(json!({"kind":"unavailable","stage":"discovery"})),
            })?;
        let generation = if let (Some(epoch), Some(number)) =
            (&args.expected_service_epoch, args.expected_generation)
        {
            let expected =
                serde_json::from_value(json!({"serviceEpoch":epoch,"generation":number}))
                    .map_err(|_| ClientError::Protocol("invalid expected generation"))?;
            if expected != generation {
                return Err(ClientError::Rejected {
                    code: -32050,
                    data: Some(json!({"kind":"staleGeneration","stage":"discovery"})),
                });
            }
            expected
        } else {
            generation
        };
        let params = NativeSendParams {
            target,
            generation,
            message: content,
            delivery: match args.delivery {
                DeliveryChoice::Auto => MessageDelivery::Auto,
                DeliveryChoice::Queue => MessageDelivery::Queue,
                DeliveryChoice::Steer => MessageDelivery::Steer,
            },
            client_user_message_id: None,
        };
        submitted = true;
        let result = if args.human_user {
            client.send_human_input(params).await
        } else {
            client.send_agent_message(params).await
        };
        let _closed = client.close().await;
        result
    });
    report(result, machine, submitted)
}

fn prepare(args: &SendArguments) -> Result<(PathBuf, SessionRef, MessageContent), String> {
    let directory = crate::endpoint_commands::resolve_directory(args.service_directory.clone())?;
    let target = serde_json::from_str(&args.to).map_err(|_| "Invalid recipient SessionRef JSON")?;
    if let Some(epoch) = &args.expected_service_epoch {
        let _: communication_protocol::CodexGeneration = serde_json::from_value(
            json!({"serviceEpoch":epoch,"generation":args.expected_generation}),
        )
        .map_err(|_| "Invalid expected generation")?;
    }
    let text = if let Some(text) = &args.text {
        text.clone()
    } else {
        let path = args.text_file.as_ref().ok_or("Message content required")?;
        let mut reader: Box<dyn Read> = if path.as_os_str() == "-" {
            Box::new(io::stdin())
        } else {
            Box::new(std::fs::File::open(path).map_err(|_| "Message file unavailable")?)
        };
        let mut text = String::new();
        reader
            .by_ref()
            .take((communication_protocol::MAX_CONTROL_FRAME_BYTES + 1) as u64)
            .read_to_string(&mut text)
            .map_err(|_| "Cannot read UTF-8 message")?;
        text
    };
    let text = text
        .try_into()
        .map_err(|_| "Invalid or oversized message text")?;
    let content = if args.human_user {
        MessageContent::HumanUser { text }
    } else {
        let sender = serde_json::from_str(
            args.sender
                .as_deref()
                .ok_or("Self-declared sender required")?,
        )
        .map_err(|_| "Invalid sender SessionRef JSON")?;
        MessageContent::Agent { sender, text }
    };
    Ok((directory, target, content))
}
fn report(result: Result<NativeSendReceipt, ClientError>, machine: bool, submitted: bool) -> i32 {
    let (record, code) = match result {
        Ok(receipt) => (json!({"kind":"result","result":receipt}), 0),
        Err(ClientError::Rejected { code, data }) => {
            let kind = data
                .as_ref()
                .and_then(|d| d.get("kind"))
                .and_then(Value::as_str);
            let exit = match kind {
                Some("outcomeUnknown") => 5,
                Some("unsupportedCapability") => 2,
                Some("unavailable") => 3,
                _ => 4,
            };
            (
                json!({"kind":"error","error":{"code":code,"data":data}}),
                exit,
            )
        }
        Err(error) => (
            json!({"kind":"error","error":{"kind":if submitted {"outcomeUnknown"} else {"unavailable"},"message":safe_connection_failure(&error)}}),
            if submitted { 5 } else { 3 },
        ),
    };
    let written = if machine {
        writeln!(io::stdout(), "{record}")
    } else {
        writeln!(
            io::stdout(),
            "{}",
            serde_json::to_string_pretty(&record)
                .unwrap_or_else(|_| "Output unavailable".to_owned())
        )
    };
    if written.is_err() {
        if submitted { 5 } else { 3 }
    } else {
        code
    }
}

fn safe_connection_failure(error: &ClientError) -> String {
    let category = match error {
        ClientError::Discovery { stage, source } => format!(
            "{stage}: IO {:?} (OS code {:?})",
            source.kind(),
            source.raw_os_error()
        ),
        ClientError::Transport(error) => {
            format!("IO {:?} (OS code {:?})", error.kind(), error.raw_os_error())
        }
        ClientError::Timeout => "request deadline".to_owned(),
        ClientError::Protocol(_) => "protocol validation".to_owned(),
        ClientError::UnsupportedCapability(_) => "unsupported capability".to_owned(),
        ClientError::Rejected { .. } => "server rejection".to_owned(),
    };
    format!("Connection failed: {category}; no message replayed")
}
