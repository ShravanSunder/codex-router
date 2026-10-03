use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RecordedProviderError {
    pub(super) account_id: AccountId,
    pub(super) route_band: codex_router_core::routes::RouteBand,
    pub(super) classification: ProviderErrorClassification,
}

#[derive(Clone, Debug)]
pub(super) struct RecordingAsyncProviderErrorObserver {
    pub(super) records: Arc<Mutex<Vec<RecordedProviderError>>>,
    pub(super) observed: Arc<Notify>,
    pub(super) post_exhaustion_outcome: PostExhaustionRouteBandOutcome,
}

impl Default for RecordingAsyncProviderErrorObserver {
    fn default() -> Self {
        Self {
            records: Arc::new(Mutex::new(Vec::new())),
            observed: Arc::new(Notify::new()),
            post_exhaustion_outcome: PostExhaustionRouteBandOutcome::SelectableAlternative,
        }
    }
}

impl RecordingAsyncProviderErrorObserver {
    pub(super) fn with_selectable_alternative(selectable_alternative: bool) -> Self {
        Self {
            post_exhaustion_outcome: if selectable_alternative {
                PostExhaustionRouteBandOutcome::SelectableAlternative
            } else {
                PostExhaustionRouteBandOutcome::NoSelectableAlternative
            },
            ..Self::default()
        }
    }

    pub(super) fn with_post_exhaustion_outcome(
        post_exhaustion_outcome: PostExhaustionRouteBandOutcome,
    ) -> Self {
        Self {
            post_exhaustion_outcome,
            ..Self::default()
        }
    }

    pub(super) fn records(&self) -> Vec<RecordedProviderError> {
        self.records.lock().map_or_else(
            |error| panic!("records lock should be available: {error}"),
            |records| records.clone(),
        )
    }
}

impl AsyncProviderErrorObserver for RecordingAsyncProviderErrorObserver {
    fn observe_provider_error<'a>(
        &'a self,
        account_id: AccountId,
        route_band: codex_router_core::routes::RouteBand,
        classification: ProviderErrorClassification,
        _observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), ProviderErrorObservationError>> {
        Box::pin(async move {
            self.records
                .lock()
                .unwrap_or_else(|error| panic!("records lock should be available: {error}"))
                .push(RecordedProviderError {
                    account_id,
                    route_band,
                    classification,
                });
            self.observed.notify_one();
            Ok(())
        })
    }

    fn route_band_post_exhaustion_outcome<'a>(
        &'a self,
        _exhausted_account_id: AccountId,
        _route_band: codex_router_core::routes::RouteBand,
        _route_profile: codex_router_core::route_profile::RouteProfile,
        _observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<PostExhaustionRouteBandOutcome, ProviderErrorObservationError>> {
        Box::pin(async move { Ok(self.post_exhaustion_outcome) })
    }

    fn enqueue_provider_quota_exhaustion(
        &self,
        account_id: AccountId,
        route_band: codex_router_core::routes::RouteBand,
        classification: ProviderErrorClassification,
        _observed_unix_seconds: u64,
    ) -> DbWriteEnqueueResult {
        self.records
            .lock()
            .unwrap_or_else(|error| panic!("records lock should be available: {error}"))
            .push(RecordedProviderError {
                account_id,
                route_band,
                classification,
            });
        self.observed.notify_one();
        DbWriteEnqueueResult::Enqueued
    }
}

#[derive(Clone, Debug)]
pub(super) struct BlockingAsyncProviderErrorObserver {
    pub(super) records: Arc<Mutex<Vec<RecordedProviderError>>>,
    pub(super) observed: Arc<Notify>,
    pub(super) release: Arc<Notify>,
    pub(super) selectable_alternative: bool,
}

impl Default for BlockingAsyncProviderErrorObserver {
    fn default() -> Self {
        Self {
            records: Arc::new(Mutex::new(Vec::new())),
            observed: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
            selectable_alternative: true,
        }
    }
}

impl BlockingAsyncProviderErrorObserver {
    pub(super) fn with_selectable_alternative(selectable_alternative: bool) -> Self {
        Self {
            selectable_alternative,
            ..Self::default()
        }
    }

    pub(super) fn records(&self) -> Vec<RecordedProviderError> {
        self.records.lock().map_or_else(
            |error| panic!("records lock should be available: {error}"),
            |records| records.clone(),
        )
    }

    pub(super) fn release_observation(&self) {
        self.release.notify_waiters();
    }
}

impl AsyncProviderErrorObserver for BlockingAsyncProviderErrorObserver {
    fn observe_provider_error<'a>(
        &'a self,
        account_id: AccountId,
        route_band: codex_router_core::routes::RouteBand,
        classification: ProviderErrorClassification,
        _observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), ProviderErrorObservationError>> {
        Box::pin(async move {
            self.records
                .lock()
                .unwrap_or_else(|error| panic!("records lock should be available: {error}"))
                .push(RecordedProviderError {
                    account_id,
                    route_band,
                    classification,
                });
            self.observed.notify_waiters();
            self.release.notified().await;
            Ok(())
        })
    }

    fn route_band_post_exhaustion_outcome<'a>(
        &'a self,
        _exhausted_account_id: AccountId,
        _route_band: codex_router_core::routes::RouteBand,
        _route_profile: codex_router_core::route_profile::RouteProfile,
        _observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<PostExhaustionRouteBandOutcome, ProviderErrorObservationError>> {
        Box::pin(async move {
            Ok(if self.selectable_alternative {
                PostExhaustionRouteBandOutcome::SelectableAlternative
            } else {
                PostExhaustionRouteBandOutcome::NoSelectableAlternative
            })
        })
    }

    fn enqueue_provider_quota_exhaustion(
        &self,
        account_id: AccountId,
        route_band: codex_router_core::routes::RouteBand,
        classification: ProviderErrorClassification,
        _observed_unix_seconds: u64,
    ) -> DbWriteEnqueueResult {
        self.records
            .lock()
            .unwrap_or_else(|error| panic!("records lock should be available: {error}"))
            .push(RecordedProviderError {
                account_id,
                route_band,
                classification,
            });
        self.observed.notify_waiters();
        DbWriteEnqueueResult::Enqueued
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct PendingAlternativeSelectionProviderErrorObserver;

impl AsyncProviderErrorObserver for PendingAlternativeSelectionProviderErrorObserver {
    fn observe_provider_error<'a>(
        &'a self,
        _account_id: AccountId,
        _route_band: codex_router_core::routes::RouteBand,
        _classification: ProviderErrorClassification,
        _observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), ProviderErrorObservationError>> {
        Box::pin(async { Ok(()) })
    }

    fn enqueue_provider_quota_exhaustion(
        &self,
        _account_id: AccountId,
        _route_band: codex_router_core::routes::RouteBand,
        _classification: ProviderErrorClassification,
        _observed_unix_seconds: u64,
    ) -> DbWriteEnqueueResult {
        DbWriteEnqueueResult::Enqueued
    }

    fn route_band_post_exhaustion_outcome<'a>(
        &'a self,
        _exhausted_account_id: AccountId,
        _route_band: codex_router_core::routes::RouteBand,
        _route_profile: codex_router_core::route_profile::RouteProfile,
        _observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<PostExhaustionRouteBandOutcome, ProviderErrorObservationError>> {
        Box::pin(async { std::future::pending().await })
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct FailingAsyncProviderErrorObserver;

impl AsyncProviderErrorObserver for FailingAsyncProviderErrorObserver {
    fn observe_provider_error<'a>(
        &'a self,
        _account_id: AccountId,
        _route_band: codex_router_core::routes::RouteBand,
        _classification: ProviderErrorClassification,
        _observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), ProviderErrorObservationError>> {
        Box::pin(async {
            Err(ProviderErrorObservationError::State(
                codex_router_state::sqlite::StateStoreError::UnsupportedSchemaVersion {
                    version: 0,
                },
            ))
        })
    }

    fn enqueue_provider_quota_exhaustion(
        &self,
        _account_id: AccountId,
        _route_band: codex_router_core::routes::RouteBand,
        _classification: ProviderErrorClassification,
        _observed_unix_seconds: u64,
    ) -> DbWriteEnqueueResult {
        DbWriteEnqueueResult::FullDegraded
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct DefaultAlternativeSelectionProviderErrorObserver;

impl AsyncProviderErrorObserver for DefaultAlternativeSelectionProviderErrorObserver {
    fn observe_provider_error<'a>(
        &'a self,
        _account_id: AccountId,
        _route_band: codex_router_core::routes::RouteBand,
        _classification: ProviderErrorClassification,
        _observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), ProviderErrorObservationError>> {
        Box::pin(async { Ok(()) })
    }

    fn enqueue_provider_quota_exhaustion(
        &self,
        _account_id: AccountId,
        _route_band: codex_router_core::routes::RouteBand,
        _classification: ProviderErrorClassification,
        _observed_unix_seconds: u64,
    ) -> DbWriteEnqueueResult {
        DbWriteEnqueueResult::Enqueued
    }
}

#[derive(Debug)]
pub(super) struct BlockingAsyncAffinityOwnerRecorder {
    pub(super) entered: Arc<Notify>,
    pub(super) release: Arc<Notify>,
}

impl BlockingAsyncAffinityOwnerRecorder {
    pub(super) fn new(entered: Arc<Notify>, release: Arc<Notify>) -> Self {
        Self { entered, release }
    }
}

impl AsyncHttpAffinityOwnerRecorder for BlockingAsyncAffinityOwnerRecorder {
    fn record_affinity_owner<'a>(
        &'a self,
        _owner: PreviousResponseAffinityOwnerRecord,
    ) -> BoxFuture<'a, Result<(), HttpProxyError>> {
        Box::pin(async move {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(())
        })
    }
}
