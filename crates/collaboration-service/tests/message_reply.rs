#![allow(clippy::expect_used)]

use automation_storage::AutomationStore;
use collaboration_client::{ClientError, ControlClient, MessageReplyError, MessageReplyRequest};
use collaboration_protocol::{
    DeliveryClientReceipt, DeliveryOutcome, DeliveryReceipt, MessageContent, MessageDelivery,
    MessageText, SessionMessageSendParams, SessionRef,
};
use collaboration_service::{
    AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext,
    DeliveryContractError, DeliveryFuture, DeliveryRequest, ServiceIdentity,
    SessionMessageDelivery, serve_control_connection,
};
use sqlx::{Connection, sqlite::SqliteConnectOptions};
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as TokioMutex;

const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";

#[derive(Default)]
struct RecordingDelivery {
    requests: Mutex<Vec<DeliveryRequest>>,
    fail_next: Mutex<bool>,
    unknown_next: Mutex<bool>,
}

impl RecordingDelivery {
    fn fail_next_delivery(&self) {
        *self.fail_next.lock().expect("failure switch") = true;
    }

    fn return_unknown_next_delivery(&self) {
        *self.unknown_next.lock().expect("unknown switch") = true;
    }
}

impl SessionMessageDelivery for RecordingDelivery {
    fn deliver<'a>(
        &'a self,
        request: DeliveryRequest,
        _: &'a dyn AttemptEvidenceSink,
    ) -> DeliveryFuture<'a, DeliveryReceipt> {
        Box::pin(async move {
            self.requests
                .lock()
                .map_err(|_| DeliveryContractError::ClientOperation)?
                .push(request);
            let fail = {
                let mut fail_next = self
                    .fail_next
                    .lock()
                    .map_err(|_| DeliveryContractError::ClientOperation)?;
                std::mem::take(&mut *fail_next)
            };
            if fail {
                return Err(DeliveryContractError::ClientOperation);
            }
            let unknown = {
                let mut unknown_next = self
                    .unknown_next
                    .lock()
                    .map_err(|_| DeliveryContractError::ClientOperation)?;
                std::mem::take(&mut *unknown_next)
            };
            if unknown {
                return Ok(DeliveryReceipt {
                    outcome: DeliveryOutcome::Unknown,
                    reachability: None,
                    client: None,
                });
            }
            Ok(DeliveryReceipt {
                outcome: DeliveryOutcome::PeerMessageWritten,
                reachability: Some(collaboration_protocol::SessionReachability::ClaudeCodePeer),
                client: Some(DeliveryClientReceipt::ClaudeCodePeer),
            })
        })
    }

    fn reconcile_attempt(
        &self,
        _: AttemptReconciliationContext,
    ) -> DeliveryFuture<'_, AttemptReconciliation> {
        Box::pin(async { Ok(AttemptReconciliation::StillUnknown) })
    }
}

fn session(endpoint_id: &str, session_id: &str) -> SessionRef {
    serde_json::from_value(serde_json::json!({
        "endpoint":{"serviceId":SERVICE_ID,"endpointId":endpoint_id},
        "sessionId":session_id
    }))
    .expect("valid session reference")
}

async fn client_with_store(
    store: Arc<TokioMutex<AutomationStore>>,
    delivery: Arc<RecordingDelivery>,
) -> (ControlClient, tokio::task::JoinHandle<Result<(), String>>) {
    client_with_optional_store(Some(store), delivery).await
}

async fn client_without_store(
    delivery: Arc<RecordingDelivery>,
) -> (ControlClient, tokio::task::JoinHandle<Result<(), String>>) {
    client_with_optional_store(None, delivery).await
}

async fn client_with_optional_store(
    store: Option<Arc<TokioMutex<AutomationStore>>>,
    delivery: Arc<RecordingDelivery>,
) -> (ControlClient, tokio::task::JoinHandle<Result<(), String>>) {
    let mut identity = ServiceIdentity::new(
        SERVICE_ID,
        SERVICE_ID,
        &format!("sha256:{}", "a".repeat(64)),
    )
    .expect("service identity");
    if let Some(store) = store {
        identity = identity.with_automation_store(store);
    }
    let identity = identity.with_session_delivery(delivery);
    let (client_stream, service_stream) = tokio::net::UnixStream::pair().expect("control pair");
    let service = tokio::spawn(async move {
        serve_control_connection(service_stream, identity)
            .await
            .map_err(|error| error.to_string())
    });
    let client = ControlClient::initialize(client_stream, "message-reply-test", "1")
        .await
        .expect("initialize client");
    (client, service)
}

#[tokio::test]
async fn reply_after_multiple_agent_deliveries_targets_the_latest_sender() {
    let temporary = tempfile::tempdir().expect("isolated reply store");
    let store = Arc::new(TokioMutex::new(
        AutomationStore::open(&temporary.path().join("automation.sqlite"))
            .await
            .expect("automation store"),
    ));
    let delivery = Arc::new(RecordingDelivery::default());
    let (mut client, service) = client_with_store(Arc::clone(&store), Arc::clone(&delivery)).await;
    let caller = session("codex-local", "caller-session");
    let first_sender = session("claude-local", "first-sender");
    let latest_sender = session("cursor-local", "latest-sender");

    for sender in [first_sender, latest_sender.clone()] {
        let receipt = client
            .send_agent_message(SessionMessageSendParams {
                target: caller.clone(),
                message: MessageContent::Agent {
                    sender,
                    text: MessageText::try_from("hello".to_owned()).expect("message text"),
                },
                mode: MessageDelivery::Auto,
                generation_guard: None,
                correlation: None,
            })
            .await
            .expect("accepted direct Agent delivery");
        assert_eq!(receipt.outcome, DeliveryOutcome::PeerMessageWritten);
    }

    let reply = client
        .reply_to_latest_agent_sender(MessageReplyRequest {
            caller: caller.clone(),
            expect_sender: Some(latest_sender.clone()),
            text: MessageText::try_from("thanks".to_owned()).expect("reply text"),
        })
        .await
        .expect("reply routes to latest sender");
    assert_eq!(reply.target, latest_sender);
    assert_eq!(reply.receipt.outcome, DeliveryOutcome::PeerMessageWritten);
    assert!(
        reply
            .target_identity
            .starts_with("▶️ cursor-local/latest-s")
    );
    client.close().await.expect("close control client");
    service.await.expect("service join").expect("service close");

    {
        let requests = delivery.requests.lock().expect("recorded delivery calls");
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[2].target, latest_sender);
        assert!(matches!(
            &requests[2].message,
            MessageContent::Agent { sender, text }
                if sender == &caller && text.as_str() == "thanks"
        ));
    }

    let stored_reply = store
        .lock()
        .await
        .latest_agent_sender_for(&latest_sender, chrono::Utc::now())
        .await
        .expect("reply record persists");
    assert_eq!(stored_reply.map(|record| record.sender), Some(caller));
}

#[tokio::test]
async fn expect_sender_refuses_when_the_latest_sender_changed_before_reply() {
    let temporary = tempfile::tempdir().expect("isolated reply store");
    let store = Arc::new(TokioMutex::new(
        AutomationStore::open(&temporary.path().join("automation.sqlite"))
            .await
            .expect("automation store"),
    ));
    let delivery = Arc::new(RecordingDelivery::default());
    let (mut client, service) = client_with_store(Arc::clone(&store), Arc::clone(&delivery)).await;
    let caller = session("codex-local", "changing-sender");
    let expected_sender = session("claude-local", "earlier-sender");
    let latest_sender = session("cursor-local", "newer-sender");

    for sender in [expected_sender.clone(), latest_sender] {
        client
            .send_agent_message(SessionMessageSendParams {
                target: caller.clone(),
                message: MessageContent::Agent {
                    sender,
                    text: MessageText::try_from("message".to_owned()).expect("message text"),
                },
                mode: MessageDelivery::Auto,
                generation_guard: None,
                correlation: None,
            })
            .await
            .expect("accepted Agent message");
    }

    let error = client
        .reply_to_latest_agent_sender(MessageReplyRequest {
            caller,
            expect_sender: Some(expected_sender),
            text: MessageText::try_from("reply".to_owned()).expect("reply text"),
        })
        .await
        .expect_err("stale sender guard must refuse the reply");
    assert!(matches!(
        &error,
        MessageReplyError::Preparation(source)
            if matches!(source.as_ref(), ClientError::Rejected {
                data: Some(data), ..
            } if data["kind"] == "latestSenderMismatch")
    ));

    client.close().await.expect("close control client");
    service.await.expect("service join").expect("service close");
    assert_eq!(delivery.requests.lock().expect("recorded calls").len(), 2);
}

#[tokio::test]
async fn reply_without_an_agent_sender_returns_a_clear_error_and_sends_nothing() {
    let temporary = tempfile::tempdir().expect("isolated reply store");
    let store = Arc::new(TokioMutex::new(
        AutomationStore::open(&temporary.path().join("automation.sqlite"))
            .await
            .expect("automation store"),
    ));
    let delivery = Arc::new(RecordingDelivery::default());
    let (mut client, service) = client_with_store(Arc::clone(&store), Arc::clone(&delivery)).await;
    let caller = session("codex-local", "no-history");

    let error = client
        .reply_to_latest_agent_sender(MessageReplyRequest {
            caller,
            expect_sender: None,
            text: MessageText::try_from("hello".to_owned()).expect("reply text"),
        })
        .await
        .expect_err("reply requires a delivered Agent sender");
    assert!(
        matches!(
            &error,
            MessageReplyError::Preparation(source)
                if matches!(source.as_ref(), ClientError::Rejected {
                    data: Some(data), ..
                } if data["kind"] == "latestSenderUnknown"
                    && data["message"] == "latest sender unknown; use message send --to <SessionRef>")
        ),
        "unexpected reply error: {error:?}"
    );

    client.close().await.expect("close control client");
    service.await.expect("service join").expect("service close");
    assert!(delivery.requests.lock().expect("recorded calls").is_empty());
}

#[tokio::test]
async fn missing_reply_store_does_not_change_an_accepted_send_outcome() {
    let delivery = Arc::new(RecordingDelivery::default());
    let (mut client, service) = client_without_store(Arc::clone(&delivery)).await;
    let caller = session("codex-local", "store-unavailable");
    let sender = session("claude-local", "sender");

    let receipt = client
        .send_agent_message(SessionMessageSendParams {
            target: caller.clone(),
            message: MessageContent::Agent {
                sender,
                text: MessageText::try_from("accepted without reply storage".to_owned())
                    .expect("message text"),
            },
            mode: MessageDelivery::Auto,
            generation_guard: None,
            correlation: None,
        })
        .await
        .expect("missing reply store must not reject accepted delivery");
    assert_eq!(receipt.outcome, DeliveryOutcome::PeerMessageWritten);

    let error = client
        .reply_to_latest_agent_sender(MessageReplyRequest {
            caller,
            expect_sender: None,
            text: MessageText::try_from("reply".to_owned()).expect("reply text"),
        })
        .await
        .expect_err("reply requires the Router automation store");
    assert!(matches!(
        &error,
        MessageReplyError::Preparation(source)
            if matches!(source.as_ref(), ClientError::Rejected {
                data: Some(data), ..
            } if data["kind"] == "replyUnavailable"
                && data["message"] == "reply unavailable: Router automation store not running")
    ));

    client.close().await.expect("close control client");
    service.await.expect("service join").expect("service close");
    assert_eq!(delivery.requests.lock().expect("recorded calls").len(), 1);
}

#[tokio::test]
async fn failed_reply_record_write_keeps_send_accepted_and_invalidates_stale_sender() {
    let temporary = tempfile::tempdir().expect("isolated reply store");
    let database_path = temporary.path().join("automation.sqlite");
    let store = Arc::new(TokioMutex::new(
        AutomationStore::open(&database_path)
            .await
            .expect("automation store"),
    ));
    let delivery = Arc::new(RecordingDelivery::default());
    let (mut client, service) = client_with_store(Arc::clone(&store), Arc::clone(&delivery)).await;
    let caller = session("codex-local", "write-failure-recipient");
    let old_sender = session("claude-local", "old-sender");
    let new_sender = session("cursor-local", "new-sender");

    let first_receipt = client
        .send_agent_message(SessionMessageSendParams {
            target: caller.clone(),
            message: MessageContent::Agent {
                sender: old_sender,
                text: MessageText::try_from("first accepted".to_owned()).expect("message text"),
            },
            mode: MessageDelivery::Auto,
            generation_guard: None,
            correlation: None,
        })
        .await
        .expect("first delivery");
    assert_eq!(first_receipt.outcome, DeliveryOutcome::PeerMessageWritten);

    let mut raw_connection =
        sqlx::SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&database_path))
            .await
            .expect("open trigger connection");
    sqlx::query(
        "CREATE TRIGGER fail_latest_agent_sender_write
         BEFORE INSERT ON latest_agent_senders
         BEGIN SELECT RAISE(FAIL, 'forced latest sender write failure'); END",
    )
    .execute(&mut raw_connection)
    .await
    .expect("install deterministic write failure");
    raw_connection
        .close()
        .await
        .expect("close trigger connection");

    let second_receipt = client
        .send_agent_message(SessionMessageSendParams {
            target: caller.clone(),
            message: MessageContent::Agent {
                sender: new_sender,
                text: MessageText::try_from("second accepted".to_owned()).expect("message text"),
            },
            mode: MessageDelivery::Auto,
            generation_guard: None,
            correlation: None,
        })
        .await
        .expect("reply record failure must not reject accepted delivery");
    assert_eq!(second_receipt.outcome, DeliveryOutcome::PeerMessageWritten);

    assert!(
        store
            .lock()
            .await
            .latest_agent_sender_for(&caller, chrono::Utc::now())
            .await
            .expect("read invalidated reply record")
            .is_none(),
        "failed write must remove the previous sender"
    );
    let error = client
        .reply_to_latest_agent_sender(MessageReplyRequest {
            caller,
            expect_sender: None,
            text: MessageText::try_from("reply".to_owned()).expect("reply text"),
        })
        .await
        .expect_err("reply must not use stale sender after failed write");
    assert!(
        matches!(
            &error,
            MessageReplyError::Preparation(source)
                if matches!(source.as_ref(), ClientError::Rejected {
                    data: Some(data), ..
                } if data["kind"] == "latestSenderUnknown")
        ),
        "unexpected reply error: {error:?}"
    );

    client.close().await.expect("close control client");
    service.await.expect("service join").expect("service close");
    assert_eq!(delivery.requests.lock().expect("recorded calls").len(), 2);
}

#[tokio::test]
async fn unknown_agent_delivery_invalidates_the_previous_reply_target() {
    let temporary = tempfile::tempdir().expect("isolated reply store");
    let store = Arc::new(TokioMutex::new(
        AutomationStore::open(&temporary.path().join("automation.sqlite"))
            .await
            .expect("automation store"),
    ));
    let delivery = Arc::new(RecordingDelivery::default());
    let (mut client, service) = client_with_store(Arc::clone(&store), Arc::clone(&delivery)).await;
    let caller = session("codex-local", "uncertain-recipient");
    let old_sender = session("claude-local", "old-sender");
    let new_sender = session("cursor-local", "uncertain-sender");

    client
        .send_agent_message(SessionMessageSendParams {
            target: caller.clone(),
            message: MessageContent::Agent {
                sender: old_sender,
                text: MessageText::try_from("accepted".to_owned()).expect("message text"),
            },
            mode: MessageDelivery::Auto,
            generation_guard: None,
            correlation: None,
        })
        .await
        .expect("seed prior accepted sender");
    delivery.fail_next_delivery();

    let error = client
        .send_agent_message(SessionMessageSendParams {
            target: caller.clone(),
            message: MessageContent::Agent {
                sender: new_sender,
                text: MessageText::try_from("uncertain".to_owned()).expect("message text"),
            },
            mode: MessageDelivery::Auto,
            generation_guard: None,
            correlation: None,
        })
        .await
        .expect_err("lost delivery outcome remains uncertain");
    assert!(matches!(
        error,
        ClientError::Rejected {
            data: Some(data), ..
        } if data["kind"] == "outcomeUnknown"
    ));
    assert!(
        store
            .lock()
            .await
            .latest_agent_sender_for(&caller, chrono::Utc::now())
            .await
            .expect("reply address query")
            .is_none(),
        "an unknown latest delivery must remove the stale reply address"
    );
    client.close().await.expect("close control client");
    service.await.expect("service join").expect("service close");
}

#[tokio::test]
async fn unknown_receipt_agent_delivery_invalidates_the_previous_reply_target() {
    let temporary = tempfile::tempdir().expect("isolated reply store");
    let store = Arc::new(TokioMutex::new(
        AutomationStore::open(&temporary.path().join("automation.sqlite"))
            .await
            .expect("automation store"),
    ));
    let delivery = Arc::new(RecordingDelivery::default());
    let (mut client, service) = client_with_store(Arc::clone(&store), Arc::clone(&delivery)).await;
    let caller = session("codex-local", "unknown-receipt-recipient");
    let old_sender = session("claude-local", "old-sender");
    let uncertain_sender = session("cursor-local", "uncertain-sender");

    client
        .send_agent_message(SessionMessageSendParams {
            target: caller.clone(),
            message: MessageContent::Agent {
                sender: old_sender,
                text: MessageText::try_from("accepted".to_owned()).expect("message text"),
            },
            mode: MessageDelivery::Auto,
            generation_guard: None,
            correlation: None,
        })
        .await
        .expect("seed prior accepted sender");
    delivery.return_unknown_next_delivery();

    let receipt = client
        .send_agent_message(SessionMessageSendParams {
            target: caller.clone(),
            message: MessageContent::Agent {
                sender: uncertain_sender,
                text: MessageText::try_from("unknown outcome".to_owned()).expect("message text"),
            },
            mode: MessageDelivery::Auto,
            generation_guard: None,
            correlation: None,
        })
        .await
        .expect("unknown receipt preserves the protocol outcome");
    assert_eq!(receipt.outcome, DeliveryOutcome::Unknown);
    assert!(
        store
            .lock()
            .await
            .latest_agent_sender_for(&caller, chrono::Utc::now())
            .await
            .expect("reply address query")
            .is_none(),
        "an unknown receipt must remove the stale reply address"
    );

    client.close().await.expect("close control client");
    service.await.expect("service join").expect("service close");
}
