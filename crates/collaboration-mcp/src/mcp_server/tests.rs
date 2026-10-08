use super::{
    CollaborationMcpServer, ConversationCreateToolRequest, conversation_create_tool_result,
};
use collaboration_client::OperationEffect;
use collaboration_protocol::{ConversationCreateOutcome, OperationId};
use serde_json::Value;
use std::collections::BTreeSet;

const FIXTURE_SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";
const FIXTURE_PUSH_ID: &str = "018f1f12-3456-7abc-8def-0123456789ab";

fn fixture_session(endpoint_id: &str, session_id: &str) -> Value {
    serde_json::json!({
        "endpoint":{"serviceId":FIXTURE_SERVICE_ID,"endpointId":endpoint_id},
        "sessionId":session_id
    })
}

fn expected_tool_name(method: &str) -> String {
    if let Some(provider_method) = method.strip_prefix("conversation/") {
        return format!(
            "conversation_{}",
            provider_method
                .chars()
                .flat_map(|character| {
                    if character.is_ascii_uppercase() {
                        vec!['_', character.to_ascii_lowercase()]
                    } else {
                        vec![character]
                    }
                })
                .collect::<String>()
        );
    }
    match method {
        "endpoint/list" => return "endpoints_list".to_owned(),
        "codex/sessionList" => return "sessions_list".to_owned(),
        "provider/sessionList" => return "provider_sessions_list".to_owned(),
        "provider/sessionInspect" => return "provider_session_inspect".to_owned(),
        "codex/sessionInspect" => return "session_inspect".to_owned(),
        "codex/sessionRename" => return "session_rename".to_owned(),
        "message/send" => return "message_send".to_owned(),
        "codex/turnInterrupt" => return "turn_interrupt".to_owned(),
        "lifecycleJournal/status" => return "journal_status".to_owned(),
        "lifecycleJournal/read" => return "journal_read".to_owned(),
        "addressBook/list" => return "addresses_list".to_owned(),
        "wake/subscribe" => return "wake_wait_until_first_fire".to_owned(),
        _ => {}
    }
    let mut output = String::new();
    for character in method.chars() {
        if character == '/' {
            output.push('_');
        } else if character.is_ascii_uppercase() {
            output.push('_');
            output.push(character.to_ascii_lowercase());
        } else {
            output.push(character);
        }
    }
    output
}

#[path = "tests/route_tests.rs"]
mod route_tests;

#[path = "tests/catalog_tests.rs"]
mod catalog_tests;

#[path = "tests/schema_tests.rs"]
mod schema_tests;

#[path = "tests/inspect_snapshot_tests.rs"]
mod inspect_snapshot_tests;
