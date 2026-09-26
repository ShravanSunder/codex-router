//! Shared input options keep immediate messages and timed wake-ups semantically identical.
use clap::{Args, ValueEnum};
use collaboration_client::protocol::{
    MessageContent, MessageDelivery, MessageText, SavedMessage, SessionRef, UuidIdentity,
};
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
    #[command(flatten)]
    pub(crate) target: crate::session_target_arguments::SessionTargetArguments,
    /// Use the supplied self address as SessionRef JSON; this is not authenticated identity.
    #[arg(long = "from", conflicts_with = "human_user")]
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

pub(crate) fn prepare(args: &SendArguments) -> Result<(PathBuf, PreparedMessage), String> {
    let directory = crate::endpoint_commands::resolve_directory(args.service_directory.clone())?;
    let target = args.target.parse()?;
    if let Some(epoch) = &args.expected_service_epoch {
        let _: collaboration_client::protocol::CodexGeneration = serde_json::from_value(
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
            .take((collaboration_client::protocol::MAX_CONTROL_FRAME_BYTES + 1) as u64)
            .read_to_string(&mut text)
            .map_err(|_| "Cannot read UTF-8 message")?;
        text
    };
    let text = text
        .try_into()
        .map_err(|_| "Invalid or oversized message text")?;
    let content = if args.human_user {
        PreparedMessageContent::HumanUser { text }
    } else {
        let sender = args
            .sender
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|_| session_ref_guidance("--from"))?;
        if sender.is_none() {
            crate::current_session_identity::read_harness_session_identity().map_err(|error| {
                format!(
                    "{error}; run agent-collaboration whoami --json or pass --from SessionRef JSON"
                )
            })?;
        }
        PreparedMessageContent::Agent { sender, text }
    };
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
        PreparedMessage {
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

pub(crate) fn session_ref_guidance(field: &str) -> String {
    format!(
        "{field} must be compact SessionRef JSON with endpoint.serviceId, endpoint.endpointId, and sessionId. nativeThreadId is a lifecycle address, not a SessionRef. Copy .target from sessions list --json."
    )
}

pub(crate) struct PreparedMessage {
    pub(crate) target: crate::session_target_arguments::ParsedSessionTarget,
    content: PreparedMessageContent,
    pub(crate) delivery: MessageDelivery,
    pub(crate) generation_guard: Option<collaboration_client::protocol::CodexGeneration>,
}
enum PreparedMessageContent {
    HumanUser {
        text: MessageText,
    },
    Agent {
        sender: Option<SessionRef>,
        text: MessageText,
    },
}
impl PreparedMessage {
    pub(crate) fn resolve(self, service_id: &UuidIdentity) -> Result<SavedMessage, String> {
        let content = match self.content {
            PreparedMessageContent::HumanUser { text } => MessageContent::HumanUser { text },
            PreparedMessageContent::Agent { sender, text } => {
                let sender = resolve_sender_ref(
                    service_id,
                    sender,
                    crate::current_session_identity::read_harness_session_identity,
                )?;
                MessageContent::Agent { sender, text }
            }
        };
        Ok(SavedMessage {
            target: self.target.resolve(service_id)?,
            content,
            delivery: self.delivery,
            generation_guard: self.generation_guard,
        })
    }
}

fn resolve_sender_ref(
    service_id: &UuidIdentity,
    explicit: Option<SessionRef>,
    read_harness: impl FnOnce() -> Result<
        crate::current_session_identity::HarnessSessionIdentity,
        crate::current_session_identity::CurrentSessionIdentityError,
    >,
) -> Result<SessionRef, String> {
    if let Some(sender) = explicit {
        return Ok(sender);
    }
    read_harness()
        .map_err(|error| {
            format!("{error}; run agent-collaboration whoami --json or pass --from SessionRef JSON")
        })?
        .session_ref(service_id)
        .map_err(|error| {
            format!("{error}; run agent-collaboration whoami --json or pass --from SessionRef JSON")
        })
}

#[cfg(test)]
mod tests {
    use super::resolve_sender_ref;
    use collaboration_client::protocol::UuidIdentity;

    #[test]
    fn sender_uses_harness_identity_and_fails_with_whoami_guidance_when_absent() {
        let service_id = UuidIdentity::try_from("00000000-0000-4000-8000-000000000001".to_owned())
            .expect("service identity");
        let resolved = resolve_sender_ref(&service_id, None, || {
            crate::current_session_identity::resolve_harness_session_identity(|name| {
                (name == "CURSOR_CONVERSATION_ID").then(|| "cursor-1".into())
            })
        })
        .expect("harness identity");
        assert_eq!(String::from(resolved.endpoint.endpoint_id), "cursor-local");
        assert_eq!(String::from(resolved.session_id), "cursor-1");

        let error = resolve_sender_ref(&service_id, None, || {
            crate::current_session_identity::resolve_harness_session_identity(|_| None)
        })
        .expect_err("sender must fail closed");
        assert!(error.contains("agent-collaboration whoami --json"));
        assert!(error.contains("--from SessionRef JSON"));
    }
}
