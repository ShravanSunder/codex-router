use super::*;

use super::credit_turn_test_support::CREDIT_TURN_FIXTURE_TIME;
use super::credit_turn_test_support::CreditTurnFixture;
use super::credit_turn_test_support::CreditTurnTestDirectory;
use super::credit_turn_test_support::ObservedRuntimeAccountAssessor;
use super::credit_turn_test_support::credit_turn_windows;
use super::credit_turn_test_support::next_client_text;
use super::credit_turn_test_support::wait_for_source_assessment;
use crate::account_selection::AccountSourceAdmission;
use crate::account_selection::AsyncAccountDecisionSelector;
use crate::account_selection::SelectedAccountDecision;
use codex_router_core::credit_usage::CreditAvailability;
use codex_router_core::credit_usage::CreditBalance;
use codex_router_core::credit_usage::CreditProviderLimitReason;
use codex_router_core::credit_usage::CreditProviderObservation;
use codex_router_core::credit_usage::CreditSpendControl;
use codex_router_core::routes::RouteBand;
use codex_router_state::quota_snapshot::PersistedSelectorQuotaWindow;
use codex_router_state::quota_snapshot::SelectorQuotaWindowStatus;
use std::time::Duration;
use tokio::sync::mpsc;

struct FixedIncludedQuotaSelector {
    account_id: AccountId,
}

impl AsyncAccountDecisionSelector for FixedIncludedQuotaSelector {
    fn select_upstream_account<'a>(
        &'a self,
        _request: &'a HttpProxyRequest,
        _token_generation: LocalTokenGeneration,
        _affinity_secret: Option<&'a RouterAffinityHashSecret>,
    ) -> BoxFuture<'a, Result<SelectedAccountDecision, HttpProxyError>> {
        Box::pin(async move {
            Ok(SelectedAccountDecision::new(
                self.account_id.clone(),
                "available_same_pool",
            ))
        })
    }
}

#[tokio::test]
async fn credit_backed_turn_makes_later_unknown_authority_fail_closed() {
    let directory = CreditTurnTestDirectory::new();
    let fixture = CreditTurnFixture::new(&directory).await;
    let available_credits = CreditProviderObservation::new(
        CreditAvailability::Available {
            balance: Some(CreditBalance::new("2.75").expect("balance should validate")),
        },
        CreditSpendControl::Clear,
        Some(CreditProviderLimitReason::RateLimitReached),
    );
    let included_windows = vec![
        PersistedSelectorQuotaWindow::new(
            fixture.account_id.clone(),
            RouteBand::Responses.as_str(),
            18_000,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(40)
        .with_reset_unix_seconds(CREDIT_TURN_FIXTURE_TIME + 18_000)
        .with_effective(true)
        .with_observed_unix_seconds(CREDIT_TURN_FIXTURE_TIME),
        PersistedSelectorQuotaWindow::new(
            fixture.account_id.clone(),
            RouteBand::Responses.as_str(),
            604_800,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(20)
        .with_reset_unix_seconds(CREDIT_TURN_FIXTURE_TIME + 604_800)
        .with_effective(false)
        .with_observed_unix_seconds(CREDIT_TURN_FIXTURE_TIME),
    ];
    fixture
        .replace_responses_snapshot(included_windows, &available_credits)
        .await;

    let (observed_source_results, mut source_results) = mpsc::unbounded_channel();
    let assessor = Arc::new(ObservedRuntimeAccountAssessor {
        assessor: fixture.assessor.clone(),
        observed_source_results,
    });
    let selector = FixedIncludedQuotaSelector {
        account_id: fixture.account_id.clone(),
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
        .expect("loopback upstream should bind");
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
            .expect("loopback upstream should accept the connection");
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
            .expect("initial included create should send");
        assert_eq!(
            wait_for_source_assessment(&mut source_results).await,
            AccountSourceAdmission::Permitted,
            "included source should retain ordinary admission"
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), upstream_websocket.next())
                .await
                .expect("initial included create should reach upstream")
                .expect("initial create frame should exist")
                .expect("initial create frame should decode")
                .to_string(),
            r#"{"type":"response.create","turn":1}"#
        );
        upstream_websocket
            .send(Message::text(r#"{"type":"response.completed","turn":1}"#))
            .await
            .expect("initial included terminal should send");
        assert_eq!(
            next_client_text(&mut client_websocket).await,
            r#"{"type":"response.completed","turn":1}"#
        );

        fixture
            .replace_responses_snapshot(
                credit_turn_windows(&fixture.account_id, CREDIT_TURN_FIXTURE_TIME),
                &available_credits,
            )
            .await;
        client_websocket
            .send(Message::text(r#"{"type":"response.create","turn":2}"#))
            .await
            .expect("known-zero credit-backed create should send");
        let _credit_backed_assessment = wait_for_source_assessment(&mut source_results).await;
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), upstream_websocket.next())
                .await
                .expect("fresh credit-backed create should reach upstream")
                .expect("credit-backed create frame should exist")
                .expect("credit-backed create frame should decode")
                .to_string(),
            r#"{"type":"response.create","turn":2}"#
        );
        upstream_websocket
            .send(Message::text(r#"{"type":"response.completed","turn":2}"#))
            .await
            .expect("credit-backed terminal should send");
        assert_eq!(
            next_client_text(&mut client_websocket).await,
            r#"{"type":"response.completed","turn":2}"#
        );

        let unknown_windows = [18_000, 604_800]
            .into_iter()
            .map(|limit_window_seconds| {
                PersistedSelectorQuotaWindow::new(
                    fixture.account_id.clone(),
                    RouteBand::Responses.as_str(),
                    limit_window_seconds,
                    SelectorQuotaWindowStatus::Unknown,
                )
                .with_remaining_headroom(0)
                .with_reset_unix_seconds(CREDIT_TURN_FIXTURE_TIME + limit_window_seconds)
                .with_effective(limit_window_seconds == 18_000)
                .with_observed_unix_seconds(CREDIT_TURN_FIXTURE_TIME)
            })
            .collect();
        fixture
            .replace_responses_snapshot(unknown_windows, &CreditProviderObservation::missing())
            .await;
        client_websocket
            .send(Message::text(r#"{"type":"response.create","turn":3}"#))
            .await
            .expect("unknown later create should be assessed against current authority");
        assert_eq!(
            wait_for_source_assessment(&mut source_results).await,
            AccountSourceAdmission::ReconnectRequired,
            "Unknown quota after an admitted credit-backed turn cannot fall back to ordinary authority"
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), client_websocket.next())
                .await
                .expect("Unknown authority should trigger reconnect")
                .expect("reconnect frame should exist")
                .expect("reconnect frame should decode")
                .to_string(),
            CODEX_WEBSOCKET_RECONNECT_SIGNAL
        );
        let upstream_end = tokio::time::timeout(Duration::from_secs(2), upstream_websocket.next())
            .await
            .expect("old upstream should close after Unknown authority");
        assert!(
            matches!(upstream_end, None | Some(Ok(Message::Close(_)))),
            "Unknown turn 3 must not reach upstream: {upstream_end:?}"
        );
    };
    let (router_result, ()) = tokio::time::timeout(Duration::from_secs(8), async {
        tokio::join!(router_future, peer_future)
    })
    .await
    .expect("credit-backed-to-Unknown WebSocket transition should finish");
    assert!(
        router_result.is_ok(),
        "router should finish after Unknown authority reconnect: {router_result:?}"
    );
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
