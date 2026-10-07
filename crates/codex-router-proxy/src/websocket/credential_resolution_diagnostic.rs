//! Fixed resolver categories at the existing failure boundary; no private context.
use codex_router_auth::resolver::CredentialResolverError;

pub(super) fn observe_credential_resolution_failure(reason: &CredentialResolverError) {
    let error_kind = match reason {
        CredentialResolverError::AccountUnavailable => "account_unavailable",
        CredentialResolverError::AccountIneligible => "account_ineligible",
        CredentialResolverError::AccountProviderMismatch => "account_provider_mismatch",
        CredentialResolverError::SecretUnavailable => "secret_unavailable",
        CredentialResolverError::CredentialStoreUnavailable => "credential_store_unavailable",
        CredentialResolverError::RefreshUnavailable => "refresh_unavailable",
    };
    tracing::warn!(target: "codex_router_proxy::websocket",
        {
        event.name = "codex_router.proxy.credential_resolution_failed",
        error.kind = error_kind,
        },
        "provider credential resolution failed"
    );
}
