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

#[path = "tests/route_tests.rs"]
mod route_tests;

#[path = "tests/catalog_tests.rs"]
mod catalog_tests;

#[path = "tests/schema_tests.rs"]
mod schema_tests;

#[path = "tests/emitted_result_schema_tests.rs"]
mod emitted_result_schema_tests;

#[path = "tests/inspect_snapshot_tests.rs"]
mod inspect_snapshot_tests;
