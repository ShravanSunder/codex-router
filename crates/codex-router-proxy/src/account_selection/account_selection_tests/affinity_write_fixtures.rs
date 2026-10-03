use super::*;

#[derive(Default)]
pub(super) struct ControlledFailingAffinityWriteRepository {
    pub(super) entered: tokio::sync::Notify,
    pub(super) release: tokio::sync::Notify,
    pub(super) completed: tokio::sync::Notify,
    pub(super) calls: AtomicUsize,
    pub(super) failed: AtomicBool,
}

impl DbWriteRepository for ControlledFailingAffinityWriteRepository {
    fn record_provider_quota_exhausted<'a>(
        &'a self,
        _account_id: AccountId,
        _route_band: RouteBand,
        _classification: ProviderErrorClassification,
        _observed_unix_seconds: u64,
    ) -> futures_util::future::BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async { Ok(()) })
    }

    fn record_session_account_affinity<'a>(
        &'a self,
        _affinity: codex_router_state::session_account_affinity::SessionAccountAffinity,
    ) -> futures_util::future::BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::AcqRel);
            self.entered.notify_one();
            self.release.notified().await;
            self.failed.store(true, Ordering::Release);
            self.completed.notify_one();
            Err(DbWriteRepositoryError::State(
                codex_router_state::sqlite::StateStoreError::Sqlite {
                    message: "controlled session-affinity write failure".to_owned(),
                },
            ))
        })
    }
}
