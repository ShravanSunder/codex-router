#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
//! A real ACP load failure keeps its trace reference through CLI show and the stored push.

use codex_router_host::{
    CollaborationRuntime, CollaborationRuntimeInputs, ExternalProviderLaunchBinding,
    ExternalProviderStartup,
};
use collaboration_protocol::{
    EndpointId, EndpointRef, ProviderRequestedPolicy, ProviderWorkingDirectory, RouterAccess,
    SessionId, SessionRef,
};
use collaboration_service::{ProviderOperationStore, ProviderSessionRecord};
use serde_json::{Value, json};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::{
    io::{self, Write},
    os::unix::fs::PermissionsExt as _,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tracing_subscriber::{fmt::MakeWriter, util::SubscriberInitExt};

const PRIVATE_PROVIDER_MARKER: &str = "ACP_PROVIDER_PRIVATE_MARKER_8F39B1";
const SENDER_SESSION_ID: &str = "provider-error-e2e-sender";
const TARGET_SESSION_ID: &str = "provider-error-e2e-target";
const WAKE_BODY: &str = "public wake sent through the real ACP route";
const PROVIDER_ERROR_CODE: i64 = -32600;

#[derive(Clone, Default)]
struct CapturedTrace(Arc<Mutex<Vec<u8>>>);

impl CapturedTrace {
    fn rendered(&self) -> String {
        String::from_utf8_lossy(
            &self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
        .into_owned()
    }
}

struct CapturedTraceWriter(Arc<Mutex<Vec<u8>>>);

impl Write for CapturedTraceWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'writer> MakeWriter<'writer> for CapturedTrace {
    type Writer = CapturedTraceWriter;

    fn make_writer(&'writer self) -> Self::Writer {
        CapturedTraceWriter(Arc::clone(&self.0))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acp_load_failure_correlation_matches_cli_show_and_stored_outcome() {
    let captured_trace = CapturedTrace::default();
    tracing_subscriber::fmt()
        .json()
        .with_ansi(false)
        .without_time()
        .with_writer(captured_trace.clone())
        .finish()
        .try_init()
        .expect("install isolated ACP trace capture");

    let root = tempfile::tempdir().expect("Host root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private Host root");
    let runtime = CollaborationRuntime::start_with_external_providers(
        CollaborationRuntimeInputs {
            directory: root.path().to_owned(),
            codex_home: root.path().to_owned(),
            backend_socket: root.path().join("backend.sock"),
            mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            native_schema: None,
            peer_registry_directory: None,
            remote_control_server_name: None,
            owner_human_id: None,
        },
        vec![ExternalProviderStartup::Launch(
            ExternalProviderLaunchBinding::cursor(
                "/usr/bin/python3".into(),
                vec!["-c".to_owned(), refusing_load_provider_fixture()],
            )
            .expect("Cursor ACP binding"),
        )],
    )
    .await
    .expect("real Host startup");

    let caller = SessionRef {
        endpoint: EndpointRef {
            service_id: runtime.service_id().clone(),
            endpoint_id: EndpointId::try_from("codex-local".to_owned()).expect("caller endpoint"),
        },
        session_id: SessionId::try_from(SENDER_SESSION_ID.to_owned()).expect("caller session"),
    };
    let target = SessionRef {
        endpoint: EndpointRef {
            service_id: runtime.service_id().clone(),
            endpoint_id: EndpointId::try_from("cursor-local".to_owned()).expect("target endpoint"),
        },
        session_id: SessionId::try_from(TARGET_SESSION_ID.to_owned()).expect("target session"),
    };
    record_unloaded_provider_session(root.path(), &target, &caller).await;

    let service_directory = root.path().display().to_string();
    let configure = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "automation",
            "configure",
            "--execution-timeout-seconds",
            "300",
            "--summary-timeout-seconds",
            "300",
            "--json",
            "--service-directory",
            &service_directory,
        ])
        .env("CODEX_THREAD_ID", SENDER_SESSION_ID)
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CURSOR_CONVERSATION_ID")
        .output()
        .await
        .expect("enable isolated wake delivery");
    assert_eq!(
        configure.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&configure.stderr)
    );

    let target_json = serde_json::to_string(&target).expect("target JSON");
    let wake_send = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "wake",
            "send",
            "--to",
            &target_json,
            "--text",
            WAKE_BODY,
            "--after",
            "1s",
            "--for",
            "1h",
            "--wait-until-first-fire",
            "--json",
            "--service-directory",
            &service_directory,
        ])
        .env("CODEX_THREAD_ID", SENDER_SESSION_ID)
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CURSOR_CONVERSATION_ID")
        .output()
        .await
        .expect("send one timed wake through the CLI");
    assert_eq!(
        wake_send.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&wake_send.stderr)
    );
    let wake_result: Value = serde_json::from_slice(&wake_send.stdout).expect("wake JSON");
    assert_eq!(
        wake_result.pointer("/result/record/firstFire/kind"),
        Some(&json!("wakeFired"))
    );

    let automation_database = root.path().join("automation.sqlite");
    let mut connection =
        SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&automation_database))
            .await
            .expect("automation database connection");
    let (push_id, delivery_state, stored_outcome) = tokio::time::timeout(
        Duration::from_secs(5),
        async {
            loop {
                let row = sqlx::query_as::<_, (String, String, Option<String>)>(
                    "SELECT push_id, delivery_state, last_outcome_json FROM router_pushes WHERE body = ? ORDER BY created_at DESC LIMIT 1",
                )
                .bind(WAKE_BODY)
                .fetch_optional(&mut connection)
                .await
                .expect("read stored wake push");
                if let Some((push_id, delivery_state, Some(stored_outcome))) = row
                    && delivery_state == "rejected"
                {
                    break (push_id, delivery_state, stored_outcome);
                }
                tokio::task::yield_now().await;
            }
        },
    )
    .await
    .expect("wake provider failure was stored");
    connection.close().await.expect("close database connection");
    assert_eq!(delivery_state, "rejected");

    let client = collaboration_client::CollaborationClient::connect(
        root.path(),
        "provider-error-proof",
        "1",
    )
    .await
    .expect("API client for the stored link");
    let stored = client
        .router_show(collaboration_protocol::PushRecordShowParams {
            caller: target.clone(),
            reference: push_id,
        })
        .await
        .expect("stored wake push inspection");
    let link = stored.link;

    let show = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "show",
            &link,
            "--service-directory",
            &service_directory,
            "--json",
        ])
        .env("CURSOR_CONVERSATION_ID", TARGET_SESSION_ID)
        .env_remove("CODEX_THREAD_ID")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .output()
        .await
        .expect("inspect the failed wake push by its inbox link");
    assert_eq!(show.status.code(), Some(0));
    let show_json = String::from_utf8(show.stdout).expect("show result is UTF-8");
    let show_result: Value = serde_json::from_str(&show_json).expect("show result JSON");
    assert_eq!(show_result.pointer("/result/link"), Some(&json!(link)));
    assert_eq!(
        show_result.pointer("/result/record/deliveryState"),
        Some(&json!("rejected"))
    );

    let log_event = captured_trace
        .rendered()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|event| {
            event
                .pointer("/fields/raw_provider_text")
                .and_then(Value::as_str)
                == Some(PRIVATE_PROVIDER_MARKER)
        })
        .expect("captured ACP error trace");
    assert_eq!(
        log_event
            .pointer("/fields/provider_code")
            .and_then(Value::as_i64),
        Some(PROVIDER_ERROR_CODE)
    );
    let correlation_id = log_event
        .pointer("/fields/correlation_id")
        .and_then(Value::as_str)
        .expect("trace correlation ID");
    assert_eq!(
        uuid::Uuid::parse_str(correlation_id)
            .expect("UUIDv7 trace correlation")
            .get_version_num(),
        7
    );
    assert!(stored_outcome.contains(&format!("provider code {PROVIDER_ERROR_CODE}")));
    assert!(stored_outcome.contains(correlation_id));
    assert!(!stored_outcome.contains(PRIVATE_PROVIDER_MARKER));
    assert!(!show_json.contains(PRIVATE_PROVIDER_MARKER));
    assert_eq!(show_json.lines().count(), 1, "{show_json}");
    let shown_detail = show_result
        .pointer("/result/record/lastOutcome/outcome/detail")
        .and_then(Value::as_str)
        .expect("stored provider failure detail");
    assert!(shown_detail.contains(&format!("provider code {PROVIDER_ERROR_CODE}")));
    assert!(shown_detail.contains(correlation_id));

    runtime.shutdown().await.expect("Host shutdown");
}

fn refusing_load_provider_fixture() -> String {
    format!(
        r#"
import json,sys
request=json.loads(sys.stdin.readline())
print(json.dumps({{"jsonrpc":"2.0","id":request["id"],"result":{{"protocolVersion":1,"agentCapabilities":{{"loadSession":True}},"agentInfo":{{"name":"error-fixture","version":"1"}}}}}})); sys.stdout.flush()
for line in sys.stdin:
 request=json.loads(line)
 if request["method"]=="shutdown":
  print(json.dumps({{"jsonrpc":"2.0","id":request["id"],"result":{{}}}})); sys.stdout.flush()
 elif request["method"]=="exit":
  break
 else:
  assert request["method"]=="session/load", request["method"]
  print(json.dumps({{"jsonrpc":"2.0","id":request["id"],"error":{{"code":-32600,"message":"{PRIVATE_PROVIDER_MARKER}","data":{{"privateText":"{PRIVATE_PROVIDER_MARKER}"}}}}}})); sys.stdout.flush()
"#
    )
}

async fn record_unloaded_provider_session(
    directory: &Path,
    target: &SessionRef,
    caller: &SessionRef,
) {
    let mut store = ProviderOperationStore::open(&directory.join("provider-operations.sqlite"))
        .await
        .expect("provider store");
    store
        .record_session(&ProviderSessionRecord {
            target: target.clone(),
            working_directory: ProviderWorkingDirectory::try_from(directory.display().to_string())
                .expect("working directory"),
            requested_policy: ProviderRequestedPolicy {
                access: RouterAccess::WriteRestricted,
            },
            created_by: caller.clone().into(),
            approver: caller.clone().into(),
            updated_at_ms: 1,
        })
        .await
        .expect("record provider session");
}
