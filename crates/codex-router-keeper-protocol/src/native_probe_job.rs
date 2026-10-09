use std::time::Duration;

use codex_native_integration::AppServerProbeAction;
use serde::{Deserialize, Serialize};

use crate::GenerationAliasPath;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum NativeProbeJobWire {
    Observe {
        alias: GenerationAliasPath,
        native_wait_ms: u64,
        remote_wait_ms: u64,
    },
    WaitForReady {
        alias: GenerationAliasPath,
        native_wait_ms: u64,
        remote_wait_ms: u64,
    },
    EnableAndObserve {
        alias: GenerationAliasPath,
        native_wait_ms: u64,
        remote_wait_ms: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeProbeJob {
    action: AppServerProbeAction,
    alias: GenerationAliasPath,
    native_readiness_wait: Duration,
    remote_control_wait: Duration,
}

impl NativeProbeJob {
    pub fn new(
        action: AppServerProbeAction,
        alias: GenerationAliasPath,
        native_readiness_wait: Duration,
        remote_control_wait: Duration,
    ) -> Self {
        Self {
            action,
            alias,
            native_readiness_wait,
            remote_control_wait,
        }
    }

    pub const fn action(&self) -> AppServerProbeAction {
        self.action
    }

    pub fn alias(&self) -> &GenerationAliasPath {
        &self.alias
    }

    pub const fn native_readiness_wait(&self) -> Duration {
        self.native_readiness_wait
    }

    pub const fn remote_control_wait(&self) -> Duration {
        self.remote_control_wait
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum NativeProbeJobConversionError {
    #[error("native readiness wait is not exactly representable as u64 milliseconds")]
    NativeReadinessWaitNotRepresentable,
    #[error("Remote Control wait is not exactly representable as u64 milliseconds")]
    RemoteControlWaitNotRepresentable,
}

impl From<NativeProbeJobWire> for NativeProbeJob {
    fn from(value: NativeProbeJobWire) -> Self {
        let (action, alias, native_wait_ms, remote_wait_ms) = match value {
            NativeProbeJobWire::Observe {
                alias,
                native_wait_ms,
                remote_wait_ms,
            } => (
                AppServerProbeAction::Observe,
                alias,
                native_wait_ms,
                remote_wait_ms,
            ),
            NativeProbeJobWire::WaitForReady {
                alias,
                native_wait_ms,
                remote_wait_ms,
            } => (
                AppServerProbeAction::WaitForReady,
                alias,
                native_wait_ms,
                remote_wait_ms,
            ),
            NativeProbeJobWire::EnableAndObserve {
                alias,
                native_wait_ms,
                remote_wait_ms,
            } => (
                AppServerProbeAction::EnableAndObserve,
                alias,
                native_wait_ms,
                remote_wait_ms,
            ),
        };
        Self::new(
            action,
            alias,
            Duration::from_millis(native_wait_ms),
            Duration::from_millis(remote_wait_ms),
        )
    }
}

impl TryFrom<NativeProbeJob> for NativeProbeJobWire {
    type Error = NativeProbeJobConversionError;

    fn try_from(value: NativeProbeJob) -> Result<Self, Self::Error> {
        let native_wait_ms = duration_millis_exact(
            value.native_readiness_wait,
            NativeProbeJobConversionError::NativeReadinessWaitNotRepresentable,
        )?;
        let remote_wait_ms = duration_millis_exact(
            value.remote_control_wait,
            NativeProbeJobConversionError::RemoteControlWaitNotRepresentable,
        )?;
        let wire = match value.action {
            AppServerProbeAction::Observe => Self::Observe {
                alias: value.alias,
                native_wait_ms,
                remote_wait_ms,
            },
            AppServerProbeAction::WaitForReady => Self::WaitForReady {
                alias: value.alias,
                native_wait_ms,
                remote_wait_ms,
            },
            AppServerProbeAction::EnableAndObserve => Self::EnableAndObserve {
                alias: value.alias,
                native_wait_ms,
                remote_wait_ms,
            },
        };
        Ok(wire)
    }
}

fn duration_millis_exact(
    duration: Duration,
    error: NativeProbeJobConversionError,
) -> Result<u64, NativeProbeJobConversionError> {
    if !duration.subsec_nanos().is_multiple_of(1_000_000) {
        return Err(error);
    }
    u64::try_from(duration.as_millis()).map_err(|_| error)
}
