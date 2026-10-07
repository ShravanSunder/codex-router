//! Static close diagnostics preserve the typed boundary without private payloads.
use super::*;
use crate::account_selection::QuotaAwareAccountSelectorError;
use crate::websocket::{WebSocketCloseReason, WebSocketTunnelError};
use codex_router_core::local_auth::LocalAuthError;

struct ObservedDiagnosticReporter {
    sender: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<String>>>,
}

impl LoopbackConnectionErrorReporter for ObservedDiagnosticReporter {
    fn report_connection_error(&self, diagnostic: &str) {
        if let Some(sender) = self.sender.lock().unwrap().take() {
            let _ = sender.send(diagnostic.to_owned());
        }
    }
}

fn close_diagnostic_cases() -> Vec<(WebSocketCloseReason, &'static str)> {
    let mut cases = Vec::new();
    for reason in [
        LocalAuthError::Missing,
        LocalAuthError::Empty,
        LocalAuthError::Old,
        LocalAuthError::Wrong,
    ] {
        cases.push((
            WebSocketCloseReason::LocalAuth { reason },
            "websocket_local_auth_rejected",
        ));
    }
    for reason in [
        QuotaAwareAccountSelectorError::NoEligibleAccounts,
        QuotaAwareAccountSelectorError::ShortQuotaExhausted {
            retry_after_seconds: 987654321,
        },
        QuotaAwareAccountSelectorError::SelectorStateUnavailable,
        QuotaAwareAccountSelectorError::StateUnavailable,
        QuotaAwareAccountSelectorError::SecretUnavailable,
        QuotaAwareAccountSelectorError::MalformedAffinityKey,
        QuotaAwareAccountSelectorError::AffinityOwnerMissing,
        QuotaAwareAccountSelectorError::AffinityOwnerUnavailable,
    ] {
        cases.push((
            WebSocketCloseReason::Selection { reason },
            "websocket_selection_rejected",
        ));
    }
    cases.push((
        WebSocketCloseReason::ProviderCredential,
        "websocket_provider_credential_rejected",
    ));
    cases.push((
        WebSocketCloseReason::UnexpectedFirstFrame,
        "websocket_first_frame_rejected",
    ));
    cases
}

#[test]
fn pre_upstream_diagnostic_reports_only_static_typed_subtype() {
    // Arrange: every typed close reason, including private detail-bearing variants.
    for (reason, expected_label) in close_diagnostic_cases() {
        let error =
            LoopbackRouterRuntimeError::WebSocket(WebSocketTunnelError::CloseReason(reason));
        // Act: the real shared classifier and sanitized renderer used by proxy logs.
        let label = websocket_runtime_error_kind(&error);
        let safe_error = sanitize_error_for_log(&error);
        let diagnostic = loopback_connection_diagnostic(&error);
        // Assert: exact independent labels; no nested reason, delay or Debug payload.
        assert_eq!(label, expected_label);
        assert_eq!(safe_error, expected_label);
        assert_eq!(diagnostic.class(), "upstream_tunnel_failure");
        assert_eq!(diagnostic.safe_reason(), expected_label);
        assert_eq!(diagnostic.severity(), "error");
        assert_eq!(
            diagnostic.render(),
            format!(
                "codex-router loopback connection failed: severity=error class=upstream_tunnel_failure reason={expected_label}"
            )
        );
        assert!(!diagnostic.render().contains("987654321"));
        assert!(!diagnostic.render().contains("ShortQuotaExhausted"));
    }
}

#[tokio::test]
async fn detached_pre_upstream_failure_reports_static_subtype_without_payload() {
    // Arrange: a real failed task with a typed private selector payload.
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let reporter = Arc::new(ObservedDiagnosticReporter {
        sender: std::sync::Mutex::new(Some(sender)),
    });
    let detached = tokio::spawn(async {
        Err(LoopbackRouterRuntimeError::WebSocket(
            WebSocketTunnelError::CloseReason(WebSocketCloseReason::Selection {
                reason: QuotaAwareAccountSelectorError::ShortQuotaExhausted {
                    retry_after_seconds: 987654321,
                },
            }),
        ))
    });
    // Act: supervise through the same reporter boundary as runtime failures.
    supervise_detached_connection_handler(detached, reporter.clone());
    let diagnostic = tokio::time::timeout(std::time::Duration::from_secs(1), receiver)
        .await
        .unwrap()
        .unwrap();
    // Assert: emitted diagnostic is fixed, not enum Debug or selector metadata.
    assert_eq!(
        diagnostic,
        "codex-router loopback connection failed: severity=error class=upstream_tunnel_failure reason=websocket_selection_rejected"
    );
    assert!(!diagnostic.contains("987654321"));
    assert!(!diagnostic.contains("ShortQuotaExhausted"));
}
