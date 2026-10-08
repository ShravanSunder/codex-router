use crate::{ImageError, OwnedProcessGroup};
use codex_router_keeper_protocol::{ChildPgid, ChildPid};
use std::{
    path::{Path, PathBuf},
    process::ExitStatus,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WarmupRejectionKind {
    BuildInfoMismatch,
    ImageUnavailable,
    BuildInfoJson,
    WarmupExit,
    WarmupTooLarge,
    WarmupTimedOut,
    Other,
}
impl WarmupRejectionKind {
    pub(crate) fn from_reason(reason: &ImageError) -> Self {
        match reason {
            ImageError::BuildInfoMismatch => Self::BuildInfoMismatch,
            ImageError::ImageUnavailable => Self::ImageUnavailable,
            ImageError::BuildInfoJson(_) => Self::BuildInfoJson,
            ImageError::WarmupExit => Self::WarmupExit,
            ImageError::WarmupTooLarge => Self::WarmupTooLarge,
            ImageError::WarmupTimedOut => Self::WarmupTimedOut,
            _ => Self::Other,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WarmupObservedStage {
    Started,
    Verified,
    Rejected(WarmupRejectionKind),
}
/// Per owning invocation only: actual semantic stage, child and stored exit.
#[derive(Clone, Debug)]
pub(crate) struct WarmupTestObservation {
    pub(crate) image_path: PathBuf,
    pub(crate) pid: ChildPid,
    pub(crate) pgid: ChildPgid,
    pub(crate) stage: WarmupObservedStage,
    pub(crate) stored_exit: Option<ExitStatus>,
}
impl WarmupTestObservation {
    pub(crate) fn started(path: &Path, group: &OwnedProcessGroup) -> Self {
        Self {
            image_path: path.to_owned(),
            pid: group.leader_pid(),
            pgid: group.process_group_id(),
            stage: WarmupObservedStage::Started,
            stored_exit: group.leader_exit_status(),
        }
    }
    pub(crate) fn note_exit(&mut self, group: &OwnedProcessGroup) {
        self.stored_exit = group.leader_exit_status();
    }
}
