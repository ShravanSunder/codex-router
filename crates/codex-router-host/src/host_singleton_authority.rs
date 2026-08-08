//! Exclusive host singleton authority and owner-only operator socket binding.

use std::ffi::OsStr;
use std::fs::File;
use std::fs::OpenOptions;
use std::fs::Permissions;
use std::fs::TryLockError;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use thiserror::Error;
use tokio::net::UnixListener;

use crate::HostCoordinationPaths;

/// Exclusive host instance and its private operator listener.
pub struct HostInstance {
    listener: UnixListener,
    socket_path: PathBuf,
    lock_file: File,
}

impl HostInstance {
    /// Acquires singleton authority before removing or binding the socket.
    pub fn acquire(paths: HostCoordinationPaths) -> Result<Self, InstanceAcquireError> {
        let lock_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .mode(0o600)
            .open(paths.instance_lock())
            .map_err(InstanceAcquireError::OpenLock)?;
        match lock_file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(InstanceAcquireError::AlreadyRunning),
            Err(TryLockError::Error(error)) => {
                return Err(InstanceAcquireError::AcquireLock(error));
            }
        }
        lock_file
            .set_permissions(Permissions::from_mode(0o600))
            .map_err(InstanceAcquireError::SetLockPermissions)?;

        Self::bind_with_lock(paths, lock_file)
    }

    /// Consumes the continuously held descriptor inherited through stdin.
    pub fn acquire_inherited(
        paths: HostCoordinationPaths,
        marker: &OsStr,
    ) -> Result<Self, InstanceAcquireError> {
        if marker != OsStr::new(inherited_lock_marker()) {
            return Err(InstanceAcquireError::InheritedMarkerMismatch);
        }
        let inherited = rustix::stdio::stdin();
        let inherited_stat =
            rustix::fs::fstat(inherited).map_err(InstanceAcquireError::InspectInheritedLock)?;
        let artifact_stat = rustix::fs::stat(paths.instance_lock())
            .map_err(InstanceAcquireError::InspectLockArtifact)?;
        if inherited_stat.st_dev != artifact_stat.st_dev
            || inherited_stat.st_ino != artifact_stat.st_ino
        {
            return Err(InstanceAcquireError::InheritedLockMismatch);
        }

        let authority =
            rustix::io::dup(inherited).map_err(InstanceAcquireError::DuplicateInheritedLock)?;
        rustix::io::fcntl_setfd(&authority, rustix::io::FdFlags::CLOEXEC)
            .map_err(InstanceAcquireError::RestoreCloseOnExec)?;
        rustix::io::fcntl_setfd(inherited, rustix::io::FdFlags::CLOEXEC)
            .map_err(InstanceAcquireError::RestoreCloseOnExec)?;
        let lock_file = File::from(authority);

        Self::bind_with_lock(paths, lock_file)
    }

    /// Places singleton authority on stdin for the one changed-update exec.
    pub fn prepare_lock_for_exec(&self) -> Result<(), InstanceAcquireError> {
        rustix::stdio::dup2_stdin(&self.lock_file)
            .map_err(InstanceAcquireError::PrepareInheritedLock)?;
        rustix::io::fcntl_setfd(rustix::stdio::stdin(), rustix::io::FdFlags::empty())
            .map_err(InstanceAcquireError::PrepareInheritedLock)
    }

    /// Replaces the prepared stdin duplicate when foreground exec returns.
    pub fn release_prepared_lock_after_exec_failure(&self) -> Result<(), InstanceAcquireError> {
        let null_input = File::open("/dev/null")
            .map_err(InstanceAcquireError::ReleasePreparedLockAfterExecFailure)?;
        rustix::stdio::dup2_stdin(&null_input).map_err(|error| {
            InstanceAcquireError::ReleasePreparedLockAfterExecFailure(error.into())
        })
    }

    /// Removes the published pathname immediately before same-process exec.
    pub fn remove_operator_socket_for_exec(&self) -> Result<(), InstanceAcquireError> {
        match std::fs::remove_file(&self.socket_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(InstanceAcquireError::RemoveOperatorSocketForExec(error)),
        }
    }

    fn bind_with_lock(
        paths: HostCoordinationPaths,
        lock_file: File,
    ) -> Result<Self, InstanceAcquireError> {
        match std::fs::remove_file(paths.operator_socket()) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(InstanceAcquireError::RemoveStaleSocket(error)),
        }
        let listener = UnixListener::bind(paths.operator_socket())
            .map_err(InstanceAcquireError::BindOperatorSocket)?;
        std::fs::set_permissions(paths.operator_socket(), Permissions::from_mode(0o600))
            .map_err(InstanceAcquireError::SetSocketPermissions)?;

        Ok(Self {
            listener,
            socket_path: paths.operator_socket().to_owned(),
            lock_file,
        })
    }

    /// Returns the owner-only Tokio listener.
    #[must_use]
    pub const fn listener(&self) -> &UnixListener {
        &self.listener
    }
}

/// Same-version private marker passed only by changed-update re-exec.
#[must_use]
pub const fn inherited_lock_marker() -> &'static str {
    concat!("codex-router-host/", env!("CARGO_PKG_VERSION"))
}

/// Private environment key carrying the same-version inherited-lock marker.
#[must_use]
pub const fn inherited_lock_environment() -> &'static str {
    "CODEX_ROUTER_HOST_INHERITED_LOCK"
}

impl Drop for HostInstance {
    fn drop(&mut self) {
        let _remove_result = std::fs::remove_file(&self.socket_path);
    }
}

/// Singleton authority or socket-publication failure.
#[derive(Debug, Error)]
pub enum InstanceAcquireError {
    /// Another process currently owns the stable lock artifact.
    #[error("shared Codex host is already running")]
    AlreadyRunning,
    /// Stable lock artifact could not be opened.
    #[error("failed opening host instance lock: {0}")]
    OpenLock(#[source] std::io::Error),
    /// Exclusive nonblocking lock operation failed.
    #[error("failed acquiring host instance lock: {0}")]
    AcquireLock(#[source] std::io::Error),
    /// Owner-private lock permissions could not be enforced.
    #[error("failed setting host instance-lock permissions: {0}")]
    SetLockPermissions(#[source] std::io::Error),
    /// The lock owner could not remove a stale socket pathname.
    #[error("failed removing stale operator socket: {0}")]
    RemoveStaleSocket(#[source] std::io::Error),
    /// The lock owner could not bind the operator socket.
    #[error("failed binding operator socket: {0}")]
    BindOperatorSocket(#[source] std::io::Error),
    /// Owner-only socket permissions could not be enforced.
    #[error("failed setting operator-socket permissions: {0}")]
    SetSocketPermissions(#[source] std::io::Error),
    /// Replacement bootstrap did not carry this binary's private marker.
    #[error("inherited host lock marker does not match this binary")]
    InheritedMarkerMismatch,
    /// Inherited stdin metadata could not be inspected.
    #[error("failed inspecting inherited host lock: {0}")]
    InspectInheritedLock(#[source] rustix::io::Errno),
    /// Configured stable artifact metadata could not be inspected.
    #[error("failed inspecting configured host lock artifact: {0}")]
    InspectLockArtifact(#[source] rustix::io::Errno),
    /// Inherited stdin did not refer to the configured stable artifact.
    #[error("inherited host lock does not match the configured artifact")]
    InheritedLockMismatch,
    /// Continuously held inherited descriptor could not be duplicated.
    #[error("failed retaining inherited host lock: {0}")]
    DuplicateInheritedLock(#[source] rustix::io::Errno),
    /// Close-on-exec could not be restored before child spawning.
    #[error("failed restoring close-on-exec for host authority: {0}")]
    RestoreCloseOnExec(#[source] rustix::io::Errno),
    /// Singleton authority could not be placed on stdin for re-exec.
    #[error("failed preparing inherited host lock: {0}")]
    PrepareInheritedLock(#[source] rustix::io::Errno),
    /// A failed exec left singleton authority duplicated on stdin.
    #[error("failed releasing prepared host lock after exec failure: {0}")]
    ReleasePreparedLockAfterExecFailure(#[source] std::io::Error),
    /// The changed-update path could not unpublish the old operator socket.
    #[error("failed removing operator socket before host replacement: {0}")]
    RemoveOperatorSocketForExec(#[source] std::io::Error),
}
