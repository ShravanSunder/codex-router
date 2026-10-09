//! A request the API sheds at its request limit was not run. Each family client reports it as
//! its own typed `overloaded` failure with "retry later", never as a protocol violation or an
//! uncertain outcome. The served API runs one request at a time on its socket; a held wake
//! wait takes that one, so every other call is shed.
use automation_storage::AutomationStore;
use collaboration_client::{
    AutomationInspectionClientError, BoardClientError, ClientError, CollaborationAccess,
    CollaborationClient, ConfigurationClientError, InstructionClientError,
    NativeTransportConnection, NativeTransportError, OperationEffect, RunClientError,
    ScheduleClientError, WakeClientError, WakeWaitError, operation_failure_from_client_error,
    resolve_public_native,
};
use collaboration_mcp::test_support::{ServedCollaborationApi, TestServeOptions};
use collaboration_protocol::{
    AutomationInspectionFailureKind, ConfigurationFailureKind, ConfigurationNextAction, EndpointId,
    InstructionFailureKind, InstructionNextAction, OperationFailureKind, OperationId,
    RunFailureKind, RunNextAction, ScheduleFailureKind, ScheduleNextAction, WakeFailureReason,
    WakeNextAction, WakeShowRequest,
};
use collaboration_service::{CollaborationApplication, ServiceIdentity};
use message_board::{BoardFailureKind, BoardNextAction};
use message_board_storage::BoardStore;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;

const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";

/// A served API that runs one request at a time, with that one held by a wake wait.
struct SaturatedApi {
    served: Arc<ServedCollaborationApi>,
    client: CollaborationClient,
    wakeup_id: Value,
    held: tokio::task::JoinHandle<()>,
    directory: tempfile::TempDir,
}

type TestResult<TValue> = Result<TValue, Box<dyn std::error::Error>>;

impl SaturatedApi {
    async fn start() -> TestResult<Self> {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")?;
        let board = BoardStore::open(&directory.path().join("board.sqlite")).await?;
        let automation = AutomationStore::open(&directory.path().join("automation.sqlite")).await?;
        let identity = ServiceIdentity::new(SERVICE_ID, "00000000-0000-4000-8000-000000000002")
            .map_err(std::io::Error::other)?
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
            .await?,
        );
        let client = served.client("overload-proof").await?;
        let wake = served.call("wake_send", wake_send_arguments()).await?;
        let wakeup_id = wake
            .pointer("/result/definition/wakeupId")
            .filter(|wakeup_id| wakeup_id.is_string())
            .cloned()
            .ok_or_else(|| format!("wake send failed: {wake}"))?;
        // The wait holds the only request slot once a probe sent after it is shed. A probe
        // that wins the slot first would shed the wait instead, so the wait is started again.
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
                let probe = served.call("automation_status", json!({})).await?;
                if probe.pointer("/error/data/kind") == Some(&json!("overloaded")) {
                    break Ok::<_, std::io::Error>(held);
                }
                held.abort();
            }
        })
        .await
        .map_err(|_| "the wake wait never held the only request slot")??;
        Ok(Self {
            served,
            client,
            wakeup_id,
            held,
            directory,
        })
    }

    async fn stop(self) -> TestResult<()> {
        self.held.abort();
        let _ended = self.held.await;
        if let Ok(served) = Arc::try_unwrap(self.served) {
            served.stop().await?;
        }
        Ok(())
    }
}

fn wake_send_arguments() -> Value {
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
    })
}

fn from_json<TValue: serde::de::DeserializeOwned>(value: Value) -> serde_json::Result<TValue> {
    serde_json::from_value(value)
}

#[tokio::test]
async fn a_shed_board_call_is_a_typed_retryable_board_overload() {
    let api = SaturatedApi::start().await.expect("saturated API");
    let shed = api
        .client
        .board_project_list(
            from_json(json!({"repository": null, "page": {}})).expect("request JSON"),
        )
        .await;
    let Err(BoardClientError::Rejected(failure)) = shed else {
        panic!("expected a typed board rejection, got {shed:?}");
    };
    assert_eq!(failure.kind, BoardFailureKind::Overloaded);
    assert_eq!(failure.next_action, BoardNextAction::RetryLater);
    api.stop().await.expect("API stops");
}

#[tokio::test]
async fn a_shed_board_write_is_not_an_unknown_outcome() {
    let api = SaturatedApi::start().await.expect("saturated API");
    let shed = api
        .client
        .board_project_create(
            from_json(json!({
                "projectId": OperationId::generate(),
                "name": "Shed",
                "description": "",
                "actor": {"kind": "human", "humanId": "owner"}
            }))
            .expect("request JSON"),
        )
        .await;
    let Err(BoardClientError::Rejected(failure)) = shed else {
        panic!("a shed write must not read as an unknown outcome: {shed:?}");
    };
    assert_eq!(failure.kind, BoardFailureKind::Overloaded);
    api.stop().await.expect("API stops");
}

#[tokio::test]
async fn a_shed_wake_call_is_a_typed_retryable_wake_overload() {
    let api = SaturatedApi::start().await.expect("saturated API");
    let operation_id = OperationId::generate();
    let mut arguments = wake_send_arguments();
    arguments["operationId"] = json!(operation_id);
    let shed = api
        .client
        .send_wakeup(from_json(arguments).expect("request JSON"))
        .await;
    let Err(WakeClientError::Rejected(failure)) = shed else {
        panic!("expected a typed wake rejection, got {shed:?}");
    };
    assert!(matches!(failure.reason, WakeFailureReason::Overloaded));
    assert!(matches!(failure.next_action, WakeNextAction::RetryLater));
    assert_eq!(failure.operation_id, Some(operation_id));
    api.stop().await.expect("API stops");
}

#[tokio::test]
async fn a_shed_first_fire_wait_can_be_started_again() {
    let api = SaturatedApi::start().await.expect("saturated API");
    let shed = api
        .client
        .clone()
        .subscribe_wakeup(WakeShowRequest {
            wakeup_id: from_json(api.wakeup_id.clone()).expect("request JSON"),
        })
        .await;
    assert!(
        matches!(shed, Err(WakeWaitError::Unavailable { .. })),
        "{:?}",
        shed.err()
    );
    api.stop().await.expect("API stops");
}

#[tokio::test]
async fn a_shed_schedule_call_is_a_typed_retryable_schedule_overload() {
    let api = SaturatedApi::start().await.expect("saturated API");
    let shed = api
        .client
        .read_schedule(
            from_json(json!({"scheduleId": OperationId::generate()})).expect("request JSON"),
        )
        .await;
    let Err(ScheduleClientError::Rejected(failure)) = shed else {
        panic!("expected a typed schedule rejection, got {shed:?}");
    };
    assert!(matches!(failure.kind, ScheduleFailureKind::Overloaded));
    assert!(matches!(
        failure.next_action,
        ScheduleNextAction::RetryLater
    ));
    api.stop().await.expect("API stops");
}

#[tokio::test]
async fn a_shed_run_call_is_a_typed_retryable_run_overload() {
    let api = SaturatedApi::start().await.expect("saturated API");
    let shed = api
        .client
        .read_run(from_json(json!({"runId": OperationId::generate()})).expect("request JSON"))
        .await;
    let Err(RunClientError::Rejected(failure)) = shed else {
        panic!("expected a typed run rejection, got {shed:?}");
    };
    assert!(matches!(failure.kind, RunFailureKind::Overloaded));
    assert!(matches!(failure.next_action, RunNextAction::RetryLater));
    api.stop().await.expect("API stops");
}

#[tokio::test]
async fn a_shed_instruction_call_is_a_typed_retryable_instruction_overload() {
    let api = SaturatedApi::start().await.expect("saturated API");
    let shed = api
        .client
        .read_instruction(
            from_json(json!({"instructionId": OperationId::generate()})).expect("request JSON"),
        )
        .await;
    let Err(InstructionClientError::Rejected(failure)) = shed else {
        panic!("expected a typed instruction rejection, got {shed:?}");
    };
    assert!(matches!(failure.kind, InstructionFailureKind::Overloaded));
    assert!(matches!(
        failure.next_action,
        InstructionNextAction::RetryLater
    ));
    api.stop().await.expect("API stops");
}

#[tokio::test]
async fn a_shed_configuration_call_is_a_typed_retryable_configuration_failure() {
    let api = SaturatedApi::start().await.expect("saturated API");
    let shed = api.client.automation_status().await;
    let Err(ConfigurationClientError::Rejected(failure)) = shed else {
        panic!("expected a typed configuration rejection, got {shed:?}");
    };
    assert!(matches!(
        failure.kind,
        ConfigurationFailureKind::AutomationUnavailable
    ));
    assert!(matches!(
        failure.next_action,
        ConfigurationNextAction::RetryLater
    ));
    api.stop().await.expect("API stops");
}

#[tokio::test]
async fn a_shed_inspection_call_is_a_typed_retryable_inspection_overload() {
    let api = SaturatedApi::start().await.expect("saturated API");
    let shed = api
        .client
        .reconcile_run(from_json(json!({"runId": OperationId::generate()})).expect("request JSON"))
        .await;
    let Err(AutomationInspectionClientError::Rejected(failure)) = shed else {
        panic!("expected a typed inspection rejection, got {shed:?}");
    };
    assert!(matches!(
        failure.kind,
        AutomationInspectionFailureKind::Overloaded
    ));
    api.stop().await.expect("API stops");
}

#[tokio::test]
async fn a_shed_untyped_call_reports_no_effect_and_retry() {
    let api = SaturatedApi::start().await.expect("saturated API");
    let shed = api
        .client
        .list_endpoints()
        .await
        .expect_err("the endpoint list is shed");
    assert!(matches!(shed, ClientError::Overloaded { .. }), "{shed:?}");
    let conversation = shed
        .conversation_failure()
        .expect("the conversation family's overload");
    assert_eq!(serde_json::json!(conversation.kind), "overloaded");
    let failure = operation_failure_from_client_error(shed, OperationEffect::Unknown);
    assert_eq!(failure.kind, OperationFailureKind::Unavailable);
    assert_eq!(failure.service_kind.as_deref(), Some("overloaded"));
    assert_eq!(failure.effect, OperationEffect::None);
    assert!(failure.message.contains("not run"), "{}", failure.message);
    api.stop().await.expect("API stops");
}

#[tokio::test]
async fn shed_native_discovery_keeps_the_overload_for_the_bridge_and_the_launcher() {
    // Arrange
    let api = SaturatedApi::start().await.expect("saturated API");
    let directory = api.directory.path().to_owned();
    let endpoint_id = EndpointId::try_from("codex-local".to_owned()).expect("endpoint id");

    // Act: the native bridge's carrier discovery, and the hosted TUI launch's.
    let bridged =
        NativeTransportConnection::connect(&CollaborationAccess::api(&directory), endpoint_id)
            .await;
    let launched = tokio::task::spawn_blocking(move || resolve_public_native(&directory))
        .await
        .expect("launch discovery joins");

    // Assert: both name the retryable overload, not an undifferentiated discovery failure.
    let Err(bridge_error) = bridged else {
        panic!("the bridge's discovery was not shed");
    };
    assert!(
        matches!(bridge_error, NativeTransportError::Overloaded { .. }),
        "{bridge_error:?}"
    );
    let launch_error = launched.expect_err("the launch discovery is shed");
    assert!(
        matches!(
            launch_error
                .get_ref()
                .and_then(|source| source.downcast_ref::<NativeTransportError>()),
            Some(NativeTransportError::Overloaded { .. })
        ),
        "{launch_error:?}"
    );
    api.stop().await.expect("API stops");
}
