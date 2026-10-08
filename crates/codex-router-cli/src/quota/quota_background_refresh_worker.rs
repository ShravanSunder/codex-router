use super::*;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Stoppable background quota refresh worker.
pub(crate) struct BackgroundQuotaRefreshWorker {
    stop_requested: CancellationToken,
    task: Option<JoinHandle<()>>,
}

pub(crate) struct BackgroundQuotaRefreshRuntime<C, D> {
    observed_clock: C,
    diagnostic_reporter: D,
    interval: Duration,
    quota_floor_notifier: Option<Arc<dyn WeeklyQuotaFloorIntentObserver>>,
}

impl<C, D> BackgroundQuotaRefreshRuntime<C, D> {
    pub(crate) const fn new(observed_clock: C, diagnostic_reporter: D, interval: Duration) -> Self {
        Self {
            observed_clock,
            diagnostic_reporter,
            interval,
            quota_floor_notifier: None,
        }
    }

    pub(crate) fn with_quota_floor_notifier(
        mut self,
        quota_floor_notifier: WebSocketQuotaFloorNotifier,
    ) -> Self {
        self.quota_floor_notifier = Some(Arc::new(quota_floor_notifier));
        self
    }
}

impl BackgroundQuotaRefreshWorker {
    fn request_stop(&self) {
        self.stop_requested.cancel();
    }

    pub(crate) async fn shutdown(&mut self) {
        self.request_stop();
        if let Some(task) = self.task.as_mut() {
            let _result = task.await;
            self.task = None;
        }
    }
}

impl Drop for BackgroundQuotaRefreshWorker {
    fn drop(&mut self) {
        self.request_stop();
    }
}

#[cfg(test)]
pub(crate) async fn start_background_quota_refresh_worker_with_dependencies<R, P>(
    state_db: PathBuf,
    secret_root: PathBuf,
    base_url: String,
    credential_resolver: R,
    quota_provider: P,
    interval: Duration,
) -> BackgroundQuotaRefreshWorker
where
    R: AsyncProviderCredentialResolver + Send + Sync + 'static,
    P: QuotaRefreshProvider + Send + Sync + 'static,
{
    start_background_quota_refresh_worker_with_clock(
        state_db,
        secret_root,
        base_url,
        credential_resolver,
        quota_provider,
        current_unix_seconds,
        interval,
    )
    .await
}

#[cfg(test)]
pub(crate) async fn start_background_quota_refresh_worker_with_clock<R, P, C>(
    state_db: PathBuf,
    secret_root: PathBuf,
    base_url: String,
    credential_resolver: R,
    quota_provider: P,
    observed_clock: C,
    interval: Duration,
) -> BackgroundQuotaRefreshWorker
where
    R: AsyncProviderCredentialResolver + Send + Sync + 'static,
    P: QuotaRefreshProvider + Send + Sync + 'static,
    C: FnMut() -> u64 + Send + 'static,
{
    start_background_quota_refresh_worker_with_reporter(
        state_db,
        secret_root,
        base_url,
        credential_resolver,
        quota_provider,
        BackgroundQuotaRefreshRuntime::new(observed_clock, |_diagnostic| {}, interval),
    )
    .await
}

pub(crate) async fn start_background_quota_refresh_worker_with_reporter<R, P, C, D>(
    state_db: PathBuf,
    secret_root: PathBuf,
    base_url: String,
    credential_resolver: R,
    quota_provider: P,
    runtime: BackgroundQuotaRefreshRuntime<C, D>,
) -> BackgroundQuotaRefreshWorker
where
    R: AsyncProviderCredentialResolver + Send + Sync + 'static,
    P: QuotaRefreshProvider + Send + Sync + 'static,
    C: FnMut() -> u64 + Send + 'static,
    D: FnMut(String) + Send + 'static,
{
    let BackgroundQuotaRefreshRuntime {
        mut observed_clock,
        mut diagnostic_reporter,
        interval,
        quota_floor_notifier,
    } = runtime;
    let stop_requested = CancellationToken::new();
    let worker_stop = stop_requested.clone();
    let task = tokio::spawn(async move {
        loop {
            let cycle_started_at = Instant::now();
            let mut sink = Vec::new();
            let observed_unix_seconds = observed_clock();
            let result = refresh_quota_store_paths_with_dependencies_and_floor_notifier(
                &mut sink,
                &state_db,
                &secret_root,
                base_url.clone(),
                &credential_resolver,
                &quota_provider,
                QuotaRefreshObservationContext {
                    observed_unix_seconds,
                    schedule: QuotaRefreshSchedule::Background {
                        interval_seconds: interval.as_secs(),
                    },
                    weekly_floor_observer: quota_floor_notifier.as_deref(),
                },
            )
            .await;
            let diagnostic_output = String::from_utf8_lossy(&sink).into_owned();
            if diagnostic_output
                .lines()
                .any(|line| line.starts_with("refresh failed:") || line.starts_with("failed:"))
            {
                diagnostic_reporter(diagnostic_output.trim_end().to_owned());
            }
            if let Err(error) = result {
                diagnostic_reporter(format!("background quota refresh failed: {error}"));
            }
            if interval.is_zero() {
                break;
            }
            let delay = refresh_cycle_delay(interval, cycle_started_at.elapsed());
            tokio::select! {
                biased;
                _ = worker_stop.cancelled() => break,
                _ = tokio::time::sleep(delay) => {}
            }
        }
    });

    BackgroundQuotaRefreshWorker {
        stop_requested,
        task: Some(task),
    }
}

pub(crate) async fn start_background_quota_refresh_worker(
    state_db: PathBuf,
    secret_root: PathBuf,
    credential_store: crate::secret_store_factory::CliRuntimeSecretStore,
    base_url: String,
    interval: Duration,
    quota_floor_notifier: WebSocketQuotaFloorNotifier,
) -> Result<BackgroundQuotaRefreshWorker, QuotaCommandError> {
    let resolver =
        AsyncCliCredentialResolver::open_with_secret_store(&state_db, credential_store).await?;
    let provider = HttpQuotaRefreshProvider::new()?;
    Ok(start_background_quota_refresh_worker_with_reporter(
        state_db,
        secret_root,
        base_url,
        resolver,
        provider,
        BackgroundQuotaRefreshRuntime::new(
            current_unix_seconds,
            |diagnostic| eprintln!("{diagnostic}"),
            interval,
        )
        .with_quota_floor_notifier(quota_floor_notifier),
    )
    .await)
}

pub(crate) fn refresh_cycle_delay(interval: Duration, elapsed: Duration) -> Duration {
    interval.saturating_sub(elapsed)
}
