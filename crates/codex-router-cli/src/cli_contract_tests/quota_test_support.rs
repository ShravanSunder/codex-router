use super::*;

pub(super) fn refresh_quota_with_dependencies<R, P>(
    stdout: &mut impl Write,
    router_root: PathBuf,
    base_url: String,
    credential_resolver: &R,
    quota_provider: &P,
    observed_unix_seconds: u64,
) -> Result<(), crate::quota::QuotaCommandError>
where
    R: AsyncProviderCredentialResolver,
    P: QuotaRefreshProvider,
{
    test_async_runtime().block_on(refresh_quota_with_dependencies_async(
        stdout,
        router_root,
        base_url,
        credential_resolver,
        quota_provider,
        observed_unix_seconds,
    ))
}

pub(super) fn refresh_quota_store_paths_with_dependencies<R, P>(
    stdout: &mut impl Write,
    state_db: &Path,
    secret_root: &Path,
    base_url: String,
    credential_resolver: &R,
    quota_provider: &P,
    observed_unix_seconds: u64,
) -> Result<(), crate::quota::QuotaCommandError>
where
    R: AsyncProviderCredentialResolver,
    P: QuotaRefreshProvider,
{
    test_async_runtime().block_on(refresh_quota_store_paths_with_dependencies_async(
        stdout,
        state_db,
        secret_root,
        base_url,
        credential_resolver,
        quota_provider,
        observed_unix_seconds,
    ))
}

pub(super) fn refresh_quota_store_paths_with_floor_observer<R, P>(
    stdout: &mut impl Write,
    state_db: &Path,
    secret_root: &Path,
    base_url: String,
    credential_resolver: &R,
    quota_provider: &P,
    observation_context: QuotaRefreshObservationContext<'_>,
) -> Result<(), crate::quota::QuotaCommandError>
where
    R: AsyncProviderCredentialResolver,
    P: QuotaRefreshProvider,
{
    test_async_runtime().block_on(
        refresh_quota_store_paths_with_dependencies_and_floor_notifier_async(
            stdout,
            state_db,
            secret_root,
            base_url,
            credential_resolver,
            quota_provider,
            observation_context,
        ),
    )
}

#[derive(Default)]
pub(super) struct RecordingWeeklyFloorObserver {
    pub(super) account_ids: Mutex<Vec<AccountId>>,
}

impl WeeklyQuotaFloorReachedObserver for RecordingWeeklyFloorObserver {
    fn weekly_quota_floor_reached(&self, account_id: &AccountId) {
        lock_test_mutex(&self.account_ids, "weekly floor observer").push(account_id.clone());
    }
}

impl<S, C> AsyncProviderCredentialResolver for RouterCredentialResolver<'_, S, C>
where
    S: SecretStore,
    C: CredentialRefreshClient,
{
    async fn resolve_provider_credentials_async(
        &self,
        account_id: &AccountId,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        ProviderCredentialResolver::resolve_provider_credentials(self, account_id)
    }
}

#[derive(Clone)]
pub(super) struct RecordingRefreshClient {
    pub(super) expected_account_id: String,
    pub(super) expected_refresh_token: String,
    pub(super) response: AccountCredentialBundle,
    pub(super) calls: Arc<AtomicUsize>,
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
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub(super) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl CredentialRefreshClient for RecordingRefreshClient {
    fn refresh_credentials(
        &self,
        account_id: &AccountId,
        refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, CredentialResolverError> {
        assert_eq!(account_id.as_str(), self.expected_account_id);
        assert_eq!(refresh_token.expose_secret(), self.expected_refresh_token);
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.response.clone())
    }
}

pub(super) type RecordedQuotaRefresh = (String, String, String, String, String);

pub(super) struct RecordingQuotaRefreshProvider {
    pub(super) remaining_headroom: u32,
    pub(super) recorded: RefCell<Vec<RecordedQuotaRefresh>>,
}

impl RecordingQuotaRefreshProvider {
    pub(super) fn new(remaining_headroom: u32) -> Self {
        Self {
            remaining_headroom,
            recorded: RefCell::new(Vec::new()),
        }
    }

    pub(super) fn take_recorded(&self) -> Vec<RecordedQuotaRefresh> {
        self.recorded.take()
    }
}

impl QuotaRefreshProvider for RecordingQuotaRefreshProvider {
    async fn fetch_quota(
        &self,
        request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaCommandError> {
        self.recorded.borrow_mut().push((
            request.account_id().as_str().to_owned(),
            request.account_label().to_owned(),
            request.route_band().to_owned(),
            request.base_url().to_owned(),
            request.access_token().expose_secret().to_owned(),
        ));
        Ok(QuotaRefreshProviderResponse {
            windows: verified_quota_windows(self.remaining_headroom),
            reset_credits_available: None,
        })
    }
}

pub(super) struct StaticQuotaRefreshProvider {
    pub(super) windows: Vec<QuotaRefreshProviderWindow>,
}

pub(super) struct FloorNotificationOrderingQuotaProvider {
    pub(super) floor_account_id: AccountId,
    pub(super) healthy_account_id: AccountId,
    pub(super) floor_observer: Arc<RecordingWeeklyFloorObserver>,
}

impl FloorNotificationOrderingQuotaProvider {
    pub(super) fn new(
        floor_account_id: AccountId,
        healthy_account_id: AccountId,
        floor_observer: Arc<RecordingWeeklyFloorObserver>,
    ) -> Self {
        Self {
            floor_account_id,
            healthy_account_id,
            floor_observer,
        }
    }
}

impl QuotaRefreshProvider for FloorNotificationOrderingQuotaProvider {
    async fn fetch_quota(
        &self,
        request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaCommandError> {
        if request.account_id() == &self.healthy_account_id && request.route_band() == "responses" {
            assert!(
                lock_test_mutex(&self.floor_observer.account_ids, "weekly floor observer",)
                    .is_empty(),
                "floor reconnect notification fired before the healthy account was published"
            );
        }
        let remaining_headroom = if request.account_id() == &self.floor_account_id {
            0
        } else {
            80
        };
        let windows = if request.account_id() == &self.floor_account_id {
            vec![
                QuotaRefreshProviderWindow {
                    limit_window_seconds: 18_000,
                    remaining_headroom,
                    reset_unix_seconds: Some(20_000),
                    effective: true,
                },
                QuotaRefreshProviderWindow {
                    limit_window_seconds: 604_800,
                    remaining_headroom,
                    reset_unix_seconds: Some(614_800),
                    effective: false,
                },
            ]
        } else {
            verified_quota_windows(remaining_headroom)
        };
        Ok(QuotaRefreshProviderResponse {
            windows,
            reset_credits_available: None,
        })
    }
}

impl StaticQuotaRefreshProvider {
    pub(super) fn new(windows: Vec<QuotaRefreshProviderWindow>) -> Self {
        Self { windows }
    }
}

impl QuotaRefreshProvider for StaticQuotaRefreshProvider {
    async fn fetch_quota(
        &self,
        _request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaCommandError> {
        Ok(QuotaRefreshProviderResponse {
            windows: self.windows.clone(),
            reset_credits_available: None,
        })
    }
}

pub(super) struct SlowQuotaRefreshProvider {
    pub(super) delay: Duration,
    pub(super) remaining_headroom: u32,
}

impl SlowQuotaRefreshProvider {
    pub(super) fn new(delay: Duration, remaining_headroom: u32) -> Self {
        Self {
            delay,
            remaining_headroom,
        }
    }
}

impl QuotaRefreshProvider for SlowQuotaRefreshProvider {
    async fn fetch_quota(
        &self,
        _request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaCommandError> {
        thread::sleep(self.delay);
        Ok(QuotaRefreshProviderResponse {
            windows: verified_quota_windows(self.remaining_headroom),
            reset_credits_available: None,
        })
    }
}

pub(super) struct BlockingQuotaRefreshProvider {
    pub(super) remaining_headroom: u32,
    pub(super) blocked_once: AtomicBool,
    pub(super) started_sender: Mutex<Option<mpsc::Sender<()>>>,
    pub(super) release_receiver: Mutex<mpsc::Receiver<()>>,
}

impl BlockingQuotaRefreshProvider {
    pub(super) fn new(
        remaining_headroom: u32,
        started_sender: mpsc::Sender<()>,
        release_receiver: mpsc::Receiver<()>,
    ) -> Self {
        Self {
            remaining_headroom,
            blocked_once: AtomicBool::new(false),
            started_sender: Mutex::new(Some(started_sender)),
            release_receiver: Mutex::new(release_receiver),
        }
    }
}

impl QuotaRefreshProvider for BlockingQuotaRefreshProvider {
    async fn fetch_quota(
        &self,
        _request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaCommandError> {
        if !self.blocked_once.swap(true, Ordering::SeqCst) {
            let maybe_started_sender =
                lock_test_mutex(&self.started_sender, "started sender").take();
            if let Some(started_sender) = maybe_started_sender
                && let Err(error) = started_sender.send(())
            {
                panic!("background refresh started signal should send: {error}");
            }
            let receive_result = lock_test_mutex(&self.release_receiver, "release receiver").recv();
            if let Err(error) = receive_result {
                panic!("test should release blocked quota refresh: {error}");
            }
        }
        Ok(QuotaRefreshProviderResponse {
            windows: verified_quota_windows(self.remaining_headroom),
            reset_credits_available: None,
        })
    }
}

pub(super) struct SignalingQuotaRefreshProvider {
    pub(super) remaining_headroom: u32,
    pub(super) sender: mpsc::Sender<String>,
}

impl SignalingQuotaRefreshProvider {
    pub(super) fn new(remaining_headroom: u32, sender: mpsc::Sender<String>) -> Self {
        Self {
            remaining_headroom,
            sender,
        }
    }
}

impl QuotaRefreshProvider for SignalingQuotaRefreshProvider {
    async fn fetch_quota(
        &self,
        request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaCommandError> {
        if let Err(error) = self.sender.send(request.route_band().to_owned()) {
            panic!("background refresh signal should send: {error}");
        }
        Ok(QuotaRefreshProviderResponse {
            windows: verified_quota_windows(self.remaining_headroom),
            reset_credits_available: None,
        })
    }
}

pub(super) struct AccountFailingQuotaRefreshProvider {
    pub(super) failing_account_label: &'static str,
    pub(super) status: u16,
    pub(super) remaining_headroom: u32,
}

impl AccountFailingQuotaRefreshProvider {
    pub(super) fn new(
        failing_account_label: &'static str,
        status: u16,
        remaining_headroom: u32,
    ) -> Self {
        Self {
            failing_account_label,
            status,
            remaining_headroom,
        }
    }
}

impl QuotaRefreshProvider for AccountFailingQuotaRefreshProvider {
    async fn fetch_quota(
        &self,
        request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaCommandError> {
        if request.account_label() == self.failing_account_label {
            return Err(crate::quota::QuotaCommandError::ProviderStatus {
                status: self.status,
            });
        }

        Ok(QuotaRefreshProviderResponse {
            windows: verified_quota_windows(self.remaining_headroom),
            reset_credits_available: None,
        })
    }
}

pub(super) fn verified_quota_windows(remaining_headroom: u32) -> Vec<QuotaRefreshProviderWindow> {
    vec![
        QuotaRefreshProviderWindow {
            limit_window_seconds: 18_000,
            remaining_headroom,
            reset_unix_seconds: Some(20_000),
            effective: true,
        },
        QuotaRefreshProviderWindow {
            limit_window_seconds: 604_800,
            remaining_headroom: remaining_headroom.max(50),
            reset_unix_seconds: Some(614_800),
            effective: false,
        },
    ]
}

pub(super) fn run_static_quota_cli<const ARGUMENT_COUNT: usize>(
    args: [&str; ARGUMENT_COUNT],
) -> CliRunOutput {
    let command = match must_ok(CliCommand::parse(args.into_iter().map(Into::into))) {
        CliCommand::Quota(command) => command,
        _ => panic!("static quota helper requires a quota command"),
    };
    let mut stdout = Vec::new();
    must_ok(
        test_async_runtime().block_on(crate::quota::run_quota_command(
            &mut stdout,
            command,
            false,
            true,
            None,
        )),
    );
    CliRunOutput {
        stdout: must_ok(String::from_utf8(stdout)),
        stderr: String::new(),
    }
}

pub(super) fn persist_effective_selector_window(
    state: &SqliteStateStore,
    account_id: &AccountId,
    route_band: &str,
    remaining_headroom: u32,
) {
    let short_window = PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        route_band,
        18_000,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(remaining_headroom)
    .with_reset_unix_seconds(18_000)
    .with_effective(true)
    .with_observed_unix_seconds(1_000);
    must_ok(SelectorQuotaRepository::upsert_selector_window(
        state,
        &short_window,
    ));
    let weekly_window = PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        route_band,
        604_800,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(100)
    .with_reset_unix_seconds(604_800)
    .with_observed_unix_seconds(1_000);
    must_ok(SelectorQuotaRepository::upsert_selector_window(
        state,
        &weekly_window,
    ));
}
