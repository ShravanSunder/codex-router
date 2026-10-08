//! A request the API sheds at its request limit was not run. The CLI reports it as an
//! `overloaded` failure to retry later: a board command in the board's typed refusal, and an
//! untyped command as the retryable overload, never as a connection failure or protocol
//! violation.
use automation_storage::AutomationStore;
use collaboration_mcp::test_support::{ServedCollaborationApi, TestServeOptions};
use collaboration_protocol::OperationId;
use collaboration_service::{CollaborationApplication, ServiceIdentity};
use message_board_storage::BoardStore;
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::Path, sync::Arc, time::Duration};
use tokio::sync::Mutex;

const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";

async fn run_cli(
    directory: &Path,
    arguments: &[&str],
) -> Result<(Option<i32>, Value), Box<dyn std::error::Error>> {
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args(arguments)
        .arg("--service-directory")
        .arg(directory)
        .arg("--json")
        .output()
        .await?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().last().unwrap_or_default();
    Ok((output.status.code(), serde_json::from_str(line)?))
}

#[tokio::test]
async fn shed_commands_report_an_overload_to_retry() {
    // Arrange: one request slot, held by a first-fire wait.
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .expect("private directory");
    let board = BoardStore::open(&directory.path().join("board.sqlite"))
        .await
        .expect("board store");
    let automation = AutomationStore::open(&directory.path().join("automation.sqlite"))
        .await
        .expect("automation store");
    let identity = ServiceIdentity::new(SERVICE_ID, "00000000-0000-4000-8000-000000000002")
        .expect("service identity")
        .with_board_store(Arc::new(Mutex::new(board)))
        .with_automation_store(Arc::new(Mutex::new(automation)));
    let served = Arc::new(
        ServedCollaborationApi::start_with(
            directory.path(),
            CollaborationApplication::new(identity),
            TestServeOptions {
                concurrent_requests: Some(1),
                ..TestServeOptions::default()
            },
        )
        .await
        .expect("served API"),
    );
    let wake = served
        .call(
            "wake_send",
            json!({
                "operationId": OperationId::generate(),
                "message": {
                    "target": {
                        "endpoint": {"serviceId": SERVICE_ID, "endpointId": "codex-local"},
                        "sessionId": "overload-target"
                    },
                    "content": {"kind": "humanUser", "text": "overload proof"},
                    "delivery": "auto",
                    "generationGuard": null
                },
                "timing": {"kind": "after", "seconds": 600},
                "expiry": {"kind": "none"}
            }),
        )
        .await
        .expect("wake send");
    let wakeup_id = wake["result"]["definition"]["wakeupId"].clone();
    let held = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let holder = Arc::clone(&served);
            let held_wakeup = wakeup_id.clone();
            let held = tokio::spawn(async move {
                let _answer = holder
                    .call(
                        "wake_wait_until_first_fire",
                        json!({"wakeupId": held_wakeup, "timeoutSeconds": 60}),
                    )
                    .await;
            });
            tokio::time::sleep(Duration::from_millis(100)).await;
            if held.is_finished() {
                continue;
            }
            let probe = served
                .call("automation_status", json!({}))
                .await
                .expect("probe");
            if probe["error"]["data"]["kind"] == "overloaded" {
                break Some(held);
            }
            held.abort();
        }
    })
    .await
    .ok()
    .flatten()
    .expect("the wake wait holds the only request slot");

    // Act
    let (board_exit, board) = run_cli(directory.path(), &["board", "project", "list"])
        .await
        .expect("board command output");
    let (endpoints_exit, endpoints) = run_cli(directory.path(), &["endpoints", "list"])
        .await
        .expect("endpoints command output");

    // Assert: the board refusal is its typed overload; the untyped command reports the
    // retryable overload with no effect.
    assert_eq!(board_exit, Some(4), "{board}");
    assert_eq!(board["error"]["kind"], "overloaded", "{board}");
    assert!(
        board["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("not run")),
        "{board}"
    );
    assert_eq!(endpoints_exit, Some(3), "{endpoints}");
    assert_eq!(endpoints["error"]["kind"], "overloaded", "{endpoints}");
    assert_eq!(endpoints["error"]["effect"], "none", "{endpoints}");
    assert_eq!(
        endpoints["error"]["nextAction"], "retryLater",
        "{endpoints}"
    );
    held.abort();
    let _ended = held.await;
    if let Ok(served) = Arc::try_unwrap(served) {
        served.stop().await.expect("API stops");
    }
}
