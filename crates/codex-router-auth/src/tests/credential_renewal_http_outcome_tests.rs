use super::*;

#[test]
fn loopback_oauth_outcomes_keep_retry_safety_and_redacted_failure_class() {
    use crate::resolver::CredentialRefreshFailure;
    use codex_router_state::credential_maintenance::CredentialFailureClass;

    for (response, expected) in [
        (
            "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 120\r\nContent-Length: 2\r\n\r\n{}",
            CredentialRefreshFailure::confirmed_unspent(
                CredentialFailureClass::RateLimited,
                Some(120),
            ),
        ),
        (
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 35\r\n\r\n{\"error\":\"temporarily_unavailable\"}",
            CredentialRefreshFailure::confirmed_unspent(
                CredentialFailureClass::ProviderTemporary,
                None,
            ),
        ),
        (
            "HTTP/1.1 400 Bad Request\r\nContent-Length: 25\r\n\r\n{\"error\":\"invalid_grant\"}",
            CredentialRefreshFailure::ambiguous(CredentialFailureClass::ProviderRejected),
        ),
        (
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}",
            CredentialRefreshFailure::ambiguous(CredentialFailureClass::MalformedResponse),
        ),
        (
            "HTTP/1.1 200 OK\r\nContent-Length: 19\r\n\r\n{\"access_token\":\"\"}",
            CredentialRefreshFailure::ambiguous(CredentialFailureClass::MalformedResponse),
        ),
        (
            "HTTP/1.1 200 OK\r\nContent-Length: 44\r\n\r\n{\"access_token\":\"access\",\"refresh_token\":\"\"}",
            CredentialRefreshFailure::ambiguous(CredentialFailureClass::MalformedResponse),
        ),
        (
            "HTTP/1.1 200 OK\r\nContent-Length: 47\r\n\r\n{\"access_token\":\"access\",\"refresh_token\":\"   \"}",
            CredentialRefreshFailure::ambiguous(CredentialFailureClass::MalformedResponse),
        ),
    ] {
        let listener = must_ok(TcpListener::bind("127.0.0.1:0"));
        let address = must_ok(listener.local_addr());
        let server_thread = thread::spawn(move || {
            let (mut stream, _) = must_ok(listener.accept());
            let mut request = [0_u8; 2048];
            let _ = must_ok(stream.read(&mut request));
            must_ok(stream.write_all(response.as_bytes()));
        });
        let client = OpenAiOAuthRefreshClient::new_with_endpoint(
            format!("http://{address}/oauth/token"),
            "test-client",
        );
        let result = client.refresh_credentials(
            &account_id("loopback-outcome"),
            &SecretString::new("refresh-token-canary"),
        );
        assert_eq!(result.err(), Some(expected));
        assert!(!format!("{expected:?}").contains("refresh-token-canary"));
        must_ok(server_thread.join().map_err(|_| "loopback server failed"));
    }
}

#[test]
fn loopback_oauth_response_without_replacement_reuses_refresh_token() {
    let listener = must_ok(TcpListener::bind("127.0.0.1:0"));
    let address = must_ok(listener.local_addr());
    let server_thread = thread::spawn(move || {
        let (mut stream, _) = must_ok(listener.accept());
        let mut request = [0_u8; 2048];
        let _ = must_ok(stream.read(&mut request));
        must_ok(stream.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Length: 26\r\n\r\n{\"access_token\":\"renewed\"}",
        ));
    });
    let client = OpenAiOAuthRefreshClient::new_with_endpoint(
        format!("http://{address}/oauth/token"),
        "test-client",
    );
    let bundle = must_ok(client.refresh_credentials(
        &account_id("loopback-reuse"),
        &SecretString::new("refresh-token-canary"),
    ));
    assert_eq!(
        bundle.refresh_token().map(SecretString::expose_secret),
        Some("refresh-token-canary")
    );
    must_ok(server_thread.join().map_err(|_| "loopback server failed"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn truncated_429_body_preserves_retry_after_and_blocks_refresh_before_cooldown() {
    use codex_router_state::credential_maintenance::CredentialFailureClass;
    use codex_router_state::credential_maintenance::CredentialMaintenanceState;

    let temp_dir = AuthTestTempDir::new("truncated-429-cooldown");
    let state = must_ok(AsyncSqliteStateStore::open(&temp_dir.path().join("state.sqlite")).await);
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            temp_dir.path().join("secrets"),
        ),
    );
    let account_id = account_id("truncated-429-account");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    codex_router_core::provider::Provider::Openai,
                    account_id.clone(),
                    "rate limited",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await,
    );
    let active_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &active_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "expired-access-canary",
                    Some("refresh-token-canary".to_owned()),
                )
                .with_expires_unix_seconds(900)
                .to_secret_string(),
            ),
        ),
    );

    let listener = must_ok(TcpListener::bind("127.0.0.1:0"));
    let address = must_ok(listener.local_addr());
    let server_listener = must_ok(listener.try_clone());
    let server_thread = thread::spawn(move || {
        let (mut stream, _) = must_ok(server_listener.accept());
        let mut request = [0_u8; 2048];
        let _ = must_ok(stream.read(&mut request));
        must_ok(stream.write_all(
            b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 120\r\nContent-Length: 80\r\nConnection: close\r\n\r\n{",
        ));
    });
    let client = OpenAiOAuthRefreshClient::new_with_endpoint(
        format!("http://{address}/oauth/token"),
        "test-client",
    );
    let resolver = AsyncRouterCredentialResolver::new(state.clone(), secrets, client, Some(1_000));

    assert_eq!(
        resolver.resolve_provider_credentials(&account_id).await,
        Err(CredentialResolverError::RefreshUnavailable)
    );
    must_ok(server_thread.join().map_err(|_| "loopback server failed"));
    let maintenance = must_ok(state.load_credential_maintenance(&account_id).await)
        .expect("429 disposition should persist");
    assert_eq!(maintenance.state, CredentialMaintenanceState::Retrying);
    assert_eq!(
        maintenance.failure_class,
        Some(CredentialFailureClass::RateLimited)
    );
    assert_eq!(maintenance.next_attempt_unix_seconds, Some(1_120));
    assert_eq!(
        resolver.resolve_provider_credentials(&account_id).await,
        Err(CredentialResolverError::RefreshUnavailable)
    );
    must_ok(listener.set_nonblocking(true));
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "cooldown must not send a second provider request"
    );
}
