use super::*;

impl LoopbackRouterRuntime {
    pub(super) fn enqueue_runtime_maintenance_hints(&self, now_unix_seconds: u64) {
        const ROLLUP_BUCKET_SECONDS: u64 = 300;
        const ACTIVE_CLIENT_STALE_AFTER_SECONDS: u64 = 600;
        const ACTIVE_SESSION_RETENTION_SECONDS: u64 = 86_400;
        const SESSION_ACCOUNT_AFFINITY_RETENTION_SECONDS: u64 = 7 * 86_400;

        if claim_session_affinity_cleanup_day(
            &self.last_session_affinity_cleanup_utc_day,
            now_unix_seconds,
        ) {
            let _cleanup_result = self.maintenance_actor.try_enqueue(
                MaintenanceHint::CleanupStaleSessionAccountAffinities {
                    stale_before_unix_seconds: now_unix_seconds
                        .saturating_sub(SESSION_ACCOUNT_AFFINITY_RETENTION_SECONDS),
                },
            );
        }

        let interval_start_unix_seconds =
            now_unix_seconds.saturating_sub(now_unix_seconds % ROLLUP_BUCKET_SECONDS);
        let interval_end_unix_seconds = interval_start_unix_seconds + ROLLUP_BUCKET_SECONDS;
        for route_band in [
            RouteBand::Responses,
            RouteBand::ResponsesCompact,
            RouteBand::Models,
            RouteBand::MemoriesTraceSummarize,
        ] {
            let _cleanup_result =
                self.maintenance_actor
                    .try_enqueue(MaintenanceHint::CleanupStaleActiveClients {
                        route_band,
                        stale_before_unix_seconds: now_unix_seconds
                            .saturating_sub(ACTIVE_CLIENT_STALE_AFTER_SECONDS),
                    });
            let _rollup_result =
                self.maintenance_actor
                    .try_enqueue(MaintenanceHint::RefreshActiveSessionRollups {
                        route_band,
                        interval_start_unix_seconds,
                        interval_end_unix_seconds,
                        bucket_seconds: ROLLUP_BUCKET_SECONDS,
                    });
            let _retention_result =
                self.maintenance_actor
                    .try_enqueue(MaintenanceHint::ApplyActiveSessionRetention {
                        route_band,
                        retain_after_unix_seconds: now_unix_seconds
                            .saturating_sub(ACTIVE_SESSION_RETENTION_SECONDS),
                    });
            let _compaction_result =
                self.maintenance_actor
                    .try_enqueue(MaintenanceHint::CompactActiveSessionHistory {
                        route_band,
                        compact_before_unix_seconds: active_session_event_compaction_before(
                            now_unix_seconds,
                        ),
                    });
        }
    }
}
