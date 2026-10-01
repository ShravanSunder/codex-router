use codex_router_auth::resolver::CredentialRefreshClient;
use codex_router_auth::resolver::CredentialRefreshFailure;
use codex_router_core::provider::Provider;
use codex_router_core::redaction::SecretString;
use codex_router_secret_store::account_tokens::AccountCredentialBundle;
use codex_router_secret_store::credential_bundle::CredentialBundle;
use codex_router_state::credential_maintenance::CredentialFailureClass;
use opentelemetry::KeyValue;
use opentelemetry::global;

#[derive(Clone)]
pub(super) struct TelemetryCredentialUpkeepRefreshClient<C> {
    inner: C,
}

impl<C> TelemetryCredentialUpkeepRefreshClient<C> {
    pub(super) const fn new(inner: C) -> Self {
        Self { inner }
    }
}

impl<C> CredentialRefreshClient for TelemetryCredentialUpkeepRefreshClient<C>
where
    C: CredentialRefreshClient,
{
    fn refresh_credentials(
        &self,
        account_id: &codex_router_core::ids::AccountId,
        refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, CredentialRefreshFailure> {
        let result = self.inner.refresh_credentials(account_id, refresh_token);
        record_credential_upkeep_refresh_outcome(
            Provider::Openai,
            result
                .as_ref()
                .map(|_| ())
                .map_err(|failure| failure.failure_class),
        );
        result
    }

    fn refresh_provider_credentials(
        &self,
        provider: Provider,
        account_id: &codex_router_core::ids::AccountId,
        refresh_token: &SecretString,
    ) -> Result<CredentialBundle, CredentialRefreshFailure> {
        let result = self
            .inner
            .refresh_provider_credentials(provider, account_id, refresh_token);
        record_credential_upkeep_refresh_outcome(
            provider,
            result
                .as_ref()
                .map(|_| ())
                .map_err(|failure| failure.failure_class),
        );
        result
    }
}

fn record_credential_upkeep_refresh_outcome(
    provider: Provider,
    result: Result<(), CredentialFailureClass>,
) {
    let (outcome, failure_class) = match result {
        Ok(()) => ("success", "none"),
        Err(failure_class) => ("failure", failure_class.as_str()),
    };
    if outcome == "success" {
        tracing::info!(
            provider = provider.as_str(),
            refresh.outcome = outcome,
            refresh.failure_class = failure_class,
            "codex_router_credential_upkeep_refresh"
        );
    } else {
        tracing::warn!(
            provider = provider.as_str(),
            refresh.outcome = outcome,
            refresh.failure_class = failure_class,
            "codex_router_credential_upkeep_refresh"
        );
    }
    global::meter("codex-router")
        .u64_counter("codex_router_credential_upkeep_refresh_total")
        .build()
        .add(
            1,
            &[
                KeyValue::new("provider", provider.as_str()),
                KeyValue::new("refresh.outcome", outcome),
                KeyValue::new("refresh.failure_class", failure_class),
            ],
        );
}

#[cfg(test)]
#[path = "telemetry_tests.rs"]
mod tests;
