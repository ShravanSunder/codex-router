use codex_native_integration::{
    AppServerObservation, AppServerObservationValidationError, RemoteControlObservation,
};
use serde::{Deserialize, Serialize};

use crate::native_probe_failure::NativeProbeFailure;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum RemoteControlObservationWire {
    Connected {
        server_name: String,
        environment_id: Option<String>,
    },
    Connecting {
        server_name: String,
        environment_id: Option<String>,
    },
    Errored {
        server_name: String,
        environment_id: Option<String>,
    },
    Disabled {
        server_name: String,
        environment_id: Option<String>,
    },
}

impl From<RemoteControlObservation> for RemoteControlObservationWire {
    fn from(value: RemoteControlObservation) -> Self {
        match value {
            RemoteControlObservation::Connected {
                server_name,
                environment_id,
            } => Self::Connected {
                server_name,
                environment_id,
            },
            RemoteControlObservation::Connecting {
                server_name,
                environment_id,
            } => Self::Connecting {
                server_name,
                environment_id,
            },
            RemoteControlObservation::Errored {
                server_name,
                environment_id,
            } => Self::Errored {
                server_name,
                environment_id,
            },
            RemoteControlObservation::Disabled {
                server_name,
                environment_id,
            } => Self::Disabled {
                server_name,
                environment_id,
            },
        }
    }
}

impl From<RemoteControlObservationWire> for RemoteControlObservation {
    fn from(value: RemoteControlObservationWire) -> Self {
        match value {
            RemoteControlObservationWire::Connected {
                server_name,
                environment_id,
            } => Self::Connected {
                server_name,
                environment_id,
            },
            RemoteControlObservationWire::Connecting {
                server_name,
                environment_id,
            } => Self::Connecting {
                server_name,
                environment_id,
            },
            RemoteControlObservationWire::Errored {
                server_name,
                environment_id,
            } => Self::Errored {
                server_name,
                environment_id,
            },
            RemoteControlObservationWire::Disabled {
                server_name,
                environment_id,
            } => Self::Disabled {
                server_name,
                environment_id,
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum NativeProbeResultWire {
    Observed {
        running_version: String,
        remote_control: RemoteControlObservationWire,
    },
    Failed {
        reason: NativeProbeFailure,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeProbeResult {
    Observed(AppServerObservation),
    Failed(NativeProbeFailure),
}

impl From<NativeProbeResult> for NativeProbeResultWire {
    fn from(value: NativeProbeResult) -> Self {
        match value {
            NativeProbeResult::Observed(observation) => Self::Observed {
                running_version: observation.running_version().to_owned(),
                remote_control: observation.remote_control().clone().into(),
            },
            NativeProbeResult::Failed(reason) => Self::Failed { reason },
        }
    }
}

impl TryFrom<NativeProbeResultWire> for NativeProbeResult {
    type Error = AppServerObservationValidationError;

    fn try_from(value: NativeProbeResultWire) -> Result<Self, Self::Error> {
        match value {
            NativeProbeResultWire::Observed {
                running_version,
                remote_control,
            } => Ok(Self::Observed(AppServerObservation::from_recorded_parts(
                running_version,
                remote_control.into(),
            )?)),
            NativeProbeResultWire::Failed { reason } => Ok(Self::Failed(reason)),
        }
    }
}
