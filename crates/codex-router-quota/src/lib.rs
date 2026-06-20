//! Quota snapshot model for codex-router.

pub mod snapshot;
pub mod worker;

/// Returns this crate's package name.
#[must_use]
pub const fn package_name() -> &'static str {
    "codex-router-quota"
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::cell::RefCell;

    use codex_router_core::ids::AccountId;

    use super::package_name;
    use crate::snapshot::QuotaRouteBand;
    use crate::snapshot::QuotaSnapshot;
    use crate::snapshot::SnapshotFreshness;
    use crate::snapshot::SnapshotSource;
    use crate::worker::QuotaRefreshRuntime;
    use crate::worker::QuotaRefreshSchedule;
    use crate::worker::QuotaSnapshotReader;
    use crate::worker::RefreshScheduler;

    #[test]
    fn reports_package_name() {
        assert_eq!(package_name(), "codex-router-quota");
    }

    #[test]
    fn snapshot_freshness_and_headroom_are_deterministic() {
        let account_id = match AccountId::new("acct_primary") {
            Ok(account_id) => account_id,
            Err(error) => panic!("account id should parse: {error}"),
        };
        let snapshot = QuotaSnapshot::new(account_id, SnapshotSource::MockEndpoint, 1_000)
            .with_route_band(QuotaRouteBand::new("responses", 50))
            .with_route_band(QuotaRouteBand::new("realtime", 10))
            .with_reset_unix_seconds(1_900);

        assert_eq!(snapshot.remaining_headroom("responses"), Some(50));
        assert_eq!(
            snapshot.freshness(1_030, 60),
            SnapshotFreshness::Fresh { age_seconds: 30 }
        );
        assert_eq!(
            snapshot.freshness(1_090, 60),
            SnapshotFreshness::StaleWithPenalty { age_seconds: 90 }
        );
    }

    #[test]
    fn missing_or_future_snapshot_is_unknown() {
        assert_eq!(
            QuotaSnapshot::freshness_for_observed_at(None, 1_000, 60),
            SnapshotFreshness::Unknown
        );
        assert_eq!(
            QuotaSnapshot::freshness_for_observed_at(Some(1_030), 1_000, 60),
            SnapshotFreshness::Unknown
        );
    }

    #[test]
    fn startup_uses_existing_snapshot_and_schedules_refresh_without_fetching_inline() {
        let account = account_id("acct_primary");
        let snapshot = QuotaSnapshot::new(account.clone(), SnapshotSource::MockEndpoint, 1_000)
            .with_route_band(QuotaRouteBand::new("responses", 40));
        let reader = FakeSnapshotReader::new(Some(snapshot.clone()));
        let scheduler = FakeRefreshScheduler::default();
        let runtime = QuotaRefreshRuntime::new(&reader, &scheduler);

        let startup = match runtime.start_for_account(&account) {
            Ok(startup) => startup,
            Err(error) => panic!("refresh runtime should start: {error}"),
        };

        assert_eq!(startup.existing_snapshot(), Some(&snapshot));
        assert_eq!(
            scheduler.scheduled.borrow().as_slice(),
            &[QuotaRefreshSchedule::Account(account)]
        );
        assert_eq!(scheduler.inline_refresh_calls.get(), 0);
    }

    struct FakeSnapshotReader {
        snapshot: Option<QuotaSnapshot>,
    }

    impl FakeSnapshotReader {
        fn new(snapshot: Option<QuotaSnapshot>) -> Self {
            Self { snapshot }
        }
    }

    impl QuotaSnapshotReader for FakeSnapshotReader {
        fn load_snapshot(&self, _account_id: &AccountId) -> Result<Option<QuotaSnapshot>, String> {
            Ok(self.snapshot.clone())
        }
    }

    #[derive(Default)]
    struct FakeRefreshScheduler {
        scheduled: RefCell<Vec<QuotaRefreshSchedule>>,
        inline_refresh_calls: Cell<u32>,
    }

    impl RefreshScheduler for FakeRefreshScheduler {
        fn schedule(&self, schedule: QuotaRefreshSchedule) -> Result<(), String> {
            self.scheduled.borrow_mut().push(schedule);
            Ok(())
        }

        fn refresh_inline_for_test(&self) {
            self.inline_refresh_calls
                .set(self.inline_refresh_calls.get() + 1);
        }
    }

    fn account_id(value: &str) -> AccountId {
        match AccountId::new(value) {
            Ok(account_id) => account_id,
            Err(error) => panic!("account id should parse: {error}"),
        }
    }
}
