//! One renderer for self-declared agent context across SDK and CLI callers.
use communication_protocol::{
    MAX_CONTROL_FRAME_BYTES, MessageContent, MessageInputKind, MessageRepresentation, SessionRef,
};

pub(crate) struct RenderedMessage {
    pub text: String,
    pub kind: MessageInputKind,
    pub representation: MessageRepresentation,
}
pub(crate) fn render_message(
    target: &SessionRef,
    message: &MessageContent,
) -> Result<RenderedMessage, serde_json::Error> {
    let (text, kind, representation) = match message {
        MessageContent::Agent { sender, text } => (
            format!(
                "Agent communication\nSelf-declared sender: {}\nIntended recipient: {}\n\n{}",
                serde_json::to_string(sender)?,
                serde_json::to_string(target)?,
                text.as_str()
            ),
            MessageInputKind::Agent,
            MessageRepresentation::DeclaredAgentText,
        ),
        MessageContent::HumanUser { text } => (
            text.as_str().to_owned(),
            MessageInputKind::HumanUser,
            MessageRepresentation::HumanUserText,
        ),
    };
    if text.len() > MAX_CONTROL_FRAME_BYTES {
        return Err(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "rendered message exceeds frame budget",
        )));
    }
    Ok(RenderedMessage {
        text,
        kind,
        representation,
    })
}
