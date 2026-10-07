use std::fmt;

use thiserror::Error;

use crate::remote_control_observation::RemoteControlObservation;

pub(crate) const MAX_NATIVE_PROTOCOL_EVIDENCE_BYTES: usize = 64 * 1024;

/// Field in a captured native app-server observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppServerObservationField {
    /// Version token reported by native initialize.
    RunningVersion,
    /// Raw server name reported by Remote Control.
    RemoteControlServerName,
    /// Optional environment identifier reported by Remote Control.
    RemoteControlEnvironmentId,
}

impl fmt::Display for AppServerObservationField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::RunningVersion => "running version",
            Self::RemoteControlServerName => "Remote Control server name",
            Self::RemoteControlEnvironmentId => "Remote Control environment identifier",
        })
    }
}

/// Rejected recorded native app-server observation fields.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AppServerObservationValidationError {
    /// Running version is not one non-empty token.
    #[error("running version must be one non-empty token")]
    InvalidRunningVersion,
    /// A captured field exceeds the native protocol evidence limit.
    #[error("{field} exceeds the 64 KiB native protocol evidence limit")]
    FieldTooLarge {
        /// Field that exceeded the evidence limit.
        field: AppServerObservationField,
    },
}

pub(crate) fn validate_recorded_observation(
    running_version: &str,
    remote_control: &RemoteControlObservation,
) -> Result<(), AppServerObservationValidationError> {
    let mut version_tokens = running_version.split_whitespace();
    if version_tokens.next() != Some(running_version) || version_tokens.next().is_some() {
        return Err(AppServerObservationValidationError::InvalidRunningVersion);
    }

    validate_field_size(running_version, AppServerObservationField::RunningVersion)?;

    let (server_name, environment_id) = match remote_control {
        RemoteControlObservation::Connected {
            server_name,
            environment_id,
        }
        | RemoteControlObservation::Connecting {
            server_name,
            environment_id,
        }
        | RemoteControlObservation::Errored {
            server_name,
            environment_id,
        }
        | RemoteControlObservation::Disabled {
            server_name,
            environment_id,
        } => (server_name, environment_id),
    };

    validate_field_size(
        server_name,
        AppServerObservationField::RemoteControlServerName,
    )?;
    if let Some(environment_id) = environment_id {
        validate_field_size(
            environment_id,
            AppServerObservationField::RemoteControlEnvironmentId,
        )?;
    }

    Ok(())
}

fn validate_field_size(
    value: &str,
    field: AppServerObservationField,
) -> Result<(), AppServerObservationValidationError> {
    if value.len() > MAX_NATIVE_PROTOCOL_EVIDENCE_BYTES {
        return Err(AppServerObservationValidationError::FieldTooLarge { field });
    }
    Ok(())
}
