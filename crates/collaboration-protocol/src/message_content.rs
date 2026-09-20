//! Message origin and delivery are independent public choices.
use crate::{MAX_CONTROL_FRAME_BYTES, SessionRef};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MessageText(String);

#[derive(Debug, thiserror::Error)]
#[error(
    "message text must be nonempty, free of C0 controls other than newline and tab, and within the Control frame limit"
)]
pub struct MessageTextError;

impl TryFrom<String> for MessageText {
    type Error = MessageTextError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty()
            || value
                .chars()
                .any(|character| character <= '\u{001f}' && !matches!(character, '\n' | '\t'))
            || value.len() > MAX_CONTROL_FRAME_BYTES
        {
            Err(MessageTextError)
        } else {
            Ok(Self(value))
        }
    }
}
impl From<MessageText> for String {
    fn from(value: MessageText) -> Self {
        value.0
    }
}
impl MessageText {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl JsonSchema for MessageText {
    fn schema_name() -> Cow<'static, str> {
        "MessageText".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"type":"string","minLength":1,"maxLength":1048576,
            "pattern":"^[^\\u0000-\\u0008\\u000B\\u000C\\u000E-\\u001F]+$","x-maxUtf8Bytes":1048576})
    }
}

#[derive(JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum MessageContent {
    Agent {
        sender: SessionRef,
        text: MessageText,
    },
    HumanUser {
        text: MessageText,
    },
    /// Router's own delivery to a session, authored by the service rather than
    /// by any agent. It names no sender because none exists.
    Router {
        text: MessageText,
    },
}

#[derive(JsonSchema, Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MessageDelivery {
    #[default]
    Auto,
    Queue,
    Steer,
}

#[derive(JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MessageInputKind {
    Agent,
    HumanUser,
}

#[derive(JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MessageRepresentation {
    DeclaredAgentText,
    HumanUserText,
}

/// Rendered public message content ready for submission to a native endpoint.
pub struct RenderedMessage {
    pub text: String,
    pub kind: MessageInputKind,
    pub representation: MessageRepresentation,
}

/// Renders one public message with its self-declared attribution exactly once.
pub fn render_message(
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
        MessageContent::Router { text } => (
            format!(
                "Router delivery\nIntended recipient: {}\n\n{}",
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

#[cfg(test)]
mod tests {
    use super::{
        MessageContent, MessageInputKind, MessageRepresentation, MessageText, render_message,
    };
    use crate::{EndpointId, EndpointRef, SessionId, SessionRef, UuidIdentity};

    #[test]
    fn message_text_allows_layout_whitespace_and_rejects_other_c0_controls() {
        assert!(MessageText::try_from("line one\n\tline two".to_owned()).is_ok());
        for control in ['\0', '\u{0008}', '\u{000b}', '\u{001f}'] {
            assert!(MessageText::try_from(format!("before{control}after")).is_err());
        }
        assert!(MessageText::try_from("before\u{007f}after".to_owned()).is_ok());
    }

    #[test]
    fn agent_rendering_declares_current_sender_and_recipient_once() {
        let session = |id: &str| SessionRef {
            endpoint: EndpointRef {
                service_id: UuidIdentity::try_from(
                    "018f47d2-24d5-7a68-b9ec-6f759c39458f".to_owned(),
                )
                .expect("valid service UUID"),
                endpoint_id: EndpointId::try_from("codex-local".to_owned())
                    .expect("valid endpoint"),
            },
            session_id: SessionId::try_from(id.to_owned()).expect("valid session"),
        };
        let sender = session("sender-thread");
        let target = session("recipient-thread");
        let rendered = render_message(
            &target,
            &MessageContent::Agent {
                sender: sender.clone(),
                text: MessageText::try_from("hello".to_owned()).expect("valid text"),
            },
        )
        .expect("rendered agent message");

        assert_eq!(rendered.kind, MessageInputKind::Agent);
        assert_eq!(
            rendered.representation,
            MessageRepresentation::DeclaredAgentText
        );
        assert_eq!(rendered.text.matches("Self-declared sender:").count(), 1);
        assert!(
            rendered
                .text
                .contains(&serde_json::to_string(&sender).expect("sender JSON"))
        );
        assert!(
            rendered
                .text
                .contains(&serde_json::to_string(&target).expect("target JSON"))
        );
        assert!(rendered.text.ends_with("\n\nhello"));
    }

    #[test]
    fn human_rendering_never_adds_agent_declarations() {
        let target = SessionRef {
            endpoint: EndpointRef {
                service_id: UuidIdentity::try_from(
                    "018f47d2-24d5-7a68-b9ec-6f759c39458f".to_owned(),
                )
                .expect("valid service UUID"),
                endpoint_id: EndpointId::try_from("codex-local".to_owned())
                    .expect("valid endpoint"),
            },
            session_id: SessionId::try_from("recipient-thread".to_owned()).expect("valid session"),
        };
        let rendered = render_message(
            &target,
            &MessageContent::HumanUser {
                text: MessageText::try_from("hello".to_owned()).expect("valid text"),
            },
        )
        .expect("rendered human message");

        assert_eq!(rendered.kind, MessageInputKind::HumanUser);
        assert_eq!(
            rendered.representation,
            MessageRepresentation::HumanUserText
        );
        assert_eq!(rendered.text, "hello");
    }
}

#[derive(JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AcceptedResumeEffect {
    NotRequested,
    Accepted,
}
