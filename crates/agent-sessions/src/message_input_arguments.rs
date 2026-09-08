//! Shared input options keep immediate messages and timed wake-ups semantically identical.
use clap::{Args, ValueEnum};
use communication_protocol::{MessageContent, MessageDelivery, SavedMessage, SessionRef};
use serde_json::json;
use std::{
    io::{self, Read},
    path::PathBuf,
};
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum DeliveryChoice {
    Auto,
    Queue,
    Steer,
}
#[derive(Args)]
pub(crate) struct SendArguments {
    /// Exact recipient as compact SessionRef JSON from discovery.
    #[arg(long)]
    pub(crate) to: String,
    /// Self-declared sender SessionRef JSON; not authenticated identity.
    #[arg(
        long = "from",
        required_unless_present = "human_user",
        conflicts_with = "human_user"
    )]
    pub(crate) sender: Option<String>,
    /// Explicit human input; omit the agent declaration.
    #[arg(long)]
    pub(crate) human_user: bool,
    /// Auto steers active work or starts/resumes. Queue requires loaded; steer requires active.
    #[arg(long, value_enum, default_value = "auto")]
    pub(crate) delivery: DeliveryChoice,
    #[arg(
        long,
        required_unless_present = "text_file",
        conflicts_with = "text_file"
    )]
    pub(crate) text: Option<String>,
    /// Read content from a file; '-' reads stdin. Content is never shell-interpolated.
    #[arg(long)]
    pub(crate) text_file: Option<PathBuf>,
    #[arg(long)]
    pub(crate) service_directory: Option<PathBuf>,
    #[arg(long, requires = "expected_generation")]
    pub(crate) expected_service_epoch: Option<String>,
    #[arg(long, requires = "expected_service_epoch")]
    pub(crate) expected_generation: Option<u64>,
    #[arg(long)]
    pub(crate) json: bool,
}

pub(crate) fn prepare(
    args: &SendArguments,
) -> Result<(PathBuf, SessionRef, MessageContent), String> {
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

pub(crate) fn saved_message(args: &SendArguments) -> Result<(PathBuf, SavedMessage), String> {
    let (directory, target, content) = prepare(args)?;
    let generation_guard = match (&args.expected_service_epoch, args.expected_generation) {
        (Some(epoch), Some(generation)) => Some(
            serde_json::from_value(json!({"serviceEpoch":epoch,"generation":generation}))
                .map_err(|_| "Invalid expected generation")?,
        ),
        (None, None) => None,
        _ => return Err("Provide both expected service epoch and generation".into()),
    };
    Ok((
        directory,
        SavedMessage {
            target,
            content,
            delivery: match args.delivery {
                DeliveryChoice::Auto => MessageDelivery::Auto,
                DeliveryChoice::Queue => MessageDelivery::Queue,
                DeliveryChoice::Steer => MessageDelivery::Steer,
            },
            generation_guard,
        },
    ))
}
