use super::*;

pub(super) struct RecordingProviderCredentialResolver {
    access_token: SecretString,
    recorded: RefCell<Vec<String>>,
}

impl RecordingProviderCredentialResolver {
    pub(super) fn new(access_token: &str) -> Self {
        Self {
            access_token: SecretString::new(access_token),
            recorded: RefCell::new(Vec::new()),
        }
    }

    pub(super) fn take_recorded(&self) -> Vec<String> {
        self.recorded.take()
    }
}

impl ProviderCredentialResolver for RecordingProviderCredentialResolver {
    fn resolve_provider_credentials(
        &self,
        account_id: &codex_router_core::ids::AccountId,
        _expected_provider: codex_router_core::provider::Provider,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        self.recorded
            .borrow_mut()
            .push(account_id.as_str().to_owned());
        Ok(ResolvedProviderCredential::new(
            account_id.clone(),
            self.access_token.clone(),
            1,
        ))
    }
}

#[derive(Clone)]
pub(super) struct RecordingAsyncProviderCredentialResolver {
    access_token: SecretString,
    recorded: Arc<Mutex<Vec<String>>>,
}

impl RecordingAsyncProviderCredentialResolver {
    pub(super) fn new(access_token: &str) -> Self {
        Self {
            access_token: SecretString::new(access_token),
            recorded: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(super) fn take_recorded(&self) -> Vec<String> {
        lock_test_mutex(&self.recorded, "async credential records")
            .drain(..)
            .collect()
    }
}

impl AsyncProviderCredentialResolver for RecordingAsyncProviderCredentialResolver {
    fn resolve_provider_credentials<'a>(
        &'a self,
        account_id: &'a codex_router_core::ids::AccountId,
        _expected_provider: codex_router_core::provider::Provider,
    ) -> BoxFuture<'a, Result<ResolvedProviderCredential, CredentialResolverError>> {
        Box::pin(async move {
            lock_test_mutex(&self.recorded, "async credential records")
                .push(account_id.as_str().to_owned());
            Ok(ResolvedProviderCredential::new(
                account_id.clone(),
                self.access_token.clone(),
                1,
            ))
        })
    }
}

pub(super) struct RejectingProviderCredentialResolver {
    reason: CredentialResolverError,
    recorded: RefCell<Vec<String>>,
}

impl RejectingProviderCredentialResolver {
    pub(super) fn new(reason: CredentialResolverError) -> Self {
        Self {
            reason,
            recorded: RefCell::new(Vec::new()),
        }
    }

    pub(super) fn take_recorded(&self) -> Vec<String> {
        self.recorded.take()
    }
}

impl ProviderCredentialResolver for RejectingProviderCredentialResolver {
    fn resolve_provider_credentials(
        &self,
        account_id: &codex_router_core::ids::AccountId,
        _expected_provider: codex_router_core::provider::Provider,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        self.recorded
            .borrow_mut()
            .push(account_id.as_str().to_owned());
        Err(self.reason.clone())
    }
}

pub(super) struct FailFirstProviderCredentialResolver {
    failing_account_id: AccountId,
    access_token: SecretString,
    recorded: RefCell<Vec<String>>,
}

impl FailFirstProviderCredentialResolver {
    pub(super) fn new(failing_account_id: AccountId, access_token: &str) -> Self {
        Self {
            failing_account_id,
            access_token: SecretString::new(access_token),
            recorded: RefCell::new(Vec::new()),
        }
    }

    pub(super) fn take_recorded(&self) -> Vec<String> {
        self.recorded.take()
    }
}

impl ProviderCredentialResolver for FailFirstProviderCredentialResolver {
    fn resolve_provider_credentials(
        &self,
        account_id: &codex_router_core::ids::AccountId,
        _expected_provider: codex_router_core::provider::Provider,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        self.recorded
            .borrow_mut()
            .push(account_id.as_str().to_owned());
        if account_id == &self.failing_account_id {
            return Err(CredentialResolverError::RefreshUnavailable);
        }

        Ok(ResolvedProviderCredential::new(
            account_id.clone(),
            self.access_token.clone(),
            1,
        ))
    }
}

#[derive(Clone)]
pub(super) struct FailFirstAsyncProviderCredentialResolver {
    failing_account_id: AccountId,
    access_token: SecretString,
    recorded: Arc<Mutex<Vec<String>>>,
}

impl FailFirstAsyncProviderCredentialResolver {
    pub(super) fn new(failing_account_id: AccountId, access_token: &str) -> Self {
        Self {
            failing_account_id,
            access_token: SecretString::new(access_token),
            recorded: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(super) fn take_recorded(&self) -> Vec<String> {
        lock_test_mutex(&self.recorded, "async fail-first credential records")
            .drain(..)
            .collect()
    }
}

impl AsyncProviderCredentialResolver for FailFirstAsyncProviderCredentialResolver {
    fn resolve_provider_credentials<'a>(
        &'a self,
        account_id: &'a codex_router_core::ids::AccountId,
        _expected_provider: codex_router_core::provider::Provider,
    ) -> BoxFuture<'a, Result<ResolvedProviderCredential, CredentialResolverError>> {
        Box::pin(async move {
            lock_test_mutex(&self.recorded, "async fail-first credential records")
                .push(account_id.as_str().to_owned());
            if account_id == &self.failing_account_id {
                return Err(CredentialResolverError::RefreshUnavailable);
            }

            Ok(ResolvedProviderCredential::new(
                account_id.clone(),
                self.access_token.clone(),
                1,
            ))
        })
    }
}

#[derive(Clone)]
pub(super) struct RecordingRefreshClient {
    expected_account_id: String,
    expected_refresh_token: String,
    response: AccountCredentialBundle,
    calls: std::sync::Arc<AtomicUsize>,
}

impl RecordingRefreshClient {
    pub(super) fn new(
        expected_account_id: &str,
        expected_refresh_token: &str,
        response: AccountCredentialBundle,
    ) -> Self {
        Self {
            expected_account_id: expected_account_id.to_owned(),
            expected_refresh_token: expected_refresh_token.to_owned(),
            response,
            calls: std::sync::Arc::new(AtomicUsize::new(0)),
        }
    }

    pub(super) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl CredentialRefreshClient for RecordingRefreshClient {
    fn refresh_credentials(
        &self,
        account_id: &codex_router_core::ids::AccountId,
        refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, codex_router_auth::resolver::CredentialRefreshFailure>
    {
        assert_eq!(account_id.as_str(), self.expected_account_id);
        assert_eq!(refresh_token.expose_secret(), self.expected_refresh_token);
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.response.clone())
    }
}
