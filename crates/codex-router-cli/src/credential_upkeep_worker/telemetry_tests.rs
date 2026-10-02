use super::*;

use codex_router_core::ids::AccountId;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;
use tracing::Event;
use tracing::Subscriber;
use tracing::field::Field;
use tracing::field::Visit;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::prelude::*;

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

#[derive(Clone, Default)]
struct StructuredEventCapture(Arc<Mutex<Vec<BTreeMap<String, String>>>>);

impl StructuredEventCapture {
    fn snapshot(&self) -> Vec<BTreeMap<String, String>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl<S> Layer<S> for StructuredEventCapture
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let mut visitor = StructuredFieldVisitor::default();
        event.record(&mut visitor);
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(visitor.0);
    }
}

#[derive(Default)]
struct StructuredFieldVisitor(BTreeMap<String, String>);

impl Visit for StructuredFieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0.insert(field.name().to_owned(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_owned(), value.to_owned());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().to_owned(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().to_owned(), value.to_string());
    }
}

#[test]
fn upkeep_refresh_outcomes_emit_structured_secret_free_events() {
    let captured = StructuredEventCapture::default();
    let subscriber = tracing_subscriber::registry().with(captured.clone());
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

    let captured_events = captured.snapshot();
    let upkeep_events = captured_events
        .iter()
        .filter(|event| {
            event
                .get("message")
                .is_some_and(|message| message.contains("codex_router_credential_upkeep_refresh"))
        })
        .collect::<Vec<_>>();
    assert_eq!(upkeep_events.len(), 2);
    assert!(upkeep_events.iter().any(|event| {
        event.get("provider").map(String::as_str) == Some("claude")
            && event.get("refresh.outcome").map(String::as_str) == Some("success")
            && event.get("refresh.failure_class").map(String::as_str) == Some("none")
    }));
    assert!(upkeep_events.iter().any(|event| {
        event.get("provider").map(String::as_str) == Some("openai")
            && event.get("refresh.outcome").map(String::as_str) == Some("failure")
            && event.get("refresh.failure_class").map(String::as_str) == Some("provider_rejected")
    }));
    let captured_fields = format!("{captured_events:?}");
    assert!(!captured_fields.contains("acct_private_to_refresh"));
    assert!(!captured_fields.contains("upkeep-refresh-secret-canary"));
}
