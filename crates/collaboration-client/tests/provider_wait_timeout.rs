#![allow(clippy::expect_used, clippy::panic)]

use collaboration_client::ControlClient;
use collaboration_protocol::*;
use collaboration_service::{
    ProviderConversationBackend, ProviderConversationFuture, ServiceIdentity,
    serve_control_connection,
};
use std::sync::Arc;

struct DelayedWaitBackend {
    entered: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    delay: std::time::Duration,
}

impl ProviderConversationBackend for DelayedWaitBackend {
    fn binding(&self, _endpoint: &EndpointRef) -> Option<ProviderBindingIdentity> {
        None
    }
    fn create(
        &self,
        _request: ConversationCreateRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
        Box::pin(async { panic!("unused create") })
    }
    fn load(
        &self,
        _request: ConversationLoadRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
        Box::pin(async { panic!("unused load") })
    }
    fn prompt(
        &self,
        _request: ConversationPromptRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
        Box::pin(async { panic!("unused prompt") })
    }
    fn cancel(
        &self,
        _request: ConversationCancelRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
        Box::pin(async { panic!("unused cancel") })
    }
    fn show(
        &self,
        _request: ConversationOperationShowRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSnapshot> {
        Box::pin(async { panic!("unused show") })
    }
    fn reconcile(
        &self,
        _request: ConversationOperationReconcileRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationSnapshot> {
        Box::pin(async { panic!("unused reconcile") })
    }
    fn wait(
        &self,
        request: ConversationOperationWaitRequest,
    ) -> ProviderConversationFuture<'_, ConversationOperationWaitResult> {
        let entered = self.entered.lock().expect("entered signal").take();
        let delay = self.delay;
        Box::pin(async move {
            if let Some(entered) = entered {
                let _result = entered.send(());
            }
            tokio::time::sleep(delay).await;
            let operation = serde_json::from_value(serde_json::json!({
                "operationId":request.operation_id,
                "operation":"conversationPrompt",
                "binding":{
                    "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},
                    "bindingId":"binding",
                    "runtime":{"provider":"claudeCode","runtimeName":"fixture","runtimeVersion":null},
                    "transport":"stdioAcp",
                    "generation":{"serviceEpoch":"00000000-0000-4000-8000-000000000002","generation":1},
                    "capabilities":[{"name":"prompt","status":"supported","evidence":"advertised"}]
                },
                "target":null,"stage":"mayHaveDispatched","effect":"unknown",
                "reconciliation":"unresolved","admittedAt":"2026-09-22T00:00:00.000Z","terminalAt":null
            })).expect("snapshot");
            Ok(ConversationOperationWaitResult {
                operation,
                output: ConversationOperationWaitOutput::Pending,
            })
        })
    }
}

#[tokio::test(start_paused = true)]
async fn provider_wait_uses_requested_server_window_plus_transport_allowance() {
    let (client_stream, server_stream) = tokio::net::UnixStream::pair().expect("pair");
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .expect("identity")
    .with_provider_conversation_backend(Arc::new(DelayedWaitBackend {
        entered: std::sync::Mutex::new(Some(entered_tx)),
        delay: std::time::Duration::from_secs(40),
    }));
    let server = tokio::spawn(serve_control_connection(server_stream, identity));
    let mut client = ControlClient::initialize(client_stream, "wait-proof", "1")
        .await
        .expect("initialize");
    let operation_id = OperationId::generate();
    let result = {
        let wait =
            client.wait_for_provider_conversation_operation(ConversationOperationWaitRequest {
                operation_id: operation_id.clone(),
                timeout_seconds: PositiveSeconds::try_from(60).expect("timeout"),
            });
        tokio::pin!(wait);
        assert!(futures_util::poll!(&mut wait).is_pending());
        entered_rx.await.expect("server wait entered");
        tokio::time::advance(std::time::Duration::from_secs(40)).await;
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        wait.await
            .expect("40-second settlement remains within requested wait window")
    };
    assert_eq!(result.operation.operation_id, operation_id);
    assert!(matches!(
        result.output,
        ConversationOperationWaitOutput::Pending
    ));
    client.close().await.expect("close");
    server.await.expect("join").expect("server");
}

#[tokio::test(start_paused = true)]
async fn provider_wait_reports_true_transport_timeout_after_allowance() {
    let (client_stream, server_stream) = tokio::net::UnixStream::pair().expect("pair");
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .expect("identity")
    .with_provider_conversation_backend(Arc::new(DelayedWaitBackend {
        entered: std::sync::Mutex::new(Some(entered_tx)),
        delay: std::time::Duration::from_secs(70),
    }));
    let server = tokio::spawn(serve_control_connection(server_stream, identity));
    let mut client = ControlClient::initialize(client_stream, "timeout-proof", "1")
        .await
        .expect("initialize");
    let wait = client.wait_for_provider_conversation_operation(ConversationOperationWaitRequest {
        operation_id: OperationId::generate(),
        timeout_seconds: PositiveSeconds::try_from(60).expect("timeout"),
    });
    tokio::pin!(wait);
    assert!(futures_util::poll!(&mut wait).is_pending());
    entered_rx.await.expect("server wait entered");
    tokio::time::advance(std::time::Duration::from_secs(65)).await;
    assert!(matches!(
        wait.await,
        Err(collaboration_client::ClientError::Timeout)
    ));
    drop(server);
}
