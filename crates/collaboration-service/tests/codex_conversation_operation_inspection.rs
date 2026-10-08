use collaboration_protocol::{
    CodexGeneration, EndpointId, EndpointRef, NonEmptyText, OperationId, SessionId, SessionRef,
    UuidIdentity,
};
use collaboration_service::{
    CodexConversationOperationRecorder, ConversationOperationRecorder, ProviderOperationStore,
    ServiceIdentity,
};
use serde_json::{Value, json};
use std::sync::Arc;
#[path = "support/served_api.rs"]
mod served_api;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

macro_rules! check_eq {
    ($left:expr, $right:expr) => {{
        let left = &$left;
        let right = &$right;
        if left != right {
            return Err(format!(
                "assertion failed: {} == {}: left={left:?}, right={right:?}",
                stringify!($left),
                stringify!($right)
            )
            .into());
        }
    }};
    ($left:expr, $right:expr, $message:expr) => {{
        let left = &$left;
        let right = &$right;
        if left != right {
            return Err(format!(
                "assertion failed: {} == {}: left={left:?}, right={right:?}: {}",
                stringify!($left),
                stringify!($right),
                $message
            )
            .into());
        }
    }};
}

async fn call(served: &served_api::ServedApi, tool: &str, params: Value) -> TestResult<Value> {
    Ok(served.call(tool, params).await?)
}

#[tokio::test]
async fn codex_create_operation_is_inspectable_and_reconcilable_through_the_api() -> TestResult {
    let path = std::env::temp_dir().join(format!(
        "codex-operation-inspection-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let store = Arc::new(tokio::sync::Mutex::new(
        ProviderOperationStore::open(&path).await?,
    ));
    let service_id = UuidIdentity::try_from("019f0000-0000-7000-8000-000000000001".to_owned())?;
    let endpoint = EndpointRef {
        service_id: service_id.clone(),
        endpoint_id: EndpointId::try_from("codex-local".to_owned())?,
    };
    let recorder = Arc::new(CodexConversationOperationRecorder::new(
        Arc::clone(&store),
        endpoint.clone(),
        NonEmptyText::try_from("/tmp/codex-acp.sock".to_owned())?,
    ));
    let identity = ServiceIdentity::new(
        &String::from(service_id),
        "019f0000-0000-7000-8000-000000000002",
    )?
    .with_provider_operation_store(Arc::clone(&store))
    .with_codex_conversation_recorder(Arc::clone(&recorder));
    let served = served_api::ServedApi::start(identity).await?;

    let generation: CodexGeneration = serde_json::from_value(
        json!({"serviceEpoch":"019f0000-0000-7000-8000-000000000002","generation":1}),
    )?;
    let operation_id = OperationId::generate();
    recorder.admit_create(&operation_id, &generation).await?;
    recorder.before_native_dispatch(&operation_id).await?;
    let shown = call(
        &served,
        "conversation_operation_show",
        json!({"operationId":operation_id}),
    )
    .await?;
    check_eq!(shown["result"]["stage"], "mayHaveDispatched");
    check_eq!(shown["result"]["binding"]["kind"], "codexAcp");
    let pending = call(
        &served,
        "conversation_operation_wait",
        json!({"operationId":operation_id,"timeoutSeconds":1}),
    )
    .await?;
    check_eq!(pending["result"]["output"]["kind"], "pending");

    let session_id = SessionId::try_from("created-thread".to_owned())?;
    recorder.record_created(&operation_id, &session_id).await?;
    let settled = call(
        &served,
        "conversation_operation_wait",
        json!({"operationId":operation_id,"timeoutSeconds":1}),
    )
    .await?;
    check_eq!(
        settled["result"]["operation"]["target"]["sessionId"],
        "created-thread",
        "{settled}"
    );
    check_eq!(
        settled["result"]["operation"]["reconciliation"],
        "confirmed"
    );
    let reconciled = call(
        &served,
        "conversation_operation_reconcile",
        json!({"operationId":operation_id}),
    )
    .await?;
    check_eq!(reconciled["result"]["reconciliation"], "confirmed");

    let recovered_id = OperationId::generate();
    recorder.admit_create(&recovered_id, &generation).await?;
    recorder.before_native_dispatch(&recovered_id).await?;
    store
        .lock()
        .await
        .record_target(
            &recovered_id,
            &SessionRef {
                endpoint,
                session_id: SessionId::try_from("recovered-thread".to_owned())?,
            },
            chrono::Utc::now().timestamp_millis(),
        )
        .await?;
    recorder.record_failure(&recovered_id, false).await?;
    let recovered = call(
        &served,
        "conversation_operation_reconcile",
        json!({"operationId":recovered_id}),
    )
    .await?;
    check_eq!(recovered["result"]["effect"], "applied");
    check_eq!(recovered["result"]["reconciliation"], "confirmed");
    check_eq!(
        recovered["result"]["target"]["sessionId"],
        "recovered-thread"
    );

    let unknown_id = OperationId::generate();
    recorder.admit_create(&unknown_id, &generation).await?;
    recorder.before_native_dispatch(&unknown_id).await?;
    recorder.record_failure(&unknown_id, false).await?;
    let unknown = call(
        &served,
        "conversation_operation_reconcile",
        json!({"operationId":unknown_id}),
    )
    .await?;
    check_eq!(unknown["result"]["effect"], "unknown");
    check_eq!(unknown["result"]["reconciliation"], "notReconcilable");

    served.stop().await?;
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    Ok(())
}
