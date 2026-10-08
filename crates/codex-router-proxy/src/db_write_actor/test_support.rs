use std::sync::Mutex;

use codex_router_core::ids::AccountId;
use codex_router_core::ids::ReservationId;
use codex_router_core::routes::RouteBand;
use futures_util::future::BoxFuture;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use tokio::sync::Notify;

use crate::provider_error::ProviderErrorClassification;
use codex_router_state::affinity_owner::PreviousResponseAffinityOwnerRecord;
use codex_router_state::session_account_affinity::SessionAccountAffinity;

use super::DbWriteRepository;
use super::DbWriteRepositoryError;

#[derive(Default)]
pub(super) struct BlockingDbWriteRepository {
    pub(super) entered: Notify,
    pub(super) release: Notify,
    calls: AtomicUsize,
}

#[derive(Default)]
pub(super) struct ShutdownBlockedDbWriteRepository {
    pub(super) provider_entered: Notify,
    pub(super) provider_release: Notify,
    pub(super) affinity_entered: Notify,
    pub(super) affinity_release: Notify,
}

impl DbWriteRepository for ShutdownBlockedDbWriteRepository {
    fn record_provider_quota_exhausted<'a>(
        &'a self,
        _account_id: AccountId,
        _route_band: RouteBand,
        _classification: ProviderErrorClassification,
        _observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async move {
            self.provider_entered.notify_one();
            self.provider_release.notified().await;
            Ok(())
        })
    }

    fn record_session_account_affinity<'a>(
        &'a self,
        _affinity: SessionAccountAffinity,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async move {
            self.affinity_entered.notify_one();
            self.affinity_release.notified().await;
            Ok(())
        })
    }
}

impl DbWriteRepository for BlockingDbWriteRepository {
    fn record_provider_quota_exhausted<'a>(
        &'a self,
        _account_id: AccountId,
        _route_band: RouteBand,
        _classification: ProviderErrorClassification,
        _observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        self.block_first_write()
    }

    fn record_active_client_acquired<'a>(
        &'a self,
        _route_band: RouteBand,
        _process_run_id: String,
        _reservation_id: ReservationId,
        _account_id: AccountId,
        _acquired_unix_seconds: u64,
        _active_pressure: u32,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        self.block_first_write()
    }

    fn record_active_client_released<'a>(
        &'a self,
        _route_band: RouteBand,
        _process_run_id: String,
        _reservation_id: ReservationId,
        _released_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        self.block_first_write()
    }

    fn record_previous_response_affinity_owner<'a>(
        &'a self,
        _owner: PreviousResponseAffinityOwnerRecord,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        self.block_first_write()
    }

    fn record_session_account_affinity<'a>(
        &'a self,
        _affinity: SessionAccountAffinity,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        self.block_first_write()
    }
}

impl BlockingDbWriteRepository {
    pub(super) fn block_first_write<'a>(
        &'a self,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async move {
            self.entered.notify_waiters();
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                self.release.notified().await;
            }
            Ok(())
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum RecordedDbWrite {
    ProviderQuotaExhausted {
        account_id: AccountId,
        route_band: RouteBand,
        classification: ProviderErrorClassification,
        observed_unix_seconds: u64,
    },
    PreviousResponseAffinityOwner(PreviousResponseAffinityOwnerRecord),
    SessionAccountAffinity(SessionAccountAffinity),
    ActiveClientAcquired {
        route_band: RouteBand,
        process_run_id: String,
        reservation_id: ReservationId,
        account_id: AccountId,
        acquired_unix_seconds: u64,
        active_pressure: u32,
    },
    ActiveClientReleased {
        route_band: RouteBand,
        process_run_id: String,
        reservation_id: ReservationId,
        released_unix_seconds: u64,
    },
}

#[derive(Default)]
pub(super) struct RecordingDbWriteRepository {
    records: Mutex<Vec<RecordedDbWrite>>,
}

impl RecordingDbWriteRepository {
    pub(super) fn records(&self) -> Vec<RecordedDbWrite> {
        self.records
            .lock()
            .unwrap_or_else(|error| panic!("recording repository lock should hold: {error}"))
            .clone()
    }
}

impl DbWriteRepository for RecordingDbWriteRepository {
    fn record_provider_quota_exhausted<'a>(
        &'a self,
        account_id: AccountId,
        route_band: RouteBand,
        classification: ProviderErrorClassification,
        observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async move {
            self.records
                .lock()
                .unwrap_or_else(|error| panic!("recording repository lock should hold: {error}"))
                .push(RecordedDbWrite::ProviderQuotaExhausted {
                    account_id,
                    route_band,
                    classification,
                    observed_unix_seconds,
                });
            Ok(())
        })
    }

    fn record_active_client_acquired<'a>(
        &'a self,
        route_band: RouteBand,
        process_run_id: String,
        reservation_id: ReservationId,
        account_id: AccountId,
        acquired_unix_seconds: u64,
        active_pressure: u32,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async move {
            self.records
                .lock()
                .unwrap_or_else(|error| panic!("recording repository lock should hold: {error}"))
                .push(RecordedDbWrite::ActiveClientAcquired {
                    route_band,
                    process_run_id,
                    reservation_id,
                    account_id,
                    acquired_unix_seconds,
                    active_pressure,
                });
            Ok(())
        })
    }

    fn record_previous_response_affinity_owner<'a>(
        &'a self,
        owner: PreviousResponseAffinityOwnerRecord,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async move {
            self.records
                .lock()
                .unwrap_or_else(|error| panic!("recording repository lock should hold: {error}"))
                .push(RecordedDbWrite::PreviousResponseAffinityOwner(owner));
            Ok(())
        })
    }

    fn record_session_account_affinity<'a>(
        &'a self,
        affinity: SessionAccountAffinity,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async move {
            self.records
                .lock()
                .unwrap_or_else(|error| panic!("recording repository lock should hold: {error}"))
                .push(RecordedDbWrite::SessionAccountAffinity(affinity));
            Ok(())
        })
    }

    fn record_active_client_released<'a>(
        &'a self,
        route_band: RouteBand,
        process_run_id: String,
        reservation_id: ReservationId,
        released_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async move {
            self.records
                .lock()
                .unwrap_or_else(|error| panic!("recording repository lock should hold: {error}"))
                .push(RecordedDbWrite::ActiveClientReleased {
                    route_band,
                    process_run_id,
                    reservation_id,
                    released_unix_seconds,
                });
            Ok(())
        })
    }
}

pub(super) struct SlowRecordingDbWriteRepository {
    delay: std::time::Duration,
    records: Mutex<Vec<RecordedDbWrite>>,
}

impl SlowRecordingDbWriteRepository {
    pub(super) fn new(delay: std::time::Duration) -> Self {
        Self {
            delay,
            records: Mutex::new(Vec::new()),
        }
    }

    pub(super) fn records(&self) -> Vec<RecordedDbWrite> {
        self.records
            .lock()
            .unwrap_or_else(|error| panic!("slow recording repository lock should hold: {error}"))
            .clone()
    }
}

impl DbWriteRepository for SlowRecordingDbWriteRepository {
    fn record_provider_quota_exhausted<'a>(
        &'a self,
        account_id: AccountId,
        route_band: RouteBand,
        classification: ProviderErrorClassification,
        observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async move {
            tokio::time::sleep(self.delay).await;
            self.records
                .lock()
                .unwrap_or_else(|error| {
                    panic!("slow recording repository lock should hold: {error}")
                })
                .push(RecordedDbWrite::ProviderQuotaExhausted {
                    account_id,
                    route_band,
                    classification,
                    observed_unix_seconds,
                });
            Ok(())
        })
    }
}

#[derive(Default)]
pub(super) struct FailingOnceDbWriteRepository {
    calls: AtomicUsize,
}

impl DbWriteRepository for FailingOnceDbWriteRepository {
    fn record_provider_quota_exhausted<'a>(
        &'a self,
        _account_id: AccountId,
        _route_band: RouteBand,
        _classification: ProviderErrorClassification,
        _observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async move {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(DbWriteRepositoryError::State(
                    codex_router_state::sqlite::StateStoreError::Sqlite {
                        message: "injected write failure".to_owned(),
                    },
                ));
            }
            Ok(())
        })
    }
}

pub(super) struct FailingWriteRepository;

impl DbWriteRepository for FailingWriteRepository {
    fn record_provider_quota_exhausted<'a>(
        &'a self,
        _account_id: AccountId,
        _route_band: RouteBand,
        _classification: ProviderErrorClassification,
        _observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async move {
            Err(DbWriteRepositoryError::State(
                codex_router_state::sqlite::StateStoreError::Sqlite {
                    message: "injected write failure before health probe".to_owned(),
                },
            ))
        })
    }
}

#[derive(Default)]
pub(super) struct FailingSessionAffinityRepository {
    pub(super) write_attempted: Notify,
}

impl DbWriteRepository for FailingSessionAffinityRepository {
    fn record_provider_quota_exhausted<'a>(
        &'a self,
        _account_id: AccountId,
        _route_band: RouteBand,
        _classification: ProviderErrorClassification,
        _observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async { Ok(()) })
    }

    fn record_session_account_affinity<'a>(
        &'a self,
        _affinity: SessionAccountAffinity,
    ) -> BoxFuture<'a, Result<(), DbWriteRepositoryError>> {
        Box::pin(async move {
            self.write_attempted.notify_one();
            Err(DbWriteRepositoryError::State(
                codex_router_state::sqlite::StateStoreError::Sqlite {
                    message: "injected session affinity write failure".to_owned(),
                },
            ))
        })
    }
}

pub(super) fn account_id(value: &str) -> AccountId {
    AccountId::new(value).unwrap_or_else(|error| panic!("test account id should parse: {error}"))
}
