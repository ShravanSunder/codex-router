use super::*;

pub(super) struct RecordingSelector {
    recorded: RefCell<Vec<(String, TokenGeneration)>>,
    recorded_session_ids: RefCell<Vec<Option<String>>>,
}

impl RecordingSelector {
    pub(super) fn new() -> Self {
        Self {
            recorded: RefCell::new(Vec::new()),
            recorded_session_ids: RefCell::new(Vec::new()),
        }
    }

    pub(super) fn take_recorded(&self) -> Vec<(String, TokenGeneration)> {
        self.recorded.take()
    }

    pub(super) fn take_recorded_session_ids(&self) -> Vec<Option<String>> {
        self.recorded_session_ids.take()
    }
}

impl AccountDecisionSelector for RecordingSelector {
    fn select_upstream_account(
        &self,
        request: &HttpProxyRequest,
        token_generation: TokenGeneration,
        _affinity_secret: Option<&RouterAffinityHashSecret>,
    ) -> Result<SelectedAccountDecision, HttpProxyError> {
        self.recorded
            .borrow_mut()
            .push((request.path().to_owned(), token_generation));
        self.recorded_session_ids
            .borrow_mut()
            .push(request.header_value("session-id").map(str::to_owned));
        let account_id = match codex_router_core::ids::AccountId::new("acct_selected") {
            Ok(account_id) => account_id,
            Err(error) => {
                return Err(HttpProxyError::Upstream {
                    message: format!("test account id failed: {error}"),
                });
            }
        };
        Ok(SelectedAccountDecision::new(account_id, "test_selection"))
    }
}

#[derive(Clone, Default)]
pub(super) struct RecordingAsyncSelector {
    recorded: Arc<Mutex<Vec<(String, TokenGeneration)>>>,
    recorded_session_ids: Arc<Mutex<Vec<Option<String>>>>,
}

impl RecordingAsyncSelector {
    pub(super) fn take_recorded(&self) -> Vec<(String, TokenGeneration)> {
        lock_test_mutex(&self.recorded, "async selector records")
            .drain(..)
            .collect()
    }

    pub(super) fn take_recorded_session_ids(&self) -> Vec<Option<String>> {
        lock_test_mutex(&self.recorded_session_ids, "async selector session ids")
            .drain(..)
            .collect()
    }
}

impl AsyncAccountDecisionSelector for RecordingAsyncSelector {
    fn select_upstream_account<'a>(
        &'a self,
        request: &'a HttpProxyRequest,
        token_generation: TokenGeneration,
        _affinity_secret: Option<&'a RouterAffinityHashSecret>,
    ) -> BoxFuture<'a, Result<SelectedAccountDecision, HttpProxyError>> {
        Box::pin(async move {
            lock_test_mutex(&self.recorded, "async selector records")
                .push((request.path().to_owned(), token_generation));
            lock_test_mutex(&self.recorded_session_ids, "async selector session ids")
                .push(request.header_value("session-id").map(str::to_owned));
            let account_id = match codex_router_core::ids::AccountId::new("acct_selected") {
                Ok(account_id) => account_id,
                Err(error) => {
                    return Err(HttpProxyError::Upstream {
                        message: format!("test account id failed: {error}"),
                    });
                }
            };
            Ok(SelectedAccountDecision::new(account_id, "test_selection"))
        })
    }
}

#[derive(Clone)]
pub(super) struct ExclusionAwareAsyncSelector {
    accounts: Arc<Vec<AccountId>>,
    recorded: Arc<Mutex<Vec<(String, TokenGeneration)>>>,
}

impl ExclusionAwareAsyncSelector {
    pub(super) fn new(accounts: Vec<AccountId>) -> Self {
        Self {
            accounts: Arc::new(accounts),
            recorded: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(super) fn take_recorded(&self) -> Vec<(String, TokenGeneration)> {
        lock_test_mutex(&self.recorded, "exclusion-aware async selector records")
            .drain(..)
            .collect()
    }
}

impl AsyncAccountDecisionSelector for ExclusionAwareAsyncSelector {
    fn select_upstream_account<'a>(
        &'a self,
        request: &'a HttpProxyRequest,
        token_generation: TokenGeneration,
        _affinity_secret: Option<&'a RouterAffinityHashSecret>,
    ) -> BoxFuture<'a, Result<SelectedAccountDecision, HttpProxyError>> {
        Box::pin(async move {
            let selected_account_id = self
                .accounts
                .iter()
                .find(|account_id| {
                    !request
                        .excluded_accounts()
                        .iter()
                        .any(|excluded_account_id| excluded_account_id == *account_id)
                })
                .cloned()
                .ok_or(HttpProxyError::Selection {
                    reason: QuotaAwareAccountSelectorError::NoEligibleAccounts,
                })?;
            lock_test_mutex(&self.recorded, "exclusion-aware async selector records")
                .push((selected_account_id.as_str().to_owned(), token_generation));
            Ok(SelectedAccountDecision::new(
                selected_account_id,
                "test_selection",
            ))
        })
    }
}

pub(super) struct RejectingSelector {
    reason: QuotaAwareAccountSelectorError,
    recorded: RefCell<Vec<(String, TokenGeneration)>>,
}

impl RejectingSelector {
    pub(super) fn new(reason: QuotaAwareAccountSelectorError) -> Self {
        Self {
            reason,
            recorded: RefCell::new(Vec::new()),
        }
    }
}

impl AccountDecisionSelector for RejectingSelector {
    fn select_upstream_account(
        &self,
        request: &HttpProxyRequest,
        token_generation: TokenGeneration,
        _affinity_secret: Option<&RouterAffinityHashSecret>,
    ) -> Result<SelectedAccountDecision, HttpProxyError> {
        self.recorded
            .borrow_mut()
            .push((request.path().to_owned(), token_generation));
        Err(HttpProxyError::Selection {
            reason: self.reason,
        })
    }
}
