use super::*;

#[test]
fn credential_diagnostics_identify_local_failures_without_private_payloads_or_egress() {
    for (scenario, malformed_bundle, expected_class) in [
        ("private_path_canary", false, "file_missing"),
        ("private_payload_canary", true, "invalid_payload"),
    ] {
        let temp_dir = ProxyTestTempDir::new(scenario);
        let state = must_ok(SqliteStateStore::open(
            &temp_dir.path().join("state.sqlite"),
        ));
        let secrets = must_ok(
            codex_router_secret_store::test_support::open_encrypted_credential_store(
                temp_dir.path().join("secrets"),
            ),
        );
        let selected_account = account_id("acct_selected");
        let account = AccountRecord::new(
            Provider::Openai,
            selected_account.clone(),
            "private_account_canary",
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(1);
        must_ok(AccountStateRepository::upsert_account(&state, &account));
        if malformed_bundle {
            let key = must_ok(openai_account_credential_bundle_key(&selected_account, 1));
            must_ok(secrets.write_secret(
                &key,
                &SecretString::new(
                    r#"{"version":"private_payload_canary","access_token":"private_token_canary"}"#,
                ),
            ));
        }
        let resolver =
            RouterCredentialResolver::new(&state, &secrets, NoopCredentialRefreshClient, 1_000);
        let upstream = RecordingUpstream::new(HttpProxyResponse::new(
            200,
            HeaderCollection::default(),
            b"must-not-forward".to_vec(),
        ));
        let selector = RecordingSelector::new();
        let auth_gate = local_auth_gate();
        let service =
            AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

        let captured = crate::test_log_capture::capture_log_output(|| {
            let result = service.handle_request(
                HttpProxyRequest::new(Method::Get, "/v1/models")
                    .with_header(Header::new("X-Codex-Router-Token", "current-token")),
            );
            assert!(matches!(
                result,
                Err(HttpProxyError::ProviderCredential { .. })
            ));
        });

        assert!(
            captured.contains(expected_class),
            "missing fixed diagnostic category"
        );
        for private_value in [
            "private_path_canary",
            "private_payload_canary",
            "private_token_canary",
            "private_account_canary",
            "acct_selected",
        ] {
            assert!(
                !captured.contains(private_value),
                "private data reached diagnostics"
            );
        }
        assert!(upstream.take_recorded().is_empty());
    }
}

#[test]
fn http_proxy_missing_refresh_token_fails_closed_before_upstream_egress() {
    let upstream = RecordingUpstream::new(HttpProxyResponse::new(
        200,
        HeaderCollection::default(),
        b"should-not-send".to_vec(),
    ));
    let selector = RecordingSelector::new();
    let resolver =
        RejectingProviderCredentialResolver::new(CredentialResolverError::RefreshUnavailable);
    let auth_gate = local_auth_gate();
    let service = AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
        .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

    let error = match service.handle_request(
        HttpProxyRequest::new(Method::Post, "/v1/responses")
            .with_header(Header::new("X-Codex-Router-Token", "current-token"))
            .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
    ) {
        Ok(response) => panic!("missing refresh token should fail closed: {response:?}"),
        Err(error) => error,
    };

    assert_eq!(
        error,
        HttpProxyError::ProviderCredential {
            reason: CredentialResolverError::RefreshUnavailable
        }
    );
    assert_eq!(resolver.take_recorded(), vec!["acct_selected".to_owned()]);
    assert!(upstream.take_recorded().is_empty());
}

#[test]
fn http_proxy_routes_around_request_local_credential_failure_before_upstream_egress() {
    let first_account_id = account_id("acct_first");
    let second_account_id = account_id("acct_second");
    let upstream = RecordingUpstream::new(HttpProxyResponse::new(
        200,
        HeaderCollection::default(),
        b"ok".to_vec(),
    ));
    let selector = QuotaAwareAccountSelector::new(vec![
        QuotaAwareAccountState::new(
            first_account_id.clone(),
            90,
            SnapshotFreshness::Fresh { age_seconds: 1 },
        ),
        QuotaAwareAccountState::new(
            second_account_id,
            80,
            SnapshotFreshness::Fresh { age_seconds: 1 },
        ),
    ]);
    let resolver =
        FailFirstProviderCredentialResolver::new(first_account_id, "second-account-token");
    let auth_gate = local_auth_gate();
    let service = AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
        .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

    let response = must_ok(
        service.handle_request(
            HttpProxyRequest::new(Method::Post, "/v1/responses")
                .with_header(Header::new("X-Codex-Router-Token", "current-token"))
                .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
        ),
    );

    assert_eq!(response.status(), 200);
    assert_eq!(
        resolver.take_recorded(),
        vec!["acct_first".to_owned(), "acct_second".to_owned()]
    );
    let recorded = upstream.take_recorded();
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].headers().values("authorization"),
        vec!["Bearer second-account-token"]
    );
}

#[test]
fn http_proxy_reports_credential_failure_when_all_request_local_credentials_fail() {
    let first_account_id = account_id("acct_first");
    let second_account_id = account_id("acct_second");
    let upstream = RecordingUpstream::new(HttpProxyResponse::new(
        200,
        HeaderCollection::default(),
        b"should-not-send".to_vec(),
    ));
    let selector = QuotaAwareAccountSelector::new(vec![
        QuotaAwareAccountState::new(
            first_account_id,
            90,
            SnapshotFreshness::Fresh { age_seconds: 1 },
        ),
        QuotaAwareAccountState::new(
            second_account_id,
            80,
            SnapshotFreshness::Fresh { age_seconds: 1 },
        ),
    ]);
    let resolver =
        RejectingProviderCredentialResolver::new(CredentialResolverError::RefreshUnavailable);
    let auth_gate = local_auth_gate();
    let service = AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
        .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);

    let error = match service.handle_request(
        HttpProxyRequest::new(Method::Post, "/v1/responses")
            .with_header(Header::new("X-Codex-Router-Token", "current-token"))
            .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
    ) {
        Ok(response) => panic!("all credential failures should fail closed: {response:?}"),
        Err(error) => error,
    };

    assert_eq!(
        error,
        HttpProxyError::ProviderCredential {
            reason: CredentialResolverError::RefreshUnavailable
        }
    );
    assert_eq!(
        resolver.take_recorded(),
        vec!["acct_first".to_owned(), "acct_second".to_owned()]
    );
    assert!(upstream.take_recorded().is_empty());
}

#[test]
fn authenticated_http_proxy_audits_selection_rejection_after_local_auth() {
    let temp_dir = ProxyTestTempDir::new("http_selection_rejection_audit");
    let audit_path = temp_dir.path().join("audit").join("events.jsonl");
    let audit_sink = AuditFileSink::new(audit_path.clone());
    let upstream = RecordingUpstream::new(HttpProxyResponse::new(
        200,
        HeaderCollection::default(),
        b"should-not-send".to_vec(),
    ));
    let selector = RejectingSelector::new(QuotaAwareAccountSelectorError::NoEligibleAccounts);
    let resolver = RecordingProviderCredentialResolver::new("selected-upstream-token");
    let auth_gate = local_auth_gate();
    let service = AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
        .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER)
        .with_audit_sink(&audit_sink);

    let error = match service.handle_request(
        HttpProxyRequest::new(Method::Post, "/v1/responses")
            .with_header(Header::new("X-Codex-Router-Token", "current-token"))
            .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
    ) {
        Ok(response) => panic!("selection rejection should fail closed: {response:?}"),
        Err(error) => error,
    };

    assert_eq!(
        error,
        HttpProxyError::Selection {
            reason: QuotaAwareAccountSelectorError::NoEligibleAccounts
        }
    );
    assert!(resolver.take_recorded().is_empty());
    assert!(upstream.take_recorded().is_empty());
    let audit_contents = must_ok(fs::read_to_string(&audit_path));
    assert!(audit_contents.contains("\"transport_kind\":\"http\""));
    assert!(audit_contents.contains("\"decision_reason\":\"selection_rejected\""));
    assert!(audit_contents.contains("\"error_class\":\"selection\""));
    assert!(audit_contents.contains("\"response_commit_state\":\"not_committed\""));
    assert!(!audit_contents.contains("current-token"));
}

#[test]
fn authenticated_http_proxy_audits_provider_credential_rejection_after_selection() {
    let temp_dir = ProxyTestTempDir::new("http_credential_rejection_audit");
    let audit_path = temp_dir.path().join("audit").join("events.jsonl");
    let audit_sink = AuditFileSink::new(audit_path.clone());
    let upstream = RecordingUpstream::new(HttpProxyResponse::new(
        200,
        HeaderCollection::default(),
        b"should-not-send".to_vec(),
    ));
    let selector = RecordingSelector::new();
    let resolver =
        RejectingProviderCredentialResolver::new(CredentialResolverError::RefreshUnavailable);
    let auth_gate = local_auth_gate();
    let service = AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
        .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER)
        .with_audit_sink(&audit_sink);

    let error = match service.handle_request(
        HttpProxyRequest::new(Method::Post, "/v1/responses")
            .with_header(Header::new("X-Codex-Router-Token", "current-token"))
            .with_body(br#"{"model":"gpt-5"}"#.to_vec()),
    ) {
        Ok(response) => panic!("credential rejection should fail closed: {response:?}"),
        Err(error) => error,
    };

    assert_eq!(
        error,
        HttpProxyError::ProviderCredential {
            reason: CredentialResolverError::RefreshUnavailable
        }
    );
    assert_eq!(resolver.take_recorded(), vec!["acct_selected".to_owned()]);
    assert!(upstream.take_recorded().is_empty());
    let audit_contents = must_ok(fs::read_to_string(&audit_path));
    assert!(audit_contents.contains("\"transport_kind\":\"http\""));
    assert!(audit_contents.contains("\"decision_reason\":\"credential_rejected\""));
    assert!(audit_contents.contains("\"error_class\":\"provider_credential\""));
    assert!(audit_contents.contains("\"account_hash\""));
    assert!(audit_contents.contains("\"response_commit_state\":\"not_committed\""));
    assert!(!audit_contents.contains("current-token"));
    assert!(!audit_contents.contains("acct_selected"));
}
