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
    "message text must be nonempty, free of C0 controls other than newline and tab, and within the message size limit"
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SessionDisplayName(String);

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("session display name must be 1 to 120 characters, contain no controls or header arrows")]
pub struct SessionDisplayNameError;

const MAX_SESSION_DISPLAY_NAME_CHARS: usize = 120;

impl TryFrom<String> for SessionDisplayName {
    type Error = SessionDisplayNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.trim().is_empty()
            || value.chars().count() > MAX_SESSION_DISPLAY_NAME_CHARS
            || value.chars().any(char::is_control)
            || value.contains(" ← ")
            || value.contains(" → ")
        {
            return Err(SessionDisplayNameError);
        }
        Ok(Self(value))
    }
}

impl SessionDisplayName {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl JsonSchema for SessionDisplayName {
    fn schema_name() -> Cow<'static, str> {
        "SessionDisplayName".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type":"string",
            "minLength":1,
            "maxLength":120,
            "pattern":"^[^\\u0000-\\u001F]+$"
        })
    }
}

impl From<SessionDisplayName> for String {
    fn from(value: SessionDisplayName) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SessionDisplayNameLookupError {
    #[error("session display name lookup is unavailable")]
    Unavailable,
}

/// A best-effort, nonblocking lookup for labels the Router already knows.
///
/// Implementations must use a cache or another nonblocking source. Returning an
/// error, or no name, selects the endpoint's stable fallback identity instead.
pub trait SessionDisplayNameLookup: Send + Sync {
    fn display_name_for(
        &self,
        session: &SessionRef,
    ) -> Result<Option<SessionDisplayName>, SessionDisplayNameLookupError>;
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

/// Formats one session as the emoji-prefixed identity used in message headers and reply output.
pub fn session_identity(session: &SessionRef, display_name: Option<&SessionDisplayName>) -> String {
    let endpoint_id = String::from(session.endpoint.endpoint_id.clone());
    let fallback_emoji = endpoint_default_emoji(&endpoint_id);
    match display_name {
        Some(name) => match leading_emoji(name.as_str()) {
            Some((emoji, label)) if !label.is_empty() => format!("{emoji} {label}"),
            Some((emoji, _)) => format!("{emoji} {}", fallback_session_label(session)),
            None => format!("{fallback_emoji} {}", name.as_str()),
        },
        None => format!("{fallback_emoji} {}", fallback_session_label(session)),
    }
}

fn fallback_session_label(session: &SessionRef) -> String {
    let endpoint_id = String::from(session.endpoint.endpoint_id.clone());
    let session_id = String::from(session.session_id.clone());
    let short_session_id: String = session_id.chars().take(8).collect();
    format!("{endpoint_id}/{short_session_id}")
}

fn endpoint_default_emoji(endpoint_id: &str) -> &'static str {
    match endpoint_id {
        "codex" | "codex-local" => "🤖",
        "claude" | "claude-local" => "✳️",
        "cursor" | "cursor-local" => "▶️",
        _ => "💬",
    }
}

fn leading_emoji(name: &str) -> Option<(&str, &str)> {
    let mut characters = name.char_indices();
    let (_, first) = characters.next()?;
    if !is_emoji_codepoint(first) {
        return None;
    }

    let mut emoji_end = first.len_utf8();
    let mut join_next = false;
    for (index, character) in characters {
        if join_next {
            if !is_emoji_codepoint(character) {
                break;
            }
            emoji_end = index + character.len_utf8();
            join_next = false;
            continue;
        }
        if is_emoji_extension(character) {
            emoji_end = index + character.len_utf8();
        } else if character == '\u{200d}' {
            emoji_end = index + character.len_utf8();
            join_next = true;
        } else {
            break;
        }
    }

    let emoji = name.get(..emoji_end)?;
    let label = name.get(emoji_end..)?.trim_start();
    Some((emoji, label))
}

fn is_emoji_extension(character: char) -> bool {
    matches!(
        character as u32,
        0xfe0e..=0xfe0f | 0x1f3fb..=0x1f3ff | 0xe0020..=0xe007f
    )
}

fn is_emoji_codepoint(character: char) -> bool {
    matches!(
        character,
        '©' | '®' | '‼' | '⁉' | '™' | 'ℹ' | '↔'..='↙' | '↩'..='↪' | '⌚'..='⌛'
            | '⌨' | '⏏' | '⏩'..='⏺' | 'Ⓜ' | '▪'..='▶' | '◀' | '◻'..='◾' | '☀'..='☄'
            | '☎'..='♟' | '♠'..='♿' | '⚀'..='⚿' | '⛀'..='⛿' | '✀'..='➿' | '⬀'..='⬛'
            | '⬜'..='⬟' | '⭐' | '⭕' | '〰' | '〽' | '㊗' | '㊙' | '🀀'..='🫿'
    )
}

#[cfg(test)]
mod tests {
    use super::{MessageText, SessionDisplayName};

    #[test]
    fn display_names_reject_controls_newlines_and_header_arrows() {
        for invalid_name in [
            "",
            "First\nSecond",
            "First\u{007f}Second",
            "Name ← forged",
            "Name → forged",
        ] {
            assert!(
                SessionDisplayName::try_from(invalid_name.to_owned()).is_err(),
                "display name should not accept {invalid_name:?}"
            );
        }
        assert!(SessionDisplayName::try_from("🐒 Sidekick · PR2".to_owned()).is_ok());
    }

    #[test]
    fn display_names_are_bounded_to_120_unicode_scalar_values() {
        let maximum_name = "🪿".repeat(120);
        let oversized_name = "🪿".repeat(121);

        assert!(SessionDisplayName::try_from(maximum_name).is_ok());
        assert!(SessionDisplayName::try_from(oversized_name).is_err());
    }

    #[test]
    fn message_text_allows_layout_whitespace_and_rejects_other_c0_controls() {
        assert!(MessageText::try_from("line one\n\tline two".to_owned()).is_ok());
        for control in ['\0', '\u{0008}', '\u{000b}', '\u{001f}'] {
            assert!(MessageText::try_from(format!("before{control}after")).is_err());
        }
        assert!(MessageText::try_from("before\u{007f}after".to_owned()).is_ok());
    }
}

#[derive(JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AcceptedResumeEffect {
    NotRequested,
    Accepted,
}
