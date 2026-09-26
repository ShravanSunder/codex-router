//! Bounded, reviewed decision details extracted from ACP permission requests.

use agent_client_protocol::schema::v1::{ContentBlock, ToolCallContent, ToolCallUpdateFields};
use collaboration_protocol::{ApprovalArgument, ApprovalPresentation};
use serde_json::Value;

const MAX_TOOL_NAME_CHARS: usize = 100;
const MAX_TITLE_CHARS: usize = 200;
const MAX_KIND_CHARS: usize = 64;
const MAX_ARGUMENTS: usize = 8;
const MAX_ARGUMENT_NAME_CHARS: usize = 64;
const MAX_ARGUMENT_VALUE_CHARS: usize = 512;
const MAX_PERMISSION_DETAILS: usize = 8;
const MAX_PERMISSION_DETAIL_CHARS: usize = 512;

pub(super) fn approval_presentation(fields: &ToolCallUpdateFields) -> ApprovalPresentation {
    ApprovalPresentation {
        tool_name: fields
            .name
            .as_deref()
            .map(|value| bounded_text(value, MAX_TOOL_NAME_CHARS)),
        title: fields
            .title
            .as_deref()
            .map(|value| sanitize_text(value, MAX_TITLE_CHARS)),
        kind: fields
            .kind
            .map(|kind| bounded_text(&format!("{kind:?}"), MAX_KIND_CHARS)),
        arguments: fields
            .raw_input
            .as_ref()
            .map(safe_arguments)
            .unwrap_or_default(),
        permission_details: fields
            .content
            .as_deref()
            .map(permission_text)
            .unwrap_or_default(),
    }
}

fn safe_arguments(raw_input: &Value) -> Vec<ApprovalArgument> {
    let Some(arguments) = raw_input.as_object() else {
        return Vec::new();
    };
    arguments
        .iter()
        .filter_map(|(name, value)| {
            if is_sensitive_name(name) || !is_reviewed_argument_name(name) {
                return None;
            }
            let value = match value {
                Value::String(value) => sanitize_text(value, MAX_ARGUMENT_VALUE_CHARS),
                Value::Number(value) => value.to_string(),
                Value::Bool(value) => value.to_string(),
                Value::Null | Value::Array(_) | Value::Object(_) => return None,
            };
            Some(ApprovalArgument {
                name: bounded_text(name, MAX_ARGUMENT_NAME_CHARS),
                value,
            })
        })
        .take(MAX_ARGUMENTS)
        .collect()
}

fn permission_text(content: &[ToolCallContent]) -> Vec<String> {
    content
        .iter()
        .filter_map(|item| match item {
            ToolCallContent::Content(content) => match &content.content {
                ContentBlock::Text(text) => {
                    Some(sanitize_text(&text.text, MAX_PERMISSION_DETAIL_CHARS))
                }
                ContentBlock::Image(_)
                | ContentBlock::Audio(_)
                | ContentBlock::ResourceLink(_)
                | ContentBlock::Resource(_) => None,
                _ => None,
            },
            ToolCallContent::Diff(_) | ToolCallContent::Terminal(_) => None,
            _ => None,
        })
        .take(MAX_PERMISSION_DETAILS)
        .collect()
}

fn sanitize_text(value: &str, maximum_chars: usize) -> String {
    let mut output = Vec::new();
    let mut redact_next = false;
    for token in value.split_whitespace() {
        let assignment = token.split_once('=');
        let sensitive_assignment = assignment.is_some_and(|(name, _)| {
            is_sensitive_name(name.rsplit(['?', '&']).next().unwrap_or(name))
        });
        if redact_next {
            output.push("[redacted]".to_owned());
            redact_next = false;
            continue;
        }
        if token.starts_with("http://") || token.starts_with("https://") {
            output.push("[redacted URL]".to_owned());
            continue;
        }
        if sensitive_assignment {
            let (name, _) = assignment.unwrap_or(("", ""));
            output.push(format!("{name}=[redacted]"));
            continue;
        }
        if is_sensitive_name(token) {
            output.push(token.to_owned());
            redact_next = true;
            continue;
        }
        output.push(token.to_owned());
    }
    bounded_text(&output.join(" "), maximum_chars)
}

fn is_sensitive_name(name: &str) -> bool {
    let lowercase = name.to_ascii_lowercase();
    [
        "password",
        "passwd",
        "secret",
        "token",
        "credential",
        "authorization",
        "api_key",
        "api-key",
        "apikey",
        "private_key",
        "private-key",
    ]
    .iter()
    .any(|sensitive_word| lowercase.contains(sensitive_word))
}

fn is_reviewed_argument_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "command"
            | "cmd"
            | "path"
            | "file"
            | "file_path"
            | "filepath"
            | "pattern"
            | "query"
            | "description"
            | "url"
    )
}

fn bounded_text(value: &str, maximum_chars: usize) -> String {
    value.chars().take(maximum_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{Content, TextContent, ToolCallUpdateFields, ToolKind};
    use serde_json::json;

    #[test]
    fn decision_description_is_bounded_and_redacts_named_secrets() {
        let fields = ToolCallUpdateFields::new()
            .name("Bash")
            .title("echo approved")
            .kind(ToolKind::Execute)
            .raw_input(json!({
                "command":"echo safe TOKEN=do-not-store --password hunter2",
                "description":"Not in allowlist: echo safe",
                "api_key":"secret-value",
                "nested":{"private":"value"}
            }))
            .content(vec![ToolCallContent::Content(Content::new(
                ContentBlock::Text(TextContent::new("Not in allowlist: echo safe")),
            ))]);

        let presentation = approval_presentation(&fields);
        assert_eq!(presentation.tool_name.as_deref(), Some("Bash"));
        assert_eq!(presentation.title.as_deref(), Some("echo approved"));
        assert_eq!(presentation.kind.as_deref(), Some("Execute"));
        assert_eq!(presentation.arguments.len(), 2);
        assert!(presentation.arguments[0].value.contains("TOKEN=[redacted]"));
        assert!(
            presentation.arguments[0]
                .value
                .contains("--password [redacted]")
        );
        assert!(
            !serde_json::to_string(&presentation)
                .expect("presentation JSON")
                .contains("do-not-store")
        );
        assert!(
            !serde_json::to_string(&presentation)
                .expect("presentation JSON")
                .contains("hunter2")
        );
        assert_eq!(
            presentation.permission_details,
            vec!["Not in allowlist: echo safe"]
        );
    }
}
