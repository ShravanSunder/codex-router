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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionDisplayName(String);

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("session display name must be nonempty and contain no controls or header arrows")]
pub struct SessionDisplayNameError;

impl TryFrom<String> for SessionDisplayName {
    type Error = SessionDisplayNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.trim().is_empty()
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RouterNoticeKind {
    Wake,
    Schedule,
    BoardListen,
    #[default]
    Other,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MessageHeaderOrigin {
    #[default]
    Agent,
    RouterNotice(RouterNoticeKind),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MessageHeaderContext {
    pub sender_display_name: Option<SessionDisplayName>,
    pub recipient_display_name: Option<SessionDisplayName>,
    pub origin: MessageHeaderOrigin,
}

impl MessageHeaderContext {
    #[must_use]
    pub fn resolve(
        target: &SessionRef,
        message: &MessageContent,
        display_names: &dyn SessionDisplayNameLookup,
        origin: MessageHeaderOrigin,
    ) -> Self {
        let sender_display_name = match message {
            MessageContent::Agent { sender, .. } => {
                display_names.display_name_for(sender).ok().flatten()
            }
            MessageContent::HumanUser { .. } | MessageContent::Router { .. } => None,
        };
        let recipient_display_name = match message {
            MessageContent::Agent { .. }
            | MessageContent::HumanUser { .. }
            | MessageContent::Router { .. } => {
                display_names.display_name_for(target).ok().flatten()
            }
        };
        Self {
            sender_display_name,
            recipient_display_name,
            origin,
        }
    }
}

struct NoSessionDisplayNames;

impl SessionDisplayNameLookup for NoSessionDisplayNames {
    fn display_name_for(
        &self,
        _: &SessionRef,
    ) -> Result<Option<SessionDisplayName>, SessionDisplayNameLookupError> {
        Ok(None)
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedAgentMessageEnvelope {
    pub sender: SessionRef,
    pub recipient: SessionRef,
    pub sender_identity: String,
    pub body: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedRouterMessageEnvelope {
    pub recipient: SessionRef,
    pub router_identity: String,
    pub body: String,
}

/// Renders one public message with its self-declared attribution exactly once.
pub fn render_message(
    target: &SessionRef,
    message: &MessageContent,
) -> Result<RenderedMessage, serde_json::Error> {
    render_message_with_lookup(
        target,
        message,
        &NoSessionDisplayNames,
        RouterNoticeKind::Other,
    )
}

/// Renders a message with cached display names when they are available.
///
/// The lookup is deliberately synchronous: its contract is nonblocking and its
/// failure is a formatting fallback, never a delivery failure.
pub fn render_message_with_lookup(
    target: &SessionRef,
    message: &MessageContent,
    display_names: &dyn SessionDisplayNameLookup,
    router_notice_kind: RouterNoticeKind,
) -> Result<RenderedMessage, serde_json::Error> {
    let origin = match message {
        MessageContent::Router { .. } => MessageHeaderOrigin::RouterNotice(router_notice_kind),
        MessageContent::Agent { .. } | MessageContent::HumanUser { .. } => {
            MessageHeaderOrigin::Agent
        }
    };
    let header_context = MessageHeaderContext::resolve(target, message, display_names, origin);
    render_message_with_context(target, message, &header_context)
}

/// Renders a message with names already resolved by the service boundary.
pub fn render_message_with_context(
    target: &SessionRef,
    message: &MessageContent,
    header_context: &MessageHeaderContext,
) -> Result<RenderedMessage, serde_json::Error> {
    let (text, kind, representation) = match message {
        MessageContent::Agent { sender, text } => (
            format!(
                "{} ← {}\nAgent communication\nSelf-declared sender: {}\nIntended recipient: {}\n\n{}",
                session_identity(target, header_context.recipient_display_name.as_ref()),
                agent_header_sender(sender, header_context),
                serde_json::to_string(sender)?,
                serde_json::to_string(target)?,
                text.as_str()
            ),
            MessageInputKind::Agent,
            MessageRepresentation::DeclaredAgentText,
        ),
        MessageContent::Router { text } => (
            format!(
                "{} ← {}\nRouter delivery\nIntended recipient: {}\n\n{}",
                session_identity(target, header_context.recipient_display_name.as_ref()),
                router_notice_identity(match header_context.origin {
                    MessageHeaderOrigin::RouterNotice(kind) => kind,
                    MessageHeaderOrigin::Agent => RouterNoticeKind::Other,
                }),
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

fn agent_header_sender(sender: &SessionRef, context: &MessageHeaderContext) -> String {
    match context.origin {
        MessageHeaderOrigin::Agent => {
            session_identity(sender, context.sender_display_name.as_ref())
        }
        MessageHeaderOrigin::RouterNotice(kind) => router_notice_identity(kind).to_owned(),
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

fn router_notice_identity(kind: RouterNoticeKind) -> &'static str {
    match kind {
        RouterNoticeKind::Wake => "⏰ Router wake",
        RouterNoticeKind::Schedule => "⏰ Router schedule",
        RouterNoticeKind::BoardListen => "📋 Router board listen",
        RouterNoticeKind::Other => "🔔 Router notice",
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

/// Parses an old or current Router agent-message envelope without interpreting its body.
pub fn parse_agent_message_envelope(text: &str) -> Option<ParsedAgentMessageEnvelope> {
    const AGENT_COMMUNICATION_MARKER: &str = "Agent communication\n";
    let (header_identity, envelope_body) =
        if let Some(envelope_body) = text.strip_prefix(AGENT_COMMUNICATION_MARKER) {
            (None, envelope_body)
        } else {
            let (header_line, envelope) = text.split_once('\n')?;
            let (_, sender_identity) = header_line.rsplit_once(" ← ")?;
            let (emoji, label) = leading_emoji(sender_identity)?;
            if label.is_empty() {
                return None;
            }
            let envelope_body = envelope.strip_prefix(AGENT_COMMUNICATION_MARKER)?;
            (Some(format!("{emoji} {label}")), envelope_body)
        };

    let (declarations, body) = envelope_body.split_once("\n\n")?;
    let mut declaration_lines = declarations.lines();
    let sender_json = declaration_lines
        .next()?
        .strip_prefix("Self-declared sender: ")?;
    let recipient_json = declaration_lines
        .next()?
        .strip_prefix("Intended recipient: ")?;
    if declaration_lines.next().is_some() {
        return None;
    }
    let sender: SessionRef = serde_json::from_str(sender_json).ok()?;
    let recipient: SessionRef = serde_json::from_str(recipient_json).ok()?;
    let first_body_line = body.lines().next()?.trim();
    if first_body_line.is_empty() {
        return None;
    }
    let sender_identity = header_identity.unwrap_or_else(|| session_identity(&sender, None));

    Some(ParsedAgentMessageEnvelope {
        sender,
        recipient,
        sender_identity,
        body: body.to_owned(),
    })
}

/// Parses an old or current Router-delivery envelope without interpreting its body.
pub fn parse_router_message_envelope(text: &str) -> Option<ParsedRouterMessageEnvelope> {
    const ROUTER_DELIVERY_MARKER: &str = "Router delivery\n";
    let (router_identity, envelope_body) =
        if let Some(envelope_body) = text.strip_prefix(ROUTER_DELIVERY_MARKER) {
            ("🔔 Router notice".to_owned(), envelope_body)
        } else {
            let (header_line, envelope) = text.split_once('\n')?;
            let (recipient_identity, router_identity) = header_line.rsplit_once(" ← ")?;
            let (recipient_emoji, recipient_label) = leading_emoji(recipient_identity)?;
            let (router_emoji, router_label) = leading_emoji(router_identity)?;
            if recipient_label.is_empty()
                || !router_label.starts_with("Router ")
                || recipient_emoji.is_empty()
                || router_emoji.is_empty()
            {
                return None;
            }
            (
                format!("{router_emoji} {router_label}"),
                envelope.strip_prefix(ROUTER_DELIVERY_MARKER)?,
            )
        };

    let (declaration, body) = envelope_body.split_once("\n\n")?;
    let recipient_json = declaration.strip_prefix("Intended recipient: ")?;
    let recipient: SessionRef = serde_json::from_str(recipient_json).ok()?;
    Some(ParsedRouterMessageEnvelope {
        recipient,
        router_identity,
        body: body.to_owned(),
    })
}

/// Checks the protocol-declared identity and body in a queued rendered message.
///
/// Display labels are intentionally ignored: they can be refreshed between the
/// original submission and read-only reconciliation, while the correlation ID,
/// declared identities, and body remain stable.
pub fn queued_message_matches_content(
    target: &SessionRef,
    message: &MessageContent,
    queued_text: &str,
) -> bool {
    match message {
        MessageContent::Agent { sender, text } => parse_agent_message_envelope(queued_text)
            .is_some_and(|envelope| {
                envelope.sender == *sender
                    && envelope.recipient == *target
                    && envelope.body == text.as_str()
            }),
        MessageContent::Router { text } => {
            parse_router_message_envelope(queued_text).is_some_and(|envelope| {
                envelope.recipient == *target && envelope.body == text.as_str()
            })
        }
        MessageContent::HumanUser { text } => queued_text == text.as_str(),
    }
}

/// Builds a concise picker title for a valid Agent or Router delivery envelope.
pub fn title_from_agent_message_envelope(text: &str) -> Option<String> {
    if let Some(envelope) = parse_agent_message_envelope(text) {
        let first_body_line = envelope.body.lines().next()?.trim();
        if first_body_line.is_empty() {
            return None;
        }
        return Some(format!("{}: {first_body_line}", envelope.sender_identity));
    }
    let envelope = parse_router_message_envelope(text)?;
    let first_body_line = envelope.body.lines().next()?.trim();
    if first_body_line.is_empty() {
        return None;
    }
    Some(format!("{}: {first_body_line}", envelope.router_identity))
}

#[cfg(test)]
mod tests {
    use super::{
        MessageContent, MessageInputKind, MessageRepresentation, MessageText, RouterNoticeKind,
        SessionDisplayName, SessionDisplayNameLookup, SessionDisplayNameLookupError,
        parse_agent_message_envelope, parse_router_message_envelope,
        queued_message_matches_content, render_message, render_message_with_lookup,
        title_from_agent_message_envelope,
    };
    use crate::{EndpointId, EndpointRef, SessionId, SessionRef, UuidIdentity};
    use std::collections::HashMap;

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

    struct MapDisplayNames(
        HashMap<String, Result<Option<SessionDisplayName>, SessionDisplayNameLookupError>>,
    );

    impl SessionDisplayNameLookup for MapDisplayNames {
        fn display_name_for(
            &self,
            session: &SessionRef,
        ) -> Result<Option<SessionDisplayName>, SessionDisplayNameLookupError> {
            self.0
                .get(&String::from(session.session_id.clone()))
                .cloned()
                .unwrap_or(Ok(None))
        }
    }

    struct FailedDisplayNameLookup;

    impl SessionDisplayNameLookup for FailedDisplayNameLookup {
        fn display_name_for(
            &self,
            _: &SessionRef,
        ) -> Result<Option<SessionDisplayName>, SessionDisplayNameLookupError> {
            Err(SessionDisplayNameLookupError::Unavailable)
        }
    }

    fn session(endpoint_id: &str, session_id: &str) -> SessionRef {
        SessionRef {
            endpoint: EndpointRef {
                service_id: UuidIdentity::try_from(
                    "018f47d2-24d5-7a68-b9ec-6f759c39458f".to_owned(),
                )
                .expect("valid service UUID"),
                endpoint_id: EndpointId::try_from(endpoint_id.to_owned()).expect("valid endpoint"),
            },
            session_id: SessionId::try_from(session_id.to_owned()).expect("valid session"),
        }
    }

    fn agent_message(sender: SessionRef) -> MessageContent {
        MessageContent::Agent {
            sender,
            text: MessageText::try_from("hello".to_owned()).expect("valid text"),
        }
    }

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
    fn named_emoji_identity_line_puts_recipient_first_and_drops_duplicate_emojis() {
        let sender = session("codex-local", "sender-thread");
        let target = session("claude-local", "recipient-thread");
        let lookup = MapDisplayNames(HashMap::from([
            (
                "sender-thread".to_owned(),
                Ok(Some(
                    SessionDisplayName::try_from("🐒 Sidekick · PR2".to_owned())
                        .expect("sender name"),
                )),
            ),
            (
                "recipient-thread".to_owned(),
                Ok(Some(
                    SessionDisplayName::try_from("✳️ codex-router Main".to_owned())
                        .expect("recipient name"),
                )),
            ),
        ]));

        let rendered = render_message_with_lookup(
            &target,
            &agent_message(sender),
            &lookup,
            RouterNoticeKind::Other,
        )
        .expect("rendered agent message");

        assert!(
            rendered
                .text
                .starts_with("✳️ codex-router Main ← 🐒 Sidekick · PR2\n")
        );
    }

    #[test]
    fn named_identity_without_emoji_uses_endpoint_defaults_and_keeps_names() {
        let sender = session("claude-local", "sender-thread");
        let target = session("codex-local", "recipient-thread");
        let lookup = MapDisplayNames(HashMap::from([
            (
                "sender-thread".to_owned(),
                Ok(Some(
                    SessionDisplayName::try_from("Review Sidekick".to_owned())
                        .expect("sender name"),
                )),
            ),
            (
                "recipient-thread".to_owned(),
                Ok(Some(
                    SessionDisplayName::try_from("Codex Main".to_owned()).expect("recipient name"),
                )),
            ),
        ]));

        let rendered = render_message_with_lookup(
            &target,
            &agent_message(sender),
            &lookup,
            RouterNoticeKind::Other,
        )
        .expect("rendered agent message");

        assert!(
            rendered
                .text
                .starts_with("🤖 Codex Main ← ✳️ Review Sidekick\n")
        );
    }

    #[test]
    fn unnamed_identity_uses_endpoint_default_emojis_and_short_session_labels() {
        let endpoint_defaults = [
            ("codex-local", "🤖"),
            ("claude-local", "✳️"),
            ("cursor-local", "▶️"),
            ("other-local", "💬"),
        ];

        for (endpoint_id, emoji) in endpoint_defaults {
            let sender = session(endpoint_id, "sender123456");
            let target = session(endpoint_id, "target123456");
            let rendered = render_message_with_lookup(
                &target,
                &agent_message(sender),
                &MapDisplayNames(HashMap::new()),
                RouterNoticeKind::Other,
            )
            .expect("rendered agent message");

            assert!(rendered.text.starts_with(&format!(
                "{emoji} {endpoint_id}/target12 ← {emoji} {endpoint_id}/sender12\n"
            )));
        }
    }

    #[test]
    fn router_delivery_identity_line_identifies_each_notice_kind() {
        let target = session("codex-local", "recipient-thread");
        let message = MessageContent::Router {
            text: MessageText::try_from("notice".to_owned()).expect("valid text"),
        };
        let lookup = MapDisplayNames(HashMap::new());
        let notice_kinds = [
            (RouterNoticeKind::Wake, "⏰ Router wake"),
            (RouterNoticeKind::Schedule, "⏰ Router schedule"),
            (RouterNoticeKind::BoardListen, "📋 Router board listen"),
            (RouterNoticeKind::Other, "🔔 Router notice"),
        ];

        for (kind, sender_label) in notice_kinds {
            let rendered = render_message_with_lookup(&target, &message, &lookup, kind)
                .expect("rendered Router delivery");
            assert!(rendered.text.starts_with(&format!(
                "🤖 codex-local/recipien ← {sender_label}\nRouter delivery\n"
            )));
        }
    }

    #[test]
    fn display_name_lookup_failure_falls_back_without_failing_rendering() {
        let sender = session("claude-local", "sender-thread");
        let target = session("codex-local", "recipient-thread");

        let rendered = render_message_with_lookup(
            &target,
            &agent_message(sender),
            &FailedDisplayNameLookup,
            RouterNoticeKind::Other,
        )
        .expect("lookup failure cannot fail message rendering");

        assert!(
            rendered
                .text
                .starts_with("🤖 codex-local/recipien ← ✳️ claude-local/sender-t\n")
        );
        assert!(rendered.text.contains("\nSelf-declared sender: "));
        assert!(rendered.text.contains("\nIntended recipient: "));
    }

    #[test]
    fn old_agent_envelope_title_uses_sender_fallback_and_first_body_line() {
        let sender = session("claude-local", "sender-session-123");
        let target = session("codex-local", "recipient-session-123");
        let envelope = format!(
            "Agent communication\nSelf-declared sender: {}\nIntended recipient: {}\n\nReview the retry path.\nIgnore later lines.",
            serde_json::to_string(&sender).expect("sender JSON"),
            serde_json::to_string(&target).expect("recipient JSON"),
        );

        assert_eq!(
            title_from_agent_message_envelope(&envelope).as_deref(),
            Some("✳️ claude-local/sender-s: Review the retry path."),
        );
    }

    #[test]
    fn new_agent_envelope_title_uses_sender_header_and_first_body_line() {
        let sender = session("claude-local", "sender-session-123");
        let target = session("codex-local", "recipient-session-123");
        let envelope = format!(
            "🤖 Codex Main ← 🐒 Sidekick · PR2\nAgent communication\nSelf-declared sender: {}\nIntended recipient: {}\n\nAdd the focused acceptance test.\nMore details.",
            serde_json::to_string(&sender).expect("sender JSON"),
            serde_json::to_string(&target).expect("recipient JSON"),
        );

        let parsed = parse_agent_message_envelope(&envelope).expect("valid new envelope");
        assert_eq!(parsed.sender, sender);
        assert_eq!(parsed.recipient, target);
        assert_eq!(parsed.sender_identity, "🐒 Sidekick · PR2");
        assert_eq!(
            title_from_agent_message_envelope(&envelope).as_deref(),
            Some("🐒 Sidekick · PR2: Add the focused acceptance test."),
        );
    }

    #[test]
    fn router_envelope_titles_handle_legacy_notices_and_scheduled_identity_lines() {
        let target = session("codex-local", "recipient-session");
        let target_json = serde_json::to_string(&target).expect("recipient JSON");
        let legacy = format!(
            "Router delivery\nIntended recipient: {target_json}\n\nLegacy notice body.\nLater line."
        );
        let scheduled = format!(
            "🤖 Codex Main ← ⏰ Router schedule\nRouter delivery\nIntended recipient: {target_json}\n\nScheduled body.\nLater line."
        );

        assert_eq!(
            title_from_agent_message_envelope(&legacy).as_deref(),
            Some("🔔 Router notice: Legacy notice body."),
        );
        assert_eq!(
            title_from_agent_message_envelope(&scheduled).as_deref(),
            Some("⏰ Router schedule: Scheduled body."),
        );
    }

    #[test]
    fn queued_agent_identity_survives_display_name_changes_and_legacy_envelopes() {
        let sender = session("claude-local", "sender-session");
        let target = session("codex-local", "target-session");
        let message = MessageContent::Agent {
            sender: sender.clone(),
            text: MessageText::try_from("queued payload".to_owned()).expect("text"),
        };
        let old = format!(
            "Agent communication\nSelf-declared sender: {}\nIntended recipient: {}\n\nqueued payload",
            serde_json::to_string(&sender).expect("sender JSON"),
            serde_json::to_string(&target).expect("recipient JSON"),
        );
        let renamed_lookup = MapDisplayNames(HashMap::from([
            (
                "sender-session".to_owned(),
                Ok(Some(
                    SessionDisplayName::try_from("🐒 Sidekick".to_owned()).expect("name"),
                )),
            ),
            (
                "target-session".to_owned(),
                Ok(Some(
                    SessionDisplayName::try_from("🤖 Renamed target".to_owned()).expect("name"),
                )),
            ),
        ]));
        let new =
            render_message_with_lookup(&target, &message, &renamed_lookup, RouterNoticeKind::Other)
                .expect("render new envelope");

        assert!(queued_message_matches_content(&target, &message, &old));
        assert!(queued_message_matches_content(&target, &message, &new.text));
        assert!(!queued_message_matches_content(
            &target,
            &message,
            &new.text.replace("queued payload", "different payload")
        ));
    }

    #[test]
    fn queued_router_identity_accepts_legacy_and_typed_notice_envelopes() {
        let target = session("codex-local", "target-session");
        let message = MessageContent::Router {
            text: MessageText::try_from("scheduled payload".to_owned()).expect("text"),
        };
        let old = format!(
            "Router delivery\nIntended recipient: {}\n\nscheduled payload",
            serde_json::to_string(&target).expect("recipient JSON"),
        );
        let new = render_message_with_lookup(
            &target,
            &message,
            &MapDisplayNames(HashMap::from([(
                "target-session".to_owned(),
                Ok(Some(
                    SessionDisplayName::try_from("🤖 Renamed target".to_owned()).expect("name"),
                )),
            )])),
            RouterNoticeKind::Schedule,
        )
        .expect("render schedule envelope");

        let parsed = parse_router_message_envelope(&new.text).expect("Router envelope");
        assert_eq!(parsed.recipient, target);
        assert_eq!(parsed.body, "scheduled payload");
        assert!(queued_message_matches_content(&target, &message, &old));
        assert!(queued_message_matches_content(&target, &message, &new.text));
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
