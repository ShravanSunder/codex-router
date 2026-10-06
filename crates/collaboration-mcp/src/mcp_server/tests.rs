use super::{
    CollaborationMcpServer, ConversationCreateToolRequest, conversation_create_tool_result,
};
use collaboration_protocol::{ConversationCreateOutcome, OperationId};
use serde_json::Value;
use std::collections::BTreeSet;

const FIXTURE_SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";
const FIXTURE_SERVICE_EPOCH: &str = "00000000-0000-4000-8000-000000000002";
const FIXTURE_PUSH_ID: &str = "018f1f12-3456-7abc-8def-0123456789ab";

fn fixture_session(endpoint_id: &str, session_id: &str) -> Value {
    serde_json::json!({
        "endpoint":{"serviceId":FIXTURE_SERVICE_ID,"endpointId":endpoint_id},
        "sessionId":session_id
    })
}

fn start_control_response_fixture(
    service_directory: &std::path::Path,
    expected_method: &'static str,
    result: Result<Value, Value>,
) -> tokio::task::JoinHandle<Value> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let digest = format!("sha256:{}", "a".repeat(64));
    std::fs::write(
        service_directory.join("service.json"),
        serde_json::to_vec(&serde_json::json!({
            "version":2,
            "serviceId":FIXTURE_SERVICE_ID,
            "serviceEpoch":FIXTURE_SERVICE_EPOCH,
            "machineLabel":"fixture-host",
            "control":{"transport":"unixJsonLines","path":"control.sock"},
            "controlSchemaDigest":digest,
            "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
        }))
        .expect("manifest JSON"),
    )
    .expect("manifest fixture");
    let listener = tokio::net::UnixListener::bind(service_directory.join("control.sock"))
        .expect("Control fixture socket");

    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("Control accept");
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        let initialize: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("read initialize")
                .expect("initialize frame"),
        )
        .expect("initialize JSON");
        assert_eq!(initialize["method"], "control/initialize");
        let initialized = serde_json::json!({
            "jsonrpc":"2.0",
            "id":initialize["id"],
            "result":{
                "version":{"major":1,"minor":0},
                "serviceId":FIXTURE_SERVICE_ID,
                "serviceEpoch":FIXTURE_SERVICE_EPOCH,
                "controlSchemaDigest":format!("sha256:{}", "a".repeat(64))
            }
        });
        write
            .write_all(format!("{initialized}\n").as_bytes())
            .await
            .expect("write initialize response");

        let request: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("read tool request")
                .expect("tool request frame"),
        )
        .expect("tool request JSON");
        assert_eq!(request["method"], expected_method);
        let response_id = request["id"].clone();
        let response = match result {
            Ok(result) => serde_json::json!({
                "jsonrpc":"2.0","id":response_id,"result":result
            }),
            Err(error) => serde_json::json!({
                "jsonrpc":"2.0","id":response_id,"error":error
            }),
        };
        write
            .write_all(format!("{response}\n").as_bytes())
            .await
            .expect("write tool response");
        request
    })
}

async fn finish_control_fixture(peer: tokio::task::JoinHandle<Value>) -> Value {
    tokio::time::timeout(std::time::Duration::from_secs(2), peer)
        .await
        .expect("Control fixture traffic deadline")
        .expect("Control fixture task")
}

fn direct_message_show_result(target: Value) -> Value {
    serde_json::json!({
        "record":{
            "pushId":FIXTURE_PUSH_ID,
            "kind":"direct-message",
            "origin":{"session":fixture_session("claude-local", "sender-session")},
            "originRouterRef":null,
            "target":target,
            "replyToPushId":null,
            "headerFacts":{"kind":"directMessage","senderDisplayName":null},
            "body":"stored body",
            "activity":null,
            "deliveryState":"delivered",
            "lastOutcome":{
                "outcome":{"kind":"peerMessageWritten"},
                "reachability":"claudeCodePeer",
                "client":{"kind":"claudeCodePeer"}
            },
            "createdAt":"2026-09-30T16:00:00Z",
            "settledAt":"2026-09-30T16:00:01Z",
            "readAt":"2026-09-30T16:00:02Z"
        },
        "link":format!("router://{FIXTURE_SERVICE_ID}/push/{FIXTURE_PUSH_ID}"),
        "activityRanges":[]
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
