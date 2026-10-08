//! One mutable owner: launch/IO futures and cleanup debt outlive any single operation wait.
use crate::{
    GroupStopProgress, GroupStopStatus, GroupStopTiming, ImageLaunchOutcome, ImageLease,
    ImageRegistry, NativeProbeError, OwnedProcessGroup, OwnedSpawnCleanup,
};
use crate::{
    native_probe_channel::{self, AttachmentFuture, AttachmentOutcome, JobWrite, ProbeChannel},
    native_probe_deadline::ProbeDeadlines,
    native_probe_launch::{ProbeLaunchFuture, ProbeLauncher},
};
use codex_router_keeper_protocol::{
    ChildPgid, ChildPid, ComponentKind, NativeProbeJob, NativeProbeResult,
};
use std::process::ExitStatus;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
enum ProbePhase {
    Idle,
    Launching(ProbeLaunchFuture),
    Attaching(AttachmentFuture),
    Running(ProbeChannel),
    DrainingLaunch(ProbeLaunchFuture),
    DrainingAttach(AttachmentFuture),
    CleanupGroup {
        group: OwnedProcessGroup,
        _image: ImageLease,
    },
    CleanupSetup {
        cleanup: OwnedSpawnCleanup,
        _image: ImageLease,
    },
    Fenced,
}
pub struct NativeProbeProcess {
    launcher: ProbeLauncher,
    phase: ProbePhase,
    job: Option<NativeProbeJob>,
    deadlines: Option<ProbeDeadlines>,
    child_pid: Option<ChildPid>,
    original_group: Option<ChildPgid>,
    exit_status: Option<ExitStatus>,
    stderr: Vec<u8>,
    #[cfg(test)]
    cleanup_observation_started: Option<Instant>,
}
impl NativeProbeProcess {
    pub fn new(registry: &ImageRegistry, image: &ImageLease) -> Self {
        Self {
            launcher: registry.native_probe_launcher(image),
            phase: ProbePhase::Idle,
            job: None,
            deadlines: None,
            child_pid: None,
            original_group: None,
            exit_status: None,
            stderr: Vec::new(),
            #[cfg(test)]
            cleanup_observation_started: None,
        }
    }
    #[cfg(test)]
    pub(crate) fn fixture(
        registry: &ImageRegistry,
        image: &ImageLease,
        prefix: Vec<std::ffi::OsString>,
    ) -> Self {
        let mut process = Self::new(registry, image);
        process.launcher = process.launcher.fixture_prefix(prefix);
        process
    }
    #[cfg(test)]
    pub(crate) fn test_cleanup_observation_start(&mut self) -> Instant {
        *self
            .cleanup_observation_started
            .get_or_insert_with(Instant::now)
    }
    pub fn start_job(&mut self, job: NativeProbeJob) -> Result<(), NativeProbeError> {
        if !matches!(self.phase, ProbePhase::Idle | ProbePhase::Fenced) {
            return Err(NativeProbeError::Busy);
        }
        let started = Instant::now();
        let deadlines = ProbeDeadlines::new(&job, started)?;
        if job.native_readiness_wait().is_zero() {
            return Err(NativeProbeError::NativeBudgetExpired);
        }
        self.deadlines = Some(deadlines);
        self.job = Some(job);
        self.child_pid = None;
        self.original_group = None;
        self.exit_status = None;
        self.stderr.clear();
        #[cfg(test)]
        {
            self.cleanup_observation_started = None;
        }
        self.phase = ProbePhase::Launching(self.launcher.launch());
        Ok(())
    }
    pub fn leader_pid(&self) -> Option<ChildPid> {
        self.child_pid
    }
    pub fn original_process_group_id(&self) -> Option<ChildPgid> {
        self.original_group
    }
    pub fn leader_exit_status(&self) -> Option<ExitStatus> {
        self.exit_status
    }
    pub fn stderr_bytes(&self) -> &[u8] {
        &self.stderr
    }
    pub fn cleanup_fenced(&self) -> bool {
        matches!(self.phase, ProbePhase::Idle | ProbePhase::Fenced)
    }
    /// Retain this owner after an error; drain_cleanup completes the same child/image debt.
    pub async fn collect(
        &mut self,
        cancel: &CancellationToken,
    ) -> Result<NativeProbeResult, NativeProbeError> {
        let outcome = self.collect_inner(cancel).await;
        if outcome.is_err() {
            self.enter_cleanup();
        }
        outcome
    }
    async fn collect_inner(
        &mut self,
        cancel: &CancellationToken,
    ) -> Result<NativeProbeResult, NativeProbeError> {
        let deadlines = self.deadlines.ok_or(NativeProbeError::NotStarted)?;
        let mut ticks = tokio::time::interval_at(
            Instant::now() + crate::lifecycle_bounds::GROUP_POLL_INTERVAL,
            crate::lifecycle_bounds::GROUP_POLL_INTERVAL,
        );
        loop {
            if cancel.is_cancelled() {
                return Err(NativeProbeError::Cancelled);
            }
            if Instant::now() >= deadlines.collection {
                return Err(NativeProbeError::CollectionExpired);
            }
            match &mut self.phase {
                ProbePhase::Launching(future) => {
                    let outcome = tokio::select! {biased;
                        _=cancel.cancelled()=>return Err(NativeProbeError::Cancelled),
                        _=tokio::time::sleep_until(deadlines.native)=>return Err(NativeProbeError::NativeBudgetExpired),
                        outcome=future=>outcome,
                    };
                    match outcome {
                        Ok(outcome) => self.accept_launch(outcome, false)?,
                        Err(reason) => {
                            self.phase = ProbePhase::Fenced;
                            return Err(reason.into());
                        }
                    }
                }
                ProbePhase::Attaching(future) => {
                    let outcome = tokio::select! {biased;
                        _=cancel.cancelled()=>return Err(NativeProbeError::Cancelled),
                        _=tokio::time::sleep_until(deadlines.native)=>return Err(NativeProbeError::NativeBudgetExpired),
                        outcome=future=>outcome,
                    };
                    self.accept_attachment(outcome, false)?;
                }
                ProbePhase::Running(channel) => {
                    enum Event {
                        Read(
                            Result<
                                (
                                    codex_router_keeper_protocol::PipeFrameReader,
                                    Option<codex_router_keeper_protocol::JsonMessage>,
                                ),
                                NativeProbeError,
                            >,
                        ),
                        Sent(Result<(), NativeProbeError>),
                        Stderr(Result<Vec<u8>, NativeProbeError>),
                        Tick,
                    }
                    let transmitting = !matches!(channel.writing, JobWrite::Closed);
                    let event = tokio::select! {biased;
                        _=cancel.cancelled()=>return Err(NativeProbeError::Cancelled),
                        _=tokio::time::sleep_until(deadlines.collection)=>return Err(NativeProbeError::CollectionExpired),
                        _=tokio::time::sleep_until(deadlines.native),if transmitting=>return Err(NativeProbeError::NativeBudgetExpired),
                        result=native_probe_channel::next_frame(&mut channel.reading)=>Event::Read(result),
                        result=native_probe_channel::send_frame(&mut channel.writing)=>Event::Sent(result),
                        result=native_probe_channel::drain_stderr(&mut channel.stderr)=>Event::Stderr(result),
                        _=ticks.tick()=>Event::Tick,
                    };
                    let observing_group = matches!(event, Event::Tick);
                    match event {
                        Event::Read(result) => {
                            let (reader, record) = result?;
                            if channel.accept_frame(
                                reader,
                                record,
                                self.launcher.image().fingerprint(ComponentKind::Keeper),
                            )? {
                                let job = self.job.as_ref().ok_or(NativeProbeError::NotStarted)?;
                                channel.send_job(deadlines.remaining_job(job, Instant::now())?)?;
                            }
                        }
                        Event::Sent(result) => {
                            result?;
                            channel.writing = JobWrite::Closed;
                        }
                        Event::Stderr(result) => {
                            channel.stderr_bytes = result?;
                            channel.stderr = None;
                        }
                        Event::Tick => {}
                    }
                    if !observing_group {
                        continue;
                    }
                    let observation = channel.group.tick(Instant::now());
                    // The sole group owner can reap the leader before a later probe fails.
                    // Preserve that real status before classifying the collection failure.
                    self.exit_status = channel.group.leader_exit_status();
                    if self.exit_status.is_some_and(|status| !status.success()) {
                        return Err(NativeProbeError::AbnormalExit);
                    }
                    let status = observation?;
                    if channel.io_finished()
                        && matches!(status, GroupStopStatus::GroupEmpty { .. })
                        && self.exit_status.is_some_and(|status| status.success())
                    {
                        let result = channel.result.take().ok_or(NativeProbeError::RecordCount)?;
                        self.stderr = std::mem::take(&mut channel.stderr_bytes);
                        self.phase = ProbePhase::Fenced;
                        return Ok(result);
                    }
                }
                ProbePhase::Idle | ProbePhase::Fenced => return Err(NativeProbeError::NotStarted),
                _ => return Err(NativeProbeError::CleanupRequired),
            }
        }
    }
    fn accept_launch(
        &mut self,
        outcome: ImageLaunchOutcome,
        cleaning: bool,
    ) -> Result<(), NativeProbeError> {
        match outcome {
            ImageLaunchOutcome::Launched { group, image } => {
                self.child_pid = Some(group.leader_pid());
                self.original_group = Some(group.process_group_id());
                self.phase = if cleaning {
                    ProbePhase::CleanupGroup {
                        group,
                        _image: image,
                    }
                } else {
                    ProbePhase::Attaching(native_probe_channel::attach(group, image))
                };
                Ok(())
            }
            ImageLaunchOutcome::Refused { reason } => {
                self.phase = ProbePhase::Fenced;
                Err(reason.into())
            }
            ImageLaunchOutcome::CleanupPending {
                reason,
                cleanup,
                image,
            } => {
                self.child_pid = cleanup.leader_pid();
                self.original_group = cleanup.original_process_group_id();
                self.phase = ProbePhase::CleanupSetup {
                    cleanup,
                    _image: image,
                };
                Err(reason.into())
            }
        }
    }
    fn accept_attachment(
        &mut self,
        outcome: AttachmentOutcome,
        cleaning: bool,
    ) -> Result<(), NativeProbeError> {
        match outcome {
            AttachmentOutcome::Attached(channel) => {
                if cleaning {
                    let ProbeChannel { group, image, .. } = channel;
                    self.phase = ProbePhase::CleanupGroup {
                        group,
                        _image: image,
                    };
                } else {
                    self.phase = ProbePhase::Running(channel);
                }
                Ok(())
            }
            AttachmentOutcome::Failed {
                reason,
                group,
                image,
            } => {
                self.phase = ProbePhase::CleanupGroup {
                    group,
                    _image: image,
                };
                Err(reason)
            }
        }
    }
    fn enter_cleanup(&mut self) {
        let previous = std::mem::replace(&mut self.phase, ProbePhase::Fenced);
        self.phase = match previous {
            ProbePhase::Launching(future) => ProbePhase::DrainingLaunch(future),
            ProbePhase::Attaching(future) => ProbePhase::DrainingAttach(future),
            ProbePhase::Running(channel) => {
                let ProbeChannel { group, image, .. } = channel;
                ProbePhase::CleanupGroup {
                    group,
                    _image: image,
                }
            }
            other => other,
        };
        // Request normal cleanup at the existing owner. An error leaves debt here;
        // drain_cleanup reports its real next observation, never invents emptiness.
        if let ProbePhase::CleanupGroup { group, .. } = &mut self.phase {
            let _initial_stop = group.begin_stop(GroupStopTiming::normal(), Instant::now());
        }
    }
    /// One bounded continuation. Error/cancellation retains this exact owner for later ticks.
    pub async fn drain_cleanup(
        &mut self,
        cancel: &CancellationToken,
    ) -> Result<(), NativeProbeError> {
        self.enter_cleanup();
        loop {
            if cancel.is_cancelled() {
                return Err(NativeProbeError::Cancelled);
            }
            match &mut self.phase {
                ProbePhase::DrainingLaunch(future) => {
                    let result = tokio::select! {biased;_=cancel.cancelled()=>return Err(NativeProbeError::Cancelled),result=future=>result};
                    match result {
                        Ok(outcome) => {
                            let _launch_reason = self.accept_launch(outcome, true);
                        }
                        Err(reason) => {
                            self.phase = ProbePhase::Fenced;
                            return Err(reason.into());
                        }
                    }
                }
                ProbePhase::DrainingAttach(future) => {
                    let outcome = tokio::select! {biased;_=cancel.cancelled()=>return Err(NativeProbeError::Cancelled),outcome=future=>outcome};
                    let _attachment_reason = self.accept_attachment(outcome, true);
                }
                ProbePhase::CleanupGroup { group, .. } => {
                    if matches!(group.progress(), GroupStopProgress::Running) {
                        let requested = group.begin_stop(GroupStopTiming::normal(), Instant::now());
                        self.exit_status = group.leader_exit_status();
                        requested?;
                    }
                    let observation = group.wait_for_stop(cancel).await;
                    self.exit_status = group.leader_exit_status();
                    let status = observation?;
                    if !matches!(status, GroupStopStatus::GroupEmpty { .. })
                        || self.exit_status.is_none()
                    {
                        return Err(NativeProbeError::CleanupIncomplete);
                    }
                    self.phase = ProbePhase::Fenced;
                    return Ok(());
                }
                ProbePhase::CleanupSetup { cleanup, .. } => {
                    cleanup.wait_for_cleanup(cancel).await?;
                    self.exit_status = cleanup.leader_exit_status();
                    self.phase = ProbePhase::Fenced;
                    return Ok(());
                }
                ProbePhase::Idle | ProbePhase::Fenced => return Ok(()),
                _ => return Err(NativeProbeError::CleanupRequired),
            }
        }
    }
}
