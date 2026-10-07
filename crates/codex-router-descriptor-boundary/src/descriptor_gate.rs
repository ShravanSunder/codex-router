//! Only nonblocking creation/receipt and synchronous spawn hold this process-wide gate.
use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
static PROCESS_GATE: DescriptorGate = DescriptorGate {
    lock: RwLock::const_new(()),
};
pub struct DescriptorGate {
    lock: RwLock<()>,
}
impl DescriptorGate {
    #[must_use]
    pub fn global() -> &'static Self {
        &PROCESS_GATE
    }
    pub async fn creation(&self) -> RwLockReadGuard<'_, ()> {
        self.lock.read().await
    }
    pub async fn spawn(&self) -> RwLockWriteGuard<'_, ()> {
        self.lock.write().await
    }
    pub async fn duplicate(
        &self,
        descriptor: std::os::fd::BorrowedFd<'_>,
    ) -> Result<std::os::fd::OwnedFd, crate::BoundaryError> {
        let _shared = self.creation().await;
        let owned = rustix::io::dup(descriptor).map_err(crate::boundary_error::io_error)?;
        rustix::io::fcntl_setfd(&owned, rustix::io::FdFlags::CLOEXEC)
            .map_err(crate::boundary_error::io_error)?;
        Ok(owned)
    }
    /// Synchronous SDK/std launch sites acquire exclusively on bounded blocking work.
    /// The operation must contain only the spawn call; initialization and wait belong outside it.
    pub async fn spawn_blocking<TOutput>(
        &'static self,
        operation: impl FnOnce() -> Result<TOutput, crate::BoundaryError> + Send + 'static,
    ) -> Result<TOutput, crate::BoundaryError>
    where
        TOutput: Send + 'static,
    {
        tokio::task::spawn_blocking(move || {
            let _exclusive = self.lock.blocking_write();
            operation()
        })
        .await
        .map_err(|_| crate::BoundaryError::WaitTask)?
    }
    /// The caller supplies a synchronous spawn, never initialization/IO awaits.
    pub async fn spawn_child(
        &self,
        command: &mut tokio::process::Command,
    ) -> Result<tokio::process::Child, crate::BoundaryError> {
        let _exclusive = self.spawn().await;
        command.spawn().map_err(crate::BoundaryError::Io)
    }
}
