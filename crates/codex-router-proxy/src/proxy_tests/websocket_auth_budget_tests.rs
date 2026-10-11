use super::auth_rejection_fixtures::AuthRejectionFixture;
use super::*;

struct BudgetCandidateSelector {
    candidates: Vec<AccountId>,
    selected: Arc<Mutex<Vec<AccountId>>>,
}

impl AsyncAccountDecisionSelector for BudgetCandidateSelector {
    fn select_upstream_account<'a>(
        &'a self,
        request: &'a HttpProxyRequest,
        _token_generation: TokenGeneration,
        _affinity_secret: Option<&'a RouterAffinityHashSecret>,
    ) -> BoxFuture<'a, Result<SelectedAccountDecision, HttpProxyError>> {
        Box::pin(async move {
            let candidate = self
                .candidates
                .iter()
                .find(|candidate| !request.excluded_accounts().contains(candidate))
                .ok_or(HttpProxyError::Selection {
                    reason: QuotaAwareAccountSelectorError::NoEligibleAccounts,
                })?;
            self.selected
                .lock()
                .expect("selection records should lock")
                .push(candidate.clone());
            Ok(SelectedAccountDecision::new(
                candidate.clone(),
                "budget_fixture",
            ))
        })
    }
}

struct AlternatingCredentialResolver;

impl AsyncProviderCredentialResolver for AlternatingCredentialResolver {
    fn resolve_provider_credentials<'a>(
        &'a self,
        account_id: &'a AccountId,
        _expected_provider: Provider,
    ) -> BoxFuture<'a, Result<ResolvedProviderCredential, CredentialResolverError>> {
        Box::pin(async move {
            let candidate_index: usize = account_id
                .as_str()
                .rsplit('-')
                .next()
                .expect("candidate index should exist")
                .parse()
                .expect("candidate index should parse");
            if candidate_index.is_multiple_of(2) {
                return Err(CredentialResolverError::SecretUnavailable);
            }
            Ok(ResolvedProviderCredential::new(
                account_id.clone(),
                SecretString::new(account_id.as_str()),
                1,
            ))
        })
    }
}

#[tokio::test]
#[allow(clippy::result_large_err)]
async fn websocket_auth_rejection_shares_one_budget_with_credential_failures() {
    let fixture = AuthRejectionFixture::new("websocket_shared_auth_budget");
    let state = must_ok(AsyncSqliteStateStore::open(&fixture.database_path).await);
    let candidates: Vec<_> = (0..17)
        .map(|index| account_id(&format!("auth-budget-{index:02}")))
        .collect();
    for candidate in &candidates {
        must_ok(
            state
                .upsert_account(
                    &AccountRecord::new(
                        Provider::Openai,
                        candidate.clone(),
                        "budget",
                        AccountStatus::Enabled,
                    )
                    .with_active_credential_generation(1),
                )
                .await,
        );
    }
    let selected = Arc::new(Mutex::new(Vec::new()));
    let selector = BudgetCandidateSelector {
        candidates: candidates.clone(),
        selected: Arc::clone(&selected),
    };
    let auth_gate = ProxyLocalAuthGate::disabled();
    let protocol_router = WebSocketProtocolRouter::new();
    let resolver = AlternatingCredentialResolver;
    let registry = WebSocketRevocationRegistry::new();
    let tunnel = AsyncWebSocketTunnel::new(&auth_gate, &selector, &resolver, &protocol_router)
        .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER)
        .with_revocation_registry(registry.clone());
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("upstream should bind");
    let upstream_url = format!(
        "ws://{}/v1/responses",
        upstream_listener.local_addr().expect("address should read")
    );
    let handshake_records = Arc::new(Mutex::new(Vec::new()));
    let router_done = Arc::new(tokio::sync::Notify::new());
    let (router_stream, client_stream) = tokio::io::duplex(4096);
    let local_websocket =
        tokio_tungstenite::WebSocketStream::from_raw_socket(router_stream, Role::Server, None)
            .await;
    let mut client =
        tokio_tungstenite::WebSocketStream::from_raw_socket(client_stream, Role::Client, None)
            .await;

    let server_future = async {
        loop {
            let stream = tokio::select! {
                biased;
                () = router_done.notified() => break,
                accepted = upstream_listener.accept() => accepted.expect("candidate handshake should accept").0,
            };
            let records = Arc::clone(&handshake_records);
            let rejected = tokio_tungstenite::accept_hdr_async(
                stream,
                move |request: &Request, _response: Response| {
                    records.lock().expect("handshake records should lock").push(
                        request
                            .headers()
                            .get("authorization")
                            .expect("candidate auth should exist")
                            .to_str()
                            .expect("candidate auth should decode")
                            .to_owned(),
                    );
                    Err(tokio_tungstenite::tungstenite::http::Response::builder()
                        .status(401)
                        .body(Some("synthetic rejection".to_owned()))
                        .expect("handshake rejection should build"))
                },
            )
            .await;
            assert!(
                matches!(rejected, Err(tokio_tungstenite::tungstenite::Error::Http(response)) if response.status().as_u16() == 401)
            );
        }
    };
    let router_future = async {
        let result = tunnel
            .handle_upgraded_connection(
                local_websocket,
                WebSocketHandshakeRequest::new(),
                &upstream_url,
            )
            .await;
        router_done.notify_one();
        result
    };
    let client_future = async {
        client
            .send(Message::text(r#"{"type":"response.create"}"#))
            .await
            .expect("first frame should send");
        client.next().await
    };
    let (router_result, (), _client_result) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(router_future, server_future, client_future)
    })
    .await
    .expect("shared budget must terminate");
    assert!(matches!(
        router_result,
        Err(crate::websocket::WebSocketTunnelError::CloseReason(
            WebSocketCloseReason::ProviderCredential
        ))
    ));
    assert_eq!(
        *selected.lock().expect("selection records should lock"),
        candidates[..16],
        "all resolver/handshake candidates share one cap; candidate17 is not selected"
    );
    assert_eq!(
        *handshake_records
            .lock()
            .expect("handshake records should lock"),
        (1..16)
            .step_by(2)
            .map(|index| format!("Bearer auth-budget-{index:02}"))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        registry.snapshot().active_sessions,
        0,
        "all failed handshake registrations must be released"
    );
    must_ok(state.close().await);
}
