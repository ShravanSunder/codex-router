//! Running Router executable version and bounded launch-path observations.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

const INSTALLED_VERSION_TIMEOUT: Duration = Duration::from_secs(2);

/// Router Host build compared with the executable still present at its launch path.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RouterExecutableRelation {
    /// The launch path still has its startup file identity or reports the same version.
    Match,
    /// The launch path now resolves to a different Router version or is missing.
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
    startup_identity: Option<ExecutableFileIdentity>,
    relation: RouterExecutableRelation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ExecutableFileIdentity {
    device: u64,
    inode: u64,
    size: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
}

impl RouterExecutableObserver {
    /// Captures only the startup file's stat identity; it does no file reads or hashing.
    pub fn capture(launch_path: Result<PathBuf, std::io::Error>, running_version: String) -> Self {
        let launch_path = launch_path.ok();
        let startup_identity = launch_path
            .as_deref()
            .and_then(|path| executable_file_identity(path).ok());
        Self {
            launch_path,
            running_version,
            startup_identity,
            relation: RouterExecutableRelation::Unknown {
                reason: "the Router launch path has not been observed yet".to_owned(),
            },
        }
    }

    /// Compares the launch path's stat identity and probes its version only after drift.
    pub async fn observe(&mut self) -> RouterExecutableRelation {
        let Some(path) = self.launch_path.as_deref() else {
            self.relation = RouterExecutableRelation::Unknown {
                reason: "the Router launch path is unavailable".to_owned(),
            };
            return self.relation.clone();
        };
        let current_identity = match executable_file_identity(path) {
            Ok(identity) => identity,
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
        if self.startup_identity == Some(current_identity) {
            self.relation = RouterExecutableRelation::Match;
            return self.relation.clone();
        }

        match installed_version(path).await {
            Ok(installed_version) if installed_version == self.running_version => {
                self.startup_identity = Some(current_identity);
                self.relation = RouterExecutableRelation::Match;
            }
            Ok(installed_version) => {
                self.relation = RouterExecutableRelation::Drift {
                    running_version: self.running_version.clone(),
                    installed_version: Some(installed_version),
                };
            }
            Err(()) => {
                self.relation = RouterExecutableRelation::Unknown {
                    reason: "the changed Router launch path could not report its version"
                        .to_owned(),
                };
            }
        }
        self.relation.clone()
    }

    /// Returns the latest relation without performing filesystem work.
    #[must_use]
    pub fn relation(&self) -> RouterExecutableRelation {
        self.relation.clone()
    }
}

fn executable_file_identity(path: &std::path::Path) -> std::io::Result<ExecutableFileIdentity> {
    use std::os::unix::fs::MetadataExt;

    let metadata = std::fs::metadata(path)?;
    Ok(ExecutableFileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        size: metadata.size(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
    })
}

async fn installed_version(path: &std::path::Path) -> Result<String, ()> {
    let mut command = tokio::process::Command::new(path);
    command.arg("--version").kill_on_drop(true);
    let output = tokio::time::timeout(INSTALLED_VERSION_TIMEOUT, command.output())
        .await
        .map_err(|_elapsed| ())?
        .map_err(|_error| ())?;
    if !output.status.success() {
        return Err(());
    }
    codex_native_integration::parse_executable_version(&output.stdout).map_err(|_error| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn same_file_is_a_match() {
        let path = std::env::current_exe().expect("test executable");
        let mut observer = RouterExecutableObserver::capture(Ok(path), "test-running".to_owned());
        assert_eq!(observer.observe().await, RouterExecutableRelation::Match);
    }

    #[tokio::test]
    async fn replaced_file_with_a_different_version_is_drift() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let launch_path = directory.path().join("codex-router");
        write_version_script(&launch_path, "0.1.36");
        let mut observer =
            RouterExecutableObserver::capture(Ok(launch_path.clone()), "0.1.36".to_owned());
        assert_eq!(observer.observe().await, RouterExecutableRelation::Match);
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
    async fn replacement_with_the_same_version_remains_a_match() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let launch_path = directory.path().join("codex-router");
        write_version_script(&launch_path, "0.1.36");
        let mut observer =
            RouterExecutableObserver::capture(Ok(launch_path.clone()), "0.1.36".to_owned());
        assert_eq!(observer.observe().await, RouterExecutableRelation::Match);
        write_version_script(&launch_path, "0.1.36");
        assert_eq!(observer.observe().await, RouterExecutableRelation::Match);
    }

    #[tokio::test]
    async fn missing_launch_path_is_drift() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("router");
        std::fs::write(&path, b"router").expect("write router");
        let mut observer = RouterExecutableObserver::capture(Ok(path.clone()), "0.1.36".to_owned());
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
        let mut observer = RouterExecutableObserver::capture(Ok(path.clone()), "0.1.36".to_owned());
        std::fs::write(path.join("changed"), b"changed").expect("change launch path identity");
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
            .expect("make script executable");
    }
}
