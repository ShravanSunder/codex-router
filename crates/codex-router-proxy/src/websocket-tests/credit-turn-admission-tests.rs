use super::*;

use super::credit_turn_test_support::CreditTurnFixture;
use super::credit_turn_test_support::CreditTurnTestDirectory;
use super::credit_turn_test_support::ObservedRuntimeAccountAssessor;
use super::credit_turn_test_support::next_client_text;
use super::credit_turn_test_support::wait_for_source_assessment;
use crate::account_selection::AccountSourceAdmission;
use crate::account_selection::LiveAccountAdmissionAssessor;
use codex_router_core::credit_usage::CreditUsagePolicy;
use codex_router_core::routes::RouteBand;
use futures_util::future::pending;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::sync::mpsc;

#[tokio::test]
async fn credit_source_assessment_does_not_wait_for_selection_reservation_lock() {
    let directory = CreditTurnTestDirectory::new();
    let fixture = CreditTurnFixture::new(&directory).await;
    let selection_lock = fixture.selection_reservation_lock();
    let _selection_guard = selection_lock.lock().await;

    let assessment = tokio::time::timeout(
        crate::websocket::POST_EXHAUSTION_ALTERNATIVE_SELECTION_TIMEOUT,
        fixture
            .assessor
            .assess_source_account(&fixture.account_id, 1, RouteBand::Responses, true),
    )
    .await
    .expect("read-only source assessment should finish inside the existing 250ms bound");

    assert_eq!(
        assessment,
        AccountSourceAdmission::PermittedByCreditBackedQuota
    );
    drop(_selection_guard);
    fixture
        .writer
        .close()
        .await
        .expect("fixture writer should close");
    fixture
        .reader
        .close()
        .await
        .expect("fixture reader should close");
}

#[tokio::test]
async fn policy_revocation_waits_for_current_terminal_and_never_forwards_the_next_create() {
    assert_policy_revocation_waits_for_terminal_delivery(
        r#"{"type":"response.completed","turn":2}"#,
    )
    .await;
}

#[tokio::test]
async fn policy_revocation_rechecks_after_failed_terminal_delivery() {
    assert_policy_revocation_waits_for_terminal_delivery(r#"{"type":"response.failed","turn":2}"#)
        .await;
}

#[tokio::test]
async fn policy_revocation_rechecks_after_incomplete_terminal_delivery() {
    assert_policy_revocation_waits_for_terminal_delivery(
        r#"{"type":"response.incomplete","response":{"id":"resp_2","status":"incomplete"}}"#,
    )
    .await;
}

#[tokio::test]
async fn policy_revocation_rechecks_after_non_quota_error_delivery() {
    assert_policy_revocation_waits_for_terminal_delivery(
        r#"{"type":"error","error":{"type":"invalid_request_error","code":"invalid_request_error","message":"terminal request error"}}"#,
    )
    .await;
}

async fn assert_policy_revocation_waits_for_terminal_delivery(terminal_event: &'static str) {
    let directory = CreditTurnTestDirectory::new();
    let fixture = CreditTurnFixture::new(&directory).await;
    assert_eq!(
        fixture
            .assessor
            .assess_source_account(&fixture.account_id, 1, RouteBand::Responses, true,)
            .await,
        AccountSourceAdmission::PermittedByCreditBackedQuota,
        "fresh positive credit authority should permit an exhausted account"
    );
    let (observed_source_results, mut source_results) = mpsc::unbounded_channel();
    let assessor = Arc::new(ObservedRuntimeAccountAssessor {
        assessor: fixture.assessor.clone(),
        observed_source_results,
    });
    let selector = FixedAsyncSelector {
        account_id: fixture.account_id.clone(),
        credit_backed_at_selection: true,
    };
    let credential_resolver = FixedAsyncCredentialResolver {
        account_id: fixture.account_id.clone(),
    };
    let auth_gate = ProxyLocalAuthGate::disabled();
    let affinity_secret_provider = FixedAffinitySecretProvider::new();
    let protocol_router = WebSocketProtocolRouter::new();
    let tunnel = AsyncWebSocketTunnel::new(
        &auth_gate,
        &selector,
        &credential_resolver,
        &protocol_router,
    )
    .with_affinity_secret_provider(&affinity_secret_provider)
    .with_account_admission_assessor(assessor);
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback WebSocket upstream should bind");
    let upstream_url = format!(
        "ws://{}/v1/responses",
        upstream_listener
            .local_addr()
            .expect("loopback upstream address should be available")
    );
    let (router_local_stream, client_stream) = tokio::io::duplex(8192);
    let router_local_websocket =
        WebSocketStream::from_raw_socket(router_local_stream, Role::Server, None).await;
    let mut client_websocket =
        WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
    let router_future = tunnel.handle_upgraded_connection(
        router_local_websocket,
        WebSocketHandshakeRequest::new(),
        &upstream_url,
    );
    let peer_future = async {
        client_websocket
            .send(Message::text(r#"{"type":"session.update"}"#))
            .await
            .expect("session update should send");
        let (upstream_stream, _) = upstream_listener
            .accept()
            .await
            .expect("loopback upstream should accept WebSocket connection");
        let mut upstream_websocket = tokio_tungstenite::accept_async(upstream_stream)
            .await
            .expect("upstream WebSocket should upgrade");
        let session_update =
            tokio::time::timeout(Duration::from_secs(2), upstream_websocket.next())
                .await
                .expect("session update should reach upstream")
                .expect("session update frame should exist")
                .expect("session update frame should decode");
        assert_eq!(session_update.to_string(), r#"{"type":"session.update"}"#);

        client_websocket
            .send(Message::text(r#"{"type":"response.create","turn":1}"#))
            .await
            .expect("first response create should send");
        assert_eq!(
            wait_for_source_assessment(&mut source_results).await,
            AccountSourceAdmission::PermittedByCreditBackedQuota
        );
        let first_create = tokio::time::timeout(Duration::from_secs(2), upstream_websocket.next())
            .await
            .expect("first create should reach upstream")
            .expect("first create frame should exist")
            .expect("first create frame should decode");
        assert_eq!(
            first_create.to_string(),
            r#"{"type":"response.create","turn":1}"#
        );
        upstream_websocket
            .send(Message::text(
                r#"{"type":"response.output_text.delta","delta":"turn one output"}"#,
            ))
            .await
            .expect("first turn output should send");
        assert_eq!(
            next_client_text(&mut client_websocket).await,
            r#"{"type":"response.output_text.delta","delta":"turn one output"}"#
        );
        upstream_websocket
            .send(Message::text(r#"{"type":"response.completed","turn":1}"#))
            .await
            .expect("first turn terminal should send");
        assert_eq!(
            next_client_text(&mut client_websocket).await,
            r#"{"type":"response.completed","turn":1}"#
        );

        client_websocket
            .send(Message::text(r#"{"type":"response.create","turn":2}"#))
            .await
            .expect("still-allowed next turn should send");
        assert_eq!(
            wait_for_source_assessment(&mut source_results).await,
            AccountSourceAdmission::PermittedByCreditBackedQuota,
            "positive credit balance should retain admission on an existing socket"
        );
        let second_create = tokio::time::timeout(Duration::from_secs(2), upstream_websocket.next())
            .await
            .expect("still-allowed create should reach upstream")
            .expect("second create frame should exist")
            .expect("second create frame should decode");
        assert_eq!(
            second_create.to_string(),
            r#"{"type":"response.create","turn":2}"#
        );

        fixture
            .writer
            .save_account_credit_usage_policy(&fixture.account_id, CreditUsagePolicy::Disallow)
            .await
            .expect("credit policy revocation should persist");
        client_websocket
            .send(Message::text(r#"{"type":"response.create","turn":3}"#))
            .await
            .expect("disallowed next turn should queue on the active socket");
        assert_eq!(
            wait_for_source_assessment(&mut source_results).await,
            AccountSourceAdmission::ReconnectRequired,
            "persisted Disallow should fail closed for the active source account"
        );

        upstream_websocket
            .send(Message::text(
                r#"{"type":"response.output_text.delta","delta":"turn two final output"}"#,
            ))
            .await
            .expect("active turn output should send after the later create is queued");
        upstream_websocket
            .send(Message::text(terminal_event))
            .await
            .expect("active turn terminal should send after output");
        assert_eq!(
            next_client_text(&mut client_websocket).await,
            r#"{"type":"response.output_text.delta","delta":"turn two final output"}"#
        );
        assert_eq!(
            next_client_text(&mut client_websocket).await,
            terminal_event,
            "the current terminal event must reach the client before admission rechecks"
        );
        assert_eq!(
            wait_for_source_assessment(&mut source_results).await,
            AccountSourceAdmission::ReconnectRequired,
            "source eligibility must be assessed again after terminal delivery"
        );
        let reconnect = tokio::time::timeout(Duration::from_secs(2), client_websocket.next())
            .await
            .expect("policy revocation should reconnect after terminal delivery")
            .expect("reconnect signal should exist")
            .expect("reconnect signal should decode");
        assert_eq!(reconnect.to_string(), CODEX_WEBSOCKET_RECONNECT_SIGNAL);

        let upstream_end = tokio::time::timeout(Duration::from_secs(2), upstream_websocket.next())
            .await
            .expect("upstream should close after source admission is revoked");
        assert!(
            matches!(upstream_end, None | Some(Ok(Message::Close(_)))),
            "queued disallowed create must not reach upstream: {upstream_end:?}"
        );
    };
    let (router_result, ()) = tokio::time::timeout(Duration::from_secs(8), async {
        tokio::join!(router_future, peer_future)
    })
    .await
    .expect("credit-turn integration should complete through the loopback WebSocket");
    assert!(
        router_result.is_ok(),
        "policy-driven WebSocket reconnect should finish cleanly: {router_result:?}"
    );
    fixture
        .writer
        .close()
        .await
        .expect("credit-turn writer should close");
    fixture
        .reader
        .close()
        .await
        .expect("credit-turn reader should close");
}

struct BlockingSourceReadAssessor {
    entered: Arc<Notify>,
}

impl LiveAccountAdmissionAssessor for BlockingSourceReadAssessor {
    fn assess_peer<'a>(
        &'a self,
        _source_account_id: &'a AccountId,
        _route_band: RouteBand,
    ) -> BoxFuture<'a, crate::account_selection::FloorSwitchPeerAssessment> {
        Box::pin(async { crate::account_selection::FloorSwitchPeerAssessment::NoPeer })
    }

    fn assess_source_account<'a>(
        &'a self,
        _source_account_id: &'a AccountId,
        _pinned_credential_generation: u64,
        _route_band: RouteBand,
        _credit_backed_at_selection: bool,
    ) -> BoxFuture<'a, AccountSourceAdmission> {
        Box::pin(async move {
            self.entered.notify_one();
            pending().await
        })
    }
}

#[tokio::test]
async fn a_closed_source_state_pool_fails_closed_before_admitting_a_create() {
    let directory = CreditTurnTestDirectory::new();
    let fixture = CreditTurnFixture::new(&directory).await;
    fixture
        .reader
        .close()
        .await
        .expect("source read pool should close for failure proof");
    assert_eq!(
        fixture
            .assessor
            .assess_source_account(&fixture.account_id, 1, RouteBand::Responses, true)
            .await,
        AccountSourceAdmission::ReconnectRequired
    );
    fixture
        .writer
        .close()
        .await
        .expect("fixture writer should close");
}

#[tokio::test]
async fn a_source_read_timeout_fails_closed_before_admitting_a_create() {
    let source_account_id =
        AccountId::new("acct_timeout_source").expect("timeout source account id should parse");
    let entered = Arc::new(Notify::new());
    let assessor = Arc::new(BlockingSourceReadAssessor {
        entered: Arc::clone(&entered),
    });
    let (_intent_sender, intent) = watch::channel(FloorSwitchIntent::default());
    let early_reconnect = CancellationToken::new();
    let admission = AccountTurnAdmission::new(
        intent,
        early_reconnect.clone(),
        CancellationToken::new(),
        Some(source_account_id),
        Some(1),
        Some(assessor),
        false,
    )
    .with_credit_backed_admission_seen(true);
    let admission_task = tokio::spawn(async move { admission.before_next_create().await });
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .expect("bounded source read should start");
    let should_reconnect = tokio::time::timeout(Duration::from_secs(2), admission_task)
        .await
        .expect("source read should hit its existing bound")
        .expect("admission task should finish");
    assert!(should_reconnect);
    assert!(early_reconnect.is_cancelled());
}
