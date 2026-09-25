//! Running Router executable identity and bounded installed-file observations.

use std::path::PathBuf;
use std::time::SystemTime;

use codex_native_integration::ExecutableIdentity;
use serde::{Deserialize, Serialize};

/// Router Host build compared with the executable still present at its launch path.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RouterExecutableRelation {
    /// The launch path still resolves to the exact content captured at startup.
    Match,
    /// The launch path now resolves to different executable content or is missing.
    Drift {
        /// Version of the executable captured when this Host started.
        running_version: String,
        /// Version currently reported at the original launch path, if readable.
        installed_version: Option<String>,
    },
    /// The launch path could not be compared reliably.
    Unknown {
        /// Safe, actionable-free reason for the failed observation.
        reason: String,
    },
}

/// Shared observer retained by the Host and refreshed on status or periodic checks.
pub struct RouterExecutableObserver {
    launch_path: Option<PathBuf>,
    running_version: String,
    running_identity: Option<ExecutableIdentity>,
    observed_stamp: Option<ExecutableFileStamp>,
    relation: RouterExecutableRelation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ExecutableFileStamp {
    length: u64,
    modified: Option<SystemTime>,
}

impl RouterExecutableObserver {
    /// Captures the Host's startup executable identity and launch-path metadata.
    pub async fn capture(
        launch_path: Result<PathBuf, std::io::Error>,
        running_version: String,
    ) -> Self {
        let launch_path = launch_path.ok();
        let running_identity = match launch_path.as_deref() {
            Some(path) => codex_native_integration::executable_identity(path)
                .await
                .ok(),
            None => None,
        };
        let observed_stamp = launch_path
            .as_deref()
            .and_then(|path| executable_file_stamp(path).ok());
        let relation = if running_identity.is_some() && observed_stamp.is_some() {
            RouterExecutableRelation::Match
        } else {
            RouterExecutableRelation::Unknown {
                reason: "the Router executable identity could not be captured at startup"
                    .to_owned(),
            }
        };
        Self {
            launch_path,
            running_version,
            running_identity,
            observed_stamp,
            relation,
        }
    }

    /// Rechecks the launch path and hashes only after its size or modification time changes.
    pub async fn observe(&mut self) -> RouterExecutableRelation {
        let Some(path) = self.launch_path.as_deref() else {
            self.relation = RouterExecutableRelation::Unknown {
                reason: "the Router launch path is unavailable".to_owned(),
            };
            return self.relation.clone();
        };
        let current_stamp = match executable_file_stamp(path) {
            Ok(stamp) => stamp,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.relation = RouterExecutableRelation::Drift {
                    running_version: self.running_version.clone(),
                    installed_version: None,
                };
                return self.relation.clone();
            }
            Err(_error) => {
                self.relation = RouterExecutableRelation::Unknown {
                    reason: "the Router launch path could not be inspected".to_owned(),
                };
                return self.relation.clone();
            }
        };
        if self.observed_stamp == Some(current_stamp) {
            return self.relation.clone();
        }
        self.observed_stamp = Some(current_stamp);
        let (installed_identity, installed_version) = tokio::join!(
            codex_native_integration::executable_identity(path),
            codex_native_integration::executable_version(path),
        );
        let (Ok(installed_identity), installed_version) = (installed_identity, installed_version)
        else {
            self.relation = RouterExecutableRelation::Unknown {
                reason: "the Router launch path changed but its identity could not be read"
                    .to_owned(),
            };
            return self.relation.clone();
        };
        if self.running_identity.as_ref() == Some(&installed_identity) {
            self.relation = RouterExecutableRelation::Match;
        } else {
            self.relation = RouterExecutableRelation::Drift {
                running_version: self.running_version.clone(),
                installed_version: installed_version.ok(),
            };
        }
        self.relation.clone()
    }
}

fn executable_file_stamp(path: &std::path::Path) -> std::io::Result<ExecutableFileStamp> {
    let metadata = std::fs::metadata(path)?;
    Ok(ExecutableFileStamp {
        length: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn same_file_remains_a_match() {
        let path = std::env::current_exe().expect("test executable");
        let mut observer =
            RouterExecutableObserver::capture(Ok(path), "test-running".to_owned()).await;
        assert_eq!(observer.observe().await, RouterExecutableRelation::Match);
    }

    #[tokio::test]
    async fn replaced_file_with_a_different_version_is_drift() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let launch_path = directory.path().join("codex-router");
        write_version_script(&launch_path, "0.1.36");
        let mut observer =
            RouterExecutableObserver::capture(Ok(launch_path.clone()), "0.1.36".to_owned()).await;
        write_version_script(&launch_path, "0.1.37");
        assert_eq!(
            observer.observe().await,
            RouterExecutableRelation::Drift {
                running_version: "0.1.36".to_owned(),
                installed_version: Some("0.1.37".to_owned())
            }
        );
    }

    #[tokio::test]
    async fn missing_launch_path_is_drift() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("router");
        std::fs::write(&path, b"router").expect("write router");
        let mut observer =
            RouterExecutableObserver::capture(Ok(path.clone()), "0.1.36".to_owned()).await;
        std::fs::remove_file(path).expect("remove launch file");
        assert_eq!(
            observer.observe().await,
            RouterExecutableRelation::Drift {
                running_version: "0.1.36".to_owned(),
                installed_version: None
            }
        );
    }

    #[tokio::test]
    async fn unreadable_launch_path_is_unknown() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("router");
        std::fs::create_dir(&path).expect("create unreadable executable directory");
        let mut observer =
            RouterExecutableObserver::capture(Ok(path.clone()), "0.1.36".to_owned()).await;
        std::fs::write(path.join("changed"), b"changed").expect("change launch path stamp");
        assert!(matches!(
            observer.observe().await,
            RouterExecutableRelation::Unknown { .. }
        ));
    }

    fn write_version_script(path: &std::path::Path, version: &str) {
        std::fs::write(
            path,
            format!("#!/bin/sh\nprintf 'codex-router {version}\\n'\n"),
        )
        .expect("write version script");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .expect("make version script executable");
    }
}
