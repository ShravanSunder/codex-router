//! Actual terminal routing paths emit only static resolver failure metadata.
use super::*;
use crate::test_log_capture::{capture_log_output, capture_log_output_async};
use std::sync::atomic::{AtomicUsize, Ordering};

struct TerminalAffinitySelector;
impl AccountDecisionSelector for TerminalAffinitySelector {
    fn select_upstream_account(
        &self,
        _: &HttpProxyRequest,
        _: TokenGeneration,
        _: Option<&RouterAffinityHashSecret>,
    ) -> Result<SelectedAccountDecision, HttpProxyError> {
        Ok(SelectedAccountDecision::new(
            account_id("acct_private_resolver_canary"),
            "previous_response_affinity",
        ))
    }
}
impl AsyncAccountDecisionSelector for TerminalAffinitySelector {
    fn select_upstream_account<'a>(
        &'a self,
        request: &'a HttpProxyRequest,
        generation: TokenGeneration,
        secret: Option<&'a RouterAffinityHashSecret>,
    ) -> BoxFuture<'a, Result<SelectedAccountDecision, HttpProxyError>> {
        Box::pin(async move {
            AccountDecisionSelector::select_upstream_account(self, request, generation, secret)
        })
    }
}
struct TypedRejectingResolver {
    reason: CredentialResolverError,
    calls: AtomicUsize,
}
impl ProviderCredentialResolver for TypedRejectingResolver {
    fn resolve_provider_credentials(
        &self,
        _: &AccountId,
        _: codex_router_core::provider::Provider,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(self.reason.clone())
    }
}
impl AsyncProviderCredentialResolver for TypedRejectingResolver {
    fn resolve_provider_credentials<'a>(
        &'a self,
        account: &'a AccountId,
        provider: codex_router_core::provider::Provider,
    ) -> BoxFuture<'a, Result<ResolvedProviderCredential, CredentialResolverError>> {
        Box::pin(async move {
            ProviderCredentialResolver::resolve_provider_credentials(self, account, provider)
        })
    }
}
fn resolver_failure_cases() -> [(CredentialResolverError, &'static str); 6] {
    [
        (
            CredentialResolverError::AccountUnavailable,
            "account_unavailable",
        ),
        (
            CredentialResolverError::AccountIneligible,
            "account_ineligible",
        ),
        (
            CredentialResolverError::AccountProviderMismatch,
            "account_provider_mismatch",
        ),
        (
            CredentialResolverError::SecretUnavailable,
            "secret_unavailable",
        ),
        (
            CredentialResolverError::CredentialStoreUnavailable,
            "credential_store_unavailable",
        ),
        (
            CredentialResolverError::RefreshUnavailable,
            "refresh_unavailable",
        ),
    ]
}
fn proof_handshake() -> WebSocketHandshakeRequest {
    WebSocketHandshakeRequest::new()
        .with_header(Header::new("X-Codex-Router-Token", "current-token"))
        .with_header(Header::new("session-id", "private_session_canary"))
}
fn assert_safe_variant_log(output: &str, expected: &str) {
    assert!(
        output.contains(r#"event.name="codex_router.proxy.credential_resolution_failed""#),
        "missing real resolver failure event: {output}"
    );
    assert!(
        output.contains(&format!("error.kind=\"{expected}\"")),
        "missing static variant: {output}"
    );
    for forbidden in [
        "acct_private_resolver_canary",
        "private_session_canary",
        "current-token",
        "AccountUnavailable",
        "CredentialStoreUnavailable",
        "account.hash",
        "access_token",
    ] {
        assert!(
            !output.contains(forbidden),
            "private field escaped: {forbidden}"
        );
    }
}
#[test]
fn sync_terminal_resolver_failure_emits_static_variant_without_private_context() {
    for (reason, expected) in resolver_failure_cases() {
        // Arrange: dependencies return one typed failure; real routing owns observation.
        let resolver = TypedRejectingResolver {
            reason,
            calls: AtomicUsize::new(0),
        };
        let selector = TerminalAffinitySelector;
        let auth = local_auth_gate();
        let protocol = WebSocketProtocolRouter::new();
        let router = AuthenticatedWebSocketRouter::new(&auth, &selector, &resolver, &protocol)
            .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);
        // Act: the terminal branch must log before its unchanged close return.
        let mut result = None;
        let output = capture_log_output(|| {
            result = Some(router.route_first_frame(
                proof_handshake(),
                WebSocketFrame::Text(br#"{"type":"response.create"}"#.to_vec()),
            ))
        });
        // Assert: actual event, fixed label, same one resolver call and close semantics.
        assert!(matches!(
            result.unwrap(),
            Err(WebSocketCloseReason::ProviderCredential)
        ));
        assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
        assert_safe_variant_log(&output, expected);
    }
}
#[tokio::test(flavor = "current_thread")]
async fn async_terminal_resolver_failure_emits_static_variant_without_private_context() {
    for (reason, expected) in resolver_failure_cases() {
        let resolver = TypedRejectingResolver {
            reason,
            calls: AtomicUsize::new(0),
        };
        let selector = TerminalAffinitySelector;
        let auth = local_auth_gate();
        let protocol = WebSocketProtocolRouter::new();
        let router = AsyncAuthenticatedWebSocketRouter::new(&auth, &selector, &resolver, &protocol)
            .with_affinity_secret_provider(&TEST_AFFINITY_SECRET_PROVIDER);
        let (output, result) = capture_log_output_async(router.route_first_frame(
            proof_handshake(),
            WebSocketFrame::Text(br#"{"type":"response.create"}"#.to_vec()),
        ))
        .await;
        assert!(matches!(
            result,
            Err(WebSocketCloseReason::ProviderCredential)
        ));
        assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
        assert_safe_variant_log(&output, expected);
    }
}
