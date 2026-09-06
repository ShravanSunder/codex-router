//! Retained app-server child, spawn identity, and bounded readiness.

use std::path::Path;
use std::time::Duration;

use codex_native_integration::ExecutableIdentity;
use codex_native_integration::RemoteControlObservation;
use codex_native_integration::observe_app_server;
use thiserror::Error;

use crate::ChildCommandSpec;
use crate::ExpectedExit;
use crate::ProcessGroupChild;
use crate::ProcessGroupError;

/// Native endpoint and Remote Control readiness observed after spawn.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppServerReadiness {
    /// Native initialize and Remote Control both converged.
    Ready {
        /// Version reported by native initialize.
        running_version: String,
    },
    /// Native initialize converged while Remote Control remained degraded.
    LocalReadyRemoteDegraded {
        /// Version reported by native initialize.
        running_version: String,
        /// Low-cardinality upstream Remote Control condition.
        remote_control: crate::RemoteControlCondition,
    },
}

/// Retained app-server child and its spawn identity.
pub struct AppServerChild {
    pub(crate) process: ProcessGroupChild,
    identity: ExecutableIdentity,
    pub(crate) expected_exit: Option<ExpectedExit>,
    expected_version: Option<String>,
    schema_export: Option<std::sync::Arc<codex_native_integration::NativeSchemaExport>>,
}

/// Cloneable managed app-server launch inputs retained for one recovery attempt.
#[derive(Clone)]
pub struct AppServerLaunchPlan {
    command: ChildCommandSpec,
    identity: ExecutableIdentity,
    expected_version: String,
    schema_directory: Option<std::path::PathBuf>,
    schema_export: Option<std::sync::Arc<codex_native_integration::NativeSchemaExport>>,
}

impl AppServerLaunchPlan {
    /// Captures one exact managed executable, argv/environment, and version.
    #[must_use]
    pub const fn new(
        command: ChildCommandSpec,
        identity: ExecutableIdentity,
        expected_version: String,
    ) -> Self {
        Self {
            command,
            identity,
            expected_version,
            schema_directory: None,
            schema_export: None,
        }
    }

    #[must_use]
    pub fn with_schema_directory(mut self, directory: std::path::PathBuf) -> Self {
        self.schema_directory = Some(directory);
        self
    }

    pub async fn prepare_schema(&mut self) {
        let Some(directory) = &self.schema_directory else {
            return;
        };
        if self
            .schema_export
            .as_ref()
            .is_some_and(|export| export.executable() == &self.identity)
        {
            return;
        }
        self.schema_export = None;
        let prepared = async {
            use std::os::unix::fs::DirBuilderExt;
            match std::fs::DirBuilder::new().mode(0o700).create(directory) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
            use std::os::unix::fs::PermissionsExt;
            let metadata = std::fs::symlink_metadata(directory)?;
            if !directory.is_absolute()
                || !metadata.is_dir()
                || metadata.permissions().mode() & 0o077 != 0
            {
                return Err(std::io::Error::other(
                    "schema export directory must be private and absolute",
                ));
            }
            let suffix = String::from(communication_service::new_service_uuid()?);
            let output = directory.join(format!("native-schema-export-{suffix}"));
            codex_native_integration::NativeSchemaExport::generate(&self.identity, &output)
                .await
                .map_err(std::io::Error::other)
        }
        .await;
        match prepared {
            Ok(export) => self.schema_export = Some(std::sync::Arc::new(export)),
            Err(_) => {
                tracing::warn!("native schema export unavailable; retaining raw native access")
            }
        }
    }

    pub(crate) fn spawn(&self) -> Result<AppServerChild, ProcessGroupError> {
        let mut command = self
            .command
            .command_for_executable(self.identity.canonical_path());
        let process = if self.command.captures_stderr_telemetry() {
            ProcessGroupChild::spawn_with_stderr_telemetry(&mut command, "app_server")?
        } else {
            ProcessGroupChild::spawn(&mut command)?
        };
        Ok(AppServerChild {
            process,
            identity: self.identity.clone(),
            expected_exit: None,
            expected_version: Some(self.expected_version.clone()),
            schema_export: self.schema_export.clone(),
        })
    }

    /// Re-resolves the installed executable identity and version before a later spawn.
    pub(crate) async fn refreshed(
        &self,
        managed_executable: &Path,
    ) -> Result<Self, codex_native_integration::ExecutableIdentityError> {
        let identity = codex_native_integration::executable_identity(managed_executable).await?;
        let expected_version =
            codex_native_integration::managed_executable_version(managed_executable).await?;
        let mut refreshed = Self::new(self.command.clone(), identity, expected_version);
        refreshed.schema_directory = self.schema_directory.clone();
        refreshed.schema_export = self.schema_export.clone();
        refreshed.prepare_schema().await;
        Ok(refreshed)
    }
}

impl AppServerChild {
    /// Creates ownership from an already-spawned isolated child.
    #[must_use]
    pub const fn new(process: ProcessGroupChild, identity: ExecutableIdentity) -> Self {
        Self {
            process,
            identity,
            expected_exit: None,
            expected_version: None,
            schema_export: None,
        }
    }

    /// Spawns one managed app-server child in its isolated process group.
    pub fn spawn(
        command: &mut tokio::process::Command,
        identity: ExecutableIdentity,
        expected_version: String,
    ) -> Result<Self, ProcessGroupError> {
        Ok(Self {
            process: ProcessGroupChild::spawn(command)?,
            identity,
            expected_exit: None,
            expected_version: Some(expected_version),
            schema_export: None,
        })
    }

    /// Returns the executable content identity recorded at spawn.
    #[must_use]
    pub const fn identity(&self) -> &ExecutableIdentity {
        &self.identity
    }
    pub(crate) fn schema_export(
        &self,
    ) -> Option<std::sync::Arc<codex_native_integration::NativeSchemaExport>> {
        self.schema_export.clone()
    }

    /// Awaits bounded native readiness while retaining child ownership on failure.
    pub async fn await_readiness(
        &mut self,
        socket_path: &Path,
        startup_deadline: Duration,
        remote_control_deadline: Duration,
    ) -> Result<AppServerReadiness, AppServerReadinessError> {
        let started_at = tokio::time::Instant::now();
        loop {
            if self.process.try_wait()?.is_some() {
                return Err(AppServerReadinessError::ChildExited);
            }
            let Some(remaining) = startup_deadline.checked_sub(started_at.elapsed()) else {
                return Err(AppServerReadinessError::StartupTimeout);
            };
            let observation =
                observe_app_server(socket_path, remaining, remote_control_deadline).await;
            match observation {
                Ok(observation) => {
                    if self.expected_version.as_deref() != Some(observation.running_version()) {
                        return Err(AppServerReadinessError::VersionMismatch);
                    }
                    let running_version = observation.running_version().to_owned();
                    return Ok(match observation.remote_control() {
                        RemoteControlObservation::Connected { .. } => {
                            AppServerReadiness::Ready { running_version }
                        }
                        RemoteControlObservation::Connecting { .. } => {
                            AppServerReadiness::LocalReadyRemoteDegraded {
                                running_version,
                                remote_control: crate::RemoteControlCondition::Connecting,
                            }
                        }
                        RemoteControlObservation::Errored { .. } => {
                            AppServerReadiness::LocalReadyRemoteDegraded {
                                running_version,
                                remote_control: crate::RemoteControlCondition::Errored,
                            }
                        }
                        RemoteControlObservation::Disabled { .. } => {
                            AppServerReadiness::LocalReadyRemoteDegraded {
                                running_version,
                                remote_control: crate::RemoteControlCondition::Disabled,
                            }
                        }
                    });
                }
                Err(codex_native_integration::CodexProtocolError::Connect(_))
                | Err(codex_native_integration::CodexProtocolError::Timeout { stage: "connect" }) =>
                {
                    tokio::time::sleep(Duration::from_millis(20).min(remaining)).await;
                }
                Err(codex_native_integration::CodexProtocolError::Timeout {
                    stage: "native readiness",
                }) => return Err(AppServerReadinessError::StartupTimeout),
                Err(error) => return Err(AppServerReadinessError::Protocol(error)),
            }
        }
    }
}

/// Spawned child failed to reach the pinned native readiness contract.
#[derive(Debug, Error)]
pub enum AppServerReadinessError {
    /// Retained child process observation failed.
    #[error(transparent)]
    Process(#[from] ProcessGroupError),
    /// Child exited before native readiness.
    #[error("managed app-server exited before native readiness")]
    ChildExited,
    /// Startup did not converge within the caller-owned bound.
    #[error("managed app-server native readiness timed out")]
    StartupTimeout,
    /// Native initialize version did not match the version captured at spawn.
    #[error("managed app-server reported an unexpected running version")]
    VersionMismatch,
    /// Reachable endpoint violated the pinned native protocol.
    #[error("managed app-server native protocol failed: {0}")]
    Protocol(#[source] codex_native_integration::CodexProtocolError),
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[tokio::test]
    async fn spawn_uses_captured_executable_after_managed_alias_moves()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::{DirBuilderExt, symlink};
        let root = std::env::temp_dir().join(format!("captured-executable-{}", std::process::id()));
        std::fs::DirBuilder::new().mode(0o700).create(&root)?;
        let original = root.join("original-codex");
        let replacement = root.join("replacement-codex");
        let alias = root.join("managed-codex");
        let marker = root.join("spawn-marker");
        std::fs::write(
            &original,
            "#!/bin/sh\nprintf original > \"$SCHEMA_PROOF_MARKER\"\n",
        )?;
        std::fs::write(
            &replacement,
            "#!/bin/sh\nprintf replacement > \"$SCHEMA_PROOF_MARKER\"\n",
        )?;
        for path in [&original, &replacement] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
        }
        symlink(&original, &alias)?;
        let identity = codex_native_integration::executable_identity(&alias).await?;
        let plan = AppServerLaunchPlan::new(
            ChildCommandSpec::new(alias.clone())
                .with_environment("SCHEMA_PROOF_MARKER", marker.as_os_str()),
            identity,
            "fixture".to_owned(),
        );
        std::fs::remove_file(&alias)?;
        symlink(&replacement, &alias)?;
        let mut child = plan.spawn()?;
        let status = child.process.wait().await?;
        let observed = std::fs::read_to_string(&marker)?;
        for path in [&marker, &alias, &original, &replacement] {
            std::fs::remove_file(path)?;
        }
        std::fs::remove_dir(root)?;
        if !status.success() || observed != "original" {
            return Err(
                "spawn followed a retargeted alias instead of captured executable identity".into(),
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn later_launch_refreshes_installed_identity_and_version()
    -> Result<(), Box<dyn std::error::Error>> {
        let executable =
            std::env::temp_dir().join(format!("codex-router-refresh-plan-{}", std::process::id()));
        std::fs::write(&executable, "#!/bin/sh\necho 'codex-cli 1.2.3'\n")?;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))?;
        let original_identity = codex_native_integration::executable_identity(&executable).await?;
        let launch_plan = AppServerLaunchPlan::new(
            ChildCommandSpec::new(executable.clone()),
            original_identity.clone(),
            "1.2.3".to_owned(),
        );
        std::fs::write(&executable, "#!/bin/sh\necho 'codex-cli 2.0.0'\n")?;

        let refreshed = launch_plan.refreshed(&executable).await?;

        let _cleanup_result = std::fs::remove_file(&executable);
        if refreshed.identity == original_identity {
            return Err("refreshed launch plan retained the previous executable identity".into());
        }
        if refreshed.expected_version != "2.0.0" {
            return Err("refreshed launch plan retained the previous executable version".into());
        }
        Ok(())
    }
}
