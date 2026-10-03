use super::*;

#[derive(Clone, Debug)]
pub(super) struct PendingAsyncSelector {
    pub(super) started: Arc<Notify>,
}

impl AsyncAccountDecisionSelector for PendingAsyncSelector {
    fn select_upstream_account<'a>(
        &'a self,
        _request: &'a HttpProxyRequest,
        _token_generation: LocalTokenGeneration,
        _affinity_secret: Option<&'a RouterAffinityHashSecret>,
    ) -> BoxFuture<'a, Result<SelectedAccountDecision, HttpProxyError>> {
        Box::pin(async move {
            self.started.notify_one();
            std::future::pending().await
        })
    }
}

#[derive(Clone, Debug)]
pub(super) struct FixedAsyncSelector {
    pub(super) account_id: AccountId,
    pub(super) credit_backed_at_selection: bool,
}

impl AsyncAccountDecisionSelector for FixedAsyncSelector {
    fn select_upstream_account<'a>(
        &'a self,
        _request: &'a HttpProxyRequest,
        _token_generation: LocalTokenGeneration,
        _affinity_secret: Option<&'a RouterAffinityHashSecret>,
    ) -> BoxFuture<'a, Result<SelectedAccountDecision, HttpProxyError>> {
        Box::pin(async move {
            Ok(
                SelectedAccountDecision::new(self.account_id.clone(), "test-fixed")
                    .with_credit_backed_at_selection(self.credit_backed_at_selection),
            )
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct RejectingAsyncSelector {
    pub(super) reason: QuotaAwareAccountSelectorError,
}

impl AsyncAccountDecisionSelector for RejectingAsyncSelector {
    fn select_upstream_account<'a>(
        &'a self,
        _request: &'a HttpProxyRequest,
        _token_generation: LocalTokenGeneration,
        _affinity_secret: Option<&'a RouterAffinityHashSecret>,
    ) -> BoxFuture<'a, Result<SelectedAccountDecision, HttpProxyError>> {
        Box::pin(async move {
            Err(HttpProxyError::Selection {
                reason: self.reason,
            })
        })
    }
}

#[derive(Clone, Debug)]
pub(super) struct FixedAsyncCredentialResolver {
    pub(super) account_id: AccountId,
}

impl AsyncProviderCredentialResolver for FixedAsyncCredentialResolver {
    fn resolve_provider_credentials<'a>(
        &'a self,
        _account_id: &'a AccountId,
        _expected_provider: Provider,
    ) -> BoxFuture<'a, Result<ResolvedProviderCredential, CredentialResolverError>> {
        Box::pin(async move {
            Ok(ResolvedProviderCredential::new(
                self.account_id.clone(),
                SecretString::new("test-access-token"),
                1,
            ))
        })
    }
}

#[derive(Clone, Debug)]
pub(super) struct RecordingAsyncCredentialResolver {
    pub(super) account_id: AccountId,
    pub(super) requested_providers: Arc<Mutex<Vec<Provider>>>,
}

impl AsyncProviderCredentialResolver for RecordingAsyncCredentialResolver {
    fn resolve_provider_credentials<'a>(
        &'a self,
        _account_id: &'a AccountId,
        expected_provider: Provider,
    ) -> BoxFuture<'a, Result<ResolvedProviderCredential, CredentialResolverError>> {
        let account_id = self.account_id.clone();
        let requested_providers = Arc::clone(&self.requested_providers);
        Box::pin(async move {
            requested_providers
                .lock()
                .unwrap_or_else(|_error| panic!("provider request lock should be available"))
                .push(expected_provider);
            Ok(ResolvedProviderCredential::new(
                account_id,
                SecretString::new("test-access-token"),
                1,
            ))
        })
    }
}

#[derive(Clone, Debug)]
pub(super) struct FixedAffinitySecretProvider {
    pub(super) secret: RouterAffinityHashSecret,
}

impl FixedAffinitySecretProvider {
    pub(super) fn new() -> Self {
        Self {
            secret: RouterAffinityHashSecret::new(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            )
            .unwrap_or_else(|error| panic!("test affinity secret should parse: {error}")),
        }
    }
}

impl HttpAffinitySecretProvider for FixedAffinitySecretProvider {
    fn load_or_create_affinity_secret(&self) -> Result<RouterAffinityHashSecret, HttpProxyError> {
        Ok(self.secret.clone())
    }
}
