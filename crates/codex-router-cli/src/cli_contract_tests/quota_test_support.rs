use super::*;
use crate::quota::QuotaWindowHeadroom;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

pub(super) struct JoinedFloorCrossingProvider {
    pub(super) floor_account_id: AccountId,
}

impl QuotaRefreshProvider for JoinedFloorCrossingProvider {
    async fn fetch_quota(
        &self,
        request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaRefreshError> {
        let weekly_remaining = if request.account_id() == &self.floor_account_id {
            8
        } else {
            80
        };
        Ok(QuotaRefreshProviderResponse {
            windows: vec![
                QuotaRefreshProviderWindow {
                    limit_window_seconds: 18_000,
                    headroom: QuotaWindowHeadroom::Percent(100),
                    reset_unix_seconds: Some(18_000),
                    effective: true,
                },
                QuotaRefreshProviderWindow {
                    limit_window_seconds: 604_800,
                    headroom: QuotaWindowHeadroom::Percent(weekly_remaining),
                    reset_unix_seconds: Some(604_800),
                    effective: false,
                },
            ],
            reset_credits_available: None,
            ..Default::default()
        })
    }
}

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
    test_async_runtime()
        .block_on(refresh_quota_with_dependencies_async(
            stdout,
            router_root,
            base_url,
            credential_resolver,
            quota_provider,
            observed_unix_seconds,
        ))
        .map(|_report| ())
        .map_err(Into::into)
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
    test_async_runtime()
        .block_on(refresh_quota_store_paths_with_dependencies_async(
            stdout,
            state_db,
            secret_root,
            base_url,
            credential_resolver,
            quota_provider,
            observed_unix_seconds,
        ))
        .map(|_report| ())
        .map_err(Into::into)
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
    test_async_runtime()
        .block_on(
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
        .map(|_report| ())
        .map_err(Into::into)
}

#[derive(Default)]
pub(super) struct RecordingWeeklyFloorObserver {
    pub(super) account_ids: Mutex<Vec<AccountId>>,
    pub(super) intents: Mutex<Vec<WeeklyQuotaFloorIntent>>,
}

impl WeeklyQuotaFloorIntentObserver for RecordingWeeklyFloorObserver {
    fn weekly_quota_floor_intent(&self, account_id: &AccountId, intent: WeeklyQuotaFloorIntent) {
        lock_test_mutex(&self.account_ids, "weekly floor observer").push(account_id.clone());
        lock_test_mutex(&self.intents, "weekly floor intents").push(intent);
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
    ) -> Result<AccountCredentialBundle, codex_router_auth::resolver::CredentialRefreshFailure>
    {
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
    fn fetch_quota(
        &self,
        request: QuotaRefreshProviderRequest,
    ) -> impl std::future::Future<
        Output = Result<QuotaRefreshProviderResponse, crate::quota::QuotaRefreshError>,
    > + Send {
        self.recorded.borrow_mut().push((
            request.account_id().as_str().to_owned(),
            request.account_label().to_owned(),
            request.route_band().to_owned(),
            request.base_url().to_owned(),
            request.access_token().expose_secret().to_owned(),
        ));
        std::future::ready(Ok(QuotaRefreshProviderResponse {
            windows: verified_quota_windows(self.remaining_headroom),
            reset_credits_available: None,
            ..Default::default()
        }))
    }
}

pub(super) struct StaticQuotaRefreshProvider {
    pub(super) windows: Vec<QuotaRefreshProviderWindow>,
}

pub(super) struct FloorNotificationOrderingQuotaProvider {
    pub(super) floor_account_id: AccountId,
    pub(super) healthy_account_id: AccountId,
    pub(super) floor_observer: Arc<RecordingWeeklyFloorObserver>,
    pub(super) state_db_path: PathBuf,
}

impl FloorNotificationOrderingQuotaProvider {
    pub(super) fn new(
        floor_account_id: AccountId,
        healthy_account_id: AccountId,
        floor_observer: Arc<RecordingWeeklyFloorObserver>,
        state_db_path: PathBuf,
    ) -> Self {
        Self {
            floor_account_id,
            healthy_account_id,
            floor_observer,
            state_db_path,
        }
    }
}

impl QuotaRefreshProvider for FloorNotificationOrderingQuotaProvider {
    async fn fetch_quota(
        &self,
        request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaRefreshError> {
        if request.account_id() == &self.healthy_account_id && request.route_band() == "responses" {
            assert_eq!(
                *lock_test_mutex(&self.floor_observer.account_ids, "weekly floor observer"),
                vec![self.floor_account_id.clone()],
                "floor reconnect must follow its saved selector evidence and precede later accounts"
            );
            let state = AsyncSqliteStateStore::open_read_only(&self.state_db_path).await?;
            let windows = state
                .selector_inputs_for_route_band("responses", 1_100)
                .await?;
            let floor = windows
                .iter()
                .find(|window| window.account_id() == &self.floor_account_id)
                .expect("floor account selector window should be saved before signal");
            assert!(
                floor
                    .windows()
                    .iter()
                    .any(|window| window.limit_window_seconds() == 604_800
                        && window.remaining_headroom() == 0)
            );
            let history = state
                .quota_history_observations_for_window(
                    &self.floor_account_id,
                    "responses",
                    604_800,
                    0,
                    2_000,
                )
                .await?;
            assert!(
                !history.is_empty(),
                "burn history must be saved before reconnect"
            );
            state.close().await?;
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
                    headroom: QuotaWindowHeadroom::Percent(remaining_headroom),
                    reset_unix_seconds: Some(20_000),
                    effective: true,
                },
                QuotaRefreshProviderWindow {
                    limit_window_seconds: 604_800,
                    headroom: QuotaWindowHeadroom::Percent(remaining_headroom),
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
            ..Default::default()
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
    ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaRefreshError> {
        Ok(QuotaRefreshProviderResponse {
            windows: self.windows.clone(),
            reset_credits_available: None,
            ..Default::default()
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
    ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaRefreshError> {
        tokio::time::sleep(self.delay).await;
        Ok(QuotaRefreshProviderResponse {
            windows: verified_quota_windows(self.remaining_headroom),
            reset_credits_available: None,
            ..Default::default()
        })
    }
}

pub(super) struct BlockingQuotaRefreshProvider {
    pub(super) remaining_headroom: u32,
    pub(super) blocked_once: AtomicBool,
    pub(super) started_sender: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    pub(super) release_receiver: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

impl BlockingQuotaRefreshProvider {
    pub(super) fn new(
        remaining_headroom: u32,
        started_sender: tokio::sync::oneshot::Sender<()>,
        release_receiver: tokio::sync::oneshot::Receiver<()>,
    ) -> Self {
        Self {
            remaining_headroom,
            blocked_once: AtomicBool::new(false),
            started_sender: Mutex::new(Some(started_sender)),
            release_receiver: Mutex::new(Some(release_receiver)),
        }
    }
}

impl QuotaRefreshProvider for BlockingQuotaRefreshProvider {
    async fn fetch_quota(
        &self,
        _request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaRefreshError> {
        if !self.blocked_once.swap(true, Ordering::SeqCst) {
            let maybe_started_sender =
                lock_test_mutex(&self.started_sender, "started sender").take();
            if let Some(started_sender) = maybe_started_sender
                && let Err(error) = started_sender.send(())
            {
                panic!("background refresh started signal should send: {error:?}");
            }
            let release_receiver = lock_test_mutex(&self.release_receiver, "release receiver")
                .take()
                .expect("release receiver should remain available");
            release_receiver.await.unwrap_or_else(|error| {
                panic!("test should release blocked quota refresh: {error:?}")
            });
        }
        Ok(QuotaRefreshProviderResponse {
            windows: verified_quota_windows(self.remaining_headroom),
            reset_credits_available: None,
            ..Default::default()
        })
    }
}

pub(super) struct SignalingQuotaRefreshProvider {
    pub(super) remaining_headroom: u32,
    pub(super) sender: tokio::sync::mpsc::UnboundedSender<String>,
}

impl SignalingQuotaRefreshProvider {
    pub(super) fn new(
        remaining_headroom: u32,
        sender: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> Self {
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
    ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaRefreshError> {
        if let Err(error) = self.sender.send(request.route_band().to_owned()) {
            panic!("background refresh signal should send: {error}");
        }
        Ok(QuotaRefreshProviderResponse {
            windows: verified_quota_windows(self.remaining_headroom),
            reset_credits_available: None,
            ..Default::default()
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
    ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaRefreshError> {
        if request.account_label() == self.failing_account_label {
            return Err(crate::quota::QuotaRefreshError::ProviderStatus {
                status: self.status,
            });
        }

        Ok(QuotaRefreshProviderResponse {
            windows: verified_quota_windows(self.remaining_headroom),
            reset_credits_available: None,
            ..Default::default()
        })
    }
}

pub(super) fn verified_quota_windows(remaining_headroom: u32) -> Vec<QuotaRefreshProviderWindow> {
    vec![
        QuotaRefreshProviderWindow {
            limit_window_seconds: 18_000,
            headroom: QuotaWindowHeadroom::Percent(remaining_headroom),
            reset_unix_seconds: Some(20_000),
            effective: true,
        },
        QuotaRefreshProviderWindow {
            limit_window_seconds: 604_800,
            headroom: QuotaWindowHeadroom::Percent(remaining_headroom.max(50)),
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
