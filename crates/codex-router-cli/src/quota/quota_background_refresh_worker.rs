use super::*;

/// Stoppable background quota refresh worker.
pub(crate) struct BackgroundQuotaRefreshWorker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

pub(crate) struct BackgroundQuotaRefreshRuntime<C, D> {
    observed_clock: C,
    diagnostic_reporter: D,
    interval: Duration,
    quota_floor_notifier: Option<Arc<dyn WeeklyQuotaFloorReachedObserver>>,
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

impl Drop for BackgroundQuotaRefreshWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _result = thread.join();
        }
    }
}

#[cfg(test)]
pub(crate) fn start_background_quota_refresh_worker_with_dependencies<R, P>(
    state_db: PathBuf,
    secret_root: PathBuf,
    base_url: String,
    credential_resolver: R,
    quota_provider: P,
    interval: Duration,
) -> BackgroundQuotaRefreshWorker
where
    R: AsyncProviderCredentialResolver + Send + 'static,
    P: QuotaRefreshProvider + Send + 'static,
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
}

#[cfg(test)]
pub(crate) fn start_background_quota_refresh_worker_with_clock<R, P, C>(
    state_db: PathBuf,
    secret_root: PathBuf,
    base_url: String,
    credential_resolver: R,
    quota_provider: P,
    observed_clock: C,
    interval: Duration,
) -> BackgroundQuotaRefreshWorker
where
    R: AsyncProviderCredentialResolver + Send + 'static,
    P: QuotaRefreshProvider + Send + 'static,
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
}

pub(crate) fn start_background_quota_refresh_worker_with_reporter<R, P, C, D>(
    state_db: PathBuf,
    secret_root: PathBuf,
    base_url: String,
    credential_resolver: R,
    quota_provider: P,
    runtime: BackgroundQuotaRefreshRuntime<C, D>,
) -> BackgroundQuotaRefreshWorker
where
    R: AsyncProviderCredentialResolver + Send + 'static,
    P: QuotaRefreshProvider + Send + 'static,
    C: FnMut() -> u64 + Send + 'static,
    D: FnMut(String) + Send + 'static,
{
    let BackgroundQuotaRefreshRuntime {
        mut observed_clock,
        mut diagnostic_reporter,
        interval,
        quota_floor_notifier,
    } = runtime;
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = Arc::clone(&stop);
    let thread = thread::spawn(move || {
        loop {
            let mut sink = Vec::new();
            let observed_unix_seconds = observed_clock();
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(QuotaCommandError::BackgroundWorkerInitialization)
                .and_then(|refresh_runtime| {
                    refresh_runtime.block_on(
                        refresh_quota_store_paths_with_dependencies_and_floor_notifier(
                            &mut sink,
                            &state_db,
                            &secret_root,
                            base_url.clone(),
                            &credential_resolver,
                            &quota_provider,
                            QuotaRefreshObservationContext {
                                observed_unix_seconds,
                                weekly_floor_observer: quota_floor_notifier.as_deref(),
                            },
                        ),
                    )
                });
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
            if interval.is_zero() || !sleep_interruptibly(&stop_for_thread, interval) {
                break;
            }
        }
    });

    BackgroundQuotaRefreshWorker {
        stop,
        thread: Some(thread),
    }
}

pub(crate) fn start_background_quota_refresh_worker(
    state_db: PathBuf,
    secret_root: PathBuf,
    base_url: String,
    interval: Duration,
    quota_floor_notifier: WebSocketQuotaFloorNotifier,
) -> Result<BackgroundQuotaRefreshWorker, QuotaCommandError> {
    let resolver = CliCredentialResolver::open(&state_db, &secret_root, current_unix_seconds())?;
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
    ))
}

fn sleep_interruptibly(stop: &AtomicBool, interval: Duration) -> bool {
    let mut remaining = interval;
    while !stop.load(Ordering::SeqCst) {
        if remaining.is_zero() {
            return true;
        }
        let step = remaining.min(Duration::from_millis(50));
        thread::sleep(step);
        remaining = remaining.saturating_sub(step);
    }

    false
}
