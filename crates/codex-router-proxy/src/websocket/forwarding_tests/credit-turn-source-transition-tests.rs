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
use codex_router_core::credit_usage::CreditUsagePolicy;
use codex_router_core::routes::RouteBand;
use codex_router_state::quota_snapshot::PersistedSelectorQuotaWindow;
use codex_router_state::quota_snapshot::SelectorQuotaWindowStatus;
use std::time::Duration;
use tokio::sync::mpsc;

#[tokio::test]
async fn ordinary_unknown_fallback_source_does_not_reconnect_before_create() {
    let directory = CreditTurnTestDirectory::new();
    let fixture = CreditTurnFixture::new(&directory).await;
    fixture
        .writer
        .save_account_credit_usage_policy(&fixture.account_id, CreditUsagePolicy::Disallow)
        .await
        .expect("credits should be disabled for an ordinary fallback source");
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
            .with_effective(false)
            .with_observed_unix_seconds(CREDIT_TURN_FIXTURE_TIME)
        })
        .collect();
    fixture
        .replace_responses_snapshot(
            unknown_windows,
            &CreditProviderObservation::new(
                CreditAvailability::Unknown,
                CreditSpendControl::Unreported,
                None,
            ),
        )
        .await;

    let (_intent_sender, intent) = watch::channel(FloorSwitchIntent::default());
    let early_reconnect = CancellationToken::new();
    let admission = AccountTurnAdmission::new(
        intent,
        early_reconnect.clone(),
        CancellationToken::new(),
        Some(fixture.account_id.clone()),
        Some(1),
        Some(Arc::new(fixture.assessor.clone())),
        false,
    );

    assert!(
        !admission.before_next_create().await,
        "ordinary unknown-fallback sessions retain existing turn behavior"
    );
    assert!(
        !early_reconnect.is_cancelled(),
        "unknown quota alone must not trigger the new credit reconnect"
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

struct FixedOrdinaryFallbackSelector {
    account_id: AccountId,
    selection_reason: &'static str,
}

impl AsyncAccountDecisionSelector for FixedOrdinaryFallbackSelector {
    fn select_upstream_account<'a>(
        &'a self,
        _request: &'a HttpProxyRequest,
        _token_generation: LocalTokenGeneration,
        _affinity_secret: Option<&'a RouterAffinityHashSecret>,
    ) -> BoxFuture<'a, Result<SelectedAccountDecision, HttpProxyError>> {
        Box::pin(async move {
            Ok(SelectedAccountDecision::new(
                self.account_id.clone(),
                self.selection_reason,
            ))
        })
    }
}

async fn ordinary_unknown_source_continues_for_two_websocket_turns() {
    let directory = CreditTurnTestDirectory::new();
    let fixture = CreditTurnFixture::new(&directory).await;
    fixture
        .writer
        .save_account_credit_usage_policy(&fixture.account_id, CreditUsagePolicy::Disallow)
        .await
        .expect("credits should be disabled for ordinary fallback");
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
            .with_effective(false)
            .with_observed_unix_seconds(CREDIT_TURN_FIXTURE_TIME)
        })
        .collect();
    fixture
        .replace_responses_snapshot(
            unknown_windows,
            &CreditProviderObservation::new(
                CreditAvailability::Unknown,
                CreditSpendControl::Unreported,
                None,
            ),
        )
        .await;

    let (observed_source_results, mut source_results) = mpsc::unbounded_channel();
    let assessor = Arc::new(ObservedRuntimeAccountAssessor {
        assessor: fixture.assessor.clone(),
        observed_source_results,
    });
    let selector = FixedOrdinaryFallbackSelector {
        account_id: fixture.account_id.clone(),
        selection_reason: "unknown_fallback_preferred",
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
    let client_websocket =
        WebSocketStream::from_raw_socket(client_stream, Role::Client, None).await;
    let router_future = tunnel.handle_upgraded_connection(
        router_local_websocket,
        WebSocketHandshakeRequest::new(),
        &upstream_url,
    );
    let peer_future = async move {
        let mut client_websocket = client_websocket;
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

        for turn in 1..=2 {
            client_websocket
                .send(Message::text(format!(
                    r#"{{"type":"response.create","turn":{turn}}}"#
                )))
                .await
                .expect("ordinary fallback response create should send");
            assert_eq!(
                wait_for_source_assessment(&mut source_results).await,
                AccountSourceAdmission::Permitted,
                "ordinary Unknown source should preserve existing turn admission"
            );
            let create = tokio::time::timeout(Duration::from_secs(2), upstream_websocket.next())
                .await
                .expect("ordinary fallback create should reach upstream")
                .expect("response create frame should exist")
                .expect("response create frame should decode");
            assert_eq!(
                create.to_string(),
                format!(r#"{{"type":"response.create","turn":{turn}}}"#)
            );
            upstream_websocket
                .send(Message::text(format!(
                    r#"{{"type":"response.completed","turn":{turn}}}"#
                )))
                .await
                .expect("ordinary fallback terminal should send");
            assert_eq!(
                next_client_text(&mut client_websocket).await,
                format!(r#"{{"type":"response.completed","turn":{turn}}}"#)
            );
        }
    };
    let (router_result, ()) = tokio::time::timeout(Duration::from_secs(8), async {
        tokio::join!(router_future, peer_future)
    })
    .await
    .expect("ordinary fallback WebSocket turns should finish");
    assert!(
        router_result.is_ok(),
        "router should finish: {router_result:?}"
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

#[tokio::test]
async fn ordinary_unknown_source_continues_on_websocket() {
    ordinary_unknown_source_continues_for_two_websocket_turns().await;
}

#[tokio::test]
async fn included_source_observes_credit_transition_and_saved_disallow_before_next_create() {
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
    let selector = FixedOrdinaryFallbackSelector {
        account_id: fixture.account_id.clone(),
        selection_reason: "available_same_pool",
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
            .expect("initial included turn should send");
        assert_eq!(
            wait_for_source_assessment(&mut source_results).await,
            AccountSourceAdmission::Permitted,
            "initial included quota should keep the source eligible"
        );
        let first_create = tokio::time::timeout(Duration::from_secs(2), upstream_websocket.next())
            .await
            .expect("initial included create should reach upstream")
            .expect("initial create frame should exist")
            .expect("initial create frame should decode");
        assert_eq!(
            first_create.to_string(),
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
            .expect("new credit-backed turn should queue");
        assert_eq!(
            wait_for_source_assessment(&mut source_results).await,
            AccountSourceAdmission::PermittedByCreditBackedQuota,
            "a healthy-to-known-zero transition with fresh Allow credits should be admitted"
        );
        let second_create = tokio::time::timeout(Duration::from_secs(2), upstream_websocket.next())
            .await
            .expect("new credit-backed create should reach upstream")
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
            .expect("credit opt-out should persist");
        client_websocket
            .send(Message::text(r#"{"type":"response.create","turn":3}"#))
            .await
            .expect("disallowed next create should queue on the active source");
        assert_eq!(
            wait_for_source_assessment(&mut source_results).await,
            AccountSourceAdmission::ReconnectRequired,
            "the always-on reassessment must observe saved Disallow after included-to-credit transition"
        );

        upstream_websocket
            .send(Message::text(
                r#"{"type":"response.output_text.delta","delta":"credit transition turn output"}"#,
            ))
            .await
            .expect("active turn output should still send");
        upstream_websocket
            .send(Message::text(r#"{"type":"response.completed","turn":2}"#))
            .await
            .expect("active turn terminal should still send");
        assert_eq!(
            next_client_text(&mut client_websocket).await,
            r#"{"type":"response.output_text.delta","delta":"credit transition turn output"}"#
        );
        assert_eq!(
            next_client_text(&mut client_websocket).await,
            r#"{"type":"response.completed","turn":2}"#
        );
        let reconnect = tokio::time::timeout(Duration::from_secs(2), client_websocket.next())
            .await
            .expect("saved Disallow should reconnect after the active terminal")
            .expect("reconnect frame should exist")
            .expect("reconnect frame should decode");
        assert_eq!(reconnect.to_string(), CODEX_WEBSOCKET_RECONNECT_SIGNAL);

        let upstream_end = tokio::time::timeout(Duration::from_secs(2), upstream_websocket.next())
            .await
            .expect("old upstream should close after the denied source create");
        assert!(
            matches!(upstream_end, None | Some(Ok(Message::Close(_)))),
            "denied turn 3 must not reach upstream: {upstream_end:?}"
        );
    };
    let (router_result, ()) = tokio::time::timeout(Duration::from_secs(8), async {
        tokio::join!(router_future, peer_future)
    })
    .await
    .expect("included-to-credit transition should finish through the loopback WebSocket");
    assert!(
        router_result.is_ok(),
        "router should finish after saved credit opt-out: {router_result:?}"
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
