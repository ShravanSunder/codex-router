use super::*;

use codex_router_core::ids::AccountId;
use std::io;
use std::sync::Arc;
use std::sync::Mutex;

#[derive(Clone)]
struct FixedRefreshClient {
    should_fail: bool,
}

impl CredentialRefreshClient for FixedRefreshClient {
    fn refresh_credentials(
        &self,
        _account_id: &AccountId,
        _refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, CredentialRefreshFailure> {
        Err(CredentialRefreshFailure::confirmed_unspent(
            CredentialFailureClass::LocalPersistence,
            None,
        ))
    }

    fn refresh_provider_credentials(
        &self,
        _provider: Provider,
        _account_id: &AccountId,
        _refresh_token: &SecretString,
    ) -> Result<CredentialBundle, CredentialRefreshFailure> {
        if self.should_fail {
            return Err(CredentialRefreshFailure::ambiguous(
                CredentialFailureClass::ProviderRejected,
            ));
        }
        CredentialBundle::new_claude(
            SecretString::new("refreshed-access-canary"),
            SecretString::new("refreshed-refresh-canary"),
            10_000,
        )
        .map_err(|_| CredentialRefreshFailure::ambiguous(CredentialFailureClass::MalformedResponse))
    }
}

#[derive(Clone)]
struct SharedLogBuffer(Arc<Mutex<Vec<u8>>>);

impl io::Write for SharedLogBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("log buffer lock poisoned"))?
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for SharedLogBuffer {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}

#[test]
fn upkeep_refresh_outcomes_emit_structured_secret_free_events() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_writer(SharedLogBuffer(Arc::clone(&bytes)))
        .finish();
    let account_id = AccountId::new("acct_private_to_refresh").expect("account id");
    let refresh_token = SecretString::new("upkeep-refresh-secret-canary");

    tracing::subscriber::with_default(subscriber, || {
        let success_client =
            TelemetryCredentialUpkeepRefreshClient::new(FixedRefreshClient { should_fail: false });
        assert!(
            success_client
                .refresh_provider_credentials(Provider::Claude, &account_id, &refresh_token)
                .is_ok()
        );

        let failure_client =
            TelemetryCredentialUpkeepRefreshClient::new(FixedRefreshClient { should_fail: true });
        assert!(
            failure_client
                .refresh_provider_credentials(Provider::Openai, &account_id, &refresh_token)
                .is_err()
        );
    });

    let captured_events = String::from_utf8(
        bytes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone(),
    )
    .expect("captured logs should be UTF-8");
    assert!(captured_events.contains("codex_router_credential_upkeep_refresh"));
    assert!(captured_events.contains("provider=\"claude\""));
    assert!(captured_events.contains("refresh.outcome=\"success\""));
    assert!(captured_events.contains("refresh.failure_class=\"none\""));
    assert!(captured_events.contains("provider=\"openai\""));
    assert!(captured_events.contains("refresh.outcome=\"failure\""));
    assert!(captured_events.contains("refresh.failure_class=\"provider_rejected\""));
    assert!(!captured_events.contains("acct_private_to_refresh"));
    assert!(!captured_events.contains("upkeep-refresh-secret-canary"));
}
