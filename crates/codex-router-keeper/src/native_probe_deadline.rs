use crate::NativeProbeError;
use codex_router_keeper_protocol::{NativeProbeJob, NativeProbeJobWire};
use std::time::Duration;
use tokio::time::Instant;
#[derive(Clone, Copy)]
pub(crate) struct ProbeDeadlines {
    pub(crate) native: Instant,
    pub(crate) collection: Instant,
}
impl ProbeDeadlines {
    pub(crate) fn new(job: &NativeProbeJob, started: Instant) -> Result<Self, NativeProbeError> {
        // Validate caller duration representation before owning a launch.
        NativeProbeJobWire::try_from(job.clone())?;
        Self::checked_windows(
            job.native_readiness_wait(),
            job.remote_control_wait(),
            started,
        )
    }
    fn checked_windows(
        native: Duration,
        remote: Duration,
        started: Instant,
    ) -> Result<Self, NativeProbeError> {
        let total = native
            .checked_add(remote)
            .ok_or(NativeProbeError::DeadlineOverflow)?;
        Ok(Self {
            native: started
                .checked_add(native)
                .ok_or(NativeProbeError::DeadlineOverflow)?,
            collection: started
                .checked_add(total)
                .ok_or(NativeProbeError::DeadlineOverflow)?,
        })
    }
    pub(crate) fn remaining_job(
        &self,
        job: &NativeProbeJob,
        now: Instant,
    ) -> Result<NativeProbeJobWire, NativeProbeError> {
        let remaining = self
            .native
            .checked_duration_since(now)
            .ok_or(NativeProbeError::NativeBudgetExpired)?;
        let whole_ms =
            u64::try_from(remaining.as_millis()).map_err(|_| NativeProbeError::DeadlineOverflow)?;
        if whole_ms == 0 {
            return Err(NativeProbeError::NativeBudgetExpired);
        }
        Ok(NativeProbeJobWire::try_from(NativeProbeJob::new(
            job.action(),
            job.alias().clone(),
            Duration::from_millis(whole_ms),
            job.remote_control_wait(),
        ))?)
    }
}
#[cfg(test)]
mod deadline_tests {
    use super::*;
    use codex_native_integration::AppServerProbeAction;
    use codex_router_keeper_protocol::GenerationAliasPath;
    fn job(native: Duration, remote: Duration) -> NativeProbeJob {
        NativeProbeJob::new(
            AppServerProbeAction::Observe,
            GenerationAliasPath::try_from(std::path::PathBuf::from("/tmp/gen-12345678-1.sock"))
                .unwrap(),
            native,
            remote,
        )
    }
    #[test]
    fn remaining_native_projection_floors_without_resetting_original_collection() {
        let started = Instant::now();
        let job = job(Duration::from_millis(20), Duration::from_millis(30));
        let deadlines = ProbeDeadlines::new(&job, started).unwrap();
        let projected = NativeProbeJob::from(
            deadlines
                .remaining_job(&job, started + Duration::from_micros(1500))
                .unwrap(),
        );
        assert_eq!(projected.native_readiness_wait(), Duration::from_millis(18));
        assert_eq!(projected.remote_control_wait(), Duration::from_millis(30));
        assert_eq!(deadlines.collection, started + Duration::from_millis(50));
        assert!(matches!(
            deadlines.remaining_job(&job, started + Duration::from_millis(20)),
            Err(NativeProbeError::NativeBudgetExpired)
        ));
    }
    #[test]
    fn unrepresentable_durations_or_absolute_deadlines_refuse_before_launch() {
        assert!(
            ProbeDeadlines::new(
                &job(Duration::from_nanos(1), Duration::from_secs(1)),
                Instant::now()
            )
            .is_err()
        );
        assert!(ProbeDeadlines::new(&job(Duration::MAX, Duration::MAX), Instant::now()).is_err());
        assert!(matches!(
            ProbeDeadlines::checked_windows(Duration::MAX, Duration::MAX, Instant::now()),
            Err(NativeProbeError::DeadlineOverflow)
        ));
    }
}
